use super::*;

/// The numbers a cell parses as, and the text it does not.
#[test]
fn a_cell_parses_with_a_thousands_separator_and_not_a_decimal_comma() {
    for (cell, want) in [
        ("42", Some(42.0)),
        (" -3.5 ", Some(-3.5)),
        ("+7", Some(7.0)),
        ("1,234.5", Some(1234.5)),
        ("12,345,678", Some(12_345_678.0)),
        (".5", Some(0.5)),
        ("1,23", None),
        ("1234,567", None),
        ("3,5", None),
        ("1.", None),
        ("n/a", None),
        ("", None),
        ("-", None),
        ("inf", None),
        ("NaN", None),
        ("1e3", None),
    ] {
        assert_eq!(parse_number(cell), want, "{cell:?}");
    }
}

/// Every kind's sample table is a valid description that draws with nothing
/// left out, a Sankey's in three columns of nodes.
#[test]
fn every_sample_validates_and_draws_whole() {
    let size = kurbo::Size::new(480.0, 280.0);
    for kind in ChartKind::ALL {
        let chart = Chart::sample(kind);
        assert_eq!(chart.validate(), Ok(()), "{kind:?}");
        let got = generate(&chart, size, "Inter").expect("draws");
        assert!(got.left_out.is_empty(), "{kind:?}: {:?}", got.left_out);
        assert!(!got.parts.is_empty(), "{kind:?}");
    }
    let got = generate(&Chart::sample(ChartKind::Sankey), size, "Inter").unwrap();
    let mut lefts: Vec<i64> = got
        .parts
        .iter()
        .filter(|p| p.key.role == PartRole::SankeyNode)
        .map(|p| p.transform.translation().x.round() as i64)
        .collect();
    lefts.sort_unstable();
    lefts.dedup();
    assert_eq!(lefts.len(), 3, "node columns: {lefts:?}");
}

/// `chart` drawn at the size a new chart has.
fn draw(chart: &Chart) -> Generated {
    generate(chart, kurbo::Size::new(480.0, 280.0), "Inter").expect("draws")
}

fn part(g: &Generated, role: PartRole, series: u32, index: u32) -> Option<&Part> {
    g.parts.iter().find(|p| {
        p.key
            == PartKey {
                role,
                series,
                index,
            }
    })
}

fn all(g: &Generated, role: PartRole, series: u32) -> Vec<&Part> {
    g.parts
        .iter()
        .filter(|p| p.key.role == role && p.key.series == series)
        .collect()
}

/// A part's drawn box in its parent's space.
fn drawn_box(p: &Part) -> kurbo::Rect {
    let b = crate::geometry::local_bounds(&p.kind, None).expect("has a box");
    p.transform.transform_rect_bbox(b)
}

fn box_size(p: &Part) -> kurbo::Size {
    p.kind.box_size().expect("a box-sized part")
}

/// The Body grid's track counts, columns then rows.
fn tracks(g: &Generated) -> (usize, usize) {
    match &part(g, PartRole::Body, 0, 0).expect("a body").display {
        Some(crate::container::Display::Grid(grid)) => (grid.columns.len(), grid.rows.len()),
        other => panic!("the body is a grid: {other:?}"),
    }
}

/// A title takes a track of its own past its axis's labels: the value axis's
/// turned a quarter to read upwards, left of every tick label; the category
/// axis's flat under the category labels.
#[test]
fn an_axis_title_takes_a_track_of_its_own_past_its_labels() {
    let plain = Chart::sample(ChartKind::Column);
    let mut titled = plain.clone();
    titled.options.value_axis.title = "Revenue".into();
    titled.options.category_axis.title = "Month".into();
    let (a, b) = (draw(&plain), draw(&titled));
    assert_eq!(tracks(&a), (2, 2));
    assert_eq!(tracks(&b), (3, 3));
    assert!(part(&a, PartRole::AxisTitle, 0, 0).is_none());

    let vt = part(&b, PartRole::AxisTitle, 0, 0).expect("a value axis title");
    assert_eq!(
        vt.parent,
        Some(part(&b, PartRole::ValueAxis, 0, 0).unwrap().key)
    );
    let c = vt.transform.as_coeffs();
    assert!(
        c[0].abs() < 1e-9 && (c[1] + 1.0).abs() < 1e-9,
        "turned: {c:?}"
    );
    let title = drawn_box(vt);
    let labels = all(&b, PartRole::TickLabel, 0);
    assert!(!labels.is_empty());
    let first_label = labels
        .iter()
        .map(|p| drawn_box(p).x0)
        .fold(f64::INFINITY, f64::min);
    assert!(
        title.x0 > -0.5 && title.x1 <= first_label,
        "{title:?} vs {first_label}"
    );
    // The labels keep their gap from the plot.
    for (l, m) in labels.iter().zip(all(&a, PartRole::TickLabel, 0)) {
        assert_eq!(l.insets.right, m.insets.right);
    }

    let ct = part(&b, PartRole::AxisTitle, 1, 0).expect("a category axis title");
    assert_eq!(
        ct.parent,
        Some(part(&b, PartRole::CategoryAxis, 0, 0).unwrap().key)
    );
    assert_eq!(ct.transform.as_coeffs()[1], 0.0, "flat");
    let deepest = all(&b, PartRole::CategoryLabel, 0)
        .iter()
        .map(|p| drawn_box(p).y1)
        .fold(0.0, f64::max);
    assert!(drawn_box(ct).y0 >= deepest);
    // Both titles' tracks come out of the plot.
    let plot = |g: &Generated| box_size(part(g, PartRole::Plot, 0, 0).unwrap());
    assert!(plot(&b).width < plot(&a).width && plot(&b).height < plot(&a).height);
}

