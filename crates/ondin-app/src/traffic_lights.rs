//! macOS's traffic lights, moved down to the middle of the tab strip
//! (`tabs::STRIP_H`). AppKit centres them in its own title bar, which is
//! shorter than the strip drawn under it, and winit has no setting for where
//! they go.
//!
//! AppKit lays them out again on a resize and on leaving full screen, so
//! [`place`] runs every frame and writes only when they are off the line. In
//! full screen they belong to the title bar that slides down with the menu bar,
//! and are left where AppKit puts them.

use objc2_app_kit::{NSView, NSWindowButton, NSWindowStyleMask};
use raw_window_handle::{HasWindowHandle, RawWindowHandle};

/// Within this of the line counts as on it, so a frame that finds them placed
/// writes nothing.
const SLACK: f64 = 0.25;

/// Centre the lights on the tab strip. A window that is not AppKit's,
/// such as a test's, is left alone.
pub(crate) fn place(window: &impl HasWindowHandle) {
    let Ok(handle) = window.window_handle() else {
        return;
    };
    let RawWindowHandle::AppKit(appkit) = handle.as_raw() else {
        return;
    };
    // SAFETY: an AppKit handle points at the window's live content view, and
    // the frame loop that calls this runs on the main thread.
    let view: &NSView = unsafe { appkit.ns_view.cast().as_ref() };
    let Some(window) = view.window() else {
        return;
    };
    if window.styleMask().contains(NSWindowStyleMask::FullScreen) {
        return;
    }
    let buttons = [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ]
    .map(|b| window.standardWindowButton(b));
    let [Some(close), Some(_), Some(_)] = &buttons else {
        return;
    };
    // SAFETY: reading the view tree, on the main thread.
    let Some(bar) = (unsafe { close.superview() }) else {
        return;
    };
    // SAFETY: as above.
    let Some(container) = (unsafe { bar.superview() }) else {
        return;
    };
    let mid = f64::from(crate::tabs::STRIP_H) / 2.0;
    let half = close.frame().size.height / 2.0;

    // The title bar's container hangs from the window's top; grown downwards, if
    // it is too short to hold a light centred on the line, so none is clipped.
    let need = (mid + half).ceil();
    let mut frame = container.frame();
    if frame.size.height < need {
        // SAFETY: as above.
        let flipped = unsafe { container.superview() }.is_some_and(|v| v.isFlipped());
        if !flipped {
            frame.origin.y -= need - frame.size.height;
        }
        frame.size.height = need;
        container.setFrame(frame);
    }

    let height = bar.frame().size.height;
    let y = if bar.isFlipped() {
        mid - half
    } else {
        height - mid - half
    };
    for button in buttons.iter().flatten() {
        let mut origin = button.frame().origin;
        if (origin.y - y).abs() > SLACK {
            origin.y = y;
            button.setFrameOrigin(origin);
        }
    }
}
