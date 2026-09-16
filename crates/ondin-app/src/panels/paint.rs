//! The paint model as the inspector and the colour picker see it (§9.2).
//!
//! Mostly pure `Brush` in, `Brush` out, and **no document**. Both the inspector's
//! fill/stroke lists and the detached picker edit the same brushes, and the rules
//! about *how* an edit rewrites a brush that live here are what stop those two
//! drifting: switching kind preserves the stops, a linear gradient's angle is
//! stored as the two endpoints the model actually holds, and a picture's alpha is
//! read in one place ([`image_alpha`]).
//!
//! 🚨 **This head used to promise more than the module delivers, and the promise
//! was load-bearing** (§15 D787, `[S23.2-L3-04]`). It read *"no egui"* and *"**every**
//! rule about how an edit rewrites a brush lives here so the two cannot drift"*,
//! and `architecture.md` §9.2 says the same thing in the document's own voice —
//! *"`paint.rs` owns every rule about how an edit rewrites a brush"*, *"it is the
//! reason the inspector's paint rows and the picker cannot disagree about what an
//! edit means"*. Both halves were false:
//!
//! - **egui.** [`CharWrite`] is `Valve(&egui::Response)`, and [`PaintDrag`] and
//!   [`Slots`] carry `f32` screen spans from last frame's layout.
//! - **Every rule.** The **hex edit** is the counter-example and `[S23.1-L1-05]`
//!   enumerated it: of the four ways its two copies differed, **three are rules
//!   about how an edit rewrites a brush** — what alpha goes out with a typed
//!   colour over a set that disagrees, whether an unchanged colour is written at
//!   all, and whether an unrepresentable colour is quantised before it is stored —
//!   and this module holds **none** of them. What it offers that path is
//!   [`with_stops`], which takes the alpha decision as an *argument*.
//!
//! ⚠️ **The lift is not done and this head no longer claims it is.** The code half
//! is `[S23.1-L1-05]`'s — delete `picker::hex_row`'s field, call
//! `inspector::paint_hex_field`, and lift the three rules here as one function —
//! and it moves a shipped control, so it is its own change. **A promise that is
//! load-bearing while untrue is worse than no promise**: a reviewer who reads
//! *"the two cannot drift"* concludes the drift is impossible and stops looking,
//! which is exactly what `[S23.1-L1-02]` then measured them doing.
//!
//! Gradient coordinates are in the node's **local geometry box**, which is why
//! every constructor takes `box_size`: a gradient built at the origin would sit
//! off the shape entirely.

use ondin_core::Brush;
use ondin_core::NodeId;
use ondin_core::kurbo::{Point, Size, Vec2};
use ondin_core::peniko::{Color, Gradient, GradientKind};

/// Which of a node's two paint lists a row belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaintList {
    Fill,
    Stroke,
}

impl PaintList {
    pub fn target(self) -> ondin_core::PaintTarget {
        match self {
            PaintList::Fill => ondin_core::PaintTarget::Fill,
            PaintList::Stroke => ondin_core::PaintTarget::Stroke,
        }
    }
}

/// A paint row being dragged up or down its list.
///
/// Which paint applies first is a real question — two translucent fills stack
/// differently either way round, and so do an inside and an outside stroke — so
/// the list order has to be editable, and a grip on the row is how every other
/// list in the app says so.
///
/// ⚠️ **`PartialEq` is here for `app::InFlight::any`** (§15 D785), which asks
/// *"is any gesture holding a value"* by comparing the whole struct against
/// `Default` — the property that makes it exhaustive by construction. Nothing
/// compares two live `PaintDrag`s, and nothing should: `slots` is last frame's
/// layout in `f32`, so equality on it is a question about pixels rather than about
/// the drag.
#[derive(PartialEq)]
pub struct PaintDrag {
    /// The node the panel is anchored on. A selection change mid-drag would leave
    /// the indices below meaning nothing, so the drag is dropped when this stops
    /// matching what the panel is showing.
    pub anchor: NodeId,
    pub list: PaintList,
    /// The index in the model's `Vec` of the paint being carried.
    pub from: usize,
    /// Where it will land, as a model index — and, because the reorder is live,
    /// **also where it is currently being shown**. The rows change places under the
    /// pointer, so this is not a promise about the future but a description of what
    /// is on screen.
    pub to: usize,
    /// The vertical span of each display slot, top-first, as the panel last laid
    /// them out.
    ///
    /// Carried across frames because the target has to be known *before* the rows
    /// can be laid out in it, so the geometry has to come from the frame before.
    /// That is exact rather than approximate: reordering the contents of the slots
    /// does not move the slots.
    pub slots: Slots,
}

/// Where each display slot of a list landed, top-first.
///
/// Slot-based rather than keyed by the item in it, which is the whole trick behind
/// a live reorder: the *contents* move while the slots stay exactly where they
/// are, so this geometry is stable for the whole gesture and a drop can be
/// resolved against it without the answer feeding back into the layout that
/// produced it. Actual spans rather than a pitch, so a list whose entries are not
/// all the same height — a stroke is two rows where a fill is one — works without
/// a special case.
// `PartialEq` for [`PaintDrag`]'s, which is for `app::InFlight::any` — see that
// struct's note. Nothing compares two live `Slots`.
#[derive(Default, Clone, PartialEq)]
pub struct Slots(Vec<(f32, f32)>);

impl Slots {
    pub fn push(&mut self, top: f32, bottom: f32) {
        self.0.push((top, bottom));
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The gap a pointer at `y` falls into, counted from the top: `0` above every
    /// slot, `len` below all of them.
    ///
    /// A slot is claimed once the pointer passes its middle, which is what makes
    /// two entries trade places exactly where they would have to.
    pub fn gap(&self, y: f32) -> usize {
        self.0
            .iter()
            .take_while(|(top, bottom)| y >= (top + bottom) * 0.5)
            .count()
    }
}

/// The model indices of a list in the order its rows should be drawn, top first —
/// with a carried entry moved to where it would land.
///
/// **Top of the stack first.** The renderer walks a paint list forward, so the last
/// element is the last painted and belongs on the top row.
pub fn display_order(n: usize, moved: Option<(usize, usize)>) -> Vec<usize> {
    let base: Vec<usize> = (0..n).rev().collect();
    match moved {
        Some((from, to)) if from != to && from < n && to < n => {
            reordered(&base, n - 1 - from, n - 1 - to)
        }
        _ => base,
    }
}

/// The model index a carried entry lands on, given the gap the pointer is in and
/// the display slot the entry is currently shown at.
///
/// The two conversions that make a live reorder settle instead of oscillate, and
/// both are off-by-one traps:
///
/// - The entry is *already in the list* at `shown_at`, so a gap counted against the
///   list as displayed is one too many for anything below it — taking the entry out
///   closes that gap up.
/// - Slots are counted from the top of the panel and the model is counted from the
///   bottom of the stack, so the two run opposite ways.
///
/// Being a pure function of where the pointer is and where the entry is *shown* —
/// rather than an accumulation — is what makes it stable: with the pointer still,
/// the answer is the slot the entry is already in, so nothing moves.
pub fn landing(n: usize, shown_at: usize, gap: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let landing = if gap > shown_at { gap - 1 } else { gap };
    n - 1 - landing.min(n - 1)
}

/// Reorder `items` so that `from` lands at `to`, both model indices.
///
/// Spelled once because two lists need it and the off-by-one is easy to get
/// wrong: removing the carried item shifts everything after it down, so an index
/// taken against the original list has to be adjusted before it is inserted
/// against the shortened one.
pub fn reordered<T: Clone>(items: &[T], from: usize, to: usize) -> Vec<T> {
    let mut out = items.to_vec();
    if from >= out.len() {
        return out;
    }
    let item = out.remove(from);
    out.insert(to.min(out.len()), item);
    out
}

/// Where the paint currently at `i` ends up after [`reordered`] moves `from` to
/// `to`. `None` when `i` names nothing.
///
/// The same off-by-one as [`reordered`], read the other way round, and it exists
/// for the *chrome* rather than for the list: an open options popup, a sides row, a
/// picker and a half-typed dash list are all keyed by list index, so a reorder that
/// renumbers the list has to renumber them too or each ends up describing whichever
/// paint inherited its number. Derived from `reordered` by construction — remove
/// `from`, which brings everything above it down one, then insert, which pushes
/// everything at or after the landing back up.
pub fn reordered_index(n: usize, from: usize, to: usize, i: usize) -> Option<usize> {
    if from >= n || i >= n {
        return None;
    }
    let to = to.min(n - 1);
    if i == from {
        return Some(to);
    }
    let shortened = if i > from { i - 1 } else { i };
    Some(if shortened >= to {
        shortened + 1
    } else {
        shortened
    })
}

/// Where the paint currently at `i` ends up once the one at `gone` is removed.
/// `None` for the one that went — whatever was keyed to it has nothing left to
/// name.
pub fn removed_index(gone: usize, i: usize) -> Option<usize> {
    match i.cmp(&gone) {
        std::cmp::Ordering::Less => Some(i),
        std::cmp::Ordering::Equal => None,
        std::cmp::Ordering::Greater => Some(i - 1),
    }
}

/// Which kind of paint a brush is.
///
/// Four kinds and three tabs: see [`PaintKind::Image`] for the one that
/// classifies a brush without being something the picker can convert to
/// (§15 D180).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaintKind {
    Solid,
    Linear,
    Radial,
    /// A picture. **Deliberately not in [`PaintKind::ALL`]**, which is the
    /// picker's tab strip: the other three are conversions of one another and an
    /// image is not — you pick one from a file dialog, not from a colour wheel,
    /// and a fourth tab would turn a photograph into a gradient on click.
    Image,
}

impl PaintKind {
    /// The kinds the picker offers as tabs, which is every kind that converts to
    /// every other. See [`PaintKind::Image`] for the one that does not.
    pub const ALL: [PaintKind; 3] = [PaintKind::Solid, PaintKind::Linear, PaintKind::Radial];

    pub fn label(self) -> &'static str {
        match self {
            PaintKind::Solid => "Solid",
            PaintKind::Linear => "Linear",
            PaintKind::Radial => "Radial",
            PaintKind::Image => "Image",
        }
    }
}