/// Tick marks are 4 px stubs out from the plot at every tick and every
/// category, pinned like the labels, which they push 4 px further out.
#[test]
fn tick_marks_stand_between_the_plot_and_the_labels() {
    let plain = Chart::sample(ChartKind::Column);
    let mut ticked = plain.clone();
    ticked.options.value_axis.ticks = true;
    ticked.options.category_axis.ticks = true;
    let (a, b) = (draw(&plain), draw(&ticked));
    assert!(all(&a, PartRole::Tick, 0).is_empty());

    let value = all(&b, PartRole::Tick, 0);
    assert_eq!(value.len(), all(&b, PartRole::TickLabel, 0).len());
    let axis_w = box_size(part(&b, PartRole::ValueAxis, 0, 0).unwrap()).width;
    for t in &value {
        assert_eq!(box_size(t), kurbo::Size::new(4.0, 1.0));
        assert_eq!(t.insets.right, Some(crate::container::LengthPct::Px(0.0)));
        assert!((drawn_box(t).x1 - axis_w).abs() < 1e-9, "against the plot");
    }
    let cats = all(&b, PartRole::Tick, 1);
    assert_eq!(cats.len(), 6);
    for t in &cats {
        assert_eq!(box_size(t), kurbo::Size::new(1.0, 4.0));
        assert_eq!(t.insets.top, Some(crate::container::LengthPct::Px(0.0)));
    }

    let gap = |g: &Generated| match all(g, PartRole::TickLabel, 0)[0].insets.right {
        Some(crate::container::LengthPct::Px(v)) => v,
        other => panic!("{other:?}"),
    };
    assert_eq!(gap(&b) - gap(&a), 4.0);
    let width = |g: &Generated| box_size(part(g, PartRole::ValueAxis, 0, 0).unwrap()).width;
    assert_eq!(width(&b) - width(&a), 4.0);
}

/// The value axis line runs along the plot's edge on the axis's side, off by
/// default; the category axis line is the baseline at zero, or the plot's edge
/// when zero is out of range, never outside the plot.
#[test]
fn axis_lines_sit_on_the_plot() {
    let mut c = Chart::sample(ChartKind::Column);
    assert!(part(&draw(&c), PartRole::AxisLine, 0, 0).is_none());
    c.options.value_axis.line = true;
    let g = draw(&c);
    let plot = box_size(part(&g, PartRole::Plot, 0, 0).unwrap());
    let line = drawn_box(part(&g, PartRole::AxisLine, 0, 0).expect("a value axis line"));
    assert_eq!(line, kurbo::Rect::new(-0.5, 0.0, 0.5, plot.height));
    c.options.value_axis.side = super::AxisSide::End;
    let g = draw(&c);
    let plot = box_size(part(&g, PartRole::Plot, 0, 0).unwrap());
    let line = drawn_box(part(&g, PartRole::AxisLine, 0, 0).unwrap());
    assert!((line.center().x - plot.width).abs() < 1e-9);

    // The sample scatter's Y runs 35 to 78, so zero is below the plot.
    let g = draw(&Chart::sample(ChartKind::Scatter));
    let plot = box_size(part(&g, PartRole::Plot, 0, 0).unwrap());
    let base = part(&g, PartRole::AxisLine, 1, 0).expect("a category axis line");
    assert_eq!(base.name, "Axis line");
    assert!((drawn_box(base).center().y - plot.height).abs() < 1e-9);
}

/// A dashed grid draws each gridline as a stroked line with a dash, pinned
/// across the plot; a solid one as a 1 px rect.
#[test]
fn a_dashed_grid_puts_its_dash_on_a_stroke() {
    let mut c = Chart::sample(ChartKind::Line);
    for p in all(&draw(&c), PartRole::Gridline, 0) {
        assert!(matches!(p.kind, crate::node::NodeKind::Rect { .. }));
    }
    c.options.value_axis.grid.dashed = true;
    let g = draw(&c);
    let lines = all(&g, PartRole::Gridline, 0);
    assert!(!lines.is_empty());
    for p in lines {
        assert!(
            matches!(p.kind, crate::node::NodeKind::Path { .. }),
            "{}",
            p.name
        );
        assert!(p.fills.is_empty());
        assert_eq!(p.strokes.len(), 1);
        assert!(!p.strokes[0].dashes.is_empty());
        assert_eq!(
            p.insets.left,
            Some(crate::container::LengthPct::Percent(0.0))
        );
        assert!(drawn_box(p).height() < 1e-9, "flat");
    }
}

