//! The canvas cursors: every pointer shown over the canvas, the arrow included,
//! drawn by the app so each one can say what a press there would do.
//!
//! Each cursor is a base drawing with at most one badge, both vector drawings on a
//! box 32 units square ([`Glyph`]), a unit being a point at pointer size 1, or
//! [`POINTING_UNIT_PT`] for the drawings that point from a tip.
//! [`Cursors::image`] rasterizes one per (cursor, pointer
//! size, display scale) and hands it to egui as a real OS cursor, so it neither lags
//! the pointer nor clips at the window edge. A `CursorIcon` is still set alongside
//! every bitmap as the fallback when rasterizing fails.
//!
//! A drawing is painted twice: a **rim** pass in the set's ink, the shape widened by
//! a 1.5 pt mitred outline (strokes get square caps), then the **core** pass in its
//! fill on top. A soft shadow goes under both, drawn into the bitmap as the OS draws
//! one under its own pointers. Windows gets the light set (white core, dark rim),
//! macOS and Linux the dark set, matching each OS's default pointer.

use eframe::egui;

/// Which way a turned cursor faces: a **screen** direction (y down), quantized to
/// [`Turn::STEPS`] steps of a full turn.
///
/// Quantizing is required: egui dedupes cursor uploads by `Arc` pointer identity,
/// so a raw angle would upload a new cursor every frame of a drag.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Default, Debug)]
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

    /// The same line, either way along it: a double-headed cursor turned by 180°
    /// is the same drawing, so resize and skew cache twelve steps, not twenty-four.
    fn axis(self) -> Self {
        Self(self.0 % (Self::STEPS / 2))
    }

    /// This step in degrees clockwise on screen. Step 0 is +x, due right.
    fn degrees(self) -> f32 {
        f32::from(self.0) * 360.0 / f32::from(Self::STEPS)
    }
}

/// Which way a guide or a gap runs on screen.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Axis {
    /// A vertical line, dragged across.
    Vertical,
    /// A horizontal line, dragged up and down.
    Horizontal,
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

/// What a badge says a press would do. One glyph, one meaning, across every base.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Badge {
    /// Adds: to the selection, a guide, a point.
    Plus,
    /// Subtracts from the selection.
    Minus,
    /// Leaves a copy: a second arrow of the same size behind the first.
    Duplicate,
    /// Picks the deepest layer.
    Deep,
    /// A handle's dot.
    Dot,
    /// The pivot, or drawing from the centre.
    Pivot,
    /// Reorders a loose run.
    Swap,
    /// A press does nothing here.
    No,
    /// The Scale tool.
    Scale,
    /// An anchor.
    Anchor,
    /// A segment.
    Segment,
    /// Converts a point.
    Convert,
    /// Bends a segment.
    Bend,
    /// Closes the path.
    Close,
    /// Continues an open end.
    Continue,
    /// Joins two open ends.
    Join,
    /// Starts a new subpath.
    Star,
}

/// What the canvas sets: the stock cursor egui falls back to, and the drawn one.
pub type Pointer = (egui::CursorIcon, Option<Bitmap>);

/// A cursor the canvas can show.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Bitmap {
    /// The Select tool's arrow, badged with what a press would do.
    Arrow(Option<Badge>),
    /// The node tool's arrow: the select arrow with its inks swapped, so the two
    /// tools read apart at a glance.
    Node(Option<Badge>),
    /// A box, point-box, crop or spacing handle, along the handle's direction.
    Resize(Turn),
    /// Ctrl on a side handle, along the side.
    Skew(Turn),
    /// The band outside a corner, facing out from the pivot towards the pointer.
    Rotate(Turn),
    /// The create tools, about to drag out a box.
    Crosshair,
    /// A create tool with Alt held: the drag grows from its centre.
    FromCentre,
    /// The Image tool's loaded cursor: the crosshair with how many files are still
    /// to be placed. The count is drawn for a single image too, since it is the only
    /// thing telling this apart from a shape tool.
    Loaded(Remaining),
    /// The pen, badged with what its press does.
    Pen(Option<Badge>),
    /// The pencil; badged [`Badge::Continue`] over a path's open end.
    Pencil(Option<Badge>),
    /// The text I-beam, over text and in a text edit.
    IBeam,
    /// The Text tool over an outline, where a click sets type along it.
    TextOnPath,
    /// An armed eyedropper off the canvas, where the loupe gives way.
    Eyedropper,
    /// A ruler, a guide, or a guide being dragged: arrows either side of the line.
    Guide(Axis),
    /// Alt during a guide drag: the drop leaves a copy behind.
    CopyGuide(Axis),
    /// A guide over its own ruler, where letting go deletes it.
    Discard,
    /// A loose run's gap: a bar either side of the arrows.
    Gap(Axis),
    /// Panning is armed: space held, the Hand tool, inside a crop.
    Pan,
    /// The view or the cropped image is being dragged.
    Panning,
}

impl Bitmap {
    /// The bitmap's drawings, in paint order, with where the pointer aims in the
    /// 32-unit box. A resize and skew of opposite headings share one drawing.
    fn recipe(self) -> Recipe {
        use Glyph as G;
        let at = |g: G| Item::at(g);
        let badge = |b: Option<Badge>, left: bool| {
            b.map(|b| {
                let item = Item::at(Glyph::of(b));
                // The pen family's bodies fill the bottom-right corner.
                match left {
                    true => item.shifted(-15.5, 1.0),
                    false => item,
                }
            })
        };
        let one = |items: Vec<Item>, hot: Hot| Recipe {
            layers: vec![items],
            hot,
            inverted: false,
        };
        match self {
            Self::Arrow(Some(Badge::Duplicate)) | Self::Node(Some(Badge::Duplicate)) => Recipe {
                // The copy behind: drawn first, so the front arrow's rim and
                // shadow fall on it. Both sit `DUPLICATE_DROP` lower, so the
                // copy's mitred tip clears the box's top.
                layers: vec![
                    vec![
                        at(G::Arrow)
                            .shifted(DUPLICATE_OFFSET.0, DUPLICATE_DROP + DUPLICATE_OFFSET.1),
                    ],
                    vec![at(G::Arrow).shifted(0.0, DUPLICATE_DROP)],
                ],
                hot: Hot::DUPLICATE,
                inverted: matches!(self, Self::Node(_)),
            },
            Self::Arrow(b) | Self::Node(b) => Recipe {
                layers: vec![
                    [Some(at(G::Arrow)), badge(b, false)]
                        .into_iter()
                        .flatten()
                        .collect(),
                ],
                hot: Hot::ARROW,
                inverted: matches!(self, Self::Node(_)),
            },
            Self::Resize(t) => one(vec![at(G::Resize).turned(t.axis())], Hot::MIDDLE),
            Self::Skew(t) => one(vec![at(G::Skew).turned(t.axis())], Hot::MIDDLE),
            // The arc is drawn bulging up, i.e. facing −y: a quarter turn back.
            Self::Rotate(t) => one(
                vec![at(G::Rotate).turned_by(t.degrees() + 90.0)],
                Hot::MIDDLE,
            ),
            Self::Crosshair => one(vec![at(G::Cross)], Hot::MIDDLE),
            Self::FromCentre => one(vec![at(G::Cross), at(G::Pivot)], Hot::MIDDLE),
            Self::Loaded(_) => one(vec![at(G::Cross)], Hot::MIDDLE),
            Self::Pen(b) => one(
                [Some(at(G::Pen)), badge(b, true)]
                    .into_iter()
                    .flatten()
                    .collect(),
                Hot::TIP,
            ),
            Self::Pencil(b) => one(
                [Some(at(G::Pencil)), badge(b, true)]
                    .into_iter()
                    .flatten()
                    .collect(),
                Hot::TIP,
            ),
            Self::IBeam => one(vec![at(G::IBeam)], Hot::MIDDLE),
            Self::TextOnPath => one(vec![at(G::TextOnPath)], Hot(16.0, 11.5)),
            Self::Eyedropper => one(vec![at(G::Dropper)], Hot::DROPPER),
            Self::Guide(a) => one(vec![at(G::Guide).turned(a.turn())], Hot::MIDDLE),
            Self::CopyGuide(a) => one(
                vec![at(G::Guide).turned(a.turn()), at(G::Plus).shifted(1.0, 3.0)],
                Hot::MIDDLE,
            ),
            Self::Discard => one(vec![at(G::Bin)], Hot::MIDDLE),
            Self::Gap(a) => one(vec![at(G::Gap).turned(a.turn())], Hot::MIDDLE),
            Self::Pan => one(vec![at(G::Hand)], Hot::MIDDLE),
            Self::Panning => one(vec![at(G::Fist)], Hot::MIDDLE),
        }
    }
}

impl Axis {
    /// The turn that stands the drawing, which is drawn vertical, along this axis.
    fn turn(self) -> Turn {
        match self {
            Self::Vertical => Turn(0),
            Self::Horizontal => Turn(Turn::STEPS / 4),
        }
    }
}

/// Where the duplicate cursor's copy sits from the front arrow, in units: 2 pt
/// right and 3 pt up at the pointing drawings' 0.75 pt a unit.
const DUPLICATE_OFFSET: (f32, f32) = (8.0 / 3.0, -4.0);
/// How far both of the duplicate cursor's arrows sit below the plain arrow's
/// place in the box, in units.
const DUPLICATE_DROP: f32 = 3.0;

/// The side of the box every drawing is laid out on, in drawing units.
const BOX: f32 = 32.0;
/// Points per drawing unit for the pointing drawings (the arrows, pen, pencil and
/// dropper) at pointer size 1: at 0.75 the arrow, outline included, is the OS's
/// own size (Windows' is 12 × 19 texels at 100%). The other drawings take a point
/// per unit, where their arrows already match the OS's resize cursors, and their
/// one-unit lines stay a whole texel at 1x.
const POINTING_UNIT_PT: f32 = 0.75;
/// Half the rim's width, in points: the shape widened by a 1.5 pt outline. Under
/// 1.0 on purpose, so at 125% and 150% the rim stays one texel rather than two.
/// In points, not units, so the outline keeps its weight at either unit size.
const RIM_PT: f32 = 0.75;
/// The rim's mitre limit, as SVG counts it: past this a join is bevelled.
const MITRE_LIMIT: f32 = 8.0;
/// The shadow's blur, its drop, and how dark it is at its densest.
const SHADOW_BLUR_PT: f32 = 0.7;
const SHADOW_DROP_PT: f32 = 0.6;
const SHADOW_ALPHA: f32 = 0.32;
/// The dark ink both sets use, slightly off black as the OS pointers are. These are
/// the cursor's own inks, the same under either theme, so not theme colours.
const DARK: [f32; 3] = [
    0x12 as f32 / 255.0,
    0x13 as f32 / 255.0,
    0x16 as f32 / 255.0,
];
const LIGHT: [f32; 3] = [1.0, 1.0, 1.0];
/// The loaded badge's digits, in units: the size whose figures fill the chip's
/// height with a point of clear space above and below.
const DIGIT_PT: f32 = 6.6; // text-size: drawn into the cursor's bitmap, not the chrome