/// Which paint the picker is editing. Indices are into the node's
/// `fills` / `strokes`, so a stale index (the row was removed while the picker
/// was open) simply reads back `None` and closes the picker.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaintSlot {
    Fill(usize),
    Stroke(usize),
    // **A `Background` variant sat here and is gone** (§15 D400) — a frame's one
    // ground, the only node slot with no index on it. A frame's fills are
    // `Fill(i)` now, which is what lets a frame's picture open crop mode and its
    // rows carry a grip.
    /// The canvas ground behind and around every frame.
    ///
    /// Not a node's paint at all — it is a property of the *view*, and the one
    /// slot with no node behind it. It rides on `PaintSlot` because everything
    /// downstream of a colour chip (the row, the picker, the hex field) is
    /// written against a slot, and a second parallel path for one colour would
    /// be two implementations of the same panel.
    Canvas,
    /// The selected ruler guides' colour — **every** one of them, since guides
    /// multi-select: a write here emits one `SetGuideColor` per selected guide, and
    /// the row reports `Swatch::Mixed` where they disagree.
    ///
    /// The second slot with no node behind it, and for the same reason as
    /// [`PaintSlot::Canvas`]: the swatch, the picker and the hex field are all
    /// written against a slot, so a guide riding one gets the whole colour panel
    /// instead of a parallel copy of it. That is also why the guide's picker is
    /// literally the same picker — the symptom that suggested otherwise was one
    /// missing preview fork (§15 D141).
    Guide,
    /// One row of the fill (or stroke) list **every** paint target in the
    /// selection shares — the same index in all of them at once.
    ///
    /// Node-less like the two above, and for the third time for the same reason:
    /// the swatch, the picker and the hex field are written against a slot, so
    /// riding one is what gets a selection the whole paint panel rather than a
    /// parallel copy of it. The scope is simply whatever is selected, which is
    /// why nothing here needs to name it.
    Selection(ondin_core::PaintTarget, usize),
    /// **Every** fill (or stroke) of every paint target in the selection, at
    /// once — the slot the mixed row edits.
    ///
    /// Its sibling above names one row of a list they all share; this one exists
    /// for when there is no such list. What it can promise instead is that
    /// whatever it writes becomes true of every paint in scope, which is why the
    /// row it drives states a value only where they already agree on one
    /// (`build::shared_over_fills`) and reads as mixed everywhere else.
    ///
    /// The write is a *property* edit, not a replacement: `build::edit_fills_all`
    /// leaves each target's own list length, order and untouched properties
    /// alone. That is the whole answer to the objection §15 D53 raised against a
    /// "Mixed" label — that the word offers one destructive move and nothing else.
    SelectionAll(ondin_core::PaintTarget),
    /// The colour of a text node's underline or strikethrough.
    ///
    /// Node-less in the sense that matters here: it names no entry in `fills` or
    /// `strokes` at all, but a **character attribute** — and it rides a
    /// `PaintSlot` for the fourth time for the third time's reason. The swatch,
    /// the picker and the hex field are all written against a slot, so a
    /// decoration riding one gets the whole colour panel instead of a hex box
    /// standing in for it (which is what it had).
    ///
    /// The one thing it does not share with the rest: its write may not be a
    /// document transaction at all. While a live editing session owns a
    /// selection, a character attribute goes into the session's own spans
    /// (§9.3), so all three slot verbs divert it — see [`PaintSlot::char_scoped`].
    TextDecoration(DecorationSide),
    /// The colour of the **text itself** over the range a character control acts
    /// on — a run colour (§15 D154).
    ///
    /// `TextDecoration`'s sibling, and the second character-scoped slot, which is
    /// what let the three verbs' diversion become one named route rather than a
    /// third copy of the same branch.
    ///
    /// Absence is a state here as it is for a decoration: no span means the run
    /// draws the node's own fill stack, which is why the row's swatch can show a
    /// *gradient* (`TypeSubject::text_ramp`) where this slot itself only ever holds
    /// one colour.
    TextColor,
    /// One colour of the Group Colors panel: every fill and every stroke in the
    /// selection carrying **this exact colour** — a frame's fills included, and no
    /// longer as a third thing to enumerate (§15 D400).
    ///
    /// Keyed by the colour rather than by a place that holds it, because that is
    /// what the edit means — recolour by colour, not by layer. The key is the
    /// committed RGBA, so it stays valid for the whole of a drag (the document
    /// does not change until release) and is retargeted once the edit commits.
    GroupColor([u8; 4]),
    /// One **gradient** of the Group Colors panel: every use of that exact
    /// gradient in the selection.
    ///
    /// `GroupColor`'s twin, keyed by a **place** rather than by a value, and the
    /// difference is forced: a gradient's identity is its stops *and* its geometry,
    /// which does not fit in a `Copy + Eq` slot the way four bytes of RGBA do. So
    /// the slot names the first place the census found it
    /// (`build::GradientUse::at`) and the edit is still by value — the brush is
    /// read back from that place and `build::repaint` changes every use of it.
    ///
    /// The location key turns out to be the *better* one, and not only the one that
    /// fits. A value key names something the document stops having the moment the
    /// edit commits, which is why `GroupColor` needs `retarget_group_color`; a place
    /// still holds the paint afterwards, so this needs no retargeting at all.
    GroupGradient(ondin_core::PaintAt),
    /// One **effect**'s colour — a drop or inner shadow's, by its index in the
    /// layer's effect stack (§5.3a).
    ///
    /// The fifth slot naming something that is not a fill, and it rides here for
    /// the fourth time for the third time's reason: the swatch, the hex field and
    /// the detached picker are all written against a slot, so a shadow's colour
    /// gets the whole colour panel instead of a hex box standing in for it.
    ///
    /// **The alpha in this slot is the shadow's opacity and there is no second
    /// spelling of it** (§15 D333). So the row it drives is the Fill row's row —
    /// a chip, a hex and a percentage — rather than a chip beside an opacity field
    /// of the effect's own, and the mockup's `Opacity` box is that percentage.
    ///
    /// An index into a list the panel is showing, exactly as `Fill(_)` is: a stale
    /// one (the effect was removed while the picker was open) reads back `None`
    /// from `slot_brush` and closes the picker, which is the same answer every
    /// index-keyed slot gives.
    Effect(usize),
    /// One **layout grid**'s colour — the bands drawn over a frame, by that grid's
    /// index in the frame's list (`ondin_core::layout`, §15 D387).
    ///
    /// The sixth slot naming something that is not a fill, and it rides here for
    /// the fifth time for the third time's reason: the swatch, the hex field and
    /// the detached picker are all written against a slot, so a grid's colour gets
    /// the whole colour panel instead of a hex box standing in for it.
    ///
    /// ⚠️ **The only slot whose paint is chrome rather than ink.** Every other one
    /// names something in the scene, so `RenderOverrides` previews its drag for
    /// free; a grid is drawn by the canvas over the artwork and
    /// `Operation::SetLayoutGrids` is absorbed by the override set as a no-op, so
    /// the preview forks the way a guide's does (`OndinApp::preview_grid`). That is
    /// the same missing fork §15 D141 was, written down before it is one again.
    ///
    /// **The alpha in this slot is the grid's own translucency and there is no
    /// second spelling of it**, exactly as [`PaintSlot::Effect`] holds a shadow's
    /// opacity (§15 D333) — a grid is a wash rather than a hairline, which is what
    /// `layout::DEFAULT_GRID_COLOR`'s own note is about, so the percentage beside
    /// the hex *is* that wash.
    ///
    /// An index into a list the panel is showing, as `Fill(_)` is: a stale one —
    /// the grid was removed while the picker was open — reads back `None` from
    /// `slot_brush` and closes the picker.
    Grid(usize),
}

/// Which of a text node's two decorations a [`PaintSlot::TextDecoration`] names.
///
/// Its own two-variant enum rather than a `CharAttrKind`, which would let the
/// slot be built naming `Size` — a state with no meaning that every reader would
/// then have to rule out.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum DecorationSide {
    Underline,
    Strikethrough,
}

impl DecorationSide {
    pub fn kind(self) -> ondin_core::CharAttrKind {
        match self {
            DecorationSide::Underline => ondin_core::CharAttrKind::Underline,
            DecorationSide::Strikethrough => ondin_core::CharAttrKind::Strikethrough,
        }
    }

    /// The side a decoration attribute names, or `None` for anything else.
    pub fn of(kind: ondin_core::CharAttrKind) -> Option<Self> {
        match kind {
            ondin_core::CharAttrKind::Underline => Some(DecorationSide::Underline),
            ondin_core::CharAttrKind::Strikethrough => Some(DecorationSide::Strikethrough),
            _ => None,
        }
    }
}

/// A paint slot that writes a **character attribute** rather than a document
/// transaction.
///
/// **The concept the three slot verbs were each branching on separately.** Every
/// other slot ends in `slot_transaction`; these cannot, because while a live editing
/// session owns a selection the write belongs in that session's spans (§9.3). With
/// one such slot that was one diverting `if` in `write_slot`, another in
/// `valve_slot`, and a `return false` in `preview_slot` — three places to keep in
/// step, and the third of them was where the press-preview gap lived (§15 D129).
/// With two slots it is a type, and `OndinApp::write_char_slot` is the single route.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CharSlot {
    Decoration(DecorationSide),
    /// The text's own ink over the acted-on range.
    Color,
}

impl CharSlot {
    /// The attribute this slot's colour lives in.
    pub fn kind(self) -> ondin_core::CharAttrKind {
        match self {
            CharSlot::Decoration(side) => side.kind(),
            CharSlot::Color => ondin_core::CharAttrKind::Color,
        }
    }

    /// The `PaintSlot` that names this — the round trip of
    /// [`PaintSlot::char_scoped`], so the picker is opened on the same slot the
    /// verbs will divert back here.
    pub fn paint_slot(self) -> PaintSlot {
        match self {
            CharSlot::Decoration(side) => PaintSlot::TextDecoration(side),
            CharSlot::Color => PaintSlot::TextColor,
        }
    }

    /// What the swatch says on hover, given whether the slot holds its own colour
    /// and whether what it inherits is a gradient.
    ///
    /// **The inherited state is a different sentence for each slot, and that is the
    /// whole of why this is not one string.** A decoration inherits *the text's*
    /// ink; the text inherits *the layer's fill*. Saying "inherited from the text"
    /// on the text's own row would be circular.
    pub fn swatch_tooltip(self, has_own: bool, inherits_gradient: bool) -> &'static str {
        match (self, has_own, inherits_gradient) {
            (_, true, _) => "Edit this colour",
            // The ramp already says "inherited, and not one colour"; the tooltip is
            // where the consequence of clicking goes, because taking a colour of your
            // own cannot keep the gradient — `write_char_slot` collapses one to its
            // first stop, which is the hex shown beside it.
            (CharSlot::Decoration(_), false, true) => {
                "Inherited from the text's gradient — click to take its first colour as \
                 this line's own"
            }
            (CharSlot::Decoration(_), false, false) => {
                "Inherited from the text — click to give it its own"
            }
            (CharSlot::Color, false, true) => {
                "Inherited from the layer's gradient fill — click to take its first \
                 colour as this text's own"
            }
            (CharSlot::Color, false, false) => {
                "Inherited from the layer's fill — click to give this text its own colour"
            }
        }
    }

    /// What the row's reset button puts the colour back to.
    pub fn reset_tip(self) -> &'static str {
        match self {
            CharSlot::Decoration(_) => "Back to the text's own colour",
            CharSlot::Color => "Back to the layer's fill",
        }
    }
}

/// Which of the three slot verbs a character-scoped write is performing.
///
/// Named rather than an `Option<&Response>`, which is what the two-verb version
/// took: the third state is not "no response" but "a response, and do not commit
/// on it", and a bool beside the option would have been two fields with one
/// meaningless combination.
#[derive(Clone, Copy)]
pub enum CharWrite<'a> {
    /// A click, or a hex typed and committed: write it now.
    Commit,
    /// A drag: preview each frame, commit once on release.
    Valve(&'a egui::Response),
    /// A pointer **down** on a colour control that has not travelled far enough to
    /// be a drag (§15 D129): show it, commit nothing.
    Preview,
}

impl PaintSlot {
    /// The character-scoped slot this is, or `None` for the ones whose write is an
    /// ordinary transaction.
    ///
    /// The one predicate all three verbs ask, so none of them can forget a slot the
    /// others divert — which is exactly how `preview_slot` came to decline a
    /// decoration while its siblings handled it.
    pub fn char_scoped(self) -> Option<CharSlot> {
        match self {
            PaintSlot::TextDecoration(side) => Some(CharSlot::Decoration(side)),
            PaintSlot::TextColor => Some(CharSlot::Color),
            _ => None,
        }
    }

    /// Whether this slot can hold a gradient.
    ///
    /// Neither node-less scalar slot can: the renderer takes a single clear
    /// colour for the canvas ground, and a guide is one hairline of chrome. An
    /// *enabled* gradient tab would promise something neither can draw.
    ///
    /// **This answers whether the tabs are live, not whether they are drawn.** The
    /// picker used to drop them entirely for a slot that answers `false` here, and
    /// what that left was a header holding nothing but a close button; they are
    /// shown and disabled now (§15 D134), so the panel is recognisably the same
    /// panel whatever it is editing.
    ///
    /// Nor can a Group Colors row, for a different reason: it is a row *in a list
    /// of solid colours*, so a gradient dropped into it would be a value the list
    /// it lives in cannot show and the census that built it does not count.
    ///
    /// Nor a decoration, which is the guide's case again: `Decoration::color` is
    /// one `Color`, and a rule two pixels thick has nowhere to put a ramp.
    ///
    /// **Nor a run colour, and that one is a model decision rather than a drawing
    /// one** (§15 D154): a gradient per run has no honest geometry to anchor to —
    /// the run's box reflows as you type, so a ramp pinned to it would move while
    /// you typed into it — so `TextStyle::color` is a `Color` and this says so.
    ///
    /// **Nor a shadow**, which is the guide's case a third time: `Shadow::color`
    /// is one `Color`, and all three writers cast the silhouette in a single ink.
    /// A ramp would be a value the model cannot hold and none of the three could
    /// draw.
    ///
    /// **Nor a layout grid**, which is the guide's case a fourth time and its
    /// nearest relative besides: `LayoutGrid::color` is one `Color`, and the bands
    /// are painted by `CanvasRenderer::draw_layout_grids` as flat convex polygons —
    /// there is no shader behind them for a ramp to go into.
    pub fn takes_gradients(self) -> bool {
        !matches!(
            self,
            PaintSlot::Canvas
                | PaintSlot::Guide
                | PaintSlot::GroupColor(_)
                | PaintSlot::TextDecoration(_)
                | PaintSlot::TextColor
                | PaintSlot::Effect(_)
                | PaintSlot::Grid(_)
        )
    }

