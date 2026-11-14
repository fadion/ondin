//! Antigravity's two pieces of global state, held for one session.
//!
//! It takes no per-invocation configuration: its MCP servers are registered
//! with `agy mcp add` in the person's own registry, and its tool permissions
//! live in a `settings.json` it shares with itself. So a session writes into
//! the person's configuration and takes it back out. Each session registers
//! under a name of its own and grants rules for that name only, so it never
//! touches a registration or a rule the person made. A claim marker per name
//! says which live Ondin owns them, and a crash's leftovers are swept at launch
//! once their owner is gone; a marker goes only once its withdrawal held.
//!
//! Known limit: while a session holds them, any `agy` the person runs finds
//! Ondin's server registered and its granted tools pre-approved. No option
//! scopes either to one process, so the window is kept to the session.

use crate::find::Launch;
use crate::harness::{
    SettingsEdit, antigravity_allow_rules, antigravity_server_name, antigravity_settings_with_only,
    antigravity_settings_without,
};
use crate::process::Owner;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Where Antigravity keeps its permissions (under `.gemini`: it shares that
/// root with Google's earlier CLI). `ONDIN_AGY_SETTINGS` overrides it, so a
/// moved file costs the tools rather than a write into the wrong file.
fn settings_path() -> Option<PathBuf> {
    if let Some(p) = std::env::var_os("ONDIN_AGY_SETTINGS").filter(|p| !p.is_empty()) {
        return Some(PathBuf::from(p));
    }
    Some(
        crate::find::home()?
            .join(".gemini")
            .join("antigravity-cli")
            .join("settings.json"),
    )
}

/// The settings file as read: a file that is there and unreadable may grant
/// anything, so it is neither overwritten nor read as missing.
#[derive(Debug, PartialEq, Eq)]
enum Existing {
    Text(String),
    Missing,
    Unreadable,
}

impl From<std::io::Result<String>> for Existing {
    fn from(read: std::io::Result<String>) -> Self {
        match read {
            Ok(t) => Existing::Text(t),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Existing::Missing,
            Err(_) => Existing::Unreadable,
        }
    }
}

/// What to do with the file. Granting may create it; withdrawing from a file
/// that is not there holds without writing (most people have no Antigravity).
#[derive(Debug, PartialEq, Eq)]
enum Plan {
    Holds,
    Declined,
    Write(String),
}

fn plan(create: bool, existing: Existing, edit: impl Fn(&str) -> Option<SettingsEdit>) -> Plan {
    let current = match (existing, create) {
        (Existing::Unreadable, _) => return Plan::Declined,
        (Existing::Missing, false) => return Plan::Holds,
        (Existing::Missing, true) => String::new(),
        (Existing::Text(t), _) => t,
    };
    match edit(&current) {
        None => Plan::Declined,
        Some(SettingsEdit::Unchanged) => Plan::Holds,
        Some(SettingsEdit::Write(next)) => Plan::Write(next),
    }
}

/// Edit the settings file under the settings lock; `true` when it now says
/// what was asked.
fn edit_settings(
    run_root: &Path,
    create: bool,
    edit: impl Fn(&str) -> Option<SettingsEdit>,
) -> bool {
    let Some(path) = settings_path() else {
        return false;
    };
    let Some(_lock) = SettingsLock::take(run_root) else {
        return false;
    };
    match plan(create, std::fs::read_to_string(&path).into(), edit) {
        Plan::Holds => true,
        Plan::Declined => false,
        Plan::Write(next) => write_atomic(&path, next.as_bytes()).is_ok(),
    }
}

/// Write through a sibling and a rename: another vendor's file must never be
/// left truncated. Through a symlink to its target (a rename would replace the
/// link), keeping the file's mode, synced before the rename.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let path = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(path)?,
        _ => path.to_path_buf(),
    };
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut name = path.as_os_str().to_os_string();
    name.push(format!(".ondin-{}.tmp", crate::run::random_tag()));
    let tmp = PathBuf::from(name);
    let result = (|| {
        {
            let mut f = std::fs::File::create(&tmp)?;
            f.write_all(bytes)?;
            f.sync_all()?;
            if let Ok(meta) = std::fs::metadata(&path) {
                let _ = f.set_permissions(meta.permissions());
            }
        }
        std::fs::rename(&tmp, &path)
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

/// The folder of claim markers and the settings lock, under the run root.
fn claims_dir(run_root: &Path) -> PathBuf {
    run_root.join("antigravity")
}

const LOCK_NAME: &str = "settings.lock";

/// Exclusive use of the settings file across every Ondin on the machine, held
/// for one read-modify-write. A dead holder's lock is taken over.
struct SettingsLock(PathBuf);

impl SettingsLock {
    fn take(run_root: &Path) -> Option<SettingsLock> {
        static IN_PROCESS: std::sync::Mutex<()> = std::sync::Mutex::new(());
        let dir = claims_dir(run_root);
        std::fs::create_dir_all(&dir).ok()?;
        let path = dir.join(LOCK_NAME);
        let me = Owner::me()?;
        let until = std::time::Instant::now() + Duration::from_secs(10);
        let _guard = IN_PROCESS.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(mut f) => {
                    f.write_all(format!("{} {}\n", me.pid, me.started).as_bytes())
                        .ok()?;
                    return Some(SettingsLock(path));
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {
                    let held = std::fs::read_to_string(&path).ok();
                    let holder = held.as_deref().and_then(|t| {
                        let mut it = t.split_whitespace();
                        Some(Owner {
                            pid: it.next()?.parse().ok()?,
                            started: it.next()?.parse().ok()?,
                        })
                    });
                    // A holder that is gone, or a lock left half-written by one.
                    if holder.is_some_and(|h| !h.is_live()) {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }
                    if std::time::Instant::now() >= until {
                        return None;
                    }
                    std::thread::sleep(Duration::from_millis(50));
                }
                Err(_) => return None,
            }
        }
    }
}

impl Drop for SettingsLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

/// What a claim marker records: the session's process, and the `agy` it
/// registered with, so a sweep reaches a program set only in Settings.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Claim {
    owner: Owner,
    program: PathBuf,
}

