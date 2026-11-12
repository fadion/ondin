//! A chart's model: the description on a frame, the parts only its regeneration
//! writes, where a chart may sit, flatten, the save format and the clipboard.

use ondin_core::chart::{
    Chart, ChartKind, ChartRule, Column, ColumnId, DataLink, Mapping, PartKey, PartRole, Table,
};
use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::{
    Document, History, IdSource, NodeId, NodeKind, OpError, Operation, Transaction, io,
};

fn frame(w: f64, h: f64) -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(w, h),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    }
}

fn rect() -> NodeKind {
    NodeKind::Rect {
        size: Size::new(10.0, 10.0),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    }
}

fn create(id: NodeId, parent: NodeId, kind: NodeKind) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index: 0,
        kind,
        transform: Some(Affine::IDENTITY),
        name: None,
    }
}

/// Two series over three months, its parts not yet generated.
fn sample() -> Chart {
    let col = |id, name: &str| Column {
        id: ColumnId(id),
        name: name.into(),
    };
    let row = |m: &str, a: &str, b: &str| vec![m.to_string(), a.to_string(), b.to_string()];
    Chart {
        kind: ChartKind::Column,
        table: Table {
            columns: vec![col(0, "Month"), col(1, "Revenue"), col(2, "Costs")],
            rows: vec![
                row("Jan", "42", "30"),
                row("Feb", "48", "32"),
                row("Mar", "1,045", "34"),
            ],
        },
        mapping: Mapping {
            category: Some(ColumnId(0)),
            series: vec![ColumnId(1), ColumnId(2)],
            ..Mapping::default()
        },
        series: Vec::new(),
        nodes: Vec::new(),
        options: Default::default(),
        source: None,
        parts: Vec::new(),
        generated: None,
        minted: 0,
    }
}

fn key(role: PartRole, index: u32) -> PartKey {
    PartKey {
        role,
        series: 1,
        index,
    }
}

/// A page holding *Chart*, a chart frame with one part *Bar* made by a
/// regeneration, and *Card*, a plain frame.
struct F {
    doc: Document,
    history: History,
    page: NodeId,
    chart: NodeId,
    bar: NodeId,
    card: NodeId,
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xC4);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let page = doc.first_page();
    let [chart, bar, card] = [(); 3].map(|_| ids.mint());
    let mut history = History::default();
    history
        .commit(
            &mut doc,
            Transaction(vec![
                create(chart, page, frame(480.0, 280.0)),
                Operation::SetChart {
                    id: chart,
                    chart: Some(Box::new(sample())),
                },
                create(bar, chart, rect()),
                Operation::SetChartParts {
                    id: chart,
                    parts: vec![(key(PartRole::Bar, 0), bar)],
                    at: None,
                },
                create(card, page, frame(100.0, 100.0)),
            ]),
        )
        .expect("a regeneration writes its parts");
    F {
        doc,
        history,
        page,
        chart,
        bar,
        card,
    }
}

fn refused(doc: &mut Document, ops: Vec<Operation>) -> OpError {
    let before = io::save(doc).unwrap();
    let err = doc.apply(&Transaction(ops)).expect_err("refused");
    assert_eq!(
        io::save(doc).unwrap(),
        before,
        "a refusal leaves the document"
    );
    err
}

/// Every write to a part, a child added to a chart frame or one moved into it,
/// and the frame's own layout, is refused outside the chart's regeneration, with
/// the sentence that says what to do instead; the same write inside one lands.
/// flip: `chart::guard` answering `Ok` → fails at "a part's fill".
#[test]
fn a_part_is_written_only_by_its_charts_regeneration() {
    let mut f = fixture();
    let mut ids = IdSource::new(0x99);
    let stray = ids.mint();
    let fills = Operation::SetFills {
        id: f.bar,
        fills: Default::default(),
    };
    for (what, ops) in [
        ("a part's fill", vec![fills.clone()]),
        ("a part deleted", vec![Operation::DeleteNode { id: f.bar }]),
        (
            "a part moved out",
            vec![Operation::Reparent {
                id: f.bar,
                new_parent: f.card,
                index: 0,
            }],
        ),
        ("a child added", vec![create(stray, f.chart, rect())]),
        (
            "the frame's layout",
            vec![Operation::SetDisplay {
                id: f.chart,
                display: None,
            }],
        ),
    ] {
        let err = refused(&mut f.doc, ops);
        assert!(
            matches!(err, OpError::ChartPart(_)),
            "{what}: refused as a part, got {err:?}"
        );
        assert_eq!(
            err.to_string(),
            "Part of a chart — edit the chart, or flatten it first"
        );
    }
    f.history
        .commit(
            &mut f.doc,
            Transaction(vec![
                fills,
                Operation::SetChartParts {
                    id: f.chart,
                    parts: vec![(key(PartRole::Bar, 0), f.bar)],
                    at: None,
                },
            ]),
        )
        .expect("the same fill in a regeneration");
    f.history
        .undo(&mut f.doc)
        .expect("a regeneration undoes")
        .expect("something to undo");
}

/// A new description keeps the chart's parts, so undoing it after a
/// regeneration (which runs outside history) leaves the parts that regeneration
/// wrote rather than a list naming nodes it deleted.
/// flip: `SetChart` taking the parts it is given → fails at "the regeneration's
/// parts".
#[test]
fn a_description_keeps_the_parts_a_regeneration_wrote() {
    let mut f = fixture();
    let mut bar_kind = sample();
    bar_kind.kind = ChartKind::Bar;
    f.history
        .commit(
            &mut f.doc,
            Transaction(vec![Operation::SetChart {
                id: f.chart,
                chart: Some(Box::new(bar_kind)),
            }]),
        )
        .unwrap();
    let new_bar = IdSource::new(0x55).mint();
    f.doc
        .apply(&Transaction(vec![
            Operation::DeleteNode { id: f.bar },
            create(new_bar, f.chart, rect()),
            Operation::SetChartParts {
                id: f.chart,
                parts: vec![(key(PartRole::Bar, 0), new_bar)],
                at: None,
            },
        ]))
        .expect("a regeneration replaces the part");
    f.history.undo(&mut f.doc).unwrap().unwrap();
    let chart = f.doc.get(f.chart).unwrap().chart().unwrap();
    assert_eq!(chart.kind, ChartKind::Column, "the description is undone");
    assert_eq!(
        chart.parts,
        vec![(key(PartRole::Bar, 0), new_bar)],
        "the regeneration's parts"
    );
}

