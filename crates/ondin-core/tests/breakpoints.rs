//! Breakpoints through modes (§5.3f): a collection picked by width, the mode a
//! top-level frame picks from its width, pins beating it, the verbs that tick,
//! untick and edit the widths, and the save format.

use ondin_core::kurbo::{RoundedRectRadii, Size};
use ondin_core::variable::{self, Collection, CollectionId, VarRule};
use ondin_core::{
    Cell, Document, GeometryPatch, IdSource, Mode, ModeId, NodeId, NodeKind, OpError, Operation,
    Transaction, Value, VarId, VarKind, Variable,
};

/// Breakpoint (Mobile, Tablet, Desktop) is the second collection, Colour (Light)
/// the first. *Home* is a top-level frame 1024 wide holding `card`, a frame, and
/// `rect` inside it; `gap` reads 16, 32 and 64.
struct F {
    doc: Document,
    ids: IdSource,
    page: NodeId,
    home: NodeId,
    card: NodeId,
    rect: NodeId,
    bp: CollectionId,
    mobile: ModeId,
    tablet: ModeId,
    desktop: ModeId,
    gap: VarId,
}

fn mode(id: ModeId, name: &str) -> Mode {
    Mode {
        id,
        name: name.into(),
    }
}

fn frame(id: NodeId, parent: NodeId, w: f64) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index: 0,
        kind: NodeKind::Artboard {
            size: Size::new(w, 400.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: None,
        name: None,
    }
}