fn claim_text(c: &Claim) -> String {
    format!(
        "{} {}\n{}\n",
        c.owner.pid,
        c.owner.started,
        c.program.display()
    )
}

/// A claim read back; anything garbled is no claim.
fn parse_claim(s: &str) -> Option<Claim> {
    let (head, rest) = s.split_once('\n')?;
    let mut it = head.split_whitespace();
    let pid = it.next()?.parse().ok()?;
    let started = it.next()?.parse().ok()?;
    if it.next().is_some() {
        return None;
    }
    let program = rest.strip_suffix('\n').unwrap_or(rest);
    Some(Claim {
        owner: Owner { pid, started },
        program: PathBuf::from(program),
    })
}

/// How long one `agy mcp …` may take before it is ended as failed: a session
/// ends on the thread that drops it, and agy may stop at a sign-in or update.
const AGY_TIMEOUT: Duration = Duration::from_secs(30);

/// Run `agy mcp …`; `true` when it succeeded in time, or, for a removal, when
/// the name was already gone.
fn agy_mcp(launch: &Launch, args: &[&str]) -> bool {
    let mut c = launch.command();
    c.arg("mcp")
        .args(args)
        .env(crate::AGENT_CHILD_ENV, "1")
        .stdin(std::process::Stdio::null());
    let Ok(Some(done)) = crate::process::run_bounded(&mut c, AGY_TIMEOUT) else {
        return false;
    };
    let said = |b: &[u8]| String::from_utf8_lossy(b).contains("not found");
    done.status.success()
        || (args.first() == Some(&"remove") && (said(&done.stderr) || said(&done.stdout)))
}

/// Take back `server`'s rules and registration; `true` when both are gone.
fn withdraw(run_root: &Path, launch: Option<&Launch>, server: &str) -> bool {
    let rules = edit_settings(run_root, false, |cur| {
        antigravity_settings_without(cur, server)
    });
    let registration = launch.is_some_and(|l| agy_mcp(l, &["remove", server]));
    rules && registration
}

/// Antigravity's registration and allow-rules for one session, withdrawn on drop.
pub(crate) struct Registration {
    launch: Launch,
    run_root: PathBuf,
    server: String,
}

impl Registration {
    /// Claim a fresh name, register Ondin's server under it, then grant exactly
    /// `allowed`. The claim goes first and is released last, so no global
    /// state ever stands without a marker accounting for it. `Err` says why,
    /// in a clause.
    pub(crate) fn install(
        launch: Launch,
        program: &Path,
        run_root: &Path,
        exe: &str,
        args: &[String],
        allowed: &[String],
    ) -> Result<Registration, String> {
        let owner = Owner::me().ok_or("this process's start time could not be read")?;
        let mine = Claim {
            owner,
            program: program.to_path_buf(),
        };
        let dir = claims_dir(run_root);
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        // A fresh name taken with `create_new`: no other session can hold it.
        let (server, mut marker) = (0..8)
            .find_map(|_| {
                let server = antigravity_server_name(&crate::run::random_tag());
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(dir.join(&server))
                    .ok()
                    .map(|f| (server, f))
            })
            .ok_or("no claim marker could be made")?;
        let written = marker
            .write_all(claim_text(&mine).as_bytes())
            .and_then(|_| marker.sync_all());
        drop(marker);
        let reg = Registration {
            launch,
            run_root: run_root.to_path_buf(),
            server,
        };
        written.map_err(|e| e.to_string())?;
        // `--` first: the command's own arguments start with `-`.
        let mut add: Vec<&str> = vec!["add", &reg.server, "--", exe];
        add.extend(args.iter().map(String::as_str));
        if !agy_mcp(&reg.launch, &add) {
            return Err("`agy mcp add` failed".into());
        }
        let rules = antigravity_allow_rules(&reg.server, allowed);
        if !edit_settings(run_root, true, |cur| {
            antigravity_settings_with_only(cur, &reg.server, &rules)
        }) {
            return Err("its settings file could not be given Ondin's tool rules".into());
        }
        Ok(reg)
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        let marker = claims_dir(&self.run_root).join(&self.server);
        if withdraw(&self.run_root, Some(&self.launch), &self.server) {
            let _ = std::fs::remove_file(marker);
        }
    }
}

