//! A flex direction and wrap bound to string variables (§5.3f): only a string
//! scoped to them binds, it holds only their words, the layout draws the mode's
//! word, a write keeps or detaches by the two-half rule, and a deletion detaches
//! to what was drawn.

use ondin_core::container::{Display, Flex, FlexDirection, FlexWrap};
use ondin_core::kurbo::{RoundedRectRadii, Size};
use ondin_core::variable::{self, Collection, Field, Scope, VarRule};
use ondin_core::{
    Cell, Document, GeometryPatch, IdSource, Mode, ModeId, NodeId, NodeKind, OpError, Operation,
    Transaction, Value, VarId, VarKind, Variable, build,
};

/// Breakpoint (Mobile, Tablet ≥ 768) picked by width; *Home* a top-level frame
/// 1024 wide holding *cards*, a flex row, holding *tile*, a rectangle; `flow` a
/// string scoped to flex direction reading Column, then Row; `width` a number
/// reading 60.
struct F {
    doc: Document,
    home: NodeId,
    cards: NodeId,
    tile: NodeId,
    flow: VarId,
    loose: VarId,
    width: VarId,
    mobile: ModeId,
    tablet: ModeId,
}

fn frame(id: NodeId, parent: NodeId, w: f64) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index: 0,
        kind: NodeKind::Artboard {
            size: Size::new(w, 300.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: None,
        name: None,
    }
}

