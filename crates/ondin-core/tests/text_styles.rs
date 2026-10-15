//! Text styles (§5.3f, §15 D1067): a text layer's type as a fourth style slot —
//! the character defaults, less the colour and the language; spans the text's
//! own marks over it, so styling a selection keeps the style and only a write of
//! the defaults detaches.

use ondin_core::kurbo::Size;
use ondin_core::peniko::Color;
use ondin_core::style::{StyleId, StyleSlot, StyleValue};
use ondin_core::{
    CharAttr, CharSpans, Document, IdSource, NodeId, NodeKind, OpError, Operation, Placement,
    TextStyle, Transaction,
};

const RED: Color = Color::from_rgba8(200, 30, 30, 255);

struct F {
    doc: Document,
    ids: IdSource,
    root: NodeId,
    frame: NodeId,
    /// Two text layers in the frame: `a` 24px Lora 700 in red, `b` plain Inter.
    a: NodeId,
    b: NodeId,
}

fn text(id: NodeId, parent: NodeId, content: &str, style: TextStyle) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index: 0,
        kind: NodeKind::Text {
            content: content.into(),
            style: Box::new(style),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing: ondin_core::TextSizing::Auto,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        },
        transform: None,
        name: None,
    }
}

fn heading() -> TextStyle {
    TextStyle {
        font_family: "Lora".into(),
        font_size: 24.0,
        weight: 700,
        color: Some(RED),
        ..Default::default()
    }
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xBE);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (frame, a, b) = (ids.mint(), ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(300.0, 200.0),
            },
            transform: None,
            name: None,
        },
        text(a, frame, "Hello world", heading()),
        text(
            b,
            frame,
            "Other words",
            TextStyle {
                locale: Some("de".into()),
                ..Default::default()
            },
        ),
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

/// Commit as the app does (`tests/styles.rs`' `commit`).
fn commit(f: &mut F, ops: Vec<Operation>) -> Result<Transaction, OpError> {
    let mut tx = Transaction(ops);
    ondin_core::propagate::owed(&f.doc, &mut tx, &mut f.ids);
    f.doc.apply(&tx).map(|o| o.inverse)
}

fn style_of(doc: &Document, id: NodeId) -> TextStyle {
    match doc.get(id).unwrap().kind() {
        NodeKind::Text { style, .. } => style.as_ref().clone(),
        _ => panic!("not text"),
    }
}

fn spans_of(doc: &Document, id: NodeId) -> CharSpans {
    match doc.get(id).unwrap().kind() {
        NodeKind::Text { spans, .. } => spans.clone(),
        _ => panic!("not text"),
    }
}

fn type_style(doc: &Document, id: NodeId) -> Option<StyleId> {
    doc.get(id).unwrap().styles().text
}

/// `a`'s type made a style named *Heading/H2*, `a` and `b` bound to it.
fn styled(f: &mut F) -> StyleId {
    let s = f.ids.mint_style();
    let (a, b) = (f.a, f.b);
    let ops = ondin_core::style::create_ops(&f.doc, s, "Heading/H2", a, StyleSlot::Text, &[a, b])
        .unwrap();
    commit(f, ops).unwrap();
    s
}

/// **A text style is the type without its colour or its language**, and
/// applying one writes the type onto the layer while the layer keeps both.
///
/// Flip, run: writing the style's `TextStyle` whole in `write_op` (no
/// `typed_over`) fails at *"b keeps its language"* — the style holds `None` and
/// takes `de` away — and `a_style_edit_reaches_a_copy_of_a_styled_main` at its
/// red.
#[test]
fn applying_a_text_style_writes_the_type_and_keeps_the_colour_and_language() {
    let mut f = fixture();
    let s = styled(&mut f);
    let st = f.doc.styles().get(s).unwrap();
    let StyleValue::Text(held) = &st.value else {
        panic!("a text style");
    };
    assert_eq!(held.color, None, "the style holds no colour");
    assert_eq!(held.font_family, "Lora");

    let b = style_of(&f.doc, f.b);
    assert_eq!(
        (b.font_family.as_str(), b.font_size, b.weight),
        ("Lora", 24.0, 700),
        "b takes the type"
    );
    assert_eq!(b.locale.as_deref(), Some("de"), "b keeps its language");
    assert_eq!(b.color, None, "and does not take a's red");
    assert_eq!(style_of(&f.doc, f.a).color, Some(RED), "a keeps its red");
    assert_eq!(type_style(&f.doc, f.b), Some(s));
}

