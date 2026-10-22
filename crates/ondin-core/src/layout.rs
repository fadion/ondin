//! Layout grids — the columns and rows a designer lines a frame's contents up
//! against (§5.3b, §15 D385).
//!
//! Chrome, not ink: a layout grid is never in the artwork (it does not export and
//! `Operation::SetLayoutGrids` answers `false` to
//! [`crate::op::Operation::changes_ink`]).
//!
//! A grid belongs to a layer and is measured against that layer's box, so the grid
//! list is a field on [`crate::Node`] and [`tracks`] takes the frame's size as an
//! argument. The field is on the node but only a frame is offered one: a grid needs
//! an authored size, and a group's box is derived from its contents. Putting the
//! field inside one kind's payload would force it out again for a second kind.
//!
//! Nothing here is a mode with a payload: [`GridAlign`] chooses which numbers are
//! read, and each is a field beside it (the rule `Node::mask_mode` states), so
//! switching mode never discards a typed value.

use crate::kurbo::{Rect, Size};
use peniko::Color;

/// The colour a grid is drawn in until someone overrides it.
///
/// The guides' dark red at 15% alpha: the hue ties the two permanent overlays
/// together, and the low alpha keeps filled bands readable over artwork.
pub const DEFAULT_GRID_COLOR: Color = Color::from_rgba8(0xa3, 0x33, 0x2d, 0x26);

/// The colours successive grids on one frame take.
///
/// [`DEFAULT_GRID_COLOR`] rotated round the wheel in 60° steps at its saturation,
/// value and alpha, so every grid is a wash of one weight; entry 0 is the constant.
///
/// The order is not wheel order: it visits the thirds (red, green, blue) before
/// the halfway points (olive, cyan, magenta), so two grids, the ordinary case, are
/// 120° apart. Past six, [`next_grid_color`] repeats; a seventh wash would not be
/// distinguishable at 15% alpha.
pub const GRID_COLORS: [Color; 6] = [
    DEFAULT_GRID_COLOR,
    Color::from_rgba8(0x2d, 0xa3, 0x33, 0x26),
    Color::from_rgba8(0x33, 0x2d, 0xa3, 0x26),
    Color::from_rgba8(0x9d, 0xa3, 0x2d, 0x26),
    Color::from_rgba8(0x2d, 0x9d, 0xa3, 0x26),
    Color::from_rgba8(0xa3, 0x2d, 0x9d, 0x26),
];

/// What colour a grid added to `existing` should be given.
///
/// The first colour in [`GRID_COLORS`] nobody is using, not the one at the new
/// grid's index: indexing would hand back a colour still on the frame after a
/// delete. A hand-picked colour blocks nothing. With every hue in use it repeats
/// (`len % 6`) rather than refusing, so two added in a row still differ.
pub fn next_grid_color(existing: &[LayoutGrid]) -> Color {
    match GRID_COLORS
        .iter()
        .find(|c| !existing.iter().any(|g| g.color == **c))
    {
        Some(free) => *free,
        None => GRID_COLORS[existing.len() % GRID_COLORS.len()],
    }
}

/// A new column grid's track count.
pub const DEFAULT_COLUMNS: u32 = 12;

/// A new row grid's track count: rows are bands of content, not a measuring
/// system, so not twelve.
pub const DEFAULT_ROWS: u32 = 5;

/// A new grid's gutter and, under the alignments that read it, its track size.
pub const DEFAULT_GUTTER: f64 = 20.0;
/// See [`DEFAULT_GUTTER`].
pub const DEFAULT_TRACK: f64 = 100.0;

/// Which way a grid's tracks run.
///
/// Named for the tracks rather than the axis they divide: "twelve columns" divides
/// the width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GridAxis {
    /// Vertical bands, dividing the frame's **width**.
    #[default]
    Columns,
    /// Horizontal bands, dividing the frame's **height**.
    Rows,
}

impl GridAxis {
    /// The label a switch or a summary shows.
    pub fn label(self) -> &'static str {
        match self {
            GridAxis::Columns => "Columns",
            GridAxis::Rows => "Rows",
        }
    }

    pub const ALL: [GridAxis; 2] = [GridAxis::Columns, GridAxis::Rows];
}

