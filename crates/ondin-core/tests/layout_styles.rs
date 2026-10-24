//! Layout styles (§5.3f, §15 D1075): a container's layout as a fifth style slot —
//! its whole `Display` and the bindings of its gaps and padding, never an item's
//! grow or `align-self`; linked copies settled at the commit, as the other slots'.

use ondin_core::container::{Display, Flex, FlexDirection};
use ondin_core::kurbo::Size;
use ondin_core::style::{StyleId, StyleSlot};
use ondin_core::{
    Cell, Document, IdSource, Mode, NodeId, NodeKind, OpError, Operation, Transaction, VarId,
    VarKind, Variable,
};

struct F {
    doc: Document,
    ids: IdSource,
    /// Two frames: `a` a column, gap 12 bound to `space/md` (12), padding 8;
    /// `b` plain, no layout. And a rect, which takes no layout.
    a: NodeId,
    b: NodeId,
    rect: NodeId,
    md: VarId,
}

fn frame(id: NodeId, parent: NodeId, index: usize) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index,
        kind: NodeKind::Artboard {
            size: Size::new(200.0, 200.0),
        },
        transform: None,
        name: None,
    }
}

fn column() -> Display {
    Display::Flex(Flex {
        direction: FlexDirection::Column,
        row_gap: 12.0,
        padding: [8.0; 4],
        ..Default::default()
    })
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xBB);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (a, b, rect, mode, md) = (
        ids.mint(),
        ids.mint(),
        ids.mint(),
        ids.mint_mode(),
        ids.mint_var(),
    );
    doc.apply(&Transaction(vec![
        frame(a, root, 0),
        frame(b, root, 1),
        Operation::CreateNode {
            id: rect,
            parent: root,
            index: 2,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        },
        Operation::SetDisplay {
            id: a,
            display: Some(column()),
        },
        Operation::AddMode {
            mode: Mode {
                id: mode,
                name: "Mode 1".into(),
            },
            index: 0,
            cells: vec![],
        },
        Operation::AddVariable {
            var: Variable {
                id: md,
                name: "space/md".into(),
                kind: VarKind::Number,
                cells: [(mode, Cell::Number(12.0))].into(),
                origin: None,
                forked: None,
                description: String::new(),
            },
        },
    ]))
    .expect("two frames, a rect and a number variable");
    let mut vars = [None; 6];
    vars[1] = Some(md); // the row gap
    let bind = ondin_core::bind_spacing_ops(&doc, a, vars);
    doc.apply(&Transaction(bind)).expect("the gap bound");
    F {
        doc,
        ids,
        a,
        b,
        rect,
        md,
    }
}

/// Commit as the app does (`tests/styles.rs`' `commit`).
fn commit(f: &mut F, ops: Vec<Operation>) -> Result<Transaction, OpError> {
    let mut tx = Transaction(ops);
    ondin_core::propagate::owed(&f.doc, &mut tx, &mut f.ids);
    f.doc.apply(&tx).map(|o| o.inverse)
}

fn layout(doc: &Document, id: NodeId) -> ondin_core::style::StyleRefs {
    doc.get(id).unwrap().styles()
}

/// `a`'s layout made a style named *Stack/Column*, `a` and `b` bound to it.
fn styled(f: &mut F) -> StyleId {
    let s = f.ids.mint_style();
    let (a, b) = (f.a, f.b);
    let ops =
        ondin_core::style::create_ops(&f.doc, s, "Stack/Column", a, StyleSlot::Layout, &[a, b])
            .expect("a has a layout");
    commit(f, ops).unwrap();
    s
}

/// **A layout style carries the container's display and its gaps' bindings onto
/// a plain frame** (§15 D1075): `b`, with no layout, bound to *Stack/Column* takes
/// the column, its padding, and the row gap **bound** to `space/md`, its literal
/// 0 beside it — the style's two operations, settled at the commit.
///
/// Flip, run: `write_ops` writing the display alone (no `SetSpacingVars`) fails
/// *"b's gap bound as a's"*.
#[test]
fn a_layout_style_carries_the_display_and_its_bindings() {
    let mut f = fixture();
    let s = styled(&mut f);
    assert_eq!(layout(&f.doc, f.a).layout, Some(s));
    assert_eq!(layout(&f.doc, f.b).layout, Some(s));
    let b = f.doc.get(f.b).unwrap();
    assert_eq!(
        b.display(),
        f.doc.get(f.a).unwrap().display(),
        "b's display is a's"
    );
    assert_eq!(b.spacing_vars()[1], Some(f.md), "b's gap bound as a's");
}

