//! String variables (§5.3f, §15 D1069): a text's content and family bound to
//! one, drawn in the mode in force, kept or detached by the commit's
//! `keep_text_bindings`, detached by a delete and by a paste into a document
//! without the variable, and round-tripped through a save.

use ondin_core::kurbo::Size;
use ondin_core::{
    Cell, CharAttr, CharAttrKind, CharSpans, Document, IdSource, Mode, ModeId, NodeId, NodeKind,
    Operation, Resolved, TextStyle, Transaction, VarId, VarKind, Variable,
};

struct F {
    doc: Document,
    ids: IdSource,
    root: NodeId,
    frame: NodeId,
    text: NodeId,
    dark: ModeId,
    /// `label`: *Sign in* in Light, *Anmelden* in Dark.
    label: VarId,
    /// `font`: *Inter* in Light, *Roboto* in Dark.
    font: VarId,
    /// A number, for the refusal.
    gap: VarId,
}

fn mode(id: ModeId, name: &str) -> Mode {
    Mode {
        id,
        name: name.into(),
    }
}

fn var(id: VarId, name: &str, kind: VarKind, cells: [(ModeId, Cell); 2]) -> Variable {
    Variable {
        id,
        name: name.into(),
        kind,
        cells: cells.into(),
        origin: None,
        forked: None,
    }
}

fn text_kind(content: &str) -> NodeKind {
    NodeKind::Text {
        content: content.into(),
        style: Box::new(TextStyle {
            font_family: "Georgia".into(),
            ..TextStyle::default()
        }),
        spans: Default::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing: ondin_core::TextSizing::Auto,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    }
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xBA);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (frame, text) = (ids.mint(), ids.mint());
    let (light, dark) = (ids.mint_mode(), ids.mint_mode());
    let (label, font, gap) = (ids.mint_var(), ids.mint_var(), ids.mint_var());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(300.0, 100.0),
            },
            transform: None,
            name: None,
        },
        Operation::CreateNode {
            id: text,
            parent: frame,
            index: 0,
            kind: text_kind("Hello world"),
            transform: None,
            name: None,
        },
        Operation::AddMode {
            mode: mode(light, "Light"),
            index: 0,
            cells: vec![],
        },
        Operation::AddMode {
            mode: mode(dark, "Dark"),
            index: 1,
            cells: vec![],
        },
        Operation::AddVariable {
            var: var(
                label,
                "copy/cta",
                VarKind::String,
                [
                    (light, Cell::String("Sign in".into())),
                    (dark, Cell::String("Anmelden".into())),
                ],
            ),
        },
        Operation::AddVariable {
            var: var(
                font,
                "type/family",
                VarKind::String,
                [
                    (light, Cell::String("Inter".into())),
                    (dark, Cell::String("Roboto".into())),
                ],
            ),
        },
        Operation::AddVariable {
            var: var(
                gap,
                "space/gap",
                VarKind::Number,
                [(light, Cell::Number(4.0)), (dark, Cell::Number(8.0))],
            ),
        },
    ]))
    .expect("a frame, a text and a two-mode table");
    F {
        doc,
        ids,
        root,
        frame,
        text,
        dark,
        label,
        font,
        gap,
    }
}

/// The text's content and family as `Resolved` hands them to the scene and the
/// export — its used kind.
fn used(doc: &Document, id: NodeId) -> (String, String) {
    match Resolved::rebuild(doc).used_kind(doc, id) {
        Some(NodeKind::Text { content, style, .. }) => (content.clone(), style.font_family.clone()),
        k => panic!("not a text: {k:?}"),
    }
}

/// The stored literals and the bindings.
fn stored(doc: &Document, id: NodeId) -> (String, String, [Option<VarId>; 2]) {
    let n = doc.get(id).unwrap();
    match n.kind() {
        NodeKind::Text { content, style, .. } => {
            (content.clone(), style.font_family.clone(), n.text_vars())
        }
        _ => unreachable!(),
    }
}

fn bold_word(doc: &mut Document, id: NodeId) {
    let NodeKind::Text { style, .. } = doc.get(id).unwrap().kind() else {
        unreachable!()
    };
    let mut spans = CharSpans::default();
    spans.set(0..5, CharAttr::Weight(700), style);
    doc.apply(&Transaction(vec![Operation::SetTextSpans { id, spans }]))
        .unwrap();
}