/// The chart frame itself is a frame like any other: it moves, renames and is
/// deleted whole, its parts with it, and the delete undoes.
#[test]
fn the_chart_frame_moves_and_deletes_whole() {
    let mut f = fixture();
    f.history
        .commit(
            &mut f.doc,
            Transaction(vec![
                Operation::SetTransform {
                    id: f.chart,
                    transform: Affine::translate((40.0, 0.0)),
                },
                Operation::SetName {
                    id: f.chart,
                    name: "Revenue".into(),
                },
            ]),
        )
        .expect("the frame's own fields");
    f.history
        .commit(
            &mut f.doc,
            Transaction(vec![Operation::DeleteNode { id: f.chart }]),
        )
        .expect("deleted whole");
    assert!(f.doc.get(f.bar).is_none());
    f.history.undo(&mut f.doc).unwrap().unwrap();
    assert!(f.doc.get(f.bar).is_some(), "and back, parts and all");
}

/// A chart frame never converts to a group, which could not hold its
/// description; its Frame / Group dropdown is disabled with the reason.
#[test]
fn a_chart_stays_a_frame() {
    let f = fixture();
    assert_eq!(
        ondin_core::build::convert_container_refusal(&f.doc, f.chart),
        Some("A chart stays a frame: flatten it first")
    );
    assert!(!ondin_core::build::can_convert_container(&f.doc, f.chart));
}

/// A chart can't be part of a component, an instance, a boolean or a mask, and
/// sits only on a frame.
/// flip: `chart::check` answering `Ok` → fails at "made a main".
#[test]
fn a_chart_sits_only_on_a_frame_outside_any_component() {
    let mut f = fixture();
    let err = refused(
        &mut f.doc,
        vec![Operation::SetComponent {
            id: f.chart,
            component: true,
        }],
    );
    assert!(
        matches!(err, OpError::ChartPlacement(..)),
        "made a main: {err:?}"
    );
    assert!(err.to_string().contains("can't be part of a component"));
    let err = refused(
        &mut f.doc,
        vec![
            Operation::SetComponent {
                id: f.card,
                component: true,
            },
            Operation::Reparent {
                id: f.chart,
                new_parent: f.card,
                index: 0,
            },
        ],
    );
    assert!(
        matches!(err, OpError::ChartPlacement(..)),
        "moved into a main: {err:?}"
    );
    let loose = IdSource::new(0x77).mint();
    f.doc
        .apply(&Transaction(vec![create(loose, f.page, rect())]))
        .unwrap();
    let err = refused(
        &mut f.doc,
        vec![Operation::SetChart {
            id: loose,
            chart: Some(Box::new(sample())),
        }],
    );
    assert!(matches!(err, OpError::WrongKindForOp), "on a rect: {err:?}");
}

/// A description `Chart::validate` refuses is refused by the operation.
/// flip: `validate` not checking `mapping.series` for repeats → fails at
/// "refused".
#[test]
fn a_description_out_of_shape_is_refused() {
    let mut f = fixture();
    let mut ragged = sample();
    ragged.table.rows[1].pop();
    let mut unknown = sample();
    unknown.mapping.series.push(ColumnId(9));
    let mut long = sample();
    long.table.rows[0][0] = "x".repeat(257);
    let mut tall = sample();
    tall.table.rows = vec![vec!["a".into(), "1".into(), "2".into()]; 10_001];
    let mut odd = sample();
    odd.options.lines.area_opacity = 2.0;
    let mut twice = sample();
    twice.mapping.series.push(ColumnId(1));
    for (chart, rule) in [
        (ragged, ChartRule::RaggedRow),
        (unknown, ChartRule::UnknownColumn),
        (twice, ChartRule::DuplicateSeries),
        (long, ChartRule::CellTooLong),
        (tall, ChartRule::TooLarge),
        (odd, ChartRule::BadOption),
    ] {
        let err = refused(
            &mut f.doc,
            vec![Operation::SetChart {
                id: f.card,
                chart: Some(Box::new(chart)),
            }],
        );
        assert!(
            matches!(err, OpError::BadChart(r) if r == rule),
            "{rule:?}: {err:?}"
        );
    }
}

/// Flatten drops the description and leaves the parts as ordinary layers, which
/// any edit then reaches; undo brings the chart back.
#[test]
fn flatten_leaves_the_parts_and_undoes() {
    let mut f = fixture();
    let op = ondin_core::chart::flatten(&f.doc, f.chart).expect("a chart");
    f.history
        .commit(&mut f.doc, Transaction(vec![op]))
        .expect("flattened");
    assert!(f.doc.get(f.chart).unwrap().chart().is_none());
    f.history
        .commit(
            &mut f.doc,
            Transaction(vec![Operation::SetFills {
                id: f.bar,
                fills: Default::default(),
            }]),
        )
        .expect("a former part is a layer");
    f.history.undo(&mut f.doc).unwrap().unwrap();
    f.history.undo(&mut f.doc).unwrap().unwrap();
    let chart = f.doc.get(f.chart).unwrap().chart().expect("back");
    assert_eq!(chart.parts, vec![(key(PartRole::Bar, 0), f.bar)]);
    assert!(ondin_core::chart::flatten(&f.doc, f.card).is_none());
}

/// The description saves and loads whole; a file whose chart is out of shape,
/// or whose chart is not on a frame, is refused rather than loaded as shapes.
#[test]
fn a_chart_round_trips_and_a_bad_one_refuses_the_file() {
    let f = fixture();
    let bytes = io::save(&f.doc).unwrap();
    let loaded = io::load(&bytes).unwrap();
    assert_eq!(
        loaded.get(f.chart).unwrap().chart(),
        f.doc.get(f.chart).unwrap().chart()
    );
    assert_eq!(io::save(&loaded).unwrap(), bytes, "byte-identical");

    let text = String::from_utf8(bytes).unwrap();
    assert!(
        text.contains(&format!("\"{}\"", f.bar.to_wire())),
        "a part by its wire id"
    );
    let mut v: serde_json::Value = serde_json::from_str(&text).unwrap();
    let edited = |v: &mut serde_json::Value, edit: &dyn Fn(&mut serde_json::Value)| {
        let mut v = v.clone();
        for n in v["nodes"].as_array_mut().unwrap() {
            if n.get("chart").is_some() {
                edit(n);
            }
        }
        serde_json::to_string(&v).unwrap()
    };
    let ragged = edited(&mut v, &|n| {
        n["chart"]["table"]["rows"][1].as_array_mut().unwrap().pop();
    });
    assert!(io::load(ragged.as_bytes()).is_err(), "a ragged table");
    let on_a_rect = edited(&mut v, &|n| {
        n["kind"] = serde_json::json!({"Rect": {"size": {"width": 1.0, "height": 1.0}}});
    });
    assert!(io::load(on_a_rect.as_bytes()).is_err(), "a chart on a rect");
}

