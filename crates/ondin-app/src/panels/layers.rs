//! The layers tree: z-order, structure, visibility and locking.
//!
//! Rows are painted by hand rather than composed from egui widgets, because the
//! design calls for a 26px row with hover-revealed controls and a size badge —
//! cheaper and more predictable to draw directly than to fight widget layout.
//!
//! Dragging a row reorders or reparents it. That goes through
//! `build::reparent_preserving_world`, not the raw `Reparent` op: local
//! transforms mean a naive reparent teleports the node by the difference
//! between the two parents.
//!
//! **The drop follows the indent, both ways.** A gap between two rows is a run of
//! insertion points rather than one — a level for each the two rows have in common —
//! and the pointer's *horizontal* travel from where the row was picked up chooses
//! between them, drawing the insertion line at that level's own column. So a layer is
//! carried out of a group by taking it to the bottom of the group and moving left, and
//! into one by moving right, which is the gesture Figma and its neighbours have.
//! [`depth_range`] is the rule and [`OndinApp::drop_hint`] is where it is read.

use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use eframe::egui;
use ondin_core::{NodeId, NodeKind, Operation, Transaction, build};
use std::collections::{HashMap, HashSet};

/// Row height. **The rows do not touch** — `item_spacing.y` is 1 in the tree, so
/// the pitch is 27 and every guide and drop line has a gap to cross or centre in.
const ROW_H: f32 = 26.0;

/// The **header**'s left and right margin, in points.
///
/// Here rather than in `app.rs`, where the `Margin` is spelled, because a copy of
/// it there would be the number that goes stale. `i8` for the same reason the
/// margin is one — `egui::Margin` takes whole points.
///
/// ⚠️ **This is not what governs a row**, and [`MIN_W`]'s derivation used it as
/// though it were until 2026-09-06. The header frame is a *sibling* of the
/// `ScrollArea`; what indents the tree is [`TREE_PAD_X`]. See that constant.
pub(crate) const PAD_X: i8 = 12;

/// The **tree**'s left and right gutter, in points — the `ScrollArea`'s inner
/// frame, and so the term that actually narrows a row (§15 D444).
///
/// ⚠️ **[`MIN_W`]'s derivation subtracted `2 * PAD_X` (24) instead of this (12),
/// so every width figure in that constant's prose and in [`MAX_W`]'s was 12pt
/// pessimistic.** Measured off a real `layers_panel` at `MIN_W`: a row runs
/// `6.0..204.0`, i.e. **198pt**, where the derivation said 186.
///
/// Nothing user-visible followed, because the error is entirely in the safe
/// direction — 210 and 600 are both *more* generous than they were argued to be.
/// What it cost is the thing `panel_width_tests` exists for and says in its own
/// `//!`: retuning the header's gutter moved three assertions about the tree, and
/// retuning the tree's own moved none of them. That is the module's stated
/// failure mode, inverted.
pub(crate) const TREE_PAD_X: i8 = 6;
/// Room left for the caret column on a row that has one, and on a top-level row
/// whether it has one or not.
const CARET_ROOM: f32 = 16.0;
/// [`CARET_ROOM`] for a row that gets none: a nested leaf.
const NO_CARET_ROOM: f32 = 2.0;
/// Half the kind glyph's 15pt box — `icon_x` is a *centre*.
const ICON_HALF: f32 = 8.0;
/// From the kind glyph's centre to where the name starts.
///
/// 15, not 12: the glyph is drawn centred, so half of its 15pt advance already
/// sits to the right, and the name was landing about 3px closer than the design's
/// 7px gap.
const NAME_GAP: f32 = 15.0;
/// The lock glyph's centre, in from the row's right edge.
const LOCK_INSET: f32 = 14.0;
/// From the lock's centre to the eye's — the glyphs are 15pt, so the old 18pt
/// pitch left them almost touching.
const EYE_PITCH: f32 = 23.0;

/// Where a row's kind glyph is centred, measured from the row rect's left edge.
///
/// Extracted from the row walk **so that [`MIN_W`] can be argued from the same
/// numbers the row is drawn with** rather than from a copy of them. One
/// production caller and one test caller; the name follows at [`NAME_GAP`] past
/// what this answers.
fn icon_offset(depth: usize, caret_room: bool) -> f32 {
    indent_of(depth)
        + if caret_room {
            CARET_ROOM
        } else {
            NO_CARET_ROOM
        }
        + ICON_HALF
}

/// The size of the eye, lock and kind glyphs, in points.
const ICON_PT: f32 = 15.0;

/// The panel's width when nothing has been dragged — §15 D330's 300, now a
/// *default* rather than an exact size (§15 D349).
pub(crate) const DEFAULT_W: f32 = 300.0;

/// How narrow the drag may take the panel.
///
/// **Measured off the row walk's own arithmetic, not chosen** — and composed from
/// the same items the walk draws with, so retuning any of them moves this rather
/// than silently invalidating it: `w − 2·`[`PAD_X`]` − `[`icon_offset`]` −
/// `[`NAME_GAP`]` − `[`LOCK_INSET`]` − `[`EYE_PITCH`]` − `[`ICON_PT`]`/2` is what
/// a **hovered** row's name has to live in, and that expression is the whole of
/// what a minimum can be argued from. Hovered, because at rest a visible,
/// unlocked, unbadged row draws nothing on its right at all and has the whole
/// row — a minimum argued from the roomy state is one that breaks the moment the
/// pointer arrives.
///
/// **210 is where two independent readings of it meet**, which is why it is this
/// number and not a round 200:
///
/// - a **top-level** row keeps 108.5pt of name while hovered — measured against
///   real names at the row's own 12.5pt, where `Rectangle 12` lays out at 75.06
///   and `Button primary hover` at 124.34, so the common case fits and the long
///   case is the one that clips. (`Hero background`, at 102.69, is the nearest
///   thing to a borderline case and also clips.) **The same two names the test
///   uses**, deliberately: this comment and
///   `panel_width_tests::the_minimum_keeps_a_real_layer_name_on_a_top_level_row`
///   advertised different long names for a while, which reads as two different
///   arguments for one number;
/// - a **four-deep** row's hovered name room stops being negative. Four levels is
///   not an arbitrary depth to test at — it is the depth §15 D330 already argued
///   the 300 from ("a layer four deep had about 90pt at 250"), so this reuses that
///   decision's own reference rather than inventing one.
///
/// ⚠️ **The second reading no longer picks 210, and this sentence is left as the
/// place that says so.** It crosses zero at **194.5**, not the 206.5 written here
/// until 2026-09-06: both figures came from subtracting the header's `2 * PAD_X`
/// where a row is narrowed by [`TREE_PAD_X`], which is half of it. So the two
/// readings do **not** meet at 210 — the first one above is now the only one
/// choosing it, and this one would have allowed 195. **210 is left alone**,
/// because the error was entirely in the safe direction and the number that looks
/// right on screen is not something a derivation gets to overrule; but the
/// argument for it is one reading, not two, and re-tightening it is a design call.
///
/// Below this the name of a nested row is drawn *underneath* the eye, which is a
/// broken picture rather than a tight one. Above it, clipping is the user's
/// choice.
///
/// ⚠️ **The panel's header is not the binding constraint and was checked, not
/// assumed** — the search field is `ui.available_width()` and flexes, and the
/// eyebrow plus its two 20pt buttons want well under the 198pt of content this
/// leaves.
pub(crate) const MIN_W: f32 = 210.0;

/// How wide the drag may take the panel: twice the default.
///
/// A cap at all, because a panel dragged over the whole window is a state with no
/// signpost out of it; egui only stops it at the window edge. Twice the default
/// because that is past every tree this has to hold — at 600 a row **eight** deep
/// still has **281.5pt** of name hovered — so the cap binds only on someone
/// starving the canvas.
///
/// ⚠️ This said 310, which is a real reading of the same row and the wrong one:
/// it is the **at-rest, badged** figure. The row has three readings, not two —
/// 314 at rest with nothing on its right, 310 once a `RowBadge`'s 4pt inset is
/// owed, and 269.5 hovered, where the eye-and-lock cluster takes 44.5. The
/// sentence is about the hovered formula [`MIN_W`] is derived from, so it wanted
/// the last of the three. Caught by `arch-scribe` reading the arithmetic back out
/// of the constants and by nothing else: the test asserts `> 200.0`, and **a
/// bound loose enough to be about the right thing is loose enough not to notice
/// which of three numbers is in the prose beside it.**
///
/// ⚠️ **All three of those readings were then 12pt low**, for a reason that has
/// nothing to do with which one the sentence wanted: the derivation subtracted
/// the header's `2 * PAD_X` where a row is narrowed by [`TREE_PAD_X`]. The
/// hovered figure is **281.5**, the other two 326 and 322. The loose `> 200.0`
/// bound is the same lesson twice over — it could not see a wrong *reading* and
/// it could not see a wrong *first term* either.
pub(crate) const MAX_W: f32 = 600.0;

/// The default has to sit **inside** the pair, or the panel opens at a width the
/// user cannot drag back to.
///
/// A `const` assertion rather than one in `panel_width_tests`, where it started:
/// all three are compile-time constants, so this is something the compiler can
/// simply refuse, and clippy said as much (`assertions_on_constants`). Failing
/// the build at the definition beats failing a test that has to be *run*, and it
/// puts the check where the next person retuning one of the three is looking.
///
/// ⚠️ Flipped by moving `DEFAULT_W` to 900: `error[E0080]: evaluation panicked`,
/// pointing at this line. It bites at compile time, which is the whole claim.
const _: () = assert!(MIN_W < DEFAULT_W && DEFAULT_W < MAX_W);

/// The side of a layer row's image thumbnail, in points.
///
/// **16, chosen by eye against 20 on the machine.** The argument for the larger
/// number was that a thumbnail wants to be recognisable and 20 is roughly the
/// row's height; seen in a real tree it was simply too heavy, and the smaller
/// square reads as a picture just as well while leaving the row quieter. It also
/// puts the thumbnail at the size of the 15-point glyph it replaces, so an image
/// row no longer sits taller than every other row in the column.
///
/// Nothing in the surrounding geometry forced either number — the column has
/// room for both. The caret is centred 19 points to the left of `icon_x` with an
/// 11-point glyph and the name starts at `icon_x + 15`, so 16 clears the caret by
/// 5.5 and the name by 7, against 3.5 and 5 at 20. What is worth keeping is the
/// ceiling: at 26 — the row's own height — the square would come within half a
/// point of the caret and two of the name, so "as tall as the row" was never
/// available here whatever it looks like elsewhere.
const THUMB: f32 = 16.0;
/// The thumbnail's corner radius, in points. Asked for on the machine at the
/// same time as the size.
///
/// 3 where the inspector's chip is 4, and they are deliberately not shared: the
/// chip's radius belongs to `ui::swatch` and every colour in the app wears it,
/// so matching them would mean either restyling every swatch or special-casing
/// the one that holds a picture.
const THUMB_RADIUS: u8 = 3;
/// The indent added per tree level. See [`indent_of`], which is where the
/// arithmetic lives, and [`caret_column`], which is what the guides hang off.
///
/// 31pt per level, not the 16 this started at: at 16 a nested row was only one
/// caret-width to the right of its parent, and with the caret drawn on the parent
/// rather than the child there was almost nothing to tell "inside this frame"
/// from "next to it". It is also the number behind the panel's 300pt width (§15
/// D330) — four levels of indent leave a name very little room at 250.
///
/// ⚠️ **The design asks for 22 and this deliberately does not follow it** —
/// asked and answered on the machine, *"indent is fine too, no need to change
/// it"* (§15 D331). That 22 is the newer number: the layers pane went from 15 per
/// level to a consistent 22 in the same pass that added the guides, which is
/// coherent as a *pair* — a guide says which container a row is in, so the indent
/// need not carry that alone and 9pt of name per level comes back. It was reported
/// rather than implemented because the change was asked for as *bigger* indents
/// and 31 already is bigger, so following the design would have tightened the tree
/// in the name of widening it. **Not an open question**: 31 is the settled number,
/// and nothing in the guides turns on it — they are drawn from [`caret_column`] at
/// whatever this says.
const INDENT: f32 = 31.0;
/// The tree guides' dash: 1.5pt of ink to every 2.5pt of gap — the design's
/// `repeating-linear-gradient(… 0 1.5px, transparent 1.5px 4px)`.
///
/// ⚠️ **Every run starts with a dash, and that is load-bearing rather than a
/// default.** `Shape::dashed_line` opens with ink at the start of the path, so a
/// run beginning at a **junction** — the elbow, or the top of a row — puts ink
/// through it whatever the arithmetic either side works out to. This shipped the
/// other way, with the below-elbow run phased to continue the lattice the row's top
/// had started (`(-along).rem_euclid(4)`), which is more correct in the abstract and
/// was reported from the machine within the hour: *"a slight gap on the vertical
/// lines where they meet the horizontal ones"*. A three-point gap opening exactly
/// where the eye follows the line around a corner, to keep a rhythm nobody can
/// count. The rhythm is now per-run, as the design's per-row backgrounds are, and
/// what that costs is one dash of double length at each junction — which reads as
/// the junction being marked.
const GUIDE_DASH: f32 = 1.5;
/// The gap between two of [`GUIDE_DASH`]'s dashes.
const GUIDE_GAP: f32 = 2.5;
/// How far clear of its trunk a row's elbow starts.
///
/// **One gap of the dash pattern past the trunk's own ink**, which is half a point
/// either side of its column: so the corner reads as the pattern turning rather
/// than as two lines meeting in a blot. The other half of the same report as
/// [`GUIDE_DASH`]'s note — *"the horizontal lines start a pixel or two more to the
/// right"* — and it shipped at 1.0, which put the elbow's first dash half a point
/// off the trunk's edge.
const ELBOW_CLEAR: f32 = 0.5 + GUIDE_GAP;
/// A tree guide at rest — the design's 8.5% of the text colour.
///
/// **Not one of [`theme::text`]'s tiers**, which start at 38% and are a *type*
/// ramp. A hairline the width of a caret's stroke has to sit under the dimmest
/// glyph in the panel, or the gutter competes with the names.
const GUIDE_REST: egui::Color32 = theme::color::text_a(22);
/// [`GUIDE_REST`] lit: the trunk from the hovered row's parent down to its elbow,
/// at the design's 34%.
const GUIDE_HOVER: egui::Color32 = theme::color::text_a(87);
/// How close to a row's edge counts as "drop between" rather than "drop into".
const EDGE_FRACTION: f32 = 0.3;
/// How far, in indents, the pointer has to leave the level the drag is currently holding
/// before the drop line moves to another one. See [`depth_asked_for`].
///
/// **0.65 against the 0.5 that plain rounding gives**, which is 20pt of travel where the
/// bare threshold was 15.5. Not chosen as a feel: 0.5 is not a threshold at all but the
/// point where two levels are equally close, so a hand that is *aiming* at neither — one
/// holding a drag still, or moving down the tree — sits on it and the line flickers
/// between two indents. Anything above 0.5 turns that edge into a band; 0.65 is the
/// smallest number that puts the band comfortably outside a still hand's jitter and the
/// few points of horizontal drift in a vertical drag, while still letting a deliberate
/// sideways move land in under two thirds of one indent.
const STICK: f32 = 0.65;
/// How long a drag has to rest on a collapsed group before it opens, in seconds.
///
/// **One second, asked for as one second.** Long enough that dragging *past* a collapsed
/// group on the way somewhere else does not open it, short enough that a hand that has
/// stopped and is waiting gets an answer before it decides the panel is not going to
/// give one. The branch opens and stays open — this is the same `collapsed` set a caret
/// click writes, so the tree is left as the drag found it only if the drop did not need
/// to go inside, which is the honest outcome either way.
const SPRING_SECS: f64 = 1.0;

/// A layer row being dragged, and where it would land if released now.
pub struct LayerDrag {
    /// Every row being carried, not just the one under the pointer.
    ///
    /// Dragging a row that is part of the selection takes the **whole**
    /// selection with it, which is what the canvas has always done for a move
    /// and what makes the tree usable for reorganising more than one layer.
    /// Already reduced to [`build::outermost`], so a selection holding both a
    /// group and something inside it does not move the inner node twice (§5.7a).
    pub ids: Vec<NodeId>,
    pub target: Option<DropTarget>,
    /// Where the pointer was when the drag started, and how deep the row under it
    /// was. **The pair is the origin the indent gesture is measured from** — see
    /// [`depth_asked_for`], which is where the reason a displacement and not an
    /// absolute x is written down.
    ///
    /// The depth is the *grabbed* row's, not the whole set's: a drag carrying rows
    /// from several levels has no one depth, and the row the pointer is actually on
    /// is the one the hand is thinking about.
    pub grab: (f32, usize),
    /// The level the indent gesture is currently holding — [`depth_asked_for`]'s
    /// answer from last frame, **before** the boundary's own range clamped it.
    ///
    /// Starts at the grabbed row's depth, so a drag that has not moved sideways asks
    /// for the level it was picked up at.
    pub level: usize,
    /// The container the drop would land in, **as it was last frame** — the row that
    /// paints its name and glyph in [`color::SELECT`] to say so.
    ///
    /// ⚠️ **A frame behind, and it has to be.** A row's ink is painted while the walk
    /// is passing it, and the drop is not worked out until the walk is over — an
    /// indent-aware target is a claim about the row above a boundary and the row below
    /// it at once ([`OndinApp::drop_hint`]). So the walk reads the answer the last one
    /// reached. That is the same deal every hover in the panel already runs on, egui
    /// reporting interaction against the previous frame's geometry, and the lag is
    /// invisible: it is one frame, and only on the frames where the drop changes which
    /// container it is in.
    ///
    /// **Not merged into [`target`], which is deliberately cleared every frame** so
    /// that releasing off the tree drops nothing rather than acting on a stale answer
    /// (§15 D19). This is that same staleness, kept on purpose, for painting only — it
    /// must never decide where a layer lands.
    ///
    /// [`target`]: LayerDrag::target
    pub lit: Option<NodeId>,
    /// The collapsed group the pointer is resting on, and the time it arrived —
    /// [`SPRING_SECS`] later the branch opens. `None` whenever the pointer is not on
    /// one, which is what makes the wait start again rather than accumulate.
    pub spring: Option<(NodeId, f64)>,
}

/// Where a dragged row would be inserted: a slot in one parent's child list, and
/// nothing else.
///
/// ⚠️ **It carried a third field, `into`, saying whether the drop was inside a
/// container rather than between siblings, and it is gone.** Nothing in production
/// ever read it — the tests compared whole values, which is why it looked alive: the
/// affordance is chosen from [`DropSpot`], which is what the pointer actually landed
/// on, and `plan_layer_move` only ever wanted the parent and the index. It had also stopped meaning what it said — a drop indented in as a
/// container's first child *is* a drop inside it and set `into: false`, because by
/// then the field meant "which of the two hints was drawn" rather than anything about
/// the destination.
///
/// ⚠️ **And no gate would have said so — the one that kept it quiet is the derived
/// `PartialEq`.** Measured rather than assumed, on a private module holding one `pub`
/// struct with one unread field: bare, `dead_code` fires; with `Debug` it still fires
/// (rustc deliberately does not count that as a read) and with `Clone, Copy` it still
/// fires; add `PartialEq, Eq` and it goes silent, `--test` or not. So a field can be
/// read forever by a derive and an `assert_eq!` over the whole value and be dead in
/// production the entire time. It is **not** the `pub`, which is worth saying because
/// that is the reflex here: `ondin-app` has no lib target, so a `pub` item inside a
/// module of it is not externally reachable and the lint does analyse it. That
/// exemption is `ondin-core`'s, one crate over. This went because the struct was read
/// against `plan_layer_move`, not because anything failed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DropTarget {
    pub parent: NodeId,
    pub index: usize,
}

/// The glyph for one boolean operation — Phosphor's own four, which is what the
/// design asks for and what every other tool draws.
///
/// Beside [`node_icon`] because it has the same two readers for the same reason: the
/// tree draws it on a boolean's row and the inspector's dropdown draws it in both its
/// closed state and its menu, and one operation looking like two different pictures
/// in the two places would be the whole cost of a second table.
pub(crate) fn op_glyph(op: ondin_core::BoolOp) -> &'static str {
    match op {
        ondin_core::BoolOp::Union => icon::UNITE_SQUARE,
        ondin_core::BoolOp::Subtract => icon::SUBTRACT_SQUARE,
        ondin_core::BoolOp::Intersect => icon::INTERSECT_SQUARE,
        ondin_core::BoolOp::Exclude => icon::EXCLUDE_SQUARE,
    }
}

/// Whether any paint on `node` names a picture `store` cannot draw — the merged
/// condition §15 D179 keeps merged.
///
/// **A free function over the store, not a method on the app**, so it can be
/// asked directly: this is the whole of the decision the row's colour rests on,
/// and a test of it should not have to draw a row to reach it.
///
/// Asked of the **decode cache** rather than of the document, so it is the same
/// question the walk asks before drawing the placeholder. A panel that answered
/// from the table alone would catch a dangling id and miss both a dead link and
/// bytes that would not decode — two of the three states.
///
/// Fills *and* strokes, unlike the canvas placeholder, which is drawn only for a
/// fill: that drawing is a statement about an *area* and a stroke has none, but
/// the row is a statement about the *layer*, and a layer whose border is a
/// missing picture is as broken as one whose middle is. Hidden paints are
/// skipped, because nothing is drawing them to be missing.
pub(crate) fn picture_missing(store: &ondin_render::ImageStore, node: &ondin_core::Node) -> bool {
    let paint = node.paint();
    // A frame's ground used to be chained on here out of its own field. It is one
    // of the fills below now (§15 D400), and so is every *other* fill a frame can
    // hold — which this could not have seen.
    paint
        .fills
        .iter()
        .filter(|f| f.visible)
        .map(|f| &f.brush)
        .chain(paint.strokes.iter().filter(|s| s.visible).map(|s| &s.brush))
        .any(|b| match b {
            ondin_core::Brush::Image(img) => store.get(&img.image.id).is_none(),
            _ => false,
        })
}

/// The picture a row draws a thumbnail of: the first visible **fill** that is
/// one.
///
/// **Deliberately narrower than [`picture_missing`], which is right beside it
/// and reads almost the same paints.** That function asks whether the layer is
/// *broken*, so a stroke painted with a dead link counts — a border that cannot
/// draw is as broken as a middle that cannot. This one asks what the layer
/// *looks like*, and a stroke's picture is a few pixels wide on the canvas and
/// nothing at all in a twenty-pixel square.
///
/// ⚠️ **A frame's ground is in scope now and used to be out of it** (§15 D400),
/// and the change is worth stating because it was never an exclusion anyone
/// wrote: this reads `fills`, a frame's ground lived in a field of its own, and
/// the comment here justified the omission after the fact ("a frame is a
/// container rather than a picture"). A frame filled with a photograph now shows
/// that photograph as its row's thumbnail, exactly as a rectangle filled with it
/// does. Its size badge is unaffected — the badge slot is the *other* end of the
/// row.
///
/// The **first** fill rather than the topmost-opaque one. A stack of image fills
/// is rare, and picking between them by coverage would make the row's answer
/// depend on alpha arithmetic nothing else in the panel does.
///
/// ⚠️ **It answers with the whole brush and used to answer with `&ImageRef`**
/// (§15 D784). The picture is what the thumbnail draws and the **sampler** is how
/// faded it draws — and dropping the second on the floor here is why this row
/// showed a 30% fill at full opacity for as long as an image fill has had an
/// alpha. A helper that returns half of a brush is how the other half gets
/// forgotten by every caller at once.
///
/// [`thumb_tint`] is what the second half is for.
fn picture_of(node: &ondin_core::Node) -> Option<&ondin_core::ImageBrush> {
    node.paint()
        .fills
        .iter()
        .filter(|f| f.visible)
        .find_map(|f| match &f.brush {
            ondin_core::Brush::Image(img) => Some(img),
            _ => None,
        })
}

/// The tint a row's thumbnail is drawn with: the row's own dimming and the
/// picture's own alpha, composed.
///
/// **The fill is the texture's tint, not a colour drawn under it** — the same 40%
/// a dimmed glyph takes, as a multiplier over the picture, so a hidden layer's
/// thumbnail fades with its name instead of staying the one bright thing on a row
/// that is out of play.
///
/// 🚨 **Two multipliers, and they compose rather than one winning** (§15 D784).
/// The row's dimming says *this layer is out of play*; the brush's alpha says
/// *this paint is faint*. They are facts about different things, so a hidden
/// layer holding a 30% picture is dimmer than either alone — and a rule that
/// picked a winner would make one of the two invisible exactly when the other
/// was also true.
///
/// ⚠️ **It is a named function so that it can be asserted at all.** The drawn
/// thumbnail is not reachable from a headless test without a decoded picture in
/// the store *and* a texture the frame budget did not defer, which is two pieces
/// of machinery between the test and the decision. The decision is this
/// expression; the rest is the thumbnail cache, which has its own tests.
fn thumb_tint(dimmed: bool, picture: Option<&ondin_core::ImageBrush>) -> egui::Color32 {
    let base = match dimmed {
        true => egui::Color32::from_white_alpha(102),
        false => egui::Color32::WHITE,
    };
    base.gamma_multiply(picture.map_or(1.0, crate::panels::paint::image_brush_alpha))
}

/// Half the width of a row control's hit band, either side of the x it is drawn
/// centred on.
///
/// **One number for the click and for the hover.** The row is a single
/// `click_and_drag` region rather than three widgets, so both the "did you press
/// the eye" test and the "is the pointer on the eye" test are hand-written
/// against these x's — and two hand-written bands are how a control comes to
/// light up somewhere it cannot be pressed. [`RowTargets`] is the one answer
/// both ask.
const CONTROL_HIT: f32 = 9.0;

/// Which of a row's three hand-drawn controls a point lands on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RowHit {
    Caret,
    Eye,
    Lock,
}

/// Where a row's controls are, and which of them it is showing.
///
/// `None` means the row is not drawing that one at all — a childless row has no
/// caret, and a resting visible row shows no eye — so it cannot be pressed and
/// must not light up either.
#[derive(Clone, Copy, Default)]
struct RowTargets {
    caret: Option<f32>,
    eye: Option<f32>,
    lock: Option<f32>,
}

impl RowTargets {
    /// The control at `x`, resolved **caret, then eye, then lock**.
    ///
    /// The order is the click's own and matters only where two bands overlap,
    /// which on a deeply indented narrow panel they can: the caret walks right
    /// with the indent while the eye is pinned to the right edge. Resolving it
    /// here rather than in an `else if` chain is what keeps the mark under the
    /// pointer and the thing the press acts on the same control.
    fn at(&self, x: f32) -> Option<RowHit> {
        let near = |c: &Option<f32>| c.is_some_and(|at| (x - at).abs() <= CONTROL_HIT);
        if near(&self.caret) {
            Some(RowHit::Caret)
        } else if near(&self.eye) {
            Some(RowHit::Eye)
        } else if near(&self.lock) {
            Some(RowHit::Lock)
        } else {
            None
        }
    }
}

