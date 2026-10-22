//! Canvas cursors that no OS provides a usable one for, as real cursor bitmaps.
//!
//! Only the text I-beam stays a stock cursor, set from `CursorIcon` in `canvas.rs`.
//! The rest are here:
//!
//! - **The hands.** winit maps both `Grab` and `Grabbing` onto Windows'
//!   `IDC_SIZEALL` move arrow, so the two pan states would be identical.
//! - **The pen.** `CursorIcon` has no pen.
//! - **The crosshair.** `IDC_CROSS` is 32 texels across, about twice what a design
//!   tool uses.
//! - **The rotate arc.** `CursorIcon` has no rotate, and no icon set has a
//!   two-headed arc (see [`rotate_arc`]).
//! - **The Scale box and the discard bin** ([`Bitmap::Scale`], [`Bitmap::Discard`]).
//! - **The loaded Image cursor**, the crosshair with a count badge
//!   ([`Bitmap::Loaded`]).
//!
//! The hands, pen, Scale box and bin are rasterized from the Phosphor glyphs the
//! tool rail uses, so a tool's cursor and its rail button are one drawing; the
//! crosshair, the rotate arc and the badge are drawn outright. Both paths meet at
//! [`compose`], which gives them a common white core and dark rim.
//!
//! egui hands the result to the OS as a true cursor via
//! `Context::set_cursor_image`, so it neither lags the pointer nor clips at the
//! window edge. A `CursorIcon` is still set alongside every bitmap as the fallback
//! when rasterizing fails.

use crate::theme;
use eframe::egui;

/// Which way a rotate cursor faces: a **screen** direction (y down), quantized to
/// [`Turn::STEPS`] steps of a full turn.
///
/// The arc is drawn at any angle ([`rotate_arc`]), so the step only sets how finely
/// to cache. 15° is fine enough to track a drag as a heading rather than a few
/// discrete states. Quantizing is required: egui dedupes cursor uploads by `Arc`
/// pointer identity, so a raw angle would upload a new cursor every frame of a
/// rotate drag.
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
/// The exact count up to 99, `99+` beyond, which bounds the cache: every distinct
/// badge is a bitmap that has to be kept, so an unclamped count would build one per
/// file in a large drop.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Remaining(u8);

impl Remaining {
    /// Above this the badge stops counting.
    const MANY: usize = 100;

    /// The badge for `n` images still to place, or `None` for an empty cursor.
    ///
    /// `None` rather than a zero badge: an armed tool with nothing to place means
    /// something failed to disarm it (`canvas::placed_one_image`), and a `0` would
    /// claim the cursor is loaded.
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
    /// The Scale tool. Select and Scale differ only in what a handle drag means, so
    /// without a cursor of its own the mode is invisible until something is scaled.
    Scale,
    /// The shape tools, about to drag out a box.
    Crosshair,
    /// The Image tool's **loaded** cursor: the same crosshair with a badge on it
    /// saying how many files are still to be placed.
    ///
    /// The count is the only thing that distinguishes this from a shape tool: a
    /// click here plants a picture, not a box. So the badge is drawn for a single
    /// remaining image too.
    ///
    /// The one badge in the app: modifier badges were declined (§15 D233) because
    /// they would sit on a stock cursor with nothing to composite onto.
    Loaded(Remaining),
    /// Hovering the band just outside a corner handle, where a drag rotates.
    /// Turned to face its handle, so it says which one has the pointer.
    Rotate(Turn),
    /// A guide dragged back onto its ruler, where letting go deletes it. A bin says
    /// the drop is accepted and destructive, which `NoDrop` (refused) cannot.
    Discard,
    /// The Text tool over the outline of a shape, where a click sets type along that
    /// path (§15 D408).
    ///
    /// Drawn outright: an I-beam ("text goes here") over a curve ("along the
    /// path"), which no icon says together and which must not be confused with the
    /// stock `CursorIcon::Text` shown for editing existing text.
    TextOnPath,
    /// An armed eyedropper off the canvas, where there is nothing to sample and
    /// the loupe gives way to a pointer saying the tool is still in hand.
    Eyedropper,
}

