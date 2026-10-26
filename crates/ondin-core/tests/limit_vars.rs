//! A layout item's size limits bound to number variables (`variable::LimitVars`,
//! held in `LayoutItem::limit_vars`): laid out at the variable's px in the mode
//! in force, the literal `Px(0)` while bound, kept by an item write of what it
//! carries, detached by any other value and by a delete, and saved.

use ondin_core::container::{Dimension, Display, Flex};
use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::{
    Cell, Document, IdSource, Mode, ModeId, NodeId, NodeKind, Operation, Resolved, Transaction,
    VarId, VarKind, Variable,
};

struct F {
    doc: Document,
    frame: NodeId,
    rect: NodeId,
    dark: ModeId,
    /// `size/min`: 100 in Light, 140 in Dark.
    min: VarId,
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xBC);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (frame, rect) = (ids.mint(), ids.mint());
    let (light, dark) = (ids.mint_mode(), ids.mint_mode());
    let min = ids.mint_var();
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
                size: Size::new(400.0, 100.0),
            },
            transform: Some(Affine::IDENTITY),
            name: None,
        },
        Operation::CreateNode {
            id: rect,
            parent: frame,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(40.0, 30.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::SetDisplay {
            id: frame,
            display: Some(Display::Flex(Flex {
                align_items: ondin_core::container::AlignItems::Start,
                ..Default::default()
            })),
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
            var: Variable {
                id: min,
                name: "size/min".into(),
                kind: VarKind::Number,
                cells: [(light, Cell::Number(100.0)), (dark, Cell::Number(140.0))].into(),
                origin: None,
                forked: None,
                description: String::new(),
            },
        },
    ]))
    .expect("a flex frame, a rect and a two-mode table");
    F {
        doc,
        frame,
        rect,
        dark,
        min,
    }
}

fn bind(f: &mut F) {
    let ops = ondin_core::build::bind_limit_ops(&f.doc, f.rect, 0, f.min);
    f.doc.apply(&Transaction(ops)).expect("bound");
}

fn width(doc: &Document, id: NodeId) -> f64 {
    Resolved::rebuild(doc)
        .world_bounds(id)
        .expect("laid")
        .width()
}

fn commit(doc: &mut Document, ops: Vec<Operation>) {
    let tx = ondin_core::build::keep_limit_bindings(doc, Transaction(ops));
    doc.apply(&tx).unwrap();
}

/// **A bound min width lays out at its variable's px in the mode in force**, its
/// literal `Px(0)`.
///
/// Flip, run: `DocView::item` answering the stored item fails at *"Light's
/// 100"*, the rect its own 40.
#[test]
fn a_bound_min_width_lays_out_in_the_mode_in_force() {
    let mut f = fixture();
    bind(&mut f);
    assert_eq!(
        f.doc.get(f.rect).unwrap().item().min_width,
        Dimension::Px(0.0)
    );
    assert_eq!(width(&f.doc, f.rect), 100.0, "Light's 100");
    f.doc
        .apply(&Transaction(vec![Operation::SetMode {
            id: f.frame,
            mode: Some(f.dark),
        }]))
        .unwrap();
    assert_eq!(width(&f.doc, f.rect), 140.0, "Dark's 140");
}

/// **An item write keeps a binding it carries and detaches a new value**: a
/// shrink written over the stored item keeps the limit bound; a min width typed
/// detaches it, the number written laid out.
///
/// Flip, run: `kept_limits` detaching unconditionally fails at *"a shrink write
/// keeps the limit bound"*.
#[test]
fn an_item_write_keeps_what_it_carries_and_detaches_a_new_value() {
    let mut f = fixture();
    bind(&mut f);
    let mut item = *f.doc.get(f.rect).unwrap().item();
    item.shrink = 0.0;
    commit(
        &mut f.doc,
        vec![Operation::SetLayoutItem { id: f.rect, item }],
    );
    assert_eq!(
        f.doc.get(f.rect).unwrap().item().limit_vars[0],
        Some(f.min),
        "a shrink write keeps the limit bound"
    );
    item.min_width = Dimension::Px(60.0);
    commit(
        &mut f.doc,
        vec![Operation::SetLayoutItem { id: f.rect, item }],
    );
    assert_eq!(f.doc.get(f.rect).unwrap().item().limit_vars[0], None);
    assert_eq!(width(&f.doc, f.rect), 60.0, "the number written");
}

/// **A delete detaches the limit to the px it laid out at**, and a save keeps a
/// binding.
#[test]
fn a_delete_detaches_and_a_save_keeps_the_binding() {
    let mut f = fixture();
    bind(&mut f);
    let bytes = ondin_core::io::save(&f.doc).unwrap();
    let back = ondin_core::io::load(&bytes).unwrap();
    assert_eq!(back.get(f.rect).unwrap().item().limit_vars[0], Some(f.min));
    let tx = Transaction(ondin_core::variable::remove_ops(&f.doc, &[f.min]));
    f.doc.apply(&tx).expect("detach, then remove");
    let item = *f.doc.get(f.rect).unwrap().item();
    assert_eq!(
        (item.min_width, item.limit_vars[0]),
        (Dimension::Px(100.0), None)
    );
}