/// The one remark a resting row makes at its right-hand end.
///
/// Two shapes rather than one string because one of them is a Phosphor glyph and
/// glyphs want `theme::icon_font`, not the proportional face — and the size that
/// puts a pictogram on the same ink height as a word is not the same number
/// ([`BADGE_STACK`], [`BADGE_MASK`]), nor the same number as each other.
enum RowBadge {
    /// An artboard's `800 × 600`.
    Text(String),
    /// A Phosphor glyph and **the type size that puts it on the word's ink
    /// height**, which is a property of the glyph rather than of the slot.
    ///
    /// The size travels with the pictogram because it is not one number. A
    /// circle reaches its extremes where a stack of bars does not, so
    /// `circle-half` at the stack's 11.0 inks 9.60 against the word's 8.80 at
    /// 125% — visibly the heavier of two marks that are meant to be the same
    /// kind of remark. One constant here would be a number measured for the
    /// first glyph and inherited by the second without anyone re-checking it.
    Icon(&'static str, f32),
}

/// Type size for the base operand's `stack-simple`, chosen so it inks the same
/// height as the worded badge beside it in the same slot.
///
/// **Measured, not picked**, by laying both out and reading the ink box off the
/// galley's `uv_rect` — which is the *tight* glyph bitmap, unlike the galley's
/// own height. `800 × 600` at 10.5 and `stack-simple` at 11.0 ink the same
/// height at 100%, 125% and 150% display scaling (9.00 / 8.80 / 8.67) and are
/// half a point apart at 200%. The next size up, 12, is taller at every scaling
/// above 100% — so this is a plateau rather than a coin toss, and
/// `the_base_badge_inks_as_tall_as_the_worded_one` holds it there.
///
/// **It matches the *word*, not the buttons.** The eye and lock take this slot
/// over on hover and are 15, but those are controls and a badge is an
/// annotation; the artboard's `800 × 600` has been the quieter of the two in
/// this exact slot since it was written.
const BADGE_STACK: f32 = 11.0;

/// Type size for a mask's `circle-half`, measured the same way and landing
/// somewhere else — which is the whole reason [`RowBadge::Icon`] carries its
/// size.
///
/// A **better** plateau than the stack's, as it happens: at 10.0 the glyph inks
/// 9.00 / 8.80 / 8.67 / 8.50 across the four scalings, which is the word's height
/// exactly at all four rather than at three of them. Either side is worse — 9.5
/// is short at 100% and 125%, and 11.0 (the stack's number, and what this would
/// have been had one constant served both) is 0.80 tall at 125% and 0.66 at 150%.
const BADGE_MASK: f32 = 10.0;

/// Whether `node` is the shape a `Subtract` cuts into — its **base** operand.
///
/// **The one thing about a boolean the tree shows without saying** (§15 D244).
/// Operand
/// order *is* child order (§15 D113: creation-time ordering, never a stored
/// flag), and only `Subtract` reads it, so the base is `children[0]` — which the
/// tree, drawing topmost-first, puts on the *bottom* row. That is a real fact
/// about the artwork and it was legible only to someone who already knew the
/// rule; every other operation folds commutatively, so a mark on any of them
/// would be claiming a significance the arithmetic does not have.
///
/// Asked of the parent rather than of the node, because being a base is not a
/// property of the shape: drag it up one row and it stops being one.
fn subtract_base(doc: &ondin_core::Document, node: &ondin_core::Node) -> bool {
    let Some(parent) = node.parent().and_then(|p| doc.get(p)) else {
        return false;
    };
    matches!(
        parent.kind(),
        NodeKind::Boolean {
            op: ondin_core::BoolOp::Subtract
        }
    ) && parent.children().first() == Some(&node.id())
}

/// The layers-tree icon (glyph + resting colour) for a node kind.
pub(crate) fn node_icon(kind: &NodeKind) -> (&'static str, egui::Color32) {
    match kind {
        NodeKind::Root => (icon::SELECTION, theme::text::MUTED),
        NodeKind::Artboard { .. } => (icon::FRAME_CORNERS, color::ACCENT_400),
        NodeKind::Group => (icon::SELECTION, theme::text::MUTED),
        NodeKind::Rect { .. } => (icon::SQUARE, theme::text::MUTED),
        NodeKind::Ellipse { .. } => (icon::CIRCLE, theme::text::MUTED),
        NodeKind::Polygon { .. } => (icon::TRIANGLE, theme::text::MUTED),
        NodeKind::Star { .. } => (icon::STAR, theme::text::MUTED),
        NodeKind::Line { .. } => (icon::LINE_SEGMENT, theme::text::MUTED),
        NodeKind::Path { .. } => (icon::POLYGON, color::ACCENT_400),
        NodeKind::Text { .. } => (icon::TEXT_T, theme::text::MUTED),
        // Accent, like `Path`: both are shapes whose outline the user authored
        // rather than picked from the shape tools, and a boolean *is* a path by the
        // time anything draws it.
        //
        // **The operation's own glyph**, now that the dropdown's four exist. The row
        // used to draw the path icon for all four and lean on the name to
        // disambiguate — which works until the layer is renamed, and a boolean gets
        // renamed like anything else ("Badge cutout"). The icon is the half of the
        // row that cannot be edited away.
        NodeKind::Boolean { op } => (op_glyph(*op), color::ACCENT_400),
    }
}

/// The egui id of a row's rename field. Stable across frames and unique per
/// row, so focus can be asked for before the widget itself exists.
///
/// `pub(crate)` because focus for it is asked for from **outside** this file:
/// `OndinApp::rename_selection` is the other way a rename opens — `Ctrl+R`, `F2`
/// and the context menu's row — and it has to ask at the same moment the
/// double-click below does (§15 D216).
pub(crate) fn rename_field_id(id: NodeId) -> egui::Id {
    egui::Id::new(("layer-rename", id))
}

/// Where the rename field goes on a row whose name is painted from `left`.
///
/// **The full row height, starting exactly at the name's own left edge.** Both
/// halves are load-bearing and both were wrong once: inset the rect and the name
/// jumps sideways the moment it is double-clicked, and shorten it and `put`'s
/// centring has less room than the painted name had, so the text rides up. The
/// field's own `margin` must be zero and its `vertical_align` centred for this
/// rect to land the text back where `Align2::LEFT_CENTER` had it — measured, not
/// assumed (see the test below).
fn rename_field_rect(row: egui::Rect, left: f32) -> egui::Rect {
    egui::Rect::from_min_max(
        egui::pos2(left, row.top()),
        egui::pos2(row.right() - 8.0, row.bottom()),
    )
}

/// The operations a layer-tree drop turns into, and whether any of them crossed
/// a parent boundary (which is only used to word the status line).
pub(crate) struct MovePlan {
    pub ops: Vec<Operation>,
    pub reparented: bool,
}

/// Plan the drop of `moving` into `target`, as one list of operations.
///
/// **Every index is computed against the list the previous operations leave
/// behind**, not against the document as it stands. That is the care
/// `build::z_order` takes and for the same reason: three rows dropped into one
/// parent with three indices all read off the original list land in the wrong
/// order, or out of range. So the child lists this touches are simulated and each
/// operation is planned against the simulation.
///
/// A free function rather than a method because it is the whole of the tricky
/// part and none of the UI — it needs no `OndinApp`, and so it can be tested
/// without one.
pub(crate) fn plan_layer_move(
    doc: &ondin_core::Document,
    res: &ondin_core::Resolved,
    moving: &[NodeId],
    target: DropTarget,
) -> Result<MovePlan, ondin_core::OpError> {
    let mut sim: HashMap<NodeId, Vec<NodeId>> = HashMap::new();
    let list_for = |sim: &mut HashMap<NodeId, Vec<NodeId>>, p: NodeId| {
        sim.entry(p).or_insert_with(|| {
            doc.get(p)
                .map(|n| n.children().to_vec())
                .unwrap_or_default()
        });
    };
    list_for(&mut sim, target.parent);

    let mut ops = Vec::new();
    let mut reparented = false;
    let mut slot = target.index;
    for id in moving {
        let Some(from) = doc.get(*id).and_then(|n| n.parent()) else {
            continue;
        };
        list_for(&mut sim, from);
        let Some(at_from) = sim[&from].iter().position(|c| c == id) else {
            continue;
        };
        sim.get_mut(&from).expect("just listed").remove(at_from);
        // Lifting a row out of the destination list ahead of the drop slot
        // shifts everything after it down by one.
        if from == target.parent && at_from < slot {
            slot -= 1;
        }
        let to = sim.get_mut(&target.parent).expect("listed first");
        let at = slot.min(to.len());
        to.insert(at, *id);

        if from == target.parent {
            if at != at_from {
                ops.push(Operation::Reorder { id: *id, index: at });
            }
        } else {
            reparented = true;
            ops.extend(build::reparent_preserving_world(doc, res, *id, target.parent, at)?.0);
        }
        slot = at + 1;
    }
    // ⚠️ **A plan whose *composition* changes nothing is no plan** (§15 D532).
    // The no-op test above is per row and compares each row's landing index with
    // the position it had **before** the rows ahead of it were lifted out, so a
    // run of rows dropped at the boundary immediately after itself gives every
    // one of them an `at` that differs from its own `at_from` while the net
    // permutation is the identity. Measured over `[k0, k1, k2]` moving
    // `[k0, k1]`: index 2 produces **two** `Reorder`s and a byte-identical child
    // list, `undo_depth 0 → 1`, the document dirtied — an autosave and a crash
    // snapshot — and a status line claiming *"Reordered 2 layers"*. The
    // single-row version of the same drop is correctly `ops = 0`.
    //
    // ⚠️ **D428's `changes_nothing` cannot see this and it is worth knowing
    // why.** `Operation::Reorder`'s arm asks `child_index(doc, id) ==
    // Some(index)` of the document as it stands, and each op moves its row to an
    // index it is not at *yet*; not one of them is a no-op, so the transaction is
    // not one either. **A composition of real operations that comes back to where
    // it started is a shape no per-operation guard can catch** — which is why the
    // test is here, against the simulation this function already keeps, rather
    // than at the commit.
    //
    // Only when nothing was reparented: a reparent's transform ops are not
    // modelled by `sim`, so a child-list comparison would be answering about half
    // the transaction.
    if !reparented
        && sim
            .iter()
            .all(|(p, list)| doc.get(*p).is_some_and(|n| n.children() == list.as_slice()))
    {
        ops.clear();
    }
    Ok(MovePlan { ops, reparented })
}

/// Which edge of a row an insertion line sits on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum RowEdge {
    Top,
    Bottom,
}

/// Width of the insertion line, in points.
///
/// **Two, asked for on the machine** — *"make the drop line 2px, right now it's a bit too
/// thin"*. It shipped at 2, went to 1 to match the outline that means "inside", and is
/// back: at one point it is a hairline the eye has to hunt for, which is the wrong trade
/// for the single most important thing a drag has to say.
///
/// ⚠️ **This is half of a pair that was deliberately matched, and the other half has not
/// moved.** The 1pt outline was chosen *because* the line was 1pt: at two, "the pair read
/// as belonging to different controls rather than as two answers to the same question".
/// That argument was about two strokes of equal length; it is weaker than it looks here,
/// because a 1pt outline runs the whole way round a 26pt row and a 2pt line is one short
/// horizontal run, so the outline is already the heavier mark of the two by area. Worth
/// re-reading if the outline is ever reported as looking thin beside it.
const DROP_LINE_PT: f32 = 2.0;

/// Where the insertion line goes: the middle of the `gap` between the two rows the
/// boundary separates, rather than either row's own edge.
///
/// **The rows do not touch**, and that is the whole reason this is not just `rect.top()`.
/// The tree lays its rows out with `item_spacing.y` between them, so "the boundary between
/// these two rows" has *two* y coordinates — the upper row's bottom and the lower row's
/// top, one gap apart. Half a gap in from whichever edge is being named lands both on the
/// same place: `upper.bottom() + gap/2` and `lower.top() - gap/2` are the same number by
/// construction.
///
/// That mattered because a boundary used to be named from two different **lists**: the
/// boundary between a frame and its topmost child is "the top of the frame's list" reached
/// from inside the frame, and "beside the frame" reached from the frame's own row — two
/// lists, two owners, two edges. Reported as the line jumping a pixel there and nowhere
/// else, which is exactly one `item_spacing.y`.
///
/// ⚠️ **That report is now unreachable rather than fixed**, and the test below is the only
/// thing still keeping the two spellings honest. [`boundary_y`] names a boundary by its
/// position *on screen*, so there is one owner by construction and only one of the two
/// edges is ever asked for at a given boundary. Which does not make the arithmetic
/// optional: the day something reaches for `rect.top()` again, the line moves a pixel at
/// every boundary rather than at one.
///
/// Centring it in the gap is also where a line between two things belongs.
fn drop_line_y(rect: egui::Rect, edge: RowEdge, gap: f32) -> f32 {
    match edge {
        RowEdge::Top => rect.top() - gap / 2.0,
        RowEdge::Bottom => rect.bottom() + gap / 2.0,
    }
}

/// The y of screen boundary `boundary`, which is the gap **above** `rows[boundary]` — so
/// `0` is the top of the list and `rows.len()` is the bottom of it. `n` rows have `n + 1`
/// of them.
///
/// **Exactly one line, by construction.** This replaced a rule every row evaluated for
/// itself, which is what let the same boundary be drawn twice: two 1pt strokes at one y
/// composite their antialiased fringes into a heavier line, and where the two rows'
/// coordinates fall either side of a device pixel the band doubles outright — reported as a
/// line that visibly changed weight as it moved down the list. Before that it was worse
/// still: a rule that never asked whether the boundary was *this* row's drew a line at
/// every boundary in the list, and two lines around one row read as a box, which is what
/// the tree uses to say "this will go **inside**" (§9.4).
///
/// Naming the boundary from the *screen* rather than from a child list is what makes both
/// of those impossible to write again, and it is the same move indent-aware dropping needed
/// anyway: which levels a drop may take is a fact about the two rows a boundary sits
/// between ([`depth_range`]), and a row painting itself does not know what follows it.
fn boundary_y(rows: &[TreeRow], boundary: usize, gap: f32) -> Option<f32> {
    match boundary.checked_sub(1) {
        // Below the row above it — which is every boundary but the topmost.
        Some(above) => rows
            .get(above)
            .map(|r| drop_line_y(r.rect, RowEdge::Bottom, gap)),
        None => rows.first().map(|r| drop_line_y(r.rect, RowEdge::Top, gap)),
    }
}

/// A horizontal band of `width` points centred on `y`, snapped to whole **device** pixels:
/// the centre and the width to hand the painter.
///
/// The same care `ui::rule` takes over its hairline, and for the same reason. A 2pt line at
/// a fractional y rasterises across three device rows with the outer two only partly
/// covered, which reads as a softer and fatter line than the identical line half a pixel
/// along — and the fractional offset of one boundary in the tree need not match the next,
/// so the drop line changed weight as it moved down the list. Snapping is what makes every
/// boundary draw the same line.
///
/// **Parity is the part that is easy to get wrong**: a band an even number of device pixels
/// wide wants its centre *on* a pixel boundary, an odd one wants it in the middle of a
/// pixel. 2pt is even at 100% and 200% and odd at 150%, so both cases are ordinary.
fn snap_across(y: f32, width: f32, ppp: f32) -> (f32, f32) {
    let px = (width * ppp).round().max(1.0);
    let centre = if (px as i64) % 2 == 0 {
        (y * ppp).round()
    } else {
        (y * ppp).floor() + 0.5
    };
    (centre / ppp, px / ppp)
}

/// The same colour at a fixed alpha, for the 40% treatment a locked or hidden
/// row gets. `Color32` is premultiplied, so the components have to be rescaled
/// with the alpha rather than just having `a` overwritten — doing the latter to
/// the accent-blue frame glyph washed it out to near-white.
fn dim_to(c: egui::Color32, alpha: u8) -> egui::Color32 {
    let [r, g, b, _] = c.to_srgba_unmultiplied();
    egui::Color32::from_rgba_unmultiplied(r, g, b, alpha)
}

/// How far a row at this depth is pushed in, before its caret's own column. Depth
/// 1 is a top-level row — the root is implicit and has no row of its own.
fn indent_of(depth: usize) -> f32 {
    6.0 + (depth as f32 - 1.0) * INDENT
}

/// The x a row at this depth centres its caret on.
///
/// **A named column because the tree guides run down it.** A guide for level `n`
/// is the trunk of an ancestor at depth `n`, and it is drawn in that ancestor's
/// *descendants'* rows — so the one thing that has to hold is that the line and
/// the caret it hangs off agree about where the column is, from rows that never
/// compute each other's geometry.
fn caret_column(left: f32, depth: usize) -> f32 {
    left + indent_of(depth) + 5.0
}

/// The depth the pointer is *asking* for, before the tree gets a say: one level per
/// [`INDENT`] moved sideways from where the drag was picked up.
///
/// ⚠️ **Relative to the grab, never absolute.** Reading the depth straight off the
/// pointer's x is the obvious spelling and it is unusable: a row is picked up by its
/// *name*, which at depth 1 already starts 45pt in, so the drag would jump to depth 2
/// before the pointer had moved at all. What the gesture actually means is "one indent
/// further in than I started", so what it measures is displacement. Figma and every tree
/// that has this read it the same way.
///
/// ⚠️ **`held` is the level this said last frame, and it is what makes the line stay
/// put.** Rounding alone puts a hard edge exactly half an indent from each level's
/// column, and a hand holding a drag steady across it flickers the line between two
/// indents — reported from the machine as *"moving the line between indents is a bit
/// twitchy"*. So the level only changes once the pointer is [`STICK`] of an indent away
/// from the one being held, which is a band 2·`STICK` wide rather than an edge; inside it
/// the answer is simply last frame's. Crossing out of the band re-rounds, so the jump is
/// still to the nearest level and never to a level the hand has passed over.
///
/// The band being wider than half an indent is the whole point and is what makes this
/// hysteresis rather than a bigger dead zone: leaving level 2 rightwards takes `2 +
/// STICK`, and coming back from 3 takes `3 - STICK`, which is a lower x — so a reversal
/// has to be deliberate before it counts.
///
/// Floored at 1 rather than clamped to anything meaningful, because [`drop_spot`] clamps
/// it into [`depth_range`] straight afterwards and that is where the tree's own answer
/// lives. The floor is here only because the arithmetic runs through a `usize`.
///
/// ⚠️ **`held` must be the level this returned, not the clamped one the drop used.** They
/// differ wherever a boundary's range bit, and feeding the clamped value back would let a
/// narrow boundary rewrite where the hand thinks it is — so passing over a one-level gap
/// would silently re-origin the gesture.
fn depth_asked_for(grab_depth: usize, grab_x: f32, x: f32, held: usize) -> usize {
    let raw = grab_depth as f32 + (x - grab_x) / INDENT;
    let level = if (raw - held as f32).abs() > STICK {
        raw.round()
    } else {
        held as f32
    };
    level.max(1.0) as usize
}

/// The depths a drop at screen boundary `boundary` may take, shallowest first.
///
/// **This is the whole of indent-aware dropping.** A gap between two rows is not one
/// insertion point but a run of them, one per level the two rows have in common, and which
/// one the drop takes is the only thing the pointer's x decides. Dragging a row to the
/// bottom of a group and then leftwards walks it out of the group and into the parent —
/// and the same gesture rightwards walks it in — because the boundary below the group's
/// last child spans both levels.
///
/// - **The shallowest** is the depth of the row *below*, and it is a hard floor: anything
///   shallower would have to close containers that the rows below it are still inside. The
///   last boundary in the tree has no row below and so bottoms out at the top level.
/// - **The deepest** is the depth of the row *above*, or the floor if that is deeper.
///
/// ⚠️ **That second clause is the "drop in as the first child" case, and it is why no row
/// has to record whether it is open.** A container's children are drawn immediately below
/// it, so "the row below is deeper than the row above" and "the row above is an expanded
/// container" are the same statement about a tree — and a *collapsed* container has
/// nothing of its own below it, so the range stops at its own level and the only way in is
/// the outline on its row (§9.4). Spelling it as `above.depth + is_expanded` was the first
/// draft and it computes the identical number on every input, which makes `is_expanded` a
/// field with nothing to say and one more thing that could come to disagree with the walk.
///
/// The two ends never cross, which is the same fact once more: whenever the floor is the
/// deeper of the pair it becomes both ends, and the boundary offers exactly one level.
fn depth_range(rows: &[TreeRow], boundary: usize) -> std::ops::RangeInclusive<usize> {
    let above = boundary.checked_sub(1).and_then(|i| rows.get(i));
    let below = rows.get(boundary);
    // No row below is the bottom of the tree, where anything may be dropped at the top
    // level; no row above is the top of it, where the first row's own level is the only
    // offer — and the first row of a tree is always top-level, so the two agree.
    let shallowest = below.map_or(1, |b| b.depth);
    let deepest = above.map_or(shallowest, |a| a.depth).max(shallowest);
    shallowest..=deepest
}

/// Where in the tree the pointer is, before the document has had a say about whether the
/// drop is legal.
///
/// Pure screen geometry, so it can be tested without a document — [`OndinApp::drop_hint`]
/// is what turns one of these into a [`DropTarget`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum DropSpot {
    /// Inside the container row at this index in `rows`, at the top of its list.
    Into(usize),
    /// Between rows, at [`boundary_y`]'s boundary and the given depth.
    Between { boundary: usize, depth: usize },
}

/// What one frame's pointer says: where the drop would go, and the level the indent
/// gesture is now holding.
///
/// **The two are not the same number and the difference is load-bearing.** `spot`'s depth
/// has been clamped into what the boundary can offer; `level` has not, and it is what gets
/// fed back to [`depth_asked_for`] next frame. Feeding the clamped one back would let a
/// boundary that offers a single level quietly re-origin the gesture, so dragging *over* a
/// one-level gap would change what the hand's position means on the far side of it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct DropRead {
    spot: DropSpot,
    level: usize,
}

/// Read the pointer against the drawn rows: which boundary it is at, or which container it
/// is inside.
///
/// **The vertical split is unchanged** — [`EDGE_FRACTION`] of a container row at each end
/// means "between", the middle means "inside" (§9.4) — and a collapsed group still takes a
/// drop only that way, since there is no boundary inside a branch that is not on screen.
///
/// ⚠️ **Horizontally the pointer is allowed to leave the panel to the left, and that is
/// deliberate.** The **whole row** is the drag handle — `allocate_exact_size` takes
/// `available_width()` — so a row can be picked up in the empty gutter left of its glyphs,
/// a few points from the panel's edge, and one [`INDENT`] of leftward travel from there is
/// off the panel altogether. `rect.contains(pointer)` would refuse exactly those drags,
/// leaving the outdent reachable from some grab points and not others, which reads as it
/// working intermittently rather than as a rule.
///
/// **The number to reason from is the grab, not the glyph.** This said "grabbed by its icon
/// at depth 2 is roughly 20pt in", which is wrong and made the case sound tighter than it
/// is: `icon_x` at depth 2 is 47 or 61 depending on whether the row keeps caret room, and an
/// outdent from *there* stays on the panel comfortably. It is the gutter grab that needs the
/// freedom, which is what `the_outdent_still_works_when_the_pointer_has_left_the_panel`
/// uses.
///
/// To the right the rect still bounds it: past the panel is the canvas, and a drop there
/// means nothing.
fn drop_spot(
    rows: &[TreeRow],
    pointer: egui::Pos2,
    grab: (f32, usize),
    held: usize,
) -> Option<DropRead> {
    let (grab_x, grab_depth) = grab;
    // ⚠️ **The first row the pointer has not passed the bottom of, rather than the row it
    // is inside.** The rows do not touch — `item_spacing.y` puts a gap between them — so
    // asking for containment leaves a dead band at every boundary, and the indicator
    // blinks off for the frame the pointer crosses it. Landing in the gap picks the row
    // *below* it, whose `t` then comes out negative and names the boundary the gap is.
    // Off the top of the list is the one case that has no row and is not a drop.
    let (i, row) = rows
        .iter()
        .enumerate()
        .find(|(_, r)| pointer.y <= r.rect.bottom())?;
    if i == 0 && pointer.y < row.rect.top() {
        return None;
    }
    if pointer.x > row.rect.right() {
        return None;
    }
    let t = (pointer.y - row.rect.top()) / row.rect.height();
    if row.container && t > EDGE_FRACTION && t < 1.0 - EDGE_FRACTION {
        // The level is carried through untouched: the pointer is not asking about
        // indents here, and zeroing it would re-origin the gesture on the way past a
        // group.
        return Some(DropRead {
            spot: DropSpot::Into(i),
            level: held,
        });
    }
    // A row's top edge is the boundary of the same index; its bottom edge is the next one.
    let boundary = if t < 0.5 { i } else { i + 1 };
    let range = depth_range(rows, boundary);
    let level = depth_asked_for(grab_depth, grab_x, pointer.x, held);
    Some(DropRead {
        spot: DropSpot::Between {
            boundary,
            depth: level.clamp(*range.start(), *range.end()),
        },
        level,
    })
}

/// Where a row's elbow crosses it: the y the horizontal run sits on, and where the
/// vertical is split.
///
/// **Snapped to a whole device pixel here, not in the painter**, which is the
/// difference between the two runs meeting and missing. The vertical is cut at
/// this y and the horizontal is drawn on it, so a snap applied to one and not the
/// other puts the junction half a pixel out of true — half of the reported gap at
/// the corner, and the half that survives any dash arithmetic.
fn elbow_y(rect: egui::Rect, ppp: f32) -> f32 {
    snap_across(rect.center().y, 1.0, ppp).0
}

/// One drawn row, as everything painted *after* the walk sees it.
///
/// **Collected during the walk and read after it**, because every guide is a
/// statement about rows *other than* the one being painted: the trunk crossing
/// this row belongs to an ancestor several rows up, whether it carries on depends
/// on a row below, and whether it is lit depends on which row the pointer is over
/// — which the walk only learns when it reaches it. One pass cannot answer any of
/// the three.
///
/// **The drop hint is now the second reader, for the same reason.** An
/// indent-aware drop is a claim about a *boundary* — the row above it and the row
/// below it together say which levels the drop may take ([`depth_range`]) — and a
/// row being painted does not yet know what follows it. That is why this is no
/// longer `TreeRow`: the walk collects the tree, and two passes read it.
///
/// Guides are drawn over the row grounds rather than under them, which is what the
/// design's per-row backgrounds do too: a `background-color` sits *behind* the
/// gradients painted on top of it. The drop hint goes over both.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TreeRow {
    /// The layer this row draws. Only the drop pass reads it — a guide is pure
    /// geometry and never asks the document anything.
    id: NodeId,
    /// The row's own box.
    rect: egui::Rect,
    /// 1 for a top-level row, which has no guides at all.
    depth: usize,
    /// Where the elbow has to stop — see [`tree_guide_segments`].
    lead: f32,
    /// Where the elbow crosses the row, [`elbow_y`].
    elbow: f32,
    selected: bool,
    hovered: bool,
    /// A `Group`, `Artboard` or `Boolean` — something the middle of whose row means
    /// "inside it". Whether it is *open* is not recorded and does not need to be:
    /// see [`depth_range`], which reads that off the row below instead.
    container: bool,
    /// A container with children that are **not** on screen — the one thing about a
    /// row's own state that the drop pass cannot work out for itself, and the one
    /// spring-loading acts on.
    ///
    /// ⚠️ **Not the negation of the `expanded` flag this struct used to carry, which
    /// is why one came back after the other went.** That flag was removable because an
    /// *open* container is exactly a row whose successor is deeper, so the row list
    /// already said it. A *closed* one is invisible in the same list: it has no rows
    /// below it and neither does a leaf, and nothing on screen tells the two apart.
    closed: bool,
}

/// A dashed run of tree guide, and what it is saying.
#[derive(Clone, Copy, Debug, PartialEq)]
struct GuideSeg {
    from: egui::Pos2,
    to: egui::Pos2,
    ink: GuideInk,
}

/// What a run of guide is saying — and, read as a ranking, which claim wins where
/// two of them cover one run.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum GuideInk {
    /// Structure and nothing else.
    Rest,
    /// On the path to the row under the pointer. **Beats [`Selected`], because the
    /// pointer is a question being asked now where a selection is a state that
    /// persists** — and a hover that could not be seen over a selected leg would
    /// leave the panel unable to answer it. *"Hover takes precedence."*
    ///
    /// [`Selected`]: GuideInk::Selected
    Hovered,
    /// On the path to a selected row: `color::SELECT`, the same blue the row's own
    /// kind glyph wears, so the leg and the glyph are one statement about one
    /// layer. Full strength rather than a tier of it — a dashed hairline inks 37%
    /// of its length, so the blue reads as a tint without being mixed as one.
    ///
    /// **A selected row under the pointer keeps this**: it is already the row the
    /// panel is talking about, so there is nothing for a hover to add. Asked for
    /// exactly that way — *"the currently selected layer doesn't apply a hover
    /// anymore"*.
    Selected,
}

impl GuideInk {
    fn colour(self) -> egui::Color32 {
        match self {
            Self::Rest => GUIDE_REST,
            Self::Hovered => GUIDE_HOVER,
            Self::Selected => color::SELECT,
        }
    }
}

/// What one row's guides run over: the level its parent's trunk is on, and the
/// half-open range of rows **above** it that trunk crosses.
///
/// The range stops before the row itself, whose connector and elbow are on the path
/// but whose trunk *below* that elbow is not — that stretch belongs to the siblings
/// further down and says nothing about this row. `None` for a top-level row, which
/// hangs off nothing.
fn guide_path(rows: &[TreeRow], row: usize) -> Option<(usize, std::ops::Range<usize>)> {
    let level = rows[row].depth.checked_sub(1).filter(|l| *l >= 1)?;
    // The parent's own row — or, in a tree filtered down to rows whose parent is
    // not among them, the row itself, which then claims nothing above it.
    let start = rows[..row]
        .iter()
        .rposition(|r| r.depth == level)
        .map_or(row, |p| p + 1);
    Some((level, start..row))
}

/// For each row, the levels whose trunk carries on *below* it — bit `n - 1` for
/// level `n`.
///
/// The trunk of a level-`n` ancestor carries on past a row when that ancestor has
/// another child after the branch this row sits in. Read off the displayed order
/// that is: scanning down, the first row shallow enough to close the branch — depth
/// `n + 1` or less — is at exactly `n + 1`.
///
/// **One pass up the list rather than that scan per row and level**, so nesting
/// cannot make this quadratic: a row at depth `d` sets the bit for level `d - 1`,
/// being one of that ancestor's children, and clears every level from `d` down,
/// being the row that closes those branches.
///
/// Levels past 64 are not tracked and their trunks are not drawn. Level 64 starts
/// `indent_of(64)` — 1959pt — from the panel's edge, which is a guide nobody can
/// see in a 300pt column.
fn trunks_below(rows: &[TreeRow]) -> Vec<u64> {
    let mut out = vec![0u64; rows.len()];
    let mut mask = 0u64;
    for (i, row) in rows.iter().enumerate().rev() {
        out[i] = mask;
        let d = row.depth.clamp(1, 64);
        mask &= (1u64 << (d - 1)) - 1;
        if d >= 2 {
            mask |= 1 << (d - 2);
        }
    }
    out
}

/// Every dashed run the tree guides need, for rows in the order they are drawn
/// (§15 D331).
///
/// Three things make up a row's guides, and the design draws all three as
/// backgrounds on the row itself:
///
/// - **the trunks**, one per ancestor whose branch is still open, each crossing the
///   row down the caret column of the level it belongs to ([`caret_column`]);
/// - **the connector**, the part of the *parent's* trunk above this row's elbow —
///   the same column, and what makes the row hang off its parent rather than
///   merely sit under it;
/// - **the elbow**, a horizontal run from that column out to `lead`, which is
///   where the row's own leading glyph begins.
///
/// ⚠️ **A trunk whose branch has already closed is not drawn at all**, which is the
/// one place this departs from the design's markup. There a row carries a
/// stub-height background for *every* level, which is right for the fixture it was
/// drawn against — no ancestor in it dies above a deeper row. Read as a rule it
/// would leave a fragment in mid-air: with `A → B → C` each an only child, A's
/// trunk ends at B's elbow, and a stub at A's column in C's row is a dash under a
/// line that stopped a row earlier.
///
/// **Hovering lights the path from the parent, not the whole ancestry** — and a
/// selection lights the same path in `color::SELECT`. The run is the parent's trunk
/// from the row under the parent down to that row's elbow, so it answers "which one
/// is this inside" once, rather than drawing a lit ladder the eye then has to read.
/// Below the elbow the same trunk goes back to [`GUIDE_REST`], and no other level
/// lights at all.
///
/// **The two claims overlap and hover wins the overlap, one run at a time** — which
/// is what makes "only on the part that applies to that layer" true: hovering a row
/// between a selected row and their shared parent greys the trunk down to the
/// hovered row's own elbow and leaves the rest of the selected leg blue. See
/// [`GuideInk`] for both halves of the precedence, including the one exception.
fn tree_guide_segments(rows: &[TreeRow], gap: f32) -> Vec<GuideSeg> {
    let below = trunks_below(rows);
    // A hovered row that is itself selected has no hover path at all — the `filter`
    // is the whole of "the selected layer no longer applies a hover".
    let hovered = rows
        .iter()
        .position(|r| r.hovered && !r.selected)
        .and_then(|h| guide_path(rows, h).map(|(l, over)| (h, l, over)));
    // Every selected row, because a selection is a set: two selected siblings light
    // two legs off one trunk, and the union is what gets drawn.
    let selected: Vec<(usize, usize, std::ops::Range<usize>)> = rows
        .iter()
        .enumerate()
        .filter(|(_, r)| r.selected)
        .filter_map(|(i, _)| guide_path(rows, i).map(|(l, over)| (i, l, over)))
        .collect();
    // Is this run on that path? The whole trunk for a row the path merely crosses,
    // and only the connector for the row it ends at.
    let on = |(owner, plevel, over): &(usize, usize, std::ops::Range<usize>),
              i: usize,
              level: usize,
              connector: bool| {
        *plevel == level && (over.contains(&i) || (connector && i == *owner))
    };
    let mut out = Vec::new();
    for (i, row) in rows.iter().enumerate() {
        let (top, elbow) = (row.rect.top(), row.elbow);
        let trunk_ink = |level: usize, connector: bool| {
            if hovered.as_ref().is_some_and(|p| on(p, i, level, connector)) {
                GuideInk::Hovered
            } else if selected.iter().any(|p| on(p, i, level, connector)) {
                GuideInk::Selected
            } else {
                GuideInk::Rest
            }
        };
        for level in 1..row.depth.min(64) {
            let x = caret_column(row.rect.left(), level);
            let carries_on = below[i] & (1 << (level - 1)) != 0;
            if level + 1 == row.depth {
                // The parent's trunk: drawn down to the elbow whether the branch
                // continues past this row or ends on it.
                out.push(GuideSeg {
                    from: egui::pos2(x, top),
                    to: egui::pos2(x, elbow),
                    ink: trunk_ink(level, true),
                });
                if carries_on {
                    out.push(GuideSeg {
                        from: egui::pos2(x, elbow),
                        to: egui::pos2(x, row.rect.bottom() + gap),
                        ink: trunk_ink(level, false),
                    });
                }
            } else if carries_on {
                out.push(GuideSeg {
                    from: egui::pos2(x, top),
                    to: egui::pos2(x, row.rect.bottom() + gap),
                    ink: trunk_ink(level, false),
                });
            }
        }
        if row.depth >= 2 {
            // Out of the parent's column, [`ELBOW_CLEAR`] clear of its ink, to the
            // row's leading glyph. The elbow belongs to this row alone — an
            // intervening row's stays at rest while the trunk through it lights,
            // which is what keeps a lit path a line rather than a comb.
            let x = caret_column(row.rect.left(), row.depth - 1);
            out.push(GuideSeg {
                from: egui::pos2(x + ELBOW_CLEAR, elbow),
                to: egui::pos2(row.lead, elbow),
                ink: match hovered {
                    Some((h, ..)) if h == i => GuideInk::Hovered,
                    _ if row.selected => GuideInk::Selected,
                    _ => GuideInk::Rest,
                },
            });
        }
    }
    out
}

