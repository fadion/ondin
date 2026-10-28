//! Panels (§9.4): the docked layers tree, the floating property inspector, and
//! the detached colour picker.
//!
//! Pure views over `EditorSession`: they read through `display_node()` so controls
//! reflect an in-progress gesture, and act only through `commit`/`edit_valve`.

pub(crate) mod binding;
mod component;
pub(crate) mod dashboard;
mod export;
pub(crate) mod history;
mod inspector;
pub(crate) mod layers;
mod layout;
pub(crate) mod paint;
mod picker;
pub(crate) mod styles;
mod typography;
pub(crate) mod variables;
pub(crate) mod variants;

/// The Component card's property readings, cached per revision.
pub(crate) use component::PropCache;
/// What an SVG export could not say exactly, worded once for every surface that
/// reports it — the export card and *Copy as SVG* (§15 D780).
pub(crate) use export::fidelity_clauses;
pub(crate) use export::{Delivered, ExportAll, ExportPreview, ExportTurns};
pub(crate) use inspector::{ImageCard, MultiField, interacting};
pub use layers::LayerDrag;
/// The kind icon shared by the tree and the inspector header.
pub(crate) use layers::node_icon as layers_icon;
/// The boolean-operation glyph, shared by the tree and the inspector's dropdown.
pub(crate) use layers::op_glyph;
pub(crate) use paint::{CharSlot, PaintDrag, PaintSlot, map_image};
pub(crate) use picker::Picker;
/// The picker's window id, for the eyedropper tests' layer fixture.
#[cfg(test)]
pub(crate) use picker::layer_id as picker_layer_id;
pub(crate) use typography::TypeSubject;
/// A value delete awaiting its confirmation (§15 D996).
pub(crate) use variants::ValueDelete;
/// A set's clash, as its card and its canvas tab both say it (§15 D982).
pub(crate) use variants::clash_text;

/// Reveal a folder in the OS file browser; shared by the export card and the
/// library's Settings card. It opens the folder, not a file, which would launch
/// whatever program is registered for it. Failure is ignored: the path is still
/// readable in the field beside the button.
pub(crate) fn show_in_file_browser(dir: &std::path::Path) {
    #[cfg(windows)]
    let _ = std::process::Command::new("explorer").arg(dir).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(dir).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(dir).spawn();
}

// --- popover dismissal ------------------------------------------------------
//
// One dismissal rule for every card that floats over the inspector: [`ClickAway`] is
// one trigger (`clicked`) plus gating terms, `on_menu` the base test and the rest
// exemptions. A hand-rolled copy misses some of them.

/// Where a click this frame landed, as far as the popup is concerned.
#[derive(Clone, Copy, Debug)]
pub(super) struct ClickAway {
    pub clicked: bool,
    /// The click landed on something floating above the page — a combo list
    /// belonging to one of the popup's own controls.
    pub over_overlay: bool,
    pub on_menu: bool,
    /// The click landed on the button that opens the popup.
    pub on_head: bool,
    /// The click landed in the detached colour picker, which this popup can open.
    pub on_picker: bool,
    /// A gesture is in flight, or was just cancelled and its button is still down.
    /// The release that ends a scrub lands wherever the pointer was dragged, and a
    /// right-click cancel leaves the left button down, so this asks about the
    /// gesture rather than which button ended it.
    pub in_gesture: bool,
    /// The press this release ends began on the popover, so the release is the end
    /// of a gesture, not a click away. See [`press_began_away`].
    pub press_away: bool,
}

/// Whether a click this frame should dismiss the popup.
///
/// A table of exemptions, each covering a different miss:
/// - `on_head`: the opening click happens the frame before the popup exists, so it
///   is a click outside itself and the popup would vanish after one frame (§15 D82).
/// - `over_overlay`: the popup's combo lists are their own `Area`s and miss `on_menu`.
/// - `on_picker`: the detached picker is a separate layer below this popup, so the
///   overlay test says no too; matched by layer identity, as it has no rect yet.
/// - `in_gesture`: a scrub's release lands wherever it was dragged, often the canvas.
/// - `press_away`: the frame a gesture ends on, where `ctx.dragged_id()` is already
///   `None`; see [`press_began_away`].
pub(super) fn dismissed_by_click(c: ClickAway) -> bool {
    c.clicked
        && !c.in_gesture
        && !c.over_overlay
        && !c.on_menu
        && !c.on_head
        && !c.on_picker
        && c.press_away
}

/// Whether the press that ends this frame began somewhere other than the popover:
/// a click away is a press away and a release away.
///
/// It must remember the press, because egui clears `press_origin` on the frame the
/// button comes up, the only frame this is asked. Call it every frame the card
/// draws, never inside a short-circuiting `&&`, as the recording happens on another
/// frame than the read. Nothing recorded counts as away, or a fresh popover would be
/// undismissable.
pub(super) fn press_began_away(
    ctx: &egui::Context,
    id: egui::Id,
    card: egui::Rect,
    head: egui::Rect,
) -> bool {
    let key = id.with("press-began-away");
    // Read `input` before touching `data`, never nested: two accessors deadlock.
    let (pressed, at) = ctx.input(|i| (i.pointer.any_pressed(), i.pointer.press_origin()));
    if pressed && let Some(p) = at {
        ctx.data_mut(|d| d.insert_temp(key, p));
    }
    // The position, not a verdict, so "no press on record" stays distinguishable.
    ctx.data_mut(|d| d.get_temp::<egui::Pos2>(key))
        .is_none_or(|p| !card.contains(p) && !head.contains(p))
}
