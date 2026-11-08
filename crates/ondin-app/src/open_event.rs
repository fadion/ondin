//! macOS's request to open documents — a double-click in Finder, a file dropped on
//! the Dock icon, `open a.ondin` — which arrives as an `odoc` Apple Event that
//! winit 0.30 does not pass on.
//!
//! The handler is installed when `NSApplicationWillFinishLaunchingNotification` is
//! posted: AppKit installs its own handlers before that, so one installed there
//! wins, and a cold launch's event comes after it. The paths queue here until a
//! frame takes them; one that comes while the window is idle wakes it.

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use objc2::rc::Retained;
use objc2::runtime::{AnyObject, NSObject};
use objc2::{MainThreadMarker, MainThreadOnly, define_class, msg_send, sel};
use objc2_app_kit::NSApplicationWillFinishLaunchingNotification;
use objc2_foundation::{
    NSAppleEventDescriptor, NSAppleEventManager, NSNotification, NSNotificationCenter,
};

/// `kCoreEventClass` and `kAEOpenDocuments`, `'aevt'` and `'odoc'`.
const CORE_EVENT_CLASS: u32 = u32::from_be_bytes(*b"aevt");
const OPEN_DOCUMENTS: u32 = u32::from_be_bytes(*b"odoc");
/// `keyDirectObject`, `'----'`: the event's list of files.
const DIRECT_OBJECT: u32 = u32::from_be_bytes(*b"----");

/// Paths opened by the OS and not yet taken by a frame, oldest first.
static PENDING: Mutex<Vec<PathBuf>> = Mutex::new(Vec::new());
/// The window's context, once it exists, so an event can wake an idle window.
static WAKE: OnceLock<egui::Context> = OnceLock::new();

define_class!(
    // SAFETY: `NSObject` has no subclassing requirements, and `Handler` has no `Drop`.
    #[unsafe(super(NSObject))]
    #[thread_kind = MainThreadOnly]
    #[name = "OndinOpenEvents"]
    struct Handler;

    impl Handler {
        #[unsafe(method(willFinishLaunching:))]
        fn will_finish_launching(&self, _note: &NSNotification) {
            let events = NSAppleEventManager::sharedAppleEventManager();
            // SAFETY: `self` is leaked, so it outlives the registration, and it
            // answers the selector with the signature the manager calls.
            unsafe {
                let _: () = msg_send![
                    &events,
                    setEventHandler: self,
                    andSelector: sel!(openDocuments:withReplyEvent:),
                    forEventClass: CORE_EVENT_CLASS,
                    andEventID: OPEN_DOCUMENTS
                ];
            }
        }

        #[unsafe(method(openDocuments:withReplyEvent:))]
        fn open_documents(&self, event: &NSAppleEventDescriptor, _reply: &NSAppleEventDescriptor) {
            // SAFETY: `paramDescriptorForKeyword:` takes an `AEKeyword`, a `u32`.
            let files: Option<Retained<NSAppleEventDescriptor>> =
                unsafe { msg_send![event, paramDescriptorForKeyword: DIRECT_OBJECT] };
            let paths = files.map(|f| paths_of(&f)).unwrap_or_default();
            log::info!(target: "ondin", "the OS asked to open {paths:?}");
            if paths.is_empty() {
                return;
            }
            pending().extend(paths);
            if let Some(ctx) = WAKE.get() {
                ctx.request_repaint();
            }
        }
    }
);

/// The file paths in an `odoc` direct object: a list of file references, or a
/// single one.
fn paths_of(files: &NSAppleEventDescriptor) -> Vec<PathBuf> {
    let path = |d: &NSAppleEventDescriptor| {
        d.fileURLValue()
            .and_then(|u| u.path())
            .map(|p| PathBuf::from(p.to_string()))
    };
    let n = files.numberOfItems();
    if n < 1 {
        return path(files).into_iter().collect();
    }
    // Apple Event lists count from 1.
    (1..=n)
        .filter_map(|i| files.descriptorAtIndex(i))
        .filter_map(|d| path(&d))
        .collect()
}

/// Listen for the open event. Called on the main thread before the event loop is
/// made, so the launch notification is still to come.
pub(crate) fn install() {
    let Some(mtm) = MainThreadMarker::new() else {
        log::warn!(target: "ondin", "open events: not on the main thread, not installed");
        return;
    };
    // SAFETY: `NSObject`'s `init` is `Handler`'s, since it adds no state.
    let handler: Retained<Handler> = unsafe { msg_send![Handler::alloc(mtm), init] };
    let center = NSNotificationCenter::defaultCenter();
    // SAFETY: `handler` is leaked below, so it outlives the observation, and it
    // answers the selector with a notification argument.
    unsafe {
        center.addObserver_selector_name_object(
            &handler,
            sel!(willFinishLaunching:),
            Some(NSApplicationWillFinishLaunchingNotification),
            None::<&AnyObject>,
        );
    }
    // Neither the notification centre nor the event manager retains its handler.
    std::mem::forget(handler);
}

/// Give the queue the window's context, so a later event wakes it.
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

/// The queue, poisoned or not: a list of paths has no invariant to break.
fn pending() -> std::sync::MutexGuard<'static, Vec<PathBuf>> {
    PENDING.lock().unwrap_or_else(|e| e.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use objc2_foundation::{NSString, NSURL};

    fn file(path: &str) -> Retained<NSAppleEventDescriptor> {
        NSAppleEventDescriptor::descriptorWithFileURL(&NSURL::fileURLWithPath(&NSString::from_str(
            path,
        )))
    }

    /// An `odoc` direct object gives its paths in order, as a list of two or as one
    /// bare reference.
    ///
    /// flip: `paths_of` counting the list from 0 → fails at the list, which loses
    /// its last file.
    #[test]
    fn the_open_event_names_its_files_in_order() {
        let list = NSAppleEventDescriptor::listDescriptor();
        list.insertDescriptor_atIndex(&file("/tmp/a b.ondin"), 1);
        list.insertDescriptor_atIndex(&file("/tmp/c.ondin"), 2);
        assert_eq!(list.numberOfItems(), 2, "the fixture is a list of two");
        assert_eq!(
            paths_of(&list),
            [
                PathBuf::from("/tmp/a b.ondin"),
                PathBuf::from("/tmp/c.ondin")
            ]
        );
        assert_eq!(
            paths_of(&file("/tmp/c.ondin")),
            [PathBuf::from("/tmp/c.ondin")]
        );
    }

    /// A file the OS asks to open while a modal is up waits in the queue, and
    /// opens on the first frame without one.
    ///
    /// flip: the frame taking the queue before its modal check → fails at "still
    /// queued".
    #[test]
    fn a_finder_open_waits_out_a_modal() {
        let dir = std::env::temp_dir().join(format!("ondin-open-event-{}", std::process::id()));
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
        for _ in 0..3 {
            let _ = crate::repeat::tests::pass(&ctx, &mut app, Vec::new());
        }
        assert_eq!(*pending(), [path.clone()], "still queued");
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