/// Where in the 32-unit box a cursor aims.
#[derive(Clone, Copy, PartialEq, Debug)]
struct Hot(f32, f32);

impl Hot {
    /// The arrow's tip.
    const ARROW: Self = Self(6.0, 4.0);
    /// The middle of the box, for the symmetric drawings.
    const MIDDLE: Self = Self(16.0, 16.0);
    /// The pen's and pencil's point.
    const TIP: Self = Self(5.5, 5.0);
    /// The dropper's point.
    const DROPPER: Self = Self(5.0, 5.0);
    /// The duplicate cursor's front arrow's tip.
    const DUPLICATE: Self = Self(6.0, 4.0 + DUPLICATE_DROP);
}

/// What one bitmap is made of.
struct Recipe {
    /// Painted in order, each rim then core, so a later layer's rim cuts an earlier
    /// layer; within one layer the rims merge.
    layers: Vec<Vec<Item>>,
    hot: Hot,
    /// The inks swapped: the node tool's arrow.
    inverted: bool,
}

impl Recipe {
    /// Points per drawing unit: smaller for the drawings that point from a tip.
    fn unit_pt(&self) -> f32 {
        match [Hot::ARROW, Hot::DUPLICATE, Hot::TIP, Hot::DROPPER].contains(&self.hot) {
            true => POINTING_UNIT_PT,
            false => 1.0,
        }
    }

    /// The drawing moved onto the texel grid at `unit` texels a unit, in texels.
    /// One aimed from its middle has its one-unit lines centred there: where a
    /// unit holds an odd number of whole texels, that centre goes on a texel's
    /// middle, or a line a texel wide smears into two grey ones, and an even
    /// number keeps it on a texel edge. One aimed from a tip has the tip on a
    /// texel corner, so the arrow's upright edge is one crisp column.
    fn nudge(&self, unit: f32) -> (f32, f32) {
        match self.hot == Hot::MIDDLE {
            true => {
                let centre = 16.0 * unit;
                let odd = (unit.floor() as i32) % 2 == 1;
                let d = centre.floor() + if odd { 0.5 } else { 0.0 } - centre;
                (d, d)
            }
            false => {
                let (x, y) = (self.hot.0 * unit, self.hot.1 * unit);
                (x.round() - x, y.round() - y)
            }
        }
    }
}

/// One drawing placed in the box: turned about the box's middle, then shifted.
#[derive(Clone, Copy)]
struct Item {
    glyph: Glyph,
    /// Degrees clockwise on screen.
    degrees: f32,
    shift: (f32, f32),
}

impl Item {
    fn at(glyph: Glyph) -> Self {
        Self {
            glyph,
            degrees: 0.0,
            shift: (0.0, 0.0),
        }
    }

    fn shifted(self, x: f32, y: f32) -> Self {
        Self {
            shift: (x, y),
            ..self
        }
    }

    fn turned(self, t: Turn) -> Self {
        self.turned_by(t.degrees())
    }

    fn turned_by(self, degrees: f32) -> Self {
        Self { degrees, ..self }
    }

    /// A point of the box back into the drawing's own frame.
    fn glyph_point(&self, p: [f32; 2]) -> [f32; 2] {
        let (x, y) = (p[0] - self.shift.0 - 16.0, p[1] - self.shift.1 - 16.0);
        let (s, c) = (-self.degrees.to_radians()).sin_cos();
        [x * c - y * s + 16.0, x * s + y * c + 16.0]
    }
}

/// The drawings, on the 32-unit box. Paths are SVG path data, absolute commands
/// only (`M L H V C Q A Z`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Glyph {
    Arrow,
    Cross,
    Pen,
    Pencil,
    Dropper,
    IBeam,
    TextOnPath,
    Resize,
    Skew,
    Rotate,
    Guide,
    Gap,
    Bin,
    Hand,
    Fist,
    Plus,
    Minus,
    Deep,
    Dot,
    Pivot,
    No,
    Swap,
    Scale,
    Anchor,
    Segment,
    Convert,
    Bend,
    Close,
    Continue,
    Join,
    Star,
}

impl Glyph {
    const ALL: [Self; 31] = [
        Self::Arrow,
        Self::Cross,
        Self::Pen,
        Self::Pencil,
        Self::Dropper,
        Self::IBeam,
        Self::TextOnPath,
        Self::Resize,
        Self::Skew,
        Self::Rotate,
        Self::Guide,
        Self::Gap,
        Self::Bin,
        Self::Hand,
        Self::Fist,
        Self::Plus,
        Self::Minus,
        Self::Deep,
        Self::Dot,
        Self::Pivot,
        Self::No,
        Self::Swap,
        Self::Scale,
        Self::Anchor,
        Self::Segment,
        Self::Convert,
        Self::Bend,
        Self::Close,
        Self::Continue,
        Self::Join,
        Self::Star,
    ];

    /// The badge's drawing. The duplicate badge is a whole second arrow, laid out
    /// by [`Bitmap::recipe`], so it maps to the arrow here.
    fn of(b: Badge) -> Self {
        match b {
            Badge::Plus => Self::Plus,
            Badge::Minus => Self::Minus,
            Badge::Duplicate => Self::Arrow,
            Badge::Deep => Self::Deep,
            Badge::Dot => Self::Dot,
            Badge::Pivot => Self::Pivot,
            Badge::Swap => Self::Swap,
            Badge::No => Self::No,
            Badge::Scale => Self::Scale,
            Badge::Anchor => Self::Anchor,
            Badge::Segment => Self::Segment,
            Badge::Convert => Self::Convert,
            Badge::Bend => Self::Bend,
            Badge::Close => Self::Close,
            Badge::Continue => Self::Continue,
            Badge::Join => Self::Join,
            Badge::Star => Self::Star,
        }
    }