/// **A bound content and family draw their strings, in the mode in force**
/// (§15 D1069) — through `Resolved`'s used kind, which is what the scene, the
/// SVG export and the snapshot all read — while the literals stay empty, and
/// binding the content took the spans with it.
///
/// Flip, run: `resolve::with_bound_size` gated on `size_vars` alone, as it was
/// before this entry, fails *"Light's string, through Resolved"* — a text
/// neither laid out nor pinned has no used kind, and its empty literal is
/// shaped.
#[test]
fn a_bound_content_and_family_draw_their_strings_in_the_mode_in_force() {
    let mut f = fixture();
    bold_word(&mut f.doc, f.text);
    let ops = ondin_core::bind_text_ops(&f.doc, f.text, [Some(f.label), Some(f.font)]);
    f.doc.apply(&Transaction(ops)).unwrap();
    assert_eq!(
        stored(&f.doc, f.text),
        (String::new(), String::new(), [Some(f.label), Some(f.font)]),
        "both literals empty, both bound"
    );
    let NodeKind::Text {
        spans, para_spans, ..
    } = f.doc.get(f.text).unwrap().kind()
    else {
        unreachable!()
    };
    assert!(
        spans.is_empty() && para_spans.is_empty(),
        "binding the content cleared the spans that indexed it"
    );
    assert_eq!(
        used(&f.doc, f.text),
        ("Sign in".into(), "Inter".into()),
        "Light's string, through Resolved"
    );
    f.doc
        .apply(&Transaction(vec![Operation::SetMode {
            id: f.frame,
            mode: Some(f.dark),
        }]))
        .unwrap();
    assert_eq!(
        used(&f.doc, f.text),
        ("Anmelden".into(), "Roboto".into()),
        "Dark's"
    );
}

/// **The family binds the default, and a run keeps a family of its own**: a
/// family-only bind clears the runs' families and leaves the content and its
/// other spans — the bold word stays bold.
#[test]
fn binding_the_family_alone_keeps_the_content_and_its_other_spans() {
    let mut f = fixture();
    bold_word(&mut f.doc, f.text);
    let NodeKind::Text { style, spans, .. } = f.doc.get(f.text).unwrap().kind() else {
        unreachable!()
    };
    let mut spans = spans.clone();
    spans.set(6..11, CharAttr::Family("Arial".into()), style);
    f.doc
        .apply(&Transaction(vec![Operation::SetTextSpans {
            id: f.text,
            spans,
        }]))
        .unwrap();
    let ops = ondin_core::bind_text_ops(&f.doc, f.text, [None, Some(f.font)]);
    f.doc.apply(&Transaction(ops)).unwrap();
    let NodeKind::Text {
        content,
        spans,
        style,
        ..
    } = f.doc.get(f.text).unwrap().kind()
    else {
        unreachable!()
    };
    assert_eq!(content, "Hello world", "the content as it was");
    assert_eq!(
        spans.values_in(style, CharAttrKind::Weight, 0..5),
        vec![CharAttr::Weight(700)],
        "the bold word kept"
    );
    assert!(
        spans
            .as_slice()
            .iter()
            .all(|s| !matches!(s.attr, CharAttr::Family(_))),
        "the run's family cleared, so the text draws its variable whole"
    );
}