/// A slice's corner radius rounds the wedge's corners and not the anchors along
/// its arcs, and the slice is pinned by the rounded outline's box.
#[test]
fn a_slice_rounds_its_corners_and_is_pinned_by_their_box() {
    let mut c = Chart::sample(ChartKind::Pie);
    c.options.slices.radius = 6.0;
    let corners = |g: &Generated| -> Vec<usize> {
        all(g, PartRole::Slice, 0)
            .iter()
            .map(|p| match &p.kind {
                crate::node::NodeKind::Path { corner_radii, .. } => {
                    corner_radii.iter().filter(|r| **r == 6.0).count()
                }
                _ => panic!("a wedge path"),
            })
            .collect()
    };
    let g = draw(&c);
    assert!(corners(&g).iter().all(|n| *n == 3), "{:?}", corners(&g));
    let frame = box_size(part(&g, PartRole::PieFrame, 0, 0).unwrap());
    // The first slice starts at twelve o'clock, where rounding moves its top.
    let first = all(&g, PartRole::Slice, 0)[0];
    let top = match first.insets.top {
        Some(crate::container::LengthPct::Percent(v)) => v / 100.0 * frame.height,
        other => panic!("{other:?}"),
    };
    assert!(top > 0.1, "pinned by the rounded box: {top}");
    assert!((top - drawn_box(first).y0).abs() < 1e-6);

    c.options.slices.donut = true;
    assert!(corners(&draw(&c)).iter().all(|n| *n == 4));
}

/// A series' own point shape wins over a scatter's `Points::shape`, and gives a
/// line's points their shape; without one a line's points are circles.
#[test]
fn a_series_point_shape_overrides_the_charts() {
    let style = |col: u32, point| SeriesStyle {
        column: ColumnId(col),
        fill: crate::variable::BrushSource::Literal(crate::Brush::Solid(peniko::Color::BLACK)),
        visible: true,
        dash: Dash::Solid,
        point,
    };
    let is = |g: &Generated, series: u32, want: fn(&crate::node::NodeKind) -> bool| {
        let points = all(g, PartRole::Point, series);
        assert!(!points.is_empty());
        points.iter().all(|p| want(&p.kind))
    };
    let rect = |k: &crate::node::NodeKind| matches!(k, crate::node::NodeKind::Rect { .. });
    let ellipse = |k: &crate::node::NodeKind| matches!(k, crate::node::NodeKind::Ellipse { .. });
    let path = |k: &crate::node::NodeKind| matches!(k, crate::node::NodeKind::Path { .. });

    let mut line = Chart::sample(ChartKind::Line);
    line.options.lines.points = PointStyle::Filled;
    line.series = vec![style(1, Some(PointShape::Square))];
    let g = draw(&line);
    assert!(is(&g, 1, rect));
    assert!(is(&g, 2, ellipse));

    let mut scatter = Chart::sample(ChartKind::Scatter);
    scatter.options.points.shape = PointShape::Diamond;
    scatter.series = vec![style(2, Some(PointShape::Circle)), style(1, None)];
    let g = draw(&scatter);
    assert!(is(&g, 1, path));
    assert!(is(&g, 2, ellipse));
}

/// A *Gradient* flow is filled with a linear gradient across its ribbon, from
/// the colour a *Source* flow takes to the one a *Target* flow takes.
#[test]
fn a_gradient_flow_runs_from_its_source_colour_to_its_target_colour() {
    let with = |colour| {
        let mut c = Chart::sample(ChartKind::Sankey);
        c.options.flows.colour = colour;
        draw(&c)
    };
    let (source, target, gradient) = (
        with(FlowColour::Source),
        with(FlowColour::Target),
        with(FlowColour::Gradient),
    );
    let flows = all(&gradient, PartRole::Flow, 0);
    assert_eq!(flows.len(), 8);
    let solid = |p: &Part| match p.fills[0].brush.literal() {
        Some(crate::Brush::Solid(c)) => *c,
        other => panic!("{other:?}"),
    };
    for (i, f) in flows.iter().enumerate() {
        let Some(crate::Brush::Gradient(g)) = f.fills[0].brush.literal() else {
            panic!("{}: a gradient", f.name)
        };
        let peniko::GradientKind::Linear(l) = g.gradient.kind else {
            panic!("linear")
        };
        let b = drawn_box(f);
        assert!((l.start.x - b.x0).abs() < 1e-9 && (l.end.x - b.x1).abs() < 1e-9);
        let stops: Vec<peniko::Color> = g
            .gradient
            .stops
            .iter()
            .map(|s| s.color.to_alpha_color())
            .collect();
        let from = solid(all(&source, PartRole::Flow, 0)[i]);
        let to = solid(all(&target, PartRole::Flow, 0)[i]);
        assert_eq!(stops, vec![from, to]);
        assert_ne!(from, to, "a flow between two colours");
    }
}
