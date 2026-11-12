//! The Chart card's Style pictures: a fixed drawing per kind and preset, not the
//! chart shrunk. Each picture changes only what its preset changes, in the
//! chart's own first colours, with the chrome in theme ink and no ground.
//!
//! Drawn on a 44 × 32 grid, origin at the frame's top left: marks on whole
//! pixels, hairlines on half pixels, outline strokes inset half a pixel.

use eframe::egui;
use ondin_core::chart::{self, Chart, ChartKind, Preset};
use ondin_core::kurbo::{self, Arc, BezPath, Circle, Point, Rect, Shape, Vec2};
use ondin_core::peniko::Color;

/// The picture's size in points.
pub(super) const SIZE: egui::Vec2 = egui::vec2(44.0, 32.0);
/// The tint an Outline picture fills its marks with: stronger than the chart's
/// own `OUTLINE_TINT`, which vanishes at this size.
const TINT: f32 = 0.22;
/// How many colours a picture draws in, the most any kind needs.
pub(super) const COLOURS: usize = 4;

/// Which drawing a chart's cells show: its kind, with Donut and Bubble apart
/// because their shape differs. Stacking variants keep their kind's picture,
/// since no preset changes stacking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Picture {
    Column,
    Bar,
    Line,
    Area,
    Pie,
    Donut,
    Scatter,
    Bubble,
    Sankey,
}

impl Picture {
    pub(super) fn of(chart: &Chart) -> Self {
        match chart.kind {
            ChartKind::Column => Self::Column,
            ChartKind::Bar => Self::Bar,
            ChartKind::Line => Self::Line,
            ChartKind::Area => Self::Area,
            ChartKind::Pie if chart.options.slices.donut => Self::Donut,
            ChartKind::Pie => Self::Pie,
            ChartKind::Scatter if chart.mapping.size.is_some() => Self::Bubble,
            ChartKind::Scatter => Self::Scatter,
            ChartKind::Sankey => Self::Sankey,
        }
    }
}

/// The colours `chart` draws its first marks in, as [`COLOURS`] solid colours:
/// series for most kinds, the first named marks (`chart::named_marks`: a pie's
/// slices, a Sankey's nodes, a grouped scatter's groups) for those coloured by
/// name; the palette fills whatever the chart has fewer of. Bound
/// brushes must be resolved first.
pub(super) fn colours(chart: &Chart) -> [Color; COLOURS] {
    let solid = |b: &ondin_core::BrushSource| {
        b.literal()
            .and_then(|brush| super::paint::stops_of(brush).first().map(|(_, c)| *c))
    };
    let own: Vec<Color> = match chart::named_marks(chart) {
        Some(marks) => marks.iter().filter_map(|(_, b)| solid(b)).collect(),
        None => chart
            .mapping
            .series
            .iter()
            .filter_map(|c| chart::series_fill(chart, *c))
            .filter_map(|b| solid(&b))
            .collect(),
    };
    let palette = chart::palette(chart.options.palette, chart.options.dark);
    std::array::from_fn(|i| {
        own.get(i).copied().unwrap_or_else(|| {
            let c = palette[i % palette.len()];
            Color::from_rgb8(c[0], c[1], c[2])
        })
    })
}

/// The chrome's inks, each the theme's text colour at a share.
pub(super) struct Ink {
    frame: Color,
    grid: Color,
    axis: Color,
    dash: Color,
    value: Color,
}

impl Ink {
    /// The inks of the theme being painted in.
    pub(super) fn of_theme() -> Self {
        let at = |share: f32| {
            let [r, g, b, a] =
                crate::theme::color::text_a((share * 255.0).round() as u8).to_srgba_unmultiplied();
            Color::from_rgba8(r, g, b, a)
        };
        Self {
            frame: at(0.14),
            grid: at(0.10),
            axis: at(0.26),
            dash: at(0.30),
            value: at(0.62),
        }
    }
}

/// Filled paths in paint order, in device pixels: the grid scaled by `k`, with
/// every straight edge and hairline snapped to the pixel grid so a picture
/// drawn smaller than its grid stays crisp.
struct Art {
    out: Vec<(BezPath, Color)>,
    /// Device pixels per grid unit.
    k: f64,
}

impl Art {
    /// A hairline's width in device pixels: one at 1×, two at 2×.
    fn hair(&self) -> f64 {
        self.k.round().max(1.0)
    }

    /// Grid edge `v` on the nearest pixel boundary.
    fn edge(&self, v: f64) -> f64 {
        (v * self.k).round()
    }

