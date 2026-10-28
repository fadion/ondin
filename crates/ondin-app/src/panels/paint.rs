//! The paint model as the inspector and the colour picker see it (§9.2).
//!
//! Mostly pure `Brush` in, `Brush` out, and **no document**. Both the inspector's
//! fill/stroke lists and the detached picker edit the same brushes, and the rules
//! about *how* an edit rewrites a brush that live here are what stop those two
//! drifting: switching kind preserves the stops, a linear gradient's angle is
//! stored as the two endpoints the model actually holds, and a picture's alpha is
//! read in one place ([`image_alpha`]).
//!
//! Not every rule is here, and the module is not egui-free ([`CharWrite`] holds an
//! `egui::Response`; [`PaintDrag`] and [`Slots`] carry `f32` screen spans). The hex
//! edit's rules (which alpha goes out with a typed colour, whether an unchanged
//! colour is written, whether an unrepresentable one is quantised) live with the
//! hex field; this module only offers [`with_stops`], which takes the alpha as an
//! argument.
//!
//! Gradient coordinates are in the node's **local geometry box**, which is why
//! every constructor takes `box_size`: a gradient built at the origin would sit
//! off the shape entirely.

use ondin_core::Brush;
use ondin_core::NodeId;
use ondin_core::kurbo::{Affine, Point, Size, Vec2};
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
/// Paint order matters (translucent fills stack differently either way round), so
/// the list order is editable by a grip on the row.
///
/// `PartialEq` exists only for `app::InFlight::any`, which compares the whole struct
/// against `Default`. Never compare two live drags: `slots` is last frame's pixels.
#[derive(PartialEq)]
pub struct PaintDrag {
    /// The node the panel is anchored on; the drag is dropped when this stops
    /// matching what the panel shows, since the indices below would mean nothing.
    pub anchor: NodeId,
    pub list: PaintList,
    /// The index in the model's `Vec` of the paint being carried.
    pub from: usize,
    /// Where it will land as a model index, and (the reorder being live) also where
    /// it is currently shown.
    pub to: usize,
    /// The vertical span of each display slot, top-first, as the panel last laid
    /// them out. Carried across frames because the target must be known before the
    /// rows are laid out; exact, since reordering contents does not move the slots.
    pub slots: Slots,
}

/// An Effects row being dragged up or down its card: [`PaintDrag`] for a list drawn
/// in model order, first at the top ([`forward_order`], [`forward_landing`]).
#[derive(PartialEq)]
pub struct EffectDrag {
    /// The layer the card is anchored on; the drag is dropped when it stops matching.
    pub anchor: NodeId,
    pub from: usize,
    pub to: usize,
    pub slots: Slots,
}

/// The model indices to draw a top-first list in, with `moved`'s entry shown at
/// its landing.
pub fn forward_order(n: usize, moved: Option<(usize, usize)>) -> Vec<usize> {
    let base: Vec<usize> = (0..n).collect();
    match moved {
        Some((from, to)) if from != to && from < n && to < n => reordered(&base, from, to),
        _ => base,
    }
}

/// [`landing`] for a top-first list, where a display slot is a model index.
pub fn forward_landing(n: usize, shown_at: usize, gap: usize) -> usize {
    let landing = if gap > shown_at { gap - 1 } else { gap };
    landing.min(n.saturating_sub(1))
}

/// Where each display slot of a list landed, top-first.
///
/// Slot-based rather than keyed by item: contents move while slots stay put, so a
/// drop resolves against stable geometry without feeding back into layout. Real
/// spans rather than a pitch, so uneven rows (a stroke is two rows, a fill one) need
/// no special case. `PartialEq` is for [`PaintDrag`]'s.
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

/// The model indices of a list in draw order, top row first, with a carried entry
/// moved to where it would land. The last element is painted last, so it is the top.
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
/// Two off-by-one conversions make a live reorder settle instead of oscillate: the
/// entry is already in the list at `shown_at`, so a gap below it is one too many
/// (taking it out closes the gap); and slots count from the top while the model
/// counts from the bottom. A pure function of pointer and shown position, so a still
/// pointer leaves the entry where it is.
pub fn landing(n: usize, shown_at: usize, gap: usize) -> usize {
    if n == 0 {
        return 0;
    }
    let landing = if gap > shown_at { gap - 1 } else { gap };
    n - 1 - landing.min(n - 1)
}

/// Reorder `items` so that `from` lands at `to`, both model indices.
///
/// Removing the carried item shifts everything after it down, so the insert index
/// is taken against the shortened list.
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
/// [`reordered`] read the other way round, for the chrome: an open options popup,
/// sides row, picker and half-typed dash list are keyed by list index, so a reorder
/// must renumber them or each describes whichever paint inherited its number.
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
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PaintKind {
    Solid,
    Linear,
    Radial,
    /// A sweep (conic) gradient, labelled *Angular*.
    Sweep,
    /// A picture. Deliberately not a picker kind: the others convert into one
    /// another, an image is picked from a file dialog, and offering it would turn a
    /// photograph into a gradient on click.
    Image,
}

impl PaintKind {
    /// The gradient kinds, in the order the picker's kind menu lists them. Each
    /// converts to every other and to Solid.
    pub const GRADIENTS: [PaintKind; 3] = [PaintKind::Linear, PaintKind::Radial, PaintKind::Sweep];

    pub fn is_gradient(self) -> bool {
        Self::GRADIENTS.contains(&self)
    }

