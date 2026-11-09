//! *Clean up…* (`clean_up.rs`): each rule takes only what draws nothing, lists
//! what it would change a pixel by taking, and the rules see each other's work.

use ondin_core::clean_up::{Kept, Plan, Rule, Rules, Scope, Subject, plan};
use ondin_core::guide::{Guide, GuideAxis, GuideId};
use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, Fill, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction, build,
    keyed_by_position,
};

fn rect() -> NodeKind {
    NodeKind::Rect {
        size: Size::new(10.0, 10.0),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    }
}

struct Fixture {
    doc: Document,
    ids: IdSource,
    page: NodeId,
    frame: NodeId,
}

impl Fixture {
    /// A page holding one frame.
    fn new() -> Self {
        let mut ids = IdSource::new(0xC1EA);
        let page = ids.mint();
        let mut f = Self {
            doc: Document::new(page),
            ids,
            page,
            frame: page,
        };
        f.frame = f.add(
            page,
            NodeKind::Artboard {
                size: Size::new(400.0, 300.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
        );
        f
    }

    fn commit(&mut self, ops: Vec<Operation>) {
        self.doc.apply(&Transaction(ops)).expect("fixture edit");
    }

    fn add(&mut self, parent: NodeId, kind: NodeKind) -> NodeId {
        let id = self.ids.mint();
        let index = self.doc.get(parent).unwrap().children().len();
        self.commit(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(Affine::IDENTITY),
            name: None,
        }]);
        id
    }

    /// A rect with these fills, `(colour, visible)` bottom first.
    fn painted(&mut self, parent: NodeId, fills: &[(Color, bool)]) -> NodeId {
        let id = self.add(parent, rect());
        self.commit(vec![Operation::SetFills {
            id,
            fills: keyed_by_position(fills.iter().map(|(c, visible)| Fill {
                brush: Brush::Solid(*c).into(),
                visible: *visible,
            })),
        }]);
        id
    }

    fn plan(&self, rules: &Rules) -> Plan {
        plan(
            &self.doc,
            &Resolved::rebuild(&self.doc),
            &Scope::Pages(vec![self.page]),
            rules,
        )
    }

    /// The plan applied to the document, as the app commits it.
    fn run(&mut self, rules: &Rules) -> Plan {
        let p = self.plan(rules);
        self.doc
            .apply(&p.tx)
            .expect("the plan's transaction applies");
        p
    }

    fn fills(&self, id: NodeId) -> Vec<Color> {
        self.doc
            .get(id)
            .unwrap()
            .paint()
            .fills
            .iter()
            .filter_map(|f| match f.value.brush.literal() {
                Some(Brush::Solid(c)) => Some(*c),
                _ => None,
            })
            .collect()
    }
}

const RED: Color = Color::from_rgb8(255, 0, 0);
const BLUE: Color = Color::from_rgb8(0, 0, 255);
const GREEN: Color = Color::from_rgb8(0, 255, 0);

fn only(rules: &[Rule]) -> Rules {
    let mut r = Rules {
        on: [false; 6],
        ..Rules::default()
    };
    for rule in rules {
        r.set(*rule, true);
    }
    r
}

/// A hidden fill goes and the visible one stays, as one item on one layer.
#[test]
fn hidden_paint_takes_only_what_is_switched_off() {
    let mut f = Fixture::new();
    let frame = f.frame;
    let a = f.painted(frame, &[(RED, true), (BLUE, false)]);
    let p = f.run(&only(&[Rule::HiddenPaint]));
    assert_eq!(p.items(), 1);
    assert_eq!(p.layers(), 1);
    assert_eq!(p.findings[0].what, "Fill");
    assert_eq!(f.fills(a), [RED], "the visible fill stays");
}

/// Fills under an opaque solid go; under a translucent one they stay.
#[test]
fn covered_fills_go_only_under_an_opaque_solid() {
    let mut f = Fixture::new();
    let frame = f.frame;
    let covered = f.painted(frame, &[(BLUE, true), (GREEN, true), (RED, true)]);
    let seen = f.painted(frame, &[(BLUE, true), (RED.with_alpha(0.5), true)]);
    let p = f.run(&only(&[Rule::CoveredFills]));
    assert_eq!(p.items(), 2);
    assert_eq!(p.findings.len(), 1, "the translucent top covers nothing");
    assert_eq!(p.findings[0].what, "2 fills under #FF0000");
    assert_eq!(f.fills(covered), [RED]);
    assert_eq!(f.fills(seen).len(), 2);
}

