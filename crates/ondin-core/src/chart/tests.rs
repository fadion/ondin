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

/// A label angle turns the labels of an axis beside the plot too: a column
/// chart's value labels and a bar chart's category labels stand upright inside
/// their axis's frame, a gap off the plot, and the frame narrows to fit them.
/// flip: `set_turn` answering 0 for `Upright` → fails at "turned".
#[test]
fn labels_beside_the_plot_turn_to_their_angle() {
    let quarter = |p: &Part| {
        let c = p.transform.as_coeffs();
        c[0].abs() < 1e-9 && (c[1] + 1.0).abs() < 1e-9
    };
    for (kind, axis, role) in [
        (ChartKind::Column, PartRole::ValueAxis, PartRole::TickLabel),
        (
            ChartKind::Bar,
            PartRole::CategoryAxis,
            PartRole::CategoryLabel,
        ),
    ] {
        let flat = Chart::sample(kind);
        let mut upright = flat.clone();
        upright.options.value_axis.angle = LabelAngle::Upright;
        upright.options.category_axis.angle = LabelAngle::Upright;
        let (a, b) = (draw(&flat), draw(&upright));
        let labels = all(&b, role, 0);
        assert!(!labels.is_empty(), "{kind:?}: the fixture has labels");
        assert!(labels.iter().all(|p| quarter(p)), "{kind:?}: turned");
        assert!(
            !all(&a, role, 0).iter().any(|p| quarter(p)),
            "{kind:?}: flat by default"
        );
        let width = |g: &Generated| box_size(part(g, axis, 0, 0).unwrap()).width;
        let w = width(&b);
        assert!(
            w < width(&a),
            "{kind:?}: the frame narrows, {w} vs {}",
            width(&a)
        );
        // Against the plot by the same gap, each one as far out as it reaches.
        let right = drawn_box(labels[0]).x1;
        assert!(
            right < w - 1.0,
            "{kind:?}: a gap off the plot, {right} in {w}"
        );
        for p in labels {
            let bx = drawn_box(p);
            assert!(
                bx.x0 >= -1e-6 && (bx.x1 - right).abs() < 1e-6,
                "{kind:?}: {bx:?} inside a {w} frame, its right edge at {right}"
            );
        }
    }
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

/// An edit carried to another chart sets what changed and keeps what that chart
/// had of its own, including an option set back to its skipped default.
#[test]
fn an_options_edit_carries_only_what_changed() {
    let before = Options::default();
    let mut after = before.clone();
    after.bars.radius = 9.0;
    after.value_axis.format.prefix = "$".into();
    let mut onto = before.clone();
    onto.bars.group_gap = 50.0;
    onto.outline = Some(3.0);
    let got = Options::carried(&before, &after, &onto);
    assert_eq!(got.bars.radius, 9.0);
    assert_eq!(got.value_axis.format.prefix, "$");
    assert_eq!(got.bars.group_gap, 50.0, "its own gap kept");
    assert_eq!(got.outline, Some(3.0), "its own outline kept");

    let mut outlined = before.clone();
    outlined.outline = Some(2.0);
    let got = Options::carried(&outlined, &before, &onto);
    assert_eq!(got.outline, None, "turned off is carried");
}

/// *Outline* draws every kind's marks as an inside stroke in their colour over
/// a tint of it, and *Clean* fills them solid with no stroke.
/// flip: `mark_paint` ignoring `Options::outline` → fails at "stroked".
#[test]
fn outlined_marks_are_stroked_over_a_tint() {
    let roles = [
        (ChartKind::Column, PartRole::Bar),
        (ChartKind::Pie, PartRole::Slice),
        (ChartKind::Scatter, PartRole::Point),
        (ChartKind::Sankey, PartRole::SankeyNode),
    ];
    for (kind, role) in roles {
        let mut chart = Chart::sample(kind);
        chart.options = Preset::Outline.apply(kind, &chart.options);
        assert_eq!(chart.validate(), Ok(()), "{kind:?}");
        let g = draw(&chart);
        let marks: Vec<&Part> = g.parts.iter().filter(|p| p.key.role == role).collect();
        assert!(!marks.is_empty(), "{kind:?}");
        for m in marks {
            let stroke = m.strokes.first().expect("stroked");
            assert_eq!(stroke.align, crate::node::StrokeAlign::Inside, "{kind:?}");
            let (Some(crate::Brush::Solid(ink)), Some(crate::Brush::Solid(tint))) =
                (stroke.brush.literal(), m.fills[0].brush.literal())
            else {
                panic!("{kind:?}: solid paints");
            };
            assert!((tint.components[3] - OUTLINE_TINT * ink.components[3]).abs() < 1e-4);
        }
        let clean = draw(&Chart::sample(kind));
        assert!(
            clean
                .parts
                .iter()
                .filter(|p| p.key.role == role)
                .all(|p| p.strokes.is_empty()),
            "{kind:?}: Clean fills solid"
        );
    }
}

/// A chart turned dark draws its bars in the dark palette, its labels in the
/// dark ink, and a hollow point filled with the dark ground.
/// flip: the generator reading the light palette → fails at "the dark
/// palette's first".
#[test]
fn a_dark_chart_draws_dark() {
    let solid = |c| crate::variable::BrushSource::Literal(crate::Brush::Solid(c));
    let night = chrome(true);
    let column = with_dark(&Chart::sample(ChartKind::Column), true);
    let g = draw(&column);
    let [r, gr, b] = palette(column.options.palette, true)[0];
    let bar = all(&g, PartRole::Bar, column.mapping.series[0].0)[0];
    assert_eq!(
        bar.fills[0].brush,
        solid(peniko::Color::from_rgb8(r, gr, b)),
        "the dark palette's first"
    );
    let label = all(&g, PartRole::TickLabel, 0)[0];
    assert_eq!(label.fills[0].brush, solid(night.ink));
    let mut line = Chart::sample(ChartKind::Line);
    line.options.lines.points = PointStyle::Hollow;
    let g = draw(&with_dark(&line, true));
    let point = g.parts.iter().find(|p| p.key.role == PartRole::Point);
    assert_eq!(
        point.expect("a hollow point").fills[0].brush,
        solid(night.ground)
    );
}

/// The value axis's tick labels, lowest first.
fn tick_text(g: &Generated) -> Vec<String> {
    all(g, PartRole::TickLabel, 0)
        .iter()
        .map(|p| match &p.kind {
            crate::node::NodeKind::Text { content, .. } => content.clone(),
            other => panic!("a label: {other:?}"),
        })
        .collect()
}

/// A stack whose sum lands a rounding error past a tick tops its axis at that
/// tick: 100% stacked shares of 9, 18 and 1 add to just over 1, and 0.1 + 0.2
/// to just over 0.3.
/// flip: `scale::ticks` without its snap → fails at "0.3"; also `lines`
/// without its 100% clamp → fails at the area's "100%". The clamp alone is not
/// pinned: the snap covers these shares.
#[test]
fn a_stack_a_rounding_error_past_a_tick_ends_at_the_tick() {
    let one = |kind, values: [&str; 3]| {
        let mut c = Chart::sample(kind);
        let mut row = vec!["Jan".to_string()];
        row.extend(values.iter().map(|v| (*v).to_string()));
        c.table.rows = vec![row];
        c
    };
    // The fixture: the shares do add past 1.
    let (a, b, c) = (9.0f64 / 28.0, 18.0 / 28.0, 1.0 / 28.0);
    assert!(a + b + c > 1.0);
    for kind in [ChartKind::Column, ChartKind::Area] {
        let mut chart = one(kind, ["9", "18", "1"]);
        chart.options.bars.stacking = Stacking::Percent;
        chart.options.lines.area_stacking = Stacking::Percent;
        let ticks = tick_text(&draw(&chart));
        assert_eq!(
            ticks.last().map(String::as_str),
            Some("100%"),
            "{kind:?}: {ticks:?}"
        );
    }
    for kind in [ChartKind::Column, ChartKind::Area] {
        let mut chart = one(kind, ["0.1", "0.2", "0"]);
        chart.options.bars.stacking = Stacking::Stacked;
        chart.options.lines.area_stacking = Stacking::Stacked;
        let ticks = tick_text(&draw(&chart));
        assert_eq!(
            ticks.last().map(String::as_str),
            Some("0.3"),
            "{kind:?}: {ticks:?}"
        );
    }
}

/// A line or area chart with value labels and no points refuses past
/// `MAX_MARKS` values, as one with points does.
/// flip: `lines` counting only points → fails at "refused".
#[test]
fn line_value_labels_count_toward_the_mark_limit() {
    for kind in [ChartKind::Line, ChartKind::Area] {
        let mut c = Chart::sample(kind);
        c.mapping.series.truncate(1);
        c.table.rows = (0..=MAX_MARKS)
            .map(|i| vec![format!("R{i}"), "1".into(), "1".into(), "1".into()])
            .collect();
        c.options.lines.points = PointStyle::None;
        assert!(draw(&c).parts.len() > 1, "the fixture draws without labels");
        c.options.values.position = ValuePosition::Inside;
        match generate(&c, kurbo::Size::new(480.0, 280.0), "Inter") {
            Err(Refusal::TooManyMarks { count, what }) => {
                assert_eq!((count, what), (MAX_MARKS + 1, "value labels"), "{kind:?}");
            }
            Err(other) => panic!("{kind:?}: {other:?}"),
            Ok(g) => panic!("{kind:?}: not refused, {} parts", g.parts.len()),
        }
    }
}

/// A bubble chart says once which row it leaves out for want of a size,
/// grouped or not.
/// flip: `scatter_clouds` skipping a sizeless row silently when ungrouped →
/// fails at "ungrouped".
#[test]
fn a_bubble_with_no_size_is_left_out_and_said() {
    let mut c = Chart::sample(ChartKind::Scatter);
    c.table.columns.push(Column {
        id: ColumnId(3),
        name: "Size".into(),
    });
    for (i, r) in c.table.rows.iter_mut().enumerate() {
        r.push(if i == 2 { String::new() } else { "5".into() });
    }
    c.mapping.size = Some(ColumnId(3));
    let said = |g: &Generated| g.left_out.iter().filter(|s| *s == "Row 3: no size").count();
    assert_eq!(said(&draw(&c)), 1, "ungrouped: {:?}", draw(&c).left_out);
    c.table.columns.push(Column {
        id: ColumnId(4),
        name: "Group".into(),
    });
    for r in &mut c.table.rows {
        r.push("a".into());
    }
    c.mapping.group = Some(ColumnId(4));
    assert_eq!(said(&draw(&c)), 1, "grouped: {:?}", draw(&c).left_out);
}

/// A slice too thin to draw past the pad angle takes no label and no leader
/// line, inside or out.
/// flip: `pie` labelling every slice → fails at "no label".
#[test]
fn a_slice_too_thin_to_draw_takes_no_label() {
    let mut c = Chart::sample(ChartKind::Pie);
    c.table.rows[0][1] = "0.1".into();
    c.options.slices.pad_angle = 10.0;
    c.options.values.position = ValuePosition::Inside;
    for outside in [true, false] {
        c.options.values.pie_outside = outside;
        let g = draw(&c);
        assert!(part(&g, PartRole::Slice, 0, 0).is_none(), "the fixture");
        assert!(part(&g, PartRole::Slice, 0, 1).is_some(), "the fixture");
        assert!(part(&g, PartRole::ValueLabel, 0, 1).is_some());
        assert!(part(&g, PartRole::ValueLabel, 0, 0).is_none(), "no label");
        assert!(part(&g, PartRole::LeaderLine, 0, 0).is_none(), "no leader");
    }
}

/// A zero on top of a stack is not its end: the segment under it keeps the
/// outside label it has when the cell is empty.
/// flip: `bar_marks` counting a zero segment as covering → fails at "outside".
#[test]
fn a_zero_on_a_stack_does_not_take_its_outside_label() {
    let with_top = |cell: &str| {
        let mut c = Chart::sample(ChartKind::Column);
        c.options.bars.stacking = Stacking::Stacked;
        c.options.values.position = ValuePosition::Outside;
        c.table.rows[0][3] = cell.into();
        draw(&c)
    };
    let (zero, empty) = (with_top("0"), with_top(""));
    let costs = |g: &Generated| part(g, PartRole::ValueLabel, 2, 0).map(|p| p.transform);
    let bar = drawn_box(part(&empty, PartRole::Bar, 2, 0).unwrap());
    let label = part(&empty, PartRole::ValueLabel, 2, 0).expect("the fixture: a label");
    assert!(
        drawn_box(label).y1 <= bar.y0 + 1e-9,
        "the fixture: above the bar"
    );
    assert_eq!(costs(&zero), costs(&empty), "outside");
}

/// Every number `p` carries is finite: its transform, its geometry and its insets.
fn finite(p: &Part) -> bool {
    use crate::container::LengthPct;
    let inset = |l: Option<LengthPct>| match l {
        Some(LengthPct::Px(v) | LengthPct::Percent(v)) => v.is_finite(),
        None => true,
    };
    let geometry = match &p.kind {
        crate::node::NodeKind::Path {
            path, corner_radii, ..
        } => crate::node::path_is_finite(path) && corner_radii.iter().all(|v| v.is_finite()),
        crate::node::NodeKind::Rect {
            size, corner_radii, ..
        } => {
            let r = corner_radii;
            [
                size.width,
                size.height,
                r.top_left,
                r.top_right,
                r.bottom_right,
                r.bottom_left,
            ]
            .iter()
            .all(|v| v.is_finite())
        }
        k => k
            .box_size()
            .is_none_or(|s| s.width.is_finite() && s.height.is_finite()),
    };
    let i = &p.insets;
    geometry
        && p.transform.as_coeffs().iter().all(|v| v.is_finite())
        && [i.top, i.right, i.bottom, i.left].into_iter().all(inset)
}

/// A chart drawn smaller than its axes' labels, so its plot has no width or no
/// height, still writes only finite numbers, whatever its curve.
/// flip: `curve` taking a slope between points with no room between them →
/// fails at "Line Smooth".
#[test]
fn a_chart_with_no_room_for_its_plot_draws_finite_parts() {
    for kind in ChartKind::ALL {
        for curve in [Curve::Straight, Curve::Smooth, Curve::Step] {
            let mut c = Chart::sample(kind);
            c.options.lines.curve = curve;
            c.options.lines.points = PointStyle::Filled;
            c.options.values.position = ValuePosition::Inside;
            for (w, h) in [(1.0, 1.0), (30.0, 30.0), (50.0, 200.0), (200.0, 20.0)] {
                let g = generate(&c, kurbo::Size::new(w, h), "Inter").expect("draws");
                for p in &g.parts {
                    assert!(finite(p), "{kind:?} {curve:?} at {w}×{h}: {p:?}");
                }
            }
        }
    }
}

/// A table holding the largest column id still names a new column with an id
/// none holds, and a replaced table keeps its ids distinct.
/// flip: `next_column_id` adding one to the highest id unchecked → overflows
/// (debug) before "a free id".
#[test]
fn a_column_id_at_the_top_still_leaves_room() {
    let mut c = Chart::sample(ChartKind::Column);
    c.table.columns[3].id = ColumnId(u32::MAX);
    c.mapping.series = vec![ColumnId(1), ColumnId(2), ColumnId(u32::MAX)];
    assert_eq!(c.validate(), Ok(()), "the fixture");
    let free = c.table.next_column_id();
    assert!(c.table.index_of(free).is_none(), "a free id: {free:?}");
    let mut table = c.table.clone();
    for (i, name) in ["New", "Newer"].into_iter().enumerate() {
        table.columns.push(Column {
            id: ColumnId(i as u32),
            name: name.into(),
        });
        for r in &mut table.rows {
            r.push("1".into());
        }
    }
    let next = c.with_table(table);
    assert_eq!(next.validate(), Ok(()), "{:?}", next.table.columns);
}

/// `kind`'s sample chart on `rows`, each a category and then one cell per
/// series column.
fn on_rows(kind: ChartKind, rows: &[&[&str]]) -> Chart {
    let mut c = Chart::sample(kind);
    c.table.rows = rows
        .iter()
        .map(|r| {
            let mut row: Vec<String> = r.iter().map(|c| (*c).to_string()).collect();
            row.resize(c.table.columns.len(), String::new());
            row
        })
        .collect();
    c
}

/// A part's box in the chart frame's space, through its parents' transforms.
fn world_box(g: &Generated, p: &Part) -> kurbo::Rect {
    let mut b = drawn_box(p);
    let mut at = p.parent;
    while let Some(k) = at {
        let parent = g.parts.iter().find(|q| q.key == k).expect("a parent");
        b = parent.transform.transform_rect_bbox(b);
        at = parent.parent;
    }
    b
}

/// The path a part draws.
fn path_of(p: &Part) -> &kurbo::BezPath {
    match &p.kind {
        crate::node::NodeKind::Path { path, .. } => path,
        other => panic!("a path: {other:?}"),
    }
}

/// A stacked Smooth area's floor is the smoothed top of the series under it,
/// not chords across it.
/// flip: `curve` refusing a floor that runs right to left → fails at "smooth
/// floor".
#[test]
fn a_stacked_smooth_area_stands_on_the_curve_below() {
    let mut c = on_rows(
        ChartKind::Area,
        &[
            &["a", "1", "1"],
            &["b", "5", "1"],
            &["c", "1", "1"],
            &["d", "5", "1"],
        ],
    );
    c.mapping.series.truncate(2);
    c.options.lines.area_stacking = Stacking::Stacked;
    c.options.lines.curve = Curve::Smooth;
    let g = draw(&c);
    let upper = part(&g, PartRole::Area, 2, 0).expect("the upper area");
    let curves = path_of(upper)
        .elements()
        .iter()
        .filter(|e| matches!(e, kurbo::PathEl::CurveTo(..)))
        .count();
    // Three segments along its top, three back along its floor.
    assert_eq!(curves, 6, "smooth floor: {:?}", path_of(upper));
}

/// With *Connect gaps* on a stacked area, a gap stands on the series below:
/// the gapped series' line dips to that series' top there.
/// flip: `runs_of` skipping a stacked gap → fails at "on the stack".
#[test]
fn a_connected_gap_in_a_stack_stands_on_the_series_below() {
    let mut c = on_rows(
        ChartKind::Area,
        &[
            &["a", "1", "1", "1"],
            &["b", "5", "", "1"],
            &["c", "1", "1", "1"],
        ],
    );
    c.options.lines.area_stacking = Stacking::Stacked;
    c.options.lines.connect_gaps = true;
    let g = draw(&c);
    let line = |s| path_of(part(&g, PartRole::Line, s, 0).unwrap()).clone();
    let (a, b) = (line(1), line(2));
    let mid = |p: &kurbo::BezPath| {
        p.elements()
            .iter()
            .filter_map(|e| match e {
                kurbo::PathEl::MoveTo(q) | kurbo::PathEl::LineTo(q) => Some(*q),
                _ => None,
            })
            .nth(1)
            .expect("a middle point")
    };
    assert!(
        (mid(&a).y - mid(&b).y).abs() < 1e-9,
        "on the stack: A {:?}, B {:?}",
        mid(&a),
        mid(&b)
    );
    assert!(
        all(&g, PartRole::Point, 2).is_empty(),
        "a gap draws no point"
    );
}

/// A line value with no neighbour is drawn as a dot, not left as a bare move.
/// flip: `lines` drawing no dot for a one-point run → fails at "a dot".
#[test]
fn a_lone_line_value_draws_a_dot() {
    let mut c = on_rows(ChartKind::Line, &[&["a", "3"], &["b", ""], &["c", "5"]]);
    c.mapping.series.truncate(1);
    let g = draw(&c);
    let dots = all(&g, PartRole::Point, 1);
    assert_eq!(dots.len(), 2, "a dot for each lone value");
    let one = on_rows(ChartKind::Line, &[&["a", "3"]]);
    assert!(
        !all(&draw(&one), PartRole::Point, 1).is_empty(),
        "a dot for a one-row chart"
    );
}

/// *Group small slices* never folds every slice: two slices both under the
/// share draw as two slices, with no progress ring's track.
/// flip: `pie_slices` folding when every slice is small → fails at "no track".
#[test]
fn grouping_leaves_a_pie_a_pie() {
    let mut c = on_rows(ChartKind::Pie, &[&["a", "30"], &["b", "40"]]);
    c.options.slices.group_below = Some(60.0);
    let g = draw(&c);
    assert!(part(&g, PartRole::Ground, 0, 0).is_none(), "no track");
    assert_eq!(all(&g, PartRole::Slice, 0).len(), 2);
}

/// A bubble whose size is below zero is left out and said; a scatter's note
/// names the table's row even after a row with no X.
/// flip: `scatter_clouds` naming the row by its place among the kept ones →
/// fails at "Row 2"; drawing a negative size at its absolute value → fails at
/// "a negative size".
#[test]
fn a_scatters_notes_name_the_tables_rows() {
    let mut c = Chart::sample(ChartKind::Scatter);
    c.table.rows = [["", "5", "1"], ["abc", "7", "1"], ["3", "9", "1"]]
        .iter()
        .map(|r| r.iter().map(|c| (*c).to_string()).collect())
        .collect();
    c.mapping.series.truncate(1);
    let g = draw(&c);
    assert!(
        g.left_out
            .iter()
            .any(|n| n == "Row 2: “abc” is not a number"),
        "Row 2: {:?}",
        g.left_out
    );
    let mut b = Chart::sample(ChartKind::Scatter);
    b.mapping.series.truncate(1);
    b.mapping.size = Some(ColumnId(2));
    b.table.rows[1][2] = "-40".into();
    let g = draw(&b);
    assert!(
        g.left_out.iter().any(|n| n == "Row 2: a negative size"),
        "a negative size: {:?}",
        g.left_out
    );
    assert_eq!(all(&g, PartRole::Point, 1).len(), b.table.rows.len() - 1);
}

/// A Sankey with one crowded column still gives its nodes height: the gaps
/// give way.
/// flip: `sankey` keeping the set node gap however many nodes share a column
/// → fails at "taller than 0".
#[test]
fn a_crowded_sankey_column_keeps_its_nodes() {
    let mut c = Chart::sample(ChartKind::Sankey);
    let mut rows: Vec<Vec<String>> = (0..30)
        .map(|i| vec![format!("S{i}"), "Hub".into(), "10".into()])
        .collect();
    rows.push(vec!["Hub".into(), "End".into(), "300".into()]);
    c.table.rows = rows;
    let g = draw(&c);
    let nodes = all(&g, PartRole::SankeyNode, 0);
    assert!(!nodes.is_empty(), "the fixture draws nodes");
    for n in nodes {
        assert!(box_size(n).height > 0.0, "taller than 0: {}", n.name);
    }
}

/// A column chart past the mark limit in categories alone is refused, and a
/// line chart's category ticks are thinned with its labels.
/// flip: `bars` counting only bars → fails at "refused"; `category_labels`
/// marking every category → fails at "thinned".
#[test]
fn categories_count_toward_the_limits() {
    let rows: Vec<Vec<String>> = (0..3_000)
        .map(|i| vec![format!("c{i}"), "1".into()])
        .collect();
    let mut c = Chart::sample(ChartKind::Column);
    c.table.rows = rows
        .iter()
        .map(|r| {
            let mut r = r.clone();
            r.resize(4, String::new());
            r
        })
        .collect();
    c.mapping.series.clear();
    c.options.category_axis.ticks = true;
    assert!(
        matches!(
            generate(&c, kurbo::Size::new(480.0, 280.0), "Inter"),
            Err(Refusal::TooManyMarks {
                what: "categories",
                ..
            })
        ),
        "refused"
    );
    c.kind = ChartKind::Line;
    let g = draw(&c);
    let ticks = all(&g, PartRole::Tick, 1).len();
    let labels = all(&g, PartRole::CategoryLabel, 0).len();
    assert!(
        ticks <= labels.max(1),
        "thinned: {ticks} ticks, {labels} labels"
    );
}

/// A turned value label at the axis's end stays inside its axis frame, so it
/// is not drawn over the legend above the plot.
/// flip: `turned_label` centring every label on its tick → fails at "inside".
#[test]
fn a_turned_end_label_stays_inside_its_axis() {
    let mut c = Chart::sample(ChartKind::Column);
    for r in &mut c.table.rows {
        for cell in &mut r[1..] {
            let v: f64 = cell.parse().unwrap();
            *cell = format!("{}", v * 25_000.0);
        }
    }
    c.options.value_axis.angle = LabelAngle::Upright;
    let g = generate(&c, kurbo::Size::new(300.0, 200.0), "Inter").unwrap();
    let axis = world_box(&g, part(&g, PartRole::ValueAxis, 0, 0).unwrap());
    let labels = all(&g, PartRole::TickLabel, 0);
    assert!(!labels.is_empty(), "the fixture");
    for l in labels {
        let b = world_box(&g, l);
        assert!(
            b.y0 >= axis.y0 - 1e-6 && b.y1 <= axis.y1 + 1e-6,
            "inside: {} at {b:?}, the axis {axis:?}",
            l.name
        );
    }
}

/// An axis title longer than its axis is cut with an ellipsis to the axis's
/// length, so it stays inside the chart.
/// flip: `within` returning the title whole → fails at "inside the chart".
#[test]
fn a_long_axis_title_is_cut_to_its_axis() {
    let mut c = Chart::sample(ChartKind::Column);
    c.options.value_axis.title = "Revenue in thousands of dollars per month".into();
    let size = kurbo::Size::new(300.0, 160.0);
    let g = generate(&c, size, "Inter").unwrap();
    let t = part(&g, PartRole::AxisTitle, 0, 0).expect("a title");
    let b = world_box(&g, t);
    assert!(
        b.y0 >= -1e-6 && b.y1 <= size.height + 1e-6,
        "inside the chart: {b:?}"
    );
    match &t.kind {
        crate::node::NodeKind::Text { content, .. } => assert!(content.ends_with('…')),
        other => panic!("a text: {other:?}"),
    }
}

/// A pie's slices and a grouped scatter's groups draw the colours *From
/// variables* binds by name, and read back as bound.
/// flip: `pie_slices` taking the palette whatever is styled → fails at "the
/// slice draws the binding".
#[test]
fn named_marks_draw_their_bound_colours() {
    let mut ids = crate::IdSource::new(2);
    let vars = [ids.mint_var(), ids.mint_var()];
    let pie = with_variables(&Chart::sample(ChartKind::Pie), &vars);
    assert_eq!(bound_variables(&pie).map(|v| v.len()), Some(6));
    let g = draw(&pie);
    let slices = all(&g, PartRole::Slice, 0);
    assert!(!slices.is_empty());
    for s in slices {
        assert!(
            s.fills.iter().any(|f| f.brush.var().is_some()),
            "the slice draws the binding: {}",
            s.name
        );
    }
    let mut scatter = Chart::sample(ChartKind::Scatter);
    scatter.mapping.series.truncate(1);
    scatter.mapping.group = Some(ColumnId(2));
    let scatter = with_variables(&scatter, &vars);
    let g = draw(&scatter);
    let points = g
        .parts
        .iter()
        .filter(|p| p.key.role == PartRole::Point)
        .collect::<Vec<_>>();
    assert!(!points.is_empty());
    assert!(
        points
            .iter()
            .all(|p| p.fills.iter().any(|f| f.brush.var().is_some())),
        "the groups draw the binding"
    );
}
