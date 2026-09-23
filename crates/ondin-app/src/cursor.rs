//! Canvas cursors that no OS provides a usable one for, as real cursor bitmaps.
//!
//! Only the text I-beam survives as a stock cursor, set straight from
//! `CursorIcon` in `canvas.rs`. The rest are here:
//!
//! - **The hands.** Windows ships no open- or closed-hand cursor, so winit maps
//!   *both* `Grab` and `Grabbing` — along with `Move` and `AllScroll` — onto
//!   `IDC_SIZEALL`, the four-headed move arrow
//!   (`winit/src/platform_impl/windows/util.rs`). The two pan states came out
//!   identical, and neither looked like a hand.
//! - **The pen.** `CursorIcon` has no pen at all, on any platform.
//! - **The crosshair.** `IDC_CROSS` exists but is 32 texels across, about twice
//!   what a design tool uses, and there is no smaller variant to ask for.
//! - **The rotate arc.** `CursorIcon` has no rotate on any platform, and no icon
//!   set has the shape: a two-headed arc. Phosphor's rings were tried and are what
//!   [`rotate_arc`] exists to explain.
//! - **The Scale tool's box and the discard bin**, which are not about a missing
//!   platform icon at all — see [`Bitmap::Scale`] and [`Bitmap::Discard`] for what
//!   each of them is for.
//! - **The loaded Image cursor**, which is the crosshair with a count badge on it
//!   ([`Bitmap::Loaded`]). The one badge in the app, and the one drawing here that
//!   is a compound of two others.
//!
//! Every design tool ships its own bitmaps for exactly these. So does this one,
//! except they are not shipped: the hands and the pen are rasterized out of the
//! Phosphor glyphs the tool rail already uses (`hand`, `hand-grabbing`,
//! `pen-nib`, `resize`, `trash-simple`), so a tool's cursor and its rail button
//! are the same drawing, and the three that no icon set has at the right weight or
//! in the right shape — the crosshair's four rectangles, the rotate arc, and the
//! badge the loaded Image cursor hangs off the crosshair — are drawn outright.
//! Both paths meet at [`compose`], which gives them a common white-core and
//! dark-rim treatment so they read as one set. The badge takes more from that than
//! the others: its digits are *holes* in a filled chip, and the rim's dilation is
//! the whole of what makes them dark.
//!
//! egui hands the result to the OS as a true cursor via
//! `Context::set_cursor_image`, which is what keeps it from lagging the pointer or
//! being clipped at the window edge — both of which a cursor painted into the
//! scene would suffer from. A `CursorIcon` is still set alongside every bitmap, as
//! the fallback for platforms that render it properly and for the case where
//! rasterizing fails.

use crate::theme;
use eframe::egui;

/// Which way a rotate cursor faces: a **screen** direction (y down), quantized to
/// [`Turn::STEPS`] steps of a full turn.
///
/// **Quarter turns until 2026-08-19, and the reason they went is what the cursor
/// is for.** Quarters were exact — a whole-quarter rotation of a rasterized
/// coverage field is a permutation of its texels, where an arbitrary angle would
/// have had to resample a 20-texel bitmap and come back soft. That reasoning was
/// sound and is now beside the point, because the arc is **drawn** rather than
/// rasterized from a glyph ([`rotate_arc`]): there is no finished bitmap to
/// resample, the angle goes into the geometry, and every step is as crisply
/// antialiased as every other. What is left is only how finely to cache.
///
/// **15°, which is 24 images.** Fine enough that the cursor tracks a drag rather
/// than snapping — a quarter-turn cursor changed four times in a full revolution,
/// which reads as four discrete states and not as a heading — and coarse enough
/// that the whole set is a couple of hundred texels each, built once. Caching by
/// *something* is not optional: egui dedupes cursor uploads by `Arc` pointer
/// identity, so an unquantized angle would push a new cursor to the OS on every
/// frame of every rotate drag.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub struct Turn(u8);

impl Turn {
    /// Steps in a full turn — 15° apart.
    pub const STEPS: u8 = 24;

    /// The step that faces along `out`, a **screen** direction (y down).
    pub fn toward(out: egui::Vec2) -> Self {
        if out.length_sq() < f32::EPSILON {
            return Self(0);
        }
        let k =
            (out.y.atan2(out.x) / std::f32::consts::TAU * f32::from(Self::STEPS)).round() as i32;
        Self(k.rem_euclid(i32::from(Self::STEPS)) as u8)
    }

    /// The screen angle this step faces, in radians. Step 0 is +x, i.e. due right.
    fn radians(self) -> f32 {
        f32::from(self.0) / f32::from(Self::STEPS) * std::f32::consts::TAU
    }

    fn index(self) -> usize {
        usize::from(self.0)
    }
}

/// How many images a loaded Image-tool cursor still has to place, as its badge
/// draws it.
///
/// **The exact count while it fits in two digits, `99+` beyond**, which is what
/// bounds the cache: egui dedupes cursor uploads by `Arc` pointer identity, so
/// every distinct badge is a bitmap that has to be kept, and an unclamped count
/// would build one per file in a two-hundred-file drop. Ninety-nine is well past
/// the point where the number stops being actionable and the badge is only
/// saying "a lot left".
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Remaining(u8);

impl Remaining {
    /// Above this the badge stops counting.
    const MANY: usize = 100;

    /// The badge for `n` images still to place, or `None` for an empty cursor.
    ///
    /// `None` rather than a zero badge: a tool armed with nothing to place is a
    /// moment that exists only if something failed to disarm it
    /// (`canvas::placed_one_image`), and the plain crosshair is the honest
    /// drawing for it — a badge reading `0` would be a claim that the cursor is
    /// loaded.
    pub fn of(n: usize) -> Option<Self> {
        (n > 0).then(|| Self(n.min(Self::MANY) as u8))
    }

    /// What the badge says.
    fn label(self) -> String {
        match usize::from(self.0) >= Self::MANY {
            true => "99+".to_owned(),
            false => self.0.to_string(),
        }
    }
}

/// A cursor that has to be drawn because no OS provides a usable one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Bitmap {
    /// Panning is armed — space held, or the hand tool chosen.
    HandOpen,
    /// The view is actually being dragged.
    HandClosed,
    /// The pen tool, about to drop an anchor point.
    Pen,
    /// The Scale tool. The one cursor here that is not about a missing platform
    /// icon: an arrow is what Select uses, and the two tools differ only in what
    /// a handle drag *means*, so without a cursor of its own the mode would be
    /// invisible until something had already been scaled.
    Scale,
    /// The shape tools, about to drag out a box.
    Crosshair,
    /// The Image tool's **loaded** cursor: the same crosshair with a badge on it
    /// saying how many files are still to be placed.
    ///
    /// **The count is the only thing that distinguishes this from a shape tool.**
    /// A click here plants a picture rather than starting a box, and without the
    /// badge the two tools show the identical bitmap — so the badge is drawn for a
    /// single remaining image too, where the number alone would be telling the
    /// user something they already know.
    ///
    /// The one badge in the app: modifier badges were declined (§15 D233) because
    /// they would have had to sit on a *stock* cursor with nothing to composite
    /// onto. This one has a drawn base under it and overrides nothing the OS
    /// supplied.
    Loaded(Remaining),
    /// Hovering the band just outside a corner handle, where a drag rotates.
    ///
    /// Turned to face the handle it belongs to, so it says *which* one has the
    /// pointer — the thing the band's 12px of ring makes easy to get wrong. One
    /// arrow held at a fixed angle read as a decoration that happened to mean
    /// "rotate".
    Rotate(Turn),
    /// A guide dragged back onto its ruler, where letting go throws it away.
    ///
    /// **The gesture had no cursor of its own.** Both the bar and the line answered
    /// the ordinary axis move cursor, so the only signal that a release would
    /// *delete* the guide was the line being drawn faded — which is easy to read as
    /// the line simply being under the ruler. A bin is what macOS and Sketch use
    /// for "drop this to discard it", and it is the message the stock icons cannot
    /// carry: `NoDrop` would say the drop is refused, when in fact it is accepted
    /// and destructive.
    Discard,
    /// The Text tool over the outline of a shape, where a click sets type **along
    /// that path** rather than planting a new layer (§15 D408).
    ///
    /// **The affordance that replaces a menu route.** Setting text on a curve was
    /// reachable only by making a text layer, selecting it with the shape and
    /// finding *Text on path* in a context menu that is already long. Affinity
    /// offers it as a cursor: hover an edge with the type tool and the pointer says
    /// you may write there. That is one gesture instead of four, and it is
    /// *discoverable*, which a menu row three levels into a selection is not.
    ///
    /// **Drawn rather than a Phosphor glyph**, and the only cursor here that is a
    /// composition of two ideas: an I-beam, which is what the platform already
    /// means by "text goes here", over a curve, which is the half that says *which*
    /// text and where. No icon in the set says both — `TEXT_UNDERLINE` is the
    /// nearest and it means underline — and the stock `CursorIcon::Text` is exactly
    /// what this must not be confused with, since that one is already in use eight
    /// lines away for "click to edit the text under the pointer".
    TextOnPath,
}

