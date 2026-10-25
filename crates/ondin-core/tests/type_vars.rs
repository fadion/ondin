//! A text's type numbers bound to number variables (`variable::TextVars`' slots
//! from `TEXT_SIZE`): drawn in the mode in force and in the literal's unit, the
//! literal 0 while bound, kept by a whole-style write of what they draw or of the
//! literal, detached by any other value, by a text style and by a delete, and
//! round-tripped through a save — a file holding only the first two slots
//! loading as it did.

use ondin_core::kurbo::Size;
use ondin_core::typography::{Length, ParagraphStyle};
use ondin_core::variable::{
    TEXT_FAMILY, TEXT_LETTER_SPACING, TEXT_LINE_HEIGHT, TEXT_PARAGRAPH_SPACING, TEXT_SIZE,
    TEXT_WEIGHT, text_vars_at,
};
use ondin_core::{
    Cell, CharAttr, CharSpans, Document, IdSource, Mode, ModeId, NodeId, NodeKind, Operation,
    Resolved, TextStyle, Transaction, VarId, VarKind, Variable,
};

struct F {
    doc: Document,
    ids: IdSource,
    frame: NodeId,
    text: NodeId,
    dark: ModeId,
    /// `type/size`: 16 in Light, 24 in Dark.
    size: VarId,
    /// `type/leading`: 150 in Light, 120 in Dark — a percentage on a `%` line height.
    leading: VarId,
    /// `type/family`, a string.
    font: VarId,
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xBB);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (frame, text) = (ids.mint(), ids.mint());
    let (light, dark) = (ids.mint_mode(), ids.mint_mode());
    let (size, leading, font) = (ids.mint_var(), ids.mint_var(), ids.mint_var());
    let var = |id, name: &str, kind, cells: [(ModeId, Cell); 2]| Variable {
        id,
        name: name.into(),
        kind,
        cells: cells.into(),
        origin: None,
        forked: None,
        description: String::new(),
    };
    let mode = |id, name: &str| Mode {
        id,
        name: name.into(),
    };
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
            kind: NodeKind::Text {
                content: "Hello world\nSecond".into(),
                style: Box::new(TextStyle {
                    line_height: Some(Length::Em(1.0)),
                    letter_spacing: Length::Px(1.0),
                    ..TextStyle::default()
                }),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: ParagraphStyle {
                    spacing: Length::Px(4.0),
                    ..Default::default()
                },
                block: Default::default(),
                sizing: ondin_core::TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
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
                size,
                "type/size",
                VarKind::Number,
                [(light, Cell::Number(16.0)), (dark, Cell::Number(24.0))],
            ),
        },
        Operation::AddVariable {
            var: var(
                leading,
                "type/leading",
                VarKind::Number,
                [(light, Cell::Number(150.0)), (dark, Cell::Number(120.0))],
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
    ]))
    .expect("a frame, a text and a two-mode table");
    F {
        doc,
        ids,
        frame,
        text,
        dark,
        size,
        leading,
        font,
    }
}

fn bind(slot: usize, var: VarId, f: &mut F) {
    let ops = ondin_core::bind_text_ops(&f.doc, f.text, text_vars_at(slot, var));
    assert!(!ops.is_empty(), "the bind writes");
    f.doc.apply(&Transaction(ops)).expect("bound");
}

fn dark(f: &mut F) {
    f.doc
        .apply(&Transaction(vec![Operation::SetMode {
            id: f.frame,
            mode: Some(f.dark),
        }]))
        .unwrap();
}

/// The text's style and paragraph as `Resolved` hands them to the scene.
fn used(doc: &Document, id: NodeId) -> (TextStyle, ParagraphStyle) {
    match Resolved::rebuild(doc).used_kind(doc, id) {
        Some(NodeKind::Text {
            style, paragraph, ..
        }) => ((**style).clone(), paragraph.clone()),
        k => panic!("not a text: {k:?}"),
    }
}

/// The stored style and paragraph — the literals.
fn stored(doc: &Document, id: NodeId) -> (TextStyle, ParagraphStyle) {
    match doc.get(id).unwrap().kind() {
        NodeKind::Text {
            style, paragraph, ..
        } => ((**style).clone(), paragraph.clone()),
        _ => unreachable!(),
    }
}

