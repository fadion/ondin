//! Kits carrying variables and styles (§5.3e, §5.3f, §15 D1064).
//!
//! Kit L binds its mains to variables of its own and styles them; a consumer D
//! vendoring a main takes what it reads into its own tables, under ids of its
//! own and with an origin, and holds the kit's modes beside its own — read
//! through **one per-kit mapping** (D1038 (b)): the entry set by hand, else the
//! kit mode of the same name, else the kit's first. These tests hold the
//! publish, the insert, the mapping, the update, the carrying paste, the file and
//! the vault's rule.

use ondin_core::kit::{self, KitError, Published};
use ondin_core::kurbo::{RoundedRectRadii, Size};
use ondin_core::meta::{DocumentMeta, KitMark};
use ondin_core::peniko::Color;
use ondin_core::style::{Style, StyleId, StyleValue};
use ondin_core::variable::{KitModes, VarRule};
use ondin_core::{
    Brush, BrushSource, Cell, Document, EFFECT_BLUR, Effect, EffectKind, Fill, IdSource, Mode,
    ModeId, NodeId, NodeKind, OpError, Operation, Placement, Shadow, Stroke, Transaction, VarId,
    VarKind, Variable, bind_effect_ops, bind_opacity_ops, bind_stroke_width_ops, io,
    keyed_by_position,
};

const L_ID: &str = "5eed0000000000000000000000000011";

const WHITE: Color = Color::from_rgba8(255, 255, 255, 255);
const NAVY: Color = Color::from_rgba8(10, 20, 80, 255);
const RED: Color = Color::from_rgba8(200, 30, 30, 255);
const GOLD: Color = Color::from_rgba8(220, 180, 40, 255);

fn frame(w: f64, h: f64) -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(w, h),
    }
}

fn rect() -> NodeKind {
    NodeKind::Rect {
        size: Size::new(10.0, 10.0),
        corner_radii: RoundedRectRadii::default(),
    }
}

fn create(id: NodeId, parent: NodeId, index: usize, kind: NodeKind, name: &str) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index,
        kind,
        transform: None,
        name: Some(name.into()),
    }
}

fn bound(var: VarId) -> Vec<ondin_core::item::Keyed<Fill>> {
    keyed_by_position([Fill {
        brush: BrushSource::Bound { var, alpha: 1.0 },
        visible: true,
    }])
}

/// Kit L: modes *Day* and *Night*; `brand` white by day and navy by night, and
/// `spare`, which nothing reads; a fill style *Accent* (gold); a main *Icon* —
/// its own fill styled *Accent*, its *Dot* bound to `brand` — and a main
/// *Badge*, a rectangle bound to `brand`.
struct Kit {
    doc: Document,
    ids: IdSource,
    icon: NodeId,
    dot: NodeId,
    badge: NodeId,
    day: ModeId,
    brand: VarId,
    spare: VarId,
    accent: StyleId,
}

fn kit() -> Kit {
    let mut ids = IdSource::new(0xAE);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let [icon, dot, badge] = [(); 3].map(|_| ids.mint());
    let (day, night) = (ids.mint_mode(), ids.mint_mode());
    let (brand, spare) = (ids.mint_var(), ids.mint_var());
    let accent = ids.mint_style();
    let gold = keyed_by_position([Fill {
        brush: GOLD.into(),
        visible: true,
    }]);
    doc.apply(&Transaction(vec![
        Operation::AddMode {
            mode: Mode {
                id: day,
                name: "Day".into(),
            },
            index: 0,
            cells: vec![],
        },
        Operation::AddMode {
            mode: Mode {
                id: night,
                name: "Night".into(),
            },
            index: 1,
            cells: vec![],
        },
        Operation::AddVariable {
            var: Variable {
                id: brand,
                name: "brand".into(),
                kind: VarKind::Color,
                cells: [(day, Cell::Color(WHITE)), (night, Cell::Color(NAVY))].into(),
                origin: None,
                forked: None,
            },
        },
        Operation::AddVariable {
            var: Variable {
                id: spare,
                name: "spare".into(),
                kind: VarKind::Number,
                cells: [(day, Cell::Number(1.0)), (night, Cell::Number(2.0))].into(),
                origin: None,
                forked: None,
            },
        },
        Operation::AddStyle {
            style: Style {
                id: accent,
                name: "Accent".into(),
                value: StyleValue::Fills(gold.clone()),
                origin: None,
                forked: None,
            },
        },
        create(icon, root, 0, frame(24.0, 24.0), "Icon"),
        create(dot, icon, 0, rect(), "Dot"),
        Operation::SetFills {
            id: icon,
            fills: gold,
        },
        Operation::SetFillStyle {
            id: icon,
            style: Some(accent),
        },
        Operation::SetFills {
            id: dot,
            fills: bound(brand),
        },
        Operation::SetComponent {
            id: icon,
            component: true,
        },
        create(badge, root, 1, frame(40.0, 16.0), "Badge"),
        Operation::SetFills {
            id: badge,
            fills: bound(brand),
        },
        Operation::SetComponent {
            id: badge,
            component: true,
        },
    ]))
    .expect("the kit");
    doc.set_meta(DocumentMeta {
        id: Some(L_ID.into()),
        name: Some("Atlas".into()),
        kit: Some(KitMark {
            version: 0,
            ..KitMark::default()
        }),
        ..DocumentMeta::default()
    });
    assert_eq!(kit::publish(&mut doc, None, 1), Some(1));
    Kit {
        doc,
        ids,
        icon,
        dot,
        badge,
        day,
        brand,
        spare,
        accent,
    }
}

fn published(k: &Kit) -> &Published {
    k.doc.published().unwrap()
}

fn version(k: &Kit) -> u32 {
    k.doc.meta().kit.as_ref().unwrap().version
}

/// Consumer D, with `modes` of its own (none for an empty list).
fn consumer(modes: &[&str]) -> (Document, IdSource, Vec<ModeId>) {
    let mut ids = IdSource::new(0xDB);
    let mut doc = Document::new(ids.mint());
    let made: Vec<ModeId> = modes.iter().map(|_| ids.mint_mode()).collect();
    let ops: Vec<Operation> = modes
        .iter()
        .zip(&made)
        .enumerate()
        .map(|(i, (name, id))| Operation::AddMode {
            mode: Mode {
                id: *id,
                name: (*name).into(),
            },
            index: i,
            cells: vec![],
        })
        .collect();
    if !ops.is_empty() {
        doc.apply(&Transaction(ops)).unwrap();
    }
    (doc, ids, made)
}

fn insert_under(
    d: &mut Document,
    ids: &mut IdSource,
    k: &Kit,
    main: NodeId,
    parent: NodeId,
) -> NodeId {
    let (tx, made) = kit::insert(d, ids, L_ID, version(k), published(k), main, parent).unwrap();
    d.apply(&tx).expect("one kit insert");
    made
}

fn insert(d: &mut Document, ids: &mut IdSource, k: &Kit, main: NodeId) -> NodeId {
    let root = d.root();
    insert_under(d, ids, k, main, root)
}

/// What `id`'s first fill draws.
fn drawn(doc: &Document, id: NodeId) -> Brush {
    let b = &doc.get(id).unwrap().paint().fills[0].brush;
    doc.resolve_brush(id, b).into_owned()
}

fn first_child(doc: &Document, id: NodeId) -> NodeId {
    doc.get(id).unwrap().children()[0]
}

/// Accept `k`'s current publish, through the commit's passes.
fn accept(d: &mut Document, ids: &mut IdSource, k: &Kit) -> kit::Update {
    let u = kit::update(d, ids, L_ID, version(k), published(k)).unwrap();
    let mut tx = u.tx.clone();
    ondin_core::propagate::owed(d, &mut tx, ids);
    d.apply(&tx).expect("an accepted update commits");
    u
}

/// **A publish carries what its mains read** and nothing else: `brand`, the
/// kit's two modes and *Accent* travel, `spare` stays home, and the frozen mains
/// keep their bindings — until step 4 a publish detached every one
/// (`variable::detached`, §15 D1039).
///
/// Flip, run: `kit::freeze` writing empty tables in place of `frozen_tables`'
/// fails here at *"brand travels"* — `freeze`'s own debug assertion holds only
/// the component and vault rules — and every insert test beside it at the
/// commit, the vault copy bound to a variable nobody holds (`BadBinding`).
#[test]
fn a_publish_carries_what_its_mains_read() {
    let k = kit();
    let p = published(&k).document();
    assert!(p.variables().get(k.brand).is_some(), "brand travels");
    assert!(p.variables().get(k.spare).is_none(), "spare stays home");
    assert_eq!(p.variables().modes().len(), 2);
    assert!(p.styles().get(k.accent).is_some(), "the style travels");
    assert_eq!(
        p.get(k.dot).unwrap().paint().fills[0].brush,
        BrushSource::Bound {
            var: k.brand,
            alpha: 1.0
        },
        "the binding is kept"
    );
    assert_eq!(p.get(k.icon).unwrap().styles().fills, Some(k.accent));
    // And the frozen copy is a document the loader takes.
    let bytes = io::save(&k.doc).unwrap();
    assert_eq!(io::load(&bytes).unwrap(), k.doc);
}

/// **An insert vendors the kit's variables and styles with the main** (§15
/// D1064): D holds one variable and one style of L's, minted its own ids and
/// carrying their origins, and L's modes beside its own — and the instance's dot
/// reads `brand` through the vault copy's binding. D has no modes of its own,
/// so the kit's first, *Day*, is read: white. One undo takes it all back.
#[test]
fn an_insert_vendors_the_kits_variables_and_the_instance_reads_them() {
    let k = kit();
    let (mut d, mut ids, _) = consumer(&[]);
    let before = d.clone();
    let root = d.root();
    let (tx, inst) = kit::insert(&d, &mut ids, L_ID, 1, published(&k), k.icon, root).unwrap();
    let undo = d.apply(&tx).unwrap().inverse;

    let t = d.variables();
    let [v] = t.iter().collect::<Vec<_>>()[..] else {
        panic!("one variable vendored, `spare` not among them")
    };
    assert_ne!(v.id, k.brand, "minted an id of D's");
    let o = v.origin.as_ref().expect("an origin");
    assert_eq!((o.kit.as_str(), o.var, o.version), (L_ID, k.brand, 1));
    assert_eq!(t.local().count(), 0, "none of D's own");
    let modes: Vec<&str> = t.kits()[L_ID]
        .modes
        .iter()
        .map(|m| m.name.as_str())
        .collect();
    assert_eq!(modes, ["Day", "Night"]);
    assert_eq!(t.kits()[L_ID].name.as_deref(), Some("Atlas"));

    let dot = first_child(&d, inst);
    assert_eq!(
        d.get(dot).unwrap().paint().fills[0].brush.var(),
        Some(v.id),
        "the instance's dot reads D's copy"
    );
    assert_eq!(drawn(&d, dot), Brush::Solid(WHITE), "the kit's first mode");

    let [s] = d.styles().iter().collect::<Vec<_>>()[..] else {
        panic!("one style vendored")
    };
    assert_eq!(s.kit(), Some(L_ID));
    assert_eq!(d.get(inst).unwrap().styles().fills, Some(s.id));
    assert_eq!(drawn(&d, inst), Brush::Solid(GOLD));

    d.apply(&undo).unwrap();
    assert_eq!(d, before, "one undo");
}