/// Paint what [`tree_guide_segments`] worked out.
///
/// **The across-the-run snapping is here rather than in the geometry**, the way
/// [`drop_line_y`] and [`snap_across`] already split: what column a guide belongs
/// in is a fact about the tree, and which device pixel that lands on is a fact
/// about the monitor. A 1pt line centred on a whole point covers half of two
/// pixels, which at [`GUIDE_REST`]'s alpha is a guide that reads as a smudge rather
/// than a line. The elbow's *own* y is the exception and is snapped in
/// [`elbow_y`], because it is a coordinate two runs have to agree on rather than
/// one run's offset from its column — so for a horizontal run this is idempotent.
fn paint_tree_guides(painter: &egui::Painter, rows: &[TreeRow], gap: f32, ppp: f32) {
    for seg in tree_guide_segments(rows, gap) {
        let vertical = seg.from.x == seg.to.x;
        let across = if vertical { seg.from.x } else { seg.from.y };
        // 1pt, the design's 1px gradient.
        let (across, width) = snap_across(across, 1.0, ppp);
        let (from, to) = if vertical {
            (egui::pos2(across, seg.from.y), egui::pos2(across, seg.to.y))
        } else {
            (egui::pos2(seg.from.x, across), egui::pos2(seg.to.x, across))
        };
        painter.extend(egui::Shape::dashed_line(
            &[from, to],
            egui::Stroke::new(width, seg.ink.colour()),
            GUIDE_DASH,
            GUIDE_GAP,
        ));
    }
}

impl OndinApp {
    pub(crate) fn layers_tree(&mut self, ui: &mut egui::Ui) {
        let root = self.session.doc.root();
        // Re-derive the drop target from scratch each frame: only the row the
        // pointer is actually over gets to set one, so dragging off the tree
        // and releasing drops nothing rather than acting on a stale target.
        //
        // **The container it named is kept for exactly one frame on the way past**, so
        // the walk below can light that row's ink — see [`LayerDrag::lit`], which is
        // where the reason a frame's lag is the only option lives. Reading it here, in
        // the one place the answer is thrown away, is what stops the two drifting: there
        // is no second assignment that could forget.
        if let Some(drag) = self.layer_drag.as_mut() {
            drag.lit = drag.target.map(|t| t.parent);
            drag.target = None;
            ui.ctx().request_repaint();
        }
        self.sync_tree_to_selection();
        // An empty search box is not a search: it would hide the whole tree the
        // instant the field opened.
        let needle = self
            .layer_filter
            .as_ref()
            .map(|f| f.trim().to_lowercase())
            .filter(|f| !f.is_empty());
        let shown = needle.as_deref().map(|n| self.matching_rows(n));

        // Every row the walk draws, for the guides and the drop hint to be worked out
        // from once it is over ([`TreeRow`]).
        let mut drawn = Vec::new();
        match &shown {
            Some(rows) if rows.is_empty() => {
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new("No layers match")
                        .size(11.5)
                        .color(theme::text::FAINT),
                );
            }
            // Collected whether or not the guides are switched on — a handful of
            // numbers per row against a walk that lays out and paints one, and the
            // alternative is a second reading of the preference per row.
            _ => self.layers_row(ui, root, 0, shown.as_ref(), &mut drawn),
        }
        if self.prefs.tree_guides {
            paint_tree_guides(
                ui.painter(),
                &drawn,
                ui.spacing().item_spacing.y,
                ui.ctx().pixels_per_point(),
            );
        }
        // **After the guides, so the hint is on top of them.** It answers a question
        // being asked right now where a guide is standing structure, and the line
        // crosses a trunk at every level it is indented past.
        self.drop_hint(ui, &drawn);

