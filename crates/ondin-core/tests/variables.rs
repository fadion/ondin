//! Variables and modes (§5.3f, §15 D1039): the table's operations and rules, a
//! binding drawn in the mode in force, the save format, `Resolved`'s bound
//! corners, the paste rule and an instance's binding as an override.

use ondin_core::Brush;
use ondin_core::container::{Display, Flex};
use ondin_core::kurbo::{RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    BrushSource, Cell, Document, Fill, IdSource, Mode, ModeId, NodeId, NodeKind, OpError,
    Operation, Placement, Resolved, Transaction, Value, VarId, VarKind, Variable,
    keyed_by_position,
};

const WHITE: Color = Color::from_rgba8(255, 255, 255, 255);
const NAVY: Color = Color::from_rgba8(10, 20, 80, 255);
const RED: Color = Color::from_rgba8(200, 30, 30, 255);

struct F {
    doc: Document,
    ids: IdSource,
    root: NodeId,
    /// A frame holding a rectangle.
    frame: NodeId,
    rect: NodeId,
    light: ModeId,
    dark: ModeId,
    /// `brand`: white in Light, navy in Dark. `radius`: 4 and 12.
    brand: VarId,
    radius: VarId,
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
    }
}

fn fixture() -> F {
    let mut ids = IdSource::new(0xBE);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (frame, rect) = (ids.mint(), ids.mint());
    let (light, dark) = (ids.mint_mode(), ids.mint_mode());
    let (brand, radius) = (ids.mint_var(), ids.mint_var());
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
                corner_radii: RoundedRectRadii::from_single_radius(2.0),
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
                brand,
                "brand/500",
                VarKind::Color,
                [(light, Cell::Color(WHITE)), (dark, Cell::Color(NAVY))],
            ),
        },
        Operation::AddVariable {
            var: var(
                radius,
                "radius/md",
                VarKind::Number,
                [(light, Cell::Number(4.0)), (dark, Cell::Number(12.0))],
            ),
        },
    ]))
    .expect("a frame, a rect and a two-mode table");
    F {
        doc,
        ids,
        root,
        frame,
        rect,
        light,
        dark,
        brand,
        radius,
    }
}

/// `live` against a rebuild, node by node over what a bound corner can move.
fn assert_matches_rebuild(doc: &Document, live: &Resolved) {
    let fresh = Resolved::rebuild(doc);
    for id in ondin_core::subtree_nodes(doc, &[doc.root()]) {
        assert_eq!(live.used_kind(doc, id), fresh.used_kind(doc, id), "{id:?}");
        assert_eq!(live.world_bounds(id), fresh.world_bounds(id), "{id:?}");
        assert_eq!(live.ink_bounds(id), fresh.ink_bounds(id), "{id:?}");
    }
}

fn bind_fill(doc: &mut Document, id: NodeId, var: VarId, alpha: f32) {
    doc.apply(&Transaction(vec![Operation::SetFills {
        id,
        fills: keyed_by_position([Fill {
            brush: BrushSource::Bound { var, alpha },
            visible: true,
        }]),
    }]))
    .expect("a bound fill");
}

fn drawn(doc: &Document, id: NodeId) -> Brush {
    let fill = &doc.get(id).unwrap().paint().fills[0];
    doc.resolve_brush(id, &fill.brush).into_owned()
}

/// A bound fill reads in the **default mode** — the first — until a frame above
/// it sets one, then in that frame's; its `%` multiplies the variable's alpha.
/// Undoing the mode puts the default back.
#[test]
fn a_bound_fill_draws_in_the_mode_in_force() {
    let mut f = fixture();
    bind_fill(&mut f.doc, f.rect, f.brand, 0.5);
    assert_eq!(
        drawn(&f.doc, f.rect),
        Brush::Solid(WHITE.multiply_alpha(0.5))
    );
    let out = f
        .doc
        .apply(&Transaction(vec![Operation::SetMode {
            id: f.frame,
            mode: Some(f.dark),
        }]))
        .unwrap();
    assert_eq!(f.doc.mode_at(f.rect), Some(f.dark));
    assert_eq!(
        drawn(&f.doc, f.rect),
        Brush::Solid(NAVY.multiply_alpha(0.5))
    );
    f.doc.apply(&out.inverse).unwrap();
    assert_eq!(f.doc.mode_at(f.rect), Some(f.light));
}

/// **A group never sets a mode, and a frame only one that exists** — refused at
/// the commit (`variable::check`), so the document is untouched.
#[test]
fn a_mode_is_set_only_on_a_frame_and_only_one_the_document_has() {
    let mut f = fixture();
    let before = f.doc.clone();
    let on_rect = f.doc.apply(&Transaction(vec![Operation::SetMode {
        id: f.rect,
        mode: Some(f.dark),
    }]));
    assert!(matches!(on_rect, Err(OpError::Variables(_))));
    let stray = f.doc.apply(&Transaction(vec![Operation::SetMode {
        id: f.frame,
        mode: Some(ModeId(f.ids.mint())),
    }]));
    assert!(matches!(stray, Err(OpError::Variables(_))));
    assert_eq!(f.doc, before);
}

/// **A variable in use cannot be removed alone; with its users detached in the
/// same transaction it can**, and one undo brings both back (D1038).
#[test]
fn removing_a_variable_in_use_needs_its_users_detached() {
    let mut f = fixture();
    bind_fill(&mut f.doc, f.rect, f.brand, 1.0);
    let alone = f.doc.apply(&Transaction(vec![Operation::RemoveVariable {
        id: f.brand,
    }]));
    assert!(matches!(alone, Err(OpError::Variables(_))));
    let detached = ondin_core::variable::detached(&f.doc, f.doc.get(f.rect).unwrap().clone());
    let before = f.doc.clone();
    let out = f
        .doc
        .apply(&Transaction(vec![
            Operation::SetFills {
                id: f.rect,
                fills: detached.paint().fills.clone(),
            },
            Operation::RemoveVariable { id: f.brand },
        ]))
        .expect("detach, then remove");
    assert_eq!(
        drawn(&f.doc, f.rect),
        Brush::Solid(WHITE),
        "drawn as it was"
    );
    assert!(f.doc.variables().get(f.brand).is_none());
    f.doc.apply(&out.inverse).unwrap();
    assert_eq!(
        f.doc, before,
        "one undo restores the variable and the binding"
    );
}

/// **Removing a mode takes its column, and undoing puts the column back** —
/// the cells travel in the inverse. A frame set to it must be returned to
/// *Inherit* in the same transaction.
#[test]
fn removing_a_mode_carries_its_column_in_the_inverse() {
    let mut f = fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetMode {
            id: f.frame,
            mode: Some(f.dark),
        }]))
        .unwrap();
    let alone = f
        .doc
        .apply(&Transaction(vec![Operation::RemoveMode { id: f.dark }]));
    assert!(matches!(alone, Err(OpError::Variables(_))));
    let before = f.doc.clone();
    let out = f
        .doc
        .apply(&Transaction(vec![
            Operation::SetMode {
                id: f.frame,
                mode: None,
            },
            Operation::RemoveMode { id: f.dark },
        ]))
        .unwrap();
    assert_eq!(f.doc.variables().modes().len(), 1);
    assert!(
        !f.doc
            .variables()
            .get(f.brand)
            .unwrap()
            .cells
            .contains_key(&f.dark)
    );
    f.doc.apply(&out.inverse).unwrap();
    assert_eq!(f.doc, before);
}