/// The rules see each other's work: a shape that loses its only fill is empty,
/// and the group that loses it holds one layer, so the group goes too.
/// flip: `Run::take` applying each rule's transaction to a throwaway clone
/// rather than the run's copy → fails at the rule list (no ungroup follows).
#[test]
fn each_rule_sees_what_the_ones_before_it_took() {
    let mut f = Fixture::new();
    let frame = f.frame;
    let a = f.painted(frame, &[(BLUE, false)]);
    let b = f.painted(frame, &[(RED, true)]);
    let (tx, g) = build::group(&f.doc, &Resolved::rebuild(&f.doc), &mut f.ids, &[a, b]).unwrap();
    f.doc.apply(&tx).unwrap();

    let p = f.run(&Rules::default());
    let rules: Vec<Rule> = p.findings.iter().map(|x| x.rule).collect();
    assert_eq!(
        rules,
        [Rule::HiddenPaint, Rule::EmptyLayers, Rule::OneLayerGroups]
    );
    assert!(!f.doc.contains(a), "the emptied shape is deleted");
    assert!(!f.doc.contains(g), "the group holding B is ungrouped");
    assert_eq!(f.doc.get(b).unwrap().parent(), Some(frame));
    assert_eq!(p.items(), 3);
}

/// Hidden layers start off; on, a locked one is listed and kept until skipping
/// is turned off.
#[test]
fn a_locked_hidden_layer_is_kept_while_skipping() {
    let mut f = Fixture::new();
    let frame = f.frame;
    let a = f.painted(frame, &[(RED, true)]);
    f.commit(vec![
        Operation::SetVisible {
            id: a,
            visible: false,
        },
        Operation::SetLocked {
            id: a,
            locked: true,
        },
    ]);
    assert!(
        f.plan(&Rules::default()).findings.is_empty(),
        "off by default"
    );

    let rules = only(&[Rule::HiddenLayers]);
    let p = f.plan(&rules);
    assert_eq!(p.findings[0].kept, Some(Kept::Locked));
    assert_eq!(p.items(), 0);
    assert!(p.tx.0.is_empty());

    f.run(&Rules {
        skip_locked: false,
        ..rules
    });
    assert!(!f.doc.contains(a), "deleted once locked layers are cleaned");
}

/// An empty layer in an auto layout parent is a spacer; outside one it goes.
#[test]
fn an_empty_layer_in_an_auto_layout_is_kept() {
    let mut f = Fixture::new();
    let frame = f.frame;
    let spacer = f.add(frame, rect());
    let mut rules = only(&[Rule::EmptyLayers]);
    rules.skip_locked = true;
    f.commit(vec![Operation::SetDisplay {
        id: frame,
        display: Some(ondin_core::container::Display::Flex(Default::default())),
    }]);
    let p = f.plan(&rules);
    assert_eq!(p.findings[0].kept, Some(Kept::Spacer));

    f.commit(vec![Operation::SetDisplay {
        id: frame,
        display: None,
    }]);
    f.run(&rules);
    assert!(!f.doc.contains(spacer));
}

/// A one-layer group with opacity keeps it: ungrouping would brighten its layer.
#[test]
fn a_faded_group_is_not_ungrouped() {
    let mut f = Fixture::new();
    let frame = f.frame;
    let a = f.painted(frame, &[(RED, true)]);
    let (tx, g) = build::group(&f.doc, &Resolved::rebuild(&f.doc), &mut f.ids, &[a]).unwrap();
    f.doc.apply(&tx).unwrap();
    f.commit(vec![Operation::SetOpacity {
        id: g,
        opacity: 0.5,
    }]);
    let p = f.run(&only(&[Rule::OneLayerGroups]));
    assert_eq!(p.findings[0].kept, Some(Kept::Opacity));
    assert!(f.doc.contains(g));
}

/// Guides start off; on, the page's go, and locked guides are kept.
#[test]
fn guides_go_when_asked_and_stay_while_locked() {
    let mut f = Fixture::new();
    let id = GuideId(f.ids.mint());
    let page = f.page;
    f.commit(vec![Operation::AddGuide {
        guide: Guide {
            id,
            axis: GuideAxis::Horizontal,
            position: 120.0,
            color: None,
            owner: page,
        },
    }]);
    assert_eq!(f.plan(&Rules::default()).items_of(Rule::Guides), 0);
    let mut rules = only(&[Rule::Guides]);
    rules.guides_locked = true;
    let p = f.plan(&rules);
    assert_eq!(p.findings[0].kept, Some(Kept::Locked));
    assert!(matches!(p.findings[0].subject, Subject::Guide { .. }));

    rules.guides_locked = false;
    f.run(&rules);
    assert!(f.doc.guides().is_empty());
}

/// The plan's transaction undoes to the document it was planned on.
#[test]
fn a_clean_up_undoes_whole() {
    let mut f = Fixture::new();
    let frame = f.frame;
    let a = f.painted(frame, &[(BLUE, false)]);
    let (tx, _) = build::group(&f.doc, &Resolved::rebuild(&f.doc), &mut f.ids, &[a]).unwrap();
    f.doc.apply(&tx).unwrap();
    let before = ondin_core::io::save(&f.doc).unwrap();
    let p = f.plan(&Rules::default().all_on());
    let outcome = f.doc.apply(&p.tx).unwrap();
    f.doc.apply(&outcome.inverse).unwrap();
    let after = ondin_core::io::save(&f.doc).unwrap();
    assert!(before == after, "undo puts the document back as it was");
}