    pub fn label(self) -> &'static str {
        match self {
            PaintKind::Solid => "Solid",
            PaintKind::Linear => "Linear",
            PaintKind::Radial => "Radial",
            PaintKind::Sweep => "Angular",
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
    /// The canvas ground behind and around every frame: a property of the view, with
    /// no node behind it. The node-less slots ride `PaintSlot` because the swatch,
    /// picker and hex field are all written against a slot.
    Canvas,
    /// The selected ruler guides' colour: a write emits one `SetGuideColor` per
    /// selected guide, and the row reports `Swatch::Mixed` where they disagree.
    Guide,
    /// One row of the fill (or stroke) list every paint target in the selection
    /// shares: the same index in all of them at once.
    Selection(ondin_core::PaintTarget, usize),
    /// Every fill (or stroke) of every paint target in the selection, at once; the
    /// slot the mixed row edits.
    ///
    /// The write is a property edit, not a replacement: `build::edit_fills_all`
    /// leaves each target's list length, order and untouched properties alone. The
    /// row states a value only where they agree (`build::shared_over_fills`).
    SelectionAll(ondin_core::PaintTarget),
    /// The colour of a text node's underline or strikethrough: a character
    /// attribute, not an entry in `fills` or `strokes`.
    ///
    /// Its write may not be a document transaction: while a live editing session
    /// owns a selection it goes into the session's spans (§9.3), so all three slot
    /// verbs divert it; see [`PaintSlot::char_scoped`].
    TextDecoration(DecorationSide),
    /// The colour of the text itself over the range a character control acts on (a
    /// run colour). No span means the run draws the node's fill stack, so the row's
    /// swatch can show a gradient (`TypeSubject::text_ramp`) where this slot holds
    /// one colour.
    TextColor,
    /// One colour of the Group Colors panel: every fill and stroke in the selection
    /// carrying this exact colour.
    ///
    /// Keyed by the committed RGBA, so it stays valid for a whole drag (the document
    /// is unchanged until release) and is retargeted once the edit commits.
    GroupColor([u8; 4]),
    /// One gradient of the Group Colors panel: every use of that exact gradient in
    /// the selection.
    ///
    /// Keyed by a place, since a gradient's identity (stops and geometry) does not
    /// fit a `Copy + Eq` slot: the first place the census found it
    /// (`build::GradientUse::at`), and `build::repaint` changes every use. A place
    /// still holds the paint after the edit, so no retargeting is needed.
    GroupGradient(ondin_core::PaintAt),
    /// One effect's colour (a drop or inner shadow's), by index in the layer's
    /// effect stack (§5.3a).
    ///
    /// The alpha here is the shadow's opacity with no second spelling, so the row is
    /// the Fill row's (chip, hex, percentage). A stale index reads back `None` from
    /// `slot_brush` and closes the picker, as for every index-keyed slot.
    Effect(usize),
    /// One layout grid's colour, by index in the frame's list (`ondin_core::layout`).
    ///
    /// The only slot whose paint is chrome rather than ink: `RenderOverrides` cannot
    /// preview it (`Operation::SetLayoutGrids` is a no-op there), so the preview forks
    /// the way a guide's does (`OndinApp::preview_grid`). The alpha is the grid's own
    /// translucency, as [`PaintSlot::Effect`]'s is a shadow's opacity. A stale index
    /// reads back `None` from `slot_brush` and closes the picker.
    Grid(usize),
    /// A colour variable's value in one mode (a cell of the styles and variables
    /// browser). Solid only, alpha kept; the picker's *Variables* face makes the
    /// cell an alias where a layer's makes a binding.
    VarCell(ondin_core::VarId, ondin_core::ModeId),
}

/// Which of a text node's two decorations a [`PaintSlot::TextDecoration`] names.
///
/// Its own enum rather than a `CharAttrKind`, which would let the slot name `Size`.
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
/// Every other slot ends in `slot_transaction`; these cannot, because while a live
/// editing session owns a selection the write belongs in that session's spans (§9.3).
/// `OndinApp::write_char_slot` is the single route all three slot verbs divert to.
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

    /// The `PaintSlot` that names this; the round trip of [`PaintSlot::char_scoped`].
    pub fn paint_slot(self) -> PaintSlot {
        match self {
            CharSlot::Decoration(side) => PaintSlot::TextDecoration(side),
            CharSlot::Color => PaintSlot::TextColor,
        }
    }

    /// What the swatch says on hover, given whether the slot holds its own colour
    /// and whether what it inherits is a gradient.
    ///
    /// The inherited state reads differently per slot: a decoration inherits the
    /// text's ink, the text inherits the layer's fill.
    pub fn swatch_tooltip(self, has_own: bool, inherits_gradient: bool) -> &'static str {
        match (self, has_own, inherits_gradient) {
            (_, true, _) => "Edit this colour",
            // Taking a colour of its own cannot keep the gradient:
            // `write_char_slot` collapses one to its first stop.
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
/// Named rather than an `Option<&Response>`: the third state is "a response, and do
/// not commit on it".
#[derive(Clone, Copy)]
pub enum CharWrite<'a> {
    /// A click, or a hex typed and committed: write it now.
    Commit,
    /// A drag: preview each frame, commit once on release.
    Valve(&'a egui::Response),
    /// A pointer down on a colour control that has not travelled far enough to be a
    /// drag: show it, commit nothing.
    Preview,
}

impl PaintSlot {
    /// The character-scoped slot this is, or `None` for the ones whose write is an
    /// ordinary transaction.
    ///
    /// The one predicate all three slot verbs ask, so none can forget a slot the
    /// others divert.
    pub fn char_scoped(self) -> Option<CharSlot> {
        match self {
            PaintSlot::TextDecoration(side) => Some(CharSlot::Decoration(side)),
            PaintSlot::TextColor => Some(CharSlot::Color),
            _ => None,
        }
    }

    /// Whether this slot can hold a gradient.
    ///
    /// This answers whether the picker's gradient tabs are live (they are shown
    /// disabled otherwise), not whether they are drawn. The canvas ground is one
    /// clear colour and a guide a hairline; a Group Colors row sits in a list of
    /// solid colours; `Decoration::color`, `Shadow::color` and `LayoutGrid::color` are
    /// one `Color` (grids are flat polygons in `CanvasRenderer::draw_layout_grids`);
    /// and a run colour is a model decision, since a ramp pinned to a reflowing run
    /// box would move as you type (`TextStyle::color` is a `Color`).
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
                | PaintSlot::VarCell(..)
        )
    }