/// The fixture, its Breakpoint collection not yet picked by width.
fn plain() -> F {
    let mut ids = IdSource::new(0xB0);
    let page = ids.mint();
    let mut doc = Document::new(page);
    let (home, card, rect) = (ids.mint(), ids.mint(), ids.mint());
    let light = ids.mint_mode();
    let (mobile, tablet, desktop) = (ids.mint_mode(), ids.mint_mode(), ids.mint_mode());
    let bp = ids.mint_collection();
    let gap = ids.mint_var();
    doc.apply(&Transaction(vec![
        frame(home, page, 1024.0),
        frame(card, home, 200.0),
        Operation::CreateNode {
            id: rect,
            parent: card,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 20.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::AddMode {
            collection: CollectionId::FIRST,
            mode: mode(light, "Light"),
            index: 0,
            cells: vec![],
        },
        Operation::AddCollection {
            collection: Collection {
                id: bp,
                name: "Breakpoint".into(),
                modes: vec![
                    mode(mobile, "Mobile"),
                    mode(tablet, "Tablet"),
                    mode(desktop, "Desktop"),
                ],
                breakpoints: None,
            },
            index: 1,
        },
        Operation::AddVariable {
            var: Variable {
                id: gap,
                name: "page/gap".into(),
                kind: VarKind::Number,
                cells: [
                    (mobile, Cell::Number(16.0)),
                    (tablet, Cell::Number(32.0)),
                    (desktop, Cell::Number(64.0)),
                ]
                .into(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        },
        Operation::SetCornerVars {
            id: rect,
            vars: [Some(gap); 4],
        },
    ]))
    .expect("a page, two collections and a bound rect");
    F {
        doc,
        ids,
        page,
        home,
        card,
        rect,
        bp,
        mobile,
        tablet,
        desktop,
        gap,
    }
}

/// [`plain`] with Breakpoint picked by width: Mobile 0, Tablet 768, Desktop 1200.
fn fixture() -> F {
    let mut f = plain();
    let ops = variable::by_width_ops(&f.doc, f.bp, true).expect("the first by width");
    f.doc.apply(&Transaction(ops)).expect("ticked");
    f
}

fn resize(doc: &mut Document, id: NodeId, w: f64) -> ondin_core::ApplyOutcome {
    doc.apply(&Transaction(vec![Operation::SetGeometry {
        id,
        geometry: GeometryPatch::Size(Size::new(w, 400.0)),
    }]))
    .expect("resized")
}

fn gap_at(f: &F, id: NodeId) -> f64 {
    match f.doc.resolve_var(id, f.gap) {
        Some(Value::Number(n)) => n,
        other => panic!("gap reads {other:?}"),
    }
}

/// Ticking gives the modes 0, 768 and 1200, and a frame 1024 wide picks
/// Tablet, which its nested frame and their contents inherit.
#[test]
fn a_top_level_frame_picks_the_mode_its_width_reaches() {
    let f = fixture();
    let c = f.doc.variables().collection(f.bp).unwrap();
    assert_eq!(c.min_width(f.mobile), Some(0.0));
    assert_eq!(c.min_width(f.tablet), Some(768.0));
    assert_eq!(c.min_width(f.desktop), Some(1200.0));
    assert_eq!(f.doc.width_pick(f.home), Some(f.tablet));
    assert_eq!(f.doc.mode_at(f.rect, f.bp), Some(f.tablet));
    assert_eq!(gap_at(&f, f.rect), 32.0);
    // A nested frame is no viewport: it inherits.
    assert!(!f.doc.picks_by_width(f.card));
    assert_eq!(f.doc.width_pick(f.card), None);
}

/// Resizing across a minimum changes the mode, dirties the bound layers under
/// the frame, and its undo restores the width and with it the mode.
#[test]
fn resizing_across_a_breakpoint_changes_the_mode_and_dirties_the_contents() {
    let mut f = fixture();
    let out = resize(&mut f.doc, f.home, 1300.0);
    assert_eq!(f.doc.mode_at(f.rect, f.bp), Some(f.desktop));
    assert_eq!(gap_at(&f, f.rect), 64.0);
    assert!(out.dirty.0.contains(&f.rect), "the bound rect is redrawn");
    let same = resize(&mut f.doc, f.home, 1250.0);
    f.doc.apply(&same.inverse).unwrap();
    f.doc.apply(&out.inverse).unwrap();
    assert_eq!(f.doc.mode_at(f.rect, f.bp), Some(f.tablet));
    resize(&mut f.doc, f.home, 767.0);
    assert_eq!(f.doc.mode_at(f.rect, f.bp), Some(f.mobile));
    resize(&mut f.doc, f.home, 768.0);
    assert_eq!(
        f.doc.mode_at(f.rect, f.bp),
        Some(f.tablet),
        "the minimum is inclusive"
    );
}

/// A mode set on the frame, or on its page, beats the width; the other
/// collections are untouched by any of it.
#[test]
fn a_pin_on_the_frame_or_its_page_beats_the_width() {
    let mut f = fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetModes {
            id: f.home,
            modes: vec![f.mobile],
        }]))
        .unwrap();
    assert_eq!(f.doc.mode_at(f.rect, f.bp), Some(f.mobile));
    assert_eq!(f.doc.width_pick(f.home), None);
    assert_eq!(
        f.doc.width_mode(f.home),
        Some(f.tablet),
        "what it would pick"
    );
    f.doc
        .apply(&Transaction(vec![
            Operation::SetModes {
                id: f.home,
                modes: vec![],
            },
            Operation::SetModes {
                id: f.page,
                modes: vec![f.desktop],
            },
        ]))
        .unwrap();
    assert_eq!(f.doc.mode_at(f.rect, f.bp), Some(f.desktop));
    assert_eq!(f.doc.width_pick(f.home), None);
}

/// A component main is no viewport: it takes its page's mode, else the
/// default, whatever its width.
/// flip: `apply_one`'s dirty walk removed → fails at `dirty.contains(rect)`.
#[test]
fn a_component_main_never_picks_by_width() {
    let mut f = fixture();
    let out = f
        .doc
        .apply(&Transaction(vec![Operation::SetComponent {
            id: f.home,
            component: true,
        }]))
        .unwrap();
    assert!(!f.doc.picks_by_width(f.home));
    assert_eq!(f.doc.mode_at(f.rect, f.bp), Some(f.mobile));
    assert!(
        out.dirty.0.contains(&f.rect),
        "making it a main moves its mode"
    );
}