/// The kept reason `rule` gives `id`'s row; panics when it has none.
fn kept(p: &Plan, rule: Rule, id: NodeId) -> Option<Kept> {
    p.findings
        .iter()
        .find(|x| x.rule == rule && x.subject == Subject::Layer(id))
        .unwrap_or_else(|| panic!("{rule:?} lists {id:?}"))
        .kept
}

fn png() -> Vec<ondin_core::Keyed<ondin_core::ExportSpec>> {
    keyed_by_position([ondin_core::ExportSpec::new(
        ondin_core::ExportFormat::Png,
        ondin_core::ExportScale::Times(1.0),
    )])
}

fn shadow() -> Vec<ondin_core::Keyed<ondin_core::Effect>> {
    keyed_by_position([ondin_core::Effect {
        kind: ondin_core::EffectKind::DropShadow(ondin_core::Shadow::default()),
        visible: true,
        vars: Default::default(),
    }])
}

fn flex() -> Option<ondin_core::container::Display> {
    Some(ondin_core::container::Display::Flex(Default::default()))
}

impl Fixture {
    /// `ids` grouped where they stand.
    fn group(&mut self, ids: &[NodeId]) -> NodeId {
        let (tx, g) = build::group(&self.doc, &Resolved::rebuild(&self.doc), &mut self.ids, ids)
            .expect("grouped");
        self.doc.apply(&tx).unwrap();
        g
    }

    /// A frame holding a red rect, made a main.
    fn main(&mut self, parent: NodeId) -> NodeId {
        let m = self.add(
            parent,
            NodeKind::Artboard {
                size: Size::new(40.0, 40.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
        );
        self.painted(m, &[(RED, true)]);
        self.commit(vec![Operation::SetComponent {
            id: m,
            component: true,
        }]);
        m
    }

    /// A set of two variants, *Default* and *Hover*, whose property is the
    /// interaction state; answers the *Default* variant.
    fn state_set(&mut self) -> NodeId {
        use ondin_core::variant::{Interaction, VariantProp, VariantSet};
        let frame = self.frame;
        let set = self.add(frame, rect_frame());
        let rest = self.add(set, rect_frame());
        let hover = self.add(set, rect_frame());
        let mut prop = VariantProp::new("State", vec!["Default".into(), "Hover".into()]);
        prop.states.insert("Hover".into(), Interaction::Hover);
        self.commit(vec![
            Operation::SetComponent {
                id: rest,
                component: true,
            },
            Operation::SetComponent {
                id: hover,
                component: true,
            },
            Operation::SetVariantSet {
                id: set,
                set: Some(VariantSet {
                    props: vec![prop],
                    ..Default::default()
                }),
            },
            Operation::SetVariant {
                id: rest,
                values: vec!["Default".into()],
            },
            Operation::SetVariant {
                id: hover,
                values: vec!["Hover".into()],
            },
        ]);
        rest
    }
}

fn rect_frame() -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(40.0, 40.0),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    }
}

/// *Empty layers* keeps an empty mask, an empty layer with an export, an empty
/// main and an empty layer in a state-paired variant, and takes none of them.
/// flip: drop any one arm of `Run::empty_layers`' kept ladder → fails at that
/// layer's `kept`.
#[test]
fn empty_layers_keep_masks_exports_mains_and_states() {
    let mut f = Fixture::new();
    let frame = f.frame;
    let mask = f.add(frame, rect());
    let exported = f.add(frame, rect());
    let main = f.add(frame, rect_frame());
    f.commit(vec![
        Operation::SetMask {
            id: mask,
            mask: true,
        },
        Operation::SetExports {
            id: exported,
            exports: png(),
        },
        Operation::SetComponent {
            id: main,
            component: true,
        },
    ]);
    let rest = f.state_set();
    let paired = f.add(rest, rect());

    let p = f.run(&only(&[Rule::EmptyLayers]));
    assert_eq!(kept(&p, Rule::EmptyLayers, mask), Some(Kept::Mask));
    assert_eq!(kept(&p, Rule::EmptyLayers, exported), Some(Kept::Export));
    assert_eq!(
        kept(&p, Rule::EmptyLayers, main),
        Some(Kept::Main { instances: 0 })
    );
    assert_eq!(kept(&p, Rule::EmptyLayers, paired), Some(Kept::State));
    for id in [mask, exported, main, paired] {
        assert!(f.doc.contains(id), "{id:?} stays");
    }
}

