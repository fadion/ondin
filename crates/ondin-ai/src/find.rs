//! Finding a harness's program, resolving what is really started behind an
//! npm or launcher shim, and the one `--help` probe that grades its seal.

use crate::harness::{Harness, Probe, Seal, seal_sentence};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Mutex, OnceLock};
use std::time::Duration;

/// A harness's program, found and probed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Found {
    pub path: PathBuf,
    pub seal: Seal,
    /// What the seal means, for Settings and the panel's head.
    pub seal_sentence: String,
}

/// A harness's program, not found anywhere Ondin looked.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NotFound {
    pub program: &'static str,
    pub looked_in: Vec<PathBuf>,
}

/// Find and probe a harness's program. Lookup order: `program_override` (a
/// Settings path, which wins outright), `ONDIN_<HARNESS>_BIN`, the harness's
/// known install folders (often off the inherited `PATH`), then `PATH` with
/// `PATHEXT`, batch shims last. Probes `--help` once per path; a probe that
/// could not run is not cached.
pub fn find(h: Harness, program_override: Option<&Path>) -> Result<Found, NotFound> {
    let path = locate(h, program_override)?;
    let probe = probe(h, &path);
    Ok(Found {
        seal: probe.seal,
        seal_sentence: seal_sentence(h, probe.seal),
        path,
    })
}

/// [`find`] on a background thread, so the first session finds its probe done.
pub fn warm(h: Harness, program_override: Option<PathBuf>) {
    std::thread::spawn(move || {
        let _ = find(h, program_override.as_deref());
    });
}

/// The path of a harness's program, without probing it.
pub(crate) fn locate(h: Harness, program_override: Option<&Path>) -> Result<PathBuf, NotFound> {
    let mut looked_in = Vec::new();
    if let Some(p) = program_override.filter(|p| !p.as_os_str().is_empty()) {
        looked_in.push(p.to_path_buf());
        return resolve_override(p).ok_or(NotFound {
            program: h.program(),
            looked_in,
        });
    }
    if let Some(p) = std::env::var_os(env_var(h)).filter(|p| !p.is_empty()) {
        let p = PathBuf::from(p);
        if p.is_file() {
            return Ok(p);
        }
        looked_in.push(p);
    }
    for cand in known_locations(h) {
        if can_start(&cand) {
            return Ok(cand);
        }
        looked_in.push(cand);
    }
    let (hit, dirs) = which_on_path(h.program());
    looked_in.extend(dirs);
    hit.ok_or(NotFound {
        program: h.program(),
        looked_in,
    })
}

/// `ONDIN_CLAUDE_BIN`, `ONDIN_CODEX_BIN`, …
fn env_var(h: Harness) -> String {
    format!("ONDIN_{}_BIN", h.key().to_ascii_uppercase())
}

/// The person's home, as Node and each CLI resolve it.
pub(crate) fn home() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    match cfg!(windows) {
        true => var("USERPROFILE").or_else(|| var("HOME")),
        false => var("HOME"),
    }
    .map(PathBuf::from)
}