/// **Editing the style reaches every user, and a door's edit of one detaches
/// it** (§15 D1075, D1062's rule): the style's direction made a row rewrites both
/// frames; then `b`'s display written by a door — a gap of 30 — detaches `b`,
/// keeping its 30, while `a` stays bound.
#[test]
fn a_style_edit_reaches_its_users_and_a_door_detaches() {
    let mut f = fixture();
    let s = styled(&mut f);
    let ondin_core::style::StyleValue::Layout(mut l) = f.doc.styles().get(s).unwrap().value.clone()
    else {
        unreachable!("a layout style")
    };
    if let Display::Flex(fx) = &mut l.display {
        fx.direction = FlexDirection::Row;
    }
    commit(
        &mut f,
        vec![Operation::SetStyleValue {
            id: s,
            value: ondin_core::style::StyleValue::Layout(l),
        }],
    )
    .unwrap();
    for id in [f.a, f.b] {
        assert!(
            matches!(
                f.doc.get(id).unwrap().display(),
                Some(Display::Flex(fx)) if fx.direction == FlexDirection::Row
            ),
            "{id:?} is a row"
        );
    }
    let mut d = f.doc.get(f.b).unwrap().display().cloned().unwrap();
    if let Display::Flex(fx) = &mut d {
        fx.column_gap = 30.0;
    }
    let b = f.b;
    commit(
        &mut f,
        vec![Operation::SetDisplay {
            id: b,
            display: Some(d),
        }],
    )
    .unwrap();
    assert_eq!(layout(&f.doc, f.b).layout, None, "b detached");
    assert!(
        matches!(f.doc.get(f.b).unwrap().display(), Some(Display::Flex(fx)) if fx.column_gap == 30.0),
        "keeping what was written"
    );
    assert_eq!(layout(&f.doc, f.a).layout, Some(s), "a stays bound");
}

/// **Only a frame or a group takes a layout style** (§15 D1075): binding the rect
/// is refused at the commit; and a layout style is made only of a layer with a
/// layout — `create_ops` over the plain `b` answers `None`.
#[test]
fn only_a_container_takes_a_layout_style() {
    let mut f = fixture();
    let s = styled(&mut f);
    let rect = f.rect;
    let refused = commit(
        &mut f,
        vec![ondin_core::style::bind_op(rect, StyleSlot::Layout, Some(s))],
    );
    assert!(refused.is_err(), "a rect takes no layout style");
    let mut g = fixture();
    let t = g.ids.mint_style();
    assert!(
        ondin_core::style::create_ops(&g.doc, t, "Plain", g.b, StyleSlot::Layout, &[g.b]).is_none(),
        "no layout, no layout style"
    );
}

/// The binding and the style round-trip through a save; deleting the style
/// detaches its users, their layouts kept; deleting the variable inside it
/// detaches the gap inside the style and on every user to its 12.
#[test]
fn a_layout_style_round_trips_and_its_deletes_detach() {
    let mut f = fixture();
    let s = styled(&mut f);
    let bytes = ondin_core::io::save(&f.doc).unwrap();
    assert_eq!(ondin_core::io::load(&bytes).unwrap(), f.doc, "round-trips");

    let mut g = Document::clone(&f.doc);
    let tx = Transaction(ondin_core::style::remove_ops(&g, &[s]));
    g.apply(&tx).expect("detach, then remove");
    assert_eq!(g.get(f.b).unwrap().styles().layout, None);
    assert!(
        g.get(f.b).unwrap().display().is_some(),
        "b keeps its layout"
    );

    let tx = Transaction(ondin_core::variable::remove_ops(&f.doc, &[f.md]));
    f.doc.apply(&tx).expect("detach, then remove");
    let ondin_core::style::StyleValue::Layout(l) = &f.doc.styles().get(s).unwrap().value else {
        unreachable!()
    };
    assert_eq!(l.spacing_vars, [None; 6], "the style's gap detached");
    assert_eq!(l.display.spacing()[1], 12.0, "at its 12");
    for id in [f.a, f.b] {
        let n = f.doc.get(id).unwrap();
        assert_eq!(n.spacing_vars()[1], None, "{id:?} detached");
        assert_eq!(n.display().unwrap().spacing()[1], 12.0);
        assert_eq!(n.styles().layout, Some(s), "{id:?} still on the style");
    }
}

