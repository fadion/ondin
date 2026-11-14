//! Antigravity's two pieces of global state, held for one session.
//!
//! It takes no per-invocation configuration: its MCP servers are registered
//! with `agy mcp add` in the person's own registry, and its tool permissions
//! live in a `settings.json` it shares with itself. So a session writes into
//! the person's configuration and takes it back out: both are installed
//! together and removed together, a claim marker says which live Ondin owns
//! them, and a crash's leftovers are swept at launch once their owner is gone.
//!
//! Known limit: while a session holds them, any `agy` the person runs finds
//! Ondin's server registered and its granted tools pre-approved. No option
//! scopes either to one process, so the window is kept to the session.

use crate::MCP_SERVER;
use crate::find::Launch;
use crate::harness::{
    SettingsEdit, antigravity_allow_rules, antigravity_settings_with_only,
    antigravity_settings_without,
};
use crate::process::Owner;
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

/// Edit the settings file; `true` when it now says what was asked.
fn edit_settings(create: bool, edit: impl Fn(&str) -> Option<SettingsEdit>) -> bool {
    let Some(path) = settings_path() else {
        return false;
    };
    match plan(create, std::fs::read_to_string(&path).into(), edit) {
        Plan::Holds => true,
        Plan::Declined => false,
        Plan::Write(next) => {
            if let Some(dir) = path.parent() {
                let _ = std::fs::create_dir_all(dir);
            }
            write_atomic(&path, next.as_bytes()).is_ok()
        }
    }
}

/// Write through a sibling and a rename: another vendor's file must never be
/// left truncated, and the marker's reader is another process.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension(format!("ondin-{}.tmp", crate::run::random_tag()));
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path).inspect_err(|_| {
        let _ = std::fs::remove_file(&tmp);
    })
}

/// Which session holds the global state: its process, and a nonce telling two
/// sessions of one process apart.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Claim {
    owner: Owner,
    nonce: u64,
}

fn claim_text(c: Claim) -> String {
    format!("{} {} {}\n", c.owner.pid, c.owner.started, c.nonce)
}

/// A claim read back; anything garbled is no claim, so a crash stays sweepable.
fn parse_claim(s: &str) -> Option<Claim> {
    let mut it = s.split_whitespace();
    let pid = it.next()?.parse().ok()?;
    let started = it.next()?.parse().ok()?;
    let nonce = it.next()?.parse().ok()?;
    it.next().is_none().then_some(Claim {
        owner: Owner { pid, started },
        nonce,
    })
}

fn marker_path(run_root: &Path) -> PathBuf {
    run_root.join("antigravity-owner")
}

fn claim_on_disk(run_root: &Path) -> Option<Claim> {
    parse_claim(&std::fs::read_to_string(marker_path(run_root)).ok()?)
}

/// May this process claim over what is on disk? Not over another live Ondin:
/// the registration names one MCP command line and cannot serve two.
fn may_claim(on_disk: Option<Claim>, me: Owner, live: impl Fn(Owner) -> bool) -> bool {
    match on_disk {
        None => true,
        Some(c) if c.owner == me => true,
        Some(c) => !live(c.owner),
    }
}

fn next_nonce() -> u64 {
    u64::from_str_radix(&crate::run::random_tag(), 16).unwrap_or(1)
}

/// How long one `agy mcp …` may take before it is ended as failed: a session
/// ends on the thread that drops it, and agy may stop at a sign-in or update.
const AGY_TIMEOUT: Duration = Duration::from_secs(30);

/// Run `agy mcp …`, its output discarded; `true` when it succeeded in time.
fn agy_mcp(launch: &Launch, args: &[&str]) -> bool {
    let mut c = launch.command();
    c.arg("mcp")
        .args(args)
        .env(crate::AGENT_CHILD_ENV, "1")
        .stdin(std::process::Stdio::null());
    crate::process::run_bounded(&mut c, AGY_TIMEOUT)
        .is_ok_and(|h| h.is_some_and(|h| h.status.success()))
}

/// Antigravity's registration and allow-rules for one session, withdrawn on drop.
pub(crate) struct Registration {
    launch: Launch,
    run_root: PathBuf,
    mine: Claim,
}

