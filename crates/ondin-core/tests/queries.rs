//! Modes by a frame's own width (§5.3f, width queries): a main whose contents
//! pick a collection's mode from the frame's width, every instance picking by
//! its own — CSS's container query. The pick's precedence, containment, the
//! laid-out width, the verbs and the save format.

use ondin_core::container::{Dimension, Display, Flex, FlexDirection};
use ondin_core::kurbo::{RoundedRectRadii, Size};
use ondin_core::variable::{self, Collection, CollectionId, ModeOwner, QUERY_STEP, VarRule};
use ondin_core::{
    Cell, Document, GeometryPatch, IdSource, Mode, ModeId, NodeId, NodeKind, OpError, Operation,
    Placement, Resolved, Transaction, VarId, VarKind, Variable, io,
};

/// Density (Compact, Regular, Wide) is the second collection. *Card* is a main
/// 300 wide holding `rect`, whose corners read `pad` (8, 16, 24), and a page
/// frame *Home* holds nothing yet.
struct F {
    doc: Document,
    ids: IdSource,
    page: NodeId,
    card: NodeId,
    rect: NodeId,
    density: CollectionId,
    compact: ModeId,
    regular: ModeId,
    wide: ModeId,
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
            size: Size::new(w, 200.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: None,
        name: Some("Card".into()),
    }
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xD7);
    let page = ids.mint();
    let mut doc = Document::new(page);
    let (card, rect) = (ids.mint(), ids.mint());
    let light = ids.mint_mode();
    let (compact, regular, wide) = (ids.mint_mode(), ids.mint_mode(), ids.mint_mode());
    let density = ids.mint_collection();
    let pad: VarId = ids.mint_var();
    doc.apply(&Transaction(vec![
        frame(card, page, 300.0),
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
                id: density,
                name: "Density".into(),
                modes: vec![
                    mode(compact, "Compact"),
                    mode(regular, "Regular"),
                    mode(wide, "Wide"),
                ],
                breakpoints: None,
            },
            index: 1,
        },
        Operation::AddVariable {
            var: Variable {
                id: pad,
                name: "density/pad".into(),
                kind: VarKind::Number,
                cells: [
                    (compact, Cell::Number(8.0)),
                    (regular, Cell::Number(16.0)),
                    (wide, Cell::Number(24.0)),
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
            vars: [Some(pad); 4],
        },
        Operation::SetComponent {
            id: card,
            component: true,
        },
    ]))
    .expect("a main and a collection");
    F {
        doc,
        ids,
        page,
        card,
        rect,
        density,
        compact,
        regular,
        wide,
    }
}

impl F {
    fn commit(&mut self, ops: Vec<Operation>) {
        let mut tx = Transaction(ops);
        ondin_core::propagate::owed(&self.doc, &mut tx, &mut self.ids);
        self.doc.apply(&tx).expect("the edit and what it owes");
    }

    fn by_width(&mut self) {
        let ops = variable::query_ops(
            &self.doc,
            self.card,
            ModeOwner::Collection(self.density),
            true,
        )
        .expect("a main 300 wide picks by width");
        self.commit(ops);
    }

    /// An instance of Card under `parent`, `w` wide.
    fn instance(&mut self, parent: NodeId, w: f64) -> NodeId {
        let (tx, made) = ondin_core::insert_subtrees(
            &self.doc,
            &mut self.ids,
            &[Placement {
                nodes: self.doc.capture_subtree(self.card).unwrap(),
                parent,
                index: None,
            }],
            Default::default(),
        );
        self.doc.apply(&tx).expect("an instance");
        let i = made[0];
        self.commit(vec![Operation::SetGeometry {
            id: i,
            geometry: GeometryPatch::Size(Size::new(w, 200.0)),
        }]);
        i
    }

    /// The density `id`'s first child reads.
    fn inside(&self, id: NodeId) -> Option<ModeId> {
        let child = self.doc.get(id).unwrap().children()[0];
        self.doc.mode_at(child, self.density)
    }
}

