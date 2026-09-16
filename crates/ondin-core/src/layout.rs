//! Layout grids — the columns and rows a designer lines a frame's contents up
//! against (§5.3b, §15 D385).
//!
//! **Chrome, not ink.** A layout grid is drawn over a frame and is never in the
//! artwork: it does not export, it is not in the scene, and
//! `Operation::SetLayoutGrids` answers `false` to
//! [`crate::op::Operation::changes_ink`] beside the guide operations, for the
//! reason that list already gives — *"a guide is a line over the artwork rather
//! than part of it"*. That is the whole of what it has in common with a guide;
//! everything else is different, and the differences are what shape this module.
//!
//! **A grid belongs to a layer, and it is measured against that layer's box.**
//! `Guide` is document state with a position in world space, because a guide is
//! placed on the page. A grid is placed on a *frame* — twelve columns across
//! whatever the frame is today, so that resizing the frame moves them — which is
//! why the grid list is a field on [`crate::Node`] and the tracks below take the
//! frame's size as an argument rather than storing anything.
//!
//! ⚠️ **The field is on the node and the *offer* is on the frame**, which reads
//! like a disagreement and is not. `Node::grids` sits beside `exports` and
//! `effects` — empty on every layer until somebody adds one, skipped by the save
//! format, ignored by every walk — and it is inert anywhere the app does not
//! offer it, exactly as `clip` is inert on a kind with no frame to clip to. What
//! makes the offer a frame's alone is that a grid is defined against an
//! *authored* size: `NodeKind::Artboard` has one, and a group's box is derived
//! from its contents, so a grid on a group would move whenever a child did. That
//! is a reason to gate the panel, not a reason to bury the field inside one
//! kind's payload — where, on the evidence of `clip`'s own note, it would have to
//! move out again the first time a second kind wants it.
//!
//! **Nothing here is a mode with a payload.** [`GridAlign`] chooses which of the
//! numbers below it are read, and every one of them is a field beside the mode
//! rather than inside it — the rule `Node::mask_mode` states: a value with
//! nowhere to live while its mode is unselected makes a mode switch quietly
//! destructive, and a designer who sets a track size, tries *Stretch* and goes
//! back should find the size they typed.

use crate::kurbo::{Rect, Size};
use peniko::Color;

/// The colour a grid is drawn in until someone overrides it.
///
/// The guides' dark red at 15% alpha, and both halves are deliberate. **The
/// hue** is the family resemblance: guides and grids are the two things this app
/// draws over artwork permanently, and giving them one colour says they are one
/// kind of thing. **The alpha** is what tells them apart in use — a guide is a
/// hairline and can be opaque, while a grid is a set of filled bands covering
/// most of a frame, and the same red at full strength would be a wash the
/// artwork could not be read through.
pub const DEFAULT_GRID_COLOR: Color = Color::from_rgba8(0xa3, 0x33, 0x2d, 0x26);