/// **An *Edit* through a user judges a bound gap as the commit's spacing pass
/// would** (§15 D1075's amendment, reading (b)): with *Stack/Column*'s row gap
/// bound to `space/md` (12), `a`'s display written with the gap it draws, 12,
/// keeps the style's binding and its 0 literal; written 30, the style's gap
/// detaches at 30.
///
/// Flip, run: `edit_through` taking the user's bindings whole (the first cut)
/// fails *"the drawn gap keeps the binding"* — bound, but over a literal of 12
/// rather than 0. Predicted at *"30 detaches the style's gap"*, which the same
/// cut also breaks; the first assertion reads first.
#[test]
fn an_edit_through_a_user_judges_a_bound_gap() {
    let mut f = fixture();
    let s = styled(&mut f);
    let write = |doc: &Document, gap: f64| {
        let mut d = doc.get(f.a).unwrap().display().cloned().unwrap();
        let mut sp = d.spacing();
        sp[1] = gap;
        d.set_spacing(sp);
        let tx = ondin_core::style::edit_through(
            doc,
            Transaction(vec![Operation::SetDisplay {
                id: f.a,
                display: Some(d),
            }]),
            s,
        );
        tx.0.into_iter()
            .find_map(|op| match op {
                Operation::SetStyleValue {
                    value: ondin_core::style::StyleValue::Layout(l),
                    ..
                } => Some(l),
                _ => None,
            })
            .expect("the edit is the style's")
    };
    let same = write(&f.doc, 12.0);
    assert_eq!(
        (same.spacing_vars[1], same.display.spacing()[1]),
        (Some(f.md), 0.0),
        "the drawn gap keeps the binding"
    );
    let other = write(&f.doc, 30.0);
    assert_eq!(
        (other.spacing_vars[1], other.display.spacing()[1]),
        (None, 30.0),
        "30 detaches the style's gap"
    );
}

/// **A gap's *Detach* under the style's *Edit* detaches the style's gap to
/// what it draws**: the detach states display and bindings together, and
/// `edit_through` takes the display as written rather than judging it against
/// the binding the same edit removes.
///
/// flip: `edit_through`'s stated-binding arm removed → fails *"the style's gap
/// detaches at 12"*, at 0.
#[test]
fn a_detach_under_a_layout_styles_edit_keeps_the_number() {
    let mut f = fixture();
    let s = styled(&mut f);
    let detach =
        ondin_core::detach_spacing_ops(&f.doc, f.a, [false, true, false, false, false, false]);
    let tx = ondin_core::style::edit_through(&f.doc, Transaction(detach), s);
    commit(&mut f, tx.0).unwrap();
    let value = match &f.doc.styles().get(s).unwrap().value {
        ondin_core::style::StyleValue::Layout(l) => (**l).clone(),
        v => panic!("not a layout: {v:?}"),
    };
    assert_eq!(
        (value.spacing_vars[1], value.display.spacing()[1]),
        (None, 12.0),
        "the style's gap detaches at 12"
    );
    let b = f.doc.get(f.b).unwrap();
    assert_eq!(
        (b.spacing_vars()[1], b.display().unwrap().spacing()[1]),
        (None, 12.0),
        "and every user with it"
    );
}

/// **A layout style's display detaches a child's size binding it makes
/// computed** (§15 D1075's amendment, reading (a); D1061's *only a hand-set size
/// binds*): a rect in the plain frame `b`, its width bound to `space/md`; `b`
/// given the column style, whose default `align-items: stretch` computes the
/// rect's width — so the binding goes, the width it drew kept. The commit door's
/// size pass had run before the style's `SetDisplay` was written.
///
/// Flip, run: `propagate::owed`'s re-run of `keep_size_bindings` after the first
/// `style::settle` disabled fails *"the stretched width detaches"*. (The re-run
/// after the last `settle` is not reached here: only the first writes the
/// display.)
#[test]
fn a_layout_style_detaches_a_child_size_its_layout_computes() {
    let mut f = fixture();
    let child = f.ids.mint();
    let b = f.b;
    f.doc
        .apply(&Transaction(vec![Operation::CreateNode {
            id: child,
            parent: b,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        }]))
        .unwrap();
    let bind = ondin_core::bind_size_ops(&f.doc, child, [Some(f.md), None]);
    assert!(!bind.is_empty(), "fixture: the width is hand-set and binds");
    f.doc.apply(&Transaction(bind)).unwrap();
    let s = f.ids.mint_style();
    let a = f.a;
    let ops = ondin_core::style::create_ops(&f.doc, s, "Stack/Column", a, StyleSlot::Layout, &[a])
        .unwrap();
    commit(&mut f, ops).unwrap();
    commit(
        &mut f,
        vec![ondin_core::style::bind_op(b, StyleSlot::Layout, Some(s))],
    )
    .unwrap();
    assert!(
        f.doc.get(b).unwrap().display().is_some(),
        "b took the column"
    );
    assert_eq!(
        f.doc.get(child).unwrap().size_vars()[0],
        None,
        "the stretched width detaches"
    );
}