/// A copy carries the description and its table but never the linked file, at
/// either door: the write leaves it behind, and the read drops one a hand-made
/// copy carries.
#[test]
fn a_copy_drops_the_linked_file() {
    let mut f = fixture();
    let mut linked = f.doc.get(f.chart).unwrap().chart().unwrap().clone();
    linked.source = Some(DataLink {
        path: "C:\\data\\sales.csv".into(),
        size: 10,
        modified: 1,
        header: None,
    });
    f.history
        .commit(
            &mut f.doc,
            Transaction(vec![Operation::SetChart {
                id: f.chart,
                chart: Some(Box::new(linked)),
            }]),
        )
        .unwrap();
    let nodes = f.doc.capture_subtree(f.chart).unwrap();
    let text = io::clip::write(std::slice::from_ref(&nodes), &[], None, "Chart").unwrap();
    assert!(!text.contains("sales.csv"), "left behind on write");
    let pasted = io::clip::read(&text).unwrap().unwrap();
    let chart = pasted.subtrees[0][0]
        .chart()
        .expect("the description travels");
    assert_eq!(chart.table, sample().table);
    assert!(chart.source.is_none());

    let forged = text.replacen(
        "\"parts\":",
        "\"source\":{\"path\":\"C:\\\\x.csv\"},\"parts\":",
        1,
    );
    assert_ne!(forged, text, "the fixture: a source was planted");
    let pasted = io::clip::read(&forged).unwrap().unwrap();
    assert!(
        pasted.subtrees[0][0].chart().unwrap().source.is_none(),
        "dropped on read"
    );
}

/// A duplicated chart's part keys follow the copies' new ids.
#[test]
fn a_remapped_chart_names_its_copied_parts() {
    let f = fixture();
    let nodes = f.doc.capture_subtree(f.chart).unwrap();
    let mut ids = IdSource::new(0x5000);
    let (copy, root) = ondin_core::document::remap_subtree(&nodes, &mut ids).unwrap();
    let chart = copy
        .iter()
        .find(|n| n.id() == root)
        .unwrap()
        .chart()
        .unwrap();
    let bar = copy.iter().find(|n| n.parent() == Some(root)).unwrap().id();
    assert_eq!(chart.parts, vec![(key(PartRole::Bar, 0), bar)]);
    let _ = f.page;
}

/// A page holding one chart made from `sample()`, not yet settled.
fn unsettled(w: f64, h: f64) -> (Document, NodeId) {
    let mut ids = IdSource::new(0xC5);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let chart = ids.mint();
    doc.apply(&Transaction(vec![
        create(chart, doc.first_page(), frame(w, h)),
        Operation::SetChart {
            id: chart,
            chart: Some(Box::new(sample())),
        },
    ]))
    .unwrap();
    (doc, chart)
}

fn part(doc: &Document, chart: NodeId, role: PartRole, series: u32, index: u32) -> Option<NodeId> {
    let k = PartKey {
        role,
        series,
        index,
    };
    doc.get(chart)?
        .chart()?
        .parts
        .iter()
        .find(|(p, _)| *p == k)
        .map(|(_, id)| *id)
}

/// Settle makes a chart's parts, stamps the size, and has nothing more to do;
/// the parts saved and loaded are the same parts.
#[test]
fn settle_makes_the_parts_once() {
    let (mut doc, chart) = unsettled(480.0, 280.0);
    let mut res = ondin_core::Resolved::rebuild(&doc);
    assert_eq!(ondin_core::chart::stale(&doc, &res), vec![chart]);
    let dirty = ondin_core::chart::settle_all(&mut doc, &mut res);
    assert!(!dirty.0.is_empty(), "parts were made");
    assert!(
        ondin_core::chart::stale(&doc, &res).is_empty(),
        "and stamped"
    );
    let c = doc.get(chart).unwrap().chart().unwrap();
    assert_eq!(c.generated, Some([480.0, 280.0]));
    // Two series over three months: six bars, each where the table says.
    let bars: Vec<_> = c
        .parts
        .iter()
        .filter(|(k, _)| k.role == PartRole::Bar)
        .collect();
    assert_eq!(bars.len(), 6);
    let bar = part(&doc, chart, PartRole::Bar, 1, 2).expect("Revenue · Mar");
    assert_eq!(doc.get(bar).unwrap().name(), "Revenue · Mar");
    let fresh = ondin_core::Resolved::rebuild(&doc);
    assert_eq!(
        res.world_bounds(bar),
        fresh.world_bounds(bar),
        "settled equals rebuilt"
    );
    let again = io::load(&io::save(&doc).unwrap()).unwrap();
    assert!(ondin_core::chart::stale(&again, &ondin_core::Resolved::rebuild(&again)).is_empty());
}

/// **Laid out, not placed**: resized and not regenerated, a bar reflows to
/// where regeneration at the new size puts it, and regeneration keeps its id.
/// flip: a bar written without its percentage insets → fails at "the bar
/// moved and widened".
#[test]
fn a_resized_chart_reflows_before_it_regenerates() {
    let (mut doc, chart) = unsettled(480.0, 280.0);
    let mut res = ondin_core::Resolved::rebuild(&doc);
    ondin_core::chart::settle_all(&mut doc, &mut res);
    let bar = part(&doc, chart, PartRole::Bar, 2, 1).expect("Costs · Feb");
    let before = res.world_bounds(bar).unwrap();
    // Wider only: the value axis's ticks do not change, so nothing but the
    // bands moves.
    doc.apply(&Transaction(vec![Operation::SetGeometry {
        id: chart,
        geometry: ondin_core::GeometryPatch::Size(Size::new(720.0, 280.0)),
    }]))
    .unwrap();
    let res = ondin_core::Resolved::rebuild(&doc);
    let reflowed = res.world_bounds(bar).unwrap();
    assert!(
        reflowed.width() > before.width() * 1.3 && reflowed.x0 > before.x0 * 1.3,
        "the fixture: the bar moved and widened, {before:?} to {reflowed:?}"
    );
    assert_eq!(ondin_core::chart::stale(&doc, &res), vec![chart]);
    let mut res = res;
    ondin_core::chart::settle_all(&mut doc, &mut res);
    assert_eq!(
        part(&doc, chart, PartRole::Bar, 2, 1),
        Some(bar),
        "the bar keeps its id"
    );
    let regenerated = res.world_bounds(bar).unwrap();
    let near = |a: f64, b: f64| (a - b).abs() < 0.01;
    assert!(
        near(reflowed.x0, regenerated.x0)
            && near(reflowed.x1, regenerated.x1)
            && near(reflowed.y0, regenerated.y0)
            && near(reflowed.y1, regenerated.y1),
        "the layout is the preview: {reflowed:?} reflowed, {regenerated:?} regenerated"
    );
}

