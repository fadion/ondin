//! **Auto-update** (§15 D954): a background check of this repository's GitHub
//! Releases at startup and every [`RECHECK_INTERVAL`] while the app is open; a
//! download of whatever is newer, without asking; and a chip in the top bar that
//! offers the one thing the user is asked — a restart to apply it.
//!
//! Ported from Schemaic's `update` (its `schemaic-core` decisions and its
//! `schemaic-app` Velopack glue), which has shipped this through several
//! releases. **The decisions are the first half of this file and are tested**:
//! whether to check at all ([`check_gate`]), whether to check *again*
//! ([`should_recheck`]), what the chip says ([`UpdateState::label`]) and which
//! progress ticks may move the state ([`UpdateState::with_progress`]). The second
//! half is [`Updater`], the I/O and the thread hops — none of which can be driven
//! without a real install and a real feed.
//!
//! **Velopack's API is synchronous network and file I/O**, so every call into it
//! runs on a worker thread and reports back over a channel the app drains once a
//! frame ([`Updater::poll`]); the worker asks for a repaint when it sends, so an
//! idle window still sees the answer.
//!
//! **Only a Velopack install updates itself**: the Windows `Setup.exe`, the
//! Linux AppImage, the macOS `.pkg` or a `.app` dragged from the `.dmg`. A
//! portable zip, a `.deb` or `.rpm` (which their package manager updates), and a
//! `cargo run` build are not, and Velopack's `UpdateManager::new` fails on
//! exactly those — which is what [`CheckGate::NotInstalled`] is read from.

use std::sync::mpsc::{Receiver, Sender, channel};
use std::time::{Duration, Instant};

use velopack::sources::GithubSource;
use velopack::{UpdateCheck, UpdateManager, VelopackAsset};

/// The release feed. Velopack reads the repository's GitHub Releases through the
/// public API — anonymous, so rate-limited to 60 requests an hour per address.
pub(crate) const RELEASE_REPO: &str = "https://github.com/fadion/ondin";

/// Set to `1`, `true`, `yes` or `on` to stop the app contacting GitHub at all.
///
/// **The updater's only gate.** The *Load web fonts* switch does not govern it:
/// §5.4a's rule that new network traffic answers to that switch is the font
/// source's, and the updater is outside it (§15 D960).
pub(crate) const OPT_OUT_VAR: &str = "ONDIN_NO_UPDATE_CHECK";

/// How long to wait before asking the feed again. A round is two requests — the
/// releases listing and a small manifest — so eight a day is sixteen requests
/// against the anonymous 1,440 a day. Not shorter because nothing is gained:
/// what is being waited for is a person tagging a release.
pub(crate) const RECHECK_INTERVAL: Duration = Duration::from_secs(3 * 60 * 60);

/// A development switch that **must stay `false` in anything committed**: `true`
/// pins the state at [`UpdateState::Ready`] and skips the check, which is the
/// only way to look at the restart chip without two tagged releases. Inert while
/// on — nothing is staged, so a click does nothing — and a test refuses it.
const FORCE_UPDATE_CHIP: bool = false;

// --- the decisions ----------------------------------------------------------

/// Whether a check round may run, and if not, why. Neither refusal is an error,
/// and neither can turn into `Allowed` while the process runs, so either ends the
/// polling ([`should_recheck`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CheckGate {
    Allowed,
    /// [`OPT_OUT_VAR`] is set.
    OptedOut,
    /// Not a Velopack install — see the module doc.
    NotInstalled,
}

/// Decide whether a round runs: `opt_out` is the raw [`OPT_OUT_VAR`] value,
/// `installed` Velopack's own verdict. The opt-out wins over everything.
pub(crate) fn check_gate(opt_out: Option<&str>, installed: bool) -> CheckGate {
    if opt_out_requested(opt_out) {
        CheckGate::OptedOut
    } else if installed {
        CheckGate::Allowed
    } else {
        CheckGate::NotInstalled
    }
}

/// Whether a variable's value means "don't check". Presence alone is not enough —
/// `ONDIN_NO_UPDATE_CHECK=0` should mean what it says — so `1`, `true`, `yes` or
/// `on`, any case, trimmed; anything else, the empty string included, is not.
pub(crate) fn opt_out_requested(raw: Option<&str>) -> bool {
    matches!(
        raw.map(|v| v.trim().to_ascii_lowercase()).as_deref(),
        Some("1" | "true" | "yes" | "on")
    )
}