/// *By width* writes the main's widths, 0 then [`QUERY_STEP`] apart, which its
/// instances follow; each instance's contents then read the mode of its own
/// width, the same Card Compact at 260, Regular at 400 and Wide at 700.
/// Flip: `modes_at_in` skipping a frame's queries reads Compact (the default)
/// in all three.
#[test]
fn each_instance_picks_by_its_own_width() {
    let mut f = fixture();
    f.by_width();
    let q = f.doc.get(f.card).unwrap().queries()[0].clone();
    assert_eq!(
        q.values().copied().collect::<Vec<_>>(),
        vec![0.0, QUERY_STEP, 2.0 * QUERY_STEP]
    );
    let page = f.page;
    let (a, b, c) = (
        f.instance(page, 260.0),
        f.instance(page, 400.0),
        f.instance(page, 700.0),
    );
    assert_eq!(
        f.doc.get(a).unwrap().queries(),
        std::slice::from_ref(&q),
        "copies follow"
    );
    assert_eq!(f.inside(a), Some(f.compact));
    assert_eq!(f.inside(b), Some(f.regular));
    assert_eq!(f.inside(c), Some(f.wide));
    assert_eq!(f.inside(f.card), Some(f.compact), "the main, 300 wide");
}

/// Precedence: the instance's own width beats a pin on its page, a pin on the
/// instance beats its width, and the instance's own fields read the mode around
/// it — a container never queries itself.
#[test]
fn a_pin_on_the_instance_beats_its_width_and_its_width_beats_the_page() {
    let mut f = fixture();
    f.by_width();
    let page = f.page;
    let i = f.instance(page, 260.0);
    f.commit(vec![Operation::SetModes {
        id: page,
        modes: vec![f.wide],
    }]);
    assert_eq!(f.inside(i), Some(f.compact), "its width beats the page");
    assert_eq!(
        f.doc.mode_at(i, f.density),
        Some(f.wide),
        "the root reads the page"
    );
    f.commit(vec![Operation::SetModes {
        id: i,
        modes: vec![f.regular],
    }]);
    assert_eq!(f.inside(i), Some(f.regular), "a pin beats the width");
}

/// The laid-out width is the one picked by: an instance 300 wide stretched to
/// 700 across a column reads Wide once the document is settled against layout,
/// and a fit-content width is held at the padding (containment), so a main
/// asked to hug cannot pick by width.
#[test]
fn the_laid_out_width_picks_and_fit_content_is_held_at_the_padding() {
    let mut f = fixture();
    f.by_width();
    let col = f.ids.mint();
    let page = f.page;
    f.commit(vec![
        frame(col, page, 700.0),
        Operation::SetDisplay {
            id: col,
            display: Some(Display::Flex(Flex {
                direction: FlexDirection::Column,
                ..Default::default()
            })),
        },
    ]);
    let i = f.instance(col, 300.0);
    let mut fill = *f.doc.get(i).unwrap().item();
    fill.width = Dimension::Percent(100.0);
    f.commit(vec![Operation::SetLayoutItem { id: i, item: fill }]);
    assert_eq!(f.inside(i), Some(f.compact), "unsettled: its own 300");
    let res = Resolved::rebuild(&f.doc);
    let dirty = f.doc.settle_widths(&res);
    assert!(
        !dirty.0.is_empty(),
        "the pick moved, so its readers are dirty"
    );
    assert_eq!(f.inside(i), Some(f.wide), "stretched to 700");

    let mut item = *f.doc.get(f.card).unwrap().item();
    item.width = Dimension::FitContent;
    f.commit(vec![Operation::SetLayoutItem { id: f.card, item }]);
    assert!(variable::why_no_query(&f.doc, f.card).is_some());
    assert_eq!(
        f.doc.resolved_item(f.card).width,
        Dimension::Px(0.0),
        "hugging, contained: its padding, here none"
    );
}

/// A main's widths: the first mode's 0 is fixed, a width at or past a
/// neighbour's is refused naming it, and unticking takes the query away.
#[test]
fn widths_keep_their_order_and_untick_goes() {
    let mut f = fixture();
    f.by_width();
    let err = variable::query_width_ops(&f.doc, f.card, f.compact, 10.0).unwrap_err();
    assert_eq!(err, "Compact starts at 0.");
    let err = variable::query_width_ops(&f.doc, f.card, f.regular, 640.0).unwrap_err();
    assert_eq!(err, "Wide starts at 640.");
    let ops = variable::query_width_ops(&f.doc, f.card, f.regular, 500.0).unwrap();
    f.commit(ops);
    assert_eq!(f.doc.get(f.card).unwrap().queries()[0][&f.regular], 500.0);
    let ops = variable::query_ops(&f.doc, f.card, ModeOwner::Collection(f.density), false).unwrap();
    f.commit(ops);
    assert!(f.doc.get(f.card).unwrap().queries().is_empty());
}