    fn parts(self) -> Vec<Part> {
        use Part::*;
        match self {
            Self::Arrow => vec![Fill(
                "M6 4V23.2L10.5 19L13.6 25.9L16.6 24.6L13.6 17.8H19.7Z",
            )],
            Self::Cross => vec![
                Rect(15.5, 9.0, 1.0, 5.0, 0.0),
                Rect(15.5, 18.0, 1.0, 5.0, 0.0),
                Rect(9.0, 15.5, 5.0, 1.0, 0.0),
                Rect(18.0, 15.5, 5.0, 1.0, 0.0),
            ],
            Self::Pen => vec![
                EvenOdd(
                    "M5.5 5L17.2 7.9L22.6 19.2L19.2 22.6L7.9 17.2Z\
                     M14.2 13.2A1 1 0 1 0 12.2 13.2A1 1 0 1 0 14.2 13.2Z\
                     M6.9 6.3L12.5 11.9L11.9 12.5L6.3 6.9Z",
                ),
                Fill("M20.4 23.8L23.8 20.4L27.4 24L24 27.4Z"),
            ],
            Self::Pencil => vec![
                Fill("M5.5 5L11.3 7L7 11.3Z"),
                Fill("M12.2 7.9L26 21.7L21.7 26L7.9 12.2Z"),
            ],
            Self::Dropper => vec![
                Fill("M5 5L9 5.8L18.6 15.4L15.4 18.6L5.8 9Z"),
                Fill("M14.2 19L19 14.2L21.4 16.6L16.6 21.4Z"),
                Fill("M19.1 21.1L21.1 19.1L23.3 21.3L21.3 23.3Z"),
                Circle(24.0, 24.0, 3.8),
            ],
            Self::IBeam => vec![
                Fill(
                    "M12.8 8C14.3 8 15.4 8.5 16 9.3C16.6 8.5 17.7 8 19.2 8V9.2\
                     C17.6 9.2 16.6 9.8 16.6 10.9H15.4C15.4 9.8 14.4 9.2 12.8 9.2Z",
                ),
                Fill(
                    "M12.8 24C14.3 24 15.4 23.5 16 22.7C16.6 23.5 17.7 24 19.2 24V22.8\
                     C17.6 22.8 16.6 22.2 16.6 21.1H15.4C15.4 22.2 14.4 22.8 12.8 22.8Z",
                ),
                Rect(15.4, 10.5, 1.2, 11.0, 0.0),
            ],
            Self::TextOnPath => vec![
                Rect(13.4, 5.8, 5.2, 1.1, 0.0),
                Rect(15.45, 6.5, 1.1, 9.8, 0.0),
                Rect(13.4, 16.0, 5.2, 1.1, 0.0),
                Stroke("M9 23.8Q16 17.6 23 23.8", 1.2),
            ],
            Self::Resize => vec![Fill(
                "M5 16L10 12.4V15.2H22V12.4L27 16L22 19.6V16.8H10V19.6Z",
            )],
            Self::Skew => vec![
                Fill("M5 11.2H22V9.1L27 12.2L22 15.3V13.2H5Z"),
                Fill("M27 19H10V16.9L5 20L10 23.1V21H27Z"),
            ],
            Self::Rotate => vec![
                Stroke("M8 16.3A10 10 0 0 1 24 16.3", 1.6),
                Fill("M6.3 18.7L9.9 17.6L6.2 15Z"),
                Fill("M25.7 18.7L22.1 17.6L25.8 15Z"),
            ],
            Self::Guide => vec![
                Rect(14.3, 8.0, 1.1, 16.0, 0.0),
                Rect(16.6, 8.0, 1.1, 16.0, 0.0),
                Fill("M4.5 16L8.5 13.1V15.2H13V16.8H8.5V18.9Z"),
                Fill("M27.5 16L23.5 13.1V15.2H19V16.8H23.5V18.9Z"),
            ],
            Self::Gap => vec![
                Rect(5.0, 9.5, 1.2, 13.0, 0.0),
                Rect(25.8, 9.5, 1.2, 13.0, 0.0),
                Fill("M7.6 16L11.1 13.2V15.2H20.9V13.2L24.4 16L20.9 18.8V16.8H11.1V18.8Z"),
            ],
            Self::Bin => vec![
                Rect(9.5, 10.0, 13.0, 1.8, 0.6),
                Rect(13.8, 7.9, 4.4, 1.6, 0.6),
                EvenOdd(
                    "M11 13.1H21L20.1 24C20.1 24.6 19.7 25 19.1 25H12.9C12.3 25 11.9 24.6 11.9 24Z\
                     M14.2 15.5V22.3H15.1V15.5Z\
                     M16.9 15.5V22.3H17.8V15.5Z",
                ),
            ],
            Self::Hand => vec![
                Rect(10.2, 6.5, 2.8, 11.0, 1.4),
                Rect(13.5, 4.8, 2.8, 12.0, 1.4),
                Rect(16.8, 5.6, 2.8, 11.5, 1.4),
                Rect(20.1, 8.2, 2.6, 9.5, 1.3),
                Fill(
                    "M10.2 14.5H22.7V20.5C22.7 24.5 20.5 27 16.5 27H15C12.5 27 11 25.8 9.8 24\
                     L6.2 18.6C5.5 17.5 5.9 16.3 6.9 15.8C7.9 15.3 9 15.7 9.6 16.6L10.2 17.6Z",
                ),
            ],
            Self::Fist => vec![
                Rect(10.2, 11.0, 2.8, 6.0, 1.4),
                Rect(13.5, 10.0, 2.8, 7.0, 1.4),
                Rect(16.8, 10.4, 2.8, 6.6, 1.4),
                Rect(20.1, 11.6, 2.6, 5.6, 1.3),
                // The palm, with the thumb folded in where the open hand's is.
                Fill(
                    "M10.2 14.5H22.7V20.5C22.7 24.5 20.5 27 16.5 27H15C12.6 27 10.9 25.9 9.8 24.2\
                     L8.1 21.4C7.4 20.2 7.7 18.8 8.8 18.2C9.5 17.8 10.2 17.9 10.2 17.9Z",
                ),
            ],
            Self::Plus => vec![Stroke("M23 20.5V27.5", 1.5), Stroke("M19.5 24H26.5", 1.5)],
            Self::Minus => vec![Stroke("M19.5 24H26.5", 1.5)],
            Self::Deep => vec![
                StrokeRect(19.6, 20.6, 6.8, 6.8, 1.1),
                Rect(21.9, 22.9, 2.2, 2.2, 0.0),
            ],
            Self::Dot => vec![Circle(23.0, 24.0, 2.4)],
            Self::Pivot => vec![Ring(23.0, 24.0, 2.8, 1.1), Circle(23.0, 24.0, 0.9)],
            Self::No => vec![
                Ring(23.0, 24.0, 3.2, 1.2),
                Stroke("M20.8 26.2L25.2 21.8", 1.2),
            ],
            Self::Swap => vec![
                Stroke("M19.5 22H25.8", 1.1),
                Stroke("M26.5 26H20.2", 1.1),
                Fill("M27.5 22L25.3 20.3V23.7Z"),
                Fill("M18.5 26L20.7 24.3V27.7Z"),
            ],
            Self::Scale => vec![
                StrokeRect(19.6, 20.6, 6.8, 6.8, 1.1),
                Stroke("M21.6 25.4L24.4 22.6", 1.1),
                Fill("M25.2 21.8H22.6L25.2 24.4Z"),
            ],
            Self::Anchor => vec![StrokeRect(20.4, 21.4, 5.2, 5.2, 1.2)],
            Self::Segment => vec![Rect(20.6, 21.6, 4.8, 4.8, 0.0)],
            Self::Convert => vec![Stroke("M19.6 27.2L23 21.6L26.4 27.2", 1.4)],
            Self::Bend => vec![Stroke("M19.4 27C20.4 22.4 25.6 22.4 26.6 27", 1.4)],
            Self::Close => vec![Ring(23.0, 24.0, 2.6, 1.3)],
            Self::Continue => vec![
                Stroke("M19.6 27.6L23.6 23.6", 1.3),
                StrokeRect(23.4, 19.6, 4.4, 4.4, 1.2),
            ],
            Self::Join => vec![
                StrokeRect(19.2, 24.0, 4.0, 4.0, 1.1),
                StrokeRect(24.0, 19.2, 4.0, 4.0, 1.1),
                Stroke("M22.8 24.4L24.4 22.8", 1.1),
            ],
            Self::Star => vec![
                Stroke("M23 20.3V27.7", 1.2),
                Stroke("M19.8 22.2L26.2 25.8", 1.2),
                Stroke("M19.8 25.8L26.2 22.2", 1.2),
            ],
        }
    }

    /// The drawing, flattened once and kept. [`Self::ALL`] is in declaration
    /// order, so a drawing missing from it panics here rather than drawing
    /// another's.
    fn shapes(self) -> &'static [Shape] {
        static ALL: std::sync::OnceLock<Vec<Vec<Shape>>> = std::sync::OnceLock::new();
        let all = ALL.get_or_init(|| {
            Self::ALL
                .iter()
                .map(|g| g.parts().into_iter().map(Part::flatten).collect())
                .collect()
        });
        &all[self as usize]
    }
}