/// Where the updater has got to. Driven straight through — checking,
/// downloading, ready — with no question before the download: the restart is
/// the one point the user is asked.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum UpdateState {
    #[default]
    Idle,
    /// Asking the feed. Shows nothing.
    Checking,
    /// Fetching the update; `pct` is 0..=100.
    Downloading { version: String, pct: u8 },
    /// Staged, and waiting for a restart to apply.
    Ready { version: String },
    /// The check or the download failed. **Shows nothing**: a background poll
    /// that could not reach GitHub is not the user's problem, the log says why,
    /// and the next round retries.
    Failed { message: String },
}

impl UpdateState {
    /// The chip's text, or `None` for a state that shows nothing.
    pub(crate) fn label(&self) -> Option<String> {
        match self {
            Self::Downloading { pct, .. } => Some(format!("Updating… {pct}%")),
            Self::Ready { .. } => Some("Restart to update".to_owned()),
            Self::Idle | Self::Checking | Self::Failed { .. } => None,
        }
    }

    /// Whether a click on the chip does something: only once an update is
    /// staged, when it means *restart and apply*.
    pub(crate) fn is_actionable(&self) -> bool {
        matches!(self, Self::Ready { .. })
    }

    /// Fold in a progress tick. **Ignored unless a download is in flight**: the
    /// channel outlives the download, and a late tick must not turn the restart
    /// offer the user is about to click back into "Updating… 100%".
    pub(crate) fn with_progress(&self, raw: i16) -> Self {
        match self {
            Self::Downloading { version, .. } => Self::Downloading {
                version: version.clone(),
                pct: raw.clamp(0, 100) as u8,
            },
            other => other.clone(),
        }
    }
}

/// Whether another round is worth arming after one settled: only while the gate
/// allows and the outcome could still change. A staged update ends the polling —
/// re-checking could only disturb the offer on screen — and so does a refused
/// gate; a **failed** check re-arms, being the one outcome expected to pass.
pub(crate) fn should_recheck(gate: CheckGate, settled: &UpdateState) -> bool {
    gate == CheckGate::Allowed && matches!(settled, UpdateState::Idle | UpdateState::Failed { .. })
}

// --- the I/O ----------------------------------------------------------------

/// What a worker sends back to the frame.
enum Msg {
    /// A download began, of this version.
    Begin(String),
    /// A download progress tick, as Velopack sends it.
    Progress(i16),
    /// A round settled.
    Settled(CheckGate, Result<Option<VelopackAsset>, String>),
}

/// What a click on the chip did ([`Updater::restart`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Restart {
    /// Nothing is staged, or nothing can apply it.
    Nothing,
    /// The window was asked to close, and the update applies once it has.
    Closing,
    /// This many other Ondin processes are running, and applying would stop
    /// them; nothing was done (§15 D970).
    OthersOpen(usize),
}

/// The background updater, owned by the app and polled once a frame.
pub(crate) struct Updater {
    state: UpdateState,
    /// The asset a round staged, applied by [`Self::on_exit`] once the chip
    /// has asked for it.
    staged: Option<VelopackAsset>,
    /// Whether the chip asked for a restart and the window's close has not
    /// been called off since. Read once, by [`Self::on_exit`].
    ///
    /// 🚨 **A flag, and Velopack's updater is not launched until the close has
    /// happened** (§15 D970). It used to be launched at the click: the updater
    /// waits sixty seconds for this process and then applies regardless,
    /// ending every Ondin process on Windows — so a user who answered the
    /// unsaved-work card *Cancel*, or took over a minute to answer it, had the
    /// window killed under them, and the cancelled close this field's
    /// predecessor (`handed_over`) was built to survive was the one it could
    /// not.
    restart_on_exit: bool,
    /// `None` for an updater that never checks — a headless app's.
    live: Option<Live>,
}

struct Live {
    ctx: egui::Context,
    tx: Sender<Msg>,
    rx: Receiver<Msg>,
    /// Velopack's progress channel, forwarded into `tx` by a thread built once
    /// for the process; each round hands Velopack a clone.
    progress: Sender<i16>,
    /// When the next round is due, or `None` while one is in flight or the
    /// polling has ended.
    next: Option<Instant>,
}

impl Default for Updater {
    /// An updater that never checks — what a headless app and a test get.
    fn default() -> Self {
        Self {
            state: UpdateState::Idle,
            staged: None,
            restart_on_exit: false,
            live: None,
        }
    }
}

