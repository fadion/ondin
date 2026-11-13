//! The agent log: one JSON line per tool call an assistant chat makes, refusals
//! included, in the machine's data folder. It says which tool, on which
//! document, for which chat, whether it answered, and when; never what it
//! answered. Always on, never read back by the app; past [`ROLL`] it starts
//! again, keeping the one before as `agent_log.jsonl.1`.

use serde_json::json;
use std::io::Write as _;
use std::path::{Path, PathBuf};

const FILE: &str = "agent_log.jsonl";

/// The size past which the log starts again.
const ROLL: u64 = 8 << 20;

/// Where the log is written, and how large it grows before it rolls over.
pub(crate) struct Log {
    path: Option<PathBuf>,
    roll: u64,
}

impl Default for Log {
    /// The machine's log; none under test, where a run must not write there.
    fn default() -> Self {
        let path = match cfg!(test) {
            true => None,
            false => crate::machine_dir::root().map(|r| r.join(FILE)),
        };
        Self { path, roll: ROLL }
    }
}

impl Log {
    #[cfg(test)]
    pub(crate) fn at(path: PathBuf, roll: u64) -> Self {
        Self {
            path: Some(path),
            roll,
        }
    }

    /// Add one call's line. A log that cannot be written is let go: the call
    /// it records has already answered.
    pub(crate) fn write(&self, tool: &str, document: Option<&Path>, key: &str, ok: bool) {
        let Some(path) = &self.path else {
            return;
        };
        let line = json!({
            "at": crate::library::clock::now(),
            "tool": tool,
            "document": document.map(|d| d.display().to_string()),
            "key": key,
            "ok": ok,
        });
        if let Err(e) = append(path, self.roll, &line.to_string()) {
            log::warn!(target: "ondin", "agent log: {e}");
        }
    }
}

/// Append `line` to `path`, first moving a file past `roll` to its `.1`.
fn append(path: &Path, roll: u64, line: &str) -> std::io::Result<()> {
    if std::fs::metadata(path).is_ok_and(|m| m.len() >= roll) {
        let mut old = path.as_os_str().to_owned();
        old.push(".1");
        std::fs::rename(path, PathBuf::from(old))?;
    } else if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    writeln!(file, "{line}")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Each call is one line with its tool, document, key and outcome, and a
    /// log past its size starts again with the old one kept as `.1`.
    ///
    /// flip: `append` without its roll → fails at "rolled".
    #[test]
    fn a_line_per_call_and_the_log_rolls_over() {
        let dir = std::env::temp_dir().join(format!("ondin-agent-log-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join(FILE);
        let log = Log::at(path.clone(), 200);
        log.write("get_node", Some(Path::new("/a.ondin")), "chat-1", true);
        log.write("set", None, "chat-1", false);
        let text = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<serde_json::Value> = text
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["tool"], "get_node");
        assert_eq!(lines[0]["key"], "chat-1");
        assert_eq!(lines[1]["ok"], false);
        assert!(lines[0]["at"].as_u64().is_some());
        for _ in 0..4 {
            log.write("get_node", None, "chat-1", true);
        }
        let rolled = dir.join(format!("{FILE}.1"));
        assert!(rolled.exists(), "rolled");
        let kept = std::fs::read_to_string(&path).unwrap().lines().count();
        assert!(kept < 6, "started again, holding {kept} of the 6 lines");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
