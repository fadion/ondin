//! D1038 (7)'s remaining bindable numbers bound to number variables: a layer's
//! opacity (§15 D1072), read as a percentage — drawn and exported at its
//! variable's number in the mode in force, its literal 0, detached by a write
//! of anything it does not draw, and detached to what it drew by a delete, a
//! paste into another document and *Paste properties*.

use ondin_core::kurbo::{RoundedRectRadii, Size};
use ondin_core::{
    Cell, Document, IdSource, Mode, ModeId, NodeId, NodeKind, Operation, Transaction, VarId,
    VarKind, Variable,
};

struct F {
    doc: Document,
    ids: IdSource,
    frame: NodeId,
    rect: NodeId,
    dark: ModeId,
    /// `opacity/muted`: 40 in Light, 70 in Dark.
    muted: VarId,
    /// `brand/500`, a colour — no opacity binds it.
    brand: VarId,
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xBA);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (frame, rect) = (ids.mint(), ids.mint());
    let (light, dark) = (ids.mint_mode(), ids.mint_mode());
    let (muted, brand) = (ids.mint_var(), ids.mint_var());
    let mode = |id, name: &str| Mode {
        id,
        name: name.into(),
    };
    let var = |id, name: &str, kind, cells: [(ModeId, Cell); 2]| Variable {
        id,
        name: name.into(),
        kind,
        cells: cells.into(),
        origin: None,
        forked: None,
    };
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(100.0, 100.0),
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
                corner_radii: RoundedRectRadii::from_single_radius(0.0),
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
                muted,
                "opacity/muted",
                VarKind::Number,
                [(light, Cell::Number(40.0)), (dark, Cell::Number(70.0))],
            ),
        },
        Operation::AddVariable {
            var: var(
                brand,
                "brand/500",
                VarKind::Color,
                [
                    (light, Cell::Color(ondin_core::peniko::Color::WHITE)),
                    (dark, Cell::Color(ondin_core::peniko::Color::BLACK)),
                ],
            ),
        },
    ]))
    .expect("a frame, a rect and a two-mode table");
    F {
        doc,
        ids,
        frame,
        rect,
        dark,
        muted,
        brand,
    }
}