/// How a grid's tracks are placed in the frame.
///
/// The alignment decides which numbers are read:
///
/// - [`Stretch`](GridAlign::Stretch) derives the track size from the frame, so
///   it reads `margin` (both edges) and ignores `size`.
/// - [`Start`](GridAlign::Start) and [`End`](GridAlign::End) keep a fixed `size`
///   and pin the run to one edge, `margin` in from it.
/// - [`Center`](GridAlign::Center) keeps the size and centres the run; `margin`
///   is not read.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum GridAlign {
    /// Fill the frame between the margins; the track size is derived.
    #[default]
    Stretch,
    /// Fixed tracks, pinned to the left edge (columns) or the top (rows).
    Start,
    /// Fixed tracks, centred. `margin` is not read.
    Center,
    /// Fixed tracks, pinned to the right edge (columns) or the bottom (rows).
    End,
}

impl GridAlign {
    /// The label a dropdown shows, per axis where the two differ.
    pub fn label(self, axis: GridAxis) -> &'static str {
        match (self, axis) {
            (GridAlign::Stretch, _) => "Stretch",
            (GridAlign::Start, GridAxis::Columns) => "Left",
            (GridAlign::Start, GridAxis::Rows) => "Top",
            (GridAlign::Center, _) => "Center",
            (GridAlign::End, GridAxis::Columns) => "Right",
            (GridAlign::End, GridAxis::Rows) => "Bottom",
        }
    }

    /// Whether this alignment reads [`LayoutGrid::size`]. False for
    /// [`Stretch`](GridAlign::Stretch) alone, which derives it.
    pub fn reads_size(self) -> bool {
        self != GridAlign::Stretch
    }

    /// Whether this alignment reads [`LayoutGrid::margin`]. False for
    /// [`Center`](GridAlign::Center), which has no edge to measure from.
    pub fn reads_margin(self) -> bool {
        self != GridAlign::Center
    }

    pub const ALL: [GridAlign; 4] = [
        GridAlign::Stretch,
        GridAlign::Start,
        GridAlign::Center,
        GridAlign::End,
    ];
}

/// Most tracks one grid draws, whatever its stored count says (§15 D488).
///
/// Like `geometry::MAX_SIDES`: past this a grid is a hatch pattern and a way to
/// make the app crawl. The panel clamps to 1000, so this is the document's own
/// bound, set well above it so a file from a build with a wider field is drawn,
/// not truncated. It is not `container::MAX_TEMPLATE_TRACKS`, the CSS grid's cap.
pub const MAX_TRACKS: u32 = 10_000;

/// One set of columns or rows over a frame.
///
/// A frame may carry several, e.g. a column grid and a row grid together.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LayoutGrid {
    pub axis: GridAxis,
    /// How many tracks. Zero draws nothing rather than being refused, and
    /// [`tracks`] clamps at [`MAX_TRACKS`] (`NodeKind::Polygon::sides`' rule: the
    /// builder clamps, the model does not).
    pub count: u32,
    /// The gap between one track and the next.
    pub gutter: f64,
    /// The gap between the frame's edge and the first track — both edges under
    /// [`GridAlign::Stretch`], one edge under `Start`/`End`, and not read at all
    /// under `Center`.
    pub margin: f64,
    /// A track's width (columns) or height (rows), read only when
    /// [`GridAlign::reads_size`] says so. Kept while `Stretch` derives it
    /// instead, so the switch is not destructive.
    pub size: f64,
    pub align: GridAlign,
    pub color: Color,
    /// Whether this grid draws; a flag so hiding keeps the settings.
    pub visible: bool,
}

impl LayoutGrid {
    /// A new grid on `axis`, with the defaults above.
    pub fn new(axis: GridAxis) -> Self {
        LayoutGrid {
            axis,
            count: match axis {
                GridAxis::Columns => DEFAULT_COLUMNS,
                GridAxis::Rows => DEFAULT_ROWS,
            },
            gutter: DEFAULT_GUTTER,
            margin: 0.0,
            size: DEFAULT_TRACK,
            align: GridAlign::Stretch,
            color: DEFAULT_GRID_COLOR,
            visible: true,
        }
    }

    /// What the panel's row says this grid is: `"12 columns"`, `"5 rows"`.
    pub fn summary(&self) -> String {
        let noun = match (self.axis, self.count) {
            (GridAxis::Columns, 1) => "column",
            (GridAxis::Columns, _) => "columns",
            (GridAxis::Rows, 1) => "row",
            (GridAxis::Rows, _) => "rows",
        };
        format!("{} {noun}", self.count)
    }
}