/// *Hidden layers* keeps a hidden main and a hidden layer in a state-paired
/// variant.
/// flip: drop `Run::hidden_layers`' `mains_within` or `state_paired` arm →
/// fails at that layer's `kept`.
#[test]
fn hidden_layers_keep_mains_and_states() {
    let mut f = Fixture::new();
    let frame = f.frame;
    let main = f.main(frame);
    let rest = f.state_set();
    let paired = f.painted(rest, &[(RED, true)]);
    for id in [main, paired] {
        f.commit(vec![Operation::SetVisible { id, visible: false }]);
    }
    let p = f.run(&only(&[Rule::HiddenLayers]));
    assert_eq!(
        kept(&p, Rule::HiddenLayers, main),
        Some(Kept::Main { instances: 0 })
    );
    assert_eq!(kept(&p, Rule::HiddenLayers, paired), Some(Kept::State));
    assert!(f.doc.contains(main) && f.doc.contains(paired));
}

/// A fill list a style supplies is the style's: *Hidden paint* and *Covered
/// fills* list it as kept and leave it whole.
/// flip: `Run::hidden_paint` or `Run::covered_fills` ignoring `styles().fills`
/// → fails at that rule's `kept`.
#[test]
fn a_styled_fill_list_is_kept() {
    use ondin_core::style::{Style, StyleValue};
    let mut f = Fixture::new();
    let frame = f.frame;
    let hidden = f.painted(frame, &[(RED, true), (BLUE, false)]);
    let covered = f.painted(frame, &[(BLUE, true), (RED, true)]);
    let mut ops = Vec::new();
    for id in [hidden, covered] {
        let style = f.ids.mint_style();
        ops.push(Operation::AddStyle {
            style: Style {
                id: style,
                name: format!("S{}", ops.len()),
                value: StyleValue::Fills(f.doc.get(id).unwrap().paint().fills.clone()),
                origin: None,
                forked: None,
                description: String::new(),
            },
        });
        ops.push(Operation::SetFillStyle {
            id,
            style: Some(style),
        });
    }
    f.commit(ops);

    let p = f.run(&only(&[Rule::HiddenPaint, Rule::CoveredFills]));
    assert_eq!(kept(&p, Rule::HiddenPaint, hidden), Some(Kept::Style));
    assert_eq!(kept(&p, Rule::CoveredFills, covered), Some(Kept::Style));
    assert_eq!(f.fills(hidden), [RED, BLUE]);
    assert_eq!(f.fills(covered), [BLUE, RED]);
}

/// A one-layer group is kept while ungrouping would lose its blend mode, its
/// effects, its layout, its mask or its export.
/// flip: drop any one of those arms of `Run::group_kept` → fails at that
/// group's `kept`.
#[test]
fn a_group_keeps_what_ungrouping_would_lose() {
    use ondin_core::BlendMode;
    let mut f = Fixture::new();
    let frame = f.frame;
    let mut groups = Vec::new();
    for _ in 0..5 {
        let a = f.painted(frame, &[(RED, true)]);
        groups.push(f.group(&[a]));
    }
    let [blend, effects, layout, mask, export] = groups[..] else {
        unreachable!()
    };
    f.commit(vec![
        Operation::SetBlend {
            id: blend,
            blend: BlendMode::Multiply,
        },
        Operation::SetEffects {
            id: effects,
            effects: shadow(),
        },
        Operation::SetDisplay {
            id: layout,
            display: flex(),
        },
        Operation::SetMask {
            id: mask,
            mask: true,
        },
        Operation::SetExports {
            id: export,
            exports: png(),
        },
    ]);
    let p = f.run(&only(&[Rule::OneLayerGroups]));
    let r = Rule::OneLayerGroups;
    assert_eq!(kept(&p, r, blend), Some(Kept::Blend));
    assert_eq!(kept(&p, r, effects), Some(Kept::Effects));
    assert_eq!(kept(&p, r, layout), Some(Kept::Layout));
    assert_eq!(kept(&p, r, mask), Some(Kept::Mask));
    assert_eq!(kept(&p, r, export), Some(Kept::Export));
    assert!(groups.iter().all(|g| f.doc.contains(*g)));
}

