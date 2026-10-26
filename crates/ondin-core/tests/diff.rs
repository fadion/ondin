//! `ondin_core::diff`: what two states of one document report as changed.

use ondin_core::diff::{Change, Diff, LayerDiff, Status, diff};
use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::variable::{VarKind, Variable};
use ondin_core::{
    Brush, Cell, Document, Fill, IdSource, Mode, ModeId, NodeId, NodeKind, Operation, Transaction,
    VarId, io, keyed_by_position,
};

const RED: Color = Color::from_rgba8(255, 0, 0, 255);
const BLUE: Color = Color::from_rgba8(0, 0, 255, 255);

/// A frame `Card` holding `Bg` (at 10, 10, red) and `Dot`; a main `Button`
/// holding `Label`, and an instance of it holding a copy of `Label`; and a
/// two-mode table with one colour variable.
struct F {
    doc: Document,
    ids: IdSource,
    card: NodeId,
    bg: NodeId,
    dot: NodeId,
    label: NodeId,
    copy: NodeId,
    dark: ModeId,
    brand: VarId,
}

fn rect(id: NodeId, parent: NodeId, index: usize, name: &str, at: (f64, f64)) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index,
        kind: NodeKind::Rect {
            size: Size::new(20.0, 20.0),
            corner_radii: RoundedRectRadii::default(),
        },
        transform: Some(Affine::translate(at)),
        name: Some(name.into()),
    }
}

fn frame(id: NodeId, parent: NodeId, index: usize, name: &str) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index,
        kind: NodeKind::Artboard {
            size: Size::new(100.0, 100.0),
        },
        transform: None,
        name: Some(name.into()),
    }
}

fn fill(id: NodeId, color: Color) -> Operation {
    Operation::SetFills {
        id,
        fills: keyed_by_position([Fill {
            brush: Brush::Solid(color).into(),
            visible: true,
        }]),
    }
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xD1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let [card, bg, dot, main, label, instance, copy] = [(); 7].map(|_| ids.mint());
    let (light, dark) = (ids.mint_mode(), ids.mint_mode());
    let brand = ids.mint_var();
    doc.apply(&Transaction(vec![
        frame(card, root, 0, "Card"),
        rect(bg, card, 0, "Bg", (10.0, 10.0)),
        fill(bg, RED),
        rect(dot, card, 1, "Dot", (40.0, 40.0)),
        frame(main, root, 1, "Button"),
        rect(label, main, 0, "Label", (0.0, 0.0)),
        Operation::SetComponent {
            id: main,
            component: true,
        },
        frame(instance, root, 2, "Button"),
        rect(copy, instance, 0, "Label", (0.0, 0.0)),
        Operation::SetLink {
            id: instance,
            link: Some(main),
        },
        Operation::SetLink {
            id: copy,
            link: Some(label),
        },
        Operation::AddMode {
            mode: Mode {
                id: light,
                name: "Light".into(),
            },
            index: 0,
            cells: vec![],
        },
        Operation::AddMode {
            mode: Mode {
                id: dark,
                name: "Dark".into(),
            },
            index: 1,
            cells: vec![],
        },
        Operation::AddVariable {
            var: Variable {
                id: brand,
                name: "brand/500".into(),
                kind: VarKind::Color,
                cells: [(light, Cell::Color(RED)), (dark, Cell::Color(BLUE))].into(),
                origin: None,
                forked: None,
                description: String::new(),
            },
        },
    ]))
    .expect("the fixture");
    F {
        doc,
        ids,
        card,
        bg,
        dot,
        label,
        copy,
        dark,
        brand,
    }
}

fn edited(f: &F, ops: Vec<Operation>) -> Document {
    let mut doc = f.doc.clone();
    doc.apply(&Transaction(ops)).expect("the edit");
    doc
}

/// The entry whose trail is `trail`, on any page.
fn layer<'a>(d: &'a Diff, trail: &[&str]) -> Option<&'a LayerDiff> {
    d.pages
        .iter()
        .flat_map(|p| &p.layers)
        .find(|l| l.trail == trail)
}

fn change(what: &str, before: &str, after: &str) -> Change {
    Change {
        what: what.into(),
        before: Some(before.into()),
        after: Some(after.into()),
    }
}

fn moved(id: NodeId, x: f64, y: f64) -> Operation {
    Operation::SetTransform {
        id,
        transform: Affine::translate((x, y)),
    }
}

/// A document against its own save and reload: deterministic ids and values,
/// so nothing at all.
#[test]
fn a_document_and_its_reload_differ_in_nothing() {
    let f = fixture();
    let reloaded = io::load(&io::save(&f.doc).unwrap()).unwrap();
    let d = diff(&f.doc, &reloaded);
    assert!(d.is_empty(), "{}", d.text());
}

