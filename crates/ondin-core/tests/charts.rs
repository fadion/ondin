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
    for (chart, rule) in [
        (ragged, ChartRule::RaggedRow),
        (unknown, ChartRule::UnknownColumn),
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