/// An `UpdateManager` for the release feed. Fails when this is not a Velopack
/// install, which is the ordinary case during development.
fn manager() -> Result<UpdateManager, velopack::Error> {
    UpdateManager::new(GithubSource::new(RELEASE_REPO, None, false), None, None)
}

impl Updater {
    /// Start checking: the first round now, the next ones on their interval.
    pub(crate) fn start(ctx: &egui::Context) -> Self {
        if FORCE_UPDATE_CHIP {
            return Self {
                state: UpdateState::Ready {
                    version: "0.0.0".to_owned(),
                },
                ..Self::default()
            };
        }
        let (tx, rx) = channel();
        let (progress, ticks) = channel::<i16>();
        let forward = (tx.clone(), ctx.clone());
        std::thread::Builder::new()
            .name("ondin-update-progress".to_owned())
            .spawn(move || {
                while let Ok(pct) = ticks.recv() {
                    if forward.0.send(Msg::Progress(pct)).is_err() {
                        break;
                    }
                    forward.1.request_repaint();
                }
            })
            .ok();
        Self {
            state: UpdateState::Idle,
            staged: None,
            restart_on_exit: false,
            live: Some(Live {
                ctx: ctx.clone(),
                tx,
                rx,
                progress,
                next: Some(Instant::now()),
            }),
        }
    }

    pub(crate) fn state(&self) -> &UpdateState {
        &self.state
    }

    /// An updater holding a staged update and nothing behind it — what
    /// `FORCE_UPDATE_CHIP` does, for a test that needs the chip drawn. Inert:
    /// nothing is staged, so a click applies nothing. Plain backticks: this
    /// item is `cfg(test)`, so the doc gate cannot see it (§15 D319).
    #[cfg(test)]
    pub(crate) fn ready(version: &str) -> Self {
        Self {
            state: UpdateState::Ready {
                version: version.to_owned(),
            },
            ..Self::default()
        }
    }

    /// Take what the workers sent and start a round that is due. Once a frame.
    pub(crate) fn poll(&mut self) {
        let Some(live) = &mut self.live else {
            return;
        };
        let msgs: Vec<Msg> = live.rx.try_iter().collect();
        for msg in msgs {
            match msg {
                Msg::Begin(version) => self.state = UpdateState::Downloading { version, pct: 0 },
                Msg::Progress(pct) => self.state = self.state.with_progress(pct),
                Msg::Settled(gate, res) => {
                    self.state = match res {
                        Ok(Some(asset)) => {
                            log::info!(target: "ondin", "update {} staged", asset.Version);
                            let version = asset.Version.clone();
                            self.staged = Some(asset);
                            UpdateState::Ready { version }
                        }
                        Ok(None) => UpdateState::Idle,
                        Err(e) => {
                            log::warn!(target: "ondin", "update check failed: {e}");
                            UpdateState::Failed { message: e }
                        }
                    };
                    if should_recheck(gate, &self.state) {
                        live.next = Some(Instant::now() + RECHECK_INTERVAL);
                    }
                }
            }
        }
        let now = Instant::now();
        match live.next {
            Some(at) if now >= at => {
                live.next = None;
                self.state = UpdateState::Checking;
                spawn_check(live);
            }
            // ⚠️ **Asked for again on every pass, not once when the round is
            // armed** (`[X1-L1-03]`). eframe keeps one "repaint at" per
            // viewport and the next input's immediate repaint overwrites it, so
            // a wake requested once was erased by the first mouse move and a
            // window then left alone never checked again until touched. egui
            // keeps the earliest of a pass's requests, so this costs nothing
            // when something sooner is due.
            Some(at) => live.ctx.request_repaint_after(at - now),
            None => {}
        }
    }

    /// The chip's click: ask the window to close, and apply the staged update
    /// once it has ([`Self::on_exit`]).
    ///
    /// **A close, not Velopack's restart-now**, which exits the process on the
    /// spot and would skip the close request — the unsaved-work question and
    /// the recovery snapshot. A close the user calls off is
    /// [`Self::restart_called_off`], and a later click asks again.
    ///
    /// **Refused while another Ondin is running** (§15 D970): the updater would
    /// stop it too — see [`crate::instances`]. The caller says so.
    pub(crate) fn restart(&mut self) -> Restart {
        self.restart_with(crate::instances::others)
    }