impl Registration {
    /// Claim, register Ondin's server, then grant exactly `allowed`. The claim
    /// goes first and is released last, so no global state ever stands without
    /// a marker accounting for it. `Err` says why, in a clause.
    pub(crate) fn install(
        launch: Launch,
        run_root: &Path,
        exe: &str,
        args: &[String],
        allowed: &[String],
    ) -> Result<Registration, String> {
        let me = Owner::me().ok_or("this process's start time could not be read")?;
        if !may_claim(claim_on_disk(run_root), me, Owner::is_live) {
            return Err("another Ondin is using Antigravity's registration".into());
        }
        let mine = Claim {
            owner: me,
            nonce: next_nonce(),
        };
        let marker = marker_path(run_root);
        write_atomic(&marker, claim_text(mine).as_bytes()).map_err(|e| e.to_string())?;
        // The rename is atomic and the decision was not: read back to see who won.
        if claim_on_disk(run_root) != Some(mine) {
            return Err("another Ondin is using Antigravity's registration".into());
        }
        let reg = Registration {
            launch,
            run_root: run_root.to_path_buf(),
            mine,
        };
        // `--` first: the command's own arguments start with `-`.
        let mut add: Vec<&str> = vec!["add", MCP_SERVER, "--", exe];
        add.extend(args.iter().map(String::as_str));
        if !agy_mcp(&reg.launch, &add) {
            return Err("`agy mcp add` failed".into());
        }
        let rules = antigravity_allow_rules(allowed);
        if !edit_settings(true, |cur| antigravity_settings_with_only(cur, &rules)) {
            return Err("its settings file could not be given Ondin's tool rules".into());
        }
        Ok(reg)
    }
}

impl Drop for Registration {
    fn drop(&mut self) {
        // Only while the claim on disk is still this session's.
        if claim_on_disk(&self.run_root) != Some(self.mine) {
            return;
        }
        edit_settings(false, antigravity_settings_without);
        agy_mcp(&self.launch, &["remove", MCP_SERVER]);
        let _ = std::fs::remove_file(marker_path(&self.run_root));
    }
}

/// Withdraw a dead owner's registration and rules. Nothing at all while a live
/// Ondin holds the claim, and `agy` is started only on a dead owner's claim.
pub(crate) fn sweep(run_root: &Path) {
    let Some(held) = claim_on_disk(run_root) else {
        return;
    };
    if held.owner.is_live() {
        return;
    }
    edit_settings(false, antigravity_settings_without);
    if let Ok(path) = crate::find::locate(crate::Harness::Antigravity, None) {
        agy_mcp(
            &crate::find::launch(crate::Harness::Antigravity, &path),
            &["remove", MCP_SERVER],
        );
    }
    let _ = std::fs::remove_file(marker_path(run_root));
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner(pid: u32) -> Owner {
        Owner { pid, started: 10 }
    }

    /// A claim round-trips, and a garbled one is no claim.
    #[test]
    fn a_claim_round_trips() {
        let c = Claim {
            owner: owner(4),
            nonce: 99,
        };
        assert_eq!(parse_claim(&claim_text(c)), Some(c));
        assert_eq!(parse_claim("4 10"), None);
        assert_eq!(parse_claim("4 10 99 extra"), None);
    }

    /// Another live Ondin's claim is not taken; a dead one's and our own are.
    #[test]
    fn only_a_dead_or_own_claim_is_taken() {
        let theirs = Some(Claim {
            owner: owner(4),
            nonce: 1,
        });
        assert!(!may_claim(theirs, owner(5), |_| true));
        assert!(may_claim(theirs, owner(5), |_| false));
        assert!(may_claim(theirs, owner(4), |_| true));
        assert!(may_claim(None, owner(5), |_| true));
    }

    /// Withdrawing from a file that is not there writes nothing; an unreadable
    /// file is declined either way; granting creates a missing file.
    #[test]
    fn the_settings_plan() {
        assert_eq!(
            plan(false, Existing::Missing, antigravity_settings_without),
            Plan::Holds
        );
        assert_eq!(
            plan(true, Existing::Unreadable, |_| Some(
                SettingsEdit::Unchanged
            )),
            Plan::Declined
        );
        let rules = antigravity_allow_rules(&["get_node".into()]);
        let Plan::Write(text) = plan(true, Existing::Missing, |c| {
            antigravity_settings_with_only(c, &rules)
        }) else {
            panic!("a grant creates the file");
        };
        assert!(text.contains("mcp(ondin/get_node)"));
        assert_eq!(
            plan(
                false,
                Existing::Text("not json".into()),
                antigravity_settings_without
            ),
            Plan::Declined
        );
    }
}