    /// Whether this slot has an opacity worth showing. The canvas ground has nothing
    /// behind it to be transparent against, and a guide faded to nothing is a deleted
    /// guide.
    pub fn takes_alpha(self) -> bool {
        !matches!(self, PaintSlot::Canvas | PaintSlot::Guide)
    }
}

pub fn kind_of(brush: &Brush) -> PaintKind {
    match brush {
        Brush::Gradient(g) => match g.gradient.kind {
            GradientKind::Radial(_) => PaintKind::Radial,
            GradientKind::Linear(_) => PaintKind::Linear,
            GradientKind::Sweep(_) => PaintKind::Sweep,
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
/// Both picture chips fold this in: the inspector's paint-row chip
/// ([`crate::ui::Swatch::Picture`]) from a `&Brush`, and the layers panel's row from
/// an `&ImageBrush` via [`image_brush_alpha`], which holds the one clamp. Not folded
/// into [`stops_of`]'s picture arm, a placeholder grey for an undecodable picture.
pub fn image_alpha(brush: &Brush) -> f32 {
    match brush {
        Brush::Image(img) => image_brush_alpha(img),
        _ => 1.0,
    }
}

/// The clamp itself: a picture brush's `ImageSampler::alpha`, held to `0..=1`.
///
/// The clamp is not decoration: the loader checks only that `alpha` is finite
/// (`build::brush_is_finite`), so a file can carry `-3.0` or `1e30`, and
/// `Color32::gamma_multiply` outside `0..=1` is not a fade.
pub fn image_brush_alpha(img: &ondin_core::ImageBrush) -> f32 {
    img.sampler.alpha.clamp(0.0, 1.0)
}

/// Replace a brush's stops, keeping its kind and geometry. A solid brush keeps
/// only the first stop — that is what "solid" means.
///
/// "Geometry" includes the brush's transform, which rides along untouched: an
/// imported elliptical gradient carries a squash that a rebuild would lose.
pub fn with_stops(brush: &Brush, mut stops: Vec<(f32, Color)>) -> Brush {
    stops.sort_by(|a, b| a.0.total_cmp(&b.0));
    match brush {
        // An image has no stops: writing them is refused, not converted to the grey
        // solid [`stops_of`] invents for the swatch.
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

/// A brush's opacity: for a solid, its alpha; for a gradient and an image, the
/// brush's own multiplier, clamped into `0..=1`.
///
/// The clamp is not decoration: it feeds a `0..=100` field, and a file can carry any
/// finite value in the `#[serde(default)]` opacity (and an out-of-range stop alpha),
/// which egui's `clamp_existing_to_range` would turn into a write on a bare click.
pub fn alpha_of(brush: &Brush) -> f32 {
    // The sampler's multiplier: `stops_of` returns a placeholder grey whose alpha
    // would report 100% for a faded photograph.
    if let Brush::Image(img) = brush {
        return img.sampler.alpha.clamp(0.0, 1.0);
    }
    // The ramp's own multiplier: a ramp at 30% still has a stop at full alpha. The
    // inspector's *Mixed* guard (`pct != paint::alpha_of(brush) * 100.0`) reads this
    // accessor, so it must be the one that knows the field.
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
/// A gradient stores it beside its stops rather than scaling them: scaling is not
/// invertible through zero (a ramp taken to 0% could only come back flat). No floor
/// keeps it off zero either; 0 means fully invisible.
pub fn with_alpha(brush: &Brush, alpha: f32) -> Brush {
    let alpha = alpha.clamp(0.0, 1.0);
    // The sampler's multiplier, as [`alpha_of`] reads it.
    if let Brush::Image(img) = brush {
        let mut img = img.clone();
        img.sampler.alpha = alpha;
        return Brush::Image(img);
    }
    // The ramp's own multiplier.
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
/// An image is returned unchanged, for [`with_stops`]' reason. The only caller today
/// (`picker::picker_body`) already avoids images, but the guard lives here so a
/// second caller inherits it.
pub fn convert(brush: &Brush, kind: PaintKind, box_size: Size) -> Brush {
    if matches!(brush, Brush::Image(_)) || kind_of(brush) == kind {
        return brush.clone();
    }
    let mut stops = stops_of(brush);
    if kind != PaintKind::Solid && stops.len() < 2 {
        let c = stops[0].1;
        stops = vec![(0.0, c), (1.0, c.with_alpha(0.0))];
    }
    // The kind changed, so old geometry is discarded and `default_brush` reframes it.
    default_brush(kind, stops, box_size)
}

/// A brush of `kind` with `stops`, framed on `box_size`.
///
/// Every gradient made here is authored in the shape's own space, so its transform
/// is the identity (`From<Gradient>`).
/// Changing the kind is how a user turns an imported ellipse back into a circle.
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
        // A full turn about the centre, starting at three o'clock.
        PaintKind::Sweep => Brush::Gradient(
            Gradient::new_sweep(Point::new(w * 0.5, h * 0.5), 0.0, std::f32::consts::TAU).into(),
        ),
        // No default image exists (an image is an id). Unreachable today; a later
        // caller gets a grey solid rather than a panic in release, and a debug
        // assert so a test finds it first.
        PaintKind::Image => {
            debug_assert!(false, "no brush can be built for PaintKind::Image");
            return Brush::Solid(stops.first().map(|s| s.1).unwrap_or(Color::BLACK));
        }
    };
    with_stops(&brush, stops)
}

/// The picture `brush` shows, or `None` for a brush that is not one.
///
/// What the image popover reads every control's current value from.
pub fn image_of(brush: &Brush) -> Option<&ondin_core::ImageRef> {
    match brush {
        Brush::Image(img) => Some(&img.image),
        _ => None,
    }
}

/// `brush` with `edit` applied to the picture it shows, keeping the sampler (alpha
/// included) and everything else. The general form of [`with_fit`]. A brush that is
/// not an image is returned unchanged.
pub fn map_image(brush: &Brush, edit: impl FnOnce(&mut ondin_core::ImageRef)) -> Brush {
    let Brush::Image(img) = brush else {
        return brush.clone();
    };
    let mut img = img.clone();
    edit(&mut img.image);
    Brush::Image(img)
}

/// `brush` in framing mode `fit`, keeping the crop rectangle and tile scale: a mode
/// switch is a change of view, so rebuilding the reference from its id would lose
/// the crop. A brush that is not an image is returned unchanged.
///
/// `frame` makes switching to `Crop` change nothing on screen: an uncropped picture
/// carries `ondin_core::whole_crop`, which stretches on a frame of another aspect,
/// so the switch seeds `ImageRef::cover_crop` (what `Fill` shows), as the canvas
/// gesture does. `None` where the frame is unknowable (a mixed selection, a lost
/// image) sets the mode without a seed.
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

/// The geometry accessors below speak the shape's space, while a `GradientBrush`
/// carries an affine from its authoring space onto it: the identity for a new
/// gradient ([`default_brush`]), a squash for an ellipse, a box's scale after a
/// resize (`paint_box`).
///
/// A reader maps forward and a writer maps back, so the panel shows what the canvas
/// shows and a typed value lands where asked. Clearing the transform instead would
/// turn an ellipse round on the first nudge; [`with_radial_axes`] alone replaces it.
///
/// The inverse is guarded: the loader validates nothing about the `#[serde(default)]`
/// transform, and a singular one is finite (`build::brush_is_finite` accepts it) but
/// inverts to `NaN`, writing a document that never opens again. Every writer that
/// inverts asks `build::affine_is_invertible` first and returns the brush untouched,
/// which `Transaction::changes_nothing` keeps off the undo stack.
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

/// Re-aim a linear gradient at `degrees`, turning its line about its middle and
/// keeping its length, so the ends the canvas handles placed stay that far apart.
///
/// A line of no length has no middle to keep a length about; it is reframed on
/// `box_size` through the box's centre, the CSS `linear-gradient(<angle>)` framing.
/// Any other brush comes back unchanged: re-aiming a radial or a sweep would
/// rewrite it as linear.
pub fn with_linear_angle(brush: &Brush, degrees: f64, box_size: Size) -> Brush {
    let Some((a, b)) = linear_ends(brush) else {
        return brush.clone();
    };
    let dir = Vec2::from_angle(degrees.to_radians());
    let half = (b - a).hypot() * 0.5;
    let (mid, half) = if half > 1e-9 {
        (a.midpoint(b), half)
    } else {
        let (w, h) = (box_size.width.max(1.0), box_size.height.max(1.0));
        (
            Point::new(w * 0.5, h * 0.5),
            (w * dir.x.abs() + h * dir.y.abs()) * 0.5,
        )
    };
    with_linear_ends(brush, mid - dir * half, mid + dir * half)
}

/// Reverse a gradient's stops in place (`0.3 → 0.7`), leaving its geometry
/// alone. The flip control; also what "reverse" means for a radial,
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
    // In the shape's space.
    let c = g.transform * pos.end_center;
    Some((c.x / w, c.y / h))
}

/// Move a radial gradient's centre to `(cx, cy)` as fractions of `box_size`,
/// keeping its radius **and its focal point**.
///
/// A radial gradient is two circles and this control names one: `start_*` is SVG's
/// focal circle (`fx`/`fy`), `end_*` the outer circle that [`radial_centre`] reports.
/// The move translates the pair, since writing the centre into both would discard
/// the focal offset (an imported highlight snapping to dead centre). The app's own
/// gradients have a zero offset.
pub fn with_radial_centre(brush: &Brush, cx: f64, cy: f64, box_size: Size) -> Brush {
    let Brush::Gradient(g) = brush else {
        return brush.clone();
    };
    let GradientKind::Radial(pos) = g.gradient.kind else {
        return brush.clone();
    };
    let (w, h) = (box_size.width.max(1.0), box_size.height.max(1.0));
    // A singular transform is left alone, as in `with_linear_angle`.
    if !ondin_core::build::affine_is_invertible(g.transform) {
        return brush.clone();
    }
    // Asked for in the shape's space, stored in the brush's.
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

// --- on-canvas geometry ----------------------------------------------------

/// `t`'s linear part applied to `v`: where a direction goes, without the move.
fn linear_part(t: Affine, v: Vec2) -> Vec2 {
    let [a, b, c, d, _, _] = t.as_coeffs();
    Vec2::new(a * v.x + c * v.y, b * v.x + d * v.y)
}

/// A linear gradient's two ends in the shape's space. `None` for anything else.
pub fn linear_ends(brush: &Brush) -> Option<(Point, Point)> {
    let Brush::Gradient(g) = brush else {
        return None;
    };
    let GradientKind::Linear(pos) = g.gradient.kind else {
        return None;
    };
    Some((g.transform * pos.start, g.transform * pos.end))
}

/// A linear gradient with its ends at `start` and `end` in the shape's space,
/// stored back through the brush's transform. Any other brush, or one whose
/// transform is singular, comes back unchanged.
pub fn with_linear_ends(brush: &Brush, start: Point, end: Point) -> Brush {
    let Brush::Gradient(g) = brush else {
        return brush.clone();
    };
    if !matches!(g.gradient.kind, GradientKind::Linear(_))
        || !ondin_core::build::affine_is_invertible(g.transform)
    {
        return brush.clone();
    }
    let mut g = g.clone();
    let inv = g.transform.inverse();
    g.gradient.kind = GradientKind::Linear(ondin_core::peniko::LinearGradientPosition::new(
        inv * start,
        inv * end,
    ));
    Brush::Gradient(g)
}

/// A radial gradient's centre and the two axes of its outer edge, in the shape's
/// space: `a` is where offset 1 lies along the first axis (the one the stops are
/// drawn on), `b` the other. A circle's axes are `(r, 0)` and `(0, r)`.
pub fn radial_axes(brush: &Brush) -> Option<(Point, Vec2, Vec2)> {
    let Brush::Gradient(g) = brush else {
        return None;
    };
    let GradientKind::Radial(pos) = g.gradient.kind else {
        return None;
    };
    let r = f64::from(pos.end_radius);
    Some((
        g.transform * pos.end_center,
        linear_part(g.transform, Vec2::new(r, 0.0)),
        linear_part(g.transform, Vec2::new(0.0, r)),
    ))
}

/// A radial gradient whose outer edge is centred on `centre` with axes `a` and `b`
/// in the shape's space, keeping the focal circle where it sits relative to the
/// outer one.
///
/// The ellipse is a circle under the brush's transform, as an imported one is. Two
/// equal, square axes write a plain circle under the identity instead, so a round
/// gradient stays one that CSS can say. Axes that span no area, and any other
/// brush, come back unchanged.
pub fn with_radial_axes(brush: &Brush, centre: Point, a: Vec2, b: Vec2) -> Brush {
    let Brush::Gradient(g) = brush else {
        return brush.clone();
    };
    let GradientKind::Radial(pos) = g.gradient.kind else {
        return brush.clone();
    };
    if a.cross(b).abs() < 1e-9 || !a.is_finite() || !b.is_finite() {
        return brush.clone();
    }
    let r = match f64::from(pos.end_radius) {
        r if r > 1e-9 => r,
        _ => a.hypot(),
    };
    let m = Affine::new([a.x / r, a.y / r, b.x / r, b.y / r, 0.0, 0.0]);
    let focal = pos.start_center - pos.end_center;
    let mut g = g.clone();
    let (la, lb) = (a.hypot(), b.hypot());
    let round = (la - lb).abs() <= 1e-6 * la.max(lb) && a.dot(b).abs() <= 1e-6 * la * lb;
    g.gradient.kind = if round {
        let s = la / r;
        g.transform = Affine::IDENTITY;
        GradientKind::Radial(ondin_core::peniko::RadialGradientPosition {
            start_center: centre + linear_part(m, focal),
            start_radius: (f64::from(pos.start_radius) * s) as f32,
            end_center: centre,
            end_radius: la as f32,
        })
    } else {
        let t = centre - m * pos.end_center;
        g.transform = Affine::translate(t) * m;
        GradientKind::Radial(pos)
    };
    Brush::Gradient(g)
}

/// A sweep's centre in the shape's space, and the direction its offset 0 starts
/// in: degrees clockwise from three o'clock, the linear angle field's convention.
pub fn sweep_geom(brush: &Brush) -> Option<(Point, f64)> {
    let Brush::Gradient(g) = brush else {
        return None;
    };
    let GradientKind::Sweep(pos) = g.gradient.kind else {
        return None;
    };
    let d = sweep_dir(brush, 0.0)?;
    Some((
        g.transform * pos.center,
        d.y.atan2(d.x).to_degrees().rem_euclid(360.0),
    ))
}

/// A sweep centred on `centre` (shape space) whose offset 0 starts at `degrees`,
/// keeping how far round it sweeps. Any other brush, or a singular transform,
/// comes back unchanged.
pub fn with_sweep_geom(brush: &Brush, centre: Point, degrees: f64) -> Brush {
    let Brush::Gradient(g) = brush else {
        return brush.clone();
    };
    let GradientKind::Sweep(pos) = g.gradient.kind else {
        return brush.clone();
    };
    if !ondin_core::build::affine_is_invertible(g.transform) {
        return brush.clone();
    }
    let inv = g.transform.inverse();
    let d = linear_part(inv, Vec2::from_angle(degrees.to_radians()));
    let start = d.y.atan2(d.x) as f32;
    let span = pos.end_angle - pos.start_angle;
    let mut g = g.clone();
    g.gradient.kind = GradientKind::Sweep(ondin_core::peniko::SweepGradientPosition {
        center: inv * centre,
        start_angle: start,
        end_angle: start + span,
    });
    Brush::Gradient(g)
}

/// The unit direction, in the shape's space, in which a sweep's offset `t` lies
/// from its centre. `None` for anything that is not a sweep.
pub fn sweep_dir(brush: &Brush, t: f32) -> Option<Vec2> {
    let Brush::Gradient(g) = brush else {
        return None;
    };
    let GradientKind::Sweep(pos) = g.gradient.kind else {
        return None;
    };
    let angle = f64::from(pos.start_angle + t * (pos.end_angle - pos.start_angle));
    let d = linear_part(g.transform, Vec2::from_angle(angle));
    let len = d.hypot();
    (len > 1e-12).then(|| d / len)
}

/// Where offset `t` sits on a gradient's canvas guide, in the shape's space: on
/// the line for a linear, on the first axis for a radial. A sweep's stops sit on a
/// ring of screen size, so they are placed by [`sweep_dir`] instead.
pub fn guide_point(brush: &Brush, t: f32) -> Option<Point> {
    let t = f64::from(t);
    if let Some((a, b)) = linear_ends(brush) {
        return Some(a.lerp(b, t));
    }
    let (c, a, _) = radial_axes(brush)?;
    Some(c + a * t)
}

/// The offset a point in the shape's space stands for on the gradient's guide:
/// projected onto the line, onto a radial's first axis, or read as an angle round
/// a sweep. Clamped to `0..=1`; `None` for a brush with no guide, or one of no
/// length.
pub fn offset_at(brush: &Brush, p: Point) -> Option<f32> {
    let along = |from: Point, d: Vec2| {
        let len2 = d.hypot2();
        (len2 > 1e-12).then(|| ((p - from).dot(d) / len2).clamp(0.0, 1.0) as f32)
    };
    if let Some((a, b)) = linear_ends(brush) {
        return along(a, b - a);
    }
    if let Some((c, a, _)) = radial_axes(brush) {
        return along(c, a);
    }
    let Brush::Gradient(g) = brush else {
        return None;
    };
    let GradientKind::Sweep(pos) = g.gradient.kind else {
        return None;
    };
    if !ondin_core::build::affine_is_invertible(g.transform) {
        return None;
    }
    let inv = g.transform.inverse();
    let v = inv * p - pos.center;
    let span = f64::from(pos.end_angle - pos.start_angle);
    if v.hypot2() < 1e-12 || span.abs() < 1e-9 {
        return None;
    }
    let turned = v.y.atan2(v.x) - f64::from(pos.start_angle);
    Some(((turned / span).rem_euclid(1.0)) as f32)
}

// --- presentation ----------------------------------------------------------

/// The row label for a paint: its hex when solid, its kind when a gradient.
pub fn label_of(brush: &Brush) -> String {
    match kind_of(brush) {
        PaintKind::Solid => crate::ui::hex_of(stops_of(brush)[0].1),
        kind => kind.label().to_string(),
    }
}

/// [`label_of`] for a paint as held: a bound one is named by its variable, a literal
/// one by its own label.
pub fn source_label(doc: &ondin_core::Document, src: &ondin_core::BrushSource) -> String {
    match src {
        ondin_core::BrushSource::Literal(b) => label_of(b),
        ondin_core::BrushSource::Bound { var, .. } => doc
            .variables()
            .get(*var)
            .map(|v| v.name.clone())
            .unwrap_or_default(),
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

    /// One step of the Scale field must not lurch the picture at the bottom of the
    /// range. A crop scale is multiplicative (content `d` from the window centre
    /// moves `d * ds / s`), so a whole-point step lurches four times more at 85% than
    /// at 300%; the field scrubs in hundredths (`Scrub::fine(_, 2)`) where the tile
    /// scale scrubs in whole units. Measured at the window's corner.
    #[test]
    fn one_scale_step_moves_the_picture_less_than_a_unit_at_the_bottom_of_the_range() {
        use ondin_core::kurbo::{Point, Rect};

        // 850 × 850 of a 1000 × 1000 picture: cover scale 0.85.
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

        // How far the content under the window's corner moves for a step of `step`
        // percentage points: the source pixel under a fixed point, and where it lands
        // afterwards. (The picture's own edge moves at a constant rate per unit of
        // `s`, so it cannot show the difference.)
        let lurch = |from: &ondin_core::ImageRef, step: f64| -> f64 {
            let now = from.crop_scale(frame, w, h).expect("a scale");
            let next = now * 100.0 + step;
            let moved = ondin_core::ImageRef {
                crop: ondin_core::crop_scaled(from.crop, now / (next / 100.0)),
                ..from.clone()
            };
            let before = from.framing(frame, w, h).transform;
            let after = moved.framing(frame, w, h).transform;
            // The window's corner, the point the change swings furthest.
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

        // The flip: whole percentage points, which the tile field still uses. Without
        // this the assertion above is "a small number is small".
        let whole = lurch(&img, 1.0);
        assert!(
            whole > 5.0,
            "a whole-point step must lurch by several units at the floor ({whole}), \
             or this test is not measuring the thing that was reported"
        );
        // The same step three times higher up the range: the ratio is the diagnosis.
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

    /// An image brush's opacity is the sampler's alpha, and the stop and kind paths
    /// refuse it rather than replacing the photograph with `stops_of`'s grey.
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

        // The kind path, for all three kinds (the tab strip's, not `Image`).
        // flip: delete `matches!(brush, Brush::Image(_)) ||` from `convert` → fails
        // at the loop's assertion on `Solid`, with the placeholder grey.
        for kind in [PaintKind::Solid, PaintKind::Linear, PaintKind::Radial] {
            assert_eq!(
                convert(&brush, kind, Size::new(100.0, 100.0)),
                brush,
                "converting an image to {kind:?} threw the picture away"
            );
        }
    }

    /// A mode switch keeps the crop and tile scale: looking at Fit and switching
    /// back must restore the picture exactly.
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

    /// Choosing Crop from the panel must not move the picture: an uncropped square
    /// photograph on a 2:1 frame would otherwise stretch. Asserted against `Fill`'s
    /// own framing, and an authored crop is never overwritten by the seed.
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

    /// An image classifies as its own kind and is absent from the picker's kinds.
    #[test]
    fn an_image_is_its_own_kind_and_stays_out_of_the_pickers_kinds() {
        assert_eq!(kind_of(&an_image()), PaintKind::Image);
        assert!(
            !PaintKind::Image.is_gradient(),
            "the picker's kind menu must not offer Image"
        );
        assert_eq!(label_of(&an_image()), "Image");
    }

    /// A sweep is its own kind, labelled *Angular*, and a linear-angle edit leaves it a
    /// sweep; converting it is an explicit kind change.
    /// flip: `kind_of`'s sweep arm filed under `Linear` → fails at the kind.
    #[test]
    fn a_sweep_keeps_its_kind() {
        let sweep = default_brush(
            PaintKind::Sweep,
            vec![(0.0, Color::BLACK), (1.0, Color::WHITE)],
            BOX,
        );
        assert_eq!(kind_of(&sweep), PaintKind::Sweep);
        assert_eq!(label_of(&sweep), "Angular");
        assert!(PaintKind::Sweep.is_gradient(), "the kind menu offers it");
        assert_eq!(
            with_linear_angle(&sweep, 45.0, BOX),
            sweep,
            "an angle edit does not re-aim a sweep as a linear"
        );
        let linear = convert(&sweep, PaintKind::Linear, BOX);
        assert_eq!(kind_of(&linear), PaintKind::Linear);
        assert_eq!(
            stops_of(&linear),
            stops_of(&sweep),
            "a conversion keeps the stops"
        );
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

    /// A ramp's opacity is stored and the stops are left alone, so a round trip
    /// through zero restores the fade.
    ///
    /// flip: `with_alpha` scaling the stops → fails at "the field reads back" (the
    /// reader and writer are one mechanism, so they cannot be flipped apart).
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
        // The solid arm of "0 must be reversible"; the ramp's arm is
        // `a_ramps_opacity_is_stored_and_survives_a_round_trip_through_zero`.
        let brush = Brush::Solid(Color::from_rgba8(255, 0, 0, 0));
        assert_eq!(alpha_of(&with_alpha(&brush, 1.0)), 1.0);
    }

    /// An out-of-range stop alpha (reachable from a hand-written file) never reaches
    /// the opacity field, and a bare click leaves the stops alone.
    ///
    /// The fixture assertion is load-bearing: were `default_brush` to clamp on the
    /// way in, every later assertion would pass vacuously.
    ///
    /// flip: drop the gradient arm from `alpha_of` → fails at the second assertion
    /// (the stop fold reports the fixture's own peak).
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

        // A bare click at the value the field already shows.
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

    /// A negative stop alpha cannot move the opacity field: it is independent of
    /// the stops.
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
            "the ramp's multiplier is untouched by what its stops say: neither \
             -1.0 nor 0.0 is reachable"
        );
    }

    /// The angle field turns the line about its middle and keeps its length.
    ///
    /// flip: `with_linear_angle` reframing on the box every time → fails at the
    /// length assertion for the off-centre line.
    #[test]
    fn linear_angle_round_trips_about_the_line_s_middle() {
        let brush = default_brush(
            PaintKind::Linear,
            vec![(0.0, Color::BLACK), (1.0, Color::WHITE)],
            BOX,
        );
        let brush = with_linear_ends(&brush, Point::new(20.0, 30.0), Point::new(60.0, 60.0));
        assert_eq!(
            linear_ends(&brush),
            Some((Point::new(20.0, 30.0), Point::new(60.0, 60.0))),
            "the fixture's ends are where they were put"
        );
        for deg in [0.0, 45.0, 90.0, 135.0, 200.0, 315.0] {
            let aimed = with_linear_angle(&brush, deg, BOX);
            let got = linear_angle(&aimed).expect("linear");
            assert!((got - deg).abs() < 1e-6, "{deg} -> {got}");
            let (a, b) = linear_ends(&aimed).expect("linear");
            let mid = a.midpoint(b);
            assert!((mid.x - 40.0).abs() < 1e-6 && (mid.y - 45.0).abs() < 1e-6);
            assert!(((b - a).hypot() - 50.0).abs() < 1e-6, "{deg}: length kept");
        }
    }

    /// A line of no length has no middle to turn about, so the angle reframes it
    /// across the box as CSS would.
    #[test]
    fn a_zero_length_line_takes_the_box_framing() {
        let brush = default_brush(
            PaintKind::Linear,
            vec![(0.0, Color::BLACK), (1.0, Color::WHITE)],
            BOX,
        );
        let flat = with_linear_ends(&brush, Point::new(5.0, 5.0), Point::new(5.0, 5.0));
        let (a, b) = linear_ends(&with_linear_angle(&flat, 0.0, BOX)).expect("linear");
        assert_eq!((a, b), (Point::new(0.0, 50.0), Point::new(200.0, 50.0)));
    }

    /// Ends round-trip through a transform the brush already carries, so a resized
    /// gradient's handle lands where it was dragged.
    #[test]
    fn linear_ends_round_trip_through_the_brush_transform() {
        let brush = default_brush(
            PaintKind::Linear,
            vec![(0.0, Color::BLACK), (1.0, Color::WHITE)],
            BOX,
        );
        let squashed =
            ondin_core::paint_box::carry_brush(&brush, Affine::scale_non_uniform(2.0, 0.5));
        assert_eq!(
            linear_ends(&squashed),
            Some((Point::ZERO, Point::new(400.0, 50.0))),
            "the map carries the ends"
        );
        let moved = with_linear_ends(&squashed, Point::new(10.0, 10.0), Point::new(90.0, 40.0));
        let (a, b) = linear_ends(&moved).expect("linear");
        assert!((a - Point::new(10.0, 10.0)).hypot() < 1e-9);
        assert!((b - Point::new(90.0, 40.0)).hypot() < 1e-9);
    }

    /// Unequal axes are a squash under the transform; equal square ones go back to
    /// a plain circle with no transform, and the focal offset turns and scales with
    /// the circle.
    ///
    /// flip: dropping the `round` branch in `with_radial_axes` → fails at the
    /// identity assertion.
    #[test]
    fn radial_axes_round_trip_and_a_circle_stays_plain() {
        use ondin_core::peniko::{Gradient, RadialGradientPosition};
        let mut gradient = Gradient::new_radial((100.0, 50.0), 40.0);
        gradient.kind = GradientKind::Radial(RadialGradientPosition {
            start_center: Point::new(90.0, 50.0),
            start_radius: 4.0,
            end_center: Point::new(100.0, 50.0),
            end_radius: 40.0,
        });
        let brush = Brush::Gradient(gradient.into());
        let (c, a, b) = radial_axes(&brush).expect("radial");
        assert_eq!(
            (c, a, b),
            (
                Point::new(100.0, 50.0),
                Vec2::new(40.0, 0.0),
                Vec2::new(0.0, 40.0)
            )
        );

        let squashed = with_radial_axes(
            &brush,
            Point::new(60.0, 40.0),
            Vec2::new(30.0, 0.0),
            Vec2::new(0.0, 10.0),
        );
        let (c2, a2, b2) = radial_axes(&squashed).expect("radial");
        assert!((c2 - Point::new(60.0, 40.0)).hypot() < 1e-9);
        assert!((a2 - Vec2::new(30.0, 0.0)).hypot() < 1e-6);
        assert!((b2 - Vec2::new(0.0, 10.0)).hypot() < 1e-6);

        // Round again, turned a quarter: the focal offset (-10, 0) at radius 40
        // becomes (0, -5) at radius 20.
        let round = with_radial_axes(
            &squashed,
            Point::new(50.0, 50.0),
            Vec2::new(0.0, 20.0),
            Vec2::new(-20.0, 0.0),
        );
        let Brush::Gradient(g) = &round else {
            panic!("gradient")
        };
        assert_eq!(g.transform, Affine::IDENTITY);
        let GradientKind::Radial(p) = g.gradient.kind else {
            panic!("radial")
        };
        assert_eq!(p.end_radius, 20.0);
        assert!(
            (p.start_center - Point::new(50.0, 45.0)).hypot() < 1e-4,
            "{:?}",
            p.start_center
        );
        assert!((p.start_radius - 2.0).abs() < 1e-6);
    }

    /// Axes that span no area are refused rather than written as a singular
    /// transform.
    #[test]
    fn flat_radial_axes_are_refused() {
        let brush = default_brush(
            PaintKind::Radial,
            vec![(0.0, Color::WHITE), (1.0, Color::BLACK)],
            BOX,
        );
        let flat = with_radial_axes(
            &brush,
            Point::new(10.0, 10.0),
            Vec2::new(20.0, 0.0),
            Vec2::new(40.0, 0.0),
        );
        assert_eq!(flat, brush);
    }

    /// A sweep's start and centre round-trip, its span is kept, and a point read
    /// back as an offset goes round from the start.
    #[test]
    fn sweep_geometry_round_trips_and_reads_offsets_round_the_ring() {
        let brush = default_brush(
            PaintKind::Sweep,
            vec![(0.0, Color::WHITE), (1.0, Color::BLACK)],
            BOX,
        );
        assert_eq!(sweep_geom(&brush), Some((Point::new(100.0, 50.0), 0.0)));
        let turned = with_sweep_geom(&brush, Point::new(40.0, 30.0), 90.0);
        let (c, deg) = sweep_geom(&turned).expect("sweep");
        assert!((c - Point::new(40.0, 30.0)).hypot() < 1e-6);
        assert!((deg - 90.0).abs() < 1e-4, "{deg}");
        // Straight down is the start; to the left is a quarter turn on.
        let down = offset_at(&turned, Point::new(40.0, 60.0)).expect("offset");
        let left = offset_at(&turned, Point::new(10.0, 30.0)).expect("offset");
        assert!(down.abs() < 1e-4 || (down - 1.0).abs() < 1e-4, "{down}");
        assert!((left - 0.25).abs() < 1e-4, "{left}");
    }

    /// Offsets project onto the line and onto a radial's first axis, clamped.
    #[test]
    fn offsets_project_onto_the_guide() {
        let lin = with_linear_ends(
            &default_brush(
                PaintKind::Linear,
                vec![(0.0, Color::BLACK), (1.0, Color::WHITE)],
                BOX,
            ),
            Point::new(0.0, 0.0),
            Point::new(100.0, 0.0),
        );
        assert_eq!(offset_at(&lin, Point::new(25.0, 40.0)), Some(0.25));
        assert_eq!(offset_at(&lin, Point::new(-30.0, 0.0)), Some(0.0));
        assert_eq!(guide_point(&lin, 0.5), Some(Point::new(50.0, 0.0)));
        let rad = default_brush(
            PaintKind::Radial,
            vec![(0.0, Color::WHITE), (1.0, Color::BLACK)],
            BOX,
        );
        // Centre (100, 50), radius 50.
        assert_eq!(offset_at(&rad, Point::new(125.0, 90.0)), Some(0.5));
        assert_eq!(guide_point(&rad, 1.0), Some(Point::new(150.0, 50.0)));
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

    /// Moving the centre translates the focal point with it. The fixture is built by
    /// hand with a focal offset, because `default_brush` gradients have none and
    /// could not show the bug; the offset is asserted relative, not as a coordinate.
    ///
    /// flip: `start_center: centre` in `with_radial_centre` → fails at the last
    /// assertion (offset `0, 0` against `-10, -5`); the older round-trip test stays
    /// green through it.
    #[test]
    fn moving_a_radial_centre_carries_its_focal_point_along() {
        use ondin_core::peniko::{Gradient, RadialGradientPosition};

        let mut gradient = Gradient::new_radial((100.0, 50.0), 40.0);
        // A highlight up and to the left of the centre.
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

    /// A singular brush transform is not inverted: its `NaN` would save as `null`
    /// and the document would never load again. Asserts the geometry stays
    /// representable, not which guard refused; the first assertion pins
    /// `determinant() == 0.0` so the fixture reaches the bug.
    #[test]
    fn a_singular_gradient_transform_is_refused_rather_than_inverted() {
        use ondin_core::kurbo::Affine;

        // Rank-1 with finite coefficients, which `build::affine_is_finite` accepts.
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
    //! The one rule both picture chips read.

    use super::*;

    fn faded(alpha: f32) -> Brush {
        let Brush::Image(mut img) = ondin_core::image_brush(ondin_core::ImageId("b".repeat(64)))
        else {
            unreachable!("image_brush builds an image brush")
        };
        img.sampler.alpha = alpha;
        Brush::Image(img)
    }

    /// A brush with no picture is the identity multiplier. `1.0` for a gradient is
    /// deliberate: its fade is `GradientBrush::opacity`, folded in by `picker::ramp`.
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

    /// The last paint is the top row.
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

    /// A live reorder settles: with the pointer still, the landing is the slot the
    /// entry is already in. Either off-by-one wrong would flip the rows every frame.
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

    /// `reordered` is the list edit the drop applies.
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

    /// What the panel draws through `display_order` equals what `reordered` commits.
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

    /// Rows of different heights: a drop resolves against each slot's own middle, not
    /// a pitch, so passing a tall stroke takes until its midpoint.
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

    /// The live reorder still settles over uneven slots.
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

    /// Following `reordered_index` to the new slot finds the same paint there; stated
    /// against `reordered` itself rather than a table of indices.
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

    /// A removal drops what belonged to the paint that went and brings the rest down.
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