    /// [`Self::restart`] with the count of other processes given, so a test
    /// can ask for either answer.
    fn restart_with(&mut self, others: impl FnOnce() -> usize) -> Restart {
        let (Some(live), Some(_)) = (&self.live, &self.staged) else {
            return Restart::Nothing;
        };
        let others = others();
        if others > 0 {
            return Restart::OthersOpen(others);
        }
        self.restart_on_exit = true;
        live.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        Restart::Closing
    }

    /// The close a restart asked for was answered *Cancel*: nothing applies
    /// when the window does close, later, for some other reason.
    pub(crate) fn restart_called_off(&mut self) {
        self.restart_on_exit = false;
    }

    /// The window is going: hand the staged update to Velopack's updater if the
    /// chip asked for it, so it waits for this process, applies, and relaunches.
    ///
    /// **From `eframe::App::on_exit`, after `disk_settle`**, which is the one
    /// point the close can no longer be called off — so the updater's sixty
    /// seconds of waiting cover only eframe's teardown, never a question still
    /// on screen. And the count of other processes is taken **again**: a
    /// window opened since the click would be stopped as surely as one open
    /// at it, and then the update simply waits for the next restart.
    pub(crate) fn on_exit(&mut self) {
        self.on_exit_with(crate::instances::others, |asset| {
            manager()
                .and_then(|um| {
                    // `silent = false` so a failure is visible; `restart = true`
                    // so the user lands back where they were.
                    um.wait_exit_then_apply_updates(asset, false, true, Vec::<String>::new())
                })
                .map_err(|e| e.to_string())
        });
    }

    /// [`Self::on_exit`] with the count and the launch given, so a test can see
    /// whether it would launch without launching anything.
    fn on_exit_with(
        &mut self,
        others: impl FnOnce() -> usize,
        launch: impl FnOnce(&VelopackAsset) -> Result<(), String>,
    ) {
        if !std::mem::take(&mut self.restart_on_exit) {
            return;
        }
        let Some(asset) = self.staged.take() else {
            return;
        };
        let running = others();
        if running > 0 {
            log::warn!(target: "ondin", "update {} not applied: {running} other Ondin running", asset.Version);
            return;
        }
        match launch(&asset) {
            Ok(()) => log::info!(target: "ondin", "update {} handed to the updater", asset.Version),
            // Nothing took the staged files, so the next launch's check finds
            // the same update already downloaded and offers it again.
            Err(e) => log::error!(target: "ondin", "could not launch the updater: {e}"),
        }
    }
}

/// **The top bar's update chip**: a small outlined pill — a clockwise arrow and
/// the state's [`UpdateState::label`] in capitals — or nothing at all for a state
/// with no label. Clickable only when an update is staged, and then it is the
/// restart. Returns whether it was clicked so; the caller applies.
///
/// Both screens draw it, in the `right_to_left` cluster beside Settings, so an
/// update staged while the library is open is offered there too.
pub(crate) fn chip(ui: &mut egui::Ui, state: &UpdateState) -> bool {
    use crate::theme::{self, color, icon};
    let Some(label) = state.label() else {
        return false;
    };
    let actionable = state.is_actionable();
    let galley = |ui: &egui::Ui, color| {
        let mut job = egui::text::LayoutJob::default();
        job.append(
            icon::ARROW_CLOCKWISE,
            0.0,
            egui::TextFormat::simple(theme::icon_font(13.0), color),
        );
        job.append(
            &label.to_uppercase(),
            5.0,
            egui::TextFormat::simple(egui::FontId::proportional(11.0), color),
        );
        ui.fonts_mut(|f| f.layout_job(job))
    };
    let size = galley(ui, color::TEXT).size() + egui::vec2(18.0, 6.0);
    let sense = if actionable {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, resp) = ui.allocate_exact_size(size, sense);
    let lit = actionable && resp.hovered();
    let fg = if lit { color::TEXT } else { theme::text::MUTED };
    let p = ui.painter();
    p.rect_stroke(
        rect,
        egui::CornerRadius::same(5),
        egui::Stroke::new(1.0, fg),
        egui::StrokeKind::Inside,
    );
    let g = galley(ui, fg);
    p.galley(rect.center() - g.size() / 2.0, g, fg);
    let resp = match state {
        UpdateState::Ready { version } => {
            resp.on_hover_text(format!("Restart Ondin to install version {version}"))
        }
        _ => resp,
    };
    actionable && resp.clicked()
}