    /// Whether this slot has an opacity worth showing. The canvas ground has
    /// nothing behind it to be transparent *against*, so a slider that fades it
    /// toward black is a control with no honest meaning. A guide is the
    /// opposite case and lands in the same place: it is a marker the user has
    /// to be able to see over the artwork, and one faded to nothing is a guide
    /// that has been deleted the hard way.
    pub fn takes_alpha(self) -> bool {
        !matches!(self, PaintSlot::Canvas | PaintSlot::Guide)
    }
}

pub fn kind_of(brush: &Brush) -> PaintKind {
    match brush {
        Brush::Gradient(g) => match g.gradient.kind {
            GradientKind::Radial(_) => PaintKind::Radial,
            // Sweep is not offered by the UI; nothing constructs one.
            _ => PaintKind::Linear,
        },
        Brush::Image(_) => PaintKind::Image,
        _ => PaintKind::Solid,
    }
}

/// The brush's colour stops. A solid brush is one stop at 0, so every control
/// downstream can be written once against a stop list.
pub fn stops_of(brush: &Brush) -> Vec<(f32, Color)> {
    match brush {
        Brush::Gradient(g) => {
            let mut stops: Vec<(f32, Color)> = g
                .gradient
                .stops
                .iter()
                .map(|s| (s.offset, s.color.to_alpha_color()))
                .collect();
            if stops.is_empty() {
                stops.push((0.0, Color::BLACK));
            }
            stops
        }
        Brush::Solid(c) => vec![(0.0, *c)],
        Brush::Image(_) => vec![(0.0, Color::from_rgba8(217, 217, 217, 255))],
    }
}

/// The alpha a picture brush is drawn at — `ImageSampler::alpha`, clamped, and
/// `1.0` for every brush that is not a picture.
///
/// **This is the image arm of §15 D773 and it is older than that entry**
/// (§15 D784). D767 moved a gradient's opacity off its stops and broke every chip
/// that had been reading the stops; D773 fixed the gradient arm and wrote down —
/// deliberately, rather than leaving it implicit — that the picture arm had the
/// identical gap and had had it since §15 D185. `ImageSampler::alpha` is folded
/// in at `color::brush_to_backend` for both backends and written by the SVG
/// writer, and was folded in by nothing in the chrome, so a fill dropped to 30%
/// drew a **fully opaque** thumbnail over a canvas drawing it faint. §15 D564's
/// *one document drawn two ways in one frame*.
///
/// 🚨 **D773 named one surface and there are two.** Besides the inspector's
/// paint-row chip ([`crate::ui::Swatch::Picture`]) the layers panel draws the
/// same picture on every image layer's row, through its own `picture_of`, which
/// took `&ImageRef` out of the brush and dropped the sampler on the floor. Both
/// fold this in now. **A rule with one statement and two implementations is this
/// project's most-recorded shape**, and the way it happened here is the ordinary
/// one: the entry that decided the rule was written against the surface its
/// author was looking at.
///
/// ⚠️ **Not folded into [`stops_of`]'s picture arm**, which is a flat grey
/// placeholder for a picture that could not be decoded rather than a picture —
/// fading it would say something about a photograph nobody can see.
///
/// ⚠️ **Two entry points and one clamp**, because the two chips reach the brush by
/// different roads: the inspector has a `&Brush` and the layers row has an
/// `&ImageBrush` out of its own `picture_of`. [`image_brush_alpha`] is the rule
/// and this is the arm that answers for a brush that is not a picture at all —
/// the alternative was the same `clamp` written twice, which is what this entry
/// exists to stop.
pub fn image_alpha(brush: &Brush) -> f32 {
    match brush {
        Brush::Image(img) => image_brush_alpha(img),
        _ => 1.0,
    }
}

/// The clamp itself: a picture brush's `ImageSampler::alpha`, held to `0..=1`.
///
/// **The clamp is not decoration.** `alpha` is a `#[serde(default)]`-shaped float
/// the loader does not range-check — `build::brush_is_finite` asks only that it is
/// finite — so a hand-edited or third-party `.ondin` can carry `-3.0` or `1e30`,
/// and `Color32::gamma_multiply` outside `0..=1` is not a fade. See [`image_alpha`]
/// for why this is one function and not two.
pub fn image_brush_alpha(img: &ondin_core::ImageBrush) -> f32 {
    img.sampler.alpha.clamp(0.0, 1.0)
}

/// Replace a brush's stops, keeping its kind and geometry. A solid brush keeps
/// only the first stop — that is what "solid" means.
///
/// ⚠️ **"Geometry" includes the brush's transform, and it rides along untouched**
/// (§15 D412). An imported elliptical gradient carries a squash; editing its
/// colours must not turn it round, which is what a rebuild-from-scratch would do.
/// The clone below is what keeps it — stated because it is invisible.
pub fn with_stops(brush: &Brush, mut stops: Vec<(f32, Color)>) -> Brush {
    stops.sort_by(|a, b| a.0.total_cmp(&b.0));
    match brush {
        // **An image has no stops, so writing them is refused rather than
        // converted.** It used to fall through to the arm below and come back a
        // `Brush::Solid` of the placeholder grey [`stops_of`] invents for the
        // swatch — so one nudge of an image row's opacity scrubber replaced the
        // photograph with a grey rectangle. Returning the brush unchanged is the
        // only honest answer: a colour is not a thing this brush can be.
        Brush::Image(_) => brush.clone(),
        Brush::Gradient(g) => {
            let mut g = g.clone();
            g.gradient.stops.clear();
            for (offset, color) in stops {
                g.gradient.stops.push(ondin_core::peniko::ColorStop {
                    offset: offset.clamp(0.0, 1.0),
                    color: ondin_core::peniko::color::DynamicColor::from_alpha_color(color),
                });
            }
            Brush::Gradient(g)
        }
        _ => Brush::Solid(stops.first().map(|s| s.1).unwrap_or(Color::BLACK)),
    }
}

/// A brush's opacity: for a solid, its alpha; for a gradient **and** an image,
/// the brush's own multiplier — **normalised into `0..=1`** (§15 D692, D767).
///
/// ⚠️ **The clamp is not decoration; this function is a producer for a
/// `0..=100` field.** `[S23.2-L1-03]`: a document carrying a stop alpha above
/// 1 — reachable before D692 through an out-of-spec `stop-opacity`, and still
/// reachable in any file already saved — put **500** into a
/// `range(0.0..=100.0)` control, and egui's `clamp_existing_to_range` then made
/// one bare click write `with_alpha(_, 1.0)`. With a raw peak of 5.0 that
/// rescaled the *whole ramp*: a second stop at 25% came back at **5%**, a
/// visible change to the picture from a click that typed nothing.
///
/// 🚨 **Both harms are gone at the root since §15 D767 and the clamp is *more*
/// load-bearing, not less.** A gradient's opacity is a field, so a rogue stop
/// cannot reach this answer by any route and [`with_alpha`] has no `peak` to
/// divide by. But the field is `#[serde(default)]` — **any six characters in a
/// `.ondin` become it** — so this clamp now guards a *producer that did not
/// exist*, which is D713's shape one field later. The out-clamp in `picker.rs`
/// carried a comment saying it could never fire; it gained a producer the same
/// day. **A safety argument that enumerates its producers is false the day a
/// second one appears.**
///
/// ⚠️ **What was lost is an accidental repair, and it is recorded rather than
/// mourned**: dividing by the peak used to *normalise* a stored out-of-range
/// stop back to 1.0 on any bare click. It no longer does, so
/// `stop-opacity="5"` stays 5.0 in the document — a stop-validation gap, which
/// it always was, and the opacity field was never the right place to close it.
///
/// `[S23.1-L1-01]`'s `radial_centre` and this were the two producers in this
/// module that normalised nothing; `linear_angle`'s `.rem_euclid(360.0)` is the
/// shape both should have had.
pub fn alpha_of(brush: &Brush) -> f32 {
    // An image's opacity is peniko's own multiplier on the sampler, not
    // something the stop list can answer: `stops_of` hands back a placeholder
    // grey for the swatch to draw, and reading *its* alpha would report 100% for
    // a half-faded photograph.
    if let Brush::Image(img) = brush {
        return img.sampler.alpha.clamp(0.0, 1.0);
    }
    // **A gradient's own multiplier, for the image's reason** (§15 D767). Since the
    // ramp's opacity is a field rather than a scaling of the stops, the stop list no
    // longer answers this question — a ramp at 30% still has a stop at full alpha,
    // and the `max` below would report 100%.
    //
    // ⚠️ **This is the accessor `inspector`'s *Mixed* guard reads** (§15 D465,
    // `pct != paint::alpha_of(brush) * 100.0`), so it has to be the one that knows
    // about the field or that guard silently compares the wrong quantity and D465's
    // bug returns. Clamped here rather than trusted from the file, which is D692's
    // rule and D713's lesson for a `#[serde(default)]` number.
    if let Brush::Gradient(g) = brush {
        return g.opacity.clamp(0.0, 1.0);
    }
    stops_of(brush)
        .iter()
        .map(|(_, c)| c.components[3])
        .fold(0.0_f32, f32::max)
        .clamp(0.0, 1.0)
}

/// Set a brush's opacity.
///
/// 🚨 **A gradient stores it rather than scaling its stops, since §15 D767 —
/// because scaling is not invertible through zero.** The old spelling multiplied
/// every stop by `alpha / peak`; taking a ramp to 0% made `peak` zero, and the only
/// thing the next call could do was write one flat alpha across the ramp.
/// `[1.0, 0.0]` — the shape **every** gradient this app authors starts as, since
/// `convert` fades a solid out — came back `[1.0, 1.0]`: flat, opaque, the fade
/// gone, and no undo step to blame. A control that destroys its subject when it
/// passes through a value the field offers is not a control.
///
/// **The image arm was the model**: it assigns a multiplier and touches no pixel,
/// which is why it was already lossless. The gradient arm was the only one of the
/// three that rewrote what it was adjusting, and that asymmetry *was* the defect.
///
/// ⚠️ **The floors that would have kept `peak` away from zero are ruled out**, not
/// merely unattractive: the maintainer's *"0 alpha means fully invisible"* means a
/// visible-but-faint fill at `0` is wrong, and a *hidden* clamp is the same thing
/// with the field lying about it.
///
/// ⚠️ **Forward-only.** A ramp already flattened in a saved document stays flat —
/// the ratio is gone from the file and nothing can invent it.
pub fn with_alpha(brush: &Brush, alpha: f32) -> Brush {
    let alpha = alpha.clamp(0.0, 1.0);
    // The sampler's multiplier, for the reason [`alpha_of`] reads it: an image
    // has no stops to scale, and the stop path would hand back a grey solid.
    if let Brush::Image(img) = brush {
        let mut img = img.clone();
        img.sampler.alpha = alpha;
        return Brush::Image(img);
    }
    // The ramp's own multiplier — the image arm's shape, one brush over.
    if let Brush::Gradient(g) = brush {
        let mut g = g.clone();
        g.opacity = alpha;
        return Brush::Gradient(g);
    }
    let stops = stops_of(brush)
        .into_iter()
        .map(|(off, c)| (off, c.with_alpha(alpha)))
        .collect();
    with_stops(brush, stops)
}

