//! macOS's request to open documents — a double-click in Finder, a file dropped on
//! the Dock icon, `open a.ondin` — which arrives as an `odoc` Apple Event that
//! winit 0.30 does not pass on.
//!
//! The handler is installed when `NSApplicationWillFinishLaunchingNotification` is
//! posted: AppKit installs its own handlers before that, so one installed there
//! wins, and a cold launch's event comes after it. The paths queue in `opens`.

use std::path::PathBuf;

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
            crate::opens::push(paths);
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
}