/// One round on a worker thread: gate, check, download, report.
fn spawn_check(live: &Live) {
    let (tx, ctx, progress) = (live.tx.clone(), live.ctx.clone(), live.progress.clone());
    let opt_out = std::env::var(OPT_OUT_VAR).ok();
    std::thread::Builder::new()
        .name("ondin-update-check".to_owned())
        .spawn(move || {
            let send = |msg| {
                let _ = tx.send(msg);
                ctx.request_repaint();
            };
            let um = manager();
            let gate = check_gate(opt_out.as_deref(), um.is_ok());
            let um = match (gate, um) {
                (CheckGate::Allowed, Ok(um)) => um,
                _ => {
                    log::info!(target: "ondin", "no update check: {gate:?}");
                    return send(Msg::Settled(gate, Ok(None)));
                }
            };
            let info = match um.check_for_updates() {
                Ok(UpdateCheck::UpdateAvailable(info)) => info,
                Ok(UpdateCheck::NoUpdateAvailable | UpdateCheck::RemoteIsEmpty) => {
                    return send(Msg::Settled(gate, Ok(None)));
                }
                Err(e) => return send(Msg::Settled(gate, Err(e.to_string()))),
            };
            // A downgrade — the feed's newest is older than what runs, a yanked
            // release or a build ahead of its tag — is never walked into.
            if info.IsDowngrade {
                return send(Msg::Settled(gate, Ok(None)));
            }
            let asset = info.TargetFullRelease.clone();
            send(Msg::Begin(asset.Version.clone()));
            let res = um
                .download_updates(&info, Some(progress))
                .map(|()| Some(asset));
            send(Msg::Settled(gate, res.map_err(|e| e.to_string())));
        })
        .ok();
}

#[cfg(test)]
mod tests {
    //! Plain backticks: a `cfg(test)` module is checked by nothing (§15 D319).
    use super::*;

    #[test]
    fn the_opt_out_takes_only_the_spellings_that_mean_it() {
        for yes in ["1", "true", "TRUE", " yes ", "on"] {
            assert!(opt_out_requested(Some(yes)), "{yes:?}");
        }
        for no in ["", "0", "false", "no", "off", "2"] {
            assert!(!opt_out_requested(Some(no)), "{no:?}");
        }
        assert!(!opt_out_requested(None));
    }

    /// The opt-out wins over an install; an install is what allows a check.
    #[test]
    fn the_gate_puts_the_opt_out_first() {
        assert_eq!(check_gate(Some("1"), true), CheckGate::OptedOut);
        assert_eq!(check_gate(None, true), CheckGate::Allowed);
        assert_eq!(check_gate(Some("0"), false), CheckGate::NotInstalled);
    }

    /// Only a download and a staged update show anything; only a staged one
    /// is clickable.
    #[test]
    fn the_chip_speaks_only_for_a_download_or_a_staged_update() {
        let v = || "0.4.0".to_owned();
        assert_eq!(
            UpdateState::Downloading {
                version: v(),
                pct: 42
            }
            .label()
            .as_deref(),
            Some("Updating… 42%")
        );
        assert_eq!(
            UpdateState::Ready { version: v() }.label().as_deref(),
            Some("Restart to update")
        );
        for silent in [
            UpdateState::Idle,
            UpdateState::Checking,
            UpdateState::Failed {
                message: "offline".into(),
            },
        ] {
            assert_eq!(silent.label(), None, "{silent:?}");
            assert!(!silent.is_actionable());
        }
        assert!(UpdateState::Ready { version: v() }.is_actionable());
        assert!(
            !UpdateState::Downloading {
                version: v(),
                pct: 1
            }
            .is_actionable()
        );
    }

    /// A tick moves a download, clamped to 0..=100, and cannot rewind a staged
    /// update into a download. **Flip run**, `with_progress` applying to every
    /// state: fails on *"a late tick cannot rewind a staged update"*, the
    /// predicted site.
    #[test]
    fn a_late_tick_cannot_rewind_a_staged_update() {
        let down = UpdateState::Downloading {
            version: "0.4.0".into(),
            pct: 0,
        };
        assert_eq!(
            down.with_progress(150),
            UpdateState::Downloading {
                version: "0.4.0".into(),
                pct: 100
            }
        );
        assert_eq!(
            down.with_progress(-1),
            UpdateState::Downloading {
                version: "0.4.0".into(),
                pct: 0
            }
        );
        let ready = UpdateState::Ready {
            version: "0.4.0".into(),
        };
        assert_eq!(
            ready.with_progress(100),
            ready,
            "a late tick cannot rewind a staged update"
        );
    }