/// **Editing the style rewrites every user, and a span stays the text's own**
/// (§15 D1067, D1037's (e)): a word made italic in *b* stays italic when the
/// style's size changes, and *b* stays bound.
#[test]
fn a_style_edit_reaches_its_users_and_keeps_their_spans() {
    let mut f = fixture();
    let s = styled(&mut f);
    // A selection styled: one span over "Other", which is not the slot's write.
    let mut spans = spans_of(&f.doc, f.b);
    let def = style_of(&f.doc, f.b);
    spans.set(0..5, CharAttr::Italic(true), &def);
    let b = f.b;
    commit(&mut f, vec![Operation::SetTextSpans { id: b, spans }]).unwrap();
    assert_eq!(
        type_style(&f.doc, f.b),
        Some(s),
        "a styled selection keeps the style"
    );

    let StyleValue::Text(mut t) = f.doc.styles().get(s).unwrap().value.clone() else {
        unreachable!()
    };
    t.font_size = 32.0;
    commit(
        &mut f,
        vec![Operation::SetStyleValue {
            id: s,
            value: StyleValue::Text(t),
        }],
    )
    .unwrap();
    assert_eq!(style_of(&f.doc, f.a).font_size, 32.0);
    assert_eq!(style_of(&f.doc, f.b).font_size, 32.0);
    assert_eq!(
        spans_of(&f.doc, f.b).as_slice().len(),
        1,
        "b's italic word survives the edit"
    );
    assert_eq!(type_style(&f.doc, f.b), Some(s));
}

/// **A write of the defaults detaches; a colour or a language alone does not**
/// — the look decides, not the operation (`style::writes_slot`'s note).
///
/// Flip, run: making `settle` detach on the write alone (dropping its
/// `same_look` skip) fails at *"a colour alone keeps the style"*.
#[test]
fn a_defaults_write_detaches_and_a_colour_alone_does_not() {
    let mut f = fixture();
    let s = styled(&mut f);
    let a = f.a;
    let recoloured = TextStyle {
        color: Some(Color::from_rgba8(0, 0, 0, 255)),
        ..style_of(&f.doc, a)
    };
    commit(
        &mut f,
        vec![Operation::SetTextStyle {
            id: a,
            style: recoloured,
            spans: None,
        }],
    )
    .unwrap();
    assert_eq!(
        type_style(&f.doc, a),
        Some(s),
        "a colour alone keeps the style"
    );

    let bigger = style_of(&f.doc, a).with(CharAttr::Size(40.0));
    commit(
        &mut f,
        vec![Operation::SetTextStyle {
            id: a,
            style: bigger,
            spans: None,
        }],
    )
    .unwrap();
    assert_eq!(type_style(&f.doc, a), None, "a size write detaches");
    assert_eq!(style_of(&f.doc, a).font_size, 40.0, "and keeps its values");
    assert_eq!(
        type_style(&f.doc, f.b),
        Some(s),
        "the other user is untouched"
    );
}

/// **Only text takes a text style** — `style::check` refuses one bound to a
/// frame, at the commit.
#[test]
fn a_text_style_on_a_layer_that_is_not_text_is_refused() {
    let mut f = fixture();
    let s = styled(&mut f);
    let frame = f.frame;
    let err = commit(
        &mut f,
        vec![ondin_core::style::bind_op(frame, StyleSlot::Text, Some(s))],
    )
    .unwrap_err();
    assert!(matches!(err, OpError::Styles(_)), "{err:?}");
}

/// **An edit through a user edits the style and keeps the user's own colour**
/// (`style::edit_through`, §15 D1063's *Edit*): a size and a colour typed in one
/// edit of *a* — the size reaches *b*, the colour stays *a*'s.
///
/// Flip, run: dropping the text arm's `out.push(op)` fails at *"a's new colour
/// is kept"* — the write is swallowed and only the style's type lands.
#[test]
fn an_edit_through_a_user_edits_the_style_and_keeps_its_colour() {
    let mut f = fixture();
    let s = styled(&mut f);
    let a = f.a;
    let black = Color::from_rgba8(0, 0, 0, 255);
    let edit = TextStyle {
        font_size: 18.0,
        color: Some(black),
        ..style_of(&f.doc, a)
    };
    let tx = ondin_core::style::edit_through(
        &f.doc,
        Transaction(vec![Operation::SetTextStyle {
            id: a,
            style: edit,
            spans: None,
        }]),
        s,
    );
    commit(&mut f, tx.0).unwrap();
    assert_eq!(style_of(&f.doc, f.b).font_size, 18.0, "b follows the style");
    assert_eq!(style_of(&f.doc, a).font_size, 18.0);
    assert_eq!(
        style_of(&f.doc, a).color,
        Some(black),
        "a's new colour is kept"
    );
    assert_eq!(type_style(&f.doc, a), Some(s), "a stays bound");
    let StyleValue::Text(t) = &f.doc.styles().get(s).unwrap().value else {
        unreachable!()
    };
    assert_eq!((t.font_size, t.color), (18.0, None));
}