/// An alias reads its target in the same mode, and a loop is refused.
#[test]
fn an_alias_follows_its_target_and_a_loop_is_refused() {
    let mut f = fixture();
    let accent = f.ids.mint_var();
    f.doc
        .apply(&Transaction(vec![Operation::AddVariable {
            var: var(
                accent,
                "accent",
                VarKind::Color,
                [(f.light, Cell::Alias(f.brand)), (f.dark, Cell::Color(RED))],
            ),
        }]))
        .unwrap();
    assert_eq!(
        f.doc.variables().resolve(accent, f.light),
        Some(Value::Color(WHITE))
    );
    let looped = f.doc.apply(&Transaction(vec![Operation::SetVariableCell {
        id: f.brand,
        mode: f.light,
        cell: Cell::Alias(accent),
    }]));
    assert!(matches!(looped, Err(OpError::Variables(_))));
    // And a colour variable cannot alias a number one.
    let wrong = f.doc.apply(&Transaction(vec![Operation::SetVariableCell {
        id: f.brand,
        mode: f.dark,
        cell: Cell::Alias(f.radius),
    }]));
    assert!(matches!(wrong, Err(OpError::Variables(_))));
}

/// **A bound corner is drawn from `Resolved`'s used kind**, and a cell edit —
/// which names no node — reaches it through `update`, equal to a rebuild.
#[test]
fn a_bound_corner_is_drawn_from_the_used_kind_and_follows_a_cell_edit() {
    let mut f = fixture();
    let ops = ondin_core::bind_corners_ops(&f.doc, f.rect, [Some(f.radius); 4]);
    f.doc.apply(&Transaction(ops)).unwrap();
    let mut res = Resolved::rebuild(&f.doc);
    let radius_of = |res: &Resolved, doc: &Document| match res.used_kind(doc, f.rect) {
        Some(NodeKind::Rect { corner_radii, .. }) => corner_radii.top_left,
        other => panic!("not a rect: {other:?}"),
    };
    assert_eq!(radius_of(&res, &f.doc), 4.0, "Light's 4, not the stored 0");
    match f.doc.get(f.rect).unwrap().kind() {
        NodeKind::Rect { corner_radii, .. } => {
            assert_eq!(corner_radii.top_left, 0.0, "the bound literal is written 0")
        }
        _ => unreachable!(),
    }
    let out = f
        .doc
        .apply(&Transaction(vec![Operation::SetVariableCell {
            id: f.radius,
            mode: f.light,
            cell: Cell::Number(9.0),
        }]))
        .unwrap();
    assert!(
        out.dirty.0.contains(&f.rect),
        "the cell edit dirties its reader"
    );
    res.update(&f.doc, &out.dirty);
    assert_eq!(radius_of(&res, &f.doc), 9.0);
    assert_matches_rebuild(&f.doc, &res);
    // A mode set on the frame dirties only the frame; `update` takes its subtree.
    let out = f
        .doc
        .apply(&Transaction(vec![Operation::SetMode {
            id: f.frame,
            mode: Some(f.dark),
        }]))
        .unwrap();
    res.update(&f.doc, &out.dirty);
    assert_eq!(radius_of(&res, &f.doc), 12.0);
    assert_matches_rebuild(&f.doc, &res);
}

/// Detaching corners writes the radius they draw, and the binding goes.
#[test]
fn detaching_corners_keeps_what_they_draw() {
    let mut f = fixture();
    let ops = ondin_core::bind_corners_ops(&f.doc, f.rect, [Some(f.radius), None, None, None]);
    f.doc.apply(&Transaction(ops)).unwrap();
    let ops = ondin_core::detach_corners_ops(&f.doc, f.rect);
    f.doc.apply(&Transaction(ops)).unwrap();
    let node = f.doc.get(f.rect).unwrap();
    assert_eq!(node.corner_vars(), [None; 4]);
    match node.kind() {
        NodeKind::Rect { corner_radii, .. } => {
            assert_eq!(
                [corner_radii.top_left, corner_radii.top_right],
                [4.0, 2.0],
                "the bound corner its number, the free one untouched"
            );
        }
        _ => unreachable!(),
    }
}

/// **A save holds the table, the bindings and the frame's mode, and reads back
/// equal**; saving twice gives the same bytes (invariant 9).
#[test]
fn the_table_and_its_bindings_round_trip_through_a_save() {
    let mut f = fixture();
    bind_fill(&mut f.doc, f.rect, f.brand, 0.25);
    let ops = ondin_core::bind_corners_ops(&f.doc, f.rect, [Some(f.radius); 4]);
    f.doc.apply(&Transaction(ops)).unwrap();
    f.doc
        .apply(&Transaction(vec![Operation::SetMode {
            id: f.frame,
            mode: Some(f.dark),
        }]))
        .unwrap();
    let bytes = ondin_core::io::save(&f.doc).unwrap();
    let back = ondin_core::io::load(&bytes).unwrap();
    assert_eq!(back, f.doc);
    assert_eq!(ondin_core::io::save(&back).unwrap(), bytes);
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.contains("\"schema_version\": 8"), "written as v8");
    assert!(
        text.contains("\"alpha\": 0.25"),
        "the `%` rides in the binding"
    );
}

/// **A file binding a variable it does not hold is refused on load**, the
/// commit's rule — a binding to nothing has no value to draw.
#[test]
fn a_file_binding_a_missing_variable_is_refused() {
    let mut f = fixture();
    bind_fill(&mut f.doc, f.rect, f.brand, 1.0);
    let bytes = ondin_core::io::save(&f.doc).unwrap();
    let mut value: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    // Re-id the bound variable in the table and nowhere else.
    let vars = value["variables"]["vars"].as_array_mut().unwrap();
    let entry = vars
        .iter_mut()
        .find(|v| v["name"] == "brand/500")
        .expect("the fixture's table entry");
    entry["id"] = serde_json::Value::from("ff:1");
    let broken = serde_json::to_vec(&value).unwrap();
    assert!(ondin_core::io::load(&broken).is_err());
    assert!(ondin_core::io::load(&bytes).is_ok(), "the control loads");
}

/// The paste rule (D1038): within the document a binding is kept; into another
/// document with a variable of the same name and kind it is rebound; into one
/// without, it detaches to the value drawn at the copy.
#[test]
fn a_paste_keeps_rebinds_or_detaches() {
    let mut f = fixture();
    bind_fill(&mut f.doc, f.rect, f.brand, 1.0);
    f.doc
        .apply(&Transaction(vec![Operation::SetMode {
            id: f.frame,
            mode: Some(f.dark),
        }]))
        .unwrap();
    let copy = vec![f.doc.capture_subtree(f.rect).unwrap()];
    let carried = ondin_core::variable::carried(&f.doc, &copy);

    // The same document: kept.
    let same = ondin_core::variable::land(&f.doc, &carried, copy[0].clone());
    assert_eq!(same[0].paint().fills[0].brush.var(), Some(f.brand));

    // Another document holding a `brand/500` colour of its own: rebound to it.
    let mut ids = IdSource::new(0xCD);
    let mut other = Document::new(ids.mint());
    let (m, theirs) = (ids.mint_mode(), ids.mint_var());
    other
        .apply(&Transaction(vec![
            Operation::AddMode {
                mode: mode(m, "Default"),
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: theirs,
                    name: "brand/500".into(),
                    kind: VarKind::Color,
                    cells: [(m, Cell::Color(RED))].into(),
                },
            },
        ]))
        .unwrap();
    let rebound = ondin_core::variable::land(&other, &carried, copy[0].clone());
    assert_eq!(rebound[0].paint().fills[0].brush.var(), Some(theirs));

    // A document with no such variable: detached to the copy's drawn value —
    // navy, the Dark mode the source frame set.
    let bare = Document::new(ids.mint());
    let landed = ondin_core::variable::land(&bare, &carried, copy[0].clone());
    assert_eq!(
        landed[0].paint().fills[0].brush,
        BrushSource::Literal(Brush::Solid(NAVY))
    );
    // And it pastes: the commit's check is satisfied.
    let root = bare.root();
    let (tx, _) = ondin_core::insert_subtrees(
        &bare,
        &mut ids,
        &[Placement {
            nodes: landed,
            parent: root,
            index: None,
        }],
        Default::default(),
    );
    bare.clone().apply(&tx).expect("a detached paste lands");
}

