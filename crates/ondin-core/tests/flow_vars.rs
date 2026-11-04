//! A flex direction and wrap bound to string variables (§5.3f): only a string
//! scoped to them binds, it holds only their words, the layout draws the mode's
//! word, a write keeps or detaches by the two-half rule, and a deletion detaches
//! to what was drawn.

use ondin_core::container::{Display, Flex, FlexDirection, FlexWrap};
use ondin_core::kurbo::{RoundedRectRadii, Size};
use ondin_core::variable::{self, Collection, Field, Scope, VarRule};
use ondin_core::{
    Cell, Document, GeometryPatch, IdSource, Mode, ModeId, NodeId, NodeKind, OpError, Operation,
    Transaction, VarId, VarKind, Variable, build,
};

/// Breakpoint (Mobile, Tablet ≥ 768) picked by width; *Home* a top-level frame
/// 1024 wide holding *cards*, a flex row; `flow` a string scoped to flex
/// direction reading Column, then Row.
struct F {
    doc: Document,
    home: NodeId,
    cards: NodeId,
    flow: VarId,
    loose: VarId,
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
    let (home, cards) = (ids.mint(), ids.mint());
    let (mobile, tablet) = (ids.mint_mode(), ids.mint_mode());
    let bp = ids.mint_collection();
    let (flow, loose) = (ids.mint_var(), ids.mint_var());
    doc.apply(&Transaction(vec![
        frame(home, page, 1024.0),
        frame(cards, home, 400.0),
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
    ]))
    .expect("a page, a by-width collection and a flex frame");
    F {
        doc,
        home,
        cards,
        flow,
        loose,
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