impl Bitmap {
    /// The Phosphor glyph this cursor is drawn from and where in it the pointer
    /// aims — or `None` for the ones drawn from scratch.
    ///
    /// 🚨 **One function because it used to be two, and four of the second one's
    /// answers were read by nothing** (§15 D682, `[S17-L3-06]`).
    /// [`Cursors::image`] routes `Crosshair`, `Rotate`, `Loaded` and `TextOnPath`
    /// to functions that pass their own [`Hotspot`], so only the `other =>
    /// rasterize(…)` arm ever asked — and `other` is exactly the five variants
    /// below that have a glyph. `Loaded` and `TextOnPath` said *"Read by nothing"*
    /// in their own arms; `Crosshair` and `Rotate` instead carried a **reason** for
    /// a value nothing read (*"a crosshair points at its own centre — which is why
    /// it has a gap there"*), so somebody moving the crosshair's aiming point would
    /// edit that line, see no change, and have to find `Cross::aim`.
    ///
    /// ⚠️ **§15 D236 is the precedent and it is the argument for merging rather
    /// than annotating**: it was written for this exact shape on the `Loaded` arm
    /// alone, and the shape came back on two more. Pairing the glyph with its
    /// hotspot means a variant with no glyph has nowhere to put a hotspot, so the
    /// routing in `image` is the only place that decides and the two lists cannot
    /// drift apart again.
    fn glyph_and_hotspot(self) -> Option<(&'static str, Hotspot)> {
        match self {
            // A hand grabs with its palm, and the bin points at itself: what is
            // being aimed at is the ruler under the whole pointer, not a particular
            // texel of it. The scale glyph is a box being pulled from its middle.
            Self::HandOpen => Some((theme::icon::HAND, Hotspot::Middle)),
            Self::HandClosed => Some((theme::icon::HAND_GRABBING, Hotspot::Middle)),
            // A pen draws from its nib, and the click has to land exactly where
            // the anchor point will appear.
            Self::Pen => Some((theme::icon::PEN_NIB, Hotspot::Nib)),
            // The rail button's own glyph, so the tool and its pointer are one
            // drawing — the same bargain the hands and the pen make.
            Self::Scale => Some((theme::icon::RESIZE, Hotspot::Middle)),
            Self::Discard => Some((theme::icon::TRASH_SIMPLE, Hotspot::Middle)),
            // All drawn outright, each naming its own aiming point where it is
            // drawn — see [`crosshair`], [`rotate_arc`], [`loaded_crosshair`] and
            // [`text_on_path`]. Two of them have to: the loaded crosshair's badge
            // grows the buffer to one side and the path cursor's curve hangs below
            // the I-beam, so in neither is the middle of the buffer the point being
            // aimed with.
            Self::Crosshair | Self::Rotate(_) | Self::Loaded(_) | Self::TextOnPath => None,
        }
    }
}

/// Where in its own bitmap a cursor points.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hotspot {
    Middle,
    /// A named texel of the **coverage** field, before [`compose`] pads it with
    /// the rim. For a drawing whose buffer is not symmetric about the point it
    /// aims through, which is what a badge hung off one corner makes of the
    /// crosshair: `Middle` would follow the badge and take the aiming point with
    /// it.
    At(i32, i32),
    /// The ink nearest the top-left corner, once the glyph has been mirrored so
    /// the pen leans like every other cursor (see [`rasterize`]). Found rather
    /// than hard-coded, so it stays on the nib if the glyph or its size changes.
    Nib,
}

/// Logical size the glyph is rasterized at. The bitmap comes out at this many
/// *physical* texels times the display scale, so a hi-dpi screen gets a sharper
/// cursor rather than a stretched one.
const GLYPH_PT: f32 = 19.0;
/// Outline thickness in *points*, matched to what a stock OS cursor uses — one
/// thin dark edge, enough that white line art does not vanish against a white
/// artboard and no more. A flat two texels read as a black cursor with white
/// detail, and was wide enough to close the gaps between the fingers.
///
/// Under 1.0 on purpose. This is multiplied by the display scale and rounded, so
/// a flat `1.0` would come back to two texels on the 125%/150% displays that are
/// the Windows norm — i.e. no thinner than the version this replaced. At 0.75 the
/// rim is one texel up to 175% and two only from 200%, where a single texel would
/// be a hairline.
const RIM_PT: f32 = 0.75;
/// Below this fraction of covered pixels the rasterization is assumed to have
/// failed (missing glyph, atlas miss) and the bitmap is discarded.
const MIN_INK: f32 = 0.02;

/// [`RIM_PT`] in physical texels. The source is in physical texels, so the rim has
/// to scale with the display or it would read as twice as thick on a 1x screen as
/// on a 2x. Shared with [`Cross`], which has to know how far a badge must stand
/// off to clear it.
fn rim_px(scale: f32) -> i32 {
    ((RIM_PT * scale).round() as i32).max(1)
}

/// One cached bitmap. The outer `Option` is "not built yet", the inner one is
/// "built and failed" — kept apart so a glyph that cannot be rasterized is not
/// retried on every frame of every gesture.
type Slot = Option<Option<egui::CustomCursorImage>>;

/// The drawn cursors, built on first use and kept for as long as the display
/// scale stays put.
///
/// Caching is not just an optimization. egui's integration dedupes by `Arc`
/// pointer identity, so handing it a freshly allocated buffer every frame would
/// upload a new cursor to the OS every frame.
#[derive(Default)]
pub struct Cursors {
    /// Scale the cached bitmaps were rasterized for.
    scale: f32,
    hand_open: Slot,
    hand_closed: Slot,
    pen: Slot,
    /// The Scale tool's. Named for the tool, because `scale` above is already the
    /// display scale these bitmaps are rasterized for.
    scale_tool: Slot,
    crosshair: Slot,
    /// One per [`Turn`], built on demand rather than up front — a rotate drag
    /// sweeps through the lot, but a session that never rotates anything builds
    /// none of them.
    rotate: [Slot; Turn::STEPS as usize],
    discard: Slot,
    text_on_path: Slot,
    /// One per count the cursor has actually been loaded with. A map rather than
    /// an array because [`Remaining`] spans a hundred values and a session
    /// normally touches a handful of them, counting down.
    loaded: std::collections::HashMap<Remaining, Slot>,
}

impl Cursors {
    /// The bitmap for `which`, or `None` if it could not be rasterized — in which
    /// case the caller's `CursorIcon` stands on its own.
    pub fn image(&mut self, ctx: &egui::Context, which: Bitmap) -> Option<egui::CustomCursorImage> {
        let scale = ctx.pixels_per_point();
        if self.scale != scale {
            *self = Self {
                scale,
                ..Default::default()
            };
        }
        let slot = match which {
            Bitmap::HandOpen => &mut self.hand_open,
            Bitmap::HandClosed => &mut self.hand_closed,
            Bitmap::Pen => &mut self.pen,
            Bitmap::Scale => &mut self.scale_tool,
            Bitmap::Crosshair => &mut self.crosshair,
            Bitmap::Rotate(t) => &mut self.rotate[t.index()],
            Bitmap::Discard => &mut self.discard,
            Bitmap::TextOnPath => &mut self.text_on_path,
            Bitmap::Loaded(n) => self.loaded.entry(n).or_default(),
        };
        slot.get_or_insert_with(|| match which {
            Bitmap::TextOnPath => text_on_path(scale),
            Bitmap::Crosshair => crosshair(scale),
            Bitmap::Rotate(t) => rotate_arc(scale, t.radians()),
            Bitmap::Loaded(n) => loaded_crosshair(ctx, scale, n),
            // §15 D682: the glyph and its aiming point come out together, so a
            // variant routed above cannot also carry a hotspot nothing reads.
            other => {
                let (glyph, hotspot) = other.glyph_and_hotspot()?;
                rasterize(ctx, glyph, hotspot)
            }
        })
        .clone()
    }
}

/// Rasterize one icon glyph into a cursor bitmap.
fn rasterize(
    ctx: &egui::Context,
    glyph: &str,
    hotspot: Hotspot,
) -> Option<egui::CustomCursorImage> {
    // Laying the glyph out is what gets it rasterized into the font atlas, and
    // the resulting galley says where in the atlas it landed.
    let galley = ctx.fonts_mut(|f| {
        f.layout_no_wrap(
            glyph.to_owned(),
            theme::icon_font(GLYPH_PT),
            egui::Color32::WHITE,
        )
    });
    let uv = galley.rows.first()?.row.glyphs.first()?.uv_rect;
    if uv.is_nothing() {
        return None;
    }
    let atlas = ctx.fonts(|f| f.image());
    let (gw, gh) = (
        i32::from(uv.max[0]) - i32::from(uv.min[0]),
        i32::from(uv.max[1]) - i32::from(uv.min[1]),
    );
    if gw <= 0 || gh <= 0 {
        return None;
    }

    // Glyph coverage lives in the atlas alpha channel — glyphs are written as
    // premultiplied white, so alpha *is* the coverage.
    let source = |x: i32, y: i32| {
        if x < 0 || y < 0 || x >= gw || y >= gh {
            return 0;
        }
        let (ax, ay) = (
            usize::from(uv.min[0]) + x as usize,
            usize::from(uv.min[1]) + y as usize,
        );
        atlas
            .pixels
            .get(ay * atlas.size[0] + ax)
            .map_or(0, |p| u32::from(p.a()))
    };
    let (w, h) = (gw, gh);
    compose(ctx.pixels_per_point(), (w, h), hotspot, |x, y| {
        // **Mirrored, not turned, for a nib.** Phosphor draws `pen-nib` with the
        // tip at the bottom-left and the barrel up to the top-right, which reads
        // as a pen leaning the wrong way: every other cursor on the system points
        // up-left and trails to the bottom-right. The fix is a reflection in the
        // horizontal axis — the tip goes to the top-left and the barrel follows
        // to the bottom-right — and a reflection is not a rotation at all, so no
        // turn of the whole bitmap gets there. A 180° one would put the tip
        // top-*right* and the barrel bottom-left, which is a third wrong answer.
        let y = if hotspot == Hotspot::Nib {
            h - 1 - y
        } else {
            y
        };
        source(x, y)
    })
}

/// A thin crosshair, drawn rather than taken from anywhere.
///
/// The stock `IDC_CROSS` is 32 texels of arm-to-arm cross and reads as enormous
/// next to a design tool's own; Phosphor's `crosshair` glyphs carry a ring that
/// makes them a target reticle rather than a precision cursor. Neither is what
/// the shape tools want, and the shape is four rectangles, so it is drawn here.
///
/// The geometry is [`Cross`], which the loaded Image cursor draws too.
fn crosshair(scale: f32) -> Option<egui::CustomCursorImage> {
    let cross = Cross::new(scale);
    let aim = cross.aim();
    compose(
        scale,
        (cross.size, cross.size),
        Hotspot::At(aim, aim),
        move |x, y| cross.ink(x, y),
    )
}