/// The colours successive grids on one frame take, and the reason a second grid
/// is worth adding at all.
///
/// **Six hues of [`DEFAULT_GRID_COLOR`], at its saturation, its value and its
/// alpha.** Nothing here is a new colour: each entry is that red rotated round
/// the wheel by a multiple of 60°, so a frame carrying four grids carries four
/// washes of one weight rather than a red one and three louder ones. Entry 0 *is*
/// the constant above, bit for bit — the ordinary single-grid frame is unchanged,
/// and the family resemblance with the guides that constant's own note is about
/// survives everywhere it was ever visible.
///
/// ⚠️ **The order is not the order of the wheel, and that is the whole design.**
/// Laid out by hue these would be 3°, 63°, 123°… and the *second* grid — a column
/// grid beside a row grid, which is the ordinary case for having two — would be
/// an olive 60° from the first. So the ramp visits the thirds first (red, green,
/// blue) and only then the halfway points between them (olive, cyan, magenta):
/// two grids are 120° apart, three are 120° apart, and the step only narrows once
/// there are more grids than anybody puts on a page. The count that matters is
/// two, and it is the one a wheel-ordered ramp serves worst.
///
/// **Six, because that is where 60° runs out**, and running out is handled by
/// [`next_grid_color`] rather than by making the steps finer: a seventh wash
/// distinguishable from six others is not a thing a 15%-alpha band can be.
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
/// **The first colour in [`GRID_COLORS`] nobody is using, not the one at the new
/// grid's index.** Indexing by position is the obvious spelling and it is wrong
/// in a way a designer meets immediately: add two grids, delete the first, add
/// another, and the two on the frame are both entry 1. Asking what is *in use*
/// answers that, and it also does the right thing for a file that arrived with
/// hand-picked colours — a grid whose colour is nobody's entry blocks nothing,
/// so the next one still starts at the top of the ramp.
///
/// ⚠️ **Past six it repeats rather than refusing.** A frame with every hue in use
/// has more grids than the ramp can tell apart, and the honest answer is a
/// duplicate colour on a control the user can change — not an error, and not a
/// seventh colour that only looks distinct in the panel. It steps by the list's
/// length so that two grids added in a row past the sixth still differ.
///
/// A free function over the list rather than a method on anything, for the reason
/// [`tracks`] is one: it is the whole of a rule, and a rule that can be asked
/// without a panel is a rule that can be checked without one.
pub fn next_grid_color(existing: &[LayoutGrid]) -> Color {
    match GRID_COLORS
        .iter()
        .find(|c| !existing.iter().any(|g| g.color == **c))
    {
        Some(free) => *free,
        None => GRID_COLORS[existing.len() % GRID_COLORS.len()],
    }
}

/// A new column grid's track count — the twelve every layout framework of the
/// last fifteen years settled on, so a designer who adds one and types nothing
/// gets the grid they meant.
pub const DEFAULT_COLUMNS: u32 = 12;

/// A new row grid's track count. **Not twelve**: rows are read as bands of
/// content rather than as a measuring system, and nobody divides a page into
/// twelve of them. Five is Figma's default for both axes and is right for this
/// one.
pub const DEFAULT_ROWS: u32 = 5;

/// A new grid's gutter and, under the alignments that read it, its track size.
pub const DEFAULT_GUTTER: f64 = 20.0;
/// See [`DEFAULT_GUTTER`].
pub const DEFAULT_TRACK: f64 = 100.0;

/// Which way a grid's tracks run.
///
/// Named for what the *tracks* are rather than for the axis they divide, because
/// that is what a designer says: "twelve columns" divides the width.
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
/// **The alignment decides which numbers are read**, and that is the whole of
/// the difference between the four:
///
/// - [`Stretch`](GridAlign::Stretch) derives the track size from the frame, so
///   it reads `margin` (both edges) and ignores `size`. Resizing the frame
///   resizes every track — the behaviour a responsive layout is drawn against,
///   and the reason this is the default.
/// - [`Start`](GridAlign::Start) and [`End`](GridAlign::End) keep the tracks a
///   fixed `size` and pin the run to one edge, `margin` in from it. The frame
///   grows away from them.
/// - [`Center`](GridAlign::Center) keeps the size and centres the run, so
///   `margin` means nothing and is not read.
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
    /// The label a dropdown shows. Named per axis where the two differ, because
    /// "Left" over a set of rows is nonsense.
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
/// **`geometry::MAX_SIDES`' precedent, and its reason nearly verbatim**: past this
/// a grid is a hatch pattern, and the field is a way to make the app crawl. The
/// panel already clamps its own input to 1000, so this is not a limit any
/// keyboard reaches — it is the one the *document* was missing, and the number is
/// deliberately well above the panel's so that a file written by some future
/// build with a wider field is drawn rather than truncated.
///
/// ⚠️ **A cap on the count is not the same guard as `tracks`' finiteness checks**,
/// which is the whole of `[S4.2-L1-02]`: those ask whether the arithmetic fits the
/// frame, in units of the frame's extent, and a count is a number of allocations.
/// Under `Stretch` a track is `extent / n`, which is positive and finite for every
/// `n` there is.
pub const MAX_TRACKS: u32 = 10_000;