        // Resolve the drag when the **primary** button is released anywhere
        // (§15 D581). `any_released()` was the workspace's only one, against
        // three `primary_released()` in `canvas.rs`, and the difference was
        // reachable: clicking the scroll wheel part-way through a row drag
        // landed the drop at whatever the pointer happened to be over, with the
        // primary still held and now controlling nothing. The secondary button
        // is already answered one layer up — `OndinApp::ui` runs `cancel_gesture`
        // before the panels and that clears `layer_drag` by name — so the middle
        // button was the one door with no answer at either end.
        if ui.input(|i| i.pointer.primary_released()) {
            self.finish_layer_drag();
        }
    }

    /// What a click on a row selects, given the modifiers.
    ///
    /// **Shift takes a range, Ctrl/Cmd toggles one row** — the split every list
    /// in every file manager and design tool has, and Figma's own in this exact
    /// panel. Shift used to toggle here, which left range-selecting a run of
    /// twenty layers as twenty clicks and left Ctrl doing nothing at all.
    ///
    /// This is not the canvas's Shift, and the two are not in conflict. On canvas
    /// there is no order to take a range *along*, so Shift adds and removes there
    /// — again as Figma does. §15 D47 governs Shift during a transform gesture
    /// (constrain, never snap-disable) and has nothing to say about either.
    ///
    /// The range runs along the rows **as displayed**: collapsed subtrees are not
    /// in it, and while a filter is up neither is anything filtered out. Picking a
    /// range off the screen and getting layers that were not on it is the one
    /// outcome nobody expects.
    fn select_from_row(&mut self, id: NodeId, shown: Option<&HashSet<NodeId>>, ui: &egui::Ui) {
        let (shift, ctrl) =
            ui.input(|i| (i.modifiers.shift, i.modifiers.ctrl || i.modifiers.command));
        if ctrl && !shift {
            self.session.selection.toggle(id);
            self.layers_range_anchor = Some(id);
            return;
        }
        if shift {
            let rows = self.visible_rows(shown);
            let anchor = self
                .layers_range_anchor
                .and_then(|a| rows.iter().position(|r| *r == a));
            let here = rows.iter().position(|r| *r == id);
            if let (Some(a), Some(b)) = (anchor, here) {
                let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
                self.session.selection.set(rows[lo..=hi].to_vec());
                // The anchor stays put, so the range can be stretched and pulled
                // back from the other end without re-picking its start.
                return;
            }
            // No anchor to measure from — the first Shift-click of a session, or
            // one whose anchor has been collapsed away. Behave as a toggle rather
            // than doing nothing.
            self.session.selection.toggle(id);
            self.layers_range_anchor = Some(id);
            return;
        }
        self.session.selection.set_one(id);
        self.layers_range_anchor = Some(id);
    }

    /// The rows the tree is currently showing, top to bottom.
    ///
    /// **Must walk exactly as [`Self::layers_row`] draws** — same filter, same
    /// collapse check, same reversed children — because a range is measured
    /// along it and any disagreement would select rows the user cannot see. The
    /// two are separate only because one paints and one does not.
    fn visible_rows(&self, shown: Option<&HashSet<NodeId>>) -> Vec<NodeId> {
        let mut out = Vec::new();
        self.collect_visible_rows(self.session.doc.root(), 0, shown, &mut out);
        out
    }

    fn collect_visible_rows(
        &self,
        id: NodeId,
        depth: usize,
        shown: Option<&HashSet<NodeId>>,
        out: &mut Vec<NodeId>,
    ) {
        if let Some(rows) = shown
            && !rows.contains(&id)
        {
            return;
        }
        let Some(node) = self.session.doc.get(id) else {
            return;
        };
        let children: Vec<NodeId> = match shown {
            Some(rows) => node
                .children()
                .iter()
                .copied()
                .filter(|c| rows.contains(c))
                .collect(),
            None => node.children().to_vec(),
        };
        if depth > 0 {
            out.push(id);
            // A filter forces every branch open, exactly as the drawing walk does.
            let expanded =
                !children.is_empty() && (shown.is_some() || !self.collapsed.contains(&id));
            if !expanded {
                return;
            }
        }
        for child in children.into_iter().rev() {
            self.collect_visible_rows(child, depth + 1, shown, out);
        }
    }

    /// The in-place rename field, laid over the row where its name was.
    ///
    /// Buffered rather than committed per keystroke, like the paint hex field:
    /// one rename is one undo step, not one per character. Enter and clicking
    /// away both keep it; Escape throws it away, which is the only way back out
    /// of a rename started by accident. A name typed empty is discarded too — a
    /// nameless row is unusable, and silently keeping the old name is kinder than
    /// refusing with a message.
    fn layer_rename_field(&mut self, ui: &mut egui::Ui, id: NodeId, row: egui::Rect, left: f32) {
        let Some((_, text)) = &self.renaming_layer else {
            return;
        };
        let mut buf = text.clone();
        // ⚠️ **The caret is asked for here, on the frame the field first draws, and
        // the alternative is a crash** (§15 D332, superseding D216's mechanism).
        // Every door onto a rename used to request
        // focus at the moment it *opened* one — the double-click below, and
        // `OndinApp::rename_selection` for the chord and the menu row — on the
        // documented assumption that egui holds a request until the widget appears.
        // It does. What it also does is hand AccessKit a focused id with no node
        // behind it, and `accesskit_consumer` panics on that rather than ignoring
        // it: *"Focused ID #… is not in the node list"*. Two of those doors could
        // not draw the field in the frame that asked — the double-click reads a
        // `renaming` flag latched earlier in the row, and the context menu is drawn
        // *after* the layers panel — so both crashed on the way in.
        //
        // `read_response` is the previous pass's, so `None` is "this field did not
        // exist last pass" and the request is a **one-shot on first draw**. Which
        // is what makes it safe to ask from in here at all: the trap the old comment
        // warned about is a re-request gated on *not having* focus, since that also
        // fires on the frame Enter or Escape takes focus away — the field grabs it
        // straight back and can never be left.
        let fid = rename_field_id(id);
        if ui.ctx().read_response(fid).is_none() {
            ui.ctx().memory_mut(|m| m.request_focus(fid));
        }
        // Sits exactly where the painted name sat — see `rename_field_rect`.
        // `TextEdit` insets its text by `Margin::symmetric(4, 2)` and aligns it
        // `LEFT_TOP` by default, which is why both are overridden below.
        let field = rename_field_rect(row, left);
        let resp = ui.put(
            field,
            egui::TextEdit::singleline(&mut buf)
                .id(rename_field_id(id))
                .frame(egui::Frame::NONE)
                .margin(egui::Margin::ZERO)
                // `TextEdit` aligns its text `LEFT_TOP` by default, and `put`
                // stretches it over the whole row, so without this the name
                // rides up to the row's top edge while every other row's sits on
                // its middle.
                .vertical_align(egui::Align::Center)
                .desired_width(field.width())
                .font(egui::FontId::proportional(12.5))
                .text_color(color::TEXT),
        );
        if let Some((_, stored)) = &mut self.renaming_layer {
            *stored = buf.clone();
        }

        // **A press anywhere else ends the rename**, whatever that somewhere is —
        // another row, a panel, the canvas. egui moves focus by itself only when
        // the thing clicked is focusable, and most of this app's surface is
        // painted by hand and senses clicks without ever taking focus, so a click
        // on the artwork would otherwise leave the field open and the edit
        // uncommitted. Gated on the field actually holding focus, so the second
        // press of the double-click that opened it cannot close it again.
        if resp.has_focus() {
            let pressed_elsewhere = ui.input(|i| i.pointer.any_pressed())
                && ui
                    .ctx()
                    .pointer_interact_pos()
                    .is_none_or(|p| !field.contains(p));
            if pressed_elsewhere {
                ui.ctx()
                    .memory_mut(|m| m.surrender_focus(rename_field_id(id)));
            }
        }

        // Escape is read before the commit, not after: it is the one way out of
        // a rename started by accident, and by the time focus is gone the key is
        // the only thing left saying which of the two happened.
        if resp.lost_focus() {
            let cancelled = ui.input(|i| i.key_pressed(egui::Key::Escape));
            let name = buf.trim().to_string();
            self.renaming_layer = None;
            if !cancelled
                && !name.is_empty()
                && self.session.doc.get(id).is_some_and(|n| n.name() != name)
            {
                self.session
                    .commit(Transaction(vec![Operation::SetName { id, name }]));
            }
        }
    }

    /// Follow a selection made somewhere else — almost always the canvas.
    ///
    /// Two things have to happen, and only on the frame the selection actually
    /// changed. **Open what it is buried in**, because a layer selected on canvas
    /// that sits three collapsed frames deep is simply not in the tree, and the
    /// panel silently showing nothing selected reads as a bug. And **ask to
    /// scroll to it**, because a long tree scrolled elsewhere is the same problem
    /// with the row merely off screen instead of folded away.
    ///
    /// Collapsing a frame the selection is inside is left alone: that is the user
    /// closing something deliberately, and re-opening it under them would make
    /// the caret look broken. Only a *change of selection* re-opens anything.
    fn sync_tree_to_selection(&mut self) {
        let current = self.session.selection.ids().to_vec();
        if current == self.layers_synced_selection {
            return;
        }
        self.layers_synced_selection = current.clone();
        for id in &current {
            // The whole chain, with no early exit. Collapse state is per node and
            // independent — an open frame can sit inside a closed one — so unlike
            // `matching_rows`, meeting an ancestor that is already open says
            // nothing at all about the ones above it.
            let mut cursor = self.session.doc.get(*id).and_then(|n| n.parent());
            while let Some(p) = cursor {
                self.collapsed.remove(&p);
                cursor = self.session.doc.get(p).and_then(|n| n.parent());
            }
        }
        self.scroll_layers_to_selection = !current.is_empty();
    }

    // **`autoscroll_while_dragging` was here and is deleted** (§15 D576,
    // `[S15.1-L1-01]`).
    //
    // The tree had **two** drag-to-edge autoscrollers, both reached every frame a
    // drag was live: this one, called from `layers_tree`, and `app::layers_panel`'s
    // band, which runs immediately above the call to `layers_tree` in the same
    // `ScrollArea`. Both read `ui.clip_rect()` and the tree's only margin is
    // horizontal, so the two bands sat on the identical y range.
    //
    // `egui::Ui::scroll_with_delta_animation` **accumulates** (`state.scroll_delta.0
    // += delta`), so they added: 16.0 pt per frame at the very edge, **960 pt/s at
    // 60 Hz**, against `LAYER_SCROLL_MAX`'s own doc — *"420 … points per second.
    // Slower than the canvas: the target is one 20px row, not a region"*. And only
    // one of the two was `dt`-scaled, so it read **1500 pt/s at 120 Hz**, which is
    // the thing `* dt` exists to prevent.
    //
    // ⚠️ **The animation is last-write-wins, so this one silently undid the other's
    // decision.** `layers_panel` passes `ScrollAnimation::none()` under a comment
    // saying why — *"egui's default easing would keep gliding after the hand stopped
    // and overshoot the row being aimed at"* — and this called plain
    // `scroll_with_delta`, which is the style default. The glide that comment says
    // was removed was reinstated one call later, every frame.
    //
    // ⚠️ **And the linear ramp swamped the quadratic one.** `ui::edge_scroll_speed`
    // is `t²` so *"a creep is available for most of the band"*; 14pt into the band
    // this one's linear term was **4.8×** the quadratic one, so the creep the curve
    // was shaped for did not exist.
    //
    // Deleted rather than merged: `app::layers_panel`'s is the one §9.4 and D278
    // describe, and it is the `dt`-scaled, quadratic, instant one. **Two
    // implementations of one feature, each with a doc comment claiming to be the
    // only one** — this one's said *"without this the panel can only be reordered
    // within one screenful"*, which was true of the other.

    /// Fold every container, or unfold them all when nothing is folded.
    ///
    /// One button rather than two: with a tree of any size the answer to "are
    /// these all open" is obvious at a glance, so a control that reads the room
    /// and does the other thing is a button's worth of chrome instead of two.
    pub(crate) fn toggle_collapse_all(&mut self) {
        let containers = self.tree_containers();
        let anything_open = containers.iter().any(|id| !self.collapsed.contains(id));
        if anything_open {
            self.collapsed.extend(containers);
        } else {
            self.collapsed.clear();
        }
    }

    /// Fold every container, whatever state the tree is in.
    ///
    /// **Not [`Self::toggle_collapse_all`], and the difference is the whole reason
    /// this exists.** That one reads the room; this one is the answer to
    /// `prefs::Prefs::collapse_groups_on_open`, which is asked immediately after
    /// `reset_transient_state` has emptied `collapsed` — so the toggle would find
    /// "everything open", fold it, and then be a *toggle* on a tree the user has
    /// not looked at yet. Two verbs because there are two questions.
    pub(crate) fn collapse_all(&mut self) {
        let containers = self.tree_containers();
        self.collapsed.extend(containers);
    }

    /// Every node in the tree that has children, root excluded — the things a
    /// caret can fold.
    fn tree_containers(&self) -> Vec<NodeId> {
        let root = self.session.doc.root();
        ondin_core::subtree_nodes(&self.session.doc, &[root])
            .into_iter()
            .filter(|id| {
                *id != root
                    && self
                        .session
                        .doc
                        .get(*id)
                        .is_some_and(|n| !n.children().is_empty())
            })
            .collect()
    }

    /// Whether every container in the tree is folded — what the collapse-all
    /// button reads to decide which way it points.
    ///
    /// ⚠️ **Through [`Self::tree_containers`], and it used to walk the tree itself**
    /// (§15 D674, `[S15.1-L3-02]`). It re-inlined that function's two-clause
    /// container filter with `&& !self.collapsed.contains(&id)` bolted on, so
    /// *"what counts as a container"* was written twice — once for the button's
    /// **glyph**, which is this, and once for the button's **action**, which is
    /// [`Self::toggle_collapse_all`]'s `anything_open`. Widen or narrow one and the
    /// arrow points one way while the press does the other, with nothing in between
    /// to disagree. `architecture.md` names the two functions in one breath as the
    /// argument for the control existing at all — *"a control that looks and does
    /// the other thing costs one button instead of two"* — which is the coupling
    /// stated and, until this, not enforced.
    ///
    /// This is the exact negation of `anything_open`, which is what makes the pair
    /// one sentence rather than two agreeing ones. It also drops the second full
    /// `subtree_nodes` walk per frame — this is read on every frame the panel
    /// draws, where the toggle's walk is per click.
    pub(crate) fn tree_is_all_collapsed(&self) -> bool {
        self.tree_containers()
            .iter()
            .all(|id| self.collapsed.contains(id))
    }

    /// Rows to show for a name filter: every node whose name matches, plus the
    /// ancestors needed to reach it.
    ///
    /// Ancestors matter — a bare list of matches would show "Headline" with no
    /// indication of which frame it belongs to, and there are usually several.
    /// Non-matching *descendants* are dropped, so a matched frame does not drag
    /// its whole subtree back into view.
    fn matching_rows(&self, needle: &str) -> HashSet<NodeId> {
        let mut keep = HashSet::new();
        let mut stack = vec![self.session.doc.root()];
        while let Some(id) = stack.pop() {
            let Some(node) = self.session.doc.get(id) else {
                continue;
            };
            if node.name().to_lowercase().contains(needle) {
                keep.insert(id);
                let mut cursor = node.parent();
                // Stop at the first ancestor already recorded: whenever one was
                // added, its own ancestors were added with it.
                while let Some(p) = cursor {
                    if !keep.insert(p) {
                        break;
                    }
                    cursor = self.session.doc.get(p).and_then(|n| n.parent());
                }
            }
            stack.extend(node.children().iter().copied());
        }
        keep
    }

    fn layers_row(
        &mut self,
        ui: &mut egui::Ui,
        id: NodeId,
        depth: usize,
        shown: Option<&HashSet<NodeId>>,
        rows: &mut Vec<TreeRow>,
    ) {
        if let Some(rows) = shown
            && !rows.contains(&id)
        {
            return;
        }
        let Some(node) = self.session.doc.get(id) else {
            return;
        };
        let children: Vec<NodeId> = match shown {
            Some(rows) => node
                .children()
                .iter()
                .copied()
                .filter(|c| rows.contains(c))
                .collect(),
            None => node.children().to_vec(),
        };
        let visible = node.visible();
        let locked = node.locked();
        let name = node.name().to_string();
        let (glyph, mut glyph_col) = node_icon(node.kind());
        // **A layer whose picture cannot be drawn wears the warning in its own
        // icon** — §5.5a's rule that a missing picture is never a silent blank has
        // a layers half as well as a canvas one (§15 D179). Asked of the
        // decode cache rather than of the document, so it is the same question the
        // walk asks before drawing the placeholder — a row saying nothing while
        // the canvas shows a grey cross would be the two disagreeing about one
        // layer.
        // **And a boolean the arithmetic gave up on wears exactly the same mark**,
        // because it is the same fact about a layer: it is there, and what it draws
        // is not what it says it is (§15 D239). One `if` and no new colour — the row
        // has to be readable at a glance while scrolling, and a second warning
        // vocabulary for a second cause would be two things to learn about one
        // symptom. `boolean_failed`, never "the outline is missing": an intersection
        // of shapes that do not overlap is empty on purpose and must not be marked.
        if picture_missing(self.canvas.images(), node) || self.session.resolved.boolean_failed(id) {
            glyph_col = theme::color::WARN;
        }
        // Cloned rather than borrowed because the node's borrow has to end
        // before the painting below — see the note a few lines down. An
        // An `ImageBrush` is an `ImageRef` — five small fields and a hash string —
        // beside a sampler, not the picture.
        let picture = picture_of(node).cloned();
        // **A boolean takes a drop like a group does.** Its children *are* its
        // operands, so dropping a shape onto one is how a third operand gets added —
        // and `build::can_parent` has always allowed it. Left out here, the middle of a
        // boolean's row offered only "between its siblings", so the one way in was to
        // aim at the gap between two operands with the row expanded: legal, and with no
        // highlight to say the drop was going inside.
        let is_container = matches!(
            node.kind(),
            NodeKind::Group | NodeKind::Artboard { .. } | NodeKind::Boolean { .. }
        );
        // The right-hand end of a resting row. **Two states, one slot**, because
        // both are the same kind of remark — a fact about the row that the name
        // and the glyph do not carry — and because the slot is already the one
        // the eye and the lock take over on hover, so a second one would have to
        // fight them for the same space. No row can want both: a frame is never
        // an operand of anything.
        // **Three states now, and the mask wins the slot.** It is the one of the
        // three that changes what you see: a mask draws nothing, so its row is the
        // only place in the app that says the layer is still there. The other two
        // are annotations on a layer you can already see.
        //
        // No row can want two of them. A frame cannot be a mask
        // (`NodeKind::can_mask`), and a boolean's operands are consumed by the
        // operation rather than painted, so a mask among them would clip nothing.
        let badge = if node.mask() {
            Some(RowBadge::Icon(icon::CIRCLE_HALF, BADGE_MASK))
        } else {
            match node.kind() {
                NodeKind::Artboard { size, .. } => Some(RowBadge::Text(format!(
                    "{} × {}",
                    size.width as i64, size.height as i64
                ))),
                _ => subtract_base(&self.session.doc, node)
                    .then_some(RowBadge::Icon(icon::STACK_SIMPLE, BADGE_STACK)),
            }
        };
        // (node borrow ends here — safe to mutate self below)

        // Root is implicit; the tree starts at its children.
        if depth > 0 {
            let selected = self.session.selection.contains(id);
            let has_children = !children.is_empty();
            // While filtering, every branch is open: a match hidden inside a
            // collapsed ancestor is a search that found nothing.
            let expanded = has_children && (shown.is_some() || !self.collapsed.contains(&id));

            let (rect, resp) = ui.allocate_exact_size(
                egui::vec2(ui.available_width(), ROW_H),
                egui::Sense::click_and_drag(),
            );
            let hovered = resp.hovered();
            let being_dragged = self
                .layer_drag
                .as_ref()
                .is_some_and(|d| d.ids.contains(&id));

            // ⚠️ **Where the drop target is worked out is no longer here**, and the
            // trap that put it here is worth keeping: it must be read from
            // `pointer_interact_pos` against a row's *rect*, never from
            // `resp.hovered()`. While a drag is in flight egui reports only the
            // dragged widget as hovered ("If currently dragging, only that and
            // nothing else is hovered"), so every other row said no, no drop target
            // was ever recorded, and releasing did nothing at all — which is what
            // made the whole reorder/reparent gesture look unimplemented (§15 D19).
            // [`OndinApp::drop_hint`] reads the rects this walk collects, after it,
            // because an indent-aware drop is a claim about two rows at once.

            // Asked before the painter is taken, so the `&mut self.thumbs` and
            // the `&self.canvas` borrows do not have to share a line with one
            // borrowed out of `ui`. `None` for a layer with no picture, one the
            // store cannot draw, or one the pass's budget deferred — and all
            // three fall back to the glyph, which for the second is the amber
            // one D179 already set above.
            let thumb = picture.as_ref().and_then(|b| {
                self.thumbs
                    .texture(ui.ctx(), self.canvas.images(), &b.image, THUMB)
            });
            let p = ui.painter();
            let r5 = egui::CornerRadius::same(5);
            if being_dragged {
                p.rect_filled(rect, r5, theme::color::text_a(20));
            } else if selected {
                p.rect_filled(rect, r5, color::SELECT_ROW);
            } else if hovered {
                p.rect_filled(rect, r5, theme::color::text_a(12));
            }
            let cy = rect.center().y;
            let left = rect.left();
            let caret_x = caret_column(left, depth);
            // **Right-side controls: eye / lock** (on hover, or when toggled off).
            // [`EYE_PITCH`] apart — the glyphs are 15px, so the old 18px pitch left
            // them almost touching.
            //
            // Their x's are worked out **here**, above the caret they are drawn
            // after, because all three go into one [`RowTargets`] and the caret
            // is the first thing painted. Nothing about them depends on anything
            // between the two points.
            let lock_x = rect.right() - LOCK_INSET;
            let eye_x = lock_x - EYE_PITCH;
            let show_eye = hovered || !visible;
            let show_lock = hovered || locked;
            let targets = RowTargets {
                caret: has_children.then_some(caret_x),
                eye: show_eye.then_some(eye_x),
                // Not gated on `show_lock`, which is how the click test has always
                // read it: a press can only arrive with the pointer on the row, and
                // the pointer on the row is what `show_lock` already means.
                lock: Some(lock_x),
            };
            // **The control the pointer is actually on**, as opposed to the row —
            // which the row's own `hovered` already says and which is what makes
            // the eye and lock appear at all. Two different questions on one
            // response, and until §15 D246 only the first was asked, so all three
            // controls arrived with nothing to say they could be pressed.
            let under = resp.hover_pos().and_then(|q| targets.at(q.x));
            // A hovered control lights to `STRONG`, the app's brightest ink, in
            // every state it has. **One destination rather than one step per
            // resting colour**: the eye rests at two greys and the lock at two
            // more, and "a tier up from wherever you were" would give four
            // different amounts of feedback for one gesture. No ground behind it
            // — the row already has a hover ground of its own, and a second
            // rectangle inside it would read as a button the rest of the panel
            // does not have.
            let lit = |on: bool, resting: egui::Color32| {
                if on { theme::text::STRONG } else { resting }
            };
            if has_children {
                p.text(
                    egui::pos2(caret_x, cy),
                    egui::Align2::CENTER_CENTER,
                    if expanded {
                        icon::CARET_DOWN
                    } else {
                        icon::CARET_RIGHT
                    },
                    theme::icon_font(11.0),
                    lit(under == Some(RowHit::Caret), theme::text::DIM),
                );
            }
            // A childless row normally reclaims the caret's 16px, which reads
            // fine mid-tree where the indent has already said how deep you are.
            // At the **top level** it does not: a frame with a caret would sit
            // further right than the loose shape above it and look like that
            // shape's child. So depth 1 always leaves the caret's room, whether
            // it has one or not, and the top level lines up as one column.
            let caret_room = has_children || depth == 1;
            let icon_x = left + icon_offset(depth, caret_room);
            // **Where this row's elbow has to stop**: short of whichever glyph
            // leads the row, which is the caret when there is one and the kind
            // icon when there is not — and below the top level those are the two
            // *different* columns `caret_room` puts them in. Two points off an
            // 11pt caret box and four off a 15pt icon's, the design's own pair:
            // the wider gap is because a Phosphor glyph's ink stops well inside
            // its box, so an equal number would read as a tighter one.
            let lead = if has_children {
                caret_x - 7.5
            } else {
                icon_x - 11.5
            };
            rows.push(TreeRow {
                id,
                rect,
                depth,
                lead,
                elbow: elbow_y(rect, ui.ctx().pixels_per_point()),
                // **The same `selected` the row's own ground and glyph are painted
                // from**, so a layer picked on the canvas lights its leg without
                // the panel being told twice: this is `session.selection`, which is
                // one selection for the whole app.
                selected,
                hovered,
                container: is_container,
                closed: is_container && has_children && !expanded,
            });
            // A layer that cannot be edited — locked or hidden — is drawn at
            // 40% throughout, glyph and name together, the way the design marks
            // them. Not two tiers: a locked layer reads as out of play for the
            // same reason a hidden one does, and giving them different greys
            // only invited the question of which was which.
            let dimmed = locked || !visible;
            // **The container an in-flight drop would land in.** The line says *where*
            // and cannot say *which group*: two levels are one 31pt step apart, so
            // without this the reader counts indent columns by eye. Asked for as the
            // whole of that affordance — *"only highlight the text and icon of the group
            // to blue, should be enough affordance"* — after a dashed outline round the
            // row was tried and did not sit right.
            //
            // A frame behind the line it belongs to, necessarily; [`LayerDrag::lit`]
            // carries the reason.
            let drop_host = self.layer_drag.as_ref().is_some_and(|d| d.lit == Some(id));
            // A selected row wears the selection hue, not the chrome accent:
            // the row and the bounding box on canvas are one statement.
            //
            // **The drop host outranks both selection and dimming**, which is the right
            // way round for the same reason a hover beats a selection in the tree guides
            // (§15 D331): a drag is a question being asked now, where the other two are
            // states that persist. Dimming in particular must lose — a locked or hidden
            // group still takes children, so a host that stayed at 40% would be the
            // panel declining to answer.
            let name_col = match (drop_host, selected, dimmed) {
                (true, ..) => color::SELECT,
                (_, true, _) => color::TEXT,
                (_, false, true) => theme::color::text_a(102),
                (_, false, false) => theme::text::MUTED,
            };
            let glyph_col = match (drop_host, selected, dimmed) {
                (true, ..) => color::SELECT,
                (_, true, _) => color::SELECT,
                (_, false, true) => dim_to(glyph_col, 102),
                (_, false, false) => glyph_col,
            };
            match thumb {
                // **Centred where the glyph would be**, because it replaces it
                // rather than joining it: the icons down the tree are one column
                // and a picture that sat beside its glyph would put this row's
                // name on a different x from every other.
                //
                // **Rounded by the tessellator, not by a corner mask**, and the
                // distinction is what makes it possible here at all. `swatch`
                // rounds the inspector's chip with [`ui::mask_corners`], which
                // paints the leftover corner *in the ground colour* — and a row's
                // ground is three colours depending on whether it is resting,
                // hovered or selected, so that treatment would show four notches
                // in the wrong grey on two rows out of three. A textured
                // `RectShape` carries its own `corner_radius` through
                // tessellation instead and needs to know nothing about what is
                // behind it. `Painter::image` is this shape with a radius of zero.
                Some(id) => {
                    p.add(egui::Shape::Rect(
                        egui::epaint::RectShape::filled(
                            egui::Rect::from_center_size(
                                egui::pos2(icon_x, cy),
                                egui::vec2(THUMB, THUMB),
                            ),
                            egui::CornerRadius::same(THUMB_RADIUS),
                            thumb_tint(dimmed, picture.as_ref()),
                        )
                        .with_texture(id, crate::thumbs::FULL_UV),
                    ));
                }
                None => {
                    p.text(
                        egui::pos2(icon_x, cy),
                        egui::Align2::CENTER_CENTER,
                        glyph,
                        theme::icon_font(ICON_PT),
                        glyph_col,
                    );
                }
            }
            // 15px, not 12: the glyph is drawn centred at `icon_x` so half of
            // its 15px advance already sits to the right, and the name was
            // landing about 3px closer than the design's 7px gap.
            let renaming = matches!(&self.renaming_layer, Some((n, _)) if *n == id);
            if !renaming {
                p.text(
                    egui::pos2(icon_x + NAME_GAP, cy),
                    egui::Align2::LEFT_CENTER,
                    &name,
                    egui::FontId::proportional(12.5),
                    name_col,
                );
            }

            if show_eye {
                p.text(
                    egui::pos2(eye_x, cy),
                    egui::Align2::CENTER_CENTER,
                    if visible { icon::EYE } else { icon::EYE_SLASH },
                    theme::icon_font(ICON_PT),
                    lit(
                        under == Some(RowHit::Eye),
                        if visible {
                            theme::text::DIM
                        } else {
                            theme::color::text_a(102)
                        },
                    ),
                );
            }
            if locked {
                // A locked layer wears a *filled* lock, permanently — the one
                // mark on the row that survives the pointer leaving, so the
                // state is legible while scanning the tree rather than only
                // while hovering it.
                crate::ui::paint_lock_filled(
                    p,
                    egui::pos2(lock_x, cy),
                    ICON_PT,
                    lit(under == Some(RowHit::Lock), theme::text::MUTED),
                    0.0,
                );
            } else if show_lock {
                p.text(
                    egui::pos2(lock_x, cy),
                    egui::Align2::CENTER_CENTER,
                    // The shackle says which state it is in, the way the eye
                    // does: shut when locked, open when not.
                    icon::LOCK_SIMPLE_OPEN,
                    theme::icon_font(ICON_PT),
                    lit(under == Some(RowHit::Lock), theme::text::DIM),
                );
            } else if let Some(b) = &badge {
                // One `text` call for both, differing only in the font: the
                // Phosphor face *is* a font here (`theme::icon_font`), so a
                // glyph badge and a worded one sit on the same baseline, in the
                // same colour, at the same right edge, with nothing to keep in
                // step by hand.
                let (s, font) = match b {
                    RowBadge::Text(s) => (s.as_str(), egui::FontId::proportional(10.5)),
                    RowBadge::Icon(g, size) => (*g, theme::icon_font(*size)),
                };
                p.text(
                    egui::pos2(rect.right() - 4.0, cy),
                    egui::Align2::RIGHT_CENTER,
                    s,
                    font,
                    theme::text::FAINT,
                );
            }

            // Not while the row is being renamed: the text field sitting on top
            // of it owns the pointer then, and dragging to select a few
            // characters must not pick the layer up and carry it off.
            if resp.drag_started() && !renaming {
                // Dragging something already selected carries the set; dragging
                // anything else selects it first, exactly as the canvas does
                // (`begin_select_drag`), so the rule is the same wherever the
                // gesture starts.
                let ids = if self.session.selection.contains(id) {
                    build::outermost(&self.session.doc, self.session.selection.ids())
                } else {
                    self.session.selection.set_one(id);
                    // The same line the click and right-click arms below carry,
                    // and for the same reason (§15 D581): this is the third door
                    // that changes the selection from inside the walk, and the
                    // only one that was not telling the panel. Without it
                    // `sync_tree_to_selection` reads the new selection as news on
                    // the next frame and `scroll_layers_to_selection` recentres
                    // the row — so picking up a row that is only *partly* in the
                    // viewport scrolls the whole tree, and the drop boundaries
                    // the hand was aiming at, out from under a held pointer. The
                    // click on the identical row moves it 0.0.
                    self.layers_synced_selection = self.session.selection.ids().to_vec();
                    vec![id]
                };
                // The **press** origin, not where the pointer is now: egui only
                // reports a drag once it has passed its threshold, so by this frame
                // the pointer has already travelled a few points from where the row
                // was picked up. Measuring from here instead would bake that slop in
                // as a constant offset on the indent for the rest of the gesture.
                let grab_x = ui
                    .input(|i| i.pointer.press_origin())
                    .map_or(rect.left(), |p| p.x);
                self.layer_drag = Some(LayerDrag {
                    ids,
                    target: None,
                    grab: (grab_x, depth),
                    level: depth,
                    lit: None,
                    spring: None,
                });
            }

            // Scroll to the selection once, on the frame it changed. The first
            // selected row drawn wins, which with several selected is the
            // topmost — the one a reader looks for.
            if selected && self.scroll_layers_to_selection {
                self.scroll_layers_to_selection = false;
                // Only when it is actually off screen. Recentring a row that is
                // already in view moves the whole tree under the pointer for no
                // reason, which is worse than not scrolling at all.
                if !ui.clip_rect().contains_rect(rect) {
                    ui.scroll_to_rect(rect, Some(egui::Align::Center));
                }
            }

            // The name is the one thing on the row worth editing in place, and a
            // double-click is where everyone reaches for it. Rename used to live
            // only in the inspector, which meant tidying up a tree was a trip to
            // the other side of the window per layer.
            if resp.double_clicked() {
                self.renaming_layer = Some((id, name.clone()));
                // **Opening it is all this does.** The caret is the field's own
                // business, asked for on the frame it first draws — which is the
                // next one, since `renaming` was latched above this. Requesting it
                // here instead is what crashed the app on every double-click:
                // AccessKit is handed a focused id and this frame has no node for
                // it. See `layer_rename_field`.
            }
            if renaming {
                self.layer_rename_field(ui, id, rect, icon_x + NAME_GAP);
            }

            // `context_menu.is_none()` for the canvas's reason (R4): the click
            // that dismisses an open menu is spent dismissing it and must not also
            // reselect a row underneath.
            if resp.clicked() && !renaming && self.context_menu.is_none() {
                // `interact_pointer_pos`, not the `under` computed above: that
                // one is the *hover* and this is the press, and on a touch or a
                // click-through frame they are not always the same point.
                // `RowTargets::at` is what they share.
                match resp.interact_pointer_pos().and_then(|q| targets.at(q.x)) {
                    Some(RowHit::Caret) => {
                        if expanded {
                            self.collapsed.insert(id);
                        } else {
                            self.collapsed.remove(&id);
                        }
                    }
                    Some(RowHit::Eye) => {
                        self.session.commit(Transaction(vec![Operation::SetVisible {
                            id,
                            visible: !visible,
                        }]));
                    }
                    Some(RowHit::Lock) => {
                        self.session.commit(Transaction(vec![Operation::SetLocked {
                            id,
                            locked: !locked,
                        }]));
                    }
                    None => self.select_from_row(id, shown, ui),
                }
                // The panel already knows about the selection it just made, so
                // `sync_tree_to_selection` must not treat it as news and scroll
                // the row the user is pointing at out from under them.
                self.layers_synced_selection = self.session.selection.ids().to_vec();
            }

            // **The panel's door** (`docs/context-menus.md` §1). The only door that
            // reaches a locked or hidden layer — `hit_test` skips both, so the
            // canvas cannot deliver one — which is why the panel's menu has to
            // carry the complete set, including the *Unlock* that is the only way
            // back (§5.9, C4).
            //
            // **C6 needs no code**: the press that opens this already committed any
            // live rename, because "a press anywhere else ends the rename"
            // (`layer_rename_field`). The press commits, the release opens the
            // menu, and there is no special case to write.
            if resp.secondary_clicked() {
                // C2, in the panel: a right-click on a row that is already part of
                // the selection acts on all of it.
                if !self.session.selection.contains(id) {
                    self.session.selection.set_one(id);
                    self.layers_synced_selection = self.session.selection.ids().to_vec();
                }
                self.open_context_menu(
                    ui.ctx(),
                    crate::menu::Target::Layer {
                        id,
                        door: crate::menu::Door::Panel,
                    },
                    None,
                );
            }

            if !expanded {
                return; // collapsed: skip children
            }
        }

        // Reversed: `children` is z-order with the **last** entry nearest the
        // viewer (§5.3), and a layers tree reads top-down as front-to-back —
        // the row at the top of the list is the layer on top of the artwork.
        // Every index this file computes stays a model index; only the walk is
        // flipped, which is why the drop-target maths below has to mirror too.
        for child in children.into_iter().rev() {
            self.layers_row(ui, child, depth + 1, shown, rows);
        }
    }

    /// Hold a drag on a collapsed group for [`SPRING_SECS`] and the branch opens.
    ///
    /// `on` is the group the pointer is resting on, or `None` for anywhere else — passing
    /// `None` is what restarts the wait, so a drag that crosses a group and comes back
    /// waits the full second again rather than accumulating two half-seconds into one.
    ///
    /// ⚠️ **The repaint request is the whole of the timing.** egui only lays out a frame
    /// when something asks it to, and a hand holding still asks for nothing — so without
    /// this the clock would be read once, on the frame the pointer arrived, and the group
    /// would open on whatever unrelated event happened to wake the panel next. The drag
    /// already requests a repaint every frame in `layers_tree`, which makes this
    /// redundant today and correct on the day that changes; `request_repaint_after` is
    /// the cheaper of the two spellings and says what it is for.
    fn spring_load(&mut self, ui: &egui::Ui, on: Option<NodeId>) {
        let now = ui.input(|i| i.time);
        let Some(drag) = self.layer_drag.as_mut() else {
            return;
        };
        let Some(id) = on else {
            drag.spring = None;
            return;
        };
        match drag.spring {
            // Still on the same group: open it once the wait is up.
            Some((waiting, since)) if waiting == id => {
                if now - since >= SPRING_SECS {
                    drag.spring = None;
                    self.collapsed.remove(&id);
                } else {
                    ui.ctx()
                        .request_repaint_after(std::time::Duration::from_secs_f64(
                            SPRING_SECS - (now - since),
                        ));
                }
            }
            // A different group, or none until now: start the wait here.
            _ => {
                drag.spring = Some((id, now));
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f64(SPRING_SECS));
            }
        }
    }

    /// Turn a [`DropSpot`] — which is pure screen geometry — into the parent and index
    /// the document will actually be asked for, or `None` where that drop is not
    /// allowed.
    ///
    /// **The reversed walk is the whole of the arithmetic.** `children` is z-order with
    /// the last entry nearest the viewer (§5.3) and the tree draws it top-down, so a
    /// slot *below* a row on screen is *before* it in the model, and the top of a
    /// container's list is `child_count` rather than 0. Every index this file computes
    /// stays a model index; only the walk is flipped.
    ///
    /// ⚠️ **A drop indented out of a container lands beside an ancestor, not beside the
    /// row the pointer is on.** That is the new half: dragging below the last child of a
    /// group and moving left means "after the group", and the group may be several
    /// levels up from the row the boundary was measured against. So the row is walked up
    /// to the depth the pointer asked for and *that* node's slot is the answer — which
    /// degenerates to the row itself when the drop is at the row's own level, so there
    /// is one path rather than a special case.
    fn resolve_drop(
        &self,
        rows: &[TreeRow],
        spot: DropSpot,
        dragged: &[NodeId],
    ) -> Option<DropTarget> {
        let target = match spot {
            // Into the container, at the end of its list — which is the top of it on
            // screen as well as the top of its z-order, so a layer dropped onto a frame
            // lands where the pointer is.
            DropSpot::Into(i) => {
                let parent = rows.get(i)?.id;
                DropTarget {
                    parent,
                    index: self.child_count(parent),
                }
            }
            DropSpot::Between { boundary, depth } => {
                match boundary.checked_sub(1).and_then(|i| rows.get(i)) {
                    // One level in from the row above: inside it, at the top of its
                    // list. [`depth_range`] only offers this for an open container.
                    Some(above) if depth > above.depth => DropTarget {
                        parent: above.id,
                        index: self.child_count(above.id),
                    },
                    Some(above) => {
                        let anchor = self.ancestor_at_depth(above.id, above.depth, depth)?;
                        let parent = self.session.doc.get(anchor)?.parent()?;
                        let index = self
                            .session
                            .doc
                            .get(parent)?
                            .children()
                            .iter()
                            .position(|c| *c == anchor)?;
                        DropTarget { parent, index }
                    }
                    // The top of the whole list, which is the end of the root's. The
                    // depth test is belt and braces: the first row of a tree is always
                    // top-level, so [`depth_range`] cannot offer anything else here.
                    None if depth == 1 => {
                        let root = self.session.doc.root();
                        DropTarget {
                            parent: root,
                            index: self.child_count(root),
                        }
                    }
                    None => return None,
                }
            }
        };

        // **The destination must not be inside what is being carried**, or the drop
        // would make a node its own descendant. Asked of the *parent* rather than of
        // the row under the pointer, which is the shape the indent gesture forces: the
        // pointer sits on a dragged row's own child all the way through an outdent, and
        // the levels that are actually illegal are only the ones at or below it.
        if dragged
            .iter()
            .any(|d| *d == target.parent || self.is_ancestor_of(*d, target.parent))
        {
            return None;
        }
        // A frame belongs to the canvas or to another frame and never to a group
        // (§5.3) — refuse rather than offer an invalid drop. Every carried row has to
        // be welcome, since they all land in the same parent: a set holding a frame
        // and a shape can only be dropped where both fit.
        dragged
            .iter()
            .all(|d| self.drop_is_legal(*d, target.parent))
            .then_some(target)
    }

    /// Walk `id`, which is at `depth`, up to the ancestor sitting at `want`. `id` itself
    /// when the two are equal; `None` if `want` is deeper, which [`depth_range`] never
    /// asks for.
    fn ancestor_at_depth(&self, id: NodeId, depth: usize, want: usize) -> Option<NodeId> {
        let mut cursor = id;
        for _ in 0..depth.checked_sub(want)? {
            cursor = self.session.doc.get(cursor)?.parent()?;
        }
        Some(cursor)
    }

    fn drop_is_legal(&self, dragged: NodeId, parent: NodeId) -> bool {
        let Some(node) = self.session.doc.get(dragged) else {
            return false;
        };
        let Some(parent_node) = self.session.doc.get(parent) else {
            return false;
        };
        build::can_parent(parent_node.kind(), node.kind())
    }

    /// Read the pointer against the rows the walk just drew: work out where the drag
    /// would land, record it for the release, and draw the affordance that says so.
    ///
    /// **One pass, after the walk, and the three used to be three places.** The target
    /// was worked out by whichever row the pointer was inside, painted by whichever row
    /// could recognise the boundary from its own child list, and landed from a third —
    /// so a boundary reachable from two lists got two answers, and the rule each row
    /// applied to decide whether the line was its own is the whole of §15 D67. None of
    /// that survives an indent-aware drop, which is a claim about the row *above* a
    /// boundary and the row *below* it at once: a row painting itself does not yet know
    /// what follows it.
    ///
    /// The hint is drawn only where the drop is legal. Nothing is the honest answer —
    /// and with the indent gesture it is also a live one, because moving left out of a
    /// group is what makes the line reappear.
    fn drop_hint(&mut self, ui: &egui::Ui, rows: &[TreeRow]) {
        let Some(drag) = self.layer_drag.as_ref() else {
            return;
        };
        // ⚠️ **A drag with no button behind it paints nothing and lands nothing**
        // (§15 D471). `[S17-L1-01]`: pick a row up, and while still holding the
        // button press the panel's own chord (`Ctrl+Alt+\`, or `Ctrl+\` for
        // present mode). `input::resolve` is *"a pure resolution of this frame's
        // events, with no state of its own"* and `dispatch` hands `ToggleView`
        // straight to `set_view_switch` with no in-flight guard, so the panel is
        // skipped on the very frame the chord lands — and `finish_layer_drag`
        // runs inside `layers_tree`, which is not drawn. The release goes
        // nowhere and `layer_drag` survives it.
        //
        // Bring the panel back and this function read `pointer_interact_pos`
        // alone, **with no test that a button was down**, which is what turns a
        // resting pointer into a live drop: measured, the tree lit a drop host,
        // painted the blue line, re-derived a target from wherever the pointer
        // happened to rest, and **the release of the next ordinary click
        // committed it** — `undo_depth 0 → 1`, the layer reparented to the root,
        // toast *"Moved layer"*, against a control at 0.
        //
        // **Here rather than at the view switch, because this closes the class.**
        // Clearing the drag in `set_view_switch` would close the one route; a
        // drag stranded by any *future* route still paints and still lands. This
        // is also the recovery on the present-mode route, where `escape` cannot
        // be one: its first rung is `self.present = false`, which restores the
        // panel with the phantom drag live.
        //
        // 🚨 **Amended by §15 D582, because the first spelling of it — a bare
        // `return` on `!primary_down()` — stopped every legitimate drop as
        // well.** `layers_tree` clears `target` at the top of each frame and
        // this is the only thing that re-derives it, and egui clears
        // `pointer.down[Primary]` on the frame it *processes* the release. So
        // the one frame that ends a drag was also the one frame that could not
        // resolve one: the release landed nothing, silently, and the drag was
        // discarded by `finish_layer_drag` two lines in. Measured — with the
        // `return` in place a press-move-release over the tree leaves
        // `undo_depth` at 0 and the child list byte-identical; with it disabled
        // the same events give *"Reordered"* and the move.
        //
        // **So the release frame is admitted, and a drag with nothing behind it
        // is now *dropped* rather than ignored.** That is strictly stronger than
        // ignoring it: a stranded drag meets an ordinary frame — no button down,
        // no release — before any later click can reach it, and dies there. The
        // old spelling left it live and merely unpainted, which is why it had to
        // refuse the release frame to stop it landing.
        let (down, released) =
            ui.input(|i| (i.pointer.primary_down(), i.pointer.primary_released()));
        if !down && !released {
            self.layer_drag = None;
            return;
        }
        let (grab, held) = (drag.grab, drag.level);
        let dragged = drag.ids.clone();
        // ⚠️ `pointer_interact_pos` against the row rects, **never** `resp.hovered()`:
        // while a drag is in flight egui reports only the dragged widget as hovered, so
        // every other row says no and no drop target is ever recorded (§15 D19).
        let Some(pointer) = ui.ctx().pointer_interact_pos() else {
            return;
        };
        let Some(read) = drop_spot(rows, pointer, grab, held) else {
            // Off the tree: the level is still carried, so coming back does not restart
            // the gesture at whatever depth the pointer happens to re-enter at.
            self.spring_load(ui, None);
            return;
        };
        let spot = read.spot;
        // **Only the "inside this one" zone springs, not the whole row.** The edges of a
        // row mean "between", and opening a branch there would shift every row below the
        // pointer out from under a drop the hand had already aimed. Aiming at the middle
        // is the one case where opening is what was being asked for, and the group's own
        // row does not move when it does.
        self.spring_load(
            ui,
            match spot {
                DropSpot::Into(i) if rows[i].closed => Some(rows[i].id),
                _ => None,
            },
        );
        let Some(target) = self.resolve_drop(rows, spot, &dragged) else {
            return;
        };
        if let Some(d) = self.layer_drag.as_mut() {
            d.target = Some(target);
            d.level = read.level;
        }

        let painter = ui.painter();
        match spot {
            DropSpot::Into(i) => {
                painter.rect_stroke(
                    rows[i].rect,
                    egui::CornerRadius::same(5),
                    egui::Stroke::new(1.0, color::SELECT),
                    egui::StrokeKind::Inside,
                );
            }
            DropSpot::Between { boundary, depth } => {
                // ⚠️ **Which container the line lands in is *not* said here.** It is the
                // destination row's own ink, painted blue by the walk a frame earlier —
                // see `drop_host` in [`OndinApp::layers_row`] and [`LayerDrag::lit`].
                // A dashed outline round that row was built here first and taken out
                // again on the machine: *"let's remove the dotted outline, that doesn't
                // sit right"*. Two boxes in a panel already using one to mean "inside"
                // is a vocabulary, and colouring the ink says the same thing without
                // adding one.
                let Some(y) = boundary_y(rows, boundary, ui.spacing().item_spacing.y) else {
                    return;
                };
                let (y, width) = snap_across(y, DROP_LINE_PT, ui.ctx().pixels_per_point());
                // Every row spans the panel, so any of them gives the two ends. The
                // boundary below the last row has no row of its own to ask.
                let rect = rows[boundary.min(rows.len() - 1)].rect;
                // **The line starts at the level's own indent, and that is the whole
                // point of the gesture being visible.** A drop between two rows at
                // different depths is ambiguous until something says which; the line's
                // left end is that something, and it is the same column the row that
                // lands there will start at.
                painter.line_segment(
                    [
                        egui::pos2(rect.left() + indent_of(depth), y),
                        egui::pos2(rect.right(), y),
                    ],
                    egui::Stroke::new(width, color::SELECT),
                );
            }
        }
    }

    /// Land the dragged rows, as one transaction and so one undo step.
    ///
    /// The index arithmetic is **not** here: [`plan_layer_move`] simulates the
    /// affected child lists and plans each operation against the simulation, and
    /// its own doc is where that is argued. What this function decides is the four
    /// things either side of that call.
    ///
    /// **A dragged id that is gone from the document is dropped, not fatal.** The
    /// set was captured when the drag began and anything can have deleted a row
    /// since — an undo, the other end of a multi-selection, a script. Failing the
    /// whole gesture because one of five rows evaporated would lose the four the
    /// hand is still holding.
    ///
    /// **The carried set is re-sorted into document order**, so it lands in the
    /// arrangement the tree was showing rather than in the order the rows happened
    /// to be clicked. Click order is an artefact of the hand; the tree is what the
    /// eye was reading.
    ///
    /// **An empty plan commits nothing** — dropping a row back where it already is
    /// is not an undo step (G16's rule, one door earlier than the seam that now
    /// enforces it).
    ///
    /// **The status line names the verb and the count**, and `plan.reparented` is
    /// what tells *Moved* from *Reordered*: the two read very differently to
    /// someone checking whether a drag went where they meant it to.
    fn finish_layer_drag(&mut self) {
        let Some(drag) = self.layer_drag.take() else {
            return;
        };
        let Some(target) = drag.target else { return };

        // Document order, so a set of rows keeps the arrangement the tree showed
        // rather than the order they happened to be clicked in.
        let order = ondin_core::subtree_nodes(&self.session.doc, &[self.session.doc.root()]);
        let mut moving: Vec<NodeId> = drag
            .ids
            .into_iter()
            .filter(|id| self.session.doc.contains(*id))
            .collect();
        moving.sort_by_key(|id| order.iter().position(|o| o == id).unwrap_or(usize::MAX));
        if moving.is_empty() {
            return;
        }

        let n = moving.len();
        match plan_layer_move(&self.session.doc, &self.session.resolved, &moving, target) {
            Ok(plan) if plan.ops.is_empty() => {}
            Ok(plan) => {
                if self.session.commit(Transaction(plan.ops)) {
                    self.session.info(match (plan.reparented, n) {
                        (true, 1) => "Moved layer".to_string(),
                        (true, n) => format!("Moved {n} layers"),
                        (false, 1) => "Reordered".to_string(),
                        (false, n) => format!("Reordered {n} layers"),
                    });
                }
            }
            Err(e) => self.session.fail(format!("Cannot move layer: {e}")),
        }
    }

    /// Whether `ancestor` is on `id`'s parent chain.
    fn is_ancestor_of(&self, ancestor: NodeId, id: NodeId) -> bool {
        let mut cursor = self.session.doc.get(id).and_then(|n| n.parent());
        while let Some(p) = cursor {
            if p == ancestor {
                return true;
            }
            cursor = self.session.doc.get(p).and_then(|n| n.parent());
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
    use ondin_core::{Document, IdSource, Resolved};

    /// Root → one frame holding `n` rects, in document order.
    fn tree(n: usize) -> (Document, Resolved, NodeId, Vec<NodeId>) {
        let mut ids = IdSource::new(0x1AEA);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let frame = ids.mint();
        let mut ops = vec![Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 300.0),
            },
            transform: Some(Affine::translate((10.0, 10.0))),
            name: None,
        }];
        let kids: Vec<NodeId> = (0..n).map(|_| ids.mint()).collect();
        for (i, id) in kids.iter().enumerate() {
            ops.push(Operation::CreateNode {
                id: *id,
                parent: frame,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            });
        }
        doc.apply(&Transaction(ops)).unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, frame, kids)
    }

    fn children(doc: &Document, parent: NodeId) -> Vec<NodeId> {
        doc.get(parent).unwrap().children().to_vec()
    }

    /// **Three rows dropped at one index arrive in order and stay together.**
    ///
    /// The failure this guards is the whole reason the planner simulates: three
    /// indices all read off the *original* list would drop the rows reversed, or
    /// past the end of it. Nothing about the result looks wrong until you count.
    #[test]
    fn several_rows_dropped_together_keep_their_order() {
        let (mut doc, res, frame, k) = tree(5);
        // Move c, d, e to the very front, in that order.
        let plan = plan_layer_move(
            &doc,
            &res,
            &[k[2], k[3], k[4]],
            DropTarget {
                parent: frame,
                index: 0,
            },
        )
        .unwrap();
        doc.apply(&Transaction(plan.ops)).unwrap();
        assert_eq!(
            children(&doc, frame),
            vec![k[2], k[3], k[4], k[0], k[1]],
            "the three moved rows should lead, in their own order"
        );
    }

    /// The same drop, at an index *after* the rows being lifted out. Every
    /// removal ahead of the slot shifts it, and forgetting that lands the set one
    /// place too far right each time.
    #[test]
    fn a_drop_after_the_moved_rows_accounts_for_their_removal() {
        let (mut doc, res, frame, k) = tree(5);
        // Move a and b to sit between d and e.
        let plan = plan_layer_move(
            &doc,
            &res,
            &[k[0], k[1]],
            DropTarget {
                parent: frame,
                index: 4,
            },
        )
        .unwrap();
        doc.apply(&Transaction(plan.ops)).unwrap();
        assert_eq!(children(&doc, frame), vec![k[2], k[3], k[0], k[1], k[4]]);
    }

    /// Rows from two different parents landing in one, which is the case where
    /// the destination list grows under the plan while a source list shrinks.
    #[test]
    fn rows_gathered_from_two_parents_land_in_order() {
        let (mut doc, _res, frame, k) = tree(2);
        // A second frame with two rects of its own.
        let mut ids = IdSource::new(0x2BEB);
        let other = ids.mint();
        let x = ids.mint();
        let y = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: other,
                parent: doc.root(),
                index: 1,
                kind: NodeKind::Artboard {
                    size: Size::new(100.0, 100.0),
                },
                transform: Some(Affine::translate((500.0, 0.0))),
                name: None,
            },
            Operation::CreateNode {
                id: x,
                parent: other,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
            Operation::CreateNode {
                id: y,
                parent: other,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
                },
                transform: Some(Affine::IDENTITY),
                name: None,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);

        // One row from each frame, into the first frame at the front.
        let plan = plan_layer_move(
            &doc,
            &res,
            &[k[1], y],
            DropTarget {
                parent: frame,
                index: 0,
            },
        )
        .unwrap();
        assert!(plan.reparented, "one of them crossed a parent");
        doc.apply(&Transaction(plan.ops)).unwrap();
        assert_eq!(children(&doc, frame), vec![k[1], y, k[0]]);
        assert_eq!(children(&doc, other), vec![x]);
    }

    /// Reparenting keeps the layer where it looks: the frames sit at different
    /// world offsets, so a raw `Reparent` would teleport it by the difference.
    #[test]
    fn a_reparented_row_does_not_move_on_screen() {
        let (mut doc, res, frame, k) = tree(1);
        let before = res.world_bounds(k[0]).unwrap();
        let root = doc.root();
        let plan = plan_layer_move(
            &doc,
            &res,
            &[k[0]],
            DropTarget {
                parent: root,
                index: 0,
            },
        )
        .unwrap();
        doc.apply(&Transaction(plan.ops)).unwrap();
        let after = Resolved::rebuild(&doc).world_bounds(k[0]).unwrap();
        assert!(
            (after.min_x() - before.min_x()).abs() < 1e-9
                && (after.min_y() - before.min_y()).abs() < 1e-9,
            "the layer teleported: {before:?} -> {after:?}"
        );
        let _ = frame;
    }

    /// **A drop that lands the rows where they already are plans nothing** (§15
    /// D532) — `[S15.1-L1-03]`.
    ///
    /// The per-row no-op test compares each row's landing index with the
    /// position it had *before* the rows ahead of it were lifted, so a run
    /// dropped at the boundary immediately after itself gives every row an `at`
    /// that differs from its own `at_from` while the net permutation is the
    /// identity. Over `[k0, k1, k2]` moving `[k0, k1]`, index 2 planned **two**
    /// `Reorder`s over a byte-identical child list: one undo step, a dirtied
    /// document and a status line claiming *"Reordered 2 layers"*.
    ///
    /// ⚠️ **D428's `changes_nothing` cannot absorb it.** `Reorder`'s arm asks
    /// `child_index(doc, id) == Some(index)` of the document as it stands, and
    /// each op moves its row to an index it is not at *yet* — so no op is a
    /// no-op and the transaction is not one either. A composition of real
    /// operations that comes back to where it started is invisible to a
    /// per-operation guard.
    ///
    /// ⚠️ **The single-row case was already correct, which is what hid it.**
    /// This test replaces one of that name which moved **one** row to index 0
    /// and stopped — so the assertion existed, carried the right name, and
    /// covered the arm that worked. The one-row case is kept below as the
    /// control; *a suite of one-row drops would find nothing wrong here.*
    ///
    /// ⚠️ Flipped by removing the composition test: red at **index 2 with two
    /// rows** — the first case in the loop that has more than one and lands
    /// where it started. Indices 0 and 1 stay green either way, which is the
    /// same reason the finding had to name the index it did.
    #[test]
    fn a_drop_that_moves_nothing_plans_nothing() {
        let (doc, res, frame, k) = tree(3);
        let plan = |moving: &[NodeId], index: usize| {
            plan_layer_move(
                &doc,
                &res,
                moving,
                DropTarget {
                    parent: frame,
                    index,
                },
            )
            .expect("a plan")
        };

        // The whole run, dropped at each boundary that leaves it where it is.
        for index in [0, 1, 2] {
            assert!(
                plan(&[k[0], k[1]], index).ops.is_empty(),
                "two rows dropped at {index} land where they already are"
            );
        }
        // The control: one boundary further on is a real move, and still plans.
        assert_eq!(
            plan(&[k[0], k[1]], 3).ops.len(),
            2,
            "and a drop that does move still plans both rows"
        );
        // The control that says the fixture can tell the two apart at all: the
        // single-row case was correct before this and must stay correct.
        assert!(plan(&[k[0]], 0).ops.is_empty());
        assert_eq!(plan(&[k[0]], 3).ops.len(), 1);
    }

    /// **One boundary, one y, whichever row is asked.** Rows do not touch — the tree puts
    /// `item_spacing.y` between them — so the upper row's bottom and the lower row's top are
    /// a gap apart and naming the same boundary from either side used to give two answers.
    ///
    /// Between siblings that never showed, because one owner draws each boundary. It showed
    /// at the boundary a frame shares with its topmost child, which is reachable from *two
    /// lists* — "the top of the frame's own list" from inside it, and "beside the frame"
    /// from the frame's row — and so has an owner in each. Reported as the line jumping a
    /// pixel there and nowhere else, which is exactly one `item_spacing.y`.
    #[test]
    fn both_sides_of_a_boundary_put_the_line_in_the_same_place() {
        for gap in [0.0_f32, 1.0, 2.0, 6.0] {
            // Two rows as the tree lays them out: `gap` of space between them.
            let upper = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(240.0, ROW_H));
            let lower = egui::Rect::from_min_size(
                egui::pos2(0.0, upper.bottom() + gap),
                egui::vec2(240.0, ROW_H),
            );
            let from_above = drop_line_y(upper, RowEdge::Bottom, gap);
            let from_below = drop_line_y(lower, RowEdge::Top, gap);
            assert!(
                (from_above - from_below).abs() < 1e-4,
                "gap {gap}: {from_above} from above, {from_below} from below"
            );
            // And it is *in* the gap rather than on either row.
            assert!(
                from_above >= upper.bottom() - 1e-4 && from_above <= lower.top() + 1e-4,
                "gap {gap}: {from_above} is outside [{}, {}]",
                upper.bottom(),
                lower.top()
            );
        }
    }

    /// The line has to be the same weight wherever it lands. A 2pt stroke at a fractional
    /// y spreads over three device rows with the outer two part-covered, and the fractional
    /// offset of one boundary in the tree need not match the next — so it appeared to
    /// thicken as it moved down the list.
    #[test]
    fn the_drop_line_lands_on_whole_device_pixels() {
        for ppp in [1.0_f32, 1.25, 1.5, 1.75, 2.0] {
            // Fractional row positions, as a real layout produces.
            for y in [40.0_f32, 66.5, 92.25, 118.7, 145.05] {
                let (snapped, width) = snap_across(y, DROP_LINE_PT, ppp);
                let px = width * ppp;
                assert!(
                    (px - px.round()).abs() < 1e-4 && px >= 1.0,
                    "width {width} is {px} device px at {ppp}"
                );
                // Both edges of the band land on device pixel boundaries, which is what
                // "no partly covered row" means.
                for edge in [snapped - width / 2.0, snapped + width / 2.0] {
                    let e = edge * ppp;
                    assert!(
                        (e - e.round()).abs() < 1e-4,
                        "edge {edge} is {e} device px at {ppp} (y={y})"
                    );
                }
                // And it stays where it was asked to be, within a pixel.
                assert!(
                    (snapped - y).abs() <= 1.0 / ppp,
                    "snapping moved the line from {y} to {snapped} at {ppp}"
                );
            }
        }
    }
}