/// Nothing inside an instance is touched, nor the instance root's own paint,
/// whatever the main holds.
/// flip: drop `Run::in_instance` from any rule's skip → that rule lists a copy.
#[test]
fn an_instance_is_left_alone() {
    let mut f = Fixture::new();
    let frame = f.frame;
    let main = f.main(frame);
    let hidden = f.painted(main, &[(RED, true), (BLUE, false)]);
    f.painted(main, &[(BLUE, true), (RED, true)]);
    f.add(main, rect());
    let lone = f.painted(main, &[(RED, true)]);
    f.group(&[lone]);
    let off = f.painted(main, &[(RED, true)]);
    f.commit(vec![
        Operation::SetVisible {
            id: off,
            visible: false,
        },
        Operation::SetFills {
            id: main,
            fills: f.doc.get(hidden).unwrap().paint().fills.clone(),
        },
        Operation::SetVisible {
            id: hidden,
            visible: true,
        },
    ]);
    let (tx, instance) = build::place_instance(&f.doc, &mut f.ids, main, frame, None).unwrap();
    f.doc.apply(&tx).unwrap();

    let p = plan(
        &f.doc,
        &Resolved::rebuild(&f.doc),
        &Scope::Layers(vec![instance]),
        &Rules::default().all_on(),
    );
    assert!(
        p.findings.is_empty(),
        "nothing in the instance is listed: {:?}",
        p.findings
    );
    assert!(p.tx.0.is_empty());
    assert!(
        !plan(
            &f.doc,
            &Resolved::rebuild(&f.doc),
            &Scope::Layers(vec![main]),
            &Rules::default().all_on(),
        )
        .findings
        .is_empty(),
        "the fixture: the main itself has things to take"
    );
}

impl Fixture {
    /// The plan applied with everything the components owe it, as the app's
    /// commit does, so a main's edit reaches its instances.
    fn run_owed(&mut self, rules: &Rules) -> Plan {
        let p = self.plan(rules);
        let mut tx = p.tx.clone();
        ondin_core::propagate::owed(&self.doc, &mut tx, &mut self.ids);
        self.doc
            .apply(&tx)
            .expect("the plan and what it owes apply");
        p
    }

    /// A second page, empty.
    fn page(&mut self) -> NodeId {
        let id = self.ids.mint();
        self.commit(vec![Operation::AddPage {
            id,
            index: 1,
            name: "Other".into(),
            ground: None,
            draft: false,
            modes: Vec::new(),
        }]);
        id
    }

    /// The layer each of `script`'s steps made, if one, run in this document.
    fn script(&mut self, script: serde_json::Value) -> Vec<Option<NodeId>> {
        ondin_core::verbs::run_script(
            &mut self.doc,
            &mut self.ids,
            ondin_core::verbs::Lent::default(),
            &script,
        )
        .unwrap_or_else(|e| panic!("{e}"))
        .iter()
        .map(|made| made.first().and_then(|w| NodeId::from_wire(w)))
        .collect()
    }
}

/// A text draws while it has content or a binding: one bound to a string
/// variable (its literal empty), one with no fill (drawn black) and one whose
/// only fill is hidden all stay at the defaults; an empty unbound text goes.
/// flip: `Run::emptiness` reading only the literal content, or sending a text
/// with content to the paint test → fails at that text's survival.
#[test]
fn a_text_with_content_or_a_binding_is_not_empty() {
    use serde_json::json;
    let mut f = Fixture::new();
    let frame = f.frame.to_wire();
    let hi = serde_json::to_value(ondin_core::Cell::String("Hi there".into())).unwrap();
    let made = f.script(json!([
        { "verb": "add_variable", "args": { "kind": "string", "name": "greeting", "value": hi }, "as": "v" },
        { "verb": "create_text", "args": { "parent": frame, "text": "Hello" }, "as": "bound" },
        { "verb": "bind_variable", "args": { "nodes": ["@bound"], "field": "content", "variable": "@v" } },
        { "verb": "create_text", "args": { "parent": frame, "text": "Hello" }, "as": "bare" },
        { "verb": "set_fills", "args": { "node": "@bare", "fills": [] } },
        { "verb": "create_text", "args": { "parent": frame, "text": "Hello" }, "as": "dark" },
        { "verb": "create_text", "args": { "parent": frame, "text": "Gone" }, "as": "empty" },
    ]));
    let [bound, bare, dark, empty] = [1, 3, 5, 6].map(|i| made[i].expect("a text"));
    let fills = keyed_by_position([Fill {
        brush: Brush::Solid(BLUE).into(),
        visible: false,
    }]);
    let NodeKind::Text { content, .. } = f.doc.get(bound).unwrap().kind() else {
        unreachable!()
    };
    assert!(content.is_empty(), "the fixture: a bound literal is empty");
    let NodeKind::Text { content, .. } = f.doc.get(empty).unwrap().kind().clone() else {
        unreachable!()
    };
    f.commit(vec![
        Operation::SetFills { id: dark, fills },
        Operation::SetText {
            id: empty,
            content: String::new(),
            spans: Default::default(),
            para_spans: Default::default(),
        },
    ]);
    assert_eq!(content, "Gone");

    let p = f.run(&Rules::default());
    for id in [bound, bare, dark] {
        assert!(
            f.doc.contains(id),
            "{id:?} draws and stays: {:?}",
            p.findings
        );
    }
    assert!(!f.doc.contains(empty), "an empty unbound text goes");
}