    /// Polling goes on through nothing found and through a failure, and stops
    /// for good on a staged update or a refused gate.
    #[test]
    fn rechecks_continue_only_while_they_can_change_the_answer() {
        use CheckGate::*;
        let failed = UpdateState::Failed {
            message: "x".into(),
        };
        let ready = UpdateState::Ready {
            version: "1".into(),
        };
        assert!(should_recheck(Allowed, &UpdateState::Idle));
        assert!(should_recheck(Allowed, &failed));
        assert!(!should_recheck(Allowed, &ready));
        assert!(!should_recheck(OptedOut, &UpdateState::Idle));
        assert!(!should_recheck(NotInstalled, &failed));
    }

    /// **The dev switch, shipped**, would offer every user a restart that does
    /// nothing, with every other gate green.
    #[test]
    #[allow(clippy::assertions_on_constants)]
    fn the_forced_update_chip_is_off_in_a_committed_tree() {
        assert!(
            !FORCE_UPDATE_CHIP,
            "flip FORCE_UPDATE_CHIP back before committing"
        );
    }

    /// **The Release workflow packs with the Velopack CLI the client was built
    /// against** (§15 D974, `[X3-L5-03]`): `vpk` is pinned in `release.yml`,
    /// and a `velopack` bump in `Cargo.lock` without the pin moving would pack
    /// with a CLI the client was not built for. Read from both files, so the
    /// mismatch fails here rather than in a release.
    ///
    /// **Flip-check, run**: the workflow's `--version 1.2.161` edited to
    /// `1.2.160` fails with both versions named.
    #[test]
    fn the_release_packs_with_the_clients_own_velopack_version() {
        let lock = include_str!("../../../Cargo.lock");
        let client = lock
            .split("[[package]]")
            .find(|p| p.contains("\nname = \"velopack\"\n"))
            .and_then(|p| p.lines().find_map(|l| l.strip_prefix("version = \"")))
            .and_then(|v| v.strip_suffix('"'))
            .expect("Cargo.lock has a velopack package");
        let workflow = include_str!("../../../.github/workflows/release.yml");
        let pinned = workflow
            .lines()
            .find_map(|l| {
                l.trim()
                    .strip_prefix("run: dotnet tool install -g vpk --version ")
            })
            .expect("release.yml installs vpk at a pinned version");
        assert_eq!(
            pinned.trim(),
            client,
            "vpk {pinned} packs for velopack {client}"
        );
    }

    /// The feed's identity and the published opt-out's name: a wrong repository
    /// would check someone else's releases, and a renamed variable silently
    /// breaks everyone who set it.
    #[test]
    fn the_feed_and_the_opt_out_keep_their_published_names() {
        assert_eq!(RELEASE_REPO, "https://github.com/fadion/ondin");
        assert_eq!(OPT_OUT_VAR, "ONDIN_NO_UPDATE_CHECK");
        let per_hour = 2.0 * 3600.0 / RECHECK_INTERVAL.as_secs_f64();
        assert!(RECHECK_INTERVAL.as_secs() > 0 && per_hour < 6.0);
    }