/// Where each installer puts the program. npm-installed harnesses list the
/// real executable ahead of the `.cmd` shim `PATH` would find.
fn known_locations(h: Harness) -> Vec<PathBuf> {
    let exe = |name: &str| match cfg!(windows) {
        true => format!("{name}.exe"),
        false => name.to_string(),
    };
    let home = home();
    let var = |k: &str| {
        std::env::var_os(k)
            .filter(|v| !v.is_empty())
            .map(PathBuf::from)
    };
    let local = var("LOCALAPPDATA");
    let appdata = var("APPDATA");
    let mut out = Vec::new();
    match h {
        Harness::Claude => {
            out.extend(home.map(|h| h.join(".local").join("bin").join(exe("claude"))))
        }
        Harness::Codex => {
            out.extend(local.map(|l| {
                l.join("Programs")
                    .join("OpenAI")
                    .join("Codex")
                    .join("bin")
                    .join(exe("codex"))
            }));
            out.extend(home.map(|h| h.join(".codex").join("bin").join(exe("codex"))));
        }
        Harness::Antigravity => {
            out.extend(local.map(|l| l.join("agy").join("bin").join(exe("agy"))));
            out.extend(home.map(|h| h.join(".local").join("bin").join(exe("agy"))));
        }
        Harness::OpenCode => {
            out.extend(appdata.map(|d| {
                d.join("npm")
                    .join("node_modules")
                    .join("opencode-ai")
                    .join("bin")
                    .join(exe("opencode"))
            }));
            if let Some(h) = home {
                out.push(h.join(".opencode").join("bin").join(exe("opencode")));
                out.push(h.join(".local").join("bin").join(exe("opencode")));
                out.push(
                    h.join(".npm-global")
                        .join("lib")
                        .join("node_modules")
                        .join("opencode-ai")
                        .join("bin")
                        .join(exe("opencode")),
                );
            }
        }
        Harness::Copilot => {
            if cfg!(windows) {
                out.extend(appdata.map(|d| copilot_native_path(&d.join("npm"))));
            }
            out.extend(home.map(|h| h.join(".local").join("bin").join(exe("copilot"))));
        }
        // The `.cmd` is what is found; `launch` starts the node behind it.
        Harness::Cursor => match cfg!(windows) {
            true => out.extend(local.map(|l| l.join("cursor-agent").join("cursor-agent.cmd"))),
            false => out.extend(home.map(|h| h.join(".local").join("bin").join("cursor-agent"))),
        },
        // Inside the desktop app's install; `launch` starts the executable.
        Harness::DeepSeek => {
            if cfg!(windows) {
                out.extend(local.map(|l| {
                    l.join("Programs")
                        .join("DeepSeek Harness")
                        .join("resources")
                        .join("runtime")
                        .join("cli")
                        .join("bin")
                        .join("dsh.cmd")
                }));
            }
        }
    }
    out
}

/// What is actually started to run a harness's program.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Launch {
    pub program: PathBuf,
    /// Arguments before the harness's own.
    pub prefix: Vec<String>,
    /// Environment the program needs to be the CLI at all.
    pub env: Vec<(String, String)>,
}

impl Launch {
    fn direct(p: &Path) -> Launch {
        Launch {
            program: p.to_path_buf(),
            prefix: Vec::new(),
            env: Vec::new(),
        }
    }

    /// The launch as a command, prefix and environment set.
    pub(crate) fn command(&self) -> Command {
        let mut c = Command::new(&self.program);
        c.args(&self.prefix);
        c.envs(self.env.iter().map(|(k, v)| (k, v)));
        c
    }
}

/// Resolve `bin` into what is started. Ending a `.cmd` ends `cmd.exe` and
/// leaves the agent running, so behind a known shim the real program is
/// started instead: Cursor's node, Copilot's native binary, DeepSeek's
/// Electron executable run as Node (without `ELECTRON_RUN_AS_NODE` it opens the
/// desktop app's window). Resolved per spawn: Cursor updates itself into a new
/// version folder.
pub(crate) fn launch(h: Harness, bin: &Path) -> Launch {
    #[cfg(test)]
    if let Some(l) = fake::launch_of(bin) {
        return l;
    }
    let shim = matches!(ext_lower(bin).as_str(), "cmd" | "ps1");
    match h {
        Harness::Cursor if shim => cursor_node_launch(bin).unwrap_or_else(|| Launch::direct(bin)),
        Harness::Copilot if shim => bin
            .parent()
            .map(copilot_native_path)
            .filter(|p| p.is_file())
            .map_or_else(|| Launch::direct(bin), |p| Launch::direct(&p)),
        Harness::DeepSeek if ext_lower(bin) == "cmd" => {
            dsh_electron_launch(bin).unwrap_or_else(|| Launch::direct(bin))
        }
        _ => Launch::direct(bin),
    }
}

fn ext_lower(p: &Path) -> String {
    p.extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default()
}