/// While a frame is previewed at another width, a read through the previewed
/// table picks that width's mode; the document's own table is unchanged.
#[test]
fn a_previewed_width_picks_its_mode_in_the_preview_table_only() {
    let f = fixture();
    let t = f.doc.variables().with_width(f.home, 500.0);
    assert_eq!(
        f.doc.resolve_var_in(&t, f.rect, f.gap),
        Some(Value::Number(16.0))
    );
    assert_eq!(gap_at(&f, f.rect), 32.0);
}

/// One collection at most is picked by width, and ticking a second is refused
/// with the first's name.
#[test]
fn a_second_collection_by_width_is_refused() {
    let f = fixture();
    let err = variable::by_width_ops(&f.doc, CollectionId::FIRST, true).unwrap_err();
    assert_eq!(err, "Breakpoint already picks its mode by width.");
    let mut doc = f.doc.clone();
    let r = doc.apply(&Transaction(vec![Operation::SetBreakpoints {
        id: CollectionId::FIRST,
        breakpoints: Some([(doc.variables().first_collection().modes[0].id, 0.0)].into()),
    }]));
    assert!(matches!(r, Err(OpError::Variables(VarRule::TwoByWidth))));
}

/// The widths must be one per mode, the first 0 and each past the one before.
#[test]
fn widths_out_of_order_or_missing_a_mode_are_refused() {
    let f = fixture();
    let bad = [
        vec![(f.mobile, 0.0), (f.tablet, 1200.0), (f.desktop, 768.0)],
        vec![(f.mobile, 0.0), (f.tablet, 768.0)],
        vec![(f.mobile, 10.0), (f.tablet, 768.0), (f.desktop, 1200.0)],
        vec![(f.mobile, 0.0), (f.tablet, 768.0), (f.desktop, 768.0)],
    ];
    for widths in bad {
        let mut doc = f.doc.clone();
        let r = doc.apply(&Transaction(vec![Operation::SetBreakpoints {
            id: f.bp,
            breakpoints: Some(widths.iter().copied().collect()),
        }]));
        assert!(
            matches!(r, Err(OpError::Variables(VarRule::BadBreakpoints(_)))),
            "{widths:?}: {r:?}"
        );
    }
}

/// A width typed at or past a neighbour's is refused naming where the
/// neighbour starts; the first mode's 0 is fixed.
#[test]
fn a_width_is_refused_at_or_past_a_neighbour() {
    let mut f = fixture();
    assert_eq!(
        variable::breakpoint_ops(&f.doc, f.tablet, 1200.0).unwrap_err(),
        "Desktop starts at 1200."
    );
    assert_eq!(
        variable::breakpoint_ops(&f.doc, f.tablet, 0.0).unwrap_err(),
        "Mobile starts at 0."
    );
    assert_eq!(
        variable::breakpoint_ops(&f.doc, f.mobile, 10.0).unwrap_err(),
        "Mobile starts at 0."
    );
    let ops = variable::breakpoint_ops(&f.doc, f.tablet, 1100.0).unwrap();
    f.doc.apply(&Transaction(ops)).unwrap();
    assert_eq!(
        f.doc.mode_at(f.rect, f.bp),
        Some(f.mobile),
        "1024 is under 1100"
    );
    assert!(
        variable::breakpoint_ops(&f.doc, f.tablet, 1100.0)
            .unwrap()
            .is_empty()
    );
}

/// Unticking keeps the modes, drops the widths, and sets every frame that
/// picked by width to the mode it drew, so nothing on the canvas changes.
#[test]
fn unticking_pins_each_frame_to_the_mode_it_drew() {
    let mut f = fixture();
    let ops = variable::by_width_ops(&f.doc, f.bp, false).unwrap();
    f.doc.apply(&Transaction(ops)).unwrap();
    let c = f.doc.variables().collection(f.bp).unwrap();
    assert!(!c.by_width());
    assert_eq!(c.modes.len(), 3);
    assert_eq!(f.doc.get(f.home).unwrap().modes(), [f.tablet]);
    assert_eq!(f.doc.mode_at(f.rect, f.bp), Some(f.tablet));
    assert!(f.doc.get(f.card).unwrap().modes().is_empty());
}