/// A line is a path pinned on both edges of the plot by its own box: resized
/// and not regenerated, it stretches to the path regeneration draws, point for
/// point, its stroke unscaled.
/// flip: `inset_stretch` answering no for a path → fails at "every point".
#[test]
fn a_line_stretches_to_the_path_regeneration_draws() {
    let (mut doc, chart) = unsettled(480.0, 280.0);
    let mut c = doc.get(chart).unwrap().chart().unwrap().clone();
    c.kind = ChartKind::Line;
    doc.apply(&Transaction(vec![Operation::SetChart {
        id: chart,
        chart: Some(Box::new(c)),
    }]))
    .unwrap();
    let mut res = ondin_core::Resolved::rebuild(&doc);
    ondin_core::chart::settle_all(&mut doc, &mut res);
    let line = part(&doc, chart, PartRole::Line, 1, 0).expect("Line · Revenue");
    let drawn = |doc: &Document, res: &ondin_core::Resolved| -> Vec<ondin_core::kurbo::Point> {
        let NodeKind::Path { path, .. } = res.used_kind(doc, line).unwrap() else {
            panic!("a path");
        };
        let world = res.world_transform(line).unwrap();
        path.elements()
            .iter()
            .filter_map(|e| e.end_point())
            .map(|p| world * p)
            .collect()
    };
    doc.apply(&Transaction(vec![Operation::SetGeometry {
        id: chart,
        geometry: ondin_core::GeometryPatch::Size(Size::new(720.0, 280.0)),
    }]))
    .unwrap();
    res = ondin_core::Resolved::rebuild(&doc);
    let reflowed = drawn(&doc, &res);
    ondin_core::chart::settle_all(&mut doc, &mut res);
    assert_eq!(part(&doc, chart, PartRole::Line, 1, 0), Some(line), "kept");
    let regenerated = drawn(&doc, &res);
    assert_eq!(reflowed.len(), regenerated.len());
    assert!(
        reflowed
            .iter()
            .zip(&regenerated)
            .all(|(a, b)| (*a - *b).hypot() < 0.01),
        "every point: {reflowed:?} reflowed, {regenerated:?} regenerated"
    );
    let NodeKind::Path { .. } = doc.get(line).unwrap().kind() else {
        panic!("a path");
    };
    assert_eq!(
        doc.get(line).unwrap().paint().strokes[0].value.width,
        2.0,
        "the stroke stays its width"
    );
}

/// A Sankey adds up same-named flows, leaves out a row it cannot draw and says
/// why, and refuses a table whose flows loop back, naming the loop.
/// flip: `find_loop` answering `None` → fails at "the loop is refused".
#[test]
fn a_sankey_adds_flows_leaves_out_rows_and_refuses_a_loop() {
    use ondin_core::chart::{Refusal, generate};
    let mut c = sample();
    c.kind = ChartKind::Sankey;
    c.table.rows = [
        ["Search", "Signup", "520"],
        ["Search", "Signup", "480"],
        ["Signup", "Trial", "900"],
        ["Trial", "Trial", "5"],
        ["Signup", "Leave", "-3"],
    ]
    .iter()
    .map(|r| r.iter().map(|s| s.to_string()).collect())
    .collect();
    c.mapping = Mapping {
        from: Some(ColumnId(0)),
        to: Some(ColumnId(1)),
        value: Some(ColumnId(2)),
        ..Mapping::default()
    };
    let g = generate(&c, Size::new(480.0, 280.0), "Inter").expect("drawn");
    let flows = g
        .parts
        .iter()
        .filter(|p| p.key.role == PartRole::Flow)
        .count();
    assert_eq!(flows, 2, "the two Search → Signup rows are one flow");
    assert_eq!(
        g.left_out.len(),
        2,
        "a flow into itself and one below zero: {:?}",
        g.left_out
    );
    c.table
        .rows
        .push(vec!["Trial".into(), "Search".into(), "10".into()]);
    assert_eq!(
        generate(&c, Size::new(480.0, 280.0), "Inter"),
        Err(Refusal::LoopsBack(
            "Search → Signup → Trial → Search".into()
        )),
        "the loop is refused"
    );
}

/// A chart's variant is named from the options of its kind's family.
#[test]
fn a_variant_is_named_from_its_kinds_options() {
    let mut c = sample();
    assert_eq!(c.variant(), Some("Grouped"));
    c.options.bars.stacking = ondin_core::chart::Stacking::Percent;
    assert_eq!(c.variant(), Some("100%"));
    c.kind = ChartKind::Area;
    assert_eq!(c.variant(), Some("Overlapping"), "an area's own stacking");
    c.kind = ChartKind::Pie;
    c.options.slices.donut = true;
    assert_eq!(c.variant(), Some("Donut"));
    c.kind = ChartKind::Scatter;
    c.mapping.size = Some(ColumnId(2));
    assert_eq!(c.variant(), Some("Bubble"));
    c.kind = ChartKind::Sankey;
    assert_eq!(c.variant(), None);
}