impl Bitmap {
    /// The Phosphor glyph this cursor is drawn from and where in it the pointer
    /// aims — or `None` for the ones drawn from scratch.
    ///
    /// One function so a variant with no glyph has nowhere to put a hotspot:
    /// [`Cursors::image`] routes the drawn variants to functions that pass their own
    /// [`Hotspot`], and only the six glyph variants reach this.
    fn glyph_and_hotspot(self) -> Option<(&'static str, Hotspot)> {
        match self {
            // A hand grabs with its palm; the bin and the Scale box aim from the
            // middle of the whole pointer.
            Self::HandOpen => Some((theme::icon::HAND, Hotspot::Middle)),
            Self::HandClosed => Some((theme::icon::HAND_GRABBING, Hotspot::Middle)),
            // A pen draws from its nib, where the anchor point will appear.
            Self::Pen => Some((theme::icon::PEN_NIB, Hotspot::Nib)),
            // The dropper's tip is bottom-left like the nib's, so it is mirrored
            // the same way.
            Self::Eyedropper => Some((theme::icon::EYEDROPPER, Hotspot::Nib)),
            Self::Scale => Some((theme::icon::RESIZE, Hotspot::Middle)),
            Self::Discard => Some((theme::icon::TRASH_SIMPLE, Hotspot::Middle)),
            // Drawn outright, each naming its own aiming point: the loaded
            // crosshair's badge and the path cursor's curve make the middle of the
            // buffer the wrong point.
            Self::Crosshair | Self::Rotate(_) | Self::Loaded(_) | Self::TextOnPath => None,
        }
    }
}

/// Where in its own bitmap a cursor points.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Hotspot {
    Middle,
    /// A named texel of the **coverage** field, before [`compose`] pads it with the
    /// rim. For a buffer not symmetric about its aiming point, as the badge makes
    /// the crosshair's.
    At(i32, i32),
    /// The ink nearest the top-left corner, once the glyph has been mirrored so the
    /// pen leans like every other cursor (see [`rasterize`]). Found rather than
    /// hard-coded, so it stays on the nib if the glyph or its size changes.
    Nib,
}

/// Logical size the glyph is rasterized at; the bitmap is this many points times
/// the display scale in texels.
const GLYPH_PT: f32 = 19.0;
/// Outline thickness in points: one thin dark edge, enough that white line art does
/// not vanish against a white artboard.
///
/// Under 1.0 on purpose: scaled and rounded, a flat `1.0` becomes two texels on
/// 125%/150% displays. At 0.75 the rim is one texel up to 175% and two from 200%.
const RIM_PT: f32 = 0.75;
/// Below this fraction of covered pixels the rasterization is assumed to have
/// failed (missing glyph, atlas miss) and the bitmap is discarded.
const MIN_INK: f32 = 0.02;

/// [`RIM_PT`] in physical texels. Shared with [`Cross`], which needs it to place
/// the badge clear of the rim.
fn rim_px(scale: f32) -> i32 {
    ((RIM_PT * scale).round() as i32).max(1)
}

/// One cached bitmap. The outer `Option` is "not built yet", the inner one is
/// "built and failed" — kept apart so a glyph that cannot be rasterized is not
/// retried on every frame of every gesture.
type Slot = Option<Option<egui::CustomCursorImage>>;

/// The drawn cursors, built on first use and kept while the display scale stays put.
///
/// Caching is required, not just an optimization: egui dedupes by `Arc` pointer
/// identity, so a fresh buffer each frame would upload a new cursor every frame.
#[derive(Default)]
pub struct Cursors {
    /// Scale the cached bitmaps were rasterized for.
    scale: f32,
    hand_open: Slot,
    hand_closed: Slot,
    pen: Slot,
    /// The Scale tool's; `scale` above is the display scale.
    scale_tool: Slot,
    crosshair: Slot,
    /// One per [`Turn`], built on demand.
    rotate: [Slot; Turn::STEPS as usize],
    discard: Slot,
    text_on_path: Slot,
    eyedropper: Slot,
    /// One per count the cursor has been loaded with; a map because a session
    /// touches a handful of [`Remaining`]'s hundred values.
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
            Bitmap::Eyedropper => &mut self.eyedropper,
            Bitmap::Loaded(n) => self.loaded.entry(n).or_default(),
        };
        slot.get_or_insert_with(|| match which {
            Bitmap::TextOnPath => text_on_path(scale),
            Bitmap::Crosshair => crosshair(scale),
            Bitmap::Rotate(t) => rotate_arc(scale, t.radians()),
            Bitmap::Loaded(n) => loaded_crosshair(ctx, scale, n),
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
    // Laying the glyph out rasterizes it into the font atlas; the galley says where.
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
        // Mirrored, not turned, for a nib: Phosphor draws the tip bottom-left, and
        // every other cursor points up-left. A reflection puts the tip top-left; a
        // 180° turn would put it top-right.
        let y = if hotspot == Hotspot::Nib {
            h - 1 - y
        } else {
            y
        };
        source(x, y)
    })
}