    /// **The chip draws only for a state with a label, and only a staged update
    /// is a button**: a click on "Restart to update" reports a click, the same
    /// click on "Updating… 40%" does not, and an idle updater allocates nothing.
    ///
    /// **Flip run**, the chip clickable in every state it draws (`actionable`
    /// read as `true`): fails on *"a download in progress is no button"* — the
    /// predicted site.
    #[test]
    fn only_a_staged_update_makes_the_chip_a_button() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let run = |state: &UpdateState, events: Vec<egui::Event>| {
            let mut clicked = false;
            let mut used = egui::Rect::NOTHING;
            let _ = ctx.run_ui(
                egui::RawInput {
                    events,
                    time: Some(ctx.input(|i| i.time) + 0.1),
                    ..Default::default()
                },
                |ui| {
                    clicked = chip(ui, state);
                    used = ui.min_rect();
                },
            );
            (clicked, used)
        };
        let at = egui::pos2(20.0, 8.0);
        let click = |state: &UpdateState| {
            let _ = run(state, vec![egui::Event::PointerMoved(at)]);
            let press = |down| egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: down,
                modifiers: Default::default(),
            };
            let a = run(state, vec![press(true)]).0;
            let b = run(state, vec![press(false)]).0;
            a || b
        };
        assert!(
            click(&UpdateState::Ready {
                version: "0.4.0".into()
            }),
            "a staged update is the restart"
        );
        assert!(
            !click(&UpdateState::Downloading {
                version: "0.4.0".into(),
                pct: 40
            }),
            "a download in progress is no button"
        );
        let (_, idle) = run(&UpdateState::Idle, Vec::new());
        assert!(idle.width() <= 0.0, "nothing drawn while idle: {idle:?}");
    }

    /// A live updater over a channel the test holds the sending end of.
    fn live(ctx: &egui::Context) -> (Updater, Sender<Msg>) {
        let (tx, rx) = channel();
        let (progress, _ticks) = channel();
        let u = Updater {
            state: UpdateState::Idle,
            staged: None,
            restart_on_exit: false,
            live: Some(Live {
                ctx: ctx.clone(),
                tx: tx.clone(),
                rx,
                progress,
                next: None,
            }),
        };
        (u, tx)
    }

    fn asset(version: &str) -> VelopackAsset {
        VelopackAsset {
            Version: version.into(),
            ..Default::default()
        }
    }

    fn closes(out: &egui::FullOutput) -> usize {
        out.viewport_output
            .get(&egui::ViewportId::ROOT)
            .map_or(0, |v| {
                v.commands
                    .iter()
                    .filter(|c| matches!(c, egui::ViewportCommand::Close))
                    .count()
            })
    }

    /// **The `Settled` arm stages what a click will apply, and re-arms only
    /// while the answer can change** (`[X1-L6-01]`): a found update is `Ready`
    /// *and* staged, with no round after it; nothing found and a failure each
    /// arm the next round.
    ///
    /// **Flip-check, run**: deleting `self.staged = Some(asset)` fails on
    /// *"a found update is what a click applies"* — the chip it leaves says
    /// *Restart to update* and does nothing, which was the finding; deleting
    /// the re-arm fails on *"nothing found arms the next round"*.
    #[test]
    fn a_settled_round_stages_the_update_and_rearms_only_while_it_can_change() {
        let ctx = egui::Context::default();
        let (mut u, tx) = live(&ctx);
        let next = |u: &Updater| u.live.as_ref().unwrap().next;
        // What `poll` does as a round starts, which is what a settle answers.
        let start_round = |u: &mut Updater| u.live.as_mut().unwrap().next = None;

        tx.send(Msg::Settled(CheckGate::Allowed, Ok(None))).unwrap();
        let _ = ctx.run_ui(Default::default(), |_| u.poll());
        assert_eq!(u.state(), &UpdateState::Idle);
        assert!(next(&u).is_some(), "nothing found arms the next round");

        start_round(&mut u);
        tx.send(Msg::Settled(CheckGate::Allowed, Err("offline".into())))
            .unwrap();
        let _ = ctx.run_ui(Default::default(), |_| u.poll());
        assert!(matches!(u.state(), UpdateState::Failed { .. }));
        assert!(next(&u).is_some(), "a failure arms the next round");

        start_round(&mut u);
        tx.send(Msg::Settled(CheckGate::Allowed, Ok(Some(asset("0.5.0")))))
            .unwrap();
        let _ = ctx.run_ui(Default::default(), |_| u.poll());
        assert_eq!(
            u.state(),
            &UpdateState::Ready {
                version: "0.5.0".into()
            }
        );
        assert_eq!(
            u.staged.as_ref().map(|a| a.Version.as_str()),
            Some("0.5.0"),
            "a found update is what a click applies"
        );
        assert_eq!(next(&u), None, "a staged update ends the polling");
    }

    /// **The recheck's wake is asked for on every pass** (`[X1-L1-03]`): eframe
    /// keeps one repaint deadline and an input's repaint overwrites it, so a
    /// wake requested once, when the round was armed, was gone at the first
    /// mouse move.
    ///
    /// **Flip-check, run**: the `Some(at) =>` arm of `poll` emptied leaves no
    /// request at all, and this fails on **pass 1** with `repaint_delay` at
    /// `Duration::MAX` — pass 0 is a fresh context's first frame, which egui
    /// repaints whatever is asked, so it is the second pass that has teeth.
    #[test]
    fn the_recheck_wake_is_asked_for_on_every_pass() {
        let ctx = egui::Context::default();
        let (mut u, _tx) = live(&ctx);
        u.live.as_mut().unwrap().next = Some(Instant::now() + RECHECK_INTERVAL);
        for pass in 0..3 {
            let out = ctx.run_ui(Default::default(), |_| u.poll());
            let delay = out.viewport_output[&egui::ViewportId::ROOT].repaint_delay;
            assert!(
                delay <= RECHECK_INTERVAL,
                "pass {pass}: the next repaint is {delay:?} away"
            );
        }
    }

    /// 🚨 **A restart launches nothing until the window has gone, and a
    /// cancelled close launches nothing at all** (§15 D970, `[R3-L5-02]`). The
    /// click used to launch Velopack's updater at once, which waits sixty
    /// seconds and then applies regardless — on Windows by killing every Ondin
    /// process, the window whose close was just answered *Cancel* included.
    ///
    /// **Flip-check, run**: `restart_called_off` emptied fails on *"a cancelled
    /// close applies nothing"* with one launch; `on_exit_with`'s
    /// `restart_on_exit` check removed fails the same assertion.
    #[test]
    fn a_restart_applies_at_the_exit_and_a_cancelled_close_applies_nothing() {
        let ctx = egui::Context::default();
        let (mut u, _tx) = live(&ctx);
        u.state = UpdateState::Ready {
            version: "0.5.0".into(),
        };
        u.staged = Some(asset("0.5.0"));
        let mut launched = Vec::new();

        let mut said = Restart::Nothing;
        let out = ctx.run_ui(Default::default(), |_| said = u.restart_with(|| 0));
        assert_eq!(said, Restart::Closing);
        assert_eq!(closes(&out), 1, "the click asks the window to close");

        // The unsaved-work card, answered *Cancel*; the window closes later for
        // an unrelated reason.
        u.restart_called_off();
        u.on_exit_with(
            || 0,
            |a| {
                launched.push(a.Version.clone());
                Ok(())
            },
        );
        assert!(launched.is_empty(), "a cancelled close applies nothing");
        assert!(u.staged.is_some(), "and the offer stands");

        let _ = ctx.run_ui(Default::default(), |_| said = u.restart_with(|| 0));
        assert_eq!(said, Restart::Closing, "a second click asks again");
        u.on_exit_with(
            || 0,
            |a| {
                launched.push(a.Version.clone());
                Ok(())
            },
        );
        assert_eq!(launched, ["0.5.0"], "a close that went through applies");
    }

    /// 🚨 **Another Ondin running holds the update back, at the click and again
    /// at the exit** (§15 D970, `[R1-L2-01]`): the updater stops every process
    /// running from the install, and a second window's unsaved work was never
    /// asked about.
    ///
    /// **Flip-check, run**: `restart_with` ignoring the count fails on *"the
    /// click is refused"*; `on_exit_with` ignoring it fails on *"a window opened
    /// since the click holds it back too"*.
    #[test]
    fn another_ondin_running_holds_the_update_back() {
        let ctx = egui::Context::default();
        let (mut u, _tx) = live(&ctx);
        u.staged = Some(asset("0.5.0"));
        let mut said = Restart::Nothing;
        let out = ctx.run_ui(Default::default(), |_| said = u.restart_with(|| 2));
        assert_eq!(said, Restart::OthersOpen(2), "the click is refused");
        assert_eq!(closes(&out), 0, "and the window stays");

        let _ = ctx.run_ui(Default::default(), |_| said = u.restart_with(|| 0));
        assert_eq!(said, Restart::Closing);
        let mut launched = 0;
        u.on_exit_with(
            || 1,
            |_| {
                launched += 1;
                Ok(())
            },
        );
        assert_eq!(
            launched, 0,
            "a window opened since the click holds it back too"
        );
    }

    /// A headless app's updater never checks and never stages anything, so a
    /// test can poll it every frame and see nothing — and its restart asks
    /// nobody how many processes are running.
    #[test]
    fn the_default_updater_is_inert() {
        let mut u = Updater::default();
        u.poll();
        assert_eq!(u.restart_with(|| panic!("counted")), Restart::Nothing);
        u.on_exit();
        assert_eq!(u.state(), &UpdateState::Idle);
    }
}