/// A replaced table keeps the mapping and the series style of each column whose
/// name survives, gives the rest fresh ids, and drops what names a lost column;
/// a mapping of which nothing survives is guessed.
///
/// flip: `Chart::with_table` matching columns by place instead of name → fails
/// at "Revenue keeps its id".
#[test]
fn a_replaced_table_keeps_the_columns_whose_names_survive() {
    use ondin_core::chart::SeriesStyle;
    let col = |id, name: &str| Column {
        id: ColumnId(id),
        name: name.into(),
    };
    let mut c = sample();
    let style = |column| SeriesStyle {
        column,
        fill: ondin_core::Brush::Solid(ondin_core::peniko::Color::BLACK).into(),
        visible: true,
        dash: Default::default(),
        point: Default::default(),
    };
    c.series = vec![style(ColumnId(1)), style(ColumnId(2))];
    let table = Table {
        columns: vec![col(0, "Profit"), col(1, "Month"), col(2, "Revenue")],
        rows: vec![vec!["3".into(), "Apr".into(), "50".into()]],
    };
    let got = c.with_table(table);
    assert_eq!(got.validate(), Ok(()));
    let id = |name: &str| {
        got.table
            .columns
            .iter()
            .find(|x| x.name == name)
            .unwrap()
            .id
    };
    assert_eq!(id("Revenue"), ColumnId(1), "Revenue keeps its id");
    assert_eq!(id("Month"), ColumnId(0));
    assert!(id("Profit").0 > 2, "a new column takes a fresh id");
    assert_eq!(got.mapping.category, Some(ColumnId(0)));
    assert_eq!(got.mapping.series, [ColumnId(1)], "Costs is gone");
    assert_eq!(got.series, [style(ColumnId(1))]);

    let fresh = Table {
        columns: vec![col(0, "City"), col(1, "Sales"), col(2, "Visits")],
        rows: vec![vec!["Oslo".into(), "4".into(), "9".into()]],
    };
    let got = c.with_table(fresh);
    assert_eq!(got.validate(), Ok(()));
    let id = |name: &str| {
        got.table
            .columns
            .iter()
            .find(|x| x.name == name)
            .unwrap()
            .id
    };
    assert_eq!(got.mapping.category, Some(id("City")), "guessed");
    assert_eq!(got.mapping.series, [id("Sales"), id("Visits")]);
    assert!(got.series.is_empty());
}

/// A guessed mapping reads the column types: a text category and number series,
/// a scatter's numeric X, a Sankey's two text columns and a number.
#[test]
fn a_mapping_is_guessed_from_the_column_types() {
    let col = |id, name: &str| Column {
        id: ColumnId(id),
        name: name.into(),
    };
    let t = Table {
        columns: vec![col(0, "Year"), col(1, "From"), col(2, "To"), col(3, "N")],
        rows: vec![vec!["2020".into(), "A".into(), "B".into(), "5".into()]],
    };
    let m = Mapping::guess(ChartKind::Column, &t);
    assert_eq!(m.category, Some(ColumnId(1)));
    assert_eq!(m.series, [ColumnId(0), ColumnId(3)]);
    let m = Mapping::guess(ChartKind::Scatter, &t);
    assert_eq!(
        (m.category, m.series),
        (Some(ColumnId(0)), vec![ColumnId(3)])
    );
    let m = Mapping::guess(ChartKind::Sankey, &t);
    assert_eq!(
        (m.from, m.to, m.value),
        (Some(ColumnId(1)), Some(ColumnId(2)), Some(ColumnId(0)))
    );
}

/// A settled chart whose two series are bound *From variables* to `brand/a`
/// and `brand/b` (red and blue), and the variables.
fn bound_chart() -> (
    Document,
    ondin_core::Resolved,
    NodeId,
    [ondin_core::VarId; 2],
) {
    use ondin_core::variable::{Cell, CollectionId, VarKind, Variable};
    let (mut doc, chart) = unsettled(480.0, 280.0);
    let mut ids = IdSource::new(0xC6);
    let light = ids.mint_mode();
    let vars = [ids.mint_var(), ids.mint_var()];
    let var = |id, name: &str, c: ondin_core::peniko::Color| Operation::AddVariable {
        var: Variable {
            id,
            name: name.into(),
            kind: VarKind::Color,
            cells: [(light, Cell::Color(c))].into(),
            scope: None,
            origin: None,
            forked: None,
            description: String::new(),
        },
    };
    let bound = ondin_core::chart::with_variables(doc.get(chart).unwrap().chart().unwrap(), &vars);
    doc.apply(&Transaction(vec![
        Operation::AddMode {
            collection: CollectionId::FIRST,
            mode: ondin_core::Mode {
                id: light,
                name: "Light".into(),
            },
            index: 0,
            cells: vec![],
        },
        var(
            vars[0],
            "brand/a",
            ondin_core::peniko::Color::from_rgb8(255, 0, 0),
        ),
        var(
            vars[1],
            "brand/b",
            ondin_core::peniko::Color::from_rgb8(0, 0, 255),
        ),
        Operation::SetChart {
            id: chart,
            chart: Some(Box::new(bound)),
        },
    ]))
    .expect("a chart bound to two colour variables");
    let mut res = ondin_core::Resolved::rebuild(&doc);
    ondin_core::chart::settle_all(&mut doc, &mut res);
    (doc, res, chart, vars)
}

/// A chart's colours bind: its parts draw the binding, the chart frame (not
/// its parts) is the variable's one user, and the commit takes it.
#[test]
fn a_charts_colours_bind_to_variables() {
    let (doc, _, chart, vars) = bound_chart();
    let bar = part(&doc, chart, PartRole::Bar, 1, 0).expect("Revenue · Jan");
    assert_eq!(
        doc.get(bar).unwrap().paint().fills[0].brush.var(),
        Some(vars[0]),
        "the bar draws the binding"
    );
    assert_eq!(ondin_core::variable::users(&doc, vars[0]), vec![chart]);
}

/// Deleting a variable a chart's colour reads detaches the description to the
/// colour it drew, in one step the commit takes, and settle remakes the parts
/// in it; undo brings the variable and the binding back.
/// flip: `detach_ops` leaving chart frames alone → fails at the commit.
#[test]
fn deleting_a_charts_variable_detaches_it_whole() {
    let (mut doc, _, chart, vars) = bound_chart();
    let red = ondin_core::peniko::Color::from_rgb8(255, 0, 0);
    let tx = Transaction(ondin_core::variable::remove_ops(&doc, &[vars[0]]));
    let undo = doc.apply(&tx).expect("the delete commits");
    let mut res = ondin_core::Resolved::rebuild(&doc);
    ondin_core::chart::settle_all(&mut doc, &mut res);
    let bar = part(&doc, chart, PartRole::Bar, 1, 0).unwrap();
    let c = doc.get(chart).unwrap().chart().unwrap();
    assert_eq!(
        ondin_core::chart::series_fill(c, ColumnId(1)).and_then(|b| b.literal().cloned()),
        Some(ondin_core::Brush::Solid(red)),
        "the description holds the colour it drew"
    );
    assert_eq!(
        doc.get(bar).unwrap().paint().fills[0]
            .brush
            .literal()
            .cloned(),
        Some(ondin_core::Brush::Solid(red)),
        "and so does the bar"
    );
    assert_eq!(
        ondin_core::chart::series_fill(c, ColumnId(2)).and_then(|b| b.var()),
        Some(vars[1]),
        "the other binding stays"
    );
    doc.apply(&undo.inverse).expect("undo");
    let c = doc.get(chart).unwrap().chart().unwrap();
    assert_eq!(
        ondin_core::chart::series_fill(c, ColumnId(1)).and_then(|b| b.var()),
        Some(vars[0])
    );
}

