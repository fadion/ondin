//! Variable collections and scope (§5.3f): each collection's modes inherited on
//! their own, aliases across collections, the save format, the loop check, the
//! collection verbs, and a scope held at the commit and narrowed by detaching.

use ondin_core::Brush;
use ondin_core::kurbo::{RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::variable::{
    self, Collection, CollectionId, Field, ModeOwner, Scope, VarRule, Variables,
};
use ondin_core::{
    BrushSource, Cell, Document, Fill, IdSource, Mode, ModeId, NodeId, NodeKind, OpError,
    Operation, Transaction, Value, VarId, VarKind, Variable, keyed_by_position,
};

const WHITE: Color = Color::from_rgba8(255, 255, 255, 255);
const NAVY: Color = Color::from_rgba8(10, 20, 80, 255);

/// Colour (Light, Dark) is the first collection; Spacing (Compact,
/// Comfortable) the second. `brand` is a colour, `gap` a number, `radius` a
/// number of Spacing too.
struct F {
    doc: Document,
    ids: IdSource,
    page: NodeId,
    frame: NodeId,
    rect: NodeId,
    spacing: CollectionId,
    light: ModeId,
    dark: ModeId,
    compact: ModeId,
    comfortable: ModeId,
    brand: VarId,
    gap: VarId,
    radius: VarId,
}

fn mode(id: ModeId, name: &str) -> Mode {
    Mode {
        id,
        name: name.into(),
    }
}

fn var(id: VarId, name: &str, kind: VarKind, cells: Vec<(ModeId, Cell)>) -> Variable {
    Variable {
        id,
        name: name.into(),
        kind,
        cells: cells.into_iter().collect(),
        scope: None,
        origin: None,
        forked: None,
        description: String::new(),
    }
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xC0);
    let page = ids.mint();
    let mut doc = Document::new(page);
    let (frame, rect) = (ids.mint(), ids.mint());
    let (light, dark, compact, comfortable) = (
        ids.mint_mode(),
        ids.mint_mode(),
        ids.mint_mode(),
        ids.mint_mode(),
    );
    let spacing = ids.mint_collection();
    let (brand, gap, radius) = (ids.mint_var(), ids.mint_var(), ids.mint_var());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: frame,
            parent: page,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(100.0, 100.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: rect,
            parent: frame,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 20.0),
                corner_radii: RoundedRectRadii::from_single_radius(2.0),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::SetCollectionName {
            id: CollectionId::FIRST,
            name: "Colour".into(),
        },
        Operation::AddMode {
            collection: CollectionId::FIRST,
            mode: mode(light, "Light"),
            index: 0,
            cells: vec![],
        },
        Operation::AddMode {
            collection: CollectionId::FIRST,
            mode: mode(dark, "Dark"),
            index: 1,
            cells: vec![],
        },
        Operation::AddCollection {
            collection: Collection {
                id: spacing,
                name: "Spacing".into(),
                modes: vec![mode(compact, "Compact"), mode(comfortable, "Comfortable")],
            },
            index: 1,
        },
        Operation::AddVariable {
            var: var(
                brand,
                "brand",
                VarKind::Color,
                vec![(light, Cell::Color(WHITE)), (dark, Cell::Color(NAVY))],
            ),
        },
        Operation::AddVariable {
            var: var(
                gap,
                "space/md",
                VarKind::Number,
                vec![
                    (compact, Cell::Number(8.0)),
                    (comfortable, Cell::Number(12.0)),
                ],
            ),
        },
        Operation::AddVariable {
            var: var(
                radius,
                "radius/md",
                VarKind::Number,
                vec![
                    (compact, Cell::Number(4.0)),
                    (comfortable, Cell::Number(6.0)),
                ],
            ),
        },
    ]))
    .expect("two collections, a frame and a rect");
    F {
        doc,
        ids,
        page,
        frame,
        rect,
        spacing,
        light,
        dark,
        compact,
        comfortable,
        brand,
        gap,
        radius,
    }
}

fn bind_fill(doc: &mut Document, id: NodeId, var: VarId) {
    doc.apply(&Transaction(vec![Operation::SetFills {
        id,
        fills: keyed_by_position([Fill {
            brush: BrushSource::Bound { var, alpha: 1.0 },
            visible: true,
        }]),
    }]))
    .expect("a bound fill");
}

fn bind_corners(doc: &mut Document, id: NodeId, var: VarId) -> Result<(), OpError> {
    doc.apply(&Transaction(vec![Operation::SetCornerVars {
        id,
        vars: [Some(var); 4],
    }]))
    .map(|_| ())
}