/// Change a brush's kind, preserving its stops and reprojecting its geometry
/// onto `box_size`. Solid → gradient fades the colour out, which is the
/// convention every vector editor uses and is immediately legible on canvas.
///
/// ⚠️ **An image is refused, for [`with_stops`]' reason and by its rule** (§15
/// D686, `[S23.2-L3-06]`). This was the one of the module's three image-facing
/// helpers with no guard at all: `with_stops` refuses outright and `default_brush`
/// carries a `debug_assert!`, while this one asked [`stops_of`], got the
/// placeholder grey that function invents *for the swatch to draw*, fanned it into
/// a two-stop fade and returned a **grey gradient**. The photograph is gone. That
/// is the same shipped bug `with_stops`' own comment records being fixed — *"one
/// nudge of an image row's opacity scrubber replaced the photograph with a grey
/// rectangle"* — in the third helper.
///
/// **No failing input today**, and the guard is the point rather than the
/// objection to it: the only caller is `picker::picker_body`, which is protected
/// twice over — `PaintKind::Image` is deliberately absent from `PaintKind::ALL`,
/// and that function returns early for an image brush. But the module decided this
/// rule *here*, twice, and the third helper's protection lived in another file.
/// A second caller — the Group Colors row, a *Convert to gradient* menu item, an
/// MCP verb — would have inherited none of it, and nothing in this file would have
/// said so.
pub fn convert(brush: &Brush, kind: PaintKind, box_size: Size) -> Brush {
    if matches!(brush, Brush::Image(_)) || kind_of(brush) == kind {
        return brush.clone();
    }
    let mut stops = stops_of(brush);
    if kind != PaintKind::Solid && stops.len() < 2 {
        let c = stops[0].1;
        stops = vec![(0.0, c), (1.0, c.with_alpha(0.0))];
    }
    // The kind has genuinely changed, so there is no old geometry worth
    // keeping — a radial's centre and radius say nothing about where a linear
    // ramp should run. `default_brush` reframes it on the box.
    default_brush(kind, stops, box_size)
}

/// A brush of `kind` with `stops`, framed on `box_size`.
///
/// **Every gradient this makes is authored in the shape's own space, so its
/// transform is the identity** — which is what `From<Gradient>` supplies and is
/// the reason nothing in this panel ever writes that field (§15 D412). It is the
/// other half of [`with_stops`]'s rule: geometry the *app* authors starts square,
/// and geometry that came in from a file keeps whatever it arrived with. Coming
/// through here is therefore how a user turns an imported ellipse back into a
/// circle — by changing the kind, which is the one gesture that means "throw the
/// old geometry away".
pub fn default_brush(kind: PaintKind, stops: Vec<(f32, Color)>, box_size: Size) -> Brush {
    let (w, h) = (box_size.width.max(1.0), box_size.height.max(1.0));
    let brush = match kind {
        PaintKind::Solid => {
            return Brush::Solid(stops.first().map(|s| s.1).unwrap_or(Color::BLACK));
        }
        PaintKind::Linear => {
            Brush::Gradient(Gradient::new_linear(Point::ZERO, Point::new(w, h)).into())
        }
        PaintKind::Radial => Brush::Gradient(
            Gradient::new_radial(Point::new(w * 0.5, h * 0.5), (w.min(h) * 0.5) as f32).into(),
        ),
        // **There is no default image**, because an image is an id and stops
        // cannot invent one. Unreachable in practice — `PaintKind::Image` is not
        // in `ALL`, so the picker's tabs cannot ask for it, and `convert` returns
        // early when the kind already matches — but a grey solid is what comes
        // out if some later caller does ask, rather than a panic in the user's
        // session over a paint row. The assert is so a test finds it first.
        PaintKind::Image => {
            debug_assert!(false, "no brush can be built for PaintKind::Image");
            return Brush::Solid(stops.first().map(|s| s.1).unwrap_or(Color::BLACK));
        }
    };
    with_stops(&brush, stops)
}

/// The picture `brush` shows, or `None` for a brush that is not one.
///
/// What the image popover reads every control's current value from — the tile
/// scale, the orientation, whether there is a crop to reset.
pub fn image_of(brush: &Brush) -> Option<&ondin_core::ImageRef> {
    match brush {
        Brush::Image(img) => Some(&img.image),
        _ => None,
    }
}

/// `brush` with `edit` applied to the picture it shows, **keeping the sampler and
/// everything else about it**.
///
/// The general form of [`with_fit`], for the popover's controls: each of them
/// changes one field of the reference and must carry the other three, the
/// sampler's alpha included — an opacity silently reset by the flip button is
/// exactly the class of bug [`with_stops`] was tightened against.
///
/// Returns a brush that is not an image unchanged, as [`with_stops`] and
/// [`with_fit`] do.
pub fn map_image(brush: &Brush, edit: impl FnOnce(&mut ondin_core::ImageRef)) -> Brush {
    let Brush::Image(img) = brush else {
        return brush.clone();
    };
    let mut img = img.clone();
    edit(&mut img.image);
    Brush::Image(img)
}

/// `brush` in framing mode `fit`, **keeping everything else about the picture**.
///
/// The mode strip's whole write, and the reason it is one function: the tempting
/// spelling is to rebuild the reference from its id, which loses the crop
/// rectangle and the tile scale — so clicking *Fit* to look at the whole picture
/// and clicking back would hand you a fresh identity crop, and the crop gesture's
/// output would be two clicks from gone. A mode switch is a change of view.
///
/// Returns a brush that is not an image unchanged, as [`with_stops`] does: there
/// is no mode to put a colour in.
/// **`frame` is what makes switching *to* `Crop` change nothing on screen.** A
/// picture nobody has cropped carries `ondin_core::whole_crop`, the whole source
/// — which mapped onto a frame of a different aspect *stretches*, a different
/// picture arrived at by picking the mode whose entire job is not to change one.
/// Given the frame, the switch seeds `ImageRef::cover_crop` instead: the
/// rectangle `Fill` is already showing.
///
/// The canvas gesture seeds the same way from the same sentinel, and this is the
/// other half — a value meaning "never cropped" has to mean it in the panel too,
/// or the two disagree about one stored number.
///
/// `None` where the frame is unknowable — a mixed selection, a shape whose image
/// the document has lost — and the mode is then set without a seed rather than
/// with a guessed one.
pub fn with_fit(
    brush: &Brush,
    fit: ondin_core::ImageFit,
    frame: Option<(ondin_core::kurbo::Rect, u32, u32)>,
) -> Brush {
    let Brush::Image(img) = brush else {
        return brush.clone();
    };
    let mut img = img.clone();
    img.image.fit = fit;
    if fit == ondin_core::ImageFit::Crop
        && img.image.crop == ondin_core::whole_crop()
        && let Some((frame, w, h)) = frame
    {
        img.image.crop = img.image.cover_crop(frame, w, h);
    }
    Brush::Image(img)
}

/// A solid mid-grey — what a freshly added fill or stroke starts as.
pub fn new_solid() -> Brush {
    Brush::Solid(Color::from_rgba8(217, 217, 217, 255))
}

// --- linear geometry -------------------------------------------------------

/// ⚠️ **The four geometry accessors below all speak the *shape's* space, and the
/// brush's geometry may not** (§15 D412).
///
/// A `GradientBrush` carries an affine from the space its ramp was authored in
/// onto the shape's own. It is the identity for everything this panel makes — see
/// [`default_brush`] — and a squash for a radial gradient that came in from a
/// file, which is the one thing `peniko`'s two circles cannot express.
///
/// **So a reader maps forward and a writer maps back**, and the rule is uniform
/// rather than per-control: what the panel shows is what the user sees on the
/// canvas, and what the user types is where they want it to land. Ignoring the
/// transform would report the wrong angle and centre for an imported gradient and
/// then write a value that misses by the same amount — a bug that is invisible on
/// every gradient the app itself authored, which is every gradient in every test
/// that existed before this.
///
/// **Clearing the transform instead would have been the cheaper answer and is
/// worse**: it turns an imported ellipse round the first time anyone nudges its
/// centre. Mapping back keeps the drawing and moves the thing that was asked for.
/// Changing the *kind* still discards it, which is the one gesture that means
/// "throw this geometry away".
///
/// ⚠️ **The inverse is guarded, and this sentence used to say it did not need to
/// be** (§15 D713). It read: *"safe because `svg_in` only stores a transform
/// whose smaller singular value exceeds `1e-9` (`Paint::squashed_radial`), so the
/// determinant is bounded away from zero."* That is true of `svg_in` and it is
/// **not true of the field**, because the field has a second producer:
/// `GradientBrush::transform` is `#[serde(default)]` and the loader validates
/// nothing, so any six numbers in a `.ondin` become the transform. A singular one
/// is *finite*, so `build::brush_is_finite` accepts it at the op boundary, and
/// `Affine::inverse` then returns `NaN` in all six coefficients — one bare click
/// in the `CX` field wrote a document that never opened again.
///
/// **So both writers ask `build::affine_is_invertible` first and return the brush
/// untouched when the answer is no.** A gradient whose space collapses to a line
/// has no centre and no angle to address; doing nothing is the honest answer, and
/// `Transaction::changes_nothing` (§15 D428) keeps it off the undo stack.
///
/// 🚨 **The general lesson is about the scope of a safety argument, not about
/// gradients.** The sentence was correct on the day it was written and named the
/// producer it had checked; what made it dangerous is that it reads as a property
/// of the *field*. **An argument that enumerates its producers is false the day a
/// second one appears, and nothing anywhere will say so.**
const _GRADIENT_GEOMETRY_IS_IN_SHAPE_SPACE: () = ();

/// A linear gradient's direction in degrees, clockwise from "left to right"
/// (screen axes: +y is down). `None` for anything that is not linear.
pub fn linear_angle(brush: &Brush) -> Option<f64> {
    let Brush::Gradient(g) = brush else {
        return None;
    };
    let GradientKind::Linear(pos) = g.gradient.kind else {
        return None;
    };
    let d = g.transform * pos.end - g.transform * pos.start;
    if d.hypot() < 1e-9 {
        return Some(0.0);
    }
    Some(d.y.atan2(d.x).to_degrees().rem_euclid(360.0))
}

/// Re-aim a linear gradient at `degrees`, spanning `box_size` through its
/// centre — the CSS `linear-gradient(<angle>)` framing, so the ramp always
/// covers the shape whatever the angle.
pub fn with_linear_angle(brush: &Brush, degrees: f64, box_size: Size) -> Brush {
    let Brush::Gradient(g) = brush else {
        return brush.clone();
    };
    let (w, h) = (box_size.width.max(1.0), box_size.height.max(1.0));
    let (sin, cos) = degrees.to_radians().sin_cos();
    let half = (w * cos.abs() + h * sin.abs()) * 0.5;
    let centre = Point::new(w * 0.5, h * 0.5);
    let d = Vec2::new(cos, sin) * half;
    // **A transform that cannot be inverted is left alone rather than inverted**
    // (§15 D713). The claim above this block is scoped to `svg_in`, which cannot
    // produce a singular transform; the *loader* is the field's second producer
    // and validates nothing, so the inverse below would write `NaN` into both
    // endpoints and the document would never open again.
    if !ondin_core::build::affine_is_invertible(g.transform) {
        return brush.clone();
    }
    let mut g = g.clone();
    // Authored in the shape's space and stored in the brush's — see
    // `_GRADIENT_GEOMETRY_IS_IN_SHAPE_SPACE`.
    let inv = g.transform.inverse();
    g.gradient.kind = GradientKind::Linear(ondin_core::peniko::LinearGradientPosition::new(
        inv * (centre - d),
        inv * (centre + d),
    ));
    Brush::Gradient(g)
}

