//! Files asked to open from outside the window, queued until a frame takes them:
//! macOS's open event (`open_event`) and a later launch handing its file to the
//! running window (`one_window`).
//!
//! A request that comes while the window is idle wakes it.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};

/// Paths asked for and not yet taken by a frame, oldest first.
static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
/// A later launch asked for the window to come forward.
static RAISE: AtomicBool = AtomicBool::new(false);
/// The window's context, once it exists, so a request can wake an idle window.
static WAKE: OnceLock<egui::Context> = OnceLock::new();

/// Queue `paths` and wake the window.
pub(crate) fn push(paths: impl IntoIterator<Item = PathBuf>) {
    pending().extend(paths);
    wake();
}

/// Ask for the window to come forward, and wake it.
pub(crate) fn raise() {
    RAISE.store(true, Ordering::Release);
    wake();
}

/// Give the queue the window's context, so a later request wakes it.
pub(crate) fn wake_with(ctx: &egui::Context) {
    let _ = WAKE.set(ctx.clone());
}

/// Take the paths queued so far, oldest first.
pub(crate) fn take() -> Vec<PathBuf> {
    std::mem::take(&mut *pending())
}

/// Take the oldest queued path alone, leaving the rest for a frame.
pub(crate) fn take_first() -> Option<PathBuf> {
    let mut q = pending();
    (!q.is_empty()).then(|| q.remove(0))
}

/// Whether the window was asked to come forward since the last call.
pub(crate) fn take_raise() -> bool {
    RAISE.swap(false, Ordering::AcqRel)
}

fn wake() {
    if let Some(ctx) = WAKE.get() {
        ctx.request_repaint();
    }
}

/// The queue, poisoned or not: a list of paths has no invariant to break.
fn pending() -> std::sync::MutexGuard<'static, Vec<PathBuf>> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A file asked for while a modal is up waits in the queue, and opens on the
    /// first frame without one; a request to come forward is taken at once.
    ///
    /// flip: the frame taking the queue before its modal check → fails at "still
    /// queued". flip: the raise inside the modal check → fails at "focused".
    #[test]
    fn an_outside_open_waits_out_a_modal() {
        let dir = std::env::temp_dir().join(format!("ondin-opens-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&dir);
        let path = dir.join("held.ondin");
        let mut ids = ondin_core::IdSource::new(7);
        let doc = ondin_core::Document::new(ids.mint());
        crate::library::store::write_document(&path, &doc).unwrap();

        let ctx = egui::Context::default();
        let mut app = crate::app::OndinApp::headless(&ctx);
        // The one headless app that takes the queue, so no other test's frame
        // drains it.
        app.os_opens = true;
        app.confirming_close = true;
        pending().push(path.clone());
        raise();
        let out = crate::repeat::tests::pass(&ctx, &mut app, Vec::new());
        let focused = out.viewport_output.values().any(|v| {
            v.commands
                .iter()
                .any(|c| matches!(c, egui::ViewportCommand::Focus))
        });
        assert!(focused, "focused");
        for _ in 0..2 {
            let _ = crate::repeat::tests::pass(&ctx, &mut app, Vec::new());
        }
        assert_eq!(
            pending().as_slice(),
            std::slice::from_ref(&path),
            "still queued"
        );
        assert_ne!(app.session.path.as_deref(), Some(path.as_path()));

        app.confirming_close = false;
        let _ = crate::repeat::tests::pass(&ctx, &mut app, Vec::new());
        assert!(pending().is_empty(), "taken");
        assert_eq!(
            app.session.path.as_deref(),
            Some(path.as_path()),
            "and opened"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