fn fill(doc: &Document, id: NodeId) -> Brush {
    let f = &doc.get(id).unwrap().paint().fills[0];
    doc.resolve_brush(id, &f.brush).into_owned()
}

fn set_modes(doc: &mut Document, id: NodeId, modes: Vec<ModeId>) {
    doc.apply(&Transaction(vec![Operation::SetModes { id, modes }]))
        .expect("modes set");
}

/// **Each collection inherits on its own**: a frame set to Dark still takes
/// Compact from its page, and its page set to Comfortable moves the spacing
/// without moving the colour.
///
/// flip: `modes_at_in` keeping only the nearest frame's modes → fails at
/// *"Comfortable from the page"*.
#[test]
fn each_collection_inherits_on_its_own() {
    let mut f = fixture();
    bind_fill(&mut f.doc, f.rect, f.brand);
    let t = f.doc.variables();
    assert_eq!(t.collections().len(), 2);
    assert_eq!(t.collection_of(f.gap).map(|c| c.id), Some(f.spacing));
    set_modes(&mut f.doc, f.frame, vec![f.dark]);
    set_modes(&mut f.doc, f.page, vec![f.comfortable]);
    assert_eq!(
        fill(&f.doc, f.rect),
        Brush::Solid(NAVY),
        "Dark from the frame"
    );
    assert_eq!(
        f.doc.resolve_var(f.rect, f.gap),
        Some(Value::Number(12.0)),
        "Comfortable from the page"
    );
    assert_eq!(f.doc.mode_at(f.rect, f.spacing), Some(f.comfortable));
    assert_eq!(f.doc.mode_at(f.rect, CollectionId::FIRST), Some(f.dark));
    // One pick in the frame's row keeps every other collection's mode.
    let t = f.doc.variables();
    let both = variable::with_mode(
        t,
        &[f.dark],
        ModeOwner::Collection(f.spacing),
        Some(f.compact),
    );
    assert_eq!(both.len(), 2);
    set_modes(&mut f.doc, f.frame, both);
    assert_eq!(fill(&f.doc, f.rect), Brush::Solid(NAVY));
    assert_eq!(f.doc.resolve_var(f.rect, f.gap), Some(Value::Number(8.0)));
}

/// A frame sets at most one mode per collection; two of one are refused.
#[test]
fn a_frame_sets_one_mode_per_collection() {
    let mut f = fixture();
    let r = f.doc.apply(&Transaction(vec![Operation::SetModes {
        id: f.frame,
        modes: vec![f.light, f.dark],
    }]));
    assert!(
        matches!(r, Err(OpError::Variables(VarRule::TwoModes(_)))),
        "{r:?}"
    );
}

/// **An alias may cross collections**, its target reading its own
/// collection's mode in force at the layer.
#[test]
fn an_alias_across_collections_reads_its_targets_mode() {
    let mut f = fixture();
    let alias = f.ids.mint_var();
    f.doc
        .apply(&Transaction(vec![Operation::AddVariable {
            var: var(
                alias,
                "gutter",
                VarKind::Number,
                vec![(f.light, Cell::Alias(f.gap)), (f.dark, Cell::Number(2.0))],
            ),
        }]))
        .expect("an alias into Spacing");
    set_modes(&mut f.doc, f.page, vec![f.comfortable]);
    assert_eq!(f.doc.resolve_var(f.rect, alias), Some(Value::Number(12.0)));
    set_modes(&mut f.doc, f.frame, vec![f.dark]);
    assert_eq!(f.doc.resolve_var(f.rect, alias), Some(Value::Number(2.0)));
}

