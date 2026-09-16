//! Panels (§9.4): the docked layers tree, the floating property inspector, and
//! the detached colour picker.
//!
//! All are pure views over `EditorSession`: they read through `display_node()`
//! so their controls reflect an in-progress gesture, and every action they take
//! goes through `commit`/`edit_valve` — never a direct mutation.

pub(crate) mod dashboard;
mod export;
mod inspector;
pub(crate) mod layers;
pub(crate) mod paint;
mod picker;
mod typography;

/// What an SVG export could not say exactly, worded once for every surface that
/// reports it — the export card and *Copy as SVG* (§15 D780).
pub(crate) use export::fidelity_clauses;
pub(crate) use export::{ExportAll, ExportPreview};
pub(crate) use inspector::{ImageCard, MultiField};
pub use layers::LayerDrag;
/// The kind icon shared by the tree and the inspector header.
pub(crate) use layers::node_icon as layers_icon;
/// The boolean-operation glyph, shared by the tree and the inspector's dropdown.
pub(crate) use layers::op_glyph;
pub(crate) use paint::{CharSlot, PaintDrag, PaintSlot, map_image};
pub(crate) use picker::Picker;
pub(crate) use typography::TypeSubject;

/// Reveal a folder in the OS file browser.
///
/// **Not `open`ing a file** — that would hand a rendered PNG to whatever is
/// registered for it, which is a program launch the user did not ask for. Opening
/// the *folder* is the whole of what "where did that go" needs.
///
/// **Here rather than in `export.rs`, where it started**, because the library's
/// Settings card asks the same question about the base folder: two panels, one
/// three-armed `cfg`, and nothing panel-specific in it. Failure is ignored on
/// purpose — there is no useful thing to say when a desktop has no file browser,
/// and the user can still read the path out of the field beside the button.
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
// **One rule for every card that floats over the inspector.** [`ClickAway`] is
// one trigger (`clicked`) and **six gating terms**, of which `on_menu` is the base
// test and five are exemptions; all but one were paid for by a bug report, and
// `press_away` is the one that was not. A hand-rolled copy is how the next popover
// gets three of the six. Lived in `typography.rs` until the image popover was found
// missing `on_picker` and closing itself whenever the detached colour picker beside
// it was touched.
//
// ⚠️ **The count is the argument here, so it is written once — in this paragraph —
// and nowhere else.** It used to be written in six places and they gave five
// different answers: `press_away` was "the fifth" and "the sixth" in two adjacent
// doc comments below, the copy count was "a fourth" here and "a fifth" in
// `inspector.rs`, "each was paid for by a report" was asserted twice and
// contradicted twice about the same set, and `image_popover` said "the four
// popovers" of five. If a term is added, this is the sentence to change.

/// Where a click this frame landed, as far as the popup is concerned.
#[derive(Clone, Copy, Debug)]
pub(super) struct ClickAway {
    pub clicked: bool,
    /// The click landed on something floating *above* the page — a combo list
    /// belonging to one of the popup's own controls.
    pub over_overlay: bool,
    pub on_menu: bool,
    /// The click landed on the button that opens the popup.
    pub on_head: bool,
    /// The click landed in the detached colour picker — which this popup can
    /// itself open, from a decoration's swatch.
    pub on_picker: bool,
    /// A gesture is in flight, or was just cancelled and its button is still down.
    ///
    /// **The release that ends a drag is not a click away from anything**, and it
    /// arrives at whatever the pointer was dragged over — which for a scrub started
    /// in the popup is usually the canvas. Reading the primary button alone was not
    /// enough: right-click *cancels* the scrub, the left button is still held, and
    /// the release that eventually comes is the click that closed the popup. This is
    /// the term that covers both, because it asks about the gesture rather than
    /// about which button ended it.
    pub in_gesture: bool,
    /// The press that this release ends began **on** the popover — so the
    /// release is the end of a gesture rather than a click away from anything.
    /// See [`press_began_away`], which is where it comes from and which has to be
    /// called every frame rather than only on the release.
    pub press_away: bool,
}