    /// Grid value `v` moved onto a pixel boundary, still in grid units, for a
    /// curved shape's straight edges.
    fn on_grid(&self, v: f64) -> f64 {
        self.edge(v) / self.k
    }

    /// Grid rect `r` on pixel boundaries: its origin snapped and its size
    /// rounded, so equal shapes stay equal, and never thinner than a pixel.
    fn snap(&self, r: Rect) -> Rect {
        let (x0, y0) = (self.edge(r.x0), self.edge(r.y0));
        let w = (r.width() * self.k).round().max(1.0);
        let h = (r.height() * self.k).round().max(1.0);
        Rect::new(x0, y0, x0 + w, y0 + h)
    }

    /// The centre of a hairline drawn near grid `v`, so it covers whole pixels.
    fn centre(&self, v: f64) -> f64 {
        if self.hair() as i64 % 2 == 1 {
            (v * self.k - 0.5).round() + 0.5
        } else {
            (v * self.k).round()
        }
    }

    /// `shape`, in grid units, scaled and filled.
    fn fill(&mut self, shape: impl Shape, c: Color) {
        let path = kurbo::Affine::scale(self.k) * shape.to_path(0.05);
        self.out.push((path, c));
    }

    /// `rect` snapped and filled, its corners rounded by grid `r`.
    fn fill_rect(&mut self, rect: Rect, r: f64, c: Color) {
        let rect = self.snap(rect);
        let path = if r > 0.0 {
            rect.to_rounded_rect(r * self.k).to_path(0.05)
        } else {
            rect.to_path(0.05)
        };
        self.out.push((path, c));
    }

    /// A pixel-space path stroked `width` px wide.
    fn stroke_px(&mut self, path: BezPath, width: f64, c: Color, round: bool, dashed: bool) {
        let mut pen = kurbo::Stroke::new(width);
        if round {
            pen = pen
                .with_caps(kurbo::Cap::Round)
                .with_join(kurbo::Join::Round);
        }
        if dashed {
            let dash = (1.5 * self.k).round().max(1.0);
            pen = pen.with_dashes(0.0, [dash, dash]);
        }
        let ink = kurbo::stroke(
            path.path_elements(0.05),
            &pen,
            &kurbo::StrokeOpts::default(),
            0.05,
        );
        self.out.push((ink, c));
    }

    /// `shape`, in grid units, stroked `width` grid units wide with round caps
    /// and joins: the curves and slants, which no pixel grid can hold.
    fn stroke(&mut self, shape: impl Shape, width: f64, c: Color) {
        let path = kurbo::Affine::scale(self.k) * shape.to_path(0.05);
        self.stroke_px(path, width * self.k, c, true, false);
    }

    /// `rect` snapped, with a hairline inside its edge.
    fn stroke_rect(&mut self, rect: Rect, c: Color) {
        let h = self.hair();
        let rect = self.snap(rect).inset(-h / 2.0);
        self.stroke_px(rect.to_path(0.05), h, c, false, false);
    }

    fn hline(&mut self, y: f64, x0: f64, x1: f64, c: Color, dashed: bool) {
        let y = self.centre(y);
        let line = kurbo::Line::new((self.edge(x0), y), (self.edge(x1), y));
        self.stroke_px(line.to_path(0.05), self.hair(), c, false, dashed);
    }

    fn vline(&mut self, x: f64, y0: f64, y1: f64, c: Color, dashed: bool) {
        let x = self.centre(x);
        let line = kurbo::Line::new((x, self.edge(y0)), (x, self.edge(y1)));
        self.stroke_px(line.to_path(0.05), self.hair(), c, false, dashed);
    }

    /// A mark's box `b`: solid, rounded by `r`, or for Outline a tint under a
    /// hairline that stays inside the box.
    fn mark(&mut self, b: Rect, c: Color, s: Preset, r: f64) {
        if s == Preset::Outline {
            self.fill_rect(b, 0.0, c.multiply_alpha(TINT));
            self.stroke_rect(b, c);
        } else {
            self.fill_rect(b, r, c);
        }
    }

    /// A value label at this size: a short ink bar centred on `cx`.
    fn value(&mut self, cx: f64, y: f64, ink: &Ink) {
        let rect = self.snap(Rect::new(cx - 2.0, y, cx + 2.0, y + 1.8));
        let path = rect.to_rounded_rect(rect.height() / 2.0).to_path(0.05);
        self.out.push((path, ink.value));
    }