/// A half-pixel move, a rename and a recolour each read as the inspector shows
/// them; a move under `EPSILON` is noise and reports nothing.
///
/// flip: `differs` answering any non-zero difference → fails at the `Dot` assertion.
#[test]
fn a_half_pixel_move_is_reported_and_float_noise_is_not() {
    let f = fixture();
    let b = edited(
        &f,
        vec![
            moved(f.bg, 10.5, 10.0),
            Operation::SetName {
                id: f.bg,
                name: "Background".into(),
            },
            fill(f.bg, BLUE),
            moved(f.dot, 40.004, 40.0),
        ],
    );
    let d = diff(&f.doc, &b);
    let bg = layer(&d, &["Card", "Background"]).expect("Bg is reported");
    assert_eq!(bg.status, Status::Changed);
    for c in [
        change("name", "“Bg”", "“Background”"),
        change("position", "10, 10", "10.5, 10"),
        change("fill 1", "#FF0000", "#0000FF"),
    ] {
        assert!(
            bg.changes.contains(&c),
            "{c:?} missing from {:?}",
            bg.changes
        );
    }
    assert!(layer(&d, &["Card", "Dot"]).is_none(), "{}", d.text());
}

/// A deleted frame is one entry counting its contents, and a new layer is one
/// entry; neither lists its descendants.
#[test]
fn a_removed_frame_counts_the_layers_it_took() {
    let mut f = fixture();
    let fresh = f.ids.mint();
    let root = f.doc.first_page();
    let b = edited(
        &f,
        vec![
            Operation::DeleteNode { id: f.card },
            rect(fresh, root, 0, "New", (0.0, 0.0)),
        ],
    );
    let d = diff(&f.doc, &b);
    let card = layer(&d, &["Card"]).expect("Card is reported");
    assert_eq!((card.status, card.inside), (Status::Removed, 2));
    assert!(layer(&d, &["Card", "Bg"]).is_none(), "{}", d.text());
    let new = layer(&d, &["New"]).expect("New is reported");
    assert_eq!(
        (new.status, new.kind.as_str()),
        (Status::Added, "Rectangle")
    );
}

/// Raising one layer past its siblings reports that layer's order, not theirs.
///
/// flip: `moved_in_order` answering every shared item whose index changed →
/// fails at the layer count, the frames moved past reported too.
#[test]
fn a_raised_layer_is_the_one_reported_as_reordered() {
    let f = fixture();
    let card = f.card;
    let b = edited(&f, vec![Operation::Reorder { id: card, index: 2 }]);
    let d = diff(&f.doc, &b);
    let entry = layer(&d, &["Card"]).expect("Card is reported");
    assert_eq!(
        entry.changes,
        [change(
            "order",
            "3 of 3 from the top",
            "1 of 3 from the top"
        )]
    );
    assert_eq!(d.pages[0].layers.len(), 1, "{}", d.text());
}

/// A change a copy shares with the layer it was copied from is reported once,
/// on the main; a copy's own override reads under its instance.
///
/// flip: dropping the echo filter in `layer_changes` → fails at the first
/// `is_none`, the copy's fill reported beside the main's.
#[test]
fn a_copy_reports_its_overrides_under_its_instance_and_not_its_mains_changes() {
    let f = fixture();
    let b = edited(&f, vec![fill(f.label, BLUE), fill(f.copy, BLUE)]);
    let d = diff(&f.doc, &b);
    assert!(layer(&d, &["Button", "Label"]).is_some(), "{}", d.text());
    assert!(
        layer(&d, &["Button (instance)", "Label"]).is_none(),
        "{}",
        d.text()
    );

    let b = edited(
        &f,
        vec![Operation::SetOpacity {
            id: f.copy,
            opacity: 0.5,
        }],
    );
    let d = diff(&f.doc, &b);
    let copy = layer(&d, &["Button (instance)", "Label"]).expect("the override is reported");
    assert_eq!(copy.changes, [change("opacity", "1", "0.5")]);
}

/// A field this module never names is compared through the saved form all the
/// same, under its own key.
#[test]
fn a_field_with_no_word_here_is_still_compared() {
    let f = fixture();
    let b = edited(
        &f,
        vec![Operation::SetProportionsLocked {
            id: f.dot,
            locked: true,
        }],
    );
    let d = diff(&f.doc, &b);
    let dot = layer(&d, &["Card", "Dot"]).expect("Dot is reported");
    assert_eq!(dot.changes, [change("proportions locked", "off", "on")]);
}

/// A variable's cell reads under the variable's and the mode's names, not ids.
#[test]
fn a_variable_cell_reads_by_name() {
    let f = fixture();
    let b = edited(
        &f,
        vec![Operation::SetVariableCell {
            id: f.brand,
            mode: f.dark,
            cell: Cell::Color(RED),
        }],
    );
    let d = diff(&f.doc, &b);
    assert_eq!(
        d.document,
        [change("variable “brand/500” Dark", "#0000FF", "#FF0000")],
        "{}",
        d.text()
    );
}

/// A list the file leaves out while empty reads as items added to an empty
/// list, not as the whole list appearing.
///
/// flip: `skipped_default` answering `None` for a list → fails at the `assert_eq`, the
/// change reading `effects added: [drop shadow]`.
#[test]
fn a_first_effect_reads_as_an_item_added() {
    let f = fixture();
    let shadow = ondin_core::EffectKind::all_defaults()[0].clone();
    let b = edited(
        &f,
        vec![Operation::SetEffects {
            id: f.dot,
            effects: keyed_by_position([ondin_core::Effect::new(shadow)]),
        }],
    );
    let d = diff(&f.doc, &b);
    let dot = layer(&d, &["Card", "Dot"]).expect("Dot is reported");
    assert_eq!(
        dot.changes,
        [Change {
            what: "effect 1".into(),
            before: None,
            after: Some("drop shadow".into()),
        }]
    );
}
