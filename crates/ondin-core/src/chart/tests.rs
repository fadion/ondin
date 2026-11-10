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
        chart.options = Preset::Outline.apply(&chart.options);
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