/// **An edit of a bound text detaches what it edits, and only what changes
/// it** (§15 D1069, `keep_text_bindings`): a `SetText` of the drawn string
/// with no spans keeps the content bound — a text session closed unedited —
/// and so does a `SetTextStyle` writing the drawn family (every other row of
/// the Type card writes the whole style); typing detaches the content to what
/// was typed, styling a range rewrites into a `SetText` of the drawn string,
/// another family detaches the family, and applying a text style detaches it.
///
/// Flip, run: the content arm's `plain &&` dropped from `keep_text_bindings`
/// fails *"a session that styled a word and typed nothing detaches the
/// content"* — a `SetText` carrying spans over the drawn string kept a binding
/// its spans cannot index.
#[test]
fn an_edit_of_a_bound_text_detaches_what_it_edits() {
    let mut f = fixture();
    let ops = ondin_core::bind_text_ops(&f.doc, f.text, [Some(f.label), Some(f.font)]);
    f.doc.apply(&Transaction(ops)).unwrap();
    let keep =
        |doc: &Document, ops: Vec<Operation>| ondin_core::keep_text_bindings(doc, Transaction(ops));
    let set_text = |content: &str, spans: CharSpans| Operation::SetText {
        id: f.text,
        content: content.into(),
        spans,
        para_spans: Default::default(),
    };

    let same = keep(&f.doc, vec![set_text("Sign in", Default::default())]);
    assert!(
        same.changes_nothing(&f.doc),
        "the drawn string written back keeps the binding"
    );

    let mut d = f.doc.clone();
    d.apply(&keep(&f.doc, vec![set_text("Log in", Default::default())]))
        .unwrap();
    assert_eq!(
        stored(&d, f.text),
        ("Log in".into(), String::new(), [None, Some(f.font)]),
        "typing detaches the content to what was typed, the family still bound"
    );

    let mut bold = CharSpans::default();
    bold.set(0..4, CharAttr::Weight(700), &TextStyle::default());
    let mut d = f.doc.clone();
    d.apply(&keep(
        &f.doc,
        vec![Operation::SetTextSpans {
            id: f.text,
            spans: bold.clone(),
        }],
    ))
    .unwrap();
    let (content, _, vars) = stored(&d, f.text);
    assert_eq!(
        (content.as_str(), vars[0]),
        ("Sign in", None),
        "a styled range detaches the content, to the string the range indexes"
    );
    let mut d = f.doc.clone();
    d.apply(&keep(&f.doc, vec![set_text("Sign in", bold.clone())]))
        .unwrap();
    assert_eq!(
        stored(&d, f.text).2[0],
        None,
        "a session that styled a word and typed nothing detaches the content"
    );

    let NodeKind::Text { style, .. } = f.doc.get(f.text).unwrap().kind() else {
        unreachable!()
    };
    let with_family = |family: &str, size: f64| Operation::SetTextStyle {
        id: f.text,
        style: TextStyle {
            font_family: family.into(),
            font_size: size,
            ..(**style).clone()
        },
        spans: None,
    };
    let mut d = f.doc.clone();
    d.apply(&keep(&f.doc, vec![with_family("Inter", 24.0)]))
        .unwrap();
    let (_, family, vars) = stored(&d, f.text);
    assert_eq!(
        (family.as_str(), vars[1]),
        ("", Some(f.font)),
        "a size written with the drawn family keeps the family bound"
    );
    let mut d = f.doc.clone();
    d.apply(&keep(&f.doc, vec![with_family("Georgia", 16.0)]))
        .unwrap();
    let (_, family, vars) = stored(&d, f.text);
    assert_eq!(
        (family.as_str(), vars[1]),
        ("Georgia", None),
        "another family detaches it"
    );

    let tx = keep(
        &f.doc,
        vec![Operation::SetTypeStyle {
            id: f.text,
            style: Some(ondin_core::style::StyleId(f.ids.mint())),
        }],
    );
    assert!(
        tx.0.iter().any(|op| matches!(
            op,
            Operation::SetTextVars { vars, .. } if vars[1].is_none() && vars[0] == Some(f.label)
        )),
        "a text style applied detaches the family and leaves the content"
    );
}

/// Detaching keeps what each field drew; deleting the variable detaches the
/// same way; the bindings and a string cell round-trip through a save; and a
/// string binding refuses a number variable, and a layer that is no text.
#[test]
fn string_bindings_detach_to_what_they_draw_and_round_trip() {
    let mut f = fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetMode {
            id: f.frame,
            mode: Some(f.dark),
        }]))
        .unwrap();
    let ops = ondin_core::bind_text_ops(&f.doc, f.text, [Some(f.label), Some(f.font)]);
    f.doc.apply(&Transaction(ops)).unwrap();
    let bytes = ondin_core::io::save(&f.doc).unwrap();
    let back = ondin_core::io::load(&bytes).unwrap();
    assert_eq!(back, f.doc, "the bindings and the string cells round-trip");
    assert!(
        String::from_utf8(bytes)
            .unwrap()
            .contains("\"string\": \"Anmelden\""),
        "a string cell is written as one"
    );

    let before = f.doc.clone();
    f.doc
        .apply(&Transaction(ondin_core::detach_text_ops(
            &f.doc,
            f.text,
            [true, true],
        )))
        .unwrap();
    assert_eq!(
        stored(&f.doc, f.text),
        ("Anmelden".into(), "Roboto".into(), [None, None]),
        "detached to Dark's strings"
    );

    f.doc = before;
    let tx = Transaction(ondin_core::variable::remove_ops(&f.doc, &[f.label]));
    f.doc.apply(&tx).expect("detach, then remove");
    assert_eq!(
        stored(&f.doc, f.text),
        ("Anmelden".into(), String::new(), [None, Some(f.font)]),
        "deleting the content's variable detaches the content alone"
    );

    let wrong = f.doc.apply(&Transaction(vec![Operation::SetTextVars {
        id: f.text,
        vars: [Some(f.gap), None],
    }]));
    assert!(wrong.is_err(), "a number variable cannot bind a content");
    let not_text = f.doc.apply(&Transaction(vec![Operation::SetTextVars {
        id: f.frame,
        vars: [Some(f.font), None],
    }]));
    assert!(not_text.is_err(), "nor can a frame bind one");
}