/// Withdraw every dead owner's registration and rules, each with the `agy` it
/// was made with (else the one found now), and remove a marker only once its
/// withdrawal held. Nothing of a live Ondin's is touched.
pub(crate) fn sweep(run_root: &Path) {
    let Ok(entries) = std::fs::read_dir(claims_dir(run_root)) else {
        return;
    };
    for e in entries.flatten() {
        let server = e.file_name().to_string_lossy().into_owned();
        if !crate::harness::is_antigravity_session_server(&server) {
            continue;
        }
        let Some(claim) = std::fs::read_to_string(e.path())
            .ok()
            .and_then(|t| parse_claim(&t))
        else {
            // Garbled: a crash between creating the marker and writing it,
            // before anything was registered.
            if e.metadata()
                .and_then(|m| m.modified())
                .is_ok_and(|t| t.elapsed().is_ok_and(|age| age > Duration::from_secs(60)))
            {
                let _ = std::fs::remove_file(e.path());
            }
            continue;
        };
        if claim.owner.is_live() {
            continue;
        }
        let program = match claim.program.is_file() {
            true => Some(claim.program.clone()),
            false => crate::find::locate(crate::Harness::Antigravity, None).ok(),
        };
        let launch = program.map(|p| crate::find::launch(crate::Harness::Antigravity, &p));
        if withdraw(run_root, launch.as_ref(), &server) {
            let _ = std::fs::remove_file(e.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A claim round-trips with its program path, and a garbled one is no claim.
    #[test]
    fn a_claim_round_trips() {
        let c = Claim {
            owner: Owner {
                pid: 4,
                started: 10,
            },
            program: PathBuf::from("C:\\Users\\me\\agy dir\\agy.exe"),
        };
        assert_eq!(parse_claim(&claim_text(&c)), Some(c));
        assert_eq!(parse_claim("4 10"), None);
        assert_eq!(parse_claim("4 10 99\nx\n"), None);
    }

    /// Withdrawing from a file that is not there writes nothing; an unreadable
    /// file is declined either way; granting creates a missing file.
    #[test]
    fn the_settings_plan() {
        let me = "ondin0123abcd";
        assert_eq!(
            plan(false, Existing::Missing, |c| antigravity_settings_without(
                c, me
            )),
            Plan::Holds
        );
        assert_eq!(
            plan(true, Existing::Unreadable, |_| Some(
                SettingsEdit::Unchanged
            )),
            Plan::Declined
        );
        let rules = antigravity_allow_rules(me, &["get_node".into()]);
        let Plan::Write(text) = plan(true, Existing::Missing, |c| {
            antigravity_settings_with_only(c, me, &rules)
        }) else {
            panic!("a grant creates the file");
        };
        assert!(text.contains("mcp(ondin0123abcd/get_node)"));
        assert_eq!(
            plan(false, Existing::Text("not json".into()), |c| {
                antigravity_settings_without(c, me)
            }),
            Plan::Declined
        );
    }

    /// A rewrite goes through a symlink to its target and keeps the file's mode.
    /// flip: `write_atomic` without its `canonicalize` → fails at "still a link".
    #[cfg(unix)]
    #[test]
    fn a_rewrite_keeps_the_link_and_the_mode() {
        use std::os::unix::fs::PermissionsExt;
        let root = std::env::temp_dir().join(format!("ondin-ai-agy-{}", crate::run::random_tag()));
        let real = root.join("dotfiles").join("settings.json");
        std::fs::create_dir_all(real.parent().unwrap()).unwrap();
        std::fs::write(&real, "{}").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o600)).unwrap();
        let link = root.join("settings.json");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, b"{\"a\":1}").unwrap();
        let meta = std::fs::symlink_metadata(&link).unwrap();
        assert!(meta.file_type().is_symlink(), "still a link");
        assert_eq!(std::fs::read_to_string(&real).unwrap(), "{\"a\":1}");
        let mode = std::fs::metadata(&real).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A session registers and grants under a name of its own, and takes back
    /// exactly that, leaving the person's own `ondin` rule and registration.
    /// flip: `antigravity_server_name` returning `MCP_SERVER` → fails at the
    /// `is_antigravity_session_server` assertion.
    #[cfg(any(windows, unix))]
    #[test]
    fn a_session_registers_under_its_own_name() {
        let root = std::env::temp_dir().join(format!("ondin-ai-agy-{}", crate::run::random_tag()));
        std::fs::create_dir_all(&root).unwrap();
        let log = root.join("argv.txt");
        let agy = match cfg!(windows) {
            true => {
                let p = root.join("agy.cmd");
                std::fs::write(&p, "@echo %*>> \"%~dp0argv.txt\"\r\n").unwrap();
                p
            }
            false => {
                let p = root.join("agy");
                std::fs::write(
                    &p,
                    "#!/bin/sh\necho \"$@\" >> \"$(dirname \"$0\")/argv.txt\"\n",
                )
                .unwrap();
                p
            }
        };
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&agy, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let settings = root.join("settings.json");
        std::fs::write(
            &settings,
            r#"{"permissions":{"allow":["mcp(ondin/get_tree)"]}}"#,
        )
        .unwrap();
        // SAFETY: test-only; the one other reader (`a_marker_survives…`) finds
        // nothing of its server in whichever file this names.
        unsafe { std::env::set_var("ONDIN_AGY_SETTINGS", &settings) };
        let launch = crate::find::launch(crate::Harness::Antigravity, &agy);
        let reg = Registration::install(
            launch,
            &agy,
            &root,
            "ondin",
            &["mcp-proxy".into()],
            &["get_node".into()],
        )
        .expect("installed");
        let server = reg.server.clone();
        assert!(crate::harness::is_antigravity_session_server(&server));
        let argv = std::fs::read_to_string(&log).unwrap();
        assert!(
            argv.contains(&format!("mcp add {server} -- ondin mcp-proxy")),
            "{argv}"
        );
        let granted = std::fs::read_to_string(&settings).unwrap();
        assert!(granted.contains(&format!("mcp({server}/get_node)")));
        assert!(granted.contains("mcp(ondin/get_tree)"));
        let marker = claims_dir(&root).join(&server);
        assert!(marker.is_file());
        drop(reg);
        let argv = std::fs::read_to_string(&log).unwrap();
        assert!(argv.contains(&format!("mcp remove {server}")), "{argv}");
        assert!(!argv.contains("remove ondin\n") && !argv.contains("remove ondin \r"));
        let after = std::fs::read_to_string(&settings).unwrap();
        assert!(!after.contains(&server));
        assert!(after.contains("mcp(ondin/get_tree)"));
        assert!(!marker.exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A dead owner's marker whose withdrawal did not hold (its recorded `agy`
    /// fails) is kept for the next sweep; a live owner's is never touched.
    /// flip: `sweep` removing the marker whatever `withdraw` said → fails at "kept".
    #[cfg(any(windows, target_os = "linux", target_os = "macos"))]
    #[test]
    fn a_marker_survives_a_withdrawal_that_did_not_hold() {
        let root = std::env::temp_dir().join(format!("ondin-ai-agy-{}", crate::run::random_tag()));
        let dir = claims_dir(&root);
        std::fs::create_dir_all(&dir).unwrap();
        let agy = match cfg!(windows) {
            true => root.join("agy.cmd"),
            false => root.join("agy"),
        };
        std::fs::write(
            &agy,
            if cfg!(windows) {
                "@exit /b 1\r\n"
            } else {
                "#!/bin/sh\nexit 1\n"
            },
        )
        .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&agy, std::fs::Permissions::from_mode(0o755)).unwrap();
        }
        let me = Owner::me().unwrap();
        let dead = Claim {
            owner: Owner {
                started: me.started.wrapping_add(1),
                ..me
            },
            program: agy.clone(),
        };
        let live = Claim {
            owner: me,
            program: agy,
        };
        let (dead_at, live_at) = (dir.join("ondin00000001"), dir.join("ondin00000002"));
        std::fs::write(&dead_at, claim_text(&dead)).unwrap();
        std::fs::write(&live_at, claim_text(&live)).unwrap();
        sweep(&root);
        assert!(dead_at.is_file(), "kept");
        assert!(live_at.is_file(), "a live owner's marker is never touched");
        assert!(!dir.join(LOCK_NAME).exists(), "the lock is released");
        let _ = std::fs::remove_dir_all(&root);
    }
}