/// **A property paste follows a paste's rule** (§15 D1047): copied off a fill
/// bound to *brand/500* at 50% in Dark, it stays bound in the same document,
/// rebinds to another document's *brand/500*, and detaches to the navy it drew
/// where there is none — each landing a commit accepts. Until D1047 the binding
/// was carried verbatim and the commit refused it anywhere but at home.
///
/// Flip, run: carrying the bound brush verbatim fails at *"lands rebound"* —
/// `BadBinding`, the commit's refusal this closes — before the predicted
/// *"rebound to theirs"* is reached.
#[test]
fn a_property_paste_keeps_rebinds_or_detaches() {
    let mut f = fixture();
    bind_fill(&mut f.doc, f.rect, f.brand, 0.5);
    f.doc
        .apply(&Transaction(vec![Operation::SetMode {
            id: f.frame,
            mode: Some(f.dark),
        }]))
        .unwrap();
    let props = ondin_core::build::properties_of(&f.doc, f.rect).unwrap();

    let target = |doc: &mut Document, ids: &mut IdSource| {
        let id = ids.mint();
        let root = doc.root();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: RoundedRectRadii::from_single_radius(0.0),
            },
            transform: None,
            name: None,
        }]))
        .unwrap();
        id
    };
    let fill0 = |doc: &Document, id: NodeId| doc.get(id).unwrap().paint().fills[0].brush.clone();

    // The same document: kept, `%` and all.
    let mut ids = IdSource::new(0xCE);
    let here = target(&mut f.doc, &mut ids);
    let tx = ondin_core::build::paste_properties(&f.doc, &[here], &props, &mut ids);
    f.doc.apply(&tx).expect("lands at home");
    assert_eq!(
        fill0(&f.doc, here),
        BrushSource::Bound {
            var: f.brand,
            alpha: 0.5
        }
    );

    // Another document holding a `brand/500` colour of its own.
    let mut other = Document::new(ids.mint());
    let (m, theirs) = (ids.mint_mode(), ids.mint_var());
    other
        .apply(&Transaction(vec![
            Operation::AddMode {
                mode: mode(m, "Default"),
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: theirs,
                    name: "brand/500".into(),
                    kind: VarKind::Color,
                    cells: [(m, Cell::Color(RED))].into(),
                },
            },
        ]))
        .unwrap();
    let there = target(&mut other, &mut ids);
    let tx = ondin_core::build::paste_properties(&other, &[there], &props, &mut ids);
    other.apply(&tx).expect("lands rebound");
    assert_eq!(
        fill0(&other, there),
        BrushSource::Bound {
            var: theirs,
            alpha: 0.5
        },
        "rebound to theirs"
    );

    // A document with none: detached to what it drew — navy at 50%.
    let mut bare = Document::new(ids.mint());
    let nowhere = target(&mut bare, &mut ids);
    let tx = ondin_core::build::paste_properties(&bare, &[nowhere], &props, &mut ids);
    bare.apply(&tx).expect("lands detached");
    assert_eq!(
        fill0(&bare, nowhere),
        BrushSource::Literal(Brush::Solid(NAVY.multiply_alpha(0.5)))
    );
}

/// **An instance's binding is an override compared by the binding** (§5.3d): a
/// main rebinding *brand* → *accent* reaches a copy still bound to *brand*, and
/// a copy bound elsewhere keeps its own.
#[test]
fn a_main_rebinding_reaches_a_copy_bound_as_it_was() {
    let mut f = fixture();
    bind_fill(&mut f.doc, f.rect, f.brand, 1.0);
    f.doc
        .apply(&Transaction(vec![Operation::SetComponent {
            id: f.frame,
            component: true,
        }]))
        .unwrap();
    let place = || Placement {
        nodes: f.doc.capture_subtree(f.frame).unwrap(),
        parent: f.root,
        index: None,
    };
    let (tx, made) =
        ondin_core::insert_subtrees(&f.doc, &mut f.ids, &[place(), place()], Default::default());
    f.doc.apply(&tx).unwrap();
    let copy_rect = |doc: &Document, i: NodeId| doc.get(i).unwrap().children()[0];
    let (kept, own) = (copy_rect(&f.doc, made[0]), copy_rect(&f.doc, made[1]));
    let accent = f.ids.mint_var();
    let red = f.ids.mint_var();
    f.doc
        .apply(&Transaction(vec![
            Operation::AddVariable {
                var: var(
                    accent,
                    "accent",
                    VarKind::Color,
                    [(f.light, Cell::Color(RED)), (f.dark, Cell::Color(RED))],
                ),
            },
            Operation::AddVariable {
                var: var(
                    red,
                    "red",
                    VarKind::Color,
                    [(f.light, Cell::Color(RED)), (f.dark, Cell::Color(RED))],
                ),
            },
        ]))
        .unwrap();
    // The second copy binds elsewhere — its own override.
    bind_fill(&mut f.doc, own, red, 1.0);
    // The main rebinds.
    let mut next = f.doc.get(f.rect).unwrap().paint().fills.clone();
    next[0].brush = BrushSource::Bound {
        var: accent,
        alpha: 1.0,
    };
    let mut tx = Transaction(vec![Operation::SetFills {
        id: f.rect,
        fills: next,
    }]);
    ondin_core::propagate::owed(&f.doc, &mut tx, &mut f.ids);
    f.doc.apply(&tx).unwrap();
    let bound = |id| f.doc.get(id).unwrap().paint().fills[0].brush.var();
    assert_eq!(
        bound(kept),
        Some(accent),
        "the copy bound as the main was follows"
    );
    assert_eq!(bound(own), Some(red), "the copy bound elsewhere keeps it");
}

/// **An instance root's visibility binding is its own, as its visibility is**
/// (§15 D1059, `propagate::is_placement`): a main's root bound to a boolean
/// reaches no instance root — which would hide every instance wherever the
/// variable is false — while a layer inside the main, bound the same way, does
/// reach its copy; and an instance root's own binding is no override, so *Reset
/// all* leaves it.
///
/// Flip, run: `SetVisibleVar` out of `is_placement` fails *"the main's root
/// binding stays the main's"*.
#[test]
fn an_instance_roots_visibility_binding_is_its_own() {
    let mut f = fixture();
    let show = show_var(&mut f);
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
    let inst = made[0];
    let copy_rect = f.doc.get(inst).unwrap().children()[0];
    let mut tx = Transaction(
        ondin_core::bind_visible_ops(&f.doc, f.frame, show)
            .into_iter()
            .chain(ondin_core::bind_visible_ops(&f.doc, f.rect, show))
            .collect(),
    );
    ondin_core::propagate::owed(&f.doc, &mut tx, &mut f.ids);
    f.doc.apply(&tx).unwrap();
    assert_eq!(
        f.doc.get(inst).unwrap().visible_var(),
        None,
        "the main's root binding stays the main's"
    );
    assert_eq!(
        f.doc.get(copy_rect).unwrap().visible_var(),
        Some(show),
        "a layer inside the main reaches its copy"
    );
    let own = Transaction(ondin_core::bind_visible_ops(&f.doc, inst, show));
    f.doc.apply(&own).unwrap();
    assert!(
        !ondin_core::reset::overrides(&f.doc, inst)
            .iter()
            .any(|o| matches!(o.reset, Operation::SetVisibleVar { .. })),
        "an instance root's own binding is no override"
    );
}