/// Removing a mode drops its width, the next taking 0 when the narrowest goes;
/// copying one gives the copy a width between its neighbours.
#[test]
fn the_mode_verbs_keep_the_queries_in_step() {
    let mut f = fixture();
    f.by_width();
    let ops = variable::remove_mode_ops(&f.doc, f.compact);
    f.commit(ops);
    let q = &f.doc.get(f.card).unwrap().queries()[0];
    assert_eq!(q.get(&f.regular), Some(&0.0));
    assert_eq!(q.get(&f.wide), Some(&(2.0 * QUERY_STEP)));
    let copy = f.ids.mint_mode();
    let ops = variable::copy_mode_ops(&f.doc, f.regular, copy, "Cozy", 1);
    f.commit(ops);
    let q = &f.doc.get(f.card).unwrap().queries()[0];
    assert_eq!(q.get(&copy), Some(&QUERY_STEP));
}

/// The queries save and load, and only on a frame: a rect holding one is
/// refused at the commit.
#[test]
fn queries_save_and_stay_on_frames() {
    let mut f = fixture();
    f.by_width();
    let back = io::load(&io::save(&f.doc).unwrap()).expect("reloads");
    assert_eq!(
        back.get(f.card).unwrap().queries(),
        f.doc.get(f.card).unwrap().queries()
    );
    let q = f.doc.get(f.card).unwrap().queries().to_vec();
    let err = f
        .doc
        .apply(&Transaction(vec![Operation::SetQueries {
            id: f.rect,
            queries: q,
        }]))
        .unwrap_err();
    assert!(
        matches!(err, OpError::Variables(VarRule::BadQuery(_))),
        "{err:?}"
    );
}

/// The width picked by is the content box, as `@container (min-width)` reads
/// it in the export: a Card with 24 px each side, 650 wide, is 602 inside and
/// so Regular, not Wide at 640. Flip: `query_width_in` without its padding
/// reads Wide.
#[test]
fn the_query_reads_the_content_box() {
    let mut f = fixture();
    f.by_width();
    f.commit(vec![Operation::SetDisplay {
        id: f.card,
        display: Some(Display::Flex(Flex {
            padding: [0.0, 24.0, 0.0, 24.0],
            ..Default::default()
        })),
    }]);
    let page = f.page;
    let i = f.instance(page, 650.0);
    assert_eq!(f.inside(i), Some(f.regular));
}

/// A mode moved in its collection keeps its place among a query's widths: with
/// Wide moved first, Regular's width can still be set between Compact's and
/// Wide's, and the order the fold lists stays narrowest first. Flip:
/// `query_width_ops` ordering by the collection → "Compact starts at 0." is
/// not the refusal, Regular is refused against Wide.
#[test]
fn a_reordered_collection_keeps_the_query_in_width_order() {
    let mut f = fixture();
    f.by_width();
    let ops = variable::move_mode_ops(&f.doc, f.wide, 0);
    f.commit(ops);
    let ops = variable::query_width_ops(&f.doc, f.card, f.regular, 500.0).expect("between");
    f.commit(ops);
    let q = f.doc.get(f.card).unwrap().queries()[0].clone();
    let order: Vec<ModeId> =
        variable::query_order(f.doc.variables(), ModeOwner::Collection(f.density), &q)
            .into_iter()
            .map(|(m, _)| m)
            .collect();
    assert_eq!(order, vec![f.compact, f.regular, f.wide]);
}