/// One piece of a drawing, as the drawing is written.
enum Part {
    /// A filled path, nonzero.
    Fill(&'static str),
    /// A filled path with holes, even-odd.
    EvenOdd(&'static str),
    /// A filled rectangle: x, y, width, height, corner radius.
    Rect(f32, f32, f32, f32, f32),
    /// A filled circle: centre and radius.
    Circle(f32, f32, f32),
    /// A stroked path and its width; butt caps in the core, square in the rim.
    Stroke(&'static str, f32),
    /// A stroked rectangle's outline and its width.
    StrokeRect(f32, f32, f32, f32, f32),
    /// A stroked circle: centre, radius, width.
    Ring(f32, f32, f32, f32),
}

impl Part {
    fn flatten(self) -> Shape {
        let rect = |x: f32, y: f32, w: f32, h: f32, r: f32| {
            let r = r.min(w / 2.0).min(h / 2.0);
            if r <= 0.0 {
                return vec![[x, y], [x + w, y], [x + w, y + h], [x, y + h]];
            }
            // Corners clockwise from the top-right, each a quarter arc.
            let corners = [
                ([x + w - r, y + r], -90.0f32),
                ([x + w - r, y + h - r], 0.0),
                ([x + r, y + h - r], 90.0),
                ([x + r, y + r], 180.0),
            ];
            corners
                .iter()
                .flat_map(|&(c, from)| {
                    (0..=ARC_STEPS / 4).map(move |i| {
                        let a = (from + 90.0 * i as f32 / (ARC_STEPS / 4) as f32).to_radians();
                        [c[0] + r * a.cos(), c[1] + r * a.sin()]
                    })
                })
                .collect()
        };
        let circle = |cx: f32, cy: f32, r: f32| -> Vec<[f32; 2]> {
            (0..ARC_STEPS)
                .map(|i| {
                    let a = std::f32::consts::TAU * i as f32 / ARC_STEPS as f32;
                    [cx + r * a.cos(), cy + r * a.sin()]
                })
                .collect()
        };
        let closed = |pts| Contour { pts, closed: true };
        match self {
            Self::Fill(d) => Shape::fill(parse(d), false),
            Self::EvenOdd(d) => Shape::fill(parse(d), true),
            Self::Rect(x, y, w, h, r) => Shape::fill(vec![closed(rect(x, y, w, h, r))], false),
            Self::Circle(cx, cy, r) => Shape::fill(vec![closed(circle(cx, cy, r))], false),
            Self::Stroke(d, w) => Shape::stroke(parse(d), w),
            Self::StrokeRect(x, y, w, h, sw) => {
                Shape::stroke(vec![closed(rect(x, y, w, h, 0.0))], sw)
            }
            Self::Ring(cx, cy, r, sw) => Shape::stroke(vec![closed(circle(cx, cy, r))], sw),
        }
    }
}

/// Segments in a full circle, when flattening arcs and curves.
const ARC_STEPS: usize = 48;
/// Segments per cubic or quadratic curve.
const CURVE_STEPS: usize = 12;

/// A flattened polyline.
#[derive(Clone, Debug)]
struct Contour {
    pts: Vec<[f32; 2]>,
    closed: bool,
}

/// A flattened drawing piece and how it is painted.
struct Shape {
    contours: Vec<Contour>,
    /// `None` for a fill, else the stroke's width.
    stroke: Option<f32>,
    even_odd: bool,
    /// The contours' bounds, before any widening.
    min: [f32; 2],
    max: [f32; 2],
    /// How many half-widths the outline reaches past the bounds ([`mitre_reach`]).
    reach: f32,
}

impl Shape {
    fn fill(contours: Vec<Contour>, even_odd: bool) -> Self {
        Self::new(contours, None, even_odd)
    }

    fn stroke(contours: Vec<Contour>, width: f32) -> Self {
        Self::new(contours, Some(width), false)
    }

    fn new(contours: Vec<Contour>, stroke: Option<f32>, even_odd: bool) -> Self {
        let (mut min, mut max) = ([f32::MAX; 2], [f32::MIN; 2]);
        for p in contours.iter().flat_map(|c| &c.pts) {
            min = [min[0].min(p[0]), min[1].min(p[1])];
            max = [max[0].max(p[0]), max[1].max(p[1])];
        }
        Self {
            reach: mitre_reach(&contours),
            contours,
            stroke,
            even_odd,
            min,
            max,
        }
    }

    /// Whether `p` is painted, the shape widened by `grow` on every side: zero for
    /// the core, [`RIM_PT`] in units for the rim.
    fn covers(&self, p: [f32; 2], grow: f32) -> bool {
        let half = self.stroke.map_or(0.0, |w| w / 2.0) + grow;
        // A square cap reaches `√2` half-widths past a stroke's end.
        let reach = half * self.reach.max(std::f32::consts::SQRT_2);
        if p[0] < self.min[0] - reach
            || p[1] < self.min[1] - reach
            || p[0] > self.max[0] + reach
            || p[1] > self.max[1] + reach
        {
            return false;
        }
        match self.stroke {
            Some(_) => self.near(p, half, grow > 0.0),
            None => self.inside(p) || (grow > 0.0 && self.near(p, grow, false)),
        }
    }

    /// Inside the filled contours, by the shape's fill rule.
    fn inside(&self, p: [f32; 2]) -> bool {
        let mut winding = 0i32;
        for c in &self.contours {
            let n = c.pts.len();
            for i in 0..n {
                let (a, b) = (c.pts[i], c.pts[(i + 1) % n]);
                if a[1] <= p[1] {
                    if b[1] > p[1] && side(a, b, p) > 0.0 {
                        winding += 1;
                    }
                } else if b[1] <= p[1] && side(a, b, p) < 0.0 {
                    winding -= 1;
                }
            }
        }
        match self.even_odd {
            true => winding % 2 != 0,
            false => winding != 0,
        }
    }

    /// Within `half` of the outline, with mitred joins and, for `square`, ends
    /// extended by `half`; butt ends otherwise.
    fn near(&self, p: [f32; 2], half: f32, square: bool) -> bool {
        for c in &self.contours {
            let n = c.pts.len();
            let segs = if c.closed { n } else { n.saturating_sub(1) };
            for i in 0..segs {
                let (a, b) = (c.pts[i], c.pts[(i + 1) % n]);
                let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
                let len = (dx * dx + dy * dy).sqrt();
                if len < 1e-6 {
                    continue;
                }
                let (ux, uy) = (dx / len, dy / len);
                let (px, py) = (p[0] - a[0], p[1] - a[1]);
                let along = px * ux + py * uy;
                let across = (px * uy - py * ux).abs();
                let open_start = !c.closed && i == 0;
                let open_end = !c.closed && i + 1 == segs;
                let lo = if open_start && square { -half } else { 0.0 };
                let hi = if open_end && square { len + half } else { len };
                if across <= half && along >= lo && along <= hi {
                    return true;
                }
            }
            // The joins: a wedge on each corner's outer side, mitred.
            let joins = if c.closed {
                0..n
            } else {
                1..n.saturating_sub(1)
            };
            for i in joins {
                let v = c.pts[i];
                let prev = c.pts[(i + n - 1) % n];
                let next = c.pts[(i + 1) % n];
                if join_covers(prev, v, next, half, p) {
                    return true;
                }
            }
        }
        false
    }
}

/// Which side of the line `a → b` the point is on.
fn side(a: [f32; 2], b: [f32; 2], p: [f32; 2]) -> f32 {
    (b[0] - a[0]) * (p[1] - a[1]) - (p[0] - a[0]) * (b[1] - a[1])
}

/// Whether `p` lies in the mitre (or, past the limit, the bevel) that fills the
/// outer side of the corner at `v` for an outline `half` wide each way.
fn join_covers(prev: [f32; 2], v: [f32; 2], next: [f32; 2], half: f32, p: [f32; 2]) -> bool {
    let unit = |a: [f32; 2], b: [f32; 2]| {
        let (dx, dy) = (b[0] - a[0], b[1] - a[1]);
        let l = (dx * dx + dy * dy).sqrt();
        (l > 1e-6).then(|| [dx / l, dy / l])
    };
    let (Some(d1), Some(d2)) = (unit(prev, v), unit(v, next)) else {
        return false;
    };
    let turn = d1[0] * d2[1] - d1[1] * d2[0];
    if turn.abs() < 1e-6 {
        return false;
    }
    // The outer side is away from the turn.
    let s = -turn.signum();
    let n1 = [-d1[1] * s, d1[0] * s];
    let n2 = [-d2[1] * s, d2[0] * s];
    let a = [v[0] + n1[0] * half, v[1] + n1[1] * half];
    let b = [v[0] + n2[0] * half, v[1] + n2[1] * half];
    let sum = [n1[0] + n2[0], n1[1] + n2[1]];
    let sq = sum[0] * sum[0] + sum[1] * sum[1];
    // `1 / cos(θ/2)` is the mitre's length over the half-width.
    let ratio = 2.0 / sq.sqrt();
    let (poly, n) = if sq > 1e-9 && ratio <= MITRE_LIMIT {
        let k = 2.0 * half / sq;
        ([v, a, [v[0] + sum[0] * k, v[1] + sum[1] * k], b], 4)
    } else {
        ([v, a, b, b], 3)
    };
    // Convex either way round: inside if on one side of every edge.
    let (mut pos, mut neg) = (true, true);
    for i in 0..n {
        let s = side(poly[i], poly[(i + 1) % n], p);
        pos &= s >= 0.0;
        neg &= s <= 0.0;
    }
    pos || neg
}

/// How far past its corner the sharpest mitre of `contours` reaches, in
/// half-widths: the bound [`Shape::covers`] culls with.
fn mitre_reach(contours: &[Contour]) -> f32 {
    let mut reach = 1.0f32;
    for c in contours {
        let n = c.pts.len();
        let joins = if c.closed {
            0..n
        } else {
            1..n.saturating_sub(1)
        };
        for i in joins {
            let (prev, v, next) = (c.pts[(i + n - 1) % n], c.pts[i], c.pts[(i + 1) % n]);
            let d1 = [v[0] - prev[0], v[1] - prev[1]];
            let d2 = [next[0] - v[0], next[1] - v[1]];
            let (l1, l2) = (d1[0].hypot(d1[1]), d2[0].hypot(d2[1]));
            if l1 < 1e-6 || l2 < 1e-6 {
                continue;
            }
            // `cos θ` between the sides; the mitre is `1 / cos(θ/2)` half-widths.
            let cos = (d1[0] * d2[0] + d1[1] * d2[1]) / (l1 * l2);
            let half_cos = ((1.0 + cos) / 2.0).max(0.0).sqrt();
            let ratio = if half_cos > 1e-6 {
                1.0 / half_cos
            } else {
                f32::MAX
            };
            reach = reach.max(if ratio <= MITRE_LIMIT { ratio } else { 1.0 });
        }
    }
    reach
}

/// SVG path data into polylines. Absolute commands only, which is all the drawings
/// use; a relative command is a mistake in [`Glyph::parts`] and panics in the tests.
fn parse(d: &str) -> Vec<Contour> {
    let mut out: Vec<Contour> = Vec::new();
    let mut cur: Vec<[f32; 2]> = Vec::new();
    let mut at = [0.0f32; 2];
    let mut start = at;
    let mut nums: Vec<f32> = Vec::new();
    let mut cmd = ' ';
    let flush = |cmd: char,
                 nums: &mut Vec<f32>,
                 cur: &mut Vec<[f32; 2]>,
                 at: &mut [f32; 2],
                 start: &mut [f32; 2],
                 out: &mut Vec<Contour>| {
        let n = nums.as_slice();
        match cmd {
            'M' => {
                if cur.len() > 1 {
                    out.push(Contour {
                        pts: std::mem::take(cur),
                        closed: false,
                    });
                }
                cur.clear();
                for (i, p) in n.as_chunks::<2>().0.iter().enumerate() {
                    *at = [p[0], p[1]];
                    if i == 0 {
                        *start = *at;
                    }
                    cur.push(*at);
                }
            }
            'L' => {
                for p in n.as_chunks::<2>().0 {
                    *at = [p[0], p[1]];
                    cur.push(*at);
                }
            }
            'H' => {
                for &x in n {
                    *at = [x, at[1]];
                    cur.push(*at);
                }
            }
            'V' => {
                for &y in n {
                    *at = [at[0], y];
                    cur.push(*at);
                }
            }
            'C' => {
                for c in n.as_chunks::<6>().0 {
                    let p0 = *at;
                    for i in 1..=CURVE_STEPS {
                        let t = i as f32 / CURVE_STEPS as f32;
                        let u = 1.0 - t;
                        let f = |k: usize| {
                            u * u * u * [p0[0], p0[1]][k]
                                + 3.0 * u * u * t * c[k]
                                + 3.0 * u * t * t * c[2 + k]
                                + t * t * t * c[4 + k]
                        };
                        cur.push([f(0), f(1)]);
                    }
                    *at = [c[4], c[5]];
                }
            }
            'Q' => {
                for c in n.as_chunks::<4>().0 {
                    let p0 = *at;
                    for i in 1..=CURVE_STEPS {
                        let t = i as f32 / CURVE_STEPS as f32;
                        let u = 1.0 - t;
                        let f = |k: usize| {
                            u * u * [p0[0], p0[1]][k] + 2.0 * u * t * c[k] + t * t * c[2 + k]
                        };
                        cur.push([f(0), f(1)]);
                    }
                    *at = [c[2], c[3]];
                }
            }
            'A' => {
                for c in n.as_chunks::<7>().0 {
                    arc(*at, c, cur);
                    *at = [c[5], c[6]];
                }
            }
            'Z' => {
                // A contour that ends on its start would repeat the vertex, and a
                // zero-length side has no direction to mitre the corner with.
                if cur.len() > 2
                    && let (Some(a), Some(b)) = (cur.first(), cur.last())
                    && (a[0] - b[0]).abs() + (a[1] - b[1]).abs() < 1e-3
                {
                    cur.pop();
                }
                if cur.len() > 1 {
                    out.push(Contour {
                        pts: std::mem::take(cur),
                        closed: true,
                    });
                }
                *at = *start;
            }
            ' ' => {}
            other => panic!("cursor drawing: unsupported path command {other:?}"),
        }
        nums.clear();
    };
    let mut token = String::new();
    let push_num = |token: &mut String, nums: &mut Vec<f32>| {
        if !token.is_empty() {
            nums.push(token.parse().expect("cursor drawing: a number"));
            token.clear();
        }
    };
    for ch in d.chars() {
        match ch {
            c if c.is_ascii_alphabetic() => {
                push_num(&mut token, &mut nums);
                flush(cmd, &mut nums, &mut cur, &mut at, &mut start, &mut out);
                cmd = c;
            }
            '-' => {
                push_num(&mut token, &mut nums);
                token.push(ch);
            }
            ' ' | ',' => push_num(&mut token, &mut nums),
            '.' if token.contains('.') => {
                push_num(&mut token, &mut nums);
                token.push(ch);
            }
            _ => token.push(ch),
        }
    }
    push_num(&mut token, &mut nums);
    flush(cmd, &mut nums, &mut cur, &mut at, &mut start, &mut out);
    if cur.len() > 1 {
        out.push(Contour {
            pts: cur,
            closed: false,
        });
    }
    out
}

/// An SVG elliptical arc from `from`, its seven numbers `c`, flattened into `out`
/// (the endpoint-to-centre conversion of the SVG spec, appendix B.2.4).
fn arc(from: [f32; 2], c: &[f32], out: &mut Vec<[f32; 2]>) {
    let (mut rx, mut ry) = (c[0].abs(), c[1].abs());
    let phi = c[2].to_radians();
    let (large, sweep) = (c[3] != 0.0, c[4] != 0.0);
    let to = [c[5], c[6]];
    if rx < 1e-6 || ry < 1e-6 {
        out.push(to);
        return;
    }
    let (sp, cp) = phi.sin_cos();
    let (dx, dy) = ((from[0] - to[0]) / 2.0, (from[1] - to[1]) / 2.0);
    let (x1, y1) = (cp * dx + sp * dy, -sp * dx + cp * dy);
    let lambda = (x1 * x1) / (rx * rx) + (y1 * y1) / (ry * ry);
    if lambda > 1.0 {
        rx *= lambda.sqrt();
        ry *= lambda.sqrt();
    }
    let num = rx * rx * ry * ry - rx * rx * y1 * y1 - ry * ry * x1 * x1;
    let den = rx * rx * y1 * y1 + ry * ry * x1 * x1;
    let mut k = (num / den).max(0.0).sqrt();
    if large == sweep {
        k = -k;
    }
    let (cx1, cy1) = (k * rx * y1 / ry, -k * ry * x1 / rx);
    let (cx, cy) = (
        cp * cx1 - sp * cy1 + (from[0] + to[0]) / 2.0,
        sp * cx1 + cp * cy1 + (from[1] + to[1]) / 2.0,
    );
    let angle = |ux: f32, uy: f32| uy.atan2(ux);
    let t1 = angle((x1 - cx1) / rx, (y1 - cy1) / ry);
    let t2 = angle((-x1 - cx1) / rx, (-y1 - cy1) / ry);
    let mut dt = t2 - t1;
    let tau = std::f32::consts::TAU;
    if sweep && dt < 0.0 {
        dt += tau;
    } else if !sweep && dt > 0.0 {
        dt -= tau;
    }
    let steps = ((dt.abs() / tau * ARC_STEPS as f32).ceil() as usize).max(2);
    for i in 1..=steps {
        let t = t1 + dt * i as f32 / steps as f32;
        let (x, y) = (rx * t.cos(), ry * t.sin());
        out.push([cp * x - sp * y + cx, sp * x + cp * y + cy]);
    }
}

/// One cached bitmap; `None` once built means it failed, kept so it is not
/// retried every frame.
type Slot = Option<egui::CustomCursorImage>;

/// The drawn cursors, built on first use and kept while the scale stays put.
///
/// Caching is required, not just an optimization: egui dedupes by `Arc` pointer
/// identity, so a fresh buffer each frame would upload a new cursor every frame.
#[derive(Default)]
pub struct Cursors {
    /// Texels per point the cached bitmaps were drawn at.
    scale: f32,
    built: std::collections::HashMap<Bitmap, Slot>,
    /// The OS pointer size over its default, and when it was last read.
    pointer: Option<(f32, std::time::Instant)>,
}

/// How often the OS pointer size is read again, so a change in the system settings
/// shows without a restart.
const POINTER_REREAD: std::time::Duration = std::time::Duration::from_secs(2);

impl Cursors {
    /// The bitmap for `which`, or `None` if it could not be drawn, in which case
    /// the caller's `CursorIcon` stands on its own.
    pub fn image(&mut self, ctx: &egui::Context, which: Bitmap) -> Option<egui::CustomCursorImage> {
        let scale = texels_per_point(ctx.pixels_per_point()) * self.pointer_scale();
        if self.scale != scale {
            self.scale = scale;
            self.built.clear();
        }
        if let Some(slot) = self.built.get(&which) {
            return slot.clone();
        }
        let slot = draw(ctx, which, scale, cfg!(windows));
        self.built.insert(which, slot.clone());
        slot
    }

    fn pointer_scale(&mut self) -> f32 {
        let now = std::time::Instant::now();
        match self.pointer {
            Some((s, at)) if now.duration_since(at) < POINTER_REREAD => s,
            _ => {
                let s = os_pointer_scale().clamp(1.0, 8.0);
                self.pointer = Some((s, now));
                s
            }
        }
    }
}

/// Texels per point in the bitmap handed to the OS.
///
/// On macOS winit 0.30 sizes the cursor's `NSImage` in points equal to the bitmap's
/// pixel count, so a 2× bitmap shows at twice the size on a Retina display. macOS
/// gets one texel per point, which it shows at the right size.
fn texels_per_point(pixels_per_point: f32) -> f32 {
    match cfg!(target_os = "macos") {
        true => 1.0,
        false => pixels_per_point,
    }
}

/// The OS pointer size over its default: `CursorBaseSize ÷ 32` on Windows, and
/// `XCURSOR_SIZE ÷ 24` on Linux. macOS scales app cursors itself when its pointer
/// size is raised, so there it is 1 and the image is not scaled twice.
fn os_pointer_scale() -> f32 {
    #[cfg(windows)]
    {
        windows_cursor_base_size().map_or(1.0, |px| px as f32 / 32.0)
    }
    #[cfg(target_os = "linux")]
    {
        std::env::var("XCURSOR_SIZE")
            .ok()
            .and_then(|s| s.trim().parse::<f32>().ok())
            .map_or(1.0, |px| px / 24.0)
    }
    #[cfg(not(any(windows, target_os = "linux")))]
    {
        1.0
    }
}

/// `HKCU\Control Panel\Cursors\CursorBaseSize`, the pixel size Windows' pointer
/// size slider writes (32 to 256), or `None` where it was never set.
#[cfg(windows)]
fn windows_cursor_base_size() -> Option<u32> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_DWORD, RegGetValueW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (key, name) = (wide("Control Panel\\Cursors"), wide("CursorBaseSize"));
    let mut value: u32 = 0;
    let mut size = std::mem::size_of::<u32>() as u32;
    // SAFETY: both strings are NUL-terminated and outlive the call, and the out
    // buffer is a `u32` whose size is passed alongside it.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&raw mut value).cast(),
            &raw mut size,
        )
    };
    (status == 0 && value > 0).then_some(value)
}