/// The Text tool's *type along this path* pointer: an I-beam over a curve
/// (§15 D408).
///
/// **Two marks, and each is doing a different job.** The I-beam is the platform's
/// own word for "text goes here" and is what makes the cursor read as the type
/// tool at all; the curve under it is what separates this from
/// `CursorIcon::Text`, which this tool already shows for "click to edit the text
/// under the pointer". Confusing the two would be worse than having neither,
/// because they are eight lines apart in the same match and one plants a layer
/// while the other enters one.
///
/// **The curve is a circular arc rather than a wave**, which was the first
/// drawing. At sixteen texels a wave's two bends land on about three texels each
/// and read as a thick smudge; one arc keeps a legible bend at 1x, and the shape
/// that has to survive is *curved rather than straight*, not the particular curve.
///
/// Sizes are in points and scaled, so the pointer is the same physical size on any
/// display — the rule every drawn cursor here follows.
fn text_on_path(scale: f32) -> Option<egui::CustomCursorImage> {
    /// Half the arc's width, and so half the whole figure's.
    ///
    /// **Wider than the I-beam on purpose**: an arc the beam's own width reads as a
    /// bowl the type is sitting *in*, and the mark has to say the curve carries on
    /// past the letters. Measured by looking at both — the first drawing was the
    /// same width as the serif and came out as a `U`.
    const HALF_W_PT: f32 = 5.0;
    /// Half the I-beam's serif.
    const SERIF_PT: f32 = 2.0;
    /// The stem's height, serifs included.
    const BEAM_H_PT: f32 = 9.0;
    /// Clear space between the beam's foot and the arc's apex.
    const GAP_PT: f32 = 1.5;
    /// How far the arc's ends hang below its middle. Shallow: this is a hint of a
    /// curve at cursor size, and a deep one reads as a bracket.
    const SAG_PT: f32 = 2.0;

    let pt = |v: f32| ((v * scale).round() as i32).max(1);
    let (half_w, serif, beam_h) = (pt(HALF_W_PT), pt(SERIF_PT), pt(BEAM_H_PT));
    let (gap, sag, thick) = (pt(GAP_PT), pt(SAG_PT), pt(1.0));
    let w = half_w * 2 + thick;
    let cx = w / 2;
    // **The apex is under the stem and the ends hang away from it**, so the figure
    // reads as type standing *on* a curve rather than sitting in one. That is the
    // relationship the feature has: the baseline is the path.
    let apex = beam_h + gap;
    let h = apex + sag + thick;

    // The circle through the apex and the two low ends, as centre and radius: the
    // chord-and-sagitta solve, `R = (c²/4 + s²) / 2s`, with the centre *below* the
    // apex — which is the half that makes it a hill rather than a bowl.
    //
    // ⚠️ **The centre is `w / 2` as a *length*, not `cx` as an index**, and the
    // half-texel between them is visible: sampling texel middles against a circle
    // centred on an index put the arc's right end a whole row below its left, on a
    // figure that is symmetric by construction. The chord is `w − 1` for the same
    // reason — it runs between the middles of the first and last texels.
    let (c, s) = (f64::from(w) - 1.0, f64::from(sag).max(1.0));
    let r = (c * c / 4.0 + s * s) / (2.0 * s);
    let (ccx, ccy) = (f64::from(w) / 2.0, f64::from(apex) + r);

    let half = f64::from(thick) / 2.0;
    compose(scale, (w, h), Hotspot::At(cx, beam_h / 2), move |x, y| {
        if x < 0 || y < 0 || x >= w || y >= h {
            return 0;
        }
        // The I-beam: a stem, and a serif at each end.
        //
        // 🚨 **Measured from `ccx`, the same *length* the arc uses, and not from
        // `cx`** (§15 D587, `[S17-L1-04]`). This is the half-texel confusion the
        // ⚠️ above records being found and fixed for the arc, applied to the arc
        // and not to the beam. The buffer's mirror is `x ↔ w − 1 − x`, whose fixed
        // point is `(w − 1) / 2`; `cx` is that axis only when `w` is odd, i.e.
        // only when `thick` is odd, since `w = half_w * 2 + thick`. `pt(1.0)` is
        // **even** at exactly 1.5×, 1.75×, 2.0× and 2.5× — the first three of
        // which are Windows norms — so the beam sat half a texel right of the arc
        // it stands on and the serif, being `cx − serif ..= cx + serif`, was one
        // column right-heavy. Measured over the mirror: 16, 16 and 22 lopsided
        // texels at 1.5×, 1.75× and 2.0×, and 0 at 1.0×, 1.25×, 2.5× and 3.0× —
        // 2.5× coming out clean only because `half_w`'s own parity shifts back.
        //
        // Sampling texel middles against `ccx` makes both marks symmetric at every
        // scale and costs an even-width buffer one extra serif column, which is
        // what symmetry there means: an even figure has no centre column.
        let from_middle = (f64::from(x) + 0.5 - ccx).abs();
        let stem = from_middle < half && y < beam_h;
        let serif = (y < thick || (beam_h - thick..beam_h).contains(&y))
            && from_middle <= f64::from(serif) + 0.5;
        // The arc: texels whose distance from the centre is the radius. `+ 0.5`
        // samples the texel's middle, which is what keeps the curve from sitting a
        // half-texel high against the beam above it.
        let (dx, dy) = (f64::from(x) + 0.5 - ccx, f64::from(y) + 0.5 - ccy);
        let arc = y >= apex && ((dx * dx + dy * dy).sqrt() - r).abs() <= half;
        u32::from(stem || serif || arc) * 255
    })
}

/// The crosshair's geometry in physical texels, split out because [`loaded_crosshair`]
/// draws the same four rectangles into a **larger** buffer and has to know where
/// they end — where the badge goes, and which texel the whole thing still aims
/// through.
///
/// Sizes are in points and scaled, so the cursor is the same physical size on any
/// display. The gap at the centre is the point of it — it is what lets you see the
/// pixel you are aiming at.
#[derive(Clone, Copy)]
struct Cross {
    /// Arm length from the centre gap outward. Sized so the whole cursor lands
    /// near half of `IDC_CROSS`'s 32 texels, which is roughly where the other
    /// design tools put theirs.
    arm: i32,
    /// Clear space each side of centre.
    gap: i32,
    /// Width of an arm.
    thick: i32,
    /// Where the centre band starts: the far arms begin `thick + gap` past it.
    near: i32,
    /// The whole figure, on both axes.
    size: i32,
    /// What [`compose`] will add all round, which the badge has to clear.
    rim: i32,
}

impl Cross {
    fn new(scale: f32) -> Self {
        const ARM_PT: f32 = 5.0;
        const GAP_PT: f32 = 1.5;

        let pt = |v: f32| ((v * scale).round() as i32).max(1);
        let (arm, gap, thick) = (pt(ARM_PT), pt(GAP_PT), pt(1.0));
        // Arms occupy `[arm+gap, arm+gap+thick)` on the cross axis, so the whole
        // bitmap is exactly the crosshair's extent.
        let near = arm + gap;
        Self {
            arm,
            gap,
            thick,
            near,
            size: near + thick + gap + arm,
            rim: rim_px(scale),
        }
    }

    /// Coverage of the four rectangles.
    ///
    /// **Bounded, unlike the closure this came out of.** That one was handed a
    /// buffer exactly its own size, so anything outside was unreachable; this one
    /// is called over a buffer the badge has grown, where an unbounded `on_arm`
    /// reads true for every `x` past the end and would run the horizontal arm off
    /// to the badge and beyond.
    fn ink(&self, x: i32, y: i32) -> u32 {
        if x < 0 || y < 0 || x >= self.size || y >= self.size {
            return 0;
        }
        let on_axis = |v: i32| v >= self.near && v < self.near + self.thick;
        let on_arm = |v: i32| v < self.arm || v >= self.near + self.thick + self.gap;
        u32::from((on_axis(x) && on_arm(y)) || (on_axis(y) && on_arm(x))) * 255
    }

    /// The texel the cursor aims through, on both axes — the middle of the gap.
    ///
    /// The same point `Hotspot::Middle` computes for the plain crosshair, whose
    /// buffer is symmetric about it; stated here because the loaded one's is not,
    /// and `the_badge_does_not_move_the_aiming_point` is what holds the two together.
    fn aim(&self) -> i32 {
        self.near + self.thick / 2
    }

    /// Where a badge's top-left corner goes: diagonally out from the centre, past
    /// the gap, in the quadrant the four arms leave empty.
    ///
    /// **Plus the rim, which is the part a picture shows and arithmetic does not.**
    /// Lining the chip up with where the arm's *coverage* ends leaves the arm's
    /// dark edge and the chip's sharing the column between them, and a badge whose
    /// outline is welded to the arm reads as one shape rather than two.
    fn badge_at(&self) -> i32 {
        self.near + self.thick + self.gap + self.rim
    }
}

/// Size of the badge's digits. Nine points is the smallest that survived being
/// read at 1x, where a digit is seven texels tall and a stroke is one: at eight
/// the `2` of a `12` closes up.
const BADGE_PT: f32 = 9.0;
/// Clear space between the digits and the chip's edge, each side.
const BADGE_PAD_PT: f32 = 1.0;
/// Corner radius of the chip, so it reads as a badge rather than as a second
/// rectangle of the cursor. Clamped against the chip's own size below — at 1x a
/// one-digit chip is five texels across and a full two-texel radius would eat
/// most of it.
const BADGE_ROUND_PT: f32 = 2.0;

