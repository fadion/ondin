//! D1038 (7)'s remaining bindable numbers bound to number variables: a layer's
//! opacity (§15 D1072), read as a percentage — drawn and exported at its
//! variable's number in the mode in force, its literal 0, detached by a write
//! of anything it does not draw, and detached to what it drew by a delete, a
//! paste into another document and *Paste properties* — and a stroke's width
//! (§15 D1073), bound in the item, drawn and measured at its number, kept by a
//! list write of what it draws or of its own 0.

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

/// The rect given one 2-wide stroke, centred; its item id.
fn stroked(f: &mut F) -> ondin_core::ItemId {
    let item = f.ids.mint_item();
    f.doc
        .apply(&Transaction(vec![Operation::SetStrokes {
            id: f.rect,
            strokes: vec![ondin_core::Keyed::new(
                item,
                ondin_core::Stroke {
                    width: 2.0,
                    ..Default::default()
                },
            )],
        }]))
        .unwrap();
    item
}

fn stroke(doc: &Document, id: NodeId) -> ondin_core::Stroke {
    doc.get(id).unwrap().paint().strokes[0].value.clone()
}

/// The width `id`'s first stroke draws at, through the one door every reader
/// takes.
fn drawn_width(doc: &Document, id: NodeId) -> f64 {
    doc.resolved_paint(id, doc.get(id).unwrap().paint()).strokes[0].width
}

/// **A bound stroke width draws, measures and hits at its variable's number**
/// (§15 D1073): bound to `opacity/muted` — a number, 40 in Light — the stroke
/// draws 40 wide over a literal 0, and the rect's world bounds grow by half of
/// it on every side (a centred stroke), so the box is 40 + 40 wide; a click 15
/// outside the rect's edge, inside that reach, hits it; Dark's 70 widens it
/// again.
///
/// Flip, run: `world_bounds_of_parts` in `Resolved` handed `node.paint()` again
/// fails *"the bounds reach the bound width"* — measured at the 0 literal, 40;
/// `query::hit_test` handing `contains_local` the literal paint fails *"a click
/// in the stroke's reach hits it"*, the bounds still right.
#[test]
fn a_bound_stroke_width_draws_and_measures_at_its_number() {
    let mut f = fixture();
    let item = stroked(&mut f);
    let ops = ondin_core::bind_stroke_width_ops(&f.doc, f.rect, item, f.muted);
    assert_eq!(ops.len(), 1, "one list write");
    f.doc.apply(&Transaction(ops)).unwrap();
    assert_eq!(stroke(&f.doc, f.rect).width, 0.0, "the literal 0");
    assert_eq!(stroke(&f.doc, f.rect).width_var, Some(f.muted));
    assert_eq!(drawn_width(&f.doc, f.rect), 40.0, "Light's 40");
    let res = ondin_core::Resolved::rebuild(&f.doc);
    let b = res.world_bounds(f.rect).unwrap();
    assert_eq!(b.width(), 80.0, "the bounds reach the bound width");
    let hit = ondin_core::hit_test(
        &res,
        &f.doc,
        ondin_core::kurbo::Point::new(-15.0, 10.0),
        0.0,
    );
    assert!(
        hit.contains(&f.rect),
        "a click in the stroke's reach hits it"
    );
    dark(&mut f);
    assert_eq!(drawn_width(&f.doc, f.rect), 70.0, "Dark's 70");
}