/// *Duplicate mode* starts halfway to the next mode, *New mode* 400 past the
/// last; deleting the first mode gives the next its 0; reordering is refused.
#[test]
fn the_mode_verbs_keep_the_widths_whole() {
    let mut f = fixture();
    let copy = f.ids.mint_mode();
    let ops = variable::copy_mode_ops(&f.doc, f.tablet, copy, "Tablet 2", 2);
    f.doc.apply(&Transaction(ops)).unwrap();
    assert_eq!(
        f.doc.variables().collection(f.bp).unwrap().min_width(copy),
        Some(984.0)
    );
    let last = f.ids.mint_mode();
    let ops = variable::copy_mode_ops(&f.doc, f.mobile, last, "Wide", 4);
    f.doc.apply(&Transaction(ops)).unwrap();
    assert_eq!(
        f.doc.variables().collection(f.bp).unwrap().min_width(last),
        Some(1600.0)
    );
    assert!(variable::move_mode_ops(&f.doc, f.desktop, 0).is_empty());
    let ops = variable::remove_mode_ops(&f.doc, f.mobile);
    f.doc.apply(&Transaction(ops)).unwrap();
    let c = f.doc.variables().collection(f.bp).unwrap();
    assert_eq!(c.min_width(f.tablet), Some(0.0));
    assert_eq!(c.min_width(f.mobile), None);
}

/// Deleting a collection's last mode leaves it picked by width no more, so
/// another collection may be.
/// flip: `remove_mode_ops` keeping an empty set of widths → fails at "no modes".
#[test]
fn deleting_the_last_mode_unticks_the_collection() {
    let mut f = fixture();
    let modes: Vec<ModeId> = f
        .doc
        .variables()
        .collection(f.bp)
        .unwrap()
        .modes
        .iter()
        .map(|m| m.id)
        .collect();
    // Its variables first, as `take` does: a variable needs a mode.
    let vars: Vec<_> = f.doc.variables().iter().map(|v| v.id).collect();
    let ops = variable::remove_ops(&f.doc, &vars);
    f.doc.apply(&Transaction(ops)).expect("variables removed");
    for m in modes {
        let ops = variable::remove_mode_ops(&f.doc, m);
        f.doc.apply(&Transaction(ops)).expect("removed");
    }
    let c = f.doc.variables().collection(f.bp).unwrap();
    assert!(c.modes.is_empty());
    assert!(!c.by_width(), "no modes");
}

/// *Duplicate collection* drops the tick, so the copy picks by nothing.
#[test]
fn a_duplicated_collection_is_not_picked_by_width() {
    let mut f = fixture();
    let ops = variable::duplicate_collection_ops(&f.doc, f.bp, "Breakpoint 2", &mut f.ids);
    f.doc.apply(&Transaction(ops)).expect("only one by width");
    assert_eq!(
        f.doc
            .variables()
            .collections()
            .iter()
            .filter(|c| c.by_width())
            .count(),
        1
    );
}

/// The widths survive a save and a load, on the first collection as on a later
/// one, and a file with none is the bytes it was.
#[test]
fn the_widths_round_trip_and_cost_nothing_unset() {
    let plain = plain();
    let before = ondin_core::io::save(&plain.doc).unwrap();
    assert!(!String::from_utf8_lossy(&before).contains("breakpoints"));
    let f = fixture();
    let bytes = ondin_core::io::save(&f.doc).unwrap();
    let back = ondin_core::io::load(&bytes).unwrap();
    assert_eq!(back.variables(), f.doc.variables());
    assert_eq!(ondin_core::io::save(&back).unwrap(), bytes);

    let mut first = plain;
    let m = first.doc.variables().first_collection().modes[0].id;
    first
        .doc
        .apply(&Transaction(vec![Operation::SetBreakpoints {
            id: CollectionId::FIRST,
            breakpoints: Some([(m, 0.0)].into()),
        }]))
        .unwrap();
    let bytes = ondin_core::io::save(&first.doc).unwrap();
    let back = ondin_core::io::load(&bytes).unwrap();
    assert!(back.variables().first_collection().by_width());
}