/// A thin crosshair of four rectangles. `IDC_CROSS` is too large and Phosphor's
/// `crosshair` glyphs carry a target ring. The geometry is [`Cross`], which the
/// loaded Image cursor draws too.
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
/// The I-beam makes it read as the type tool; the curve separates it from
/// `CursorIcon::Text`, which means "edit the text under the pointer". The curve is
/// one circular arc, since a wave's bends smudge at sixteen texels. Sizes are in
/// points and scaled, as for every drawn cursor here.
fn text_on_path(scale: f32) -> Option<egui::CustomCursorImage> {
    /// Half the arc's width, and so half the whole figure's. Wider than the I-beam
    /// on purpose: an arc the beam's width reads as a bowl the type sits in.
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
    // The apex is under the stem and the ends hang away from it: type standing on
    // a curve, the baseline being the path.
    let apex = beam_h + gap;
    let h = apex + sag + thick;

    // The circle through the apex and the two low ends (chord-and-sagitta,
    // `R = (c²/4 + s²) / 2s`), centred below the apex so it is a hill, not a bowl.
    // The centre is `w / 2` as a length, not `cx` as an index, and the chord is
    // `w − 1` (first to last texel middle); an index centre puts one end a row
    // lower.
    let (c, s) = (f64::from(w) - 1.0, f64::from(sag).max(1.0));
    let r = (c * c / 4.0 + s * s) / (2.0 * s);
    let (ccx, ccy) = (f64::from(w) / 2.0, f64::from(apex) + r);

    let half = f64::from(thick) / 2.0;
    compose(scale, (w, h), Hotspot::At(cx, beam_h / 2), move |x, y| {
        if x < 0 || y < 0 || x >= w || y >= h {
            return 0;
        }
        // The I-beam: a stem, and a serif at each end. Measured from `ccx`, the
        // length the arc uses, not from `cx`: the buffer's mirror axis is
        // `(w − 1) / 2`, which `cx` matches only when `thick` is odd, and `pt(1.0)`
        // is even at 1.5x, 1.75x, 2.0x and 2.5x. An even-width figure gets one
        // extra serif column, since it has no centre column.
        let from_middle = (f64::from(x) + 0.5 - ccx).abs();
        let stem = from_middle < half && y < beam_h;
        let serif = (y < thick || (beam_h - thick..beam_h).contains(&y))
            && from_middle <= f64::from(serif) + 0.5;
        // The arc: texels whose distance from the centre is the radius, sampled at
        // the texel's middle.
        let (dx, dy) = (f64::from(x) + 0.5 - ccx, f64::from(y) + 0.5 - ccy);
        let arc = y >= apex && ((dx * dx + dy * dy).sqrt() - r).abs() <= half;
        u32::from(stem || serif || arc) * 255
    })
}

/// The crosshair's geometry in physical texels, shared with [`loaded_crosshair`],
/// which draws the same four rectangles into a larger buffer and needs to know where
/// they end and which texel it still aims through. The centre gap lets you see the
/// pixel you are aiming at.
#[derive(Clone, Copy)]
struct Cross {
    /// Arm length from the centre gap outward; the whole cursor is about half of
    /// `IDC_CROSS`'s 32 texels.
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
        // Arms occupy `[arm+gap, arm+gap+thick)` on the cross axis.
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
    /// Bounded, because it is called over a buffer the badge has grown, where an
    /// unbounded `on_arm` would run the horizontal arm out past the end.
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
    /// The same point `Hotspot::Middle` gives the plain crosshair; the loaded
    /// buffer is not symmetric, and `the_badge_does_not_move_the_aiming_point`
    /// holds the two together.
    fn aim(&self) -> i32 {
        self.near + self.thick / 2
    }