    fn legend(&mut self, cs: &[Color], ink: &Ink) {
        let xs: &[f64] = if cs.len() == 3 {
            &[6.0, 17.0, 28.0]
        } else {
            &[6.0, 18.0]
        };
        // The chart's legend swatch is one rounded square in every preset.
        for (x, c) in xs.iter().zip(cs) {
            let swatch = Rect::new(*x, 4.5, x + 3.0, 7.5);
            self.fill_rect(swatch, 0.6, *c);
            self.hline(6.0, x + 5.0, x + 9.0, ink.dash, false);
        }
    }

    /// The value gridlines: solid for Clean and Outline, dashed for Soft, none
    /// for Bold and Minimal.
    fn hgrid(&mut self, s: Preset, ink: &Ink) {
        if matches!(s, Preset::Bold | Preset::Minimal) {
            return;
        }
        for y in [13.5, 20.5] {
            self.hline(y, 4.0, 40.0, ink.grid, s == Preset::Soft);
        }
    }
}

/// Picture `pic` in preset `s`'s style, in `cs` and `ink`, as filled paths in
/// device pixels at `k` pixels per grid unit: `(SIZE * k).round()` pixels in all.
pub(super) fn art(
    pic: Picture,
    s: Preset,
    cs: &[Color; COLOURS],
    ink: &Ink,
    k: f64,
) -> Vec<(BezPath, Color)> {
    let mut a = Art { out: Vec::new(), k };
    let h = a.hair();
    let outer = Rect::new(0.0, 0.0, a.edge(SIZE.x as f64), a.edge(SIZE.y as f64));
    let frame = outer.inset(-h / 2.0).to_rounded_rect(3.0 * k - h / 2.0);
    a.stroke_px(frame.to_path(0.05), h, ink.frame, false, false);
    match pic {
        Picture::Column => column(&mut a, s, cs, ink),
        Picture::Bar => bar(&mut a, s, cs, ink),
        Picture::Line => lineish(&mut a, s, cs, ink, false),
        Picture::Area => lineish(&mut a, s, cs, ink, true),
        Picture::Pie => pieish(&mut a, s, cs, ink, false),
        Picture::Donut => pieish(&mut a, s, cs, ink, true),
        Picture::Scatter => dots(&mut a, s, cs, ink, false),
        Picture::Bubble => dots(&mut a, s, cs, ink, true),
        Picture::Sankey => sankey(&mut a, s, cs, ink),
    }
    a.out
}

/// Two groups of two. Bold widens the bars; Minimal gives the legend's room to
/// taller bars.
fn column(a: &mut Art, s: Preset, cs: &[Color; COLOURS], ink: &Ink) {
    let (bold, min) = (s == Preset::Bold, s == Preset::Minimal);
    let w = if bold { 7.0 } else { 6.0 };
    let xs = if bold {
        [5.0, 13.0, 24.0, 32.0]
    } else {
        [6.0, 13.0, 25.0, 32.0]
    };
    let hs = if min {
        [13.0, 7.0, 18.0, 11.0]
    } else {
        [11.0, 6.0, 15.0, 9.0]
    };
    let bottom = if min { 26.0 } else { 27.0 };
    let r = if s == Preset::Soft { 2.2 } else { 0.0 };
    if !min {
        a.legend(&cs[..2], ink);
    }
    a.hgrid(s, ink);
    for (i, (x, h)) in xs.iter().zip(hs).enumerate() {
        a.mark(Rect::new(*x, bottom - h, x + w, bottom), cs[i % 2], s, r);
        if bold {
            a.value(x + w / 2.0, bottom - h - 3.4, ink);
        }
    }
    if !min {
        let (x0, x1) = if bold { (3.0, 41.0) } else { (4.0, 40.0) };
        a.hline(27.5, x0, x1, ink.axis, false);
    }
}

