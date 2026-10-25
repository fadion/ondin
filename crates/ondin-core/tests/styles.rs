//! Styles (§5.3f, §15 D1062): the table and its rules, the commit's settling —
//! a style applied or edited written onto its users, a slot written by the edit
//! detached — the save format, an instance's style as an override, the paste
//! rule, and a deleted variable inside a style.

use ondin_core::item::Keyed;
use ondin_core::kurbo::{RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::style::{Style, StyleId, StyleSlot, StyleValue};
use ondin_core::{
    Brush, BrushSource, Cell, Document, Fill, IdSource, Mode, NodeId, NodeKind, OpError, Operation,
    Placement, Transaction, VarKind, Variable,
};

const RED: Color = Color::from_rgba8(200, 30, 30, 255);
const BLUE: Color = Color::from_rgba8(20, 40, 220, 255);
const GREEN: Color = Color::from_rgba8(20, 200, 40, 255);

struct F {
    doc: Document,
    ids: IdSource,
    root: NodeId,
    frame: NodeId,
    /// Two rectangles in the frame, `a` red and `b` blue.
    a: NodeId,
    b: NodeId,
}

fn rect(id: NodeId, parent: NodeId) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index: 0,
        kind: NodeKind::Rect {
            size: Size::new(40.0, 20.0),
            corner_radii: RoundedRectRadii::from_single_radius(0.0),
        },
        transform: None,
        name: None,
    }
}

fn solid(ids: &mut IdSource, c: Color) -> Keyed<Fill> {
    Keyed::new(
        ids.mint_item(),
        Fill {
            brush: Brush::Solid(c).into(),
            visible: true,
        },
    )
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xBA);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (frame, a, b) = (ids.mint(), ids.mint(), ids.mint());
    let (ra, rb) = (solid(&mut ids, RED), solid(&mut ids, BLUE));
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(200.0, 200.0),
            },
            transform: None,
            name: None,
        },
        rect(a, frame),
        rect(b, frame),
        Operation::SetFills {
            id: a,
            fills: vec![ra],
        },
        Operation::SetFills {
            id: b,
            fills: vec![rb],
        },
    ]))
    .unwrap();
    F {
        doc,
        ids,
        root,
        frame,
        a,
        b,
    }
}

/// Commit as the app does: the doors' rewrites are the caller's, then what the
/// edit owes (`propagate::owed`, which settles the styles), then `apply`.
fn commit(f: &mut F, ops: Vec<Operation>) -> Result<Transaction, OpError> {
    let mut tx = Transaction(ops);
    ondin_core::propagate::owed(&f.doc, &mut tx, &mut f.ids);
    f.doc.apply(&tx).map(|o| o.inverse)
}

fn fill(doc: &Document, id: NodeId) -> Brush {
    doc.get(id).unwrap().paint().fills[0]
        .brush
        .literal()
        .cloned()
        .unwrap()
}

fn fill_style(doc: &Document, id: NodeId) -> Option<StyleId> {
    doc.get(id).unwrap().styles().fills
}

/// `a`'s fills made a style named *surface/raised*, `a` and `b` bound to it.
fn styled(f: &mut F) -> StyleId {
    let s = f.ids.mint_style();
    let (a, b) = (f.a, f.b);
    let ops =
        ondin_core::style::create_ops(&f.doc, s, "surface/raised", a, StyleSlot::Fills, &[a, b])
            .unwrap();
    commit(f, ops).unwrap();
    s
}

/// **Applying a style writes its values onto the layer** — the binding is the
/// only operation the verb writes, and the commit copies the style's list in
/// (`style::settle`), item ids and all.
///
/// Flip, run: dropping `owed`'s first `settle` leaves this green — the second
/// applies the style alone — which is why the first has a test of its own,
/// `a_mains_edit_of_a_styled_slot_reaches_its_copy` below.
#[test]
fn applying_a_style_writes_its_values_onto_the_layer() {
    let mut f = fixture();
    let s = styled(&mut f);
    assert_eq!(
        fill(&f.doc, f.b),
        Brush::Solid(RED),
        "b takes the style's red"
    );
    assert_eq!(fill_style(&f.doc, f.a), Some(s));
    assert_eq!(fill_style(&f.doc, f.b), Some(s));
    let style = f.doc.styles().get(s).unwrap();
    assert_eq!(
        StyleValue::of(f.doc.get(f.b).unwrap(), StyleSlot::Fills),
        Some(style.value.clone()),
        "a copy of the style's list, its item ids kept"
    );
}