    /// Where a badge's top-left corner goes: diagonally out past the gap, in the
    /// quadrant the arms leave empty, plus the rim so the chip's outline is not
    /// welded to the arm's.
    fn badge_at(&self) -> i32 {
        self.near + self.thick + self.gap + self.rim
    }
}

/// Size of the badge's digits: the smallest that reads at 1x, where at eight the
/// `2` of a `12` closes up.
const BADGE_PT: f32 = 9.0;
/// Clear space between the digits and the chip's edge, each side.
const BADGE_PAD_PT: f32 = 1.0;
/// Corner radius of the chip, clamped against the chip's own size in [`badge`].
const BADGE_ROUND_PT: f32 = 2.0;

/// The Image tool's loaded cursor: [`crosshair`] with a count badge hung off the
/// bottom-right.
///
/// The digits are knocked out of a solid chip, not drawn as ink: a nine-point digit
/// at 1x is a one-texel stroke, so drawn ink becomes a white hairline in a black
/// outline. Knocked out, [`compose`] paints the chip white and the rim's dilation
/// turns the holes dark at every scale.
///
/// The hotspot stays on the crosshair's aiming point ([`Hotspot::At`]), because the
/// buffer grows down and right and its middle is no longer the cross's.
fn loaded_crosshair(
    ctx: &egui::Context,
    scale: f32,
    count: Remaining,
) -> Option<egui::CustomCursorImage> {
    let cross = Cross::new(scale);
    let (bw, bh, chip) = badge(ctx, scale, &count.label())?;
    let at = cross.badge_at();
    let aim = cross.aim();
    // The badge only ever grows the buffer.
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
    // Never narrower than tall: Inter's `1` is three texels wide at 1x, so padding
    // alone would make a white bar.
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
            // Outside the corner arcs is off the chip: the texel's centre against
            // the nearest point of the rounded rect's inner box.
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

/// One short string's coverage, lifted out of the font atlas as [`rasterize`] lifts
/// a glyph. A glyph's `pos` and `uv_rect.offset` are logical, so they are scaled
/// here; a miss is invisible at 1x and shears the digits apart at 1.5x.
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
    // Before the fold, not after: with nothing placed the fold returns its seeds
    // and `hi_x - lo_x` overflows (a debug panic, a release wrap), which the
    // `w <= 0` guard below is too late to catch. Not reachable from the app today.
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
/// Drawn rather than taken from Phosphor: its rings stay rings at any turn, so the
/// orientation (which handle you have) would be invisible, and it has no
/// two-headed arc. An arc of about 120° with a head at each end (a rotate drag goes
/// either way) reads at a glance.
///
/// Sized from the circumscribed circle, which makes the angle free: the outermost
/// ink is `RADIUS_PT + HEAD_W_PT` from the hotspot at any angle, so nothing clips,
/// and the angle goes into the geometry, so nothing is resampled.
fn rotate_arc(scale: f32, angle: f32) -> Option<egui::CustomCursorImage> {
    /// Radius of the arc's centreline.
    const RADIUS_PT: f32 = 5.6;
    /// Width of the band.
    const THICK_PT: f32 = 1.9;
    /// Half an arrowhead's base, measured radially; wider than the band it caps.
    const HEAD_W_PT: f32 = 2.3;
    /// An arrowhead's length along the tangent; longer than its base is wide, or the
    /// head reads as the band flaring.
    const HEAD_L_PT: f32 = 4.0;
    /// Half the *band's* sweep, in radians. Each head adds `HEAD_L / RADIUS`, so the
    /// figure spans about 128°, far from a ring.
    const BAND: f32 = 0.40;
    /// Samples per texel per axis: antialiasing by counting plain insideness hits.
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

    // Precomputed: `compose` calls its coverage closure once per texel and once per
    // texel of the rim's dilation window, so inline supersampling would repeat the
    // geometry.
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

    // `n` is odd and `compose` pads both sides equally, so `Middle` lands on the
    // texel `centre` puts the centre of rotation in.
    compose(scale, (n, n), Hotspot::Middle, |x, y| {
        if x < 0 || y < 0 || x >= n || y >= n {
            return 0;
        }
        grid[(y * n + x) as usize]
    })
}

/// Turn a coverage field into a cursor bitmap: white core, dark rim, straight
/// alpha. Shared by the glyph-sourced and the drawn cursors.
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
            // The rim is the shape dilated by `rim_px`; its alpha is the occupancy.
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
            // White at the core, black in the dilated ring, the ratio between.
            let lum = (fill * 255 / rim) as u8;
            rgba.extend_from_slice(&[lum, lum, lum, rim as u8]);
        }
    }

    // A glyph the font lacks lays out blank, and a transparent cursor would look
    // like the pointer had vanished.
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
/// Measured, since the tip is not at the bitmap's corner. The search direction and
/// the mirror in [`rasterize`] must move together;
/// `the_pen_points_from_its_nib_not_its_middle` holds them.
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

    /// Where a cursor's ink sits, as an offset from its hotspot in texels, weighted
    /// by alpha so the answer does not jump with the rasterizer's phase.
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

    /// Each cursor has ink, a light core and a dark rim: a wrong Phosphor codepoint
    /// lays out blank, and white-on-white is invisible over an artboard.
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
            ("eyedropper", Bitmap::Eyedropper),
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

    /// The Scale cursor is a bitmap of its own, not one of the others.
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

    /// The discard cursor is a bitmap of its own: a transposed codepoint
    /// (`trash-simple` U+E4A8 vs `trash` U+E4A6) yields some other glyph, which only
    /// differing bitmaps reveal.
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

    /// A pen points with its nib, not the bitmap's middle.
    #[test]
    fn the_pen_points_from_its_nib_not_its_middle() {
        let ctx = context();
        let mut cursors = Cursors::default();
        let pen = cursors.image(&ctx, Bitmap::Pen).expect("pen");
        let (w, h) = (i32::from(pen.size[0]), i32::from(pen.size[1]));
        let (hx, hy) = (i32::from(pen.hotspot[0]), i32::from(pen.hotspot[1]));
        // The glyph is mirrored so the tip is up-left, so the hotspot belongs in the
        // top-left; dropping the reflection in `rasterize` would put it at the bottom.
        assert!(hx < w / 3, "hotspot x {hx} is not near the left of {w}");
        assert!(hy < h / 3, "hotspot y {hy} is not near the top of {h}");
    }

    /// Every rotate step is its own drawing; a `Turn` that never reached the pixels
    /// would leave every handle looking identical.
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

    /// The arc points where it was asked to: its ink centroid, measured from the
    /// hotspot, lies on the ray it faces. Measuring from the hotspot also pins the
    /// centre of rotation to it.
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
            // Wrapped into (−π, π], not via `sin().asin()`, which folds 180° onto 0.
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

    /// Nothing clips at any angle: no ink on the buffer's border, and the alpha mass
    /// is within 4% across steps (alpha-weighted, since a texel count swings on
    /// rasterizer phase). The border arm only catches the padding being trimmed,
    /// since the arc is nearly round; the mass arm catches a shortened head.
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

    /// It is an arc, not a ring: the half-plane behind the facing direction is
    /// empty, which a closed glyph with an off-centre centroid would not satisfy.
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

    /// A screen direction (y down, the sign a maths-axes `toward` would get wrong on
    /// every corner) picks the step facing it.
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
            // 45° is a whole number of 15° steps, so each diagonal lands on one.
            assert!(
                (degrees / 360.0 * steps).fract() == 0.0,
                "{name} does not land on a step, so this asserts the wrong one"
            );
        }
        // Between two steps, the nearer wins rather than the earlier.
        assert_eq!(Turn::toward(egui::Vec2::angled(0.24)), Turn(1));
        assert_eq!(Turn::toward(egui::Vec2::angled(0.02)), Turn(0));
        // A degenerate direction answers rather than panics.
        assert_eq!(Turn::toward(egui::Vec2::ZERO), Turn(0));
    }

    /// The crosshair is well under the stock `IDC_CROSS`'s size, the reason it is
    /// drawn.
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

    /// The aiming point is see-through: the arms never meet over it.
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

    /// The same buffer comes back by `Arc` identity, which egui dedupes uploads on.
    #[test]
    fn the_bitmaps_are_reused_rather_than_rebuilt() {
        let ctx = context();
        let mut cursors = Cursors::default();
        let first = cursors.image(&ctx, Bitmap::HandOpen).expect("open hand");
        let again = cursors.image(&ctx, Bitmap::HandOpen).expect("open hand");
        assert!(std::sync::Arc::ptr_eq(&first.rgba, &again.rgba));
        // Every rotate step has a slot of its own.
        let one = cursors.image(&ctx, Bitmap::Rotate(Turn(1))).unwrap();
        let one_again = cursors.image(&ctx, Bitmap::Rotate(Turn(1))).unwrap();
        assert!(std::sync::Arc::ptr_eq(&one.rgba, &one_again.rgba));
    }

    /// Every display scale a badge has to survive: 1.25 and 1.5 are Windows norms
    /// where rounding can undo a change; at 3.0 the digits thicken faster than the
    /// rim, which the knockout depends on reaching into a stroke.
    const SCALES: [f32; 5] = [1.0, 1.25, 1.5, 2.0, 3.0];

    /// A context at `scale`, with a pass run so the atlas is rasterized for it.
    fn scaled(scale: f32) -> egui::Context {
        let ctx = context();
        ctx.set_pixels_per_point(scale);
        let _ = ctx.run_ui(Default::default(), |_| {});
        ctx
    }

    /// The badge does not move the aiming point: the loaded cursor's hotspot equals
    /// the plain crosshair's (the buffer grows, so `Hotspot::Middle` would be off),
    /// and the point stays see-through.
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

    /// Every count is its own drawing, and the clamp holds from both sides: 99
    /// differs from 100, and counts past the clamp share a bitmap, which bounds the
    /// cache.
    #[test]
    fn the_badge_draws_the_count_it_was_given() {
        let ctx = scaled(1.0);
        let mut cursors = Cursors::default();
        let of = |n: usize| Bitmap::Loaded(Remaining::of(n).expect("loaded"));
        let mut seen: Vec<(usize, std::sync::Arc<[u8]>)> = Vec::new();
        for n in [1usize, 2, 3, 9, 10, 42, 99] {
            let img = cursors.image(&ctx, of(n)).expect("loaded cursor");
            // `assert!` rather than `assert_ne!`, which would print both buffers.
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

    /// The digits are knocked out of the chip and survive the rim: each digit texel
    /// is opaque and dark. A hole the rim's dilation cannot reach (strokes outrunning
    /// the rim at a high scale) goes transparent; flip: `BADGE_PT` 9 → 40 → fails at
    /// the transparency assertion. The darkness arm restates the coverage.
    ///
    /// Digit texels come from the badge's own coverage, as the gaps not reachable by
    /// filling in from the chip's border.
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
                let label = Remaining::of(n).expect("loaded").label();
                let (bw, bh, chip) = badge(&ctx, scale, &label).expect("badge");
                // Gaps reached from the border are the rounded corners, not digits;
                // a one-texel stroke is never enclosed on four sides. A knocked-out
                // texel is below `KNOCKED`, not zero: an antialiased one-texel stem
                // never reaches a clean hole.
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

    /// An empty cursor is not a badge reading zero.
    #[test]
    fn an_unloaded_cursor_has_no_badge() {
        assert_eq!(Remaining::of(0), None);
        assert!(Remaining::of(1).is_some());
    }

    /// The type-on-a-path pointer is mirror-symmetric and its curve is its widest
    /// mark (§15 D408). Asserted on alpha, since the dark rim is part of the drawing.
    /// flip: build the beam from `cx` in `text_on_path` → fails at the lopsided
    /// assertion at 1.5x.
    #[test]
    fn the_type_on_a_path_pointer_is_symmetric_and_its_curve_is_the_wider_mark() {
        // `pt(1.0)` is even at 1.5, 1.75, 2.0 and 2.5, where an index-centred beam
        // is off; 1.0 is symmetric regardless. 2.5 is a control that stays clean.
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

    /// A label with no ink answers `None` instead of overflowing the bounding-box
    /// fold in `digits`. flip: move `if placed.is_empty()` below the
    /// `let (w, h) = …` → panics "attempt to subtract with overflow" in debug.
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