fn bind(f: &mut F) {
    let ops = ondin_core::bind_opacity_ops(&f.doc, f.rect, f.muted);
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

/// **A bound opacity draws its variable's number as a percentage, in the mode in
/// force** (§15 D1072): 40 in Light draws 0.4, Dark's 70 draws 0.7, the literal
/// 0 throughout. (The SVG writer's reading is `ondin-export`'s
/// `tests/svg.rs`, `a_bound_opacity_exports_at_its_percentage`.)
///
/// Flip, run: `opacity_drawn_in` reading the number as a fraction (no `/ 100`)
/// fails *"Light's 40 is 40%"* at 1 — clamped to 100%.
#[test]
fn a_bound_opacity_draws_its_percentage_in_the_mode_in_force() {
    let mut f = fixture();
    bind(&mut f);
    assert_eq!(f.doc.get(f.rect).unwrap().opacity(), 0.0, "the literal 0");
    assert!(
        (f.doc.drawn_opacity(f.rect) - 0.4).abs() < 1e-6,
        "Light's 40 is 40%"
    );
    dark(&mut f);
    assert!(
        (f.doc.drawn_opacity(f.rect) - 0.7).abs() < 1e-6,
        "Dark's 70"
    );
}

/// **An opacity written over a bound layer detaches it — and only a write of
/// what it draws keeps it** (§15 D1072): one half, visibility's. A write of
/// 0.4 keeps it bound, its literal put back; a write of 0 — *make it invisible*,
/// and the literal — detaches it, drawn at 0.
///
/// Flip, run: the literal half added (`|| *opacity == node.opacity()`) fails *"0%
/// over a bound layer detaches it"* — the write excused as the literal, the layer
/// left bound and drawn at 40%.
#[test]
fn an_opacity_written_over_a_bound_layer_detaches_it() {
    let mut f = fixture();
    bind(&mut f);
    let write = |doc: &Document, opacity| {
        ondin_core::keep_opacity_bindings(
            doc,
            Transaction(vec![Operation::SetOpacity {
                id: f.rect,
                opacity,
            }]),
        )
    };
    assert!(
        write(&f.doc, 0.4).changes_nothing(&f.doc),
        "a write of what it draws keeps the binding"
    );
    let tx = write(&f.doc, 0.0);
    f.doc.apply(&tx).unwrap();
    assert_eq!(
        f.doc.get(f.rect).unwrap().opacity_var(),
        None,
        "0% over a bound layer detaches it"
    );
    assert_eq!(f.doc.drawn_opacity(f.rect), 0.0, "and draws it invisible");
}

/// **`set_opacity_all` compares what each layer draws**, not its literal: 0%
/// over a bound layer is a write — the literal is already 0 — and 40% over one
/// drawing 40% is none.
///
/// Flip, run: the filter on `n.opacity()` again fails *"40% over 40% writes
/// nothing"* — the literal 0 differs from 0.4, so the no-op is written (and the
/// commit's pass then keeps the binding, the write being what it draws).
/// Predicted at *"0% is written"*, which the literal's 0 drops; the first
/// assertion reads first.
#[test]
fn set_opacity_all_compares_what_a_bound_layer_draws() {
    let mut f = fixture();
    bind(&mut f);
    let none = ondin_core::set_opacity_all(&f.doc, &[f.rect], 0.4);
    assert!(none.0.is_empty(), "40% over 40% writes nothing");
    let zero = ondin_core::set_opacity_all(&f.doc, &[f.rect], 0.0);
    assert_eq!(zero.0.len(), 1, "0% is written");
}

/// Detaching keeps what it drew; deleting the variable detaches the same way,
/// in the mode in force; the binding round-trips through a save; and an
/// opacity refuses a colour variable.
#[test]
fn an_opacity_binding_detaches_to_what_it_draws_and_round_trips() {
    let mut f = fixture();
    dark(&mut f);
    bind(&mut f);
    let bytes = ondin_core::io::save(&f.doc).unwrap();
    let back = ondin_core::io::load(&bytes).unwrap();
    assert_eq!(back, f.doc, "the binding round-trips");
    let before = f.doc.clone();
    f.doc
        .apply(&Transaction(ondin_core::detach_opacity_ops(&f.doc, f.rect)))
        .unwrap();
    let node = f.doc.get(f.rect).unwrap();
    assert_eq!(node.opacity_var(), None, "detached");
    assert!((node.opacity() - 0.7).abs() < 1e-6, "at Dark's 70%");
    f.doc = before;
    let tx = Transaction(ondin_core::variable::remove_ops(&f.doc, &[f.muted]));
    f.doc.apply(&tx).expect("detach, then remove");
    let node = f.doc.get(f.rect).unwrap();
    assert_eq!(node.opacity_var(), None, "deleted, detached");
    assert!((node.opacity() - 0.7).abs() < 1e-6, "at what it drew");
    let wrong = f.doc.apply(&Transaction(vec![Operation::SetOpacityVar {
        id: f.rect,
        var: Some(f.brand),
    }]));
    assert!(wrong.is_err(), "a colour variable cannot bind an opacity");
}

/// **A node leaving for a document with none of its variables draws as it
/// did** (`variable::detached`): the copy's opacity is the percentage it drew,
/// unbound.
#[test]
fn a_detached_copy_keeps_the_opacity_it_drew() {
    let mut f = fixture();
    bind(&mut f);
    let node = f.doc.get(f.rect).unwrap().clone();
    let out = ondin_core::variable::detached(&f.doc, node);
    assert_eq!(out.opacity_var(), None);
    assert!((out.opacity() - 0.4).abs() < 1e-6, "40%, not the literal 0");
}

/// **A bound opacity follows *Paste properties*' rule** (§15 D1072, D1047):
/// pasted within the document onto a second rect it lands bound to the same
/// variable; copied while bound and pasted after the variable is gone, it lands
/// at the 40% it drew.
#[test]
fn paste_properties_carries_a_bound_opacity_by_the_paste_rule() {
    let mut f = fixture();
    bind(&mut f);
    let other = f.ids.mint();
    f.doc
        .apply(&Transaction(vec![Operation::CreateNode {
            id: other,
            parent: f.frame,
            index: 1,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: RoundedRectRadii::from_single_radius(0.0),
            },
            transform: None,
            name: None,
        }]))
        .unwrap();
    let props = ondin_core::build::properties_of(&f.doc, f.rect).unwrap();
    let tx = ondin_core::build::paste_properties(&f.doc, &[other], &props, &mut f.ids);
    let mut kept = f.doc.clone();
    kept.apply(&tx).unwrap();
    assert_eq!(
        kept.get(other).unwrap().opacity_var(),
        Some(f.muted),
        "kept"
    );

    let tx = Transaction(ondin_core::variable::remove_ops(&f.doc, &[f.muted]));
    f.doc.apply(&tx).unwrap();
    let tx = ondin_core::build::paste_properties(&f.doc, &[other], &props, &mut f.ids);
    f.doc.apply(&tx).unwrap();
    let node = f.doc.get(other).unwrap();
    assert_eq!(node.opacity_var(), None, "no variable to keep: detached");
    assert!((node.opacity() - 0.4).abs() < 1e-6, "at the 40% it drew");
}