/// **Editing a style reaches every user in the one commit, and one undo takes it
/// all back** (D1038 G9, live to every user).
#[test]
fn editing_a_style_rewrites_every_user_and_undoes_whole() {
    let mut f = fixture();
    let s = styled(&mut f);
    let mut value = f.doc.styles().get(s).unwrap().value.clone();
    if let StyleValue::Fills(v) = &mut value {
        v[0].value.brush = Brush::Solid(GREEN).into();
    }
    let undo = commit(&mut f, vec![Operation::SetStyleValue { id: s, value }]).unwrap();
    assert_eq!(fill(&f.doc, f.a), Brush::Solid(GREEN));
    assert_eq!(fill(&f.doc, f.b), Brush::Solid(GREEN));
    f.doc.apply(&undo).unwrap();
    assert_eq!(fill(&f.doc, f.a), Brush::Solid(RED));
    assert_eq!(fill(&f.doc, f.b), Brush::Solid(RED));
    assert_eq!(fill_style(&f.doc, f.b), Some(s));
}

/// **A slot written by the edit detaches its layer, values kept** (D1038 (1)):
/// the canvas, *Paste properties*, MCP — any door that writes `SetFills` on a
/// styled layer. The other user stays bound.
///
/// Flip, run: settling every mismatch by rewriting (the `written` arm dropped)
/// fails at *"the written blue stands"* — the style's red put back over it.
#[test]
fn a_slot_written_by_the_edit_detaches_its_layer() {
    let mut f = fixture();
    let s = styled(&mut f);
    let blue = solid(&mut f.ids, BLUE);
    let (a, b) = (f.a, f.b);
    commit(
        &mut f,
        vec![Operation::SetFills {
            id: b,
            fills: vec![blue],
        }],
    )
    .unwrap();
    assert_eq!(
        fill(&f.doc, f.b),
        Brush::Solid(BLUE),
        "the written blue stands"
    );
    assert_eq!(fill_style(&f.doc, f.b), None, "b is detached");
    assert_eq!(fill_style(&f.doc, f.a), Some(s), "a is not");
    // Writing back the style's own look keeps the binding.
    let same = f.doc.get(f.a).unwrap().paint().fills.clone();
    commit(&mut f, vec![Operation::SetFills { id: a, fills: same }]).ok();
    assert_eq!(fill_style(&f.doc, f.a), Some(s));
}

/// **Deleting a style detaches its users, their values kept, and a bare removal
/// of one in use is refused** (D1038 (6)).
#[test]
fn deleting_a_style_detaches_its_users() {
    let mut f = fixture();
    let s = styled(&mut f);
    let bare = commit(&mut f, vec![Operation::RemoveStyle { id: s }]);
    assert!(
        matches!(bare, Err(OpError::Styles(_))),
        "a removal leaving bindings is refused: {bare:?}"
    );
    let ops = ondin_core::style::remove_ops(&f.doc, &[s]);
    let undo = commit(&mut f, ops).unwrap();
    assert!(f.doc.styles().get(s).is_none());
    assert_eq!(fill_style(&f.doc, f.b), None);
    assert_eq!(fill(&f.doc, f.b), Brush::Solid(RED), "the values stay");
    f.doc.apply(&undo).unwrap();
    assert_eq!(
        fill_style(&f.doc, f.b),
        Some(s),
        "one undo brings both back"
    );
}