/// The Image tool's loaded cursor: [`crosshair`] with a count badge hung off the
/// bottom-right.
///
/// **The digits are knocked *out* of a solid chip, not drawn as ink.** Both were
/// dumped as ASCII art before this was written, and drawn ink loses at the size
/// that matters: [`compose`] gives every shape a white core and a dark rim, and a
/// nine-point digit at 1x is a one-texel stroke, so drawing it puts a white
/// hairline inside a black outline — a blob. Knocked out, the same treatment
/// paints the chip white and the digits' holes fall in the rim's dilation and come
/// out dark, which is a black number on a white badge at every scale. The badge
/// asks nothing of `compose` that the other cursors do not.
///
/// **The hotspot stays on the crosshair's aiming point**, which is the trap here:
/// the buffer grows down and right to make room, so the middle of it is no longer
/// the middle of the cross. [`Hotspot::At`] names the texel instead.
fn loaded_crosshair(
    ctx: &egui::Context,
    scale: f32,
    count: Remaining,
) -> Option<egui::CustomCursorImage> {
    let cross = Cross::new(scale);
    let (bw, bh, chip) = badge(ctx, scale, &count.label())?;
    let at = cross.badge_at();
    let aim = cross.aim();
    // The badge only ever grows the buffer — a chip smaller than the arms it hangs
    // beside leaves the crosshair's own extent standing.
    let size = ((at + bw).max(cross.size), (at + bh).max(cross.size));

    compose(scale, size, Hotspot::At(aim, aim), move |x, y| {
        let (bx, by) = (x - at, y - at);
        let on_chip = if bx < 0 || by < 0 || bx >= bw || by >= bh {
            0
        } else {
            u32::from(chip[(by * bw + bx) as usize])
        };
        cross.ink(x, y).max(on_chip)
    })
}

/// The badge's coverage: a rounded chip with `label` knocked out of it.
///
/// Returns its size and the field, both in physical texels.
fn badge(ctx: &egui::Context, scale: f32, label: &str) -> Option<(i32, i32, Vec<u8>)> {
    let (tw, th, text) = digits(ctx, scale, label)?;
    let pad = ((BADGE_PAD_PT * scale).round() as i32).max(1);
    // **Never narrower than it is tall.** Inter's `1` is three texels of ink at
    // 1x where a `4` is six, so padding alone gives a one-digit badge a chip half
    // the width of a two-digit one — a white bar rather than a badge. A square
    // floor keeps every count reading as the same object, and the digits centre
    // in whatever room that leaves.
    let h = th + pad * 2;
    let w = (tw + pad * 2).max(h);
    let inset = (w - tw) / 2;
    // A radius is a fraction of the smaller side or it swallows a narrow chip.
    let r = (BADGE_ROUND_PT * scale)
        .round()
        .min((w.min(h) / 3) as f32)
        .max(0.0);

    let mut cov = vec![0u8; (w * h) as usize];
    for y in 0..h {
        for x in 0..w {
            // Outside the corner arcs is off the chip. Measured from the texel's
            // centre to the nearest point of the rounded rect's inner box, which
            // is the corner circle's own centre in a corner and the texel itself
            // everywhere else.
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let (cx, cy) = (px.clamp(r, w as f32 - r), py.clamp(r, h as f32 - r));
            if (px - cx).powi(2) + (py - cy).powi(2) > r * r {
                continue;
            }
            let (gx, gy) = (x - inset, y - pad);
            let ink = if gx < 0 || gy < 0 || gx >= tw || gy >= th {
                0
            } else {
                text[(gy * tw + gx) as usize]
            };
            cov[(y * w + x) as usize] = 255 - ink;
        }
    }
    Some((w, h, cov))
}

/// One short string's coverage, lifted out of the font atlas the same way
/// [`rasterize`] lifts a single glyph — laying it out is what rasterizes it, and
/// each glyph's `uv_rect` says where it landed.
///
/// **The positions come back in points and the atlas is in texels**, which is the
/// one thing this has to do that a single glyph did not: a glyph's `pos` and its
/// `uv_rect.offset` are logical, so they are scaled here. Getting that wrong is
/// invisible at 1x and shears the digits apart at 1.5x.
fn digits(ctx: &egui::Context, scale: f32, text: &str) -> Option<(i32, i32, Vec<u8>)> {
    let galley = ctx.fonts_mut(|f| {
        f.layout_no_wrap(
            text.to_owned(),
            egui::FontId::new(BADGE_PT, egui::FontFamily::Proportional),
            egui::Color32::WHITE,
        )
    });
    // Read into a local before touching the context again: two `Context`
    // accessors nested take the same lock and deadlock.
    let placed: Vec<_> = galley
        .rows
        .iter()
        .flat_map(|row| row.row.glyphs.iter().map(move |g| (row.pos, g)))
        .filter(|(_, g)| !g.uv_rect.is_nothing())
        .map(|(origin, g)| {
            let uv = g.uv_rect;
            (
                ((origin.x + g.pos.x + uv.offset.x) * scale).round() as i32,
                ((origin.y + g.pos.y + uv.offset.y) * scale).round() as i32,
                i32::from(uv.max[0]) - i32::from(uv.min[0]),
                i32::from(uv.max[1]) - i32::from(uv.min[1]),
                uv.min,
            )
        })
        .filter(|&(_, _, w, h, _)| w > 0 && h > 0)
        .collect();
    // 🚨 **Before the fold, not after it** (§15 D684, `[S17-L1-09]`). The guard for
    // an empty label used to be the `w <= 0 || h <= 0` below, which is one statement
    // too late to see this state: with nothing placed, the fold returns its seeds
    // unchanged and `hi_x - lo_x` is `i32::MIN - i32::MAX`. That is not a small
    // wrong number, it is an **overflow** — a debug build panics with *"attempt to
    // subtract with overflow"*, and a release build wraps to `1` and goes on to
    // index a one-byte buffer with glyph offsets.
    //
    // ⚠️ **Not reachable from the app today**, and the arithmetic is the reason it
    // is written down anyway: the only labels are `Remaining::label`'s `"1"`–`"99"`
    // and `"99+"`, Inter carries all of them, and `theme::install` has run before
    // any cursor is built. What is one line from being right is the guard the whole
    // `Option` chain above this — `badge(…)?` in `loaded_crosshair` — is written to
    // receive. `rasterize`, the sibling, already tests before it subtracts.
    if placed.is_empty() {
        return None;
    }
    let (lo_x, lo_y, hi_x, hi_y) = placed.iter().fold(
        (i32::MAX, i32::MAX, i32::MIN, i32::MIN),
        |(lx, ly, hx, hy), &(x, y, w, h, _)| (lx.min(x), ly.min(y), hx.max(x + w), hy.max(y + h)),
    );
    let (w, h) = (hi_x - lo_x, hi_y - lo_y);
    if w <= 0 || h <= 0 {
        return None;
    }

    let atlas = ctx.fonts(|f| f.image());
    let mut cov = vec![0u8; (w * h) as usize];
    for (x0, y0, gw, gh, min) in placed {
        for y in 0..gh {
            for x in 0..gw {
                let a = atlas
                    .pixels
                    .get(
                        (usize::from(min[1]) + y as usize) * atlas.size[0]
                            + usize::from(min[0])
                            + x as usize,
                    )
                    .map_or(0, |p| p.a());
                let slot = &mut cov[((y0 - lo_y + y) * w + x0 - lo_x + x) as usize];
                *slot = (*slot).max(a);
            }
        }
    }
    Some((w, h, cov))
}