/// Column on its side. Bold's values sit past each bar's end.
fn bar(a: &mut Art, s: Preset, cs: &[Color; COLOURS], ink: &Ink) {
    let (bold, min) = (s == Preset::Bold, s == Preset::Minimal);
    let x0 = if min { 6.0 } else { 7.0 };
    let ys = if min {
        [5.0, 11.0, 19.0, 25.0]
    } else {
        [9.0, 14.0, 21.0, 26.0]
    };
    let h = if min { 5.0 } else { 4.0 };
    let ls = match s {
        Preset::Minimal => [22.0, 13.0, 31.0, 20.0],
        Preset::Bold => [19.0, 11.0, 28.0, 17.0],
        _ => [22.0, 13.0, 31.0, 19.0],
    };
    let r = if s == Preset::Soft { 1.6 } else { 0.0 };
    if !min {
        a.legend(&cs[..2], ink);
    }
    if !matches!(s, Preset::Bold | Preset::Minimal) {
        for x in [19.5, 32.5] {
            a.vline(x, 9.0, 30.0, ink.grid, s == Preset::Soft);
        }
    }
    for (i, (y, l)) in ys.iter().zip(ls).enumerate() {
        a.mark(Rect::new(x0, *y, x0 + l, y + h), cs[i % 2], s, r);
        if bold {
            a.value(x0 + l + 3.5, y + h / 2.0 - 0.9, ink);
        }
    }
    if !min {
        a.vline(6.5, 8.5, 30.5, ink.axis, false);
    }
}

/// Two lines of four points, over a fill for an area.
fn lineish(a: &mut Art, s: Preset, cs: &[Color; COLOURS], ink: &Ink, area: bool) {
    const XS: [f64; 4] = [6.0, 16.0, 27.0, 38.0];
    let min = s == Preset::Minimal;
    let (top, low) = if min {
        ([18.0, 10.0, 13.0, 5.0], [24.0, 19.0, 21.0, 14.0])
    } else {
        ([20.0, 14.0, 16.0, 9.0], [24.0, 21.0, 22.0, 17.0])
    };
    let width = match (s, area) {
        (Preset::Outline, _) => 1.0,
        (Preset::Bold, true) => 2.5,
        (Preset::Bold, false) => 3.0,
        (_, true) => 1.5,
        (_, false) => 1.75,
    };
    if !min {
        a.legend(&cs[..2], ink);
    }
    a.hgrid(s, ink);
    let poly = |ys: &[f64; 4]| {
        let mut p = BezPath::new();
        p.move_to((XS[0], ys[0]));
        for (x, y) in XS.iter().zip(ys).skip(1) {
            p.line_to((*x, *y));
        }
        p
    };
    let series = [(&top, cs[0]), (&low, cs[1])];
    if area {
        for (ys, c) in series {
            let mut p = poly(ys);
            let bottom = a.on_grid(27.0);
            p.line_to((38.0, bottom));
            p.line_to((6.0, bottom));
            p.close_path();
            a.fill(p, c.multiply_alpha(0.32));
        }
    }
    for (ys, c) in series {
        a.stroke(poly(ys), width, c);
    }
    if s == Preset::Soft {
        for (ys, c) in series {
            for (x, y) in XS.iter().zip(ys) {
                a.fill(Circle::new((*x, *y), 1.6), c);
            }
        }
    }
    if s == Preset::Bold {
        for (x, y) in XS.iter().zip(top) {
            a.value(*x, y - 4.6, ink);
        }
    }
    if !min {
        a.hline(27.5, 4.0, 40.0, ink.axis, false);
    }
}

/// A slice from `a1` to `a2` (radians clockwise from twelve) inset by `d` on
/// every side, so a stroke of up to `2d` in its own colour stays inside it; its
/// round joins round the corners.
fn slice(c: Point, outer: f64, inner: f64, a1: f64, a2: f64, d: f64) -> BezPath {
    let quarter = std::f64::consts::FRAC_PI_2;
    let at = |rho: f64, a: f64| c + Vec2::new(rho * a.sin(), -rho * a.cos());
    let ro = outer - d;
    let d_out = (d / ro).min(1.0).asin();
    let mut p = BezPath::new();
    p.move_to(at(ro, a1 + d_out));
    let arc = Arc::new(
        c,
        (ro, ro),
        a1 + d_out - quarter,
        a2 - a1 - 2.0 * d_out,
        0.0,
    );
    p.extend(arc.append_iter(0.05));
    if inner > 0.0 {
        let ri = inner + d;
        let d_in = (d / ri).min(1.0).asin();
        p.line_to(at(ri, a2 - d_in));
        let arc = Arc::new(
            c,
            (ri, ri),
            a2 - d_in - quarter,
            -(a2 - a1 - 2.0 * d_in),
            0.0,
        );
        p.extend(arc.append_iter(0.05));
    } else if d > 0.0 {
        let half = (a2 - a1) / 2.0;
        p.line_to(at(d / half.sin(), a1 + half));
    } else {
        p.line_to(c);
    }
    p.close_path();
    p
}