/// **The mapping** (D1038 (b)): D's *Light* has no kit mode of its name and reads
/// the kit's first, *Day*; D's *Night* reads the kit's *Night* by name; and an
/// entry set by hand — *Light → Night* — wins over both.
///
/// Flip, run: `Variables::kit_mode`'s name match taken out fails *"by name"*,
/// the instance in the Night frame white.
#[test]
fn the_mapping_reads_by_name_then_the_first_and_a_hand_set_entry_wins() {
    let k = kit();
    let (mut d, mut ids, modes) = consumer(&["Light", "Night"]);
    let (light, night) = (modes[0], modes[1]);
    let root = d.root();
    let dark = ids.mint();
    d.apply(&Transaction(vec![
        create(dark, root, 0, frame(100.0, 100.0), "Dark"),
        Operation::SetMode {
            id: dark,
            mode: Some(night),
        },
    ]))
    .unwrap();
    let plain = insert(&mut d, &mut ids, &k, k.badge);
    let in_dark = insert_under(&mut d, &mut ids, &k, k.badge, dark);
    assert_eq!(drawn(&d, plain), Brush::Solid(WHITE), "no Light: the first");
    assert_eq!(drawn(&d, in_dark), Brush::Solid(NAVY), "by name");

    let mut record: KitModes = d.variables().kits()[L_ID].clone();
    let kit_night = record.modes[1].id;
    record.map.insert(light, kit_night);
    d.apply(&Transaction(vec![Operation::SetKitModes {
        kit: L_ID.into(),
        modes: Some(record),
    }]))
    .unwrap();
    assert_eq!(drawn(&d, plain), Brush::Solid(NAVY), "the hand-set entry");
    assert_eq!(
        d.mode_at(plain),
        Some(light),
        "a kit's mode is no mode of D's"
    );

    // And deleting the mode an entry names takes the entry with it.
    let ops = ondin_core::variable::remove_mode_ops(&d, light);
    d.apply(&Transaction(ops))
        .expect("the entry goes with its mode");
    assert!(d.variables().kits()[L_ID].map.is_empty());
}

/// A second insert of a main reading what D already holds reuses it — one
/// variable, one style, one kit record — as a held main is reused.
#[test]
fn a_second_insert_reuses_the_held_variables() {
    let k = kit();
    let (mut d, mut ids, _) = consumer(&["Light"]);
    insert(&mut d, &mut ids, &k, k.icon);
    insert(&mut d, &mut ids, &k, k.badge);
    insert(&mut d, &mut ids, &k, k.icon);
    assert_eq!(d.variables().iter().count(), 1);
    assert_eq!(d.styles().iter().count(), 1);
    assert_eq!(d.variables().kits().len(), 1);
}

/// **An update rewrites what D holds of the kit** (5A): L adds a mode and
/// recolours `brand`'s *Day*; the instance follows. Then L stops binding its
/// mains to `brand`, which no longer travels — and D's own layer bound to it
/// keeps the colour it drew, detached (*keep its values*), and the variable goes.
///
/// 🚨 **The plain instance follows its main rather than detaching**, and the
/// first build did not: the removal detached every user, the instance among
/// them, and the commit read that write as an override, so the instance kept red
/// where its main turned white. Flip, run: `tokens::plan`'s removal without its
/// follow filter fails at *"the instance follows its main"*, red.
#[test]
fn an_update_rewrites_the_kits_variables_and_removes_what_it_dropped() {
    let mut k = kit();
    let (mut d, mut ids, _) = consumer(&[]);
    let inst = insert(&mut d, &mut ids, &k, k.badge);
    let held = d.variables().iter().next().unwrap().id;
    // D's own layer, bound to the kit's variable.
    let mine = ids.mint();
    let root = d.root();
    d.apply(&Transaction(vec![
        create(mine, root, 0, rect(), "Mine"),
        Operation::SetFills {
            id: mine,
            fills: bound(held),
        },
    ]))
    .unwrap();

    let dusk = k.ids.mint_mode();
    k.doc
        .apply(&Transaction(vec![
            Operation::AddMode {
                mode: Mode {
                    id: dusk,
                    name: "Dusk".into(),
                },
                index: 2,
                cells: vec![(k.brand, Cell::Color(NAVY)), (k.spare, Cell::Number(3.0))],
            },
            Operation::SetVariableCell {
                id: k.brand,
                mode: k.day,
                cell: Cell::Color(RED),
            },
        ]))
        .unwrap();
    assert!(kit::changes(&k.doc).tokens, "worth a publish");
    assert_eq!(kit::publish(&mut k.doc, None, 2), Some(2));
    let u = accept(&mut d, &mut ids, &k);
    assert_eq!(u.vars_changed, vec![held]);
    assert_eq!(drawn(&d, inst), Brush::Solid(RED), "the instance follows");
    assert_eq!(drawn(&d, mine), Brush::Solid(RED), "and D's own layer");
    assert_eq!(d.variables().kits()[L_ID].modes.len(), 3);
    assert_eq!(
        d.variables()
            .get(held)
            .unwrap()
            .origin
            .as_ref()
            .unwrap()
            .version,
        2
    );

    // Now L draws its mains in literals: `brand` no longer travels.
    k.doc
        .apply(&Transaction(vec![
            Operation::SetFills {
                id: k.dot,
                fills: keyed_by_position([Fill {
                    brush: WHITE.into(),
                    visible: true,
                }]),
            },
            Operation::SetFills {
                id: k.badge,
                fills: keyed_by_position([Fill {
                    brush: WHITE.into(),
                    visible: true,
                }]),
            },
        ]))
        .unwrap();
    assert_eq!(kit::publish(&mut k.doc, None, 3), Some(3));
    let u = accept(&mut d, &mut ids, &k);
    assert_eq!(u.vars_removed, vec![held]);
    assert!(d.variables().get(held).is_none(), "the variable goes");
    assert_eq!(
        d.get(mine).unwrap().paint().fills[0].brush,
        BrushSource::from(RED),
        "D's own layer keeps what it drew"
    );
    assert_eq!(
        drawn(&d, inst),
        Brush::Solid(WHITE),
        "the instance follows its main"
    );
}

/// **A main from a newer publish whose kit gained a mode is refused until the
/// update** (`KitError::Modes`): D's copy of `brand` has no cell in it.
#[test]
fn an_insert_over_older_kit_modes_asks_for_the_update() {
    let mut k = kit();
    let (mut d, mut ids, _) = consumer(&[]);
    insert(&mut d, &mut ids, &k, k.icon);
    let dusk = k.ids.mint_mode();
    k.doc
        .apply(&Transaction(vec![Operation::AddMode {
            mode: Mode {
                id: dusk,
                name: "Dusk".into(),
            },
            index: 2,
            cells: vec![(k.brand, Cell::Color(RED)), (k.spare, Cell::Number(3.0))],
        }]))
        .unwrap();
    kit::publish(&mut k.doc, None, 2);
    let root = d.root();
    let r = kit::insert(&d, &mut ids, L_ID, 2, published(&k), k.badge, root);
    assert!(
        matches!(r, Err(KitError::Modes(ref kit)) if kit == L_ID),
        "{r:?}"
    );
    accept(&mut d, &mut ids, &k);
    insert(&mut d, &mut ids, &k, k.badge);
}

/// **A carrying paste vendors the kits' variables with the mains** (§15 D1064):
/// an instance copied out of D lands in E — which has never used the kit — an
/// instance still, its dot bound to E's own copy of `brand`, not detached.
///
/// Flip, run: `variable::land` without its `holds_var` fallback fails *"bound to
/// E's copy"*, the pasted dot a literal white.
#[test]
fn a_carrying_paste_vendors_the_kits_variables_too() {
    let k = kit();
    let (mut d, mut ids, _) = consumer(&[]);
    let inst = insert(&mut d, &mut ids, &k, k.icon);

    let subtrees = vec![d.capture_subtree(inst).unwrap()];
    let kits = kit::carried(&d, &subtrees);
    let vars = ondin_core::variable::carried(&d, &subtrees);
    assert!(
        !vars.tokens.is_empty(),
        "the copy carries the kit's variable"
    );
    let text = io::clip::write_carrying(&subtrees, &kits, &[], &vars, None, "Icon").unwrap();
    let payload = io::clip::read(&text).unwrap().unwrap();

    let mut e = Document::new(ids.mint());
    let root = e.root();
    let placements: Vec<Placement> = payload
        .subtrees
        .into_iter()
        .map(|nodes| Placement {
            nodes: ondin_core::variable::land(&e, &payload.vars, nodes),
            parent: root,
            index: None,
        })
        .collect();
    let (mut tx, made) = kit::insert_carrying(
        &e,
        &mut ids,
        &payload.kits,
        &payload.vars.tokens,
        &placements,
        Default::default(),
    )
    .unwrap();
    ondin_core::propagate::owed(&e, &mut tx, &mut ids);
    e.apply(&tx).expect("a carrying paste");

    let pasted = made[0];
    assert!(e.get(pasted).unwrap().link().is_some(), "still an instance");
    let [v] = e.variables().iter().collect::<Vec<_>>()[..] else {
        panic!("one variable vendored")
    };
    assert_eq!(v.origin.as_ref().unwrap().var, k.brand);
    let dot = first_child(&e, pasted);
    assert_eq!(
        e.get(dot).unwrap().paint().fills[0].brush.var(),
        Some(v.id),
        "bound to E's copy"
    );
    assert_eq!(drawn(&e, dot), Brush::Solid(WHITE));
    assert_eq!(e.styles().iter().count(), 1, "the style came too");
}