/// Reverse a gradient's stops in place (`0.3 → 0.7`), leaving its geometry
/// alone. The design's flip control; also what "reverse" means for a radial,
/// where re-aiming makes no sense.
pub fn reverse_stops(brush: &Brush) -> Brush {
    let stops = stops_of(brush)
        .into_iter()
        .map(|(off, c)| (1.0 - off, c))
        .collect();
    with_stops(brush, stops)
}

// --- radial geometry -------------------------------------------------------

/// A radial gradient's centre as a fraction of `box_size`. `None` for anything
/// that is not radial.
pub fn radial_centre(brush: &Brush, box_size: Size) -> Option<(f64, f64)> {
    let Brush::Gradient(g) = brush else {
        return None;
    };
    let GradientKind::Radial(pos) = g.gradient.kind else {
        return None;
    };
    let (w, h) = (box_size.width.max(1.0), box_size.height.max(1.0));
    // In the shape's space — see `_GRADIENT_GEOMETRY_IS_IN_SHAPE_SPACE`.
    let c = g.transform * pos.end_center;
    Some((c.x / w, c.y / h))
}

/// Move a radial gradient's centre to `(cx, cy)` as fractions of `box_size`,
/// keeping its radius **and its focal point**.
///
/// ⚠️ **A radial gradient is two circles, and this control names only one of
/// them** (§15 D413). `peniko`'s `start_center`/`start_radius` is SVG's focal
/// circle — `fx`/`fy`, the point the ramp radiates *from*, which is what makes a
/// sphere look lit from one side — and `end_center`/`end_radius` is the outer
/// circle, which is the one [`radial_centre`] reports and this one moves. Writing
/// the new centre into **both**, which this did until 2026-09-03, discards the
/// offset between them: an imported highlight snapped to dead centre the first
/// time anyone nudged it, and nothing anywhere said so.
///
/// **So the move is a translation of the pair**, not an assignment to each. The
/// focal offset is a property of the gradient's shape, exactly like its radius —
/// which this function's own summary line already promised to keep, for the other
/// circle, since before there was a second one.
///
/// Every gradient the app itself authors has a zero offset (`Gradient::new_radial`
/// puts both centres on the same point), so this is invisible except on an import
/// — which is precisely why nothing caught it.
pub fn with_radial_centre(brush: &Brush, cx: f64, cy: f64, box_size: Size) -> Brush {
    let Brush::Gradient(g) = brush else {
        return brush.clone();
    };
    let GradientKind::Radial(pos) = g.gradient.kind else {
        return brush.clone();
    };
    let (w, h) = (box_size.width.max(1.0), box_size.height.max(1.0));
    // **A transform that cannot be inverted is left alone rather than inverted**
    // — `with_linear_angle`'s guard, for its reason (§15 D713).
    if !ondin_core::build::affine_is_invertible(g.transform) {
        return brush.clone();
    }
    // Asked for in the shape's space and stored in the brush's, so an imported
    // ellipse moves rather than turning round — see
    // `_GRADIENT_GEOMETRY_IS_IN_SHAPE_SPACE`.
    let centre = g.transform.inverse() * Point::new(cx * w, cy * h);
    let focal = pos.start_center - pos.end_center;
    let mut g = g.clone();
    g.gradient.kind = GradientKind::Radial(ondin_core::peniko::RadialGradientPosition {
        start_center: centre + focal,
        start_radius: pos.start_radius,
        end_center: centre,
        end_radius: pos.end_radius,
    });
    Brush::Gradient(g)
}

// --- presentation ----------------------------------------------------------

/// The row label for a paint: its hex when solid, its kind when a gradient.
pub fn label_of(brush: &Brush) -> String {
    match kind_of(brush) {
        PaintKind::Solid => crate::ui::hex_of(stops_of(brush)[0].1),
        kind => kind.label().to_string(),
    }
}

/// Sample a stop list at `t`, for adding a stop where the user clicked without
/// visibly changing the ramp.
pub fn sample(stops: &[(f32, Color)], t: f32) -> Color {
    if stops.is_empty() {
        return Color::BLACK;
    }
    let first = stops[0];
    if t <= first.0 {
        return first.1;
    }
    for pair in stops.windows(2) {
        let (a, b) = (pair[0], pair[1]);
        if t <= b.0 {
            let span = b.0 - a.0;
            let f = if span > f32::EPSILON {
                (t - a.0) / span
            } else {
                0.0
            };
            let (ca, cb) = (a.1.components, b.1.components);
            return Color::new([
                ca[0] + (cb[0] - ca[0]) * f,
                ca[1] + (cb[1] - ca[1]) * f,
                ca[2] + (cb[2] - ca[2]) * f,
                ca[3] + (cb[3] - ca[3]) * f,
            ]);
        }
    }
    stops[stops.len() - 1].1
}

#[cfg(test)]
mod tests {
    use super::*;

    const BOX: Size = Size::new(200.0, 100.0);

    fn an_image() -> Brush {
        ondin_core::image_brush(ondin_core::ImageId("sha256:photo".into()))
    }

    /// **One step of the Scale field must not lurch the picture**, and the place
    /// that has to hold is the *bottom* of the range (§15 D273).
    ///
    /// Reported as scrubbing being "glitchy/jumpy… especially around the 85–120%
    /// (for my exact case 85% is the min scale). Up from there it tends to be
    /// smooth", which names the cause once you write the arithmetic down. A crop
    /// scale is a **multiplicative** quantity: what the eye reads is the *relative*
    /// change, `Δs / s`, because a feature `d` from the centre of the window moves
    /// by `d · Δs / s`. A step of one whole percentage point is therefore 1/85 of
    /// the picture near the floor and 1/300 of it at 300% — the same keystroke,
    /// four times the lurch, which is exactly the band that was reported and
    /// exactly where it stops.
    ///
    /// So the field scrubs in **hundredths** (`Scrub::fine(_, 2)`) where the tile
    /// scale beside it scrubs in whole units, and the two differ for a reason
    /// rather than by oversight: a tile scale changes how big one repeat is, and a
    /// repeat has no centre for a rounding error to swing a distant corner around.
    ///
    /// Measured at the window's own corner, which is where the swing is largest
    /// and where the eye was.
    #[test]
    fn one_scale_step_moves_the_picture_less_than_a_unit_at_the_bottom_of_the_range() {
        use ondin_core::kurbo::{Point, Rect};

        // 850 × 850 of a 1000 × 1000 picture: cover is 0.85, the reporter's floor.
        let (w, h) = (1000u32, 1000u32);
        let frame = Rect::new(0.0, 0.0, 850.0, 850.0);
        let mut img = ondin_core::ImageRef::new(ondin_core::ImageId("x".into()));
        img.fit = ondin_core::ImageFit::Crop;
        img.crop = img.cover_crop(frame, w, h);
        let floor = img.cover_scale(frame, w, h).expect("a cover scale");
        assert!(
            (floor - 0.85).abs() < 1e-9,
            "the fixture is not at the reported floor: {floor}"
        );

        // **How far the thing under the window's corner appears to move**, for a
        // step of `step` percentage points.
        //
        // Not how far the *picture's* edge moves — that was the first metric and it
        // reported the same number at every scale, correctly: the picture's width
        // is `s · iw`, so its edges travel at a constant rate per unit of `s`
        // whatever `s` is. What the eye reads is the content inside a window that
        // is standing still, and *that* moves by `d · Δs / s`. Measured by taking
        // the source pixel under a fixed local point and asking where it lands
        // afterwards.
        let lurch = |from: &ondin_core::ImageRef, step: f64| -> f64 {
            let now = from.crop_scale(frame, w, h).expect("a scale");
            let next = now * 100.0 + step;
            let moved = ondin_core::ImageRef {
                crop: ondin_core::crop_scaled(from.crop, now / (next / 100.0)),
                ..from.clone()
            };
            let before = from.framing(frame, w, h).transform;
            let after = moved.framing(frame, w, h).transform;
            // The corner of the *window*, the furthest point from the centre the
            // change swings and the one a user is most likely watching.
            let at = Point::new(frame.x0, frame.y0);
            let source = before.inverse() * at;
            ((after * source) - at).hypot()
        };

        let fine = lurch(&img, 0.01);
        assert!(
            fine < 1.0,
            "a hundredth-of-a-percent step slides the picture {fine} units at the \
             floor — that is a visible lurch on a control the hand expects to be \
             continuous"
        );

        // **The flip: whole percentage points, which is what it shipped with and
        // what the tile field beside it still uses.** Without this the assertion
        // above is "a small number is small" and would pass against the bug.
        let whole = lurch(&img, 1.0);
        assert!(
            whole > 5.0,
            "a whole-point step must lurch by several units at the floor ({whole}), \
             or this test is not measuring the thing that was reported"
        );
        // And the same whole-point step three times higher up the range, where it
        // was *not* reported: the ratio is the whole diagnosis.
        let zoomed = ondin_core::ImageRef {
            crop: ondin_core::crop_scaled(img.crop, 1.0 / 3.0),
            ..img.clone()
        };
        let high = lurch(&zoomed, 1.0);
        assert!(
            whole > high * 2.5,
            "the same step should lurch far more at the floor than high up \
             ({whole} against {high}); if it does not, the reported band has \
             another cause"
        );
    }

    /// **A picture is not a colour, and the paint helpers must stop treating it
    /// as one.**
    ///
    /// Every control on a paint row is written against `stops_of` and
    /// `with_stops`, and an image has no stops — so `stops_of` hands back a
    /// placeholder grey for the swatch to draw, and `with_stops` used to fall
    /// through to `Brush::Solid` and hand that grey *back*. The two together
    /// meant a single nudge of the opacity scrubber on an image row replaced the
    /// photograph with a grey rectangle, and typing in the hex field did the
    /// same. Reachable today: an image fill classifies as `PaintKind::Image`
    /// and its row draws both controls.
    ///
    /// The opacity is real and belongs to the sampler — peniko's `ImageSampler`
    /// carries an alpha multiplier — so it is read and written there rather than
    /// being refused.
    #[test]
    fn an_images_opacity_dims_it_rather_than_replacing_it_with_grey() {
        let brush = an_image();
        assert_eq!(alpha_of(&brush), 1.0, "a fresh image brush is opaque");

        let dimmed = with_alpha(&brush, 0.5);
        let Brush::Image(img) = &dimmed else {
            panic!("dimming an image must leave an image, got {dimmed:?}");
        };
        assert_eq!(img.sampler.alpha, 0.5, "the alpha is the sampler's");
        assert_eq!(img.image.id, ondin_core::ImageId("sha256:photo".into()));
        assert_eq!(alpha_of(&dimmed), 0.5, "and reads back");

        // Nothing about the picture moved — the fit included, since the opacity
        // scrubber has no business re-framing anything.
        let Brush::Image(before) = &brush else {
            unreachable!()
        };
        assert_eq!(img.image.fit, before.image.fit);

        // And the stop path, which is what the hex field writes through, refuses
        // rather than converting: an image has no stops to replace.
        let typed = with_stops(&brush, vec![(0.0, Color::from_rgba8(1, 2, 3, 255))]);
        assert_eq!(typed, brush, "a colour written at an image must not take");

        // §15 D686, `[S23.2-L3-06]` — and the *kind* path, which was the third
        // helper and the one with no guard at all. Asked for a linear it took
        // `stops_of`'s placeholder grey, fanned it into a two-stop fade and
        // returned a grey gradient.
        //
        // ⚠️ **All three kinds, because the early return it already had only
        // covered `PaintKind::Image`** — the one target that is not reachable from
        // the tab strip, since `PaintKind::ALL` deliberately omits it. The arm that
        // matters is every other one.
        //
        // **Flip:** delete `matches!(brush, Brush::Image(_)) ||` from `convert` and
        // this fails on **`Solid`** with `Solid(0.851, 0.851, 0.851, 1.0)` — the
        // placeholder grey. ⚠️ Predicted wrongly twice over: the prediction named
        // `Linear` and a *gradient*, on the strength of the finding's own headline.
        // The loop starts at `Solid`, and `default_brush`'s `Solid` arm returns the
        // first stop rather than building anything — so the photograph comes back
        // as a flat grey swatch, which is if anything the plainer symptom.
        for kind in [PaintKind::Solid, PaintKind::Linear, PaintKind::Radial] {
            assert_eq!(
                convert(&brush, kind, Size::new(100.0, 100.0)),
                brush,
                "converting an image to {kind:?} threw the picture away"
            );
        }
    }