/// Three slices, 50/30/20 from twelve. Soft cuts gaps and rounds every corner;
/// Bold writes its values inside the slices, where a pie draws them unless
/// told to lead them outside.
fn pieish(a: &mut Art, s: Preset, cs: &[Color; COLOURS], ink: &Ink, donut: bool) {
    const SHARES: [(f64, f64); 3] = [(0.0, 0.5), (0.5, 0.8), (0.8, 1.0)];
    let tau = std::f64::consts::TAU;
    let (cy, r) = match s {
        Preset::Minimal => (16.0, 12.0),
        _ => (19.5, 9.5),
    };
    let c = Point::new(22.0, cy);
    let hole = if donut { r * 0.6 } else { 0.0 };
    if s != Preset::Minimal {
        a.legend(&cs[..3], ink);
    }
    for (i, (s0, s1)) in SHARES.iter().enumerate() {
        let (a1, a2, col) = (s0 * tau, s1 * tau, cs[i]);
        match s {
            Preset::Soft => {
                let p = slice(c, r, hole, a1, a2, 1.4);
                a.fill(p.clone(), col);
                a.stroke(p, 1.6, col);
            }
            Preset::Outline => {
                let p = slice(c, r, hole, a1, a2, 0.5);
                a.fill(p.clone(), col.multiply_alpha(TINT));
                a.stroke(p, 1.0, col);
            }
            _ => a.fill(slice(c, r, hole, a1, a2, 0.0), col),
        }
        if s == Preset::Bold {
            let mid = (s0 + s1) / 2.0 * tau;
            let rho = (r + hole) / 2.0 + if donut { 0.0 } else { r * 0.15 };
            a.value(c.x + rho * mid.sin(), c.y - rho * mid.cos() - 0.9, ink);
        }
    }
}

/// Two clouds of five, or six bubbles of four sizes. A scatter draws no value
/// labels, so Bold here only drops the gridlines.
fn dots(a: &mut Art, s: Preset, cs: &[Color; COLOURS], ink: &Ink, bubble: bool) {
    const SCATTER: [[(f64, f64, f64); 5]; 2] = [
        [
            (8.0, 20.0, 0.0),
            (14.0, 15.0, 0.0),
            (22.0, 17.0, 0.0),
            (30.0, 11.0, 0.0),
            (37.0, 9.0, 0.0),
        ],
        [
            (10.0, 25.0, 0.0),
            (18.0, 23.0, 0.0),
            (25.0, 21.0, 0.0),
            (33.0, 19.0, 0.0),
            (38.0, 15.0, 0.0),
        ],
    ];
    const BUBBLES: [[(f64, f64, f64); 3]; 2] = [
        [(10.0, 21.0, 3.0), (21.0, 15.0, 5.0), (33.0, 13.0, 3.5)],
        [(14.0, 24.0, 2.5), (27.0, 22.0, 4.0), (37.0, 22.0, 2.0)],
    ];
    let min = s == Preset::Minimal;
    let dot = match s {
        Preset::Outline => 2.0,
        _ => 1.8,
    };
    let shift = if min { -3.0 } else { 0.0 };
    if !min {
        a.legend(&cs[..2], ink);
    }
    a.hgrid(s, ink);
    let sets: [&[(f64, f64, f64)]; 2] = if bubble {
        [&BUBBLES[0], &BUBBLES[1]]
    } else {
        [&SCATTER[0], &SCATTER[1]]
    };
    for (pts, col) in sets.iter().zip(cs) {
        for &(x, y, size) in *pts {
            let r = if bubble { size } else { dot };
            let at = (x, y + shift);
            if s == Preset::Outline {
                a.fill(Circle::new(at, r - 0.5), col.multiply_alpha(TINT));
                a.stroke(Circle::new(at, r - 0.5), 1.0, *col);
            } else {
                let alpha = if bubble { 0.8 } else { 0.9 };
                a.fill(Circle::new(at, r), col.multiply_alpha(alpha));
            }
        }
    }
    if !min {
        a.hline(27.5, 4.0, 40.0, ink.axis, false);
    }
}