/// The rotate cursor: a stroked arc with an arrowhead at each end, facing
/// `angle` — a **screen** angle in radians, y down, pointing from the pivot out
/// towards the handle under the pointer.
///
/// **Drawn rather than taken from Phosphor, because the glyph could not carry the
/// one thing this cursor exists to say.** `arrow-clockwise` is a ring open down
/// its right side, and at 19pt a ring turned by any amount is still a ring: the
/// only moving evidence was a few texels of gap and arrowhead, so the cursor's
/// orientation — which is *which handle you have* — was invisible. Phosphor has
/// no two-headed arc to swap in either; `arrow-arc-right` is single-headed and
/// `arrows-clockwise` is another ring. An arc of about 120° with a head at each
/// end is asymmetric enough to read at a glance, which is why every other design
/// tool draws one, and it is a few dozen lines of arithmetic at this size.
///
/// **Two heads, not one.** A rotate drag goes either way, and a single head would
/// be a claim about which.
///
/// **Sized from the circumscribed circle**, which is what makes the angle free.
/// Rotating a finished square bitmap needs √2 the extent at 45° and the rotation
/// taken about the hotspot, or the drawing clips at exactly the odd angles — the
/// last place anyone looks. Here the figure is a disc about its own hotspot by
/// construction: its outermost ink is `RADIUS_PT + HEAD_W_PT` away whatever the
/// angle, so nothing has to grow and nothing can clip. The angle goes into the
/// geometry rather than onto a rasterized result, so there is no resampling and no
/// softness either.
fn rotate_arc(scale: f32, angle: f32) -> Option<egui::CustomCursorImage> {
    /// Radius of the arc's centreline.
    const RADIUS_PT: f32 = 5.6;
    /// Width of the band.
    const THICK_PT: f32 = 1.9;
    /// Half an arrowhead's base, measured radially — so the base is wider than the
    /// band it caps, which is what makes it read as a head rather than a blunt end.
    const HEAD_W_PT: f32 = 2.3;
    /// An arrowhead's length, along the tangent. Longer than its base is wide, or
    /// it comes out squat and the head reads as the band merely flaring.
    const HEAD_L_PT: f32 = 4.0;
    /// Half the *band's* sweep, in radians. Each head adds `HEAD_L / RADIUS` more,
    /// so the whole figure spans about 128° — near enough the 120° that reads, and
    /// far enough from 360° that it cannot be mistaken for a ring.
    const BAND: f32 = 0.40;
    /// Samples per texel per axis. The band and the heads are tested as plain
    /// insideness and the antialiasing comes from counting hits, which is exact
    /// enough at 16 samples and much harder to get subtly wrong than a signed
    /// distance to a triangle would be.
    const SS: i32 = 4;

    let (r, t, hw, hl) = (
        RADIUS_PT * scale,
        THICK_PT * scale,
        HEAD_W_PT * scale,
        HEAD_L_PT * scale,
    );
    // The outermost ink is an arrowhead's base corner, at `r + hw`; the apex is
    // nearer, at `hypot(r, hl)`. One texel of slack for the antialiased fringe.
    let half = (r + hw).ceil() as i32 + 1;
    let n = half * 2 + 1;

    // In the arc's own frame, with its midpoint on +x.
    let cross = |u: egui::Vec2, v: egui::Vec2| u.x * v.y - u.y * v.x;
    let in_triangle = |p: egui::Vec2, a: egui::Vec2, b: egui::Vec2, c: egui::Vec2| {
        let (d1, d2, d3) = (
            cross(b - a, p - a),
            cross(c - b, p - b),
            cross(a - c, p - c),
        );
        (d1 >= 0.0 && d2 >= 0.0 && d3 >= 0.0) || (d1 <= 0.0 && d2 <= 0.0 && d3 <= 0.0)
    };
    let inside = |p: egui::Vec2| {
        if (p.length() - r).abs() <= t * 0.5 && p.y.atan2(p.x).abs() <= BAND {
            return true;
        }
        // A head at each end of the band, sitting on the tangent and pointing
        // away from it — `s` is which end, and which way "away" is.
        [1.0f32, -1.0].into_iter().any(|s| {
            let (sn, cs) = (s * BAND).sin_cos();
            let (base, radial) = (egui::vec2(cs, sn) * r, egui::vec2(cs, sn));
            let apex = base + egui::vec2(-sn, cs) * s * hl;
            in_triangle(p, apex, base + radial * hw, base - radial * hw)
        })
    };

    // Precomputed rather than sampled on demand: `compose` calls its coverage
    // closure once per texel *and* once per texel of the rim's dilation window, so
    // an inline supersample would run the geometry nine times over.
    let (sa, ca) = angle.sin_cos();
    let centre = half as f32 + 0.5;
    let mut grid = vec![0u32; (n * n) as usize];
    for y in 0..n {
        for x in 0..n {
            let mut hits = 0;
            for j in 0..SS {
                for i in 0..SS {
                    let d = egui::vec2(
                        x as f32 + (i as f32 + 0.5) / SS as f32 - centre,
                        y as f32 + (j as f32 + 0.5) / SS as f32 - centre,
                    );
                    // Into the arc's own frame: turn the sample by −angle rather
                    // than the drawing by +angle.
                    if inside(egui::vec2(d.x * ca + d.y * sa, d.y * ca - d.x * sa)) {
                        hits += 1;
                    }
                }
            }
            grid[(y * n + x) as usize] = hits * 255 / (SS * SS) as u32;
        }
    }

    // `n` is odd and `compose` adds the same rim on both sides, so the hotspot it
    // computes lands on the middle texel — which is the one `centre` puts the
    // arc's own centre of rotation in.
    compose(scale, (n, n), Hotspot::Middle, |x, y| {
        if x < 0 || y < 0 || x >= n || y >= n {
            return 0;
        }
        grid[(y * n + x) as usize]
    })
}

/// Turn a coverage field into a cursor bitmap: white core, dark rim, straight
/// alpha. Shared by the glyph-sourced cursors and the drawn ones so they get the
/// same treatment and read as one set.
fn compose(
    scale: f32,
    (gw, gh): (i32, i32),
    hotspot: Hotspot,
    coverage: impl Fn(i32, i32) -> u32,
) -> Option<egui::CustomCursorImage> {
    let rim_px = rim_px(scale);

    let (w, h) = (gw + rim_px * 2, gh + rim_px * 2);
    let mut rgba = Vec::with_capacity((w * h * 4) as usize);
    let mut ink = 0usize;
    for y in 0..h {
        for x in 0..w {
            let (gx, gy) = (x - rim_px, y - rim_px);
            let fill = coverage(gx, gy);
            // The rim is the shape dilated by `rim_px`; the alpha of the union is
            // what the cursor occupies.
            let mut rim = 0;
            for dy in -rim_px..=rim_px {
                for dx in -rim_px..=rim_px {
                    rim = rim.max(coverage(gx + dx, gy + dy));
                }
            }
            if rim == 0 {
                rgba.extend_from_slice(&[0, 0, 0, 0]);
                continue;
            }
            ink += 1;
            // White at the shape's core, black out in the dilated ring, and the
            // ratio between them across the antialiased edge.
            let lum = (fill * 255 / rim) as u8;
            rgba.extend_from_slice(&[lum, lum, lum, rim as u8]);
        }
    }

    // A glyph the font does not have still lays out — it comes back blank, and a
    // fully transparent cursor would look like the pointer had vanished.
    if (ink as f32) < MIN_INK * (w * h) as f32 {
        return None;
    }
    let point = match hotspot {
        Hotspot::Middle => [w / 2, h / 2],
        Hotspot::At(x, y) => [x + rim_px, y + rim_px],
        Hotspot::Nib => nib(&rgba, w, h).unwrap_or([w / 2, h / 2]),
    };
    Some(egui::CustomCursorImage {
        rgba: rgba.into(),
        size: [w as u16, h as u16],
        hotspot: [point[0] as u16, point[1] as u16],
    })
}