/// **A paste into a document without the variable detaches to what the copy
/// drew** (D1038's paste rule): the twin taken at the copy carries the strings.
#[test]
fn a_paste_without_the_variable_detaches_to_the_copied_strings() {
    let mut f = fixture();
    let ops = ondin_core::bind_text_ops(&f.doc, f.text, [Some(f.label), Some(f.font)]);
    f.doc.apply(&Transaction(ops)).unwrap();
    let node = f.doc.get(f.text).unwrap().clone();
    let carried = ondin_core::variable::carried(&f.doc, &[vec![node.clone()]]);
    let mut ids = IdSource::new(0xBB);
    let other = Document::new(ids.mint());
    let landed = ondin_core::variable::land(&other, &carried, vec![node]);
    let n = &landed[0];
    let NodeKind::Text { content, style, .. } = n.kind() else {
        unreachable!()
    };
    assert_eq!(
        (content.as_str(), style.font_family.as_str(), n.text_vars()),
        ("Sign in", "Inter", [None, None]),
        "Light's strings, the bindings gone"
    );
}

/// A component's text property is a view over the content **as drawn** (D982,
/// §15 D1069) — the bound string, not the empty literal.
#[test]
fn a_text_property_reads_the_bound_string() {
    let mut f = fixture();
    let ops = ondin_core::bind_text_ops(&f.doc, f.text, [Some(f.label), None]);
    f.doc.apply(&Transaction(ops)).unwrap();
    assert_eq!(
        ondin_core::variant::field_value(&f.doc, f.text, ondin_core::variant::PropKind::Text),
        Some(ondin_core::variant::PropValue::Text("Sign in".into()))
    );
}

/// **A bound text emptied detaches, empty** (§15 D1069's first reading): the
/// empty literal is what a session that deletes every character writes, and a
/// Text property typed empty — one half for the content, D1059's reason.
///
/// Flip, run: `|| content.is_empty()` put back beside `*content == drawn` in
/// `keep_text_bindings` fails *"emptied, it stays empty"* — the binding kept
/// and *Sign in* back on release.
#[test]
fn a_bound_text_emptied_detaches_empty() {
    let mut f = fixture();
    let ops = ondin_core::bind_text_ops(&f.doc, f.text, [Some(f.label), None]);
    f.doc.apply(&Transaction(ops)).unwrap();
    let tx = ondin_core::keep_text_bindings(
        &f.doc,
        Transaction(vec![Operation::SetText {
            id: f.text,
            content: String::new(),
            spans: Default::default(),
            para_spans: Default::default(),
        }]),
    );
    f.doc.apply(&tx).unwrap();
    assert_eq!(
        (stored(&f.doc, f.text).2[0], used(&f.doc, f.text).0),
        (None, String::new()),
        "emptied, it stays empty"
    );
}