#[cfg(test)]
mod rename_layout_tests {
    use super::*;

    use crate::app::OndinApp;
    use crate::input::Action;
    use ondin_core::kurbo::Size;

    /// A headless app holding one rect at the root, and that rect's id.
    fn app_with_a_layer(ctx: &egui::Context) -> (OndinApp, NodeId) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(9);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let rect = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: rect,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        }]))
        .expect("a rect");
        app.session.adopt_document(doc, None);
        (app, rect)
    }

    /// One frame of the layers tree, wide enough for a row.
    fn frame(app: &mut OndinApp, ctx: &egui::Context) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(300.0, 600.0),
            )),
            ..Default::default()
        };
        ctx.run_ui(input, |ui| app.layers_tree(ui))
    }

    /// **The field asks for the caret on the frame it first draws, and never
    /// again.**
    ///
    /// A rename opens from four doors and only one of them — the `Ctrl+R` / `F2`
    /// chord — can draw the field in the frame that opened it. So the field is what
    /// asks, on its first pass, which is the only arrangement that works from all
    /// four *and* the only one that cannot hand AccessKit a focused id with no node
    /// (see `OndinApp::layer_rename_field` and the test below).
    ///
    /// The second half is the trap the request has to avoid. Flip-checked against
    /// the obvious spelling — re-request whenever the field does not have focus —
    /// which grabs focus back on the frame Enter or Escape takes it away: focus
    /// comes back as the field, so a rename could never be committed or left.
    #[test]
    fn the_rename_field_asks_for_the_caret_on_the_frame_it_first_draws() {
        let ctx = egui::Context::default();
        let (mut app, id) = app_with_a_layer(&ctx);
        app.renaming_layer = Some((id, "Ellipse".to_owned()));

        let _ = frame(&mut app, &ctx);
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(rename_field_id(id)),
            "the field has to arrive focused, or the rename opens a box nobody is \
             typing into"
        );

        // What Enter and Escape both do first. The field is still open this frame,
        // so a version that asks again would take the caret straight back.
        ctx.memory_mut(|m| m.surrender_focus(rename_field_id(id)));
        let _ = frame(&mut app, &ctx);
        assert_ne!(
            ctx.memory(|m| m.focused()),
            Some(rename_field_id(id)),
            "a field that re-takes focus can never be committed or cancelled"
        );
    }

    /// ⚠️ **No frame may end with the focus on a widget that frame did not draw.**
    ///
    /// The reported crash, from a double-click on a layer row:
    /// *"panicked at accesskit_consumer-0.35.0\\src\\tree.rs:71: Focused ID
    /// #2077194978197920161 is not in the node list"*. egui builds its AccessKit
    /// update out of the nodes the pass registered and sets `focus` from
    /// `Memory::focused()` regardless — so requesting focus for a widget that will
    /// not be in the frame is not the deferral it looks like, it is a panic in the
    /// consumer at the end of that frame.
    ///
    /// **Two doors could not draw the field in the frame that asked**: the
    /// double-click reads a `renaming` flag latched earlier in the row, and the
    /// context menu is drawn *after* the layers panel. The order asserted here is
    /// the menu's, because it is the one that cannot be fixed by moving a line — the
    /// panel is already behind us.
    ///
    /// Flip-checked by putting the request back into `OndinApp::rename_selection`:
    /// the first assertion fails, reporting a focus id against a node list that does
    /// not contain it.
    #[test]
    fn a_rename_opened_after_the_panel_has_drawn_leaves_no_dangling_focus() {
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        let (mut app, id) = app_with_a_layer(&ctx);
        app.session.selection.set_one(id);

        let in_the_tree = |full: &egui::FullOutput, when: &str| {
            let update = full
                .platform_output
                .accesskit_update
                .as_ref()
                .expect("the fixture: AccessKit is on, or this test asserts nothing");
            assert!(
                update.nodes.iter().any(|(nid, _)| *nid == update.focus),
                "{when}: the focus is not among the {} nodes the frame registered, \
                 which is the panic",
                update.nodes.len()
            );
        };

        // The context menu's order: the panel has drawn, and *then* a row opens a
        // rename. Nothing can put the field in this frame.
        let opened = {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(300.0, 600.0),
                )),
                ..Default::default()
            };
            ctx.run_ui(input, |ui| {
                app.layers_tree(ui);
                app.dispatch(ui.ctx(), Action::Rename);
            })
        };
        assert!(
            app.renaming_layer.is_some(),
            "the fixture: the rename did open"
        );
        in_the_tree(&opened, "the frame the rename opened in");

        let arrived = frame(&mut app, &ctx);
        in_the_tree(&arrived, "the frame the field arrived in");
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(rename_field_id(id)),
            "and the caret still lands in it, one frame later"
        );
    }

    /// **The rename field lands exactly on the name it replaces.**
    ///
    /// Both numbers here were wrong in the first cut and both are the kind of
    /// wrong that only shows up on screen: the field was inset 3px and shortened
    /// 3px at each end, which — once `TextEdit`'s own 4px/2px margin and its
    /// `LEFT_TOP` default were added on top — moved the name up and sideways the
    /// instant a row was double-clicked. Measured against a real `Align2::
    /// LEFT_CENTER` galley rather than reasoned about, because reasoning about it
    /// is what got it wrong.
    #[test]
    fn the_rename_field_sits_where_the_painted_name_sits() {
        let ctx = egui::Context::default();
        let _ = ctx.run_ui(Default::default(), |ui| {
            let row = egui::Rect::from_min_size(egui::pos2(0.0, 40.0), egui::vec2(240.0, ROW_H));
            let left = 60.0;
            let painted = ui.painter().text(
                egui::pos2(left, row.center().y),
                egui::Align2::LEFT_CENTER,
                "Ellipse",
                egui::FontId::proportional(12.5),
                egui::Color32::WHITE,
            );
            let field = rename_field_rect(row, left);

            assert!(
                (field.left() - painted.left()).abs() < 0.01,
                "the field starts at {} but the name starts at {}",
                field.left(),
                painted.left()
            );
            assert!(
                (field.center().y - painted.center().y).abs() < 0.01,
                "the field centres on {} but the name centres on {}",
                field.center().y,
                painted.center().y
            );
            // Full row height, so the centred text has the room the painted name
            // had. Shortening this is what pushed the name upward.
            assert!(
                (field.height() - ROW_H).abs() < 0.01,
                "the field is {} tall, not the row's {ROW_H}",
                field.height()
            );
        });
    }
}

#[cfg(test)]
mod missing_picture_tests {
    use super::picture_missing;
    use ondin_core::kurbo::{RoundedRectRadii, Size};
    use ondin_core::peniko::Color;
    use ondin_core::{
        Brush, Document, Fill, IdSource, ImageId, NodeId, NodeKind, Operation, Stroke, StrokeAlign,
        Transaction, image_brush,
    };

    /// One rect carrying `fills` and `strokes`.
    fn node_with(fills: Vec<Fill>, strokes: Vec<Stroke>) -> (Document, NodeId) {
        let mut ids = IdSource::new(3);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let rect = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: rect,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: RoundedRectRadii::default(),
            },
            transform: None,
            name: None,
        }]))
        .expect("a rect");
        doc.apply(&Transaction(vec![
            Operation::SetFills { id: rect, fills },
            Operation::SetStrokes { id: rect, strokes },
        ]))
        .expect("paint it");
        (doc, rect)
    }

    fn fill(brush: Brush, visible: bool) -> Fill {
        Fill { brush, visible }
    }

    fn stroke(brush: Brush, visible: bool) -> Stroke {
        Stroke {
            brush,
            width: 1.0,
            join: ondin_core::kurbo::Join::Miter,
            cap: ondin_core::kurbo::Cap::Butt,
            dashes: Vec::new(),
            dash_offset: 0.0,
            sides: Default::default(),
            miter_limit: 4.0,
            dash_fit: false,
            align: StrokeAlign::Center,
            visible,
        }
    }

    /// **The row's mark answers for the whole layer, not just its middle.**
    ///
    /// Four cases in one, because they are one rule with three chances to be
    /// written as two: a picture nothing can draw marks the row whether it is a
    /// fill or a *stroke*, a paint that is switched off marks nothing — the
    /// canvas is not drawing it to be missing — and a layer with no picture at
    /// all is never marked, which is the case every other row in the tree is.
    #[test]
    fn a_layer_is_marked_for_any_visible_paint_whose_picture_is_gone() {
        // A store that never saw anything: every reference is unresolvable, which
        // is the merged state D179 insists on (a dangling id, a dead link and
        // undecodable bytes are indistinguishable here on purpose).
        let store = ondin_render::ImageStore::new();
        let missing = || image_brush(ImageId("gone".into()));
        let solid = || Brush::Solid(Color::BLACK);

        let (doc, id) = node_with(vec![fill(missing(), true)], Vec::new());
        assert!(
            picture_missing(&store, doc.get(id).unwrap()),
            "a visible image fill with no pixels has to mark the row"
        );

        let (doc, id) = node_with(vec![fill(solid(), true)], vec![stroke(missing(), true)]);
        assert!(
            picture_missing(&store, doc.get(id).unwrap()),
            "a border painted with a missing picture is as broken as a middle"
        );

        let (doc, id) = node_with(vec![fill(missing(), false)], Vec::new());
        assert!(
            !picture_missing(&store, doc.get(id).unwrap()),
            "a hidden fill draws nothing, so there is nothing missing to say"
        );

        let (doc, id) = node_with(vec![fill(solid(), true)], vec![stroke(solid(), true)]);
        assert!(
            !picture_missing(&store, doc.get(id).unwrap()),
            "a layer with no picture on it must never wear the warning"
        );
    }
}

#[cfg(test)]
mod subtract_base_tests {
    use super::subtract_base;
    use ondin_core::kurbo::{RoundedRectRadii, Size};
    use ondin_core::{BoolOp, Document, IdSource, NodeId, NodeKind, build};

    /// Two rects under the root, then `build::boolean` over them with `op` and
    /// `key`. Returns the operands in *child* order, which is the order the
    /// boolean settled on — bottom first.
    fn boolean_of_two(op: BoolOp, key: Option<usize>) -> (Document, Vec<NodeId>, Vec<NodeId>) {
        let mut ids = IdSource::new(23);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let mut made = Vec::new();
        for i in 0..2 {
            let id = ids.mint();
            doc.apply(&ondin_core::Transaction(vec![
                ondin_core::Operation::CreateNode {
                    id,
                    parent: root,
                    index: i,
                    kind: NodeKind::Rect {
                        size: Size::new(10.0, 10.0),
                        corner_radii: RoundedRectRadii::default(),
                    },
                    transform: None,
                    name: None,
                },
            ]))
            .expect("a rect");
            made.push(id);
        }
        let (tx, container) =
            build::boolean(&doc, &mut ids, &made, op, key.map(|k| made[k])).expect("a boolean");
        doc.apply(&tx).expect("apply the boolean");
        let operands = doc
            .get(container)
            .expect("the container")
            .children()
            .to_vec();
        (doc, operands, made)
    }

    /// **The bottom operand of a `Subtract`, and only that one.** `children[0]`
    /// is the base and the tree draws topmost-first, so it is the bottom row —
    /// the fact §15 D113 makes true and nothing in the panel used to say.
    ///
    /// Asserted on the operand *above* it as well, which is the discriminating
    /// half: marking every child, or marking the last one, is what a reading of
    /// "the boolean's operands" rather than "the base" would produce, and both
    /// pass an assertion that only looks at `children[0]`.
    #[test]
    fn only_the_bottom_operand_of_a_subtract_is_the_base() {
        let (doc, operands, _) = boolean_of_two(BoolOp::Subtract, None);
        assert_eq!(operands.len(), 2, "the fixture built {operands:?}");
        assert!(
            subtract_base(&doc, doc.get(operands[0]).unwrap()),
            "the bottom operand is the shape being cut into"
        );
        assert!(
            !subtract_base(&doc, doc.get(operands[1]).unwrap()),
            "the operand above it is what cuts, not what is cut"
        );
    }

    /// **The other three fold commutatively**, so there is no base to name and a
    /// mark on one would claim a significance the arithmetic cannot see (§15
    /// D113). Union stands in for all three: they take the same arm.
    #[test]
    fn a_union_has_no_base_to_mark() {
        let (doc, operands, _) = boolean_of_two(BoolOp::Union, None);
        assert!(
            operands
                .iter()
                .all(|id| !subtract_base(&doc, doc.get(*id).unwrap())),
            "a union marked one of its operands"
        );
    }

    /// The key layer *is* the base, by having been moved to the bottom when the
    /// boolean was built — so the mark travels with the designation, which is
    /// the whole point of reading it off the tree rather than off a stored flag.
    ///
    /// Asserted by **identity**, not by position: `operands[0]` is the base
    /// under any spelling of this function, so the thing that has to be checked
    /// is *which shape ended up there* — the one that was second in z-order
    /// before the key moved it.
    #[test]
    fn designating_the_key_moves_the_mark_with_it() {
        let (_, plain_ops, plain_made) = boolean_of_two(BoolOp::Subtract, None);
        assert_eq!(
            plain_ops[0], plain_made[0],
            "with no key the bottom of the z-order is already the base"
        );
        let (doc, operands, made) = boolean_of_two(BoolOp::Subtract, Some(1));
        assert_eq!(
            operands[0], made[1],
            "the designated layer should have gone to the bottom"
        );
        assert!(subtract_base(&doc, doc.get(made[1]).unwrap()));
        assert!(
            !subtract_base(&doc, doc.get(made[0]).unwrap()),
            "the shape that was the base without a key must not still be one"
        );
    }

    /// A shape with no boolean over it is nobody's operand, and the root has no
    /// parent to ask at all.
    #[test]
    fn a_loose_shape_and_the_root_are_never_a_base() {
        let (mut doc, _, _) = boolean_of_two(BoolOp::Subtract, None);
        let root_id = doc.root();
        assert!(
            !subtract_base(&doc, doc.get(root_id).unwrap()),
            "the root has no parent"
        );
        // A rect sitting under the root beside the boolean — first child of a
        // container, exactly as a base is, and a base of nothing.
        let mut ids = IdSource::new(99);
        let loose = ids.mint();
        doc.apply(&ondin_core::Transaction(vec![
            ondin_core::Operation::CreateNode {
                id: loose,
                parent: root_id,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: RoundedRectRadii::default(),
                },
                transform: None,
                name: None,
            },
        ]))
        .expect("a loose rect");
        assert!(
            !subtract_base(&doc, doc.get(loose).unwrap()),
            "a first child of the root is not the base of anything"
        );
    }
}

#[cfg(test)]
mod badge_ink_tests {
    use super::*;

    /// The **ink** height of a laid-out string, in points.
    ///
    /// `uv_rect` is the tight glyph bitmap, so `uv_rect.size` is what was
    /// actually rasterized — where the galley's own height is the line box and
    /// says nothing about how tall the drawing inside it is. That difference is
    /// the whole reason this measurement exists: two things can share a line box
    /// and look nothing alike.
    fn ink_height(ctx: &egui::Context, s: &str, font: egui::FontId) -> f32 {
        let galley = ctx.fonts_mut(|f| f.layout_no_wrap(s.to_string(), font, egui::Color32::WHITE));
        let mut lo = f32::INFINITY;
        let mut hi = f32::NEG_INFINITY;
        for row in &galley.rows {
            for g in &row.row.glyphs {
                if g.uv_rect.size.y <= 0.0 {
                    continue;
                }
                lo = lo.min(g.uv_rect.offset.y);
                hi = hi.max(g.uv_rect.offset.y + g.uv_rect.size.y);
            }
        }
        hi - lo
    }

    /// **The badges share a slot, so they have to share a weight.** An artboard's
    /// size, the base operand's stack glyph and a mask's half-circle are the same
    /// kind of remark at the same right-hand edge, and a pictogram that inks
    /// taller than the word reads as a control rather than as an annotation.
    ///
    /// **Every glyph that can appear there, not just the first one, and each at
    /// its own size.** This asserted `stack-simple` alone until masks arrived, and
    /// the second pictogram inherited that size without anyone re-checking it —
    /// which is precisely the failure: Phosphor glyphs do not all ink to the same
    /// height, a circle reaching its extremes where a stack of bars does not, so
    /// `circle-half` at the stack's 11.0 came out 9.60 against the word's 8.80 at
    /// 125%. Extending the loop is what found that. Adding a third badge means
    /// adding it here, with a size measured rather than borrowed.
    ///
    /// Across four display scalings, because the atlas quantizes to whole texels
    /// and a size chosen at 100% can drift at 150% — a failure this codebase has
    /// already caught a "fix" making. `BADGE_STACK` at 12 is the plausible wrong
    /// version and it inks taller at every scaling above 100%.
    #[test]
    fn the_base_badge_inks_as_tall_as_the_worded_one() {
        for ppp in [1.0_f32, 1.25, 1.5, 2.0] {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let mut input = egui::RawInput::default();
            let vp = input.viewport_id;
            input
                .viewports
                .get_mut(&vp)
                .unwrap()
                .native_pixels_per_point = Some(ppp);
            let _ = ctx.run_ui(input, |_| {});

            let word = ink_height(&ctx, "800 × 600", egui::FontId::proportional(10.5));
            for (name, badge, size) in [
                ("stack", icon::STACK_SIMPLE, BADGE_STACK),
                ("half-circle", icon::CIRCLE_HALF, BADGE_MASK),
            ] {
                let glyph = ink_height(&ctx, badge, theme::icon_font(size));
                assert!(
                    (glyph - word).abs() <= 0.51,
                    "at {ppp}× the {name} glyph inks {glyph:.2} at {size} against \
                     the word's {word:.2}"
                );
            }
        }
    }
}

#[cfg(test)]
mod row_target_tests {
    use super::{CONTROL_HIT, RowHit, RowTargets};

    /// A row with all three controls out, at plausible x's.
    fn full() -> RowTargets {
        RowTargets {
            caret: Some(11.0),
            eye: Some(163.0),
            lock: Some(186.0),
        }
    }

    /// **A control the row is not drawing must not answer.** A childless row has
    /// no caret and a resting visible row shows no eye — and the row is one
    /// `click_and_drag` region, so nothing but this stops the band being live
    /// over empty space. It is also what stops the *hover* lighting a glyph that
    /// is not there, which is the half added in §15 D246: before it, both
    /// questions were separate hand-written comparisons and only the press asked
    /// whether the control existed.
    #[test]
    fn a_control_the_row_is_not_drawing_is_not_a_target() {
        let bare = RowTargets {
            caret: None,
            eye: None,
            lock: Some(186.0),
        };
        assert_eq!(bare.at(11.0), None, "a childless row has no caret to press");
        assert_eq!(
            bare.at(163.0),
            None,
            "a row showing no eye has no eye to press"
        );
        assert_eq!(bare.at(186.0), Some(RowHit::Lock));
    }

    /// The band is `CONTROL_HIT` either side and nothing beyond it — asserted at
    /// the edge and a hair past it, because a band that is right in the middle
    /// and wrong at its rim is exactly what a hover and a press written
    /// separately drift into.
    #[test]
    fn the_band_reaches_control_hit_either_side_and_stops() {
        let t = full();
        for d in [-CONTROL_HIT, -CONTROL_HIT + 0.5, 0.0, CONTROL_HIT] {
            assert_eq!(t.at(163.0 + d), Some(RowHit::Eye), "at offset {d}");
        }
        for d in [-CONTROL_HIT - 0.1, CONTROL_HIT + 0.1] {
            assert_ne!(t.at(163.0 + d), Some(RowHit::Eye), "at offset {d}");
        }
    }

    /// **Where two bands overlap, one of them has to win, and it has to be the
    /// same one for the mark and for the press.** The caret walks right with the
    /// indent while the eye is pinned to the right edge, so a deep row in a
    /// narrow panel can put them together — and the order here is the press's
    /// own, which is the whole reason both go through this function.
    #[test]
    fn overlapping_bands_resolve_the_press_order() {
        let together = RowTargets {
            caret: Some(160.0),
            eye: Some(163.0),
            lock: Some(186.0),
        };
        assert_eq!(together.at(161.0), Some(RowHit::Caret));
    }
}

#[cfg(test)]
mod tree_guide_tests {
    //! The dashed lines that say which container a row is inside, and the path
    //! that lights on hover.
    //!
    //! Most of this is asserted on `tree_guide_segments` rather than through the
    //! app, because every interesting claim is about rows *other than* the one being
    //! drawn — whether a trunk carries on, whether it has already died, which rows a
    //! hover lights — and a synthetic list of depths states those cases in one line
    //! each. The app test at the end is what proves the walk feeds this function and
    //! that the preference reaches the painter.
    use super::*;
    use crate::app::OndinApp;
    use ondin_core::kurbo::Size;
    use ondin_core::{Document, IdSource};

    /// The tree's own row pitch: rows do not touch, `item_spacing.y` is 1.
    const GAP: f32 = 1.0;

    /// `marked` with nothing selected.
    fn rows(depths: &[usize], hovered: Option<usize>) -> Vec<TreeRow> {
        marked(depths, hovered, &[])
    }

    /// Rows at the given depths, laid out down a panel whose left edge is 0 —
    /// `ROW_H` tall on a `ROW_H + GAP` pitch, which is how the tree lays them out.
    /// `hovered` and `selected` are indices into `depths`.
    ///
    /// `lead` is the caret row's, `caret_x - 7.5`; nothing below turns on which of
    /// the two a row is, only on where the elbow is asked to stop. The elbow goes
    /// through `elbow_y` rather than being written out, so the fixture cannot
    /// disagree with the panel about where a junction is.
    ///
    /// **`container` is read off the depths rather than passed in**: in a real walk an
    /// *open* container is exactly a row the next row is deeper than. That understates
    /// it — a collapsed or empty group is a container too — and nothing here or in
    /// `depth_range` turns on the difference, which is the note on
    /// `TreeRow::container` seen from the fixture's side.
    ///
    /// `pub(super)` because `drop_indent_tests` wants the same rows: what a boundary
    /// offers and what a guide draws are both read off a list of depths, and two
    /// fixtures free to disagree about what "a tree three deep" looks like would be two
    /// trees.
    pub(super) fn marked(
        depths: &[usize],
        hovered: Option<usize>,
        selected: &[usize],
    ) -> Vec<TreeRow> {
        let mut ids = IdSource::new(0x9A17);
        depths
            .iter()
            .enumerate()
            .map(|(i, &depth)| {
                let rect = egui::Rect::from_min_size(
                    egui::pos2(0.0, i as f32 * (ROW_H + GAP)),
                    egui::vec2(288.0, ROW_H),
                );
                TreeRow {
                    id: ids.mint(),
                    rect,
                    depth,
                    lead: caret_column(0.0, depth) - 7.5,
                    elbow: elbow_y(rect, 1.0),
                    selected: selected.contains(&i),
                    hovered: hovered == Some(i),
                    container: depths.get(i + 1).is_some_and(|&next| next > depth),
                    // A list of depths cannot say this — which is the point of the field
                    // and the reason it is not the negation of the one that went. Nothing
                    // reached from a `marked` fixture reads it; spring-loading is driven
                    // through the panel, over a real document.
                    closed: false,
                }
            })
            .collect()
    }

    /// The vertical runs in one row, top to bottom.
    fn verticals(segs: &[GuideSeg], row: &TreeRow) -> Vec<GuideSeg> {
        let mut v: Vec<GuideSeg> = segs
            .iter()
            .copied()
            .filter(|s| s.from.x == s.to.x && s.from.y >= row.rect.top() - 0.01)
            .filter(|s| s.from.y < row.rect.bottom() + 0.01)
            .collect();
        v.sort_by(|a, b| {
            a.from
                .x
                .total_cmp(&b.from.x)
                .then(a.from.y.total_cmp(&b.from.y))
        });
        v
    }

