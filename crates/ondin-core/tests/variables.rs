//! Variables and modes (§5.3f, §15 D1039): the table's operations and rules, a
//! binding drawn in the mode in force, the save format, `Resolved`'s bound
//! corners, the paste rule and an instance's binding as an override.

use ondin_core::Brush;
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
    assert!(text.contains("\"schema_version\": 7"), "written as v7");
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
        let mut rects = vec![f.rect];
        let mut hist = ondin_core::History::new();
        let mut res = Resolved::rebuild(&f.doc);
        for step in 0..50 {
            let modes: Vec<ModeId> = f.doc.variables().modes().iter().map(|m| m.id).collect();
            let rect = rects[rng.below(rects.len())];
            let tx = match rng.below(8) {
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