/// A querying frame nested in another settles in a second round: *Shell*, 300
/// wide, stretched to 700 by a column, picks Wide only once laid out; the Card
/// inside it is as wide as `density/w` reads there (260 Compact, 700 Wide), so
/// it can pick Wide only after Shell has. Flip: `Resolved::settle` running one
/// round → the inner Card reads Compact.
#[test]
fn a_nested_query_settles_after_the_one_around_it() {
    let mut f = fixture();
    f.by_width();
    let (col, shell) = (f.ids.mint(), f.ids.mint());
    let w: VarId = f.ids.mint_var();
    let page = f.page;
    f.commit(vec![
        frame(col, page, 700.0),
        Operation::SetDisplay {
            id: col,
            display: Some(Display::Flex(Flex {
                direction: FlexDirection::Column,
                ..Default::default()
            })),
        },
        frame(shell, col, 300.0),
        Operation::AddVariable {
            var: Variable {
                id: w,
                name: "density/w".into(),
                kind: VarKind::Number,
                cells: [
                    (f.compact, Cell::Number(260.0)),
                    (f.regular, Cell::Number(400.0)),
                    (f.wide, Cell::Number(700.0)),
                ]
                .into(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        },
    ]);
    let mut fill = *f.doc.get(shell).unwrap().item();
    fill.width = Dimension::Percent(100.0);
    f.commit(vec![Operation::SetLayoutItem {
        id: shell,
        item: fill,
    }]);
    let ops = variable::query_ops(&f.doc, shell, ModeOwner::Collection(f.density), true)
        .expect("a plain frame picks by width");
    f.commit(ops);
    let inner = f.instance(shell, 300.0);
    let ops = ondin_core::bind_size_ops(&f.doc, inner, [Some(w), None]);
    assert!(!ops.is_empty(), "the fixture: the width binds");
    f.commit(ops);
    assert_eq!(
        f.inside(shell),
        Some(f.compact),
        "unsettled: Shell's own 300"
    );
    assert_eq!(f.inside(inner), Some(f.compact), "unsettled: 260");

    let res = Resolved::settled(&mut f.doc);
    assert_eq!(f.inside(shell), Some(f.wide), "Shell stretched to 700");
    let Some(NodeKind::Artboard { size, .. }) = res.used_kind(&f.doc, inner) else {
        panic!("the inner Card is a frame");
    };
    assert_eq!(size.width, 700.0, "the inner Card reads Wide's width");
    assert_eq!(f.inside(inner), Some(f.wide), "and then picks Wide itself");
}

/// A frame querying both Density and a kit's Density mapped onto it: the kit's
/// own pick answers for the kit, whatever the queries' order. At 700 the
/// document query picks Wide, which maps to the kit's Wide; the kit query, Wide
/// only from 1000, picks the kit's Compact. Flip: `modes_at_in` taking the
/// queries in stored order → the kit's Wide.
#[test]
fn a_kit_query_beats_the_mapping_from_a_document_query() {
    use ondin_core::variable::{KitMode, KitModes};
    const KIT: &str = "0123456789abcdef0123456789abcdef";
    let mut f = fixture();
    let (kc, kw) = (f.ids.mint_mode(), f.ids.mint_mode());
    let (from_c, from_w) = (f.ids.mint_mode(), f.ids.mint_mode());
    let kit_mode = |id, from, name: &str| KitMode {
        id,
        name: name.into(),
        from,
        collection: "Density".into(),
    };
    let ours = [
        (f.compact, 0.0),
        (f.regular, QUERY_STEP),
        (f.wide, 2.0 * QUERY_STEP),
    ]
    .into();
    f.commit(vec![
        Operation::SetKitModes {
            kit: KIT.into(),
            modes: Some(KitModes {
                name: Some("Brand".into()),
                modes: vec![
                    kit_mode(kc, from_c, "Compact"),
                    kit_mode(kw, from_w, "Wide"),
                ],
                map: Default::default(),
            }),
        },
        Operation::SetQueries {
            id: f.card,
            queries: vec![ours, [(kc, 0.0), (kw, 1000.0)].into()],
        },
    ]);
    let page = f.page;
    let i = f.instance(page, 700.0);
    let child = f.doc.get(i).unwrap().children()[0];
    assert_eq!(
        f.inside(i),
        Some(f.wide),
        "the fixture: the document's Wide"
    );
    let set = f.doc.modes_at(child);
    assert_eq!(
        f.doc.variables().kit_mode_at(KIT, "Density", &set),
        Some(kc),
        "the kit's own Compact, not Wide mapped"
    );
}