/// **Deleting a variable detaches every user and every alias to it** — the fill
/// to the colour it drew, the bound corner to its number, an alias to the value
/// it resolved to — and one undo restores the lot (`variable::remove_ops`, D1038).
#[test]
fn deleting_a_variable_detaches_its_fields_and_its_aliases() {
    let mut f = fixture();
    bind_fill(&mut f.doc, f.rect, f.brand, 1.0);
    let ops = ondin_core::bind_corners_ops(&f.doc, f.rect, [Some(f.radius), None, None, None]);
    f.doc.apply(&Transaction(ops)).unwrap();
    let alias = f.ids.mint_var();
    f.doc
        .apply(&Transaction(vec![Operation::AddVariable {
            var: var(
                alias,
                "button/bg",
                VarKind::Color,
                [(f.light, Cell::Alias(f.brand)), (f.dark, Cell::Color(RED))],
            ),
        }]))
        .unwrap();
    assert_eq!(ondin_core::variable::users(&f.doc, f.brand), vec![f.rect]);
    let before = f.doc.clone();
    let tx = Transaction(ondin_core::variable::remove_ops(
        &f.doc,
        &[f.brand, f.radius],
    ));
    let out = f.doc.apply(&tx).expect("detach, then remove");
    assert_eq!(drawn(&f.doc, f.rect), Brush::Solid(WHITE));
    let node = f.doc.get(f.rect).unwrap();
    assert_eq!(node.corner_vars(), [None; 4]);
    assert!(matches!(
        node.kind(),
        NodeKind::Rect { corner_radii, .. } if corner_radii.top_left == 4.0 && corner_radii.top_right == 2.0
    ));
    assert_eq!(
        f.doc.variables().get(alias).unwrap().cells[&f.light],
        Cell::Color(WHITE),
        "the alias became the value it read"
    );
    f.doc.apply(&out.inverse).unwrap();
    assert_eq!(f.doc, before);
}

/// Deleting a mode returns its frames to *Inherit*; renaming a group moves its
/// members and refuses a name already taken (G14).
#[test]
fn deleting_a_mode_and_renaming_a_group() {
    let mut f = fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetMode {
            id: f.frame,
            mode: Some(f.dark),
        }]))
        .unwrap();
    assert_eq!(ondin_core::variable::frames_set_to(&f.doc, f.dark), 1);
    let tx = Transaction(ondin_core::variable::remove_mode_ops(&f.doc, f.dark));
    f.doc.apply(&tx).unwrap();
    assert_eq!(f.doc.get(f.frame).unwrap().mode(), None);

    let ops = ondin_core::variable::rename_group_ops(&f.doc, "brand", "ink").unwrap();
    f.doc.apply(&Transaction(ops)).unwrap();
    assert_eq!(f.doc.variables().get(f.brand).unwrap().name, "ink/500");
    let ops = ondin_core::variable::rename_group_ops(&f.doc, "ink", "").unwrap();
    f.doc.apply(&Transaction(ops)).unwrap();
    assert_eq!(f.doc.variables().get(f.brand).unwrap().name, "500");
    // `radius/md` → `md` is free; then `radius` again over a taken `md`.
    let clash = f.ids.mint_var();
    f.doc
        .apply(&Transaction(vec![Operation::AddVariable {
            // One cell: Dark went above.
            var: Variable {
                id: clash,
                name: "md".into(),
                kind: VarKind::Number,
                cells: [(f.light, Cell::Number(1.0))].into(),
            },
        }]))
        .unwrap();
    assert_eq!(
        ondin_core::variable::rename_group_ops(&f.doc, "radius", ""),
        Err("md".to_string())
    );
}

/// **A radius written over a bound corner detaches only that corner**
/// (`build::keep_corner_bindings`, D1038 (2)): written as a field writes it —
/// every corner as drawn — a click is a no-op and a per-corner edit of one leaves
/// its bound neighbours bound at their 0 literal; written as the Scale tool
/// writes it — from the literals — a bound corner keeps its binding rather than
/// turning square; and a bind, which writes the bindings itself, is left alone.
///
/// Flip, run: keeping a binding only for the drawn value — the second cut —
/// fails *"a literal-built write keeps the binding"*.
///
/// Flips, run: the pass answering `tx` untouched, and comparing the written
/// value with the corner's literal rather than with what it draws — the first
/// cut — both fail *"a click through a bound field is a no-op"*, the first
/// assertion either can reach: a click writes the drawn 4 over a literal 0, and
/// both read that as an edit. (This doc predicted *"the edited corner detaches"*
/// for the first; it is never reached.)
#[test]
fn a_radius_written_over_a_bound_corner_detaches_that_corner() {
    let mut f = fixture();
    let bind = Transaction(ondin_core::bind_corners_ops(
        &f.doc,
        f.rect,
        [Some(f.radius); 4],
    ));
    let bind = ondin_core::keep_corner_bindings(&f.doc, bind);
    f.doc.apply(&bind).unwrap();
    assert_eq!(
        f.doc.get(f.rect).unwrap().corner_vars(),
        [Some(f.radius); 4],
        "a bind is its own"
    );

    let corners = |doc: &Document| match doc.get(f.rect).unwrap().kind() {
        NodeKind::Rect { corner_radii, .. } => *corner_radii,
        _ => unreachable!(),
    };
    // **What a field writes**: all four corners as drawn — Light's 4 — with the
    // edited one changed. A bare click writes all four unchanged.
    let click = Transaction(vec![Operation::SetGeometry {
        id: f.rect,
        geometry: ondin_core::GeometryPatch::CornerRadius(4.0),
    }]);
    let click = ondin_core::keep_corner_bindings(&f.doc, click);
    assert!(
        click.changes_nothing(&f.doc),
        "a click through a bound field is a no-op"
    );
    let one = Transaction(vec![Operation::SetGeometry {
        id: f.rect,
        geometry: ondin_core::GeometryPatch::CornerRadii(RoundedRectRadii {
            top_left: 9.0,
            top_right: 4.0,
            bottom_right: 4.0,
            bottom_left: 4.0,
        }),
    }]);
    f.doc
        .apply(&ondin_core::keep_corner_bindings(&f.doc, one))
        .unwrap();
    let vars = f.doc.get(f.rect).unwrap().corner_vars();
    assert_eq!(vars[0], None, "the edited corner detaches");
    assert_eq!(vars[1..], [Some(f.radius); 3], "its neighbours stay bound");
    assert_eq!(
        [corners(&f.doc).top_left, corners(&f.doc).top_right],
        [9.0, 0.0],
        "and keep their 0 literal"
    );

    // **A writer that built the radii from the literals** — the Scale tool's
    // photographic scale — writes a bound corner its own 0: still bound, not
    // turned square, while the free corner takes its scaled value.
    let scaled = Transaction(vec![Operation::SetGeometry {
        id: f.rect,
        geometry: ondin_core::GeometryPatch::CornerRadii(RoundedRectRadii {
            top_left: 18.0,
            top_right: 0.0,
            bottom_right: 0.0,
            bottom_left: 0.0,
        }),
    }]);
    f.doc
        .apply(&ondin_core::keep_corner_bindings(&f.doc, scaled))
        .unwrap();
    let vars = f.doc.get(f.rect).unwrap().corner_vars();
    assert_eq!(
        vars[1..],
        [Some(f.radius); 3],
        "a literal-built write keeps the binding"
    );
    assert_eq!(corners(&f.doc).top_left, 18.0);
}

/// `f`'s frame made a flex row — gap 5, no padding — holding its rect and a
/// second one beside it, which is returned.
fn flex_frame(f: &mut F) -> NodeId {
    let second = f.ids.mint();
    f.doc
        .apply(&Transaction(vec![
            Operation::CreateNode {
                id: second,
                parent: f.frame,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(40.0, 20.0),
                    corner_radii: RoundedRectRadii::from_single_radius(0.0),
                },
                transform: None,
                name: None,
            },
            Operation::SetDisplay {
                id: f.frame,
                display: Some(Display::Flex(Flex {
                    column_gap: 5.0,
                    ..Flex::default()
                })),
            },
        ]))
        .expect("a flex row of two");
    second
}