/// A consumer holding a kit's variables, modes, mapping and styles saves and
/// loads byte for byte.
#[test]
fn a_consumer_holding_kit_variables_round_trips_through_a_file() {
    let k = kit();
    let (mut d, mut ids, modes) = consumer(&["Light"]);
    insert(&mut d, &mut ids, &k, k.icon);
    let mut record = d.variables().kits()[L_ID].clone();
    record.map.insert(modes[0], record.modes[1].id);
    d.apply(&Transaction(vec![Operation::SetKitModes {
        kit: L_ID.into(),
        modes: Some(record),
    }]))
    .unwrap();
    let bytes = io::save(&d).unwrap();
    let back = io::load(&bytes).unwrap();
    assert_eq!(back, d);
    assert_eq!(io::save(&back).unwrap(), bytes);
}

/// **A vault node reads only a kit's variables** (`variable::check`'s
/// `InVault`): binding a vendored main's dot to one of D's own is refused, as a
/// link out of the vault is (§15 D1036 (1)).
#[test]
fn a_vault_node_bound_to_the_documents_own_variable_is_refused() {
    let k = kit();
    let (mut d, mut ids, modes) = consumer(&["Light"]);
    let inst = insert(&mut d, &mut ids, &k, k.icon);
    let main = d.get(inst).unwrap().link().unwrap();
    let vault_dot = first_child(&d, main);
    let own = ids.mint_var();
    let r = d.apply(&Transaction(vec![
        Operation::AddVariable {
            var: Variable {
                id: own,
                name: "own".into(),
                kind: VarKind::Color,
                cells: [(modes[0], Cell::Color(RED))].into(),
                origin: None,
                forked: None,
            },
        },
        Operation::SetFills {
            id: vault_dot,
            fills: bound(own),
        },
    ]));
    assert!(
        matches!(r, Err(OpError::Variables(VarRule::InVault(n))) if n == vault_dot),
        "{r:?}"
    );
}

/// D's own variable aliasing the kit's reads the kit's column for D's mode —
/// the mapping applied at every step of the walk.
#[test]
fn an_alias_of_the_documents_own_to_a_kits_reads_through_the_mapping() {
    let k = kit();
    let (mut d, mut ids, modes) = consumer(&["Night"]);
    insert(&mut d, &mut ids, &k, k.badge);
    let held = d.variables().iter().next().unwrap().id;
    let mine = ids.mint_var();
    d.apply(&Transaction(vec![Operation::AddVariable {
        var: Variable {
            id: mine,
            name: "surface".into(),
            kind: VarKind::Color,
            cells: [(modes[0], Cell::Alias(held))].into(),
            origin: None,
            forked: None,
        },
    }]))
    .unwrap();
    assert_eq!(
        d.variables().resolve(mine, modes[0]),
        Some(ondin_core::Value::Color(NAVY)),
        "Night by name"
    );
    // And a kit's variable may not alias one of D's.
    let mut v = d.variables().get(held).unwrap().clone();
    let col = *v.cells.keys().next().unwrap();
    v.cells.insert(col, Cell::Alias(mine));
    let r = d.apply(&Transaction(vec![Operation::SetVariable { var: v }]));
    assert!(
        matches!(r, Err(OpError::Variables(VarRule::BadAlias(_)))),
        "{r:?}"
    );
}

/// Copy `inst` out of `from` through the clipboard's text and paste it into
/// `to`'s root, through `land` and the carrying insert and the commit's passes.
fn paste(from: &Document, inst: NodeId, to: &mut Document, ids: &mut IdSource) -> NodeId {
    let subtrees = vec![from.capture_subtree(inst).unwrap()];
    let kits = kit::carried(from, &subtrees);
    let vars = ondin_core::variable::carried(from, &subtrees);
    let text = io::clip::write_carrying(&subtrees, &kits, &[], &vars, None, "x").unwrap();
    let payload = io::clip::read(&text).unwrap().unwrap();
    let root = to.root();
    let placements: Vec<Placement> = payload
        .subtrees
        .into_iter()
        .map(|nodes| Placement {
            nodes: ondin_core::variable::land(to, &payload.vars, nodes),
            parent: root,
            index: None,
        })
        .collect();
    let (mut tx, made) = kit::insert_carrying(
        to,
        ids,
        &payload.kits,
        &payload.vars.tokens,
        &placements,
        Default::default(),
    )
    .unwrap();
    ondin_core::propagate::owed(to, &mut tx, ids);
    to.apply(&tx).expect("a carrying paste");
    made[0]
}

/// L gains a third mode, *Dusk* — `brand` red in it — and publishes.
fn add_dusk(k: &mut Kit) -> ModeId {
    let dusk = k.ids.mint_mode();
    k.doc
        .apply(&Transaction(vec![Operation::AddMode {
            mode: Mode {
                id: dusk,
                name: "Dusk".into(),
            },
            index: 2,
            cells: vec![(k.brand, Cell::Color(RED)), (k.spare, Cell::Number(3.0))],
        }]))
        .unwrap();
    kit::publish(&mut k.doc, None, 9);
    dusk
}

const A_ID: &str = "5eed0000000000000000000000000012";

/// Kit A, built on L: a main *Card* nesting an instance of L's *Badge*, which A
/// vendored at L's current publish. Answers A and Card.
fn kit_a(k: &Kit) -> (Document, IdSource, NodeId) {
    let mut ids = IdSource::new(0xAF);
    let mut a = Document::new(ids.mint());
    let card = ids.mint();
    let root = a.root();
    a.apply(&Transaction(vec![create(
        card,
        root,
        0,
        frame(80.0, 40.0),
        "Card",
    )]))
    .unwrap();
    insert_under(&mut a, &mut ids, k, k.badge, card);
    a.apply(&Transaction(vec![Operation::SetComponent {
        id: card,
        component: true,
    }]))
    .unwrap();
    a.set_meta(DocumentMeta {
        id: Some(A_ID.into()),
        name: Some("Cards".into()),
        kit: Some(KitMark {
            version: 0,
            ..KitMark::default()
        }),
        ..DocumentMeta::default()
    });
    kit::publish(&mut a, None, 1);
    (a, ids, card)
}

/// 🚨 **A kit instance pasted back into its own kit reads the kit's own
/// variables** (D1023's own-kit arm made for the tables, `arch-scribe`'s reading
/// of the first build): L holds no copy of its own `brand`, and the pasted Icon's
/// dot reads the original — before the fix L gained a kit record of itself and
/// a second `brand`, and every bound field of the paste was an override.
///
/// Flip, run: `tokens::plan`'s `own` taken as `None` fails at *"no copy of its
/// own variables"*, three variables where two were.
#[test]
fn a_kit_instance_pasted_back_into_its_kit_reads_its_own_variables() {
    let mut k = kit();
    let (mut d, mut ids, _) = consumer(&[]);
    let inst = insert(&mut d, &mut ids, &k, k.icon);
    let pasted = paste(&d, inst, &mut k.doc, &mut k.ids);
    assert_eq!(
        k.doc.variables().iter().count(),
        2,
        "no copy of its own variables"
    );
    assert!(
        k.doc.variables().kits().is_empty(),
        "no kit record of itself"
    );
    assert_eq!(k.doc.styles().iter().count(), 1, "no copy of its own style");
    let dot = first_child(&k.doc, pasted);
    assert_eq!(
        k.doc.get(dot).unwrap().paint().fills[0].brush.var(),
        Some(k.brand)
    );
    assert_eq!(
        k.doc.get(pasted).unwrap().link(),
        Some(k.icon),
        "the live main"
    );
}

/// And a kit instance pasted back into a kit that has **deleted** the variable
/// it reads detaches to what it drew, by D1038's rule (`arch-scribe`'s reading of
/// the own-kit fix: kept for a plan with nothing to point it at, the commit
/// refused the paste as a binding to nothing).
///
/// Flip, run: `land` keeping an own-kit binding as any carried one (`own_var`
/// answering `None`) fails in `paste`, the commit refusing a `BadBinding`.
#[test]
fn a_paste_back_into_a_kit_that_deleted_the_variable_detaches() {
    let mut k = kit();
    let (mut d, mut ids, _) = consumer(&[]);
    let inst = insert(&mut d, &mut ids, &k, k.icon);
    let ops = ondin_core::variable::remove_ops(&k.doc, &[k.brand]);
    k.doc.apply(&Transaction(ops)).unwrap();
    let pasted = paste(&d, inst, &mut k.doc, &mut k.ids);
    let dot = first_child(&k.doc, pasted);
    assert_eq!(
        k.doc.get(dot).unwrap().paint().fills[0].brush,
        BrushSource::from(WHITE),
        "detached to what it drew"
    );
}

/// **A consumer holding a third kit newer than the kit carrying it** — L with
/// *Dusk*, where A vendored L before it — inserts A's Card all the same: the
/// held record covers what A read (`arch-scribe`'s reading: refused for good as
/// an update owed, which no update could give).
///
/// Flip, run: `covers` made the old equal-sets test fails at *"inserts"*,
/// `Err(Modes(L))`.
#[test]
fn a_third_kit_held_newer_than_the_kit_carrying_it_is_reused() {
    let mut k = kit();
    let (a, _, card) = kit_a(&k);
    add_dusk(&mut k);
    let (mut d, mut ids, _) = consumer(&[]);
    insert(&mut d, &mut ids, &k, k.badge);
    let root = d.root();
    let pa = a.published().unwrap();
    let r = kit::insert(&d, &mut ids, A_ID, 1, pa, card, root);
    let (tx, inst) = r.expect("inserts");
    d.apply(&tx).expect("commits");
    assert_eq!(d.variables().iter().count(), 1, "L's brand, held once");
    assert_eq!(
        d.variables().kits()[L_ID].modes.len(),
        3,
        "the newer record kept"
    );
    let badge = first_child(&d, inst);
    assert_eq!(drawn(&d, badge), Brush::Solid(WHITE));
}