fn string(id: VarId, name: &str, cells: [(ModeId, &str); 2], scope: Option<Scope>) -> Variable {
    Variable {
        id,
        name: name.into(),
        kind: VarKind::String,
        cells: cells
            .into_iter()
            .map(|(m, s)| (m, Cell::String(s.into())))
            .collect(),
        scope,
        origin: None,
        forked: None,
        description: String::new(),
    }
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xF1);
    let page = ids.mint();
    let mut doc = Document::new(page);
    let (home, cards, tile) = (ids.mint(), ids.mint(), ids.mint());
    let (mobile, tablet) = (ids.mint_mode(), ids.mint_mode());
    let bp = ids.mint_collection();
    let (flow, loose, width) = (ids.mint_var(), ids.mint_var(), ids.mint_var());
    doc.apply(&Transaction(vec![
        frame(home, page, 1024.0),
        frame(cards, home, 400.0),
        Operation::CreateNode {
            id: tile,
            parent: cards,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 20.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::SetDisplay {
            id: cards,
            display: Some(Display::Flex(Flex::default())),
        },
        Operation::AddCollection {
            collection: Collection {
                id: bp,
                name: "Breakpoint".into(),
                modes: vec![
                    Mode {
                        id: mobile,
                        name: "Mobile".into(),
                    },
                    Mode {
                        id: tablet,
                        name: "Tablet".into(),
                    },
                ],
                breakpoints: Some([(mobile, 0.0), (tablet, 768.0)].into()),
            },
            index: 1,
        },
        Operation::AddVariable {
            var: string(
                flow,
                "page/cards-flow",
                [(mobile, "Column"), (tablet, "Row")],
                Some(Scope::of([Field::FlexDirection])),
            ),
        },
        Operation::AddVariable {
            var: string(loose, "label", [(mobile, "Column"), (tablet, "Go")], None),
        },
        Operation::AddVariable {
            var: Variable {
                id: width,
                name: "tile/width".into(),
                kind: VarKind::Number,
                cells: [(mobile, Cell::Number(60.0)), (tablet, Cell::Number(60.0))].into(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        },
    ]))
    .expect("a page, a by-width collection and a flex frame");
    F {
        doc,
        home,
        cards,
        tile,
        flow,
        loose,
        width,
        mobile,
        tablet,
    }
}

/// `doc` with `ops` committed through the flow and size passes, in the
/// session's order.
fn commit(doc: &mut Document, ops: Vec<Operation>) {
    let tx = build::keep_flow_bindings(doc, Transaction(ops));
    let tx = build::keep_size_bindings(doc, None, tx);
    doc.apply(&tx).expect("committed");
}

fn cell(id: VarId, mode: ModeId, s: &str) -> Operation {
    Operation::SetVariableCell {
        id,
        mode,
        cell: Cell::String(s.into()),
    }
}

fn direction(doc: &Document, id: NodeId) -> FlexDirection {
    match doc.resolved_display(id).as_deref() {
        Some(Display::Flex(f)) => f.direction,
        other => panic!("not flex: {other:?}"),
    }
}

fn bind(var: VarId, f: &mut F) -> Result<(), OpError> {
    let ops = build::bind_flow_ops(&f.doc, f.cards, 0, Some(var));
    f.doc.apply(&Transaction(ops)).map(|_| ())
}

/// The bound direction draws its mode's word, and the frame's width picks the
/// mode: Row at 1024, Column at 600.
#[test]
fn a_bound_direction_draws_the_word_its_mode_holds() {
    let mut f = fixture();
    bind(f.flow, &mut f).unwrap();
    assert_eq!(direction(&f.doc, f.cards), FlexDirection::Row);
    f.doc
        .apply(&Transaction(vec![Operation::SetGeometry {
            id: f.home,
            geometry: GeometryPatch::Size(Size::new(600.0, 300.0)),
        }]))
        .unwrap();
    assert_eq!(direction(&f.doc, f.cards), FlexDirection::Column);
}

/// An unscoped string never binds a direction: a label's words are not one.
#[test]
fn an_unscoped_string_does_not_bind_a_direction() {
    let mut f = fixture();
    let r = bind(f.loose, &mut f);
    assert!(
        matches!(r, Err(OpError::Variables(VarRule::OutOfScope(_)))),
        "{r:?}"
    );
}

/// A string scoped to flex direction alone holds only its words: scoping one
/// sets each other word to *Row*, and a cell typed another word is refused.
#[test]
fn a_direction_string_holds_only_its_words() {
    let mut f = fixture();
    let ops = variable::scope_ops(&f.doc, f.loose, Some(Scope::of([Field::FlexDirection])));
    f.doc.apply(&Transaction(ops)).expect("rewritten to words");
    let v = f.doc.variables().get(f.loose).unwrap();
    assert!(
        v.cells
            .values()
            .all(|c| matches!(c, Cell::String(s) if s == "Column" || s == "Row")),
        "{:?}",
        v.cells
    );
    let mode = *v.cells.keys().next().unwrap();
    let r = f.doc.apply(&Transaction(vec![Operation::SetVariableCell {
        id: f.flow,
        mode,
        cell: Cell::String("Sideways".into()),
    }]));
    assert!(matches!(r, Err(OpError::Variables(VarRule::NotAWord(_)))));
    assert_eq!(
        FlexWrap::from_word("wrap-reverse"),
        Some(FlexWrap::WrapReverse)
    );
}

/// Scoping a string to direction words leaves a variable aliasing it reading
/// what it read: the rewritten cell's old value is written into the alias.
/// flip: `scope_ops` without its alias pass → fails at *"Go kept"*.
#[test]
fn scoping_a_string_to_words_keeps_what_its_aliases_read() {
    let mut f = fixture();
    let text = IdSource::new(0xF2).mint_var();
    f.doc
        .apply(&Transaction(vec![Operation::AddVariable {
            var: Variable {
                id: text,
                name: "button/text".into(),
                kind: VarKind::String,
                cells: [
                    (f.mobile, Cell::Alias(f.loose)),
                    (f.tablet, Cell::Alias(f.loose)),
                ]
                .into(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        }]))
        .expect("an alias of label");
    let ops = variable::scope_ops(&f.doc, f.loose, Some(Scope::of([Field::FlexDirection])));
    f.doc.apply(&Transaction(ops)).expect("scoped");
    let t = f.doc.variables();
    assert_eq!(
        t.resolve(text, f.tablet),
        Some(Value::String("Go".into())),
        "Go kept"
    );
    assert_eq!(
        t.get(text).unwrap().cells[&f.mobile],
        Cell::Alias(f.loose),
        "a word the scope takes stays an alias"
    );
}

/// A string another collection aliases is not scoped to direction words: that
/// alias reads it per mode of the string's own collection, which no literal of
/// its own could keep, so the narrowing is refused and names it.
/// flip: `scope_ops` without its `scope_held_by` refusal → fails at "refused".
#[test]
fn a_string_aliased_from_another_collection_is_not_scoped_to_words() {
    let mut f = fixture();
    let mut ids = IdSource::new(0xF3);
    let (theme, light, text) = (ids.mint_collection(), ids.mint_mode(), ids.mint_var());
    f.doc
        .apply(&Transaction(vec![
            Operation::AddCollection {
                collection: Collection {
                    id: theme,
                    name: "Theme".into(),
                    modes: vec![Mode {
                        id: light,
                        name: "Light".into(),
                    }],
                    breakpoints: None,
                },
                index: 2,
            },
            Operation::AddVariable {
                var: Variable {
                    id: text,
                    name: "button/text".into(),
                    kind: VarKind::String,
                    cells: [(light, Cell::Alias(f.loose))].into(),
                    scope: None,
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ]))
        .expect("a second collection aliasing label");
    let words = Some(Scope::of([Field::FlexDirection]));
    assert_eq!(variable::scope_held_by(&f.doc, f.loose, words), Some(text));
    assert!(
        variable::scope_ops(&f.doc, f.loose, words).is_empty(),
        "refused"
    );
    let any = Some(Scope::of([Field::Content]));
    assert_eq!(
        variable::scope_held_by(&f.doc, f.loose, any),
        None,
        "a scope that keeps every word is not held"
    );
}

/// A layout written with the binding kept stays bound when it writes what is
/// drawn or the literal; a new direction detaches it, a cleared binding stays
/// cleared.
/// flip: `kept_flow` keeping the binding over any direction → fails at
/// "a picked direction detaches".
#[test]
fn a_write_keeps_or_detaches_by_the_two_half_rule() {
    let mut f = fixture();
    bind(f.flow, &mut f).unwrap();
    let stored = match f.doc.get(f.cards).unwrap().display() {
        Some(Display::Flex(x)) => *x,
        _ => unreachable!(),
    };
    let write = |direction| {
        Display::Flex(Flex {
            direction,
            ..stored
        })
    };
    let kept = build::kept_flow(&f.doc, f.cards, &write(FlexDirection::Row));
    assert!(matches!(kept, Display::Flex(x) if x.direction_var == Some(f.flow)));
    let picked = build::kept_flow(&f.doc, f.cards, &write(FlexDirection::ColumnReverse));
    assert!(
        matches!(picked, Display::Flex(x) if x.direction_var.is_none()
            && x.direction == FlexDirection::ColumnReverse),
        "a picked direction detaches"
    );
    let cleared = Display::Flex(Flex {
        direction_var: None,
        ..stored
    });
    assert_eq!(build::kept_flow(&f.doc, f.cards, &cleared), cleared);
}

/// Deleting the variable detaches the direction to the one it drew.
#[test]
fn deleting_the_variable_detaches_to_what_was_drawn() {
    let mut f = fixture();
    bind(f.flow, &mut f).unwrap();
    f.doc
        .apply(&Transaction(vec![Operation::SetGeometry {
            id: f.home,
            geometry: GeometryPatch::Size(Size::new(600.0, 300.0)),
        }]))
        .unwrap();
    let ops = variable::remove_ops(&f.doc, &[f.flow]);
    f.doc
        .apply(&Transaction(ops))
        .expect("detached and removed");
    assert!(f.doc.get(f.cards).unwrap().flow_vars()[0].is_none());
    assert_eq!(direction(&f.doc, f.cards), FlexDirection::Column);
}

/// The binding rides in the layout through a save and a load.
#[test]
fn the_binding_round_trips() {
    let mut f = fixture();
    bind(f.flow, &mut f).unwrap();
    let bytes = ondin_core::io::save(&f.doc).unwrap();
    let back = ondin_core::io::load(&bytes).unwrap();
    assert_eq!(back.get(f.cards).unwrap().flow_vars()[0], Some(f.flow));
    assert_eq!(ondin_core::io::save(&back).unwrap(), bytes);
}

/// A bound width some mode's direction stretches: binding the direction detaches
/// it, though the mode in force draws a row where the width is the tile's own;
/// and no width binds back while the direction is bound.
/// flip: `keep_size_bindings` judging with `size_axes_authored` (the mode in
/// force) → fails at "detached by the Column mode".
#[test]
fn binding_a_direction_detaches_a_width_another_mode_stretches() {
    let mut f = fixture();
    let ops = build::bind_size_ops(&f.doc, f.tile, [Some(f.width), None]);
    f.doc.apply(&Transaction(ops)).unwrap();
    assert_eq!(f.doc.get(f.tile).unwrap().size_vars()[0], Some(f.width));
    let ops = build::bind_flow_ops(&f.doc, f.cards, 0, Some(f.flow));
    commit(&mut f.doc, ops);
    assert_eq!(f.doc.get(f.cards).unwrap().flow_vars()[0], Some(f.flow));
    assert_eq!(direction(&f.doc, f.cards), FlexDirection::Row);
    assert_eq!(
        f.doc.get(f.tile).unwrap().size_vars()[0],
        None,
        "detached by the Column mode"
    );
    assert_eq!(build::bindable_size_axes(&f.doc, f.tile), [false, false]);
}

/// A cell turning a bound direction in a mode not in force detaches a width that
/// direction stretches.
/// flip: the `SetVariableCell` arm out of `keep_size_bindings`' layout half →
/// fails at "detached by the cell".
#[test]
fn a_cell_turning_a_direction_detaches_a_width() {
    let mut f = fixture();
    commit(&mut f.doc, vec![cell(f.flow, f.mobile, "Row")]);
    let ops = build::bind_flow_ops(&f.doc, f.cards, 0, Some(f.flow));
    commit(&mut f.doc, ops);
    let ops = build::bind_size_ops(&f.doc, f.tile, [Some(f.width), None]);
    commit(&mut f.doc, ops);
    assert_eq!(
        f.doc.get(f.tile).unwrap().size_vars()[0],
        Some(f.width),
        "a row in every mode leaves the width the tile's"
    );
    commit(&mut f.doc, vec![cell(f.flow, f.mobile, "Column")]);
    assert_eq!(
        f.doc.get(f.tile).unwrap().size_vars()[0],
        None,
        "detached by the cell"
    );
    assert_eq!(f.doc.get(f.cards).unwrap().flow_vars()[0], Some(f.flow));
}

/// A string scoped wider than flex direction stays bound while every cell names
/// one; a cell naming none detaches it at the commit to the direction drawn.
/// flip: `keep_flow_bindings` without its every-mode judgement → fails at
/// "a label detaches it".
#[test]
fn a_string_naming_no_direction_detaches_at_the_commit() {
    let mut f = fixture();
    let wide = Some(Scope::of([Field::FlexDirection, Field::Content]));
    let mut ops = variable::scope_ops(&f.doc, f.loose, wide);
    ops.push(cell(f.loose, f.tablet, "Row"));
    commit(&mut f.doc, ops);
    assert!(variable::names_one_everywhere(
        f.doc.variables(),
        f.loose,
        Field::FlexDirection
    ));
    let ops = build::bind_flow_ops(&f.doc, f.cards, 0, Some(f.loose));
    commit(&mut f.doc, ops);
    assert_eq!(f.doc.get(f.cards).unwrap().flow_vars()[0], Some(f.loose));
    commit(&mut f.doc, vec![cell(f.loose, f.tablet, "Go")]);
    assert_eq!(
        f.doc.get(f.cards).unwrap().flow_vars()[0],
        None,
        "a label detaches it"
    );
    assert_eq!(direction(&f.doc, f.cards), FlexDirection::Row);
}

/// The cut lands on the direction drawn before the edit, not on the placeholder
/// the binding stored: bound while Row, drawn Column, then cut to Column.
/// flip: `keep_flow_bindings` taking the direction drawn after the edit → fails
/// at "what it drew".
#[test]
fn a_cut_direction_keeps_what_it_drew() {
    let mut f = fixture();
    let wide = Some(Scope::of([Field::FlexDirection, Field::Content]));
    let mut ops = variable::scope_ops(&f.doc, f.loose, wide);
    ops.push(cell(f.loose, f.tablet, "Row"));
    commit(&mut f.doc, ops);
    let ops = build::bind_flow_ops(&f.doc, f.cards, 0, Some(f.loose));
    commit(&mut f.doc, ops);
    commit(&mut f.doc, vec![cell(f.loose, f.tablet, "Column")]);
    assert_eq!(direction(&f.doc, f.cards), FlexDirection::Column);
    commit(&mut f.doc, vec![cell(f.loose, f.tablet, "Go")]);
    assert_eq!(f.doc.get(f.cards).unwrap().flow_vars()[0], None);
    assert_eq!(
        direction(&f.doc, f.cards),
        FlexDirection::Column,
        "what it drew"
    );
}

/// Unscoping a direction's string to every field (`None`, which leaves out the
/// fields that need a scope) detaches the direction, so the commit holds.
/// flip: `remove_and_scope_ops` reading `None` as every field → fails at
/// "unscoped".
#[test]
fn unscoping_a_direction_string_detaches_it() {
    let mut f = fixture();
    let ops = build::bind_flow_ops(&f.doc, f.cards, 0, Some(f.flow));
    commit(&mut f.doc, ops);
    let ops = variable::scope_ops(&f.doc, f.flow, None);
    f.doc.apply(&Transaction(ops)).expect("unscoped");
    assert_eq!(f.doc.get(f.cards).unwrap().flow_vars()[0], None);
    assert_eq!(direction(&f.doc, f.cards), FlexDirection::Row);
}

/// **Binding a size axis takes an aspect ratio off**, rather than the ratio
/// reading the bound axis's stored 0 as a size: the tile's height stays 20.
/// flip: `keep_ratios` reading no `SetSizeVars` as binding → fails at *"the ratio
/// off"*.
#[test]
fn binding_a_size_takes_a_ratio_off() {
    use ondin_core::container::{AspectRatio, RatioSide};
    let mut f = fixture();
    let mut item = *f.doc.get(f.tile).unwrap().item();
    item.aspect_ratio = Some(AspectRatio {
        ratio: 2.0,
        side: RatioSide::Height,
    });
    let tx = build::keep_ratios(
        &f.doc,
        Transaction(vec![Operation::SetLayoutItem { id: f.tile, item }]),
    );
    f.doc.apply(&tx).expect("a ratio");
    let ops = build::bind_size_ops(&f.doc, f.tile, [Some(f.width), None]);
    let tx = build::keep_ratios(&f.doc, Transaction(ops));
    f.doc.apply(&tx).expect("bound");
    let tile = f.doc.get(f.tile).unwrap();
    assert_eq!(tile.size_vars()[0], Some(f.width));
    assert_eq!(tile.item().aspect_ratio, None, "the ratio off");
    assert_eq!(
        ondin_core::edit::resizable_size(tile.kind()).map(|s| s.height),
        Some(20.0),
        "the height kept"
    );
}