/// Every `(key, id)` chart `id` holds now.
fn parts_of(doc: &Document, chart: NodeId) -> Vec<(PartKey, NodeId)> {
    doc.get(chart).unwrap().chart().unwrap().parts.clone()
}

/// A variable's delete writes only the chart's description, so a later
/// regeneration that remakes the parts under new ids leaves its undo standing;
/// and a part id a regeneration deleted is never minted again for another part.
/// flip: `rebind_ops` writing the parts' fills again → fails at "the delete
/// undoes"; `settle_round` reserving past only the ids the document holds →
/// fails at "never re-minted".
#[test]
fn a_variables_delete_undoes_after_its_chart_is_remade() {
    let (mut doc, _, chart, vars) = bound_chart();
    let mut history = History::default();
    let mut seen: Vec<(PartKey, NodeId)> = parts_of(&doc, chart);
    let settle = |doc: &mut Document, seen: &mut Vec<(PartKey, NodeId)>| {
        let mut res = ondin_core::Resolved::rebuild(doc);
        ondin_core::chart::settle_all(doc, &mut res);
        for (k, id) in parts_of(doc, chart) {
            let other = seen.iter().find(|(k2, id2)| *id2 == id && *k2 != k);
            assert!(
                other.is_none(),
                "{id:?} never re-minted: was {other:?}, now {k:?}"
            );
            seen.push((k, id));
        }
    };
    let delete = Transaction(ondin_core::variable::remove_ops(&doc, &[vars[0]]));
    history
        .commit(&mut doc, delete)
        .expect("the delete commits");
    settle(&mut doc, &mut seen);
    let bar = part(&doc, chart, PartRole::Bar, 1, 0).unwrap();
    assert_eq!(
        doc.get(bar).unwrap().paint().fills[0].brush.var(),
        None,
        "settled, the bar draws the detached colour"
    );
    let mut fewer = doc.get(chart).unwrap().chart().unwrap().clone();
    fewer.table.rows.pop();
    history
        .commit(
            &mut doc,
            Transaction(vec![Operation::SetChart {
                id: chart,
                chart: Some(Box::new(fewer)),
            }]),
        )
        .unwrap();
    settle(&mut doc, &mut seen);
    history.undo(&mut doc).unwrap().unwrap();
    settle(&mut doc, &mut seen);
    if let Err(e) = history.undo(&mut doc) {
        panic!("the delete undoes: {e}");
    }
    settle(&mut doc, &mut seen);
    let bar = part(&doc, chart, PartRole::Bar, 1, 0).unwrap();
    assert_eq!(
        doc.get(bar).unwrap().paint().fills[0].brush.var(),
        Some(vars[0]),
        "the binding is back on the bar"
    );
}

/// A variable scoped to fills binds a line chart's series, whose parts draw
/// it as a stroke: the scope is judged on the description, not the parts.
/// flip: `variable::check` judging the parts' scope too → the regeneration is refused.
#[test]
fn a_fill_scoped_colour_draws_a_charts_lines() {
    let (mut doc, mut res, chart, vars) = bound_chart();
    let mut v = doc.variables().get(vars[0]).unwrap().clone();
    v.scope = Some(ondin_core::variable::Scope::of([
        ondin_core::variable::Field::Fill,
    ]));
    let mut line = doc.get(chart).unwrap().chart().unwrap().clone();
    line.kind = ChartKind::Line;
    doc.apply(&Transaction(vec![
        Operation::SetVariable { var: v },
        Operation::SetChart {
            id: chart,
            chart: Some(Box::new(line)),
        },
    ]))
    .expect("a fill colour binds a chart");
    ondin_core::chart::settle_all(&mut doc, &mut res);
    let k = PartKey {
        role: PartRole::Line,
        series: 1,
        index: 0,
    };
    let c = doc.get(chart).unwrap().chart().unwrap();
    let line = c
        .parts
        .iter()
        .find(|(p, _)| *p == k)
        .map(|(_, id)| *id)
        .unwrap();
    assert_eq!(
        doc.get(line).unwrap().paint().strokes[0].brush.var(),
        Some(vars[0]),
        "the lines draw the binding"
    );
}

/// A chart leaving its document detaches its colours, description and parts.
#[test]
fn a_detached_chart_holds_its_colours() {
    let (doc, _, chart, _) = bound_chart();
    let node = ondin_core::variable::detached(&doc, doc.get(chart).unwrap().clone());
    assert!(node.chart().unwrap().brushes().all(|b| !b.is_bound()));
}

/// Colour review's census leaves a chart's parts out: a merge may not write
/// them. The chart frame's own ground stays in.
#[test]
fn colour_review_leaves_a_charts_parts_out() {
    let (doc, _, chart, _) = bound_chart();
    let roots = ondin_core::color_review::roots(&doc, None);
    let census = ondin_core::color_review::census(&doc, &roots);
    let parts: Vec<NodeId> = doc
        .get(chart)
        .unwrap()
        .chart()
        .unwrap()
        .parts
        .iter()
        .map(|(_, id)| *id)
        .collect();
    assert!(!parts.is_empty());
    assert!(
        census
            .iter()
            .all(|s| s.nodes.iter().all(|n| !parts.contains(n))),
        "no swatch names a part"
    );
}