fn spacing(doc: &Document, id: NodeId) -> [f64; 6] {
    doc.get(id).unwrap().display().unwrap().spacing()
}

/// **A bound gap and padding are laid out at their variable's number, in the
/// mode in force** (§15 D1058), never at the 0 their literals hold: bound to
/// `radius/md` (4 in Light, 12 in Dark), the column gap and the left padding
/// place the first rect at 4 and the second at 4 + 40 + 4, then at 12 and 64
/// once the frame sets Dark; a cell's edit moves them again.
///
/// Flip, run: `UsedView::display` and `CommittedView::display` answering the
/// stored layout (`n.display()`) rather than `resolved_display` fails *"the
/// padding is Light's 4"* at 0 — the literal laid out.
#[test]
fn a_bound_gap_and_padding_lay_out_at_the_variables_number() {
    let mut f = fixture();
    let second = flex_frame(&mut f);
    let mut vars = [None; 6];
    vars[0] = Some(f.radius);
    vars[5] = Some(f.radius);
    let ops = ondin_core::bind_spacing_ops(&f.doc, f.frame, vars);
    f.doc.apply(&Transaction(ops)).unwrap();
    assert_eq!(
        spacing(&f.doc, f.frame),
        [0.0; 6],
        "the bound literals are 0"
    );
    let x = |doc: &Document, id| Resolved::rebuild(doc).world_bounds(id).unwrap().min_x();
    assert_eq!(x(&f.doc, f.rect), 4.0, "the padding is Light's 4");
    assert_eq!(x(&f.doc, second), 48.0, "and so is the gap");
    f.doc
        .apply(&Transaction(vec![Operation::SetMode {
            id: f.frame,
            mode: Some(f.dark),
        }]))
        .unwrap();
    assert_eq!(x(&f.doc, f.rect), 12.0, "Dark's 12");
    assert_eq!(x(&f.doc, second), 64.0);
    f.doc
        .apply(&Transaction(vec![Operation::SetVariableCell {
            id: f.radius,
            mode: f.dark,
            cell: Cell::Number(2.0),
        }]))
        .unwrap();
    assert_eq!(x(&f.doc, second), 44.0, "a cell's edit lays it out again");
}

/// **A gap or a padding written over a bound slot detaches it, with
/// `keep_corner_bindings`' two halves** (§15 D1058): written what it lays out
/// at, or its own 0 literal, it stays bound; written anything else it detaches
/// to that number; a layout taken away clears every binding.
///
/// Flip, run: dropping the literal half (`|| written[k] == literal[k]`) fails
/// *"a write from the document's own layout keeps the binding"* — the Container
/// card's `display_tx` changing the row gap carries the bound column gap's 0
/// forward, which then read as an edit and detached it.
#[test]
fn a_spacing_written_over_a_bound_slot_detaches_it() {
    let mut f = fixture();
    flex_frame(&mut f);
    let mut vars = [None; 6];
    vars[0] = Some(f.radius);
    vars[2] = Some(f.radius);
    let bind = Transaction(ondin_core::bind_spacing_ops(&f.doc, f.frame, vars));
    f.doc
        .apply(&ondin_core::keep_spacing_bindings(&f.doc, bind))
        .unwrap();
    let write = |doc: &Document, s: [f64; 6]| {
        let mut d = doc.get(f.frame).unwrap().display().unwrap().clone();
        d.set_spacing(s);
        ondin_core::keep_spacing_bindings(
            doc,
            Transaction(vec![Operation::SetDisplay {
                id: f.frame,
                display: Some(d),
            }]),
        )
    };
    // The document's own layout with the row gap changed: both bound slots
    // written their 0.
    let tx = write(&f.doc, [0.0, 7.0, 0.0, 0.0, 0.0, 0.0]);
    f.doc.apply(&tx).unwrap();
    assert_eq!(
        f.doc.get(f.frame).unwrap().spacing_vars(),
        vars,
        "a write from the document's own layout keeps the binding"
    );
    // A field's write over the layout as drawn: the gap's Light 4 handed back,
    // the top padding written 9.
    let tx = write(&f.doc, [4.0, 7.0, 9.0, 0.0, 0.0, 0.0]);
    f.doc.apply(&tx).unwrap();
    let now = f.doc.get(f.frame).unwrap().spacing_vars();
    assert_eq!(
        now[0],
        Some(f.radius),
        "the gap written as drawn stays bound"
    );
    assert_eq!(now[2], None, "the padding written 9 detaches");
    assert_eq!(spacing(&f.doc, f.frame)[..3], [0.0, 7.0, 9.0]);
    let tx = ondin_core::keep_spacing_bindings(
        &f.doc,
        Transaction(vec![Operation::SetDisplay {
            id: f.frame,
            display: None,
        }]),
    );
    f.doc.apply(&tx).unwrap();
    assert_eq!(
        f.doc.get(f.frame).unwrap().spacing_vars(),
        [None; 6],
        "a layout taken away takes its bindings"
    );
}

/// Detaching a slot keeps the number it laid out at; deleting the variable
/// detaches every slot bound to it the same way; and the bindings survive a save.
#[test]
fn spacing_bindings_detach_to_what_they_draw_and_round_trip() {
    let mut f = fixture();
    flex_frame(&mut f);
    let mut vars = [None; 6];
    vars[1] = Some(f.radius);
    vars[3] = Some(f.radius);
    let ops = ondin_core::bind_spacing_ops(&f.doc, f.frame, vars);
    f.doc.apply(&Transaction(ops)).unwrap();
    let bytes = ondin_core::io::save(&f.doc).unwrap();
    let back = ondin_core::io::load(&bytes).unwrap();
    assert_eq!(back, f.doc, "the bindings round-trip");
    let mut which = [false; 6];
    which[1] = true;
    let ops = ondin_core::detach_spacing_ops(&f.doc, f.frame, which);
    f.doc.apply(&Transaction(ops)).unwrap();
    assert_eq!(f.doc.get(f.frame).unwrap().spacing_vars()[1], None);
    assert_eq!(
        spacing(&f.doc, f.frame)[1],
        4.0,
        "the row gap keeps Light's 4"
    );
    let tx = Transaction(ondin_core::variable::remove_ops(&f.doc, &[f.radius]));
    f.doc.apply(&tx).expect("detach, then remove");
    assert_eq!(f.doc.get(f.frame).unwrap().spacing_vars(), [None; 6]);
    assert_eq!(spacing(&f.doc, f.frame)[3], 4.0, "the right padding too");
}

/// A boolean variable `show`: true in Light, false in Dark.
fn show_var(f: &mut F) -> VarId {
    let show = f.ids.mint_var();
    f.doc
        .apply(&Transaction(vec![Operation::AddVariable {
            var: var(
                show,
                "state/show",
                VarKind::Boolean,
                [
                    (f.light, Cell::Boolean(true)),
                    (f.dark, Cell::Boolean(false)),
                ],
            ),
        }]))
        .expect("a boolean variable");
    show
}

/// **A layer's visibility bound to a boolean variable shows by its value in
/// the mode in force** (§15 D1059) — drawn, picked and laid out by it: in a flex
/// row, the bound rect hidden in Dark leaves the flow and the second rect takes
/// its place at the padding's edge; Light brings it back.
///
/// Flip, run: `DocView::visible` answering the literal (`n.visible()`) fails
/// *"hidden in Dark, out of the flow"* — the literal's `true` laying it out.
#[test]
fn a_bound_visibility_shows_by_its_variable_in_the_mode_in_force() {
    let mut f = fixture();
    let second = flex_frame(&mut f);
    let show = show_var(&mut f);
    let ops = ondin_core::bind_visible_ops(&f.doc, f.rect, show);
    f.doc.apply(&Transaction(ops)).unwrap();
    assert!(f.doc.shown(f.rect), "Light's true");
    let x = |doc: &Document| Resolved::rebuild(doc).world_bounds(second).unwrap().min_x();
    assert_eq!(x(&f.doc), 45.0, "beside the bound rect, gap 5");
    f.doc
        .apply(&Transaction(vec![Operation::SetMode {
            id: f.frame,
            mode: Some(f.dark),
        }]))
        .unwrap();
    assert!(!f.doc.shown(f.rect), "Dark's false");
    assert!(
        f.doc.get(f.rect).unwrap().visible(),
        "its literal untouched"
    );
    assert_eq!(x(&f.doc), 0.0, "hidden in Dark, out of the flow");
}