    /// **The mode strip changes the view and keeps the picture's framing.**
    ///
    /// The one assertion the model cannot make for itself: `crop` and
    /// `tile_scale` live beside the mode precisely so a switch can preserve them,
    /// and this is the function that has to actually do it. Written against the
    /// implementation somebody would reach for — rebuild the reference from its
    /// id and set the mode — which reads fine, passes any test that only checks
    /// the mode came out right, and quietly throws away everything the crop
    /// gesture produced.
    ///
    /// The round trip is the shape of the bug: nobody loses a crop by switching
    /// *to* Fit, they lose it by switching to Fit to look and then switching
    /// back.
    #[test]
    fn switching_framing_mode_keeps_the_crop_and_the_tile_scale() {
        use ondin_core::ImageFit;

        let mut brush = an_image();
        let Brush::Image(img) = &mut brush else {
            unreachable!()
        };
        img.image.fit = ImageFit::Crop;
        img.image.crop = ondin_core::kurbo::Rect::new(0.25, 0.1, 0.75, 0.9);
        img.image.tile_scale = 4.0;

        // Read through `image_of`, which is what the card does: `fit_of` was a
        // helper for the mode strip and went with it (§15 D268).
        let fit_of = |b: &Brush| image_of(b).map(|r| r.fit);
        assert_eq!(fit_of(&brush), Some(ImageFit::Crop));
        for away in [ImageFit::Fill, ImageFit::Fit, ImageFit::Tile] {
            let looked = with_fit(&brush, away, None);
            assert_eq!(fit_of(&looked), Some(away), "the mode did change");
            let back = with_fit(&looked, ImageFit::Crop, None);
            assert_eq!(
                back, brush,
                "a look at {away:?} and back must leave the picture exactly as it was"
            );
        }

        // Nothing to switch on a colour, and nothing to damage either.
        let solid = Brush::Solid(Color::from_rgba8(1, 2, 3, 255));
        assert_eq!(with_fit(&solid, ImageFit::Tile, None), solid);
        assert_eq!(fit_of(&solid), None);
    }

    /// **Choosing Crop from the panel must not move the picture**, which is the
    /// half of the seed that lives outside the canvas gesture.
    ///
    /// A picture nobody has cropped carries the whole source, and mapping *that*
    /// onto a 2:1 frame stretches a square photograph to twice its width — a
    /// visible jump, produced by picking the mode whose job is not to produce
    /// one. Asserted against `Fill`'s own framing rather than against a
    /// rectangle, so it stays true if `cover_crop` is ever re-derived.
    ///
    /// And the seed is for a picture that has *never* been cropped: one the user
    /// has already framed keeps what they chose, which is the same rule
    /// `with_fit` follows for a mode switch.
    #[test]
    fn choosing_crop_from_the_panel_seeds_the_picture_already_on_screen() {
        use ondin_core::{ImageFit, ImageRef, kurbo::Rect};
        let frame = Rect::new(0.0, 0.0, 200.0, 100.0);
        let (w, h) = (100, 100);

        let seeded = with_fit(&an_image(), ImageFit::Crop, Some((frame, w, h)));
        let Brush::Image(img) = &seeded else {
            unreachable!()
        };
        assert_eq!(
            img.image.framing(frame, w, h).transform,
            ImageRef::new(img.image.id.clone())
                .framing(frame, w, h)
                .transform,
            "choosing Crop drew something other than what Fill was drawing"
        );

        // Without a frame to seed from, the mode still changes — it simply does
        // not guess.
        let bare = with_fit(&an_image(), ImageFit::Crop, None);
        let Brush::Image(img) = &bare else {
            unreachable!()
        };
        assert_eq!(img.image.crop, ondin_core::whole_crop());
        assert_eq!(img.image.fit, ImageFit::Crop);

        // An authored crop is never overwritten by the seed.
        let mut chosen = an_image();
        let Brush::Image(img) = &mut chosen else {
            unreachable!()
        };
        img.image.crop = Rect::new(0.1, 0.1, 0.4, 0.4);
        let kept = with_fit(&chosen, ImageFit::Crop, Some((frame, w, h)));
        let Brush::Image(img) = &kept else {
            unreachable!()
        };
        assert_eq!(img.image.crop, Rect::new(0.1, 0.1, 0.4, 0.4));
    }

    /// **An image classifies as its own kind, and is not one of the picker's.**
    ///
    /// `PaintKind::ALL` is the picker's Solid/Linear/Radial tab strip, and an
    /// image is not something that strip can offer — you pick one from a file
    /// dialog, not from a colour wheel. So the variant exists for the *row* to
    /// branch on and is deliberately absent from `ALL`, which is what stops a
    /// fourth tab appearing that would convert a photograph to a gradient on
    /// click.
    #[test]
    fn an_image_is_its_own_kind_and_stays_out_of_the_pickers_tabs() {
        assert_eq!(kind_of(&an_image()), PaintKind::Image);
        assert!(
            !PaintKind::ALL.contains(&PaintKind::Image),
            "the picker's tab strip must not offer Image"
        );
        assert_eq!(label_of(&an_image()), "Image");
    }

    #[test]
    fn switching_kind_preserves_the_stops() {
        let solid = Brush::Solid(Color::from_rgba8(0x6D, 0x8C, 0xD9, 255));
        let linear = convert(&solid, PaintKind::Linear, BOX);
        assert_eq!(kind_of(&linear), PaintKind::Linear);
        let stops = stops_of(&linear);
        assert_eq!(stops.len(), 2, "solid fans out to a fade");
        assert_eq!(stops[0].1.components[..3], solid_components(&solid)[..3]);
        assert_eq!(stops[1].1.components[3], 0.0, "fades to transparent");

        // Linear -> radial -> linear keeps every stop.
        let radial = convert(&linear, PaintKind::Radial, BOX);
        assert_eq!(kind_of(&radial), PaintKind::Radial);
        assert_eq!(stops_of(&radial), stops);
        let back = convert(&radial, PaintKind::Linear, BOX);
        assert_eq!(stops_of(&back), stops);

        // ...and collapsing to solid keeps the first.
        let flat = convert(&radial, PaintKind::Solid, BOX);
        assert_eq!(stops_of(&flat), vec![stops[0]]);
    }

    fn solid_components(b: &Brush) -> [f32; 4] {
        match b {
            Brush::Solid(c) => c.components,
            _ => panic!("not solid"),
        }
    }

    /// **A ramp's opacity is stored, and the stops are left exactly alone** (§15
    /// D767).
    ///
    /// 🚨 **This test was `opacity_scales_a_ramp_rather_than_flattening_it` and it
    /// asserted the defect.** It drove `with_alpha(_, 0.5)` and checked the stops
    /// had *become* `[0.5, 0.0]` — proportional scaling, which is exactly the
    /// rewrite that cannot be undone through zero. It passed, and `[S23.2-L1-01]`
    /// was filed against the code it was green for, because it only ever went one
    /// way: 1.0 → 0.5 keeps a ratio, and 0.5 → 0.0 → 1.0 does not.
    ///
    /// The round trip is the assertion that matters and the old test had no round
    /// trip in it at all.
    ///
    /// **Flip, run — and the predicted site was wrong in a way worth keeping.**
    /// Restoring the scale-the-stops body in `with_alpha` alone fails at *"the field
    /// reads back"*, three assertions before the round trip: with the writer scaling
    /// stops and the reader returning `GradientBrush::opacity`, nothing writes the
    /// field, so `alpha_of` answers 1.0 after a `with_alpha(_, 0.5)`.
    ///
    /// ⚠️ **That is the model change showing up as a failure mode: the reader and
    /// the writer are now one mechanism and cannot be flipped apart.** Reproducing
    /// the old behaviour needs `alpha_of`'s gradient arm removed as well — which is
    /// exactly why the defect was hard to see before, when the "field" was a
    /// derived `max` over data the writer was free to rewrite underneath it.
    #[test]
    fn a_ramps_opacity_is_stored_and_survives_a_round_trip_through_zero() {
        let brush = default_brush(
            PaintKind::Linear,
            vec![
                (0.0, Color::from_rgba8(255, 0, 0, 255)),
                (1.0, Color::from_rgba8(0, 0, 255, 0)),
            ],
            BOX,
        );
        let ramp = |b: &Brush| {
            stops_of(b)
                .iter()
                .map(|(_, c)| c.components[3])
                .collect::<Vec<_>>()
        };
        assert_eq!(alpha_of(&brush), 1.0);
        assert_eq!(ramp(&brush), vec![1.0, 0.0], "the fixture fades out");

        let half = with_alpha(&brush, 0.5);
        assert!((alpha_of(&half) - 0.5).abs() < 1e-6, "the field reads back");
        assert_eq!(
            ramp(&half),
            vec![1.0, 0.0],
            "and the stops are untouched — the opacity is a multiplier beside \
             them, which is what makes it reversible"
        );

        // 🚨 The round trip the old test never made, and the whole finding.
        let gone = with_alpha(&half, 0.0);
        assert_eq!(alpha_of(&gone), 0.0, "0 means 0, not a floor");
        let back = with_alpha(&gone, 1.0);
        assert_eq!(
            ramp(&back),
            vec![1.0, 0.0],
            "the fade came back — scaling the stops made this `[1.0, 1.0]`, flat \
             and opaque, with no undo step to blame (§15 D767)"
        );
        assert_eq!(alpha_of(&back), 1.0);
    }

    #[test]
    fn a_fully_transparent_brush_can_be_made_opaque_again() {
        // ⚠️ **The solid arm, and it is the only one this can still be about**
        // (§15 D767). It was written when `with_alpha` divided every stop by the
        // peak and needed a fallback at zero or the brush stuck invisible; there
        // is no peak and no fallback anywhere in that function now — a solid's
        // alpha is assigned, and a gradient's and an image's are a field. The
        // assertion is kept because "0 must be reversible" is the property, and
        // `a_ramps_opacity_is_stored_and_survives_a_round_trip_through_zero` is
        // the same property one arm over.
        let brush = Brush::Solid(Color::from_rgba8(255, 0, 0, 0));
        assert_eq!(alpha_of(&with_alpha(&brush, 1.0)), 1.0);
    }