/// Draw one cursor at `scale` texels per point, in the light set when `light`.
fn draw(
    ctx: &egui::Context,
    which: Bitmap,
    scale: f32,
    light: bool,
) -> Option<egui::CustomCursorImage> {
    let recipe = which.recipe();
    let (mut fill, mut ink) = match light {
        true => (LIGHT, DARK),
        false => (DARK, LIGHT),
    };
    if recipe.inverted {
        std::mem::swap(&mut fill, &mut ink);
    }
    // Texels per drawing unit, and the rim in units.
    let unit = scale * recipe.unit_pt();
    let rim_u = RIM_PT / recipe.unit_pt();
    let n = (BOX * unit).ceil() as usize;
    // Samples per texel per axis: large bitmaps have texels small next to the
    // drawing's features, so fewer suffice.
    let ss = if unit <= 2.0 { 4 } else { 2 };
    let nudge = recipe.nudge(unit);

    // The loaded badge: a chip in the set's core, the count in its ink.
    let chip = match which {
        Bitmap::Loaded(count) => Some(Chip::new(ctx, unit, nudge.0, &count.label())?),
        _ => None,
    };

    // Premultiplied colour and alpha.
    let over = |dst: &mut [f32; 4], rgb: [f32; 3], a: f32| {
        if a <= 0.0 {
            return;
        }
        for k in 0..3 {
            dst[k] = rgb[k] * a + dst[k] * (1.0 - a);
        }
        dst[3] = a + dst[3] * (1.0 - a);
    };
    let total = (ss * ss) as f32;
    // Each layer is drawn with its own shadow and laid over the ones before, so
    // a later layer's shadow falls on an earlier one.
    let mut px = vec![[0.0f32; 4]; n * n];
    for layer in &recipe.layers {
        let mut drawn = vec![[0.0f32; 4]; n * n];
        for y in 0..n {
            for x in 0..n {
                let texel = &mut drawn[y * n + x];
                let (mut rim, mut core) = (0u32, 0u32);
                for j in 0..ss {
                    for i in 0..ss {
                        let p = [
                            (x as f32 + (i as f32 + 0.5) / ss as f32 - nudge.0) / unit,
                            (y as f32 + (j as f32 + 0.5) / ss as f32 - nudge.1) / unit,
                        ];
                        let (mut r, mut c) = (false, false);
                        for item in layer {
                            let q = item.glyph_point(p);
                            for shape in item.glyph.shapes() {
                                c = c || shape.covers(q, 0.0);
                                r = r || c || shape.covers(q, rim_u);
                            }
                            if let Some(chip) = &chip {
                                c = c || chip.covers(p, 0.0);
                                r = r || c || chip.covers(p, rim_u);
                            }
                        }
                        rim += u32::from(r);
                        core += u32::from(c);
                    }
                }
                over(texel, ink, rim as f32 / total);
                over(texel, fill, core as f32 / total);
                if let Some(chip) = &chip {
                    over(texel, ink, chip.digit(x, y));
                }
            }
        }
        // A glyph that drew nothing would look like the pointer had vanished.
        if drawn.iter().all(|p| p[3] <= 0.0) {
            return None;
        }
        for (dst, src) in px.iter_mut().zip(with_shadow(drawn, n, scale)) {
            for k in 0..4 {
                dst[k] = src[k] + dst[k] * (1.0 - src[3]);
            }
        }
    }

    let rgba: Vec<u8> = px
        .iter()
        .flat_map(|p| {
            let a = p[3].clamp(0.0, 1.0);
            let c = |v: f32| match a > 0.0 {
                true => ((v / a).clamp(0.0, 1.0) * 255.0).round() as u8,
                false => 0,
            };
            [c(p[0]), c(p[1]), c(p[2]), (a * 255.0).round() as u8]
        })
        .collect();
    let hot = |v: f32, d: f32| ((v * unit + d).floor() as usize).min(n - 1) as u16;
    Some(egui::CustomCursorImage {
        rgba: rgba.into(),
        size: [n as u16, n as u16],
        hotspot: [hot(recipe.hot.0, nudge.0), hot(recipe.hot.1, nudge.1)],
    })
}