/// **Accept all takes a kit carrying a third kit's new mode beside that kit's
/// own update, in either order** (`arch-scribe`'s reading: `[A, L]` refused
/// where `[L, A]` took).
///
/// Flip, run: `update_all` without its retry fails at *"A first, L second"*,
/// `Err(Modes(L))`.
#[test]
fn accept_all_takes_a_kit_carrying_a_newer_third_kit_in_either_order() {
    let mut k = kit();
    let (mut a, mut aids, card) = kit_a(&k);
    let (mut d, mut ids, _) = consumer(&[]);
    let root = d.root();
    let (tx, _) = kit::insert(&d, &mut ids, A_ID, 1, a.published().unwrap(), card, root).unwrap();
    d.apply(&tx).unwrap();
    add_dusk(&mut k);
    // A takes L's new publish and publishes again.
    let u = kit::update(&a, &mut aids, L_ID, version(&k), published(&k)).unwrap();
    let mut tx = u.tx;
    ondin_core::propagate::owed(&a, &mut tx, &mut aids);
    a.apply(&tx).unwrap();
    kit::publish(&mut a, None, 2);

    let av = a.meta().kit.as_ref().unwrap().version;
    let u = kit::update_all(
        &d,
        &mut ids,
        &[
            (A_ID, av, a.published().unwrap()),
            (L_ID, version(&k), published(&k)),
        ],
    )
    .expect("A first, L second");
    let mut tx = u.tx;
    ondin_core::propagate::owed(&d, &mut tx, &mut ids);
    d.apply(&tx).expect("commits");
    assert_eq!(d.variables().kits()[L_ID].modes.len(), 3);
}

/// **The stale-mode reset spares a copy that set the mode as its main did**
/// (`arch-scribe`'s reading): L's Icon set to *Night*, an instance of it in D
/// with it; then L moves Icon to *Dusk* and removes *Night*. The instance
/// follows to *Dusk* — the reset's write had made it an override, left on
/// *Inherit*.
///
/// Flip, run: the reset without its `follows` filter fails at *"follows"*,
/// `None`.
#[test]
fn a_copy_on_a_removed_kit_mode_follows_its_main_to_the_new_one() {
    let mut k = kit();
    let night = k.doc.variables().modes()[1].id;
    k.doc
        .apply(&Transaction(vec![Operation::SetMode {
            id: k.icon,
            mode: Some(night),
        }]))
        .unwrap();
    let dusk = add_dusk(&mut k);
    let (mut d, mut ids, _) = consumer(&[]);
    let inst = insert(&mut d, &mut ids, &k, k.icon);
    let kit_mode = |d: &Document, name: &str| {
        d.variables().kits()[L_ID]
            .modes
            .iter()
            .find(|m| m.name == name)
            .map(|m| m.id)
    };
    assert_eq!(d.get(inst).unwrap().mode(), kit_mode(&d, "Night"));
    // The mode's removal returns Icon to *Inherit*, and Icon is then set to
    // *Dusk* — in that order, or the removal's write lands last.
    let mut ops = ondin_core::variable::remove_mode_ops(&k.doc, night);
    ops.push(Operation::SetMode {
        id: k.icon,
        mode: Some(dusk),
    });
    k.doc.apply(&Transaction(ops)).unwrap();
    kit::publish(&mut k.doc, None, 3);
    accept(&mut d, &mut ids, &k);
    assert_eq!(d.get(inst).unwrap().mode(), kit_mode(&d, "Dusk"), "follows");
}

/// In a document with no modes of its own, a style of its own reading a kit's
/// variable the kit removes is detached to what it drew — before, `remove_ops`
/// detached styles only under a default mode, and the update was refused.
///
/// Flip, run: the styles' detach gated on a default mode again fails in
/// `accept`, the commit refusing the style (`BadVariable`).
#[test]
fn a_documents_own_style_reading_a_removed_kit_variable_is_detached() {
    let mut k = kit();
    let (mut d, mut ids, _) = consumer(&[]);
    insert(&mut d, &mut ids, &k, k.badge);
    let held = d.variables().iter().next().unwrap().id;
    let mine = ids.mint_style();
    d.apply(&Transaction(vec![Operation::AddStyle {
        style: Style {
            id: mine,
            name: "Mine".into(),
            value: StyleValue::Fills(bound(held)),
            origin: None,
            forked: None,
        },
    }]))
    .unwrap();
    let plain = keyed_by_position([Fill {
        brush: WHITE.into(),
        visible: true,
    }]);
    k.doc
        .apply(&Transaction(vec![
            Operation::SetFills {
                id: k.dot,
                fills: plain.clone(),
            },
            Operation::SetFills {
                id: k.badge,
                fills: plain,
            },
        ]))
        .unwrap();
    kit::publish(&mut k.doc, None, 2);
    accept(&mut d, &mut ids, &k);
    let StyleValue::Fills(f) = &d.styles().get(mine).unwrap().value else {
        panic!("a fill style")
    };
    assert_eq!(
        f[0].brush,
        BrushSource::from(WHITE),
        "the kit's default, Day"
    );
}

/// Binding a main root's visibility on the kit's page is no change a consumer
/// receives — an instance root keeps its own visibility, bound or not.
///
/// Flip, run: `reach` leaving a top's `visible_var` fails here, Icon counted
/// changed. And nor is the boolean a change of the tables (D1064's amendment):
/// `freeze` detaches a top's own visibility to what it shows, so the variable
/// does not travel and a cell edit of it enables no publish. Flip, run: the
/// `top_visibility_frozen` call in `freeze`'s published-tops loop removed fails
/// at *"no table change"*, `tokens` true — the edge D1064 left.
#[test]
fn a_main_roots_visibility_binding_is_not_a_change() {
    let mut k = kit();
    let shown = k.ids.mint_var();
    let modes: Vec<ModeId> = k.doc.variables().modes().iter().map(|m| m.id).collect();
    k.doc
        .apply(&Transaction(vec![
            Operation::AddVariable {
                var: Variable {
                    id: shown,
                    name: "shown".into(),
                    kind: VarKind::Boolean,
                    cells: modes.iter().map(|m| (*m, Cell::Boolean(true))).collect(),
                    origin: None,
                    forked: None,
                },
            },
            Operation::SetVisibleVar {
                id: k.icon,
                var: Some(shown),
            },
        ]))
        .unwrap();
    let c = kit::changes(&k.doc);
    assert!(!c.changed.contains(&k.icon), "{c:?}");
    assert!(!c.tokens, "no table change: {c:?}");
    // Hidden by its variable, the frozen main is hidden by its literal.
    k.doc
        .apply(&Transaction(vec![Operation::SetVariableCell {
            id: shown,
            mode: modes[0],
            cell: Cell::Boolean(false),
        }]))
        .unwrap();
    let c = kit::changes(&k.doc);
    assert!(!c.any(), "a cell no consumer reads: {c:?}");
    let frozen = kit::freeze(&k.doc).unwrap();
    let icon = frozen.document().get(k.icon).unwrap();
    assert_eq!((icon.visible_var(), icon.visible()), (None, false));
    assert!(frozen.document().variables().get(shown).is_none());
}

/// *Make local* `inst`'s main through the commit's passes, so its instances
/// follow — and what it forked.
fn make_local(d: &mut Document, ids: &mut IdSource, inst: NodeId) -> (NodeId, kit::MadeLocal) {
    let main = d.get(inst).unwrap().link().unwrap();
    let mut made = kit::make_local_report(d, ids, &[main]);
    let mut tx = std::mem::take(&mut made.tx);
    ondin_core::propagate::owed(d, &mut tx, ids);
    // The undo in `tx`'s place: what a test hands back is what takes it back.
    made.tx = d.apply(&tx).expect("make local commits").inverse;
    (main, made)
}

/// **A main made local forks the kit's variables and styles it reads** (§15
/// D1066, the maintainer's ruling): D gains its own *Atlas/brand* and
/// *Atlas/Accent*, marked with the kit's identity they were forked from, every
/// mode of D given the cell the per-kit mapping read there — *Light*, with no
/// kit mode of its name, the kit's first (*Day*, white); *Night* the kit's
/// *Night* — and the revealed main and, by the follow, its instance bound to
/// them. Nothing drawn changes, and one undo takes it all back.
///
/// Flip, run: `make_local_report` without its fork (its `fork.ops` and
/// rebinds dropped) fails at *"one variable of D's own"*, 0.
#[test]
fn make_local_forks_the_kits_variables_and_styles() {
    let k = kit();
    let (mut d, mut ids, modes) = consumer(&["Light", "Night"]);
    let inst = insert(&mut d, &mut ids, &k, k.icon);
    let before = d.clone();
    let (main, made) = make_local(&mut d, &mut ids, inst);

    let t = d.variables();
    let [fork] = t.local().collect::<Vec<_>>()[..] else {
        panic!("one variable of D's own, got {}", t.local().count())
    };
    assert_eq!(fork.name, "Atlas/brand");
    assert_eq!(
        fork.forked.as_ref().map(|o| (o.kit.as_str(), o.var)),
        Some((L_ID, k.brand))
    );
    assert_eq!(
        fork.cells
            .iter()
            .map(|(m, c)| (*m, c.clone()))
            .collect::<Vec<_>>(),
        [
            (modes[0], Cell::Color(WHITE)),
            (modes[1], Cell::Color(NAVY))
        ]
    );
    let [style] = d
        .styles()
        .iter()
        .filter(|s| s.origin.is_none())
        .collect::<Vec<_>>()[..]
    else {
        panic!("one style of D's own")
    };
    assert_eq!(style.name, "Atlas/Accent");
    assert_eq!(made.forked, (1, 1));

    let main_dot = first_child(&d, main);
    let dot = first_child(&d, inst);
    for (n, what) in [(main_dot, "the main's dot"), (dot, "the instance's dot")] {
        assert_eq!(
            d.get(n).unwrap().paint().fills[0].brush.var(),
            Some(fork.id),
            "{what} reads the fork"
        );
    }
    assert_eq!(d.get(main).unwrap().styles().fills, Some(style.id));
    assert_eq!(d.get(inst).unwrap().styles().fills, Some(style.id));
    assert_eq!(drawn(&d, dot), Brush::Solid(WHITE), "nothing drawn changes");
    assert_eq!(drawn(&d, inst), Brush::Solid(GOLD));

    d.apply(&made.tx).unwrap();
    assert_eq!(d, before, "one undo");
}

/// A document with **no modes of its own** takes the forked kit's, by name — it
/// can hold no variable of its own otherwise (`NoModes`) — and with no hand-set
/// entry each maps to the kit mode of its name, so the kit's own variables
/// read as before too.
#[test]
fn make_local_in_a_document_without_modes_takes_the_kits() {
    let k = kit();
    let (mut d, mut ids, _) = consumer(&[]);
    let inst = insert(&mut d, &mut ids, &k, k.icon);
    make_local(&mut d, &mut ids, inst);
    let t = d.variables();
    let names: Vec<&str> = t.modes().iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["Day", "Night"]);
    let fork = t.local().next().expect("a fork");
    let cells: Vec<Cell> = t
        .modes()
        .iter()
        .map(|m| fork.cells[&m.id].clone())
        .collect();
    assert_eq!(cells, [Cell::Color(WHITE), Cell::Color(NAVY)]);
    assert_eq!(drawn(&d, first_child(&d, inst)), Brush::Solid(WHITE));
}