    /// **A stop alpha above 1 is normalised, and a bare click stops rescaling
    /// the ramp** (§15 D692, `[S23.2-L1-03]`).
    ///
    /// The route is a document already on disk: D692 clamps the importer, but a
    /// file saved before it — or any hand-written one — still loads a stop at
    /// 5.0, so `alpha_of` has to be sound on input it did not produce.
    ///
    /// Two harms, and they need separate assertions. The field was handed
    /// **500** into a `range(0.0..=100.0)`; and because `with_alpha` divides
    /// every stop by `alpha_of`'s answer, one click at 100% rescaled the whole
    /// ramp by 1/5 and took a 25% stop to **5%** — a visible change to the
    /// picture from a click that typed nothing.
    ///
    /// ⚠️ **The fixture assertion is load-bearing.** If `default_brush` were to
    /// clamp on the way in, every later assertion would pass without the
    /// function under test doing anything at all.
    ///
    /// 🚨 **§15 D767 answered both harms at the root and changed what this test
    /// can assert.** `alpha_of` no longer folds the stops for a gradient — it
    /// returns `GradientBrush::opacity`, a `#[serde(default)]` field that
    /// `build::brush_is_finite` range-checks with `valid_opacity` and that this
    /// function clamps on the way out. So the field cannot be handed 500 by a stop
    /// at all, rather than being handed it and clamping.
    ///
    /// And the ramp can no longer be rescaled by a click, because `with_alpha`
    /// does not touch stops: the second harm is not mitigated, it is **unreachable**.
    ///
    /// ⚠️ **What is genuinely lost is an accidental repair, and it is recorded
    /// rather than mourned.** Clicking the field used to *normalise* the stored
    /// out-of-range stop back to 1.0 — a side effect of dividing by the peak, not a
    /// designed behaviour. It no longer happens, so a hand-written `stop-opacity="5"`
    /// stays 5.0 in the document. That is the state D692 already described as
    /// reachable *"from a file saved before it"*, `brush_is_finite` accepts it (it
    /// checks finiteness, not range), and the raster clamps it — so nothing the user
    /// sees changed. **It is a stop-validation gap, and it always was; the opacity
    /// field was never the right place to fix it.**
    ///
    /// **Flip, run:** dropping the gradient arm from `alpha_of` sends it back to the
    /// stop fold and fails the second assertion with `left: 1.0` against the
    /// fixture's own peak — which is the assertion that says the field reads a field.
    #[test]
    fn a_stop_alpha_above_one_no_longer_reaches_the_opacity_field() {
        let brush = default_brush(
            PaintKind::Linear,
            vec![
                (0.0, Color::from_rgba8(255, 0, 0, 255).with_alpha(5.0)),
                (1.0, Color::from_rgba8(0, 0, 255, 255).with_alpha(0.25)),
            ],
            BOX,
        );
        assert_eq!(
            stops_of(&brush)[0].1.components[3],
            5.0,
            "the fixture must reach the state: an out-of-range stop really is stored"
        );

        assert_eq!(
            alpha_of(&brush),
            1.0,
            "the opacity field reads the ramp's own multiplier, so a rogue stop \
             cannot reach it — 500 was never in range and is now not in the path"
        );

        // A bare click at the value the field is already showing. It used to
        // rescale the whole ramp by 1/5; now it is a no-op on the stops.
        let clicked = with_alpha(&brush, alpha_of(&brush));
        let stops = stops_of(&clicked);
        assert_eq!(
            stops[0].1.components[3], 5.0,
            "the click left the stops alone — rescaling them is the defect §15 \
             D767 removed, and normalising them was only ever its side effect"
        );
        assert!(
            (stops[1].1.components[3] - 0.25).abs() < 1e-6,
            "and the in-range stop is untouched either way, got {}",
            stops[1].1.components[3]
        );
    }

    /// **The negative direction, measured rather than reasoned** — and it
    /// corrects `[S23.2-L1-03]`'s account of it.
    ///
    /// The finding says `stop-opacity="-1"` gives `alpha_of == -1.0`, and it
    /// cannot: the fold is seeded at `0.0` and `f32::max(0.0, -1.0)` is `0.0`,
    /// so the raw function already floored a negative peak before D692's clamp
    /// was there. The *consequence* the finding names is real and unchanged —
    /// a zero peak takes `with_alpha`'s non-positive fallback and every stop
    /// goes to the requested alpha — so the verdict stood and one step of the
    /// mechanism did not. Asserted here so the next reader does not re-derive
    /// it from the finding's number.
    /// ⚠️ **§15 D767 made this a question about a different quantity.** `alpha_of`
    /// reads `GradientBrush::opacity` for a gradient now, so a negative *stop* alpha
    /// never reaches it by any route — where before it was floored by the fold's
    /// `0.0` seed. The finding's *consequence* — a zero peak taking `with_alpha`'s
    /// fallback and flattening every stop — is gone with the fallback itself.
    ///
    /// The assertion is kept and turned around: the field is independent of the
    /// stops, which is the property that makes the flatten unreachable. **A test
    /// that simply deleted itself here would leave nothing saying a rogue stop
    /// cannot move the opacity control.**
    #[test]
    fn a_negative_stop_alpha_cannot_move_the_opacity_field() {
        let brush = default_brush(
            PaintKind::Linear,
            vec![
                (0.0, Color::from_rgba8(255, 0, 0, 255).with_alpha(-1.0)),
                (1.0, Color::from_rgba8(0, 0, 255, 255).with_alpha(-1.0)),
            ],
            BOX,
        );
        assert_eq!(
            alpha_of(&brush),
            1.0,
            "the ramp's multiplier is untouched by what its stops say; the \
             finding's -1.0 was never reachable and now neither is 0.0"
        );
    }

    #[test]
    fn linear_angle_round_trips_and_spans_the_box() {
        let brush = default_brush(
            PaintKind::Linear,
            vec![(0.0, Color::BLACK), (1.0, Color::WHITE)],
            BOX,
        );
        for deg in [0.0, 45.0, 90.0, 135.0, 200.0, 315.0] {
            let aimed = with_linear_angle(&brush, deg, BOX);
            let got = linear_angle(&aimed).expect("linear");
            assert!((got - deg).abs() < 1e-6, "{deg} -> {got}");
            // Centred on the box, whatever the angle.
            let Brush::Gradient(g) = &aimed else {
                panic!("gradient")
            };
            let GradientKind::Linear(pos) = g.gradient.kind else {
                panic!("linear")
            };
            let mid = pos.start.midpoint(pos.end);
            assert!((mid.x - 100.0).abs() < 1e-6 && (mid.y - 50.0).abs() < 1e-6);
        }
    }

    #[test]
    fn radial_centre_round_trips_as_a_fraction_of_the_box() {
        let brush = default_brush(
            PaintKind::Radial,
            vec![(0.0, Color::WHITE), (1.0, Color::BLACK)],
            BOX,
        );
        assert_eq!(radial_centre(&brush, BOX), Some((0.5, 0.5)));
        let moved = with_radial_centre(&brush, 0.25, 0.75, BOX);
        let (cx, cy) = radial_centre(&moved, BOX).expect("radial");
        assert!((cx - 0.25).abs() < 1e-9 && (cy - 0.75).abs() < 1e-9);
    }

    /// **Moving the centre translates the focal point with it** (§15 D413), which
    /// `with_radial_centre` did not do until 2026-09-03: it wrote the new centre
    /// into `start_center` *and* `end_center`, so an imported highlight snapped to
    /// dead centre the first time anyone nudged the control, and nothing said so.
    ///
    /// ⚠️ **No gradient this app authors could ever have shown it.**
    /// `Gradient::new_radial` puts both centres on the same point, so the offset is
    /// zero for everything `default_brush` makes and the bug is unreachable from any
    /// fixture built the way the test above builds one. **The fixture has to come
    /// from the other direction** — a gradient with a focal point, which in practice
    /// means an import — and building one by hand here is what makes the case
    /// reachable at all. That is this session's fifth instance of a rule needing a
    /// fixture chosen to reach it.
    ///
    /// The offset is asserted **relative**, not absolute: what a focal point means is
    /// "ten left and five up from the centre", and a test pinning it to a coordinate
    /// would pass a version that moved the pair by the wrong amount in the same
    /// direction.
    ///
    /// **Flipped**: restoring `start_center: centre` fails the last assertion with
    /// `Vec2 { 0.0, 0.0 }` against `-10, -5` — the collapse itself, named. ⚠️ **And
    /// `radial_centre_round_trips_as_a_fraction_of_the_box` stays green through that
    /// flip**, which is the more useful half: the older test builds its fixture with
    /// `default_brush`, whose focal offset is zero, so it agrees with the buggy
    /// version and the correct one alike. *A test cannot see a field its fixture
    /// leaves at the default.*
    #[test]
    fn moving_a_radial_centre_carries_its_focal_point_along() {
        use ondin_core::peniko::{Gradient, RadialGradientPosition};

        let mut gradient = Gradient::new_radial((100.0, 50.0), 40.0);
        // A highlight up and to the left of the centre — a lit sphere, and the one
        // thing about a radial gradient that this control used to destroy.
        gradient.kind = GradientKind::Radial(RadialGradientPosition {
            start_center: Point::new(90.0, 45.0),
            start_radius: 0.0,
            end_center: Point::new(100.0, 50.0),
            end_radius: 40.0,
        });
        let brush = Brush::Gradient(gradient.into());

        let offset_of = |b: &Brush| {
            let Brush::Gradient(g) = b else {
                panic!("gradient")
            };
            let GradientKind::Radial(p) = g.gradient.kind else {
                panic!("radial")
            };
            p.start_center - p.end_center
        };
        assert_eq!(
            offset_of(&brush),
            Vec2::new(-10.0, -5.0),
            "the fixture is lit"
        );

        let moved = with_radial_centre(&brush, 0.25, 0.75, BOX);
        assert_eq!(
            radial_centre(&moved, BOX).expect("radial"),
            (0.25, 0.75),
            "the centre still goes where it was asked to"
        );
        assert_eq!(
            offset_of(&moved),
            Vec2::new(-10.0, -5.0),
            "and the highlight is still up and to the left of it, by the same amount"
        );
    }

    /// **A singular brush transform must not be inverted, and the loss is a file
    /// that never opens again** (§15 D713).
    ///
    /// `_GRADIENT_GEOMETRY_IS_IN_SHAPE_SPACE` argues the inverse is safe because
    /// `svg_in` bounds the determinant away from zero. That is true of `svg_in`
    /// and the field has a **second** producer: `GradientBrush::transform` is
    /// `#[serde(default)]` with no validation, so any six numbers in a `.ondin`
    /// become the field. `Affine::inverse` divides by the determinant, so a
    /// singular one gives `NaN` in every coefficient, `serde_json` writes `NaN`
    /// as `null`, and the next load answers *"invalid type: null, expected f64"*.
    /// One bare click in the `CX` field is the whole gesture.
    ///
    /// **The loss is asserted before the mechanism**: what matters is that the
    /// geometry stays representable, not which guard refused it.
    ///
    /// ⚠️ **The fixture has to be checked into the state the test names.** A
    /// transform that is merely *unusual* is invertible and both writers are
    /// right to use it, so the first assertion pins `determinant() == 0.0` —
    /// without it this test passes against a fixture that never reaches the bug.
    #[test]
    fn a_singular_gradient_transform_is_refused_rather_than_inverted() {
        use ondin_core::kurbo::Affine;

        // Rank-1: the whole plane collapses onto a line. Every coefficient is
        // finite, which is why `build::affine_is_finite` never sees it.
        let singular = Affine::new([1.0, 2.0, 2.0, 4.0, 0.0, 0.0]);
        assert_eq!(
            singular.determinant(),
            0.0,
            "the fixture is singular, or this test is about nothing"
        );

        let radial = {
            let mut g: ondin_core::GradientBrush = Gradient::new_radial((50.0, 50.0), 40.0).into();
            g.transform = singular;
            Brush::Gradient(g)
        };
        let linear = {
            let mut g: ondin_core::GradientBrush =
                Gradient::new_linear((0.0, 0.0), (100.0, 0.0)).into();
            g.transform = singular;
            Brush::Gradient(g)
        };

        let finite = |b: &Brush, what: &str| {
            let Brush::Gradient(g) = b else {
                panic!("gradient")
            };
            let pts: Vec<Point> = match g.gradient.kind {
                GradientKind::Radial(p) => vec![p.start_center, p.end_center],
                GradientKind::Linear(p) => vec![p.start, p.end],
                GradientKind::Sweep(_) => vec![],
            };
            assert!(
                pts.iter().all(|p| p.x.is_finite() && p.y.is_finite()),
                "{what} wrote unrepresentable geometry: {pts:?} — this document \
                 saves and never loads again"
            );
        };

        finite(
            &with_radial_centre(&radial, 0.25, 0.75, BOX),
            "the radial writer",
        );
        finite(&with_linear_angle(&linear, 45.0, BOX), "the linear writer");
    }

    #[test]
    fn sampling_hits_the_stops_and_interpolates_between_them() {
        let stops = vec![
            (0.0, Color::from_rgba8(0, 0, 0, 255)),
            (1.0, Color::from_rgba8(255, 255, 255, 255)),
        ];
        assert_eq!(sample(&stops, 0.0).components, stops[0].1.components);
        assert_eq!(sample(&stops, 1.0).components, stops[1].1.components);
        let mid = sample(&stops, 0.5).components;
        assert!((mid[0] - 0.5).abs() < 1e-6);
        // Outside the stop range clamps rather than extrapolating.
        assert_eq!(sample(&stops, -1.0).components, stops[0].1.components);
        assert_eq!(sample(&stops, 2.0).components, stops[1].1.components);
    }