/// **A bound text in a layout is re-measured when its frame's mode changes**
/// (§15 D1069's fourth reading): `SetMode` dirties the frame alone, and the
/// layout's text memo dropped only dirty ids, so the row kept placing the
/// second item beside *Sign in*'s width after the text read *Anmelden*.
///
/// Flip, run: the `memo.retain` over bound texts taken out of
/// `Resolved::update` fails *"update equals rebuild after the mode"*.
#[test]
fn a_mode_change_re_measures_a_bound_text_in_a_layout() {
    let mut f = fixture();
    let second = f.ids.mint();
    f.doc
        .apply(&Transaction(vec![
            Operation::CreateNode {
                id: second,
                parent: f.frame,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(40.0, 20.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
            Operation::SetDisplay {
                id: f.frame,
                display: Some(ondin_core::container::Display::Flex(
                    ondin_core::container::Flex {
                        column_gap: 5.0,
                        ..Default::default()
                    },
                )),
            },
        ]))
        .unwrap();
    let ops = ondin_core::bind_text_ops(&f.doc, f.text, [Some(f.label), None]);
    f.doc.apply(&Transaction(ops)).unwrap();
    let mut live = Resolved::rebuild(&f.doc);
    let x = |r: &Resolved| r.world_bounds(second).unwrap().min_x();
    let light = x(&live);
    let out = f
        .doc
        .apply(&Transaction(vec![Operation::SetMode {
            id: f.frame,
            mode: Some(f.dark),
        }]))
        .unwrap();
    live.update(&f.doc, &out.dirty);
    let fresh = Resolved::rebuild(&f.doc);
    assert_ne!(
        x(&fresh),
        light,
        "the fixture moves the item: the strings differ in width"
    );
    assert_eq!(x(&live), x(&fresh), "update equals rebuild after the mode");
}

/// An instance whose main's text is bound: making the copy's text its own,
/// then resetting the Text property, takes the main's **binding** back — not
/// the empty literal beside it (§15 D1069's second reading).
///
/// Flip, run: `property_reset_ops`' `SetTextVars` arm removed fails *"the
/// reset draws the main's string"* at `""`.
#[test]
fn a_text_property_reset_takes_the_mains_binding_back() {
    let mut f = fixture();
    let ops = ondin_core::bind_text_ops(&f.doc, f.text, [Some(f.label), None]);
    f.doc.apply(&Transaction(ops)).unwrap();
    f.doc
        .apply(&Transaction(vec![Operation::SetComponent {
            id: f.frame,
            component: true,
        }]))
        .unwrap();
    let place = ondin_core::Placement {
        nodes: f.doc.capture_subtree(f.frame).unwrap(),
        parent: f.root,
        index: None,
    };
    let (tx, made) = ondin_core::insert_subtrees(&f.doc, &mut f.ids, &[place], Default::default());
    f.doc.apply(&tx).unwrap();
    let copy = f.doc.get(made[0]).unwrap().children()[0];
    assert_eq!(
        f.doc.get(copy).unwrap().text_vars()[0],
        Some(f.label),
        "the copy follows"
    );
    let tx = ondin_core::keep_text_bindings(
        &f.doc,
        Transaction(vec![Operation::SetText {
            id: copy,
            content: "Hi".into(),
            spans: Default::default(),
            para_spans: Default::default(),
        }]),
    );
    f.doc.apply(&tx).unwrap();
    let p = ondin_core::variant::Property {
        name: "Label".into(),
        kind: ondin_core::variant::PropKind::Text,
        bound: vec![f.text],
        filter: String::new(),
    };
    let (_, overridden) = ondin_core::variant::property_state(&f.doc, made[0], &p).unwrap();
    assert!(overridden, "typed over, it is an override");
    let reset = ondin_core::variant::reset_property(&f.doc, &[made[0]], &p);
    f.doc.apply(&Transaction(reset)).unwrap();
    assert_eq!(
        (
            used(&f.doc, copy).0,
            f.doc.get(copy).unwrap().text_vars()[0]
        ),
        ("Sign in".to_string(), Some(f.label)),
        "the reset draws the main's string"
    );
}

/// **A copy's own family binding goes when its main takes a text style**
/// (§15 D1069's third reading): the copy receives the main's `SetTypeStyle`
/// from propagation, after `keep_text_bindings`, so `style::settle` drops the
/// binding where it writes the style's plain family.
///
/// Flip, run: the `SetTextVars` push in `style::settle` taken out fails *"the
/// copy draws the style's family"* at the variable's *Inter*.
#[test]
fn a_copys_family_binding_goes_with_its_mains_text_style() {
    let mut f = fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetComponent {
            id: f.frame,
            component: true,
        }]))
        .unwrap();
    let place = ondin_core::Placement {
        nodes: f.doc.capture_subtree(f.frame).unwrap(),
        parent: f.root,
        index: None,
    };
    let (tx, made) = ondin_core::insert_subtrees(&f.doc, &mut f.ids, &[place], Default::default());
    f.doc.apply(&tx).unwrap();
    let copy = f.doc.get(made[0]).unwrap().children()[0];
    let ops = ondin_core::bind_text_ops(&f.doc, copy, [None, Some(f.font)]);
    f.doc.apply(&Transaction(ops)).unwrap();
    let s = ondin_core::style::StyleId(f.ids.mint());
    f.doc
        .apply(&Transaction(vec![Operation::AddStyle {
            style: ondin_core::style::Style {
                id: s,
                name: "Body".into(),
                value: ondin_core::style::StyleValue::Text(Box::new(TextStyle {
                    font_family: "Georgia".into(),
                    ..TextStyle::default()
                })),
                origin: None,
                forked: None,
            },
        }]))
        .unwrap();
    let mut tx = ondin_core::keep_text_bindings(
        &f.doc,
        Transaction(vec![Operation::SetTypeStyle {
            id: f.text,
            style: Some(s),
        }]),
    );
    ondin_core::propagate::owed(&f.doc, &mut tx, &mut f.ids);
    f.doc.apply(&tx).unwrap();
    assert_eq!(
        (
            used(&f.doc, copy).1,
            f.doc.get(copy).unwrap().text_vars()[1]
        ),
        ("Georgia".to_string(), None),
        "the copy draws the style's family"
    );
}
