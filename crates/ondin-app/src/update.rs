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
    /// The updater was launched — or was not, and this asset goes back up.
    Handover(Result<(), String>, VelopackAsset),
}

/// The background updater, owned by the app and polled once a frame.
pub(crate) struct Updater {
    state: UpdateState,
    /// The asset a round staged, taken by a click on the chip.
    staged: Option<VelopackAsset>,
    /// Whether Velopack's updater has been launched and is waiting for this
    /// process to exit. A close the user then cancels — the unsaved-work
    /// question answered *Cancel* — leaves the offer standing, and the next
    /// click must close again rather than do nothing (§15 D954's amendment).
    handed_over: bool,
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
            handed_over: false,
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
            handed_over: false,
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
                        live.ctx.request_repaint_after(RECHECK_INTERVAL);
                    }
                }
                Msg::Handover(Ok(()), _) => {
                    // **A close, not Velopack's restart-now**, which exits the
                    // process on the spot and would skip the close request — the
                    // unsaved-work question and the recovery snapshot. The
                    // updater just launched is waiting for this process to go.
                    self.handed_over = true;
                    live.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
                Msg::Handover(Err(e), asset) => {
                    // **The offer goes back up**: nothing took the staged files,
                    // so a retry is as good as the first try — and `Failed`
                    // would make a click the user did make remove the update.
                    log::error!(target: "ondin", "could not launch the updater: {e}");
                    self.state = UpdateState::Ready {
                        version: asset.Version.clone(),
                    };
                    self.staged = Some(asset);
                }
            }
        }
        if live.next.is_some_and(|at| Instant::now() >= at) {
            live.next = None;
            self.state = UpdateState::Checking;
            spawn_check(live);
        }
    }

    /// The chip's click: hand the staged update to Velopack's updater, which
    /// waits for this process to exit, applies it and relaunches. **Taken, not
    /// cloned** — a second click while the first is handing over is then a
    /// no-op rather than a second updater racing the first for the same files.
    pub(crate) fn apply(&mut self) {
        // Handed over already and the close was cancelled: the updater is still
        // waiting, so the restart is only the close again.
        if let (Some(live), true) = (&self.live, self.handed_over) {
            live.ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            return;
        }
        let (Some(live), Some(asset)) = (&self.live, self.staged.take()) else {
            return;
        };
        let (tx, ctx) = (live.tx.clone(), live.ctx.clone());
        std::thread::Builder::new()
            .name("ondin-update-apply".to_owned())
            .spawn(move || {
                // `silent = false` so a failure is visible; `restart = true` so
                // the user lands back where they were.
                let launched = manager().map_err(|e| e.to_string()).and_then(|um| {
                    um.wait_exit_then_apply_updates(&asset, false, true, Vec::<String>::new())
                        .map_err(|e| e.to_string())
                });
                let _ = tx.send(Msg::Handover(launched, asset));
                ctx.request_repaint();
            })
            .ok();
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

    /// **After the handover, a cancelled close leaves the chip working**: the
    /// updater launched closes the window, the user cancels at the unsaved-work
    /// question, the chip still offers the restart — and a click on it closes
    /// again, rather than finding nothing staged and doing nothing while
    /// Velopack's updater waits on (§15 D954's amendment, found reading `apply`).
    ///
    /// **Flip run**, the `handed_over` arm of `apply` removed: fails on *"the
    /// second click closes again"*, 0 closes — the predicted site.
    #[test]
    fn a_cancelled_close_after_the_handover_leaves_the_restart_working() {
        let ctx = egui::Context::default();
        let (tx, rx) = channel();
        let (progress, _ticks) = channel();
        let mut u = Updater {
            state: UpdateState::Ready {
                version: "0.4.0".into(),
            },
            staged: None,
            handed_over: false,
            live: Some(Live {
                ctx: ctx.clone(),
                tx: tx.clone(),
                rx,
                progress,
                next: None,
            }),
        };
        let closes = |out: egui::FullOutput| {
            out.viewport_output
                .get(&egui::ViewportId::ROOT)
                .map_or(0, |v| {
                    v.commands
                        .iter()
                        .filter(|c| matches!(c, egui::ViewportCommand::Close))
                        .count()
                })
        };
        tx.send(Msg::Handover(Ok(()), VelopackAsset::default()))
            .unwrap();
        let first = closes(ctx.run_ui(Default::default(), |_| u.poll()));
        assert_eq!(first, 1, "the handover closes the window");
        assert!(
            u.state().is_actionable(),
            "a cancelled close leaves the offer up"
        );
        let again = closes(ctx.run_ui(Default::default(), |_| u.apply()));
        assert_eq!(again, 1, "the second click closes again");
    }

    /// A headless app's updater never checks and never stages anything, so a
    /// test can poll it every frame and see nothing.
    #[test]
    fn the_default_updater_is_inert() {
        let mut u = Updater::default();
        u.poll();
        u.apply();
        assert_eq!(u.state(), &UpdateState::Idle);
    }
}