/// The DeepSeek install's executable and CLI script behind its `dsh.cmd`.
fn dsh_electron_launch(bin: &Path) -> Option<Launch> {
    let resources = bin.parent()?.parent()?.parent()?.parent()?;
    let install = resources.parent()?;
    let exe = install.join("DeepSeek Harness.exe");
    let asar = resources.join("app.asar");
    if !exe.is_file() || !asar.is_file() {
        return None;
    }
    let cli = asar
        .join("dsh")
        .join("node_modules")
        .join("@deepseek-ai")
        .join("dsh-desktop-host")
        .join("lib")
        .join("cli.js");
    Some(Launch {
        program: exe,
        prefix: vec![
            "--expose-internals".into(),
            cli.to_string_lossy().into_owned(),
        ],
        env: vec![("ELECTRON_RUN_AS_NODE".into(), "1".into())],
    })
}

/// Where npm puts Copilot's native binary, from the shim's folder.
fn copilot_native_path(shim_dir: &Path) -> PathBuf {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        _ => "x64",
    };
    shim_dir
        .join("node_modules")
        .join("@github")
        .join("copilot")
        .join("node_modules")
        .join("@github")
        .join(format!("copilot-win32-{arch}"))
        .join("copilot.exe")
}

/// `node.exe index.js` behind a Cursor launcher script: beside it, or in the
/// newest `versions\<v>\` by the launcher's own rule.
fn cursor_node_launch(bin: &Path) -> Option<Launch> {
    let dir = bin.parent()?;
    let pair = |d: &Path| {
        let node = d.join("node.exe");
        let index = d.join("index.js");
        (node.is_file() && index.is_file()).then(|| Launch {
            program: node,
            prefix: vec![index.to_string_lossy().into_owned()],
            env: Vec::new(),
        })
    };
    if let Some(l) = pair(dir) {
        return Some(l);
    }
    let versions = dir.join("versions");
    let names: Vec<String> = std::fs::read_dir(&versions)
        .ok()?
        .flatten()
        .filter(|e| e.path().is_dir())
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .collect();
    let newest = newest_cursor_version(names.iter().map(String::as_str))?;
    pair(&versions.join(newest))
}

/// The newest of Cursor's version folders (`YYYY.M.D-<hex>` or
/// `YYYY.M.D-HH-MM-SS-<hex>`), the build time breaking a same-day tie.
fn newest_cursor_version<'a>(names: impl Iterator<Item = &'a str>) -> Option<&'a str> {
    fn key(name: &str) -> Option<[u32; 6]> {
        let (date, rest) = name.split_once('-')?;
        let mut d = date.split('.');
        let (y, m, day) = (d.next()?, d.next()?, d.next()?);
        if d.next().is_some()
            || y.len() != 4
            || !(1..=2).contains(&m.len())
            || !(1..=2).contains(&day.len())
        {
            return None;
        }
        let num = |s: &str| {
            s.bytes()
                .all(|b| b.is_ascii_digit())
                .then(|| s.parse::<u32>().ok())
                .flatten()
        };
        let hex = |s: &str| {
            !s.is_empty()
                && s.bytes()
                    .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        };
        let parts: Vec<&str> = rest.split('-').collect();
        let time = match parts.as_slice() {
            [c] if hex(c) => [0, 0, 0],
            [h, mi, s, c] if hex(c) && [h, mi, s].iter().all(|x| x.len() == 2) => {
                [num(h)?, num(mi)?, num(s)?]
            }
            _ => return None,
        };
        Some([num(y)?, num(m)?, num(day)?, time[0], time[1], time[2]])
    }
    names
        .filter_map(|n| key(n).map(|k| (k, n)))
        .max_by_key(|(k, _)| *k)
        .map(|(_, n)| n)
}

/// `PATHEXT`'s extensions on Windows; empty elsewhere.
fn pathext() -> Vec<String> {
    match cfg!(windows) {
        true => std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT".to_string())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(str::to_string)
            .collect(),
        false => Vec::new(),
    }
}

/// The `PATHEXT` extensions `CreateProcess` can start; the rest (`.PS1`,
/// `.VBS`, `.JS`) open in another program, or not at all.
const STARTABLE: [&str; 4] = [".EXE", ".COM", ".CMD", ".BAT"];