/// **Names are unique within a slot, not across slots.**
#[test]
fn a_name_is_unique_within_its_slot() {
    let mut f = fixture();
    styled(&mut f);
    let twin = Style {
        id: f.ids.mint_style(),
        name: "surface/raised".into(),
        value: StyleValue::Fills(Vec::new()),
        origin: None,
        forked: None,
        description: String::new(),
    };
    assert!(matches!(
        commit(&mut f, vec![Operation::AddStyle { style: twin }]),
        Err(OpError::Styles(_))
    ));
    let stroke = Style {
        id: f.ids.mint_style(),
        name: "surface/raised".into(),
        value: StyleValue::Strokes(Vec::new()),
        origin: None,
        forked: None,
        description: String::new(),
    };
    commit(&mut f, vec![Operation::AddStyle { style: stroke }])
        .expect("a stroke style may share a fill style's name");
}

/// **Renaming a group moves its styles of that slot**, subgroups too; a name
/// already held outside the group refuses it, and another slot's style of the
/// same path stays.
///
/// flip: `moved` in `rename_group_ops` keeping the old prefix → fails at the new name.
#[test]
fn renaming_a_style_group_moves_its_slots_styles() {
    let mut f = fixture();
    let s = styled(&mut f);
    let add = |f: &mut F, name: &str, value: StyleValue| {
        let id = f.ids.mint_style();
        let style = Style {
            id,
            name: name.into(),
            value,
            origin: None,
            forked: None,
            description: String::new(),
        };
        commit(f, vec![Operation::AddStyle { style }]).unwrap();
        id
    };
    let deep = add(&mut f, "surface/inset/low", StyleValue::Fills(Vec::new()));
    let stroke = add(&mut f, "surface/raised", StyleValue::Strokes(Vec::new()));
    let ops =
        ondin_core::style::rename_group_ops(&f.doc, StyleSlot::Fills, "surface", "card").unwrap();
    commit(&mut f, ops).unwrap();
    let name = |f: &F, id| f.doc.styles().get(id).unwrap().name.clone();
    assert_eq!(name(&f, s), "card/raised");
    assert_eq!(name(&f, deep), "card/inset/low");
    assert_eq!(name(&f, stroke), "surface/raised", "another slot's stays");
    add(&mut f, "raised", StyleValue::Fills(Vec::new()));
    assert_eq!(
        ondin_core::style::rename_group_ops(&f.doc, StyleSlot::Fills, "card", ""),
        Err("raised".to_string())
    );
}

/// The table and the bindings round-trip through a save; a file binding a
/// style it does not hold is refused.
#[test]
fn styles_round_trip_and_a_dangling_binding_is_refused() {
    let mut f = fixture();
    let s = styled(&mut f);
    let bytes = ondin_core::io::save(&f.doc).unwrap();
    let back = ondin_core::io::load(&bytes).unwrap();
    assert_eq!(back.styles(), f.doc.styles());
    assert_eq!(fill_style(&back, f.b), Some(s));
    assert_eq!(ondin_core::io::save(&back).unwrap(), bytes, "byte-stable");

    let text = String::from_utf8(bytes).unwrap();
    let broken = text.replacen("\"styles\": [", "\"styles_gone\": [", 1);
    assert_ne!(broken, text, "the fixture writes a styles table");
    assert!(ondin_core::io::load(broken.as_bytes()).is_err());
}