/// **Taking a kit's modes leaves every other kit reading what it read**: D has
/// no modes and holds kit B, whose modes run *Night, Day* — so B's *Chip*
/// reads B's first, *Night*, navy. Making L's *Icon* local gives D L's *Day,
/// Night*; by name, B would now read its *Day*. The default is pinned to B's
/// first instead.
///
/// Flip, run: the pinning loop in `tokens::fork` dropped fails at *"B still
/// reads its first mode"*, white.
#[test]
fn adopting_a_kits_modes_leaves_another_kit_reading_its_first() {
    const B_ID: &str = "5eed0000000000000000000000000022";
    let mut bi = IdSource::new(0xB4);
    let broot = bi.mint();
    let mut b = Document::new(broot);
    let (night, day) = (bi.mint_mode(), bi.mint_mode());
    let ink = bi.mint_var();
    let chip = bi.mint();
    let mode = |id, name: &str, index| Operation::AddMode {
        mode: Mode {
            id,
            name: name.into(),
        },
        index,
        cells: vec![],
    };
    b.apply(&Transaction(vec![
        mode(night, "Night", 0),
        mode(day, "Day", 1),
        Operation::AddVariable {
            var: Variable {
                id: ink,
                name: "ink".into(),
                kind: VarKind::Color,
                cells: [(night, Cell::Color(NAVY)), (day, Cell::Color(WHITE))].into(),
                origin: None,
                forked: None,
            },
        },
        create(chip, broot, 0, frame(10.0, 10.0), "Chip"),
        Operation::SetFills {
            id: chip,
            fills: bound(ink),
        },
        Operation::SetComponent {
            id: chip,
            component: true,
        },
    ]))
    .unwrap();
    b.set_meta(DocumentMeta {
        id: Some(B_ID.into()),
        name: Some("Basalt".into()),
        kit: Some(KitMark::default()),
        ..DocumentMeta::default()
    });
    kit::publish(&mut b, None, 1);

    let k = kit();
    let (mut d, mut ids, _) = consumer(&[]);
    let root = d.root();
    let (tx, b_inst) =
        kit::insert(&d, &mut ids, B_ID, 1, b.published().unwrap(), chip, root).unwrap();
    d.apply(&tx).unwrap();
    let inst = insert(&mut d, &mut ids, &k, k.icon);
    assert_eq!(drawn(&d, b_inst), Brush::Solid(NAVY), "fixture: B's first");
    make_local(&mut d, &mut ids, inst);
    assert_eq!(d.variables().modes().len(), 2, "fixture: D took L's modes");
    assert_eq!(
        drawn(&d, b_inst),
        Brush::Solid(NAVY),
        "B still reads its first mode"
    );
}

/// **A kit instance nested in the revealed main keeps the kit's bindings**: its
/// *Dot* reads `brand` because its vault main's does, and the commit's follow
/// carries that main — still the kit's — so a fork written over it would read
/// as an override of the kit's *Icon*. *Badge*'s own fill is forked.
///
/// Flip, run: `tokens::own` answering its binding whatever the source holds
/// fails at *"the nested Dot keeps the kit's brand"*.
#[test]
fn make_local_leaves_a_nested_kit_instance_the_kits() {
    let mut k = kit();
    let (tx, _) = ondin_core::insert_subtrees(
        &k.doc,
        &mut k.ids,
        &[Placement {
            nodes: k.doc.capture_subtree(k.icon).unwrap(),
            parent: k.badge,
            index: None,
        }],
        Default::default(),
    );
    k.doc.apply(&tx).unwrap();
    kit::publish(&mut k.doc, None, 2);
    let (mut d, mut ids, _) = consumer(&["Day"]);
    let badge = insert(&mut d, &mut ids, &k, k.badge);
    let held = d.variables().by_origin((L_ID, k.brand)).unwrap().id;
    let (main, _) = make_local(&mut d, &mut ids, badge);
    let fork = d.variables().local().next().expect("a fork").id;
    assert_eq!(
        d.get(main).unwrap().paint().fills[0].brush.var(),
        Some(fork),
        "Badge's own fill is forked"
    );
    let nested = first_child(&d, main);
    let nested_dot = first_child(&d, nested);
    assert!(d.get(nested).unwrap().link().is_some_and(|l| d.in_vault(l)));
    assert_eq!(
        d.get(nested_dot).unwrap().paint().fills[0].brush.var(),
        Some(held),
        "the nested Dot keeps the kit's brand"
    );
}

/// **A drifted copy's fills are matched to its source's by item id** (D980):
/// the nested *Dot* in *Badge* has a fill of its own put **in front** of the
/// one it follows, both bound to `brand`. Made local, the own one is forked and
/// the followed one keeps the kit's.
///
/// Flip, run: `own_reads` and `rebind_ops` matching a fill to the source's at
/// the same index again (in place of `src_fill`) fails at *"the Dot's own fill
/// is forked"* — position 0 met the source's `brand` and was taken as
/// followed.
#[test]
fn make_local_matches_a_drifted_copys_fills_by_item() {
    let mut k = kit();
    let (tx, _) = ondin_core::insert_subtrees(
        &k.doc,
        &mut k.ids,
        &[Placement {
            nodes: k.doc.capture_subtree(k.icon).unwrap(),
            parent: k.badge,
            index: None,
        }],
        Default::default(),
    );
    k.doc.apply(&tx).unwrap();
    let nested_dot = first_child(&k.doc, first_child(&k.doc, k.badge));
    let mut fills = k.doc.get(nested_dot).unwrap().paint().fills.clone();
    fills.insert(
        0,
        ondin_core::Keyed::new(
            k.ids.mint_item(),
            Fill {
                brush: BrushSource::Bound {
                    var: k.brand,
                    alpha: 0.5,
                },
                visible: true,
            },
        ),
    );
    k.doc
        .apply(&Transaction(vec![Operation::SetFills {
            id: nested_dot,
            fills,
        }]))
        .unwrap();
    kit::publish(&mut k.doc, None, 2);
    let (mut d, mut ids, _) = consumer(&["Day"]);
    let badge = insert(&mut d, &mut ids, &k, k.badge);
    let held = d.variables().by_origin((L_ID, k.brand)).unwrap().id;
    let (main, _) = make_local(&mut d, &mut ids, badge);
    let fork = d.variables().local().next().expect("a fork").id;
    let dot = first_child(&d, first_child(&d, main));
    let vars: Vec<_> = d
        .get(dot)
        .unwrap()
        .paint()
        .fills
        .iter()
        .map(|f| f.brush.var())
        .collect();
    assert_eq!(vars[0], Some(fork), "the Dot's own fill is forked");
    assert_eq!(vars[1], Some(held), "the followed fill keeps the kit's");
}

/// **A main the kit removes is made local with its variables forked**, even
/// one only it read — which the new publish therefore no longer carries, so
/// the update removes it: the fork is taken before the kit's tables are
/// rewritten, or the main kept only a literal (`arch-scribe`'s reading 3 of
/// D1066's first cut).
///
/// Flip, run: `update_in_order`'s up-front fork of the removed tops dropped
/// fails at *"tag forked"* — nothing was: by `make_local`'s turn the main held
/// the literal the removal detached it to. And `reads_as_its_source`'s `alike`
/// without its fork arm fails at *"the instance follows onto the fork"*,
/// `None`: the instance was cut to a literal.
///
/// *Badge* is styled too, by *Tagged*, which reads `tag` and goes with it: the
/// style is forked and the instance stays styled by the fork (reading (e)).
/// Flip, run: the style filter's fork arm in `tokens::plan` dropped fails at
/// *"the instance follows onto the style's fork"*, `None`. And the variables'
/// removal no longer skipping a `SetStyleValue` on a style the same update
/// removes fails in `accept` (`Incomplete`) — every update dropping a style
/// and a variable it read had failed so, fork or none.
#[test]
fn a_main_the_kit_removes_forks_a_variable_only_it_read() {
    let mut k = kit();
    let tag = k.ids.mint_var();
    let tagged = k.ids.mint_style();
    let modes: Vec<ModeId> = k.doc.variables().modes().iter().map(|m| m.id).collect();
    k.doc
        .apply(&Transaction(vec![
            Operation::AddVariable {
                var: Variable {
                    id: tag,
                    name: "tag".into(),
                    kind: VarKind::Color,
                    cells: [(modes[0], Cell::Color(RED)), (modes[1], Cell::Color(GOLD))].into(),
                    origin: None,
                    forked: None,
                },
            },
            Operation::AddStyle {
                style: Style {
                    id: tagged,
                    name: "Tagged".into(),
                    value: StyleValue::Fills(bound(tag)),
                    origin: None,
                    forked: None,
                },
            },
            Operation::SetFills {
                id: k.badge,
                fills: bound(tag),
            },
            Operation::SetFillStyle {
                id: k.badge,
                style: Some(tagged),
            },
        ]))
        .unwrap();
    kit::publish(&mut k.doc, None, 2);
    let (mut d, mut ids, dm) = consumer(&["Day", "Night"]);
    let inst = insert(&mut d, &mut ids, &k, k.badge);
    let held = d.get(inst).unwrap().link().unwrap();
    k.doc
        .apply(&Transaction(vec![Operation::DeleteNode { id: k.badge }]))
        .unwrap();
    kit::publish(&mut k.doc, None, 3);
    let u = accept(&mut d, &mut ids, &k);
    assert_eq!(u.made_local, [held]);
    let fork = d
        .variables()
        .local()
        .find(|v| v.name == "Atlas/tag")
        .expect("tag forked");
    assert_eq!(
        d.get(held).unwrap().paint().fills[0].brush.var(),
        Some(fork.id),
        "Badge reads a fork, not a literal"
    );
    assert_eq!(fork.cells[&dm[1]], Cell::Color(GOLD), "every mode kept");
    // And the page's instance follows its main onto the fork — never cut to a
    // literal the commit reads as an override (`arch-scribe`'s reading (a)).
    assert_eq!(
        d.get(inst).unwrap().paint().fills[0].brush.var(),
        Some(fork.id),
        "the instance follows onto the fork"
    );
    // And its style the same way (reading (e)): forked, and the instance still
    // styled by it — never unbound from the style its main reads.
    let style = d
        .styles()
        .iter()
        .find(|s| s.name == "Atlas/Tagged")
        .expect("the style forked")
        .id;
    assert_eq!(d.get(held).unwrap().styles().fills, Some(style));
    assert_eq!(
        d.get(inst).unwrap().styles().fills,
        Some(style),
        "the instance follows onto the style's fork"
    );
    assert!(
        !ondin_core::reset::drift(&d, inst).any(),
        "no override on the instance"
    );
    assert!(
        d.variables().by_origin((L_ID, tag)).is_none(),
        "the kit's is gone"
    );
}