/// A boolean's operands draw their outline into it whatever their own paint, so
/// unpainted operands are not empty and the boolean stays.
/// flip: drop the boolean-ancestor arm of `Run::emptiness` → fails at "the
/// operands stay".
#[test]
fn a_booleans_unpainted_operands_stay() {
    let mut f = Fixture::new();
    let frame = f.frame;
    let a = f.add(frame, rect());
    let b = f.add(frame, rect());
    let (tx, boolean) = build::boolean(
        &f.doc,
        &Resolved::rebuild(&f.doc),
        &mut f.ids,
        &[a, b],
        ondin_core::BoolOp::Subtract,
        None,
    )
    .unwrap();
    f.doc.apply(&tx).unwrap();
    f.commit(vec![Operation::SetFills {
        id: boolean,
        fills: keyed_by_position([Fill {
            brush: Brush::Solid(RED).into(),
            visible: true,
        }]),
    }]);
    f.run(&Rules::default());
    assert!(f.doc.contains(a) && f.doc.contains(b), "the operands stay");
    assert!(f.doc.contains(boolean));
}

/// An empty layer in a group in an auto layout is a spacer: the group's box is
/// its layers', so taking it would move the group's siblings.
/// flip: `Run::in_layout` asking only the direct parent → fails at the kept
/// reason.
#[test]
fn an_empty_layer_in_a_group_in_an_auto_layout_is_kept() {
    let mut f = Fixture::new();
    let frame = f.frame;
    f.painted(frame, &[(RED, true)]);
    let blank = f.add(frame, rect());
    let g = f.group(&[blank]);
    let after = f.painted(frame, &[(RED, true)]);
    f.commit(vec![Operation::SetDisplay {
        id: frame,
        display: flex(),
    }]);
    let x0 = |f: &Fixture| Resolved::rebuild(&f.doc).world_bounds(after).unwrap().x0;
    let before = x0(&f);
    let p = f.run(&Rules::default());
    assert_eq!(kept(&p, Rule::EmptyLayers, blank), Some(Kept::Spacer));
    assert!(f.doc.contains(blank) && f.doc.contains(g));
    assert_eq!(x0(&f), before, "the next sibling stays where it was");
}

/// A group whose only layer is a mask stays a group: ungrouped, the mask would
/// clip the layers above the group in its parent.
/// flip: drop the lone-mask arm of `Run::group_kept` → fails at the kept reason.
#[test]
fn a_group_holding_only_a_mask_is_kept() {
    let mut f = Fixture::new();
    let frame = f.frame;
    let mask = f.painted(frame, &[(RED, true)]);
    let blank = f.add(frame, rect());
    let g = f.group(&[mask, blank]);
    let above = f.painted(frame, &[(BLUE, true)]);
    f.commit(vec![Operation::SetMask {
        id: mask,
        mask: true,
    }]);
    let p = f.run(&Rules::default());
    assert!(!f.doc.contains(blank), "the fixture: the empty layer goes");
    assert_eq!(kept(&p, Rule::OneLayerGroups, g), Some(Kept::Mask));
    assert_eq!(f.doc.get(frame).unwrap().children(), [g, above]);
}

/// *Hidden layers* keeps a hidden layer with an export, or one holding such a
/// layer, since *Export all* still writes it.
/// flip: drop the export arm of `Run::hidden_layers` → fails at the kept reason.
#[test]
fn a_hidden_layer_with_an_export_is_kept() {
    let mut f = Fixture::new();
    let frame = f.frame;
    let own = f.painted(frame, &[(RED, true)]);
    let inner = f.painted(frame, &[(RED, true)]);
    let other = f.painted(frame, &[(RED, true)]);
    let holder = f.group(&[inner, other]);
    f.commit(vec![
        Operation::SetExports {
            id: own,
            exports: png(),
        },
        Operation::SetExports {
            id: inner,
            exports: png(),
        },
        Operation::SetVisible {
            id: own,
            visible: false,
        },
        Operation::SetVisible {
            id: holder,
            visible: false,
        },
    ]);
    let p = f.run(&only(&[Rule::HiddenLayers]));
    assert_eq!(kept(&p, Rule::HiddenLayers, own), Some(Kept::Export));
    assert_eq!(kept(&p, Rule::HiddenLayers, holder), Some(Kept::Export));
    assert!(f.doc.contains(own) && f.doc.contains(inner));
}

/// With *Skip locked* on, a hidden group holding a locked layer is kept whole;
/// off, it goes.
/// flip: `Run::hidden_layers` asking only the outermost layer's lock → fails at
/// the kept reason.
#[test]
fn a_hidden_group_holding_a_locked_layer_is_kept_while_skipping() {
    let mut f = Fixture::new();
    let frame = f.frame;
    let locked = f.painted(frame, &[(RED, true)]);
    let other = f.painted(frame, &[(RED, true)]);
    let g = f.group(&[locked, other]);
    f.commit(vec![
        Operation::SetLocked {
            id: locked,
            locked: true,
        },
        Operation::SetVisible {
            id: g,
            visible: false,
        },
    ]);
    let rules = only(&[Rule::HiddenLayers]);
    let p = f.run(&rules);
    assert_eq!(kept(&p, Rule::HiddenLayers, g), Some(Kept::Locked));
    assert!(f.doc.contains(locked));
    f.run(&Rules {
        skip_locked: false,
        ..rules
    });
    assert!(!f.doc.contains(g));
}