/// Whether a click this frame should dismiss the popup.
///
/// **A function, and a table, because the opening click is the case that gets
/// missed.** The click that opens the popup happens on the frame *before* the
/// popup exists, so from the popup's first frame it is by definition a click
/// outside itself: `clicked` is true, `on_menu` is false because the pointer is on
/// the button, and without `on_head` the popup showed for exactly one frame and
/// vanished. That was the reported bug (§15 D82).
///
/// `over_overlay` is the second exemption and covers a different miss: the combo
/// lists inside the popup are their own `Area`s, so choosing a language or a wrap
/// mode is a click that also misses `on_menu`. Asked as a question about layer
/// order rather than about popups, exactly as the stroke menu asks it.
///
/// `on_picker` is the third, and misses in a third way. The detached colour
/// picker a decoration's swatch opens is an `egui::Window` at `Order::Middle` —
/// *below* this popup, so the overlay test says no — and it is a separate layer,
/// so `on_menu` says no too. Without it, touching the picker closed the popup that
/// owned it and the picker went with it: the control was unusable in one click.
/// Asked by layer **identity** rather than by rect, since the picker is drawn
/// after the inspector and has no rect yet when this runs.
///
/// `in_gesture` is the fourth, and the one that took two attempts. A scrub started
/// in the popup is finished — or cancelled with a right-click — with the pointer
/// wherever it has been dragged to, which is usually out over the canvas, and the
/// release that ends it read as a click on the page. Restricting the test to the
/// primary button was not enough: the *cancel* is the right-click, the left button
/// is still down, and the release that follows is a primary event. So the question
/// is about the gesture, not about the button that ended it.
///
/// `press_away` is the term that was not paid for by a report — see
/// [`press_began_away`]. It is the other half of `in_gesture`: that term catches a
/// gesture *in flight*, and this one catches the frame it ends on, where
/// `ctx.dragged_id()` has already gone back to `None`.
pub(super) fn dismissed_by_click(c: ClickAway) -> bool {
    c.clicked
        && !c.in_gesture
        && !c.over_overlay
        && !c.on_menu
        && !c.on_head
        && !c.on_picker
        && c.press_away
}

/// Whether the press that ends this frame **began** somewhere other than the
/// popover — the one exemption not paid for by a report.
///
/// **A click away is a press away *and* a release away, and only the second half
/// was being asked.** A scrub begun on one of a card's own rows ends with the
/// pointer wherever it was dragged to, and every other term is satisfied by a
/// gesture that never left the card. It is not what closed the image popover —
/// that was `on_picker` — but it is a hole all the same, and the adjustments
/// card's seven scrubbable rows make it the easiest one to fall into.
///
/// **It has to *remember* the press.** egui clears `press_origin` on the very
/// frame the button comes up, which is the only frame this is ever asked, so a
/// version reading it directly answers "away" for every release there has ever
/// been — present, compiling, and doing nothing.
///
/// **Called every frame the card draws**, never from inside a short-circuiting
/// `&&`: the recording is half of what it does, and the frame it must record on
/// is not the frame it is read on.
///
/// Rects rather than `contains_pointer`, because where a press *was* is not
/// something a later layer can change. Nothing recorded — the first press of a
/// session — counts as away, or a fresh popover would be undismissable.
pub(super) fn press_began_away(
    ctx: &egui::Context,
    id: egui::Id,
    card: egui::Rect,
    head: egui::Rect,
) -> bool {
    let key = id.with("press-began-away");
    // Read out of `input` before `data` is touched, never nested: each accessor
    // takes the context's own lock and two together deadlock — epaint says so
    // after ten seconds in a debug build and simply hangs in a release one.
    let (pressed, at) = ctx.input(|i| (i.pointer.any_pressed(), i.pointer.press_origin()));
    if pressed && let Some(p) = at {
        ctx.data_mut(|d| d.insert_temp(key, p));
    }
    // The position rather than a verdict, so "no press on record" and "a press
    // outside" stay distinguishable — they answer the same here and mean quite
    // different things when this is wrong.
    ctx.data_mut(|d| d.get_temp::<egui::Pos2>(key))
        .is_none_or(|p| !card.contains(p) && !head.contains(p))
}