/// **The loop check is exact across collections**: a chain that returns to
/// its start only by reading one collection in two modes at once is no loop,
/// and a real loop through another collection is refused.
///
/// flip: `reaches` branching over every column of a collection already
/// chosen → fails at *"no layer walks it"*.
#[test]
fn a_loop_across_collections_is_refused_and_an_unwalkable_chain_is_not() {
    let mut f = fixture();
    let (a, b, a2) = (f.ids.mint_var(), f.ids.mint_var(), f.ids.mint_var());
    // a (Light) → b; b (both) → a2; a2 (Dark) → a. Walking from a in Light
    // reaches a2 in Light, a value: no layer walks back to a.
    let chain = vec![
        Operation::AddVariable {
            var: var(
                a2,
                "a2",
                VarKind::Number,
                vec![(f.light, Cell::Number(1.0)), (f.dark, Cell::Number(0.0))],
            ),
        },
        Operation::AddVariable {
            var: var(
                b,
                "b",
                VarKind::Number,
                vec![
                    (f.compact, Cell::Alias(a2)),
                    (f.comfortable, Cell::Alias(a2)),
                ],
            ),
        },
        Operation::AddVariable {
            var: var(
                a,
                "a",
                VarKind::Number,
                vec![(f.light, Cell::Alias(b)), (f.dark, Cell::Number(3.0))],
            ),
        },
        Operation::SetVariableCell {
            id: a2,
            mode: f.dark,
            cell: Cell::Alias(a),
        },
    ];
    f.doc.apply(&Transaction(chain)).expect("no layer walks it");
    assert_eq!(
        f.doc.variables().resolve(a, f.light),
        Some(Value::Number(1.0))
    );
    // a2 (Light) → a too closes one: a (Light) → b → a2 (Light) → a.
    let r = f.doc.apply(&Transaction(vec![Operation::SetVariableCell {
        id: a2,
        mode: f.light,
        cell: Cell::Alias(a),
    }]));
    assert!(
        matches!(r, Err(OpError::Variables(VarRule::AliasLoop(_)))),
        "{r:?}"
    );
}

/// **A document with one collection saves the bytes it did before
/// collections**; two collections and a frame setting a mode of each round-trip
/// whole.
///
/// flip: `VariablesDto::from_table` writing the first collection's name always
/// → fails at *"no first collection name"*.
#[test]
fn collections_round_trip_and_one_writes_as_before() {
    let mut f = fixture();
    set_modes(&mut f.doc, f.frame, vec![f.dark, f.compact]);
    let bytes = ondin_core::io::save(&f.doc).unwrap();
    let back = ondin_core::io::load(&bytes).unwrap();
    assert_eq!(back.variables(), f.doc.variables());
    assert_eq!(back.get(f.frame).unwrap().modes().len(), 2);
    assert_eq!(ondin_core::io::save(&back).unwrap(), bytes);

    let mut ids = IdSource::new(0xC1);
    let mut one = Document::new(ids.mint());
    let m = ids.mint_mode();
    one.apply(&Transaction(vec![Operation::AddMode {
        collection: CollectionId::FIRST,
        mode: mode(m, "Default"),
        index: 0,
        cells: vec![],
    }]))
    .unwrap();
    let text = String::from_utf8(ondin_core::io::save(&one).unwrap()).unwrap();
    assert!(!text.contains("\"collection"), "no collection key: {text}");
    assert!(!text.contains("\"Variables\""), "no first collection name");
    assert_eq!(
        ondin_core::io::load(text.as_bytes()).unwrap().variables(),
        one.variables()
    );
}

/// **A binding outside its variable's scope is refused at the commit**, and
/// a variable scoped to a family binds there.
#[test]
fn a_binding_outside_its_scope_is_refused() {
    let mut f = fixture();
    let gap = f.doc.variables().get(f.gap).unwrap().clone();
    f.doc
        .apply(&Transaction(vec![Operation::SetVariable {
            var: Variable {
                scope: Some(Scope::of([Field::Spacing])),
                ..gap
            },
        }]))
        .unwrap();
    let r = bind_corners(&mut f.doc, f.rect, f.gap);
    assert!(
        matches!(r, Err(OpError::Variables(VarRule::OutOfScope(_)))),
        "{r:?}"
    );
    bind_corners(&mut f.doc, f.rect, f.radius).expect("an unscoped one binds");
}

/// **Narrowing a scope detaches what it drops**, in one transaction, to the
/// value it drew; the counts beside the boxes see each family.
///
/// flip: `scope_ops` without its `detach_ops` → fails at *"detached, then
/// scoped"*, the commit refusing `OutOfScope`.
#[test]
fn narrowing_a_scope_detaches_what_it_drops() {
    let mut f = fixture();
    bind_corners(&mut f.doc, f.rect, f.radius).unwrap();
    let by_field = variable::users_by_field(&f.doc, f.radius);
    let corner = by_field.iter().find(|(x, _)| *x == Field::Corner).unwrap();
    assert_eq!(corner.1, vec![f.rect]);
    let ops = variable::scope_ops(&f.doc, f.radius, Some(Scope::of([Field::Spacing])));
    f.doc
        .apply(&Transaction(ops))
        .expect("detached, then scoped");
    let rect = f.doc.get(f.rect).unwrap();
    assert_eq!(rect.corner_vars(), [None; 4]);
    assert_eq!(
        rect.kind().corner_radii().unwrap().top_left,
        4.0,
        "the radius it drew in Compact"
    );
    assert!(
        !f.doc
            .variables()
            .get(f.radius)
            .unwrap()
            .binds(Field::Corner)
    );
}

