//! Cursor's working folder, the approval its project server needs, and what it
//! records about that folder in the person's own Cursor folder.
//!
//! Cursor's restriction and Ondin's server are both project files found from
//! its working directory (`.cursor/cli.json`, `.cursor/mcp.json`). A project
//! server loads only once `cursor-agent mcp enable ondin` approves it, which
//! approves Ondin's alone where `--approve-mcps` would approve every server in
//! reach. The approval and the conversations land in the person's Cursor
//! folder, keyed by Ondin's working folder; they are found by their content
//! naming that folder, never by recomputing the CLI's hash, so nothing of the
//! person's own can match.

use crate::MCP_SERVER;
use crate::find::Launch;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// How long `mcp enable` may take; measured runs took a few seconds.
const APPROVE_TIMEOUT: Duration = Duration::from_secs(60);

/// The working folder inside a session's run folder.
pub(crate) fn workspace_in(run_folder: &Path) -> PathBuf {
    run_folder.join("cursor")
}

/// Approve Ondin's server for `workspace`. `Err` carries what the CLI said.
pub(crate) fn approve(
    launch: &Launch,
    workspace: &Path,
    env: &[(String, String)],
    clear: &[&str],
) -> Result<(), String> {
    let mut c = launch.command();
    c.args(["mcp", "enable", MCP_SERVER]).current_dir(workspace);
    for k in clear {
        c.env_remove(k);
    }
    c.envs(env.iter().map(|(k, v)| (k, v)))
        .env(crate::AGENT_CHILD_ENV, "1")
        .stdin(std::process::Stdio::null());
    let Some(done) =
        crate::process::run_bounded(&mut c, APPROVE_TIMEOUT).map_err(|e| e.to_string())?
    else {
        return Err("`cursor-agent mcp enable` did not finish".into());
    };
    let status = done.status;
    if status.success() {
        return Ok(());
    }
    let text = |b: &[u8]| String::from_utf8_lossy(b).trim().to_string();
    let (stdout, stderr) = (text(&done.stdout), text(&done.stderr));
    Err(match (stderr.is_empty(), stdout.is_empty()) {
        (false, _) => stderr,
        (true, false) => stdout,
        (true, true) => format!("`cursor-agent mcp enable` exited with {status}"),
    })
}

/// The person's Cursor folder, by the CLI's own rule: `CURSOR_CONFIG_DIR`, else
/// `$XDG_CONFIG_HOME/cursor`, else `<home>/.cursor`.
fn cursor_config_dir() -> Option<PathBuf> {
    let var = |k: &str| std::env::var_os(k).filter(|v| !v.is_empty());
    if let Some(d) = var("CURSOR_CONFIG_DIR") {
        return Some(PathBuf::from(d));
    }
    if let Some(x) = var("XDG_CONFIG_HOME") {
        return Some(PathBuf::from(x).join("cursor"));
    }
    Some(crate::find::home()?.join(".cursor"))
}

/// Is `recorded` exactly one of `ours`? Case-folded where paths are; a path
/// spelled differently does not match, which leaves an entry rather than
/// removing someone else's.
fn names_one_of(recorded: &str, ours: &[PathBuf], fold_case: bool) -> bool {
    let p = Path::new(recorded);
    ours.iter().any(|o| match fold_case {
        true => o.to_string_lossy().to_lowercase() == p.to_string_lossy().to_lowercase(),
        false => o == p,
    })
}

/// The workspace a Cursor state file records.
fn recorded_workspace(file: &Path, field: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(file).ok()?).ok()?;
    v.get(field)?.as_str().map(str::to_string)
}

/// Remove what Cursor keeps about `workspaces` in the person's Cursor folder:
/// `projects/<slug>/` (its `.workspace-trusted` names the folder, and it holds
/// the approval) and `chats/<hash>/<chat>/` (its `meta.json` names it).
pub(crate) fn forget(workspaces: &[PathBuf]) {
    let Some(cfg) = cursor_config_dir() else {
        return;
    };
    let ours = |w: &str| names_one_of(w, workspaces, cfg!(windows));
    if let Ok(entries) = std::fs::read_dir(cfg.join("projects")) {
        for e in entries.flatten() {
            if recorded_workspace(&e.path().join(".workspace-trusted"), "workspacePath")
                .is_some_and(|w| ours(&w))
            {
                let _ = std::fs::remove_dir_all(e.path());
            }
        }
    }
    if let Ok(groups) = std::fs::read_dir(cfg.join("chats")) {
        for g in groups.flatten() {
            let Ok(chats) = std::fs::read_dir(g.path()) else {
                continue;
            };
            let mut removed = false;
            for c in chats.flatten() {
                if recorded_workspace(&c.path().join("meta.json"), "cwd").is_some_and(|w| ours(&w))
                {
                    removed |= std::fs::remove_dir_all(c.path()).is_ok();
                }
            }
            // Empty-only, and only where this emptied it.
            if removed {
                let _ = std::fs::remove_dir(g.path());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Only an exact recorded path matches, case-folded only where asked.
    #[test]
    fn only_our_own_workspaces_match() {
        let ours = vec![PathBuf::from("/run/s1-2-ab/cursor")];
        assert!(names_one_of("/run/s1-2-ab/cursor", &ours, false));
        assert!(!names_one_of("/run/s1-2-ab/cursor2", &ours, false));
        assert!(!names_one_of("/run/s1-2-ab", &ours, false));
        assert!(!names_one_of("/RUN/s1-2-ab/cursor", &ours, false));
        assert!(names_one_of("/RUN/s1-2-ab/cursor", &ours, true));
    }
}