    /// The horizontal run in one row — its elbow.
    fn elbow(segs: &[GuideSeg], row: &TreeRow) -> GuideSeg {
        *segs
            .iter()
            .find(|s| s.from.y == s.to.y && (s.from.y - row.elbow).abs() < 0.01)
            .expect("an elbow")
    }

    /// **A row hangs off the column its parent's caret is on, and its elbow reaches
    /// its own glyph.**
    ///
    /// The two ends of the claim `caret_column` exists for: the guide is drawn in
    /// the child's row and the caret in the parent's, and neither computes the
    /// other's geometry. A top-level row is checked to have no guides at all —
    /// there is no row above it to hang off, the root being implicit.
    ///
    /// Flip-checked by hanging the connector off `caret_column(left, depth)` — the
    /// row's own column rather than its parent's, which is the plausible off-by-one
    /// — reporting 42 where 11 belongs.
    #[test]
    fn a_row_hangs_off_the_column_its_parents_caret_is_on() {
        let r = rows(&[1, 2], None);
        let segs = tree_guide_segments(&r, GAP);
        assert!(
            verticals(&segs, &r[0]).is_empty()
                && !segs.iter().any(|s| (s.from.y - r[0].elbow).abs() < 0.01),
            "a top-level row has nothing to hang off: {segs:#?}"
        );

        let column = caret_column(0.0, 1);
        assert_eq!(
            column, 11.0,
            "the frame's caret column, from the panel edge"
        );
        let v = verticals(&segs, &r[1]);
        assert_eq!(v.len(), 1, "one connector and no trunk past it: {v:#?}");
        assert_eq!(v[0].from, egui::pos2(column, r[1].rect.top()));
        assert_eq!(v[0].to, egui::pos2(column, r[1].elbow));

        let e = elbow(&segs, &r[1]);
        assert_eq!(
            e.from.x,
            column + ELBOW_CLEAR,
            "clear of the parent's column by one gap of the dash pattern, so the \
             corner is not a blot"
        );
        assert_eq!(
            e.to.x, r[1].lead,
            "and up to the row's own leading glyph, not past it"
        );
    }

    /// **A trunk runs down past every child but the last, and stops at that one's
    /// elbow.**
    ///
    /// The one rule that makes the gutter readable: a line that carried on past the
    /// last child would say the frame holds something further down, and a line that
    /// stopped at the first would leave the rest of them hanging off nothing.
    ///
    /// Flip-checked by drawing the below-elbow run unconditionally: the last child
    /// grows a second vertical and the assertion on its count fails.
    #[test]
    fn a_trunk_runs_to_the_last_child_and_stops_at_its_elbow() {
        let r = rows(&[1, 2, 2, 2], None);
        let segs = tree_guide_segments(&r, GAP);
        for (i, carries_on) in [(1, true), (2, true), (3, false)] {
            let v = verticals(&segs, &r[i]);
            assert_eq!(
                v.len(),
                if carries_on { 2 } else { 1 },
                "row {i} of three children: {v:#?}"
            );
            assert_eq!(v[0].to.y, r[i].elbow, "row {i}'s connector");
            if carries_on {
                assert_eq!(
                    v[1].to.y,
                    r[i].rect.bottom() + GAP,
                    "row {i}'s trunk crosses the gap to the row below, or the line \
                     breaks once per row"
                );
            }
        }
    }

    /// ⚠️ **A trunk whose branch has already closed is not drawn.**
    ///
    /// `A → B → C`, each an only child. A's trunk ends at B's elbow, so C's row —
    /// which is *inside* A — must draw nothing in A's column. This is the case the
    /// design's markup does not contain and the one a literal reading of it gets
    /// wrong: there, every row carries a stub-height background for every level.
    ///
    /// Flip-checked against exactly that reading — ending a closed level's run at
    /// the elbow rather than leaving it out — which puts a second run in C's row, at
    /// A's column, under a line that stopped a row earlier. This is the only one of
    /// the six here that bites for that flip.
    #[test]
    fn a_trunk_that_already_ended_leaves_no_fragment_in_a_deeper_row() {
        let r = rows(&[1, 2, 3], None);
        let segs = tree_guide_segments(&r, GAP);
        let v = verticals(&segs, &r[2]);
        assert_eq!(v.len(), 1, "C hangs off B and off nothing else: {v:#?}");
        assert_eq!(
            v[0].from.x,
            caret_column(0.0, 2),
            "and the one run it has is B's column, not A's"
        );
    }

    /// **Hovering a row lights the path from its parent down to its own elbow, and
    /// nothing else.**
    ///
    /// Four separate claims, and each is a different way of getting this wrong: the
    /// parent's trunk lights over the rows *between* the two, the hovered row's
    /// connector lights but the same trunk below its elbow does not, the elbow of an
    /// intervening row stays dark, and no other level lights at all. A lit ladder
    /// down every ancestor answers "how deep is this" — which the indent already
    /// says — where the point is "which one is it inside".
    ///
    /// Flip-checked twice. Lighting the trunk over `start..=h` rather than `start..h`
    /// — the version that reads more naturally — lights the hovered row's trunk
    /// below its elbow and fails the second claim. Dropping the `*l == level` test
    /// lights the level-1 trunk as well and fails the fourth.
    #[test]
    fn hovering_a_row_lights_the_trunk_it_hangs_off() {
        // frame ▸ group ▸ three deep rows ▸ a second child of the frame. ⚠️ Both
        // extras are load-bearing: the hovered row needs a **sibling after it** or
        // it has no trunk below its own elbow and the second claim is vacuous — the
        // first version of this test had none, and the `start..=h` flip below did not
        // bite. The frame's second child is what puts a level-1 trunk through the
        // deep rows for the fourth claim to be about.
        let r = rows(&[1, 2, 3, 3, 3, 2], Some(3));
        let segs = tree_guide_segments(&r, GAP);
        let level2 = caret_column(0.0, 2);
        let level1 = caret_column(0.0, 1);

        let lit = |s: &GuideSeg| s.ink == GuideInk::Hovered;

        let between = verticals(&segs, &r[2]);
        for s in between.iter().filter(|s| s.from.x == level2) {
            assert!(
                lit(s),
                "the whole of the parent's trunk between the two rows lights: {s:?}"
            );
        }
        assert!(
            between.iter().any(|s| s.from.x == level2),
            "the fixture: row 2 does carry the parent's trunk"
        );
        let frames_trunk: Vec<GuideSeg> = between
            .iter()
            .copied()
            .filter(|s| s.from.x == level1)
            .collect();
        assert_eq!(
            frames_trunk.len(),
            1,
            "the fixture: row 2 also carries the frame's trunk: {between:#?}"
        );
        assert_eq!(
            frames_trunk[0].ink,
            GuideInk::Rest,
            "a level the hover is not about"
        );
        assert_eq!(
            elbow(&segs, &r[2]).ink,
            GuideInk::Rest,
            "only the hovered row's own elbow lights — an intervening row's is the \
             path passing through, not a row being pointed at"
        );

        let hovered = verticals(&segs, &r[3]);
        let connector = hovered
            .iter()
            .find(|s| s.from.x == level2 && s.from.y == r[3].rect.top())
            .expect("the hovered row's connector");
        assert!(lit(connector), "the connector into the hovered row lights");
        assert!(lit(&elbow(&segs, &r[3])), "and so does its elbow");
        let under: Vec<GuideSeg> = hovered
            .iter()
            .copied()
            .filter(|s| s.from.x == level2 && s.from.y > r[3].rect.top())
            .collect();
        assert_eq!(
            under.len(),
            1,
            "the fixture: the hovered row has a sibling after it, so there is a \
             trunk under its elbow to be about"
        );
        assert_eq!(
            under[0].ink,
            GuideInk::Rest,
            "below the hovered row's elbow the trunk goes back to rest"
        );

        for (i, row) in r.iter().enumerate() {
            for s in verticals(&segs, row).iter().filter(|s| s.from.x == level1) {
                assert!(!lit(s), "row {i}: the frame's trunk is not on the path");
            }
            if i > 3 {
                assert!(
                    verticals(&segs, row).iter().all(|s| !lit(s)) && !lit(&elbow(&segs, row)),
                    "row {i} is below the hovered one and lights nothing"
                );
            }
        }
    }

    /// **A selected row wears its leg in the selection blue, and hover takes the
    /// overlap.**
    ///
    /// Three claims, all reported as one rule from the machine. The selected row's
    /// leg — its parent's trunk over the rows between, its connector, its elbow — is
    /// `GuideInk::Selected`. Hovering *another* row greys only the part of that trunk
    /// on the hovered row's own path, so the blue survives below it. And hovering the
    /// selected row itself changes nothing: *"the currently selected layer doesn't
    /// apply a hover anymore"*.
    ///
    /// Flip-checked three times, one per claim: testing `selected` before `hovered`
    /// keeps the leg blue under another row's hover and fails the second; dropping
    /// the `&& !r.selected` greys the whole leg the moment the pointer lands on it
    /// and fails the third; and marking the *whole* trunk rather than the path
    /// (`over.contains(&i) || i == owner` without the `connector` term) turns the
    /// stretch below the selected row's elbow blue, which is the fourth assertion.
    #[test]
    fn a_selected_row_wears_its_leg_in_the_selection_blue() {
        // frame ▸ group ▸ three deep rows, the last of them selected, so its leg
        // crosses two rows and there is a stretch of trunk below its own elbow that
        // must *not* light.
        let level2 = caret_column(0.0, 2);
        let depths = [1, 2, 3, 3, 3];
        let leg = |segs: &[GuideSeg], row: &TreeRow| -> Vec<GuideSeg> {
            verticals(segs, row)
                .into_iter()
                .filter(|s| s.from.x == level2)
                .collect()
        };

        let r = marked(&depths, None, &[3]);
        let segs = tree_guide_segments(&r, GAP);
        for i in [2, 3] {
            for s in leg(&segs, &r[i]) {
                let below_the_elbow = s.from.y > r[i].rect.top();
                assert_eq!(
                    s.ink,
                    if i == 3 && below_the_elbow {
                        GuideInk::Rest
                    } else {
                        GuideInk::Selected
                    },
                    "row {i}, {}: {s:?}",
                    if below_the_elbow {
                        "under the elbow"
                    } else {
                        "the connector"
                    }
                );
            }
        }
        assert_eq!(
            elbow(&segs, &r[3]).ink,
            GuideInk::Selected,
            "the selected row's own elbow"
        );
        assert_eq!(
            elbow(&segs, &r[2]).ink,
            GuideInk::Rest,
            "and not an intervening row's"
        );

        // The row between the selection and its parent, hovered: grey down to *its*
        // elbow, blue from there on.
        let crossed = tree_guide_segments(&marked(&depths, Some(2), &[3]), GAP);
        assert_eq!(
            leg(&crossed, &r[2])
                .iter()
                .map(|s| s.ink)
                .collect::<Vec<_>>(),
            vec![GuideInk::Hovered, GuideInk::Selected],
            "hover takes the connector it points at and leaves the rest of the leg \
             blue"
        );
        assert_eq!(
            leg(&crossed, &r[3])[0].ink,
            GuideInk::Selected,
            "and the selected row's own connector, below all of that, is untouched"
        );

        // The selected row itself, hovered.
        let same = tree_guide_segments(&marked(&depths, Some(3), &[3]), GAP);
        assert_eq!(
            leg(&same, &r[2]).iter().map(|s| s.ink).collect::<Vec<_>>(),
            vec![GuideInk::Selected, GuideInk::Selected],
            "a hover on the selected row adds nothing — the panel is already \
             talking about it"
        );
        assert_eq!(elbow(&same, &r[3]).ink, GuideInk::Selected);
    }

    /// Root → one frame → one group → two rects: three deep, which is the shallowest
    /// tree that has an ancestor trunk *and* a branch that closes.
    fn app_with_a_nested_tree(ctx: &egui::Context) -> OndinApp {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x1AEA);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let frame = ids.mint();
        let group = ids.mint();
        let mut ops = vec![
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 300.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: group,
                parent: frame,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
        ];
        for i in 0..2 {
            ops.push(Operation::CreateNode {
                id: ids.mint(),
                parent: group,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            });
        }
        doc.apply(&Transaction(ops)).expect("the tree");
        app.session.adopt_document(doc, None);
        app
    }

    /// Every line segment the panel paints, with the pointer wherever `at` says.
    ///
    /// ⚠️ **Five passes.** A widget's interaction state is last frame's, so one pass
    /// reports nothing hovered however the pointer is placed. Nothing here animates
    /// its geometry, so this is settling rather than sampling.
    fn dashes(
        app: &mut OndinApp,
        ctx: &egui::Context,
        at: Option<egui::Pos2>,
    ) -> Vec<(egui::Pos2, egui::Pos2, egui::Color32)> {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(300.0, 600.0),
            )),
            ..Default::default()
        };
        if let Some(p) = at {
            input.events.push(egui::Event::PointerMoved(p));
        }
        let mut full = ctx.run_ui(input.clone(), |ui| app.layers_tree(ui));
        for _ in 1..5 {
            full = ctx.run_ui(input.clone(), |ui| app.layers_tree(ui));
        }
        full.shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::LineSegment { points, stroke } => {
                    Some((points[0], points[1], stroke.color))
                }
                _ => None,
            })
            .collect()
    }

    /// ⚠️ **The trunk is inked through the junction the elbow leaves it at.**
    ///
    /// The reported defect: *"a slight gap on the vertical lines where they meet the
    /// horizontal ones"*. The cause was a dash **offset** on the below-elbow run,
    /// phasing it to continue the lattice the row's top had started, which put the
    /// pattern's 2.5pt gap directly under every corner — three points of nothing
    /// exactly where the eye follows the line round the bend.
    ///
    /// Asserted on the **painted dashes**, since the defect is inside a run rather
    /// than in the list of runs: for every elbow whose trunk carries on below it, the
    /// first dash under the elbow has to start *at* the elbow.
    ///
    /// The other half — that ink arrives from above — is a guard rather than the
    /// biting half, and this is worth knowing before trusting it: the connector's
    /// last dash **ended exactly at the elbow in the broken version too**, so an
    /// assertion phrased as "there is ink at the elbow's y" passes either way. What
    /// changed is only what happens below it.
    ///
    /// Flip-checked by restoring the offset — `(-(elbow - top)).rem_euclid(4.0)` —
    /// which reports the trunk resuming **2.5pt** below the elbow. It was 3 in the
    /// version that shipped, the other half-point being the unsnapped elbow that
    /// `elbow_y` now fixes.
    #[test]
    fn the_trunk_is_inked_through_the_junction() {
        let ctx = egui::Context::default();
        let mut app = app_with_a_nested_tree(&ctx);
        let all = dashes(&mut app, &ctx, None);
        let vertical = |c: f32| -> Vec<(egui::Pos2, egui::Pos2)> {
            all.iter()
                .filter(|(a, b, _)| a.x == b.x && (a.x - c).abs() < 0.01)
                .map(|(a, b, _)| (*a, *b))
                .collect()
        };

        // Each elbow, as the y it sits on and the column it leaves. ⚠️ Every *dash*
        // of the run comes back separately, so the run's own start is the leftmost
        // of them — which is where `ELBOW_CLEAR` was measured from, off an unsnapped
        // column, while the trunk is drawn on the snapped one.
        let mut ys: Vec<f32> = all
            .iter()
            .filter(|(a, b, _)| a.y == b.y)
            .map(|(a, ..)| a.y)
            .collect();
        ys.sort_by(f32::total_cmp);
        ys.dedup();
        assert_eq!(ys.len(), 3, "the fixture: three rows have elbows");
        let elbows: Vec<(f32, f32)> = ys
            .iter()
            .map(|&y| {
                let start = all
                    .iter()
                    .filter(|(a, b, _)| a.y == b.y && a.y == y)
                    .map(|(a, ..)| a.x)
                    .fold(f32::INFINITY, f32::min);
                (y, snap_across(start - ELBOW_CLEAR, 1.0, 1.0).0)
            })
            .collect();

        let mut carried = 0;
        for (y, column) in elbows {
            let runs = vertical(column);
            assert!(
                runs.iter().any(|(a, b)| a.y < y && b.y >= y - 0.01),
                "the guard: the trunk at x {column} reaches the elbow at y {y} from \
                 above at all"
            );
            let Some(first) = runs
                .iter()
                .filter(|(_, b)| b.y > y + 0.01)
                .map(|(a, _)| a.y)
                .min_by(f32::total_cmp)
            else {
                continue; // a last child: nothing carries on below this elbow
            };
            carried += 1;
            assert!(
                first <= y + 0.01,
                "the trunk at x {column} resumes {:.1}pt below the elbow at y {y}",
                first - y
            );
        }
        assert!(
            carried > 0,
            "the fixture: at least one elbow has a trunk carrying on below it, or \
             the assertion above was never reached"
        );
    }

    /// **The pointer on a row lights that row's guides, through the panel.**
    ///
    /// The *rule* is `tree_guide_segments`' and is pinned above; what this adds is
    /// the wiring — that the `hovered` a row hands the guides is the same
    /// `resp.hovered()` its own ground is painted from, so the two cannot disagree
    /// for a frame. It is also the only assertion here that the guides light at all
    /// in the running app.
    ///
    /// The pointer goes on the **last** row, whose parent is two rows up, so the lit
    /// path has to cross a row to reach it. With the pointer below the tree nothing
    /// lights, which is the half that makes the other one mean something.
    ///
    /// Flip-checked by pushing `hovered: false` into every `TreeRow`: the lit count
    /// comes back 0 where 18 belongs.
    #[test]
    fn the_pointer_on_a_row_lights_its_guides() {
        let ctx = egui::Context::default();
        let mut app = app_with_a_nested_tree(&ctx);
        let lit = |v: &[(egui::Pos2, egui::Pos2, egui::Color32)]| {
            v.iter().filter(|(.., c)| *c == GUIDE_HOVER).count()
        };

        let away = dashes(&mut app, &ctx, Some(egui::pos2(250.0, 500.0)));
        assert!(!away.is_empty(), "the fixture: the panel is drawing guides");
        assert_eq!(
            lit(&away),
            0,
            "with the pointer below the tree nothing is on the path"
        );

        // Four rows, 26pt on a 27pt pitch from the top of the panel: y 100 is the
        // last of them.
        let on_row = dashes(&mut app, &ctx, Some(egui::pos2(120.0, 100.0)));
        assert!(lit(&on_row) > 0, "the pointer on a row lights its path");
        for (a, b, _) in on_row.iter().filter(|(.., c)| *c == GUIDE_HOVER) {
            if a.x == b.x {
                assert_eq!(
                    a.x, 42.5,
                    "the lit trunk is the one the row hangs off, not another level"
                );
            }
        }
    }

    /// **The walk feeds the guides, and the preference reaches the painter.**
    ///
    /// The one thing the segment tests above cannot say: that `layers_row` collects
    /// the rows it draws and that `Prefs::tree_guides` is read where the drawing
    /// happens. Asserted on the dashes' x's as well as their count, because "some
    /// line segments arrived" would pass for a drop hint or a rule.
    ///
    /// ⚠️ The columns are the *snapped* ones — 11.5 and 42.5 rather than 11 and 42 —
    /// which is `paint_tree_guides` putting a 1pt hairline on a whole device pixel
    /// at 100%.
    ///
    /// Flip-checked by dropping the `self.prefs.tree_guides` test: the second half
    /// fails with the same count as the first.
    #[test]
    fn the_panel_draws_the_guides_and_the_preference_switches_them_off() {
        let ctx = egui::Context::default();
        let mut app = app_with_a_nested_tree(&ctx);
        let on = dashes(&mut app, &ctx, None);
        assert!(
            !on.is_empty(),
            "a tree three deep draws guides, and this one drew none"
        );
        for (a, b, _) in &on {
            if a.x == b.x {
                assert!(
                    [11.5_f32, 42.5].contains(&a.x),
                    "a vertical guide off the caret columns, at x {}",
                    a.x
                );
            } else {
                assert_eq!(a.y, b.y, "a guide that is neither vertical nor horizontal");
            }
        }
        assert!(
            on.iter().any(|(a, b, _)| a.y == b.y && a.x != b.x),
            "and the elbows are drawn too, not only the trunks"
        );

        app.prefs.tree_guides = false;
        assert!(
            dashes(&mut app, &ctx, None).is_empty(),
            "switched off, the panel draws no line at all"
        );
    }
}

/// Dropping follows the indent: one gap between two rows is a *run* of insertion
/// points, one per level, and the pointer's x picks which.
#[cfg(test)]
mod drop_indent_tests {
    use super::*;
    use crate::app::OndinApp;
    use ondin_core::kurbo::Size;
    use ondin_core::{Document, IdSource};

    /// ```text
    /// root
    /// ├ 0 e            ← drawn last, at the bottom
    /// └ 1 frame
    ///     ├ 0 d
    ///     └ 1 group
    ///         ├ 0 c1
    ///         └ 1 c2   ← drawn first inside the group
    /// ```
    ///
    /// The walk reverses each child list (§5.3), so top to bottom the rows are
    /// `frame, group, c2, c1, d, e` at depths `1, 2, 3, 3, 2, 1`.
    ///
    /// ⚠️ **The group holds *two* rects and the frame holds something besides the
    /// group, and both are load-bearing.** With one rect in the group, dragging it to
    /// the bottom of the group is a drop beside itself, so "stayed inside" and "went
    /// nowhere" are the same outcome and the interesting assertion is vacuous. Without
    /// `d`, the boundary under the group has no shallower row below it and the range
    /// collapses to one level — there would be nothing to pick between.
    struct Fixture {
        app: OndinApp,
        root: NodeId,
        frame: NodeId,
        group: NodeId,
        c1: NodeId,
        c2: NodeId,
        d: NodeId,
        e: NodeId,
    }