/// The drawing tip of a nib glyph: the solid pixel closest to the **top-left**
/// corner, which is where the tip sits once [`rasterize`] has mirrored the glyph.
///
/// Measured rather than assumed. The tip is not at the corner of the bitmap — the
/// glyph is a whole pen, held at an angle — so a corner hotspot would leave the
/// click landing a few pixels off wherever the user aimed. It searched from the
/// bottom-left while the glyph was unmirrored; the two have to move together, and
/// `the_pen_points_from_its_nib_not_its_middle` is what says so.
fn nib(rgba: &[u8], w: i32, h: i32) -> Option<[i32; 2]> {
    /// Ignore the faint antialiased fringe, which reaches past the visible tip.
    const SOLID: u8 = 128;
    let mut best: Option<([i32; 2], i32)> = None;
    for y in 0..h {
        for x in 0..w {
            if rgba[((y * w + x) * 4 + 3) as usize] < SOLID {
                continue;
            }
            let distance = x + y;
            if best.is_none_or(|(_, d)| distance < d) {
                best = Some(([x, y], distance));
            }
        }
    }
    best.map(|(p, _)| p)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Where a cursor's ink sits, as an offset from its **hotspot** in texels.
    ///
    /// Weighted by alpha rather than counting texels, so the antialiased fringe
    /// pulls its own fraction and the answer does not jump about with the
    /// rasterizer's phase.
    fn centroid(img: &egui::CustomCursorImage) -> egui::Vec2 {
        let (w, h) = (usize::from(img.size[0]), usize::from(img.size[1]));
        let (hx, hy) = (f32::from(img.hotspot[0]), f32::from(img.hotspot[1]));
        let (mut sum, mut mass) = (egui::Vec2::ZERO, 0.0);
        for y in 0..h {
            for x in 0..w {
                let a = f32::from(img.rgba[(y * w + x) * 4 + 3]);
                sum += egui::vec2(x as f32 - hx, y as f32 - hy) * a;
                mass += a;
            }
        }
        if mass == 0.0 {
            egui::Vec2::ZERO
        } else {
            sum / mass
        }
    }

    /// A context with the theme's fonts installed. `fonts_mut` panics until a
    /// pass has run, so one is run here.
    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        ctx
    }

    /// Every drawn cursor is only as good as the glyph behind it, and a wrong
    /// Phosphor codepoint lays out perfectly well — it just comes back blank. So
    /// check each has ink, and has both a light core and a dark rim, since a
    /// white-on-white cursor is invisible over an artboard.
    #[test]
    fn every_bitmap_cursor_rasterizes_legibly() {
        let ctx = context();
        let mut cursors = Cursors::default();
        for (name, which) in [
            ("hand-open", Bitmap::HandOpen),
            ("hand-closed", Bitmap::HandClosed),
            ("pen", Bitmap::Pen),
            ("scale", Bitmap::Scale),
            ("crosshair", Bitmap::Crosshair),
            ("rotate-0", Bitmap::Rotate(Turn(0))),
            ("rotate-1", Bitmap::Rotate(Turn(1))),
            ("rotate-6", Bitmap::Rotate(Turn(6))),
            ("rotate-23", Bitmap::Rotate(Turn(23))),
            ("discard", Bitmap::Discard),
            ("text-on-path", Bitmap::TextOnPath),
            ("loaded", Bitmap::Loaded(Remaining(3))),
        ] {
            let img = cursors.image(&ctx, which).expect(name);
            let expected = usize::from(img.size[0]) * usize::from(img.size[1]) * 4;
            assert_eq!(img.rgba.len(), expected, "{name}: buffer vs {:?}", img.size);
            let opaque = || img.rgba.as_chunks::<4>().0.iter().filter(|p| p[3] > 0);
            assert!(opaque().count() > 0, "{name}: blank cursor");
            assert!(opaque().any(|p| p[0] > 200), "{name}: no light core");
            assert!(opaque().any(|p| p[0] < 60), "{name}: no dark rim");
            assert!(
                img.hotspot[0] < img.size[0] && img.hotspot[1] < img.size[1],
                "{name}: hotspot outside the bitmap"
            );
        }
    }

    /// The Scale tool's cursor is what makes the mode visible before anything has
    /// been scaled, so it has to be its **own** drawing — a wrong Phosphor
    /// codepoint lays out perfectly well and would just come back looking like one
    /// of the others.
    #[test]
    fn the_scale_cursor_is_its_own_drawing() {
        let ctx = context();
        let mut cursors = Cursors::default();
        let scale = cursors.image(&ctx, Bitmap::Scale).expect("scale");
        for (name, other) in [
            ("hand-open", Bitmap::HandOpen),
            ("pen", Bitmap::Pen),
            ("crosshair", Bitmap::Crosshair),
            ("rotate", Bitmap::Rotate(Turn::default())),
            ("discard", Bitmap::Discard),
        ] {
            let other = cursors.image(&ctx, other).expect(name);
            assert_ne!(
                scale.rgba, other.rgba,
                "the scale cursor came out the same bitmap as {name}"
            );
        }
    }

    /// The discard cursor is what says a guide dragged back onto its ruler will be
    /// **deleted** rather than merely moved, so it has to be its own drawing.
    ///
    /// A wrong Phosphor codepoint lays out perfectly well and comes back blank —
    /// which `every_bitmap_cursor_rasterizes_legibly` catches — but a *transposed*
    /// one comes back as some other glyph entirely, and the only thing that
    /// distinguishes "a bin" from "a hand" here is that they are different bitmaps.
    /// `trash-simple` (U+E4A8) and `trash` (U+E4A6) are one hex digit apart.
    #[test]
    fn the_discard_cursor_is_its_own_drawing() {
        let ctx = context();
        let mut cursors = Cursors::default();
        let discard = cursors.image(&ctx, Bitmap::Discard).expect("discard");
        for (name, other) in [
            ("hand-open", Bitmap::HandOpen),
            ("hand-closed", Bitmap::HandClosed),
            ("pen", Bitmap::Pen),
            ("scale", Bitmap::Scale),
            ("crosshair", Bitmap::Crosshair),
            ("rotate", Bitmap::Rotate(Turn::default())),
        ] {
            let other = cursors.image(&ctx, other).expect(name);
            assert_ne!(
                discard.rgba, other.rgba,
                "the discard cursor came out the same bitmap as {name}"
            );
        }
    }

    /// The whole reason the hands are not `CursorIcon::Grab`: on Windows that
    /// collapses both states onto one move arrow.
    #[test]
    fn the_two_hands_are_different_drawings() {
        let ctx = context();
        let mut cursors = Cursors::default();
        let open = cursors.image(&ctx, Bitmap::HandOpen).expect("open hand");
        let closed = cursors
            .image(&ctx, Bitmap::HandClosed)
            .expect("closed hand");
        assert_ne!(open.rgba, closed.rgba);
    }

    /// A pen has to point with its nib. The centre of the bitmap — right for a
    /// hand — would put the click a good third of the glyph away from where the
    /// user aimed, which on the pen tool is a misplaced anchor point every time.
    #[test]
    fn the_pen_points_from_its_nib_not_its_middle() {
        let ctx = context();
        let mut cursors = Cursors::default();
        let pen = cursors.image(&ctx, Bitmap::Pen).expect("pen");
        let (w, h) = (i32::from(pen.size[0]), i32::from(pen.size[1]));
        let (hx, hy) = (i32::from(pen.hotspot[0]), i32::from(pen.hotspot[1]));
        // The glyph is mirrored on the way in so the pen leans like every other
        // cursor — tip up-left, barrel trailing to the bottom-right — so the
        // hotspot belongs in the *top*-left corner. Reported as "the pen cursor
        // is rotated to the top-right"; before the mirror this asserted the
        // bottom, which is the half that would silently come back if the
        // reflection in `rasterize` were dropped.
        assert!(hx < w / 3, "hotspot x {hx} is not near the left of {w}");
        assert!(hy < h / 3, "hotspot y {hy} is not near the top of {h}");
    }

    /// Every rotate step has to be its own *drawing*. A `Turn` plumbed all the
    /// way through and never reaching the pixels would leave every handle looking
    /// identical while every test about the mapping still passed — which is the
    /// exact failure the quarter-turn version had by design, four drawings being
    /// the most it could tell apart in a revolution.
    #[test]
    fn every_rotate_step_is_its_own_drawing() {
        let ctx = context();
        let mut cursors = Cursors::default();
        let all: Vec<_> = (0..Turn::STEPS)
            .map(|k| {
                cursors
                    .image(&ctx, Bitmap::Rotate(Turn(k)))
                    .expect("rotate")
            })
            .collect();
        for (i, a) in all.iter().enumerate() {
            for (j, b) in all.iter().enumerate().skip(i + 1) {
                assert_ne!(a.rgba, b.rgba, "steps {i} and {j} came out the same bitmap");
            }
        }
    }

    /// **The arc points where it was asked to.**
    ///
    /// This is the one thing the cursor exists to say — *which handle you have* —
    /// and it was what the ring could not carry: at 19pt a ring turned by any
    /// amount is still a ring. The arc is symmetric about the direction it faces
    /// and lies entirely to one side of its own centre, so its ink centroid sits
    /// on that ray, which makes the heading directly measurable rather than
    /// inferred from which quadrant is emptiest.
    ///
    /// Measured from the **hotspot** rather than from the middle of the buffer,
    /// because those being the same point is half of what is under test: a figure
    /// whose centre of rotation drifted off the hotspot would swing round it.
    #[test]
    fn the_arc_faces_the_direction_it_was_given() {
        let ctx = context();
        let mut cursors = Cursors::default();
        for k in 0..Turn::STEPS {
            let turn = Turn(k);
            let img = cursors.image(&ctx, Bitmap::Rotate(turn)).expect("rotate");
            let got = centroid(&img);
            assert!(
                got.length() > 1.0,
                "step {k}: the ink is centred on the hotspot, so it has no heading"
            );
            // Wrapped into (−π, π] the long way round rather than through
            // `sin().asin()`, which folds 180° onto 0 and would pass a cursor
            // pointing at the opposite handle.
            use std::f32::consts::{PI, TAU};
            let mut off = (got.y.atan2(got.x) - turn.radians()).rem_euclid(TAU);
            if off > PI {
                off -= TAU;
            }
            assert!(
                off.abs() < 0.13,
                "step {k} faces {:.0}° but was asked for {:.0}°",
                got.y.atan2(got.x).to_degrees(),
                turn.radians().to_degrees(),
            );
        }
    }

    /// **Nothing clips at the odd angles**, which is the trap a rotated cursor
    /// falls into: a square drawing needs √2 the extent at 45°, so a buffer sized
    /// for the upright version loses the corners at exactly the angles nobody
    /// checks. `rotate_arc` sizes from the circumscribed circle instead, and
    /// that is checked two ways, because either alone is weak.
    ///
    /// **The border stays clear**, which is what clipping would violate directly —
    /// ink pressed flat against an edge. There is real room here rather than a
    /// hairsbreadth: the drawing reaches `RADIUS + HEAD_W` plus the rim, inside a
    /// half-width of `ceil(RADIUS + HEAD_W) + 1` plus the same rim.
    ///
    /// Worth saying plainly, since the trap it is named for is what justified it:
    /// **for this figure the √2 is mild**, because the arc is nearly round already.
    /// Its outermost ink is a head's base corner, 7.9pt out radially but only 7.3pt
    /// along x while the arc is upright — so a buffer sized angle-blind from the
    /// upright extent falls about 8% short and clips slightly, not spectacularly. What
    /// this arm actually catches is the padding being trimmed: taking a texel off
    /// `half` puts ink on the border at **every** step, step 0 included. That is
    /// cheap insurance rather than a live catch, and it is the arm below that would
    /// bite on a drawing shaped more like a square.
    ///
    /// **And the ink weighs the same at every angle**, which catches a loss the
    /// border test would not — one taken out of the middle, or a head shortened
    /// rather than cut off. Weighed by alpha rather than counted by texel:
    /// counting puts the antialiased fringe on one side of a threshold or the
    /// other and swings 6% on rasterizer phase alone, where the mass moves 3%. An
    /// arrowhead is about a third of the drawing, so 4% is nowhere near able to
    /// hide one.
    #[test]
    fn no_angle_clips_the_arc() {
        let ctx = context();
        let mut cursors = Cursors::default();
        let mut mass = Vec::new();
        for k in 0..Turn::STEPS {
            let img = cursors
                .image(&ctx, Bitmap::Rotate(Turn(k)))
                .expect("rotate");
            let (w, h) = (usize::from(img.size[0]), usize::from(img.size[1]));
            let at = |x: usize, y: usize| img.rgba[(y * w + x) * 4 + 3];
            let edge = (0..w)
                .map(|x| (x, 0))
                .chain((0..w).map(|x| (x, h - 1)))
                .chain((0..h).map(|y| (0, y)))
                .chain((0..h).map(|y| (w - 1, y)))
                .find(|&(x, y)| at(x, y) > 0);
            assert_eq!(edge, None, "step {k}: ink on the buffer's own border");
            mass.push(
                img.rgba
                    .as_chunks::<4>()
                    .0
                    .iter()
                    .map(|p| u32::from(p[3]))
                    .sum::<u32>(),
            );
        }
        let (lo, hi) = (*mass.iter().min().unwrap(), *mass.iter().max().unwrap());
        assert!(
            (hi - lo) * 100 < hi * 4,
            "the drawing weighs {lo}..={hi} across the {} steps — some angle is \
             losing part of it",
            Turn::STEPS,
        );
    }

    /// **It is an arc, not a ring**, which is the whole reason it is drawn here
    /// rather than taken from Phosphor. A ring has no heading to read; the test
    /// above would pass on one only by accident, and would go on passing if
    /// somebody swapped a closed glyph back in and the centroid landed a texel off
    /// centre. So this asserts the gap: the half-plane *behind* the direction the
    /// cursor faces has to be empty.
    #[test]
    fn the_arc_has_an_open_side() {
        let ctx = context();
        let mut cursors = Cursors::default();
        for k in 0..Turn::STEPS {
            let turn = Turn(k);
            let img = cursors.image(&ctx, Bitmap::Rotate(turn)).expect("rotate");
            let (facing, behind) = (
                egui::Vec2::angled(turn.radians()),
                egui::Vec2::angled(turn.radians() + std::f32::consts::PI),
            );
            let (w, h) = (usize::from(img.size[0]), usize::from(img.size[1]));
            let (hx, hy) = (f32::from(img.hotspot[0]), f32::from(img.hotspot[1]));
            let mut ahead = 0;
            let mut back = 0;
            for y in 0..h {
                for x in 0..w {
                    if img.rgba[(y * w + x) * 4 + 3] == 0 {
                        continue;
                    }
                    let d = egui::vec2(x as f32 - hx, y as f32 - hy);
                    ahead += usize::from(d.dot(facing) > 1.0);
                    back += usize::from(d.dot(behind) > 1.0);
                }
            }
            assert!(ahead > 0, "step {k}: no ink in front of the hotspot at all");
            assert_eq!(
                back, 0,
                "step {k}: {back} texels of ink behind the hotspot — this is a \
                 ring, and a ring turned by any amount is still a ring"
            );
        }
    }

    /// A screen direction picks the step facing it. Getting this backwards puts
    /// the arc on the handle opposite the pointer, which reads as a bug in the hit
    /// test rather than in the cursor.
    ///
    /// **Screen space, so y points down** — the sign that decides whether the
    /// cursor mirrors vertically, and the one thing a `toward` written against
    /// maths-convention axes would get exactly wrong on every corner.
    #[test]
    fn a_screen_direction_picks_the_step_facing_it() {
        let steps = f32::from(Turn::STEPS);
        for (out, degrees, name) in [
            (egui::vec2(1.0, 0.0), 0.0, "right"),
            (egui::vec2(1.0, 1.0), 45.0, "down-right"),
            (egui::vec2(0.0, 1.0), 90.0, "down"),
            (egui::vec2(-1.0, 1.0), 135.0, "down-left"),
            (egui::vec2(-1.0, -1.0), 225.0, "up-left"),
            (egui::vec2(1.0, -1.0), 315.0, "up-right"),
        ] {
            let want = Turn((degrees / 360.0 * steps) as u8);
            assert_eq!(Turn::toward(out), want, "{name}");
            // 45° is a whole number of 15° steps, so each diagonal lands *on* one
            // rather than between two — which is what makes an unrotated box's
            // corner cursor exact rather than 7.5° out.
            assert!(
                (degrees / 360.0 * steps).fract() == 0.0,
                "{name} does not land on a step, so this asserts the wrong one"
            );
        }
        // Between two steps, the nearer wins rather than the earlier.
        assert_eq!(Turn::toward(egui::Vec2::angled(0.24)), Turn(1));
        assert_eq!(Turn::toward(egui::Vec2::angled(0.02)), Turn(0));
        // And a degenerate direction has to answer *something* rather than panic.
        assert_eq!(Turn::toward(egui::Vec2::ZERO), Turn(0));
    }

    /// The whole reason for drawing the crosshair instead of asking for
    /// `IDC_CROSS`: at 32 texels the stock one is about twice the size a design
    /// tool wants. If this ever creeps back up, it has stopped being worth it.
    #[test]
    fn the_crosshair_is_far_smaller_than_the_stock_one() {
        /// `IDC_CROSS`, for scale.
        const STOCK: u16 = 32;
        let ctx = context();
        assert_eq!(ctx.pixels_per_point(), 1.0, "sized against a 1x display");
        let cross = Cursors::default()
            .image(&ctx, Bitmap::Crosshair)
            .expect("crosshair");
        assert!(
            cross.size[0] <= STOCK * 2 / 3 && cross.size[1] <= STOCK * 2 / 3,
            "{:?} is not meaningfully smaller than {STOCK}px",
            cross.size
        );
    }

    /// The gap in the middle is the point of a crosshair: you aim through it. If
    /// the arms ever met, the cursor would cover the very pixel it is pointing at.
    #[test]
    fn the_crosshair_can_be_seen_through_at_its_aiming_point() {
        let ctx = context();
        let cross = Cursors::default()
            .image(&ctx, Bitmap::Crosshair)
            .expect("crosshair");
        let (w, hx, hy) = (
            usize::from(cross.size[0]),
            usize::from(cross.hotspot[0]),
            usize::from(cross.hotspot[1]),
        );
        let alpha = cross.rgba[(hy * w + hx) * 4 + 3];
        assert_eq!(alpha, 0, "the aiming point is painted over");
    }

    /// egui dedupes cursor uploads by `Arc` pointer identity, so handing it a
    /// fresh buffer each frame would push a new cursor to the OS every frame of
    /// every pan.
    #[test]
    fn the_bitmaps_are_reused_rather_than_rebuilt() {
        let ctx = context();
        let mut cursors = Cursors::default();
        let first = cursors.image(&ctx, Bitmap::HandOpen).expect("open hand");
        let again = cursors.image(&ctx, Bitmap::HandOpen).expect("open hand");
        assert!(std::sync::Arc::ptr_eq(&first.rgba, &again.rgba));
        // Including every rotate step, which is why they have a slot each: a
        // rotate drag sweeps through all of them and back, and an unquantized
        // angle with no cache would upload one per frame.
        let one = cursors.image(&ctx, Bitmap::Rotate(Turn(1))).unwrap();
        let one_again = cursors.image(&ctx, Bitmap::Rotate(Turn(1))).unwrap();
        assert!(std::sync::Arc::ptr_eq(&one.rgba, &one_again.rgba));
    }

    /// Every display scale a badge has to survive. **1.25 and 1.5 are the Windows
    /// norms** and are where a cursor change has rounded back to a no-op here
    /// before; 3.0 is in because the digits thicken faster than the rim does, and
    /// the knockout depends on the rim reaching into a stroke.
    const SCALES: [f32; 5] = [1.0, 1.25, 1.5, 2.0, 3.0];

    /// A context at `scale`, with a pass run so the atlas is rasterized for it.
    fn scaled(scale: f32) -> egui::Context {
        let ctx = context();
        ctx.set_pixels_per_point(scale);
        let _ = ctx.run_ui(Default::default(), |_| {});
        ctx
    }

    /// **The badge must not take the aiming point with it.**
    ///
    /// This is the trap the whole feature has: the buffer grows down and right to
    /// make room for the chip, so the middle of it is no longer the middle of the
    /// cross — and `Hotspot::Middle`, which every other drawn cursor here uses and
    /// which the plain crosshair used until this landed, would silently put the
    /// click a few texels off wherever the user aimed. Asserted against the plain
    /// crosshair's own hotspot rather than against a computed number, since the two
    /// being interchangeable under the pointer is the actual requirement.
    ///
    /// **And the point is still see-through**, which is the crosshair's own reason
    /// for existing: a hotspot that lands on the aiming gap but paints over it
    /// would satisfy the first half and lose the point of the drawing.
    #[test]
    fn the_badge_does_not_move_the_aiming_point() {
        for scale in SCALES {
            let ctx = scaled(scale);
            let mut cursors = Cursors::default();
            let plain = cursors.image(&ctx, Bitmap::Crosshair).expect("crosshair");
            for n in [1usize, 2, 9, 10, 47, 100] {
                let which = Bitmap::Loaded(Remaining::of(n).expect("loaded"));
                let img = cursors.image(&ctx, which).expect("loaded cursor");
                assert_eq!(
                    img.hotspot, plain.hotspot,
                    "{scale}x n={n}: the badge moved the aiming point off the \
                     crosshair's own"
                );
                let (w, hx, hy) = (
                    usize::from(img.size[0]),
                    usize::from(img.hotspot[0]),
                    usize::from(img.hotspot[1]),
                );
                assert_eq!(
                    img.rgba[(hy * w + hx) * 4 + 3],
                    0,
                    "{scale}x n={n}: the aiming point is painted over"
                );
            }
        }
    }

    /// **Every count is its own drawing.** A `Remaining` plumbed all the way
    /// through and never reaching the pixels would leave the cursor looking
    /// identical at nine images and at one while everything about the routing
    /// still passed — and the number is the whole feature.
    ///
    /// The clamp is asserted from both sides: the last exact count differs from the
    /// first clamped one, and two counts past the clamp are the same bitmap, which
    /// is what keeps the cache bounded.
    #[test]
    fn the_badge_draws_the_count_it_was_given() {
        let ctx = scaled(1.0);
        let mut cursors = Cursors::default();
        let of = |n: usize| Bitmap::Loaded(Remaining::of(n).expect("loaded"));
        let mut seen: Vec<(usize, std::sync::Arc<[u8]>)> = Vec::new();
        for n in [1usize, 2, 3, 9, 10, 42, 99] {
            let img = cursors.image(&ctx, of(n)).expect("loaded cursor");
            // Compared with `assert!` rather than `assert_ne!`, which prints both
            // buffers on failure — a few thousand bytes of noise around one line
            // of meaning.
            for (m, other) in &seen {
                assert!(
                    img.rgba != *other,
                    "{n} images and {m} came out the same bitmap"
                );
            }
            seen.push((n, img.rgba.clone()));
        }
        let (many, more) = (
            cursors.image(&ctx, of(100)).expect("100"),
            cursors.image(&ctx, of(4000)).expect("4000"),
        );
        assert!(
            cursors.image(&ctx, of(99)).unwrap().rgba != many.rgba,
            "99 and 100 draw the same badge, so the count stops being exact a step \
             early"
        );
        assert!(
            many.rgba == more.rgba,
            "past the clamp every count is its own bitmap, so one big drop would \
             build one per file"
        );
    }

    /// **The digits are knocked out of the chip, and they survive the rim.**
    ///
    /// `compose` paints a white core and a dark rim, so the chip comes out white
    /// and the digits — being holes in it — come out dark *because* the rim's
    /// dilation reaches into them. That is what makes a nine-point number legible
    /// at 1x where drawn ink is a blob, and it is also what could fail quietly at a
    /// high display scale, where a stroke thickens faster than the rim: a hole the
    /// dilation cannot reach has `rim == 0` and goes fully **transparent**, which
    /// is a see-through number rather than a dark one.
    ///
    /// **The transparency arm is the one that can fail**, and it does: at 40pt
    /// instead of nine the strokes outrun the rim and the digits come out as
    /// see-through slots in the badge. The darkness arm restates the coverage —
    /// inside a chip the dilation always finds white, so `lum` comes back equal to
    /// what the closure returned — and is here as the visible outcome rather than
    /// as a second catch.
    ///
    /// **Which texels are the digits is taken from the badge's own coverage, and
    /// found by filling in from the chip's border.** Two drafts that eyeballed a
    /// region of the finished cursor instead were each vacuous in their own way:
    /// the first counted the chip's dark *rim* as a digit and passed against a
    /// blank lozenge, and the second asked a gap to be enclosed on four sides,
    /// which no texel of a one-texel-wide `1` is.
    #[test]
    fn the_count_reads_dark_on_a_light_chip() {
        for scale in SCALES {
            let ctx = scaled(scale);
            let mut cursors = Cursors::default();
            for n in [1usize, 8, 88] {
                let img = cursors
                    .image(&ctx, Bitmap::Loaded(Remaining::of(n).expect("loaded")))
                    .expect("loaded cursor");
                let w = usize::from(img.size[0]);
                // Classified from the badge's own coverage rather than by looking
                // at the finished cursor and guessing which texels are which: the
                // chip's dark **rim** is what an eyeballed region catches instead
                // of the digits, and a first draft that counted it passed happily
                // against a chip with nothing knocked out at all.
                let label = Remaining::of(n).expect("loaded").label();
                let (bw, bh, chip) = badge(&ctx, scale, &label).expect("badge");
                // **Which gaps are digits, found by filling from the chip's own
                // border rather than by a surrounded-on-four-sides test.** The
                // corners are rounded, so they are gaps too — and a stroke one
                // texel wide has gap above it and gap below it, so anything that
                // asks a gap to be enclosed classifies the whole of a `1` as
                // outside and then reports there is nothing to check.
                // **A knocked-out texel is not a bare zero.** A nine-point stem is
                // one texel wide and antialiased, so the glyph's own coverage
                // peaks well under 255 and the chip under it never reaches a clean
                // hole — asking for one classified the whole of a `1` as chip and
                // reported there was nothing to check.
                const KNOCKED: u8 = 128;
                let gap = |i: usize| chip[i] < KNOCKED;
                let mut outside = vec![false; (bw * bh) as usize];
                let mut edge: Vec<(i32, i32)> = (0..bw)
                    .flat_map(|x| [(x, 0), (x, bh - 1)])
                    .chain((0..bh).flat_map(|y| [(0, y), (bw - 1, y)]))
                    .collect();
                while let Some((x, y)) = edge.pop() {
                    if x < 0 || y < 0 || x >= bw || y >= bh {
                        continue;
                    }
                    let i = (y * bw + x) as usize;
                    if outside[i] || !gap(i) {
                        continue;
                    }
                    outside[i] = true;
                    edge.extend([(x - 1, y), (x + 1, y), (x, y - 1), (x, y + 1)]);
                }
                let origin = Cross::new(scale).badge_at() + rim_px(scale);
                let (mut digits, mut face) = (0, 0);
                for y in 0..bh {
                    for x in 0..bw {
                        let p = &img.rgba
                            [((y + origin) as usize * w + (x + origin) as usize) * 4..][..4];
                        let i = (y * bw + x) as usize;
                        if !gap(i) {
                            face += usize::from(p[3] > 0 && p[0] > 200);
                            continue;
                        }
                        if outside[i] {
                            continue;
                        }
                        digits += 1;
                        assert!(
                            p[3] > 0,
                            "{scale}x n={n}: the digit texel at {x},{y} is \
                             transparent — a stroke the rim could not reach is a \
                             see-through number, not a dark one"
                        );
                        assert!(
                            p[0] < KNOCKED,
                            "{scale}x n={n}: the digit texel at {x},{y} paints \
                             {} rather than dark",
                            p[0]
                        );
                    }
                }
                assert!(
                    digits > 0,
                    "{scale}x n={n}: nothing is knocked out of the chip, so the \
                     badge is a blank lozenge"
                );
                assert!(face > 0, "{scale}x n={n}: the chip has no light face");
            }
        }
    }

    /// **An empty cursor is not a badge reading zero.** The Image tool disarms
    /// itself as the last picture goes down (`canvas::placed_one_image`), so this
    /// is a state that exists only if something failed to; the plain crosshair is
    /// the honest drawing for it, and a `0` would be the cursor claiming to be
    /// loaded.
    #[test]
    fn an_unloaded_cursor_has_no_badge() {
        assert_eq!(Remaining::of(0), None);
        assert!(Remaining::of(1).is_some());
    }

    /// **The type-on-a-path pointer is symmetric, and its curve is wider than its
    /// I-beam** (§15 D408) — the two things that make it read as what it is.
    ///
    /// ⚠️ **Both were wrong in the first drawing and both were found by dumping it
    /// as ASCII**, which is what this test replaces. The arc was centred on the
    /// texel *index* `w / 2` where the figure's own centre is the half-texel `w /
    /// 2.0`, so its right end sat a whole row below its left — a lopsided curve,
    /// invisible in the source and obvious in the picture. And the arc was the
    /// beam's own width, which reads as a bowl the letters sit *in* rather than a
    /// path they stand on.
    ///
    /// **Asserted on the alpha rather than on the luminance**, because `compose`
    /// dilates the shape into a dark rim: the rim is part of the drawing and is
    /// symmetric with it, where a threshold on brightness would be measuring where
    /// the core happens to fade.
    #[test]
    fn the_type_on_a_path_pointer_is_symmetric_and_its_curve_is_the_wider_mark() {
        // 🚨 **Seven scales, and it used to be `1.0` alone** (§15 D587,
        // `[S17-L1-04]`). At 1.0 the figure is symmetric whatever the arithmetic
        // does, because `pt(1.0)` is odd there and `cx` happens to be the mirror
        // axis; the beam was half a texel out at 1.5×, 1.75× and 2.0×, which are
        // the display scales this project's own notes call the Windows norms. The
        // constant and the habit were already in this file — the two badge tests
        // four hundred lines up both loop `SCALES` — and this test, the one named
        // for symmetry, did not.
        //
        // ⚠️ **1.75 and 2.5 are added to `SCALES`' own list here rather than to
        // the constant**, so the badge tests keep the coverage they were written
        // against. `pt(1.0)` is even at exactly 1.5, 1.75, 2.0 and 2.5 — the four
        // scales the parity argument names — and the constant carried only two of
        // them. 2.5 measured *clean* under the bug, because `half_w`'s own parity
        // shifts the axis back, so it is here as the control that argument needs
        // rather than as a case that ever failed.
        //
        // **Flip run**, the beam built from `cx` again: fails at *"at 1.5x, row 0
        // is lopsided at column 5"*, the site the finding predicted.
        for scale in SCALES.iter().copied().chain([1.75, 2.5]) {
            let img = text_on_path(scale).expect("drawn");
            let (w, h) = (usize::from(img.size[0]), usize::from(img.size[1]));
            let alpha = |x: usize, y: usize| img.rgba[(y * w + x) * 4 + 3];

            for y in 0..h {
                for x in 0..w {
                    assert_eq!(
                        alpha(x, y),
                        alpha(w - 1 - x, y),
                        "at {scale}x, row {y} is lopsided at column {x} — a mark is \
                         centred on an index rather than on the figure's own middle"
                    );
                }
            }

            // The widest inked row belongs to the curve, which is below the beam.
            let width_of = |y: usize| (0..w).filter(|x| alpha(*x, y) > 0).count();
            let widest = (0..h).max_by_key(|y| width_of(*y)).expect("rows");
            let beam_rows = (0..h).filter(|y| width_of(*y) > 0).count();
            assert!(
                widest > beam_rows / 2,
                "at {scale}x the widest mark must be the curve in the lower half, \
                 not the serif: widest row {widest} of {beam_rows} inked"
            );
        }
    }

    /// **A label with no ink answers `None` instead of overflowing** (§15 D684,
    /// `[S17-L1-09]`).
    ///
    /// `digits` folds a bounding box out of the glyphs it placed, seeded at
    /// `(i32::MAX, i32::MAX, i32::MIN, i32::MIN)`. Place none and those seeds come
    /// straight back out, so `hi_x - lo_x` is `i32::MIN - i32::MAX` — an overflow,
    /// which panics in debug and wraps to `1` in release, and the `w <= 0` guard
    /// meant to catch exactly this sits one statement past it.
    ///
    /// ⚠️ **The empty string is the honest input, not a contrivance.** It is the
    /// one label whose glyph list is empty for a reason nothing about fonts can
    /// change; a missing-glyph label would do it too and needs a font that does not
    /// exist here. What the test is about is the guard's *position*, and both
    /// inputs reach it the same way.
    ///
    /// **Flip:** move `if placed.is_empty()` back below the `let (w, h) = …` and
    /// this fails — in debug, by panicking with *"attempt to subtract with
    /// overflow"* rather than by an assertion, which is the whole point of the
    /// finding. Predicted correctly.
    #[test]
    fn a_badge_with_nothing_to_draw_refuses_rather_than_overflowing() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        // Fonts are only available once a frame has run.
        let _ = ctx.run_ui(Default::default(), |_| {});
        assert!(digits(&ctx, 1.0, "").is_none());
        // The control: a real label still measures, so the guard has not eaten the
        // ordinary case.
        assert!(digits(&ctx, 1.0, "99+").is_some());
    }
}