/// Replace all, *Replace* font and a Group colours recolour over a page holding
/// a chart write the other layers and leave the parts to the chart: a family the
/// chart sets is rewritten in its description.
/// flip: `find::find` searching under a chart frame → fails at "replace all";
/// `families::text_layers` listing parts → fails at "the chart is the family's
/// one layer";
/// `build::recolor` walking `subtree_nodes` → fails at "recolour".
#[test]
fn page_wide_edits_leave_a_charts_parts_to_it() {
    use ondin_core::find::{Query, Scope};
    let (mut doc, chart) = unsettled(480.0, 280.0);
    let mut c = doc.get(chart).unwrap().chart().unwrap().clone();
    c.options.text.family = "Gone Sans".into();
    doc.apply(&Transaction(vec![Operation::SetChart {
        id: chart,
        chart: Some(Box::new(c)),
    }]))
    .unwrap();
    let mut res = ondin_core::Resolved::rebuild(&doc);
    ondin_core::chart::settle_all(&mut doc, &mut res);
    let page = doc.first_page();
    let bar = part(&doc, chart, PartRole::Bar, 1, 0).unwrap();
    let ink = match doc.get(bar).unwrap().paint().fills[0].brush.literal() {
        Some(ondin_core::Brush::Solid(c)) => *c,
        other => panic!("the fixture: a solid bar, not {other:?}"),
    };
    let mut ids = IdSource::new(0xC9);
    let (note, swatch) = (ids.mint(), ids.mint());
    let text = NodeKind::Text {
        content: "Jan report".into(),
        style: Box::default(),
        spans: Default::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing: ondin_core::TextSizing::Auto,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    };
    doc.apply(&Transaction(vec![
        create(note, page, text),
        create(swatch, page, rect()),
        Operation::SetFills {
            id: swatch,
            fills: ondin_core::keyed_by_position([ondin_core::Fill {
                brush: ondin_core::Brush::Solid(ink).into(),
                visible: true,
            }]),
        },
    ]))
    .unwrap();

    let q = Query::new("Jan", "January", false, false, false, false).unwrap();
    let hits = ondin_core::find::find(&doc, &Scope::Page(page), &q);
    let tx = ondin_core::find::replace(&doc, &hits, &q, false).tx;
    doc.apply(&tx).expect("replace all");
    match doc.get(note).unwrap().kind() {
        NodeKind::Text { content, .. } => assert_eq!(content, "January report"),
        _ => unreachable!(),
    }

    let uses = ondin_core::families::families_in_use(&doc);
    let gone = uses.iter().find(|(f, _)| f == "Gone Sans").expect("listed");
    assert_eq!(gone.1, vec![chart], "the chart is the family's one layer");
    let tx = ondin_core::families::replace_family(&doc, "Gone Sans", "Inter");
    doc.apply(&tx).expect("replace font");
    assert_eq!(
        doc.get(chart).unwrap().chart().unwrap().options.text.family,
        "Inter"
    );

    let red = ondin_core::peniko::Color::from_rgb8(255, 0, 0);
    let tx = ondin_core::build::recolor(&doc, &[page], ink, red);
    doc.apply(&tx).expect("recolour");
    assert_eq!(
        doc.get(swatch).unwrap().paint().fills[0].brush.literal(),
        Some(&ondin_core::Brush::Solid(red))
    );
}

/// Chart `id` settled once, its regeneration applied and asserted accepted.
fn settle_checked(doc: &mut Document, id: NodeId, what: &str) {
    let res = ondin_core::Resolved::rebuild(doc);
    let mut ids = IdSource::new(0xC6);
    ondin_core::reserve_existing_ids(doc, &mut ids);
    let tx = ondin_core::chart::settle(doc, &res, &mut ids, id, "Inter").expect("a chart");
    if let Err(e) = doc.apply(&tx) {
        panic!("{what}: the regeneration was refused: {e}");
    }
}

/// Every kind settles at sizes that leave its plot no width or no height, and
/// the document accepts what it writes.
/// flip: `bar_marks` dividing by a plot of no width → fails at "refused".
#[test]
fn a_chart_too_small_for_its_plot_settles() {
    for kind in ChartKind::ALL {
        for (w, h) in [(1.0, 1.0), (30.0, 30.0), (50.0, 200.0), (200.0, 20.0)] {
            let (mut doc, chart) = unsettled(w, h);
            let mut c = Chart::sample(kind);
            c.options.lines.curve = ondin_core::chart::Curve::Smooth;
            c.options.values.position = ondin_core::chart::ValuePosition::Inside;
            doc.apply(&Transaction(vec![Operation::SetChart {
                id: chart,
                chart: Some(Box::new(c)),
            }]))
            .unwrap();
            settle_checked(&mut doc, chart, &format!("{kind:?} at {w}×{h}"));
        }
    }
}

/// A deleted chart's parts are held by its undo step, so another chart's
/// regeneration never mints their ids: undoing the delete brings it back.
/// flip: `settle_round` minting every chart's parts under one actor → fails at
/// "the delete undoes".
#[test]
fn a_regeneration_never_takes_a_deleted_charts_part_ids() {
    let mut ids = IdSource::new(0xC7);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let page = doc.first_page();
    let mut history = History::default();
    let settle = |doc: &mut Document| {
        let mut res = ondin_core::Resolved::rebuild(doc);
        ondin_core::chart::settle_all(doc, &mut res);
    };
    let [b, a] = [(); 2].map(|_| ids.mint());
    for id in [b, a] {
        history
            .commit(
                &mut doc,
                Transaction(vec![
                    create(id, page, frame(480.0, 280.0)),
                    Operation::SetChart {
                        id,
                        chart: Some(Box::new(sample())),
                    },
                ]),
            )
            .unwrap();
        settle(&mut doc);
    }
    let a_parts: Vec<NodeId> = doc
        .get(a)
        .unwrap()
        .chart()
        .unwrap()
        .parts
        .iter()
        .map(|(_, n)| *n)
        .collect();
    history
        .commit(&mut doc, Transaction(vec![Operation::DeleteNode { id: a }]))
        .unwrap();
    settle(&mut doc);
    let mut c = doc.get(b).unwrap().chart().unwrap().clone();
    c.kind = ChartKind::Line;
    history
        .commit(
            &mut doc,
            Transaction(vec![Operation::SetChart {
                id: b,
                chart: Some(Box::new(c)),
            }]),
        )
        .unwrap();
    settle(&mut doc);
    let b_parts: Vec<NodeId> = doc
        .get(b)
        .unwrap()
        .chart()
        .unwrap()
        .parts
        .iter()
        .map(|(_, n)| *n)
        .collect();
    history.undo(&mut doc).unwrap().unwrap();
    settle(&mut doc);
    let reused = b_parts.iter().filter(|n| a_parts.contains(n)).count();
    if let Err(e) = history.undo(&mut doc) {
        panic!("the delete undoes: {e} ({reused} of the line's part ids were the deleted chart's)");
    }
    assert!(doc.get(a).is_some());
    settle(&mut doc);
}