/// **A visibility written over a bound layer detaches it — and only a write of
/// what it shows keeps it** (§15 D1059): one half, where corners and spacing
/// have two. Hidden by Dark, its literal `true`: the eye writing `true` to show
/// it detaches, landing shown.
///
/// Flip, run: the literal half added (`|| *visible == node.visible()`) fails
/// *"showing a hidden bound layer detaches it"* — the write excused as the
/// literal, the layer left bound and hidden.
#[test]
fn a_visibility_written_over_a_bound_layer_detaches_it() {
    let mut f = fixture();
    let show = show_var(&mut f);
    let ops = ondin_core::bind_visible_ops(&f.doc, f.rect, show);
    f.doc.apply(&Transaction(ops)).unwrap();
    let write = |doc: &Document, visible| {
        ondin_core::keep_visible_bindings(
            doc,
            Transaction(vec![Operation::SetVisible {
                id: f.rect,
                visible,
            }]),
        )
    };
    let same = write(&f.doc, true);
    assert!(
        same.changes_nothing(&f.doc),
        "a write of what it shows keeps the binding"
    );
    f.doc
        .apply(&Transaction(vec![Operation::SetMode {
            id: f.frame,
            mode: Some(f.dark),
        }]))
        .unwrap();
    let tx = write(&f.doc, true);
    f.doc.apply(&tx).unwrap();
    assert_eq!(
        f.doc.get(f.rect).unwrap().visible_var(),
        None,
        "showing a hidden bound layer detaches it"
    );
    assert!(f.doc.shown(f.rect), "and it shows");
}

/// Detaching a visibility keeps what it showed; deleting its variable detaches
/// it the same way; the binding and a boolean cell round-trip through a save;
/// and a boolean binding refuses a number variable.
#[test]
fn a_visibility_binding_detaches_to_what_it_shows_and_round_trips() {
    let mut f = fixture();
    let show = show_var(&mut f);
    f.doc
        .apply(&Transaction(vec![Operation::SetMode {
            id: f.frame,
            mode: Some(f.dark),
        }]))
        .unwrap();
    let ops = ondin_core::bind_visible_ops(&f.doc, f.rect, show);
    f.doc.apply(&Transaction(ops)).unwrap();
    let bytes = ondin_core::io::save(&f.doc).unwrap();
    let back = ondin_core::io::load(&bytes).unwrap();
    assert_eq!(back, f.doc, "the binding and the boolean cells round-trip");
    assert!(
        String::from_utf8(bytes)
            .unwrap()
            .contains("\"boolean\": false"),
        "a boolean cell is written as one"
    );
    let before = f.doc.clone();
    f.doc
        .apply(&Transaction(ondin_core::detach_visible_ops(&f.doc, f.rect)))
        .unwrap();
    let node = f.doc.get(f.rect).unwrap();
    assert_eq!(
        (node.visible_var(), node.visible()),
        (None, false),
        "detached hidden"
    );
    f.doc = before;
    let tx = Transaction(ondin_core::variable::remove_ops(&f.doc, &[show]));
    f.doc.apply(&tx).expect("detach, then remove");
    let node = f.doc.get(f.rect).unwrap();
    assert_eq!(
        (node.visible_var(), node.visible()),
        (None, false),
        "deleted, hidden"
    );
    let wrong = f.doc.apply(&Transaction(vec![Operation::SetVisibleVar {
        id: f.rect,
        var: Some(f.radius),
    }]));
    assert!(wrong.is_err(), "a number variable cannot bind visibility");
}

/// Binding a paint keeps its `%`; detaching it leaves the colour it drew.
#[test]
fn binding_a_paint_keeps_its_percent_and_detaching_keeps_its_colour() {
    let mut f = fixture();
    f.doc
        .apply(&Transaction(vec![Operation::SetFills {
            id: f.rect,
            fills: keyed_by_position([Fill {
                brush: RED.with_alpha(0.5).into(),
                visible: true,
            }]),
        }]))
        .unwrap();
    let ids = [f.rect];
    let tx = ondin_core::bind_brush_at_all(&f.doc, &ids, ondin_core::PaintTarget::Fill, 0, f.brand);
    f.doc.apply(&tx).unwrap();
    assert_eq!(
        f.doc.get(f.rect).unwrap().paint().fills[0].brush,
        BrushSource::Bound {
            var: f.brand,
            alpha: 0.5
        }
    );
    let tx = ondin_core::detach_brush_at_all(&f.doc, &ids, ondin_core::PaintTarget::Fill, 0);
    f.doc.apply(&tx).unwrap();
    assert_eq!(
        f.doc.get(f.rect).unwrap().paint().fills[0].brush,
        BrushSource::Literal(Brush::Solid(WHITE.multiply_alpha(0.5)))
    );
}

/// A small deterministic PRNG — `tests/resolve.rs`'s shape.
struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6364136223846793005)
            .wrapping_add(1442695040888963407);
        self.0 >> 33
    }
    fn below(&mut self, n: usize) -> usize {
        (self.next() % n as u64) as usize
    }
}