/// **A removal detaches a drifted copy's own fill, matched by item id** (D980,
/// `arch-scribe`'s reading (b) of D1066's fixes): *Badge* has two fills bound
/// to `tag`, and the page's copy has replaced the first with one of its own,
/// bound alike — so by position every bound fill lines up with its main's.
/// When the kit drops `tag`, the update commits and the own fill is detached.
/// ⚠️ The followed fill is detached with it — an override where its main went
/// white: the detach is one `SetFills` for the node, and the follow takes no
/// item of a list the transaction wrote (`reads_as_its_source`'s doc).
///
/// Flip, run: `reads_as_its_source` matching fills by position again fails in
/// `accept` — the node was spared whole, its own fill left bound to a variable
/// the update removes (`BadBinding`).
#[test]
fn a_removal_matches_a_drifted_copys_fills_by_item() {
    let mut k = kit();
    let tag = k.ids.mint_var();
    let modes: Vec<ModeId> = k.doc.variables().modes().iter().map(|m| m.id).collect();
    k.doc
        .apply(&Transaction(vec![
            Operation::AddVariable {
                var: Variable {
                    id: tag,
                    name: "tag".into(),
                    kind: VarKind::Color,
                    cells: modes.iter().map(|m| (*m, Cell::Color(RED))).collect(),
                    origin: None,
                    forked: None,
                },
            },
            Operation::SetFills {
                id: k.badge,
                fills: keyed_by_position([0.5, 1.0].map(|alpha| Fill {
                    brush: BrushSource::Bound { var: tag, alpha },
                    visible: true,
                })),
            },
        ]))
        .unwrap();
    kit::publish(&mut k.doc, None, 2);
    let (mut d, mut ids, _) = consumer(&["Day"]);
    let inst = insert(&mut d, &mut ids, &k, k.badge);
    let held = d.variables().by_origin((L_ID, tag)).unwrap().id;
    // The copy's first fill replaced by one of its own, as its main's — so
    // every bound fill lines up with its main's by position, not by item.
    let mut fills = d.get(inst).unwrap().paint().fills.clone();
    fills[0] = ondin_core::Keyed::new(
        ids.mint_item(),
        Fill {
            brush: BrushSource::Bound {
                var: held,
                alpha: 0.5,
            },
            visible: true,
        },
    );
    d.apply(&Transaction(vec![Operation::SetFills { id: inst, fills }]))
        .unwrap();
    k.doc
        .apply(&Transaction(vec![Operation::SetFills {
            id: k.badge,
            fills: keyed_by_position([Fill {
                brush: WHITE.into(),
                visible: true,
            }]),
        }]))
        .unwrap();
    kit::publish(&mut k.doc, None, 3);
    accept(&mut d, &mut ids, &k);
    let f = &d.get(inst).unwrap().paint().fills;
    assert!(f[0].brush.var().is_none(), "the own fill detached");
    assert!(
        f[1].brush.var().is_none(),
        "the known edge: the followed fill detached with it"
    );
}

/// **Only this update's fork counts as alike** (`arch-scribe`'s reading (d) of
/// D1066's fixes): *Badge* was made local before, so it reads *Atlas/brand*,
/// and a page instance of it has been bound back to the kit's `brand` by hand.
/// When the kit drops `brand`, that copy is detached, values kept — its main is
/// outside the vault and no edit of this update's, so nothing would follow it.
///
/// Flip, run: `reads_as_its_source`'s fork arm counted on any source (its
/// `vault` ignored) fails in `accept` — the copy spared and left bound to the
/// removed `brand` (`BadBinding`).
#[test]
fn an_earlier_fork_does_not_spare_a_copy_from_a_removal() {
    let mut k = kit();
    let (mut d, mut ids, _) = consumer(&["Day"]);
    insert(&mut d, &mut ids, &k, k.icon);
    let inst = insert(&mut d, &mut ids, &k, k.badge);
    let (main, _) = make_local(&mut d, &mut ids, inst);
    let held = d.variables().by_origin((L_ID, k.brand)).unwrap().id;
    d.apply(&Transaction(vec![Operation::SetFills {
        id: inst,
        fills: d
            .get(inst)
            .unwrap()
            .paint()
            .fills
            .iter()
            .map(|f| {
                f.map(|f| Fill {
                    brush: BrushSource::Bound {
                        var: held,
                        alpha: 1.0,
                    },
                    ..f.clone()
                })
            })
            .collect(),
    }]))
    .unwrap();
    let plain = keyed_by_position([Fill {
        brush: WHITE.into(),
        visible: true,
    }]);
    k.doc
        .apply(&Transaction(vec![
            Operation::SetFills {
                id: k.dot,
                fills: plain.clone(),
            },
            Operation::SetFills {
                id: k.badge,
                fills: plain,
            },
            Operation::RemoveVariable { id: k.brand },
        ]))
        .unwrap();
    kit::publish(&mut k.doc, None, 2);
    accept(&mut d, &mut ids, &k);
    assert!(d.variables().by_origin((L_ID, k.brand)).is_none());
    assert!(
        d.get(inst).unwrap().paint().fills[0].brush.var().is_none(),
        "the copy detached, values kept"
    );
    assert!(
        d.get(main).unwrap().paint().fills[0].brush.var().is_some(),
        "its main keeps its fork"
    );
}

/// **A second *Make local* of the same kit binds to the first one's fork**
/// rather than forking `brand` twice — the mark the maintainer ruled for.
///
/// Flip, run: `fork`'s `forked_var` answering `None` fails at *"still one
/// fork"*, 2 — *Atlas/brand 2* beside it.
#[test]
fn a_second_make_local_reuses_the_fork() {
    let k = kit();
    let (mut d, mut ids, _) = consumer(&["Day"]);
    let icon = insert(&mut d, &mut ids, &k, k.icon);
    let badge = insert(&mut d, &mut ids, &k, k.badge);
    make_local(&mut d, &mut ids, icon);
    let (badge_main, made) = make_local(&mut d, &mut ids, badge);
    let forks: Vec<_> = d.variables().local().collect();
    assert_eq!(forks.len(), 1, "still one fork");
    assert_eq!(made.forked, (0, 0));
    assert_eq!(
        d.get(badge_main).unwrap().paint().fills[0].brush.var(),
        Some(forks[0].id)
    );
}

/// **A kit's stroke style holding a bound width lands reading the consumer's
/// copy of the variable** (§15 D1073, D1064): the badge given a stroke style
/// *Edge* whose width is bound to the kit's `spare`, published and inserted; the
/// vendored style's stroke, and the instance's, read D's `spare`, not L's id.
///
/// Flip, run: `Plan::remap_value` leaving `width_var` as it found it — D1073's
/// committed cut — fails *"the insert commits"* with `Styles(BadVariable)`: the
/// vendored style names L's id, which D does not hold, so `style::check` refuses
/// the whole insert. Predicted at *"the style's width reads D's copy"*; the
/// defect was worse than a wrong binding — such a kit could not be inserted.
#[test]
fn a_kits_stroke_style_with_a_bound_width_lands_on_the_consumers_copy() {
    let mut k = kit();
    let (edge, item) = (k.ids.mint_style(), k.ids.mint_item());
    let strokes = vec![ondin_core::Keyed::new(
        item,
        ondin_core::Stroke {
            width: 0.0,
            width_var: Some(k.spare),
            ..Default::default()
        },
    )];
    k.doc
        .apply(&Transaction(vec![
            Operation::AddStyle {
                style: Style {
                    id: edge,
                    name: "Edge".into(),
                    value: StyleValue::Strokes(strokes.clone()),
                    origin: None,
                    forked: None,
                },
            },
            Operation::SetStrokes {
                id: k.badge,
                strokes,
            },
            Operation::SetStrokeStyle {
                id: k.badge,
                style: Some(edge),
            },
        ]))
        .expect("the badge stroked by Edge");
    assert_eq!(kit::publish(&mut k.doc, None, 2), Some(2));
    let (mut d, mut ids, _) = consumer(&[]);
    let root = d.root();
    let (tx, inst) = kit::insert(&d, &mut ids, L_ID, 2, published(&k), k.badge, root).unwrap();
    d.apply(&tx).expect("the insert commits");
    let spare = d
        .variables()
        .iter()
        .find(|v| v.name == "spare")
        .map(|v| v.id)
        .expect("spare vendored");
    assert_ne!(spare, k.spare, "under an id of D's");
    let style = d
        .styles()
        .iter()
        .find(|s| s.name == "Edge")
        .expect("Edge vendored");
    let StyleValue::Strokes(v) = &style.value else {
        unreachable!("a stroke style")
    };
    assert_eq!(
        v[0].width_var,
        Some(spare),
        "the style's width reads D's copy"
    );
    assert_eq!(
        d.get(inst).unwrap().paint().strokes[0].width_var,
        Some(spare),
        "and so does the instance's"
    );
}

/// Kit L with *Badge* given a stroke and a drop shadow, and two number
/// variables of the kit's own, `a` and `b` — `values[0]` and `values[1]` in
/// both modes — then `bind` reading `a` on *Badge*, published as version 2.
fn kit_with_numbers(
    bind: &dyn Fn(&Document, NodeId, VarId) -> Vec<Operation>,
    values: [f64; 2],
) -> (Kit, VarId, VarId) {
    let mut k = kit();
    let (a, b) = (k.ids.mint_var(), k.ids.mint_var());
    let modes: Vec<ModeId> = k.doc.variables().modes().iter().map(|m| m.id).collect();
    let number = |id, name: &str, x| Operation::AddVariable {
        var: Variable {
            id,
            name: name.into(),
            kind: VarKind::Number,
            cells: modes.iter().map(|m| (*m, Cell::Number(x))).collect(),
            origin: None,
            forked: None,
        },
    };
    k.doc
        .apply(&Transaction(vec![
            number(a, "a", values[0]),
            number(b, "b", values[1]),
            Operation::SetStrokes {
                id: k.badge,
                strokes: keyed_by_position([Stroke {
                    width: 1.0,
                    ..Default::default()
                }]),
            },
            Operation::SetEffects {
                id: k.badge,
                effects: keyed_by_position([Effect::new(
                    EffectKind::DropShadow(Shadow::default()),
                )]),
            },
        ]))
        .expect("Badge stroked and shadowed");
    let ops = bind(&k.doc, k.badge, a);
    assert!(!ops.is_empty(), "fixture: a binding to make");
    k.doc.apply(&Transaction(ops)).expect("Badge bound to a");
    assert_eq!(kit::publish(&mut k.doc, None, 2), Some(2));
    (k, a, b)
}