/// **A bound size and line height draw their variables in the mode in force**,
/// the line height in the `%` its literal holds — 150 is 1.5 em — and both
/// literals 0 in their units, with no run left over the text.
///
/// Flip, run: `set_text_slot_number` writing a `%` line height as px fails at
/// *"Light's 150%"*; `bind_text_ops` clearing the runs against the zeroed
/// literal fails at *"no run of its own"*, a size-1 run over every byte.
#[test]
fn a_bound_size_and_line_height_draw_in_the_mode_in_force() {
    let mut f = fixture();
    bind(TEXT_SIZE, f.size, &mut f);
    bind(TEXT_LINE_HEIGHT, f.leading, &mut f);
    let (s, _) = stored(&f.doc, f.text);
    assert_eq!((s.font_size, s.line_height), (0.0, Some(Length::Em(0.0))));
    let NodeKind::Text { spans, .. } = f.doc.get(f.text).unwrap().kind() else {
        unreachable!()
    };
    assert!(spans.is_empty(), "no run of its own: {spans:?}");
    let (s, _) = used(&f.doc, f.text);
    assert_eq!(s.font_size, 16.0);
    assert_eq!(s.line_height, Some(Length::Em(1.5)), "Light's 150%");
    dark(&mut f);
    let (s, _) = used(&f.doc, f.text);
    assert_eq!((s.font_size, s.line_height), (24.0, Some(Length::Em(1.2))));
}

/// **A bound paragraph spacing draws its variable as px**, its literal's unit,
/// and a weight is rounded and kept to the fields' 1–1000.
#[test]
fn a_bound_paragraph_spacing_and_weight_draw_in_their_units() {
    let mut f = fixture();
    bind(TEXT_PARAGRAPH_SPACING, f.size, &mut f);
    bind(TEXT_WEIGHT, f.leading, &mut f);
    assert_eq!(stored(&f.doc, f.text).1.spacing, Length::Px(0.0));
    let (s, p) = used(&f.doc, f.text);
    assert_eq!(p.spacing, Length::Px(16.0));
    assert_eq!(s.weight, 150);
}

/// **The commit keeps a binding a whole-style write carries** — the literal 0 or
/// the drawn number, as every other Type card row writes — and detaches one
/// written another value; a run given a size of its own keeps the default
/// bound under it.
///
/// Flip, run: `keep_text_bindings`' number arm detaching unconditionally fails
/// at *"a letter-spacing write keeps the size bound"*.
#[test]
fn a_style_write_keeps_what_it_carries_and_detaches_a_new_value() {
    let mut f = fixture();
    bind(TEXT_SIZE, f.size, &mut f);
    let commit = |doc: &mut Document, ops: Vec<Operation>| {
        let tx = ondin_core::keep_text_bindings(doc, Transaction(ops));
        doc.apply(&tx).unwrap();
    };
    let (mut s, _) = stored(&f.doc, f.text);
    s.letter_spacing = Length::Px(3.0);
    commit(
        &mut f.doc,
        vec![Operation::SetTextStyle {
            id: f.text,
            style: s.clone(),
            spans: None,
        }],
    );
    assert_eq!(
        f.doc.get(f.text).unwrap().text_vars()[TEXT_SIZE],
        Some(f.size),
        "a letter-spacing write keeps the size bound"
    );
    assert_eq!(used(&f.doc, f.text).0.font_size, 16.0);

    let mut spans = CharSpans::default();
    spans.set(0..5, CharAttr::Size(40.0), &s);
    commit(
        &mut f.doc,
        vec![Operation::SetTextSpans { id: f.text, spans }],
    );
    assert_eq!(
        f.doc.get(f.text).unwrap().text_vars()[TEXT_SIZE],
        Some(f.size),
        "a run's own size leaves the default bound"
    );

    s.font_size = 30.0;
    commit(
        &mut f.doc,
        vec![Operation::SetTextStyle {
            id: f.text,
            style: s,
            spans: None,
        }],
    );
    assert_eq!(f.doc.get(f.text).unwrap().text_vars()[TEXT_SIZE], None);
    assert_eq!(used(&f.doc, f.text).0.font_size, 30.0, "the number written");
}

/// **A bound line height moved to another unit detaches**: the same 0 in px is
/// not the `%` literal it holds.
#[test]
fn a_line_height_moved_to_another_unit_detaches() {
    let mut f = fixture();
    bind(TEXT_LINE_HEIGHT, f.leading, &mut f);
    let (mut s, _) = stored(&f.doc, f.text);
    s.line_height = Some(Length::Px(0.0));
    let tx = ondin_core::keep_text_bindings(
        &f.doc,
        Transaction(vec![Operation::SetTextStyle {
            id: f.text,
            style: s,
            spans: None,
        }]),
    );
    f.doc.apply(&tx).unwrap();
    assert_eq!(
        f.doc.get(f.text).unwrap().text_vars()[TEXT_LINE_HEIGHT],
        None
    );
}