/// The image composited over its own shadow: its alpha blurred, dropped and
/// darkened.
fn with_shadow(px: Vec<[f32; 4]>, n: usize, scale: f32) -> Vec<[f32; 4]> {
    let sigma = SHADOW_BLUR_PT * scale;
    let radius = (sigma * 3.0).ceil() as i32;
    let kernel: Vec<f32> = (-radius..=radius)
        .map(|i| (-(i * i) as f32 / (2.0 * sigma * sigma)).exp())
        .collect();
    let sum: f32 = kernel.iter().sum();
    let at = |v: &[f32], x: i32, y: i32| match (0..n as i32).contains(&x)
        && (0..n as i32).contains(&y)
    {
        true => v[y as usize * n + x as usize],
        false => 0.0,
    };
    let alpha: Vec<f32> = px.iter().map(|p| p[3]).collect();
    let blur = |src: &[f32], dx: i32, dy: i32| -> Vec<f32> {
        (0..n * n)
            .map(|i| {
                let (x, y) = ((i % n) as i32, (i / n) as i32);
                kernel
                    .iter()
                    .zip(-radius..=radius)
                    .map(|(w, k)| w * at(src, x + k * dx, y + k * dy))
                    .sum::<f32>()
                    / sum
            })
            .collect()
    };
    let soft = blur(&blur(&alpha, 1, 0), 0, 1);
    let drop = (SHADOW_DROP_PT * scale).round() as i32;
    px.iter()
        .enumerate()
        .map(|(i, p)| {
            let (x, y) = ((i % n) as i32, (i / n) as i32);
            let shadow = at(&soft, x, y - drop) * SHADOW_ALPHA;
            // The image over a black shadow: the colour is already premultiplied.
            [p[0], p[1], p[2], p[3] + shadow * (1.0 - p[3])]
        })
        .collect()
}

/// The loaded Image cursor's badge: a rounded chip with the count on it.
struct Chip {
    /// The chip, in units.
    shape: Shape,
    /// The count's coverage, in texels, and where its top-left texel sits.
    digits: Vec<u8>,
    size: (usize, usize),
    origin: (i32, i32),
}

impl Chip {
    /// The chip's left and top edge, its height, and the clear space either side of
    /// the count, in units.
    const AT: f32 = 18.6;
    const H: f32 = 8.0;
    const PAD: f32 = 1.8;
    /// The narrowest chip, for one digit.
    const MIN_W: f32 = 9.6;
    /// How far right the chip may reach, so its rim stays clear of the box's last
    /// texel. The crosshair it hangs on takes a point per unit.
    const RIGHT: f32 = BOX - RIM_PT - 0.75;

    /// The chip for `label` at `scale` texels per unit, its count shifted by the
    /// drawing's `nudge` in texels, as the chip itself is.
    fn new(ctx: &egui::Context, scale: f32, nudge: f32, label: &str) -> Option<Self> {
        let (tw, th, digits) = digits(ctx, scale, label)?;
        let w = (tw as f32 / scale + Self::PAD * 2.0).max(Self::MIN_W);
        let x = Self::AT.min(Self::RIGHT - w);
        let shape = Part::Rect(x, Self::AT, w, Self::H, 2.0).flatten();
        let origin = (
            ((x + w / 2.0) * scale + nudge - tw as f32 / 2.0).round() as i32,
            ((Self::AT + Self::H / 2.0) * scale + nudge - th as f32 / 2.0).round() as i32,
        );
        Some(Self {
            shape,
            digits,
            size: (tw as usize, th as usize),
            origin,
        })
    }

    fn covers(&self, p: [f32; 2], grow: f32) -> bool {
        self.shape.covers(p, grow)
    }

    /// The count's coverage at a texel of the bitmap.
    fn digit(&self, x: usize, y: usize) -> f32 {
        let (dx, dy) = (x as i32 - self.origin.0, y as i32 - self.origin.1);
        if dx < 0 || dy < 0 || dx as usize >= self.size.0 || dy as usize >= self.size.1 {
            return 0.0;
        }
        f32::from(self.digits[dy as usize * self.size.0 + dx as usize]) / 255.0
    }
}