/// A part id a regeneration deleted is never minted again, for this chart or
/// after a save and load: each id names one part for good.
/// flip: `settle_round` reserving past only the ids the document holds → fails
/// at "names one part".
#[test]
fn a_regeneration_never_mints_a_deleted_parts_id_again() {
    let (mut doc, chart) = unsettled(480.0, 280.0);
    let whole = sample();
    let mut short = sample();
    short.table.rows.pop();
    let mut seen: Vec<(PartKey, NodeId)> = Vec::new();
    // The short table remakes the value labels, which hiding the axis then
    // deletes: the newest ids leave with nothing newer kept.
    for (i, (table, kind, axis)) in [
        (&whole, ChartKind::Column, true),
        (&short, ChartKind::Column, true),
        (&short, ChartKind::Column, false),
        (&whole, ChartKind::Column, false),
        (&short, ChartKind::Column, true),
        (&short, ChartKind::Column, false),
        (&whole, ChartKind::Line, true),
        (&whole, ChartKind::Pie, true),
    ]
    .into_iter()
    .enumerate()
    {
        if i == 5 {
            doc = io::load(&io::save(&doc).unwrap()).unwrap();
        }
        let mut c = doc.get(chart).unwrap().chart().unwrap().clone();
        c.table = table.table.clone();
        c.kind = kind;
        c.options.value_axis.show = axis;
        doc.apply(&Transaction(vec![Operation::SetChart {
            id: chart,
            chart: Some(Box::new(c)),
        }]))
        .unwrap();
        let mut res = ondin_core::Resolved::rebuild(&doc);
        ondin_core::chart::settle_all(&mut doc, &mut res);
        for (k, id) in parts_of(&doc, chart) {
            let other = seen.iter().find(|(k2, id2)| *id2 == id && *k2 != k);
            assert!(
                other.is_none(),
                "{id:?} names one part: {other:?}, now {k:?}"
            );
            seen.push((k, id));
        }
    }
}

/// No guide is scoped to a chart's part frame, which its regeneration deletes:
/// the commit refuses one, and a file holding one loads with the guide moved
/// onto the page at the place it drew.
/// flip: `chart::check` skipping its guide-owner walk → fails at "refused";
/// the loader keeping a part's guide → fails at "moves off the chart".
#[test]
fn a_guide_is_never_scoped_to_a_charts_part() {
    use ondin_core::guide::{Guide, GuideAxis, GuideId};
    let (mut doc, chart) = unsettled(480.0, 280.0);
    let mut res = ondin_core::Resolved::rebuild(&doc);
    ondin_core::chart::settle_all(&mut doc, &mut res);
    let plot = part(&doc, chart, PartRole::Plot, 0, 0).expect("a plot frame");
    assert!(matches!(
        doc.get(plot).unwrap().kind(),
        NodeKind::Artboard { .. }
    ));
    let page = doc.first_page();
    let mut ids = IdSource::new(0xC8);
    let on_part = Guide {
        id: GuideId(ids.mint()),
        axis: GuideAxis::Vertical,
        position: 10.0,
        color: None,
        owner: plot,
    };
    let err = refused(&mut doc, vec![Operation::AddGuide { guide: on_part }]);
    assert!(
        matches!(err, OpError::ChartPlacement(id, _) if id == plot),
        "refused: {err}"
    );
    doc.apply(&Transaction(vec![Operation::AddGuide {
        guide: Guide {
            owner: page,
            ..on_part
        },
    }]))
    .unwrap();
    let saved = String::from_utf8(io::save(&doc).unwrap()).unwrap();
    let mut file = saved.clone();
    for gap in ["", " "] {
        file = file.replace(
            &format!("\"owner\":{gap}\"{}\"", page.to_wire()),
            &format!("\"owner\":{gap}\"{}\"", plot.to_wire()),
        );
    }
    assert_ne!(file, saved, "the fixture: the guide names the part");
    let loaded = io::load(file.as_bytes()).expect("a guide on a part loads");
    let g = loaded.guides()[0];
    assert_eq!(g.owner, page, "moves off the chart");
    let mut x = 10.0;
    let mut at = Some(plot);
    while let Some(id) = at.filter(|id| *id != page) {
        x += loaded.get(id).unwrap().transform().translation().x;
        at = loaded.get(id).unwrap().parent();
    }
    assert!(
        (g.position - x).abs() < 1e-9 && x > 10.0,
        "where it drew: {} vs {x}",
        g.position
    );
}

/// `id`'s description with its kind set to `kind`, applied.
fn set_kind(doc: &mut Document, id: NodeId, kind: ChartKind) {
    let mut c = doc.get(id).unwrap().chart().unwrap().clone();
    c.kind = kind;
    doc.apply(&Transaction(vec![Operation::SetChart {
        id,
        chart: Some(Box::new(c)),
    }]))
    .unwrap();
}

/// A chart with value labels switches between a pie and every axis kind both
/// ways: the labels' frame hangs under the plot in one and the pie's frame in
/// the other, so it is made again rather than moved out of a deleted parent.
/// flip: `settle::ops` keeping a part by its key alone → fails at "the
/// regeneration was refused".
#[test]
fn value_labels_survive_a_switch_to_and_from_a_pie() {
    for kind in [
        ChartKind::Column,
        ChartKind::Bar,
        ChartKind::Line,
        ChartKind::Area,
    ] {
        let (mut doc, chart) = unsettled(480.0, 280.0);
        let mut c = doc.get(chart).unwrap().chart().unwrap().clone();
        c.options.values.position = ondin_core::chart::ValuePosition::Inside;
        doc.apply(&Transaction(vec![Operation::SetChart {
            id: chart,
            chart: Some(Box::new(c)),
        }]))
        .unwrap();
        set_kind(&mut doc, chart, kind);
        settle_checked(&mut doc, chart, &format!("{kind:?}"));
        assert!(
            part(&doc, chart, PartRole::Values, 0, 0).is_some(),
            "the fixture: {kind:?} has its value labels"
        );
        set_kind(&mut doc, chart, ChartKind::Pie);
        settle_checked(&mut doc, chart, &format!("{kind:?} to pie"));
        let values = part(&doc, chart, PartRole::Values, 0, 0).expect("a pie's values");
        assert!(doc.get(values).is_some());
        set_kind(&mut doc, chart, kind);
        settle_checked(&mut doc, chart, &format!("pie to {kind:?}"));
        let values = part(&doc, chart, PartRole::Values, 0, 0).expect("values again");
        let plot = part(&doc, chart, PartRole::Plot, 0, 0).unwrap();
        assert_eq!(doc.get(values).unwrap().parent(), Some(plot), "{kind:?}");
    }
}