/// **Move to** keeps the id, so every binding holds, and takes each cell from
/// the source's mode of the same name, else its default; a name the target
/// holds is refused.
#[test]
fn moving_a_variable_keeps_its_bindings() {
    let mut f = fixture();
    bind_corners(&mut f.doc, f.rect, f.radius).unwrap();
    let ops = variable::move_ops(&f.doc, &[f.radius], CollectionId::FIRST).unwrap();
    f.doc.apply(&Transaction(ops)).expect("moved");
    let t = f.doc.variables();
    assert_eq!(
        t.collection_of(f.radius).map(|c| c.id),
        Some(CollectionId::FIRST)
    );
    assert_eq!(t.resolve(f.radius, f.dark), Some(Value::Number(4.0)));
    assert_eq!(
        f.doc.get(f.rect).unwrap().corner_vars(),
        [Some(f.radius); 4]
    );
    // `brand` in Spacing would be fine; a second `radius/md` would not.
    let twin = f.ids.mint_var();
    f.doc
        .apply(&Transaction(vec![Operation::AddVariable {
            var: var(
                twin,
                "radius/md",
                VarKind::Number,
                vec![
                    (f.compact, Cell::Number(1.0)),
                    (f.comfortable, Cell::Number(1.0)),
                ],
            ),
        }]))
        .expect("names are unique per collection, not across");
    assert_eq!(
        variable::move_ops(&f.doc, &[twin], CollectionId::FIRST),
        Err("radius/md".to_string())
    );
}

/// **Deleting a collection** detaches its users to what they drew, returns
/// frames set to its modes to *Inherit* for it, and one undo puts it all back.
#[test]
fn deleting_a_collection_detaches_and_inherits_in_one_step() {
    let mut f = fixture();
    bind_corners(&mut f.doc, f.rect, f.radius).unwrap();
    set_modes(&mut f.doc, f.frame, vec![f.dark, f.comfortable]);
    assert_eq!(
        variable::collection_reach(&f.doc, f.spacing),
        (2, 1, 1),
        "two variables, one layer, one frame"
    );
    let before = f.doc.clone();
    let ops = variable::remove_collection_ops(&f.doc, f.spacing);
    let inverse = f.doc.apply(&Transaction(ops)).expect("deleted");
    assert!(f.doc.variables().collection(f.spacing).is_none());
    assert_eq!(f.doc.get(f.frame).unwrap().modes(), [f.dark]);
    let rect = f.doc.get(f.rect).unwrap();
    assert_eq!(rect.corner_vars(), [None; 4]);
    assert_eq!(rect.kind().corner_radii().unwrap().top_left, 6.0);
    f.doc.apply(&inverse.inverse).expect("undone");
    assert_eq!(f.doc.variables(), before.variables());
    assert_eq!(
        f.doc.get(f.frame).unwrap().modes(),
        before.get(f.frame).unwrap().modes()
    );
    // The last collection does not go.
    let mut one = Document::new(NodeId { actor: 9, seq: 1 });
    assert!(variable::remove_collection_ops(&one, CollectionId::FIRST).is_empty());
    let r = one.apply(&Transaction(vec![Operation::RemoveCollection {
        id: CollectionId::FIRST,
    }]));
    assert!(
        matches!(r, Err(OpError::Variables(VarRule::NoCollection))),
        "{r:?}"
    );
}

/// **Duplicate collection** copies its modes and variables under new ids, an
/// alias inside it pointing at the copy and one outside it kept.
#[test]
fn duplicating_a_collection_points_its_aliases_at_the_copies() {
    let mut f = fixture();
    let alias = f.ids.mint_var();
    f.doc
        .apply(&Transaction(vec![Operation::AddVariable {
            var: var(
                alias,
                "space/alias",
                VarKind::Number,
                vec![
                    (f.compact, Cell::Alias(f.gap)),
                    (f.comfortable, Cell::Number(1.0)),
                ],
            ),
        }]))
        .unwrap();
    let ops = variable::duplicate_collection_ops(&f.doc, f.spacing, "Spacing copy", &mut f.ids);
    f.doc.apply(&Transaction(ops)).expect("duplicated");
    let t: &Variables = f.doc.variables();
    let copy = t.collections()[2].clone();
    assert_eq!(copy.name, "Spacing copy");
    let copied_alias = t.by_name_in(copy.id, "space/alias").unwrap();
    let copied_gap = t.by_name_in(copy.id, "space/md").unwrap();
    assert_eq!(
        copied_alias.cells.get(&copy.modes[0].id),
        Some(&Cell::Alias(copied_gap.id))
    );
}