/// One short string's coverage at `scale` texels per unit, lifted out of the font
/// atlas. The font is asked for at the size that makes the atlas's texels the
/// bitmap's; a glyph's `pos` and `uv_rect.offset` are in egui points, so scaled
/// here by the context's pixels per point.
fn digits(ctx: &egui::Context, scale: f32, text: &str) -> Option<(i32, i32, Vec<u8>)> {
    let ppp = ctx.pixels_per_point();
    let galley = ctx.fonts_mut(|f| {
        f.layout_no_wrap(
            text.to_owned(),
            egui::FontId::new(DIGIT_PT * scale / ppp, egui::FontFamily::Proportional),
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
                ((origin.x + g.pos.x + uv.offset.x) * ppp).round() as i32,
                ((origin.y + g.pos.y + uv.offset.y) * ppp).round() as i32,
                i32::from(uv.max[0]) - i32::from(uv.min[0]),
                i32::from(uv.max[1]) - i32::from(uv.min[1]),
                uv.min,
            )
        })
        .filter(|&(_, _, w, h, _)| w > 0 && h > 0)
        .collect();
    // Before the fold: with nothing placed it returns its seeds and the width
    // overflows.
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

#[cfg(test)]
mod tests {
    use super::*;

    /// A context with the theme's fonts installed. `fonts_mut` panics until a
    /// pass has run, so one is run here.
    fn context() -> egui::Context {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        ctx
    }

    /// Every cursor the canvas can ask for, once each.
    fn every_cursor() -> Vec<Bitmap> {
        let badges = [
            None,
            Some(Badge::Plus),
            Some(Badge::Minus),
            Some(Badge::Duplicate),
            Some(Badge::Deep),
            Some(Badge::Dot),
            Some(Badge::Pivot),
            Some(Badge::Swap),
            Some(Badge::No),
            Some(Badge::Scale),
            Some(Badge::Anchor),
            Some(Badge::Segment),
            Some(Badge::Convert),
            Some(Badge::Bend),
            Some(Badge::Close),
            Some(Badge::Continue),
            Some(Badge::Join),
            Some(Badge::Star),
        ];
        let mut all: Vec<Bitmap> = badges.iter().map(|b| Bitmap::Arrow(*b)).collect();
        all.extend(badges.iter().map(|b| Bitmap::Node(*b)));
        all.extend(badges.iter().map(|b| Bitmap::Pen(*b)));
        all.extend([Bitmap::Pencil(None), Bitmap::Pencil(Some(Badge::Continue))]);
        for t in [0, 2, 3, 6, 9] {
            all.extend([
                Bitmap::Resize(Turn(t)),
                Bitmap::Skew(Turn(t)),
                Bitmap::Rotate(Turn(t)),
            ]);
        }
        all.extend([
            Bitmap::Crosshair,
            Bitmap::FromCentre,
            Bitmap::Loaded(Remaining(3)),
            Bitmap::Loaded(Remaining(42)),
            Bitmap::Loaded(Remaining(Remaining::MANY as u8)),
            Bitmap::IBeam,
            Bitmap::TextOnPath,
            Bitmap::Eyedropper,
            Bitmap::Guide(Axis::Vertical),
            Bitmap::Guide(Axis::Horizontal),
            Bitmap::CopyGuide(Axis::Vertical),
            Bitmap::Discard,
            Bitmap::Gap(Axis::Vertical),
            Bitmap::Gap(Axis::Horizontal),
            Bitmap::Pan,
            Bitmap::Panning,
        ]);
        all
    }

    /// The texel at `(x, y)`: its value (0 dark, 255 light) and alpha.
    fn texel(img: &egui::CustomCursorImage, x: usize, y: usize) -> (u8, u8) {
        let k = (y * usize::from(img.size[0]) + x) * 4;
        (img.rgba[k], img.rgba[k + 3])
    }

    /// Where a cursor's ink sits, as an offset from its hotspot in texels, weighted
    /// by alpha so the answer does not jump with the rasterizer's phase.
    fn centroid(img: &egui::CustomCursorImage) -> egui::Vec2 {
        let (w, h) = (usize::from(img.size[0]), usize::from(img.size[1]));
        let (hx, hy) = (f32::from(img.hotspot[0]), f32::from(img.hotspot[1]));
        let (mut sum, mut mass) = (egui::Vec2::ZERO, 0.0);
        for y in 0..h {
            for x in 0..w {
                let a = f32::from(texel(img, x, y).1);
                sum += egui::vec2(x as f32 - hx, y as f32 - hy) * a;
                mass += a;
            }
        }
        sum / mass
    }

    /// Every cursor draws at the common display scales, in both sets, with a core
    /// and a rim in the set's two inks and its hotspot inside the bitmap. A drawing
    /// that parses to nothing, or a rim pass that paints the core, fails here.
    #[test]
    fn every_cursor_draws_its_core_and_rim_in_both_sets() {
        let ctx = context();
        for scale in [1.0, 1.5, 2.0] {
            for light in [false, true] {
                for which in every_cursor() {
                    let img = draw(&ctx, which, scale, light)
                        .unwrap_or_else(|| panic!("{which:?} at {scale}: nothing drawn"));
                    let n = (BOX * which.recipe().unit_pt() * scale).ceil() as u16;
                    assert_eq!(img.size, [n, n], "{which:?} at {scale}");
                    // Over half covered: at 1x the rim is under a texel wide.
                    let inked: Vec<u8> = img
                        .rgba
                        .as_chunks::<4>()
                        .0
                        .iter()
                        .filter(|p| p[3] > 128)
                        .map(|p| p[0])
                        .collect();
                    assert!(
                        inked.iter().any(|&v| v > 200),
                        "{which:?} at {scale}: no light ink"
                    );
                    assert!(
                        inked.iter().any(|&v| v < 40),
                        "{which:?} at {scale}: no dark ink"
                    );
                    assert!(
                        img.hotspot[0] < n && img.hotspot[1] < n,
                        "{which:?}: hotspot"
                    );
                }
            }
        }
    }

    /// The arrow aims from its tip's first texel, which is ink, and the light set
    /// swaps the inks without moving it; the node arrow is the select arrow with
    /// the inks swapped.
    /// flip: `draw`'s `light` arm returning the dark inks → fails at "the light set's core".
    #[test]
    fn the_sets_swap_inks_around_the_same_aim() {
        let ctx = context();
        for scale in [1.0, 2.0] {
            let u = scale * POINTING_UNIT_PT;
            let dark = draw(&ctx, Bitmap::Arrow(None), scale, false).expect("dark");
            let light = draw(&ctx, Bitmap::Arrow(None), scale, true).expect("light");
            assert_eq!(dark.hotspot, light.hotspot);
            // The tip snapped to the nearest texel corner; the texel below and
            // right of it.
            assert_eq!(
                dark.hotspot,
                [(6.0 * u).round() as u16, (4.0 * u).round() as u16],
                "the tip's first texel"
            );
            let (hx, hy) = (usize::from(dark.hotspot[0]), usize::from(dark.hotspot[1]));
            assert!(texel(&dark, hx, hy).1 > 128, "the hotspot is on the ink");
            // Well inside the arrow's body.
            let (bx, by) = ((8.0 * u) as usize, (14.0 * u) as usize);
            assert!(texel(&dark, bx, by).0 < 40, "the dark set's core");
            assert!(texel(&light, bx, by).0 > 230, "the light set's core");
            let node = draw(&ctx, Bitmap::Node(None), scale, false).expect("node");
            assert!(
                texel(&node, bx, by).0 > 230,
                "the node arrow swaps the inks"
            );
        }
    }

    /// At 1x the arrow, outline included, is the size of Windows' own arrow
    /// (12 × 19 texels at 100%), within a texel: the shadow aside, nothing makes
    /// the canvas pointer read larger than the system one beside it.
    /// flip: `POINTING_UNIT_PT` at 1.0 → fails at "the arrow's width" (16).
    #[test]
    fn the_arrow_is_the_size_of_the_systems() {
        let ctx = context();
        let img = draw(&ctx, Bitmap::Arrow(None), 1.0, true).expect("drawn");
        let n = usize::from(img.size[0]);
        let inked: Vec<(usize, usize)> = (0..n)
            .flat_map(|y| (0..n).map(move |x| (x, y)))
            .filter(|&(x, y)| texel(&img, x, y).1 > 128)
            .collect();
        let span = |f: fn(&(usize, usize)) -> usize| {
            let v: Vec<usize> = inked.iter().map(f).collect();
            v.iter().max().unwrap_or(&0) - v.iter().min().unwrap_or(&0) + 1
        };
        let (w, h) = (span(|p| p.0), span(|p| p.1));
        assert!((11..=14).contains(&w), "the arrow's width: {w}");
        assert!((18..=21).contains(&h), "the arrow's height: {h}");
    }

    /// The rim is a mitred outline: it reaches past the arrow's square-cut tail
    /// corner by about the mitre, not just the rim's width.
    /// flip: `join_covers` returning false → fails at "the mitre".
    #[test]
    fn the_rim_is_mitred() {
        let ctx = context();
        let u = 4.0 * POINTING_UNIT_PT;
        let img = draw(&ctx, Bitmap::Arrow(None), 4.0, false).expect("drawn");
        // The tail's right corner is at (19.7, 17.8): a right angle whose mitre
        // reaches `√2` rims along the diagonal, past the rim's own reach.
        let along = RIM_PT / POINTING_UNIT_PT * 1.2;
        let (x, y) = ((19.7 + along) * u, (17.8 + along * 0.2) * u);
        assert!(texel(&img, x as usize, y as usize).1 > 128, "the mitre");
    }

    /// The duplicate cursor's copy sits behind the front arrow, up and to the
    /// right: its ink reaches further right and higher than the front arrow's tip,
    /// the pointer aims from the front tip, and the front arrow's core is on top
    /// where the two overlap.
    /// flip: the copy drawn as the second layer → fails at "the front arrow on top".
    #[test]
    fn the_duplicate_is_a_second_arrow_behind() {
        let ctx = context();
        let plain = draw(&ctx, Bitmap::Arrow(None), 2.0, false).expect("plain");
        let dup = draw(&ctx, Bitmap::Arrow(Some(Badge::Duplicate)), 2.0, false).expect("dup");
        let inked = |img: &egui::CustomCursorImage| {
            let w = usize::from(img.size[0]);
            (0..w)
                .flat_map(|y| (0..w).map(move |x| (x, y)))
                .filter(|&(x, y)| texel(img, x, y).1 > 200)
                .collect::<Vec<_>>()
        };
        let right = |img| inked(img).iter().map(|p| p.0).max().unwrap_or(0);
        let top = |img| inked(img).iter().map(|p| p.1).min().unwrap_or(0);
        let u = 2.0 * POINTING_UNIT_PT;
        assert!(right(&dup) >= right(&plain) + (DUPLICATE_OFFSET.0 * u) as usize - 1);
        let hot = [
            (6.0 * u).round() as u16,
            ((4.0 + DUPLICATE_DROP) * u).round() as u16,
        ];
        assert_eq!(dup.hotspot, hot, "the front arrow's tip");
        assert!(
            usize::from(hot[1]) > top(&dup) + (-DUPLICATE_OFFSET.1 * u) as usize - 2,
            "the copy rises above the front tip"
        );
        // Inside the front arrow, near its right edge, where the copy lies under
        // it: the front arrow's dark core.
        let (x, y) = ((13.0 * u) as usize, ((15.0 + DUPLICATE_DROP) * u) as usize);
        assert!(texel(&dup, x, y).0 < 40, "the front arrow on top");
    }

    /// A turned cursor faces the way it was given: the rotate arc's ink lies
    /// toward its heading, and resize and skew share a bitmap with their
    /// opposite heading.
    #[test]
    fn turned_cursors_face_their_heading() {
        let ctx = context();
        for (step, dir) in [
            (0, egui::vec2(1.0, 0.0)),
            (6, egui::vec2(0.0, 1.0)),
            (18, egui::vec2(0.0, -1.0)),
        ] {
            let img = draw(&ctx, Bitmap::Rotate(Turn(step)), 2.0, false).expect("drawn");
            let c = centroid(&img);
            assert!(c.normalized().dot(dir) > 0.9, "step {step}: centroid {c:?}");
        }
        let mut cursors = Cursors::default();
        for make in [Bitmap::Resize, Bitmap::Skew] {
            let a = cursors.image(&ctx, make(Turn(3))).expect("drawn");
            let b = cursors.image(&ctx, make(Turn(15))).expect("drawn");
            assert_eq!(a.rgba, b.rgba, "{:?}: opposite headings", make(Turn(3)));
        }
    }

    /// A screen direction picks the 15° step facing it, y down, the nearer step
    /// winning on either side of zero.
    /// flip: `Turn::toward` truncating → fails at "13.75°".
    #[test]
    fn a_screen_direction_picks_the_step_facing_it() {
        assert_eq!(Turn::toward(egui::vec2(1.0, 0.0)), Turn(0));
        assert_eq!(Turn::toward(egui::vec2(0.0, 1.0)), Turn(6));
        assert_eq!(Turn::toward(egui::vec2(-1.0, 0.0)), Turn(12));
        assert_eq!(Turn::toward(egui::vec2(0.0, -1.0)), Turn(18));
        assert_eq!(Turn::toward(egui::vec2(1.0, -1.0)), Turn(21));
        assert_eq!(Turn::toward(egui::Vec2::ZERO), Turn(0));
        assert_eq!(Turn::toward(egui::Vec2::angled(0.24)), Turn(1), "13.75°");
        assert_eq!(Turn::toward(egui::Vec2::angled(-0.24)), Turn(23), "−13.75°");
        assert_eq!(Turn::toward(egui::Vec2::angled(0.02)), Turn(0), "1.1°");
        assert_eq!(Turn::toward(egui::Vec2::angled(-0.02)), Turn(0), "−1.1°");
        assert_eq!(Turn(15).axis(), Turn(3));
    }

    /// A cursor is built once and handed back as the same buffer, which egui
    /// dedupes uploads by; a change of scale rebuilds.
    #[test]
    fn the_bitmaps_are_reused_until_the_scale_changes() {
        let ctx = context();
        let mut cursors = Cursors::default();
        let a = cursors.image(&ctx, Bitmap::Pan).expect("drawn");
        let b = cursors.image(&ctx, Bitmap::Pan).expect("drawn");
        assert!(std::sync::Arc::ptr_eq(&a.rgba, &b.rgba));
        cursors.scale = 0.0;
        cursors.built.insert(Bitmap::Pan, None);
        let c = cursors.image(&ctx, Bitmap::Pan).expect("rebuilt");
        assert!(!std::sync::Arc::ptr_eq(&a.rgba, &c.rgba));
    }

    /// The bitmap is drawn at the display's texels per point, except on macOS,
    /// where winit sizes the image in points equal to its pixel count.
    #[test]
    fn the_bitmap_is_sized_for_the_display() {
        let want = if cfg!(target_os = "macos") { 1.0 } else { 2.0 };
        assert_eq!(texels_per_point(2.0), want);
    }

    /// The loaded badge says the count it was given: different counts draw
    /// differently, and the wide `99+` chip keeps its rim inside the box.
    #[test]
    fn the_badge_draws_the_count_it_was_given() {
        let ctx = context();
        let three = draw(&ctx, Bitmap::Loaded(Remaining(3)), 2.0, false).expect("3");
        let four = draw(&ctx, Bitmap::Loaded(Remaining(4)), 2.0, false).expect("4");
        assert_ne!(three.rgba, four.rgba);
        let many = draw(
            &ctx,
            Bitmap::Loaded(Remaining(Remaining::MANY as u8)),
            2.0,
            false,
        )
        .expect("99+");
        let n = usize::from(many.size[0]);
        let edge = (0..n).map(|y| texel(&many, n - 1, y).1).max().unwrap_or(0);
        assert!(edge < 128, "the chip is clipped at the box's edge");
        assert_eq!(Remaining::of(0), None);
        assert_eq!(Remaining(100).label(), "99+");
    }

    /// The count is clamped at 100: every larger drop reads `99+` and shares one
    /// bitmap, which bounds the cache, while 99 still reads exactly.
    /// flip: `Remaining::of` without its clamp → fails at "300 files".
    #[test]
    fn the_count_stops_at_ninety_nine_plus() {
        assert_eq!(
            Remaining::of(300).map(Remaining::label).as_deref(),
            Some("99+"),
            "300 files"
        );
        assert_eq!(Remaining::of(4000), Remaining::of(100), "one bitmap");
        assert_ne!(Remaining::of(99), Remaining::of(100));
        assert_eq!(
            Remaining::of(99).map(Remaining::label).as_deref(),
            Some("99")
        );
    }

    /// A context drawing at `ppp` pixels per point, with the theme's fonts.
    fn scaled(ppp: f32) -> egui::Context {
        let ctx = context();
        ctx.set_pixels_per_point(ppp);
        let _ = ctx.run_ui(Default::default(), |_| {});
        assert_eq!(ctx.pixels_per_point(), ppp, "fixture: the display scale");
        ctx
    }

    /// The pointing drawings aim from their tip at every display scale: the
    /// hotspot texel is ink, and the ink's top-left extreme (its mitred rim) is
    /// within a couple of texels of it. Each also takes the pointing unit.
    /// flip: `Bitmap::recipe` aiming the pen from `Hot::MIDDLE` → fails at the
    /// pen's unit, and without that assertion at its tip (22 texels past at 1x).
    #[test]
    fn the_pointing_cursors_aim_from_their_tip() {
        let ctx = context();
        for scale in [1.0, 1.25, 1.5, 2.0] {
            for which in [
                Bitmap::Pen(None),
                Bitmap::Pen(Some(Badge::Close)),
                Bitmap::Pencil(None),
                Bitmap::Eyedropper,
                Bitmap::Arrow(None),
                Bitmap::Arrow(Some(Badge::Duplicate)),
                Bitmap::Node(None),
            ] {
                assert_eq!(which.recipe().unit_pt(), POINTING_UNIT_PT, "{which:?}");
                let img = draw(&ctx, which, scale, false).expect("drawn");
                let (hx, hy) = (usize::from(img.hotspot[0]), usize::from(img.hotspot[1]));
                assert!(
                    texel(&img, hx, hy).1 > 128,
                    "{which:?} at {scale}: on the ink"
                );
                let n = usize::from(img.size[0]);
                let reach = (0..n)
                    .flat_map(|y| (0..n).map(move |x| (x, y)))
                    .filter(|&(x, y)| texel(&img, x, y).1 > 128)
                    .map(|(x, y)| x + y)
                    .min()
                    .expect("ink");
                let past = (hx + hy).saturating_sub(reach) as f32;
                assert!(
                    reach <= hx + hy && past <= 3.0 * scale + 1.0,
                    "{which:?} at {scale}: the tip is {past} texels past the hotspot"
                );
            }
        }
    }

    /// The drawings aimed from their middle centre their ink on the hotspot,
    /// at every heading and display scale, within a texel.
    #[test]
    fn the_middle_cursors_centre_on_their_hotspot() {
        let ctx = context();
        for scale in [1.0, 1.5, 2.0] {
            for t in [0, 2, 3, 6, 9] {
                for which in [Bitmap::Resize(Turn(t)), Bitmap::Skew(Turn(t))] {
                    let img = draw(&ctx, which, scale, false).expect("drawn");
                    let c = centroid(&img);
                    assert!(c.length() <= 1.0, "{which:?} at {scale}: centroid {c:?}");
                }
            }
        }
    }

    /// Every two cursors draw differently, so no state wears another's drawing,
    /// and `Glyph::ALL` is in declaration order, which `Glyph::shapes` indexes by.
    /// flip: `Bitmap::recipe` drawing `Panning` with the open hand → fails at
    /// "Pan and Panning".
    #[test]
    fn every_cursor_is_its_own_drawing() {
        let ctx = context();
        let mut seen: std::collections::HashMap<Vec<u8>, Bitmap> = Default::default();
        for which in every_cursor() {
            let img = draw(&ctx, which, 1.0, false).expect("drawn");
            if let Some(was) = seen.insert(img.rgba.to_vec(), which) {
                panic!("{was:?} and {which:?} draw alike");
            }
        }
        for (i, g) in Glyph::ALL.iter().enumerate() {
            assert_eq!(*g as usize, i, "{g:?} out of place in `Glyph::ALL`");
        }
    }

    /// The loaded badge's count is laid out in texels at every display scale,
    /// macOS's one texel a point at 2x included: two digits are as wide as two
    /// of one, the count lies inside its chip, and it is the set's ink over the
    /// chip's fill, dark on light in the light set.
    /// flip: `digits` placing glyphs in points, not texels → fails at "88" at 1.5.
    #[test]
    fn the_count_is_laid_out_in_texels_at_every_scale() {
        for (ppp, scale) in [(1.25, 1.25), (1.5, 1.5), (2.0, 2.0), (2.0, 1.0)] {
            let ctx = scaled(ppp);
            let width = |s: &str| digits(&ctx, scale, s).expect("drawn").0;
            assert!(
                width("88") >= 2 * width("8") - 1,
                "\"88\" at {ppp} and {scale}: {} against {}",
                width("88"),
                width("8")
            );
            for count in [1, 8, 88, 100] {
                let which = Bitmap::Loaded(Remaining(count));
                let label = Remaining(count).label();
                let unit = scale * which.recipe().unit_pt();
                let nudge = which.recipe().nudge(unit);
                let chip = Chip::new(&ctx, unit, nudge.0, &label).expect("a chip");
                for light in [false, true] {
                    let img = draw(&ctx, which, scale, light).expect("drawn");
                    let n = usize::from(img.size[0]);
                    let mut inked = 0;
                    for (x, y) in (0..n).flat_map(|y| (0..n).map(move |x| (x, y))) {
                        if chip.digit(x, y) < 0.75 {
                            continue;
                        }
                        let p = [
                            (x as f32 + 0.5 - nudge.0) / unit,
                            (y as f32 + 0.5 - nudge.1) / unit,
                        ];
                        assert!(
                            chip.covers(p, 0.0),
                            "{label} at {ppp}: ({x}, {y}) off the chip"
                        );
                        let (v, a) = texel(&img, x, y);
                        assert!(a > 200, "{label} at {ppp}: opaque");
                        match light {
                            true => assert!(v < 80, "{label} at {ppp}: dark on light, {v}"),
                            false => assert!(v > 175, "{label} at {ppp}: light on dark, {v}"),
                        }
                        inked += 1;
                    }
                    assert!(inked > 0, "{label} at {ppp}: the count is drawn");
                }
            }
        }
    }

    /// The crosshair and the loaded cursor aim at the same texel, which holds no
    /// ink, only the shadow's haze, so the target shows through it, at every
    /// display scale.
    #[test]
    fn the_crosshair_aims_through_a_clear_middle() {
        let ctx = context();
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let cross = draw(&ctx, Bitmap::Crosshair, scale, false).expect("drawn");
            let loaded = draw(&ctx, Bitmap::Loaded(Remaining(7)), scale, false).expect("drawn");
            assert_eq!(
                cross.hotspot, loaded.hotspot,
                "at {scale}: the badge moves the aim"
            );
            for img in [&cross, &loaded] {
                let (x, y) = (usize::from(img.hotspot[0]), usize::from(img.hotspot[1]));
                let a = texel(img, x, y).1;
                assert!(
                    f32::from(a) <= SHADOW_ALPHA * 255.0,
                    "at {scale}: the aim holds ink, {a}"
                );
            }
        }
    }

    /// No heading clips a turned cursor at the bitmap's edge, and the rotate
    /// arc keeps its weight at every step.
    #[test]
    fn no_heading_clips_a_turned_cursor() {
        let ctx = context();
        for scale in [1.0, 1.5, 2.0] {
            let mut masses = Vec::new();
            for t in 0..Turn::STEPS {
                for which in [
                    Bitmap::Rotate(Turn(t)),
                    Bitmap::Resize(Turn(t)),
                    Bitmap::Skew(Turn(t)),
                ] {
                    let img = draw(&ctx, which, scale, false).expect("drawn");
                    let n = usize::from(img.size[0]);
                    let edge = (0..n)
                        .flat_map(|i| [(i, 0), (i, n - 1), (0, i), (n - 1, i)])
                        .map(|(x, y)| texel(&img, x, y).1)
                        .max()
                        .unwrap_or(0);
                    assert!(edge < 64, "{which:?} at {scale}: {edge} on the edge");
                    if let Bitmap::Rotate(_) = which {
                        masses.push(
                            img.rgba
                                .iter()
                                .skip(3)
                                .step_by(4)
                                .map(|&a| f32::from(a))
                                .sum::<f32>(),
                        );
                    }
                }
            }
            let (lo, hi) = masses
                .iter()
                .fold((f32::MAX, 0.0f32), |(l, h), &m| (l.min(m), h.max(m)));
            assert!(
                hi / lo < 1.04,
                "at {scale}: the arc's ink varies {lo}..{hi}"
            );
        }
    }

    /// A label that lays out nothing is refused before the extents' fold, whose
    /// seeds would overflow the width.
    /// flip: `digits` without its empty guard → fails here (an overflow panic).
    #[test]
    fn an_empty_label_draws_no_count() {
        assert!(digits(&context(), 2.0, "").is_none());
    }

    /// Every drawing parses into ink; a relative command, which the parser does
    /// not take, is refused rather than drawn wrong.
    #[test]
    fn every_drawing_parses() {
        for g in Glyph::ALL {
            assert!(
                g.shapes()
                    .iter()
                    .all(|s| s.contours.iter().all(|c| c.pts.len() > 1)),
                "{g:?}"
            );
        }
        let relative = std::panic::catch_unwind(|| parse("M0 0l1 1"));
        assert!(relative.is_err());
    }
}