/// One number binding through a kit's three paths (§15 D1064, D1066): `bind`
/// makes it, `read` answers the variable a node's field is bound to, and
/// `drawn` the number that field draws, in the variable's unit.
///
/// 1. **The insert**: *Badge*'s field bound to `a` lands, on the vault main and
///    its instance, bound to D's copy of `a` — never the kit's id — and draws
///    `a`'s value.
/// 2. **The update**: the kit rebinds the field to `b`, so `a` no longer
///    travels; the vault main is rebound to D's copy of `b`, `a` is removed,
///    and the instance **follows** its main — no override.
/// 3. ***Make local***: the main's binding is its own, so `b` is forked as
///    *Atlas/b* and the main and, by the follow, its instance read the fork;
///    nothing drawn changes.
fn carry_a_number(
    bind: &dyn Fn(&Document, NodeId, VarId) -> Vec<Operation>,
    read: &dyn Fn(&Document, NodeId) -> Option<VarId>,
    drawn: &dyn Fn(&Document, NodeId) -> f64,
    values: [f64; 2],
) {
    let (mut k, a, b) = kit_with_numbers(bind, values);
    let (mut d, mut ids, _) = consumer(&[]);
    let inst = insert(&mut d, &mut ids, &k, k.badge);
    let main = d.get(inst).unwrap().link().unwrap();
    let held_a = d.variables().by_origin((L_ID, a)).expect("a vendored").id;
    assert_ne!(held_a, a, "under an id of D's");
    assert_eq!(read(&d, main), Some(held_a), "the vault main reads D's a");
    assert_eq!(read(&d, inst), Some(held_a), "the instance reads D's a");
    assert_eq!(drawn(&d, inst), values[0], "a's value is drawn");

    let ops = bind(&k.doc, k.badge, b);
    k.doc.apply(&Transaction(ops)).expect("Badge rebound to b");
    assert_eq!(kit::publish(&mut k.doc, None, 3), Some(3));
    accept(&mut d, &mut ids, &k);
    assert!(
        d.variables().by_origin((L_ID, a)).is_none(),
        "a no longer travels and is removed"
    );
    let held_b = d.variables().by_origin((L_ID, b)).expect("b vendored").id;
    assert_eq!(read(&d, main), Some(held_b), "the update rebinds the main");
    assert_eq!(
        read(&d, inst),
        Some(held_b),
        "the instance follows its main"
    );
    assert_eq!(drawn(&d, inst), values[1], "b's value is drawn");
    assert!(
        !ondin_core::reset::drift(&d, inst).any(),
        "no override on the instance"
    );

    let (made_main, made) = make_local(&mut d, &mut ids, inst);
    assert_eq!(made_main, main);
    let fork = d
        .variables()
        .local()
        .find(|v| v.name == "Atlas/b")
        .expect("b forked");
    assert_eq!(
        fork.forked.as_ref().map(|o| (o.kit.as_str(), o.var)),
        Some((L_ID, b))
    );
    // `b`, and `brand`, which *Badge*'s fill reads.
    assert_eq!(made.forked, (2, 0));
    assert_eq!(read(&d, main), Some(fork.id), "the main reads the fork");
    assert_eq!(
        read(&d, inst),
        Some(fork.id),
        "the instance follows onto the fork"
    );
    assert_eq!(drawn(&d, inst), values[1], "nothing drawn changes");
}

/// **A layer's opacity bound to a kit's number variable** (§15 D1072) through
/// the insert, an update and *Make local* (`carry_a_number`): 50 and then 25,
/// read as a percentage.
///
/// Flips, run: `Plan::apply`'s `opacity_var` arm deleted fails at *"one kit
/// insert"* — the vault copy names L's `a`, which D does not hold, and the
/// commit refuses it (`BadBinding`). `reads_as_its_source`'s opacity arm
/// answering `false` for a bound opacity (never alike) fails at *"the instance
/// follows its main"* — the removal detached the instance to a literal.
/// `Fork::rebind_ops`' opacity arm deleted fails at *"the main reads the fork"*:
/// `b` was forked (`own_reads` found it) and the main left on the kit's.
#[test]
fn a_kits_opacity_binding_is_carried_to_the_consumers_own_variable() {
    carry_a_number(
        &|d, id, v| bind_opacity_ops(d, id, v),
        &|d, id| d.get(id).unwrap().opacity_var(),
        &|d, id| f64::from(d.drawn_opacity(id)) * 100.0,
        [50.0, 25.0],
    );
}

/// **A stroke's width bound to a kit's number variable** (§15 D1073) through
/// the insert, an update and *Make local* (`carry_a_number`) — the width
/// bound on the layer itself, beside
/// `a_kits_stroke_style_with_a_bound_width_lands_on_the_consumers_copy`'s style.
///
/// Flips, run: `Plan::apply`'s `width_var` arm deleted fails at *"one kit
/// insert"* (`BadBinding`). `reads_as_its_source`'s width arm answering
/// `false` for a bound width fails at *"the instance follows its main"*.
/// `Fork::rebind_ops`' width arm deleted fails at *"the main reads the fork"*.
#[test]
fn a_kits_stroke_width_binding_is_carried_to_the_consumers_own_variable() {
    carry_a_number(
        &|d, id, v| {
            let item = d.get(id).unwrap().paint().strokes[0].id;
            bind_stroke_width_ops(d, id, item, v)
        },
        &|d, id| d.get(id).unwrap().paint().strokes[0].width_var,
        &|d, id| {
            let s = &d.get(id).unwrap().paint().strokes[0];
            s.width_var
                .map_or(s.width, |v| d.stroke_width_drawn_in(d.variables(), id, v))
        },
        [3.0, 5.0],
    );
}

/// **An effect's blur bound to a kit's number variable** (§15 D1074) through
/// the insert, an update and *Make local* (`carry_a_number`).
///
/// Flips, run: `Plan::apply`'s effects arm deleted fails at *"one kit insert"*
/// (`BadBinding`). `reads_as_its_source`'s effects arm comparing against no
/// source binding (`none` for every item) fails at *"the instance follows its
/// main"*. `Fork::rebind_ops`' effects arm never pushing its `SetEffects` fails
/// at *"the main reads the fork"*. Each of the three runs flipped the same arm
/// of all three bindings at once — opacity, width and effects — which these
/// three tests tell apart, each binding only one of them.
#[test]
fn a_kits_effect_binding_is_carried_to_the_consumers_own_variable() {
    carry_a_number(
        &|d, id, v| {
            let item = d.get(id).unwrap().effects()[0].id;
            bind_effect_ops(d, id, item, EFFECT_BLUR, v)
        },
        &|d, id| d.get(id).unwrap().effects()[0].vars[EFFECT_BLUR],
        &|d, id| {
            let n = d.get(id).unwrap();
            d.resolved_effects(id, n.effects())[0].numbers()[EFFECT_BLUR].unwrap()
        },
        [6.0, 9.0],
    );
}

/// **A kit's effects style holding a bound number lands reading the consumer's
/// copy of the variable** (§15 D1074, D1064) — `Plan::remap_value`'s effects
/// arm, which D1074's amendment recorded as read and not run: *Badge* given an
/// effects style *Glow* whose shadow's blur is bound to the kit's `spare`,
/// published and inserted; the vendored style's effect, and the instance's,
/// read D's `spare`, and the blur draws `spare`'s *Day*, 1.
///
/// Flip, run: `remap_value`'s effects arm leaving `vars` as it found them
/// fails at *"the insert commits"* with `Styles(BadVariable)` — the vendored
/// style names L's id, as the stroke style's twin above did. No other test in
/// this file failed under it.
#[test]
fn a_kits_effects_style_with_a_bound_blur_lands_on_the_consumers_copy() {
    let mut k = kit();
    let (glow, item) = (k.ids.mint_style(), k.ids.mint_item());
    let mut shadow = Effect::new(EffectKind::DropShadow(Shadow::default()));
    shadow.set_number(EFFECT_BLUR, 0.0);
    shadow.vars[EFFECT_BLUR] = Some(k.spare);
    let effects = vec![ondin_core::Keyed::new(item, shadow)];
    k.doc
        .apply(&Transaction(vec![
            Operation::AddStyle {
                style: Style {
                    id: glow,
                    name: "Glow".into(),
                    value: StyleValue::Effects(effects.clone()),
                    origin: None,
                    forked: None,
                },
            },
            Operation::SetEffects {
                id: k.badge,
                effects,
            },
            Operation::SetEffectsStyle {
                id: k.badge,
                style: Some(glow),
            },
        ]))
        .expect("the badge shadowed by Glow");
    assert_eq!(kit::publish(&mut k.doc, None, 2), Some(2));
    let (mut d, mut ids, _) = consumer(&[]);
    let root = d.root();
    let (tx, inst) = kit::insert(&d, &mut ids, L_ID, 2, published(&k), k.badge, root).unwrap();
    d.apply(&tx).expect("the insert commits");
    let spare = d
        .variables()
        .by_origin((L_ID, k.spare))
        .expect("spare vendored")
        .id;
    assert_ne!(spare, k.spare, "under an id of D's");
    let style = d
        .styles()
        .iter()
        .find(|s| s.name == "Glow")
        .expect("Glow vendored");
    let StyleValue::Effects(v) = &style.value else {
        unreachable!("an effects style")
    };
    assert_eq!(
        v[0].vars[EFFECT_BLUR],
        Some(spare),
        "the style's blur reads D's copy"
    );
    let n = d.get(inst).unwrap();
    assert_eq!(
        n.effects()[0].vars[EFFECT_BLUR],
        Some(spare),
        "and so does the instance's"
    );
    assert_eq!(
        d.resolved_effects(inst, n.effects())[0].numbers()[EFFECT_BLUR],
        Some(1.0),
        "spare's Day is drawn"
    );
}