/// *Covered fills* in a main leaves a fill an instance on another page still
/// shows through its own top item, and takes it where every instance draws the
/// main's list.
/// flip: `Run::covered_fills` not asking the main's copies → fails at "the
/// instance keeps the fill it shows".
#[test]
fn covered_fills_in_a_main_keep_what_an_instance_shows() {
    let mut f = Fixture::new();
    let frame = f.frame;
    let other = f.page();
    let white = Color::from_rgb8(255, 255, 255);
    let mut layers = Vec::new();
    for _ in 0..2 {
        let main = f.add(frame, rect_frame());
        let layer = f.painted(main, &[(RED, true), (white, true)]);
        f.commit(vec![Operation::SetComponent {
            id: main,
            component: true,
        }]);
        let (tx, instance) = build::place_instance(&f.doc, &mut f.ids, main, other, None).unwrap();
        f.doc.apply(&tx).unwrap();
        let copy = f.doc.get(instance).unwrap().children()[0];
        layers.push((layer, copy));
    }
    let [(shown, shown_copy), (plain, plain_copy)] = layers[..] else {
        unreachable!()
    };
    let mut fills = f.doc.get(shown_copy).unwrap().paint().fills.clone();
    fills[1].value.brush = Brush::Solid(white.with_alpha(0.5)).into();
    f.commit(vec![Operation::SetFills {
        id: shown_copy,
        fills,
    }]);

    let p = f.run_owed(&only(&[Rule::CoveredFills]));
    assert_eq!(
        kept(&p, Rule::CoveredFills, shown),
        Some(Kept::Instance),
        "{:?}",
        p.findings
    );
    assert_eq!(
        f.fills(shown_copy).len(),
        2,
        "the instance keeps the fill it shows"
    );
    assert_eq!(
        f.fills(plain),
        [white],
        "an untouched main loses its covered fill"
    );
    assert_eq!(f.fills(plain_copy), [white], "and so does its instance");
}

/// A one-layer group is kept as a flex item (a spacer of its layer's box), in a
/// state-paired variant, with a bound opacity, and locked while skipping.
/// flip: drop the spacer, state or locked arm of `Run::group_kept`, or the
/// `opacity_var` half of its opacity arm → fails at that group's `kept`.
#[test]
fn a_group_keeps_its_flow_state_bound_opacity_and_lock() {
    let mut f = Fixture::new();
    let frame = f.frame;
    let mut groups = Vec::new();
    for _ in 0..3 {
        let a = f.painted(frame, &[(RED, true)]);
        groups.push(f.group(&[a]));
    }
    let [spacer, faded, locked] = groups[..] else {
        unreachable!()
    };
    let rest = f.state_set();
    let a = f.painted(rest, &[(RED, true)]);
    let state = f.group(&[a]);
    let (mut ops, fade) = ondin_core::variable::add_variable_ops(
        f.doc.variables(),
        &mut f.ids,
        ondin_core::variable::CollectionId::FIRST,
        ondin_core::VarKind::Number,
        Some("fade"),
        Some(ondin_core::Cell::Number(1.0)),
    )
    .expect("a number variable");
    ops.push(Operation::SetDisplay {
        id: frame,
        display: flex(),
    });
    ops.push(Operation::SetLocked {
        id: locked,
        locked: true,
    });
    f.commit(ops);
    // The binding alone, its literal left at 1, so only `opacity_var` keeps it.
    f.commit(vec![Operation::SetOpacityVar {
        id: faded,
        var: Some(fade),
    }]);
    let n = f.doc.get(faded).unwrap();
    assert!(
        n.opacity_var().is_some() && n.opacity() == 1.0,
        "the fixture: bound, its literal whole"
    );

    let rules = only(&[Rule::OneLayerGroups]);
    let p = f.run(&rules);
    let r = Rule::OneLayerGroups;
    assert_eq!(kept(&p, r, spacer), Some(Kept::Spacer));
    assert_eq!(kept(&p, r, faded), Some(Kept::Opacity));
    assert_eq!(kept(&p, r, locked), Some(Kept::Locked));
    assert_eq!(kept(&p, r, state), Some(Kept::State));
    assert!(
        [spacer, faded, locked, state]
            .iter()
            .all(|g| f.doc.contains(*g))
    );
}