/// **Applying a text style detaches the slots it holds and keeps the paragraph
/// spacing**, which no text style holds.
///
/// Flip, run: `keep_text_bindings`' `SetTypeStyle` arm detaching the family
/// alone fails at *"the size goes with the style"*.
#[test]
fn a_text_style_detaches_the_character_numbers_alone() {
    let mut f = fixture();
    bind(TEXT_SIZE, f.size, &mut f);
    bind(TEXT_PARAGRAPH_SPACING, f.size, &mut f);
    bind(TEXT_FAMILY, f.font, &mut f);
    let s = ondin_core::style::StyleId(f.ids.mint());
    f.doc
        .apply(&Transaction(vec![Operation::AddStyle {
            style: ondin_core::style::Style {
                id: s,
                name: "Body".into(),
                value: ondin_core::style::StyleValue::Text(Box::default()),
                origin: None,
                forked: None,
                description: String::new(),
            },
        }]))
        .unwrap();
    let tx = ondin_core::keep_text_bindings(
        &f.doc,
        Transaction(vec![Operation::SetTypeStyle {
            id: f.text,
            style: Some(s),
        }]),
    );
    f.doc.apply(&tx).unwrap();
    let v = f.doc.get(f.text).unwrap().text_vars();
    assert_eq!(v[TEXT_SIZE], None, "the size goes with the style");
    assert_eq!(v[TEXT_FAMILY], None);
    assert_eq!(
        v[TEXT_PARAGRAPH_SPACING],
        Some(f.size),
        "the paragraph's stays"
    );
    assert!(
        ondin_core::bind_text_ops(&f.doc, f.text, text_vars_at(TEXT_SIZE, f.size)).is_empty(),
        "and no character number binds under a style"
    );
}

/// **A delete detaches each bound number to what it drew**, and a variable of
/// the wrong kind is refused.
#[test]
fn a_delete_detaches_to_the_drawn_number() {
    let mut f = fixture();
    bind(TEXT_SIZE, f.size, &mut f);
    bind(TEXT_LETTER_SPACING, f.size, &mut f);
    dark(&mut f);
    let tx = Transaction(ondin_core::variable::remove_ops(&f.doc, &[f.size]));
    f.doc.apply(&tx).expect("detach, then remove");
    let (s, _) = stored(&f.doc, f.text);
    assert_eq!(
        (s.font_size, s.letter_spacing),
        (24.0, Length::Px(24.0)),
        "Dark's 24 in each"
    );
    let wrong = f.doc.apply(&Transaction(vec![Operation::SetTextVars {
        id: f.text,
        vars: text_vars_at(TEXT_SIZE, f.font),
    }]));
    assert!(wrong.is_err(), "a string variable cannot bind a size");
}

/// **A save keeps every slot, and a file written with the first two loads**: the
/// format writes `text_vars` up to its last bound slot, never fewer than two.
///
/// Flip, run: `text_vars_serde::deserialize` without its padding (a plain
/// `[Option<VarId>; 7]`) fails at *"the two-slot file loads"*.
#[test]
fn a_save_round_trips_the_slots_and_reads_a_two_slot_file() {
    let mut f = fixture();
    bind(TEXT_PARAGRAPH_SPACING, f.size, &mut f);
    let bytes = ondin_core::io::save(&f.doc).unwrap();
    let back = ondin_core::io::load(&bytes).unwrap();
    assert_eq!(
        back.get(f.text).unwrap().text_vars(),
        f.doc.get(f.text).unwrap().text_vars()
    );

    let mut f = fixture();
    bind(TEXT_FAMILY, f.font, &mut f);
    let bytes = ondin_core::io::save(&f.doc).unwrap();
    let json = String::from_utf8(bytes.clone()).unwrap();
    let slots = json
        .split("\"text_vars\"")
        .nth(1)
        .and_then(|t| t.split(']').next())
        .map(|t| t.matches(',').count() + 1);
    assert_eq!(slots, Some(2), "a family binding is written in two slots");
    let back = ondin_core::io::load(&bytes).expect("the two-slot file loads");
    assert_eq!(
        back.get(f.text).unwrap().text_vars()[TEXT_FAMILY],
        Some(f.font)
    );
}