/// **A paste into another document rebinds to the namesake in the collection
/// of the same name**, and not to one whose scope refuses the field.
#[test]
fn a_paste_prefers_the_collection_of_the_same_name_and_respects_scope() {
    let mut f = fixture();
    bind_corners(&mut f.doc, f.rect, f.radius).unwrap();
    let copy = vec![f.doc.capture_subtree(f.rect).unwrap()];
    let carried = variable::carried(&f.doc, &copy);

    let mut ids = IdSource::new(0xC2);
    let mut other = Document::new(ids.mint());
    let (m1, m2, sp) = (ids.mint_mode(), ids.mint_mode(), ids.mint_collection());
    let (first, theirs) = (ids.mint_var(), ids.mint_var());
    other
        .apply(&Transaction(vec![
            Operation::AddMode {
                collection: CollectionId::FIRST,
                mode: mode(m1, "Default"),
                index: 0,
                cells: vec![],
            },
            Operation::AddCollection {
                collection: Collection {
                    id: sp,
                    name: "Spacing".into(),
                    modes: vec![mode(m2, "Compact")],
                },
                index: 1,
            },
            Operation::AddVariable {
                var: var(
                    first,
                    "radius/md",
                    VarKind::Number,
                    vec![(m1, Cell::Number(1.0))],
                ),
            },
            Operation::AddVariable {
                var: var(
                    theirs,
                    "radius/md",
                    VarKind::Number,
                    vec![(m2, Cell::Number(9.0))],
                ),
            },
        ]))
        .unwrap();
    let landed = variable::land(&other, &carried, copy[0].clone());
    assert_eq!(landed[0].corner_vars(), [Some(theirs); 4]);

    // Scoped away from corners there, it detaches to what the copy drew.
    let mut v = other.variables().get(theirs).unwrap().clone();
    v.scope = Some(Scope::of([Field::Spacing]));
    let mut w = other.variables().get(first).unwrap().clone();
    w.scope = Some(Scope::of([Field::Spacing]));
    other
        .apply(&Transaction(vec![
            Operation::SetVariable { var: v },
            Operation::SetVariable { var: w },
        ]))
        .unwrap();
    let landed = variable::land(&other, &carried, copy[0].clone());
    assert_eq!(landed[0].corner_vars(), [None; 4]);
    assert_eq!(landed[0].kind().corner_radii().unwrap().top_left, 4.0);
}

/// **Two variables sharing one node's list detach in one pass**: one corner
/// bound to `radius` and three to `gap`, both narrowed off corners — two passes
/// would each write the whole list and the second would rebind the first's.
///
/// flip: `remove_and_scope_ops` calling `detach_ops` once per scoped variable
/// → fails at the commit, `OutOfScope`.
#[test]
fn narrowing_two_scopes_on_one_list_detaches_both() {
    let mut f = fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetCornerVars {
            id: f.rect,
            vars: [Some(f.radius), Some(f.gap), Some(f.gap), Some(f.gap)],
        }]))
        .unwrap();
    let off = Some(Scope::of([Field::Spacing]));
    let mut ops = variable::remove_and_scope_ops(&f.doc, &[], &[(f.radius, off), (f.gap, off)]);
    for v in [f.radius, f.gap] {
        let held = f.doc.variables().get(v).unwrap().clone();
        ops.push(Operation::SetVariable {
            var: Variable { scope: off, ..held },
        });
    }
    f.doc.apply(&Transaction(ops)).expect("both detached");
    assert_eq!(f.doc.get(f.rect).unwrap().corner_vars(), [None; 4]);
}

/// Nothing moves into a collection with no mode: it has no column for the
/// cells, and the commit would refuse a variable with none.
#[test]
fn moving_into_a_modeless_collection_is_nothing() {
    let mut f = fixture();
    let empty = f.ids.mint_collection();
    f.doc
        .apply(&Transaction(vec![Operation::AddCollection {
            collection: Collection {
                id: empty,
                name: "Empty".into(),
                modes: Vec::new(),
            },
            index: 2,
        }]))
        .unwrap();
    assert_eq!(variable::move_ops(&f.doc, &[f.gap], empty), Ok(Vec::new()));
}