/// With *Skip locked* on, a locked layer is listed as kept by *Empty layers*,
/// *Hidden paint* and *Covered fills*, and keeps what each would take.
/// flip: drop the `Kept::Locked` arm of `Run::empty_layers`,
/// `Run::hidden_paint` or `Run::covered_fills` → fails at that rule's `kept`.
#[test]
fn each_rule_keeps_a_locked_layer_while_skipping() {
    let mut f = Fixture::new();
    let frame = f.frame;
    let empty = f.add(frame, rect());
    let hidden = f.painted(frame, &[(RED, true), (BLUE, false)]);
    let covered = f.painted(frame, &[(BLUE, true), (RED, true)]);
    f.commit(
        [empty, hidden, covered]
            .map(|id| Operation::SetLocked { id, locked: true })
            .to_vec(),
    );
    let rules = only(&[Rule::EmptyLayers, Rule::HiddenPaint, Rule::CoveredFills]);
    let p = f.run(&rules);
    assert_eq!(kept(&p, Rule::EmptyLayers, empty), Some(Kept::Locked));
    assert_eq!(kept(&p, Rule::HiddenPaint, hidden), Some(Kept::Locked));
    assert_eq!(kept(&p, Rule::CoveredFills, covered), Some(Kept::Locked));
    assert!(f.doc.contains(empty));
    assert_eq!(f.fills(hidden), [RED, BLUE]);
    assert_eq!(f.fills(covered), [BLUE, RED]);
}

/// *Covered fills* grows linearly with the layers it lists: it asks each layer's
/// instance copies from one map, not by a scan of the document per layer. Its
/// cost over a plan with no rule, at eight times the layers, is about eight
/// times (sixty-four when quadratic). Both sizes sit past the cache knee: from
/// 1,000 layers a fast runner's release build grows well past eight. The bound
/// is not checked on macOS, whose release runner grows near thirty times with
/// the linear code; the Windows and Linux legs hold it.
/// flip: `Run::covered_fills` building `Run::copies` per covered layer → fails
/// at the growth bound (about sixty times).
#[test]
fn covered_fills_is_linear_in_the_layers() {
    let white = Color::from_rgb8(255, 255, 255);
    let cost = |n: usize| {
        let mut f = Fixture::new();
        let mut ops = Vec::new();
        for index in 0..n {
            let id = f.ids.mint();
            ops.push(Operation::CreateNode {
                id,
                parent: f.frame,
                index,
                kind: rect(),
                transform: None,
                name: None,
            });
            ops.push(Operation::SetFills {
                id,
                fills: keyed_by_position([BLUE, white].map(|c| Fill {
                    brush: Brush::Solid(c).into(),
                    visible: true,
                })),
            });
        }
        f.commit(ops);
        let best = |rules: &Rules| {
            (0..3)
                .map(|_| {
                    let at = std::time::Instant::now();
                    let p = f.plan(rules);
                    (at.elapsed().as_secs_f64(), p.findings.len())
                })
                .fold((f64::MAX, 0), |a, b| (a.0.min(b.0), b.1))
        };
        let (covered, found) = best(&only(&[Rule::CoveredFills]));
        let (bare, _) = best(&only(&[]));
        assert_eq!(found, n, "the fixture: every layer is listed");
        (covered - bare).max(f64::EPSILON)
    };
    let (small, large) = (cost(4000), cost(32000));
    assert!(
        cfg!(target_os = "macos") || large / small < 24.0,
        "4,000 layers {small:.4}s, 32,000 layers {large:.4}s"
    );
}

/// A one-layer group inside a plain group in an auto layout goes: its box is
/// its layer's, so the outer group's box and the flow stay as they were.
/// flip: `Run::group_kept` asking the walk through groups
/// (`container::lays_out_above`) for its spacer arm → fails at "not kept".
#[test]
fn a_nested_one_layer_group_in_an_auto_layout_goes() {
    let mut f = Fixture::new();
    let frame = f.frame;
    let a = f.painted(frame, &[(RED, true)]);
    let x = f.painted(frame, &[(BLUE, true)]);
    f.commit(vec![Operation::SetTransform {
        id: x,
        transform: Affine::translate((30.0, 20.0)),
    }]);
    let inner = f.group(&[a]);
    let outer = f.group(&[inner, x]);
    let after = f.painted(frame, &[(GREEN, true)]);
    f.commit(vec![Operation::SetDisplay {
        id: frame,
        display: flex(),
    }]);
    let boxes = |f: &Fixture| {
        let res = Resolved::rebuild(&f.doc);
        [a, x, outer, after].map(|id| res.world_bounds(id).unwrap())
    };
    let before = boxes(&f);
    let p = f.run(&only(&[Rule::OneLayerGroups]));
    assert_eq!(kept(&p, Rule::OneLayerGroups, inner), None, "not kept");
    assert!(!f.doc.contains(inner), "ungrouped");
    assert_eq!(f.doc.get(a).unwrap().parent(), Some(outer));
    assert_eq!(boxes(&f), before, "nothing moves");
}