/// The bands a grid draws over a frame of `size`, in the frame's local space
/// (`(0, 0)` is its top-left), so the caller needs only the transform and a clip.
///
/// A free function so the arithmetic is checkable without pixels. A zero count, a
/// non-finite number, or margins and gutters that leave no room give an empty
/// result rather than a clamped hairline: such numbers do not fit the frame.
pub fn tracks(grid: &LayoutGrid, size: Size) -> Vec<Rect> {
    let (extent, across) = match grid.axis {
        GridAxis::Columns => (size.width, size.height),
        GridAxis::Rows => (size.height, size.width),
    };
    if grid.count == 0
        || !extent.is_finite()
        || !across.is_finite()
        || extent <= 0.0
        || across <= 0.0
        || !grid.gutter.is_finite()
        || !grid.margin.is_finite()
        || !grid.size.is_finite()
    {
        return Vec::new();
    }
    // The fit guards above cannot bound the count: under `Stretch` a track is
    // `extent / n`, finite for any `n`. Nothing in the model or loader limits
    // `count` (a hand-edited file round-trips `u32::MAX`), and the allocation
    // failure aborts, so clamp here; the field must not become an error.
    let count = grid.count.min(MAX_TRACKS);
    let n = f64::from(count);
    let gutters = grid.gutter * (n - 1.0);
    // The track size, and where the run starts along the axis.
    let (track, start) = match grid.align {
        GridAlign::Stretch => {
            let available = extent - 2.0 * grid.margin - gutters;
            (available / n, grid.margin)
        }
        GridAlign::Start => (grid.size, grid.margin),
        GridAlign::Center => (grid.size, (extent - (grid.size * n + gutters)) / 2.0),
        GridAlign::End => (grid.size, extent - grid.margin - (grid.size * n + gutters)),
    };
    if !track.is_finite() || track <= 0.0 || !start.is_finite() {
        return Vec::new();
    }
    (0..count)
        .map(|i| {
            let at = start + f64::from(i) * (track + grid.gutter);
            match grid.axis {
                GridAxis::Columns => Rect::new(at, 0.0, at + track, across),
                GridAxis::Rows => Rect::new(0.0, at, across, at + track),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn columns(count: u32) -> LayoutGrid {
        LayoutGrid {
            count,
            gutter: 0.0,
            margin: 0.0,
            ..LayoutGrid::new(GridAxis::Columns)
        }
    }

    /// Stretch divides what is left after the margins and the gutters, and the
    /// tracks meet the frame's edges. `t[0].x0` has no teeth.
    ///
    /// flip: subtract one margin instead of two in `tracks` → fails at the width
    /// (180 against 170).
    #[test]
    fn stretch_fills_the_frame_between_its_margins() {
        let g = columns(4);
        let t = tracks(&g, Size::new(400.0, 200.0));
        assert_eq!(t.len(), 4);
        assert_eq!(t[0], Rect::new(0.0, 0.0, 100.0, 200.0));
        assert_eq!(t[3], Rect::new(300.0, 0.0, 400.0, 200.0));

        // 400 less two 20pt margins and the one gutter, over two tracks of 170.
        let g = LayoutGrid {
            count: 2,
            gutter: 20.0,
            margin: 20.0,
            ..columns(2)
        };
        let t = tracks(&g, Size::new(400.0, 200.0));
        assert_eq!(t[0].x0, 20.0);
        assert_eq!(t[0].width(), 170.0);
        assert_eq!(
            t[1].x1, 380.0,
            "the run ends one margin in from the far edge"
        );
    }

    /// Rows divide the height and span the width: the axis swap is two
    /// substitutions, and swapping only the first draws rows a height wide.
    #[test]
    fn rows_divide_the_height_and_span_the_width() {
        let g = LayoutGrid {
            count: 2,
            gutter: 0.0,
            margin: 0.0,
            ..LayoutGrid::new(GridAxis::Rows)
        };
        let t = tracks(&g, Size::new(400.0, 200.0));
        assert_eq!(t.len(), 2);
        assert_eq!(t[0], Rect::new(0.0, 0.0, 400.0, 100.0));
        assert_eq!(t[1], Rect::new(0.0, 100.0, 400.0, 200.0));
    }

    /// Each fixed-size alignment is pinned to the edge it names.
    #[test]
    fn the_fixed_alignments_pin_the_run_to_the_edge_they_name() {
        let base = LayoutGrid {
            count: 2,
            gutter: 10.0,
            margin: 25.0,
            size: 50.0,
            ..columns(2)
        };
        // Two 50s and one 10 gutter: a 110-wide run in a 400-wide frame.
        let start = tracks(
            &LayoutGrid {
                align: GridAlign::Start,
                ..base
            },
            Size::new(400.0, 200.0),
        );
        assert_eq!((start[0].x0, start[1].x1), (25.0, 135.0));

        let end = tracks(
            &LayoutGrid {
                align: GridAlign::End,
                ..base
            },
            Size::new(400.0, 200.0),
        );
        assert_eq!(
            (end[0].x0, end[1].x1),
            (265.0, 375.0),
            "pinned one margin in from the right edge"
        );

        let centre = tracks(
            &LayoutGrid {
                align: GridAlign::Center,
                ..base
            },
            Size::new(400.0, 200.0),
        );
        assert_eq!(
            (centre[0].x0, centre[1].x1),
            (145.0, 255.0),
            "centred, and the margin is not read at all"
        );
        // A different margin gives the same picture (`reads_margin`).
        let moved = tracks(
            &LayoutGrid {
                align: GridAlign::Center,
                margin: 100.0,
                ..base
            },
            Size::new(400.0, 200.0),
        );
        assert_eq!(centre, moved);
    }

    /// Numbers that do not fit draw nothing: zero count, margins wider than the
    /// frame, a zero-sized frame and a NaN each reach a different guard.
    #[test]
    fn a_grid_that_does_not_fit_draws_nothing() {
        let frame = Size::new(400.0, 200.0);
        assert!(tracks(&columns(0), frame).is_empty(), "no tracks");
        assert!(
            tracks(
                &LayoutGrid {
                    margin: 250.0,
                    ..columns(3)
                },
                frame
            )
            .is_empty(),
            "margins wider than the frame"
        );
        assert!(
            tracks(&columns(3), Size::new(0.0, 200.0)).is_empty(),
            "a frame with no width"
        );
        assert!(
            tracks(
                &LayoutGrid {
                    gutter: f64::NAN,
                    ..columns(3)
                },
                frame
            )
            .is_empty(),
            "a field that has been typed into and not yet parsed"
        );
    }

    /// A grid with too many tracks draws `MAX_TRACKS` of them, since the fit guards
    /// cannot see a count (`u32::MAX` is 137 GB of rects); `1000`, the panel's
    /// maximum, is the control.
    ///
    /// flip: raise `MAX_TRACKS` to `2_000_000` → fails at 2000000 against 10000
    /// (removing the `.min` outright aborts the runner).
    #[test]
    fn a_grid_with_more_tracks_than_anyone_can_see_is_clamped() {
        let frame = Size::new(400.0, 200.0);
        assert_eq!(
            tracks(&columns(u32::MAX), frame).len(),
            MAX_TRACKS as usize,
            "the fit guards cannot see a count, so this is the one that has to"
        );
        assert_eq!(
            tracks(&columns(1000), frame).len(),
            1000,
            "and the panel's own maximum is nowhere near it"
        );
    }

    /// The summary is what the panel's row reads.
    #[test]
    fn the_summary_counts_and_names_the_tracks() {
        assert_eq!(columns(12).summary(), "12 columns");
        assert_eq!(columns(1).summary(), "1 column");
        assert_eq!(
            LayoutGrid::new(GridAxis::Rows).summary(),
            format!("{DEFAULT_ROWS} rows")
        );
    }

    /// Hue in degrees, saturation and value of a colour; local because core has no
    /// colour-model dependency.
    fn hsv(c: Color) -> (f64, f64, f64) {
        let [r, g, b, _] = c.to_rgba8().to_u8_array().map(|v| f64::from(v) / 255.0);
        let (max, min) = (r.max(g).max(b), r.min(g).min(b));
        let d = max - min;
        let h = if d == 0.0 {
            0.0
        } else if max == r {
            60.0 * (((g - b) / d) % 6.0)
        } else if max == g {
            60.0 * ((b - r) / d + 2.0)
        } else {
            60.0 * ((r - g) / d + 4.0)
        };
        (
            (h + 360.0) % 360.0,
            if max == 0.0 { 0.0 } else { d / max },
            max,
        )
    }

    /// `GRID_COLORS` is one wash at six angles, in thirds-first order. The
    /// saturation and value assertion is the one with teeth: a hand-picked palette
    /// would pass the distinctness check.
    ///
    /// flip: exchange entries 1 and 3 and entries 2 and 4 → fails with
    /// `two grids are 60° apart`.
    #[test]
    fn the_grid_ramp_is_one_wash_at_six_angles_and_starts_with_the_thirds() {
        let (_, s0, v0) = hsv(DEFAULT_GRID_COLOR);
        assert_eq!(
            GRID_COLORS[0], DEFAULT_GRID_COLOR,
            "the ramp starts elsewhere"
        );
        for (i, c) in GRID_COLORS.iter().enumerate() {
            let (_, s, v) = hsv(*c);
            assert!(
                (s - s0).abs() < 0.02 && (v - v0).abs() < 0.02,
                "entry {i} is a different wash: saturation {s:.3} against {s0:.3}, \
                 value {v:.3} against {v0:.3}"
            );
            assert_eq!(
                c.to_rgba8().to_u8_array()[3],
                DEFAULT_GRID_COLOR.to_rgba8().to_u8_array()[3],
                "entry {i} is a different strength"
            );
        }
        // Separation measured the short way round.
        let apart = |a: usize, b: usize| {
            let d = (hsv(GRID_COLORS[a]).0 - hsv(GRID_COLORS[b]).0).abs();
            d.min(360.0 - d)
        };
        assert!(
            (apart(0, 1) - 120.0).abs() < 1.0,
            "two grids are {:.0}° apart",
            apart(0, 1)
        );
        assert!(
            (apart(1, 2) - 120.0).abs() < 1.0 && (apart(0, 2) - 120.0).abs() < 1.0,
            "three grids are not evenly spread"
        );
        // Every entry is its own hue, at least 60° from every other.
        for i in 0..GRID_COLORS.len() {
            for j in (i + 1)..GRID_COLORS.len() {
                assert!(
                    apart(i, j) > 59.0,
                    "entries {i} and {j} are {:.0}° apart",
                    apart(i, j)
                );
            }
        }
    }

    /// A new grid takes the first colour nobody is using. Two cases catch an index
    /// by list length: delete-then-add and a hand-picked colour.
    #[test]
    fn a_new_grid_takes_the_first_colour_the_frame_is_not_using() {
        let with = |colors: &[Color]| {
            colors
                .iter()
                .map(|c| LayoutGrid {
                    color: *c,
                    ..LayoutGrid::new(GridAxis::Columns)
                })
                .collect::<Vec<_>>()
        };
        assert_eq!(next_grid_color(&[]), DEFAULT_GRID_COLOR);
        assert_eq!(next_grid_color(&with(&GRID_COLORS[..1])), GRID_COLORS[1]);
        assert_eq!(next_grid_color(&with(&GRID_COLORS[..2])), GRID_COLORS[2]);

        // The first of two removed, then one added: an index would repeat entry 1.
        let left = with(&[GRID_COLORS[1]]);
        assert_eq!(
            next_grid_color(&left),
            GRID_COLORS[0],
            "the new grid is the colour of the one already there"
        );

        // A hand-picked colour blocks nothing.
        let odd = with(&[Color::from_rgba8(0x11, 0x22, 0x33, 0x26)]);
        assert_eq!(next_grid_color(&odd), GRID_COLORS[0]);

        // Past the end it repeats, stepping by the length.
        let all = with(&GRID_COLORS);
        assert_eq!(next_grid_color(&all), GRID_COLORS[0]);
        let mut seven = all.clone();
        seven.push(LayoutGrid::new(GridAxis::Rows));
        assert_eq!(next_grid_color(&seven), GRID_COLORS[1]);
    }

    /// Which numbers an alignment reads, stated where the panel can ask.
    #[test]
    fn stretch_derives_the_size_and_centre_ignores_the_margin() {
        assert!(!GridAlign::Stretch.reads_size());
        assert!(GridAlign::Stretch.reads_margin());
        assert!(GridAlign::Center.reads_size());
        assert!(!GridAlign::Center.reads_margin());
        for a in [GridAlign::Start, GridAlign::End] {
            assert!(a.reads_size() && a.reads_margin());
        }
        // Only the two edge-naming alignments change label with the axis.
        assert_eq!(GridAlign::Start.label(GridAxis::Columns), "Left");
        assert_eq!(GridAlign::Start.label(GridAxis::Rows), "Top");
        assert_eq!(GridAlign::Center.label(GridAxis::Rows), "Center");
    }
}