    fn fixture(ctx: &egui::Context) -> Fixture {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = IdSource::new(0x0D40);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (frame, group) = (ids.mint(), ids.mint());
        let (c1, c2, d, e) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
        let rect = |id, parent, index| Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        };
        let ops = vec![
            rect(e, root, 0),
            Operation::CreateNode {
                id: frame,
                parent: root,
                index: 1,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 300.0),
                },
                transform: None,
                name: None,
            },
            rect(d, frame, 0),
            Operation::CreateNode {
                id: group,
                parent: frame,
                index: 1,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            rect(c1, group, 0),
            rect(c2, group, 1),
        ];
        doc.apply(&Transaction(ops)).expect("the tree");
        app.session.adopt_document(doc, None);
        Fixture {
            app,
            root,
            frame,
            group,
            c1,
            c2,
            d,
            e,
        }
    }

    /// ⚠️ **Five passes**, for the same reason `tree_guide_tests::dashes` takes
    /// five: a widget's interaction state is last frame's, so one pass reports
    /// nothing hovered however the pointer is placed.
    fn pump(app: &mut OndinApp, ctx: &egui::Context, at: Option<egui::Pos2>) -> egui::FullOutput {
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(300.0, 600.0),
            )),
            ..Default::default()
        };
        if let Some(p) = at {
            input.events.push(egui::Event::PointerMoved(p));
        }
        // ⚠️ **The button goes down on the first pass and stays down** (§15 D471).
        // These tests simulate a *drag*, and `drop_hint` refuses to read a target
        // while no primary button is held — a hover is not a drop, which is what
        // let a drag stranded by the panel's own chord land on the next ordinary
        // click. Pressing once and letting egui carry `primary_down` is the
        // faithful shape: repeating the press in all five passes would work too
        // and would be a hand on a key rather than a hand on a mouse.
        let mut first = input.clone();
        if let Some(p) = at {
            first.events.push(press(p, true));
        }
        let mut full = ctx.run_ui(first, |ui| app.layers_tree(ui));
        for _ in 1..5 {
            full = ctx.run_ui(input.clone(), |ui| app.layers_tree(ui));
        }
        full
    }

    /// A primary press or release at `p`.
    fn press(p: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// `pump` with the frame clock pinned, for the one thing here that is about time.
    ///
    /// ⚠️ **All five passes see the same `t`.** That is what makes the wait explicit
    /// rather than a race with the wall clock: the pass that first sees the group starts
    /// the timer at `t`, and the four settling passes after it read `t − t = 0`, so a
    /// spring can only fire on a later call with a later `t`. `pump` itself deliberately
    /// leaves `time` unset and runs on egui's own clock, because pinning it there would
    /// change the conditions of every test that already passes through it.
    fn pump_at(
        app: &mut OndinApp,
        ctx: &egui::Context,
        at: egui::Pos2,
        t: f64,
    ) -> egui::FullOutput {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(300.0, 600.0),
            )),
            time: Some(t),
            events: vec![egui::Event::PointerMoved(at)],
            ..Default::default()
        };
        // The button down, for [`pump`]'s reason (§15 D471).
        let mut first = input.clone();
        first.events.push(press(at, true));
        let mut full = ctx.run_ui(first, |ui| app.layers_tree(ui));
        for _ in 1..5 {
            full = ctx.run_ui(input.clone(), |ui| app.layers_tree(ui));
        }
        full
    }

    /// The six row boxes, top to bottom.
    ///
    /// **Read out of the paint rather than worked out here.** A row's y depends on
    /// `item_spacing` and on whatever margin the `Ui` starts with, and a test that
    /// derives them for itself agrees with its own arithmetic rather than with the
    /// panel. Selecting every layer makes each row paint a `SELECT_ROW` ground, which
    /// is one box per row and nothing else in that colour.
    fn row_rects(f: &mut Fixture, ctx: &egui::Context, light: &[NodeId]) -> Vec<egui::Rect> {
        f.app.session.selection.set(light.to_vec());
        let full = pump(&mut f.app, ctx, None);
        f.app.session.selection.clear();
        let mut rects: Vec<egui::Rect> = full
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Rect(r) if r.fill == color::SELECT_ROW => Some(r.rect),
                _ => None,
            })
            .collect();
        rects.sort_by(|a, b| a.top().total_cmp(&b.top()));
        assert_eq!(
            rects.len(),
            light.len(),
            "the fixture: one ground per lit row"
        );
        rects
    }

    /// Every row of an untouched fixture, top to bottom.
    ///
    /// ⚠️ **Which rows are lit is not free, which is why `row_rects` takes them.**
    /// `sync_tree_to_selection` opens every ancestor of anything selected, so lighting a
    /// row *inside* a group expands that group — and a test that collapses one and then
    /// measures with this would be measuring a tree it had just re-opened. It is not
    /// hypothetical: the spring-loading test below was written that way and its fixture
    /// assertion caught it at six rows against the four it expected.
    fn open_rows(f: &mut Fixture, ctx: &egui::Context) -> Vec<egui::Rect> {
        let all = [f.frame, f.group, f.c1, f.c2, f.d, f.e];
        row_rects(f, ctx, &all)
    }

    /// **A drag with no button behind it paints nothing and lands nothing**
    /// (§15 D471) — `[S17-L1-01]`.
    ///
    /// Pick a row up and, still holding the button, press the panel's own chord
    /// (`Ctrl+Alt+\`, or `Ctrl+\` for present mode). `input::resolve` is *"a pure
    /// resolution of this frame's events, with no state of its own"* and
    /// `dispatch` hands `ToggleView` straight to `set_view_switch` with no
    /// in-flight guard, so the panel is skipped on the very frame the chord
    /// lands — and `finish_layer_drag` runs inside `layers_tree`, which is not
    /// drawn. **The release goes nowhere and `layer_drag` survives it.**
    ///
    /// Bring the panel back and `drop_hint` read `pointer_interact_pos` alone,
    /// with no test that a button was down. Measured: the tree lit a drop host,
    /// painted the blue line, re-derived a target from wherever the pointer was
    /// *resting*, and the release of the next ordinary click committed it —
    /// `undo_depth 0 → 1`, `c1.parent` root where it had been the group, toast
    /// *"Moved layer"*, against a control at 0.
    ///
    /// ⚠️ **Guarded in `drop_hint` rather than at the view switch, because that
    /// closes the class.** Clearing the drag in `set_view_switch` closes the one
    /// route; a drag stranded by any *future* route still paints and still lands.
    /// It is also the recovery on the present-mode route, where `escape` cannot
    /// be one: its first rung is `self.present = false`, which restores the panel
    /// with the phantom drag live, and only a *second* Escape reaches
    /// `cancel_gesture`.
    ///
    /// ⚠️ **The guard broke six existing tests, and they were the ones that were
    /// wrong.** `pump` and `pump_at` drove a `PointerMoved` and no press, so
    /// every drop-target test in this module was simulating a drag with the mouse
    /// button up — the exact state the defect is about, asserted as if it were
    /// normal. Both helpers press on the first pass now and let egui carry
    /// `primary_down`.
    ///
    /// ⚠️ **Flipped** by deleting the `primary_down` early return: fails on the
    /// first assertion, the panel answering a drop target for a pointer merely
    /// resting on a row. The control below — the same position with the button
    /// held — is what says this is not a test about `drop_hint` having stopped
    /// working.
    ///
    /// 🚨 **Amended by §15 D582, and the amendment is why this test is not the
    /// whole story.** D471's guard was a bare `return`, which left the stranded
    /// drag *live* and merely unpainted — so it also had to refuse the frame a
    /// legitimate release arrives on, and that killed every drop in the panel for
    /// a day. The guard now **clears** the drag instead, and admits the release
    /// frame. This test passes either way, because it never releases anything:
    /// what it measures is that a button-less pointer resolves no target and
    /// draws no line, and both spellings do that. The release side is
    /// `drag_start_sync_tests::releasing_the_primary_over_the_tree_lands_the_drop`
    /// and it is a **different** assertion — a green run here says nothing at all
    /// about it, which is exactly how the regression got in.
    #[test]
    fn a_drag_with_no_button_behind_it_paints_nothing() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let rows = open_rows(&mut f, &ctx);
        let over_root_row = rows[0].center();

        /// Arm a drag and pump with the button either held or up, answering with
        /// the target the panel derived and how many drop lines it drew.
        fn stranded(
            f: &mut Fixture,
            ctx: &egui::Context,
            at: egui::Pos2,
            held: bool,
        ) -> (Option<DropTarget>, usize) {
            f.app.layer_drag = Some(LayerDrag {
                ids: vec![f.c1],
                target: None,
                grab: (0.0, 3),
                level: 3,
                lit: None,
                spring: None,
            });
            let input = |events: Vec<egui::Event>| egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(300.0, 600.0),
                )),
                events,
                ..Default::default()
            };
            let mut events = vec![egui::Event::PointerMoved(at)];
            if held {
                events.push(press(at, true));
            }
            let mut full = ctx.run_ui(input(events), |ui| f.app.layers_tree(ui));
            for _ in 1..5 {
                full = ctx.run_ui(input(vec![egui::Event::PointerMoved(at)]), |ui| {
                    f.app.layers_tree(ui)
                });
            }
            let lines = full
                .shapes
                .iter()
                .filter(|cs| {
                    matches!(&cs.shape,
                        egui::Shape::LineSegment { stroke, .. } if stroke.color == color::SELECT)
                })
                .count();
            let target = f.app.layer_drag.as_ref().and_then(|d| d.target);
            f.app.layer_drag = None;
            (target, lines)
        }

        assert_eq!(
            stranded(&mut f, &ctx, over_root_row, false),
            (None, 0),
            "a pointer merely resting over a row is not a drop: no target, no line"
        );
        let (target, _lines) = stranded(&mut f, &ctx, over_root_row, true);
        assert!(
            target.is_some(),
            "control: with the button held the same position is a live drop, \
             got {target:?}"
        );
        // ⚠️ **The control asserts the *target* and not the line**, because this
        // aim point is the middle of a container row — a drop *inside* it, which
        // lights the host's ground rather than drawing a between-rows line. The
        // target is the load-bearing half anyway: it is what a release commits.
        // Measured while writing this: `Some(DropTarget { parent: frame, index:
        // 2 })` with **0** lines, which is correct and would have failed a
        // careless `lines > 0`.
    }

    /// Put a drag in flight carrying `ids`, picked up at `grab`, and move the pointer
    /// to `to`. Answers with what the panel decided and where it drew the line.
    fn drag(
        f: &mut Fixture,
        ctx: &egui::Context,
        ids: Vec<NodeId>,
        grab: (f32, usize),
        to: egui::Pos2,
    ) -> (Option<DropTarget>, Vec<(egui::Pos2, egui::Pos2)>) {
        f.app.layer_drag = Some(LayerDrag {
            ids,
            target: None,
            grab,
            // The gesture starts holding the level it was picked up at, as the panel's
            // own `drag_started` arm does. ⚠️ **Not 0**: `depth_asked_for` measures
            // against this, so a fixture starting elsewhere would be a fixture whose
            // pointer means something different from the panel's.
            level: grab.1,
            lit: None,
            spring: None,
        });
        let full = pump(&mut f.app, ctx, Some(to));
        let lines = full
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::LineSegment { points, stroke } if stroke.color == color::SELECT => {
                    Some((points[0], points[1]))
                }
                _ => None,
            })
            .collect();
        let target = f.app.layer_drag.as_ref().and_then(|d| d.target);
        f.app.layer_drag = None;
        (target, lines)
    }

    /// **The reported gesture, end to end through the panel.** One gap — the one under
    /// the group's last row — and two answers, chosen by moving the pointer sideways:
    /// left of it the layer lands beside the group inside the frame, right of it the
    /// layer stays at the bottom of the group. And the line that says which is drawn at
    /// that level's own indent, a whole `INDENT` apart between the two.
    ///
    /// Flip-checked against the version this replaced rather than against no feature at
    /// all: before the change the boundary named one parent — whichever list the row
    /// under the pointer was in — so both x's were the row's left edge and both targets
    /// were `{group, 0}`. Every assertion below fails on that, which is the point;
    /// asserting only that *a* line is drawn would have passed it.
    #[test]
    fn one_gap_offers_a_level_per_indent_and_the_pointer_picks_it() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let rows = open_rows(&mut f, &ctx);
        // The gap between `c1` (row 3, depth 3) and `d` (row 4, depth 2): the last row
        // of the group, and the row after the group.
        let under_c1 = rows[3].bottom() - 4.0;
        // Picked up on `c2`'s name — the row above `c1`, and the same level.
        let grab_x = 150.0;
        let grab = (grab_x, 3);

        let c2 = f.c2;
        let (inside, inside_lines) =
            drag(&mut f, &ctx, vec![c2], grab, egui::pos2(grab_x, under_c1));
        assert_eq!(
            inside,
            Some(DropTarget {
                parent: f.group,
                index: 0,
            }),
            "straight down from the grab, the drop stays in the group, under `c1` — \
             which is model index 0, the walk being reversed"
        );

        let (outside, outside_lines) = drag(
            &mut f,
            &ctx,
            vec![c2],
            grab,
            egui::pos2(grab_x - INDENT, under_c1),
        );
        assert_eq!(
            outside,
            Some(DropTarget {
                parent: f.frame,
                index: 1,
            }),
            "one indent to the left, the same gap drops the layer out of the group and \
             beside it in the frame"
        );

        // ...and the line moved with it, by exactly one level.
        let left = |lines: &[(egui::Pos2, egui::Pos2)]| {
            assert_eq!(lines.len(), 1, "one drop, one line: {lines:?}");
            assert_eq!(lines[0].0.y, lines[0].1.y, "the drop line is horizontal");
            lines[0].0.x
        };
        let (deep, shallow) = (left(&inside_lines), left(&outside_lines));
        assert_eq!(
            deep - shallow,
            INDENT,
            "the two lines are drawn at the same x, so the panel is not saying which \
             level it means"
        );
        assert_eq!(
            deep - rows[3].left(),
            indent_of(3),
            "the deeper line starts where a depth-3 row starts"
        );
        assert_eq!(
            shallow - rows[3].left(),
            indent_of(2),
            "and the shallower one where a depth-2 row starts"
        );
    }

    /// **The other half of the asked-for gesture: making a layer belong to a group.**
    /// The middle of a container row means "inside it" and the indent has no vote there
    /// — which is worth pinning precisely *because* every other part of the drag now
    /// reads the pointer's x, so "this one does not" is a claim rather than an absence.
    /// It is also the only way into a collapsed group **without opening it**: the branch
    /// has no boundary of its own on screen for the indent gesture to reach, and the
    /// other way in — resting on it until it springs — leaves it open afterwards.
    ///
    /// And it is an outline, never a line: two lines around one row read as a box, and
    /// a box is what the tree uses to say "inside" (§15 D67).
    ///
    /// ⚠️ **`d` is what gets carried, not one of the group's own rects.** Dragging a
    /// node into the list it is already in makes `plan_layer_move` plan nothing, so the
    /// assertion would hold for a drop that does nothing at all.
    ///
    /// Flip-checked by making the middle of a container row an ordinary boundary — the
    /// version somebody would write on the way to "the x decides everything" — which
    /// fails on the leftmost of the three x's.
    #[test]
    fn the_middle_of_a_container_row_still_means_inside_it_whatever_the_x_says() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let rows = open_rows(&mut f, &ctx);
        // The group's own row, dead centre — and `d` is the thing being carried, so the
        // drop is a real reparent rather than a node landing in its own list.
        let d = f.d;
        let grab_x = 150.0;
        let centre = rows[1].center().y;
        // Left, centre and right across the whole row: the indent has no vote here.
        for x in [grab_x - INDENT * 2.0, grab_x, grab_x + INDENT] {
            let (target, lines) = drag(&mut f, &ctx, vec![d], (grab_x, 2), egui::pos2(x, centre));
            assert_eq!(
                target,
                Some(DropTarget {
                    parent: f.group,
                    // The end of the model list is the top of the group on screen, so
                    // the layer lands where the pointer is (§5.3).
                    index: 2,
                }),
                "x {x}: the middle of a container row is `into`, at the top of its list"
            );
            assert!(
                lines.is_empty(),
                "x {x}: `into` is an outline, never a line — two lines around a row read \
                 as a box, and a box is what says \"inside\" (§15 D67)"
            );
        }

        // ...and the outline is drawn, on that row and no other. Filtered by colour and
        // by `StrokeKind::Inside`, which is what `drop_hint` asks for and what a row's
        // own ground (a *filled* rect) is not.
        f.app.layer_drag = Some(LayerDrag {
            ids: vec![d],
            target: None,
            grab: (grab_x, 2),
            level: 2,
            lit: None,
            spring: None,
        });
        let full = pump(&mut f.app, &ctx, Some(egui::pos2(grab_x, centre)));
        f.app.layer_drag = None;
        let outlines: Vec<egui::Rect> = full
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Rect(r) if r.stroke.color == color::SELECT => Some(r.rect),
                _ => None,
            })
            .collect();
        assert_eq!(outlines, vec![rows[1]], "one outline, on the group's row");
    }

    /// **The gap between two rows is not a dead band.** Rows do not touch, so asking
    /// which row *contains* the pointer has no answer for the frame it spends crossing a
    /// boundary — and the indicator blinking off for one frame while the hand is moving
    /// steadily is exactly the kind of artefact that gets caught by stepping a capture.
    /// A pointer in the gap means the boundary the gap is.
    ///
    /// Flip-checked against `rect.y_range().contains(pointer.y)`, which is what this was
    /// first written as: the target comes back `None` and no line is drawn.
    #[test]
    fn the_gap_between_two_rows_is_the_boundary_rather_than_nothing() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let rows = open_rows(&mut f, &ctx);
        let gap = rows[4].top() - rows[3].bottom();
        assert!(gap > 0.0, "the fixture: the tree's rows do not touch");
        let c2 = f.c2;
        let grab_x = 150.0;
        let in_the_gap = egui::pos2(grab_x, rows[3].bottom() + gap / 2.0);
        let (target, lines) = drag(&mut f, &ctx, vec![c2], (grab_x, 3), in_the_gap);
        assert_eq!(
            target,
            Some(DropTarget {
                parent: f.group,
                index: 0,
            }),
            "the gap under `c1` names the same boundary its lower edge does"
        );
        assert_eq!(lines.len(), 1, "and the line is still drawn: {lines:?}");
    }

    /// **The pointer may leave the panel to the left, and the outdent needs it to.**
    ///
    /// A row grabbed by its icon is barely 20pt from the panel's edge, so reaching the
    /// top level from depth 3 asks the pointer for a negative x. Bounding the gesture
    /// with `rect.contains(pointer)` — which is what the old per-row target did, and the
    /// obvious thing to write — leaves the outdent reachable from some grab points and
    /// not from others, which reads as it working intermittently.
    ///
    /// To the right the rect still bounds it: past the panel is the canvas.
    #[test]
    fn the_outdent_still_works_when_the_pointer_has_left_the_panel() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let rows = open_rows(&mut f, &ctx);
        let under_d = rows[4].bottom() - 4.0;
        // Grabbed at the far left of a depth-2 row, then dragged one level further left
        // — which is off the panel altogether.
        let grab_x = 18.0;
        let to = egui::pos2(grab_x - INDENT, under_d);
        assert!(
            to.x < rows[4].left(),
            "the fixture: the pointer is off the panel"
        );
        let d = f.d;
        let (target, lines) = drag(&mut f, &ctx, vec![d], (grab_x, 2), to);
        assert_eq!(
            target,
            Some(DropTarget {
                parent: f.root,
                index: 1,
            }),
            "the gap under `d` at the top level puts the layer beside the frame"
        );
        assert_eq!(lines.len(), 1, "and the line is drawn: {lines:?}");

        // The same pointer to the *right* of the panel is off the tree and drops nothing.
        let (past, no_lines) = drag(
            &mut f,
            &ctx,
            vec![d],
            (grab_x, 2),
            egui::pos2(rows[4].right() + 20.0, under_d),
        );
        assert_eq!(past, None, "off the right-hand edge is not a drop target");
        assert!(no_lines.is_empty(), "and nothing is drawn: {no_lines:?}");
    }

    /// **A boundary offers the levels its two rows have in common, and no others.**
    ///
    /// The floor is the row below's depth because anything shallower would have to
    /// close containers those rows are still inside; the ceiling is the row above's
    /// because there is nothing deeper open there to go into.
    ///
    /// Flip-checked by taking the floor from the row *above* instead — the symmetrical
    /// mistake, and the one that reads more naturally next to the ceiling. It collapses
    /// every boundary to a single level, which is the whole feature gone, and it is
    /// caught at boundary 1 — *before* either of the two boundaries this feature is
    /// about, so the flip proves less here than the two drag tests it also fails.
    #[test]
    fn a_boundary_spans_the_levels_its_two_rows_share() {
        let rows = super::tree_guide_tests::marked(&[1, 2, 3, 3, 2, 1], None, &[]);
        let range = |b| {
            let r = depth_range(&rows, b);
            (*r.start(), *r.end())
        };
        // Above the first row: only the top level, and it is the only offer.
        assert_eq!(range(0), (1, 1));
        // Under the frame, which is open: its first child's level and nothing else.
        assert_eq!(range(1), (2, 2));
        assert_eq!(range(2), (3, 3));
        // Between the group's two rects: both are depth 3, so is the drop.
        assert_eq!(range(3), (3, 3));
        // Under the group's last rect, above `d`: **out of the group, or stay in it.**
        assert_eq!(range(4), (2, 3));
        // Under `d`, above `e`: out of the frame, or stay in it.
        assert_eq!(range(5), (1, 2));
        // Under the last row, which is itself top-level: the one level there is.
        assert_eq!(range(6), (1, 1));

        // The bottom of the list is only uninteresting because *this* tree closes
        // every branch before it ends. A tree whose last row is nested offers every
        // level from that row's out to the top, which is the drop at the very bottom
        // of the panel.
        let deep_last = super::tree_guide_tests::marked(&[1, 2, 3], None, &[]);
        assert_eq!(
            {
                let r = depth_range(&deep_last, 3);
                (*r.start(), *r.end())
            },
            (1, 3)
        );
    }

    /// **The indent is measured from where the row was picked up, never from the
    /// pointer's own x.** A row is grabbed by its name, which at depth 1 already starts
    /// 45pt in, so an absolute reading would jump to depth 2 before the pointer had
    /// moved at all.
    #[test]
    fn the_depth_asked_for_is_a_displacement_from_the_grab() {
        // Not moved: the level it was picked up at, wherever along the row that was.
        for x in [0.0_f32, 45.0, 150.0, 288.0] {
            assert_eq!(
                depth_asked_for(3, x, x, 3),
                3,
                "grabbed at x {x} and held still"
            );
        }
        // One indent each way.
        assert_eq!(depth_asked_for(3, 150.0, 150.0 + INDENT, 3), 4);
        assert_eq!(depth_asked_for(3, 150.0, 150.0 - INDENT, 3), 2);
        assert_eq!(depth_asked_for(3, 150.0, 150.0 - INDENT * 2.0, 3), 1);
        // Dragged far past the left edge: it bottoms out rather than wrapping round,
        // which an unsigned subtraction would do.
        assert_eq!(depth_asked_for(2, 20.0, -400.0, 2), 1);
    }

    /// **The level sticks to the one being held, and that is what stops the flicker.**
    /// Reported from the machine as *"moving the line between indents is a bit twitchy"*:
    /// plain rounding puts a hard edge exactly half an indent from each column, and a
    /// hand that is not aiming at either level — one holding still, or moving *down* the
    /// tree with a few points of sideways drift — sits on that edge and the line flips
    /// between two indents frame to frame.
    ///
    /// Two claims, and the second is the one that makes it hysteresis rather than a wider
    /// dead zone: the band is centred on the level being **held**, so it moves with the
    /// answer, and leaving rightwards and returning leftwards happen at *different* x.
    ///
    /// Flip-checked at `STICK = 0.5`, which is the same as no feature at all — plain
    /// rounding, since `|raw − held| > 0.5` and `round(raw) != held` then name the same
    /// set. ⚠️ **Only the ±0.64 ends of the first loop fail on that flip**, and that is
    /// worth saying rather than claiming the whole test bites: everything nearer the
    /// column agrees with plain rounding by construction, so those two entries are the
    /// test. The pair below them fails too, which is the stronger half — plain rounding
    /// has no history to disagree with itself about.
    #[test]
    fn the_level_sticks_until_the_pointer_has_really_left_it() {
        let g = 150.0_f32;
        // Held at 3. Anywhere inside ±STICK of level 3's column stays 3, including the
        // ±0.5 that bare rounding treats as the edge between two levels.
        for f in [-0.64_f32, -0.5, -0.49, -0.2, 0.0, 0.2, 0.49, 0.5, 0.64] {
            assert_eq!(
                depth_asked_for(3, g, g + INDENT * f, 3),
                3,
                "{f} of an indent from the held level is still that level"
            );
        }
        // Past the band it re-rounds, and to the *nearest* level — so leaving the band
        // rightwards lands on 4 rather than creeping, and a long throw does not stop one
        // level short of where the hand is.
        assert_eq!(depth_asked_for(3, g, g + INDENT * 0.66, 3), 4);
        assert_eq!(depth_asked_for(3, g, g - INDENT * 0.8, 3), 2);
        assert_eq!(depth_asked_for(3, g, g + INDENT * 2.9, 3), 6);

        // **The band travels with the level being held, which is the whole of the
        // hysteresis.** One x, two answers, decided by where the drag already was: 2.55
        // indents from the grab is inside the band of a drag holding 3 *and* inside the
        // band of one holding 2, so each keeps what it had. Nothing about the pointer
        // distinguishes those two frames — only the history does.
        let at = |f: f32, held| depth_asked_for(3, g, g + INDENT * (f - 3.0), held);
        assert_eq!(at(2.55, 3), 3, "arrived from the right, still 3");
        assert_eq!(at(2.55, 2), 2, "arrived from the left, still 2");
        assert_ne!(
            at(2.55, 3),
            at(2.55, 2),
            "the same x reads two ways depending on where the drag was — without that \
             there is no hysteresis, only a wider edge"
        );
        // And the two edges are genuinely at different x: 3 is held down to 2.35, where
        // plain rounding would have handed it back at 2.5.
        assert_eq!(at(2.4, 3), 3, "coming back from 3, not yet");
        assert_eq!(at(2.3, 3), 2, "and now");
    }

    /// **Rest a drag on a collapsed group and it opens after `SPRING_SECS`.** Asked for
    /// on the machine: *"drag a layer, hover over a group for 1 second, it auto expands"*.
    /// Without it a collapsed branch can only be dropped *onto*, never ordered within,
    /// and the way in is to abandon the drag and click the caret.
    ///
    /// Four claims, and the two negative ones carry it: it does **not** open on the frame
    /// the pointer arrives, and does **not** open early — so dragging across a collapsed
    /// group on the way somewhere else leaves the tree alone. Then that it does open at
    /// the second, and that the wait restarts rather than accumulates: two visits of 0.6s
    /// are not one of 1.2.
    ///
    /// Flip-checked by starting the timer afresh every frame (`spring = Some((id, now))`
    /// unconditionally, which is the shape you get from writing the assignment before the
    /// comparison): the group never opens at all and the first assertion below fails.
    #[test]
    fn resting_a_drag_on_a_collapsed_group_opens_it() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        f.app.collapsed.insert(f.group);
        // ⚠️ **Only the rows outside the group.** Lighting one *inside* it would reopen
        // it through `sync_tree_to_selection`, which walks the whole parent chain — the
        // measurement would undo the state being measured. See [`open_rows`].
        let outside = [f.frame, f.group, f.d, f.e];
        let rows = row_rects(&mut f, &ctx, &outside);
        assert!(
            f.app.collapsed.contains(&f.group),
            "the fixture: measuring the rows left the group collapsed"
        );
        // `frame, group, d, e` — the group is row 1 whether or not it is open.
        let centre = rows[1].center();
        let e = f.e;
        let start = |f: &mut Fixture| {
            f.app.layer_drag = Some(LayerDrag {
                ids: vec![e],
                target: None,
                grab: (centre.x, 1),
                level: 1,
                lit: None,
                spring: None,
            });
        };

        start(&mut f);
        pump_at(&mut f.app, &ctx, centre, 10.0);
        assert!(
            f.app.collapsed.contains(&f.group),
            "arriving is not resting: the group does not open on the frame it is reached"
        );
        pump_at(&mut f.app, &ctx, centre, 10.0 + SPRING_SECS * 0.6);
        assert!(
            f.app.collapsed.contains(&f.group),
            "and it does not open early — dragging across a collapsed group must leave \
             the tree as it found it"
        );
        pump_at(&mut f.app, &ctx, centre, 10.0 + SPRING_SECS + 0.05);
        assert!(
            !f.app.collapsed.contains(&f.group),
            "held for the full second, the branch opens"
        );

        // **The wait restarts, it does not accumulate.** Two visits of 0.6s with a step
        // off the group in between leave it shut, where a timer that kept counting would
        // have opened it on the second.
        f.app.collapsed.insert(f.group);
        f.app.layer_drag = None;
        start(&mut f);
        let away = egui::pos2(centre.x, rows[3].center().y);
        pump_at(&mut f.app, &ctx, centre, 20.0);
        pump_at(&mut f.app, &ctx, centre, 20.0 + SPRING_SECS * 0.6);
        pump_at(&mut f.app, &ctx, away, 20.0 + SPRING_SECS * 0.7);
        pump_at(&mut f.app, &ctx, centre, 20.0 + SPRING_SECS * 0.8);
        pump_at(&mut f.app, &ctx, centre, 20.0 + SPRING_SECS * 1.3);
        assert!(
            f.app.collapsed.contains(&f.group),
            "the second visit is 0.5s old, not 1.1s: leaving the group restarts the wait"
        );
    }

    /// **The container a drop would land in says so with its own ink.** The line says
    /// *where* and cannot say *which group*: two levels are a single 31pt step apart, so
    /// otherwise the reader counts indent columns by eye. Asked for as exactly this and
    /// no more — *"only highlight the text and icon of the group to blue, should be
    /// enough affordance"* — after a dashed outline round the row was built, seen, and
    /// taken out again.
    ///
    /// The two halves of the claim are that it marks the *destination parent* — not the
    /// row under the pointer, which during an outdent is one of that parent's
    /// grandchildren — and that nothing is marked at the top level, where the parent is
    /// the root and has no row.
    ///
    /// ⚠️ **This is the one assertion here that needs more than one pass, and `pump`'s
    /// five are why it holds.** A row's ink is painted while the walk passes it and the
    /// drop is not resolved until the walk is over, so the answer a row paints is the
    /// previous frame's (`LayerDrag::lit`). One pass would show nothing lit and prove
    /// the opposite of the truth.
    ///
    /// Flip-checked by clearing `lit` at the top of the frame instead of carrying last
    /// frame's answer across — which is the mistake with a comment three lines above it
    /// telling you to make it, since that is exactly what `target` beside it wants. The
    /// row's ink never changes and the first assertion fails on `[]`.
    ///
    /// ⚠️ **A second flip — lighting the row *above* the line rather than the
    /// destination parent — did not bite, and that is a fact about the code rather than
    /// a failed experiment.** `lit` is derived in exactly one place, from `target.parent`
    /// at the moment `target` is discarded, so a wrong answer written anywhere later is
    /// overwritten before any row reads it. The error this test names is therefore not
    /// reachable by a one-line slip; what would reintroduce it is someone deriving `lit`
    /// from the rows instead, and the assertions below are aimed at that.
    #[test]
    fn the_line_names_its_container_by_colouring_that_rows_ink() {
        let ctx = egui::Context::default();
        let mut f = fixture(&ctx);
        let rows = open_rows(&mut f, &ctx);
        let c2 = f.c2;
        let grab_x = 150.0;
        // The gap under `c1`, the group's last row: depth 3 stays in the group, depth 2
        // lands in the frame — the same two answers as the indent test, now asking which
        // row gets marked.
        let under_c1 = egui::pos2(grab_x, rows[3].bottom() - 4.0);

        /// Every galley the frame painted in `SELECT`, as `(text, y of its baseline)`.
        ///
        /// ⚠️ **The colour is read off the layout job's section, not off a mesh.**
        /// `FullOutput::shapes` is pre-tessellation, so there are no vertices to look
        /// at yet — the two places a galley carries its colour, and the one available
        /// here is the input rather than the ink. `painter.text` puts the colour there
        /// and nothing in this panel overrides it afterwards, so for these rows the two
        /// agree.
        fn lit_text(full: &egui::FullOutput) -> Vec<(String, f32)> {
            full.shapes
                .iter()
                .filter_map(|cs| match &cs.shape {
                    egui::Shape::Text(t)
                        if t.galley
                            .job
                            .sections
                            .iter()
                            .any(|s| s.format.color == color::SELECT) =>
                    {
                        Some((t.galley.text().to_string(), t.pos.y))
                    }
                    _ => None,
                })
                .collect()
        }

        let lit = |f: &mut Fixture, at: egui::Pos2| -> Vec<(String, f32)> {
            f.app.layer_drag = Some(LayerDrag {
                ids: vec![c2],
                target: None,
                grab: (grab_x, 3),
                level: 3,
                lit: None,
                spring: None,
            });
            let full = pump(&mut f.app, &ctx, Some(at));
            f.app.layer_drag = None;
            lit_text(&full)
        };

        // What the two rows are called and what glyphs they wear, asked of the document
        // rather than written out: an auto-generated name is not this test's business.
        let named = |id: NodeId, doc: &ondin_core::Document| {
            let n = doc.get(id).expect("in the tree");
            (n.name().to_string(), node_icon(n.kind()).0.to_string())
        };
        let (group_name, group_glyph) = named(f.group, &f.app.session.doc);
        let (frame_name, frame_glyph) = named(f.frame, &f.app.session.doc);

        let inside = lit(&mut f, under_c1);
        let texts: Vec<&str> = inside.iter().map(|(t, _)| t.as_str()).collect();
        assert!(
            texts.contains(&group_name.as_str()) && texts.contains(&group_glyph.as_str()),
            "a drop inside the group lights that row's name and glyph, not {texts:?}"
        );
        // ...and on the group's own row, which is what says it is *that* group rather
        // than some other row that happens to carry the same word.
        for (t, y) in &inside {
            assert!(
                *y >= rows[1].top() && *y <= rows[1].bottom(),
                "{t:?} was lit at y {y}, off the group's row {:?}",
                rows[1]
            );
        }

        // One indent left, the drop lands in the frame — and the frame is what lights.
        let outdented = lit(&mut f, egui::pos2(grab_x - INDENT, under_c1.y));
        let texts: Vec<&str> = outdented.iter().map(|(t, _)| t.as_str()).collect();
        assert!(
            texts.contains(&frame_name.as_str()) && texts.contains(&frame_glyph.as_str()),
            "outdented, the mark moves to the frame's row, not {texts:?}"
        );
        assert!(
            !texts.contains(&group_name.as_str()),
            "and leaves the group's, or both containers claim the drop: {texts:?}"
        );

        // At the top level the parent is the root, which has no row: nothing to mark, and
        // nothing that needed saying.
        let at_root = lit(&mut f, egui::pos2(grab_x, rows[5].bottom() - 4.0));
        assert!(
            at_root.is_empty(),
            "a drop at the top level lights no container: {at_root:?}"
        );
    }
}

#[cfg(test)]
mod panel_width_tests {
    //! What `MIN_W` and `MAX_W` are argued from (§15 D349).
    //!
    //! The panel is draggable now, so its width is a number the *user* sets and
    //! the bounds are the only thing standing between them and a broken picture.
    //! Both were derived rather than chosen, and a derivation nothing re-runs is a
    //! number that stops being true the first time a row is retuned — `INDENT`
    //! alone has moved twice (16 → 31, with the design asking for 22).
    //!
    //! ⚠️ **The composition below is the one thing here that is a copy.** Every
    //! *term* is the constant or function the row walk itself draws with, so
    //! retuning any of them moves these assertions; what is spelled twice is the
    //! order they are added in. It could only be shared by giving the row walk a
    //! "how much room has the name" question it never asks — it paints from the
    //! left and lets the clip rect end the name, which is exactly why a panel
    //! dragged too narrow is silent on screen.

    use super::*;

    /// Room for a hovered row's name at panel width `w`, in points.
    fn hovered_name_room(w: f32, depth: usize) -> f32 {
        let content = w - 2.0 * TREE_PAD_X as f32;
        content - icon_offset(depth, true) - NAME_GAP - (LOCK_INSET + EYE_PITCH + ICON_PT / 2.0)
    }

    /// **A top-level row keeps a real name at the narrowest the drag allows.**
    ///
    /// Measured against names laid out at the row's own 12.5pt rather than against
    /// a character count: `Rectangle 12` is 75.06pt and `Button primary hover`
    /// 124.34, so at `MIN_W` the common case fits with room over and the long case
    /// is the one that clips. That is the trade the minimum makes, stated as the
    /// two numbers either side of it.
    ///
    /// ⚠️ Flipped to 180, the rounder wrong answer: `Rectangle 12` no longer fits
    /// and this fails on the **first** assertion, at 66.50pt against 75.06 —
    /// predicted there and observed there. Flipped the other way to 300 the
    /// *second* assertion fails, and that is the half worth having: a minimum
    /// which is merely generous stops the user doing something reasonable, and
    /// nothing on screen would ever say so.
    ///
    /// ⚠️ **What the 300 flip also did was take the other two tests with it**, which
    /// was not predicted and is the more useful finding: the band test's *lower*
    /// bound failed (83.50pt of room ten points under a 300 minimum), and so did
    /// the ordering invariant, because a minimum equal to the default leaves the
    /// panel opening at a width it cannot be dragged back to. Three assertions,
    /// three different reasons — so the ordering one was never the formality it
    /// looked like.
    ///
    /// ⚠️ **That flip can no longer be run as described, and the reason is worth
    /// more than the flip was.** The ordering invariant has since become a
    /// `const _: () = assert!(…)` beside the constants, so `MIN_W = 300` now fails
    /// the **build** and none of these three tests execute at all. The invariant
    /// went from "one of three tests notices" to "the compiler refuses", which is
    /// strictly better — but it means the past tense above is the only honest
    /// tense, and a reader reproducing it will get a compile error rather than the
    /// three failures it describes.
    #[test]
    fn the_minimum_keeps_a_real_layer_name_on_a_top_level_row() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        // One pass so the fonts exist to lay names out with.
        let _ = ctx.run_ui(Default::default(), |_| {});
        let lays_out = |s: &str| {
            ctx.fonts_mut(|f| {
                f.layout_no_wrap(
                    s.to_string(),
                    egui::FontId::proportional(12.5),
                    egui::Color32::WHITE,
                )
                .size()
                .x
            })
        };

