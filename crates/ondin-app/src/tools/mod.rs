//! Interactive tools (§9.4). Each gesture produces one `Transaction` committed
//! on release (§9.3). The transaction-building logic is pure and unit-tested;
//! the canvas supplies the egui events.
//!
//! World-space projections that more than one caller needs (move, reparent,
//! group) live in `ondin_core::build` so MCP and the future command line share
//! them; what stays here is tool-shaped: turning a drag rectangle into a node.
//!
//! 🚨 **That rule is not kept, the gap is scheduled rather than accepted, and the
//! trigger is written down** (§15 D786, `[A2-L7-01]`). Most of the production half
//! of this file and of `crate::preview` is **headless geometry** — neither file's
//! `use` list names egui, winit or wgpu, and every egui mention in either is inside
//! a doc comment — and `resize_box_to`, `rotate_node`, `pen_anchors`, `insert_point`,
//! `align_points`, `distribute_points` and `space_points` are not *"turning a drag
//! rectangle into a node"*. `ondin-app` declares only `[[bin]] name = "ondin"` and
//! has no `src/lib.rs`, so **nothing outside this binary can link against any of
//! it**, and §3's graph puts `ondin-mcp` *below* `ondin-app`, so the edge cannot be
//! added without inverting the graph. `build::align` and `build::distribute` are
//! already the node-level twins of two of these, in the other crate.
//!
//! ⚠️ **The size is a command and not a number, because the finding's went stale in
//! a year.** It recorded *"~4.8k"* over production halves of 3,609 and 1,244; the
//! halves are what sits above the first `#[cfg(test)]` in each file — ask
//! `grep -n '^#\[cfg(test)\]'` — and that read 4,114 and 1,337 on 2026-09-15. Not
//! every line of a half is headless geometry: `CaretBlink` is in `preview.rs`'s and
//! stays.
//!
//! ⚠️ **The move is deferred and not declined**, because the cost is
//! asymmetric in time: doing it before the first MCP handler exists is a
//! compiler-enumerated sweep, and doing it after means the duplicate is already
//! written and two implementations have to be reconciled — §15 D2's shape and
//! `[A1-L7-07]`'s. **The trigger is the first MCP tool handler, not a date and not
//! M6 in general**: §15 D4 defers `serve`/`mcp-proxy` to M6, so the window is
//! long, and the thing that closes it is somebody needing `set_geometry` from
//! outside this binary. What moves is everything taking only `Document` /
//! `Resolved` / kurbo types; what stays is the gesture state — `Drag`, `PenState`,
//! `TextSession`, `CaretBlink`.

use crate::preview::{Handle, LineEnd, PenAnchor, PenSubpath, PointRef, Side};
use ondin_core::Brush;
use ondin_core::build::{self, Axis, Basis, Edge};
use ondin_core::kurbo::{
    Affine, BezPath, CubicBez, ParamCurve, PathEl, Point, Rect, RoundedRectRadii, Size, Vec2,
};
use ondin_core::peniko::Color;
use ondin_core::{
    Document, Fill, GeometryPatch, ImageEntry, ImageId, NodeId, NodeKind, Operation, Resolved,
    Stroke, TextSizing, TextStyle, Transaction, geometry,
};

/// The active canvas tool. `Select` manipulates existing nodes; the others draw
/// something new (§9.4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    /// Drag anywhere to pan, without having to find empty canvas or hold space.
    Hand,
    /// Select's gestures, with the handle drag scaling the layer *photographically*
    /// instead of editing its geometry — see [`Scaling`].
    Scale,
    Frame,
    Rect,
    Ellipse,
    Polygon,
    Star,
    Line,
    Pen,
    /// Edits the *points* of a path rather than the path as a layer — anchors,
    /// their handles, add and remove.
    ///
    /// **A tool, not a mode**, and this is the case that rule was written for
    /// (todo, *when a mode earns its keep*): it is visible on the rail, named,
    /// has a letter, and the chrome it puts on the canvas announces it. A vector
    /// focus *mode* would have all the same behaviour and none of those four.
    ///
    /// **The pen is reachable from inside it** without the edit being dropped —
    /// `P` arms `app::pen_bias` rather than switching tool (§15 D125). That is not
    /// the mode this doc argues against: it reroutes no key, adds no state to
    /// `input::Mode`, and changes exactly one thing, what a press on the canvas
    /// means. It also borrows the pen's own letter, cursor and rail slot, so what
    /// announces it is a tool that already has all four of the things above.
    Node,
    Text,
    /// Places images the cursor is **loaded** with — and disarms itself when it
    /// runs out, which no other tool on the rail does.
    ///
    /// **Not a persistent mode, deliberately** (§5.5a, §9.4): you pick files
    /// first, and then each click places one of them, in order. Arming it with
    /// nothing loaded is what opens the file dialog, so the rail button and
    /// `Ctrl+Shift+K` are the same action. When the last one is placed the tool
    /// hands back to Select on its own.
    ///
    /// That is Figma's behaviour and the right one — an image tool that stayed
    /// armed would have nothing to do with the next click — but it *is* unusual
    /// here, so it is said out loud rather than left to read as a bug. The
    /// remaining count belongs on the cursor, as a badge over the drawn
    /// crosshair — the only cursor badge still wanted (§15 D233).
    Image,
    /// **Image editing**: the layer's box becomes a window onto its picture.
    /// Dragging a handle crops, dragging the picture pans it, and the framing,
    /// the mode and the seven adjustments come up in a card (§15 D268).
    ///
    /// **The one member with no rail button and no letter**, which is a
    /// deliberate reversal of what D181 argued and needs saying out loud. The old
    /// reading of D118 was that a state entered by double-click has to earn a
    /// slot, a name and a key or it is an invisible mode; the crop tool had all
    /// three and was *still* the wrong shape, because picking it off the rail
    /// armed a gesture with no subject, and because a mode strip offering *Crop*
    /// beside a box that resized rather than cropped was two controls disagreeing
    /// about one word. What answers D118 here is that the mode cannot be entered
    /// without a picture to enter, and that entering it puts a card on screen and
    /// an outline round the whole photograph: it announces itself by being
    /// impossible to be in by accident, which a rail button is precisely not.
    ///
    /// **Still a `Tool` rather than a field of its own.** Everything that makes a
    /// mode work here is keyed off `self.tool` — the input routing, the cursor,
    /// `chrome_hidden`, the `Escape` ladder and `Enter`'s toggle — and a parallel
    /// `editing_image: Option<..>` would be a second answer to "what is the canvas
    /// doing", free to disagree with the first. The subject stays derived:
    /// `canvas::edited_image` is the one place that answers which fill, exactly as
    /// `edited_path` does for the node tool.
    ImageEdit,
}

impl Tool {
    /// Whether this tool draws by click-dragging a box (as opposed to the
    /// click-driven pen, and Select's own gesture machinery).
    ///
    /// **Text is in here and is also a click tool**, which no other member is: a click
    /// plants an auto-width node and a drag draws a fixed box, and which of the two
    /// happened is egui's drag threshold to decide. The two gestures are exclusive —
    /// egui never reports both `clicked()` and `drag_started()` for one press — so the
    /// Text tool's click arm and its create arm cannot both fire. What they can both
    /// miss is a drag that goes out and comes **back** to where it started: egui calls
    /// that a drag and not a click, while `MIN_CREATE_PX` finds no box in it, so the
    /// text release plants a caret rather than doing nothing. A shape abandons that
    /// gesture, and for a shape that is right. See
    /// `canvas::text_gesture` for both premises, measured.
    pub fn drags_a_box(self) -> bool {
        matches!(
            self,
            Tool::Frame
                | Tool::Rect
                | Tool::Ellipse
                | Tool::Polygon
                | Tool::Star
                | Tool::Line
                | Tool::Text
                // **In here for the same reason Text is**: a drag draws the box
                // the image fills, and a click places it at its fitted size. It
                // differs from Text in what a click *does* and in nothing about
                // the drag.
                | Tool::Image
        )
    }

    /// What a plain click with this tool makes, or `None` for a tool whose click
    /// means something else — or nothing.
    ///
    /// **Exhaustive on purpose, with no wildcard** (§15 D295). This started as
    /// `clicks_out_a_default_box`, a `matches!` over four tools, and its doc
    /// asserted the rest of the table in prose: "every member of `drags_a_box` now
    /// answers a click with *something* — Text plants an auto-width node, Image
    /// places its picture at the fitted size." **Half of that was false as it was
    /// written.** Text had a click arm; Image had none at all, so a loaded cursor
    /// could only be discharged by dragging a box, and the one gesture the tool
    /// exists for did nothing. A prose claim about a routing table is a claim
    /// nothing checks; a `match` with no wildcard is the same claim as a build
    /// error.
    ///
    /// The three answers, and why each tool has the one it does:
    ///
    /// - **Rect, Ellipse, Polygon, Star** — a square at [`CLICK_CREATE_SIZE`] with
    ///   the tool's own defaults (§15 D291).
    /// - **Image** — the picture at its *own* size, shrunk only if it exceeds the
    ///   frame ([`fitted_image_size`]). A click gives no box, so the size comes from
    ///   the file; a drag gives one and is obeyed exactly, aspect included.
    /// - **Text** — a node that sizes itself to what is typed, or the caret placed
    ///   in text that is already there (§15 D170).
    ///
    /// **Frame and Line answer `None` deliberately**, each for its own reason. A
    /// frame is a container, so a default-sized one guesses at a layout rather than
    /// at a shape, and Figma answers a frame click with a size *menu*. A line has no
    /// box: a default one would come out diagonal, a direction nobody asked for, and
    /// a horizontal default would be a rule invented here. Both are one arm away.
    /// The rest — Select, Scale, Pen, Node, ImageEdit, Hand — are not creation tools
    /// at all, and the middle four never reach the click routing anyway, having
    /// claimed the pointer earlier in `canvas::normal_mode_input`.
    pub fn click_makes(self) -> Option<ClickCreate> {
        match self {
            Tool::Rect | Tool::Ellipse | Tool::Polygon | Tool::Star => {
                Some(ClickCreate::DefaultBox)
            }
            Tool::Image => Some(ClickCreate::FittedImage),
            Tool::Text => Some(ClickCreate::TextNode),
            Tool::Frame | Tool::Line => None,
            Tool::Select | Tool::Scale | Tool::Pen | Tool::Node | Tool::ImageEdit | Tool::Hand => {
                None
            }
        }
    }

    /// Whether this tool picks, moves and transforms existing layers — Select and
    /// Scale, which differ only in what a handle drag *means*.
    ///
    /// Spelled once because it is asked in **five** places — the click and the drag
    /// start (`canvas::normal_mode_input`), the hover outline, the pivot marker, and
    /// the guide lines in `rulers.rs` — and a tool that answered yes in four of them
    /// would be subtly half-built. The cursor is a sixth decision but not a sixth
    /// caller: it needs the two tools' *different* rest states, so it matches on
    /// `Tool::Select | Tool::Scale` and forks (`canvas::rest_cursor`).
    pub fn selects(self) -> bool {
        matches!(self, Tool::Select | Tool::Scale)
    }

    /// What a handle drag with this tool does to the layer.
    pub fn scaling(self) -> Scaling {
        match self {
            Tool::Scale => Scaling::Photographic,
            _ => Scaling::Geometry,
        }
    }
}

/// What a plain click with a creation tool makes ([`Tool::click_makes`]).
///
/// Three answers rather than a boolean, because the three gestures differ in
/// where the *size* comes from — a constant, the file, or what gets typed — and
/// that is the whole of what a click has to decide when it is given no box.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClickCreate {
    /// A [`CLICK_CREATE_SIZE`] square carrying the tool's own defaults.
    DefaultBox,
    /// The loaded picture at its intrinsic size, shrunk only if it exceeds the
    /// frame it lands in ([`fitted_image_size`]).
    FittedImage,
    /// A text node that sizes itself to what is typed — or, over text that
    /// already exists, the caret placed in it rather than a new node over it.
    TextNode,
}

/// What a handle drag changes — the whole difference between Select and Scale.
///
/// **A gesture, not a preference.** You resize a card for layout and scale an
/// icon photographically in the same session, so a document-level "scale strokes
/// too" switch would be walked to once and forgotten. Two tools, two verbs, and
/// the cursor says which one is armed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Scaling {
    /// §5.6: the layer's *geometry* changes and nothing else. A 1px border stays
    /// 1px, a 4px radius stays 4px, 16pt type stays 16pt — which is what makes a
    /// resize a layout edit.
    #[default]
    Geometry,
    /// Everything scales with the box, as though the layer were a picture being
    /// enlarged: geometry, stroke widths, corner radii and font size.
    ///
    /// **It was never only about strokes.** Stroke width is one of three things a
    /// resize deliberately leaves alone; a 4px radius on a shape scaled to twice
    /// the size reads wrong for exactly the same reason a 1px border does, and so
    /// does 16pt type. One tool covers all three or it covers none of them
    /// convincingly.
    Photographic,
}

impl Scaling {
    /// The single factor the scalars take, given the two the box was scaled by.
    ///
    /// **The geometric mean, and it has to be one number.** A stroke width has no
    /// non-uniform reading — one width, two factors — so a squashed shape has to
    /// pick something, and `sqrt(|sx·sy|)` is the choice that degenerates to
    /// exactly `s` under a uniform scale, which is the case that has to be
    /// perfect. The same argument covers corner radii and font size.
    ///
    /// Absolute, so a mirror scales the numbers rather than negating them: a
    /// stroke of -1 is not a thing.
    fn scalar(self, csx: f64, csy: f64) -> Option<f64> {
        if self != Scaling::Photographic {
            return None;
        }
        let s = (csx.abs() * csy.abs()).sqrt();
        (s.is_finite() && (s - 1.0).abs() > SCALE_NOOP).then_some(s)
    }
}

/// The box a shape tool's *click* gets, in world units, when there is no drag to
/// take one from ([`ClickCreate::DefaultBox`]).
///
/// **Square, and the same number for all four tools**, so the answer is one fact
/// rather than a table: a 100 × 100 rect, a 100-diameter circle, a triangle and a
/// star in a 100 × 100 box. It is Figma's number, and round enough to be an
/// obvious starting point rather than a size anyone will mistake for considered.
///
/// World units, not screen pixels, so the shape a click makes is the same size in
/// the document at every zoom — the size is a document *default*, and one that
/// changed with the viewport would be a different shape every time.
pub const CLICK_CREATE_SIZE: f64 = 100.0;

/// Sides a fresh polygon is drawn with — a triangle, the smallest shape the
/// count can describe, so the field has somewhere to go.
pub const POLYGON_DEFAULT_SIDES: u32 = 3;
/// Points a fresh star is drawn with, and the inner radius that makes it the
/// star everybody draws: five points, inner ring at the golden-section radius
/// where the spokes' edges line up straight through the middle.
pub const STAR_DEFAULT_POINTS: u32 = 5;
pub const STAR_DEFAULT_RATIO: f64 = 0.382;

/// The **authored** local box of a node — the `size` a user typed or dragged out — or
/// `None` for the kinds that have no such field (lines, paths, groups, booleans).
///
/// `None` is not "cannot be resized by dragging a handle", which is what this used to
/// say. A `Path` and a `Line` are both resizable now; what they have is geometry rather
/// than a box, so they scale through [`resize_geometry`] and `scale_geometry` instead of
/// through a `GeometryPatch::Size`. This is the question "is there a size field to
/// replace", and every caller wants exactly that.
pub fn resizable_size(kind: &NodeKind) -> Option<Size> {
    match kind {
        NodeKind::Rect { size, .. }
        | NodeKind::Ellipse { size }
        | NodeKind::Polygon { size, .. }
        | NodeKind::Star { size, .. }
        | NodeKind::Artboard { size, .. } => Some(*size),
        // Auto-sized text has no box to drag until it is given one; a resize
        // gesture is exactly what converts it to a fixed box. Auto-height has
        // half of one — the width is authored, the height is not — and reports
        // zero *height* rather than no box, so the handles still take a width
        // drag and the height still follows the content.
        NodeKind::Text { sizing, .. } => Some(match sizing {
            TextSizing::Fixed(size) => *size,
            TextSizing::AutoHeight(w) => Size::new(*w, 0.0),
            TextSizing::Auto => Size::ZERO,
        }),
        _ => None,
    }
}

/// The box a resize gesture **anchors on**, in the node's own local space: the box
/// the handles were drawn on, which is `query::local_box`.
///
/// This is not [`resizable_size`], and the difference was a bug. That function
/// answers "is there a size field to replace", so it reports `Size::ZERO` for
/// `TextSizing::Auto` and `(w, 0)` for `AutoHeight` — honestly, since neither has
/// one, and every other caller wants exactly that. But hold the opposite edge of a
/// zero box still and it is at the local origin rather than under the other
/// handle, so dragging the **left** edge of an auto-sized label widened it
/// rightward from the origin instead of leftward from the ink.
///
/// **The extent is keyed on the sizing mode, not on which extents happen to be
/// zero**, so the rule is the one [`TextSizing`] already states: an authored
/// extent is the user's number and an emergent one is the content's. With no
/// cached layout it falls back to `resizable_size`'s answer rather than inventing
/// a box.
///
/// **The origin is not always zero, and only a text node can move it.** A trimmed
/// **auto** box starts below the local origin — `text::box_of` gives it
/// `(0, trim_top)` so that tightening the box moves no ink (§15 D78) — while a
/// `Fixed` box is the authored one, untrimmed, at the origin. So the handles of a
/// trimmed auto node sit on a box the gesture was measuring from the wrong datum,
/// and [`resize_to_handle`] shifts the pointer by this origin to put the two back
/// in one space.
fn anchored_box(kind: &NodeKind, text: Option<&ondin_core::TextLayout>) -> Option<Rect> {
    let size = resizable_size(kind)?;
    let (NodeKind::Text { sizing, .. }, Some(layout)) = (kind, text) else {
        return Some(Rect::from_origin_size(Point::ZERO, size));
    };
    // **A railed node's box is its bent box, and `sizing` is not it** (§15 D405).
    // On a rail the three sizing modes are inert — the rail's length is the
    // measure — so reading a `Fixed` box here would anchor the gesture to a
    // rectangle the handles are not drawn on: the layer would resize from a corner
    // that is not on screen. `layout.bounds()` is what the canvas draws, which is
    // the invariant this whole function exists to keep.
    if layout.warp.is_some() {
        return Some(layout.bounds());
    }
    let size = match sizing {
        TextSizing::Fixed(s) => *s,
        TextSizing::AutoHeight(w) => Size::new(*w, layout.size.height),
        TextSizing::Auto => layout.size,
    };
    Some(Rect::from_origin_size(layout.origin.to_point(), size))
}

/// The [`TextSizing`] a resize to `new_size` leaves a text layer in.
///
/// **Which mode a resize produces is not the size's business, it is the
/// *gesture's*** — specifically whether the thing the user grabbed owns `y`.
/// A side handle does not ([`Handle::anchored`] hands back no `y` for one), and
/// neither does the panel's `W` field with the proportion lock off; a corner, a
/// top/bottom handle and the `H` field all do. That is the whole of
/// `authors_height`.
///
/// So an auto-width label dragged or typed *wider* becomes `AutoHeight` — it
/// gains a wrap width and the lines keep deciding the height, which is what
/// Figma does and what §15 D148 built. Give it a height and it becomes `Fixed`.
/// An already-`Fixed` box stays fixed whichever number moved: the rule is about
/// *height authorship*, and that question is already answered for one.
///
/// Spelled once, here, because the canvas and the inspector are two ways of
/// asking for the same box and a second copy of this rule is a second answer to
/// "what does typing a width mean" (§15 D242's finding, one kind further on).
pub fn resized_text_sizing(sizing: TextSizing, new_size: Size, authors_height: bool) -> TextSizing {
    if authors_height || sizing.height_is_authored() {
        TextSizing::Fixed(new_size)
    } else {
        TextSizing::AutoHeight(new_size.width)
    }
}

/// The box a resize gesture produces, in the node's own local space.
///
/// Split out from [`resize_to_handle`] because it is the whole of the geometry
/// and none of the document: the anchored-axis rule that makes a *side* drag
/// change one dimension and a *corner* drag change two lives here, once, and is
/// unit-testable without a `Document`.
///
/// `size` is the box being resized (its origin is the local origin, §5.6) and `p`
/// is the pointer in that same local space. Returns `(origin, size, mirrored)`,
/// where `mirrored` is per axis.
///
/// **`symmetric` is about the box's own centre, and deliberately not about the
/// node's transform origin** — see §15 D60 for why the two cannot be reconciled.
///
/// 🚨 **The third return is the flip, and this function used to swallow it**
/// (§15 D753, `[S13.1-L2-02]`). The extent closure takes `(at - held).abs()` and
/// the edge closure reflects the *box* across the anchor, so the box lands
/// perfectly on the far side and **no sign survives to become a mirror**. §5.6
/// says, unqualified, *"a handle dragged past the corner it is anchored to
/// **flips the shape**, and the flip *is* the sign of the scale factor"* — and
/// every kind that goes through `resize_factors` obeys it, because `scale_along`
/// keeps the sign and `split_geometry_scale` peels it into `Orientation::mirror`.
/// A `Rect`, a `Text` and an image-filled layer come through **here** instead and
/// did not. Measured: same box to the unit, `Mirror::X` against `Mirror::None`.
///
/// **Reporting rather than applying** is the shape, because this function knows
/// nothing about transforms: it answers a question about a box, and
/// [`resize_to_handle`] composes the reflection.
fn resized_box(
    size: Size,
    handle: Handle,
    p: Point,
    keep_ratio: bool,
    symmetric: bool,
) -> (Point, Size, (bool, bool)) {
    let (hold_x, hold_y) = handle.anchored(size.width, size.height);
    let centre = Point::new(size.width / 2.0, size.height / 2.0);

    // How far the box reaches on each axis the handle owns.
    //
    // Normally the *opposite edge* stays put and the extent is the distance from
    // it. With `symmetric` (Alt) the **centre** stays put instead and both edges
    // move out together, so the extent is twice the distance from the middle —
    // dragging the right side pulls the left one out to match, which is what
    // "resize from the centre" means everywhere else it exists.
    //
    // An axis the handle does not own keeps the box's current extent either way.
    let extent = |hold: Option<f64>, at: f64, mid: f64, current: f64| match hold {
        None => current,
        Some(_) if symmetric => 2.0 * (at - mid).abs(),
        Some(held) => (at - held).abs(),
    };
    let mut w = extent(hold_x, p.x, centre.x, size.width);
    let mut h = extent(hold_y, p.y, centre.y, size.height);

    if keep_ratio && size.width > 0.0 && size.height > 0.0 {
        let ratio = size.width / size.height;
        match (hold_x.is_some(), hold_y.is_some()) {
            // A corner follows whichever axis the pointer pushed further, so
            // the gesture feels like it tracks the cursor.
            (true, true) => {
                if w / ratio >= h {
                    h = w / ratio;
                } else {
                    w = h * ratio;
                }
            }
            // A side owns one axis and the other simply follows it.
            (true, false) => h = w / ratio,
            (false, true) => w = h * ratio,
            (false, false) => {}
        }
    }
    let (w, h) = (w.max(1.0), h.max(1.0));

    // Where the box starts.
    //
    // Symmetric: laid out around the centre, on **both** axes and regardless of
    // which one the handle owns. Centring an axis the handle does not own is a
    // no-op in the ordinary case — its extent is unchanged, and `mid` is half of
    // it, so this comes out at 0 exactly as the anchored rule would — but it is
    // not a no-op when `keep_ratio` has driven that axis from the other one. That
    // is the Alt+Shift case on a *side* handle, and leaving it at 0 would centre
    // the axis being dragged while pinning the top edge of the axis following it.
    //
    // Anchored: the held edge stays and the box grows away from it; an axis with
    // no anchor does not move.
    let edge = |hold: Option<f64>, at: f64, mid: f64, extent: f64| {
        if symmetric {
            return mid - extent / 2.0;
        }
        match hold {
            None => 0.0,
            Some(held) if at < held => held - extent,
            Some(held) => held,
        }
    };
    // **Whether the pointer went past the thing it was anchored to** (§15 D753).
    //
    // 🚨 **Not `at < held`.** That is `edge`'s question — *which side of the anchor
    // does the box lie on* — and it is the same as "crossed" only for a handle on
    // the high side. Drag `Handle::Left` (which holds the **right** edge) past that
    // edge and `at > held`, with the box still to the right of it; a crossing test
    // written as `at < held` would report every ordinary left drag as a flip and no
    // left flip at all.
    //
    // The direction the box extends from its anchor is `span - 2 * held`, since
    // `held` is 0 or `span` — positive for a handle on the high side, negative for
    // one on the low side. A crossing is the pointer's offset from the anchor
    // having the *opposite* sign to that.
    //
    // ⚠️ **With Alt the anchor is the centre**, and the same product answers it —
    // measured, not assumed: a `Path` dragged past its own centre with Alt held
    // comes back `Mirror::X` exactly as it does past an edge, so the two cases are
    // one rule and not two.
    let crossed = |hold: Option<f64>, at: f64, mid: f64, span: f64| match hold {
        None => false,
        Some(held) => {
            let anchor = if symmetric { mid } else { held };
            (at - anchor) * (span - 2.0 * held) < 0.0
        }
    };
    (
        Point::new(
            edge(hold_x, p.x, centre.x, w),
            edge(hold_y, p.y, centre.y, h),
        ),
        Size::new(w, h),
        (
            crossed(hold_x, p.x, centre.x, size.width),
            crossed(hold_y, p.y, centre.y, size.height),
        ),
    )
}

/// Resize `id` by dragging `handle` to `pointer_world`, holding the opposite
/// corner or side still.
///
/// Per §5.6 this edits *geometry*, never the transform's scale — stroke widths
/// and children are unaffected, matching Figma. Dragging a top/left handle also
/// translates the node, since geometry always starts at the local origin: the
/// box shrinks toward the anchor and the origin moves to meet it.
///
/// `opts.keep_ratio` (Shift) locks the aspect ratio to the box's current one;
/// `opts.symmetric` (Alt) holds the box's **centre** still instead of the
/// opposite edge, so both sides move out together. The two compose: Alt+Shift
/// resizes proportionally about the centre. `opts.scaling` is what separates the
/// Scale tool from this one.
///
/// Not about the node's transform origin, which it briefly was — §15 D60.
pub fn resize_to_handle(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    handle: Handle,
    pointer_world: Point,
    opts: Resize,
) -> Transaction {
    let Some(node) = doc.get(id) else {
        return Transaction(vec![]);
    };
    let Some(drawn) = anchored_box(node.kind(), res.text_layout(id)) else {
        return Transaction(vec![]);
    };
    let Some(world) = res.world_transform(id) else {
        return Transaction(vec![]);
    };
    let size = drawn.size();

    // Everything below is in the node's own local space, so rotation and the
    // parent chain are handled by the inverse world transform alone.
    //
    // **Shifted into the drawn box's own space**, which is a no-op for everything
    // except a trimmed auto text node — the one case whose box does not start at
    // the local origin ([`anchored_box`]). `resized_box` measures from `(0, 0)`, so
    // without this a top or bottom drag on one is out by the trim: the gesture held
    // an edge the handles never drew.
    let inset = drawn.origin().to_vec2();
    let (origin, new_size, mirrored) = resized_box(
        size,
        handle,
        (world.inverse() * pointer_world) - inset,
        opts.keep_ratio,
        opts.symmetric,
    );

    let geometry = match node.kind() {
        // A dragged text box wraps at the width the gesture gave it. Whether its
        // **height** also becomes the user's is the *handle's* business, not the
        // gesture's: a side handle owns one axis and says nothing about `y`
        // (`Handle::anchored`), so it leaves the height as it found it. That is
        // the auto-height mode this used to be unable to reach — dragging the
        // right edge of an auto-width node gives it a wrap width and lets the
        // lines keep deciding the height, which is what Figma does. A corner, or
        // a top/bottom handle, owns `y`; dragging one is the gesture that makes
        // the height authored.
        //
        // Already-`Fixed` text keeps its box either way: the rule is about
        // *height authorship*, and a side drag on a fixed box changes neither the
        // mode nor `new_size.height`.
        //
        // The rule itself is [`resized_text_sizing`], because the inspector's W and
        // H fields are the same two gestures with the pointer typed in.
        // **On a rail, a resize edits the rail** (§15 D405). It has to edit
        // *something*: `TextSizing` is inert here, so the arm below would hand back
        // a patch the layout ignores and the handles would drag with nothing
        // moving — a dead control on a selected layer, which is worse than no
        // handle at all. And it is the right thing rather than the available one:
        // §5.6's rule is that this gesture edits **geometry** and never scale, and
        // on a railed node the geometry *is* the rail. The type keeps its size and
        // re-flows along the new curve, which is what a longer path means.
        //
        // ⚠️ **Solved against the type this transaction will *leave*, not the type
        // it found** (§15 D604, `[S13.2-L1-03]`). Under the Scale tool
        // `scale_scalars` below multiplies the font size by the same `s`, so a
        // solve against the current kind converges on a box the document never
        // ends up with. Measured before the fix, `BottomRight` to 2× on the same
        // fixture the Select control uses: the held top-left corner slid
        // `(9.58, 15.00)` out from under the pointer and the height overshot by
        // 26%. The Select tool's control lands the anchor to the bit, which is
        // what says the solve is right and the composition was wrong.
        NodeKind::Text {
            on_path: Some(rail),
            ..
        } => {
            let solve_against = kind_to_solve_against(
                node.kind(),
                opts.scaling,
                ratio(size.width, new_size.width),
                ratio(size.height, new_size.height),
            );
            GeometryPatch::TextPath(Some(railed_resize(
                solve_against.as_ref().unwrap_or(node.kind()),
                rail,
                Rect::from_origin_size(inset.to_point(), size),
                Rect::from_origin_size(inset.to_point() + origin.to_vec2(), new_size),
                // The corner this gesture promised to hold (§15 D789).
                handle,
            )))
        }
        NodeKind::Text { sizing, .. } => {
            let (_, holds_y) = handle.anchored(size.width, size.height);
            GeometryPatch::TextSizing(resized_text_sizing(*sizing, new_size, holds_y.is_some()))
        }
        _ => GeometryPatch::Size(new_size),
    };

    // See the rail arm above: it wrote the move into the geometry, so the shift
    // below would apply it a second time.
    let geometry_moved_itself = matches!(geometry, GeometryPatch::TextPath(_));
    let mut ops = vec![Operation::SetGeometry { id, geometry }];
    // **`inset` goes back on, because the box being written starts at the origin
    // even when the box being measured did not.** A trimmed auto node hands its
    // trim in as the shift above and takes it back here, which is what keeps the
    // two edges the gesture did *not* touch where they were drawn; a `Fixed` box,
    // and every other kind, has a zero inset and this is the shift it always was.
    let shift = origin.to_vec2() + inset;
    // **The flip, about the box the gesture just produced** (§15 D753).
    //
    // `resized_box` has already put the box on the far side of the anchor, so what
    // is missing is only the *content* being turned over inside it — and a
    // reflection about the new box's own centre maps that box onto itself and does
    // exactly that. Reflecting about the **anchor** instead would move the box a
    // second time, since the placement is already in `origin`.
    //
    // ⚠️ **Composed on the right**, because it is expressed in the node's *new*
    // local space — the one `shift` establishes. On the left it would reflect the
    // parent's axes and carry the layer across the page.
    //
    // ⚠️ **Not on the rail arm.** `geometry_moved_itself` marks the case where the
    // geometry wrote its own placement (`GeometryPatch::TextPath`); type on a path
    // is positioned by the curve rather than by a box, so a flip there is §15
    // D406's question — the `on_path_flip` field — and not this one.
    let mirror = |on: bool, centre: f64| -> Affine {
        if !on {
            return Affine::IDENTITY;
        }
        Affine::translate((centre, 0.0))
            * Affine::scale_non_uniform(-1.0, 1.0)
            * Affine::translate((-centre, 0.0))
    };
    let flip = if geometry_moved_itself {
        Affine::IDENTITY
    } else {
        // The y reflection is the x one with the axes swapped, so it is spelled by
        // conjugating with a transpose rather than by a second closure that could
        // drift from the first.
        let swap = Affine::new([0.0, 1.0, 1.0, 0.0, 0.0, 0.0]);
        mirror(mirrored.0, inset.x + new_size.width / 2.0)
            * (swap * mirror(mirrored.1, inset.y + new_size.height / 2.0) * swap)
    };
    let moved = shift.x.abs() > f64::EPSILON || shift.y.abs() > f64::EPSILON;
    if !geometry_moved_itself && (moved || flip != Affine::IDENTITY) {
        ops.push(Operation::SetTransform {
            id,
            transform: node.transform() * Affine::translate(shift) * flip,
        });
    }
    // The box came from the node's own extent, so the factors the scalars take are
    // the ratio between the two — the leaf case has no `csx`/`csy` handed to it.
    scale_scalars(
        doc,
        id,
        opts.scaling,
        ratio(size.width, new_size.width),
        ratio(size.height, new_size.height),
        &mut ops,
    );
    Transaction(ops)
}

/// The box `rail` would give this node — the real thing, through the same layout
/// the canvas draws.
///
/// **Shared by [`railed_resize`]'s own correction loop and by
/// [`scale_geometry`]** (§15 D472), which needs the *current* rail's box to know
/// what a residual scale factor is a scale **of**. Every cheaper model of this
/// box is what `railed_resize`'s own note is about: on a rail the type keeps its
/// size and re-flows, so the box is a function of the curve and the shaping and
/// nothing simpler.
fn railed_box(kind: &NodeKind, rail: &ondin_core::kurbo::BezPath) -> Option<Rect> {
    let content = ondin_core::TextRef::of(kind)?.content.to_owned();
    let mut parts = ondin_core::TextParts::of(kind)?;
    parts.on_path = Some(rail.clone());
    Some(ondin_core::text::layout(parts.as_ref(&content)).bounds())
}

/// The rail a resize from `drawn` to `want` leaves a text node with, both boxes in
/// the node's own local space (§15 D405).
///
/// ⚠️ **This *shapes*, twice, which no other arm of `resize_to_handle` does — and
/// the reason is that a railed node's box is not a linear function of its rail.**
/// The box is the ribbon the rail sweeps: the curve, plus a normal offset at every
/// point. A normal is a unit vector, so it does not scale with the curve under it,
/// and a non-uniform scale turns the tangents as well. Scaling the rail by the
/// ratio of the two boxes therefore lands somewhere else, and it does so on
/// **both** counts: measured on a 150-long rail with a gentle S, asking for a box
/// of 317.2 wide gave 304.7 — 4% short — and the top-left corner the drag was
/// holding still moved 9.8 units to the right.
///
/// The drift of the held corner is the one that could not stand: a corner drag is
/// a promise about the *opposite* corner, and a layer that slides out from under
/// the pointer reads as a broken gesture rather than an imprecise one.
///
/// So the factor is solved rather than computed: scale, measure the box that
/// produced, correct, and once more — which converges because the normal offset is
/// a bounded constant beside a term that scales. Then the whole rail is translated
/// so the corner `hold` holds lands exactly where `want` puts it — 🚨 **the box's
/// own origin only when that is the held corner, which is `BottomRight` and no
/// other** (§15 D789; this line read *"the measured box's own origin lands exactly
/// on `want`'s"* and was the bug). **The placement comes out exact and the extent
/// comes out close**, which is the right way round: the anchor is a promise and the
/// far corner is a target.
///
/// Two shapings per gesture frame, on one node, of text that is already being
/// re-shaped by the drag. Cheap against being wrong.
///
/// ⚠️ **This comment spent one commit sitting on [`railed_box`]**, which was
/// inserted above it and took it — `CLAUDE.md`'s *anchor above, never below* trap,
/// committed by the session that spent the day fixing that class elsewhere. Found
/// by `arch-scribe` reading the change, not by any gate: the merged run compiles,
/// tests, lints, formats and passes `cargo doc`.
fn railed_resize(
    kind: &NodeKind,
    rail: &ondin_core::kurbo::BezPath,
    drawn: Rect,
    want: Rect,
    hold: Handle,
) -> ondin_core::kurbo::BezPath {
    // The box this rail would give *this node* — the real thing, through the same
    // layout the canvas draws, because every cheaper model of it is what the note
    // above is about.
    let measure =
        |candidate: &ondin_core::kurbo::BezPath| -> Option<Rect> { railed_box(kind, candidate) };
    let about = drawn.origin().to_vec2();
    let scaled = |sx: f64, sy: f64| -> ondin_core::kurbo::BezPath {
        (Affine::translate(about) * Affine::scale_non_uniform(sx, sy) * Affine::translate(-about))
            * rail.clone()
    };

    let (mut sx, mut sy) = (
        ratio(drawn.width(), want.width()),
        ratio(drawn.height(), want.height()),
    );
    let mut out = scaled(sx, sy);
    // Two corrections. One leaves a residual an order smaller than the first
    // guess's; a third has never moved the third decimal on any fixture here.
    for _ in 0..2 {
        let Some(got) = measure(&out) else { return out };
        sx *= ratio(got.width(), want.width());
        sy *= ratio(got.height(), want.height());
        out = scaled(sx, sy);
    }
    // And the anchor, exactly: whatever the extent came out as, the corner the
    // gesture held is where it said it would be.
    //
    // 🚨 **`hold`, not the origin — and for three of the four corners those are
    // different points** (§15 D789). This read
    // `Affine::translate(want.origin() - got.origin())`, which pins the box's
    // **top-left**. That is the held corner for exactly one handle:
    // `BottomRight`. Drag `BottomLeft` and the gesture holds the top-*right*, so
    // pinning the origin fixes `x0` and lets `x1 = x0 + got.width()` carry the
    // extent residual — and the residual is not small, because this solve is
    // deliberately *close* on extent and exact on placement. Measured on the
    // maintainer's own document, `BottomLeft`: the held corner slid **+0.41 units
    // at 55%, +2.61 at 25% and +10.37 at 10%**, matching the width residual at
    // every step to a hundredth. **The comment above was true of the code and
    // false of three handles out of four.**
    //
    // [`Handle::anchored`] already answers this: per axis it gives the coordinate
    // the handle holds still, measured from the box's own origin, or `None` for an
    // axis it does not touch. Asking it of **each** box is what matters — the held
    // edge is `origin + width` on one side and `origin + 0` on the other, so the
    // two boxes' offsets differ by exactly the residual being corrected for.
    let anchor = |r: Rect| {
        let (hx, hy) = hold.anchored(r.width(), r.height());
        r.origin() + Vec2::new(hx.unwrap_or(0.0), hy.unwrap_or(0.0))
    };
    match measure(&out) {
        Some(got) => Affine::translate(anchor(want) - anchor(got)) * out,
        None => out,
    }
}

/// `after / before`, or 1 when there is no extent to have scaled.
fn ratio(before: f64, after: f64) -> f64 {
    if before.abs() < MIN_EXTENT {
        return 1.0;
    }
    after / before
}

/// Resize the single layer `id` by dragging `handle` to `pointer_world`, choosing the
/// path its kind needs.
///
/// The routing `canvas::resize_tx` applies to one layer, spelled here so the inspector's
/// W/H fields can reuse it — a typed number and a dragged handle must not be able to mean
/// two different things about the same shape. Three answers:
///
/// - an **authored** `size` is replaced ([`resize_to_handle`]);
/// - a **childless leaf** whose shape *is* its geometry has that scaled about the held
///   corner ([`resize_geometry`]) — a `Path`, a `Line`;
/// - anything else is a **container** and the scale recurses into its contents
///   ([`resize_group`]) — a `Group`, a `Boolean`, a `Frame` with children.
///
/// The test between the last two is structural rather than a list of kinds: an empty
/// group answers "childless" and gets the nothing it has always got, since
/// `scale_geometry` recurses into contents it does not have.
pub fn resize_layer(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    handle: Handle,
    pointer_world: Point,
    opts: Resize,
) -> Transaction {
    let Some(node) = doc.get(id) else {
        return Transaction::default();
    };
    if resizable_size(node.kind()).is_some() {
        return resize_to_handle(doc, res, id, handle, pointer_world, opts);
    }
    let Some(local) = ondin_core::local_box(doc, res, id) else {
        return Transaction::default();
    };
    if node.children().is_empty() {
        resize_geometry(doc, res, id, local, handle, pointer_world, opts)
    } else {
        resize_group(doc, res, id, local, handle, pointer_world, opts)
    }
}

/// Whether a resize over `ids` has to keep its proportions: **§15 D50's all-or-nothing
/// rule, in one place**.
///
/// A set is only as constrained as its least constrained member. Taking *any* locked
/// layer as locking the whole box would let one member silently constrain an edit the
/// others never asked to have constrained, with no way to see which one did it;
/// all-or-nothing can surprise nobody. Empty is not locked, which is what stops an empty
/// selection reading as "every member is locked" through a vacuous `all`.
///
/// ⚠️ **Lifted out of `canvas::keep_ratio` when the rule acquired a third reader, and
/// that is the whole reason it is a function.** D50 was decided for the *handles* and
/// spelled inline there, so the two numeric doors onto the same box — the inspector's
/// multi W/H fields and `Ctrl`+arrows — simply did not consult it: dragging a corner over
/// a set of locked layers kept their proportions and typing a number into the field
/// beside it did not. A rule written once in the place that first needed it is a rule the
/// next door has to notice in order to obey.
///
/// Pass the **whole selection**, not [`build::outermost`]'s reduction of it: the question
/// is what the user has picked, and a group whose child is also selected is a set of two
/// as far as "did every member ask for this" goes.
pub fn proportions_locked(doc: &Document, ids: &[NodeId]) -> bool {
    !ids.is_empty()
        && ids
            .iter()
            .all(|id| doc.get(*id).is_some_and(|n| n.proportions_locked()))
}

/// The size a locked box wants, given the size that was asked for and the box it has
/// now: the axis the caller drove keeps its number and the other follows the aspect.
///
/// **One expression for all three numeric doors** — the single card's W and H fields, the
/// multi card's, and the keyboard's `Ctrl`+arrows — because "make it 4 wider" typed and
/// pressed must not come out as two different boxes.
///
/// ⚠️ **`drove_x` is which field the *user* touched, and it cannot be inferred from the
/// two sizes.** Asking "which axis differs from the current box" is wrong on the press
/// that lands a paired value exactly on the other axis's current number, and wrong again
/// when a floor has already clamped the driven axis to something it was already at.
///
/// Both guards are the panel's, in the panel's order: a box with no width has no ratio to
/// keep and falls back to 1, and a zero aspect cannot be divided by, so a height-driven
/// edit on one leaves the width alone. The floor is applied to whichever value this
/// derives, never to the one the caller asked for — that is the caller's own.
pub fn paired_size(now: Size, want: Size, drove_x: bool) -> Size {
    let aspect = if now.width > 0.0 {
        now.height / now.width
    } else {
        1.0
    };
    match (drove_x, aspect > 0.0) {
        (true, _) => Size::new(want.width, (want.width * aspect).max(1.0)),
        (false, true) => Size::new((want.height / aspect).max(1.0), want.height),
        (false, false) => Size::new(now.width, want.height),
    }
}

/// Make `id`'s box exactly `want`, holding its top-left corner — the inspector's W and H
/// fields for a shape whose extent is **derived** rather than authored.
///
/// Expressed as a synthesized bottom-right handle drag rather than as its own arithmetic,
/// which is the point: a `Path`'s width has no field to assign, so the only definition of
/// "make it 200 wide" is the one the handle already implements, and going through
/// [`resize_layer`] means the two cannot drift. The pointer is placed at the corner the
/// new box wants, in world space, because that is what dragging there would have done.
pub fn resize_box_to(doc: &Document, res: &Resolved, id: NodeId, want: Size) -> Transaction {
    let Some(local) = ondin_core::local_box(doc, res, id) else {
        return Transaction::default();
    };
    let Some(world) = res.world_transform(id) else {
        return Transaction::default();
    };
    let corner = world * Point::new(local.x0 + want.width, local.y0 + want.height);
    resize_layer(
        doc,
        res,
        id,
        Handle::BottomRight,
        corner,
        // Geometry only, and neither modifier: a typed W must not multiply the stroke
        // widths (§5.6), and the aspect pairing the panel does itself — the proportion
        // lock is applied to `want` before it gets here, so `keep_ratio` would apply it
        // twice.
        Resize::geometry(false, false),
    )
}

/// Put one end of the line `id` at the world point `target`.
///
/// **The two ends are not symmetric, because the model stores only one of them.** A
/// `Line` keeps its far point; the near one *is* the node's origin (§5.3). So moving
/// the end is a single `LineEnd` patch, and moving the start is a transform edit — the
/// translation moves, the basis does not — plus a re-expressed `end`, so the far point
/// stays exactly where it was instead of travelling with the origin. Keeping the basis
/// is what lets a line that has been rotated or skewed keep that while one point moves.
///
/// Everything is read off the **committed** tree. The caller has already decided where
/// `target` is (snapped, or constrained by Shift); re-deriving an endpoint from the
/// preview mid-drag would compound it every frame, which is the rule `resize_tx` states
/// for its box.
pub fn move_line_end(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    which: LineEnd,
    target: Point,
) -> Transaction {
    let Some(node) = doc.get(id) else {
        return Transaction::default();
    };
    let NodeKind::Line { end } = *node.kind() else {
        return Transaction::default();
    };
    let Some(world) = res.world_transform(id) else {
        return Transaction::default();
    };
    match which {
        LineEnd::End => Transaction(vec![Operation::SetGeometry {
            id,
            geometry: GeometryPatch::LineEnd(world.inverse() * target),
        }]),
        LineEnd::Start => {
            let end_world = world * end;
            let parent_world = node
                .parent()
                .and_then(|p| res.world_transform(p))
                .unwrap_or(Affine::IDENTITY);
            let local = parent_world.inverse() * target;
            // The basis kept, the translation replaced: a local transform's last two
            // coefficients *are* the node's origin in its parent's space.
            let [a, b, c, d, _, _] = node.transform().as_coeffs();
            let transform = Affine::new([a, b, c, d, local.x, local.y]);
            Transaction(vec![
                Operation::SetTransform { id, transform },
                Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::LineEnd(
                        (parent_world * transform).inverse() * end_world,
                    ),
                },
            ])
        }
    }
}

/// Where the two ends of the line `id` are in world space, near end first.
///
/// Spelled here rather than at the two call sites — the chrome and the constraint
/// anchor — because "the start is the origin" is a model fact, and a second reading of
/// it is a second place to get the rotated case wrong.
pub fn line_ends_world(doc: &Document, res: &Resolved, id: NodeId) -> Option<(Point, Point)> {
    let node = doc.get(id)?;
    let NodeKind::Line { end } = *node.kind() else {
        return None;
    };
    let world = res.world_transform(id)?;
    Some((world * Point::ZERO, world * end))
}

/// Resize a **leaf whose geometry is its own shape** — a `Line` or a `Path` — by
/// dragging `handle`.
///
/// The third of the three resize paths, and the one that was missing. A `Rect` has an
/// authored `size` and goes through [`resize_to_handle`]; a container's *contents*
/// scale and it goes through [`resize_group`]; a line and a pen path have neither an
/// authored box nor children, so they fell through `resize_group` to `scale_subtree`,
/// which iterates children — a line has none, so the transaction came out **empty**.
/// The handles were drawn, they armed, they showed the right cursor, and a line's
/// length and a path's shape were frozen after release.
///
/// `scale_geometry` could always do the work (it has had `Line` and `Path` arms since
/// multi-selection resize was built, which is why dragging one of these *inside* a
/// selection of two always worked). What was missing is the anchor arithmetic, and it
/// is the whole of this function:
///
/// - the geometry scales about the node's **origin** — that is all `scale_geometry`
///   can do, and a path's bounding box is usually nowhere near its origin;
/// - so the node's transform takes the correction `anchor·(1 − S)`, which is exactly
///   what puts the held corner back where it was.
///
/// [`scaled_about`] on the identity is that pair, and [`split_geometry_scale`] divides
/// it the way §5.6 requires — a mirror through a handle lands in the transform as a
/// flip rather than as a negative size.
pub fn resize_geometry(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    local: Rect,
    handle: Handle,
    pointer_world: Point,
    opts: Resize,
) -> Transaction {
    let Some(world) = res.world_transform(id) else {
        return Transaction(vec![]);
    };
    let Some(node) = doc.get(id) else {
        return Transaction(vec![]);
    };
    let (sx, sy, anchor) = resize_factors(
        local,
        handle,
        world.inverse() * pointer_world,
        opts.keep_ratio,
        opts.symmetric,
    );
    // The identity's coefficients are 0 and 1 and `sx`/`sy` are `MAX_SCALE`-capped,
    // so this arm cannot overflow — but it is spelled rather than unwrapped,
    // because the reason is about *this* call site and not about `scaled_about`
    // (§15 D481). The `?`-free `else` is a refusal to edit, which is the same
    // answer the two loops give per node.
    let Some(scaled) = scaled_about(Affine::IDENTITY, anchor, sx, sy) else {
        return Transaction(Vec::new());
    };
    // A basis with no exact split refuses the edit, exactly as the overflow
    // above it does (§15 D714).
    let Some((rest, csx, csy)) = split_geometry_scale(scaled) else {
        return Transaction(Vec::new());
    };
    let mut ops = Vec::new();
    let before = node.transform();
    let transform = before * rest;
    if transform.as_coeffs() != before.as_coeffs() {
        ops.push(Operation::SetTransform { id, transform });
    }
    scale_geometry(doc, id, csx, csy, opts.scaling, &mut ops);
    Transaction(ops)
}

/// Resize a **container** by dragging `handle`, scaling everything inside it.
///
/// §5.6's group-resize rule: the edit recurses into geometry rather than
/// multiplying a scale into the group's transform, so stroke widths, corner
/// radii and text sizes are unaffected and a resized group is still a group of
/// ordinary nodes. Each descendant's position within the group scales with the
/// box; each leaf's own geometry scales with it too.
///
/// **Exact at every angle**, including the case that used to be approximated: a
/// descendant rotated off-axis inside a group scaled *non-uniformly*. Squashing
/// a rotated rectangle horizontally turns it into a parallelogram, and the
/// transform can now say so — [`split_geometry_scale`] puts the skew in the
/// child's basis and the rest in its geometry, which between them reproduce the
/// scale exactly rather than evening it out to a geometric mean (§15 D50).
pub fn resize_group(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    local: Rect,
    handle: Handle,
    pointer_world: Point,
    opts: Resize,
) -> Transaction {
    let Some(world) = res.world_transform(id) else {
        return Transaction(vec![]);
    };
    // A container's box is its children's union, which is the space its own pivot
    let (sx, sy, anchor) = resize_factors(
        local,
        handle,
        world.inverse() * pointer_world,
        opts.keep_ratio,
        opts.symmetric,
    );
    let mut ops = Vec::new();
    scale_subtree(doc, id, anchor, sx, sy, opts.scaling, &mut ops);
    Transaction(ops)
}

/// What a handle drag was asked for, beyond where the pointer is: the two
/// modifiers and the tool's verb.
///
/// One struct rather than three trailing `bool`s and an enum. The run was already
/// eight positional arguments on [`resize_selection`] with two
/// `#[allow(clippy::too_many_arguments)]` holding it up, and §15 D41 asked for a
/// parts struct before another was added — so this is that, before the ninth.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Resize {
    /// Shift, or a layer with its proportions locked: hold the box's aspect ratio.
    pub keep_ratio: bool,
    /// Alt: hold the box's own **centre** still rather than the opposite edge.
    pub symmetric: bool,
    /// Select's verb or Scale's.
    pub scaling: Scaling,
}

impl Resize {
    /// A §5.6 resize — geometry only, which is what every gesture but the Scale
    /// tool's asks for.
    pub fn geometry(keep_ratio: bool, symmetric: bool) -> Self {
        Self {
            keep_ratio,
            symmetric,
            scaling: Scaling::Geometry,
        }
    }
}

/// The scale factors a handle drag implies for `box_`, and the point held still
/// while they are applied — both in whatever space `box_` and `p` are measured
/// in.
///
/// Split out of [`resize_group`] because a multi-selection needs exactly this
/// arithmetic in *world* space, over an upright union box, and a second copy of
/// the anchor rules is a second place for Alt to stop meaning what it means
/// ([`resize_selection`]).
///
/// `symmetric` (Alt) holds `box_`'s own centre, not the node's transform origin —
/// §15 D60.
fn resize_factors(
    box_: Rect,
    handle: Handle,
    p: Point,
    keep_ratio: bool,
    symmetric: bool,
) -> (f64, f64, Point) {
    // The coordinate held still on each axis — the opposite corner for a corner
    // handle, the opposite side for a side one, and (`None`) nothing at all on
    // the axis a side leaves alone.
    let (hold_x, hold_y) = handle.anchored(box_.width(), box_.height());
    let hold_x = hold_x.map(|x| box_.min_x() + x);
    let hold_y = hold_y.map(|y| box_.min_y() + y);

    // Scale factors about the anchor: how far the pointer now is from it, over how
    // far the corner being dragged *started* from it.
    //
    // **Both distances are signed, and the second one is what was wrong.** A
    // handle on the box's *minimum* side sits at −extent from its anchor, not at
    // +extent, so dividing by the box's own width turned every drag of the
    // top-left corner — including one that had not moved at all — into a factor of
    // −1: the selection mirrored across its own bottom-right corner the instant
    // the handle was grabbed. It went unseen because `resized_box` handles the
    // common case (a `Rect` and its relatives take their `.abs()` and reflect
    // through their own edge rule), so the only callers here are a **group**, a
    // **multi-selection** and a point box — and every test of those used
    // `BottomRight`, the one handle whose sign happens to come out right.
    // `signed_from_anchor` states the distance once, for both the anchored case
    // and Alt's.
    //
    // The *pointer's* sign still means what it always meant: dragged past the
    // corner it is anchored to, the factor goes negative and the shape mirrors,
    // which is what the user is asking for. `split_geometry_scale` knows where to
    // put one — `Basis::of` peels the reflection into the transform's orientation
    // and hands the geometry a positive scale, which is §5.6's rule.
    let centre = box_.center();
    let (ux, uy) = handle.unit();
    // Anchored: the dragged corner starts a whole extent from the far side, on the
    // side the handle is on. Symmetric: half an extent from the middle.
    let signed_from_anchor = |u: f64, extent: f64| match symmetric {
        true => (u - 0.5) * extent,
        false => (2.0 * u - 1.0) * extent,
    };
    let (mut sx, mut sy) = if symmetric {
        (
            hold_x.map_or(1.0, |_| {
                scale_along(signed_from_anchor(ux, box_.width()), p.x - centre.x)
            }),
            hold_y.map_or(1.0, |_| {
                scale_along(signed_from_anchor(uy, box_.height()), p.y - centre.y)
            }),
        )
    } else {
        (
            hold_x.map_or(1.0, |hx| {
                scale_along(signed_from_anchor(ux, box_.width()), p.x - hx)
            }),
            hold_y.map_or(1.0, |hy| {
                scale_along(signed_from_anchor(uy, box_.height()), p.y - hy)
            }),
        )
    };
    if keep_ratio {
        // Only the axes the handle actually drives have an opinion; a side
        // handle's locked axis must follow rather than pin the pair to 1.
        let s = match (hold_x.is_some(), hold_y.is_some()) {
            (true, true) => sx.abs().max(sy.abs()),
            (true, false) => sx.abs(),
            (false, true) => sy.abs(),
            (false, false) => 1.0,
        };
        // The *magnitude* is shared and each axis keeps its own sign, so a proportional
        // drag can still flip — and flip one axis only, which is what dragging a corner
        // sideways past its anchor means.
        sx = s.copysign(sx);
        sy = s.copysign(sy);
    }

    // A side drag leaves one axis alone, so its anchor is arbitrary — the box's
    // own minimum keeps the untouched coordinates exactly where they are. On an
    // axis the handle *does* drive, `symmetric` pins the centre instead of the
    // far edge, which is the whole of what Alt means here.
    //
    // ⚠️ **`symmetric` pins the centre on *both* axes, and asking `hold` first is
    // what was wrong** (§15 D559). Centring an axis the handle does not own is a
    // no-op in the ordinary case — its factor is 1, so scaling about the middle
    // and scaling about the minimum put every coordinate back where it was — but
    // it is not a no-op once `keep_ratio` has driven that axis from the other
    // one, eleven lines above. That is Alt+Shift on a *side* handle, and pinning
    // the minimum there centres the axis being dragged while anchoring the top
    // edge of the axis following it: a 100 × 50 path dragged to x = 80 came out
    // at `(20, 0) – (80, 30)` where the same gesture on a `Rect` gives
    // `(20, 10) – (80, 40)`. [`resized_box`]'s `edge` closure returns
    // `mid - extent / 2.0` before it looks at `hold` for exactly this reason, and
    // its comment says so; this is that rule, in the copy that did not have it.
    let pin = |hold: Option<f64>, mid: f64, min: f64| match hold {
        _ if symmetric => mid,
        Some(held) => held,
        None => min,
    };
    let anchor = Point::new(
        pin(hold_x, centre.x, box_.min_x()),
        pin(hold_y, centre.y, box_.min_y()),
    );
    (sx, sy, anchor)
}

/// The box a whole selection is resized about: the frame it is turned by, and its
/// extent measured in that frame's own axes (§15 D326, D478).
///
/// **One parameter rather than a seventh and an eighth**, which is [`Resize`]'s own
/// argument and §15 D41's rule: the two are never meaningful apart, and handing them
/// over separately is a second place for a caller to pass a box measured in one space
/// with a frame belonging to another.
///
/// [`Self::upright`] is the world-axis reading — a plain AABB with no orientation on
/// it — and it is what the inspector's W/H fields still ask for, because the number
/// those fields *report* for a set is the upright union. The canvas asks for the
/// oriented one, because that is the box its handles are drawn on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SelectionBox {
    /// The transform the set is turned by — `Affine::IDENTITY` for an upright one.
    pub frame: Affine,
    /// The extent, measured in `frame`'s own axes rather than the world's.
    pub extent: Rect,
}

impl SelectionBox {
    /// A set measured on the **world** axes: the upright union, with no orientation.
    ///
    /// Exactly the arithmetic this function did before D326's resize half was
    /// closed, and it is preserved for the callers whose *reported* box is the
    /// upright one — an identity frame makes every `frame⁻¹` below a no-op, so the
    /// result is bit-for-bit what it was.
    pub fn upright(union: Rect) -> Self {
        Self {
            frame: Affine::IDENTITY,
            extent: union,
        }
    }
}

/// Resize a whole selection about one box drawn over all of it.
///
/// `roots` must already be [`build::outermost`] — the members with no ancestor in
/// the selection — or a layer inside a selected group would be scaled twice, once
/// on its own and once by its container (§5.7a).
///
/// **The factors and the anchor are computed in `sel.frame`'s space, not the
/// world's, and that is what §15 D326's second half asked for** (§15 D478). The pointer comes
/// in world coordinates and is mapped through `frame⁻¹` before it is measured; each
/// root is scaled in that same space and mapped back out. With an upright
/// [`SelectionBox::upright`] the two maps are the identity and this is the old
/// world-space arithmetic unchanged — which is what makes the inspector's callers
/// safe. With a turned one it is the difference between a corner landing on the
/// pointer and landing 62 units away from it, because the chrome draws its handles
/// on `frame` and the arithmetic used to measure the upright union of world bounds.
///
/// From there this is `resize_group`'s per-child step with the frame in place of a
/// shared parent: each root is carried bodily through the scale and then split
/// into the transform it may keep and the scale its geometry takes
/// ([`split_geometry_scale`]). A root rotated off-axis under a non-uniform scale
/// needs a skew, and gets one — which matters more here than inside a group,
/// since a selection is exactly where unrelated, differently-turned layers end
/// up together.
pub fn resize_selection(
    doc: &Document,
    res: &Resolved,
    roots: &[NodeId],
    sel: SelectionBox,
    handle: Handle,
    pointer_world: Point,
    opts: Resize,
) -> Transaction {
    let to_frame = sel.frame.inverse();
    let (sx, sy, anchor) = resize_factors(
        sel.extent,
        handle,
        to_frame * pointer_world,
        opts.keep_ratio,
        opts.symmetric,
    );
    let mut ops = Vec::new();
    for id in roots {
        let Some(node) = doc.get(*id) else { continue };
        let Some(world) = res.world_transform(*id) else {
            continue;
        };
        let parent_world = node
            .parent()
            .and_then(|p| res.world_transform(p))
            .unwrap_or(Affine::IDENTITY);
        // The root carried bodily through the scale, projected back into its own
        // parent, and only *then* split. Splitting in world space and projecting
        // the result afterwards would leave the parent's basis wrapped round the
        // outside of it, which is scale back in the transform by another route.
        //
        // The scale happens in the frame's space and comes straight back out of
        // it: `frame · S · frame⁻¹ · world`. Written as a world-space scale with
        // the frame folded into `anchor` instead, it would still be scaling along
        // the world's axes and only the anchor would have moved — which is the
        // half-fix that leaves the dragged corner off the pointer.
        // A member whose basis overflows is left alone and the rest of the
        // selection still resizes (§15 D481).
        let Some(scaled) = scaled_about(to_frame * world, anchor, sx, sy) else {
            continue;
        };
        let desired = sel.frame * scaled;
        // And a member whose basis has no exact split is skipped for the same
        // reason the overflowing one is (§15 D714).
        let Some((transform, csx, csy)) = split_geometry_scale(parent_world.inverse() * desired)
        else {
            continue;
        };
        if transform.as_coeffs() != node.transform().as_coeffs() {
            ops.push(Operation::SetTransform { id: *id, transform });
        }
        scale_geometry(doc, *id, csx, csy, opts.scaling, &mut ops);
    }
    Transaction(ops)
}

/// A world-space turn of `angle` about `pivot`.
///
/// **The one expression, so the chrome and the shapes cannot disagree** (§15 D325,
/// and the same argument [`side_shear`] carries for a lean): a set's frame is drawn
/// with this affine while a rotation is in flight, and every member's new world
/// transform is this affine times its old one. Written twice, the frame and the
/// layers would turn by two angles that merely happen to match today.
pub fn turn_about(pivot: Point, angle: f64) -> Affine {
    Affine::translate(pivot.to_vec2()) * Affine::rotate(angle) * Affine::translate(-pivot.to_vec2())
}

/// Rotate a whole selection by `angle` about one shared `pivot`.
///
/// Each member turns about the *selection's* centre rather than its own, so the
/// set holds its arrangement and sweeps round as one thing — which is the only
/// reading that makes sense for a box drawn over all of it. Per-node this is
/// [`rotate_node`], and it composes because the rotation is applied in world
/// space before being projected back through each node's own parent.
///
/// `roots` must be [`build::outermost`], as for [`resize_selection`].
pub fn rotate_selection(
    doc: &Document,
    res: &Resolved,
    roots: &[NodeId],
    pivot: Point,
    angle: f64,
) -> Transaction {
    let mut ops = Vec::new();
    for id in roots {
        ops.extend(rotate_node(doc, res, *id, pivot, angle).0);
    }
    Transaction(ops)
}

/// `t` scaled by `(sx, sy)` about `anchor`, in whatever space `t` is measured
/// in. Both the origin and the basis take the scale; splitting the result back
/// into what a node may store is [`split_geometry_scale`]'s job.
fn scaled_about(t: Affine, anchor: Point, sx: f64, sy: f64) -> Option<Affine> {
    let [a, b, c, d, e, f] = t.as_coeffs();
    let out = Affine::new([
        a * sx,
        b * sy,
        c * sx,
        d * sy,
        anchor.x + (e - anchor.x) * sx,
        anchor.y + (f - anchor.y) * sy,
    ]);
    // ⚠️ **`None` is all-finite operands whose *product* overflows** (§15 D481,
    // `[S13.1-L1-03]`), which is a state no caller can rule out beforehand.
    // `MAX_SCALE` caps `sx` and `sy` — a bound on the *factor*, not on the
    // quantity that overflows. A node storing `Affine::scale(1e308)` with a size
    // of `1e-306` has entirely ordinary world bounds of `(0,0)–(100,100)`, and one
    // handle drag multiplies its basis to `inf`. The `inf` reaches `Basis::of`,
    // where `inf.hypot(inf) / inf` is `NaN`, and all four **basis** coefficients
    // of the matrix that comes back are `NaN` — pushed unconditionally, because
    // `transform.as_coeffs() != node.transform().as_coeffs()` is true of `NaN`.
    //
    // ⚠️ The *translation* pair is finite, being `(e, f)` off the scaled affine —
    // `(0, 0)` for that fixture. Said precisely because "every coefficient is
    // `NaN`" is the obvious shorthand and is wrong: it is still enough for the
    // `!=` and for `OpError::NonFinite`, but a reader chasing the translation
    // would find a number and conclude this was about something else.
    //
    // **Refusing per node rather than per gesture is the whole of the fix.**
    // Since D421 the op layer catches the `NaN` and `Document::apply` is atomic,
    // so what a user gets today is a resize of a whole selection silently doing
    // nothing because one member overflowed. Skipping that member leaves it
    // exactly where it was and lets the rest of the selection move.
    out.as_coeffs().iter().all(|v| v.is_finite()).then_some(out)
}

/// Split the transform a node is *intended* to have into the one it may store
/// and the scale its geometry takes instead (§5.6).
///
/// The two together reproduce `local` exactly — `Basis::exact` is an RQ
/// decomposition, which is exact for every **invertible** matrix — so a caller
/// that applies the returned transform *and* scales the node's geometry by the
/// returned factors lands the node precisely where the intended matrix put it.
/// Rotation, skew and flip stay in the transform; only scale is handed to the
/// geometry, which is the whole of §5.6's rule.
///
/// ⚠️ **`None` where no split reproduces `local`, and this used to return a wrong
/// one instead** (§15 D714). The sentence above read *"exact for every matrix"*
/// and dropped §5.6's own word: `Orientation::to_affine`'s determinant is always
/// ±1, so `orientation · diag(sx, sy)` can only ever offer a second column of
/// zero once `sy` is zero, and a **rank-1** basis has one that is not.
/// `split_geometry_scale(Affine::new([1, 2, 2, 4, 5, 6]))` came back
/// `sx = 2.236, sy = 0` — wrong by 4.0 in a coefficient — and
/// [`scale_geometry`] turned that `csy = 0` into `(height · 0).max(1.0)`, so the
/// layer was silently rewritten **1 unit tall at 63°**. `Affine::scale(1e-13)` is
/// the other arm: `sx = 0` with an identity orientation, and under
/// `Scaling::Photographic` `s = sqrt(0 · sy) = 0` zeroes every stroke width in
/// the node.
///
/// **Refusing per node rather than per gesture**, which is the answer §15 D481
/// already gives one function up: a member whose basis cannot be split is left
/// exactly where it was and the rest of the selection still moves. §6.2's rule
/// is the argument — a missing edit is recoverable and a lying one is not.
///
/// ⚠️ **Reachable only from outside.** `scale_along` floors every factor at
/// `MIN_EXTENT`, so no gesture manufactures a singular basis; it has to arrive
/// through `op_set_transform`, which validates nothing beyond finiteness, or
/// through `svg_in`'s verbatim `transform=`.
///
/// This is what replaced the geometric-mean fudge (§15 D50): the part of a
/// non-uniform scale that a turned node's own axes cannot absorb is a skew, and
/// a skew now has somewhere to live.
///
/// `local` must be the node's **local** transform, not its world one. Splitting
/// in world space and projecting afterwards puts the parent's basis back on the
/// outside of the result, which reintroduces exactly the scale this is removing.
fn split_geometry_scale(local: Affine) -> Option<(Affine, f64, f64)> {
    let [_, _, _, _, e, f] = local.as_coeffs();
    let basis = Basis::exact(local)?;
    Some((
        Affine::translate((e, f)) * basis.orientation.to_affine(),
        basis.scale.x,
        basis.scale.y,
    ))
}

/// A text node's box under a container scale of `(csx, csy)`, or `None` where
/// there is no box to scale.
///
/// **The mode is preserved, one axis at a time.** Which of `TextSizing`'s three
/// states a layer is in is the user's answer to "who decides this box", and a
/// scale is only allowed to multiply the extents that answer *the user* — so each
/// axis is scaled exactly when it is authored, and an axis the content owns is
/// left for the content to decide again at the new width. `Auto` therefore has
/// nothing here at all, `AutoHeight` has only its wrap width, and `Fixed` has
/// both.
///
/// ⚠️ **§5.6 states that rule in the design document's own voice, and named
/// [`railed_resize`] as its owner until 2026-09-15** (§15 D790). The sentence
/// there opened *"That function"*, and a passage inserted between it and its
/// antecedent rebound the pronoun without changing a character of it. On a rail
/// the mode is inert, so `railed_resize` has no `TextSizing` in it at all — this
/// is the only implementation of the rule, and §5.6 the only other statement of
/// it.
///
/// The clamp is [`scale_geometry`]'s: a factor of zero would otherwise author a
/// zero-width wrap and there would be no width left to scale back out of.
fn scaled_text_sizing(sizing: TextSizing, csx: f64, csy: f64) -> Option<TextSizing> {
    match sizing {
        TextSizing::Auto => None,
        TextSizing::AutoHeight(w) => Some(TextSizing::AutoHeight((w * csx).max(1.0))),
        TextSizing::Fixed(s) => Some(TextSizing::Fixed(Size::new(
            (s.width * csx).max(1.0),
            (s.height * csy).max(1.0),
        ))),
    }
}

/// The geometry edits that scale what `id` *is* by `(csx, csy)`, already measured
/// in `id`'s own axes.
///
/// A leaf's own size, a line's end, a path's points — or, for a container, the
/// same treatment recursed into its contents about its own origin, since a
/// container has no geometry to scale.
///
/// **Text answers first and on its own terms** — see [`scaled_text_sizing`].
/// Its box is a *mode*, and a scale is not allowed to change which one: turning
/// an auto-sized layer into a fixed box would be this function deciding the
/// layer's sizing as a side effect. The generic path below cannot do that job,
/// because it would read the zero extent an auto mode reports through
/// `resizable_size` as a real one and clamp it into a **1×1 box**, collapsing
/// the layer outright.
///
/// **Auto-sized text still takes the font size under [`Scaling::Photographic`].**
/// The early return is about its *box*; its type size exists either way, and a
/// scaled icon set with a label in it would otherwise come out with the label at
/// its original size. That is [`scale_scalars`]'s to do, before the return. Such
/// a layer also still *moves* with the box, because the transform half of the
/// edit is the caller's and happens regardless.
fn scale_geometry(
    doc: &Document,
    id: NodeId,
    csx: f64,
    csy: f64,
    scaling: Scaling,
    ops: &mut Vec<Operation>,
) {
    let Some(node) = doc.get(id) else { return };
    scale_scalars(doc, id, scaling, csx, csy, ops);
    // ⚠️ **On a rail, a resize edits the rail — through this door too**
    // (§15 D472). `[S13.2-L1-02]`: `resize_to_handle` has had this arm since D405
    // and its doc explains the very failure the site without it produced —
    // *"it has to edit **something**: `TextSizing` is inert here, so the arm
    // below would hand back a patch the layout ignores and the handles would drag
    // with nothing moving"*. On a rail `TextSizing` really is inert (the rail's
    // length is the measure), so for `Auto` `scaled_text_sizing` answers `None`
    // and the whole transaction for the child was a bare `SetTransform`.
    //
    // Measured on a group holding one railed `Auto` text plus an 80×40 rect,
    // `BottomRight` to 2× on both axes: the rect doubled, the railed text did not
    // move **at all**, and because its ribbon pins the group's width the **group**
    // came out 171.88 wide against 323.76 asked — **47% short of the drag**, on
    // the first frame, with no error and no report.
    //
    // **The two boxes come from the rail rather than from `Resolved`**, which is
    // what makes this reachable from here: `scale_geometry` has no `Resolved`, and
    // [`railed_box`] measures the current rail through the same layout the canvas
    // draws. `want` is that box scaled by the residual factors about its own
    // origin, which is exactly what `scale_subtree` means by handing them down.
    if let NodeKind::Text {
        on_path: Some(rail),
        ..
    } = node.kind()
        && let Some(drawn) = railed_box(node.kind(), rail)
    {
        let want = Rect::from_origin_size(
            drawn.origin(),
            Size::new(drawn.width() * csx, drawn.height() * csy),
        );
        // The same correction as `resize_to_handle`'s rail arm, at the second door
        // (§15 D604). `scale_scalars` ran a few lines above and has already queued
        // the scaled type, so the solve below has to converge on it.
        // ⚠️ **`drawn` stays the *unscaled* box** — it is where the node is now,
        // and `want` is the target derived from it. Only the kind the solve
        // *measures* moves.
        let solve_against = kind_to_solve_against(node.kind(), scaling, csx, csy);
        ops.push(Operation::SetGeometry {
            id,
            geometry: GeometryPatch::TextPath(Some(railed_resize(
                solve_against.as_ref().unwrap_or(node.kind()),
                rail,
                drawn,
                want,
                // **There is no handle here and the origin really is the anchor**
                // (§15 D789): `want` is `drawn` scaled about `drawn.origin()`, which
                // is what `scale_subtree` means by handing the factors down. That is
                // the corner `BottomRight` holds, so naming it says what this site
                // wants rather than borrowing a default.
                Handle::BottomRight,
            ))),
        });
        return;
    }
    if let NodeKind::Text { sizing, .. } = node.kind() {
        if let Some(sizing) = scaled_text_sizing(*sizing, csx, csy) {
            ops.push(Operation::SetGeometry {
                id,
                geometry: GeometryPatch::TextSizing(sizing),
            });
        }
        return;
    }
    match resizable_size(node.kind()) {
        Some(size) => {
            let new = Size::new((size.width * csx).max(1.0), (size.height * csy).max(1.0));
            ops.push(Operation::SetGeometry {
                id,
                geometry: GeometryPatch::Size(new),
            });
        }
        None => match node.kind() {
            NodeKind::Line { end } => ops.push(Operation::SetGeometry {
                id,
                geometry: GeometryPatch::LineEnd(Point::new(end.x * csx, end.y * csy)),
            }),
            NodeKind::Path { path, corner_radii } => {
                let mut scaled = path.clone();
                scaled.apply_affine(Affine::scale_non_uniform(csx, csy));
                ops.push(Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::Path {
                        path: scaled,
                        // **Carried, not scaled** — this is the geometry resize,
                        // and §5.6's rule is that a 4px radius stays 4px through
                        // one. A `Rect`'s radii are left alone here for exactly
                        // the same reason; the Scale tool multiplies both, in
                        // `scale_scalars`.
                        corner_radii: corner_radii.clone(),
                    },
                });
            }
            _ => scale_subtree(doc, id, Point::ZERO, csx, csy, scaling, ops),
        },
    }
}

/// The edits that make a scale *photographic*: stroke widths, corner radii,
/// font size — all by the geometric mean of the two factors
/// ([`Scaling::scalar`]) — and a frame's own guides, by the axis factor.
///
/// Nothing at all under [`Scaling::Geometry`], which is §5.6's rule and every
/// gesture but the Scale tool's.
///
/// The four text scopes as a photographic scale of `s` leaves them.
///
/// **Every absolute length in the node, and only those.** The attribute model made
/// this one rule rather than a list the scale tool has to keep in step:
/// `Length::scaled` multiplies the `Px` variants and leaves the `Em` ones, which
/// already scale because the font size did. Missing one is invisible until a
/// scaled heading turns out to have kept its old 4px underline offset.
///
/// The two span lists are re-normalized against the **scaled** defaults: a span
/// equal to the old default was never stored, so normalizing against the new one
/// is what keeps a paragraph that asked for its own indent from silently rejoining
/// the node's.
///
/// 🚨 **Split out of [`scale_scalars`] so that a *rail* can be solved against the
/// type it will end up with** (§15 D604). `railed_resize` converges the rail's
/// scale against `railed_box`, which lays the text out — and both rail arms handed
/// it the node's **current** kind while `scale_scalars` was about to multiply the
/// font size by the same `s`. So the solve converged on `layout(new rail, old
/// font)` and the document ended up with `layout(new rail, new font)`.
/// `[S13.2-L1-03]`.
fn scaled_text_scopes(
    style: &TextStyle,
    spans: &ondin_core::CharSpans,
    para_spans: &ondin_core::ParaSpans,
    paragraph: &ondin_core::ParagraphStyle,
    s: f64,
) -> (
    TextStyle,
    ondin_core::CharSpans,
    ondin_core::ParagraphStyle,
    ondin_core::ParaSpans,
) {
    // ⚠️ **`TextStyle::scaled`, not a literal here** (§15 D664, `[S6.1-L3-07]`).
    // This used to re-state all six of `CharAttr::scaled_by`'s decisions —
    // `Size` clamped to core's two constants, four lengths through
    // `Length::scaled`, both decorations — above a **byte-identical private
    // twin** of `typography::scale_decoration`. Core owned the rule for the span
    // half and published nothing for the node half, and the asymmetry was the
    // tell: `ParagraphStyle::scaled` existed and is called one line below. The
    // two copies agreed; nothing made them agree tomorrow.
    let scaled = style.scaled(s);
    let new_spans = spans.scaled(s, &scaled);
    let new_paragraph = paragraph.scaled(s);
    let new_para_spans = para_spans.scaled(s, &new_paragraph);
    (scaled, new_spans, new_paragraph, new_para_spans)
}

/// The node kind [`railed_resize`] should **measure** against, given the scaling
/// the same gesture is about to apply.
///
/// A photographic scale on a text node multiplies its type, and on a rail the box
/// is a function of the curve *and* the type — so the rail solve has to converge
/// on the type the transaction will leave behind, not the one it started from
/// (§15 D604). Anything else, and any non-photographic scaling, measures the kind
/// unchanged, which is what it always did.
fn kind_to_solve_against(
    kind: &NodeKind,
    scaling: Scaling,
    csx: f64,
    csy: f64,
) -> Option<NodeKind> {
    let s = scaling.scalar(csx, csy)?;
    // **Cloned and mutated rather than rebuilt field by field**, so that a ninth
    // field on `NodeKind::Text` — `on_path_flip` and `on_path_offset` were the
    // seventh and eighth — travels through here on its own instead of being an
    // error the next person silently copies past.
    let mut out = kind.clone();
    let NodeKind::Text {
        style,
        spans,
        para_spans,
        paragraph,
        ..
    } = &mut out
    else {
        return None;
    };
    let (new_style, new_spans, new_paragraph, new_para_spans) =
        scaled_text_scopes(style, spans, para_spans, paragraph, s);
    **style = new_style;
    *spans = new_spans;
    *paragraph = new_paragraph;
    *para_spans = new_para_spans;
    Some(out)
}

/// **A corner radius is scaled from the radius, not re-derived from the box.**
/// Reading the new size back and taking a fraction of it would silently square
/// off a corner whose radius had been clamped, since `local_path` clamps to half
/// the shorter side and the stored number can legitimately exceed it.
fn scale_scalars(
    doc: &Document,
    id: NodeId,
    scaling: Scaling,
    csx: f64,
    csy: f64,
    ops: &mut Vec<Operation>,
) {
    // 🚨 **Ahead of the `scalar` guard, and gated on the fork directly** (§15
    // D694, `[S13.1-L2-06]`). [`Scaling::scalar`] answers `None` for *two*
    // unrelated reasons — a Geometry drag, and a Photographic one whose
    // geometric mean is within [`SCALE_NOOP`] of 1 — and only the first of them
    // is a reason to leave a guide alone. Behind that guard, a photographic
    // `(2.0, 0.5)` scaled the frame to 800 × 100 and left a guide down its
    // middle at 200: 25% of the frame instead of 50%, silently and permanently.
    // §5.6 names the mean as the one quantity that must not decide this —
    // *"the geometric mean would drift a guide off the feature it was placed
    // against under any non-uniform scale"* — and the gate in front of the call
    // **was** the mean. Everything below this line genuinely does not move at
    // mean 1 (a stroke, a radius, a font size), which is why the early return is
    // right for all of it and was wrong for exactly this one item.
    if scaling == Scaling::Photographic {
        ops.extend(scaled_guides(doc, id, csx, csy));
    }

    let Some(s) = scaling.scalar(csx, csy) else {
        return;
    };
    let Some(node) = doc.get(id) else { return };

    let strokes = node.paint().strokes.clone();
    if !strokes.is_empty() {
        let strokes = strokes
            .into_iter()
            .map(|mut stroke| {
                stroke.width *= s;
                // A dash pattern is a set of lengths along the outline, so it has
                // to travel with the shape too — a scaled-up dotted border whose
                // spacing stayed put would come back as a solid line.
                for d in &mut stroke.dashes {
                    *d *= s;
                }
                stroke.dash_offset *= s;
                // Per-side widths are widths, and are scaled as such. A side of 0
                // stays 0, which is what keeps "no stroke on this side" meaning
                // that at every size.
                if let ondin_core::StrokeSides::Custom {
                    top,
                    right,
                    bottom,
                    left,
                } = &mut stroke.sides
                {
                    for w in [top, right, bottom, left] {
                        *w *= s;
                    }
                }
                stroke
            })
            .collect();
        ops.push(Operation::SetStrokes { id, strokes });
    }

    match node.kind() {
        NodeKind::Rect { corner_radii, .. } => {
            let r = *corner_radii;
            if !ondin_core::geometry::is_square_cornered(&r) {
                ops.push(Operation::SetGeometry {
                    id,
                    geometry: GeometryPatch::CornerRadii(RoundedRectRadii::new(
                        r.top_left * s,
                        r.top_right * s,
                        r.bottom_right * s,
                        r.bottom_left * s,
                    )),
                });
            }
        }
        // A path's per-anchor radii are the same kind of absolute length as a
        // rect's four, so the Scale tool multiplies them and the resize leaves
        // them alone. Missing this is the "photographic scale that isn't" the
        // comment below warns about.
        NodeKind::Path { path, corner_radii } if corner_radii.iter().any(|r| *r > 0.0) => {
            ops.push(Operation::SetGeometry {
                id,
                geometry: GeometryPatch::Path {
                    path: path.clone(),
                    corner_radii: corner_radii.iter().map(|r| r * s).collect(),
                },
            });
        }
        // **Every absolute length in the node, and only those.** The attribute
        // model made this one rule rather than a list the scale tool has to keep
        // in step: `Length::scaled` multiplies the `Px` variants and leaves the
        // `Em` ones, which already scale because the font size did. Missing one is
        // invisible until a scaled heading turns out to have kept its old
        // 4px underline offset.
        NodeKind::Text {
            style,
            spans,
            para_spans,
            paragraph,
            ..
        } => {
            let (scaled, spans, paragraph, para_spans) =
                scaled_text_scopes(style, spans, para_spans, paragraph, s);
            // `spans: None` on both style ops — the scaled lists arrive in the two
            // spans ops beside them, and the re-statement each style op does in the
            // meantime is exactly what those lists then overwrite (§15 D163).
            ops.push(Operation::SetTextStyle {
                id,
                style: scaled,
                spans: None,
            });
            ops.push(Operation::SetTextSpans { id, spans });
            ops.push(Operation::SetParagraphStyle {
                id,
                paragraph,
                spans: None,
            });
            ops.push(Operation::SetParagraphSpans {
                id,
                spans: para_spans,
            });
        }
        _ => {}
    }
}

/// A frame's own guides, multiplied by the scale — the Scale tool only.
///
/// **A guide's offset from its frame's origin is an absolute length, so it
/// inherits §5.6's Geometry-versus-Photographic split rather than needing a rule
/// of its own.** A resize leaves absolute lengths alone (a 1px border stays 1px,
/// a 4px radius stays 4px, 16pt type stays 16pt) and so leaves a guide where it
/// was, which is also why an offset that outgrows a shrunk frame is kept rather
/// than clamped — the clip hides the line and re-growing the frame brings it
/// back. The Scale tool takes all three, and takes this too: a guide down the
/// middle of a frame that stayed at 100 while the frame doubled to 400 would no
/// longer be down the middle, and a photographic scale is precisely the gesture
/// that promises it would be.
///
/// **By the axis factor, not the geometric mean.** A stroke width has no
/// direction and takes [`Scaling::scalar`]; a guide's position is a coordinate on
/// one named axis, so it scales the way a path's points do
/// (`scale_non_uniform`) — a vertical guide by `csx`, a horizontal one by `csy`.
/// The mean would drift a guide off the feature it was placed against under any
/// non-uniform scale.
///
/// Called from [`scale_scalars`] rather than from `scale_geometry`, which is
/// where the `Scaling` fork already is: an `Artboard` reports a `resizable_size`,
/// so `scale_geometry` sets its box and returns without recursing, and a branch
/// added there would have to re-ask the question this function is already behind.
fn scaled_guides(doc: &Document, id: NodeId, csx: f64, csy: f64) -> Vec<Operation> {
    let mut ops: Vec<Operation> = doc
        .guides_owned_by(id)
        .filter_map(|g| {
            let factor = match g.axis {
                ondin_core::GuideAxis::Vertical => csx,
                ondin_core::GuideAxis::Horizontal => csy,
            };
            let position = g.position * factor;
            // A guide already at the frame's origin has nothing to scale, and an
            // op that changes nothing is a step the user has to undo past.
            (position != g.position).then_some((g.id, position))
        })
        .map(|(id, position)| Operation::SetGuidePosition { id, position })
        .collect();
    // Deterministic order, as `build::guides_of` is and for the same reason: the
    // transaction is what lands in history.
    ops.sort_by_key(|op| match op {
        Operation::SetGuidePosition { id, .. } => *id,
        _ => unreachable!("only positions are produced here"),
    });
    ops
}

/// Below this a box has no extent to scale from and the factor is meaningless.
const MIN_EXTENT: f64 = 1e-6;
/// How far a scale factor has to be from 1 to be worth an operation.
///
/// Not `f64::EPSILON`: the factors come out of a square root, so a scale that is
/// genuinely 1 lands a few ulps off it, and a test that tight makes *every* lean
/// carry a phantom resize into the undo stack alongside it.
const SCALE_NOOP: f64 = 1e-9;
/// Cap on a single resize step, so dragging a handle across a nearly-flat group
/// cannot produce an astronomically large one.
///
/// ⚠️ **It is not what keeps the resize arithmetic finite, and this is the first
/// place a reader looks for that** (§15 D481). It bounds the *factor* the gesture
/// asks for and says nothing about what the node already holds: a basis of `1e308`
/// times a perfectly ordinary factor of 2 is `inf`. [`scaled_about`] is the guard
/// on the product, and it refuses per node.
const MAX_SCALE: f64 = 1e4;

/// The factor that takes `current` to `wanted`, **sign included**.
///
/// **The sign is the flip, and clamping it away is what stopped one.** This used to be
/// `(wanted / current).clamp(MIN_EXTENT, MAX_SCALE)`, which turns every negative factor
/// into `1e-6`: drag a corner past the one opposite and instead of the shape turning
/// inside out it collapsed to nothing and then grew again on the *same* side, tracking
/// the pointer on the inverted axis. Reported for booleans and paths, and it was every
/// caller of [`resize_factors`] — a group did it too; only a `Rect` and its relatives
/// escaped, because they go through `resized_box`, which reflects the box across the
/// anchor instead of scaling it.
///
/// A negative factor is safe to hand on precisely because [`split_geometry_scale`] knows
/// what to do with one: `Basis::of` peels the reflection off into
/// `Orientation::mirror` and the geometry still gets a positive scale, which is §5.6's
/// rule — a mirrored shape is a transform that mirrors, never a negative size.
///
/// The magnitude is still clamped, at both ends and for the reasons the constants give.
fn scale_along(current: f64, wanted: f64) -> f64 {
    if current.abs() < MIN_EXTENT {
        return 1.0;
    }
    let factor = wanted / current;
    factor.signum() * factor.abs().clamp(MIN_EXTENT, MAX_SCALE)
}

/// Scale everything under `id` by `(sx, sy)` about `anchor`, expressed in the
/// space `id`'s children live in.
fn scale_subtree(
    doc: &Document,
    id: NodeId,
    anchor: Point,
    sx: f64,
    sy: f64,
    scaling: Scaling,
    ops: &mut Vec<Operation>,
) {
    let Some(node) = doc.get(id) else { return };
    for child_id in node.children() {
        let Some(child) = doc.get(*child_id) else {
            continue;
        };
        let t = child.transform();

        // The child scaled bodily — origin and basis both — and then split back
        // into what it may store and what its geometry has to take.
        //
        // `R⁻¹·S·R` is diagonal only when the child is axis-aligned (or the
        // scale is uniform). Otherwise the leftover is a skew, and the split
        // parks that in the basis rather than rounding it away, so the child
        // ends up exactly where the box put it at every angle.
        //
        // A nested container has no geometry of its own, so `scale_geometry`
        // recurses into its contents about its own origin — in *its* axes, which
        // is why the scale that goes down is the child's and not the parent's.
        // Handing the outer factors to a rotated sub-group would stretch its
        // contents along the wrong pair of axes.
        // As in `resize_selection`: a child whose basis overflows is skipped, and
        // its siblings still scale (§15 D481).
        let Some(scaled) = scaled_about(t, anchor, sx, sy) else {
            continue;
        };
        let Some((transform, csx, csy)) = split_geometry_scale(scaled) else {
            continue;
        };
        if transform.as_coeffs() != t.as_coeffs() {
            ops.push(Operation::SetTransform {
                id: *child_id,
                transform,
            });
        }
        scale_geometry(doc, *child_id, csx, csy, scaling, ops);
    }
}

/// Rotate a single node by `angle` radians about `pivot` (world space).
/// `new_local = parent_world⁻¹ · rotate_about(pivot, angle) · old_world`.
///
/// ⚠️ **Split, like every other transform-writing gesture in this file** (§15 D577).
/// `[S13.1-L2-04]`: this was the one that was not, and `P⁻¹ · R · P` is a rotation
/// only when `P` is conformal — a **skewed** parent makes it a general unimodular
/// matrix, so rotating a child inside a leaning group stored `scale = (1.4547,
/// 0.6875)` where it had been `(1, 1)`.
///
/// §5.6 is the rule it broke, verbatim: *"`transform` carries translation, rotation,
/// skew and flip. **It does not carry scale in normal editing.** Scale is the one
/// factor kept out, and for a reason the others do not share: a layer's size must be
/// one number in one place (its geometry)."*
///
/// ⚠️ **Nothing rendered wrong, which is why this survived — the *next* gesture is
/// where it is paid.** The split reproduces the world placement exactly, so the
/// pixels were right and the node's stored `size` went on reading `100 × 100` while
/// it drew 223 units wide. Then a 0.006° lean — a pointer moved one hundredth of a
/// unit — ran the accumulated basis through `split_geometry_scale` and moved the
/// stored size to `145.47 × 68.74`. **The W/H fields jump 45% on a gesture that
/// asked for a hundredth of a degree**, and *"make it 200 wide"* has quietly changed
/// meaning. A transform that carries scale is not a wrong picture, it is a wrong
/// *number*, and the number surfaces later and somewhere else.
///
/// The `SCALE_NOOP` gate is `skew_to_handle`'s and is here for its reason: the
/// factors come out of a square root, so an ordinary rotation lands a few ulps off
/// 1 and a tighter test would put a phantom resize in the undo stack beside every
/// turn.
pub fn rotate_node(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    pivot: Point,
    angle: f64,
) -> Transaction {
    let Some(node) = doc.get(id) else {
        return Transaction(vec![]);
    };
    let Some(old_world) = res.world_transform(id) else {
        return Transaction(vec![]);
    };
    let parent_world = node
        .parent()
        .and_then(|p| res.world_transform(p))
        .unwrap_or(Affine::IDENTITY);
    let new_world = turn_about(pivot, angle) * old_world;
    let new_local = parent_world.inverse() * new_world;
    let Some((transform, gx, gy)) = split_geometry_scale(new_local) else {
        return Transaction(vec![]);
    };
    let mut ops = Vec::new();
    if transform.as_coeffs() != node.transform().as_coeffs() {
        ops.push(Operation::SetTransform { id, transform });
    }
    if (gx - 1.0).abs() > SCALE_NOOP || (gy - 1.0).abs() > SCALE_NOOP {
        scale_geometry(doc, id, gx, gy, Scaling::Geometry, &mut ops);
    }
    Transaction(ops)
}

/// How far a skew may lean: a degree short of the quarter turn at which a shape
/// collapses onto a line and its matrix stops being invertible. Clamped rather
/// than rejected, so a wild drag pins at the extreme instead of snapping back.
pub const MAX_SKEW: f64 = std::f64::consts::FRAC_PI_2 - 0.017_453_292_519_943_295;

/// Skew step with Shift held: 15°, the same step a rotation snaps to. The two
/// gestures produce the same kind of number about the same box, so a designer
/// who has learnt one has learnt the other.
const SKEW_SNAP: f64 = std::f64::consts::FRAC_PI_2 / 6.0;

/// Where a side-drag began and where its pointer is now, in one space.
///
/// **One argument rather than two, so they cannot be exchanged.** Both are
/// `Point`s that would sit adjacent in the signature, and swapping them negates
/// the lean — a mistake that compiles, draws something plausible and reads as a
/// sign error in the arithmetic rather than as a transposition. It also keeps
/// `skew_to_handle` and `skew_selection` inside clippy's argument budget, which is
/// the lint noticing the same thing: two of these belong together.
#[derive(Clone, Copy, Debug)]
pub struct SideDrag {
    /// Where the press landed — the origin the lean is measured from (§15 D322).
    pub press: Point,
    /// Where the pointer is now.
    pub at: Point,
}

impl SideDrag {
    /// The same drag in another space, both points through one transform.
    ///
    /// A method rather than two multiplications at the call site, because mapping
    /// only one of them is exactly the bug this type exists to make hard: the
    /// measurement's origin would sit in a different frame from its pointer.
    fn mapped(self, by: Affine) -> Self {
        Self {
            press: by * self.press,
            at: by * self.at,
        }
    }
}

/// The lean a side drag implies for `box_`, as an affine in that box's own
/// space. `None` for a corner handle and for a box too thin to lean.
///
/// **Skew is a side gesture, and only a side gesture.** The dragged side follows
/// the pointer along its own length while the opposite side stays put — the same
/// bargain a resize strikes, which is why the two can share a band and a
/// modifier. A corner has no side facing it to hold still, so there is nothing
/// for the gesture to mean there; corners keep resize and the rotate ring.
///
/// The box's *extent across* the gesture is the lever: dragging the top of a
/// short wide box leans it much further per pixel than the top of a tall one,
/// because it is the same displacement over a shorter drop. That is the same
/// arithmetic a rotation does and it reads the same way under the hand.
///
/// **`press` is where the drag began, and the offset is measured from it rather
/// than from the box's centre** (§15 D322). Measuring from the centre made the
/// lean a function of *where on the side you grabbed*, so the first frame of every
/// off-centre drag jumped to whatever angle that grab already implied — reported
/// as "the moment I click and do a very small drag, it jumps to −26.76°", which on
/// the 200×120 rect it was reported against is a grab 80% along the top edge:
/// `atan(60.5 / -120)`. Grabbing a side's exact midpoint was the only place it
/// behaved, which is why it survived so long.
///
/// The consequence worth keeping in mind: this returns the shear the gesture has
/// asked for *since the press*, so it composes onto the node's existing transform
/// and a second skew gesture starts from zero rather than re-adding the offset.
///
/// The two points travel as one [`SideDrag`] so they cannot be exchanged; see
/// there for why that is worth a type.
pub fn side_shear(
    box_: Rect,
    handle: Handle,
    drag: SideDrag,
    snap_to_steps: bool,
) -> Option<Affine> {
    let (p, press) = (drag.at, drag.press);
    // `atan` already keeps the angle off the asymptote; the clamp pulls it in
    // the last degree, where `tan` is large enough to be numerically useless.
    // The snap lands *before* the clamp so that its top step is a round 75°
    // rather than the clamp's 89°.
    let lean = |offset: f64, span: f64| {
        (span.abs() >= MIN_EXTENT).then(|| {
            let a = (offset / span).atan();
            let a = if snap_to_steps {
                (a / SKEW_SNAP).round() * SKEW_SNAP
            } else {
                a
            };
            a.clamp(-MAX_SKEW, MAX_SKEW)
        })
    };
    let (shear, held) = match handle {
        // A horizontal side leans sideways, about the side facing it.
        Handle::Top => (
            build::skew_x(lean(p.x - press.x, box_.min_y() - box_.max_y())?),
            Vec2::new(0.0, box_.max_y()),
        ),
        Handle::Bottom => (
            build::skew_x(lean(p.x - press.x, box_.max_y() - box_.min_y())?),
            Vec2::new(0.0, box_.min_y()),
        ),
        Handle::Left => (
            build::skew_y(lean(p.y - press.y, box_.min_x() - box_.max_x())?),
            Vec2::new(box_.max_x(), 0.0),
        ),
        Handle::Right => (
            build::skew_y(lean(p.y - press.y, box_.max_x() - box_.min_x())?),
            Vec2::new(box_.min_x(), 0.0),
        ),
        _ => return None,
    };
    Some(Affine::translate(held) * shear * Affine::translate(-held))
}

/// Skew a **single** node by dragging `handle`, in the node's own frame.
///
/// `box_` is the node's local box — its geometry's for a shape, its contents'
/// for a container ([`ondin_core::local_box`]) — so the lean is measured against
/// the same rectangle the handles are drawn on, and a turned layer skews along
/// the edge the user is actually pulling rather than along a world axis.
///
/// Unlike a resize this needs no recursion and no world round trip: the shear is
/// expressed in the node's own space, so `new_local = old_local · shear` and
/// everything inside a container comes along for free. The split afterwards is
/// still needed, because leaning a shape that is *already* leaning the other way
/// genuinely changes its extents, and §5.6 keeps that in the geometry.
pub fn skew_to_handle(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    box_: Rect,
    handle: Handle,
    drag: SideDrag,
    snap_to_steps: bool,
) -> Transaction {
    let Some(node) = doc.get(id) else {
        return Transaction(vec![]);
    };
    let Some(world) = res.world_transform(id) else {
        return Transaction(vec![]);
    };
    // **Both points through the same inverse**, which is what `mapped` is for: the
    // lean is measured in the node's own space, so mapping one and not the other
    // would put the origin of the measurement in a different frame from its
    // pointer and reintroduce the jump at an angle.
    let Some(shear) = side_shear(box_, handle, drag.mapped(world.inverse()), snap_to_steps) else {
        return Transaction(vec![]);
    };
    let Some((transform, gx, gy)) = split_geometry_scale(node.transform() * shear) else {
        return Transaction(vec![]);
    };
    let mut ops = Vec::new();
    if transform.as_coeffs() != node.transform().as_coeffs() {
        ops.push(Operation::SetTransform { id, transform });
    }
    if (gx - 1.0).abs() > SCALE_NOOP || (gy - 1.0).abs() > SCALE_NOOP {
        scale_geometry(doc, id, gx, gy, Scaling::Geometry, &mut ops);
    }
    Transaction(ops)
}

/// Apply one world-space `shear` to a whole selection.
///
/// `roots` must be [`build::outermost`], as for [`resize_selection`]. Every member
/// takes the *same* shear rather than each leaning along its own edges, which is
/// what makes a set lean as one parallelogram.
///
/// **It is handed the shear rather than deriving one** (§15 D324). The lean comes
/// from [`side_shear`] over the set's upright union, and the *chrome* needs the
/// very same affine to draw the frame leaning with the shapes — so computing it
/// here would have meant two callers deriving one answer, which is D266's shape.
/// One computation, two consumers.
///
/// A member turned off-axis needs a scale as well as a shear to land exactly, and
/// [`split_geometry_scale`] gives it one. Nothing here is approximate.
pub fn skew_selection(
    doc: &Document,
    res: &Resolved,
    roots: &[NodeId],
    shear: Affine,
) -> Transaction {
    let mut ops = Vec::new();
    for id in roots {
        let Some(node) = doc.get(*id) else { continue };
        let Some(world) = res.world_transform(*id) else {
            continue;
        };
        let parent_world = node
            .parent()
            .and_then(|p| res.world_transform(p))
            .unwrap_or(Affine::IDENTITY);
        // Projected back into the member's own parent before the split, for the
        // reason `resize_selection` gives: splitting in world space leaves the
        // parent's basis on the outside and puts scale back in the transform.
        let Some((transform, gx, gy)) =
            split_geometry_scale(parent_world.inverse() * shear * world)
        else {
            continue;
        };
        if transform.as_coeffs() != node.transform().as_coeffs() {
            ops.push(Operation::SetTransform { id: *id, transform });
        }
        if (gx - 1.0).abs() > SCALE_NOOP || (gy - 1.0).abs() > SCALE_NOOP {
            scale_geometry(doc, *id, gx, gy, Scaling::Geometry, &mut ops);
        }
    }
    Transaction(ops)
}

/// The parent-local origin + size of the axis-aligned box spanned by two world
/// points, given the parent's world transform. Used by the create tools so a
/// drawn shape lands correctly under any (translated/scaled) parent.
fn local_box(parent_world: Affine, a: Point, b: Point) -> (Point, Size) {
    let inv = parent_world.inverse();
    let la = inv * a;
    let lb = inv * b;
    let origin = Point::new(la.x.min(lb.x), la.y.min(lb.y));
    let size = Size::new((la.x - lb.x).abs(), (la.y - lb.y).abs());
    (origin, size)
}

/// Create an artboard spanning world points `a`..`b` under `parent` — the root, or
/// the frame it was drawn inside (§5.3, frames nest).
///
/// `parent_world` is what the box is projected through, exactly as for a shape: it
/// was `Affine::IDENTITY` for as long as a frame could only be a child of the root,
/// and a nested frame drawn inside a frame that has been moved or scaled needs the
/// real one or it lands wherever its parent's origin happens to be.
pub fn create_artboard(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    a: Point,
    b: Point,
    background: Color,
) -> Transaction {
    let (origin, size) = local_box(parent_world, a, b);
    // **Two operations, exactly as [`create_rect`] takes two**, and for the same
    // reason: `CreateNode` carries a kind and a transform, and paint is a separate
    // op. The frame's ground used to be a field of the kind and rode in on the
    // first (§15 D400).
    Transaction(vec![
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Artboard {
                size: Size::new(size.width.max(1.0), size.height.max(1.0)),
            },
            transform: Some(Affine::translate(origin.to_vec2())),
            name: None,
        },
        Operation::SetFills {
            id,
            fills: vec![Fill {
                brush: Brush::Solid(background),
                visible: true,
            }],
        },
    ])
}

/// Create a rectangle spanning world points `a`..`b` under `parent`, with a
/// solid `fill`. `index` is the child slot (append = current child count).
pub fn create_rect(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    a: Point,
    b: Point,
    fill: Color,
) -> Transaction {
    let (origin, size) = local_box(parent_world, a, b);
    Transaction(vec![
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Rect {
                size,
                corner_radii: RoundedRectRadii::default(),
            },
            transform: Some(Affine::translate(origin.to_vec2())),
            name: None,
        },
        Operation::SetFills {
            id,
            fills: vec![Fill {
                brush: Brush::Solid(fill),
                visible: true,
            }],
        },
    ])
}

/// Create an **empty text node** filling the world box `a`..`b` under `parent` —
/// the Text tool's drag, as against its click.
///
/// **The gesture decides the sizing mode, and that is the whole point of the drag.**
/// A click plants `TextSizing::Auto`, which grows with what is typed; a drag says a
/// width was wanted, so this plants `Fixed` at the box that was drawn. The vertical
/// extent is honoured too rather than being dropped for `AutoHeight`: the user
/// dragged a height, and a drag whose second axis silently did nothing is the
/// "inert is indistinguishable from forgotten" trap this codebase already avoids for
/// `indent_end` (§15 D163). `TextOverflow::Visible` is the default, so a box typed
/// past simply overflows rather than clipping — the honest reading of "this is the
/// box I asked for".
///
/// **A minimum of one line box high**, because a sloppy thin drag would otherwise
/// commit a node whose very first character overflows it, which reads as the tool
/// being broken rather than as a small box. The width has no such floor: a narrow
/// column is a real thing to want, and wrapping handles it.
///
/// `parts` carries the app's current defaults (family, size, fills) exactly as the
/// click path takes them, so the two gestures cannot disagree about what a new text
/// node *is* — only about how it is measured.
pub fn create_text(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    a: Point,
    b: Point,
    parts: &ondin_core::TextParts,
) -> Transaction {
    let (origin, size) = local_box(parent_world, a, b);
    // One line box, from the face rather than from the style: `line_height: None`
    // means "the font decides", so the only honest source is a shaped layout — and
    // `TextLayout::line_height` is the field that exists to answer exactly this
    // (§15 D162). An empty string is enough to get it.
    let mut auto = parts.clone();
    auto.sizing = TextSizing::Auto;
    let line = ondin_core::text::layout(auto.as_ref("")).line_height;
    let size = Size::new(size.width, size.height.max(line));
    Transaction(vec![Operation::CreateNode {
        id,
        parent,
        index,
        kind: NodeKind::Text {
            content: String::new(),
            style: Box::new(parts.style.clone()),
            spans: parts.spans.clone(),
            para_spans: parts.para_spans.clone(),
            paragraph: parts.paragraph.clone(),
            block: parts.block,
            sizing: TextSizing::Fixed(size),
            // As `canvas::begin_text`: a new node is never on a rail.
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        },
        transform: Some(Affine::translate(origin.to_vec2())),
        name: None,
    }])
}

/// The size to place an image at inside a box of `frame` — **shrink to fit,
/// never scale up** (§5.5a, §15 D177).
///
/// A 32px icon dropped into a 1200px frame stays 32px; only an image that
/// *exceeds* the box is scaled down, preserving aspect. "Fit" read as "always
/// fill the frame" is how a small asset lands enormous and blurry, and it is the
/// half of this rule that has to be written down or it arrives later as a bug
/// report.
///
/// The box is the frame the image is being dropped into, or — on bare canvas,
/// where there is no frame — the viewport, which is the only bounded thing
/// available. That is also what keeps a 6000px photograph from landing 6000
/// units wide; *original size* is one command away either way.
///
/// A degenerate box (zero or negative on either axis) falls back to the intrinsic
/// size rather than to nothing, because a zero-sized layer is not a placement a
/// user can see or fix.
pub fn fitted_image_size(intrinsic: Size, frame: Size) -> Size {
    if frame.width <= 0.0 || frame.height <= 0.0 {
        return intrinsic;
    }
    let scale = (frame.width / intrinsic.width).min(frame.height / intrinsic.height);
    if scale >= 1.0 {
        return intrinsic;
    }
    Size::new(intrinsic.width * scale, intrinsic.height * scale)
}

/// Place `image` as a new Rect centred on the world point `at`, fitted to `frame`.
///
/// **Centred on the point rather than hung from it**, because this is what a
/// loaded cursor is holding: the pointer is where the picture goes, not where its
/// top-left corner goes. A *drag* lays out its own box and goes through
/// [`create_image_in_box`] instead, exactly as a rectangle does — the two gestures
/// differ in how the layer is measured and in nothing else.
///
/// `entry` is `None` when the document already carries this id — placing the same
/// file twice adds a *reference*, not a second copy of the bytes, which is what
/// the content hash is for. Passing `Some` for an id already in the table is the
/// one way to get `OpError::DuplicateImage`.
#[allow(clippy::too_many_arguments)]
pub fn create_image(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    at: Point,
    frame: Size,
    image: ImageId,
    entry: Option<ImageEntry>,
    intrinsic: Size,
    name: Option<String>,
) -> Transaction {
    let size = fitted_image_size(intrinsic, frame);
    let local = parent_world.inverse() * at;
    let origin = Point::new(local.x - size.width / 2.0, local.y - size.height / 2.0);
    image_ops(id, parent, index, origin, size, image, entry, name)
}

/// [`create_image`] for a drag: the shape is the box the user drew, at whatever
/// aspect they drew it.
///
/// **No fit, and no aspect lock.** A drawn box is an instruction; second-guessing
/// it is what makes a tool feel like it is arguing. The fit rule exists for the
/// gesture that gives *no* size — a click — and applying it here would make a
/// deliberate drag snap to something else.
///
/// What the *picture* then does inside that box is the fill's framing and not
/// this function's: [`ondin_core::ImageFit::Fill`] covers the box and cuts off
/// the overflow, so a wide box drawn over a tall photograph crops it rather than
/// squashing it. A click-placed shape carries the picture's own aspect, so there
/// the two are the same drawing.
#[allow(clippy::too_many_arguments)]
pub fn create_image_in_box(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    a: Point,
    b: Point,
    image: ImageId,
    entry: Option<ImageEntry>,
    name: Option<String>,
) -> Transaction {
    let (origin, size) = local_box(parent_world, a, b);
    image_ops(id, parent, index, origin, size, image, entry, name)
}

/// The operations both placement gestures produce, given a box.
///
/// **The order is the order the undo needs.** `AddImage` first, so the inverse
/// removes the node before the bytes it refers to — the same rule that makes a
/// frame's guides removable before the frame (`build::guides_of`).
#[allow(clippy::too_many_arguments)]
fn image_ops(
    id: NodeId,
    parent: NodeId,
    index: usize,
    origin: Point,
    size: Size,
    image: ImageId,
    entry: Option<ImageEntry>,
    name: Option<String>,
) -> Transaction {
    let mut ops = Vec::with_capacity(3);
    if let Some(entry) = entry {
        ops.push(Operation::AddImage {
            id: image.clone(),
            entry,
        });
    }
    ops.push(Operation::CreateNode {
        id,
        parent,
        index,
        kind: NodeKind::Rect {
            size,
            corner_radii: RoundedRectRadii::default(),
        },
        transform: Some(Affine::translate(origin.to_vec2())),
        name,
    });
    ops.push(Operation::SetFills {
        id,
        fills: vec![Fill {
            brush: ondin_core::image_brush(image),
            visible: true,
        }],
    });
    Transaction(ops)
}

/// The frame after `handle` is dragged to `at`, **bounded by the picture** — the
/// crop gesture's whole geometry, in the layer's local space.
///
/// `limit` is `ImageRef::picture_box`: the rectangle the whole photograph
/// occupies. The frame is a window onto it, so a window that reached past the
/// edge would be showing `Extend::Pad`'s smear of the edge row — which is why the
/// outline drawn round the picture is a real limit rather than a decoration, and
/// why the handle simply stops there. Figma's crop stops in the same place, and
/// the reason is the same: there is nothing beyond it to show.
///
/// **The opposite edges are held**, so `at` is the moved corner and the rectangle
/// is the one spanned by the two. That is [`resized_box`]'s rule with neither
/// `keep_ratio` nor `symmetric`, restated in the frame's own coordinates — and it
/// has to be the *same* rule, because the transaction this feeds resizes the node
/// through [`resize_layer`], which applies that one. Feeding it the corner of the
/// rectangle returned here is what makes the two agree by construction rather
/// than by two people writing the same arithmetic twice.
///
/// An edge handle moves one axis and leaves the other exactly as it found it;
/// [`MIN_CROP`] is the floor on both.
///
/// **A handle stays on its own side of the box, where a resize handle may cross
/// over.** Drag a selection's top-left corner past the bottom-right one and the
/// layer mirrors, which is a real and useful thing for a shape to do. A crop
/// window has no such reading — a mirrored window would flip the photograph
/// inside a mode whose whole premise is that the photograph does not move — so
/// the moved edge stops [`MIN_CROP`] short of the held one instead. It is also
/// what keeps the corner this returns *the corner that moved*, which
/// `crop_resize_tx` relies on to aim [`resize_layer`] at it: past the crossover
/// the two corners swap, the aim lands on the held edge, and the layer collapses
/// to a point. Caught by
/// `a_crop_resize_writes_the_very_box_its_crop_was_computed_against`.
pub fn crop_resized(frame: Rect, handle: Handle, at: Point, limit: Rect) -> Rect {
    let (ux, uy) = handle.unit();
    // `unit` is 0 at the near edge, 1 at the far one, and 0.5 on an axis the
    // handle does not own — which is exactly the three cases below, so the rule
    // is read off the handle rather than inferred from where the pointer went.
    let span = |u: f64, moved: f64, lo: f64, hi: f64, lim_lo: f64, lim_hi: f64| -> (f64, f64) {
        let (mut a, mut b) = if u == 0.0 {
            (moved, hi)
        } else if u == 1.0 {
            (lo, moved)
        } else {
            (lo, hi)
        };
        // The picture is the wall: a window reaching past it would be showing
        // `Extend::Pad`'s smear of the edge row rather than the photograph.
        a = a.clamp(lim_lo, lim_hi);
        b = b.clamp(lim_lo, lim_hi);
        // Then the floor, given back to whichever edge is *not* moving, so a
        // handle run all the way across leaves the held edge where it was.
        if b - a < MIN_CROP {
            if u == 0.0 {
                a = b - MIN_CROP;
            } else {
                b = a + MIN_CROP;
            }
        }
        (a, b)
    };
    let (x0, x1) = span(ux, at.x, frame.x0, frame.x1, limit.x0, limit.x1);
    let (y0, y1) = span(uy, at.y, frame.y0, frame.y1, limit.y0, limit.y1);
    Rect::new(x0, y0, x1, y1)
}

/// The smallest a crop window gets, in the layer's local units.
///
/// One unit, which is [`resized_box`]'s own floor (`w.max(1.0)`) — deliberately
/// the same number, because [`crop_resized`] hands its rectangle's corner to
/// [`resize_layer`] and a different floor in the two would let the frame the
/// crop was computed against differ from the frame that got written.
const MIN_CROP: f64 = 1.0;

/// Which fill of `paint` image editing acts on: the first **visible** image.
///
/// Visible, because editing a fill you cannot see would look like nothing
/// happening; first, because a shape with two pictures is exotic and any rule
/// here is arbitrary — what matters is that it is *stated* in one place rather
/// than falling out of an iterator somewhere.
///
/// **That one place is now [`ondin_core::Paint::image_fill`]**, because the model
/// came to ask the same question — `build::mask_target` has to know which member
/// of a selection is the photograph — and a rule spelled on both sides of the
/// crate boundary is the drift §15 D261 records the cost of. This name stays
/// because sixteen call sites here read as *croppable*, which is what they mean.
pub fn croppable_fill(paint: &ondin_core::Paint) -> Option<usize> {
    paint.image_fill()
}

// **`original_refusal` was here and went on 2026-08-23** with the *Export
// original…* rows it answered for. It said *why* a picture's stored bytes could
// not be written out, in three states rather than two — linked, missing, or
// fine — because both doors had asked `bytes().is_some()`, which is equally false
// for a linked picture and for an id the table does not hold, so a document that
// had simply lost a picture was told it used **linking**, a feature the app
// cannot author (§15 D280).
//
// ⚠️ **It was `ondin_core::ImageEntry::is_linked`'s only caller**, and D280's own
// record says that predicate had sat `pub` in core with none before this — "the
// rot CLAUDE.md warns about", where it cost behaviour rather than a stale
// comment. It is back in that state now. `ondin-core` is a library, so
// `dead_code` will never say so; the next reader of `is_linked` should know it is
// reached from nothing, and the three-state distinction above is the thing to
// rebuild rather than re-derive if linking is ever authored.

/// Create an ellipse inscribed in the world box `a`..`b` under `parent`.
pub fn create_ellipse(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    a: Point,
    b: Point,
    fill: Color,
) -> Transaction {
    let (origin, size) = local_box(parent_world, a, b);
    Transaction(vec![
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Ellipse { size },
            transform: Some(Affine::translate(origin.to_vec2())),
            name: None,
        },
        Operation::SetFills {
            id,
            fills: vec![Fill {
                brush: Brush::Solid(fill),
                visible: true,
            }],
        },
    ])
}

/// One press of `↑` or `↓` on a polygon or star being drawn
/// (`docs/shortcuts.md` §9, Illustrator exact).
///
/// **Clamped to the model's own bounds**, which are the same pair the
/// inspector's sides field scrubs between — so a shape cannot be drawn with a
/// count the field would then refuse. Both ends matter: `↓` held at three would
/// wrap a `u32` straight to four billion, and `↑` held has no natural stop at
/// all.
///
/// Both keys at once is a no-op, which is what falls out of taking them as a
/// signed step rather than as two branches.
pub fn step_sides(sides: u32, up: bool, down: bool) -> u32 {
    let delta = i64::from(up) - i64::from(down);
    (i64::from(sides) + delta).clamp(
        i64::from(geometry::MIN_SIDES),
        i64::from(geometry::MAX_SIDES),
    ) as u32
}

/// Create a regular polygon inscribed in the world box `a`..`b`.
///
/// `sides` is passed in rather than taken from [`POLYGON_DEFAULT_SIDES`]
/// because `↑`/`↓` change it while the drag is still in flight
/// (`docs/shortcuts.md` §9); the constant is what the gesture *starts* at.
///
/// Eight arguments, and allowed the way [`create_line`] already is — §15 D41's
/// rule is a parts struct before the **ninth**, and the whole create family
/// shares one shape that a struct for two of them would break up.
#[allow(clippy::too_many_arguments)]
pub fn create_polygon(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    a: Point,
    b: Point,
    sides: u32,
    fill: Color,
) -> Transaction {
    let (origin, size) = local_box(parent_world, a, b);
    shape_at(
        id,
        parent,
        index,
        origin,
        NodeKind::Polygon {
            size,
            sides: sides.clamp(geometry::MIN_SIDES, geometry::MAX_SIDES),
        },
        fill,
    )
}

/// Create a star inscribed in the world box `a`..`b`.
///
/// `points` is live for the same reason [`create_polygon`]'s `sides` is, and the
/// allow is that function's.
#[allow(clippy::too_many_arguments)]
pub fn create_star(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    a: Point,
    b: Point,
    points: u32,
    fill: Color,
) -> Transaction {
    let (origin, size) = local_box(parent_world, a, b);
    shape_at(
        id,
        parent,
        index,
        origin,
        NodeKind::Star {
            size,
            points: points.clamp(geometry::MIN_SIDES, geometry::MAX_SIDES),
            inner_ratio: STAR_DEFAULT_RATIO,
        },
        fill,
    )
}

/// Create a filled shape at a parent-local origin. The two-op create-then-fill
/// shape every drawn node shares.
fn shape_at(
    id: NodeId,
    parent: NodeId,
    index: usize,
    origin: Point,
    kind: NodeKind,
    fill: Color,
) -> Transaction {
    Transaction(vec![
        Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(Affine::translate(origin.to_vec2())),
            name: None,
        },
        Operation::SetFills {
            id,
            fills: vec![Fill {
                brush: Brush::Solid(fill),
                visible: true,
            }],
        },
    ])
}

/// Create a line from world point `a` to `b` under `parent`, stroked with
/// `color` at `width`. The node origin sits at `a`; `end` is `b` in local space.
#[allow(clippy::too_many_arguments)]
pub fn create_line(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    a: Point,
    b: Point,
    color: Color,
    width: f64,
) -> Transaction {
    let inv = parent_world.inverse();
    let la = inv * a;
    let lb = inv * b;
    let end = Point::new(lb.x - la.x, lb.y - la.y);
    Transaction(vec![
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Line { end },
            transform: Some(Affine::translate(la.to_vec2())),
            name: None,
        },
        Operation::SetStrokes {
            id,
            strokes: vec![Stroke {
                brush: Brush::Solid(color),
                width,
                ..Default::default()
            }],
        },
    ])
}

/// The outline a set of pen anchors describes, in **world** space.
///
/// Public because the canvas draws the in-progress path from the same function
/// it will be committed with — a rubber band built separately is a rubber band
/// that lies about the curve you are about to get.
///
/// A segment is a straight line when the two handles it uses are both zero
/// (`PenAnchor::straight_to`), and a cubic otherwise — so a path drawn entirely
/// with clicks comes out as the polyline it looks like rather than as curves
/// whose control points happen to be collinear, and so does the straight half of
/// a path that curves elsewhere.
pub fn pen_outline(anchors: &[PenAnchor], close: bool) -> BezPath {
    let mut path = BezPath::new();
    if anchors.is_empty() {
        return path;
    }
    path.move_to(anchors[0].at);
    let segment = |a: &PenAnchor, b: &PenAnchor, path: &mut BezPath| {
        if a.straight_to(b) {
            path.line_to(b.at);
        } else {
            path.curve_to(a.leaving(), b.arriving(), b.at);
        }
    };
    for pair in anchors.windows(2) {
        segment(&pair[0], &pair[1], &mut path);
    }
    if close && anchors.len() >= 2 {
        segment(&anchors[anchors.len() - 1], &anchors[0], &mut path);
        path.close_path();
    }
    path
}

/// Every subpath in `subpaths`, one after another — [`pen_outline`] for a whole
/// path rather than a single run of anchors.
///
/// The partner of [`pen_anchors`], and **not its inverse** — `pen_path(&pen_anchors(&p, &[]))`
/// is `p` again only for paths the pen itself wrote.
///
/// ⚠️ **Three ways the round trip moves** (§15 D583): a quad is elevated to the
/// cubic with the same curve; a `ClosePath` whose last point is already the first
/// folds that duplicate away; and — the one that is easy to miss, because it adds
/// rather than removes — a closed subpath whose last point is *not* the first
/// comes back with the closing `LineTo` written out, since [`pen_outline`] draws
/// the closing segment and emits `ClosePath`. That last is the ordinary SVG
/// spelling, so it is the common case rather than a corner one, and it is why
/// [`rewrite_path`]'s no-op guard has to canonicalise both sides rather than
/// compare the stored path directly.
pub fn pen_path(subpaths: &[PenSubpath]) -> BezPath {
    let mut path = BezPath::new();
    for sub in subpaths {
        for el in pen_outline(&sub.anchors, sub.closed).elements() {
            path.push(*el);
        }
    }
    path
}

/// Put `subpaths` through `t` — anchors and both handles.
///
/// A handle is an *offset*, so it takes the transform's linear part only. Doing
/// that by transforming the control points the handles describe, rather than by
/// pulling the linear part out of the `Affine`, keeps it exact under a
/// translation and correct under a rotation without a second code path.
pub fn map_subpaths(subpaths: &mut [PenSubpath], t: Affine) {
    for sub in subpaths {
        for a in &mut sub.anchors {
            let (leaving, arriving) = (t * a.leaving(), t * a.arriving());
            a.at = t * a.at;
            a.out = leaving - a.at;
            a.back = arriving - a.at;
        }
    }
}

/// Read a `BezPath` back into the anchors that would draw it — the inverse of
/// [`pen_outline`], and what every point edit needs before it can start.
///
/// **The round trip is easy for paths the pen wrote and the work is everything
/// else.** `pen_outline` emits only `MoveTo`, `LineTo`, `CurveTo` and
/// `ClosePath`, one subpath, so reading one of its own paths back is
/// bookkeeping. A flattened boolean, an SVG import or a scaled path can hold
/// **quads** and **several subpaths**, and both are handled here rather than at
/// the call sites:
///
/// - A **quad** is elevated to the cubic with the same curve (the standard
///   `c1 = p0 + 2/3(c − p0)`, `c2 = p + 2/3(c − p)`), so it comes back as an
///   editable anchor pair instead of being dropped or flattened. The elevation is
///   exact in geometry and lossy in *representation*: writing back produces a
///   cubic, which is the one place the round trip is not byte-identical.
/// - A **`ClosePath` whose last point is already the first** — which is exactly
///   what `pen_outline` emits, since it draws the closing segment *and* closes —
///   folds: the duplicate anchor goes and its incoming handle moves onto the
///   first anchor, where the closing segment will look for it. Without the fold
///   every read-and-write of a closed path would grow an anchor.
///
/// A handle is recorded on the anchor it belongs to as the segments are walked,
/// so `out` is set only when the segment *leaving* an anchor is a curve — which
/// is what keeps [`PenAnchor::is_corner`], and therefore line-or-cubic on the way
/// back out, agreeing with what came in.
pub fn pen_anchors(path: &BezPath, radii: &[f64]) -> Vec<PenSubpath> {
    /// Below this, two points are the same point. Far under anything visible at
    /// any zoom, and well above the rounding an `Affine` round trip introduces.
    const SAME: f64 = 1e-9;

    let mut out: Vec<PenSubpath> = Vec::new();
    // A segment before any `MoveTo` has no anchor to hang its start on. kurbo
    // does not produce one; ignoring it keeps this total rather than panicking.
    for el in path.elements() {
        match *el {
            PathEl::MoveTo(p) => out.push(PenSubpath {
                anchors: vec![PenAnchor::corner(p)],
                closed: false,
            }),
            PathEl::LineTo(p) => {
                if let Some(sub) = out.last_mut() {
                    sub.anchors.push(PenAnchor::corner(p));
                }
            }
            PathEl::CurveTo(c1, c2, p) => {
                if let Some(sub) = out.last_mut()
                    && let Some(from) = sub.anchors.last_mut()
                {
                    from.out = c1 - from.at;
                    sub.anchors.push(PenAnchor {
                        at: p,
                        out: Vec2::ZERO,
                        back: c2 - p,
                        radius: 0.0,
                    });
                }
            }
            PathEl::QuadTo(c, p) => {
                if let Some(sub) = out.last_mut()
                    && let Some(from) = sub.anchors.last_mut()
                {
                    let p0 = from.at;
                    from.out = (c - p0) * (2.0 / 3.0);
                    sub.anchors.push(PenAnchor {
                        at: p,
                        out: Vec2::ZERO,
                        back: (c - p) * (2.0 / 3.0),
                        radius: 0.0,
                    });
                }
            }
            PathEl::ClosePath => {
                if let Some(sub) = out.last_mut() {
                    sub.closed = true;
                    let folds = sub.anchors.len() >= 2
                        && (sub.anchors[sub.anchors.len() - 1].at - sub.anchors[0].at).hypot()
                            < SAME;
                    if folds {
                        let dup = sub.anchors.pop().expect("checked len");
                        sub.anchors[0].back = dup.back;
                    }
                }
            }
        }
    }
    // The radii last, over the anchors as they finally stand — after the close
    // fold, which is the one step that changes how many there are. Distributed
    // here rather than during the walk so there is exactly one statement of
    // "anchor `base + i` takes `radii[base + i]`", and it is measured against the
    // list the caller will get back.
    let mut base = 0usize;
    for sub in &mut out {
        for (i, a) in sub.anchors.iter_mut().enumerate() {
            a.radius = radii.get(base + i).copied().unwrap_or(0.0).max(0.0);
        }
        base += sub.anchors.len();
    }
    out
}

/// The per-anchor radii of `subpaths`, flattened for the model.
///
/// **Trailing zeros trimmed, which is not tidiness.** The list is
/// `skip_serializing_if = "Vec::is_empty"`, so an unrounded path must produce an
/// *empty* vec or every path in every file grows a `corner_radii` key it does not
/// need — and byte-stable output (invariant 9) requires one canonical form for
/// one shape, which two lists differing only in trailing zeros would break. It is
/// also what `rewrite_path`'s no-op comparison needs to keep working.
pub fn pen_radii(subpaths: &[PenSubpath]) -> Vec<f64> {
    let mut radii: Vec<f64> = subpaths
        .iter()
        .flat_map(|s| s.anchors.iter().map(|a| a.radius.max(0.0)))
        .collect();
    while radii.last().is_some_and(|r| *r <= 0.0) {
        radii.pop();
    }
    radii
}

/// Move every anchor in `points` by `delta`, handles riding along.
///
/// The handles are offsets from the anchor, so moving a point is one addition
/// and the curve through it keeps its shape exactly — which is the behaviour,
/// not an implementation detail: dragging a point must bend its neighbours'
/// segments without changing how it *leaves* itself.
pub fn move_points(subpaths: &mut [PenSubpath], points: &[PointRef], delta: Vec2) {
    for (sub, anchor) in points {
        if let Some(a) = subpaths
            .get_mut(*sub)
            .and_then(|s| s.anchors.get_mut(*anchor))
        {
            a.at += delta;
        }
    }
}

/// Aim one anchor's handle at `to`, breaking the mirror when `break_pair`.
///
/// **Within `snap` of the anchor the handle retracts onto it**, which is how a
/// smooth point becomes a corner again. Dragging a handle *exactly* onto its
/// anchor is not a thing a hand can do — `is_corner` wants both handles at zero
/// to better than 1e-9 — so without this the conversion is unreachable by the
/// gesture that ought to perform it.
///
/// The substitution itself is [`PenAnchor::pull_within`], stated once on the type
/// because the pen's own shaping drag needs the same rule — see it for why.
///
/// **The direction snaps too**, by the same substitution: `tangent` is how close
/// the handle has to come to a direction worth having before it lands on it, and
/// [`tangent_snap`] says which those are.
pub fn pull_handle(
    subpaths: &mut [PenSubpath],
    at: PointRef,
    side: Side,
    to: Point,
    break_pair: bool,
    snap: f64,
    tangent: f64,
) {
    let Some(a) = subpaths.get(at.0).and_then(|s| s.anchors.get(at.1)) else {
        return;
    };
    let to = tangent_snap(a, side, to, tangent);
    if let Some(a) = subpaths.get_mut(at.0).and_then(|s| s.anchors.get_mut(at.1)) {
        a.pull_within(side, to, break_pair, snap);
    }
}

/// `to` pulled onto a direction worth having, if it is within `tolerance` of one.
///
/// **Three kinds of direction, and the first is the one this is for.**
///
/// - **Collinear with the other handle** — the direction that makes the anchor
///   *smooth*. It is the whole point: `Alt` breaks a pair easily and there was no
///   way back to a smooth point except by eye, and "nearly smooth" is exactly the
///   thing a curve shows and a hand cannot fix. Offered only when the pair is
///   **already broken**, and that guard is not tidiness. The anchor handed in here
///   still holds *last frame's* handles, so on a mirrored anchor the "smooth"
///   direction is the one this very handle is already pointing — the snap would
///   pull the handle back to where it was, and a slow drag would stick and then
///   jump. A mirrored anchor is smooth by construction anyway, so there is nothing
///   for the candidate to buy. Found by wiring this to the pen, whose every
///   unmodified drag is the mirrored case.
/// - **The eight axis and diagonal directions**, which is all this app has to
///   offer today: the same set a Shift-drawn line snaps to (`canvas::axial_corner`),
///   for the same reason — they are the directions a designer names.
///
/// **A perpendicular *distance*, not an angle.** An angular tolerance is a
/// different sized target at each end of a handle: forgiving on a short one, a
/// hair's breadth on a long one, and the long ones are where the aim matters. The
/// distance from the pointer to the candidate ray is the same target everywhere,
/// which is what the hand actually experiences. The **length is kept** — the
/// pointer is projected onto the direction rather than moved to it — so the snap
/// changes where the handle points and nothing else.
///
/// A candidate the pointer is *behind* is skipped: a projection onto the far side
/// would flip the handle through its own anchor, which is a jump, not a snap.
pub fn tangent_snap(a: &PenAnchor, side: Side, to: Point, tolerance: f64) -> Point {
    let v = to - a.at;
    if v.hypot() < 1e-9 || tolerance <= 0.0 {
        return to;
    }
    let other = match side {
        Side::Out => a.back,
        Side::Back => a.out,
    };
    let mut best: Option<(f64, Point)> = None;
    let smooth = (!a.is_smooth() && other.hypot() > 1e-9).then(|| -other.normalize());
    let axes = (0..8).map(|i| {
        let angle = std::f64::consts::FRAC_PI_4 * i as f64;
        Vec2::new(angle.cos(), angle.sin())
    });
    for dir in smooth.into_iter().chain(axes) {
        let along = v.dot(dir);
        if along <= 0.0 {
            continue;
        }
        let landed = a.at + dir * along;
        let off = (to - landed).hypot();
        if off <= tolerance && best.is_none_or(|(d, _)| off < d) {
            best = Some((off, landed));
        }
    }
    best.map_or(to, |(_, p)| p)
}

/// The two anchors a segment runs between — the one it leaves (`at`) and the one
/// it arrives at, which wraps to 0 on the closing segment of a closed subpath.
///
/// `None` when `at` names no segment: an out-of-range index, a subpath too short
/// to have one, or the last anchor of an **open** run, which nothing leaves. That
/// last case is why a segment is named by its *leaving* anchor and why this
/// answer is `Option` rather than arithmetic at the call site — the closing
/// segment and the missing one live at the same index.
pub fn segment_ends(subpaths: &[PenSubpath], at: PointRef) -> Option<(PointRef, PointRef)> {
    let sub = subpaths.get(at.0)?;
    let n = sub.anchors.len();
    if n < 2 || at.1 >= n {
        return None;
    }
    let next = (at.1 + 1) % n;
    (next != 0 || sub.closed).then_some(((at.0, at.1), (at.0, next)))
}

/// Whether the segment leaving `at` is written out as a straight line —
/// [`PenAnchor::straight_to`] asked about a segment rather than about a pair.
///
/// Not derivable from [`segment_cubic`] any more, and that is the point: the cubic
/// it returns for a line has control points a third of the way along, exactly like
/// a real curve's.
pub fn segment_is_straight(subpaths: &[PenSubpath], at: PointRef) -> bool {
    let Some((a, b)) = segment_ends(subpaths, at) else {
        return false;
    };
    match (
        subpaths.get(a.0).and_then(|s| s.anchors.get(a.1)),
        subpaths.get(b.0).and_then(|s| s.anchors.get(b.1)),
    ) {
        (Some(a), Some(b)) => a.straight_to(b),
        _ => false,
    }
}

/// The segment leaving `at`, as a cubic — `pen_outline`'s own line-or-cubic rule
/// ([`PenAnchor::straight_to`]) resolved into one type, so a caller can ask for a
/// position at a parameter without branching.
///
/// **A straight segment comes back *degree-elevated*, with its control points at
/// the thirds — not sitting on its own endpoints.** Both are the same line and it
/// looks like a distinction without a difference; it is the difference between two
/// **parameterisations**. The degenerate form puts `B(0.25)` at 15.6% along, and
/// `geometry::nearest_segment` — which measures against the real `PathSeg::Line` —
/// reports the *linear* parameter. Feed one's `t` to the other and the answer slides
/// by up to a tenth of the segment: reported as the insert affordance drifting away
/// from the pointer as it moved along a straight edge, worst at the quarter points
/// and exact at the middle, which is that error's signature. Elevated, `B(t)` is
/// `lerp(t)` identically — `[(1−t) + t]² = 1` falls out of the algebra — so the two
/// modules agree by construction rather than by luck.
pub fn segment_cubic(subpaths: &[PenSubpath], at: PointRef) -> Option<CubicBez> {
    let (a, b) = segment_ends(subpaths, at)?;
    let a = subpaths.get(a.0)?.anchors.get(a.1)?;
    let b = subpaths.get(b.0)?.anchors.get(b.1)?;
    Some(if a.straight_to(b) {
        let third = (b.at - a.at) / 3.0;
        CubicBez::new(a.at, a.at + third, b.at - third, b.at)
    } else {
        CubicBez::new(a.at, a.leaving(), b.arriving(), b.at)
    })
}

/// Where parameter `t` falls on the segment leaving `at`, in whatever space
/// `subpaths` are measured in.
///
/// What a segment gesture is *aiming*, and therefore what its snap is measured
/// against: the point under the hand, not either of the anchors that bound it.
pub fn segment_point(subpaths: &[PenSubpath], at: PointRef, t: f64) -> Option<Point> {
    Some(segment_cubic(subpaths, at)?.eval(t.clamp(0.0, 1.0)))
}

/// Split the segment leaving `at` at parameter `t`, returning the new anchor.
///
/// **The curve does not change shape.** `CubicBez::subsegment` gives the two
/// halves of the very cubic that was there, so the four control points involved —
/// the two neighbours' inward handles and the new anchor's two — are read off the
/// split rather than guessed, and the outline through the inserted point is the
/// one that was already drawn. A straight segment splits into two straight ones,
/// which falls out: both halves' control points land on their own endpoints, so
/// every handle written back is zero and the new anchor is a corner.
///
/// The new anchor takes **no radius**, and its neighbours keep theirs — which is
/// the whole payoff of a radius riding on the anchor (§15 D119) rather than
/// sitting in a list that every insert would have to renumber.
///
/// **A straight segment is its own case, and pretending otherwise grew the path.**
/// [`segment_cubic`] gives a line back as the cubic with its control points on its
/// endpoints — the same curve, traversed at the same speed — but *subdividing*
/// that cubic puts the halves' control points partway along the line rather than
/// on their own endpoints. Geometrically identical, three times the data, and not
/// what the reader gives back: exactly the shape D117 found, where a path grew
/// every time it was read and written. So a line splits by lerp and every handle
/// stays zero.
///
/// `None` when `at` names no segment (see [`segment_ends`]).
pub fn insert_point(subpaths: &mut [PenSubpath], at: PointRef, t: f64) -> Option<PointRef> {
    let t = t.clamp(0.0, 1.0);
    let (start, end) = segment_ends(subpaths, at)?;
    let a = *subpaths.get(start.0)?.anchors.get(start.1)?;
    let b = *subpaths.get(end.0)?.anchors.get(end.1)?;
    let inserted = if a.straight_to(&b) {
        PenAnchor::corner(a.at.lerp(b.at, t))
    } else {
        let curve = CubicBez::new(a.at, a.leaving(), b.arriving(), b.at);
        let (left, right) = (curve.subsegment(0.0..t), curve.subsegment(t..1.0));
        let mid = left.p3;
        let sub = subpaths.get_mut(at.0)?;
        sub.anchors[start.1].out = left.p1 - curve.p0;
        sub.anchors[end.1].back = right.p2 - curve.p3;
        PenAnchor {
            at: mid,
            out: right.p1 - mid,
            back: left.p2 - mid,
            radius: 0.0,
        }
    };
    let sub = subpaths.get_mut(at.0)?;
    // Always at `at.1 + 1`, the closing segment included: there the arriving
    // anchor is 0 again, so the new point belongs at the *end* of the run, which
    // is what an insert at `len` is.
    sub.anchors.insert(at.1 + 1, inserted);
    Some((at.0, at.1 + 1))
}

/// Remove every segment in `cuts`, leaving the anchors either side of each one
/// in place.
///
/// **The counterpart to `remove_points`' healing, and the reason the two are
/// separate verbs.** Deleting an *anchor* joins its neighbours, which the
/// representation does for free; deleting a *segment* leaves a gap, and a gap is
/// structural — a closed subpath becomes open, and an open one becomes two, or
/// three, or as many as there are cuts in it.
///
/// **One walk for any number of cuts, and the closed case is the same walk.** A
/// closed run is *rotated* so the traversal starts just after its first cut and
/// ends on it, which turns it into an open run with one fewer break to make; from
/// there both kinds are "walk the anchors in order, and end the run whenever the
/// segment leaving this one is cut". Written as a special case per cut count it
/// would have been three arms and a wrap-around, which is where the off-by-one
/// lives.
///
/// The handles at each new end are zeroed, because they described a segment that
/// has gone. Not tidiness: `pen_path` writes no segment out of the last anchor of
/// an open run or into its first, so leaving them set would be a value only the
/// app knew about — which would then reappear if the run were closed again,
/// bending a segment nobody drew.
///
/// A remainder of fewer than two anchors is dropped, as it is in
/// [`remove_points`]: one point draws nothing and can never be selected again. An
/// empty result means the path is gone and the caller deletes the node.
pub fn remove_segments(subpaths: &[PenSubpath], cuts: &[PointRef]) -> Vec<PenSubpath> {
    /// One run between two cuts, kept only if it still draws something, with the
    /// handles that described the removed segments zeroed off its new ends.
    fn push(out: &mut Vec<PenSubpath>, anchors: &mut Vec<PenAnchor>) {
        let mut anchors = std::mem::take(anchors);
        if anchors.len() < 2 {
            return;
        }
        if let Some(first) = anchors.first_mut() {
            first.back = Vec2::ZERO;
        }
        if let Some(last) = anchors.last_mut() {
            last.out = Vec2::ZERO;
        }
        out.push(PenSubpath {
            anchors,
            closed: false,
        });
    }

    let mut out: Vec<PenSubpath> = Vec::with_capacity(subpaths.len() + 1);
    for (i, sub) in subpaths.iter().enumerate() {
        // Only the cuts this subpath actually has: an index naming a segment that
        // does not exist — the last anchor of an open run — names nothing.
        let mut cut: Vec<usize> = cuts
            .iter()
            .filter(|at| at.0 == i && segment_ends(subpaths, **at).is_some())
            .map(|at| at.1)
            .collect();
        cut.sort_unstable();
        cut.dedup();
        if cut.is_empty() {
            out.push(sub.clone());
            continue;
        }
        let n = sub.anchors.len();
        // Closed: start just after the first cut, so the run ends *on* that cut
        // and the wrap-around needs no arm of its own.
        let order: Vec<usize> = match sub.closed {
            true => (cut[0] + 1..n).chain(0..=cut[0]).collect(),
            false => (0..n).collect(),
        };
        let mut run: Vec<PenAnchor> = Vec::new();
        for j in order {
            run.push(sub.anchors[j]);
            if cut.contains(&j) {
                push(&mut out, &mut run);
            }
        }
        push(&mut out, &mut run);
    }
    out
}

/// Bend a segment: pull the point at parameter `t` by `delta`, leaving both
/// anchors where they are.
///
/// **`Ctrl` shapes the curvature** — the other half of the segment drag, and the
/// same meaning `Ctrl` has on an anchor, where it pulls handles out of a point
/// that already exists.
///
/// The two control points that shape this segment take the whole of the pull,
/// split so that the point at `t` lands exactly under the pointer with the
/// *smallest* change to either: `B(t)`'s dependence on them is `u = 3(1−t)²t` and
/// `v = 3(1−t)t²`, so `u·δ₁ + v·δ₂ = delta` is one equation in two unknowns and
/// the minimum-norm solution is `δ₁ = delta·u/(u²+v²)`, `δ₂ = delta·v/(u²+v²)`.
/// At the midpoint that is a symmetric pull, which is what the gesture looks like.
///
/// A **straight** segment bends the same way, once it has control points to move.
/// It is elevated to the cubic with them at the thirds first — the same curve, and
/// the one [`segment_cubic`] already reports, so `t` means what the pointer meant
/// when it grabbed. Starting them on the endpoints instead would make the grabbed
/// point somewhere other than where the hand took hold, by up to a tenth of the
/// segment. The two corners become curve anchors, which is exactly what "shapes
/// the curvature" asks for, and the markers say so the moment it happens
/// ([`PenAnchor::is_corner`]).
///
/// The anchors' *other* handles are left alone, so bending beside a smooth point
/// breaks its pair. That is the honest reading of "bend this segment": the
/// alternative is a bend that also rotates the neighbouring segment, which the
/// hand did not ask for. Near `t = 0` or `t = 1` the two control points have no
/// leverage on the curve at all and the bend does nothing rather than diverging.
pub fn bend_segment(subpaths: &mut [PenSubpath], at: PointRef, t: f64, delta: Vec2) {
    let Some((a, b)) = segment_ends(subpaths, at) else {
        return;
    };
    // **Nothing at all for a bend that has not moved**, which is what keeps the
    // elevation below off the undo stack: it changes the representation without
    // changing the shape, so a `Ctrl`-press that never travelled would otherwise
    // rewrite the path (`rewrite_path` compares elements, and rightly).
    if delta == Vec2::ZERO {
        return;
    }
    let t = t.clamp(0.0, 1.0);
    let (u, v) = (3.0 * (1.0 - t) * (1.0 - t) * t, 3.0 * (1.0 - t) * t * t);
    let denom = u * u + v * v;
    if denom < 1e-9 {
        return;
    }
    let (d1, d2) = (delta * (u / denom), delta * (v / denom));
    // The control points this segment starts from — the elevated pair for a line,
    // which is what makes `t` mean the same thing here as it does to the pointer.
    let curve = match segment_cubic(subpaths, at) {
        Some(c) => c,
        None => return,
    };
    // Read the pair out before writing either, so a segment whose two ends are
    // the same anchor could not have one write undo the other.
    let (from, to) = (curve.p1 + d1, curve.p2 + d2);
    let anchors = (subpaths[a.0].anchors[a.1].at, subpaths[b.0].anchors[b.1].at);
    subpaths[a.0].anchors[a.1].out = from - anchors.0;
    subpaths[b.0].anchors[b.1].back = to - anchors.1;
}

/// The upright box the selected anchors occupy — the frame a scale gesture over a
/// point set is measured in.
///
/// **The anchors only, not their handles.** The box is drawn over the points and
/// scales the points; including a handle tip would make it jump out past the ink
/// whenever a curve had a long one, and the corner the hand is aiming at would
/// sit somewhere no point is.
///
/// `None` below two points: one point is a box with no extent, and there is
/// nothing to scale it about.
///
/// **`None` for a *flat* box as well**, and that is not a degenerate-arithmetic
/// guard — [`resize_factors`] already answers 1.0 on an axis with no extent. It is
/// that a flat box's four corners all land **on** the extreme points themselves,
/// where the point wins the press (`canvas::NodeGrab`), so the chrome would promise
/// four handles none of which can be grabbed. Two points one above the other are
/// the everyday case.
pub fn points_box(subpaths: &[PenSubpath], points: &[PointRef]) -> Option<Rect> {
    if points.len() < 2 {
        return None;
    }
    points
        .iter()
        .filter_map(|(s, a)| Some(subpaths.get(*s)?.anchors.get(*a)?.at))
        .map(|p| Rect::from_points(p, p))
        .reduce(|a, b| a.union(b))
        .filter(|b| b.width() > MIN_EXTENT && b.height() > MIN_EXTENT)
}

/// A run being drawn, with `arriving` attached to its tail.
///
/// **The orientation is the whole of it, and it is the opposite of the one the pen
/// resumes with.** `PenEdit::working` turns the target so the grabbed endpoint is
/// **last**, because there the pen appends to that run; here the pen appends *to
/// us*, so the grabbed endpoint has to come **first** or the join crosses the whole
/// target and back. `from_start` says the grab was at index 0 — already first, so
/// that is the case that needs no reversal, which is exactly backwards from
/// `working` and the reason this is a named function with a test rather than a
/// `if` at the call site.
pub fn join_runs(head: &[PenAnchor], arriving: &PenSubpath, from_start: bool) -> PenSubpath {
    let mut tail = arriving.clone();
    if !from_start {
        tail.reverse();
    }
    PenSubpath {
        anchors: head.iter().copied().chain(tail.anchors).collect(),
        closed: false,
    }
}

/// The two **open ends** a point selection names, or `None` when it does not name
/// exactly two — the subject *Join* acts on (`context-menus.md` §6.5).
///
/// **The predicate the menu row is gated on and the builder's own precondition,
/// spelled once**, so a live row cannot reach a [`join_ends`] that then refuses.
/// That is the same rule the kind-gated rows follow (§15 D87, D230): ask core's
/// own question rather than a restatement of it.
///
/// Three refusals, and the third is the one reading would miss:
///
/// - A **closed** run has no ends. Nothing to attach to.
/// - An anchor in the **middle** of a run is not an end; joining there would have
///   to split the run first, which is a different verb.
/// - **A run of two anchors cannot be closed.** Its two ends are the ends of its
///   only segment, so a `closed` flag adds a second segment retracing the first —
///   a shape with no interior, and an edit that looks on screen exactly like the
///   no-op it very nearly is. Splicing a two-anchor run onto a *different* one is
///   fine and is not refused; it is closing it that produces nothing.
pub fn joinable_ends(subpaths: &[PenSubpath], points: &[PointRef]) -> Option<(PointRef, PointRef)> {
    let [a, b] = points else { return None };
    let open_end = |&(s, i): &PointRef| {
        let sub = subpaths.get(s)?;
        let last = sub.anchors.len().checked_sub(1)?;
        (!sub.closed && (i == 0 || i == last)).then_some(())
    };
    open_end(a)?;
    open_end(b)?;
    if a.0 == b.0 && subpaths.get(a.0).is_some_and(|s| s.anchors.len() < 3) {
        return None;
    }
    (a != b).then_some((*a, *b))
}

/// Fuse the two open ends [`joinable_ends`] found, and say which **segment** the
/// join created.
///
/// Two shapes under one verb, which is Illustrator's *Join* and is what makes the
/// single row honest:
///
/// - Both ends of **one** run — the run closes, and the new segment is the
///   closing one, which `geometry::anchor_runs` names by the *last* anchor.
/// - Ends of **two** runs — they splice into one open run, which takes the
///   **lower** of the two subpath indices so the list order stays put, and the
///   other entry goes.
///
/// **The orientation is [`join_runs`]'s and is not re-derived here**: the selected
/// end of the head has to come last, so a head grabbed at index 0 is reversed
/// before the splice, and the tail's `from_start` is passed through to be turned
/// the other way. Getting either backwards crosses the whole run and comes back,
/// which is the trap `join_runs` is a named function to hold.
///
/// **Nothing is merged.** Two ends that happen to sit on the same point become two
/// anchors with a zero-length segment between them rather than one anchor, exactly
/// as the pen's own join leaves them (`canvas::pen_join`). Merging would need a
/// tolerance and a rule about whose handles survive; consistency with the gesture
/// is worth more than either.
pub fn join_ends(
    subpaths: &[PenSubpath],
    a: PointRef,
    b: PointRef,
) -> Option<(Vec<PenSubpath>, PointRef)> {
    let mut list = subpaths.to_vec();
    if a.0 == b.0 {
        let sub = list.get_mut(a.0)?;
        sub.closed = true;
        let last = sub.anchors.len().checked_sub(1)?;
        return Some((list, (a.0, last)));
    }
    let mut head = list.get(a.0)?.clone();
    if a.1 == 0 {
        head.reverse();
    }
    let seam = head.anchors.len().checked_sub(1)?;
    let joined = join_runs(&head.anchors, list.get(b.0)?, b.1 == 0);
    let (keep, gone) = (a.0.min(b.0), a.0.max(b.0));
    list[keep] = joined;
    list.remove(gone);
    Some((list, (keep, seam)))
}

/// Turn the selected anchors about `pivot` by `angle` radians.
///
/// Handles turn with their anchors and keep their length, which is what makes this
/// a rotation of the *shape* rather than of its vertices: the curves come round
/// with the points. They are offsets, so the same `Affine` applied to the control
/// points they describe does both, exactly as [`map_subpaths`] does — and going
/// through the control points rather than pulling the linear part out keeps a
/// zero-length handle at zero instead of at a rounding error.
pub fn rotate_points(subpaths: &mut [PenSubpath], points: &[PointRef], pivot: Point, angle: f64) {
    let t = Affine::translate(pivot.to_vec2())
        * Affine::rotate(angle)
        * Affine::translate(-pivot.to_vec2());
    for at in points {
        let Some(a) = subpaths.get_mut(at.0).and_then(|s| s.anchors.get_mut(at.1)) else {
            continue;
        };
        let (leaving, arriving) = (t * a.leaving(), t * a.arriving());
        a.at = t * a.at;
        a.out = leaving - a.at;
        a.back = arriving - a.at;
    }
}

/// Scale the selected anchors about `box_`, as a handle drag on it asks.
///
/// The arithmetic is [`resize_factors`]', unchanged — the same function a layer
/// resize and a multi-selection resize go through, so Alt means the same thing
/// here (hold the box's own centre) and a handle dragged past its anchor mirrors
/// the set rather than collapsing it.
///
/// Handles scale with their anchors, componentwise, because they are offsets: a
/// set of points squashed to half its width has its curves squashed with it,
/// which is what makes this a *scale* of the shape rather than a rearrangement of
/// its vertices. Radii are left alone — §5.6's rule, and what a `Rect`'s four
/// already do under a resize (the Scale tool is the one that multiplies them).
pub fn resize_points(
    subpaths: &mut [PenSubpath],
    points: &[PointRef],
    box_: Rect,
    handle: Handle,
    p: Point,
    opts: Resize,
) {
    let (sx, sy, anchor) = resize_factors(box_, handle, p, opts.keep_ratio, opts.symmetric);
    for at in points {
        let Some(an) = subpaths.get_mut(at.0).and_then(|s| s.anchors.get_mut(at.1)) else {
            continue;
        };
        an.at = Point::new(
            anchor.x + (an.at.x - anchor.x) * sx,
            anchor.y + (an.at.y - anchor.y) * sy,
        );
        an.out = Vec2::new(an.out.x * sx, an.out.y * sy);
        an.back = Vec2::new(an.back.x * sx, an.back.y * sy);
    }
}

/// One coordinate of an anchor, on `axis`.
fn coord(p: Point, axis: Axis) -> f64 {
    match axis {
        Axis::X => p.x,
        Axis::Y => p.y,
    }
}

fn offset(axis: Axis, d: f64) -> Vec2 {
    match axis {
        Axis::X => Vec2::new(d, 0.0),
        Axis::Y => Vec2::new(0.0, d),
    }
}

/// The extent of `points` along `axis` — `None` when fewer than two are given,
/// since one point is already aligned with itself and already evenly spaced.
fn span(subpaths: &[PenSubpath], points: &[PointRef], axis: Axis) -> Option<(f64, f64)> {
    let mut vals = points
        .iter()
        .filter_map(|(s, a)| subpaths.get(*s)?.anchors.get(*a))
        .map(|an| coord(an.at, axis));
    let first = vals.next()?;
    let (lo, hi) = vals.fold((first, first), |(lo, hi), v| (lo.min(v), hi.max(v)));
    (points.len() >= 2).then_some((lo, hi))
}

/// Align the selected anchors along `axis`, against the box **they** occupy.
///
/// **Against their own extent, and there is no other candidate.** For layers the
/// target is a real question — the key, the union, the container (`align_target`
/// weighs all three). A set of points inside one path has no key to designate and
/// no container but the path itself, whose box is derived from these very points,
/// so the union is not a default here: it is the only box in the problem.
pub fn align_points(subpaths: &mut [PenSubpath], points: &[PointRef], axis: Axis, edge: Edge) {
    let Some((lo, hi)) = span(subpaths, points, axis) else {
        return;
    };
    let to = match edge {
        Edge::Min => lo,
        Edge::Mid => (lo + hi) * 0.5,
        Edge::Max => hi,
    };
    for at in points {
        let Some(an) = subpaths.get_mut(at.0).and_then(|s| s.anchors.get_mut(at.1)) else {
            continue;
        };
        an.at += offset(axis, to - coord(an.at, axis));
    }
}

/// Space the selected anchors evenly along `axis`.
///
/// **Equal *positions*, where layers get equal gaps.** `build::distribute` has to
/// subtract each layer's own width to leave equal space between them; a point has
/// no width, so the two readings collapse into one and the arithmetic is a
/// straight lerp between the extremes. Fewer than three does nothing — the two
/// end points are pinned by definition, so there is nothing in between to place.
pub fn distribute_points(subpaths: &mut [PenSubpath], points: &[PointRef], axis: Axis) {
    if points.len() < 3 {
        return;
    }
    let Some((lo, hi)) = span(subpaths, points, axis) else {
        return;
    };
    // Sorted by where they already are, so distributing does not also reorder
    // them: the point nearest the left edge stays leftmost.
    let order = points_along(subpaths, points, axis);
    let step = (hi - lo) / (order.len() - 1).max(1) as f64;
    for (i, (at, was)) in order.iter().enumerate() {
        let Some(an) = subpaths.get_mut(at.0).and_then(|s| s.anchors.get_mut(at.1)) else {
            continue;
        };
        an.at += offset(axis, lo + step * i as f64 - was);
    }
}

/// How long a handle invented for a smooth anchor is, as a fraction of the chord
/// to the neighbour it faces.
///
/// A third is the standard construction — it is what a Catmull-Rom spline through
/// the same points reduces to, and what Illustrator's *Convert to smooth* produces
/// — and the property that matters is that it is a *fraction* rather than a length:
/// a handle a third of the way to its neighbour looks the same on a 20pt path and a
/// 2000pt one, where any absolute number is wrong on one of them.
const SMOOTH_HANDLE_FRACTION: f64 = 1.0 / 3.0;

/// Make the selected anchors corners or smooth points — Illustrator's *Convert to
/// corner / smooth*, and the menu rows over `context-menus.md` §6.5 (§15 D250).
///
/// **A corner is the easy direction and a smooth point is not**, which is what the
/// spec's "`PenAnchor::corner` and `::smooth` are the constructors" scoring misses:
/// `PenAnchor::smooth(at, out)` needs an `out`, and a corner has no handle to make
/// one from. So there are two cases, and only the second invents anything:
///
/// - **The anchor already has a handle** — a pair broken with `Alt`, or an anchor
///   that arrives straight and leaves on a curve. Then "smooth" means *mirror*, and
///   the longer handle is the one kept: it is the one that was aimed on purpose, and
///   keeping the shorter one would throw away the shaping and read as a reset.
/// - **The anchor has none.** Then the tangent comes from the neighbours — the
///   chord `next − prev`, which is the direction a curve through all three actually
///   travels in — at [`SMOOTH_HANDLE_FRACTION`] of the **shorter** leg.
///
/// **Both handles get the same length, and that is not a simplification.** A third
/// of each anchor's own leg draws a better curve — it is what a Catmull-Rom spline
/// does — and it is wrong here, because this app's *definition* of a smooth anchor
/// is `back == −out` exactly (`PenAnchor::pull_side` builds one that way and
/// `is_smooth` reads it that way). An anchor with collinear handles of different
/// lengths is not smooth by that predicate, so the very next handle drag would take
/// the `break_pair` arm and pull the two apart — a *Make smooth* whose result comes
/// unsmoothed the moment it is touched. Found by running it; reading the constructor
/// would not have shown it. The shorter leg is then the one that sets the length,
/// since a handle longer than a third of its own leg starts to loop and only the
/// short side can be overshot.
///
/// **An endpoint of an open subpath gets one handle, not two.** It has one segment,
/// so a mirrored handle would point at nothing and still be draggable — a control
/// over a curve that does not exist. It is therefore not "smooth" by
/// [`PenAnchor::is_smooth`] afterwards, and that is honest rather than a gap: an
/// endpoint is not a joint between two segments, which is the only thing smoothness
/// is a property of.
///
/// **The corner radius is left alone in both directions.** A radius on a smooth
/// anchor already draws nothing — `geometry::fillet` returns `None` for collinear
/// tangents and says so — so clearing it here would be discarding a number the user
/// typed in exchange for nothing, and it comes back the moment the anchor is a
/// corner again.
pub fn set_smoothness(subpaths: &mut [PenSubpath], points: &[PointRef], smooth: bool) {
    for at in points {
        let Some(sub) = subpaths.get(at.0) else {
            continue;
        };
        let Some(handles) = (if smooth {
            smoothed_handles(sub, at.1)
        } else {
            sub.anchors.get(at.1).map(|_| (Vec2::ZERO, Vec2::ZERO))
        }) else {
            continue;
        };
        if let Some(an) = subpaths.get_mut(at.0).and_then(|s| s.anchors.get_mut(at.1)) {
            (an.out, an.back) = handles;
        }
    }
}

/// The `(out, back)` a smoothed anchor `i` of `sub` should carry, or `None` when
/// there is nothing to derive one from — a lone anchor, or a chord of zero length.
///
/// Split out so the arithmetic is testable against a subpath directly, and because
/// it is the whole of the decision [`set_smoothness`] documents.
fn smoothed_handles(sub: &PenSubpath, i: usize) -> Option<(Vec2, Vec2)> {
    let an = sub.anchors.get(i)?;
    let n = sub.anchors.len();
    let prev = if i > 0 {
        Some(i - 1)
    } else if sub.closed && n > 1 {
        Some(n - 1)
    } else {
        None
    };
    let next = if i + 1 < n {
        Some(i + 1)
    } else if sub.closed && n > 1 {
        Some(0)
    } else {
        None
    };
    // Already shaped: mirror the handle that was aimed on purpose.
    //
    // ⚠️ **Which side exists is asked here too, and it used to be asked only
    // below** (§15 D560). The endpoint arms at the bottom of this function are the
    // ones that return `Vec2::ZERO` on the missing side, and they were unreachable
    // for any anchor that already carried a handle — so *Convert to smooth* on the
    // first anchor of a path beginning `M 0 0 C …` mirrored it into a `back`
    // pointing off the end, which `node_grab` and the overlay both draw and offer,
    // and `is_smooth()` then answered `true` so the next drag on the real handle
    // swung the invisible one. That is the doc's *"one handle, not two"* and D250's
    // rule verbatim; the test above measures both ends.
    //
    // ⚠️ **A lone anchor keeps the mirrored pair, and that is deliberate rather
    // than an oversight in the arm below.** It has no segment on *either* side, so
    // D250's argument would zero both handles — but the shaping is already there
    // to read, `making_a_point_smooth_mirrors_a_handle_or_derives_one_from_the_neighbours`
    // asserts the mirror on exactly that fixture, and a corner in the same
    // position gets `None` from the derive arms rather than an invention. The
    // asymmetry is the existing decision — *no invention where there is shaping to
    // read* — and this entry does not reopen it.
    let (out, back) = (an.out, an.back);
    if out.hypot() > 1e-9 || back.hypot() > 1e-9 {
        let long = if out.hypot() >= back.hypot() {
            out
        } else {
            -back
        };
        return Some(match (prev.is_some(), next.is_some()) {
            (false, true) => (long, Vec2::ZERO),
            (true, false) => (Vec2::ZERO, -long),
            _ => (long, -long),
        });
    }
    let leg = |j: usize| sub.anchors.get(j).map(|o| o.at - an.at);
    match (prev.and_then(leg), next.and_then(leg)) {
        // The through-tangent: `next − prev`, not either leg, so a curve through
        // all three travels the way it looks like it should.
        (Some(back_leg), Some(out_leg)) => {
            let dir = out_leg - back_leg;
            (dir.hypot() > 1e-9).then(|| {
                let unit = dir / dir.hypot();
                // One length for both, from the shorter leg — see the note above on
                // why an asymmetric pair is not smooth here however good it looks.
                let len = out_leg.hypot().min(back_leg.hypot()) * SMOOTH_HANDLE_FRACTION;
                (unit * len, -unit * len)
            })
        }
        // An endpoint: one segment, one handle, aimed along it.
        (Some(leg), None) => {
            (leg.hypot() > 1e-9).then(|| (Vec2::ZERO, leg * SMOOTH_HANDLE_FRACTION))
        }
        (None, Some(leg)) => {
            (leg.hypot() > 1e-9).then(|| (leg * SMOOTH_HANDLE_FRACTION, Vec2::ZERO))
        }
        (None, None) => None,
    }
}

/// The selected anchors, paired with where they sit on `axis`, **sorted along
/// it** — the order both point arrangements lay out in.
fn points_along(subpaths: &[PenSubpath], points: &[PointRef], axis: Axis) -> Vec<(PointRef, f64)> {
    let mut order: Vec<(PointRef, f64)> = points
        .iter()
        .filter_map(|at| {
            let an = subpaths.get(at.0)?.anchors.get(at.1)?;
            Some((*at, coord(an.at, axis)))
        })
        .collect();
    order.sort_by(|a, b| a.1.total_cmp(&b.1));
    order
}

/// The distance between each neighbouring pair of selected anchors along `axis`.
///
/// The point subject's answer to `build::gaps_along`, and **the same number means
/// something slightly different here**: a layer's gap is measured between facing
/// *edges* and a point has none, so this is centre-to-centre — which is what the
/// even distribution already collapses the two readings into
/// ([`distribute_points`]).
pub fn point_spacings(subpaths: &[PenSubpath], points: &[PointRef], axis: Axis) -> Vec<f64> {
    points_along(subpaths, points, axis)
        .windows(2)
        .map(|pair| pair[1].1 - pair[0].1)
        .collect()
}

/// Put exactly `gap` between each neighbouring pair of selected anchors along
/// `axis`, holding the **first on that axis** where it is.
///
/// The point subject's answer to `build::distribute_spacing` (§15 D243). It takes no anchor
/// argument because a set of points has no key to designate — the same reason
/// [`align_points`] takes no target — so the first is not a fallback here, it is
/// the only candidate.
///
/// Two points are enough, for the reason `distribute_spacing` states: two have
/// one gap, and [`distribute_points`] needs three only because with two there is
/// no interior to place anything in.
pub fn space_points(subpaths: &mut [PenSubpath], points: &[PointRef], axis: Axis, gap: f64) {
    if points.len() < 2 || !gap.is_finite() {
        return;
    }
    let order = points_along(subpaths, points, axis);
    let Some(start) = order.first().map(|(_, v)| *v) else {
        return;
    };
    for (i, (at, was)) in order.iter().enumerate() {
        let Some(an) = subpaths.get_mut(at.0).and_then(|s| s.anchors.get_mut(at.1)) else {
            continue;
        };
        an.at += offset(axis, start + gap * i as f64 - was);
    }
}

/// Remove `points`, healing each subpath by joining the anchors either side.
///
/// **Healing is what falls out of the representation, and that is the argument
/// for it being the default.** A `PenSubpath` is a run of anchors with the
/// segments implied between them, so dropping one leaves its neighbours adjacent
/// and the curve simply runs from one to the other through the handles they
/// already have. Removing the *segment* instead — leaving a gap — is the edit
/// that needs machinery, and it is [`remove_segments`]: a separate verb on the same
/// key rather than the other half of a modifier, chosen by which of the two things
/// is selected.
///
/// A subpath left with fewer than two anchors is dropped: one point draws
/// nothing, and leaving it would be a subpath that renders as nothing and can
/// never be selected again. An empty result means the path is gone, and the
/// caller deletes the node rather than committing a path with no ink.
pub fn remove_points(subpaths: &[PenSubpath], points: &[PointRef]) -> Vec<PenSubpath> {
    let mut out: Vec<PenSubpath> = Vec::with_capacity(subpaths.len());
    for (i, sub) in subpaths.iter().enumerate() {
        let anchors: Vec<PenAnchor> = sub
            .anchors
            .iter()
            .enumerate()
            .filter(|(j, _)| !points.contains(&(i, *j)))
            .map(|(_, a)| *a)
            .collect();
        if anchors.len() >= 2 {
            out.push(PenSubpath {
                anchors,
                closed: sub.closed,
            });
        }
    }
    out
}

/// The anchor count of each subpath — what `PointSet::retain_valid` checks a
/// stale selection against.
pub fn subpath_lengths(subpaths: &[PenSubpath]) -> Vec<usize> {
    subpaths.iter().map(|s| s.anchors.len()).collect()
}

/// Which subpaths a set of point (or segment) references lands in, ascending and
/// without repeats.
///
/// The bridge between a selection, which is per-*point*, and the verbs whose
/// subject is a whole run — reversing one, and one day splitting or closing one.
/// Spelled here rather than at the call site because "the subpaths these points
/// are on" is the same question every such verb asks.
pub fn subpaths_touched(points: &[PointRef]) -> Vec<usize> {
    let mut out: Vec<usize> = points.iter().map(|(s, _)| *s).collect();
    out.sort_unstable();
    out.dedup();
    out
}

/// Traverse the named subpaths the other way round.
///
/// **The verb a contained subpath needs, and the reason it cannot be automatic.**
/// We fill non-zero (`boolean.rs`), so a subpath drawn the same way round as the
/// one containing it fills *solid* and the opposite way cuts a *hole* — the draw
/// direction is the only thing that decides it, and nothing on screen says which
/// you are about to get. Auto-reversing a contained run would be magic, and it
/// breaks the moment two runs partly overlap, where neither answer is "inside"
/// (§15 D125). So the direction stays the designer's, and this is how it is
/// changed after the fact instead of by deleting the run and drawing it again.
///
/// [`PenSubpath::reverse`] does the work and already carried the handle swap; the
/// list form is here so the model boundary sees one call, and so the **index
/// remap it forces** has exactly one partner to be read against
/// (`PointSet::reverse_within`).
pub fn reverse_subpaths(subpaths: &mut [PenSubpath], which: &[usize]) {
    for i in which {
        if let Some(sub) = subpaths.get_mut(*i) {
            sub.reverse();
        }
    }
}

/// Where the pen should snap to close the path, at pointer position `world`.
///
/// `None` when closing is not on offer or the pointer is not near enough; `Some`
/// is the first anchor's position, which the caller uses **both** to draw the
/// pending segment and to decide that a press closes.
///
/// **One answer for the snap and for the click, which is the whole point.** The
/// snap is the affordance — the pending segment jumps onto the start point and
/// stops there — so it is a promise that the next press will close. Two radii,
/// one for the look and one for the behaviour, is that promise being broken in
/// whichever direction they differ: a click that closes with no snap, or a snap
/// that closes nothing.
///
/// Only the **first** anchor, not any placed anchor. It is the only one with
/// something to offer — snapping to a middle anchor would put a second point on
/// top of an existing one, which is not a thing anyone is reaching for.
///
/// `radius_px` is screen pt, so the world tolerance shrinks as the view zooms in
/// and the affordance stays the same size under the pointer.
pub fn pen_close_snap(
    anchors: &[PenAnchor],
    world: Point,
    zoom: f64,
    radius_px: f64,
) -> Option<Point> {
    // Two anchors, because closing a single point is not a shape: `pen_outline`
    // draws nothing for it and `create_path` refuses it.
    let first = anchors.first().filter(|_| anchors.len() >= 2)?;
    let near = (first.at - world).hypot() * zoom < radius_px;
    near.then_some(first.at)
}

/// Create a `Path` node from world-space pen `anchors` under `parent`, stroked
/// with `color` at `width`.
///
/// The node origin sits at the first anchor; everything else is stored relative
/// to it in parent-local space. A segment is a **line** when both of the anchors
/// bounding it are corners, and a cubic otherwise — so a path drawn entirely
/// with clicks comes out as the polyline it looks like, rather than as curves
/// whose control points happen to be collinear. `close` joins the last anchor
/// back to the first, through their handles.
///
/// Fewer than two anchors yields an empty transaction.
#[allow(clippy::too_many_arguments)]
pub fn create_path(
    id: NodeId,
    parent: NodeId,
    index: usize,
    parent_world: Affine,
    anchors: &[PenAnchor],
    close: bool,
    color: Color,
    width: f64,
) -> Transaction {
    if anchors.len() < 2 {
        return Transaction(vec![]);
    }
    let origin = parent_world.inverse() * anchors[0].at;
    let mut path = pen_outline(anchors, close);
    // World → parent-local → node-local, so the first anchor lands on the
    // node's own origin and the transform carries where that is.
    path.apply_affine(Affine::translate(-origin.to_vec2()) * parent_world.inverse());
    Transaction(vec![
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Path {
                path,
                // The pen draws no radii, but reading them off the anchors rather
                // than writing `Vec::new()` is what makes this survive a pen that
                // one day does.
                corner_radii: pen_radii(&[PenSubpath {
                    anchors: anchors.to_vec(),
                    closed: close,
                }]),
            },
            transform: Some(Affine::translate(origin.to_vec2())),
            name: None,
        },
        Operation::SetStrokes {
            id,
            strokes: vec![Stroke {
                brush: Brush::Solid(color),
                width,
                ..Default::default()
            }],
        },
    ])
}

/// Write world-space `subpaths` back into the existing `Path` node `id`, whose
/// own world transform is `node_world`.
///
/// The partner of [`create_path`] for the resume case, and different from it in
/// the one way that matters: **the transform is not touched**. `create_path` puts
/// the node origin on the first anchor, which is right for a new node and wrong
/// for an edit — extending the start of a path would move its origin, and every
/// number the inspector shows for it with it. So the path is expressed in the
/// node's own space and the node stays where it is.
///
/// Fewer than two anchors in every subpath yields an empty transaction, so a
/// resumed path cannot be reduced to a point — and so does a rewrite that
/// matches `current`, the path the node already holds.
///
/// **That second case is why `current` is a parameter at all.** Resuming a path
/// and then pressing Escape has to leave the document untouched, and counting
/// anchors does not establish that — a backspace and a click leave the count
/// where it was. Comparing the finished path against the stored one catches
/// every way of changing nothing, including the ones nobody thought of. It only
/// works because the caller hands the subpath back in the orientation it was
/// read in (`canvas::finish_pen` undoes the resume-from-the-start reversal), so
/// an untouched path really does compare equal.
pub fn rewrite_path(
    id: NodeId,
    node_world: Affine,
    subpaths: &[PenSubpath],
    current: Option<(&BezPath, &[f64])>,
) -> Transaction {
    if !subpaths.iter().any(|s| s.anchors.len() >= 2) {
        return Transaction(vec![]);
    }
    let mut path = pen_path(subpaths);
    path.apply_affine(node_world.inverse());
    let corner_radii = pen_radii(subpaths);
    // **The radii are part of "changed nothing".** Scrubbing a corner radius back
    // to where it started leaves the outline identical, so comparing the path
    // alone would call that a no-op and drop the edit — the same class of bug as
    // the text session comparing content without its spans (§15 D109's neighbour).
    //
    // ⚠️ **Both sides are put in the pen's own canonical form first** (§15 D583),
    // and without that this guard cannot fire for most of the paths in a real
    // document. `path` has been through `pen_anchors` → `pen_path`, and that
    // round trip is **not** the identity for the ordinary SVG spelling of a
    // closed shape: `pen_outline` draws the closing segment *and* emits
    // `ClosePath`, so `M0,0 L80,0 L80,60 Z` — byte for byte what `svg_in` builds
    // for a `<polygon>`, and what `BezPath::from_svg` gives for any `d` ending in
    // `Z` — comes back with an extra `LineTo(0,0)`. Compared raw, a *click* into
    // the corner-radius field wrote a `SetGeometry` and the saved outline grew a
    // segment the user never drew. `pen_radii`'s trailing-zero trim is the same
    // failure on the other half: a stored `[0,0,0]` is not `[]`.
    //
    // **Comparing forward rather than normalising the model**, which is the other
    // half of the choice D117 left open: canonicalising on load would be a
    // migration and invariant 9's territory, and this needs neither. Nothing is
    // lost by it — the two forms differ only in a segment of zero length and in
    // radii that are already zero, so a `true` here is never a dropped edit. A
    // quad canonicalises to the cubic with the same curve, so a bare click on an
    // imported quad no longer rewrites it either, which is the same argument.
    let canonical = current.map(|(c, r)| {
        let subs = pen_anchors(c, r);
        (pen_path(&subs), pen_radii(&subs))
    });
    if canonical.is_some_and(|(c, r)| c.elements() == path.elements() && r == corner_radii) {
        return Transaction(vec![]);
    }
    Transaction(vec![Operation::SetGeometry {
        id,
        geometry: GeometryPatch::Path { path, corner_radii },
    }])
}

#[cfg(test)]
mod tests {
    // Moving/reparenting under a transformed parent is covered by
    // `ondin-core/tests/build.rs`, which owns those projections now.
    use super::*;
    use ondin_core::IdSource;

    /// **A crop handle holds the opposite edges and stops at the picture.**
    ///
    /// The clamp is the half worth a test: without it a handle dragged past the
    /// photograph gives a window onto nothing, which draws as `Extend::Pad`
    /// smearing the edge row — the failure that reads as a corrupt image rather
    /// than as a gesture running out of room.
    #[test]
    fn a_crop_handle_holds_the_opposite_edges_and_stops_at_the_picture() {
        let frame = Rect::new(0.0, 0.0, 100.0, 100.0);
        // The picture overflows the frame to the right and below, as a Fill does.
        let limit = Rect::new(-20.0, -10.0, 160.0, 140.0);

        // Pulled in: the held corner does not move, and the grabbed one lands on
        // the pointer exactly.
        let in_ = crop_resized(frame, Handle::BottomRight, Point::new(60.0, 70.0), limit);
        assert_eq!(in_, Rect::new(0.0, 0.0, 60.0, 70.0));

        // Pushed out past the picture: it stops on the picture's edge, not on the
        // pointer.
        let out = crop_resized(frame, Handle::BottomRight, Point::new(500.0, 500.0), limit);
        assert_eq!(out, Rect::new(0.0, 0.0, limit.x1, limit.y1));

        // An edge handle owns one axis and says nothing about the other — the
        // case that passes vacuously against an implementation written only for
        // corners, since a corner moves both.
        let side = crop_resized(frame, Handle::Left, Point::new(-50.0, 999.0), limit);
        assert_eq!(
            side,
            Rect::new(limit.x0, 0.0, 100.0, 100.0),
            "a left drag moved an edge it does not own"
        );

        // **Dragged clean past the held edge**, which is where a crop handle and a
        // resize handle part company: a resize mirrors the layer, and a crop must
        // not, because a mirrored window flips the photograph inside a mode whose
        // premise is that the photograph does not move.
        //
        // Asserted as an exact rectangle rather than as "not inverted": the
        // mirroring version also satisfies `x0 < x1` — it just swaps which edge is
        // which — so the weaker claim passes against the bug, which is how it got
        // as far as `a_crop_resize_writes_the_very_box_its_crop_was_computed_against`
        // to be caught. The held corner staying put is what distinguishes them.
        let past = crop_resized(
            frame,
            Handle::BottomRight,
            Point::new(-500.0, -500.0),
            limit,
        );
        assert_eq!(
            past,
            Rect::new(0.0, 0.0, MIN_CROP, MIN_CROP),
            "a handle run across the box must collapse onto the held corner, not \
             mirror about it"
        );
        // And the same from the other corner, where the moved edge is the *near*
        // one — the arm an implementation written for `BottomRight` alone gets
        // backwards.
        let past_tl = crop_resized(frame, Handle::TopLeft, Point::new(500.0, 500.0), limit);
        assert_eq!(
            past_tl,
            Rect::new(100.0 - MIN_CROP, 100.0 - MIN_CROP, 100.0, 100.0),
            "the top-left handle must collapse onto the bottom-right corner"
        );
    }

    /// **The first *visible* image fill**, which is two rules and neither is the
    /// obvious `position(is_image)`: a hidden picture would be cropped by a
    /// gesture with nothing on screen to show for it, and a solid above an image
    /// must not shadow it.
    #[test]
    fn the_cropped_fill_is_the_first_visible_image() {
        use ondin_core::{Fill, Paint, image_brush};
        let img = |v: bool| Fill {
            brush: image_brush(ondin_core::ImageId("a".into())),
            visible: v,
        };
        let solid = Fill {
            brush: ondin_core::Brush::Solid(ondin_core::peniko::Color::BLACK),
            visible: true,
        };
        let paint = |fills: Vec<Fill>| Paint {
            fills,
            strokes: Vec::new(),
        };
        assert_eq!(
            croppable_fill(&paint(vec![solid.clone(), img(true)])),
            Some(1)
        );
        assert_eq!(
            croppable_fill(&paint(vec![img(false), img(true)])),
            Some(1),
            "a hidden picture is not the one being cropped"
        );
        assert_eq!(croppable_fill(&paint(vec![solid.clone()])), None);
        assert_eq!(croppable_fill(&paint(vec![img(false)])), None);
        assert_eq!(croppable_fill(&paint(Vec::new())), None);
    }

    /// **The box the crop is computed against is the box that gets written.**
    ///
    /// `crop_resize_tx` computes a rectangle with `crop_resized`, derives the
    /// new crop from it with `ondin_core::crop_reframed`, and then asks
    /// `resize_layer` for the geometry by handing it that rectangle's own
    /// corner in world space. The claim in both doc comments is that the two
    /// therefore agree *by construction* rather than by two people writing the
    /// same arithmetic twice — and that claim is exactly the kind that is true
    /// when written and false after somebody adds a floor, a snap or an aspect
    /// rule to one side of it. So it is asserted rather than argued.
    ///
    /// A disagreement here is not a rounding artefact on screen: the crop was
    /// computed for one frame and the layer ends up as another, so the picture
    /// slides by the difference — which is precisely the "I dragged the handle
    /// and the photograph moved" failure the whole gesture exists to prevent.
    ///
    /// **Compared in world space, and that is the finding rather than a
    /// convenience.** A `Rect`'s geometry always starts at its own local origin,
    /// so `resize_to_handle` writes the new size *and translates the transform*
    /// to meet it: drag the top-left corner of a 50×50 in by (11, 7) and the
    /// local box becomes `(0,0)..(39,43)` under a transform that moved. The
    /// rectangle `crop_resized` returned — `(11,7)..(50,50)` — is in the *old*
    /// local frame, and the two describe the same place. This test asserted the
    /// local boxes first and failed on exactly that, which is worth recording:
    /// the crop is unaffected either way, because it is normalized to the source
    /// and `ImageRef::framing` maps it onto whatever the frame has become.
    #[test]
    fn a_crop_resize_writes_the_very_box_its_crop_was_computed_against() {
        let (doc, res, rect) = artboard_with_rect();
        let frame = ondin_core::local_box(&doc, &res, rect).expect("a box");
        let world = res.world_transform(rect).expect("a transform");
        // Room to grow in every direction, so a handle pulled out is testing the
        // resize rather than the clamp — which has its own test.
        let limit = frame.inflate(200.0, 200.0);

        for (handle, at) in [
            (Handle::BottomRight, Point::new(30.0, 20.0)),
            (Handle::TopLeft, Point::new(11.0, 7.0)),
            (Handle::Left, Point::new(-40.0, 0.0)),
            (Handle::Bottom, Point::new(0.0, 90.0)),
            // Dragged past the held edge, where `crop_resized`'s floor fires and
            // `resized_box`'s `max(1.0)` has to fire on the same number.
            (Handle::TopLeft, Point::new(999.0, 999.0)),
        ] {
            let next = crop_resized(frame, handle, at, limit);
            let (ux, uy) = handle.unit();
            let corner = Point::new(
                next.min_x() + next.width() * ux,
                next.min_y() + next.height() * uy,
            );
            let tx = resize_layer(
                &doc,
                &res,
                rect,
                handle,
                world * corner,
                Resize::geometry(false, false),
            );
            let mut after = doc.clone();
            after.apply(&tx).expect("the resize applies");
            let res_after = Resolved::rebuild(&after);
            let local = ondin_core::local_box(&after, &res_after, rect).expect("a box after");
            let world_after = res_after.world_transform(rect).expect("a transform after");
            // Both rectangles into the page's own space, where "the same box"
            // means the same thing on both sides of a re-origining resize.
            let want = world.transform_rect_bbox(next);
            let got = world_after.transform_rect_bbox(local);
            assert!(
                (got.x0 - want.x0).abs() < 1e-6
                    && (got.y0 - want.y0).abs() < 1e-6
                    && (got.x1 - want.x1).abs() < 1e-6
                    && (got.y1 - want.y1).abs() < 1e-6,
                "{handle:?} to {at:?}: the crop was computed for {want:?} in world \
                 space and the layer became {got:?} — the picture slides by the \
                 difference"
            );
        }
    }

    /// **Cropping does not move the photograph.** The whole gesture, end to end,
    /// asserted at the symptom.
    ///
    /// The three pieces are each tested on their own — `crop_resized` bounds the
    /// window, `crop_reframed` re-reads the mapping, `resize_layer` writes the
    /// box — and every one of them can be right while the composition is wrong,
    /// which is the failure the user would actually report: *I dragged the edge
    /// in and the picture jumped.* So this measures the thing the eye measures.
    /// The brush transform is source pixels → local (`ImageRef::framing`), so
    /// composed with the node's world transform it is source pixels → the page,
    /// and that expression is where the photograph *is*. It must come out
    /// identical on both sides of the drag.
    ///
    /// It covers the sign of every step at once: a `crop_reframed` that slid the
    /// rectangle, a handle that flipped past its held edge, a `resize_layer`
    /// aimed at the wrong corner — each moves this expression, and none of them
    /// moves it by a rounding bit.
    #[test]
    fn a_crop_drag_leaves_the_photograph_exactly_where_it_was() {
        let (doc, res, rect) = artboard_with_rect();
        let frame = ondin_core::local_box(&doc, &res, rect).expect("a box");
        let world = res.world_transform(rect).expect("a transform");
        // An oblong source at an off-centre crop, so a rule that only holds for a
        // square picture centred in its frame has somewhere to go wrong.
        let (w, h) = (400u32, 300u32);
        let img = ondin_core::ImageRef {
            fit: ondin_core::ImageFit::Crop,
            crop: Rect::new(0.2, 0.1, 0.9, 0.6),
            ..ondin_core::ImageRef::new(ondin_core::ImageId("x".into()))
        };
        let limit = img.picture_box(frame, w, h).expect("a picture");
        let before = world * img.framing(frame, w, h).transform;

        for (handle, at) in [
            (Handle::BottomRight, Point::new(30.0, 20.0)),
            (Handle::TopLeft, Point::new(12.0, 9.0)),
            (Handle::Left, Point::new(-30.0, 0.0)),
            (Handle::Bottom, Point::new(0.0, 120.0)),
            // Out to the picture's own edge, where the clamp does the deciding.
            (Handle::TopLeft, Point::new(-9_999.0, -9_999.0)),
        ] {
            let next = crop_resized(frame, handle, at, limit);
            let after_img = ondin_core::ImageRef {
                crop: ondin_core::crop_reframed(img.crop, frame, next),
                ..img.clone()
            };
            let (ux, uy) = handle.unit();
            let corner = Point::new(
                next.min_x() + next.width() * ux,
                next.min_y() + next.height() * uy,
            );
            let tx = resize_layer(
                &doc,
                &res,
                rect,
                handle,
                world * corner,
                Resize::geometry(false, false),
            );
            let mut after = doc.clone();
            after.apply(&tx).expect("the resize applies");
            let res_after = Resolved::rebuild(&after);
            let frame_after = ondin_core::local_box(&after, &res_after, rect).expect("a box after");
            let world_after = res_after.world_transform(rect).expect("a transform after");
            let got = world_after * after_img.framing(frame_after, w, h).transform;

            let (a, b) = (before.as_coeffs(), got.as_coeffs());
            assert!(
                a.iter().zip(b.iter()).all(|(x, y)| (x - y).abs() < 1e-6),
                "{handle:?} to {at:?}: the picture was at {a:?} and is now at \
                 {b:?} — cropping moved the photograph"
            );
            // And the crop stayed inside the source, which is what the clamp is
            // for: outside it `Extend::Pad` smears the edge row into the frame.
            let c = after_img.crop;
            assert!(
                c.x0 >= -1e-9 && c.y0 >= -1e-9 && c.x1 <= 1.0 + 1e-9 && c.y1 <= 1.0 + 1e-9,
                "{handle:?} to {at:?}: the crop left the source: {c:?}"
            );
        }
    }

    fn artboard_with_rect() -> (Document, Resolved, NodeId) {
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        let rect = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 400.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: rect,
                parent: ab,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(50.0, 50.0),
                    corner_radii: RoundedRectRadii::default(),
                },
                transform: Some(Affine::translate((100.0, 100.0))),
                name: None,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, rect)
    }

    /// A group holding one text node in `sizing`, offset inside it.
    fn group_with_text(sizing: TextSizing) -> (Document, Resolved, NodeId, NodeId) {
        let mut ids = IdSource::new(7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let group = ids.mint();
        let text = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: text,
                parent: group,
                index: 0,
                kind: NodeKind::Text {
                    content: "Hello there".into(),
                    style: Box::new(ondin_core::TextStyle {
                        font_family: "Inter".into(),
                        font_size: 16.0,
                        weight: 400,
                        italic: false,
                        line_height: Some(ondin_core::Length::Em(1.2)),
                        ..Default::default()
                    }),
                    spans: Default::default(),
                    para_spans: Default::default(),
                    paragraph: Default::default(),
                    block: Default::default(),
                    sizing,
                    on_path: None,
                    on_path_flip: false,
                    on_path_offset: 0.0,
                },
                transform: Some(Affine::translate((10.0, 10.0))),
                name: None,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, group, text)
    }

    /// The text child's sizing after the group's bottom-right corner is dragged
    /// out to exactly double the group's box on both axes.
    ///
    /// The pointer is placed a *whole extra extent* past the held corner rather
    /// than at twice the box's far edge: the anchor is the group's minimum corner,
    /// not the origin, so `2 × max` is a factor of 2 only for a box that starts at
    /// zero — and this one is offset inside its group.
    fn double_group(sizing: TextSizing) -> TextSizing {
        let (mut doc, res, group, text) = group_with_text(sizing);
        let local = ondin_core::local_box(&doc, &res, group).unwrap();
        doc.apply(&resize_group(
            &doc,
            &res,
            group,
            local,
            Handle::BottomRight,
            Point::new(
                local.min_x() + 2.0 * local.width(),
                local.min_y() + 2.0 * local.height(),
            ),
            Resize::geometry(false, false),
        ))
        .unwrap();
        sizing_of(&doc, text)
    }

    /// **Resizing a group must not collapse an auto-sized text child.**
    ///
    /// `resizable_size` reports `Size::ZERO` for auto text — it has no size
    /// field, its box comes from its content — and the `max(1.0)` that stops a
    /// resize driving a shape to nothing turned that zero into a **1×1 box**,
    /// silently converting the layer to fixed sizing at a size that shows one
    /// character. It has to be skipped before that arithmetic, not after.
    #[test]
    fn scaling_a_group_leaves_auto_sized_text_alone() {
        let sizing = double_group(TextSizing::Auto);
        assert!(
            matches!(sizing, TextSizing::Auto),
            "auto text was converted to {sizing:?} by a group resize"
        );
    }

    /// **The same collapse, one axis over.** Auto-height reports `(w, 0)` rather
    /// than no box at all, so it walked straight past the `Auto` guard into the
    /// clamp and came out `Fixed(w × 2, 1)` — a group resize turning a wrapping
    /// paragraph into a 1px sliver. Its width is a field and scales; its height
    /// is still the content's.
    #[test]
    fn scaling_a_group_scales_an_auto_height_child_without_fixing_it() {
        let sizing = double_group(TextSizing::AutoHeight(50.0));
        assert!(
            matches!(sizing, TextSizing::AutoHeight(w) if (w - 100.0).abs() < 1e-9),
            "expected auto-height at 100, got {sizing:?}"
        );
    }

    /// A group of two rects, deliberately **not** starting at its own origin: the
    /// first child sits at (10,5) inside it, so the group's local box runs
    /// 10,5..160,80 — 150 × 75. The group itself is offset under the root, so a
    /// world transform is in play too.
    fn group_of_two_rects() -> (Document, Resolved, NodeId) {
        let mut ids = IdSource::new(11);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let group = ids.mint();
        let (a, b) = (ids.mint(), ids.mint());
        let rect = |size: Size| NodeKind::Rect {
            size,
            corner_radii: RoundedRectRadii::default(),
        };
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                transform: Some(Affine::translate((30.0, 20.0))),
                name: None,
            },
            Operation::CreateNode {
                id: a,
                parent: group,
                index: 0,
                kind: rect(Size::new(40.0, 20.0)),
                transform: Some(Affine::translate((10.0, 5.0))),
                name: None,
            },
            Operation::CreateNode {
                id: b,
                parent: group,
                index: 1,
                kind: rect(Size::new(60.0, 30.0)),
                transform: Some(Affine::translate((100.0, 50.0))),
                name: None,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, group)
    }

    /// **A group's W and H are a real numeric resize**, which is what the
    /// inspector row added in §15 D242 rides on — the one thing a handle drag
    /// could do that no field could. `resize_box_to` already had the container
    /// arm (`resize_layer` dispatches a node with children to `resize_group`);
    /// nothing had ever asserted it, so this is the whole path the field walks.
    ///
    /// **The fixture's group does not start at its own origin, and that is what
    /// makes the position assertion worth having.** Scaling the contents about
    /// (0,0) instead of about the held corner — the shape a hand-written answer
    /// takes, since a group has no `size` field to assign — doubles this box to
    /// exactly 300 × 150 as well and walks its minimum corner from (10,5) to
    /// (20,10). Asserting the size alone would pass against it. *That one is
    /// arithmetic rather than a run;* what was run is holding the **wrong**
    /// corner (`Handle::TopLeft` in `resize_box_to`), which fails on the size —
    /// 150 × 75 out of a 300 × 150 request.
    #[test]
    fn typing_a_width_on_a_group_resizes_it_from_its_own_top_left() {
        let (mut doc, res, group) = group_of_two_rects();
        let before = ondin_core::local_box(&doc, &res, group).unwrap();
        assert!(
            (before.width() - 150.0).abs() < 1e-9 && (before.height() - 75.0).abs() < 1e-9,
            "the fixture's group measured {before:?} rather than 150 × 75"
        );
        doc.apply(&resize_box_to(&doc, &res, group, Size::new(300.0, 150.0)))
            .unwrap();
        let after = ondin_core::local_box(&doc, &Resolved::rebuild(&doc), group).unwrap();
        assert!(
            (after.width() - 300.0).abs() < 1e-6 && (after.height() - 150.0).abs() < 1e-6,
            "asked for 300 × 150 and the group measured {after:?}"
        );
        assert!(
            (after.min_x() - before.min_x()).abs() < 1e-6
                && (after.min_y() - before.min_y()).abs() < 1e-6,
            "the group's top-left moved from {:?} to {:?}",
            before.origin(),
            after.origin()
        );
    }

    /// **Dragging an auto-sized label's left edge widens it leftward, from the
    /// ink.** The reported bug had it widen *rightward from the origin*: the
    /// handles are drawn on the measured box (`query::local_box`) while
    /// `resizable_size` reports `Size::ZERO`, so the "opposite edge" the gesture
    /// held still was the local origin — which is where the *left* handle is, not
    /// the right one. The label jumped left and lost its width in one drag.
    ///
    /// Asserted on the **right edge**, because that is the edge the gesture is
    /// supposed to hold and the only reading that separates the two behaviours:
    /// both produce the same new origin, and the buggy one puts the far edge back
    /// at the origin the ink used to start at.
    #[test]
    fn dragging_an_auto_labels_left_edge_holds_its_right_edge() {
        let (mut doc, res, _group, text) = group_with_text(TextSizing::Auto);
        // The text sits at (10,10) in the group, which sits at the root: so the
        // ink starts at world x=10 and the measured box gives the far edge.
        let measured = ondin_core::local_box(&doc, &res, text).unwrap();
        let right = 10.0 + measured.width();
        let tx = resize_to_handle(
            &doc,
            &res,
            text,
            Handle::Left,
            Point::new(-30.0, 10.0),
            Resize::geometry(false, false),
        );
        doc.apply(&tx).unwrap();

        let sizing = sizing_of(&doc, text);
        let wanted = right + 30.0;
        assert!(
            matches!(sizing, TextSizing::AutoHeight(w) if (w - wanted).abs() < 0.5),
            "expected auto-height at {wanted} (the box from -30 to {right}), got {sizing:?}"
        );
        let res = Resolved::rebuild(&doc);
        let world = res.world_transform(text).unwrap();
        let origin = world * Point::ZERO;
        assert!(
            (origin.x - -30.0).abs() < 1e-9,
            "the dragged edge is not under the pointer: {origin:?}"
        );
        let far = world * Point::new(wanted, 0.0);
        assert!(
            (far.x - right).abs() < 0.5,
            "the held edge moved from {right} to {}",
            far.x
        );
    }

    /// **A trimmed auto box does not start at the local origin, and the gesture
    /// has to measure from where the handles are.** `text::box_of` gives an auto
    /// node `(0, trim_top)` so that tightening the box moves no ink (§15 D78), so
    /// its drawn top is *below* the origin `resized_box` measures from — and a top
    /// drag therefore held a bottom edge that was `trim_top` above the one on
    /// screen, pulling the box up by that much.
    ///
    /// Asserted on the **bottom** edge again, for the reason D158 gives: it is the
    /// edge the gesture promises to hold, and the wrong answer moves it.
    #[test]
    fn dragging_the_top_of_a_trimmed_label_holds_the_bottom_the_handles_drew() {
        let (mut doc, _res, _group, text) = group_with_text(TextSizing::Auto);
        doc.apply(&Transaction(vec![Operation::SetBlockStyle {
            id: text,
            block: ondin_core::BlockStyle {
                trim: ondin_core::BoxTrim::CapToBaseline,
                ..Default::default()
            },
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let drawn = ondin_core::local_box(&doc, &res, text).unwrap();
        assert!(
            drawn.min_y() > 0.1,
            "cap-to-baseline trim must inset the box from the origin: {drawn:?}"
        );
        // The text sits at (10,10) in its group, so the drawn box in world is that
        // plus its own local box.
        let bottom = 10.0 + drawn.max_y();

        let tx = resize_to_handle(
            &doc,
            &res,
            text,
            Handle::Top,
            Point::new(10.0, -20.0),
            Resize::geometry(false, false),
        );
        doc.apply(&tx).unwrap();

        let TextSizing::Fixed(size) = sizing_of(&doc, text) else {
            panic!("a top drag authors the height");
        };
        let res = Resolved::rebuild(&doc);
        let after = ondin_core::local_box(&doc, &res, text).unwrap();
        let world = res.world_transform(text).unwrap();
        let far = world * Point::new(0.0, after.max_y());
        assert!(
            (far.y - bottom).abs() < 0.5,
            "the held edge moved from {bottom} to {} (box {size:?}, drawn {after:?})",
            far.y
        );
        let top = world * Point::new(0.0, after.min_y());
        assert!(
            (top.y - -20.0).abs() < 0.5,
            "and the dragged edge must be under the pointer, not at {}",
            top.y
        );
    }

    #[test]
    fn resize_from_bottom_right_only_changes_size() {
        let (mut doc, res, rect) = artboard_with_rect();
        // Rect world origin is (100,100); drag its BR corner to (260,180).
        // Anchor is the top-left, so the origin must not move.
        let tx = resize_to_handle(
            &doc,
            &res,
            rect,
            Handle::BottomRight,
            Point::new(260.0, 180.0),
            Resize::geometry(false, false),
        );
        doc.apply(&tx).unwrap();
        let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
            panic!();
        };
        assert!((size.width - 160.0).abs() < 1e-9);
        assert!((size.height - 80.0).abs() < 1e-9);
        let origin = Resolved::rebuild(&doc).world_transform(rect).unwrap() * Point::ZERO;
        assert!((origin.x - 100.0).abs() < 1e-9 && (origin.y - 100.0).abs() < 1e-9);
    }

    #[test]
    fn resize_from_top_left_moves_the_origin_and_holds_the_far_corner() {
        let (mut doc, res, rect) = artboard_with_rect();
        // Rect spans world (100,100)..(150,150). Drag the TL corner to (120,110):
        // the BR corner must stay at (150,150) and the box become 30x40.
        let tx = resize_to_handle(
            &doc,
            &res,
            rect,
            Handle::TopLeft,
            Point::new(120.0, 110.0),
            Resize::geometry(false, false),
        );
        doc.apply(&tx).unwrap();
        let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
            panic!();
        };
        assert!((size.width - 30.0).abs() < 1e-9, "w={}", size.width);
        assert!((size.height - 40.0).abs() < 1e-9, "h={}", size.height);

        let res = Resolved::rebuild(&doc);
        let w = res.world_transform(rect).unwrap();
        let origin = w * Point::ZERO;
        let far = w * Point::new(size.width, size.height);
        assert!((origin.x - 120.0).abs() < 1e-9 && (origin.y - 110.0).abs() < 1e-9);
        assert!(
            (far.x - 150.0).abs() < 1e-9 && (far.y - 150.0).abs() < 1e-9,
            "anchor corner moved: {far:?}"
        );
    }

    /// **Every kind flips, and the two resize paths agree about it** — §15 D753,
    /// `[S13.1-L2-02]`, on the maintainer's ruling that a `Text` node mirrors too
    /// because it *"keeps the action consistent"*.
    ///
    /// 🚨 **The box was never the bug.** A `Rect` dragged past its anchor lands in
    /// exactly the same world box as a `Path` — measured to the unit, both rows
    /// below — and the difference is entirely in the basis: `Mirror::X` against
    /// `Mirror::None`. So a rect with one rounded corner carried it to the far side
    /// *unturned*, where the same drag on a path put it against the held edge.
    /// `resized_box` takes `(at - held).abs()` and reflects the box across the
    /// anchor, so no sign survived to become a flip; every other kind goes through
    /// `resize_factors`, where `scale_along` keeps the sign on purpose.
    ///
    /// ⚠️ **Alt is the same rule, and this is the assertion that says so.** The
    /// anchor is the box's own centre rather than an edge, and a `Path` dragged
    /// past *that* comes back `Mirror::X` too — measured before the fix was
    /// written, which is what stopped it being scoped to side handles alone. A
    /// version that flipped only the anchored case passes the first half of this
    /// and fails the second.
    ///
    /// ⚠️ **`Mirror` and not the ink's bounding box**, because the box is
    /// identical either way — that is the whole finding. An assertion on where the
    /// ink lands is green against the unfixed code, which is what
    /// `dragging_a_handle_past_the_opposite_corner_flips_a_path` below asserts and
    /// why that test did not catch this.
    ///
    /// ⚠️ **Flip, run — twice, and the second result was much better than
    /// predicted.** Returning `false` from `resized_box`'s `crossed` closure fails
    /// at the rect's `Mirror::X` assertion (`left: None, right: X`) and takes only
    /// this test with it, the path rows staying green because a path never reaches
    /// that code.
    ///
    /// 🚨 **Spelling `crossed` as `at < anchor` — the obvious wrong version, and
    /// what the `edge` closure beside it uses for a *different* question — fails
    /// SIX tests, four of them older than this change**:
    /// `an_ordinary_resize_flips_nothing`,
    /// `resize_from_top_left_moves_the_origin_and_holds_the_far_corner`,
    /// `dragging_an_auto_labels_left_edge_holds_its_right_edge`,
    /// `dragging_the_top_of_a_trimmed_label_holds_the_bottom_the_handles_drew` and
    /// `a_crop_drag_leaves_the_photograph_exactly_where_it_was`, besides this one.
    /// Every one of them drives a `TopLeft`, `Left` or `Top` handle, where `at <
    /// anchor` is the *ordinary* drag — so that spelling flips on almost every
    /// resize the app makes. **The suite had real teeth against the loud mistake
    /// and none at all against the quiet one**, which is the shape worth carrying:
    /// a missing flip is invisible and a spurious flip is caught by everything.
    #[test]
    fn a_rect_flips_past_its_anchor_exactly_as_a_path_does() {
        use ondin_core::build::{Basis, Mirror};

        let rect = || NodeKind::Rect {
            size: Size::new(100.0, 100.0),
            corner_radii: ondin_core::kurbo::RoundedRectRadii::new(16.0, 0.0, 0.0, 0.0),
        };
        let path = || {
            let mut pen = BezPath::new();
            pen.move_to((0.0, 0.0));
            pen.line_to((100.0, 0.0));
            pen.line_to((100.0, 100.0));
            pen.line_to((0.0, 100.0));
            pen.close_path();
            NodeKind::Path {
                path: pen,
                corner_radii: Vec::new(),
            }
        };

        // `(what, handle, pointer, symmetric)` — each row is one drag, run against
        // both kinds, and the two must agree.
        let cases: [(&str, Handle, Point, bool); 3] = [
            (
                "the right handle dragged past the left edge",
                Handle::Right,
                Point::new(-50.0, 50.0),
                false,
            ),
            (
                "the right handle dragged past the centre with Alt",
                Handle::Right,
                Point::new(20.0, 50.0),
                true,
            ),
            // **The mirror image, and it is the case the obvious wrong predicate
            // gets backwards.** `Handle::Left` holds the *right* edge, so an
            // ordinary left drag already satisfies `at < held`; only a drag past
            // 100 is a crossing here.
            (
                "the left handle dragged past the right edge",
                Handle::Left,
                Point::new(150.0, 50.0),
                false,
            ),
        ];

        for (what, handle, at, symmetric) in cases {
            let mut seen: Vec<(Mirror, Rect)> = Vec::new();
            for (label, kind) in [("rect", rect()), ("path", path())] {
                let mut ids = IdSource::new(0x1F3);
                let root = ids.mint();
                let mut doc = Document::new(root);
                let id = ids.mint();
                doc.apply(&Transaction(vec![Operation::CreateNode {
                    id,
                    parent: root,
                    index: 0,
                    kind,
                    transform: None,
                    name: None,
                }]))
                .expect("a node");
                let res = Resolved::rebuild(&doc);
                let tx = resize_layer(
                    &doc,
                    &res,
                    id,
                    handle,
                    at,
                    Resize::geometry(false, symmetric),
                );
                let mut after = doc.clone();
                after.apply(&tx).expect("applies");
                let ra = Resolved::rebuild(&after);
                let world = ra.world_transform(id).expect("a transform");
                let local = ondin_core::local_box(&after, &ra, id).expect("a box");
                let ink: BezPath = ondin_core::kurbo::Shape::to_path(&local, 0.01);
                let world_box = ondin_core::kurbo::Shape::bounding_box(&(world * ink));
                let mirror = Basis::of(world).orientation.mirror;
                assert_eq!(
                    mirror,
                    Mirror::X,
                    "{what}: the {label} is turned over — §5.6's flip, unqualified"
                );
                seen.push((mirror, world_box));
            }
            // **The two kinds agreed about the box before this change and that is
            // the control**: without it the assertions above would pass for a rect
            // that flipped *and* landed somewhere the path does not.
            let (_, rect_box) = seen[0];
            let (_, path_box) = seen[1];
            assert!(
                (rect_box.x0 - path_box.x0).abs() < 1e-6
                    && (rect_box.x1 - path_box.x1).abs() < 1e-6
                    && (rect_box.y0 - path_box.y0).abs() < 1e-6
                    && (rect_box.y1 - path_box.y1).abs() < 1e-6,
                "{what}: the two kinds land in the same world box — \
                 rect {rect_box:?} against path {path_box:?}"
            );
        }
    }

    /// **A drag that stops short of the anchor does not flip anything**, which is
    /// the control for the test above (§15 D753).
    ///
    /// 🚨 **Without this, `crossed` returning `true` unconditionally passes every
    /// assertion up there.** That is not a hypothetical spelling: the predicate is
    /// a sign comparison, and dropping the `< 0.0` for a `!= 0.0` — or comparing
    /// against the wrong end of the box — turns an ordinary resize into a flip.
    ///
    /// ⚠️ **The `Left` rows are the ones that earn their place**, and measured
    /// rather than reasoned: spelling `crossed` as `at < anchor` fails this test at
    /// *"the left handle pulled in"* and four older tests besides, because
    /// `Handle::Left` holds the **right** edge and every ordinary left drag
    /// satisfies that predicate. The `Right` rows are green under the same
    /// mutation.
    #[test]
    fn an_ordinary_resize_flips_nothing() {
        use ondin_core::build::{Basis, Mirror};

        for (what, handle, at) in [
            ("the right handle pulled out", Handle::Right, 150.0),
            (
                "the right handle pulled in, short of the edge",
                Handle::Right,
                30.0,
            ),
            ("the left handle pulled out", Handle::Left, -50.0),
            (
                "the left handle pulled in, short of the edge",
                Handle::Left,
                70.0,
            ),
        ] {
            let mut ids = IdSource::new(0x1F4);
            let root = ids.mint();
            let mut doc = Document::new(root);
            let id = ids.mint();
            doc.apply(&Transaction(vec![Operation::CreateNode {
                id,
                parent: root,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            }]))
            .expect("a rect");
            let res = Resolved::rebuild(&doc);
            let tx = resize_layer(
                &doc,
                &res,
                id,
                handle,
                Point::new(at, 50.0),
                Resize::geometry(false, false),
            );
            let mut after = doc.clone();
            after.apply(&tx).expect("applies");
            let ra = Resolved::rebuild(&after);
            let world = ra.world_transform(id).expect("a transform");
            assert_eq!(
                Basis::of(world).orientation.mirror,
                Mirror::None,
                "{what}: nothing was dragged past anything"
            );
        }
    }

    /// **Dragging a handle past the one opposite flips the shape**, for a path and a group
    /// as well as for a rect.
    ///
    /// Reported for booleans and paths: "bring it diagonally within the shape to the
    /// opposite handle… then continue dragging diagonally, you expect for the shape to flip
    /// and resize on the other side. With boolean groups and paths, it doesn't. Instead it
    /// keeps resizing on the inverse axis." Every caller of `resize_factors` had it — a
    /// group too — because `scale_along` clamped its factor to `MIN_EXTENT..MAX_SCALE`, and
    /// a negative factor *is* the flip. Only rects and their relatives escaped, since
    /// `resized_box` reflects the box across the anchor instead of scaling it.
    ///
    /// Asserted on where the ink ends up in world space, which is what the eye reads: drag
    /// the bottom-right corner of a 0..20 box to −20, and the shape should occupy −20..0
    /// rather than collapse at 0.
    #[test]
    fn dragging_a_handle_past_the_opposite_corner_flips_a_path() {
        let mut ids = IdSource::new(0x1F1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let path = ids.mint();
        let mut pen = BezPath::new();
        pen.move_to((0.0, 0.0));
        pen.line_to((20.0, 0.0));
        pen.line_to((20.0, 20.0));
        pen.close_path();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: path,
            parent: root,
            index: 0,
            kind: NodeKind::Path {
                path: pen,
                corner_radii: Vec::new(),
            },
            transform: None,
            name: None,
        }]))
        .expect("a path");
        let res = Resolved::rebuild(&doc);
        let local = ondin_core::local_box(&doc, &res, path).expect("a box");

        // Bottom-right dragged well past the top-left, which is the corner it holds.
        let tx = resize_geometry(
            &doc,
            &res,
            path,
            local,
            Handle::BottomRight,
            Point::new(-20.0, -20.0),
            Resize::default(),
        );
        let mut after = doc.clone();
        after.apply(&tx).expect("applies");
        let res_after = Resolved::rebuild(&after);
        let world = res_after.world_transform(path).unwrap();
        let NodeKind::Path { path: shape, .. } = after.get(path).unwrap().kind() else {
            panic!("still a path")
        };
        let now = ondin_core::kurbo::Shape::bounding_box(&(world * shape.clone()));

        assert!(
            (now.x1 - 0.0).abs() < 1e-6 && (now.y1 - 0.0).abs() < 1e-6,
            "the held corner stayed at the origin: {now:?}"
        );
        assert!(
            (now.x0 + 20.0).abs() < 1e-6 && (now.y0 + 20.0).abs() < 1e-6,
            "and the shape is now on the other side of it, not collapsed: {now:?}"
        );
        // A collapsed shape is what the clamp produced, and it is worth naming
        // separately: the box had a thousandth of a unit of width and grew back on the
        // *same* side as the pointer moved on.
        assert!(
            now.width() > 19.0 && now.height() > 19.0,
            "it kept its size through the flip: {now:?}"
        );
    }

    /// The sign survives the clamp, and the magnitude is still clamped at both ends.
    #[test]
    fn a_scale_factor_keeps_its_sign_and_clamps_its_magnitude() {
        assert_eq!(scale_along(10.0, 20.0), 2.0);
        assert_eq!(
            scale_along(10.0, -20.0),
            -2.0,
            "a mirror is a negative factor"
        );
        assert_eq!(
            scale_along(10.0, 0.0),
            MIN_EXTENT,
            "and zero is still floored"
        );
        assert_eq!(
            scale_along(1.0, -1e9),
            -MAX_SCALE,
            "the cap applies to the magnitude, not to the value"
        );
        assert_eq!(scale_along(0.0, 5.0), 1.0, "no extent to have scaled");
    }
    /// **Dragging a line's far end moves only that end.**
    #[test]
    fn dragging_a_lines_end_moves_only_that_end() {
        let (doc, res, line) = line_fixture(Affine::translate((40.0, 10.0)));
        let (start_before, _) = line_ends_world(&doc, &res, line).unwrap();

        let tx = move_line_end(&doc, &res, line, LineEnd::End, Point::new(200.0, 90.0));
        let mut after = doc.clone();
        after.apply(&tx).expect("applies");
        let res_after = Resolved::rebuild(&after);
        let (start, end) = line_ends_world(&after, &res_after, line).unwrap();

        approx_pt(
            end,
            Point::new(200.0, 90.0),
            "the dragged end went to the pointer",
        );
        approx_pt(start, start_before, "and the other end did not move");
    }

    /// **Dragging the *start* holds the far end still**, which is the half that is not
    /// obvious: the start is the node's own origin (§5.3 stores only the far point), so
    /// moving it is a transform edit — and `end`, being measured *from* that origin, has
    /// to be re-expressed or the far end travels along with it.
    #[test]
    fn dragging_a_lines_start_holds_the_far_end_still() {
        let (doc, res, line) = line_fixture(Affine::translate((40.0, 10.0)));
        let (_, end_before) = line_ends_world(&doc, &res, line).unwrap();

        let tx = move_line_end(&doc, &res, line, LineEnd::Start, Point::new(-30.0, 55.0));
        let mut after = doc.clone();
        after.apply(&tx).expect("applies");
        let res_after = Resolved::rebuild(&after);
        let (start, end) = line_ends_world(&after, &res_after, line).unwrap();

        approx_pt(
            start,
            Point::new(-30.0, 55.0),
            "the start went to the pointer",
        );
        approx_pt(
            end,
            end_before,
            "and the far end stayed exactly where it was",
        );
    }

    /// **The basis survives, so a turned line stays turned.** The gesture moves one
    /// point; it does not reset the node. Both ends are checked because a transform edit
    /// that dropped the rotation would still put the *grabbed* point in the right place
    /// and silently straighten everything else — a line has only one other point, and it
    /// is the one that gives the game away.
    #[test]
    fn moving_an_end_of_a_rotated_line_keeps_its_basis() {
        // A quarter turn, so the local x axis runs down the world y axis.
        let turn = Affine::rotate(std::f64::consts::FRAC_PI_2) * Affine::translate((0.0, 0.0));
        let (doc, res, line) = line_fixture(Affine::translate((40.0, 10.0)) * turn);
        let basis_before = doc.get(line).unwrap().transform().as_coeffs();

        for (which, target) in [
            (LineEnd::End, Point::new(90.0, 120.0)),
            (LineEnd::Start, Point::new(-10.0, 5.0)),
        ] {
            let tx = move_line_end(&doc, &res, line, which, target);
            let mut after = doc.clone();
            after.apply(&tx).expect("applies");
            let res_after = Resolved::rebuild(&after);
            let (start, end) = line_ends_world(&after, &res_after, line).unwrap();
            let moved = match which {
                LineEnd::Start => start,
                LineEnd::End => end,
            };
            approx_pt(moved, target, &format!("{which:?} landed on the pointer"));
            let basis = after.get(line).unwrap().transform().as_coeffs();
            for i in 0..4 {
                assert!(
                    (basis[i] - basis_before[i]).abs() < 1e-9,
                    "{which:?}: the basis changed — coefficient {i} went {} → {}",
                    basis_before[i],
                    basis[i]
                );
            }
        }
    }

    /// Root → line at `at`, running 100 along its own x axis.
    fn line_fixture(at: Affine) -> (Document, Resolved, NodeId) {
        let mut ids = IdSource::new(0x11D);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let line = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: line,
            parent: root,
            index: 0,
            kind: NodeKind::Line {
                end: Point::new(100.0, 0.0),
            },
            transform: Some(at),
            name: None,
        }]))
        .expect("a line");
        let res = Resolved::rebuild(&doc);
        (doc, res, line)
    }

    fn approx_pt(got: Point, want: Point, what: &str) {
        assert!(
            (got.x - want.x).abs() < 1e-6 && (got.y - want.y).abs() < 1e-6,
            "{what}: {got:?} vs {want:?}"
        );
    }
    /// **A lone line and a lone pen path scale.** They did not: neither has an
    /// authored `size`, so both fell to `resize_group` → `scale_subtree`, which
    /// iterates *children* — and a line has none, so the transaction came out empty.
    /// The handles were drawn, they armed, they showed the right cursor, and nothing
    /// moved. Asserted through the committed geometry rather than the op list, since
    /// "some operations were produced" was never the question.
    ///
    /// A path's box is deliberately placed away from its origin (x ∈ [10, 30]), which
    /// is the case the anchor arithmetic exists for: `scale_geometry` can only scale
    /// about the origin, so the held edge stays put only if the transform takes the
    /// `anchor·(1 − S)` correction.
    #[test]
    fn a_lone_line_and_path_scale_from_a_handle() {
        let mut ids = IdSource::new(0x11E);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (line, path) = (ids.mint(), ids.mint());
        let mut pen = BezPath::new();
        pen.move_to((10.0, 0.0));
        pen.line_to((30.0, 0.0));
        pen.line_to((30.0, 40.0));
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: line,
                parent: root,
                index: 0,
                kind: NodeKind::Line {
                    end: Point::new(100.0, 0.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: path,
                parent: root,
                index: 1,
                kind: NodeKind::Path {
                    path: pen,
                    corner_radii: Vec::new(),
                },
                transform: None,
                name: None,
            },
        ]))
        .expect("a line and a path");
        let res = Resolved::rebuild(&doc);
        let opts = Resize::default();

        // The line's box is 0..100 wide with no height; drag the right edge to 250.
        let local = ondin_core::local_box(&doc, &res, line).expect("a line has a box");
        let tx = resize_geometry(
            &doc,
            &res,
            line,
            local,
            Handle::Right,
            Point::new(250.0, 0.0),
            opts,
        );
        assert!(!tx.0.is_empty(), "a line's handle drag must do something");
        let mut after = doc.clone();
        after.apply(&tx).expect("applies");
        let NodeKind::Line { end } = *after.get(line).unwrap().kind() else {
            panic!("still a line")
        };
        assert!(
            (end.x - 250.0).abs() < 1e-6 && end.y.abs() < 1e-6,
            "the endpoint follows the handle: {end:?}"
        );

        // The path's box is x ∈ [10, 30]; drag the right edge to 50, so the *left*
        // edge has to stay at 10 while the width doubles.
        let local = ondin_core::local_box(&doc, &res, path).expect("a path has a box");
        assert_eq!(
            (local.x0, local.x1),
            (10.0, 30.0),
            "the box is off the origin"
        );
        let tx = resize_geometry(
            &doc,
            &res,
            path,
            local,
            Handle::Right,
            Point::new(50.0, 20.0),
            opts,
        );
        assert!(!tx.0.is_empty(), "a path's handle drag must do something");
        let mut after = doc.clone();
        after.apply(&tx).expect("applies");
        // Measured in the node's own *world* space, which is what the eye sees: the
        // scale went into the geometry and the correction into the transform, and only
        // the two together put the held edge back.
        let res_after = Resolved::rebuild(&after);
        let world = res_after.world_transform(path).unwrap();
        let NodeKind::Path { path: shape, .. } = after.get(path).unwrap().kind() else {
            panic!("still a path")
        };
        let box_now = ondin_core::kurbo::Shape::bounding_box(&(world * shape.clone()));
        assert!(
            (box_now.x0 - 10.0).abs() < 1e-6,
            "the held left edge stayed at 10: {box_now:?}"
        );
        assert!(
            (box_now.x1 - 50.0).abs() < 1e-6,
            "and the dragged right edge reached 50: {box_now:?}"
        );
        assert!(
            (box_now.y1 - 40.0).abs() < 1e-6,
            "a side drag left the other axis alone: {box_now:?}"
        );
    }

    /// Resizing a group scales what is *inside* it — positions and geometry —
    /// rather than multiplying a scale into the group's own transform. §5.6:
    /// the model has no scale in normal editing, and baking one in would take
    /// stroke widths and text sizes with it.
    #[test]
    fn resizing_a_group_scales_its_children_and_their_spacing() {
        let mut ids = IdSource::new(0x6E);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (ab, g, a, b) = (ids.mint(), ids.mint(), ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 400.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: g,
                parent: ab,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            // Two 20×20 squares at x = 0 and x = 80: the group's box is 100×20.
            Operation::CreateNode {
                id: a,
                parent: g,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 20.0),
                    corner_radii: RoundedRectRadii::default(),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: b,
                parent: g,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 20.0),
                    corner_radii: RoundedRectRadii::default(),
                },
                transform: Some(Affine::translate((80.0, 0.0))),
                name: None,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let local = ondin_core::local_box(&doc, &res, g).unwrap();
        assert_eq!((local.width(), local.height()), (100.0, 20.0));

        // Drag the bottom-right corner to double the width, height unchanged.
        doc.apply(&resize_group(
            &doc,
            &res,
            g,
            local,
            Handle::BottomRight,
            Point::new(200.0, 20.0),
            Resize::geometry(false, false),
        ))
        .unwrap();

        let res = Resolved::rebuild(&doc);
        for (id, want_x, want_w) in [(a, 0.0, 40.0), (b, 160.0, 40.0)] {
            let NodeKind::Rect { size, .. } = doc.get(id).unwrap().kind() else {
                panic!();
            };
            assert!(
                (size.width - want_w).abs() < 1e-9 && (size.height - 20.0).abs() < 1e-9,
                "child geometry {size:?}, want {want_w}x20"
            );
            let origin = res.world_transform(id).unwrap() * Point::ZERO;
            assert!(
                (origin.x - want_x).abs() < 1e-9,
                "child at {origin:?}, want x {want_x}"
            );
        }
        // And the group's box followed.
        let after = ondin_core::local_box(&doc, &res, g).unwrap();
        assert!((after.width() - 200.0).abs() < 1e-9, "{after:?}");
    }

    /// A group's transform must come out untouched: the scale went into the
    /// children, so the group itself is still translation-and-rotation only
    /// (§5.6). A scale left here would multiply into every future edit.
    #[test]
    fn resizing_a_group_leaves_the_groups_own_transform_alone() {
        let mut ids = IdSource::new(0x77);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (ab, g, a) = (ids.mint(), ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 400.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: g,
                parent: ab,
                index: 0,
                kind: NodeKind::Group,
                transform: Some(Affine::translate((30.0, 40.0))),
                name: None,
            },
            Operation::CreateNode {
                id: a,
                parent: g,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(50.0, 50.0),
                    corner_radii: RoundedRectRadii::default(),
                },
                transform: None,
                name: None,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let before = doc.get(g).unwrap().transform().as_coeffs();
        let local = ondin_core::local_box(&doc, &res, g).unwrap();

        doc.apply(&resize_group(
            &doc,
            &res,
            g,
            local,
            Handle::BottomRight,
            Point::new(130.0, 140.0), // world → 100×100 in group space
            Resize::geometry(false, false),
        ))
        .unwrap();

        assert_eq!(doc.get(g).unwrap().transform().as_coeffs(), before);
        let NodeKind::Rect { size, .. } = doc.get(a).unwrap().kind() else {
            panic!();
        };
        assert!((size.width - 100.0).abs() < 1e-9, "{size:?}");
    }

    /// Dragging a **side** moves one axis and leaves the other exactly alone —
    /// including the origin on that axis, which is what stops a width drag
    /// from also nudging the shape up or down.
    #[test]
    fn a_side_handle_resizes_one_axis_and_leaves_the_other_untouched() {
        let size = Size::new(100.0, 60.0);
        for (handle, pointer, want_origin, want_size) in [
            // Right edge out to x = 160: width follows, height and y do not.
            (
                Handle::Right,
                Point::new(160.0, 12.0),
                Point::new(0.0, 0.0),
                Size::new(160.0, 60.0),
            ),
            // Left edge in to x = 30: the right edge stays at 100.
            (
                Handle::Left,
                Point::new(30.0, 12.0),
                Point::new(30.0, 0.0),
                Size::new(70.0, 60.0),
            ),
            // Bottom edge down to y = 90.
            (
                Handle::Bottom,
                Point::new(-40.0, 90.0),
                Point::new(0.0, 0.0),
                Size::new(100.0, 90.0),
            ),
            // Top edge down to y = 20: the bottom edge stays at 60.
            (
                Handle::Top,
                Point::new(-40.0, 20.0),
                Point::new(0.0, 20.0),
                Size::new(100.0, 40.0),
            ),
        ] {
            let (origin, got, _) = resized_box(size, handle, pointer, false, false);
            assert_eq!(got, want_size, "{handle:?} size");
            assert_eq!(origin, want_origin, "{handle:?} origin");
        }
    }

    /// A corner still drives both axes, and holds the opposite corner.
    #[test]
    fn a_corner_handle_still_drives_both_axes() {
        let size = Size::new(100.0, 60.0);
        let (origin, got, _) =
            resized_box(size, Handle::TopLeft, Point::new(20.0, 10.0), false, false);
        assert_eq!(got, Size::new(80.0, 50.0));
        assert_eq!(origin, Point::new(20.0, 10.0));
    }

    /// With Shift, a side drag carries the other axis with it rather than
    /// pinning it — the ratio is the point of the modifier.
    #[test]
    fn shift_on_a_side_scales_the_other_axis_too() {
        // 100x50 is a 2:1 box; dragging the right edge to 200 doubles it.
        let (_, got, _) = resized_box(
            Size::new(100.0, 50.0),
            Handle::Right,
            Point::new(200.0, 0.0),
            true,
            false,
        );
        assert_eq!(got, Size::new(200.0, 100.0));
    }

    /// **A moved pivot must not change what Alt-resize does** (§15 D60).
    ///
    /// It briefly did, and the result was unusable: holding the pivot fixed *and*
    /// having the dragged edge track the pointer forces the scale factor to
    /// `|pointer − pivot| / |draggedEdge − pivot|`, whose denominator vanishes as
    /// the pivot approaches the edge you are dragging. A pivot near a corner made
    /// the far side fly out at a huge multiple of the pointer's movement, and a
    /// pivot exactly on the corner made the gesture do nothing at all.
    ///
    /// Written against the whole transaction rather than `resized_box`, because the
    /// point is that no *caller* consults the pivot either.
    #[test]
    fn a_moved_pivot_does_not_change_an_alt_resize() {
        use ondin_core::Pivot;
        use ondin_core::kurbo::Vec2;

        let alt_resize = |doc: &Document, res: &Resolved, id: NodeId| {
            resize_to_handle(
                doc,
                res,
                id,
                Handle::Right,
                Point::new(85.0, 30.0),
                Resize::geometry(false, true),
            )
        };

        let (doc, res, rect) = artboard_with_rect();
        let plain = alt_resize(&doc, &res, rect);

        // Every placement that used to break it: a corner, and a whisker off one.
        for pivot in [
            Pivot::Normalized(Vec2::new(1.0, 1.0)),
            Pivot::Normalized(Vec2::new(0.98, 0.5)),
            Pivot::Normalized(Vec2::new(0.0, 0.0)),
            Pivot::Local(Point::new(3.0, 3.0)),
        ] {
            let (mut doc, res, rect) = artboard_with_rect();
            doc.apply(&Transaction(vec![Operation::SetPivot {
                id: rect,
                pivot: Some(pivot),
            }]))
            .expect("set pivot");
            assert_eq!(
                alt_resize(&doc, &res, rect),
                plain,
                "{pivot:?} changed the resize"
            );
        }
    }

    /// Alt holds the **centre** still: dragging one side moves the opposite one
    /// out to match, so the box grows twice as fast and stays centred where it
    /// was. The whole point of it is that the far edge does *not* stay put, which
    /// is the one thing an ordinary resize guarantees.
    #[test]
    fn alt_resizes_about_the_centre() {
        let size = Size::new(100.0, 50.0);
        // Right side dragged to x = 80. Centre is 50, so the half-width becomes
        // 30 and the box is 60 wide, laid out from 20 to 80.
        let (origin, got, _) = resized_box(size, Handle::Right, Point::new(80.0, 0.0), false, true);
        assert_eq!(got, Size::new(60.0, 50.0));
        assert_eq!(origin, Point::new(20.0, 0.0));
        // The untouched axis is untouched, origin included.
        assert_eq!(origin.y, 0.0);

        // Without Alt the same drag holds the left edge and gives 80 wide.
        let (plain_origin, plain, _) =
            resized_box(size, Handle::Right, Point::new(80.0, 0.0), false, false);
        assert_eq!(plain, Size::new(80.0, 50.0));
        assert_eq!(plain_origin, Point::new(0.0, 0.0));

        // A corner does both axes at once, still about the centre.
        let (o, s, _) = resized_box(
            size,
            Handle::BottomRight,
            Point::new(80.0, 40.0),
            false,
            true,
        );
        assert_eq!(s, Size::new(60.0, 30.0));
        assert_eq!(o, Point::new(20.0, 10.0));

        // Dragging *past* the centre is symmetric too — the box shrinks through
        // zero and grows again rather than flipping about a far edge.
        //
        // ⚠️ **"Rather than flipping" is about the *box*, and since §15 D753 the
        // shape does flip.** The third return says so — this is the one drag in
        // this test that crosses its anchor, and it comes back `(true, false)` —
        // and `resize_to_handle` turns that into a reflection in the transform.
        // The box is unchanged either way, which is exactly why the mirror had to
        // be reported separately rather than read off these two assertions.
        let (o, s, crossed) = resized_box(size, Handle::Right, Point::new(20.0, 0.0), false, true);
        assert_eq!(s, Size::new(60.0, 50.0));
        assert_eq!(o, Point::new(20.0, 0.0));
        assert_eq!(
            crossed,
            (true, false),
            "the pointer went past the centre on x and nowhere near it on y"
        );
    }
    /// Alt and Shift compose: proportional *and* centred, which is what every
    /// other editor does with the pair.
    #[test]
    fn alt_and_shift_resize_proportionally_about_the_centre() {
        let size = Size::new(100.0, 50.0);
        // Right side to x = 80 → 60 wide; the 2:1 ratio makes it 30 tall, and
        // both are centred on (50, 25).
        let (origin, got, _) = resized_box(size, Handle::Right, Point::new(80.0, 0.0), true, true);
        assert_eq!(got, Size::new(60.0, 30.0));
        assert_eq!(origin, Point::new(20.0, 10.0));
    }

    /// The same gesture through the *other* implementation of the anchor rules,
    /// on the same numbers — §15 D559.
    ///
    /// `resized_box` answers `(20, 10) – (80, 40)` for Alt+Shift on the right
    /// side of a 100 × 50 box, and the test above says so. `resize_factors` is
    /// the copy a `Path`, a group and a multi-selection go through, and it used
    /// to answer `(20, 0) – (80, 30)`: the ratio lock drives the axis the side
    /// handle does *not* own, and the pin was chosen from whether the handle owns
    /// the axis rather than from whether the axis ended up scaled, so the
    /// following axis was scaled about `box_.min_y()` while the dragged one was
    /// scaled about the centre. Ten units up, from the same modifiers.
    ///
    /// **The flip is `pin`'s guard back to `Some(_) if symmetric`**, which is the
    /// shape the bug had: the width assertion stays green and `y0`/`y1` come back
    /// `0` and `30`. Which is `CLAUDE.md`'s "predicted failure site" note again —
    /// the interesting half of this test is the *y* pair, not the box.
    #[test]
    fn alt_and_shift_on_a_side_handle_centre_the_followed_axis_for_a_path_too() {
        let mut ids = IdSource::new(0x5B1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let path = ids.mint();
        let mut pen = BezPath::new();
        pen.move_to((0.0, 0.0));
        pen.line_to((100.0, 0.0));
        pen.line_to((100.0, 50.0));
        pen.line_to((0.0, 50.0));
        pen.close_path();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: path,
            parent: root,
            index: 0,
            kind: NodeKind::Path {
                path: pen,
                corner_radii: Vec::new(),
            },
            transform: None,
            name: None,
        }]))
        .expect("a path");
        let res = Resolved::rebuild(&doc);

        // The fixture is the box the assertion is about, not a box the assertion
        // hopes for: 100 × 50 at the origin, which is `resized_box`'s `size`.
        let before = ondin_core::local_box(&doc, &res, path).expect("a box");
        assert_eq!(before, Rect::new(0.0, 0.0, 100.0, 50.0));

        let tx = resize_layer(
            &doc,
            &res,
            path,
            Handle::Right,
            Point::new(80.0, 0.0),
            Resize::geometry(true, true),
        );
        let mut after = doc.clone();
        after.apply(&tx).expect("applies");
        let res_after = Resolved::rebuild(&after);
        let world = res_after.world_transform(path).unwrap();
        let NodeKind::Path { path: shape, .. } = after.get(path).unwrap().kind() else {
            panic!("still a path")
        };
        let now = ondin_core::kurbo::Shape::bounding_box(&(world * shape.clone()));

        assert!(
            (now.x0 - 20.0).abs() < 1e-9 && (now.x1 - 80.0).abs() < 1e-9,
            "the dragged axis is centred, as it always was: {now:?}"
        );
        assert!(
            (now.y0 - 10.0).abs() < 1e-9 && (now.y1 - 40.0).abs() < 1e-9,
            "and so is the axis the ratio lock drove: {now:?}"
        );
    }

    #[test]
    fn resize_never_goes_below_one_unit() {
        let (mut doc, res, rect) = artboard_with_rect();
        // Drag the BR handle past the anchor corner.
        let tx = resize_to_handle(
            &doc,
            &res,
            rect,
            Handle::BottomRight,
            Point::new(100.0, 100.0),
            Resize::geometry(false, false),
        );
        doc.apply(&tx).unwrap();
        let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
            panic!();
        };
        assert!(size.width >= 1.0 && size.height >= 1.0);
    }

    #[test]
    fn shift_resize_keeps_the_aspect_ratio() {
        let (mut doc, res, rect) = artboard_with_rect(); // 50x50, ratio 1
        let tx = resize_to_handle(
            &doc,
            &res,
            rect,
            Handle::BottomRight,
            Point::new(300.0, 160.0),
            Resize::geometry(true, false),
        );
        doc.apply(&tx).unwrap();
        let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
            panic!();
        };
        assert!(
            (size.width - size.height).abs() < 1e-9,
            "expected a square, got {size:?}"
        );
    }

    /// A 400×400 artboard with one text node in `sizing` at its origin.
    fn artboard_with_text(sizing: TextSizing) -> (Document, Resolved, NodeId) {
        let mut ids = IdSource::new(9);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        let t = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 400.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: t,
                parent: ab,
                index: 0,
                kind: NodeKind::Text {
                    content: "wrap me".into(),
                    style: Box::new(ondin_core::TextStyle {
                        font_family: "Inter".into(),
                        font_size: 16.0,
                        weight: 400,
                        italic: false,
                        line_height: Some(ondin_core::Length::Em(1.2)),
                        ..Default::default()
                    }),
                    spans: Default::default(),
                    para_spans: Default::default(),
                    paragraph: Default::default(),
                    block: Default::default(),
                    sizing,
                    on_path: None,
                    on_path_flip: false,
                    on_path_offset: 0.0,
                },
                transform: None,
                name: None,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, t)
    }

    fn sizing_of(doc: &Document, id: NodeId) -> TextSizing {
        let NodeKind::Text { sizing, .. } = doc.get(id).unwrap().kind() else {
            panic!("not text");
        };
        *sizing
    }

    /// Dragging `handle` of the text node to `p`, and what its sizing became.
    fn drag_text(sizing: TextSizing, handle: Handle, p: Point) -> TextSizing {
        let (mut doc, res, t) = artboard_with_text(sizing);
        let tx = resize_to_handle(&doc, &res, t, handle, p, Resize::geometry(false, false));
        doc.apply(&tx).unwrap();
        sizing_of(&doc, t)
    }

    #[test]
    fn resizing_auto_text_from_a_corner_converts_it_to_a_fixed_box() {
        let sizing = drag_text(
            TextSizing::Auto,
            Handle::BottomRight,
            Point::new(80.0, 40.0),
        );
        let TextSizing::Fixed(size) = sizing else {
            panic!("expected a fixed box, got {sizing:?}");
        };
        assert!((size.width - 80.0).abs() < 1e-9 && (size.height - 40.0).abs() < 1e-9);
    }

    /// **A side handle gives auto *height*, not a fixed box.**
    ///
    /// A drag that owns only `x` has said nothing about the height, so the
    /// content keeps deciding it — dragging the right edge of an auto-width node
    /// is the gesture that gives it a wrap width, which is what Figma does and
    /// what nothing here used to reach: every resize produced `Fixed`, so the
    /// mode the three-way control offers was unreachable by dragging.
    ///
    /// **Each handle is dragged *outward*, and the widths differ because the
    /// anchors do.** The right edge holds the origin, so its width is the pointer;
    /// the left edge holds the far side of the *measured* box, so its width is the
    /// ink plus the travel — see `anchored_box`, which is the function that
    /// decides it. ⚠️ **This was an intra-doc link to a name nothing in the
    /// workspace has ever declared** — `anchored_box` with the wrong second
    /// half — and no gate could say so, because the link was
    /// inside a `#[cfg(test)]` module, which `cargo doc` builds without
    /// (§15 D319). Plain backticks now, which is the convention that case asks
    /// for. This case asserted 80 for both
    /// until 2026-08-04, which was the bug: with a zero-sized anchor box the left
    /// edge held the origin too.
    #[test]
    fn dragging_a_side_of_auto_text_gives_auto_height() {
        let (doc, res, t) = artboard_with_text(TextSizing::Auto);
        let ink = ondin_core::local_box(&doc, &res, t).unwrap().width();
        for (handle, p, wanted) in [
            (Handle::Right, Point::new(80.0, 40.0), 80.0),
            (Handle::Left, Point::new(-30.0, 40.0), ink + 30.0),
        ] {
            let sizing = drag_text(TextSizing::Auto, handle, p);
            assert!(
                matches!(sizing, TextSizing::AutoHeight(w) if (w - wanted).abs() < 1e-9),
                "{handle:?} to {p:?} gave {sizing:?}, wanted auto-height at {wanted}"
            );
        }
    }

    /// The same drag on a node that is *already* auto-height keeps it there — and
    /// keeps it visible. `resizable_size` reports `(w, 0)` for auto-height, and a
    /// side drag leaves an untouched axis at its current extent, so routing this
    /// through `Fixed` handed the clamp a zero height and produced a **1px-high**
    /// box: the layer's text vanished on a gesture that only widened it.
    #[test]
    fn dragging_a_side_of_auto_height_text_keeps_the_mode() {
        let sizing = drag_text(
            TextSizing::AutoHeight(50.0),
            Handle::Right,
            Point::new(120.0, 40.0),
        );
        assert!(
            matches!(sizing, TextSizing::AutoHeight(w) if (w - 120.0).abs() < 1e-9),
            "expected auto-height at 120, got {sizing:?}"
        );
    }

    /// The height becoming the user's is what makes a box fixed, so the *vertical*
    /// handles convert an auto mode even though the sides do not.
    #[test]
    fn dragging_the_bottom_of_auto_height_text_fixes_the_box() {
        let sizing = drag_text(
            TextSizing::AutoHeight(50.0),
            Handle::Bottom,
            Point::new(0.0, 90.0),
        );
        assert!(
            matches!(sizing, TextSizing::Fixed(s) if (s.height - 90.0).abs() < 1e-9),
            "expected a 90-high fixed box, got {sizing:?}"
        );
    }

    /// **Dragging a handle on railed text moves the ink** (§15 D405).
    ///
    /// ⚠️ **The failure this exists for is a *dead control*, which no assertion
    /// about sizing would catch.** On a rail the three `TextSizing` modes are
    /// inert, so the arm that writes one produces a patch the layout ignores: the
    /// handle drags, the transaction commits, undo grows a step, and nothing on
    /// screen moves. Every test above it stays green, because they all assert on
    /// `sizing` — which is exactly the value that stopped meaning anything.
    ///
    /// So this asserts on the **box**, end to end: apply the transaction, rebuild,
    /// and ask what the layer's extent became. Dragging the bottom-right corner to
    /// twice the width has to roughly double it.
    ///
    /// ⚠️ **And the anchored corner must not move**, which is the half that
    /// catches the double-move: the rail arm writes the gesture's translation into
    /// the geometry, so the `SetTransform` shift that every other kind needs would
    /// apply it a *second* time here. With the shift left in, the top-left runs
    /// away from the corner the drag was supposed to be holding still.
    ///
    /// ⚠️ **The rail does *not* take the factor the boxes are in ratio by, and a
    /// test asserting that it does was the first draft here.** A railed node's box
    /// is the ribbon the rail sweeps — the curve plus a unit normal at every point
    /// — so it is affine in the scale rather than linear, and a non-uniform scale
    /// turns the tangents too. On this fixture the naive factor of 2.0 left the
    /// box 4% short *and slid the held corner 9.8 units to the right*, which is the
    /// failure that mattered: a corner drag is a promise about the opposite corner.
    /// `railed_resize` solves the factor instead — here it settles at about
    /// 2.084 — so **the box is what to assert on, and tightly.**
    ///
    /// The rail is still checked, but only for the thing that stays true whatever
    /// the factor is: the drag held `y`, so the rail's height must not move.
    #[test]
    fn dragging_railed_text_scales_its_rail() {
        use ondin_core::kurbo::{BezPath, Shape};

        fn rail_box(doc: &Document, id: NodeId) -> Rect {
            let NodeKind::Text {
                on_path: Some(r), ..
            } = doc.get(id).unwrap().kind()
            else {
                panic!("not railed");
            };
            r.bounding_box()
        }

        let (mut doc, _, t) = artboard_with_text(TextSizing::Auto);
        let mut rail = BezPath::new();
        rail.move_to((0.0, 0.0));
        rail.curve_to((40.0, -20.0), (110.0, 20.0), (150.0, 0.0));
        doc.apply(&Transaction(vec![Operation::SetGeometry {
            id: t,
            geometry: GeometryPatch::TextPath(Some(rail)),
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let before = ondin_core::local_box(&doc, &res, t).expect("a railed box");
        let rail_before = rail_box(&doc, t);
        assert!(
            before.width() > 1.0,
            "the fixture must have a box: {before:?}"
        );

        // Bottom-right to twice the width and the same height, in the node's own
        // space — the artboard is at the origin and the node has no transform, so
        // world and local agree here.
        let target = Point::new(
            before.x0 + before.width() * 2.0,
            before.y0 + before.height(),
        );
        let tx = resize_to_handle(
            &doc,
            &res,
            t,
            Handle::BottomRight,
            target,
            Resize::geometry(false, false),
        );
        doc.apply(&tx).unwrap();
        let res = Resolved::rebuild(&doc);
        let after = ondin_core::local_box(&doc, &res, t).expect("a railed box");
        let rail_after = rail_box(&doc, t);

        // The box lands where the pointer asked. This is the assertion the whole
        // solve exists for — and the one a `TextSizing` patch fails outright,
        // since it moves nothing at all.
        assert!(
            (after.width() - before.width() * 2.0).abs() < 0.5,
            "the box must land where the drag asked: {} -> {} (wanted {})",
            before.width(),
            after.width(),
            before.width() * 2.0
        );
        assert!(
            (after.x0 - before.x0).abs() < 0.01 && (after.y0 - before.y0).abs() < 0.01,
            "and the held corner must not move at all: {before:?} -> {after:?}"
        );
        // The drag owned `x` only, so the rail keeps its height whatever factor
        // the solve chose for the width.
        // Not exactly zero, and the residual is the solve rather than float noise:
        // the ribbon's *height* shifts a little when its width changes, because
        // the tangents turn, so `sy` is nudged by a fraction of a thousandth to
        // hold the height the drag asked for. 0.0002 on this fixture.
        assert!(
            (rail_after.height() - rail_before.height()).abs() < 1e-3,
            "the rail must not move on the axis the drag held: {} -> {}",
            rail_before.height(),
            rail_after.height()
        );
        assert!(
            rail_after.width() > rail_before.width() * 1.5,
            "and it must actually have grown: {} -> {}",
            rail_before.width(),
            rail_after.width()
        );
    }

    /// 🚨 **Every corner holds the corner it promised to, not just the one the
    /// test above happened to drag** (§15 D789).
    ///
    /// `railed_resize` anchored the box's **origin**, and its own comment claimed
    /// that was *"the corner the gesture held"*. It is — for `BottomRight` alone.
    /// The other three hold a corner the origin is not, so pinning the origin left
    /// the held corner carrying the solve's **extent residual**, which this
    /// function is deliberately only *close* on. Reported from use: *"I held the
    /// bottom-left corner handle and the box started shifting to the right."*
    ///
    /// ⚠️ **It went unseen because of what was covered, not because it was
    /// subtle.** Every resize test in this file drags `BottomRight` or `TopLeft`;
    /// `BottomLeft` and `TopRight` appeared **zero** times. And the drift only
    /// exists on a *railed* node, because only this path leaves a residual —
    /// ordinary geometry resize is exact — so the one railed test, dragging the
    /// one handle where the origin *is* the anchor, could never show it.
    ///
    /// **Shrinking, not growing, and far**: the residual scales with the distance
    /// from 1, so a 2× grow hides what a 0.25× shrink shows. On the reporter's own
    /// document the slide measured **+0.41 units at 55%, +2.61 at 25%, +10.37 at
    /// 10%** — equal to the width residual at every step.
    ///
    /// **Flipped**, by restoring `want.origin() - got.origin()`: this fails at
    /// `BottomLeft` — the predicted handle — with *"the held corner slid 2.844
    /// units"*, and **`dragging_railed_text_scales_its_rail` stays green**, which
    /// is the whole argument for this test existing rather than an assertion added
    /// to that one. ⚠️ `TopRight` and `TopLeft` pass under the flip too on this
    /// fixture: the rail is a gentle S and their residual lands under the
    /// tolerance. **One of four handles fails it, and that is enough** — a test
    /// that only fired when every arm was wrong would have been green for this bug.
    #[test]
    fn every_handle_holds_its_own_corner_on_a_rail() {
        use ondin_core::kurbo::{BezPath, Vec2};

        for handle in [
            Handle::BottomRight,
            Handle::BottomLeft,
            Handle::TopRight,
            Handle::TopLeft,
        ] {
            let (mut doc, _, t) = artboard_with_text(TextSizing::Auto);
            let mut rail = BezPath::new();
            rail.move_to((0.0, 0.0));
            rail.curve_to((40.0, -20.0), (110.0, 20.0), (150.0, 0.0));
            doc.apply(&Transaction(vec![Operation::SetGeometry {
                id: t,
                geometry: GeometryPatch::TextPath(Some(rail)),
            }]))
            .unwrap();
            let res = Resolved::rebuild(&doc);
            let before = ondin_core::local_box(&doc, &res, t).expect("a railed box");

            // **A quarter of the size**, because the residual grows with the
            // distance from 1 and a gentle drag hides it. The artboard is at the
            // origin and the node has no transform, so world and local agree.
            let (w, h) = (before.width() * 0.25, before.height() * 0.25);
            let (hx, hy) = handle.anchored(before.width(), before.height());
            // The corner this handle holds, and the pointer that drags the
            // opposite one to a quarter-size box.
            let held = before.origin() + Vec2::new(hx.unwrap_or(0.0), hy.unwrap_or(0.0));
            let pointer = Point::new(
                match hx {
                    Some(v) if v > 0.0 => held.x - w,
                    _ => held.x + w,
                },
                match hy {
                    Some(v) if v > 0.0 => held.y - h,
                    _ => held.y + h,
                },
            );

            let tx = resize_to_handle(
                &doc,
                &res,
                t,
                handle,
                pointer,
                Resize::geometry(false, false),
            );
            doc.apply(&tx).unwrap();
            let res = Resolved::rebuild(&doc);
            let after = ondin_core::local_box(&doc, &res, t).expect("a railed box");

            // The fixture must actually have shrunk, or "the corner did not move"
            // is true of a gesture that did nothing.
            assert!(
                after.width() < before.width() * 0.5,
                "{handle:?}: the fixture must have shrunk: {} -> {}",
                before.width(),
                after.width()
            );

            let (ax, ay) = handle.anchored(after.width(), after.height());
            let held_after = after.origin() + Vec2::new(ax.unwrap_or(0.0), ay.unwrap_or(0.0));
            assert!(
                (held_after - held).hypot() < 0.05,
                "{handle:?}: the held corner slid {:.3} units — {held:?} -> {held_after:?}",
                (held_after - held).hypot()
            );
        }
    }

    /// 🚨 **The Scale tool on a rail held the corner it promised to hold** — it did
    /// not, and by 9.6 × 15 units (§15 D604, `[S13.2-L1-03]`).
    ///
    /// `railed_resize` solves the rail's scale against `railed_box`, which lays the
    /// text out; `scale_scalars` then writes `font_size * s`. So the solve
    /// converged on `layout(new rail, **old** font)` while the document ended up
    /// with `layout(new rail, **new** font)`. Measured before the fix, both tools,
    /// the same drag on this fixture:
    ///
    /// ```text
    /// Select : asked 317.173 x 61.494 at (-6.708, -20.773)
    ///          got   318.427 x 58.324 at (-6.708, -20.773)   <-- anchor exact
    /// Scale  : asked 317.173 x 61.494 at (-6.708, -20.773)
    ///          got   330.690 x 77.524 at (-16.289, -35.773)  <-- anchor moved
    ///          font 16.00 -> 32.00
    /// ```
    ///
    /// ⚠️ **The Select run is the control and it is what localises the fault.** The
    /// solve lands the anchor to the bit whenever nothing else touches the type, so
    /// this was never `railed_resize` being imprecise — it was the *composition* of
    /// two correct steps in the wrong order. `railed_resize`'s own doc names the
    /// symptom as the thing that *"could not stand"*: *"a corner drag is a promise
    /// about the **opposite** corner, and a layer that slides out from under the
    /// pointer reads as a broken gesture"*.
    ///
    /// ⚠️ **`Scaling::Photographic` and `on_path` had never met in any test.**
    /// Grepped at the time: nine `Photographic` sites, none railed; three rail
    /// sites, none photographic. The two features are three years of gestures apart
    /// and the suite had no fixture holding both.
    ///
    /// **The anchor is asserted tightly and the extent loosely**, which is
    /// `dragging_railed_text_scales_its_rail`'s reading above and the same one:
    /// a ribbon's box is affine rather than linear in the scale, so the far corner
    /// is a target and the near one is a promise.
    ///
    /// Flip-check, run: `kind_to_solve_against` returning `None` unconditionally —
    /// i.e. the solve back on the unscaled kind — fails at the **anchor**
    /// assertion, at `x0: -16.2887…, y0: -35.7732…` against `-6.7082…, -20.7734…`.
    /// **That is the finding's `(9.58, 15.00)` to the decimal**, three sessions
    /// after it was measured, which is worth more than the assertion passing: the
    /// fixture reproduces the report.
    #[test]
    fn the_scale_tool_on_a_rail_holds_the_corner_the_select_tool_holds() {
        use ondin_core::kurbo::BezPath;

        let railed = || {
            let (mut doc, _, t) = artboard_with_text(TextSizing::Auto);
            let mut rail = BezPath::new();
            rail.move_to((0.0, 0.0));
            rail.curve_to((40.0, -20.0), (110.0, 20.0), (150.0, 0.0));
            doc.apply(&Transaction(vec![Operation::SetGeometry {
                id: t,
                geometry: GeometryPatch::TextPath(Some(rail)),
            }]))
            .unwrap();
            let res = Resolved::rebuild(&doc);
            (doc, res, t)
        };

        let run = |scaling: Scaling| {
            let (mut doc, res, t) = railed();
            let before = ondin_core::local_box(&doc, &res, t).expect("a railed box");
            let target = Point::new(
                before.x0 + before.width() * 2.0,
                before.y0 + before.height() * 2.0,
            );
            let tx = resize_to_handle(
                &doc,
                &res,
                t,
                Handle::BottomRight,
                target,
                Resize {
                    scaling,
                    ..Resize::geometry(false, false)
                },
            );
            doc.apply(&tx).unwrap();
            let res = Resolved::rebuild(&doc);
            let after = ondin_core::local_box(&doc, &res, t).expect("a railed box");
            (before, after)
        };

        let (before, select) = run(Scaling::Geometry);
        let (_, scale) = run(Scaling::Photographic);

        // The control, first: the Select tool lands the held corner exactly, which
        // is what says the solve is right and only the composition was wrong.
        assert!(
            (select.x0 - before.x0).abs() < 0.01 && (select.y0 - before.y0).abs() < 0.01,
            "the Select control must hold the corner: {before:?} -> {select:?}"
        );
        // And the Scale tool has to hold it to the same tolerance. It slid
        // (9.58, 15.00) before.
        assert!(
            (scale.x0 - before.x0).abs() < 0.01 && (scale.y0 - before.y0).abs() < 0.01,
            "the Scale tool must hold the same corner: {before:?} -> {scale:?}"
        );
        // The far corner is a target rather than a promise, so it is asserted
        // loosely — but a 26% overshoot is not inside "loosely".
        let asked = before.height() * 2.0;
        assert!(
            (scale.height() - asked).abs() / asked < 0.05,
            "and the height must land near the drag: wanted {asked}, got {}",
            scale.height()
        );
    }

    /// **Resizing a *group* that holds a railed text edits the rail too**
    /// (§15 D472) — `[S13.2-L1-02]`.
    ///
    /// `resize_to_handle` has had the rail arm since §15 D405 and `scale_geometry`
    /// did not, so a drag through the **group** door reached the railed child as
    /// `scale_scalars` plus a bare `SetTransform`: on a rail `TextSizing` is inert
    /// — the rail's length is the measure — so for `Auto` `scaled_text_sizing`
    /// answers `None` and there was nothing left to write.
    ///
    /// Measured on this fixture, `BottomRight` to 2× on both axes:
    ///
    /// ```text
    /// group        161.88 x 170.77  ->  171.88 x 320.77   (asked 323.76 x 341.54)
    /// railed text  158.59 x  30.75  ->  158.59 x  30.75   <-- unchanged
    /// rect control  80.00 x  40.00  ->  160.00 x  80.00   <-- correct
    /// ```
    ///
    /// The width is **47% short of the drag**, on the first frame, with no error
    /// and no report — because the text's own ribbon pins the group's width, so
    /// the one child that did not move decided the container's box.
    ///
    /// ⚠️ **The rect in the group is the control and it is load-bearing.** Without
    /// it, "the group did not reach the box" and "nothing in the group moved" are
    /// the same observation, and a `scale_subtree` that had stopped working
    /// entirely would pass.
    ///
    /// ⚠️ **Nothing reached a rail through this door before.**
    /// `dragging_railed_text_scales_its_rail` above is the only rail test on any
    /// resize path in the tree and it calls `resize_to_handle` directly — which is
    /// how an arm could be missing from the other door for as long as the feature
    /// has existed.
    ///
    /// ⚠️ **Asserted loosely on the group and tightly on the child's growth.**
    /// `railed_resize` solves the factor rather than taking it, because a railed
    /// box is the ribbon the rail sweeps and is affine rather than linear in the
    /// scale, so the exact width is the solve's business. What is being asserted
    /// is that the child moved *with* its sibling instead of standing still.
    ///
    /// ⚠️ **Flipped** by deleting the `on_path` arm from `scale_geometry`: fails
    /// on the child assertion at 158.59 — unchanged to the hundredth, which is the
    /// finding's own number.
    #[test]
    fn resizing_a_group_carries_a_railed_text_with_it() {
        use ondin_core::kurbo::BezPath;

        let (mut doc, _, t) = artboard_with_text(TextSizing::Auto);
        let ab = doc.get(t).unwrap().parent().expect("the artboard");
        let mut ids = IdSource::new(0x9A11);
        let (group, rect) = (ids.mint(), ids.mint());
        let mut rail = BezPath::new();
        rail.move_to((0.0, 0.0));
        rail.curve_to((40.0, -20.0), (110.0, 20.0), (150.0, 0.0));
        doc.apply(&Transaction(vec![
            Operation::SetGeometry {
                id: t,
                geometry: GeometryPatch::TextPath(Some(rail)),
            },
            Operation::CreateNode {
                id: group,
                parent: ab,
                index: 1,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: rect,
                parent: group,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(80.0, 40.0),
                    corner_radii: Default::default(),
                },
                transform: Some(Affine::translate((0.0, 120.0))),
                name: None,
            },
            Operation::Reparent {
                id: t,
                new_parent: group,
                index: 0,
            },
        ]))
        .expect("group a railed text with a rect");

        let res = Resolved::rebuild(&doc);
        let g0 = ondin_core::local_box(&doc, &res, group).expect("the group has a box");
        let t0 = ondin_core::local_box(&doc, &res, t).expect("the text has a box");
        let r0 = ondin_core::local_box(&doc, &res, rect).expect("the rect has a box");

        // Drag the group's bottom-right corner to twice its box, in world space —
        // the artboard and the group are both at the origin here.
        let world = res.world_transform(group).expect("world");
        let target = world * Point::new(g0.x0 + g0.width() * 2.0, g0.y0 + g0.height() * 2.0);
        let tx = resize_group(
            &doc,
            &res,
            group,
            g0,
            Handle::BottomRight,
            target,
            Resize::geometry(false, false),
        );
        doc.apply(&tx).expect("the group resize applies");
        let res = Resolved::rebuild(&doc);
        let t1 = ondin_core::local_box(&doc, &res, t).expect("still a box");
        let r1 = ondin_core::local_box(&doc, &res, rect).expect("still a box");

        // **The control first**: the rect doubled, so the gesture reached the
        // subtree at all.
        assert!(
            (r1.width() - r0.width() * 2.0).abs() < 0.5,
            "control: the plain rect must double, {} -> {}",
            r0.width(),
            r1.width()
        );
        assert!(
            t1.width() > t0.width() * 1.5,
            "the railed text must grow with its sibling rather than stand still: \
             {} -> {} (unchanged is the defect)",
            t0.width(),
            t1.width()
        );
    }

    /// **A fixed box is not un-fixed by a side drag.** The rule is about which
    /// axis the *user* authors, not about the handle alone: an already-authored
    /// height stays authored however the width is changed.
    #[test]
    fn dragging_a_side_of_fixed_text_keeps_it_fixed() {
        let sizing = drag_text(
            TextSizing::Fixed(Size::new(60.0, 30.0)),
            Handle::Right,
            Point::new(150.0, 0.0),
        );
        assert!(
            matches!(sizing, TextSizing::Fixed(s)
                if (s.width - 150.0).abs() < 1e-9 && (s.height - 30.0).abs() < 1e-9),
            "expected a 150×30 fixed box, got {sizing:?}"
        );
    }

    #[test]
    fn create_rect_lands_at_world_drag_box_under_translated_parent() {
        // Artboard translated by (50,20); draw a rect from world (100,100) to
        // (160,140). Expect size 60x40 and world origin exactly (100,100).
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
            },
            transform: Some(Affine::translate((50.0, 20.0))),
            name: None,
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let parent_world = res.world_transform(ab).unwrap();

        let rect = ids.mint();
        let tx = create_rect(
            rect,
            ab,
            0,
            parent_world,
            Point::new(100.0, 100.0),
            Point::new(160.0, 140.0),
            Color::from_rgba8(1, 2, 3, 255),
        );
        doc.apply(&tx).unwrap();
        let res = Resolved::rebuild(&doc);

        let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
            panic!("expected a rect");
        };
        assert!((size.width - 60.0).abs() < 1e-9);
        assert!((size.height - 40.0).abs() < 1e-9);
        let origin = res.world_transform(rect).unwrap() * Point::ZERO;
        assert!((origin.x - 100.0).abs() < 1e-9, "x={}", origin.x);
        assert!((origin.y - 100.0).abs() < 1e-9, "y={}", origin.y);
        assert!(doc.get(rect).unwrap().paint().fills.len() == 1);
    }

    /// A frame drawn inside a frame becomes its child and lands where it was drawn
    /// — which needs the parent's world transform, exactly as a shape does. While
    /// frames could only hang off the root this took `Affine::IDENTITY`, and a nested
    /// frame under a moved parent would have come out offset by the parent's own
    /// translation (§5.3).
    #[test]
    fn a_frame_drawn_inside_a_moved_frame_lands_where_it_was_drawn() {
        let mut ids = IdSource::new(0x5EED);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let page = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: page,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
            },
            transform: Some(Affine::translate((50.0, 20.0))),
            name: None,
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let parent_world = res.world_transform(page).unwrap();

        let card = ids.mint();
        let tx = create_artboard(
            card,
            page,
            0,
            parent_world,
            Point::new(100.0, 100.0),
            Point::new(160.0, 140.0),
            Color::WHITE,
        );
        doc.apply(&tx).expect("a frame nests inside a frame");
        let res = Resolved::rebuild(&doc);

        assert_eq!(doc.get(card).unwrap().parent(), Some(page));
        let NodeKind::Artboard { size, .. } = doc.get(card).unwrap().kind() else {
            panic!("expected a frame");
        };
        assert!((size.width - 60.0).abs() < 1e-9);
        assert!((size.height - 40.0).abs() < 1e-9);
        let origin = res.world_transform(card).unwrap() * Point::ZERO;
        assert!((origin.x - 100.0).abs() < 1e-9, "x={}", origin.x);
        assert!((origin.y - 100.0).abs() < 1e-9, "y={}", origin.y);
    }

    /// **A dragged text box is `Fixed` at what was drawn, and lands where it was
    /// drawn under a moved parent** — the whole of what the Text tool's drag adds over
    /// its click, which plants `TextSizing::Auto` at a point.
    ///
    /// Reported as: "the Text tool can only set a single cursor on the screen, it
    /// can't draw a bounding box for the text, something typical when you want a fixed
    /// width text."
    #[test]
    fn a_dragged_text_box_is_fixed_at_the_drawn_size() {
        let mut ids = IdSource::new(0x7E87);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let page = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: page,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
            },
            transform: Some(Affine::translate((50.0, 20.0))),
            name: None,
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let parent_world = res.world_transform(page).unwrap();

        let parts = ondin_core::TextParts {
            style: ondin_core::TextStyle {
                font_family: "Inter".into(),
                font_size: 16.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let t = ids.mint();
        let tx = create_text(
            t,
            page,
            0,
            parent_world,
            Point::new(100.0, 100.0),
            Point::new(260.0, 180.0),
            &parts,
        );
        doc.apply(&tx).expect("a text box nests inside a frame");
        let res = Resolved::rebuild(&doc);

        assert_eq!(doc.get(t).unwrap().parent(), Some(page));
        let NodeKind::Text {
            content, sizing, ..
        } = doc.get(t).unwrap().kind()
        else {
            panic!("expected a text node");
        };
        assert!(content.is_empty(), "created empty, to be typed into");
        let ondin_core::TextSizing::Fixed(size) = sizing else {
            panic!("a drag asked for a width, so the mode is Fixed: {sizing:?}");
        };
        assert!((size.width - 160.0).abs() < 1e-9, "w={}", size.width);
        assert!((size.height - 80.0).abs() < 1e-9, "h={}", size.height);
        // Under a parent translated by (50,20) — so the box has to go through the
        // parent's world transform, as every other create tool's does.
        let origin = res.world_transform(t).unwrap() * Point::ZERO;
        assert!((origin.x - 100.0).abs() < 1e-9, "x={}", origin.x);
        assert!((origin.y - 100.0).abs() < 1e-9, "y={}", origin.y);
    }

    /// **A thin drag still gets a box one line tall**, because a node whose first
    /// character overflows it reads as the tool being broken rather than as a small
    /// box. The floor comes from a shaped layout rather than from `font_size`, since
    /// `line_height: None` means "the font decides" (§15 D162).
    ///
    /// The *width* deliberately has no floor: a narrow column is a real thing to
    /// want, and wrapping handles it.
    #[test]
    fn a_thin_text_drag_still_clears_one_line() {
        let mut ids = IdSource::new(0x7E88);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let parts = ondin_core::TextParts {
            style: ondin_core::TextStyle {
                font_family: "Inter".into(),
                font_size: 40.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let t = ids.mint();
        doc.apply(&create_text(
            t,
            root,
            0,
            Affine::IDENTITY,
            Point::new(10.0, 10.0),
            // Two units tall — well under one 40pt line.
            Point::new(38.0, 12.0),
            &parts,
        ))
        .unwrap();
        let NodeKind::Text { sizing, .. } = doc.get(t).unwrap().kind() else {
            panic!("expected a text node");
        };
        let ondin_core::TextSizing::Fixed(size) = sizing else {
            panic!("{sizing:?}");
        };
        // One 40pt line of Inter at the default line height, whatever that resolves
        // to — asserted as "at least the type size" so this does not pin a metric.
        assert!(
            size.height >= 40.0,
            "a 2-unit drag should still hold a line of 40pt type, got {}",
            size.height
        );
        assert!(
            (size.width - 28.0).abs() < 1e-9,
            "and the width is left exactly as drawn: {}",
            size.width
        );
    }

    /// **What "fixed width" is actually for: the text wraps to the drawn box.** The
    /// field being `Fixed` is only the claim; this is the behaviour the report asked
    /// for, so it is asserted against the shaped layout rather than against the model.
    ///
    /// Also pins the half a `Fixed` box could get wrong in the other direction — the
    /// box does **not** grow to fit, which is what distinguishes it from the click's
    /// `Auto` and is why `TextOverflow::Visible` (the default) matters: text past the
    /// bottom hangs out rather than vanishing.
    #[test]
    fn a_dragged_text_box_wraps_what_is_typed_into_it() {
        let mut ids = IdSource::new(0x7E89);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let parts = ondin_core::TextParts {
            style: ondin_core::TextStyle {
                font_family: "Inter".into(),
                font_size: 16.0,
                ..Default::default()
            },
            ..Default::default()
        };
        let t = ids.mint();
        doc.apply(&create_text(
            t,
            root,
            0,
            Affine::IDENTITY,
            Point::new(0.0, 0.0),
            Point::new(90.0, 200.0),
            &parts,
        ))
        .unwrap();

        let kind = doc.get(t).unwrap().kind();
        let node_parts = ondin_core::TextParts::of(kind).expect("a text node");
        let sentence = "wrap this sentence over several lines";
        let mut edit = ondin_core::TextEdit::new("", node_parts.clone());
        edit.insert(sentence);
        let laid = edit.text_layout();

        // The same string in an auto-width node is one long line; in the drawn box it
        // is several, and the box is the width that was drawn.
        let mut auto = node_parts;
        auto.sizing = ondin_core::TextSizing::Auto;
        let unwrapped = ondin_core::text::layout(auto.as_ref(sentence));
        assert!(
            unwrapped.size.width > 150.0,
            "the fixture has to be wider than the box to prove anything: {}",
            unwrapped.size.width
        );
        assert!(
            (laid.size.width - 90.0).abs() < 1e-9,
            "the box keeps the drawn width: {}",
            laid.size.width
        );
        assert!(
            (laid.size.height - 200.0).abs() < 1e-9,
            "and the drawn height, rather than growing to the text: {}",
            laid.size.height
        );
        // Distinct baselines is the wrap, without pinning how many lines Inter takes.
        let mut baselines: Vec<i32> = laid
            .runs
            .iter()
            .filter_map(|r| r.glyphs.first().map(|g| (g.y * 100.0) as i32))
            .collect();
        baselines.sort_unstable();
        baselines.dedup();
        assert!(
            baselines.len() > 1,
            "a 90-unit box must wrap this sentence: {baselines:?}"
        );
    }

    /// The Text tool is the one member of `drags_a_box` that is *also* a click tool,
    /// and the two gestures are exclusive because egui reports a press as one or the
    /// other. Pinned because the set is a `matches!` list, which the compiler does not
    /// check against intent.
    #[test]
    fn text_both_drags_a_box_and_stays_a_click_tool() {
        assert!(Tool::Text.drags_a_box());
        // Every other box tool, unchanged.
        for tool in [
            Tool::Frame,
            Tool::Rect,
            Tool::Ellipse,
            Tool::Polygon,
            Tool::Star,
            Tool::Line,
        ] {
            assert!(tool.drags_a_box(), "{tool:?}");
        }
        // And the ones that draw by other means, or not at all, still do not.
        //
        // **The exclusions carry more weight than they used to**, which is why
        // `ImageEdit` was added here: since §15 D294 this predicate also decides
        // which tools *clear the selection* when armed, and the four below are all
        // tools that either operate on the selection or are entered from it.
        // `Node` and `ImageEdit` are the sharp ones — a double-click switches to
        // them with the layer selected, and `edited_path` reads that selection to
        // know what it is editing, so a wrong answer here would empty the tool at
        // the instant it opened. `ImageEdit` was the one member never asserted
        // against this predicate at all.
        for tool in [
            Tool::Pen,
            Tool::Node,
            Tool::ImageEdit,
            Tool::Select,
            Tool::Scale,
            Tool::Hand,
        ] {
            assert!(!tool.drags_a_box(), "{tool:?}");
        }
    }

    /// **The whole click table, tool by tool** (§15 D291, D295), and the
    /// containment that goes with it: anything that answers a click also drags a
    /// box, or it would be a creation tool with only one way to be given a size.
    ///
    /// Written as the complete list rather than as spot checks, because this is the
    /// claim that was previously made in **prose** — `clicks_out_a_default_box`'s
    /// doc asserted that Text and Image both had click answers of their own, and
    /// Image's did not exist. The `match` in `click_makes` is what makes a *missing*
    /// arm a build error; this is what makes a *wrong* arm a test failure. The two
    /// cover different halves and neither is redundant.
    #[test]
    fn every_tool_answers_a_click_with_exactly_what_it_should() {
        use ClickCreate::*;
        for (tool, want) in [
            (Tool::Rect, Some(DefaultBox)),
            (Tool::Ellipse, Some(DefaultBox)),
            (Tool::Polygon, Some(DefaultBox)),
            (Tool::Star, Some(DefaultBox)),
            // The gesture the Image tool exists for: a loaded cursor discharged by
            // saying where the picture goes, its size coming off the file.
            (Tool::Image, Some(FittedImage)),
            (Tool::Text, Some(TextNode)),
            // Box tools deliberately without a click answer: a frame's default size
            // would guess at a layout, and a line's would come out diagonal.
            (Tool::Frame, None),
            (Tool::Line, None),
            // Not creation tools at all.
            (Tool::Select, None),
            (Tool::Scale, None),
            (Tool::Pen, None),
            (Tool::Node, None),
            (Tool::ImageEdit, None),
            (Tool::Hand, None),
        ] {
            assert_eq!(tool.click_makes(), want, "{tool:?}");
            if want.is_some() {
                assert!(
                    tool.drags_a_box(),
                    "{tool:?} answers a click but cannot be dragged out, so it has \
                     only one way to be given a size"
                );
            }
        }
    }

    #[test]
    fn create_shape_box_is_order_independent() {
        // Dragging bottom-right→top-left yields the same box as top-left→BR.
        let pw = Affine::IDENTITY;
        let (o1, s1) = local_box(pw, Point::new(10.0, 10.0), Point::new(50.0, 30.0));
        let (o2, s2) = local_box(pw, Point::new(50.0, 30.0), Point::new(10.0, 10.0));
        assert_eq!(o1, o2);
        assert_eq!(s1, s2);
        assert_eq!(o1, Point::new(10.0, 10.0));
        assert_eq!(s1, Size::new(40.0, 20.0));
    }

    #[test]
    fn create_path_points_map_back_to_world() {
        use ondin_core::kurbo::PathEl;
        let mut ids = IdSource::new(4);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
            },
            transform: Some(Affine::translate((12.0, 34.0))),
            name: None,
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let pw = res.world_transform(ab).unwrap();

        let path = ids.mint();
        let world_pts = [
            Point::new(50.0, 60.0),
            Point::new(120.0, 60.0),
            Point::new(120.0, 140.0),
        ];
        let anchors: Vec<PenAnchor> = world_pts.iter().copied().map(PenAnchor::corner).collect();
        doc.apply(&create_path(
            path,
            ab,
            0,
            pw,
            &anchors,
            false,
            Color::BLACK,
            2.0,
        ))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let w = res.world_transform(path).unwrap();
        let NodeKind::Path { path: bez, .. } = doc.get(path).unwrap().kind() else {
            panic!("expected a path");
        };
        let local_pts: Vec<Point> = bez
            .elements()
            .iter()
            .filter_map(|el| match el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => Some(*p),
                _ => None,
            })
            .collect();
        assert_eq!(local_pts.len(), 3);
        for (local, expected) in local_pts.iter().zip(world_pts.iter()) {
            let got = w * *local;
            assert!(
                (got.x - expected.x).abs() < 1e-9 && (got.y - expected.y).abs() < 1e-9,
                "{got:?} vs {expected:?}"
            );
        }
    }

    /// The close snap, which is one answer serving two jobs — where the pending
    /// segment ends, and whether a press closes the path.
    ///
    /// **The zoom case is the one worth the test.** The radius is screen pt, so
    /// the world tolerance has to shrink as the view zooms in; a version that
    /// compared world distance against the constant directly would pass every
    /// assertion below except that one, and would feel wrong only at zoom levels
    /// nobody tests at.
    #[test]
    fn the_pen_snaps_closed_within_a_screen_radius_not_a_world_one() {
        const R: f64 = 5.0;
        let square = [
            PenAnchor::corner(Point::new(0.0, 0.0)),
            PenAnchor::corner(Point::new(100.0, 0.0)),
            PenAnchor::corner(Point::new(100.0, 100.0)),
        ];

        // 4 world units from the start, at 1:1 — inside 5pt, so it snaps, and it
        // snaps *to the anchor* rather than to the pointer.
        assert_eq!(
            pen_close_snap(&square, Point::new(4.0, 0.0), 1.0, R),
            Some(Point::new(0.0, 0.0))
        );
        assert_eq!(pen_close_snap(&square, Point::new(40.0, 0.0), 1.0, R), None);

        // The same 4 world units at 4x is 16 screen pt away, so it must not.
        assert_eq!(
            pen_close_snap(&square, Point::new(4.0, 0.0), 4.0, R),
            None,
            "the radius is screen pt: zooming in must tighten the world tolerance"
        );
        // ...and zoomed out, a point 4 world units away is 2pt away and snaps.
        assert_eq!(
            pen_close_snap(&square, Point::new(4.0, 0.0), 0.5, R),
            Some(Point::new(0.0, 0.0))
        );

        // Nothing to close with fewer than two anchors: `create_path` refuses a
        // one-anchor path, so offering to close one would be a promise it breaks.
        let lone = [PenAnchor::corner(Point::new(0.0, 0.0))];
        assert_eq!(pen_close_snap(&lone, Point::new(0.0, 0.0), 1.0, R), None);
        assert_eq!(pen_close_snap(&[], Point::new(0.0, 0.0), 1.0, R), None);

        // Only the first anchor offers a close — snapping to a middle one would
        // stack a second point on an existing one.
        assert_eq!(
            pen_close_snap(&square, Point::new(100.0, 0.0), 1.0, R),
            None,
            "the second anchor is not a close target"
        );
    }

    /// A pen anchor with handles produces a *curve*, and one without produces a
    /// line. The straight case has to keep falling out of the curved one: a path
    /// clicked out with corners must not come back as cubics whose control
    /// points happen to be collinear, or every polyline in the file grows three
    /// times the data and stops being editable as a polyline.
    #[test]
    fn pen_anchors_curve_only_where_they_have_handles() {
        use ondin_core::kurbo::{PathEl, Vec2};
        let corner = |x, y| PenAnchor::corner(Point::new(x, y));
        let smooth = |x, y, hx, hy| PenAnchor::smooth(Point::new(x, y), Vec2::new(hx, hy));

        let straight = pen_outline(&[corner(0.0, 0.0), corner(10.0, 0.0)], false);
        assert!(
            matches!(straight.elements()[1], PathEl::LineTo(_)),
            "two corners are a line: {:?}",
            straight.elements()
        );

        // A smooth anchor's handles are mirrored, so the curve leaves along
        // `out` and arrives at the next anchor against its own.
        let curved = pen_outline(&[smooth(0.0, 0.0, 5.0, 5.0), corner(10.0, 0.0)], false);
        let PathEl::CurveTo(c1, c2, end) = curved.elements()[1] else {
            panic!("a handled anchor should curve: {:?}", curved.elements());
        };
        assert_eq!(c1, Point::new(5.0, 5.0), "leaves along the handle");
        assert_eq!(c2, Point::new(10.0, 0.0), "a corner has no incoming handle");
        assert_eq!(end, Point::new(10.0, 0.0));

        // Closing joins the last anchor back to the first through both handles.
        let closed = pen_outline(
            &[corner(0.0, 0.0), corner(10.0, 0.0), corner(10.0, 10.0)],
            true,
        );
        assert!(
            matches!(closed.elements().last(), Some(PathEl::ClosePath)),
            "{:?}",
            closed.elements()
        );
    }

    /// **A broken handle pair is the whole reason an anchor carries two vectors.**
    ///
    /// Alt while shaping leaves the incoming handle behind, so the segment
    /// *arrives* straight and *leaves* on a curve. Under the old mirrored single
    /// vector this shape was unsayable — the incoming control point was always
    /// `at - out` — so the assertion to make is that the two sides of one anchor
    /// disagree, which is exactly what a mirrored anchor cannot do.
    #[test]
    fn an_anchor_can_break_its_handles_and_bend_only_one_side() {
        use ondin_core::kurbo::{PathEl, Vec2};

        let broken = PenAnchor {
            at: Point::new(10.0, 0.0),
            out: Vec2::new(5.0, 5.0),
            back: Vec2::ZERO,
            radius: 0.0,
        };
        let path = pen_outline(
            &[
                PenAnchor::corner(Point::new(0.0, 0.0)),
                broken,
                PenAnchor::corner(Point::new(20.0, 0.0)),
            ],
            false,
        );

        // Into the broken anchor: no incoming handle, so the segment is *straight*
        // — the strongest form of "it did not mirror the outgoing one". A cubic
        // with both control points on its own endpoints would draw the same line
        // and is what the segment rule used to emit (§15).
        assert_eq!(
            path.elements()[1],
            PathEl::LineTo(Point::new(10.0, 0.0)),
            "the incoming side was broken, so the segment into it must not bend: {:?}",
            path.elements()
        );
        // Out of it: the handle the drag pulled.
        let PathEl::CurveTo(c1, _, _) = path.elements()[2] else {
            panic!(
                "out of a handled anchor should curve: {:?}",
                path.elements()
            );
        };
        assert_eq!(c1, Point::new(15.0, 5.0), "leaves along `out`");

        // The A/B: mirrored, the same anchor bends both ways.
        let mirrored = pen_outline(
            &[
                PenAnchor::corner(Point::new(0.0, 0.0)),
                PenAnchor::smooth(Point::new(10.0, 0.0), Vec2::new(5.0, 5.0)),
                PenAnchor::corner(Point::new(20.0, 0.0)),
            ],
            false,
        );
        let PathEl::CurveTo(_, c2m, _) = mirrored.elements()[1] else {
            panic!("{:?}", mirrored.elements());
        };
        assert_eq!(
            c2m,
            Point::new(5.0, -5.0),
            "mirrored, the incoming control point is the reflection of `out`"
        );
    }

    /// `pull_within` is what a pen drag does to an anchor, and the Alt case is the
    /// half that used to be unsayable — so both branches are asserted, and the
    /// re-mirroring one matters because Alt is read live every frame: pressing
    /// and releasing it mid-drag has to put the handles back together.
    ///
    /// The retract radius is asserted here too, because the pen depends on it now:
    /// a press plants its anchor on the *snapped* point and shapes it from the raw
    /// pointer, so without the substitution a plain click would leave a handle as
    /// long as the snap adjustment — a corner that is not quite a corner.
    #[test]
    fn pulling_a_handle_mirrors_unless_the_pair_is_broken() {
        use ondin_core::kurbo::Vec2;
        let at = Point::new(10.0, 10.0);
        // No retraction unless asked for; the radius has its own case below.
        let pull = |a: &mut PenAnchor, to: Point, alt: bool| a.pull_within(Side::Out, to, alt, 0.0);

        let mut a = PenAnchor::corner(at);
        pull(&mut a, Point::new(15.0, 10.0), false);
        assert_eq!(a.out, Vec2::new(5.0, 0.0));
        assert_eq!(a.back, Vec2::new(-5.0, 0.0), "mirrored");

        // Alt: the incoming handle stays exactly where the mirror left it.
        pull(&mut a, Point::new(10.0, 20.0), true);
        assert_eq!(a.out, Vec2::new(0.0, 10.0));
        assert_eq!(
            a.back,
            Vec2::new(-5.0, 0.0),
            "a broken pair leaves the incoming handle alone"
        );

        // Releasing Alt re-mirrors, rather than leaving the anchor broken for good.
        pull(&mut a, Point::new(10.0, 20.0), false);
        assert_eq!(a.back, Vec2::new(0.0, -10.0));

        // Within the retract radius the handle lands *on* the anchor, so the
        // anchor is a corner again — which is what keeps a snapped pen click from
        // drawing a hair of a curve.
        a.pull_within(Side::Out, Point::new(12.0, 11.0), false, 7.0);
        assert!(a.is_corner(), "out={:?} back={:?}", a.out, a.back);
        // And just outside it, the handle is where the pointer put it.
        a.pull_within(Side::Out, Point::new(30.0, 10.0), false, 7.0);
        assert_eq!(a.out, Vec2::new(20.0, 0.0));
    }
    /// A corner is both handles being nothing, not just the outgoing one.
    ///
    /// The predicate decides whether a segment is a line or a cubic, so an anchor
    /// with only an incoming handle reading as a corner would straighten a curve
    /// the user drew.
    #[test]
    fn an_anchor_with_only_an_incoming_handle_is_not_a_corner() {
        use ondin_core::kurbo::Vec2;
        let incoming_only = PenAnchor {
            at: Point::new(1.0, 1.0),
            out: Vec2::ZERO,
            back: Vec2::new(-3.0, 0.0),
            radius: 0.0,
        };
        assert!(!incoming_only.is_corner());
        assert!(PenAnchor::corner(Point::new(1.0, 1.0)).is_corner());
    }

    /// The pen's curve has to survive the trip into a node: `create_path`
    /// re-expresses it in local space, and a curve whose control points were
    /// dropped or translated differently from its anchors is a different shape.
    #[test]
    fn a_curved_pen_path_keeps_its_control_points_in_world_space() {
        use ondin_core::kurbo::{PathEl, Vec2};
        let mut ids = IdSource::new(11);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
            },
            transform: Some(Affine::translate((17.0, 23.0))),
            name: None,
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let pw = res.world_transform(ab).unwrap();

        let anchors = [
            PenAnchor::smooth(Point::new(50.0, 60.0), Vec2::new(20.0, -20.0)),
            PenAnchor::corner(Point::new(150.0, 60.0)),
        ];
        let want = pen_outline(&anchors, false);

        let id = ids.mint();
        doc.apply(&create_path(
            id,
            ab,
            0,
            pw,
            &anchors,
            false,
            Color::BLACK,
            2.0,
        ))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let world = res.world_transform(id).unwrap();
        let NodeKind::Path { path, .. } = doc.get(id).unwrap().kind() else {
            panic!("expected a path");
        };

        // Mapping the stored path back through its own world transform must
        // reproduce the world-space outline the pen drew, control points and all.
        let mut back = path.clone();
        back.apply_affine(world);
        let (a, b) = (want.elements(), back.elements());
        assert_eq!(a.len(), b.len(), "{a:?} vs {b:?}");
        for (x, y) in a.iter().zip(b.iter()) {
            let pts = |el: &PathEl| match *el {
                PathEl::MoveTo(p) | PathEl::LineTo(p) => vec![p],
                PathEl::QuadTo(c, p) => vec![c, p],
                PathEl::CurveTo(c1, c2, p) => vec![c1, c2, p],
                PathEl::ClosePath => vec![],
            };
            let (px, py) = (pts(x), pts(y));
            assert_eq!(px.len(), py.len(), "{x:?} vs {y:?}");
            for (p, q) in px.iter().zip(py.iter()) {
                assert!(
                    (p.x - q.x).abs() < 1e-9 && (p.y - q.y).abs() < 1e-9,
                    "{p:?} vs {q:?}"
                );
            }
        }
    }

    /// Points of a path element, for comparing two paths element by element.
    fn el_points(el: &PathEl) -> Vec<Point> {
        match *el {
            PathEl::MoveTo(p) | PathEl::LineTo(p) => vec![p],
            PathEl::QuadTo(c, p) => vec![c, p],
            PathEl::CurveTo(c1, c2, p) => vec![c1, c2, p],
            PathEl::ClosePath => vec![],
        }
    }

    /// Two paths describe the same curve, element for element.
    fn assert_same_path(a: &BezPath, b: &BezPath) {
        let (ea, eb) = (a.elements(), b.elements());
        assert_eq!(
            ea.len(),
            eb.len(),
            "different element counts:\n{ea:#?}\nvs\n{eb:#?}"
        );
        for (x, y) in ea.iter().zip(eb.iter()) {
            assert_eq!(
                std::mem::discriminant(x),
                std::mem::discriminant(y),
                "{x:?} vs {y:?}"
            );
            for (p, q) in el_points(x).iter().zip(el_points(y).iter()) {
                assert!(
                    (p.x - q.x).abs() < 1e-9 && (p.y - q.y).abs() < 1e-9,
                    "{x:?} vs {y:?}"
                );
            }
        }
    }

    /// **`pen_anchors` is `pen_outline` backwards, and the round trip is the
    /// property that matters** — every point edit starts by reading a stored path
    /// and ends by writing one, so a trip that changes the shape corrupts a path
    /// merely by selecting it.
    ///
    /// The closed case is the one with a trap in it: `pen_outline` draws the
    /// closing segment *and* emits `ClosePath`, so the final point repeats the
    /// first. Read naively that is an extra anchor, and each read-and-write would
    /// add another — which is why the anchor count is asserted, not just the
    /// geometry.
    #[test]
    fn reading_a_pen_path_back_gives_the_anchors_that_drew_it() {
        use ondin_core::kurbo::Vec2;

        let anchors = vec![
            PenAnchor::corner(Point::new(0.0, 0.0)),
            PenAnchor::smooth(Point::new(40.0, 30.0), Vec2::new(10.0, 0.0)),
            // A broken pair: arrives straight, leaves on a curve.
            PenAnchor {
                at: Point::new(80.0, 0.0),
                out: Vec2::new(0.0, -12.0),
                back: Vec2::ZERO,
                radius: 0.0,
            },
            PenAnchor::corner(Point::new(120.0, 20.0)),
        ];

        for closed in [false, true] {
            let drawn = pen_outline(&anchors, closed);
            let read = pen_anchors(&drawn, &[]);
            assert_eq!(read.len(), 1, "one `MoveTo` is one subpath");
            assert_eq!(read[0].closed, closed);
            assert_eq!(
                read[0].anchors.len(),
                anchors.len(),
                "closed={closed}: the closing segment is not an anchor of its own"
            );
            for (want, got) in anchors.iter().zip(read[0].anchors.iter()) {
                assert_eq!((want.at, want.out, want.back), (got.at, got.out, got.back));
            }
            assert_same_path(&drawn, &pen_path(&read));
        }
    }

    /// The reader has to cope with paths the pen never wrote: **quads** and
    /// **several subpaths**, either of which a flattened boolean or an import can
    /// hold. Dropping a subpath here would silently delete artwork the moment a
    /// point edit committed.
    ///
    /// A quad is the one place the round trip is not byte-identical — it comes
    /// back a cubic — so the assertion is that it is the *same curve*, elevated
    /// exactly the way kurbo elevates it.
    #[test]
    fn reading_a_general_path_elevates_quads_and_keeps_every_subpath() {
        use ondin_core::kurbo::QuadBez;

        let mut path = BezPath::new();
        // Subpath 1: a line, then a quad, left open.
        path.move_to((0.0, 0.0));
        path.line_to((10.0, 0.0));
        path.quad_to((20.0, 10.0), (30.0, 0.0));
        // Subpath 2: a closed triangle whose `ClosePath` implies the last edge —
        // no repeated point to fold.
        path.move_to((100.0, 0.0));
        path.line_to((140.0, 0.0));
        path.line_to((140.0, 40.0));
        path.close_path();

        let read = pen_anchors(&path, &[]);
        assert_eq!(read.len(), 2, "two `MoveTo`s are two subpaths");
        assert!(!read[0].closed);
        assert_eq!(read[0].anchors.len(), 3);
        assert!(read[1].closed);
        assert_eq!(
            read[1].anchors.len(),
            3,
            "an implied closing edge adds no anchor"
        );

        // The quad's endpoints became one anchor pair, elevated to the cubic with
        // the same curve — kurbo's own elevation, so this cannot drift from it.
        let raised = QuadBez::new((10.0, 0.0), (20.0, 10.0), (30.0, 0.0)).raise();
        let (a, b) = (&read[0].anchors[1], &read[0].anchors[2]);
        assert_eq!(a.leaving(), raised.p1);
        assert_eq!(b.arriving(), raised.p2);
        assert!(a.back.hypot() < 1e-9, "a line arrived at it");

        // Writing back: same shape, with the quad now a cubic.
        let out = pen_path(&read);
        let mut want = BezPath::new();
        want.move_to((0.0, 0.0));
        want.line_to((10.0, 0.0));
        want.curve_to(raised.p1, raised.p2, raised.p3);
        want.move_to((100.0, 0.0));
        want.line_to((140.0, 0.0));
        want.line_to((140.0, 40.0));
        // The edge `ClosePath` implied comes back explicit, and still a line.
        want.line_to((100.0, 0.0));
        want.close_path();
        assert_same_path(&out, &want);
    }

    /// Reversing a subpath is what lets the pen resume from a path's **first**
    /// anchor: it only ever appends, so the run has to be turned around. The
    /// handles have to swap with it or every curve flips inside out.
    #[test]
    fn reversing_a_subpath_draws_the_same_curve_backwards() {
        use ondin_core::kurbo::Vec2;

        let mut sub = PenSubpath {
            anchors: vec![
                PenAnchor::smooth(Point::new(0.0, 0.0), Vec2::new(10.0, 10.0)),
                PenAnchor::smooth(Point::new(60.0, 0.0), Vec2::new(10.0, -10.0)),
            ],
            closed: false,
        };
        let forward = pen_path(std::slice::from_ref(&sub));
        sub.reverse();
        let back = pen_path(std::slice::from_ref(&sub));

        // One cubic, run the other way: the endpoints trade places and so do the
        // two control points.
        let mut want = BezPath::new();
        want.move_to((60.0, 0.0));
        want.curve_to((50.0, 10.0), (10.0, 10.0), (0.0, 0.0));
        assert_same_path(&back, &want);

        // And it is an involution, which is what makes it safe to apply on the
        // way in without recording that it happened.
        sub.reverse();
        assert_same_path(&pen_path(std::slice::from_ref(&sub)), &forward);
    }

    /// **Rewriting a resumed path must not move the node.** `create_path` puts
    /// the origin on the first anchor, which is right for a new node and wrong
    /// here: extending the *start* of a path would shift its transform, and with
    /// it every number the inspector shows. So the edit goes back through the
    /// node's own world transform and the transform itself is left alone.
    #[test]
    fn rewriting_a_path_leaves_its_transform_and_its_other_subpaths_alone() {
        let mut ids = IdSource::new(21);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
            },
            transform: Some(Affine::translate((17.0, 23.0))),
            name: None,
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let pw = res.world_transform(ab).unwrap();

        // A two-anchor path, created the ordinary way so its origin sits on its
        // first anchor — the arrangement the rewrite must not disturb.
        let drawn = [
            PenAnchor::corner(Point::new(50.0, 60.0)),
            PenAnchor::corner(Point::new(150.0, 60.0)),
        ];
        let id = ids.mint();
        doc.apply(&create_path(
            id,
            ab,
            0,
            pw,
            &drawn,
            false,
            Color::BLACK,
            2.0,
        ))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let before = doc.get(id).unwrap().transform();
        let node_world = res.world_transform(id).unwrap();

        // Resume it at its *first* anchor: read it back, reverse, extend.
        let NodeKind::Path { path, .. } = doc.get(id).unwrap().kind() else {
            panic!("expected a path");
        };
        let current = path.clone();
        let mut subpaths = pen_anchors(&current, &[]);
        map_subpaths(&mut subpaths, node_world);
        // A second subpath nobody is editing, to prove it survives.
        subpaths.push(PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(200.0, 200.0)),
                PenAnchor::corner(Point::new(260.0, 200.0)),
            ],
            closed: true,
        });

        // Resuming and changing nothing is not an edit — including when the grab
        // reversed the run, which is why `finish_pen` turns it back before this
        // sees it.
        let mut untouched = subpaths[0].clone();
        untouched.reverse();
        untouched.reverse();
        let idle = PenSubpath {
            anchors: untouched.anchors,
            closed: false,
        };
        let mut idle_all = subpaths.clone();
        idle_all[0] = idle;
        assert!(
            rewrite_path(id, node_world, &idle_all[..1], Some((&current, &[])))
                .0
                .is_empty(),
            "a resume that changed nothing must not become an undo step"
        );

        // Now the real edit: the pen reverses the run, appends, and reverses back,
        // so extending the *start* prepends rather than flipping the path.
        let mut working = subpaths[0].clone();
        working.reverse();
        working
            .anchors
            .push(PenAnchor::corner(Point::new(50.0, 140.0)));
        working.reverse();
        subpaths[0] = working;

        doc.apply(&rewrite_path(
            id,
            node_world,
            &subpaths,
            Some((&current, &[])),
        ))
        .unwrap();
        let res = Resolved::rebuild(&doc);

        assert_eq!(
            doc.get(id).unwrap().transform(),
            before,
            "extending the start of a path must not move the node"
        );
        let NodeKind::Path { path, .. } = doc.get(id).unwrap().kind() else {
            panic!("expected a path");
        };
        let mut world = path.clone();
        world.apply_affine(res.world_transform(id).unwrap());
        let read = pen_anchors(&world, &[]);
        assert_eq!(read.len(), 2, "the untouched subpath is still there");
        let ends: Vec<Point> = read[0].anchors.iter().map(|a| a.at).collect();
        assert_eq!(
            ends,
            vec![
                Point::new(50.0, 140.0),
                Point::new(50.0, 60.0),
                Point::new(150.0, 60.0)
            ],
            "extending the start prepends, leaving the path's direction alone"
        );
        assert!(read[1].closed);
        assert_eq!(read[1].anchors[0].at, Point::new(200.0, 200.0));
    }

    /// **Moving a point bends its neighbours and leaves the point itself alone.**
    /// The handles are offsets, so they ride along for free — and that is the
    /// behaviour, not an accident of the representation: dragging an anchor must
    /// not change how the curve *leaves* it, only where it leaves from.
    #[test]
    fn moving_a_point_carries_its_handles_and_leaves_its_neighbours_shaped() {
        use ondin_core::kurbo::Vec2;
        let mut subs = vec![PenSubpath {
            anchors: vec![
                PenAnchor::smooth(Point::new(0.0, 0.0), Vec2::new(10.0, 0.0)),
                PenAnchor::smooth(Point::new(50.0, 0.0), Vec2::new(10.0, 10.0)),
                PenAnchor::smooth(Point::new(100.0, 0.0), Vec2::new(10.0, 0.0)),
            ],
            closed: false,
        }];
        let before = subs[0].anchors[0].leaving();

        move_points(&mut subs, &[(0, 1)], Vec2::new(5.0, -20.0));

        let moved = subs[0].anchors[1];
        assert_eq!(moved.at, Point::new(55.0, -20.0));
        assert_eq!(
            (moved.out, moved.back),
            (Vec2::new(10.0, 10.0), Vec2::new(-10.0, -10.0)),
            "the handles are offsets, so the point keeps its shape"
        );
        assert_eq!(
            moved.leaving(),
            Point::new(65.0, -10.0),
            "and the control points travelled with it"
        );
        assert_eq!(
            subs[0].anchors[0].leaving(),
            before,
            "the neighbour did not move, so its own handle must not have"
        );

        // An index the path does not have is ignored rather than panicking: a
        // stale `PointSet` read one frame late must not take the app down.
        move_points(&mut subs, &[(9, 9), (0, 9)], Vec2::new(1.0, 1.0));
    }

    /// **Delete heals.** Dropping an anchor leaves its neighbours adjacent and the
    /// curve running between them through the handles they already have — which
    /// falls out of the representation rather than needing to be built, and is
    /// the argument for it being the default rather than a modifier.
    #[test]
    fn deleting_a_point_heals_the_path_and_drops_a_subpath_that_cannot_draw() {
        use ondin_core::kurbo::Vec2;
        let subs = vec![
            PenSubpath {
                anchors: vec![
                    PenAnchor::corner(Point::new(0.0, 0.0)),
                    PenAnchor::smooth(Point::new(50.0, 40.0), Vec2::new(10.0, 0.0)),
                    PenAnchor::smooth(Point::new(100.0, 0.0), Vec2::new(0.0, -10.0)),
                    PenAnchor::corner(Point::new(150.0, 0.0)),
                ],
                closed: false,
            },
            // Two anchors: removing one leaves a point, which draws nothing.
            PenSubpath {
                anchors: vec![
                    PenAnchor::corner(Point::new(200.0, 0.0)),
                    PenAnchor::corner(Point::new(240.0, 0.0)),
                ],
                closed: false,
            },
        ];

        let left = remove_points(&subs, &[(0, 1), (1, 0)]);
        assert_eq!(left.len(), 1, "the two-anchor subpath is gone, not emptied");
        assert_eq!(subpath_lengths(&left), vec![3]);

        // The healed segment runs from the anchor before the deleted one to the
        // anchor after it, through both of *their* handles — the survivor's own
        // incoming handle is what shapes it now.
        let healed = pen_path(&left);
        let PathEl::CurveTo(c1, c2, end) = healed.elements()[1] else {
            panic!("the heal should curve: {:?}", healed.elements());
        };
        assert_eq!(c1, Point::new(0.0, 0.0), "a corner leaves straight");
        assert_eq!(
            c2,
            Point::new(100.0, 10.0),
            "into the survivor's own handle"
        );
        assert_eq!(end, Point::new(100.0, 0.0));

        // Removing everything leaves nothing — the caller deletes the node rather
        // than committing a path with no ink.
        assert!(remove_points(&subs, &[(0, 0), (0, 1), (0, 2), (0, 3), (1, 0), (1, 1)]).is_empty());
    }

    /// **Retracting a handle onto its anchor is how a smooth point becomes a
    /// corner**, and it needs the snap: `is_corner` wants both handles at zero to
    /// better than 1e-9, which no hand can drag to. The snap is a substitution
    /// rather than a branch, so the mirror does the second half for free.
    #[test]
    fn a_handle_dragged_near_its_anchor_retracts_and_makes_a_corner() {
        use ondin_core::kurbo::Vec2;
        let at = Point::new(50.0, 50.0);
        let mut subs = vec![PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(0.0, 0.0)),
                PenAnchor::smooth(at, Vec2::new(20.0, 0.0)),
                PenAnchor::corner(Point::new(100.0, 0.0)),
            ],
            closed: false,
        }];

        // Just outside the radius: an ordinary reshape, still a smooth point.
        pull_handle(
            &mut subs,
            (0, 1),
            Side::Out,
            at + Vec2::new(8.0, 0.0),
            false,
            7.0,
            0.0,
        );
        assert!(!subs[0].anchors[1].is_corner());
        assert_eq!(subs[0].anchors[1].out, Vec2::new(8.0, 0.0));

        // Inside it: both handles collapse, so the segments either side go
        // straight — which is the visible result, not just the predicate.
        pull_handle(
            &mut subs,
            (0, 1),
            Side::Out,
            at + Vec2::new(5.0, 2.0),
            false,
            7.0,
            0.0,
        );
        assert!(subs[0].anchors[1].is_corner());
        let path = pen_path(&subs);
        assert!(
            path.elements()[1..]
                .iter()
                .all(|el| matches!(el, PathEl::LineTo(_))),
            "a retracted anchor between two corners leaves two lines: {:?}",
            path.elements()
        );

        // `Alt` still breaks the pair, so a *one-sided* retract stays sayable —
        // the segment in arrives on a curve and the one out leaves straight.
        let mut broken = vec![PenSubpath {
            anchors: vec![PenAnchor::smooth(at, Vec2::new(20.0, 0.0))],
            closed: false,
        }];
        pull_handle(&mut broken, (0, 0), Side::Out, at, true, 7.0, 0.0);
        assert_eq!(broken[0].anchors[0].out, Vec2::ZERO);
        assert_eq!(
            broken[0].anchors[0].back,
            Vec2::new(-20.0, 0.0),
            "the incoming handle is untouched"
        );
        assert!(!broken[0].anchors[0].is_corner());
    }

    /// Align and distribute over **points**, where the box is the one the points
    /// themselves occupy — there is no key and no container to choose between,
    /// which is the whole difference from the layer versions.
    #[test]
    fn aligning_and_distributing_points_works_on_their_own_extent() {
        use ondin_core::kurbo::Vec2;
        let mut subs = vec![PenSubpath {
            anchors: vec![
                PenAnchor::smooth(Point::new(0.0, 10.0), Vec2::new(5.0, 5.0)),
                PenAnchor::corner(Point::new(30.0, 40.0)),
                PenAnchor::corner(Point::new(90.0, 70.0)),
                // Not selected, so nothing below may touch it.
                PenAnchor::corner(Point::new(45.0, 200.0)),
            ],
            closed: false,
        }];
        let picked = [(0, 0), (0, 1), (0, 2)];

        align_points(&mut subs, &picked, Axis::Y, Edge::Min);
        let ys: Vec<f64> = subs[0].anchors.iter().map(|a| a.at.y).collect();
        assert_eq!(
            ys,
            vec![10.0, 10.0, 10.0, 200.0],
            "to the topmost, and only the three"
        );
        assert_eq!(
            subs[0].anchors[0].leaving(),
            Point::new(5.0, 15.0),
            "the handles travelled with the anchor"
        );

        // X is untouched by a Y align, so distribute has something to space.
        distribute_points(&mut subs, &picked, Axis::X);
        let xs: Vec<f64> = subs[0].anchors.iter().map(|a| a.at.x).collect();
        assert_eq!(
            xs,
            vec![0.0, 45.0, 90.0, 45.0],
            "evenly between the two extremes, which stay put"
        );

        // Two points cannot be distributed — both are extremes — and one cannot
        // be aligned. Neither is an error; both are a no-op.
        let before: Vec<Point> = subs[0].anchors.iter().map(|a| a.at).collect();
        distribute_points(&mut subs, &picked[..2], Axis::X);
        align_points(&mut subs, &picked[..1], Axis::X, Edge::Max);
        assert_eq!(
            subs[0].anchors.iter().map(|a| a.at).collect::<Vec<_>>(),
            before
        );
    }

    /// **`rotate_points` turns the shape, not the vertices** (§15 D617,
    /// `[S13.2-L6-05]`, and §15 D123, whose rule this is).
    ///
    /// 🚨 **It had zero test callers.** One production call site
    /// (the Node tool's rotate arm in `canvas.rs`) and nothing in any
    /// `#[cfg(test)]` module or any file
    /// under `crates/*/tests/`. `rotate_about_pivot_moves_points_correctly` is
    /// named for it and exercises `turn_about`/`rotate_selection` instead. Its
    /// neighbours `align_points` and `distribute_points` are covered by the test
    /// directly above, which is the contrast that makes this a gap rather than a
    /// policy.
    ///
    /// **The decision under test is D123's**: each handle is carried through the
    /// transform *via the control point it describes* rather than by rotating the
    /// offset, **so a zero-length handle stays at zero rather than at a rounding
    /// error**. That is why the fixture has a corner anchor in it with no handles
    /// at all — a version that rotated the offsets would leave it at `(0, 0)` plus
    /// float dust, which no assertion on the *anchor* can see.
    ///
    /// ⚠️ **A quarter turn is exact and that is the point**: `Affine::rotate` at
    /// `FRAC_PI_2` gives clean ±1 and ~1e-17 zeros, so the assertions can be tight
    /// enough to catch a handle that turned by the wrong amount rather than merely
    /// turned. The pivot is off-origin so a version that forgot to translate back
    /// fails on the anchor.
    ///
    /// **Flip-check, run**: `rotate_points` reduced to `a.at = t * a.at` alone —
    /// the review's own flip, which was green across 972 tests — fails at *"the
    /// handle turned with its anchor"* with `out` still `(5, 0)` where it must be
    /// `(0, 5)`. The predicted site; the anchor assertion above it stays green
    /// under the flip, which is what says the vertices were never the interesting
    /// half. ⚠️ The *expected* handle position had to be corrected once against
    /// the run — `Affine::rotate(FRAC_PI_2)` sends `+x` to `+y`, so `leaving()` is
    /// `(10, 25)` and not the `(5, 20)` first written, which is where a −90° turn
    /// would put it.
    #[test]
    fn rotating_points_carries_their_handles_round_with_them() {
        use ondin_core::kurbo::Vec2;
        let mut subs = vec![PenSubpath {
            anchors: vec![
                // A handle pointing along +x, so a quarter turn must aim it +y.
                PenAnchor::smooth(Point::new(20.0, 10.0), Vec2::new(5.0, 0.0)),
                // No handles at all — D123's zero-length case.
                PenAnchor::corner(Point::new(30.0, 10.0)),
                // Not selected, so nothing here may touch it.
                PenAnchor::corner(Point::new(50.0, 50.0)),
            ],
            closed: false,
        }];
        let picked = [(0, 0), (0, 1)];
        let pivot = Point::new(10.0, 10.0);

        rotate_points(&mut subs, &picked, pivot, std::f64::consts::FRAC_PI_2);

        let near = |a: Point, b: Point| (a - b).hypot() < 1e-9;
        assert!(
            near(subs[0].anchors[0].at, Point::new(10.0, 20.0)),
            "the anchor turned about the pivot: {:?}",
            subs[0].anchors[0].at
        );
        assert!(
            near(subs[0].anchors[0].leaving(), Point::new(10.0, 25.0)),
            "the handle turned with its anchor: {:?}",
            subs[0].anchors[0].out
        );
        assert!(
            (subs[0].anchors[0].out.hypot() - 5.0).abs() < 1e-9,
            "and kept its length: {}",
            subs[0].anchors[0].out.hypot()
        );
        // D123's case: a corner's zero handles are still exactly zero.
        assert_eq!(
            (subs[0].anchors[1].out, subs[0].anchors[1].back),
            (Vec2::ZERO, Vec2::ZERO),
            "a zero-length handle must stay at zero rather than at a rounding error"
        );
        assert_eq!(
            subs[0].anchors[2].at,
            Point::new(50.0, 50.0),
            "and the unselected anchor did not move"
        );
    }

    /// **`space_points` holds the first point on the axis** (§15 D617,
    /// `[S13.2-L6-05]`, and §15 D243, whose rule this is).
    ///
    /// 🚨 **It had zero test callers either**, and its one load-bearing decision
    /// is exactly the one an implementation gets backwards: the run is laid out
    /// from the **first on that axis**, so the leftmost point stays put and the
    /// others come to it. D243 lifted `points_along` out *"so the two point
    /// arrangements cannot disagree about which point is first"*, and nothing
    /// asserted which one that is.
    ///
    /// **0 / 10 / 40 at a gap of 5 gives 0 / 5 / 10 and not 30 / 35 / 40** — the
    /// finding's own fixture, chosen because the two answers are disjoint. A
    /// fixture whose points were already evenly spaced would give the same list
    /// under both.
    ///
    /// ⚠️ **The selection is given in a deliberately scrambled order**, because
    /// `points_along` sorts and the guarantee is about the *axis* rather than
    /// about the order the user clicked in. Passing them left-to-right would let a
    /// version that simply walked `points` pass.
    ///
    /// **Flip-check, run**: `order` reversed — the review's own flip, green across
    /// 972 tests — fails at the predicted site with **`[50.0, 45.0, 40.0, 90.0]`**.
    /// ⚠️ The finding predicted `[30, 35, 40]`, and this write-up copied it: that
    /// is what "held at the last point" would give if the run were also laid out
    /// *backwards*. It is not — the gap still accumulates forwards from `start`,
    /// so reversing the order moves the whole run to the far side of the anchor it
    /// held. **A flip's failing value is worth reading even when its site was
    /// predicted correctly.**
    #[test]
    fn spacing_points_holds_the_first_one_along_the_axis() {
        let mut subs = vec![PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(0.0, 0.0)),
                PenAnchor::corner(Point::new(10.0, 0.0)),
                PenAnchor::corner(Point::new(40.0, 0.0)),
                // Not selected.
                PenAnchor::corner(Point::new(90.0, 0.0)),
            ],
            closed: false,
        }];
        // Scrambled: the axis decides the order, not this list.
        let picked = [(0, 2), (0, 0), (0, 1)];

        space_points(&mut subs, &picked, Axis::X, 5.0);

        let xs: Vec<f64> = subs[0].anchors.iter().map(|a| a.at.x).collect();
        assert_eq!(
            xs,
            vec![0.0, 5.0, 10.0, 90.0],
            "the leftmost stays and the rest come to it, five apart"
        );

        // Fewer than two points, and a gap that is not a number, are both no-ops
        // rather than errors — the guard at the top of the function.
        let before = xs;
        space_points(&mut subs, &picked[..1], Axis::X, 5.0);
        space_points(&mut subs, &picked, Axis::X, f64::NAN);
        assert_eq!(
            subs[0].anchors.iter().map(|a| a.at.x).collect::<Vec<_>>(),
            before
        );
    }

    /// **Making a corner smooth has two cases and only one of them invents
    /// anything** (§15 D250).
    ///
    /// An anchor that already carries a handle is *mirrored* — the longer handle
    /// is the one that was aimed on purpose — and an anchor with none has its
    /// tangent derived from its neighbours. The second is where a wrong answer
    /// hides, because both produce a curve and only one produces the *right* one.
    ///
    /// ⚠️ Flipped three ways, and each is a spelling somebody reaches for.
    /// Deriving the tangent from one leg (`next − at`) rather than from the chord
    /// (`next − prev`) is the obvious reading of "point along the path": here it
    /// answers a handle straight down where the through-tangent leans, so the curve
    /// leaves flat and then has to turn twice. Taking the **longer** leg's third
    /// overshoots the short side, which the unequal legs below are chosen to expose.
    /// And giving each handle a third of *its own* leg — the Catmull-Rom answer, and
    /// the better curve — leaves `is_smooth()` false, which is the assertion below
    /// that matters most: the app defines a smooth anchor as an exactly mirrored
    /// pair, so an asymmetric one comes apart on the next handle drag.
    #[test]
    fn making_a_point_smooth_mirrors_a_handle_or_derives_one_from_the_neighbours() {
        use ondin_core::kurbo::Vec2;
        // A right-angle corner between two legs of *different* lengths: 10 across,
        // 20 down. Equal legs would hide a handle length taken from the wrong one.
        let mut subs = vec![PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(0.0, 0.0)),
                PenAnchor::corner(Point::new(10.0, 0.0)),
                PenAnchor::corner(Point::new(10.0, 20.0)),
            ],
            closed: false,
        }];
        set_smoothness(&mut subs, &[(0, 1)], true);
        let mid = subs[0].anchors[1];
        assert!(
            mid.is_smooth(),
            "the pair has to mirror *exactly*, or the next drag breaks it: {mid:?}"
        );
        // The through-tangent is (10, 20) − (0, 0) = (10, 20) at the *anchor*
        // between them — i.e. `next − prev` — normalized, at a third of the
        // shorter leg (10, not 20).
        let dir = Vec2::new(10.0, 20.0) / Vec2::new(10.0, 20.0).hypot();
        let want_out = dir * (10.0 / 3.0);
        assert!(
            (mid.out - want_out).hypot() < 1e-9,
            "out {:?} wanted {want_out:?}",
            mid.out
        );
        // The neighbours were not touched.
        assert_eq!(subs[0].anchors[0].out, Vec2::ZERO);
        assert_eq!(subs[0].anchors[2].out, Vec2::ZERO);

        // A **broken** pair mirrors instead, keeping the longer handle: no
        // invention, because the shaping is already there to read.
        let mut broken = vec![PenSubpath {
            anchors: vec![PenAnchor {
                at: Point::ZERO,
                out: Vec2::new(9.0, 0.0),
                back: Vec2::new(0.0, 2.0),
                radius: 4.0,
            }],
            closed: false,
        }];
        set_smoothness(&mut broken, &[(0, 0)], true);
        let a = broken[0].anchors[0];
        assert_eq!(a.out, Vec2::new(9.0, 0.0), "the longer handle survives");
        assert_eq!(a.back, Vec2::new(-9.0, 0.0), "and the other mirrors it");
        assert_eq!(
            a.radius, 4.0,
            "the radius is left alone — it already draws nothing on a smooth anchor"
        );

        // And back to a corner is both handles gone, radius still untouched.
        set_smoothness(&mut broken, &[(0, 0)], false);
        let a = broken[0].anchors[0];
        assert_eq!((a.out, a.back), (Vec2::ZERO, Vec2::ZERO));
        assert_eq!(a.radius, 4.0);
    }

    /// **An endpoint of an open subpath gets one handle, not two** (§15 D250): it
    /// has one segment, so a mirrored handle would be a control over a curve that
    /// does not exist.
    ///
    /// The consequence is that it does not report `is_smooth()` afterwards, which
    /// is asserted here rather than left to be discovered — an endpoint is not a
    /// joint between two segments, and smoothness is a property of a joint.
    ///
    /// Closing the same subpath gives the same anchor two neighbours and it takes
    /// the two-handle path, which is the assertion that this is about *ends* rather
    /// than about first and last indices.
    ///
    /// ⚠️ **Both branches of `smoothed_handles` are driven, and until §15 D560 only
    /// one was.** The corner fixtures reach the derive-from-neighbours arms, whose
    /// endpoint cases were correct all along; the two shaped endpoints at the
    /// bottom reach the already-shaped arm, which mirrored before it asked whether
    /// the anchor was an end. **The flip is deleting either of that arm's two
    /// one-sided cases**, and each of them bites only at its own end — which is why
    /// the fixture is built twice rather than once with a loop over indices.
    #[test]
    fn an_open_subpaths_endpoint_smooths_on_the_side_it_has() {
        use ondin_core::kurbo::Vec2;
        let anchors = vec![
            PenAnchor::corner(Point::new(0.0, 0.0)),
            PenAnchor::corner(Point::new(30.0, 0.0)),
            PenAnchor::corner(Point::new(30.0, 30.0)),
        ];
        let mut open = vec![PenSubpath {
            anchors: anchors.clone(),
            closed: false,
        }];
        set_smoothness(&mut open, &[(0, 0)], true);
        let end = open[0].anchors[0];
        assert_eq!(end.back, Vec2::ZERO, "there is no segment behind it");
        assert_eq!(end.out, Vec2::new(10.0, 0.0), "a third of its one leg");
        assert!(!end.is_smooth(), "and an end is not a joint");

        // Closed: the same index now has a neighbour on both sides.
        let mut closed = vec![PenSubpath {
            anchors,
            closed: true,
        }];
        set_smoothness(&mut closed, &[(0, 0)], true);
        let joint = closed[0].anchors[0];
        assert!(
            joint.is_smooth() && joint.back.hypot() > 0.0,
            "a closed subpath has no ends: {joint:?}"
        );

        // A lone anchor has nothing to derive from and is left exactly as it was.
        let mut lone = vec![PenSubpath {
            anchors: vec![PenAnchor::corner(Point::new(5.0, 5.0))],
            closed: false,
        }];
        set_smoothness(&mut lone, &[(0, 0)], true);
        assert_eq!(lone[0].anchors[0].at, Point::new(5.0, 5.0));
        assert_eq!(lone[0].anchors[0].out, Vec2::ZERO);

        // ⚠️ **The case this test was named for and did not construct** — §15
        // D560. Every anchor above is a `PenAnchor::corner`, so the endpoint has
        // no handle and takes the derive-from-neighbours branch; the *"side it
        // has"* clause was answered by the one arm the rule was never broken in.
        // An endpoint that already carries a handle — an SVG import beginning
        // `M 0 0 C …`, or a pen path whose first click was a shaping drag — went
        // through the already-shaped branch instead, which mirrors before it asks
        // whether the anchor is an end.
        let mut shaped = vec![PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(0.0, 0.0)),
                PenAnchor::corner(Point::new(30.0, 0.0)),
            ],
            closed: false,
        }];
        shaped[0].anchors[0].out = Vec2::new(10.0, 0.0);
        // The fixture is in the state the assertion is about: one handle, on the
        // side the segment is on.
        assert_eq!(shaped[0].anchors[0].back, Vec2::ZERO);
        set_smoothness(&mut shaped, &[(0, 0)], true);
        let end = shaped[0].anchors[0];
        assert_eq!(
            end.out,
            Vec2::new(10.0, 0.0),
            "the handle that was aimed on purpose is kept"
        );
        assert_eq!(
            end.back,
            Vec2::ZERO,
            "and no second one is invented over a segment that does not exist"
        );
        assert!(!end.is_smooth(), "an end is still not a joint");

        // The same at the *other* end, where the missing side is `out` — because
        // the two arms of the endpoint rule are separate lines and only one of
        // them would fail a one-ended test.
        let mut tail = vec![PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(0.0, 0.0)),
                PenAnchor::corner(Point::new(30.0, 0.0)),
            ],
            closed: false,
        }];
        tail[0].anchors[1].back = Vec2::new(-10.0, 0.0);
        assert_eq!(tail[0].anchors[1].out, Vec2::ZERO);
        set_smoothness(&mut tail, &[(0, 1)], true);
        let end = tail[0].anchors[1];
        assert_eq!((end.back, end.out), (Vec2::new(-10.0, 0.0), Vec2::ZERO));
        assert!(!end.is_smooth());
    }

    /// **The app and core must number a path's anchors alike, or a radius set on
    /// one corner appears on another.**
    ///
    /// `pen_anchors` and `geometry::anchor_runs` are two walks of the same
    /// `BezPath` written for different purposes — one carries handles for
    /// editing, the other only what the fillet needs — and the model's
    /// `corner_radii` is indexed by *both*: the app writes it and core reads it.
    /// Nothing in the type system ties the two together, so this does. The
    /// failure it guards is silent and looks like a rendering bug.
    #[test]
    fn the_app_and_core_number_a_paths_anchors_alike() {
        let mut cases: Vec<BezPath> = Vec::new();
        // A closed run written the way `pen_outline` writes one — the repeated
        // closing point is the case the two walks could disagree about.
        let mut a = BezPath::new();
        a.move_to((0.0, 0.0));
        a.line_to((10.0, 0.0));
        a.curve_to((15.0, 5.0), (15.0, 5.0), (10.0, 10.0));
        a.line_to((0.0, 0.0));
        a.close_path();
        cases.push(a);
        // A `ClosePath` with an *implied* final edge — nothing to fold.
        let mut b = BezPath::new();
        b.move_to((0.0, 0.0));
        b.line_to((10.0, 0.0));
        b.line_to((10.0, 10.0));
        b.close_path();
        cases.push(b);
        // Several subpaths, a quad, and an open run — the numbering has to run
        // across all of them in the same order.
        let mut c = BezPath::new();
        c.move_to((0.0, 0.0));
        c.quad_to((5.0, 5.0), (10.0, 0.0));
        c.move_to((50.0, 50.0));
        c.line_to((60.0, 50.0));
        c.line_to((60.0, 60.0));
        c.close_path();
        cases.push(c);
        cases.push(BezPath::new());

        for path in &cases {
            let subs = pen_anchors(path, &[]);
            let runs = ondin_core::geometry::anchor_runs(path);
            let mine: Vec<Point> = subs
                .iter()
                .flat_map(|s| s.anchors.iter().map(|a| a.at))
                .collect();
            let theirs: Vec<Point> = runs.iter().flat_map(|r| r.points.clone()).collect();
            assert_eq!(mine, theirs, "for {:?}", path.elements());
            assert_eq!(mine.len(), ondin_core::geometry::anchor_count(path));

            // **The *segments* have to agree too**, because that numbering is what
            // names a segment for an insert, a removal or a drag
            // (`geometry::nearest_segment` answers in it and `segment_ends` acts on
            // it). A run carries one segment per anchor when closed and one fewer
            // when open, each *leaving* the anchor it is indexed by — so a
            // disagreement here would put an inserted point on the wrong side of a
            // corner, and nothing else would notice.
            assert_eq!(subs.len(), runs.len(), "for {:?}", path.elements());
            for (i, (sub, run)) in subs.iter().zip(runs.iter()).enumerate() {
                let want = match sub.closed {
                    true => sub.anchors.len(),
                    false => sub.anchors.len().saturating_sub(1),
                };
                assert_eq!(run.segs.len(), want, "subpath {i} segment count");
                for j in 0..run.segs.len() {
                    let (from, to) =
                        segment_ends(&subs, (i, j)).unwrap_or_else(|| panic!("segment {i}.{j}"));
                    assert_eq!(from, (i, j), "a segment is named by the anchor it leaves");
                    let seg = run.segs[j];
                    assert_eq!(seg.start(), sub.anchors[from.1].at, "subpath {i} seg {j}");
                    assert_eq!(seg.end(), sub.anchors[to.1].at, "subpath {i} seg {j}");
                }
                // One past the last segment names nothing — which for an open run is
                // its *final anchor*, the index that has an anchor but no segment
                // leaving it.
                assert!(
                    segment_ends(&subs, (i, run.segs.len())).is_none(),
                    "subpath {i} claims a segment it does not have"
                );
            }
        }

        // And the radii come back out in the order they went in, which is the
        // property the model actually depends on.
        let mut subs = pen_anchors(&cases[0], &[0.0, 4.0, 7.0]);
        assert_eq!(subs[0].anchors[1].radius, 4.0);
        assert_eq!(subs[0].anchors[2].radius, 7.0);
        assert_eq!(pen_radii(&subs), vec![0.0, 4.0, 7.0]);
        // Trailing zeros are trimmed, so an unrounded path stores nothing at all
        // and two equal shapes cannot serialize differently (invariant 9).
        subs[0].anchors[1].radius = 0.0;
        subs[0].anchors[2].radius = 0.0;
        assert!(pen_radii(&subs).is_empty());
    }

    #[test]
    fn create_line_endpoints_map_to_world() {
        let mut ids = IdSource::new(3);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let ab = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: ab,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 400.0),
            },
            transform: Some(Affine::translate((5.0, 7.0))),
            name: None,
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let pw = res.world_transform(ab).unwrap();

        let line = ids.mint();
        let (a, b) = (Point::new(20.0, 30.0), Point::new(120.0, 80.0));
        doc.apply(&create_line(line, ab, 0, pw, a, b, Color::BLACK, 2.0))
            .unwrap();
        let res = Resolved::rebuild(&doc);
        let world = res.world_transform(line).unwrap();
        let NodeKind::Line { end } = doc.get(line).unwrap().kind() else {
            panic!("expected a line");
        };
        // Node origin maps to world `a`; local `end` maps to world `b`.
        let w_start = world * Point::ZERO;
        let w_end = world * *end;
        assert!((w_start.x - a.x).abs() < 1e-9 && (w_start.y - a.y).abs() < 1e-9);
        assert!((w_end.x - b.x).abs() < 1e-9 && (w_end.y - b.y).abs() < 1e-9);
    }

    #[test]
    fn rotate_about_pivot_moves_points_correctly() {
        let (mut doc, res, rect) = artboard_with_rect();
        // Rotate 90° about the rect's world origin (100,100). The rect's local
        // (50,0) point sits at world (150,100); after +90° about (100,100) it
        // should land at (100,150).
        let tx = rotate_node(
            &doc,
            &res,
            rect,
            Point::new(100.0, 100.0),
            std::f64::consts::FRAC_PI_2,
        );
        doc.apply(&tx).unwrap();
        let res2 = Resolved::rebuild(&doc);
        let w = res2.world_transform(rect).unwrap();
        let p = w * Point::new(50.0, 0.0);
        assert!((p.x - 100.0).abs() < 1e-6, "x={}", p.x);
        assert!((p.y - 150.0).abs() < 1e-6, "y={}", p.y);
    }

    /// **A rotation inside a *skewed* group leaves no scale in the transform**
    /// (`[S13.1-L2-04]`, §15 D577).
    ///
    /// `P⁻¹ · R · P` is a rotation only when `P` is conformal. Under a leaning
    /// parent it is a general unimodular matrix, and `rotate_node` was the one
    /// transform-writing gesture in this file that wrote it straight into
    /// `SetTransform` — so the child stored a scale, against §5.6's *"it does not
    /// carry scale in normal editing"*.
    ///
    /// ⚠️ **The pixels were right the whole time, so the assertion is on the stored
    /// basis and not on where anything lands.** The split reproduces the world
    /// placement exactly; what was wrong was the *number*, and it surfaced on the
    /// next gesture as a 45% jump in the W/H fields. The world-placement assertion
    /// below is therefore a **control** — it must stay green under the flip, or this
    /// test is about a rotation that stopped working rather than about where the
    /// scale went.
    ///
    /// **The unskewed control is the second half.** Without it a `split_geometry_scale`
    /// that always returned `(1, 1)` would pass, and so would a build where
    /// `rotate_node` had stopped rotating.
    ///
    /// **Flip run**, the split removed and `new_local` written straight into
    /// `SetTransform`: fails on *"a rotation under a skewed parent stores no
    /// scale"* at `(0.6197, 1.6138)`, with **both** world-placement controls green
    /// in the same run — which is the shape of the whole finding, a wrong number
    /// under a right picture. ⚠️ **Not the finding's `(1.4547, 0.6875)`**: this
    /// fixture leans by a unit shear and turns 30° about the rect's own centre where
    /// the finding turned 30° about the group's, so the residual is a different pair.
    /// The two numbers are the same fact and neither is a constant to check against.
    #[test]
    fn a_rotation_under_a_skewed_parent_leaves_the_scale_in_the_geometry() {
        let scale_after = |lean: f64| {
            let mut ids = IdSource::new(0x5CE);
            let root = ids.mint();
            let mut doc = Document::new(root);
            let (group, rect) = (ids.mint(), ids.mint());
            doc.apply(&Transaction(vec![
                Operation::CreateNode {
                    id: group,
                    parent: root,
                    index: 0,
                    kind: NodeKind::Group,
                    // A 45° lean when `lean` is 1.0, and no lean at all at 0.0 —
                    // the same matrix `skew_to_handle` writes for `Handle::Top`.
                    transform: Some(Affine::new([1.0, 0.0, lean, 1.0, 0.0, 0.0])),
                    name: None,
                },
                Operation::CreateNode {
                    id: rect,
                    parent: group,
                    index: 0,
                    kind: NodeKind::Rect {
                        size: Size::new(100.0, 100.0),
                        corner_radii: RoundedRectRadii::default(),
                    },
                    transform: None,
                    name: None,
                },
            ]))
            .expect("the fixture");
            let res = Resolved::rebuild(&doc);
            let before = res.world_transform(rect).expect("a world basis");
            let probe = before * Point::new(100.0, 0.0);

            let pivot = Point::new(50.0, 50.0);
            let angle = std::f64::consts::FRAC_PI_6;
            let tx = rotate_node(&doc, &res, rect, pivot, angle);
            doc.apply(&tx).expect("the rotation applies");
            let res = Resolved::rebuild(&doc);

            // Where the rect's top-right **corner** actually landed, against where
            // the rotation promised to put it — the control.
            //
            // ⚠️ **Read through the node's own `size`, not through the local point
            // `(100, 0)`.** The split moves scale out of the transform and into the
            // geometry, so after it the same local coordinate names a different
            // place: the first version of this assertion compared `(100, 0)` before
            // and after and failed by **61 units** with the fix working perfectly.
            // The corner is the thing that has to stay put; its local coordinate is
            // not.
            let node = doc.get(rect).expect("a live node");
            let after = res.world_transform(rect).expect("a world basis");
            let NodeKind::Rect { size, .. } = node.kind() else {
                panic!("a rect");
            };
            let want = turn_about(pivot, angle) * probe;
            let got = after * Point::new(size.width, 0.0);
            (Basis::of(node.transform()).scale, (got - want).hypot())
        };

        let (skewed, moved) = scale_after(1.0);
        assert!(
            moved < 1e-6,
            "the control: the rotation still puts the point where it promised, \
             off by {moved}"
        );
        assert!(
            (skewed.x - 1.0).abs() < 1e-6 && (skewed.y - 1.0).abs() < 1e-6,
            "a rotation under a skewed parent stores no scale: {skewed:?}"
        );

        let (upright, moved) = scale_after(0.0);
        assert!(moved < 1e-6, "the upright control's placement: {moved}");
        assert!(
            (upright.x - 1.0).abs() < 1e-6 && (upright.y - 1.0).abs() < 1e-6,
            "and neither does one under an unskewed parent — which is the arm that \
             always worked: {upright:?}"
        );
    }

    // --- transforming a whole selection ------------------------------------

    /// Two 20×20 squares in one frame, at x = 0 and x = 80 — so the union of
    /// their world bounds is 100×20, the same box the group tests use, but with
    /// the two layers *siblings* rather than members of a group.
    fn two_squares() -> (Document, Resolved, NodeId, NodeId) {
        let mut ids = IdSource::new(0x5E1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (ab, a, b) = (ids.mint(), ids.mint(), ids.mint());
        let square = || NodeKind::Rect {
            size: Size::new(20.0, 20.0),
            corner_radii: RoundedRectRadii::default(),
        };
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 400.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: a,
                parent: ab,
                index: 0,
                kind: square(),
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: b,
                parent: ab,
                index: 1,
                kind: square(),
                transform: Some(Affine::translate((80.0, 0.0))),
                name: None,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, a, b)
    }

    fn union_of(res: &Resolved, ids: &[NodeId]) -> Rect {
        ids.iter()
            .filter_map(|id| res.world_bounds(*id))
            .reduce(|x, y| x.union(y))
            .unwrap()
    }

    /// **A selection scales like a group would, without being one.** Both the
    /// spacing between the members and each member's own geometry take the
    /// factor, so the union box ends up exactly where the handle was dragged.
    #[test]
    fn resizing_a_selection_scales_its_members_and_the_gaps_between_them() {
        let (mut doc, res, a, b) = two_squares();
        let roots = [a, b];
        let union = union_of(&res, &roots);
        assert_eq!((union.width(), union.height()), (100.0, 20.0));

        // Bottom-right corner dragged to double the width, height unchanged.
        doc.apply(&resize_selection(
            &doc,
            &res,
            &roots,
            SelectionBox::upright(union),
            Handle::BottomRight,
            Point::new(200.0, 20.0),
            Resize::geometry(false, false),
        ))
        .unwrap();

        let res = Resolved::rebuild(&doc);
        // The same numbers `resize_group` produces for the identical box: each
        // square 40 wide, the far one pushed out to 160.
        for (id, want_x, want_w) in [(a, 0.0, 40.0), (b, 160.0, 40.0)] {
            let NodeKind::Rect { size, .. } = doc.get(id).unwrap().kind() else {
                panic!("not a rect");
            };
            assert!(
                (size.width - want_w).abs() < 1e-9 && (size.height - 20.0).abs() < 1e-9,
                "{id:?} geometry {size:?}, want {want_w}x20"
            );
            let origin = res.world_transform(id).unwrap() * Point::ZERO;
            assert!(
                (origin.x - want_x).abs() < 1e-9,
                "{id:?} at {origin:?}, want x {want_x}"
            );
        }
        let after = union_of(&res, &roots);
        assert!(
            (after.width() - 200.0).abs() < 1e-9 && (after.height() - 20.0).abs() < 1e-9,
            "the union box came out {after:?}"
        );
    }

    /// The anchor is the far side, so the dragged corner is the only one that
    /// moves — a resize must not slide the whole selection across the page.
    #[test]
    fn resizing_a_selection_holds_the_opposite_corner_still() {
        let (mut doc, res, a, b) = two_squares();
        let roots = [a, b];
        let union = union_of(&res, &roots);
        doc.apply(&resize_selection(
            &doc,
            &res,
            &roots,
            SelectionBox::upright(union),
            Handle::BottomRight,
            Point::new(50.0, 10.0),
            Resize::geometry(false, false),
        ))
        .unwrap();
        let after = union_of(&Resolved::rebuild(&doc), &roots);
        assert!(
            (after.min_x() - union.min_x()).abs() < 1e-9
                && (after.min_y() - union.min_y()).abs() < 1e-9,
            "the held corner moved: {union:?} -> {after:?}"
        );
    }

    /// **A selection turns about its own centre, not each member's.** The layers
    /// hold their arrangement and sweep round together, which is the whole point
    /// of a shared box: a quarter turn about the union centre swaps the box's
    /// width and height and carries both squares round with it.
    #[test]
    fn rotating_a_selection_turns_it_about_the_shared_centre() {
        let (mut doc, res, a, b) = two_squares();
        let roots = [a, b];
        let union = union_of(&res, &roots);
        let pivot = union.center();

        doc.apply(&rotate_selection(
            &doc,
            &res,
            &roots,
            pivot,
            std::f64::consts::FRAC_PI_2,
        ))
        .unwrap();

        let res = Resolved::rebuild(&doc);
        let after = union_of(&res, &roots);
        // 100×20 becomes 20×100, still centred on the same point.
        assert!(
            (after.width() - 20.0).abs() < 1e-9 && (after.height() - 100.0).abs() < 1e-9,
            "the box did not turn: {after:?}"
        );
        assert!(
            (after.center().x - pivot.x).abs() < 1e-9 && (after.center().y - pivot.y).abs() < 1e-9,
            "the selection drifted off its pivot: {:?} -> {:?}",
            pivot,
            after.center()
        );
        // Each square kept its size — a rotation is not a resize.
        for id in roots {
            let NodeKind::Rect { size, .. } = doc.get(id).unwrap().kind() else {
                panic!("not a rect");
            };
            assert_eq!((size.width, size.height), (20.0, 20.0));
        }
        // And they swapped places along the new long axis rather than landing on
        // top of each other, which is what rotating each about its own centre
        // would have done.
        let (ca, cb) = (
            res.world_bounds(a).unwrap().center(),
            res.world_bounds(b).unwrap().center(),
        );
        assert!(
            (ca.y - cb.y).abs() > 50.0,
            "the two squares did not separate along y: {ca:?} {cb:?}"
        );
    }

    /// A member turned off-axis under a non-uniform scale is the case that used
    /// to be approximated: the honest answer is a shear, the model had nowhere
    /// to put one, and the member kept its shape on a geometric-mean scale
    /// instead (§15 D50). It now leans, and the whole selection lands **exactly**
    /// where the box says.
    ///
    /// Worth pinning for a *selection* specifically, because unrelated layers at
    /// odd angles is exactly what a selection is, where inside a group it is the
    /// exception. The assertion is on the resulting world bounds rather than on
    /// any one node's fields, since the split is free to divide a given result
    /// between transform and geometry however it likes — what must not move is
    /// the picture.
    #[test]
    fn a_selection_member_at_an_odd_angle_leans_instead_of_evening_out() {
        let (mut doc, _, a, b) = two_squares();
        // Turn one square 30° about its own origin.
        doc.apply(&Transaction(vec![Operation::SetTransform {
            id: b,
            transform: Affine::translate((80.0, 0.0)) * Affine::rotate(30f64.to_radians()),
        }]))
        .unwrap();
        let res2 = Resolved::rebuild(&doc);
        let roots = [a, b];
        let union = union_of(&res2, &roots);
        let before = res2.world_bounds(b).unwrap();

        doc.apply(&resize_selection(
            &doc,
            &res2,
            &roots,
            SelectionBox::upright(union),
            Handle::BottomRight,
            // Wider only: a thoroughly non-uniform scale.
            Point::new(union.max_x() * 3.0, union.max_y()),
            Resize::geometry(false, false),
        ))
        .unwrap();
        let after = Resolved::rebuild(&doc);

        // The turned member's own box, stretched 3× in x about the union's held
        // corner and left alone in y — which is what the handle asked for, and
        // what the old geometric mean could only approximate.
        let hold = Point::new(union.min_x(), union.min_y());
        let want = Rect::new(
            hold.x + (before.min_x() - hold.x) * 3.0,
            before.min_y(),
            hold.x + (before.max_x() - hold.x) * 3.0,
            before.max_y(),
        );
        let got = after.world_bounds(b).unwrap();
        for (g, w) in [
            (got.min_x(), want.min_x()),
            (got.min_y(), want.min_y()),
            (got.max_x(), want.max_x()),
            (got.max_y(), want.max_y()),
        ] {
            assert!(
                (g - w).abs() < 1e-6,
                "the turned member did not follow the box: {got:?} vs {want:?}"
            );
        }

        // And it really is leaning now, rather than having been stretched along
        // its own axes: a 30° square under a 3:1 squash is not square to itself
        // any more.
        let skew = build::Orientation::of(after.world_transform(b).unwrap()).skew;
        assert!(
            skew.abs() > 1f64.to_radians(),
            "the turned member came out unsheared: {}°",
            skew.to_degrees()
        );

        // The axis-aligned one took the scale as pure geometry and stayed
        // upright, so the two really were treated differently.
        let NodeKind::Rect { size: flat, .. } = doc.get(a).unwrap().kind() else {
            panic!("not a rect");
        };
        assert!(
            flat.width > flat.height * 1.5,
            "the axis-aligned member should have stretched: {flat:?}"
        );
        assert!(
            build::Orientation::of(after.world_transform(a).unwrap())
                .skew
                .abs()
                < 1e-9,
            "an axis-aligned member must not pick up a lean"
        );
    }

    /// Ctrl-dragging a side leans the box and holds the side facing it still.
    /// The held edge staying put is the whole feel of the gesture — a skew that
    /// slides the whole shape sideways reads as a broken move.
    #[test]
    fn skewing_a_side_holds_the_one_facing_it() {
        let (mut doc, res, a, _) = two_squares();
        let before = res.world_bounds(a).unwrap();

        // Drag the top side to the right by the box's own height, which is a
        // 45° lean.
        doc.apply(&skew_to_handle(
            &doc,
            &res,
            a,
            Rect::new(0.0, 0.0, 20.0, 20.0),
            Handle::Top,
            // Pressed at the side's midpoint and dragged 20 right — the *delta* is
            // what leans it now (§15 D322), so the press has to be stated. It was
            // implicit in the pointer alone while the lean was measured from the
            // box centre, which is the bug that entry is about.
            SideDrag {
                press: Point::new(before.center().x, before.min_y()),
                at: Point::new(before.center().x + 20.0, before.min_y()),
            },
            false,
        ))
        .unwrap();
        let after = Resolved::rebuild(&doc);

        // −45, not 45: `skewX` displaces +x in proportion to +y and y points
        // down, so dragging the *top* to the right is the negative lean. The
        // sign is pinned deliberately — it is what the inspector's Skew field
        // shows and what the SVG writer emits, and the three have to agree.
        let o = build::Orientation::of(after.world_transform(a).unwrap());
        assert!(
            (o.skew.to_degrees() + 45.0).abs() < 1e-6,
            "a top side dragged one height right is a −45° lean, not {}°",
            o.skew.to_degrees()
        );
        // The bottom edge has not moved; the top has.
        let got = after.world_bounds(a).unwrap();
        assert!(
            (got.max_y() - before.max_y()).abs() < 1e-6,
            "the held side moved: {before:?} -> {got:?}"
        );
        assert!(
            got.width() > before.width() + 10.0,
            "the box did not lean at all: {before:?} -> {got:?}"
        );
    }

    /// **Grabbing a side away from its midpoint must not lean anything until the
    /// pointer moves** (§15 D322) — the reported bug, in the reported numbers.
    ///
    /// It was measured against the box **centre**, so the first frame of a drag
    /// jumped to whatever lean the grab point already implied. Reported as *"the
    /// moment I click and do a very small drag, it jumps to −26.76°"* on a 200×120
    /// rectangle grabbed near the right end of its top edge — and −26.76° is
    /// `atan(60.5 / -120)` to two decimals, which puts the grab 80.3% along the
    /// edge. That arithmetic is the whole of the diagnosis, so it is the fixture.
    ///
    /// Three points, because the bug is entirely about the *first* frame:
    ///
    /// - **Pressed and not moved** is a no-op. A gesture that leans a shape before
    ///   the pointer has travelled is the symptom itself.
    /// - **A 1-unit drag** leans by `atan(1/120)`, about half a degree. This is what
    ///   pins the scale as well as the origin: an implementation that measured the
    ///   delta against the wrong span would pass the assertion above and fail here.
    /// - **A 60.5-unit drag** — the offset the old code was reading off the grab
    ///   alone — reaches the reported −26.76°. So the angle is still available; it
    ///   now takes a drag rather than a click, which is the entire fix.
    ///
    /// ⚠️ Flipped by restoring `p.x - c.x` in place of `p.x - press.x`: the no-op
    /// case leans −26.76° with the pointer stationary, reproducing the screenshot.
    #[test]
    fn a_side_grabbed_off_centre_does_not_lean_until_the_pointer_moves() {
        // The reported rectangle, at the origin for arithmetic that can be read.
        let mut ids = IdSource::new(7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(200.0, 120.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        }]))
        .expect("the reported rectangle");
        let res = Resolved::rebuild(&doc);
        let box_ = Rect::new(0.0, 0.0, 200.0, 120.0);
        // 80.3% along the top edge — 60.5 right of the centre, which is what
        // −26.76° decodes to on a 120-tall box.
        let press = Point::new(160.5, 0.0);

        let lean_after = |dx: f64| {
            let tx = skew_to_handle(
                &doc,
                &res,
                id,
                box_,
                Handle::Top,
                SideDrag {
                    press,
                    at: Point::new(press.x + dx, press.y),
                },
                false,
            );
            let mut d = doc.clone();
            if !tx.0.is_empty() {
                d.apply(&tx).expect("the skew applies");
            }
            let r = Resolved::rebuild(&d);
            build::Orientation::of(r.world_transform(id).unwrap())
                .skew
                .to_degrees()
        };

        assert!(
            lean_after(0.0).abs() < 1e-9,
            "pressing without moving leant it {}° — this is the reported jump",
            lean_after(0.0)
        );

        let small = lean_after(1.0);
        let want = (1.0f64 / -120.0).atan().to_degrees();
        assert!(
            (small - want).abs() < 1e-6,
            "a 1-unit drag should lean {want}°, got {small}°"
        );

        let full = lean_after(60.5);
        assert!(
            (full + 26.76).abs() < 0.01,
            "dragging the old grab-offset should still reach the reported −26.76°, \
             got {full}° — the angle is not gone, it just takes a drag now"
        );
    }

    /// **A selection leans as one parallelogram, exactly, whatever its members
    /// are turned to.** A member at an odd angle needs a scale as well as a
    /// shear to follow a world-axis lean, and the split gives it one.
    ///
    /// So the comparison is on where the *shape* lands, not on the transform:
    /// the split is free to divide the result between the matrix and the
    /// geometry however it likes, and on a turned member it does — its transform
    /// comes back with a unit basis and the scale in its size. What has to hold
    /// is that the map from each member's unit square to the world is the old
    /// one with the shared shear in front of it.
    #[test]
    fn a_leaning_selection_carries_a_turned_member_exactly() {
        let (mut doc, _, a, b) = two_squares();
        doc.apply(&Transaction(vec![Operation::SetTransform {
            id: b,
            transform: Affine::translate((80.0, 0.0)) * Affine::rotate(37f64.to_radians()),
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let roots = [a, b];
        let union = union_of(&res, &roots);
        // Unit square → world, which is the whole of where a member's shape is.
        let placement = |d: &Document, r: &Resolved, id: NodeId| {
            let NodeKind::Rect { size, .. } = d.get(id).unwrap().kind() else {
                panic!("not a rect")
            };
            r.world_transform(id).unwrap() * Affine::scale_non_uniform(size.width, size.height)
        };
        let before: Vec<Affine> = roots.iter().map(|id| placement(&doc, &res, *id)).collect();

        // Drag the top side right by the union's own height: a 45° lean about
        // the bottom edge.
        // The shear is derived once and handed over (§15 D324), which is also what
        // the chrome is given — so this drives the same pair of steps the canvas
        // does rather than a shortcut through one function.
        let shear = side_shear(
            union,
            Handle::Top,
            SideDrag {
                press: Point::new(union.center().x, union.min_y()),
                at: Point::new(union.center().x + union.height(), union.min_y()),
            },
            false,
        )
        .expect("a lean this box can take");
        doc.apply(&skew_selection(&doc, &res, &roots, shear))
            .unwrap();
        let after = Resolved::rebuild(&doc);

        // Negative: `skewX` displaces +x in proportion to +y, and y points down,
        // so pushing the *top* right is the lean that pulls the bottom left.
        let want_shear = Affine::translate((0.0, union.max_y()))
            * build::skew_x(-45f64.to_radians())
            * Affine::translate((0.0, -union.max_y()));
        for (id, was) in roots.iter().zip(before) {
            let want = want_shear * was;
            let got = placement(&doc, &after, *id);
            for (g, w) in got.as_coeffs().iter().zip(want.as_coeffs().iter()) {
                assert!(
                    (g - w).abs() < 1e-6,
                    "member landed off the shared shear: {:?} vs {:?}",
                    got.as_coeffs(),
                    want.as_coeffs()
                );
            }
        }
    }

    /// A lean is a lean, not a resize: the shape's area survives it. This is what
    /// separates the gesture from dragging the same band without Ctrl, and it is
    /// the property a wrong pivot or a doubled shear quietly destroys.
    #[test]
    fn a_lean_does_not_change_how_much_shape_there_is() {
        let (mut doc, res, a, _) = two_squares();
        let area = |d: &Document| {
            let NodeKind::Rect { size, .. } = d.get(a).unwrap().kind() else {
                panic!("not a rect")
            };
            let det = d.get(a).unwrap().transform().determinant().abs();
            size.width * size.height * det
        };
        let before = area(&doc);

        doc.apply(&skew_to_handle(
            &doc,
            &res,
            a,
            Rect::new(0.0, 0.0, 20.0, 20.0),
            Handle::Right,
            // A vertical side dragged down leans the other way round. Pressed at
            // the side's midpoint, dragged 20 down.
            SideDrag {
                press: Point::new(
                    res.world_bounds(a).unwrap().max_x(),
                    res.world_bounds(a).unwrap().center().y,
                ),
                at: Point::new(res.world_bounds(a).unwrap().max_x(), 30.0),
            },
            false,
        ))
        .unwrap();

        assert!(
            (area(&doc) - before).abs() < 1e-6,
            "the lean changed the area: {before} -> {}",
            area(&doc)
        );
    }

    // --- the Scale tool ----------------------------------------------------

    /// A rounded, stroked, dotted rect under an artboard, for scaling.
    fn artboard_with_a_decorated_rect() -> (Document, Resolved, NodeId) {
        let (mut doc, _, rect) = artboard_with_rect();
        doc.apply(&Transaction(vec![
            Operation::SetGeometry {
                id: rect,
                geometry: GeometryPatch::CornerRadii(RoundedRectRadii::new(4.0, 4.0, 8.0, 8.0)),
            },
            Operation::SetStrokes {
                id: rect,
                strokes: vec![Stroke {
                    width: 2.0,
                    dashes: vec![0.0, 6.0],
                    dash_offset: 3.0,
                    sides: ondin_core::StrokeSides::Custom {
                        top: 1.0,
                        right: 0.0,
                        bottom: 3.0,
                        left: 2.0,
                    },
                    ..Default::default()
                }],
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, rect)
    }

    fn radii(doc: &Document, id: NodeId) -> RoundedRectRadii {
        let NodeKind::Rect { corner_radii, .. } = doc.get(id).unwrap().kind() else {
            panic!("not a rect");
        };
        *corner_radii
    }

    /// **The whole point of the tool, and it is not only about strokes.** A
    /// photographic scale takes the three things a §5.6 resize deliberately leaves
    /// alone — stroke width, corner radii and font size — and a resize still
    /// leaves all three exactly where they were.
    ///
    /// Asserted as a pair, because either half alone would pass with the flag
    /// plumbed through and never read.
    #[test]
    fn scaling_takes_the_stroke_radii_and_type_that_a_resize_leaves_alone() {
        for (scaling, want) in [(Scaling::Geometry, 2.0), (Scaling::Photographic, 4.0)] {
            let (mut doc, res, rect) = artboard_with_a_decorated_rect();
            // 50 -> 100 on both axes: a clean doubling, so the mean is exactly 2.
            doc.apply(&resize_to_handle(
                &doc,
                &res,
                rect,
                Handle::BottomRight,
                Point::new(200.0, 200.0),
                Resize {
                    scaling,
                    ..Default::default()
                },
            ))
            .unwrap();

            let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
                panic!()
            };
            assert_eq!(size, &Size::new(100.0, 100.0), "{scaling:?}: the box");
            let stroke = &doc.get(rect).unwrap().paint().strokes[0];
            assert_eq!(stroke.width, want, "{scaling:?}: stroke width");
            // The factor the scalars took: 1 for a resize, 2 for a scale.
            let s = want / 2.0;
            // Each corner scales from its **own** radius rather than being
            // re-derived from the new box, so an unevenly rounded rect stays
            // unevenly rounded in the same proportion.
            assert_eq!(radii(&doc, rect).top_left, 4.0 * s, "{scaling:?}: top-left");
            assert_eq!(
                radii(&doc, rect).bottom_right,
                8.0 * s,
                "{scaling:?}: bottom-right"
            );
            // A dash pattern is a set of lengths along the outline, so it travels
            // with the shape: a scaled-up dotted border whose spacing stayed put
            // would come back as a solid line.
            assert_eq!(stroke.dashes, vec![0.0, 6.0 * s], "{scaling:?}: dashes");
            assert_eq!(stroke.dash_offset, 3.0 * s, "{scaling:?}: dash offset");
            // Per-side widths are widths. A side of 0 stays 0, so "no stroke on
            // this side" keeps meaning that at every size.
            assert_eq!(
                stroke.sides,
                ondin_core::StrokeSides::Custom {
                    top: s,
                    right: 0.0,
                    bottom: 3.0 * s,
                    left: 2.0 * s,
                },
                "{scaling:?}: per-side widths"
            );
        }
    }

    /// A square corner has to stay square. Its radius is 0, so scaling it is a
    /// no-op arithmetically — but emitting the operation anyway would put a
    /// `CornerRadii` patch in the undo stack for every scale of every plain
    /// rectangle in the document.
    #[test]
    fn scaling_a_square_cornered_rect_writes_no_radius_operation() {
        let (doc, res, rect) = artboard_with_rect();
        let tx = resize_to_handle(
            &doc,
            &res,
            rect,
            Handle::BottomRight,
            Point::new(200.0, 200.0),
            Resize {
                scaling: Scaling::Photographic,
                ..Default::default()
            },
        );
        assert!(
            !tx.0.iter().any(|op| matches!(
                op,
                Operation::SetGeometry {
                    geometry: GeometryPatch::CornerRadii(_),
                    ..
                }
            )),
            "a square corner needs no radius patch: {tx:?}"
        );
        // Nor a stroke operation, on a layer with no strokes.
        assert!(
            !tx.0
                .iter()
                .any(|op| matches!(op, Operation::SetStrokes { .. })),
            "an unstroked layer needs no stroke patch: {tx:?}"
        );
    }

    /// A stroke width has no non-uniform reading — one width, two factors — so a
    /// squashed shape scales its scalars by the **geometric mean**, which is the
    /// choice that degenerates to exactly `s` under a uniform scale.
    #[test]
    fn a_non_uniform_scale_moves_the_scalars_by_the_geometric_mean() {
        let (mut doc, res, rect) = artboard_with_a_decorated_rect();
        // 50x50 -> 200x50: sx = 4, sy = 1, so the mean is 2.
        doc.apply(&resize_to_handle(
            &doc,
            &res,
            rect,
            Handle::BottomRight,
            Point::new(300.0, 150.0),
            Resize {
                scaling: Scaling::Photographic,
                ..Default::default()
            },
        ))
        .unwrap();
        let NodeKind::Rect { size, .. } = doc.get(rect).unwrap().kind() else {
            panic!()
        };
        assert_eq!(size, &Size::new(200.0, 50.0), "the box took both factors");
        assert_eq!(
            doc.get(rect).unwrap().paint().strokes[0].width,
            4.0,
            "sqrt(4 * 1) = 2, so a 2px stroke becomes 4px"
        );
    }

    /// Scaling a **group** carries the treatment down to every leaf, which is the
    /// case the tool exists for: an icon is a group, and scaling it has to take
    /// its hairlines with it.
    #[test]
    fn scaling_a_group_scales_its_childrens_strokes_too() {
        let (mut doc, _, rect) = artboard_with_a_decorated_rect();
        let parent = doc.get(rect).unwrap().parent().expect("artboard");
        let mut ids = IdSource::new(99);
        let group = ids.mint();
        // Wrap the rect in a group, so the scale has to recurse to reach it.
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: group,
            parent,
            index: 1,
            kind: NodeKind::Group,
            transform: None,
            name: None,
        }]))
        .unwrap();
        doc.apply(&Transaction(vec![Operation::Reparent {
            id: rect,
            new_parent: group,
            index: 0,
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let local = ondin_core::local_box(&doc, &res, group).expect("group box");

        doc.apply(&resize_group(
            &doc,
            &res,
            group,
            local,
            Handle::BottomRight,
            Point::new(local.max_x() * 2.0, local.max_y() * 2.0),
            Resize {
                scaling: Scaling::Photographic,
                ..Default::default()
            },
        ))
        .unwrap();
        assert!(
            doc.get(rect).unwrap().paint().strokes[0].width > 2.5,
            "the child's stroke stayed at {}",
            doc.get(rect).unwrap().paint().strokes[0].width
        );
    }

    /// **Auto-sized text takes the font size but never a box.** The early return in
    /// `scale_geometry` is about its box, which does not exist; its type size does,
    /// and a scaled icon with a label in it would otherwise come out with the label
    /// at its original size.
    ///
    /// This is the pairing that makes it a real assertion: the size moves *and* the
    /// sizing mode does not, which is the §5.6 rule the resize path holds.
    #[test]
    fn scaling_auto_sized_text_moves_its_type_size_but_not_its_sizing_mode() {
        let (mut doc, res, group, text) = group_with_auto_text();
        let local = ondin_core::local_box(&doc, &res, group).expect("group box");
        doc.apply(&resize_group(
            &doc,
            &res,
            group,
            local,
            Handle::BottomRight,
            Point::new(local.max_x() * 2.0, local.max_y() * 2.0),
            Resize {
                scaling: Scaling::Photographic,
                ..Default::default()
            },
        ))
        .unwrap();
        let NodeKind::Text { sizing, style, .. } = doc.get(text).unwrap().kind() else {
            panic!("not text")
        };
        assert!(
            matches!(sizing, TextSizing::Auto),
            "auto text must keep its sizing mode: {sizing:?}"
        );
        assert!(
            style.font_size > 16.5,
            "the type size stayed at {}",
            style.font_size
        );
    }

    /// A group with an auto-sized text layer and a rect beside it, so the group has
    /// a box to scale.
    fn group_with_auto_text() -> (Document, Resolved, NodeId, NodeId) {
        let mut ids = IdSource::new(7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let group = ids.mint();
        let text = ids.mint();
        let rect = ids.mint();
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: NodeKind::Group,
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: text,
                parent: group,
                index: 0,
                kind: NodeKind::Text {
                    content: "Label".into(),
                    style: Box::new(TextStyle {
                        font_family: "Inter".into(),
                        font_size: 16.0,
                        weight: 400,
                        italic: false,
                        line_height: Some(ondin_core::Length::Em(1.2)),
                        ..Default::default()
                    }),
                    spans: Default::default(),
                    para_spans: Default::default(),
                    paragraph: Default::default(),
                    block: Default::default(),
                    sizing: TextSizing::Auto,
                    on_path: None,
                    on_path_flip: false,
                    on_path_offset: 0.0,
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: rect,
                parent: group,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(100.0, 100.0),
                    corner_radii: RoundedRectRadii::default(),
                },
                transform: Some(Affine::translate((0.0, 100.0))),
                name: None,
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, group, text)
    }

    /// A scale that is not a scale writes nothing. `Scaling::scalar` is the guard,
    /// and without it every *move* of a selection — which runs the same split and
    /// comes back with factors a few ulps off 1 — would drag a stroke rewrite into
    /// the undo stack behind it.
    #[test]
    fn a_scale_of_one_writes_no_scalar_operations() {
        let (doc, res, rect) = artboard_with_a_decorated_rect();
        // The handle dropped exactly where it started.
        let tx = resize_to_handle(
            &doc,
            &res,
            rect,
            Handle::BottomRight,
            Point::new(150.0, 150.0),
            Resize {
                scaling: Scaling::Photographic,
                ..Default::default()
            },
        );
        assert!(
            !tx.0
                .iter()
                .any(|op| matches!(op, Operation::SetStrokes { .. })),
            "an unchanged box must not rewrite the strokes: {tx:?}"
        );
    }

    // --- segments: insert, remove, move, bend ------------------------------

    /// A closed run with one curved side, as `pen_anchors` would hand it over.
    fn quad_with_a_curve() -> Vec<PenSubpath> {
        use ondin_core::kurbo::Vec2;
        vec![PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(0.0, 0.0)),
                PenAnchor::corner(Point::new(100.0, 0.0)),
                // Arrives straight, leaves on a curve — the shape D114 added the
                // second handle for, and one an insert can easily get wrong.
                PenAnchor {
                    at: Point::new(100.0, 100.0),
                    out: Vec2::new(-40.0, 20.0),
                    back: Vec2::ZERO,
                    radius: 0.0,
                },
                PenAnchor {
                    at: Point::new(0.0, 100.0),
                    out: Vec2::ZERO,
                    back: Vec2::new(40.0, 20.0),
                    radius: 0.0,
                },
            ],
            closed: true,
        }]
    }

    /// Assert that two paths trace the same shape, **independently of how either
    /// one is cut into segments**.
    ///
    /// `assert_same_path` cannot answer this: splitting a segment changes the
    /// element list on purpose. Nor can sampling "fraction `u` of the whole path"
    /// on both, which is what this started as — five segments and four segments put
    /// the same `u` in different places, so it failed on a split that was perfectly
    /// correct. Every sample of each path landing *on* the other is the property
    /// actually wanted, and `nearest_segment` is how to ask it.
    ///
    /// The bound is the *query's* accuracy rather than the split's: a nearest-point
    /// solve on a 100-unit curve settles a few parts in a million out, which is
    /// nowhere near a device pixel and nowhere near the tens of units a
    /// wrongly-written handle would be off by.
    fn assert_same_shape(a: &BezPath, b: &BezPath) {
        for (from, onto, which) in [(a, b, "a→b"), (b, a, "b→a")] {
            for (i, seg) in from.segments().enumerate() {
                for f in 0..=8 {
                    let p = seg.eval(f as f64 / 8.0);
                    let hit = ondin_core::geometry::nearest_segment(onto, p)
                        .expect("the other path has segments");
                    assert!(
                        hit.distance < 1e-3,
                        "{which}: segment {i} at t={} is {:.6} off the other path",
                        f as f64 / 8.0,
                        hit.distance
                    );
                }
            }
        }
    }

    /// **Inserting a point does not change the shape**, which is the whole
    /// contract: the two halves come off `CubicBez::subsegment`, so the outline
    /// through the new anchor is the outline that was already drawn.
    ///
    /// Sampled rather than compared element by element, because the split *does*
    /// change the representation — one cubic becomes two — and must not change the
    /// geometry. The round trip through `pen_anchors` is asserted beside it, which
    /// is the test CLAUDE.md names for this class of edit: a handle written to the
    /// wrong side of an anchor shows up there and nowhere else.
    #[test]
    fn inserting_a_point_splits_a_segment_without_moving_the_curve() {
        for (at, t) in [((0, 2), 0.5), ((0, 2), 0.25), ((0, 0), 0.5), ((0, 3), 0.5)] {
            let before = quad_with_a_curve();
            let mut after = before.clone();
            let new = insert_point(&mut after, at, t).expect("a segment to split");
            assert_eq!(new, (at.0, at.1 + 1), "the new anchor follows the old one");
            assert_eq!(after[0].anchors.len(), before[0].anchors.len() + 1);
            assert!(after[0].closed, "closure survives an insert");

            let (a, b) = (pen_path(&before), pen_path(&after));
            assert_same_shape(&a, &b);
            // And the trip through the model is stable, which is what a committed
            // insert actually does — read back, written out, byte for byte.
            assert_same_path(&b, &pen_path(&pen_anchors(&b, &[])));
        }
    }

    /// Splitting a **straight** segment gives two straight segments and a corner,
    /// not two cubics whose control points happen to sit on their endpoints. The
    /// reader gives the lines back either way, so the assertion is on what is
    /// written.
    #[test]
    fn splitting_a_line_leaves_two_lines_and_a_corner() {
        let mut subs = quad_with_a_curve();
        insert_point(&mut subs, (0, 0), 0.5).unwrap();
        assert!(subs[0].anchors[1].is_corner(), "the new point is a corner");
        assert_eq!(subs[0].anchors[1].at, Point::new(50.0, 0.0));
        let els = pen_path(&subs).elements().to_vec();
        assert!(
            matches!(els[1], PathEl::LineTo(_)) && matches!(els[2], PathEl::LineTo(_)),
            "{els:#?}"
        );
    }

    /// A radius rides on its anchor, so an insert cannot slide one onto the
    /// neighbouring corner — the failure D119 says reads as a rendering bug.
    #[test]
    fn an_insert_leaves_every_radius_on_the_corner_that_owned_it() {
        let mut subs = quad_with_a_curve();
        subs[0].anchors[0].radius = 3.0;
        subs[0].anchors[1].radius = 8.0;
        insert_point(&mut subs, (0, 0), 0.5).unwrap();
        let radii: Vec<f64> = subs[0].anchors.iter().map(|a| a.radius).collect();
        assert_eq!(
            radii,
            vec![3.0, 0.0, 8.0, 0.0, 0.0],
            "the new point is at 1"
        );
        assert_eq!(pen_radii(&subs), vec![3.0, 0.0, 8.0]);
    }

    /// **Removing a segment opens a closed run**, and the run comes back starting
    /// after the cut — including when the cut *is* the closing segment, which is
    /// the case that has no code of its own.
    #[test]
    fn removing_a_segment_opens_a_closed_run_at_the_cut() {
        use ondin_core::kurbo::Vec2;
        let subs = quad_with_a_curve();
        let corners: Vec<Point> = subs[0].anchors.iter().map(|a| a.at).collect();

        // Cut the segment leaving anchor 1: the run runs 2, 3, 0, 1.
        let left = remove_segments(&subs, &[(0, 1)]);
        assert_eq!(left.len(), 1);
        assert!(!left[0].closed, "a cut run cannot still be closed");
        let got: Vec<Point> = left[0].anchors.iter().map(|a| a.at).collect();
        assert_eq!(got, vec![corners[2], corners[3], corners[0], corners[1]]);
        // The handles that described the removed segment are gone: `pen_path`
        // writes neither of them, so leaving them set would be a value only the
        // app knew about — and it would reappear the moment the run was closed.
        assert_eq!(left[0].anchors[0].back, Vec2::ZERO);
        assert_eq!(left[0].anchors[3].out, Vec2::ZERO);

        // The closing segment: the same order, merely open.
        let closing = remove_segments(&subs, &[(0, 3)]);
        let got: Vec<Point> = closing[0].anchors.iter().map(|a| a.at).collect();
        assert_eq!(got, corners);
        assert!(!closing[0].closed);
    }

    /// On an **open** run it splits instead, and a remainder too short to draw is
    /// dropped — `remove_points`' rule, for the same reason.
    #[test]
    fn removing_a_segment_splits_an_open_run_and_drops_a_lone_point() {
        let mut subs = quad_with_a_curve();
        subs[0].closed = false;

        let two = remove_segments(&subs, &[(0, 1)]);
        assert_eq!(two.len(), 2, "one run became two");
        assert_eq!(two[0].anchors.len(), 2);
        assert_eq!(two[1].anchors.len(), 2);

        // Cutting the first segment leaves one point behind, which draws nothing.
        let one = remove_segments(&subs, &[(0, 0)]);
        assert_eq!(one.len(), 1);
        assert_eq!(one[0].anchors.len(), 3);

        // The last anchor of an open run has no segment leaving it, so nothing is
        // removed at all rather than something arbitrary being removed.
        assert!(segment_ends(&subs, (0, 3)).is_none());
        let none = remove_segments(&subs, &[(0, 3)]);
        assert_eq!(none.len(), 1);
        assert_eq!(none[0].anchors.len(), 4);

        // A two-point path loses its only segment and there is nothing left —
        // which is what tells the caller to delete the node.
        let pair = vec![PenSubpath {
            anchors: subs[0].anchors[..2].to_vec(),
            closed: false,
        }];
        assert!(remove_segments(&pair, &[(0, 0)]).is_empty());
    }

    /// A plain segment drag moves both its anchors and nothing else, so the curve
    /// between them keeps its shape exactly and the neighbours bend to follow.
    ///
    /// It is `move_points` over the ends `segment_ends` names, which is also what
    /// makes a *set* of segments move as one piece: two that adjoin share an anchor,
    /// and the caller's deduplicated union (`canvas::subject_points`) moves it once
    /// rather than twice.
    #[test]
    fn moving_a_segment_carries_both_its_anchors_and_no_others() {
        use ondin_core::kurbo::Vec2;
        let mut subs = quad_with_a_curve();
        let before = subs.clone();
        let delta = Vec2::new(10.0, -5.0);
        let (a, b) = segment_ends(&subs, (0, 2)).expect("a segment");
        move_points(&mut subs, &[a, b], delta);
        for i in 0..4 {
            let (a, b) = (before[0].anchors[i], subs[0].anchors[i]);
            let want = if i == 2 || i == 3 { a.at + delta } else { a.at };
            assert_eq!(b.at, want, "anchor {i}");
            assert_eq!(b.out, a.out, "anchor {i}: handles ride along unchanged");
            assert_eq!(b.back, a.back, "anchor {i}");
        }
    }

    /// **Several cuts in one run, and the closed case is the same walk.** Two cuts
    /// in a closed quad leave two open runs; the rotation that turns a closed run
    /// into an open one is where an off-by-one would live, so the anchor order is
    /// asserted rather than just the count.
    #[test]
    fn removing_several_segments_at_once_leaves_a_run_between_each_pair() {
        let subs = quad_with_a_curve();
        let p: Vec<Point> = subs[0].anchors.iter().map(|a| a.at).collect();

        // Closed, cut after anchor 0 and after anchor 2: runs 1–2 and 3–0.
        let two = remove_segments(&subs, &[(0, 0), (0, 2)]);
        assert_eq!(two.len(), 2);
        let got: Vec<Vec<Point>> = two
            .iter()
            .map(|s| s.anchors.iter().map(|a| a.at).collect())
            .collect();
        assert_eq!(got, vec![vec![p[1], p[2]], vec![p[3], p[0]]], "{got:?}");
        assert!(two.iter().all(|s| !s.closed));

        // Cutting every segment of a closed run leaves nothing that draws: four
        // lone points, all dropped, which is what deletes the node.
        assert!(remove_segments(&subs, &[(0, 0), (0, 1), (0, 2), (0, 3)]).is_empty());

        // An index naming no segment is ignored rather than shifting the others:
        // the last anchor of an *open* run has nothing leaving it.
        let mut open = subs.clone();
        open[0].closed = false;
        let one = remove_segments(&open, &[(0, 3), (0, 1)]);
        let sizes: Vec<usize> = one.iter().map(|s| s.anchors.len()).collect();
        assert_eq!(sizes, vec![2, 2], "only the real cut took");
    }

    /// **A bend puts the grabbed point exactly under the pointer** and leaves both
    /// anchors where they are. That is what the minimum-norm split buys, and it has
    /// to hold at any `t` rather than only at the midpoint.
    #[test]
    fn bending_a_segment_moves_the_grabbed_point_by_the_whole_delta() {
        use ondin_core::kurbo::Vec2;
        for t in [0.25, 0.5, 0.75] {
            // From the *straight* side, which is also the case that turns two
            // corners into curve anchors.
            let mut subs = quad_with_a_curve();
            let before = segment_point(&subs, (0, 0), t).unwrap();
            let ends = (subs[0].anchors[0].at, subs[0].anchors[1].at);
            let delta = Vec2::new(6.0, -30.0);
            bend_segment(&mut subs, (0, 0), t, delta);

            let after = segment_point(&subs, (0, 0), t).unwrap();
            assert!(
                (after - (before + delta)).hypot() < 1e-9,
                "t={t}: {after:?} vs {:?}",
                before + delta
            );
            assert_eq!(
                (subs[0].anchors[0].at, subs[0].anchors[1].at),
                ends,
                "t={t}: a bend moves no anchor"
            );
            assert!(!subs[0].anchors[0].is_corner(), "t={t}: it curves now");
            // The *other* side of each anchor is untouched, so the bend is local to
            // the segment it was asked about.
            assert_eq!(subs[0].anchors[0].back, Vec2::ZERO);
            assert_eq!(subs[0].anchors[1].out, Vec2::ZERO);
        }

        // At the ends the two control points have no leverage on the curve, so the
        // bend declines rather than diverging.
        let mut subs = quad_with_a_curve();
        let before = subs[0].anchors[0].out;
        bend_segment(&mut subs, (0, 0), 0.0, Vec2::new(10.0, 10.0));
        assert_eq!(subs[0].anchors[0].out, before);
    }

    /// The box is the selected **anchors**' extent, and a handle drag scales the
    /// set about the corner opposite the one held — `resize_factors`' own rule, so
    /// Shift and Alt mean here exactly what they mean on a layer.
    #[test]
    fn a_point_box_scales_the_selected_points_about_the_held_corner() {
        let subs = quad_with_a_curve();
        let all: Vec<PointRef> = (0..4).map(|i| (0, i)).collect();
        let box_ = points_box(&subs, &all).expect("four points make a box");
        assert_eq!(box_, Rect::new(0.0, 0.0, 100.0, 100.0));
        // One point is no box: there is nothing to scale it about.
        assert!(points_box(&subs, &[(0, 0)]).is_none());
        // Nor is a flat one — its four corners would land on the two points
        // themselves, which win the press, so every handle would be unreachable.
        assert!(points_box(&subs, &[(0, 0), (0, 1)]).is_none());

        // Drag the bottom-right corner out to twice the box: the top-left stays put
        // and everything else doubles away from it, handles included.
        let mut scaled = subs.clone();
        resize_points(
            &mut scaled,
            &all,
            box_,
            Handle::BottomRight,
            Point::new(200.0, 200.0),
            Resize::geometry(false, false),
        );
        assert_eq!(scaled[0].anchors[0].at, Point::new(0.0, 0.0));
        assert_eq!(scaled[0].anchors[2].at, Point::new(200.0, 200.0));
        assert_eq!(scaled[0].anchors[2].out, subs[0].anchors[2].out * 2.0);

        // Only the selected points move — the box is over a subset as often as not.
        // The two diagonal corners here box the same square, so the same drag
        // doubles the one that is selected and leaves the two that are not.
        let picked = [(0, 0), (0, 2)];
        let mut some = subs.clone();
        resize_points(
            &mut some,
            &picked,
            points_box(&subs, &picked).unwrap(),
            Handle::BottomRight,
            Point::new(200.0, 200.0),
            Resize::geometry(false, false),
        );
        assert_eq!(some[0].anchors[2].at, Point::new(200.0, 200.0));
        assert_eq!(some[0].anchors[1].at, subs[0].anchors[1].at, "unselected");
        assert_eq!(some[0].anchors[3].at, subs[0].anchors[3].at, "unselected");
    }

    /// **Every handle, dropped exactly where it was picked up, changes nothing.**
    ///
    /// The bug this exists for: `resize_factors` divided the pointer's signed
    /// distance from the anchor by the box's *positive* extent, and a handle on the
    /// minimum side starts at −extent from its anchor. So grabbing a group's or a
    /// multi-selection's **top-left** corner and not moving it mirrored the whole
    /// thing across its own bottom-right corner, before the hand had travelled a
    /// pixel. Reported against the box over a path's selected points, which is the
    /// third caller; the other two had it all along.
    ///
    /// It survived because every test of those two used `Handle::BottomRight` —
    /// the one handle whose sign comes out right either way. Hence the loop: the
    /// identity is the cheapest property that distinguishes the two versions, and
    /// it has to be asked of all eight.
    #[test]
    fn a_handle_dropped_where_it_was_grabbed_scales_by_one() {
        let box_ = Rect::new(10.0, 20.0, 110.0, 70.0);
        for handle in Handle::CORNERS.into_iter().chain(Handle::EDGES) {
            let (ux, uy) = handle.unit();
            let at = Point::new(
                box_.min_x() + box_.width() * ux,
                box_.min_y() + box_.height() * uy,
            );
            for symmetric in [false, true] {
                let (sx, sy, _) = resize_factors(box_, handle, at, false, symmetric);
                assert!(
                    (sx - 1.0).abs() < 1e-9 && (sy - 1.0).abs() < 1e-9,
                    "{handle:?} symmetric={symmetric} at rest gave ({sx}, {sy})"
                );
            }
        }
    }

    /// And a handle actually dragged scales the way round it looks: the box
    /// follows the pointer and the opposite side stays where it is, from the
    /// minimum side as much as from the maximum one.
    #[test]
    fn a_min_side_handle_scales_rather_than_mirroring() {
        let box_ = Rect::new(0.0, 0.0, 100.0, 50.0);
        // Top-left pulled out to (-100, -50): twice the box, held at the far corner.
        let (sx, sy, anchor) = resize_factors(
            box_,
            Handle::TopLeft,
            Point::new(-100.0, -50.0),
            false,
            false,
        );
        assert!(
            (sx - 2.0).abs() < 1e-9 && (sy - 2.0).abs() < 1e-9,
            "{sx}, {sy}"
        );
        assert_eq!(anchor, Point::new(100.0, 50.0));

        // Halfway in from the left: half the width, height untouched.
        let (sx, sy, anchor) =
            resize_factors(box_, Handle::Left, Point::new(50.0, 25.0), false, false);
        assert!(
            (sx - 0.5).abs() < 1e-9 && (sy - 1.0).abs() < 1e-9,
            "{sx}, {sy}"
        );
        assert_eq!(anchor.x, 100.0);

        // Dragged *past* the anchor it holds, the sign flips — the mirror is still
        // sayable, and it is what the pointer is asking for.
        let (sx, _, _) =
            resize_factors(box_, Handle::TopLeft, Point::new(200.0, 0.0), false, false);
        assert!(sx < 0.0, "a handle dragged past its anchor mirrors: {sx}");
    }

    /// **`t` means the same thing to core and to the app**, on a line as much as on
    /// a curve — the segment counterpart of
    /// `the_app_and_core_number_a_paths_anchors_alike`, and the same class of
    /// silent disagreement.
    ///
    /// `geometry::nearest_segment` measures against the real `PathSeg`, so a
    /// straight segment gives back the *linear* parameter. `segment_cubic` used to
    /// express that segment as the cubic with its control points on its own
    /// endpoints — the same line, a different parameterisation, `B(0.25)` at 15.6%
    /// along. Reported as the insert affordance drifting away from the pointer as
    /// it moved along a straight edge: **28 units out of 300 at the quarter point,
    /// and exact at the middle**, which is that error's signature. Degree-elevating
    /// the line makes the two agree identically, and this is what says so.
    #[test]
    fn a_segments_parameter_means_the_same_to_the_hit_test_and_to_the_app() {
        use ondin_core::kurbo::Vec2;
        let subs = vec![PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(0.0, 0.0)),
                // A long straight run — the case that drifted.
                PenAnchor::corner(Point::new(300.0, 0.0)),
                PenAnchor::smooth(Point::new(400.0, 200.0), Vec2::new(0.0, 90.0)),
                PenAnchor::corner(Point::new(100.0, 260.0)),
            ],
            closed: false,
        }];
        let path = pen_path(&subs);
        // Walk a pointer just off the ink, all along the path.
        for seg in 0..3usize {
            let cubic = segment_cubic(&subs, (0, seg)).expect("a segment");
            for k in 1..8 {
                let u = k as f64 / 8.0;
                let pointer = cubic.eval(u) + Vec2::new(0.0, -2.0);
                let hit = ondin_core::geometry::nearest_segment(&path, pointer).expect("a hit");
                let mine = segment_point(&subs, (hit.subpath, hit.anchor), hit.t).expect("a point");
                assert!(
                    (mine - hit.at).hypot() < 1e-6,
                    "segment {seg} at u={u}: core says {:?}, the app says {mine:?}",
                    hit.at
                );
            }
        }

        // And the straight one is exactly a lerp, which is the property that makes
        // it agree: an assertion on the arithmetic, not on the round trip.
        let line = segment_cubic(&subs, (0, 0)).unwrap();
        for k in 0..=4 {
            let t = k as f64 / 4.0;
            assert!((line.eval(t).x - 300.0 * t).abs() < 1e-9, "t={t}");
        }
    }

    /// A bend grabs the point the pointer grabbed, on a **straight** segment too.
    ///
    /// The same disagreement from the other side: the pull is split by `B(t)`'s
    /// dependence on the two control points, so those control points have to be the
    /// ones `t` was measured against. Starting them on the endpoints put the grabbed
    /// point somewhere else entirely.
    #[test]
    fn bending_a_straight_segment_moves_the_point_the_pointer_took() {
        use ondin_core::kurbo::Vec2;
        for t in [0.25, 0.5, 0.75] {
            let mut subs = vec![PenSubpath {
                anchors: vec![
                    PenAnchor::corner(Point::new(0.0, 0.0)),
                    PenAnchor::corner(Point::new(300.0, 0.0)),
                ],
                closed: false,
            }];
            let before = segment_point(&subs, (0, 0), t).unwrap();
            assert!((before.x - 300.0 * t).abs() < 1e-9, "the grab is a lerp");
            let delta = Vec2::new(10.0, -40.0);
            bend_segment(&mut subs, (0, 0), t, delta);
            let after = segment_point(&subs, (0, 0), t).unwrap();
            assert!(
                (after - (before + delta)).hypot() < 1e-9,
                "t={t}: {after:?} vs {:?}",
                before + delta
            );
        }

        // A bend that has not moved writes nothing at all — the elevation changes
        // the representation without changing the shape, and a `Ctrl`-press that
        // never travelled must not land on the undo stack.
        let mut subs = vec![PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(0.0, 0.0)),
                PenAnchor::corner(Point::new(300.0, 0.0)),
            ],
            closed: false,
        }];
        bend_segment(&mut subs, (0, 0), 0.5, Vec2::ZERO);
        assert!(subs[0].anchors[0].is_corner(), "still a plain line");
    }

    /// **The direction that makes the anchor smooth is the one this exists for.**
    /// `Alt` breaks a pair in a moment and there was no way back to a smooth point
    /// except by eye — and "nearly smooth" is precisely what a curve shows and a
    /// hand cannot fix.
    ///
    /// The length is kept, so the snap changes where the handle points and nothing
    /// else; and the tolerance is a perpendicular *distance*, so it is the same
    /// target on a long handle as on a short one — which is the whole reason it is
    /// not an angle.
    #[test]
    fn a_dragged_handle_snaps_to_the_direction_that_makes_its_anchor_smooth() {
        use ondin_core::kurbo::Vec2;
        let at = Point::new(100.0, 100.0);
        // Incoming handle points up-left at an awkward angle; the smooth direction
        // for the outgoing one is its exact opposite.
        let broken = PenAnchor {
            at,
            out: Vec2::new(5.0, 0.0),
            back: Vec2::new(-30.0, -40.0),
            radius: 0.0,
        };
        let smooth_dir = Vec2::new(30.0, 40.0).normalize();

        // Aiming 3 units off that ray, 50 units out: it lands on the ray, at the
        // length the pointer asked for.
        let want = at + smooth_dir * 50.0;
        let perp = Vec2::new(-smooth_dir.y, smooth_dir.x) * 3.0;
        let got = tangent_snap(&broken, Side::Out, want + perp, 4.0);
        assert!((got - want).hypot() < 1e-9, "{got:?} vs {want:?}");

        // 6 units off is past the tolerance, and free aim wins.
        let free = want + perp * 2.0;
        assert_eq!(tangent_snap(&broken, Side::Out, free, 4.0), free);

        // **The same 3 units off at four times the length still snaps**, which an
        // angular tolerance would not do — that is the point of measuring across.
        let far = at + smooth_dir * 200.0;
        let got = tangent_snap(&broken, Side::Out, far + perp, 4.0);
        assert!((got - far).hypot() < 1e-9, "{got:?} vs {far:?}");

        // The eight compass directions are candidates too — the set a Shift-drawn
        // line already snaps to.
        let axis = at + Vec2::new(60.0, 0.0);
        let got = tangent_snap(&broken, Side::Back, axis + Vec2::new(0.0, 2.0), 4.0);
        assert!((got - axis).hypot() < 1e-9, "{got:?} vs {axis:?}");

        // A candidate the pointer is *behind* is not offered: projecting onto the
        // far side would flip the handle through its own anchor, which is a jump.
        let behind = at - smooth_dir * 50.0;
        let got = tangent_snap(&broken, Side::Out, behind, 4.0);
        assert!(
            (got - behind).hypot() < 1e-9 || (got - at).dot(smooth_dir) < 0.0,
            "the handle jumped to the other side: {got:?}"
        );

        // And a corner offers no smooth direction, having no other handle to be
        // collinear with — only the compass points.
        let corner = PenAnchor::corner(at);
        let off = at + Vec2::new(50.0, 20.0);
        assert_eq!(tangent_snap(&corner, Side::Out, off, 4.0), off);

        // **Nor does an anchor that is already smooth**, and that one is a trap
        // rather than a nicety. The handles handed in here are last frame's, so on
        // a mirrored anchor the "smooth" direction is the one this handle is
        // already pointing: offering it would pull the handle back to where it was,
        // and a slow drag would stick and then jump. Every unmodified *pen* drag is
        // this case, which is how it was found.
        let mirrored = PenAnchor::smooth(at, Vec2::new(40.0, 10.0));
        let nudged = at + Vec2::new(41.0, 11.5);
        assert_eq!(
            tangent_snap(&mirrored, Side::Out, nudged, 4.0),
            nudged,
            "a smooth anchor snapped its own handle back to where it was"
        );

        // **Through `pull_handle`, which is what the drag actually calls.** The
        // rule above is worth nothing if the gesture does not ask for it, and a
        // test of the function alone cannot tell. `Alt` here so the mirror is not
        // what puts the handle back in line.
        let mut subs = vec![PenSubpath {
            anchors: vec![broken],
            closed: false,
        }];
        pull_handle(&mut subs, (0, 0), Side::Out, want + perp, true, 0.0, 4.0);
        assert!(
            (subs[0].anchors[0].leaving() - want).hypot() < 1e-9,
            "the drag did not ask for the tangent snap: {:?}",
            subs[0].anchors[0].leaving()
        );
    }

    /// **A join meets the endpoint the press grabbed**, whichever end of the target
    /// that was — and the orientation is the *opposite* of the one the pen resumes
    /// with, which is the thing to get wrong.
    ///
    /// Asserted against `PenEdit::endpoint`, the same accessor that decides where
    /// the ring is drawn: the point promised by the affordance is the point the two
    /// runs meet at, or the join crosses the whole target and comes back.
    #[test]
    fn a_join_meets_the_endpoint_the_press_grabbed() {
        use crate::preview::PenEdit;
        use ondin_core::kurbo::Affine;

        let head = [
            PenAnchor::corner(Point::new(0.0, 0.0)),
            PenAnchor::corner(Point::new(10.0, 0.0)),
        ];
        let target = PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(100.0, 0.0)),
                PenAnchor::corner(Point::new(150.0, 50.0)),
                PenAnchor::corner(Point::new(200.0, 0.0)),
            ],
            closed: false,
        };
        for from_start in [true, false] {
            let edit = PenEdit {
                id: IdSource::new(1).mint(),
                world: Affine::IDENTITY,
                subpaths: vec![target.clone()],
                active: 0,
                from_start,
                minted: false,
            };
            let joined = join_runs(&head, &target, from_start);
            assert_eq!(joined.anchors.len(), head.len() + target.anchors.len());
            assert!(!joined.closed, "a join produces an open run");
            assert_eq!(
                joined.anchors[head.len()].at,
                edit.endpoint().expect("a run with anchors has an endpoint"),
                "from_start={from_start}: the runs must meet at the ringed endpoint"
            );
            // The head is untouched and comes first, so the pen's own direction is
            // the joined run's.
            assert_eq!(joined.anchors[0].at, head[0].at);
            assert_eq!(joined.anchors[head.len() - 1].at, head[head.len() - 1].at);
            // And the far end of the target is now the far end of the join.
            let other = match from_start {
                true => target.anchors[target.anchors.len() - 1].at,
                false => target.anchors[0].at,
            };
            assert_eq!(joined.anchors[joined.anchors.len() - 1].at, other);
        }
    }

    /// A run of `n` anchors along the x axis, offset so two runs are tellable apart.
    fn run(n: usize, x0: f64, closed: bool) -> PenSubpath {
        PenSubpath {
            anchors: (0..n)
                .map(|i| PenAnchor::corner(Point::new(x0 + i as f64 * 10.0, 0.0)))
                .collect(),
            closed,
        }
    }

    /// **What *Join* will and will not take as a subject** (`context-menus.md`
    /// §6.5), which is the whole of the row's dimming.
    ///
    /// The last case is the one reading would not find: **two ends of a two-anchor
    /// run**. They are open ends, they are two, they are on one run — every term of
    /// the obvious predicate says yes — and closing that run adds a second segment
    /// retracing its only one. A shape with no interior, out of an edit that looks
    /// on screen exactly like the no-op it nearly is.
    ///
    /// ⚠️ Flipped by dropping the `anchors.len() < 3` arm (the last case comes back
    /// `Some`), and by asking `sub.closed` on only the first of the pair (the
    /// closed-run case passes when the *second* ref is the closed one, which is the
    /// half a single check silently misses).
    #[test]
    fn join_takes_two_open_ends_and_refuses_everything_else() {
        let two_runs = [run(3, 0.0, false), run(3, 100.0, false)];
        let ok = |points: &[PointRef]| joinable_ends(&two_runs, points).is_some();

        // One end of each run — the splice.
        assert!(ok(&[(0, 2), (1, 0)]));
        assert!(ok(&[(0, 0), (1, 2)]), "either end of either run");
        // Both ends of one run — the close.
        assert!(ok(&[(0, 0), (0, 2)]));

        // A middle anchor is not an end.
        assert!(!ok(&[(0, 1), (1, 0)]));
        // One is not two, and three is not two.
        assert!(!ok(&[(0, 0)]));
        assert!(!ok(&[(0, 0), (0, 2), (1, 0)]));
        // Nothing selected at all.
        assert!(!ok(&[]));
        // An index the path does not have.
        assert!(!ok(&[(0, 9), (1, 0)]));

        // A closed run has no ends — asserted for *both* positions in the pair,
        // since a predicate that checks only one of them passes half the time.
        let closed = [run(3, 0.0, true), run(3, 100.0, false)];
        assert!(!joinable_ends(&closed, &[(0, 0), (1, 0)]).is_some());
        assert!(!joinable_ends(&closed, &[(1, 0), (0, 0)]).is_some());

        // And the one that is only obvious once it is written down.
        let short = [run(2, 0.0, false), run(3, 100.0, false)];
        assert!(
            joinable_ends(&short, &[(0, 0), (0, 1)]).is_none(),
            "closing a two-anchor run retraces its only segment"
        );
        assert!(
            joinable_ends(&short, &[(0, 0), (1, 0)]).is_some(),
            "but splicing that same run onto another one is fine"
        );
    }

    /// **A splice runs from one far end to the other through the seam**, whichever
    /// ends were picked — and it leaves the joined run at the *lower* subpath index
    /// with the other entry gone.
    ///
    /// The four combinations are the test: each of the two runs can be grabbed at
    /// either end, and the orientation rule has to turn exactly the ones that need
    /// turning. A version that reversed neither, or both, still produces a run of
    /// the right *length* — which is why the assertion is on the sequence of
    /// positions rather than on the count.
    ///
    /// ⚠️ Flipped by reversing the head on `a.1 == last` instead of `a.1 == 0`: the
    /// two `(0, 0)` cases fail and the two `(0, 2)` cases pass, so half the table is
    /// what catches it.
    #[test]
    fn a_splice_reads_end_to_end_through_the_seam() {
        let subpaths = [run(3, 0.0, false), run(3, 100.0, false)];
        let xs = |sub: &PenSubpath| sub.anchors.iter().map(|a| a.at.x).collect::<Vec<_>>();

        for (a, b, want) in [
            // grabbed at 0/20 → the head turns; grabbed at 100 → the tail does not.
            ((0, 2), (1, 0), vec![0.0, 10.0, 20.0, 100.0, 110.0, 120.0]),
            ((0, 2), (1, 2), vec![0.0, 10.0, 20.0, 120.0, 110.0, 100.0]),
            ((0, 0), (1, 0), vec![20.0, 10.0, 0.0, 100.0, 110.0, 120.0]),
            ((0, 0), (1, 2), vec![20.0, 10.0, 0.0, 120.0, 110.0, 100.0]),
        ] {
            let (list, seam) = join_ends(&subpaths, a, b).expect("these are open ends");
            assert_eq!(list.len(), 1, "{a:?}→{b:?}: the two runs became one");
            assert_eq!(xs(&list[0]), want, "{a:?}→{b:?}");
            assert!(!list[0].closed, "a splice leaves the run open");
            // The seam names the segment *leaving* the head's last anchor — the
            // one the join just created (`geometry::anchor_runs`' numbering).
            assert_eq!(seam, (0, 2), "{a:?}→{b:?}");
        }
    }

    /// **The joined run keeps the lower index and the other entry goes**, so a path
    /// whose other subpaths were never touched does not have them renumbered out
    /// from under a selection — and the seam is reported against the index that
    /// survived, not against the one that was written to.
    ///
    /// ⚠️ Flipped by writing the joined run to `a.0` and removing `b.0`: with the
    /// pair given the other way round the surviving run lands in the wrong slot and
    /// `untouched` comes back in the wrong place.
    #[test]
    fn a_splice_keeps_the_lower_slot_whichever_end_was_grabbed_first() {
        let untouched = run(4, 500.0, true);
        let subpaths = [run(3, 0.0, false), untouched.clone(), run(3, 100.0, false)];
        for (a, b) in [((2, 0), (0, 2)), ((0, 2), (2, 0))] {
            let (list, seam) = join_ends(&subpaths, a, b).expect("these are open ends");
            assert_eq!(list.len(), 2, "{a:?}→{b:?}");
            assert_eq!(list[0].anchors.len(), 6, "the join is in slot 0");
            assert_eq!(
                list[1].anchors.iter().map(|p| p.at.x).collect::<Vec<_>>(),
                untouched.anchors.iter().map(|p| p.at.x).collect::<Vec<_>>(),
                "{a:?}→{b:?}: the run nobody touched came through unchanged"
            );
            assert_eq!(seam.0, 0, "{a:?}→{b:?}: the seam names the surviving slot");
        }
    }

    /// **Both ends of one run close it**, and the closing segment is the one
    /// `geometry::anchor_runs` names by the run's *last* anchor.
    ///
    /// ⚠️ Flipped by returning `(a.0, 0)` for the seam — plausible, since the close
    /// joins the last anchor back to the first, and wrong: a segment is named by the
    /// anchor it *leaves*, so naming it by the one it arrives at would light the
    /// first segment of the run instead of the new one.
    #[test]
    fn closing_a_run_adds_no_anchors_and_names_the_closing_segment() {
        let subpaths = [run(4, 0.0, false), run(3, 100.0, false)];
        let (list, seam) = join_ends(&subpaths, (0, 0), (0, 3)).expect("its own two ends");
        assert_eq!(list.len(), 2, "closing removes no subpath");
        assert_eq!(list[0].anchors.len(), 4, "and adds no anchor");
        assert!(list[0].closed);
        assert!(!list[1].closed, "the other run is left alone");
        assert_eq!(seam, (0, 3));
    }

    /// **Reversing a run flips its winding and nothing else**, which is the whole
    /// reason the verb exists: we fill non-zero, so a subpath inside another fills
    /// solid one way round and cuts a hole the other, and the direction is the only
    /// thing that decides it (§15 D125).
    ///
    /// Asserted on the **signed area**, because that is the reported symptom — "this
    /// hole is filling solid" — rather than on the anchor order, which is the
    /// mechanism. The area's *magnitude* has to survive: a reversal that also moved a
    /// handle would change the shape, and a sign flip alone would not catch it.
    ///
    /// And it reaches only the runs it is given. A verb whose subject is "the
    /// subpaths these points are on" that quietly turned the others round would flip
    /// every hole in the path while fixing one.
    #[test]
    fn reversing_a_subpath_flips_its_winding_and_leaves_the_others_alone() {
        use ondin_core::kurbo::Shape;

        // A square ring, wound clockwise, with a smaller one inside it wound the
        // same way — the case that fills solid where a hole was wanted.
        let ring = |x: f64, y: f64, w: f64| PenSubpath {
            anchors: vec![
                PenAnchor::corner(Point::new(x, y)),
                PenAnchor::corner(Point::new(x + w, y)),
                PenAnchor::corner(Point::new(x + w, y + w)),
                PenAnchor::corner(Point::new(x, y + w)),
            ],
            closed: true,
        };
        let mut subs = vec![ring(0.0, 0.0, 100.0), ring(25.0, 25.0, 50.0)];
        // A radius on the inner run's third corner, so the per-anchor value has
        // somewhere to be lost.
        subs[1].anchors[2].radius = 6.0;
        let area = |s: &PenSubpath| pen_outline(&s.anchors, s.closed).area();
        let (outer0, inner0) = (area(&subs[0]), area(&subs[1]));
        assert!(
            outer0.signum() == inner0.signum(),
            "the fixture must start with both runs wound the same way"
        );

        reverse_subpaths(&mut subs, &[1]);
        let (outer1, inner1) = (area(&subs[0]), area(&subs[1]));
        assert_eq!(
            inner1.signum(),
            -inner0.signum(),
            "the inner run still winds {inner0} — non-zero fills it solid instead of \
             cutting a hole"
        );
        assert!(
            (inner1.abs() - inner0.abs()).abs() < 1e-9,
            "the reversal changed the shape: |area| {} → {}",
            inner0.abs(),
            inner1.abs()
        );
        assert_eq!(outer1, outer0, "the run nobody named was turned round too");
        // The radius rode along with its corner: third of four from the start is
        // second of four from the end.
        assert_eq!(subs[1].anchors[1].radius, 6.0, "the radius left its corner");
        // And it flattens to the model at the index that corner now occupies —
        // fifth of the eight anchors, the list trimmed of its trailing zeros.
        assert_eq!(pen_radii(&subs), vec![0.0, 0.0, 0.0, 0.0, 0.0, 6.0]);
    }

    /// The bridge from a point selection to the runs it names. Ascending and
    /// deduplicated, because the caller reverses each once and indexes into the list
    /// afterwards to remap the selection.
    #[test]
    fn the_subpaths_a_selection_touches_are_listed_once_each() {
        assert_eq!(
            subpaths_touched(&[(2, 0), (0, 5), (2, 1), (0, 0), (1, 3)]),
            vec![0, 1, 2]
        );
        assert!(subpaths_touched(&[]).is_empty());
    }
    // --- placing an image (§5.5a, §15 D177) ----------------------------------

    fn entry_of(w: u32, h: u32) -> ImageEntry {
        ImageEntry {
            source: ondin_core::ImageSource::Embedded(vec![1, 2, 3].into()),
            format: ondin_core::ImageFormat::Png,
            width: w,
            height: h,
        }
    }

    /// **Fit shrinks and never grows**, which is two rules in one function and the
    /// second is the one that gets forgotten.
    ///
    /// A small asset staying small is the whole reason "fit" is not "fill": a 32px
    /// icon blown up to a 1200px frame is enormous and blurry, and reads as the
    /// tool having mangled it.
    #[test]
    fn a_placed_image_shrinks_to_fit_and_is_never_scaled_up() {
        let frame = Size::new(1000.0, 800.0);

        // Smaller than the frame on both axes: untouched, 1px = 1 unit.
        assert_eq!(
            fitted_image_size(Size::new(32.0, 32.0), frame),
            Size::new(32.0, 32.0)
        );
        // Exactly the frame: also untouched — the boundary is not a scale.
        assert_eq!(fitted_image_size(frame, frame), frame);

        // Wider than the frame: scaled by the width, aspect preserved.
        let wide = fitted_image_size(Size::new(4000.0, 2000.0), frame);
        assert_eq!(wide, Size::new(1000.0, 500.0));

        // Taller than the frame: the *height* is the binding constraint, which is
        // the case a `min` written the other way round gets wrong — it would come
        // back 1000×4000 and hang off the frame.
        let tall = fitted_image_size(Size::new(1000.0, 4000.0), frame);
        assert_eq!(tall, Size::new(200.0, 800.0));
        assert!(
            tall.height <= frame.height && tall.width <= frame.width,
            "a fitted image must be inside the box it was fitted to, got {tall:?}"
        );
    }

    /// A frame with no size falls back to the intrinsic size rather than to
    /// nothing: a zero-sized layer is not a placement anyone can see or fix.
    #[test]
    fn a_degenerate_frame_places_the_image_at_its_own_size() {
        let intrinsic = Size::new(64.0, 48.0);
        assert_eq!(fitted_image_size(intrinsic, Size::ZERO), intrinsic);
        assert_eq!(
            fitted_image_size(intrinsic, Size::new(-10.0, 100.0)),
            intrinsic
        );
    }

    /// **The click point is the image's centre**, because that is what a loaded
    /// cursor is holding — the pointer says where the picture goes, not where its
    /// corner goes.
    #[test]
    fn a_placed_image_is_centred_on_the_click() {
        let mut ids = IdSource::new(1);
        let (node, parent) = (ids.mint(), ids.mint());
        let tx = create_image(
            node,
            parent,
            0,
            Affine::IDENTITY,
            Point::new(500.0, 400.0),
            Size::new(1000.0, 1000.0),
            ImageId("sha256:x".into()),
            Some(entry_of(200, 100)),
            Size::new(200.0, 100.0),
            None,
        );
        let Operation::CreateNode {
            kind, transform, ..
        } =
            tx.0.iter()
                .find(|o| matches!(o, Operation::CreateNode { .. }))
                .unwrap()
        else {
            unreachable!()
        };
        assert_eq!(
            *kind,
            NodeKind::Rect {
                size: Size::new(200.0, 100.0),
                corner_radii: RoundedRectRadii::default(),
            },
            "it fits, so it keeps its pixel size"
        );
        // Centred: the 200×100 box starts half its size back from the click.
        assert_eq!(
            transform.unwrap().translation(),
            Vec2::new(400.0, 350.0),
            "the click should be the centre of the placed box"
        );
    }

    /// The `NodeKind` a creation builder writes, for the tests below.
    fn made(tx: &Transaction) -> NodeKind {
        match tx
            .0
            .iter()
            .find(|o| matches!(o, Operation::CreateNode { .. }))
        {
            Some(Operation::CreateNode { kind, .. }) => kind.clone(),
            _ => panic!("a creation builder makes a node"),
        }
    }

    /// **The commit door clamps its side count, as the keyboard door does**
    /// (§15 D727, `[S13.2-L6-06]`).
    ///
    /// `step_sides` has three tests for exactly this bound and its doc says why it
    /// matters — *"`↓` held at three would wrap a `u32` straight to four
    /// billion"* — but that is the **keyboard** door. `create_polygon` and
    /// `create_star` are the **commit** door, and nothing asserted the two agree:
    /// deleting either `clamp` left the whole suite green.
    ///
    /// ⚠️ **A `u32` cannot go below zero, so the interesting end is not the one it
    /// looks like.** `MIN_SIDES` is 3, and the overflow lives in `step_sides`'
    /// *subtraction* — this door cannot reach it. The two fail differently, which is
    /// why both need the clamp.
    ///
    /// 🚨 **And what this door protects is the *document*, not the drawing** — the
    /// sentence here said otherwise for a day and was flatly wrong. It read *"what a
    /// bare `0` or `2` produces is a polygon with no area — `geometry` has no arm
    /// that draws one"*, and `geometry::star_path` opens with
    /// `let n = n.clamp(MIN_SIDES, MAX_SIDES)`, so a stored `0` **draws a triangle**.
    /// `ondin_core::geometry`'s own `tests::a_degenerate_side_count_still_builds_a_shape`
    /// asserts exactly that, for `0`, `1` and `2`, with `path.area().abs() > 0.0` —
    /// plain backticks because it is `#[cfg(test)]` and no gate resolves a link into
    /// one (§15 D319).
    /// So an unclamped count is not an invisible shape: it is a **count nothing
    /// honours** — the canvas draws three, the model holds zero, the file saves zero
    /// and the *Sides* field shows zero — which is a worse failure than a missing
    /// shape and a quieter one. Clamping at the draw *and* at the commit is not
    /// belt-and-braces; the two guard different things.
    ///
    /// ⚠️ **Nothing can deliver an out-of-range count today.** `canvas.rs`'s
    /// `Drag::Create` seeds from `POLYGON_DEFAULT_SIDES` / `STAR_DEFAULT_POINTS` and
    /// moves it only through `step_sides`, which clamps already. That is why deleting
    /// this clamp left the suite green *and* left the app correct — and exactly why a
    /// future reader would delete it. This test is what makes that reader stop.
    ///
    /// Both builders are driven, because the constant reaches them through two
    /// differently-named fields (`sides` and `points`) and a fix applied to one is
    /// exactly the shape that misses the other.
    #[test]
    fn the_creation_builders_clamp_their_side_count_at_both_ends() {
        let mut ids = IdSource::new(1);
        let (parent, a, b) = (ids.mint(), Point::ZERO, Point::new(100.0, 100.0));
        let mut poly = |sides| {
            create_polygon(
                ids.mint(),
                parent,
                0,
                Affine::IDENTITY,
                a,
                b,
                sides,
                Color::BLACK,
            )
        };
        for (asked, want) in [
            (0, 3),
            (2, 3),
            (3, 3),
            (7, 7),
            (64, 64),
            (65, 64),
            (u32::MAX, 64),
        ] {
            match made(&poly(asked)) {
                NodeKind::Polygon { sides, .. } => assert_eq!(
                    sides, want,
                    "a polygon asked for {asked} sides must be made with {want}"
                ),
                other => panic!("create_polygon makes a polygon, not {other:?}"),
            }
        }

        let mut star = |points| {
            create_star(
                ids.mint(),
                parent,
                0,
                Affine::IDENTITY,
                a,
                b,
                points,
                Color::BLACK,
            )
        };
        for (asked, want) in [(0, 3), (2, 3), (5, 5), (65, 64), (u32::MAX, 64)] {
            match made(&star(asked)) {
                NodeKind::Star { points, .. } => assert_eq!(
                    points, want,
                    "a star asked for {asked} points must be made with {want}"
                ),
                other => panic!("create_star makes a star, not {other:?}"),
            }
        }

        // The only place `STAR_DEFAULT_RATIO` reaches a node.
        match made(&star(5)) {
            NodeKind::Star { inner_ratio, .. } => assert_eq!(inner_ratio, STAR_DEFAULT_RATIO),
            other => panic!("not {other:?}"),
        }
    }

    /// **A drawn box is an instruction: `create_image_in_box` does not fit and
    /// does not lock the aspect** (§15 D727, `[S13.2-L6-06]`).
    ///
    /// That is the whole of this builder's contract and its doc's own headline —
    /// *"No fit, and no aspect lock. A drawn box is an instruction; second-guessing
    /// it is what makes a tool feel like it is arguing."* It is a **negative**
    /// contract: what it must not do is call `fitted_image_size`, which its sibling
    /// `create_image` does.
    ///
    /// 🚨 **Nothing asserted it, and the two image tests above look as though they
    /// do.** `a_placed_image_shrinks_to_fit_and_is_never_scaled_up` and
    /// `a_placed_image_is_centred_on_the_click` both drive the **click** path only,
    /// so the one distinction between the two placement gestures was asserted
    /// nowhere. *A negative contract needs a fixture in which the positive one
    /// would visibly differ* — hence an intrinsic size that `create_image` would
    /// shrink, and an aspect the box disagrees with.
    ///
    /// ⚠️ **Flip-check, run: `fitted_image_size` inserted into
    /// `create_image_in_box`, which is the mistake this guards.** Red here at
    /// `50.0W×25.0H` against `50.0W×300.0H` — and **both `a_placed_image_…` tests
    /// stayed green**, which is the measurement rather than the assertion: it says
    /// their coverage really does stop at the click path, and is why this test had
    /// to exist rather than be folded into one of them.
    #[test]
    fn a_dragged_image_takes_the_box_it_was_drawn_at() {
        let mut ids = IdSource::new(1);
        let (node, parent) = (ids.mint(), ids.mint());
        // 400x200 intrinsic, drawn into a 50x300 box: the wrong aspect *and*
        // smaller than intrinsic, so a fit would change both numbers.
        let intrinsic = Size::new(400.0, 200.0);
        let drawn = Size::new(50.0, 300.0);
        assert_ne!(
            fitted_image_size(intrinsic, drawn),
            drawn,
            "the fixture must be a box the fit rule would argue with, or this \
             test cannot see the difference it is about"
        );

        let tx = create_image_in_box(
            node,
            parent,
            0,
            Affine::IDENTITY,
            Point::new(10.0, 20.0),
            Point::new(60.0, 320.0),
            ImageId("sha256:x".into()),
            Some(entry_of(400, 200)),
            None,
        );
        match made(&tx) {
            NodeKind::Rect { size, .. } => assert_eq!(
                size, drawn,
                "the layer is the box that was drawn, at the aspect it was drawn"
            ),
            other => panic!("an image is placed as a Rect, not {other:?}"),
        }
    }

    /// **An ellipse is made at the box it was dragged**, which is the one thing
    /// `create_ellipse` decides (§15 D727, `[S13.2-L6-06]`).
    ///
    /// Its own `local_box` is shared and covered by
    /// `create_shape_box_is_order_independent`; what was uncovered is that this
    /// builder passes it through to an `Ellipse` — the test whose name suggests
    /// otherwise calls the private helper directly and never reaches a builder.
    #[test]
    fn an_ellipse_is_made_at_the_box_it_was_dragged() {
        let mut ids = IdSource::new(1);
        let tx = create_ellipse(
            ids.mint(),
            ids.mint(),
            0,
            Affine::IDENTITY,
            Point::new(60.0, 320.0),
            Point::new(10.0, 20.0),
            Color::BLACK,
        );
        match made(&tx) {
            // Corners given the other way round: the box is the same box.
            NodeKind::Ellipse { size } => assert_eq!(size, Size::new(50.0, 300.0)),
            other => panic!("create_ellipse makes an ellipse, not {other:?}"),
        }
    }

    /// **The transaction adds the bytes before the node that refers to them**, so
    /// its inverse removes the node first. The reverse order inverts to
    /// "restore a fill pointing at an image that is not back yet", which is the
    /// shape that made a frame's guides load-bearing (`build::guides_of`).
    #[test]
    fn placing_adds_the_image_before_the_layer_and_fills_with_it() {
        let mut ids = IdSource::new(2);
        let (node, parent) = (ids.mint(), ids.mint());
        let id = ImageId("sha256:y".into());
        let tx = create_image(
            node,
            parent,
            0,
            Affine::IDENTITY,
            Point::ZERO,
            Size::new(500.0, 500.0),
            id.clone(),
            Some(entry_of(10, 10)),
            Size::new(10.0, 10.0),
            Some("hero.png".into()),
        );
        assert!(
            matches!(tx.0[0], Operation::AddImage { .. }),
            "the bytes come first: {:?}",
            tx.0[0]
        );
        assert!(matches!(tx.0[1], Operation::CreateNode { .. }));
        let Operation::SetFills { fills, .. } = &tx.0[2] else {
            panic!("expected the fill last, got {:?}", tx.0[2]);
        };
        assert_eq!(fills[0].brush, ondin_core::image_brush(id));

        // The layer is named for the file — an auto "Rectangle 3" would lose the
        // one piece of identity an image arrives with.
        let Operation::CreateNode { name, .. } = &tx.0[1] else {
            unreachable!()
        };
        assert_eq!(name.as_deref(), Some("hero.png"));
    }

    /// **Placing a file the document already carries adds a reference, not a
    /// second copy** — which is the whole point of hashing the bytes, and is
    /// visible here as the transaction being one operation shorter.
    #[test]
    fn placing_an_image_the_document_already_has_adds_no_second_copy() {
        let mut ids = IdSource::new(3);
        let (node, parent) = (ids.mint(), ids.mint());
        let id = ImageId("sha256:z".into());
        let tx = create_image(
            node,
            parent,
            0,
            Affine::IDENTITY,
            Point::ZERO,
            Size::new(500.0, 500.0),
            id.clone(),
            None,
            Size::new(10.0, 10.0),
            None,
        );
        assert_eq!(tx.0.len(), 2, "expected no AddImage: {:?}", tx.0);
        assert!(!tx.0.iter().any(|o| matches!(o, Operation::AddImage { .. })));
        let Operation::SetFills { fills, .. } = &tx.0[1] else {
            unreachable!()
        };
        assert_eq!(
            fills[0].brush,
            ondin_core::image_brush(id),
            "the second layer still points at the one copy"
        );
    }
}

#[cfg(test)]
mod guide_scale_tests {
    use super::*;
    use ondin_core::{Guide, GuideAxis, GuideId, IdSource};

    /// A frame with one guide down it and one across it, both scoped to the frame.
    fn framed_guides() -> (Document, Resolved, NodeId, GuideId, GuideId) {
        let mut ids = IdSource::new(0x6D1DE);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let frame = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 200.0),
            },
            transform: None,
            name: None,
        }]))
        .unwrap();
        let (down, across) = (GuideId(ids.mint()), GuideId(ids.mint()));
        doc.apply(&Transaction(vec![
            Operation::AddGuide {
                guide: Guide {
                    id: down,
                    axis: GuideAxis::Vertical,
                    position: 200.0,
                    color: None,
                    owner: Some(frame),
                },
            },
            Operation::AddGuide {
                guide: Guide {
                    id: across,
                    axis: GuideAxis::Horizontal,
                    position: 100.0,
                    color: None,
                    owner: Some(frame),
                },
            },
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        (doc, res, frame, down, across)
    }

    /// **A resize holds a frame-scoped guide's offset; the Scale tool multiplies
    /// it.** §5.6's Geometry-versus-Photographic split, applied to the one absolute
    /// length a guide has — so the rule is inherited rather than invented here.
    ///
    /// **By the axis factor, not the geometric mean.** A stroke width has no
    /// direction; a guide's position is a coordinate on one named axis, so it
    /// scales the way a path's points do. Under a non-uniform scale the mean would
    /// drift a guide off the very feature it was placed against, which is the whole
    /// reason someone put it there.
    #[test]
    fn the_scale_tool_multiplies_a_frames_guides_and_a_resize_does_not() {
        // 400x200 -> 800x200: sx = 2, sy = 1. Non-uniform on purpose, so the mean
        // (sqrt 2) is visibly wrong for both guides if it were used.
        let corner = Point::new(800.0, 200.0);

        // Geometry — the ordinary resize. The frame grows and the guides stay.
        let (mut doc, res, frame, down, across) = framed_guides();
        doc.apply(&resize_to_handle(
            &doc,
            &res,
            frame,
            Handle::BottomRight,
            corner,
            Resize::default(),
        ))
        .unwrap();
        assert_eq!(
            doc.guide(down).unwrap().position,
            200.0,
            "a resize leaves an absolute length alone (§5.6)"
        );
        assert_eq!(doc.guide(across).unwrap().position, 100.0);

        // Photographic — the Scale tool. Each guide takes its own axis's factor.
        let (mut doc, res, frame, down, across) = framed_guides();
        doc.apply(&resize_to_handle(
            &doc,
            &res,
            frame,
            Handle::BottomRight,
            corner,
            Resize {
                scaling: Scaling::Photographic,
                ..Default::default()
            },
        ))
        .unwrap();
        assert_eq!(
            doc.guide(down).unwrap().position,
            400.0,
            "the vertical guide takes sx = 2, so the guide down the middle of a \
             400-wide frame is down the middle of an 800-wide one"
        );
        assert_eq!(
            doc.guide(across).unwrap().position,
            100.0,
            "the horizontal guide takes sy = 1 — not the geometric mean, which \
             would have moved it to 141.4 under a scale that did not touch its axis"
        );
    }

    /// **A photographic scale whose geometric mean is exactly 1 still moves the
    /// guides** (§15 D694, `[S13.1-L2-06]`).
    ///
    /// 🚨 **The case above passes for a reason that hides this one.** `(2.0,
    /// 1.0)` has a mean of 1.414, so `Scaling::scalar` answers `Some` and the
    /// `scaled_guides` call behind it is reached. `(2.0, 0.5)` has a mean of
    /// exactly **1**, `scalar` answers `None` for a Photographic drag — the
    /// no-op reading — and the whole tail of `scale_scalars` was skipped,
    /// guides included. The frame doubled in width and its centre guide stayed
    /// at 200: a quarter of the way across instead of halfway.
    ///
    /// That is §5.6's rule broken by the *gate* rather than by the arithmetic —
    /// the mean, which the invariant names as the one quantity that must not
    /// decide a guide, deciding whether a guide is looked at at all.
    ///
    /// ⚠️ **Both guides are asserted and the second is the load-bearing one.**
    /// `sy = 0.5` moves the horizontal guide from 100 to 50, so a repair that
    /// scaled *both* guides by `csx` would pass the first assertion and fail
    /// this one — which is the plausible wrong version, since `csx` is the
    /// factor the reported symptom is about.
    ///
    /// ⚠️ **Flip:** putting the `scaled_guides` call back behind the `scalar`
    /// guard fails the first assertion with `left: 200.0, right: 400.0` — the
    /// guide not having moved at all.
    ///
    /// (Plain backticks per §15 D319 — `cargo doc` builds without the `test` cfg.)
    #[test]
    fn a_photographic_scale_with_a_mean_of_one_still_moves_the_guides() {
        // 400x200 -> 800x100: sx = 2, sy = 0.5, and sqrt(|2 * 0.5|) == 1.
        let corner = Point::new(800.0, 100.0);
        let (mut doc, res, frame, down, across) = framed_guides();
        doc.apply(&resize_to_handle(
            &doc,
            &res,
            frame,
            Handle::BottomRight,
            corner,
            Resize {
                scaling: Scaling::Photographic,
                ..Default::default()
            },
        ))
        .unwrap();

        assert_eq!(
            doc.guide(down).unwrap().position,
            400.0,
            "the vertical guide takes sx = 2 whatever the mean of the two factors is"
        );
        assert_eq!(
            doc.guide(across).unwrap().position,
            50.0,
            "and the horizontal one takes sy = 0.5 — each guide its own axis, \
             which is the half a single scalar cannot express"
        );
    }

    /// A guide sitting on the frame's own origin has nothing to scale, and an
    /// operation that changes nothing is a step the user has to press Ctrl+Z past.
    /// The same rule `scaling_a_square_cornered_rect_writes_no_radius_operation`
    /// applies to a zero radius.
    #[test]
    fn scaling_a_frame_writes_no_operation_for_a_guide_at_its_origin() {
        let mut ids = IdSource::new(0x6D1DE);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let frame = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 200.0),
            },
            transform: None,
            name: None,
        }]))
        .unwrap();
        let at_origin = GuideId(ids.mint());
        doc.apply(&Transaction(vec![Operation::AddGuide {
            guide: Guide {
                id: at_origin,
                axis: GuideAxis::Vertical,
                position: 0.0,
                color: None,
                owner: Some(frame),
            },
        }]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        let tx = resize_to_handle(
            &doc,
            &res,
            frame,
            Handle::BottomRight,
            Point::new(800.0, 400.0),
            Resize {
                scaling: Scaling::Photographic,
                ..Default::default()
            },
        );
        assert!(
            !tx.0
                .iter()
                .any(|op| matches!(op, Operation::SetGuidePosition { .. })),
            "a guide on the origin needs no patch: {tx:?}"
        );
    }

    /// A guide scoped to **another** frame is untouched by scaling this one, and a
    /// global guide is untouched by scaling anything. Both are the `owner` filter
    /// doing its job, and both would be silent if `guides_owned_by` were ever
    /// widened to "every guide".
    #[test]
    fn scaling_a_frame_leaves_other_frames_and_global_guides_alone() {
        let mut ids = IdSource::new(0x6D1DE);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (scaled, other) = (ids.mint(), ids.mint());
        let board = |id, index| Operation::CreateNode {
            id,
            parent: root,
            index,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 200.0),
            },
            transform: None,
            name: None,
        };
        doc.apply(&Transaction(vec![board(scaled, 0), board(other, 1)]))
            .unwrap();
        let (elsewhere, global) = (GuideId(ids.mint()), GuideId(ids.mint()));
        let guide = |id, owner| Operation::AddGuide {
            guide: Guide {
                id,
                axis: GuideAxis::Vertical,
                position: 120.0,
                color: None,
                owner,
            },
        };
        doc.apply(&Transaction(vec![
            guide(elsewhere, Some(other)),
            guide(global, None),
        ]))
        .unwrap();
        let res = Resolved::rebuild(&doc);
        doc.apply(&resize_to_handle(
            &doc,
            &res,
            scaled,
            Handle::BottomRight,
            Point::new(800.0, 400.0),
            Resize {
                scaling: Scaling::Photographic,
                ..Default::default()
            },
        ))
        .unwrap();
        assert_eq!(doc.guide(elsewhere).unwrap().position, 120.0);
        assert_eq!(doc.guide(global).unwrap().position, 120.0);
    }
}

/// `↑`/`↓` re-counting a polygon or star mid-draw (`docs/shortcuts.md` §9).
#[cfg(test)]
mod step_sides_tests {
    use super::{POLYGON_DEFAULT_SIDES, STAR_DEFAULT_POINTS, step_sides};
    use ondin_core::geometry::{MAX_SIDES, MIN_SIDES};

    /// One press is one side, in the direction the key points.
    #[test]
    fn each_press_is_one_side() {
        assert_eq!(step_sides(5, true, false), 6);
        assert_eq!(step_sides(5, false, true), 4);
        assert_eq!(step_sides(5, false, false), 5);
        // Both at once cancel, which is what taking them as a signed step buys.
        assert_eq!(step_sides(5, true, true), 5);
    }

    /// **`↓` at the floor stays at the floor rather than wrapping.**
    ///
    /// The failure this guards is not "the number goes below three" — it is that
    /// `u32` subtraction below zero wraps, so a naive `sides - 1` at the bottom
    /// of the range produces a four-billion-sided polygon and hangs the
    /// tessellator. Three is `MIN_SIDES`, and a triangle is what the polygon tool
    /// starts on, so the floor is reachable by pressing `↓` once at the default.
    #[test]
    fn the_floor_holds_instead_of_wrapping() {
        assert_eq!(step_sides(MIN_SIDES, false, true), MIN_SIDES);
        assert_eq!(step_sides(POLYGON_DEFAULT_SIDES, false, true), MIN_SIDES);
        assert_eq!(step_sides(MIN_SIDES + 1, false, true), MIN_SIDES);
        // And from zero, which is not a state the gesture reaches but is the
        // value an uninitialised field would hold.
        assert_eq!(step_sides(0, false, true), MIN_SIDES);
    }

    /// `↑` stops at the ceiling, which exists so the field cannot be used to make
    /// the app crawl.
    #[test]
    fn the_ceiling_holds_too() {
        assert_eq!(step_sides(MAX_SIDES, true, false), MAX_SIDES);
        assert_eq!(step_sides(MAX_SIDES - 1, true, false), MAX_SIDES);
        assert_eq!(step_sides(MAX_SIDES + 100, true, false), MAX_SIDES);
    }

    /// Held down, both directions converge on their bound and stay there — which
    /// is the case a user actually produces, by holding the key.
    #[test]
    fn holding_a_key_settles_at_the_bound() {
        let mut up = STAR_DEFAULT_POINTS;
        let mut down = STAR_DEFAULT_POINTS;
        for _ in 0..(MAX_SIDES * 2) {
            up = step_sides(up, true, false);
            down = step_sides(down, false, true);
        }
        assert_eq!(up, MAX_SIDES);
        assert_eq!(down, MIN_SIDES);
    }
}

#[cfg(test)]
mod overflow_tests {
    //! A resize whose *product* overflows, and the member that cannot take it
    //! (§15 D481, `[S13.1-L1-03]`).

    use super::*;
    use ondin_core::kurbo::{RoundedRectRadii, Size};
    use ondin_core::{Document, IdSource, NodeKind, Operation, Resolved, Transaction};

    /// Two rects in a frame. The first stores `Affine::scale(1e308)` over a size
    /// of `1e-306` — **every coefficient finite**, world bounds an entirely
    /// ordinary `(0,0)–(100,100)` — and the second is a plain 20×20 at x=80.
    ///
    /// `svg_in::import` takes `transform="scale(1e308)"` verbatim, so this is a
    /// document a paste can produce.
    ///
    /// ⚠️ **Not because `svg_in` fails to filter — it does**, since §15 D421:
    /// `numbers` runs `.filter(|f| f.is_finite())` with fifteen lines above it
    /// saying why. `1e308` *is* finite and passes. Written the wrong way round
    /// first, one function away from the guard it claimed did not exist, which is
    /// the sharpest form of a comment that would mislead the next reader.
    fn a_pathological_member_and_an_ordinary_one() -> (Document, Resolved, NodeId, NodeId) {
        let mut ids = IdSource::new(0x5E1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (ab, a, b) = (ids.mint(), ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 400.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: a,
                parent: ab,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(1e-306, 1e-306),
                    corner_radii: RoundedRectRadii::default(),
                },
                transform: Some(Affine::scale(1e308)),
                name: None,
            },
            Operation::CreateNode {
                id: b,
                parent: ab,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 20.0),
                    corner_radii: RoundedRectRadii::default(),
                },
                transform: Some(Affine::translate((80.0, 0.0))),
                name: None,
            },
        ]))
        .expect("the fixture is an ordinary, all-finite document");
        let res = Resolved::rebuild(&doc);
        (doc, res, a, b)
    }

    /// **One member whose basis overflows does not cost the whole selection its
    /// resize** (§15 D481).
    ///
    /// `MAX_SCALE` bounds the *factor*; the quantity that overflows is the
    /// product. This drag's factor is **2** — anchor `(0,0)`, the dragged corner
    /// starting 100 from it and the pointer now 200 — and `1e308 × 2` is `inf`.
    /// `Basis::of` then divides `inf` by `inf`, and all four **basis**
    /// coefficients of the matrix that comes back are `NaN`, pushed
    /// unconditionally because `!=` is true of `NaN`. (The translation pair stays
    /// finite; `scaled_about`'s own comment has why that distinction is worth
    /// keeping.)
    ///
    /// ⚠️ **What the finding measured is no longer what happens, and the
    /// difference is the whole reason this is Medium and not Critical.** It
    /// recorded the `NaN` transform and an infinite width being *applied*, saved,
    /// and the file never reloading. Since §15 D421 the op layer refuses a
    /// non-finite transform, and `Document::apply` is atomic — so what a user got
    /// instead was **the entire gesture silently doing nothing**, including for
    /// every well-behaved layer in the selection. Measured before this fix:
    /// `apply = Err(NonFinite)`, four ops discarded, two of them perfectly good.
    ///
    /// **The three assertions are the three halves of "no-op for that member
    /// only".** The ordinary rect moves; the pathological one is byte-identical;
    /// the transaction applies at all.
    ///
    /// ⚠️ **Flip-check, run: `scaled_about`'s `then_some` replaced by `Some(out)`.**
    /// Fails on the *third* assertion with `Err(NonFinite)` — and that is not the
    /// site that was predicted. The prediction was the first, on the argument that
    /// the good member would fail to move; it does fail to move, but the `apply`
    /// happens first and it is the atomicity rather than the arithmetic that the
    /// user meets. **The predicted site was one step too far downstream**, which is
    /// the correction worth keeping: with an atomic transaction, the failure a test
    /// sees is never per node.
    #[test]
    fn one_overflowing_member_does_not_refuse_the_whole_resize() {
        let (mut doc, res, a, b) = a_pathological_member_and_an_ordinary_one();
        let before = doc.get(a).expect("a is there").transform().as_coeffs();
        let union = [a, b]
            .iter()
            .filter_map(|id| res.world_bounds(*id))
            .reduce(|x, y| x.union(y))
            .expect("both resolve");
        assert_eq!(
            (union.width(), union.height()),
            (100.0, 100.0),
            "the fixture's union has to be ordinary, or the drag below is not one"
        );

        let tx = resize_selection(
            &doc,
            &res,
            &[a, b],
            SelectionBox::upright(union),
            Handle::BottomRight,
            Point::new(200.0, 200.0),
            Resize::geometry(false, false),
        );
        let applied = doc.apply(&tx);

        assert!(
            tx.0.iter().any(|op| matches!(
                op,
                Operation::SetGeometry { id, .. } | Operation::SetTransform { id, .. } if *id == b
            )),
            "the ordinary member has to be scaled — refusing per gesture is what \
             this fixes: {:?}",
            tx.0
        );
        assert_eq!(
            doc.get(a).expect("a survives").transform().as_coeffs(),
            before,
            "and the member that cannot take the scale is left exactly as it was"
        );
        assert!(
            applied.is_ok(),
            "the transaction has to reach the document: {applied:?}"
        );
    }

    /// **And the document it leaves behind still loads** — the half of
    /// `[S13.1-L1-03]` that made it more than a cosmetic refusal.
    ///
    /// Written as a save/load round trip rather than as an inspection of the ops,
    /// because *"the file never reloads"* is the failure the finding is named for
    /// and an op-shaped assertion would be a proxy for it. `io::load` is the only
    /// thing that decides.
    ///
    /// ⚠️ **This passes with the fix removed**, and it is kept anyway. Since D421
    /// the refusal happens at the op layer, so a `NaN` never reaches the document
    /// and the file is safe either way — the standing record that the two guards
    /// are separate, and that if the op guard ever narrows this is what notices.
    #[test]
    fn the_document_a_resize_leaves_behind_still_loads() {
        let (mut doc, res, a, b) = a_pathological_member_and_an_ordinary_one();
        let union = [a, b]
            .iter()
            .filter_map(|id| res.world_bounds(*id))
            .reduce(|x, y| x.union(y))
            .expect("both resolve");
        let _ = doc.apply(&resize_selection(
            &doc.clone(),
            &res,
            &[a, b],
            SelectionBox::upright(union),
            Handle::BottomRight,
            Point::new(200.0, 200.0),
            Resize::geometry(false, false),
        ));
        let bytes = ondin_core::io::save(&doc).expect("it saves");
        assert!(
            ondin_core::io::load(&bytes).is_ok(),
            "a document the app just wrote has to be one the app can read"
        );
    }
}

#[cfg(test)]
mod unrepresentable_basis_tests {
    //! A member whose basis has no **exact** split, and the gesture that used to
    //! collapse it (§15 D714, `[S13.1-L1-07]`).
    //!
    //! **A sibling of `overflow_tests` above and deliberately not folded into
    //! it.** That module's subject is a basis that overflows to `inf`, caught by
    //! `scaled_about`'s finiteness `then_some`; this one is about a basis that is
    //! entirely finite, passes every guard in the file, and simply **cannot be
    //! written as `orientation · scale`**. Same refusal shape, different reason,
    //! and a module doc that covered both would be true of neither.

    use super::*;
    use ondin_core::kurbo::{RoundedRectRadii, Size};
    use ondin_core::{Document, IdSource, NodeKind, Operation, Resolved, Transaction};

    /// One rect storing a **rank-1** transform: both columns on the line
    /// `y = 2x`, so the whole plane collapses onto it.
    ///
    /// Every coefficient is finite, so `build::affine_is_finite` accepts it and
    /// `op_set_transform` stores it. `svg_in` takes `transform="matrix(1,2,2,4,
    /// 0,0)"` verbatim, so this is a document a paste can produce — the same
    /// door `[A1-L2-01]` came through, one rank down.
    fn a_rank_one_member() -> (Document, Resolved, NodeId, NodeId) {
        let mut ids = IdSource::new(0x5E2);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let (ab, a, b) = (ids.mint(), ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            Operation::CreateNode {
                id: ab,
                parent: root,
                index: 0,
                kind: NodeKind::Artboard {
                    size: Size::new(400.0, 400.0),
                },
                transform: None,
                name: None,
            },
            Operation::CreateNode {
                id: a,
                parent: ab,
                index: 0,
                kind: NodeKind::Rect {
                    size: Size::new(40.0, 40.0),
                    corner_radii: RoundedRectRadii::default(),
                },
                transform: Some(Affine::new([1.0, 2.0, 2.0, 4.0, 0.0, 0.0])),
                name: None,
            },
            // The ordinary sibling, so "the selection still resizes" has
            // something to be true of.
            Operation::CreateNode {
                id: b,
                parent: ab,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(20.0, 20.0),
                    corner_radii: RoundedRectRadii::default(),
                },
                transform: Some(Affine::translate((80.0, 0.0))),
                name: None,
            },
        ]))
        .expect("a rank-1 transform is finite, so the op layer stores it");
        let res = Resolved::rebuild(&doc);
        (doc, res, a, b)
    }

    /// **A gesture on a member with no exact split leaves it alone rather than
    /// rewriting it into a different shape** (§15 D714).
    ///
    /// The loss first: before this, `rotate_node` on the fixture wrote a
    /// transform of `R(63.43°)` with `csy = 0`, and `scale_geometry` turned
    /// that zero into `(40 · 0).max(1.0)` — so a 40×40 rect became **1 unit tall
    /// at 63°**, from one turn, with every gate green and the op layer perfectly
    /// happy because none of those numbers is `NaN`.
    ///
    /// ⚠️ **The fixture is asserted into the state the test names.** A transform
    /// that is merely unusual splits fine and the gesture *should* rewrite it, so
    /// the determinant is pinned first — without that this test passes against a
    /// fixture that never reaches the defect.
    ///
    /// **`resize_selection` is here as the second consumer** and not as
    /// decoration: the six callers of `split_geometry_scale` fall into two
    /// shapes — one returns an empty transaction, one `continue`s past the
    /// member — and a fix reaching only the arm the finding measured would leave
    /// the other live. It is also the caller the finding names as the risky one,
    /// because it is the multi-select drag, and *"refuse the member"* must not
    /// become *"refuse the gesture"*: that is `[S13.1-L1-03]`'s mistake, one
    /// mechanism over, and §15 D481 is the entry that made it a per-node answer.
    ///
    /// ⚠️ **`skew_to_handle` was the first choice for the second consumer and is
    /// the wrong one**: it returns early when `side_shear` answers `None`, so the
    /// assertion would have been green whether or not the split refused
    /// anything. *A test whose subject is reached only sometimes is a test about
    /// something else.*
    ///
    /// ⚠️ **Flipped twice, separately, because one flip cannot reach both arms.**
    /// Putting `Basis::of` back inside `split_geometry_scale` fails on the
    /// **first** assertion, and the message is the defect in full:
    /// `SetGeometry { geometry: Size(89.44W × 1.0H) }` — the 40×40 rect one unit
    /// tall. But it aborts there, so the `continue` arm below is **never
    /// exercised by it**. The second flip therefore restores the old splitter at
    /// `resize_selection`'s call site alone, and fails on *"the rank-1 member is
    /// untouched"* with `[0.707, 0.707, −0.707, 0.707, 0, 0]` against the stored
    /// `[1, 2, 2, 4, 0, 0]`. 🚨 **The obvious single flip proves only the first
    /// half**, which is worth knowing generally: where a test asserts two
    /// consumers of one function, flipping the *function* stops at the first and
    /// says nothing about the rest.
    #[test]
    fn a_rank_one_member_is_left_alone_rather_than_collapsed() {
        let (doc, res, a, b) = a_rank_one_member();
        let before = doc.get(a).expect("a is there").transform().as_coeffs();
        assert_eq!(
            Affine::new(before).determinant(),
            0.0,
            "the fixture is singular, or this test is about nothing"
        );

        let turned = rotate_node(&doc, &res, a, Point::new(0.0, 0.0), 0.5);
        assert!(
            turned.0.is_empty(),
            "a basis with no exact split refuses the turn; got {:?}",
            turned.0
        );

        // And the multi-select drag: the rank-1 member sits it out, the ordinary
        // one still resizes, and the transaction applies.
        let mut doc = doc.clone();
        let union = [a, b]
            .iter()
            .filter_map(|id| res.world_bounds(*id))
            .reduce(|x, y| x.union(y))
            .expect("both resolve");
        let b_before = doc.get(b).expect("b is there").transform().as_coeffs();
        let tx = resize_selection(
            &doc.clone(),
            &res,
            &[a, b],
            SelectionBox::upright(union),
            Handle::BottomRight,
            Point::new(400.0, 400.0),
            Resize::geometry(false, false),
        );
        doc.apply(&tx).expect("the surviving ops are all finite");
        assert_eq!(
            doc.get(a).expect("a is there").transform().as_coeffs(),
            before,
            "the rank-1 member is untouched rather than rewritten"
        );
        assert_ne!(
            doc.get(b).expect("b is there").transform().as_coeffs(),
            b_before,
            "and its ordinary sibling still resized"
        );
    }
}

// **A `refusal_tests` module was written here on 2026-08-23 and removed the same
// hour**, which is worth recording because the reasoning nearly outlived the code.
// It was moved out of `menu.rs` to keep D280's finding pinned — that a *missing*
// picture and a *linked* one must refuse in different sentences — when the
// *Export original…* menu row went. Then the inspector's door went too,
// `original_refusal` had no caller left, and a test of a deleted function is not a
// test. The finding is written above the space where the function was; if linking
// is ever authored, that paragraph is the spec to build back from.

#[cfg(test)]
mod rewrite_no_op_tests {
    //! **`rewrite_path`'s no-op guard, over a path the pen did not write**
    //! (§15 D583, `[S14.1-L1-02]`).
    //!
    //! The guard compares the path it is about to store against the one already
    //! stored, and both have to be in the pen's canonical form for that question
    //! to mean anything: `pen_anchors` → `pen_path` writes out the closing
    //! segment of a closed subpath, which the ordinary SVG spelling leaves
    //! implicit, and `pen_radii` trims trailing zeros, which a stored list need
    //! not have had trimmed. Either difference alone turned a *click* into the
    //! corner-radius field into a `SetGeometry`, an undo step, a dirty document —
    //! and, in the path's case, a saved outline with a segment in it the user
    //! never drew.
    //!
    //! Plain backticks throughout, per §15 D319 — this is a `#[cfg(test)]` module
    //! and `cargo doc` cannot see it, so a `[link]` here would be checked by
    //! nothing.

    use super::*;
    use ondin_core::IdSource;

    /// The `<polygon>` shape, byte for byte: `svg_in::node_ignoring` runs
    /// `path.close_path()` and stores `corner_radii: Vec::new()`, and
    /// `BezPath::from_svg` gives the same elements for any `d` ending in `Z`.
    fn imported_triangle() -> BezPath {
        let mut p = BezPath::new();
        p.move_to((0.0, 0.0));
        p.line_to((80.0, 0.0));
        p.line_to((80.0, 60.0));
        p.close_path();
        p
    }

    fn a_node() -> NodeId {
        IdSource::new(0x5A1).mint()
    }

    /// **A click that changed nothing plans nothing, on a path the pen did not
    /// write.**
    ///
    /// ⚠️ **The fixture assertion is the first one and it is the whole point.**
    /// If `pen_path(&pen_anchors(…))` gave the stored path back unchanged there
    /// would be nothing here to fix, and the guard would have been firing all
    /// along — so the round trip is asserted to *move* before anything is claimed
    /// about the transaction. Measured: `MoveTo LineTo LineTo ClosePath` in,
    /// `MoveTo LineTo LineTo LineTo ClosePath` out.
    ///
    /// **Flip run**, the canonicalisation replaced by the old raw comparison
    /// against `current`: fails on *"a click that changed nothing rewrote the
    /// geometry"*, carrying the extra `LineTo((0.0, 0.0))` in the message. The
    /// control below stays green under the same flip, which says the flip did not
    /// simply disable the guard.
    #[test]
    fn a_no_op_over_an_imported_closed_path_plans_nothing() {
        let stored = imported_triangle();
        let subpaths = pen_anchors(&stored, &[]);

        assert_ne!(
            pen_path(&subpaths).elements(),
            stored.elements(),
            "the fixture: the round trip must move, or there is nothing here to guard"
        );

        let tx = rewrite_path(a_node(), Affine::IDENTITY, &subpaths, Some((&stored, &[])));
        assert_eq!(
            tx,
            Transaction(vec![]),
            "a click that changed nothing rewrote the geometry"
        );
    }

    /// **The radii half, which misses for its own reason.**
    ///
    /// `pen_radii` trims trailing zeros because the model's canonical form
    /// requires it (invariant 9), so a stored `[0, 0, 0]` — which nothing forbids
    /// — never equals the `[]` the rewrite produces, and the guard missed on the
    /// radii even when the path matched exactly.
    ///
    /// The path here is deliberately the **pen's own** spelling (an open subpath),
    /// so the element lists agree with no canonicalisation at all and the radii
    /// are the only thing under test. Two independent misses on one node is what
    /// the finding measured, and this is the second of them isolated.
    #[test]
    fn a_no_op_over_untrimmed_stored_radii_plans_nothing() {
        let mut stored = BezPath::new();
        stored.move_to((0.0, 0.0));
        stored.line_to((80.0, 0.0));
        stored.line_to((80.0, 60.0));
        let radii = [0.0, 0.0, 0.0];
        let subpaths = pen_anchors(&stored, &radii);

        assert_eq!(
            pen_path(&subpaths).elements(),
            stored.elements(),
            "the fixture: an open subpath must round-trip exactly, so the radii \
             are the only difference left"
        );
        assert_ne!(
            pen_radii(&subpaths).as_slice(),
            radii.as_slice(),
            "the fixture: the trim must move the list, or there is nothing to guard"
        );

        let tx = rewrite_path(
            a_node(),
            Affine::IDENTITY,
            &subpaths,
            Some((&stored, &radii)),
        );
        assert_eq!(
            tx,
            Transaction(vec![]),
            "a click that changed nothing rewrote the radii"
        );
    }

    /// **The control: a real edit still commits**, and it is a *radius* edit
    /// rather than a moved point, because the canonicalisation touches the radii
    /// too and a guard that swallowed those would pass a point-move control.
    #[test]
    fn a_radius_actually_set_still_plans_a_write() {
        let stored = imported_triangle();
        let mut subpaths = pen_anchors(&stored, &[]);
        subpaths[0].anchors[1].radius = 4.0;

        let tx = rewrite_path(a_node(), Affine::IDENTITY, &subpaths, Some((&stored, &[])));
        assert_eq!(tx.0.len(), 1, "a real radius edit was swallowed: {tx:?}");
        assert!(matches!(
            tx.0[0],
            Operation::SetGeometry {
                geometry: GeometryPatch::Path { .. },
                ..
            }
        ));
    }
}

/// **All four of `side_shear`'s edge arms, and the two modifiers on top of
/// them** (§15 D617, `[S13.1-L6-05]`, **G9**).
///
/// (Plain backticks throughout — this module is `#[cfg(test)]`, so `cargo doc`
/// never sees it and a `[link]` here is decoration no gate can validate.
/// §15 D319, and D622 for the size of that unchecked population.)
///
/// 🚨 **Half of that function was executed by nothing.** The four skew call sites
/// in the whole workspace pass `Handle::Top` three times and `Handle::Right`
/// once, and **all four pass `snap_to_steps: false`** — so `Handle::Bottom`'s
/// span sign, `Handle::Left`'s held edge, `SKEW_SNAP` and `MAX_SKEW` had no
/// caller anywhere. §15 D322 exists because this function got its *origin* wrong,
/// and it pinned `Handle::Top` alone.
///
/// **Two flips run by the review, both green across the whole workspace**:
/// `Bottom`'s span sign reversed together with `Left`'s held edge moved to
/// `min_x()` — a bottom drag then leans backwards and a left drag slides the
/// layer sideways instead of leaning about the far side — and the entire
/// snap-and-clamp body of `lean` replaced by a bare `(offset / span).atan()`.
/// ⚠️ A third flip run as a **control** did bite (neutering `paired_size` failed
/// four tests), so the suite is not blind everywhere in this file — only here, in
/// the function with the most arms.
///
/// **The arms are asserted as a bargain rather than as numbers**: the side facing
/// the grabbed one does not move, and the grabbed one moves by the drag. That is
/// what the doc promises, it is the same sentence for all four arms, and it is
/// what a sign error breaks.
#[cfg(test)]
mod side_shear_tests {
    use super::*;
    use crate::preview::Handle;

    /// The corner of `box_` that `handle` grabs, and the corner on the side
    /// facing it — the two points the bargain is about.
    ///
    /// ⚠️ **Corners rather than edge midpoints, on purpose.** A midpoint of the
    /// *held* side is fixed under a shear about that side for a trivial reason —
    /// it sits on the axis — so it would stay put under a shear about the box's
    /// centre too. A corner of the held side is only fixed if the whole side is.
    fn probe(box_: Rect, handle: Handle) -> (Point, Point) {
        match handle {
            Handle::Top => (
                Point::new(box_.min_x(), box_.min_y()),
                Point::new(box_.min_x(), box_.max_y()),
            ),
            Handle::Bottom => (
                Point::new(box_.min_x(), box_.max_y()),
                Point::new(box_.min_x(), box_.min_y()),
            ),
            Handle::Left => (
                Point::new(box_.min_x(), box_.min_y()),
                Point::new(box_.max_x(), box_.min_y()),
            ),
            Handle::Right => (
                Point::new(box_.max_x(), box_.min_y()),
                Point::new(box_.min_x(), box_.min_y()),
            ),
            _ => unreachable!("only edges shear"),
        }
    }

    /// The middle of the side `handle` owns — where the press goes, because §15
    /// D322 measures the offset from the press and pressing anywhere else is a
    /// legitimately different answer.
    fn grip(box_: Rect, handle: Handle) -> Point {
        match handle {
            Handle::Top => Point::new(box_.center().x, box_.min_y()),
            Handle::Bottom => Point::new(box_.center().x, box_.max_y()),
            Handle::Left => Point::new(box_.min_x(), box_.center().y),
            Handle::Right => Point::new(box_.max_x(), box_.center().y),
            _ => unreachable!("only edges shear"),
        }
    }

    /// ⚠️ **Flip-check, run, three of them, and the interesting part is a wrong
    /// *correction* rather than a wrong prediction.**
    /// - `Handle::Left`'s held edge moved to `box_.min_x()` fails at *"Left: the
    ///   held side moved, by (0, -20)"* — the predicted site.
    /// - `Handle::Bottom`'s span sign reversed fails at *"Bottom: the grabbed side
    ///   moved by (-20, 0), wanted (20, 0)"* — also the predicted site. **This
    ///   write-up first said it would bite the held-side assertion instead**, on
    ///   the reasoning that a reversed span leans the box the other way about the
    ///   same edge. That is wrong: the held edge is pinned by the `Vec2` beside
    ///   the span and is untouched by its sign, so what a reversed span changes is
    ///   the *direction the grabbed side travels*. **The prediction was right, the
    ///   correction written over it was not, and only running it said so.**
    /// - The whole snap-and-clamp body of `lean` replaced by a bare
    ///   `(offset / span).atan()` fails the *snap* assertion at −18.43° against
    ///   −15°, which is the review's own second flip, no longer green.
    #[test]
    fn every_edge_leans_its_own_side_and_holds_the_one_facing_it() {
        // 200 across and 120 down, so a transposed span shows up as a different
        // magnitude rather than as the same number twice.
        let box_ = Rect::new(0.0, 0.0, 200.0, 120.0);
        for handle in Handle::EDGES {
            let press = grip(box_, handle);
            // Along the side's own length: sideways for a horizontal edge, down
            // for a vertical one. Twenty units, well inside the clamp.
            let at = match handle {
                Handle::Top | Handle::Bottom => press + Vec2::new(20.0, 0.0),
                _ => press + Vec2::new(0.0, 20.0),
            };
            let shear = side_shear(box_, handle, SideDrag { press, at }, false)
                .unwrap_or_else(|| panic!("{handle:?} is an edge and must shear"));

            let (grabbed, held) = probe(box_, handle);
            let still = shear * held;
            assert!(
                (still - held).hypot() < 1e-9,
                "{handle:?}: the held side moved, by {:?}",
                still - held
            );
            // The grabbed corner travels the drag's own displacement: the press
            // was at the middle of the side and the lean is measured from it, so
            // `atan(20/span)` about the far side puts this corner exactly 20 on.
            let want = at - press;
            let moved = shear * grabbed;
            assert!(
                ((moved - grabbed) - want).hypot() < 1e-9,
                "{handle:?}: the grabbed side moved by {:?}, wanted {want:?}",
                moved - grabbed
            );
        }
    }

    /// **Shift snaps the lean to 15°, and the snap lands before the clamp** — the
    /// ordering `side_shear`'s own comment pins, *"so that its top step is a
    /// round 75° rather than the clamp's 89°"*.
    ///
    /// ⚠️ **The third case is what makes the ordering assertable at all.** A drag
    /// far past the clamp snaps to 90°, which is a step, and is then pulled in to
    /// `MAX_SKEW`; snapping *after* the clamp would round 89° back up to 90°
    /// and leave it there, which is the degenerate matrix the clamp exists to
    /// avoid. Only a drag past the clamp can tell the two orders apart.
    ///
    /// **The first case is the anti-vacuity control**: it asserts the fixture's
    /// own raw angle is 18.43°, so "snapped to 15" means something moved rather
    /// than 15 arriving by coincidence.
    #[test]
    fn shift_snaps_a_lean_to_fifteen_degree_steps_before_the_clamp_pulls_it_in() {
        let box_ = Rect::new(0.0, 0.0, 200.0, 120.0);
        let press = Point::new(100.0, 0.0);
        // `skew_x` puts `tan(a)` in the matrix's xy slot, so this is the lean.
        let lean_of = |a: Affine| a.as_coeffs()[2].atan();
        let drag = |dx: f64, snap: bool| {
            side_shear(
                box_,
                Handle::Top,
                SideDrag {
                    press,
                    at: press + Vec2::new(dx, 0.0),
                },
                snap,
            )
            .expect("an edge shears")
        };

        // 40 across a 120 drop is atan(1/3) = 18.43°, which is nearer 15 than 30.
        //
        // ⚠️ **Negative, and the sign is asserted rather than taken away with an
        // `abs()`.** `Handle::Top`'s span is `min_y − max_y`, i.e. −120, so a
        // rightward drag on the top edge is a negative `skew_x` — which under
        // y-down is the top leaning right. Asserting the magnitude alone would
        // pass for the arm that leans the wrong way, which is one of the two the
        // review's green flip produced.
        assert!(
            (lean_of(drag(40.0, false)).to_degrees() + 18.4349).abs() < 1e-3,
            "the fixture's own angle moved: {}",
            lean_of(drag(40.0, false)).to_degrees()
        );
        assert!(
            (lean_of(drag(40.0, true)).to_degrees() + 15.0).abs() < 1e-9,
            "Shift did not snap: {}",
            lean_of(drag(40.0, true)).to_degrees()
        );
        assert!(
            (lean_of(drag(100_000.0, true)) + MAX_SKEW).abs() < 1e-9,
            "the clamp did not run after the snap: {}",
            lean_of(drag(100_000.0, true)).to_degrees()
        );
    }

    /// A corner has no side facing it, so there is nothing for the gesture to
    /// mean there — and a box too thin to lean answers the same way.
    #[test]
    fn a_corner_and_a_slit_have_no_lean() {
        let box_ = Rect::new(0.0, 0.0, 200.0, 120.0);
        let drag = SideDrag {
            press: Point::new(0.0, 0.0),
            at: Point::new(20.0, 20.0),
        };
        assert!(side_shear(box_, Handle::TopLeft, drag, false).is_none());
        let slit = Rect::new(0.0, 0.0, 200.0, MIN_EXTENT * 0.5);
        assert!(side_shear(slit, Handle::Top, drag, false).is_none());
    }
}