/// **`update` ≡ `rebuild` over random variable and mode edits** (§5.9, D1037
/// premise (ii)): the operations that name no node — a cell edit, a mode added or
/// removed — and the ones that change which mode a subtree reads — a frame's
/// mode, a reparent across frames — authored at random over rectangles whose
/// corners are bound, with undos between. `tests/resolve.rs`'s three runs author
/// none of these, so they could never see a stale bound corner.
///
/// Flip, run: a cell edit dirtying nothing (`op_set_variable_cell` without its
/// `bound_nodes` extend) fails here with a used-kind mismatch on a bound rect —
/// the live map's resolved radii one cell edit behind the rebuild's.
#[test]
fn incremental_update_equals_rebuild_over_random_variable_ops() {
    for seed in 0..40u64 {
        let mut rng = Lcg(seed.wrapping_mul(0x9E3779B97F4A7C15).wrapping_add(7));
        let mut f = fixture();
        let other = f.ids.mint();
        f.doc
            .apply(&Transaction(vec![Operation::CreateNode {
                id: other,
                parent: f.root,
                index: 1,
                kind: NodeKind::Artboard {
                    size: Size::new(80.0, 80.0),
                },
                transform: None,
                name: None,
            }]))
            .unwrap();
        let frames = [f.frame, other];
        // A flex row, so a bound gap or padding moves its items (§15 D1058).
        let second = flex_frame(&mut f);
        // And a boolean, so a bound visibility takes an item out of the flow
        // (§15 D1059).
        let show = show_var(&mut f);
        let mut rects = vec![f.rect, second];
        let mut hist = ondin_core::History::new();
        let mut res = Resolved::rebuild(&f.doc);
        for step in 0..50 {
            let modes: Vec<ModeId> = f.doc.variables().modes().iter().map(|m| m.id).collect();
            let rect = rects[rng.below(rects.len())];
            let tx = match rng.below(14) {
                0 => {
                    let id = f.ids.mint();
                    rects.push(id);
                    Transaction(vec![Operation::CreateNode {
                        id,
                        parent: frames[rng.below(2)],
                        index: 0,
                        kind: NodeKind::Rect {
                            size: Size::new(10.0, 10.0),
                            corner_radii: RoundedRectRadii::from_single_radius(1.0),
                        },
                        transform: None,
                        name: None,
                    }])
                }
                1 => {
                    let mut vars = [None; 4];
                    vars[rng.below(4)] = Some(f.radius);
                    Transaction(ondin_core::bind_corners_ops(&f.doc, rect, vars))
                }
                2 => Transaction(ondin_core::detach_corners_ops(&f.doc, rect)),
                3 => Transaction(vec![Operation::SetVariableCell {
                    id: f.radius,
                    mode: modes[rng.below(modes.len())],
                    cell: Cell::Number(rng.below(30) as f64),
                }]),
                4 => {
                    let mode = match rng.below(modes.len() + 1) {
                        0 => None,
                        k => Some(modes[k - 1]),
                    };
                    Transaction(vec![Operation::SetMode {
                        id: frames[rng.below(2)],
                        mode,
                    }])
                }
                5 => Transaction(vec![Operation::Reparent {
                    id: rect,
                    new_parent: frames[rng.below(2)],
                    index: 0,
                }]),
                // A mode at the front — which moves the default every unset
                // frame reads in — with a cell for every variable.
                6 => {
                    let id = f.ids.mint_mode();
                    let cells = f
                        .doc
                        .variables()
                        .iter()
                        .map(|v| {
                            let cell = match v.kind {
                                VarKind::Color => Cell::Color(RED),
                                VarKind::Number => Cell::Number(rng.below(20) as f64),
                                VarKind::Boolean => Cell::Boolean(rng.below(2) == 0),
                            };
                            (v.id, cell)
                        })
                        .collect();
                    Transaction(vec![Operation::AddMode {
                        mode: mode(id, &format!("M{step}")),
                        index: 0,
                        cells,
                    }])
                }
                // Bind a gap or a padding of the flex frame, or detach one.
                7 => {
                    let mut vars = [None; 6];
                    vars[rng.below(6)] = Some(f.radius);
                    Transaction(ondin_core::bind_spacing_ops(&f.doc, f.frame, vars))
                }
                8 => {
                    let mut which = [false; 6];
                    which[rng.below(6)] = true;
                    Transaction(ondin_core::detach_spacing_ops(&f.doc, f.frame, which))
                }
                // Bind a rect's visibility, or detach it.
                9 => Transaction(ondin_core::bind_visible_ops(&f.doc, rect, show)),
                10 => Transaction(ondin_core::detach_visible_ops(&f.doc, rect)),
                // Bind a rect's or a frame's width or height, or detach one
                // (§15 D1061) — a bound item's size moves its flex neighbours.
                11 | 12 => {
                    let id = if rng.below(3) == 0 {
                        frames[rng.below(2)]
                    } else {
                        rect
                    };
                    let mut axes = [false; 2];
                    axes[rng.below(2)] = true;
                    if step % 2 == 0 {
                        let vars = axes.map(|a| a.then_some(f.radius));
                        Transaction(ondin_core::bind_size_ops(&f.doc, id, vars))
                    } else {
                        Transaction(ondin_core::detach_size_ops(&f.doc, id, axes))
                    }
                }
                // Remove a mode (never the last), returning its frames to Inherit.
                _ if modes.len() > 1 => {
                    let gone = modes[rng.below(modes.len())];
                    let mut ops: Vec<Operation> = frames
                        .iter()
                        .filter(|fr| f.doc.get(**fr).unwrap().mode() == Some(gone))
                        .map(|fr| Operation::SetMode {
                            id: *fr,
                            mode: None,
                        })
                        .collect();
                    ops.push(Operation::RemoveMode { id: gone });
                    Transaction(ops)
                }
                _ => continue,
            };
            if tx.0.is_empty() {
                continue;
            }
            let dirty = match hist.commit(&mut f.doc, tx) {
                Ok(d) => d,
                Err(_) => continue,
            };
            res.update(&f.doc, &dirty);
            assert_matches_rebuild(&f.doc, &res);
            if rng.below(5) == 0
                && let Some(d) = hist.undo(&mut f.doc).ok().flatten()
            {
                res.update(&f.doc, &d);
                assert_matches_rebuild(&f.doc, &res);
            }
        }
    }
}

/// **A `%` on a bound paint keeps its binding; a colour detaches it**
/// (`build::edited_brush`, D1038).
#[test]
fn an_edit_as_drawn_keeps_a_binding_for_a_percent_and_detaches_for_a_colour() {
    let mut f = fixture();
    bind_fill(&mut f.doc, f.rect, f.brand, 1.0);
    let src = f.doc.get(f.rect).unwrap().paint().fills[0].brush.clone();
    let faded = ondin_core::edited_brush(&f.doc, f.rect, &src, |b| match b {
        Brush::Solid(c) => Brush::Solid(c.with_alpha(0.4)),
        other => other.clone(),
    });
    match faded {
        BrushSource::Bound { var, alpha } => {
            assert_eq!(var, f.brand);
            assert!((alpha - 0.4).abs() < 1e-6, "alpha {alpha}");
        }
        other => panic!("detached: {other:?}"),
    }
    let recoloured = ondin_core::edited_brush(&f.doc, f.rect, &src, |_| Brush::Solid(RED));
    assert_eq!(recoloured, BrushSource::Literal(Brush::Solid(RED)));
}

/// A number variable `size/w`: 60 in Light, 80 in Dark.
fn width_var(f: &mut F) -> VarId {
    let w = f.ids.mint_var();
    f.doc
        .apply(&Transaction(vec![Operation::AddVariable {
            var: var(
                w,
                "size/w",
                VarKind::Number,
                [(f.light, Cell::Number(60.0)), (f.dark, Cell::Number(80.0))],
            ),
        }]))
        .unwrap();
    w
}

/// `id`'s stored size.
fn stored_size(doc: &Document, id: NodeId) -> Size {
    doc.get(id).unwrap().kind().box_size().unwrap()
}

/// **A bound width is drawn and laid out at its variable's number, in the mode
/// in force** (§15 D1061), never at the 0 its literal holds — on the layer
/// itself, on a child pinned against a bound frame, and on an item's flex
/// neighbour; and a live `Resolved` updated by a cell's edit and a mode's
/// switch agrees with a rebuild.
///
/// Flips, run: `DocView::kind` answering the stored kind (`n.kind().clone()`)
/// fails *"the neighbour sits past the bound width"* at 5 — the first rect laid
/// at 0 wide, the neighbour one gap in; `with_bound_size` dropped from
/// `place_node` fails *"the rect is drawn at Light's 60"* at 0; and
/// `Node::reads_variables` leaving the size out — a cell's edit dirtying no
/// bound size — fails *"a cell's edit places it again"* at 70, and the random
/// test below with a frame's used size a cell behind.
#[test]
fn a_bound_size_is_drawn_and_laid_out_at_the_variables_number() {
    let mut f = fixture();
    let w = width_var(&mut f);
    let ops = ondin_core::bind_size_ops(&f.doc, f.rect, [Some(w), None]);
    f.doc.apply(&Transaction(ops)).unwrap();
    assert_eq!(
        stored_size(&f.doc, f.rect),
        Size::new(0.0, 20.0),
        "the literal is 0"
    );
    let width = |doc: &Document, id| Resolved::rebuild(doc).world_bounds(id).unwrap().width();
    assert_eq!(
        width(&f.doc, f.rect),
        60.0,
        "the rect is drawn at Light's 60"
    );

    // A child pinned 10 from the right of a frame whose width is bound.
    let pinned = f.ids.mint();
    f.doc
        .apply(&Transaction(vec![Operation::CreateNode {
            id: pinned,
            parent: f.frame,
            index: 1,
            kind: NodeKind::Ellipse {
                size: Size::new(10.0, 10.0),
            },
            transform: None,
            name: None,
        }]))
        .unwrap();
    f.doc
        .apply(&Transaction(vec![Operation::SetInsets {
            id: pinned,
            insets: ondin_core::Insets {
                right: Some(ondin_core::LengthPct::Px(10.0)),
                ..Default::default()
            },
        }]))
        .unwrap();
    let ops = ondin_core::bind_size_ops(&f.doc, f.frame, [Some(w), None]);
    f.doc.apply(&Transaction(ops)).unwrap();
    let mut live = Resolved::rebuild(&f.doc);
    let right = |live: &Resolved| live.world_bounds(pinned).unwrap().max_x();
    assert_eq!(right(&live), 50.0, "pinned against the frame's bound 60");
    let out = f
        .doc
        .apply(&Transaction(vec![Operation::SetMode {
            id: f.frame,
            mode: Some(f.dark),
        }]))
        .unwrap();
    live.update(&f.doc, &out.dirty);
    assert_eq!(right(&live), 70.0, "Dark's 80");
    assert_matches_rebuild(&f.doc, &live);
    let out = f
        .doc
        .apply(&Transaction(vec![Operation::SetVariableCell {
            id: w,
            mode: f.dark,
            cell: Cell::Number(100.0),
        }]))
        .unwrap();
    live.update(&f.doc, &out.dirty);
    assert_eq!(right(&live), 90.0, "a cell's edit places it again");
    assert_matches_rebuild(&f.doc, &live);

    // A flex row: the second item follows the first's bound width.
    let mut g = fixture();
    let w = width_var(&mut g);
    let second = flex_frame(&mut g);
    let ops = ondin_core::bind_size_ops(&g.doc, g.rect, [Some(w), None]);
    g.doc.apply(&Transaction(ops)).unwrap();
    let x = Resolved::rebuild(&g.doc)
        .world_bounds(second)
        .unwrap()
        .min_x();
    assert_eq!(x, 65.0, "the neighbour sits past the bound width");
}