    #[test]
    fn stops_are_kept_sorted_so_the_ramp_never_folds_back() {
        let brush = default_brush(PaintKind::Linear, vec![(0.0, Color::BLACK)], BOX);
        let messy = with_stops(
            &brush,
            vec![
                (0.9, Color::WHITE),
                (0.1, Color::BLACK),
                (0.5, Color::from_rgba8(1, 2, 3, 255)),
            ],
        );
        let offsets: Vec<f32> = stops_of(&messy).iter().map(|s| s.0).collect();
        assert_eq!(offsets, vec![0.1, 0.5, 0.9]);
    }

    #[test]
    fn reversing_mirrors_the_offsets() {
        let brush = default_brush(
            PaintKind::Linear,
            vec![
                (0.0, Color::BLACK),
                (0.25, Color::from_rgba8(1, 2, 3, 255)),
                (1.0, Color::WHITE),
            ],
            BOX,
        );
        let back = reverse_stops(&brush);
        let offsets: Vec<f32> = stops_of(&back).iter().map(|s| s.0).collect();
        assert_eq!(offsets, vec![0.0, 0.75, 1.0]);
        assert_eq!(stops_of(&back)[0].1.components, Color::WHITE.components);
    }
}

#[cfg(test)]
mod image_alpha_tests {
    //! §15 D784 — the one rule both picture chips read.
    //!
    //! (Plain backticks per §15 D319.)

    use super::*;

    fn faded(alpha: f32) -> Brush {
        let Brush::Image(mut img) = ondin_core::image_brush(ondin_core::ImageId("b".repeat(64)))
        else {
            unreachable!("image_brush builds an image brush")
        };
        img.sampler.alpha = alpha;
        Brush::Image(img)
    }

    /// The arm that exists so a caller holding any brush can ask without matching.
    ///
    /// ⚠️ **`1.0` for a gradient is not an oversight**: a gradient's own fade is
    /// `GradientBrush::opacity` and `picker::ramp` folds that one in. This
    /// function answers *"how faded is the picture"*, and a brush with no picture
    /// has no answer but the identity.
    #[test]
    fn a_brush_with_no_picture_is_the_identity_multiplier() {
        assert_eq!(image_alpha(&Brush::Solid(Color::BLACK)), 1.0);
        assert_eq!(image_alpha(&faded(0.25)), 0.25);
    }

    #[test]
    fn the_clamp_holds_at_both_ends() {
        assert_eq!(image_alpha(&faded(-3.0)), 0.0);
        assert_eq!(image_alpha(&faded(1e30)), 1.0);
        assert_eq!(image_alpha(&faded(0.5)), 0.5, "and leaves the range alone");
    }
}

#[cfg(test)]
mod reorder_tests {
    use super::*;

    /// Three slots 28px tall at a 40px pitch, as a paint list lays them out.
    fn slots() -> Slots {
        let mut s = Slots::default();
        for top in [0.0_f32, 40.0, 80.0] {
            s.push(top, top + 28.0);
        }
        s
    }

    /// The gap the pointer is in: 0 above everything, `len` below it, and a slot is
    /// claimed once the pointer passes its middle.
    #[test]
    fn the_pointers_gap_is_counted_from_the_top() {
        let s = slots();
        assert_eq!(s.gap(-5.0), 0, "above every slot");
        assert_eq!(s.gap(10.0), 0, "in the first slot's upper half");
        assert_eq!(s.gap(20.0), 1, "past the first slot's middle");
        assert_eq!(s.gap(60.0), 2, "past the second's");
        assert_eq!(s.gap(500.0), 3, "below every slot");
    }

    /// **Top of the stack first**: the last element is the last painted, so it is
    /// the top row.
    #[test]
    fn the_display_order_puts_the_last_paint_on_top() {
        assert_eq!(display_order(3, None), vec![2, 1, 0]);
        assert_eq!(display_order(0, None), Vec::<usize>::new());
        // A carried entry is shown where it would land, so the order is the result.
        assert_eq!(display_order(3, Some((0, 2))), vec![0, 2, 1]);
        assert_eq!(display_order(3, Some((2, 0))), vec![1, 0, 2]);
        // Nothing moved, and out-of-range indices, both fall back to the base order.
        assert_eq!(display_order(3, Some((1, 1))), vec![2, 1, 0]);
        assert_eq!(display_order(3, Some((9, 0))), vec![2, 1, 0]);
    }

    /// **A live reorder has to settle.** The landing is a pure function of where the
    /// pointer is and where the entry is *shown*, so with the pointer still the
    /// answer is the slot the entry is already in and nothing moves. Were it an
    /// accumulation, or were either off-by-one wrong, the rows would flip back and
    /// forth every frame — which is the failure this pins.
    #[test]
    fn a_live_reorder_settles_where_the_pointer_is() {
        // Three paints. Display slots run 0,1,2 top-down; model indices run the
        // other way, so model 0 starts shown at slot 2.
        let n = 3;
        let mut to = 0_usize; // carrying model 0, still in place
        let shown = |to: usize| n - 1 - to;

        // Pointer in the middle slot's upper half: gap 1. The entry is at slot 2, so
        // taking it out does not close that gap — it lands at slot 1, model 1.
        to = landing(n, shown(to), 1);
        assert_eq!(to, 1, "should move up one");
        // Pointer unchanged: it is already there, so it stays.
        for _ in 0..3 {
            to = landing(n, shown(to), 1);
            assert_eq!(to, 1, "a still pointer must not keep moving it");
        }

        // All the way to the top.
        to = landing(n, shown(to), 0);
        assert_eq!(to, 2, "the top slot is the last model index");
        to = landing(n, shown(to), 0);
        assert_eq!(to, 2, "and it settles there");

        // All the way to the bottom: gap 3 with the entry shown at slot 0, so the
        // gap closes up by one and it lands at slot 2, model 0.
        to = landing(n, shown(to), 3);
        assert_eq!(to, 0, "the bottom slot is model 0");
        to = landing(n, shown(to), 3);
        assert_eq!(to, 0, "and it settles there too");
    }

    /// `reordered` is the list edit the drop applies. Its off-by-one is the mirror
    /// of `landing`'s: an index taken against the full list, used against the
    /// list with the carried item already out of it.
    #[test]
    fn reordering_moves_one_paint_and_keeps_the_rest_in_order() {
        let items = ['a', 'b', 'c', 'd'];
        assert_eq!(reordered(&items, 0, 3), vec!['b', 'c', 'd', 'a']);
        assert_eq!(reordered(&items, 3, 0), vec!['d', 'a', 'b', 'c']);
        assert_eq!(reordered(&items, 1, 2), vec!['a', 'c', 'b', 'd']);
        assert_eq!(reordered(&items, 2, 2), items.to_vec());
        // Out of range is a no-op rather than a panic: the list can shrink between
        // the frame a drag started on and the frame it is resolved on.
        assert_eq!(reordered(&items, 9, 0), items.to_vec());
    }

    /// The two halves compose: what the pointer says, through `display_order`, must
    /// put the carried entry in the slot the pointer is over — and `reordered` on
    /// the model list must agree with it.
    #[test]
    fn the_shown_order_and_the_committed_order_agree() {
        let n = 4;
        let fills = ['a', 'b', 'c', 'd']; // model order, 'd' on top
        for gap in 0..=n {
            let from = 1_usize;
            let to = landing(n, n - 1 - from, gap);
            // What the panel draws while carrying...
            let shown: Vec<char> = display_order(n, Some((from, to)))
                .into_iter()
                .map(|i| fills[i])
                .collect();
            // ...and what the release commits, read top-down to compare.
            let committed: Vec<char> = reordered(&fills, from, to).into_iter().rev().collect();
            assert_eq!(
                shown, committed,
                "gap {gap}: the panel showed {shown:?} but would commit {committed:?}"
            );
        }
    }

    /// **Rows of different heights.** A stroke showing its sides selector is four
    /// rows tall where the one beside it is two, so the slots are genuinely uneven —
    /// and a drop has to resolve against each slot's *own* middle rather than against
    /// a pitch. `Slots` stores real spans precisely so this needs no special case;
    /// this is the assertion that it does.
    ///
    /// Stated as the visible outcome: dragging the short stroke down past the tall
    /// one has to take until the *tall* one's midpoint, not until one short row's
    /// worth of travel.
    #[test]
    fn a_drop_resolves_against_each_slots_own_middle_however_tall_it_is() {
        // Slot 0 is a plain stroke (two 28px rows, 6px apart = 62), slot 1 is one
        // showing its sides selector and four custom widths (four rows = 130).
        let mut s = Slots::default();
        s.push(0.0, 62.0);
        s.push(71.0, 201.0);
        s.push(210.0, 272.0);

        // The tall slot's middle is at 136, not at 71 + 31.
        assert_eq!(s.gap(100.0), 1, "still in the tall slot's upper half");
        assert_eq!(s.gap(130.0), 1, "and still, most of the way down it");
        assert_eq!(s.gap(140.0), 2, "past its own middle at 136");
        // A pitch-based reading would have claimed it at 71 + 31 = 102, so this is
        // the pair of assertions that tells the two apart.
        assert_ne!(
            s.gap(110.0),
            2,
            "a tall slot must not be claimed a short row in"
        );
        // The ends still behave.
        assert_eq!(s.gap(-5.0), 0);
        assert_eq!(s.gap(500.0), 3);
    }

    /// And the live reorder still settles over uneven slots — the property that
    /// matters, since an oscillation is what the user would actually see.
    #[test]
    fn a_live_reorder_settles_over_slots_of_different_heights() {
        let mut s = Slots::default();
        s.push(0.0, 62.0);
        s.push(71.0, 201.0);
        s.push(210.0, 272.0);
        let n = 3;
        let shown = |to: usize| n - 1 - to;

        // Carrying model 2 (the top slot), pointer deep in the tall middle slot.
        let mut to = 2_usize;
        for _ in 0..4 {
            let next = landing(n, shown(to), s.gap(140.0));
            assert_eq!(next, to.min(1), "a still pointer must not keep moving it");
            to = next;
        }
        assert_eq!(to, 1, "it landed in the middle slot and stayed");
    }

    /// **The chrome's renumbering is the list's renumbering.** Stated as a property
    /// against `reordered` itself rather than as a table of expected indices,
    /// because a table is where an off-by-one hides: the claim is that following
    /// `reordered_index` to a new slot finds the same paint there, and that is
    /// checkable by moving labelled items about.
    #[test]
    fn a_reorder_renumbers_the_chrome_the_way_it_renumbers_the_list() {
        let items = ["a", "b", "c", "d"];
        for from in 0..items.len() {
            for to in 0..items.len() {
                let after = reordered(&items, from, to);
                for i in 0..items.len() {
                    let landed = reordered_index(items.len(), from, to, i).unwrap();
                    assert_eq!(
                        after[landed], items[i],
                        "moving {from}->{to}, item {i} ({}) claimed slot {landed} of {after:?}",
                        items[i]
                    );
                }
            }
        }
        assert_eq!(reordered_index(2, 0, 0, 5), None, "out of range");
        assert_eq!(reordered_index(0, 0, 0, 0), None, "an empty list");
    }

    /// A removal drops what belonged to the paint that went and brings the rest
    /// down — the same claim, checked the same way.
    #[test]
    fn a_removal_brings_the_chrome_above_it_down_one() {
        let items = ["a", "b", "c"];
        for gone in 0..items.len() {
            let mut after = items.to_vec();
            after.remove(gone);
            for (i, item) in items.iter().enumerate() {
                match removed_index(gone, i) {
                    None => assert_eq!(i, gone, "only the removed one loses its key"),
                    Some(landed) => assert_eq!(&after[landed], item),
                }
            }
        }
    }
}