/// Two sources into two targets, the flows crossing, each in its source's
/// colour. Labels are dashes outside the columns, which no preset restyles;
/// Minimal has none.
fn sankey(a: &mut Art, s: Preset, cs: &[Color; COLOURS], ink: &Ink) {
    let (bold, min) = (s == Preset::Bold, s == Preset::Minimal);
    let nw = if bold { 5.0 } else { 3.0 };
    let lx = if min { 6.0 } else { 11.0 };
    let rx = if min {
        35.0
    } else if bold {
        28.0
    } else {
        30.0
    };
    let opacity = if bold { 0.6 } else { 0.4 };
    let (x0, x1) = (a.on_grid(lx + nw), a.on_grid(rx));
    let mid = (x0 + x1) / 2.0;
    let mut band = |y1: f64, y2: f64, y3: f64, y4: f64, c: Color| {
        let [y1, y2, y3, y4] = [y1, y2, y3, y4].map(|y| a.on_grid(y));
        let mut p = BezPath::new();
        p.move_to((x0, y1));
        p.curve_to((mid, y1), (mid, y3), (x1, y3));
        p.line_to((x1, y4));
        p.curve_to((mid, y4), (mid, y2), (x0, y2));
        p.close_path();
        a.fill(p, c.multiply_alpha(opacity));
    };
    band(5.0, 14.0, 18.0, 27.0, cs[0]);
    band(18.0, 27.0, 5.0, 14.0, cs[1]);
    let r = if s == Preset::Soft { 1.5 } else { 0.0 };
    for (x, y, c) in [
        (lx, 5.0, cs[0]),
        (lx, 18.0, cs[1]),
        (rx, 5.0, cs[2]),
        (rx, 18.0, cs[3]),
    ] {
        a.mark(Rect::new(x, y, x + nw, y + 9.0), c, s, r);
    }
    if !min {
        for y in [9.5, 22.5] {
            a.hline(y, 4.0, lx - 2.0, ink.dash, false);
            a.hline(y, rx + nw + 2.0, 40.0, ink.dash, false);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Picture; 9] = [
        Picture::Column,
        Picture::Bar,
        Picture::Line,
        Picture::Area,
        Picture::Pie,
        Picture::Donut,
        Picture::Scatter,
        Picture::Bubble,
        Picture::Sankey,
    ];

    fn ink() -> Ink {
        Ink::of_theme()
    }

    fn drawn(pic: Picture, s: Preset) -> String {
        let cs = [Color::from_rgb8(1, 2, 3); COLOURS];
        format!("{:?}", art(pic, s, &cs, &ink(), 1.0))
    }

    /// Every kind's five pictures are different drawings, so no two cells look
    /// alike. It checks that they differ, not that each trait is one the
    /// preset writes and the generator draws for the kind.
    #[test]
    fn every_kind_draws_its_presets_apart() {
        for pic in ALL {
            for (i, a) in Preset::ALL.iter().enumerate() {
                for b in &Preset::ALL[i + 1..] {
                    assert_ne!(drawn(pic, *a), drawn(pic, *b), "{pic:?}: {a:?} and {b:?}");
                }
            }
        }
    }

    /// Nothing a picture draws leaves its frame, at full size or drawn smaller.
    #[test]
    fn every_picture_stays_inside_its_frame() {
        let cs = [Color::from_rgb8(1, 2, 3); COLOURS];
        for k in [1.0, 39.0 / 44.0, 78.0 / 44.0] {
            let frame = Rect::new(
                0.0,
                0.0,
                (SIZE.x as f64 * k).round(),
                (SIZE.y as f64 * k).round(),
            );
            each_inside(frame, k, &cs);
        }
    }

    /// Every path of every picture at `k` lies inside `frame`.
    fn each_inside(frame: Rect, k: f64, cs: &[Color; COLOURS]) {
        for pic in ALL {
            for s in Preset::ALL {
                for (path, _) in art(pic, s, cs, &ink(), k) {
                    let b = path.bounding_box();
                    let e = 1e-6;
                    assert!(
                        b.x0 >= frame.x0 - e
                            && b.y0 >= frame.y0 - e
                            && b.x1 <= frame.x1 + e
                            && b.y1 <= frame.y1 + e,
                        "{pic:?} {s:?} draws at {b:?}"
                    );
                }
            }
        }
    }

    /// A chart's own series colours lead, and the palette fills the rest.
    #[test]
    fn a_picture_wears_the_charts_own_colours_first() {
        let mut chart = Chart::sample(ChartKind::Column);
        let col = chart.mapping.series[0];
        let red = Color::from_rgb8(255, 0, 0);
        chart = chart::with_series_fill(&chart, col, ondin_core::Brush::Solid(red).into());
        let got = colours(&chart);
        assert_eq!(got[0], red);
        let p = chart::palette(chart.options.palette, chart.options.dark);
        assert_eq!(got[3], Color::from_rgb8(p[3][0], p[3][1], p[3][2]));
    }
}