/// **A size written over a bound axis detaches it, with the corners' two
/// halves** (§15 D1061): an axis written what it draws, or its own 0 literal,
/// stays bound with its literal put back; written anything else it detaches to
/// that size.
///
/// Flip, run: dropping the literal half (`|| w[k] == l[k]`) fails *"a write of
/// the stored size keeps the binding"*.
#[test]
fn a_size_written_over_a_bound_axis_detaches_it() {
    let mut f = fixture();
    let w = width_var(&mut f);
    let ops = ondin_core::bind_size_ops(&f.doc, f.rect, [Some(w), Some(w)]);
    f.doc.apply(&Transaction(ops)).unwrap();
    let write = |doc: &Document, s: Size| {
        ondin_core::keep_size_bindings(
            doc,
            Transaction(vec![Operation::SetGeometry {
                id: f.rect,
                geometry: ondin_core::GeometryPatch::Size(s),
            }]),
        )
    };
    f.doc.apply(&write(&f.doc, Size::new(0.0, 0.0))).unwrap();
    assert_eq!(
        f.doc.get(f.rect).unwrap().size_vars(),
        [Some(w), Some(w)],
        "a write of the stored size keeps the binding"
    );
    // A side handle: the width as drawn, the height dragged to 30.
    f.doc.apply(&write(&f.doc, Size::new(60.0, 30.0))).unwrap();
    assert_eq!(
        f.doc.get(f.rect).unwrap().size_vars(),
        [Some(w), None],
        "the width written as drawn stays bound, the height detaches"
    );
    assert_eq!(stored_size(&f.doc, f.rect), Size::new(0.0, 30.0));
}

/// **Only a hand-set axis binds, and an axis an edit makes computed is
/// detached** (§15 D1061, the maintainer's ruling): in a flex row the rect's
/// width — its main axis — binds; given growth it is the line's, so the commit
/// detaches it to the width it drew; and with growth already set, the bind
/// refuses the width and takes the height, which `align-self: start` keeps from
/// the row's stretch.
///
/// Flip, run: `keep_size_bindings` without its layout half (the `touched`
/// block) fails *"growth detaches the width"* — the width still bound.
#[test]
fn an_axis_made_computed_detaches_and_a_computed_axis_does_not_bind() {
    let mut f = fixture();
    let w = width_var(&mut f);
    flex_frame(&mut f);
    let ops = ondin_core::bind_size_ops(&f.doc, f.rect, [Some(w), None]);
    f.doc.apply(&Transaction(ops)).unwrap();
    assert_eq!(f.doc.get(f.rect).unwrap().size_vars(), [Some(w), None]);
    // Aligned to the start, so its height — stretched under the row's default —
    // is its own.
    let grow = ondin_core::container::LayoutItem {
        grow: 1.0,
        align_self: Some(ondin_core::container::AlignItems::Start),
        ..*f.doc.get(f.rect).unwrap().item()
    };
    let tx = ondin_core::keep_size_bindings(
        &f.doc,
        Transaction(vec![Operation::SetLayoutItem {
            id: f.rect,
            item: grow,
        }]),
    );
    f.doc.apply(&tx).unwrap();
    assert_eq!(
        f.doc.get(f.rect).unwrap().size_vars(),
        [None, None],
        "growth detaches the width"
    );
    assert_eq!(
        stored_size(&f.doc, f.rect).width,
        60.0,
        "to the width it drew"
    );
    assert_eq!(
        ondin_core::bindable_size_axes(&f.doc, f.rect),
        [false, true],
        "a growing item's width is the line's"
    );
    let ops = ondin_core::bind_size_ops(&f.doc, f.rect, [Some(w), Some(w)]);
    f.doc.apply(&Transaction(ops)).unwrap();
    assert_eq!(f.doc.get(f.rect).unwrap().size_vars(), [None, Some(w)]);
}

/// Detaching an axis keeps the size it drew; deleting the variable detaches
/// every axis bound to it the same way; the bindings survive a save; and a
/// path refuses one at the commit.
#[test]
fn size_bindings_detach_to_what_they_draw_round_trip_and_fit_their_kind() {
    let mut f = fixture();
    let w = width_var(&mut f);
    let ops = ondin_core::bind_size_ops(&f.doc, f.rect, [Some(w), Some(w)]);
    f.doc.apply(&Transaction(ops)).unwrap();
    let bytes = ondin_core::io::save(&f.doc).unwrap();
    let back = ondin_core::io::load(&bytes).unwrap();
    assert_eq!(back, f.doc, "the bindings round-trip");
    let ops = ondin_core::detach_size_ops(&f.doc, f.rect, [true, false]);
    f.doc.apply(&Transaction(ops)).unwrap();
    assert_eq!(f.doc.get(f.rect).unwrap().size_vars(), [None, Some(w)]);
    assert_eq!(stored_size(&f.doc, f.rect), Size::new(60.0, 0.0));
    let tx = Transaction(ondin_core::variable::remove_ops(&f.doc, &[w]));
    f.doc.apply(&tx).expect("detach, then remove");
    assert_eq!(f.doc.get(f.rect).unwrap().size_vars(), [None, None]);
    assert_eq!(
        stored_size(&f.doc, f.rect),
        Size::new(60.0, 60.0),
        "the height too"
    );

    let w = width_var(&mut f);
    let line = f.ids.mint();
    f.doc
        .apply(&Transaction(vec![Operation::CreateNode {
            id: line,
            parent: f.frame,
            index: 0,
            kind: NodeKind::Line {
                end: ondin_core::kurbo::Point::new(10.0, 0.0),
            },
            transform: None,
            name: None,
        }]))
        .unwrap();
    assert!(
        ondin_core::bind_size_ops(&f.doc, line, [Some(w), None]).is_empty(),
        "no stored size, no binding"
    );
    assert!(
        f.doc
            .apply(&Transaction(vec![Operation::SetSizeVars {
                id: line,
                vars: [Some(w), None],
            }]))
            .is_err(),
        "the commit refuses one"
    );
}
