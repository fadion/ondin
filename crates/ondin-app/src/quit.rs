//! macOS's Quit — ⌘Q from the app menu, the Dock's Quit, logging out — which
//! AppKit carries out through `terminate:` without asking the window to close, so
//! the unsaved-changes question would never be put.
//!
//! winit's application delegate answers no `applicationShouldTerminate:`, so one
//! is added to its class. With unsaved work it cancels the quit and asks the
//! window to close instead, which puts the question; with none the quit goes
//! ahead, and `OndinApp::on_exit` settles the writes as on every exit. winit ends
//! its own loop with `stop:`, never `terminate:`, so the window's close after the
//! question does not come back here.

use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use objc2::runtime::{AnyObject, Imp, Sel};
use objc2::{MainThreadMarker, sel};
use objc2_app_kit::NSApplication;

/// `NSTerminateCancel` and `NSTerminateNow`.
const TERMINATE_CANCEL: usize = 0;
const TERMINATE_NOW: usize = 1;

/// Whether any open document has unsaved changes, as of the last frame.
static UNSAVED: AtomicBool = AtomicBool::new(false);
/// The window's context, to ask it to close.
static WAKE: OnceLock<egui::Context> = OnceLock::new();

/// AppKit's question before a quit: go ahead only when nothing is unsaved.
extern "C-unwind" fn should_terminate(
    _this: &AnyObject,
    _cmd: Sel,
    _sender: *mut AnyObject,
) -> usize {
    answer(UNSAVED.load(Ordering::Relaxed))
}

/// The answer to a quit, given whether work is unsaved; with some, the window
/// is asked to close, which puts the question.
fn answer(unsaved: bool) -> usize {
    if !unsaved {
        return TERMINATE_NOW;
    }
    if let Some(ctx) = WAKE.get() {
        // A quit from the Dock can find the window minimised; the question is
        // put in it, so it comes forward first.
        ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(false));
        ctx.send_viewport_cmd(egui::ViewportCommand::Focus);
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        ctx.request_repaint();
    }
    TERMINATE_CANCEL
}

/// Route a quit through the window's close request. Called once the window
/// exists, since winit sets its delegate when the event loop is made.
pub(crate) fn install(ctx: &egui::Context) {
    let _ = WAKE.set(ctx.clone());
    let Some(mtm) = MainThreadMarker::new() else {
        log::warn!(target: "ondin", "quit: not on the main thread, not installed");
        return;
    };
    let Some(delegate) = NSApplication::sharedApplication(mtm).delegate() else {
        log::warn!(target: "ondin", "quit: the application has no delegate, not installed");
        return;
    };
    let object: &AnyObject = delegate.as_ref();
    let class = object.class();
    let selector = sel!(applicationShouldTerminate:);
    if class.responds_to(selector) {
        log::warn!(target: "ondin", "quit: the delegate already answers {selector}, left alone");
        return;
    }
    // SAFETY: `should_terminate` has the signature `c"Q@:@"` describes, an
    // `NSUInteger` returned for the receiver, its selector and the sender, and
    // AppKit calls it on the main thread.
    let added = unsafe {
        let imp: Imp = std::mem::transmute(
            should_terminate as extern "C-unwind" fn(&AnyObject, Sel, *mut AnyObject) -> usize,
        );
        objc2::ffi::class_addMethod(
            std::ptr::from_ref(class).cast_mut(),
            selector,
            imp,
            c"Q@:@".as_ptr(),
        )
    };
    if !added.as_bool() {
        log::warn!(target: "ondin", "quit: could not add {selector}");
    }
}

/// Record whether a quit now would lose work. Called at the end of every frame.
pub(crate) fn set_unsaved(unsaved: bool) {
    UNSAVED.store(unsaved, Ordering::Relaxed);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A quit goes ahead with nothing unsaved and is called off with unsaved work.
    /// It asks `answer`, not `UNSAVED`, which every test driving a frame writes.
    ///
    /// flip: `answer` returning `TERMINATE_NOW` whatever it is given → fails at
    /// "unsaved work calls the quit off".
    #[test]
    fn a_quit_waits_for_the_question_only_when_work_is_unsaved() {
        assert_eq!(
            answer(false),
            TERMINATE_NOW,
            "nothing unsaved: the quit goes ahead"
        );
        assert_eq!(
            answer(true),
            TERMINATE_CANCEL,
            "unsaved work calls the quit off"
        );
    }
}