/// **An edit through a user keeps that user's spans, as every other user keeps
/// theirs** (§15 D1068's amendment). The Type card's write of a default comes
/// with a `SetTextSpans` clearing that attribute's spans over the whole layer
/// (§15 D164, `char_attrs_tx`'s shape, built here by hand); through the style's
/// *Edit* that clearing would flatten a word on the one layer it was typed on.
///
/// Flip, run: `edit_through`'s `SetTextSpans` drop removed fails at *"a keeps its
/// 30px word"*.
#[test]
fn an_edit_through_a_user_keeps_its_spans() {
    let mut f = fixture();
    let s = styled(&mut f);
    let a = f.a;
    let def = style_of(&f.doc, a);
    let mut spans = spans_of(&f.doc, a);
    spans.set(0..5, CharAttr::Size(30.0), &def);
    commit(
        &mut f,
        vec![Operation::SetTextSpans {
            id: a,
            spans: spans.clone(),
        }],
    )
    .unwrap();
    assert_eq!(
        spans_of(&f.doc, a).as_slice().len(),
        1,
        "the fixture: one span"
    );
    // The card's whole-layer size write: new default, the size's spans cleared.
    let new = def.with(CharAttr::Size(20.0));
    let mut cleared = spans_of(&f.doc, a);
    cleared.clear(0..11, ondin_core::CharAttrKind::Size, &new);
    let tx = ondin_core::style::edit_through(
        &f.doc,
        Transaction(vec![
            Operation::SetTextStyle {
                id: a,
                style: new,
                spans: None,
            },
            Operation::SetTextSpans {
                id: a,
                spans: cleared,
            },
        ]),
        s,
    );
    commit(&mut f, tx.0).unwrap();
    assert_eq!(style_of(&f.doc, f.b).font_size, 20.0, "the edit reaches b");
    assert_eq!(
        spans_of(&f.doc, a).as_slice().len(),
        1,
        "a keeps its 30px word"
    );
    assert_eq!(type_style(&f.doc, a), Some(s));
}

/// **A styled main's style edit reaches its instance's copy**, the binding a
/// whole field like the paint cards' (`propagate`'s `Mode::Whole`).
#[test]
fn a_style_edit_reaches_a_copy_of_a_styled_main() {
    let mut f = fixture();
    let s = styled(&mut f);
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
    let ca = *f
        .doc
        .get(made[0])
        .unwrap()
        .children()
        .iter()
        .find(|c| f.doc.get(**c).unwrap().link() == Some(f.a))
        .unwrap();
    assert_eq!(
        type_style(&f.doc, ca),
        Some(s),
        "the copy is bound as its main"
    );

    let StyleValue::Text(mut t) = f.doc.styles().get(s).unwrap().value.clone() else {
        unreachable!()
    };
    t.weight = 400;
    commit(
        &mut f,
        vec![Operation::SetStyleValue {
            id: s,
            value: StyleValue::Text(t),
        }],
    )
    .unwrap();
    assert_eq!(style_of(&f.doc, ca).weight, 400);
    assert_eq!(
        style_of(&f.doc, ca).color,
        Some(RED),
        "with its main's red kept"
    );
}

/// **A text style round-trips through the file**, byte-stable.
#[test]
fn a_text_style_round_trips() {
    let mut f = fixture();
    let s = styled(&mut f);
    let bytes = ondin_core::io::save(&f.doc).unwrap();
    let back = ondin_core::io::load(&bytes).unwrap();
    assert_eq!(back.styles().get(s), f.doc.styles().get(s));
    assert_eq!(back.get(f.b).unwrap().styles().text, Some(s));
    assert_eq!(ondin_core::io::save(&back).unwrap(), bytes, "byte-stable");
}