/// One set of columns or rows over a frame.
///
/// A frame may carry several — a column grid and a row grid together is the
/// ordinary case, and two column grids at different counts is how a designer
/// checks one against another.
#[derive(Clone, Copy, Debug, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct LayoutGrid {
    pub axis: GridAxis,
    /// How many tracks. Zero draws nothing rather than being refused: a field
    /// being dragged through zero on its way somewhere else must not be an
    /// error, which is `NodeKind::Polygon::sides`' rule (the outline builder
    /// clamps, the model does not).
    ///
    /// ⚠️ **`sides`' rule has an upper half too, and this had only copied the
    /// lower one** (§15 D488). [`tracks`] clamps at [`MAX_TRACKS`], as
    /// `geometry::star_path` clamps at `MAX_SIDES` and for the same stated reason:
    /// past a point the field stops being a design control and becomes a way to
    /// make the app crawl. Unbounded, a hand-edited `count` of `u32::MAX`
    /// round-tripped through save and load and asked the paint path for **137 GB**.
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
    /// Whether this grid draws. **A flag rather than removing the grid**, so
    /// "let me see it without" costs one click and no retyping — the same
    /// bargain a fill's eye makes.
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

/// The bands a grid draws over a frame of `size`, in the frame's own local
/// space.
///
/// **The one piece of arithmetic in this feature, and it is a free function**
/// for the reason this project keeps lifting decisions out of `&mut self`
/// methods: a rule that lives in a drawing routine can only be checked by
/// looking at pixels, and this one has four alignments, two axes and a handful
/// of degenerate inputs to get right.
///
/// Returns rects in local coordinates — `(0, 0)` is the frame's top-left, which
/// is where a frame's own box starts — so the caller's only job is the frame's
/// transform and a clip.
///
/// ⚠️ **Empty rather than clamped when the arithmetic runs out.** A count of
/// zero, a non-finite number, or margins and gutters that leave no room draw
/// *nothing*, and that is the honest picture: a track of zero or negative width
/// is not a thin track, it is a grid whose numbers do not fit the frame it is
/// on. Clamping to a hairline would draw something that lines nothing up.
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
    // ⚠️ **The guards above ask whether the numbers *fit*, and never how many
    // there are** (§15 D488, `[S4.2-L1-02]`). Every clause is denominated in units
    // of the frame's extent; what grows here is a **count of allocations**. Under
    // `Stretch` the track width is `extent / n`, positive and finite for any `n`;
    // under `Start`/`End`/`Center` it is the stored `size` and does not depend on
    // `n` at all. So the fit guard cannot fire however large the count is.
    //
    // `count` is a `u32` bounded by nothing in the model, the save format or the
    // loader — `op_set_grids` is a `mem::replace` and `schema.rs` is `grids:
    // dto.grids` — and only the *panel* clamps it, to 1000, which is a guard on the
    // keyboard rather than on the document. Measured from a hand-edited `.ondin`
    // that round-trips cleanly: 8,000,000 tracks allocate and free 256 MB **every
    // frame** at 35 ms, a permanent 3 fps, and `u32::MAX` asks for **137 GB**,
    // where the allocation failure calls `handle_alloc_error` and **aborts** — not
    // a panic, and nothing can catch it. The file only has to be open:
    // `canvas::draw_layout_grids` runs over every visible frame with grids,
    // unconditionally, from the canvas paint.
    //
    // The clamp rather than a refusal is this field's own bargain, argued at
    // `LayoutGrid::count`: a field dragged through a silly number must not be an
    // error. That is `NodeKind::Polygon::sides`' rule, and this had copied its
    // lower half — zero draws nothing — while leaving the upper half behind.
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
    /// tracks meet the frame's edges.
    ///
    /// ⚠️ **Every wrong version of this arithmetic still puts the first track at
    /// the margin**, because the ways to get it wrong are all about what is
    /// subtracted *before* the division. So `t[0].x0` is the assertion with no
    /// teeth, and it is kept only to say the run starts where it should.
    ///
    /// ⚠️ **Flip-check, run, and the predicted site was wrong.** Subtracting one
    /// margin instead of two (`extent - grid.margin - gutters`) was predicted to
    /// fail at the *last edge*, on the argument that a too-wide track shows up as
    /// a run overshooting the far margin. It fails one line earlier, at the
    /// **width** — 180 against 170 — because the width is asserted first and a
    /// division that got more room to share is wrong at every track, not only at
    /// the end of the run. Both assertions bite; the correction is about which
    /// one a reader should treat as load-bearing.
    #[test]
    fn stretch_fills_the_frame_between_its_margins() {
        let g = columns(4);
        let t = tracks(&g, Size::new(400.0, 200.0));
        assert_eq!(t.len(), 4);
        assert_eq!(t[0], Rect::new(0.0, 0.0, 100.0, 200.0));
        assert_eq!(t[3], Rect::new(300.0, 0.0, 400.0, 200.0));

        // With margins and gutters: 400 less two 20pt margins and the **one**
        // gutter two tracks have between them, over two tracks of 170.
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

    /// Rows are the same arithmetic on the other axis, and the tracks span the
    /// width.
    ///
    /// ⚠️ **Asserted because the axis swap is two substitutions, not one** — the
    /// extent being divided *and* the extent being spanned — and a version that
    /// swaps only the first draws rows that are the frame's *height* wide.
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

    /// The three fixed-size alignments differ only in where the run starts, and
    /// each is checked against the edge it is pinned to.
    #[test]
    fn the_fixed_alignments_pin_the_run_to_the_edge_they_name() {
        let base = LayoutGrid {
            count: 2,
            gutter: 10.0,
            margin: 25.0,
            size: 50.0,
            ..columns(2)
        };
        // Two 50s and one 10 gutter is a 110-wide run inside a 400-wide frame.
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
        // The same grid with a different margin is the same picture, which is
        // what `reads_margin` says and the only way to prove it here.
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

    /// Numbers that do not fit draw nothing at all.
    ///
    /// ⚠️ **Four separate ways in, and they are not one case.** A zero count is a
    /// field being dragged through zero; margins wider than the frame is a grid
    /// on a frame that has since been shrunk; a zero-sized frame is a node mid
    /// creation-drag; a NaN is a field parsed from text. Each reaches a different
    /// guard, and a version guarding only the count — which is the one everybody
    /// writes — returns tracks of negative width for the second.
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

    /// **A grid that does not *fit* draws nothing; a grid with too many tracks
    /// draws `MAX_TRACKS` of them** (§15 D488, `[S4.2-L1-02]`) — two different
    /// guards, and the one above is not the one this needs.
    ///
    /// Every clause the test above exercises is denominated in units of the
    /// frame's extent, and what grows here is a **count of allocations**. Under
    /// `Stretch` a track is `extent / n`, positive and finite for every `n`, so the
    /// fit guard is structurally unable to fire however large the count is:
    /// `u32::MAX` on a 400×200 frame gives a track of `9.31e-8`, which is greater
    /// than zero and finite, and 4,294,967,295 rects is **137 GB**. The allocation
    /// fails, `handle_alloc_error` **aborts**, and nothing can catch it. The file
    /// only has to be open — `canvas::draw_layout_grids` runs over every visible
    /// frame with grids from the canvas paint.
    ///
    /// ⚠️ **Nothing in the model or the loader bounded it.** `op_set_grids` is a
    /// `mem::replace`, `schema.rs` is `grids: dto.grids`, and the *panel* clamps to
    /// 1000 — a guard on the keyboard, not on the document. A hand-edited `.ondin`
    /// carrying `u32::MAX` round-trips through save and load unchanged.
    ///
    /// **Asserted as the length rather than as an absence**, because clamping is
    /// the answer this field's own doc argues for: a number dragged past sense is
    /// not an error, it is a grid drawn as far as is useful. The `1000` case is the
    /// control — the panel's own maximum has to come through untouched, or the cap
    /// has been put somewhere a user can feel it.
    ///
    /// ⚠️ **Flip-check, run — and deliberately a *bounded* one.** Removing the
    /// `.min` outright does not fail this test, it takes the runner down with the
    /// 137 GB allocation, which is the finding rather than a check on it. The cap
    /// was raised to `2_000_000` instead: fails at 2000000 against 10000, the
    /// predicted site, proving the assertion reads the clamp rather than some
    /// incidental ceiling. **A flip whose honest form is destructive still gets
    /// run — at a size the machine survives.**
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

    /// The summary is what the panel's row reads, and it is the one string here
    /// a user sees.
    #[test]
    fn the_summary_counts_and_names_the_tracks() {
        assert_eq!(columns(12).summary(), "12 columns");
        assert_eq!(columns(1).summary(), "1 column");
        assert_eq!(
            LayoutGrid::new(GridAxis::Rows).summary(),
            format!("{DEFAULT_ROWS} rows")
        );
    }

    /// The hue of a colour, in degrees, and its saturation and value — enough to
    /// say that six colours are one colour turned round the wheel.
    ///
    /// Written here rather than reached for, because core has no colour model
    /// beyond `peniko::Color`'s components and a conversion imported for one test
    /// would be a dependency the library does not otherwise have.
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

    /// **`GRID_COLORS` is one colour six times, and its order is not the
    /// wheel's** — the two claims the constant's doc makes, and neither is
    /// readable off the six hex literals.
    ///
    /// ⚠️ **The saturation and value assertion is the one with teeth.** Six
    /// distinct hues is what any six colours somebody liked would give; what makes
    /// this a *ramp* is that they are the same wash at six angles, so a frame with
    /// four grids has four bands of one weight. A hand-picked palette would pass a
    /// distinctness check and fail this.
    ///
    /// ⚠️ **Flip-check, with the mutation written out because "sort it into wheel
    /// order" is not one edit.** Wheel order is the permutation `0,3,1,4,2,5` — a
    /// pair of 3-cycles, not the pair of swaps it looks like. What was run is the
    /// blunter version, entries 1 and 3 exchanged and 2 and 4 exchanged, which is
    /// *toward* wheel order rather than it; both put olive at index 1, which is all
    /// the assertion below needs. It fails with `two grids are 60° apart` — the
    /// message the format string actually prints, and worth quoting exactly, since
    /// the earlier note here quoted a sentence that never appears. Nothing else in
    /// the workspace notices, which is the point of asserting it here: the cost of
    /// that tidy-up lands on the second grid a designer adds and nowhere a compiler
    /// looks.
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
        // The separation the order exists for, measured the short way round.
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
        // And every entry is its own hue, 60° apart from its nearest neighbour —
        // which is what says the six *are* the whole wheel rather than three
        // colours and three near-repeats.
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

    /// **A new grid takes the first colour nobody is using**, which is the rule
    /// that survives a removal — and the reason it is not an index.
    ///
    /// ⚠️ **Two of the seven cases here catch an index by list length, and this
    /// comment claimed one did.** Walking them under that mutation: empty ✓, one ✓,
    /// two ✓, **delete-then-add ✗**, **hand-picked colour ✗**, all-six ✓, seven ✓.
    /// The first failure is the flow the feature exists for — remove one, add
    /// another, and an index hands back the entry still on the frame. The second
    /// catches it from the other side and is the one protecting a file somebody
    /// else made: one grid at a colour that is nobody's entry has length 1, so an
    /// index would skip to entry 1 where the ramp should still start at the top.
    /// Corrected after `arch-scribe` walked the assertions against the claim —
    /// a sentence saying which case is load-bearing has to be counted, not
    /// remembered.
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

        // Two grids, the *first* removed, then a third added: an index would hand
        // back entry 1, which is exactly what is still on the frame.
        let left = with(&[GRID_COLORS[1]]);
        assert_eq!(
            next_grid_color(&left),
            GRID_COLORS[0],
            "the new grid is the colour of the one already there"
        );

        // A file that arrived with a hand-picked colour blocks nothing: it is
        // nobody's entry, so the ramp still starts at the top.
        let odd = with(&[Color::from_rgba8(0x11, 0x22, 0x33, 0x26)]);
        assert_eq!(next_grid_color(&odd), GRID_COLORS[0]);

        // And past the end it repeats rather than refusing — stepping by the
        // length, so two added in a row still differ.
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
        // The labels change with the axis for the two that name an edge, and
        // only for those two.
        assert_eq!(GridAlign::Start.label(GridAxis::Columns), "Left");
        assert_eq!(GridAlign::Start.label(GridAxis::Rows), "Top");
        assert_eq!(GridAlign::Center.label(GridAxis::Rows), "Center");
    }
}