/// **A list write keeps a bound width by corners' two halves** (§15 D1073):
/// the stroke written back with its own 0 — every stroke-card edit, a colour
/// here — stays bound, and so does one written the 40 it draws, its literal put
/// back; a width of 5 detaches it at 5; and the stroke made `Custom` detaches it
/// at the 40 it drew, its own four widths then drawn.
///
/// Flip, run: the literal half taken out of `kept_stroke_widths` (`s.width ==
/// old.width ||` dropped) fails *"a colour edit keeps the binding"* — the
/// list's 0 read as a write of 0.
#[test]
fn a_stroke_list_write_keeps_a_bound_width_by_two_halves() {
    let mut f = fixture();
    let item = stroked(&mut f);
    let ops = ondin_core::bind_stroke_width_ops(&f.doc, f.rect, item, f.muted);
    f.doc.apply(&Transaction(ops)).unwrap();
    let write = |doc: &Document, edit: &dyn Fn(&mut ondin_core::Stroke)| {
        let mut strokes = doc.get(f.rect).unwrap().paint().strokes.clone();
        edit(&mut strokes[0].value);
        ondin_core::keep_stroke_width_bindings(
            doc,
            Transaction(vec![Operation::SetStrokes {
                id: f.rect,
                strokes,
            }]),
        )
    };
    let after = |doc: &Document, tx: Transaction| {
        let mut d = doc.clone();
        d.apply(&tx).unwrap();
        stroke(&d, f.rect)
    };
    let recoloured = after(
        &f.doc,
        write(&f.doc, &|s| {
            s.brush = ondin_core::peniko::Color::WHITE.into()
        }),
    );
    assert_eq!(
        (recoloured.width, recoloured.width_var),
        (0.0, Some(f.muted)),
        "a colour edit keeps the binding"
    );
    let drawn = after(&f.doc, write(&f.doc, &|s| s.width = 40.0));
    assert_eq!(
        (drawn.width, drawn.width_var),
        (0.0, Some(f.muted)),
        "a write of what it draws keeps it, the literal put back"
    );
    let five = after(&f.doc, write(&f.doc, &|s| s.width = 5.0));
    assert_eq!((five.width, five.width_var), (5.0, None), "5 detaches at 5");
    let custom = after(
        &f.doc,
        write(&f.doc, &|s| {
            s.sides = ondin_core::StrokeSides::Custom {
                top: 1.0,
                right: 2.0,
                bottom: 3.0,
                left: 4.0,
            }
        }),
    );
    assert_eq!(
        (custom.width, custom.width_var),
        (40.0, None),
        "made Custom, detached at what it drew"
    );
    assert!(
        ondin_core::bind_stroke_width_ops(
            &{
                let mut d = f.doc.clone();
                d.apply(&write(&f.doc, &|s| {
                    s.sides = ondin_core::StrokeSides::Custom {
                        top: 1.0,
                        right: 1.0,
                        bottom: 1.0,
                        left: 1.0,
                    }
                }))
                .unwrap();
                d
            },
            f.rect,
            item,
            f.muted
        )
        .is_empty(),
        "a Custom stroke takes no width binding"
    );
}

/// Detaching keeps what it drew; deleting the variable detaches it the same
/// way; a copy leaving for another document carries the number; the binding
/// round-trips; a colour variable is refused; and a stroke style holding a
/// bound width passes the style check, which refused any variable not a
/// colour until D1073.
#[test]
fn a_stroke_width_binding_detaches_to_what_it_draws_and_round_trips() {
    let mut f = fixture();
    let item = stroked(&mut f);
    let ops = ondin_core::bind_stroke_width_ops(&f.doc, f.rect, item, f.muted);
    f.doc.apply(&Transaction(ops)).unwrap();
    let bytes = ondin_core::io::save(&f.doc).unwrap();
    assert_eq!(ondin_core::io::load(&bytes).unwrap(), f.doc, "round-trips");

    let mut d = f.doc.clone();
    d.apply(&Transaction(ondin_core::detach_stroke_width_ops(
        &d, f.rect, item,
    )))
    .unwrap();
    let s = stroke(&d, f.rect);
    assert_eq!((s.width, s.width_var), (40.0, None), "detached at 40");

    let mut d = f.doc.clone();
    let tx = Transaction(ondin_core::variable::remove_ops(&d, &[f.muted]));
    d.apply(&tx).expect("detach, then remove");
    let s = stroke(&d, f.rect);
    assert_eq!(
        (s.width, s.width_var),
        (40.0, None),
        "deleted, detached at 40"
    );

    let node = f.doc.get(f.rect).unwrap().clone();
    let out = ondin_core::variable::detached(&f.doc, node);
    assert_eq!(
        out.paint().strokes[0].width,
        40.0,
        "a detached copy draws 40"
    );
    assert_eq!(out.paint().strokes[0].width_var, None);

    let mut bad = f.doc.get(f.rect).unwrap().paint().strokes.clone();
    bad[0].value.width_var = Some(f.brand);
    let wrong = f.doc.apply(&Transaction(vec![Operation::SetStrokes {
        id: f.rect,
        strokes: bad,
    }]));
    assert!(wrong.is_err(), "a colour variable cannot bind a width");

    let s = f.ids.mint_style();
    let ops = ondin_core::style::create_ops(
        &f.doc,
        s,
        "Border/Muted",
        f.rect,
        ondin_core::style::StyleSlot::Strokes,
        &[f.rect],
    )
    .unwrap();
    f.doc
        .apply(&Transaction(ops))
        .expect("a stroke style holding a bound width is a valid style");
}