/// Which file in one folder is the executable for `name`. On Windows the bare
/// name is npm's `#!/bin/sh` script, not an executable, an extension that
/// cannot be started is never taken, and a batch shim goes last: Rust refuses
/// a multi-line argument to one. Elsewhere the name must match exactly.
fn pick_executable<'a>(name: &str, present: &[&'a str], exts: &[String]) -> Option<&'a str> {
    if exts.is_empty() {
        return present.iter().copied().find(|p| *p == name);
    }
    let has = |n: &str| present.iter().copied().find(|p| p.eq_ignore_ascii_case(n));
    let exts: Vec<&String> = exts
        .iter()
        .filter(|e| STARTABLE.iter().any(|s| s.eq_ignore_ascii_case(e)))
        .collect();
    let lower = name.to_ascii_lowercase();
    if exts
        .iter()
        .any(|e| lower.ends_with(&e.to_ascii_lowercase()))
        && let Some(hit) = has(name)
    {
        return Some(hit);
    }
    let is_batch = |e: &str| e.eq_ignore_ascii_case(".BAT") || e.eq_ignore_ascii_case(".CMD");
    exts.iter()
        .filter(|e| !is_batch(e))
        .chain(exts.iter().filter(|e| is_batch(e)))
        .find_map(|e| has(&format!("{name}{e}")))
}

/// Can the OS start the file at `p`? A file, through any link (a dangling one
/// left by an uninstall is not), and off Windows with an execute bit.
fn can_start(p: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(p) else {
        return false;
    };
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        meta.is_file() && meta.permissions().mode() & 0o111 != 0
    }
    #[cfg(not(unix))]
    {
        meta.is_file()
    }
}

/// Locate `name` on `PATH`, and the folders searched. A relative entry (the
/// empty one a trailing `;` makes) is skipped: it would answer against the
/// working folder.
fn which_on_path(name: &str) -> (Option<PathBuf>, Vec<PathBuf>) {
    let Some(path) = std::env::var_os("PATH") else {
        return (None, Vec::new());
    };
    let exts = pathext();
    let mut searched = Vec::new();
    for dir in std::env::split_paths(&path) {
        if dir.as_os_str().is_empty() || dir.is_relative() {
            continue;
        }
        searched.push(dir.clone());
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        let names: Vec<String> = entries
            .flatten()
            .filter(|e| e.file_type().is_ok_and(|t| !t.is_dir()))
            .map(|e| e.file_name().to_string_lossy().into_owned())
            .collect();
        let refs: Vec<&str> = names.iter().map(String::as_str).collect();
        if let Some(hit) = pick_executable(name, &refs, &exts)
            && can_start(&dir.join(hit))
        {
            return (Some(dir.join(hit)), searched);
        }
    }
    (None, searched)
}

/// A Settings path: a file, a bare command on `PATH`, or on Windows a path
/// missing its extension.
fn resolve_override(p: &Path) -> Option<PathBuf> {
    if p.is_file() {
        return Some(p.to_path_buf());
    }
    let s = p.to_string_lossy();
    if !s.contains('/') && !s.contains('\\') {
        return which_on_path(&s).0;
    }
    if cfg!(windows) && p.extension().is_none() {
        return pathext()
            .iter()
            .filter(|e| STARTABLE.iter().any(|x| x.eq_ignore_ascii_case(e)))
            .map(|e| PathBuf::from(format!("{s}{e}")))
            .find(|c| c.is_file());
    }
    None
}

type Cache = Mutex<HashMap<(Harness, PathBuf), Probe>>;

fn cache() -> &'static Cache {
    static CACHE: OnceLock<Cache> = OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// Fake harnesses for the session tests: a program path that launches this
/// test binary's `fake_harness` test, with its probe answered in advance.
#[cfg(test)]
pub(crate) mod fake {
    use super::*;