        let room = hovered_name_room(MIN_W, 1);
        let common = lays_out("Rectangle 12");
        assert!(
            common <= room,
            "at the {MIN_W}pt minimum a top-level row has {room:.2}pt of name and \
             the default name of a new shape lays out at {common:.2} — the panel \
             can be dragged to a width where nothing is readable"
        );
        // And not so generous that the minimum is doing nothing: a long name is
        // still expected to clip, or `MIN_W` has quietly become a comfortable
        // width rather than a floor.
        let long = lays_out("Button primary hover");
        assert!(
            long > room,
            "at {MIN_W}pt a {long:.2}pt name fits in {room:.2}pt, so the minimum is \
             not a floor — it is wider than the narrowest usable panel and is \
             refusing widths the user should be allowed"
        );
    }

    /// **The minimum is also where a four-deep row stops drawing its name
    /// underneath the eye.**
    ///
    /// The second, independent reading of the same number, and the reason it is
    /// 210 rather than a round 200. Four levels is not an arbitrary depth to test
    /// at: it is the depth §15 D330 argued the 300 from, so this reuses that
    /// decision's own reference rather than inventing one.
    ///
    /// ⚠️ **The crossing is at 194.5, not 206.5, and this reading no longer picks
    /// 210 — which is a question for the maintainer rather than a bug.** Both
    /// numbers came out of a derivation that subtracted the *header*'s `2 * PAD_X`
    /// (24) where a row is narrowed by the tree's own `TREE_PAD_X` (12); see
    /// that constant. Corrected, a four-deep row stops drawing under the eye at
    /// **194.5**, so on this reading alone 195 would have done and `MIN_W` carries
    /// 15.5pt of slack rather than 3.5. `MIN_W`'s prose says *"210 is where two
    /// independent readings of it meet"*; after the correction **they do not
    /// meet** — the top-level-name reading is now the only one choosing 210, and
    /// whether that is still the number wanted is a design call. Nothing on screen
    /// changed: the error was wholly in the safe direction.
    ///
    /// So the band is anchored on the **crossing** rather than on `MIN_W − 10`,
    /// which was only ever a stand-in for it and is now on the wrong side.
    /// Asserting `hovered_name_room(MIN_W, 4) == 15.5` would pin the slack rather
    /// than the property, and would fail on any retune that kept the picture
    /// correct. Flipped to 180 the first assertion fails at −14.50pt, a name a
    /// glyph under the eye.
    #[test]
    fn a_four_deep_row_is_not_drawn_under_the_eye_at_the_minimum() {
        let at_min = hovered_name_room(MIN_W, 4);
        assert!(
            at_min >= 0.0,
            "four deep at the {MIN_W}pt minimum, the name has {at_min:.2}pt — it is \
             drawn underneath the eye, which is a broken row rather than a tight one"
        );
        // Just under the measured crossing, so the formula is shown to have a real
        // zero and this is a band rather than a one-sided bound.
        let below = hovered_name_room(194.0, 4);
        assert!(
            below < 0.0,
            "half a point under the 194.5 crossing a four-deep name still has {below:.2}pt, \
             so {MIN_W} is not where the overlap starts and the number has drifted \
             from what its doc comment says it was derived from"
        );
    }

    /// **The maximum clears the deepest tree anyone would keep open.**
    ///
    /// `MAX_W` exists to stop the panel eating the canvas, not to ration room, so
    /// what has to hold is that it binds only on someone starving the canvas — at
    /// eight levels there is still a full name's worth of space.
    ///
    /// ⚠️ **The other half of this — that `DEFAULT_W` sits between the two — was
    /// here and is not any more.** All three are compile-time constants, so it is
    /// a `const _: () = assert!(…)` beside them now: the compiler refuses the
    /// build rather than a test refusing a run, and the check sits where the next
    /// person retuning one of the three is already looking. Clippy named it
    /// (`assertions_on_constants`) and was right.
    #[test]
    fn the_maximum_still_holds_a_deep_tree() {
        let deep = hovered_name_room(MAX_W, 8);
        assert!(
            deep > 200.0,
            "eight deep at the {MAX_W}pt maximum leaves {deep:.2}pt of name — the \
             cap is rationing room rather than protecting the canvas"
        );
    }
}

#[cfg(test)]
mod autoscroll_ownership_tests {
    //! **The tree has one drag-to-edge autoscroller, and it is not in this file**
    //! (§15 D576, `[S15.1-L1-01]`).
    //!
    //! `layers_tree` had its own, running every frame beside `app::layers_panel`'s
    //! on the identical y range. `scroll_with_delta_animation` accumulates, so they
    //! added — 960 pt/s at 60 Hz against a documented ceiling of 420, 1500 pt/s at
    //! 120 Hz because only one was `dt`-scaled, and the later of the two put back
    //! the easing the earlier had passed `ScrollAnimation::none()` to remove.
    //!
    //! Plain backticks throughout, per §15 D319 — this is a `#[cfg(test)]` module
    //! and `cargo doc` cannot see it, so a `[link]` here would be checked by nothing.

    use super::*;
    use crate::app::OndinApp;
    use ondin_core::kurbo::Size;

    const VIEW: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::Pos2::new(300.0, 120.0),
    };

    /// A headless app holding `n` rects at the root — enough rows that the tree
    /// overflows the 120pt viewport and there is somewhere to scroll to.
    fn app_with_rows(ctx: &egui::Context, n: usize) -> OndinApp {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(31);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let mut ops = Vec::new();
        let mut made = Vec::new();
        for i in 0..n {
            let id = ids.mint();
            made.push(id);
            ops.push(Operation::CreateNode {
                id,
                parent: root,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            });
        }
        doc.apply(&Transaction(ops)).expect("the rows");
        app.session.adopt_document(doc, None);
        // A live drag on the first row, which is the whole precondition both
        // autoscrollers were gated on.
        app.layer_drag = Some(LayerDrag {
            ids: vec![made[0]],
            target: None,
            grab: (20.0, 0),
            level: 0,
            lit: None,
            spring: None,
        });
        app
    }

    /// One frame of `layers_tree` inside a scroll area, with the pointer parked at
    /// `at`. Returns the scroll offset the area reports **after** the frame.
    fn frame(ctx: &egui::Context, app: &mut OndinApp, at: egui::Pos2) -> f32 {
        let mut offset = 0.0;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(VIEW),
                events: vec![egui::Event::PointerMoved(at)],
                ..Default::default()
            },
            |ui| {
                let out = egui::ScrollArea::vertical()
                    .id_salt("autoscroll-probe")
                    .show(ui, |ui| app.layers_tree(ui));
                offset = out.state.offset.y;
            },
        );
        offset
    }

    /// **`layers_tree` scrolls nothing, however hard the pointer is pushed into the
    /// edge.**
    ///
    /// The deletion, stated as behaviour. The pointer sits **above** the viewport's
    /// top — past the far end of the band, where the deleted ramp was at its
    /// `MAX_STEP` of 9pt per frame — and eight frames go by. The area must not have
    /// moved at all.
    ///
    /// ⚠️ **The fixture assertion is what stops this being vacuous**, and it is the
    /// one that had to be got right: a tree that fits its viewport cannot scroll for
    /// reasons that have nothing to do with the fix, so the rows are asserted to
    /// overflow before anything is claimed about the offset. Without it a build that
    /// autoscrolls perfectly well passes.
    ///
    /// **Flip run**, `autoscroll_while_dragging` and its call restored: fails on
    /// *"the tree scrolls itself on pass 1"* at `0.6666667`. ⚠️ **The offset is read
    /// *after* the frame and it lags by one**: `scroll_with_delta_animation` writes a
    /// delta the area consumes on its next pass, so a single-frame version of this
    /// test is green under the bug. That is why there are eight — and why the failure
    /// is on pass 1 rather than pass 0.
    #[test]
    fn the_tree_does_not_scroll_itself_when_a_drag_reaches_its_edge() {
        let ctx = egui::Context::default();
        let mut app = app_with_rows(&ctx, 40);

        // The fixture: the content overflows, so there is somewhere to scroll to.
        let mut content = 0.0;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(VIEW),
                ..Default::default()
            },
            |ui| {
                let out = egui::ScrollArea::vertical()
                    .id_salt("autoscroll-probe")
                    .show(ui, |ui| app.layers_tree(ui));
                content = out.content_size.y;
            },
        );
        assert!(
            content > VIEW.height() * 2.0,
            "the fixture: 40 rows must overflow a 120pt viewport, got {content}"
        );

        // ⚠️ **The *bottom* edge, and the top one is a fixture that cannot reach the
        // state.** The first version of this test parked the pointer above the top
        // edge, where the autoscroller scrolls the content *down* to reveal what is
        // above it — and the area starts at offset 0, so there is nothing above and
        // egui clamps the delta to nothing. The flip stayed green with the deleted
        // function fully restored and running, which is the "its fixture never
        // reached the state" vacuity exactly. Confirmed by probe: the pointer was
        // seen at `(150, -4)` against a clip rect of `[0,0]-[300,120]` and the step
        // computed to its full 9pt; the offset simply could not move.
        let at = egui::pos2(150.0, VIEW.bottom() + 4.0);
        for pass in 0..8 {
            let offset = frame(&ctx, &mut app, at);
            assert_eq!(
                offset, 0.0,
                "the tree scrolls itself on pass {pass}: offset {offset}"
            );
        }
    }
}

#[cfg(test)]
mod drag_start_sync_tests {
    //! **Three doors change the selection from inside the row walk; all three now
    //! tell the panel** (§15 D581, `[S15.1-L1-02]`), and **the drag ends on the
    //! primary button** (§15 D581, `[S15.1-L1-04]`).
    //!
    //! The click and right-click arms both closed with
    //! `layers_synced_selection = …`, under a comment saying why; the
    //! `drag_started` arm's `set_one` did not. So `sync_tree_to_selection` read the
    //! new selection as news on the following frame,
    //! `scroll_layers_to_selection` fired, and `layers_row` recentred the row —
    //! moving the whole tree, and every drop boundary in it, under a held pointer.
    //! Invisible in the middle of a list, because the recentre is gated on
    //! `!clip_rect.contains_rect(rect)`: it needs a row that is only *partly* in
    //! the viewport, which is the ordinary case at either edge.
    //!
    //! Plain backticks throughout, per §15 D319 — this is a `#[cfg(test)]` module
    //! and `cargo doc` cannot see it, so a `[link]` here would be checked by
    //! nothing.

    use super::*;
    use crate::app::OndinApp;
    use ondin_core::kurbo::Size;

    const VIEW: egui::Rect = egui::Rect {
        min: egui::Pos2::ZERO,
        max: egui::Pos2::new(300.0, 120.0),
    };

    /// A headless app holding `n` rects at the root, and their ids top to bottom.
    fn app_with_rows(ctx: &egui::Context, n: usize) -> (OndinApp, Vec<NodeId>) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(37);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let mut ops = Vec::new();
        let mut made = Vec::new();
        for i in 0..n {
            let id = ids.mint();
            made.push(id);
            ops.push(Operation::CreateNode {
                id,
                parent: root,
                index: i,
                kind: NodeKind::Rect {
                    size: Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            });
        }
        doc.apply(&Transaction(ops)).expect("the rows");
        app.session.adopt_document(doc, None);
        // The tree draws children in list order and the root is not a row, so the
        // deepest child is the *last* row. `made` is in that order already.
        (app, made)
    }

    /// One frame of `layers_tree` inside a scroll area. Returns the offset the
    /// area reports after the frame and the shapes it produced.
    fn frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        events: Vec<egui::Event>,
    ) -> (f32, Vec<egui::epaint::ClippedShape>) {
        let mut offset = 0.0;
        let full = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(VIEW),
                events,
                ..Default::default()
            },
            |ui| {
                let out = egui::ScrollArea::vertical()
                    .id_salt("drag-sync-probe")
                    .show(ui, |ui| app.layers_tree(ui));
                offset = out.state.offset.y;
            },
        );
        (offset, full.shapes)
    }

    /// A primary press or release at `p`.
    fn press(p: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos: p,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// The row boxes of a fixture whose every row is selected, top to bottom.
    ///
    /// ⚠️ **This is a measurement and it leaves state behind.** Selecting to make
    /// the grounds paint is `row_rects`' technique two modules up; what is extra
    /// here is that the caller has to put `layers_synced_selection` back, or the
    /// very bookkeeping under test starts the real run in the wrong state.
    fn lit_rows(ctx: &egui::Context, app: &mut OndinApp, ids: &[NodeId]) -> Vec<egui::Rect> {
        app.session.selection.set(ids.to_vec());
        app.layers_synced_selection = ids.to_vec();
        let (_, shapes) = frame(ctx, app, Vec::new());
        app.session.selection.clear();
        app.layers_synced_selection = Vec::new();
        let mut rects: Vec<egui::Rect> = shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                egui::Shape::Rect(r) if r.fill == color::SELECT_ROW => Some(r.rect),
                _ => None,
            })
            .collect();
        rects.sort_by(|a, b| a.top().total_cmp(&b.top()));
        rects
    }

    /// The root's child list, which is what a drop rearranges.
    fn children_of(app: &OndinApp) -> Vec<NodeId> {
        app.session
            .doc
            .get(app.session.doc.root())
            .map(|n| n.children().to_vec())
            .unwrap_or_default()
    }

    /// The row that straddles the viewport's bottom edge, with a point inside its
    /// visible half to aim at — the whole precondition, since the recentre is
    /// gated on the row not being wholly inside the clip rect.
    fn clipped_row(rects: &[egui::Rect]) -> egui::Pos2 {
        let r = rects
            .iter()
            .find(|r| r.top() < VIEW.bottom() && r.bottom() > VIEW.bottom())
            .expect("the fixture: some row must straddle the bottom edge");
        egui::pos2(150.0, (r.top() + VIEW.bottom()) * 0.5)
    }

    /// **Picking up a partly-clipped row does not move the tree, and the click on
    /// the identical row is the control.**
    ///
    /// Both arms end with the same one-row selection, so the selection is not what
    /// the assertion is about — the bookkeeping is.
    ///
    /// ⚠️ **The fixture assertion is `clipped_row`'s `expect`, and it is what
    /// stops this being vacuous.** A row wholly inside the viewport is never
    /// recentred at all, so a fixture whose rows all fit passes with the fix
    /// removed. Session 11 shipped three flip-checks that failed exactly this way;
    /// the row is asserted to straddle the edge before anything is claimed.
    ///
    /// **Flip run**, the new `layers_synced_selection` line in the `drag_started`
    /// arm removed: fails at `3.6296296` on the **fourth** offset, and the click
    /// control stays at `0.0` — the pair that says the assertion is measuring
    /// this mechanism and not the fixture.
    ///
    /// ⚠️ **The number is not the 20.7 the finding measured, and the difference
    /// is the assertion's whole shape.** `scroll_to_rect` is animated, so the
    /// 20.7 arrives over several frames and the fourth reads the first slice of
    /// it. A test written to assert the *magnitude* would have to know how many
    /// frames egui's easing takes; asserting that it is **zero on every frame**
    /// needs no such number and is what the fix actually promises.
    #[test]
    fn picking_up_a_partly_clipped_row_does_not_scroll_the_tree() {
        let ctx = egui::Context::default();
        let (mut app, ids) = app_with_rows(&ctx, 6);
        let rects = lit_rows(&ctx, &mut app, &ids);
        assert_eq!(rects.len(), ids.len(), "the fixture: one ground per row");
        let at = clipped_row(&rects);

        // The press, then a move past egui's drag threshold — `drag_started` is
        // reported on the move, not on the press.
        let (o0, _) = frame(&ctx, &mut app, vec![press(at, true)]);
        let (o1, _) = frame(
            &ctx,
            &mut app,
            vec![egui::Event::PointerMoved(at + egui::vec2(0.0, -20.0))],
        );
        assert!(
            app.layer_drag.is_some(),
            "the fixture: the press and move must actually start a drag"
        );
        let picked_up = app.session.selection.ids().to_vec();
        assert_eq!(
            picked_up.len(),
            1,
            "the fixture: the drag arm selected the row it picked up"
        );
        // Two more frames: `sync_tree_to_selection` runs at the top of the *next*
        // `layers_tree` and `scroll_to_rect`'s offset is consumed by the pass
        // after that, so a single-frame version of this is green under the bug.
        let (o2, _) = frame(&ctx, &mut app, Vec::new());
        let (o3, _) = frame(&ctx, &mut app, Vec::new());
        assert_eq!(
            (o0, o1, o2, o3),
            (0.0, 0.0, 0.0, 0.0),
            "the drag scrolled the tree"
        );

        // The control: the identical row, clicked rather than dragged, through the
        // arm that has carried the line all along.
        let (mut app, ids) = app_with_rows(&ctx, 6);
        let rects = lit_rows(&ctx, &mut app, &ids);
        let at = clipped_row(&rects);
        let (c0, _) = frame(&ctx, &mut app, vec![press(at, true)]);
        let (c1, _) = frame(&ctx, &mut app, vec![press(at, false)]);
        assert_eq!(
            app.session.selection.ids(),
            picked_up.as_slice(),
            "the control: the click must end with the identical selection, or the \
             selection is the difference between the two arms rather than the \
             bookkeeping"
        );
        let (c2, _) = frame(&ctx, &mut app, Vec::new());
        assert_eq!(
            (c0, c1, c2),
            (0.0, 0.0, 0.0),
            "the control moved, so the fixture is what is being measured"
        );
    }

    /// 🚨 **Releasing the primary over the tree lands the drop** — §15 D582, the
    /// regression D471 opened and nothing could see.
    ///
    /// `layers_tree` clears `LayerDrag::target` at the top of every frame and
    /// `drop_hint` is the only thing that re-derives it; D471 gave `drop_hint` a
    /// bare `return` on `!primary_down()`. egui clears `down[Primary]` on the
    /// frame it processes the release, so **the one frame that ends a drag was
    /// the one frame that could not resolve one.** The release landed nothing,
    /// silently — `finish_layer_drag` took the drag, found `target: None` two
    /// lines in, and returned.
    ///
    /// ⚠️ **Nothing in the suite was measuring this**, which is why a day-old
    /// regression in the panel's whole point survived a review finding written
    /// against the very function. `[S15.1-L1-04]`'s own probe drove a **middle**
    /// button release — which leaves the primary *down*, so `drop_hint` runs and
    /// the target is there — and the eight `drop_indent_tests` set `layer_drag`
    /// by hand and never release at all. **No test drove a press, a move and a
    /// release end to end**, and that is the entire hole.
    ///
    /// **Flip run**, `!down && !released` narrowed back to `!down`: fails here on
    /// *"the release landed nothing"* at `undo_depth 0`, and takes
    /// `a_middle_button_release_does_not_land_a_layer_drag`'s **control** with it
    /// — which is the control doing its job. `a_drag_with_no_button_behind_it_
    /// paints_nothing` stays green under the same flip, so the two behaviours are
    /// independent rather than one predicate traded for the other.
    #[test]
    fn releasing_the_primary_over_the_tree_lands_the_drop() {
        let ctx = egui::Context::default();
        let (mut app, ids) = app_with_rows(&ctx, 6);
        let rects = lit_rows(&ctx, &mut app, &ids);
        let before = children_of(&app);
        let (from, to) = (rects[2].center(), rects[0].center());

        let _ = frame(&ctx, &mut app, vec![press(from, true)]);
        for _ in 0..5 {
            let _ = frame(&ctx, &mut app, vec![egui::Event::PointerMoved(to)]);
        }
        // The fixture, in its own terms: a drag is in flight and it has resolved
        // somewhere to go. Without this the release is being asked to land
        // nothing, and a build that never resolves a target passes.
        assert!(
            app.layer_drag.as_ref().is_some_and(|d| d.target.is_some()),
            "the fixture: the drag must be in flight with a target"
        );

        let _ = frame(&ctx, &mut app, vec![press(to, false)]);
        assert!(app.layer_drag.is_none(), "the release ended the drag");
        assert_eq!(
            app.session.history.undo_depth(),
            1,
            "the release landed nothing"
        );
        assert_ne!(
            children_of(&app),
            before,
            "the release landed nothing: the child list is unchanged"
        );
        assert_eq!(app.session.status().text, "Reordered");
    }

    /// **A middle-button release does not land a layer drag.**
    ///
    /// `layers_tree`'s `any_released()` was the workspace's only one; the canvas
    /// asks `primary_released()` at all three of its sites. Clicking the scroll
    /// wheel part-way through a row drag therefore dropped the rows wherever the
    /// pointer happened to be, and left the still-held primary controlling
    /// nothing.
    ///
    /// ⚠️ **The fixture assertion is the resolved target.** A drag with
    /// `target: None` returns from `finish_layer_drag` two lines in whatever the
    /// predicate says, so a fixture that never resolves one passes under the bug.
    /// The target is asserted `is_some()` before the middle button is pressed.
    ///
    /// **Flip run**, `primary_released()` put back to `any_released()`: fails on
    /// *"the middle button landed the drag"* — `layer_drag` is `None` and
    /// `undo_depth` is 1. The primary release below is the control and it lands
    /// the drag in both versions.
    #[test]
    fn a_middle_button_release_does_not_land_a_layer_drag() {
        let ctx = egui::Context::default();
        let (mut app, ids) = app_with_rows(&ctx, 6);
        let rects = lit_rows(&ctx, &mut app, &ids);
        // The third row up to the first — both wholly inside the viewport, and a
        // real move rather than a drop where it started, which `plan_layer_move`
        // would plan nothing for and the undo depth could not tell apart.
        let from = rects[2].center();
        let to = rects[0].center();

        let _ = frame(&ctx, &mut app, vec![press(from, true)]);
        // ⚠️ **Five passes**, for `drop_indent_tests::pump`'s reason: a widget's
        // interaction state is last frame's, and `drop_hint` reads the target off
        // the hover. The button goes down once and egui carries `primary_down`.
        for _ in 0..5 {
            let _ = frame(&ctx, &mut app, vec![egui::Event::PointerMoved(to)]);
        }
        assert!(
            app.layer_drag.as_ref().is_some_and(|d| d.target.is_some()),
            "the fixture: the drag must have resolved a target, or the release \
             predicate is not what decides anything"
        );
        let depth = app.session.history.undo_depth();
        let _ = frame(
            &ctx,
            &mut app,
            vec![egui::Event::PointerButton {
                pos: to,
                button: egui::PointerButton::Middle,
                pressed: false,
                modifiers: Default::default(),
            }],
        );
        assert!(
            app.layer_drag.is_some(),
            "the middle button landed the drag"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            depth,
            "the middle button committed a move"
        );

        // The control: the primary release ends it, in this build and in the one
        // before the fix.
        let _ = frame(&ctx, &mut app, vec![press(to, false)]);
        assert!(
            app.layer_drag.is_none(),
            "the control: the primary release must still land the drag"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            depth + 1,
            "the control: the primary release must still commit the move"
        );
    }
}

#[cfg(test)]
mod collapse_all_tests {
    //! The collapse-all button's glyph and its action ask one question (§15 D674).
    //!
    //! Plain backticks throughout, per §15 D319 — this is a `#[cfg(test)]` module
    //! and `cargo doc` cannot see it, so a `[link]` here would be checked by
    //! nothing.

    use super::*;
    use crate::app::OndinApp;
    use ondin_core::kurbo::Size;

    /// A headless app over a tree with one container of each interesting shape:
    /// an `Artboard` holding a rect, a `Boolean` holding two, and a **childless**
    /// `Group`, which is the node the two predicates could most easily disagree
    /// about.
    fn app_with_containers(ctx: &egui::Context) -> (OndinApp, Vec<NodeId>) {
        theme::install(ctx);
        let mut app = OndinApp::headless(ctx);
        let mut ids = ondin_core::IdSource::new(41);
        let root = ids.mint();
        let mut doc = ondin_core::Document::new(root);
        let rect = |w: f64| NodeKind::Rect {
            size: Size::new(w, w),
            corner_radii: Default::default(),
        };
        let (board, inside, boolean, a, b, empty) = (
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
        );
        let mut ops = vec![
            Operation::CreateNode {
                id: board,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(100.0, 100.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: inside,
                parent: board,
                index: 0,
                kind: rect(10.0),
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: boolean,
                parent: board,
                index: 1,
                kind: NodeKind::Boolean {
                    op: ondin_core::node::BoolOp::Union,
                },
                transform: None,
                name: None,
            },
        ];
        for (i, id) in [a, b].into_iter().enumerate() {
            ops.push(Operation::CreateNode {
                id,
                parent: boolean,
                index: i,
                kind: rect(20.0),
                transform: None,
                name: None,
            });
        }
        ops.push(Operation::CreateNode {
            id: empty,
            parent: root,
            index: 1,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        });
        doc.apply(&Transaction(ops)).expect("the tree");
        app.session.adopt_document(doc, None);
        (app, vec![board, boolean, empty])
    }

    /// **The arrow and the press agree in every state of the tree**
    /// (`[S15.1-L3-02]`).
    ///
    /// `tree_is_all_collapsed` is the button's glyph and `toggle_collapse_all` is
    /// its action, and they used to reach *"what counts as a container"* by two
    /// independent walks. This drives the button rather than comparing the two
    /// predicates: press it until it stops changing anything, and assert the glyph
    /// says the opposite each time.
    ///
    /// ⚠️ **The childless `Group` is the fixture's point.** It is a container by
    /// the tree's eye and not by either predicate — `!n.children().is_empty()` is
    /// false — so a "fix" that folded it would be caught here and nowhere else.
    ///
    /// **Flip:** put back the inlined walk with the `id != root` clause dropped —
    /// the plausible slip, since the root is not a row — and the **second**
    /// assertion fails: *"after folding, the button must offer to unfold:
    /// collapsed = {29:4, 29:2}"*. The glyph reads *"not all folded"* on a tree the
    /// press has just folded, because the root is a container nothing can collapse.
    /// Site predicted correctly.
    #[test]
    fn the_glyph_and_the_press_read_one_definition_of_container() {
        let ctx = egui::Context::default();
        let (mut app, containers) = app_with_containers(&ctx);

        assert!(
            !app.tree_is_all_collapsed(),
            "a fresh tree is open, so the button must offer to fold"
        );
        app.toggle_collapse_all();
        assert!(
            app.tree_is_all_collapsed(),
            "after folding, the button must offer to unfold: collapsed = {:?}",
            app.collapsed
        );
        // The set the press wrote is exactly the containers with children — the
        // empty `Group` is not among them, and that is the same answer the glyph
        // gives.
        let mut folded: Vec<NodeId> = app.collapsed.iter().copied().collect();
        folded.sort_by_key(|id| format!("{id:?}"));
        let mut want: Vec<NodeId> = containers[..2].to_vec();
        want.sort_by_key(|id| format!("{id:?}"));
        assert_eq!(folded, want, "the childless Group is not a container");

        app.toggle_collapse_all();
        assert!(
            !app.tree_is_all_collapsed(),
            "and back, on the same definition"
        );
    }
}

/// §15 D784 — a faded picture's row thumbnail is faded, and the row's own dimming
/// is a second multiplier rather than a competing one.
#[cfg(test)]
mod thumb_tint_tests {
    use super::*;

    /// An image brush at `alpha`. `ImageId` does not have to name anything a store
    /// can draw: this is the *tint* rule, and the tint is computed before anybody
    /// asks the thumbnail cache for a texture.
    fn faded(alpha: f32) -> ondin_core::ImageBrush {
        let ondin_core::Brush::Image(mut img) =
            ondin_core::image_brush(ondin_core::ImageId("a".repeat(64)))
        else {
            unreachable!("image_brush builds an image brush")
        };
        img.sampler.alpha = alpha;
        img
    }

    /// The reported symptom, at the surface D773 did not name.
    ///
    /// **The loss comes first**: a 30% fill drew at *full* opacity here, so the
    /// assertion that has to bite is "not opaque", and the exact value is checked
    /// after it. Asserting only the value would pass for a tint that had gone
    /// wrong in some other direction and read as a fade.
    #[test]
    fn a_faded_picture_fades_its_row_thumbnail() {
        let faint = thumb_tint(false, Some(&faded(0.3)));
        assert_ne!(
            faint,
            egui::Color32::WHITE,
            "a 30% picture drew an opaque thumbnail — the defect this test is for"
        );
        assert_eq!(faint, egui::Color32::WHITE.gamma_multiply(0.3));
        assert_eq!(
            thumb_tint(false, Some(&faded(1.0))),
            egui::Color32::WHITE,
            "and an opaque picture is still the identity multiplier, not a fade"
        );
        assert_eq!(
            thumb_tint(false, None),
            egui::Color32::WHITE,
            "a row with no picture at all is untinted"
        );
    }

    /// The two multipliers compose. **This is the assertion the obvious
    /// implementation fails**: a `match` that answered the dimming *or* the alpha
    /// is green on both tests above and wrong only where both are true.
    ///
    /// **Flipped, both ways, and the predicted sites held.** Ignoring the picture
    /// (the state before §15 D784) fails the test above at its *first* assertion —
    /// `left != right`, `#FF_FF_FF_FF` against itself, which is the reported
    /// symptom in the failure message — and fails this one at `102 vs 102`.
    /// Folding the alpha into the **undimmed arm only** is green everywhere except
    /// here, at the same `102 vs 102`: one flip, one assertion, which is what says
    /// this test is carrying the composition rule on its own.
    #[test]
    fn dimming_and_a_faded_picture_compose_rather_than_one_winning() {
        let dim_only = thumb_tint(true, None);
        let faint_only = thumb_tint(false, Some(&faded(0.3)));
        let both = thumb_tint(true, Some(&faded(0.3)));
        assert!(
            both.a() < dim_only.a(),
            "a hidden layer's 30% picture must be fainter than dimming alone \
             ({} vs {})",
            both.a(),
            dim_only.a()
        );
        assert!(
            both.a() < faint_only.a(),
            "…and fainter than the fade alone ({} vs {})",
            both.a(),
            faint_only.a()
        );
    }

    /// `alpha` is a float the loader does not range-check — `build::brush_is_finite`
    /// asks only that it is finite — so a hand-edited `.ondin` reaches this with
    /// anything finite in it, and `gamma_multiply` outside `0..=1` is not a fade.
    ///
    /// ⚠️ **This one is vacuous without the fold and the flip says so.** Against
    /// the pre-fix `thumb_tint` (picture ignored) it is **green**, because both
    /// sides of each assertion collapse to `WHITE` — it has teeth only once
    /// something multiplies. Recorded rather than left to be rediscovered: it is a
    /// test about the clamp, and the clamp is downstream of the decision the two
    /// tests above pin.
    #[test]
    fn an_out_of_range_alpha_is_clamped_at_both_ends() {
        assert_eq!(
            thumb_tint(false, Some(&faded(-3.0))),
            thumb_tint(false, Some(&faded(0.0))),
            "a negative alpha reads as fully transparent, not as a wrap"
        );
        assert_eq!(
            thumb_tint(false, Some(&faded(1e30))),
            egui::Color32::WHITE,
            "and an enormous one is opaque rather than an overflowed tint"
        );
    }
}