/// **A `Custom` stroke binds where it draws its outline** (§15 D1073's
/// amendment, reading (b)): on an ellipse a `Custom` stroke's sides are not
/// honoured (`geometry::effective_sides` answers `All`), so it draws `width` and
/// binds like any other; on the rect it draws its own four widths and the bind
/// is refused.
///
/// Flip, run: `draws_own_sides` reading `stroke.sides` bare fails *"an
/// ellipse's Custom stroke binds"* with an empty transaction. And
/// `kept_stroke_widths`' own-sides check skipped fails *"a bound Custom stroke
/// pasted onto a box lands detached"* — the stroke new to the list, its binding
/// stated, left bound on the box (D1073's amendment's second reading) — and
/// the two-halves test's *"made Custom, detached at what it drew"* with it.
#[test]
fn a_custom_stroke_binds_only_where_it_draws_its_outline() {
    let mut f = fixture();
    let custom = ondin_core::StrokeSides::Custom {
        top: 1.0,
        right: 1.0,
        bottom: 1.0,
        left: 1.0,
    };
    let (ellipse, item) = (f.ids.mint(), f.ids.mint_item());
    f.doc
        .apply(&Transaction(vec![
            Operation::CreateNode {
                id: ellipse,
                parent: f.frame,
                index: 1,
                kind: NodeKind::Ellipse {
                    size: Size::new(10.0, 10.0),
                },
                transform: None,
                name: None,
            },
            Operation::SetStrokes {
                id: ellipse,
                strokes: vec![ondin_core::Keyed::new(
                    item,
                    ondin_core::Stroke {
                        sides: custom,
                        ..Default::default()
                    },
                )],
            },
        ]))
        .unwrap();
    let ops = ondin_core::bind_stroke_width_ops(&f.doc, ellipse, item, f.muted);
    assert_eq!(ops.len(), 1, "an ellipse's Custom stroke binds");
    let rect_item = stroked(&mut f);
    let mut strokes = f.doc.get(f.rect).unwrap().paint().strokes.clone();
    strokes[0].value.sides = custom;
    f.doc
        .apply(&Transaction(vec![Operation::SetStrokes {
            id: f.rect,
            strokes,
        }]))
        .unwrap();
    assert!(
        ondin_core::bind_stroke_width_ops(&f.doc, f.rect, rect_item, f.muted).is_empty(),
        "a rect's Custom stroke does not"
    );

    // The ellipse's stroke, bound, written onto the rect as a paste writes it —
    // a stroke new to the list, its binding stated — lands detached at the 40 it
    // drew, since on a box it draws its own sides.
    f.doc.apply(&Transaction(ops)).unwrap();
    let pasted = f.doc.get(ellipse).unwrap().paint().strokes.clone();
    assert_eq!(
        pasted[0].width_var,
        Some(f.muted),
        "fixture: bound on the ellipse"
    );
    let kept = ondin_core::kept_stroke_widths(&f.doc, f.rect, pasted);
    assert_eq!(
        (kept[0].width, kept[0].width_var),
        (40.0, None),
        "a bound Custom stroke pasted onto a box lands detached"
    );
}

/// **A bound stroke width follows *Paste properties*' rule** (§15 D1073's
/// amendment, reading (a)): pasted within the document onto a second rect it
/// lands bound to the same variable; copied while bound and pasted after the
/// variable is gone, it lands at the 40 it drew — and the paste commits.
///
/// Flip, run: `paste_properties` copying `width_var` verbatim again (`..s.clone()`
/// alone) fails *"a paste with the variable gone commits"* — the binding
/// dangling, refused by `variable::check`.
#[test]
fn paste_properties_carries_a_bound_stroke_width_by_the_paste_rule() {
    let mut f = fixture();
    let item = stroked(&mut f);
    let ops = ondin_core::bind_stroke_width_ops(&f.doc, f.rect, item, f.muted);
    f.doc.apply(&Transaction(ops)).unwrap();
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
    assert_eq!(stroke(&kept, other).width_var, Some(f.muted), "kept");

    let tx = Transaction(ondin_core::variable::remove_ops(&f.doc, &[f.muted]));
    f.doc.apply(&tx).unwrap();
    let tx = ondin_core::build::paste_properties(&f.doc, &[other], &props, &mut f.ids);
    f.doc
        .apply(&tx)
        .expect("a paste with the variable gone commits");
    let s = stroke(&f.doc, other);
    assert_eq!((s.width, s.width_var), (40.0, None), "at the 40 it drew");
}