    fn launches() -> &'static Mutex<HashMap<PathBuf, Launch>> {
        static L: OnceLock<Mutex<HashMap<PathBuf, Launch>>> = OnceLock::new();
        L.get_or_init(|| Mutex::new(HashMap::new()))
    }

    pub(crate) fn launch_of(bin: &Path) -> Option<Launch> {
        launches().lock().ok()?.get(bin).cloned()
    }

    /// Make `program` (an empty file) start the fake in `mode`, sealed.
    pub(crate) fn install(h: Harness, program: &Path, mode: &str) {
        let me = std::env::current_exe().expect("the test binary");
        let launch = Launch {
            program: me,
            prefix: [
                "session::tests::fake_harness",
                "--exact",
                "--ignored",
                "--nocapture",
                "-q",
                "--",
            ]
            .map(String::from)
            .to_vec(),
            env: vec![("ONDIN_AI_FAKE".into(), mode.into())],
        };
        launches()
            .lock()
            .unwrap()
            .insert(program.to_path_buf(), launch);
        let probe = Probe {
            seal: match h {
                Harness::Claude => Seal::Sealed,
                _ => Seal::Restricted,
            },
            claude: crate::harness::ClaudeFlags::ALL,
            isolate_config: false,
        };
        cache()
            .lock()
            .unwrap()
            .insert((h, program.to_path_buf()), probe);
    }
}

/// How long `--help` may take. A cold start of an npm CLI under an antivirus
/// scan takes seconds; a program that is not a CLI at all (a GUI typed into
/// Settings' Path) never answers.
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);

/// What `--help` says about the program at `path`, cached per (harness, path).
/// A probe that could not run or did not finish (an antivirus lock, an upgrade
/// in place, a program that is not a CLI) is not an answer and is not cached.
pub(crate) fn probe(h: Harness, path: &Path) -> Probe {
    let key = (h, path.to_path_buf());
    if let Some(p) = cache().lock().ok().and_then(|c| c.get(&key).copied()) {
        return p;
    }
    let mut cmd = launch(h, path).command();
    cmd.args(h.help_args())
        .env(crate::AGENT_CHILD_ENV, "1")
        .stdin(std::process::Stdio::null());
    let Ok(Some(out)) = crate::process::run_bounded(&mut cmd, PROBE_TIMEOUT) else {
        return Probe::read(h, false, "", "");
    };
    let probe = Probe::read(
        h,
        out.status.success(),
        &String::from_utf8_lossy(&out.stdout),
        &String::from_utf8_lossy(&out.stderr),
    );
    if let Ok(mut c) = cache().lock() {
        c.insert(key, probe);
    }
    probe
}

#[cfg(test)]
mod tests {
    use super::*;

    /// On Windows the bare npm script never wins, and a batch shim is taken
    /// only when nothing else is there.
    #[test]
    fn a_batch_shim_goes_last_and_the_bare_script_never_wins() {
        let exts: Vec<String> = [".CMD", ".EXE"].map(String::from).to_vec();
        assert_eq!(
            pick_executable("codex", &["codex", "codex.cmd", "codex.exe"], &exts),
            Some("codex.exe")
        );
        assert_eq!(
            pick_executable("codex", &["codex", "codex.cmd"], &exts),
            Some("codex.cmd")
        );
        assert_eq!(pick_executable("codex", &["codex"], &exts), None);
        assert_eq!(pick_executable("codex", &["codex"], &[]), Some("codex"));
        assert_eq!(
            pick_executable("naïve.exe", &["naïve.exe"], &exts),
            Some("naïve.exe")
        );
    }

    /// An extension `CreateProcess` cannot start is never taken, wherever
    /// `PATHEXT` lists it, and off Windows a name matches only exactly.
    /// flip: `pick_executable` without its `STARTABLE` filter → fails at the first assertion.
    #[test]
    fn a_file_that_cannot_be_started_is_never_picked() {
        let exts: Vec<String> = [".COM", ".PS1", ".CMD", ".EXE"].map(String::from).to_vec();
        assert_eq!(
            pick_executable("codex", &["codex", "codex.cmd", "codex.ps1"], &exts),
            Some("codex.cmd")
        );
        assert_eq!(pick_executable("codex", &["codex.ps1"], &exts), None);
        assert_eq!(pick_executable("claude", &["Claude"], &[]), None);
    }

