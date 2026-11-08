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