/// **The document's own stroke and effects styles reading a kit's number
/// variable the kit drops are detached to the number they drew** (§15 D1073,
/// D1074; `style::detach_vars`' width and effects arms): D holds the kit's `a`
/// (*Badge*'s width bound to it, 3) and styles *Rim* and *Haze* of its own read
/// it — a width and a blur. The kit rebinds *Badge* to `b`, `a` is removed, and
/// both styles keep 3 as a literal, unbound.
///
/// Flips, run: `detach_vars`' width arm keeping every `width_var` fails in
/// `accept`, the commit refusing *Rim* (`BadVariable`); its effects arm
/// skipping the slots fails the same way over *Haze*.
#[test]
fn a_documents_own_stroke_and_effects_styles_detach_a_removed_kit_number() {
    let bind = |d: &Document, id: NodeId, v: VarId| {
        let item = d.get(id).unwrap().paint().strokes[0].id;
        bind_stroke_width_ops(d, id, item, v)
    };
    let (mut k, a, b) = kit_with_numbers(&bind, [3.0, 5.0]);
    let (mut d, mut ids, _) = consumer(&[]);
    insert(&mut d, &mut ids, &k, k.badge);
    let held = d.variables().by_origin((L_ID, a)).expect("a vendored").id;
    let (rim, haze) = (ids.mint_style(), ids.mint_style());
    let mut shadow = Effect::new(EffectKind::DropShadow(Shadow::default()));
    shadow.set_number(EFFECT_BLUR, 0.0);
    shadow.vars[EFFECT_BLUR] = Some(held);
    d.apply(&Transaction(vec![
        Operation::AddStyle {
            style: Style {
                id: rim,
                name: "Rim".into(),
                value: StyleValue::Strokes(keyed_by_position([Stroke {
                    width: 0.0,
                    width_var: Some(held),
                    ..Default::default()
                }])),
                origin: None,
                forked: None,
            },
        },
        Operation::AddStyle {
            style: Style {
                id: haze,
                name: "Haze".into(),
                value: StyleValue::Effects(keyed_by_position([shadow])),
                origin: None,
                forked: None,
            },
        },
    ]))
    .expect("D's own styles reading the kit's a");

    let ops = bind(&k.doc, k.badge, b);
    k.doc.apply(&Transaction(ops)).unwrap();
    kit::publish(&mut k.doc, None, 3);
    accept(&mut d, &mut ids, &k);
    assert!(d.variables().get(held).is_none(), "fixture: a removed");
    let StyleValue::Strokes(s) = &d.styles().get(rim).unwrap().value else {
        panic!("a stroke style")
    };
    assert_eq!(
        (s[0].width, s[0].width_var),
        (3.0, None),
        "Rim keeps the width it drew"
    );
    let StyleValue::Effects(e) = &d.styles().get(haze).unwrap().value else {
        panic!("an effects style")
    };
    assert_eq!(
        (e[0].numbers()[EFFECT_BLUR], e[0].vars[EFFECT_BLUR]),
        (Some(3.0), None),
        "Haze keeps the blur it drew"
    );
}

/// **A revealed frame set to a kit's mode is reported, and stays** (§15 D1066,
/// `MadeLocal::kit_frames`): *Icon* is set to the kit's *Night* on the kit's
/// page; in D its vault copy is set to D's held *Night* of the kit's. Made
/// local, the main keeps that mode and is the one frame reported.
///
/// Flip, run: `tokens::fork` reporting a frame on one of the **document's own**
/// modes (`t.mode(m)` for `t.kit_mode_of(m)`) fails at *"the main is
/// reported"*, an empty list.
#[test]
fn make_local_reports_a_frame_set_to_a_kits_mode() {
    let mut k = kit();
    let night = k.doc.variables().modes()[1].id;
    k.doc
        .apply(&Transaction(vec![Operation::SetMode {
            id: k.icon,
            mode: Some(night),
        }]))
        .unwrap();
    assert_eq!(kit::publish(&mut k.doc, None, 2), Some(2));
    let (mut d, mut ids, _) = consumer(&["Day"]);
    let inst = insert(&mut d, &mut ids, &k, k.icon);
    let main = d.get(inst).unwrap().link().unwrap();
    let mode = d.get(main).unwrap().mode();
    assert_eq!(
        mode.and_then(|m| d.variables().kit_mode_of(m))
            .map(|(kit, m)| (kit.to_string(), m.name.clone())),
        Some((L_ID.to_string(), "Night".to_string())),
        "fixture: the vault main is set to the kit's Night"
    );
    let (_, made) = make_local(&mut d, &mut ids, inst);
    assert_eq!(made.kit_frames, [main], "the main is reported");
    assert_eq!(d.get(main).unwrap().mode(), mode, "and keeps its mode");
}

/// ***Make local* forks a style the main reads with its value pointed at the
/// forked variables** (§15 D1066 (d)): *Badge* is styled *Tinted*, a fill style
/// bound to `brand`. Made local, D gains *Atlas/brand* and *Atlas/Tinted*, the
/// forked style's fill reads *Atlas/brand* — not D's held copy of the kit's —
/// and the main and its instance are styled by it and bound to it.
///
/// Flips, run: `tokens::fork` writing the forked style's value unmapped
/// (`style.value.clone()` for `remap.remap_value(&style.value)`) fails at *"the
/// forked style reads the forked variable"*, D's held `brand` (and
/// `a_main_the_kit_removes_forks_a_variable_only_it_read` at *"Badge reads a
/// fork, not a literal"*). `Fork::rebind_ops`' style arm never binding fails at
/// *"the main is styled by the fork"*, `None` — the commit detached the slot —
/// as two older tests here fail too. And `fork` no longer pushing a forked style's own variables
/// (`pending.extend(st.value.vars())` dropped) **does not bite** — the reading
/// D1066 (d) records: a styled slot holds its style's paints, which
/// `own_reads` pushes anyway.
#[test]
fn make_local_forks_a_style_with_its_variables_remapped() {
    let mut k = kit();
    let tinted = k.ids.mint_style();
    k.doc
        .apply(&Transaction(vec![
            Operation::AddStyle {
                style: Style {
                    id: tinted,
                    name: "Tinted".into(),
                    value: StyleValue::Fills(bound(k.brand)),
                    origin: None,
                    forked: None,
                },
            },
            Operation::SetFillStyle {
                id: k.badge,
                style: Some(tinted),
            },
        ]))
        .unwrap();
    assert_eq!(kit::publish(&mut k.doc, None, 2), Some(2));
    let (mut d, mut ids, _) = consumer(&["Day"]);
    let inst = insert(&mut d, &mut ids, &k, k.badge);
    let held = d.variables().by_origin((L_ID, k.brand)).unwrap().id;
    assert!(
        d.get(inst).unwrap().styles().fills.is_some(),
        "fixture: the instance is styled"
    );
    let (main, made) = make_local(&mut d, &mut ids, inst);
    assert_eq!(made.forked, (1, 1));
    let fork = d
        .variables()
        .local()
        .find(|v| v.name == "Atlas/brand")
        .expect("brand forked")
        .id;
    assert_ne!(fork, held);
    let style = d
        .styles()
        .iter()
        .find(|s| s.name == "Atlas/Tinted")
        .expect("Tinted forked");
    let StyleValue::Fills(f) = &style.value else {
        panic!("a fill style")
    };
    assert_eq!(
        f[0].brush.var(),
        Some(fork),
        "the forked style reads the forked variable"
    );
    assert_eq!(
        d.get(main).unwrap().styles().fills,
        Some(style.id),
        "the main is styled by the fork"
    );
    assert_eq!(d.get(inst).unwrap().styles().fills, Some(style.id));
    for n in [main, inst] {
        assert_eq!(d.get(n).unwrap().paint().fills[0].brush.var(), Some(fork));
    }
    assert_eq!(
        drawn(&d, inst),
        Brush::Solid(WHITE),
        "nothing drawn changes"
    );
    assert!(
        !ondin_core::reset::drift(&d, inst).any(),
        "no override on the instance"
    );
}

/// **An update whose publish reads none of a kit's variables drops the kit's
/// record**, so a later publish reading them again is inserted, not refused
/// `Modes` for good with no update to offer.
///
/// flip: `plan`'s mode-less record kept (the `filter` in its final loop
/// removed) → fails *"the record goes"*. `held`'s filter is a second defence,
/// for a mode-less record a file carries; removing it alone stays green.
#[test]
fn a_kit_whose_variables_went_unread_is_inserted_again_when_they_return() {
    let mut k = kit();
    let (mut d, mut ids, _) = consumer(&[]);
    insert(&mut d, &mut ids, &k, k.badge);
    let white = || {
        keyed_by_position([Fill {
            brush: WHITE.into(),
            visible: true,
        }])
    };
    k.doc
        .apply(&Transaction(vec![
            Operation::SetFills {
                id: k.dot,
                fills: white(),
            },
            Operation::SetFills {
                id: k.badge,
                fills: white(),
            },
        ]))
        .unwrap();
    assert_eq!(kit::publish(&mut k.doc, None, 2), Some(2));
    accept(&mut d, &mut ids, &k);
    assert!(!d.variables().kits().contains_key(L_ID), "the record goes");
    k.doc
        .apply(&Transaction(vec![Operation::SetFills {
            id: k.badge,
            fills: bound(k.brand),
        }]))
        .unwrap();
    assert_eq!(kit::publish(&mut k.doc, None, 3), Some(3));
    let inst = insert(&mut d, &mut ids, &k, k.badge);
    assert_eq!(drawn(&d, inst), Brush::Solid(WHITE), "brand's Day value");
}

/// **A kit variable whose kit holds no mode is refused** at the load and the
/// commit: it has no column to read.
///
/// flip: `check_table`'s mode-less arm removed → the commit is accepted.
#[test]
fn a_kit_variable_under_a_mode_less_record_is_refused() {
    let k = kit();
    let (mut d, mut ids, _) = consumer(&[]);
    insert(&mut d, &mut ids, &k, k.badge);
    // Cell-less as well, so no other rule refuses it first.
    let held = Variable {
        cells: Default::default(),
        ..d.variables().iter().next().unwrap().clone()
    };
    let r = d.apply(&Transaction(vec![
        Operation::SetKitModes {
            kit: L_ID.into(),
            modes: Some(KitModes {
                name: None,
                modes: vec![],
                map: Default::default(),
            }),
        },
        Operation::SetVariable { var: held },
    ]));
    assert!(
        matches!(r, Err(OpError::Variables(VarRule::NoKit(_)))),
        "{r:?}"
    );
}