    /// A dangling link and a file without an execute bit cannot be started.
    #[cfg(unix)]
    #[test]
    fn a_dangling_link_or_a_plain_file_cannot_start() {
        use std::os::unix::fs::PermissionsExt;
        let dir = std::env::temp_dir().join(format!("ondin-ai-start-{}", crate::run::random_tag()));
        std::fs::create_dir_all(&dir).unwrap();
        let plain = dir.join("plain");
        std::fs::write(&plain, "#!/bin/sh\n").unwrap();
        assert!(!can_start(&plain));
        std::fs::set_permissions(&plain, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(can_start(&plain));
        let link = dir.join("link");
        std::os::unix::fs::symlink(dir.join("gone"), &link).unwrap();
        assert!(!can_start(&link));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The newest Cursor version by date, then build time; other folders ignored.
    #[test]
    fn the_newest_cursor_version_wins() {
        let names = [
            "2026.9.3-abc123",
            "2026.10.1-12-00-00-def456",
            "2026.10.1-09-30-00-aaa111",
            "latest",
            "2026.10.1-ABC",
        ];
        assert_eq!(
            newest_cursor_version(names.into_iter()),
            Some("2026.10.1-12-00-00-def456")
        );
    }

    /// A Settings path that does not exist is not found, and names only itself.
    #[test]
    fn a_missing_override_is_not_found() {
        let p = Path::new("/no/such/folder/claude-x");
        let err = locate(Harness::Claude, Some(p)).unwrap_err();
        assert_eq!(err.program, "claude");
        assert_eq!(err.looked_in, vec![p.to_path_buf()]);
    }

    /// Not a test: the probe run by `a_probe_never_runs_a_program_planted_beside_ondin`
    /// from a process whose working folder holds the planted program.
    #[test]
    #[ignore = "started by a probe test in a planted working folder"]
    fn probe_from_a_planted_folder() {
        if let Some(shim) = std::env::var_os("ONDIN_AI_PLANT_SHIM") {
            probe(Harness::Codex, Path::new(&shim));
        }
    }

    /// An npm-style shim probed while Ondin's working folder holds a `node.bat`
    /// runs the real `node`, never the planted one. Two defences, each enough:
    /// flip: `run_bounded` without its scratch `current_dir` and `Tracker::spawn`
    /// without `NoDefaultCurrentDirectoryInExePath` → fails at the last assertion.
    #[cfg(windows)]
    #[test]
    fn a_probe_never_runs_a_program_planted_beside_ondin() {
        const GUARD: &str = "NoDefaultCurrentDirectoryInExePath";
        let dir = std::env::temp_dir().join(format!("ondin-ai-plant-{}", crate::run::random_tag()));
        let bin = dir.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        std::fs::write(dir.join("node.bat"), "@echo planted> \"%~dp0marker\"\r\n").unwrap();
        let shim = bin.join("fake.cmd");
        std::fs::write(&shim, "@SET \"_prog=node\"\r\n@\"%_prog%\" --version\r\n").unwrap();
        let marker = dir.join("marker");
        // The fixture bites: run plainly from that folder, the shim runs the plant.
        let plain = Command::new(&shim)
            .current_dir(&dir)
            .env_remove(GUARD)
            .stdout(std::process::Stdio::null())
            .status();
        assert!(plain.is_ok());
        assert!(marker.is_file(), "the planted node.bat ran");
        std::fs::remove_file(&marker).unwrap();
        let me = std::env::current_exe().unwrap();
        let status = Command::new(me)
            .args([
                "find::tests::probe_from_a_planted_folder",
                "--exact",
                "--ignored",
                "-q",
            ])
            .current_dir(&dir)
            .env_remove(GUARD)
            .env("ONDIN_AI_PLANT_SHIM", &shim)
            .stdout(std::process::Stdio::null())
            .status()
            .unwrap();
        assert!(status.success());
        assert!(!marker.exists(), "the probe ran the planted node.bat");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The environment override is named after the harness's key.
    #[test]
    fn env_overrides_follow_the_key() {
        assert_eq!(env_var(Harness::DeepSeek), "ONDIN_DEEPSEEK_BIN");
        assert_eq!(env_var(Harness::Antigravity), "ONDIN_ANTIGRAVITY_BIN");
    }
}