/// A style's and a variable's descriptions are saved and loaded, and absent from
/// the file while empty.
#[test]
fn descriptions_round_trip_and_are_absent_while_empty() {
    use ondin_core::Described;
    let mut f = fixture();
    let s = styled(&mut f);
    let empty = String::from_utf8(ondin_core::io::save(&f.doc).unwrap()).unwrap();
    assert!(
        !empty.contains("description"),
        "nothing written while empty"
    );
    let (mode, var) = (f.ids.mint_mode(), f.ids.mint_var());
    commit(
        &mut f,
        vec![
            Operation::AddMode {
                mode: Mode {
                    id: mode,
                    name: "Light".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: var,
                    name: "brand".into(),
                    kind: VarKind::Color,
                    cells: [(mode, Cell::Color(RED))].into(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::SetDescription {
                of: Described::Style(s),
                text: "Cards that float".into(),
            },
            Operation::SetDescription {
                of: Described::Variable(var),
                text: "The one accent".into(),
            },
        ],
    )
    .unwrap();
    let back = ondin_core::io::load(&ondin_core::io::save(&f.doc).unwrap()).unwrap();
    assert_eq!(
        back.styles().get(s).unwrap().description,
        "Cards that float"
    );
    assert_eq!(
        back.variables().get(var).unwrap().description,
        "The one accent"
    );
}

fn instance(f: &mut F) -> NodeId {
    f.doc
        .apply(&Transaction(vec![Operation::SetComponent {
            id: f.frame,
            component: true,
        }]))
        .unwrap();
    let place = Placement {
        nodes: f.doc.capture_subtree(f.frame).unwrap(),
        parent: f.root,
        index: None,
    };
    let (tx, made) = ondin_core::insert_subtrees(&f.doc, &mut f.ids, &[place], Default::default());
    f.doc.apply(&tx).unwrap();
    made[0]
}

fn copy_of(doc: &Document, inst: NodeId, main: NodeId) -> NodeId {
    *doc.get(inst)
        .unwrap()
        .children()
        .iter()
        .find(|c| doc.get(**c).unwrap().link() == Some(main))
        .unwrap()
}

/// **On an instance the main's own style is no override, another style is, and
/// a reset puts the main's back** (D1038 F9). A copy on another style keeps it —
/// its values too — when the main's style gains an item that the component
/// follow would otherwise carry onto it.
///
/// Flip, run: `owed`'s second `settle` dropped fails at *"the copy keeps its own
/// style's look"*: the follow inserts the main's new item into the copy's list,
/// and with nothing to put it back the copy draws two fills under one style.
#[test]
fn an_instances_style_is_an_override() {
    let mut f = fixture();
    let s = styled(&mut f);
    let inst = instance(&mut f);
    let (ca, cb) = (copy_of(&f.doc, inst, f.a), copy_of(&f.doc, inst, f.b));
    assert_eq!(
        fill_style(&f.doc, ca),
        Some(s),
        "a copy carries the binding"
    );

    // A second style, the copy of `b` moved onto it.
    let t = f.ids.mint_style();
    let blue = solid(&mut f.ids, BLUE);
    commit(
        &mut f,
        vec![
            Operation::AddStyle {
                style: Style {
                    id: t,
                    name: "surface/sunken".into(),
                    value: StyleValue::Fills(vec![blue]),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::SetFillStyle {
                id: cb,
                style: Some(t),
            },
        ],
    )
    .unwrap();
    assert_eq!(fill(&f.doc, cb), Brush::Solid(BLUE));

    // The main's style gains an item.
    let mut value = f.doc.styles().get(s).unwrap().value.clone();
    let green = solid(&mut f.ids, GREEN);
    if let StyleValue::Fills(v) = &mut value {
        v.push(green);
    }
    commit(&mut f, vec![Operation::SetStyleValue { id: s, value }]).unwrap();
    let len = |doc: &Document, id| doc.get(id).unwrap().paint().fills.len();
    assert_eq!(len(&f.doc, f.b), 2, "the main follows its style");
    assert_eq!(len(&f.doc, ca), 2, "a copy on the main's style follows it");
    assert_eq!(len(&f.doc, cb), 1, "the copy keeps its own style's look");
    assert_eq!(fill_style(&f.doc, cb), Some(t), "and its binding");

    // A reset of the copy puts the main's style back.
    let main = f.doc.get(f.b).unwrap();
    let ops = vec![
        Operation::SetFills {
            id: cb,
            fills: main.paint().fills.clone(),
        },
        Operation::SetFillStyle {
            id: cb,
            style: main.styles().fills,
        },
    ];
    commit(&mut f, ops).unwrap();
    assert_eq!(fill_style(&f.doc, cb), Some(s));
    assert_eq!(len(&f.doc, cb), 2);
}

/// **A main's own edit of a styled slot detaches the main and reaches its copy**
/// — the new value and the detach both, as any edit of a main does.
///
/// Flip, run: `owed`'s first `settle` dropped fails at *"the copy takes the
/// main's blue"*: the main's binding is cut only by the second, after the
/// component follows, so the copy keeps the style's binding, and that same
/// second pass puts the style's red back over the blue it was just given.
#[test]
fn a_mains_edit_of_a_styled_slot_reaches_its_copy() {
    let mut f = fixture();
    let s = styled(&mut f);
    let inst = instance(&mut f);
    let ca = copy_of(&f.doc, inst, f.a);
    let mut fills = f.doc.get(f.a).unwrap().paint().fills.clone();
    fills[0].value.brush = Brush::Solid(BLUE).into();
    let a = f.a;
    commit(&mut f, vec![Operation::SetFills { id: a, fills }]).unwrap();
    assert_eq!(fill_style(&f.doc, f.a), None, "the main is detached");
    assert_eq!(
        fill(&f.doc, ca),
        Brush::Solid(BLUE),
        "the copy takes the main's blue"
    );
    assert_eq!(fill_style(&f.doc, ca), None, "and the detach");
    assert_eq!(
        fill_style(&f.doc, f.b),
        Some(s),
        "the other user is untouched"
    );
}

/// **A paste keeps a style at home, rebinds to a namesake elsewhere — taking
/// its values — and detaches where there is none** (§15 D1062, D1038's rule for
/// a variable made for a style).
#[test]
fn a_paste_keeps_rebinds_or_detaches_a_style() {
    let mut f = fixture();
    let s = styled(&mut f);
    let copy = vec![f.doc.capture_subtree(f.b).unwrap()];
    let carried = ondin_core::variable::carried(&f.doc, &copy);
    let same = ondin_core::variable::land(&f.doc, &carried, copy[0].clone());
    assert_eq!(same[0].styles().fills, Some(s));

    // Another document with a *surface/raised* fill style of its own, green.
    let mut ids = IdSource::new(0xCE);
    let mut other = Document::new(ids.mint());
    let theirs = ids.mint_style();
    let green = solid(&mut ids, GREEN);
    other
        .apply(&Transaction(vec![Operation::AddStyle {
            style: Style {
                id: theirs,
                name: "surface/raised".into(),
                value: StyleValue::Fills(vec![green]),
                origin: None,
                forked: None,
                description: String::new(),
            },
        }]))
        .unwrap();
    let landed = ondin_core::variable::land(&other, &carried, copy[0].clone());
    assert_eq!(landed[0].styles().fills, Some(theirs));
    let root = other.first_page();
    let (tx, made) = ondin_core::insert_subtrees(
        &other,
        &mut ids,
        &[Placement {
            nodes: landed,
            parent: root,
            index: None,
        }],
        Default::default(),
    );
    let mut tx = tx;
    ondin_core::propagate::owed(&other, &mut tx, &mut ids);
    other.apply(&tx).unwrap();
    assert_eq!(
        fill(&other, made[0]),
        Brush::Solid(GREEN),
        "rebound, it takes that style's values"
    );

    // A document with no such style: detached, the red it was copied with.
    let bare = Document::new(ids.mint());
    let detached = ondin_core::variable::land(&bare, &carried, copy[0].clone());
    assert_eq!(detached[0].styles().fills, None);
    assert_eq!(
        detached[0].paint().fills[0].brush.literal(),
        Some(&Brush::Solid(RED))
    );
}

/// **Deleting a variable a style's paint reads detaches the style's paint to
/// the default mode's colour, and its users stay styled** (§15 D1062).
#[test]
fn deleting_a_variable_inside_a_style_detaches_the_style() {
    let mut f = fixture();
    let s = styled(&mut f);
    let (m, v) = (f.ids.mint_mode(), f.ids.mint_var());
    let mut value = f.doc.styles().get(s).unwrap().value.clone();
    if let StyleValue::Fills(list) = &mut value {
        list[0].value.brush = BrushSource::Bound { var: v, alpha: 1.0 };
    }
    commit(
        &mut f,
        vec![
            Operation::AddMode {
                mode: Mode {
                    id: m,
                    name: "Default".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: v,
                    name: "brand".into(),
                    kind: VarKind::Color,
                    cells: [(m, Cell::Color(GREEN))].into(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
            Operation::SetStyleValue { id: s, value },
        ],
    )
    .unwrap();
    assert_eq!(
        f.doc.get(f.b).unwrap().paint().fills[0].brush.var(),
        Some(v),
        "a user holds the bound paint"
    );
    let ops = ondin_core::variable::remove_ops(&f.doc, &[v]);
    commit(&mut f, ops).unwrap();
    assert_eq!(fill(&f.doc, f.b), Brush::Solid(GREEN));
    assert_eq!(fill_style(&f.doc, f.b), Some(s), "still styled");
    let StyleValue::Fills(list) = &f.doc.styles().get(s).unwrap().value else {
        panic!("a fill style");
    };
    assert_eq!(list[0].brush, BrushSource::from(Brush::Solid(GREEN)));
}

/// **An effects style binding a number its effect's kind lacks is refused**
/// (§15 D1074): `settle` writes the list verbatim onto every user, whose own
/// rule refuses it, so such a style could never be applied.
///
/// flip: `style::check`'s per-kind clause removed → the `AddStyle` commits.
#[test]
fn an_effects_style_binding_a_number_its_kind_lacks_is_refused() {
    let mut f = fixture();
    let (mode, var) = (f.ids.mint_mode(), f.ids.mint_var());
    commit(
        &mut f,
        vec![
            Operation::AddMode {
                mode: Mode {
                    id: mode,
                    name: "Default".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: var,
                    name: "space/x".into(),
                    kind: VarKind::Number,
                    cells: [(mode, Cell::Number(4.0))].into(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ],
    )
    .unwrap();
    let mut blur = ondin_core::Effect::new(ondin_core::EffectKind::LayerBlur { radius: 4.0 });
    blur.vars[ondin_core::EFFECT_X] = Some(var);
    let id = StyleId(f.ids.mint());
    let r = commit(
        &mut f,
        vec![Operation::AddStyle {
            style: Style {
                id,
                name: "Soft".into(),
                value: StyleValue::Effects(ondin_core::keyed_by_position([blur])),
                origin: None,
                forked: None,
                description: String::new(),
            },
        }],
    );
    assert!(
        matches!(
            r,
            Err(OpError::Styles(ondin_core::style::StyleRule::BadVariable(
                _
            )))
        ),
        "{r:?}"
    );
}

/// **A resize keeps a layer bound to a gradient fill style**: the gradient pass
/// leaves a styled list to its style, so the resize writes no fill to detach.
///
/// flip: `keep_gradients` carrying a styled list (`slots` all true) → fails at
/// *"still bound"*.
#[test]
fn a_resize_keeps_a_gradient_fill_style() {
    let mut f = fixture();
    let ramp = ondin_core::peniko::Gradient::new_linear((0.0, 0.0), (40.0, 0.0));
    let mut fills = f.doc.get(f.a).unwrap().paint().fills.clone();
    fills[0].brush = Brush::Gradient(ramp.into()).into();
    let a = f.a;
    commit(&mut f, vec![Operation::SetFills { id: a, fills }]).unwrap();
    let s = styled(&mut f);
    let before = fill(&f.doc, f.a);
    let tx = ondin_core::paint_box::keep_gradients(
        &f.doc,
        Transaction(vec![Operation::SetGeometry {
            id: a,
            geometry: ondin_core::GeometryPatch::Size(Size::new(80.0, 20.0)),
        }]),
    );
    commit(&mut f, tx.0).unwrap();
    assert_eq!(fill_style(&f.doc, f.a), Some(s), "still bound");
    assert_eq!(fill(&f.doc, f.a), before, "drawn as its style");
}
