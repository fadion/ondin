//! Code export's interaction states: a set whose property is marked as the
//! interaction state writes each state as a diff against rest under its
//! pseudo-class, forced by the state prop, and a state that does not line up
//! with rest's tree falls back to the prop and is reported.

use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::op::GeometryPatch;
use ondin_core::peniko::Color;
use ondin_core::variant::{self, VariantProp, VariantSet};
use ondin_core::{
    Brush, Document, Fill, IdSource, NodeId, NodeKind, Operation, Placement, Resolved, TextSizing,
    Transaction, keyed_by_position,
};
use ondin_export::code::{CodeExport, Target};

fn text(content: &str) -> NodeKind {
    NodeKind::Text {
        content: content.into(),
        style: Box::default(),
        spans: Default::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing: TextSizing::Auto,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    }
}

fn frame(w: f64, h: f64) -> NodeKind {
    NodeKind::Artboard {
        size: Size::new(w, h),
        corner_radii: RoundedRectRadii::default(),
        corner_smoothing: 0.0,
    }
}

fn create(
    id: NodeId,
    parent: NodeId,
    index: usize,
    kind: NodeKind,
    at: (f64, f64),
    name: &str,
) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index,
        kind,
        transform: Some(Affine::translate(at)),
        name: Some(name.into()),
    }
}

fn paint(id: NodeId, rgb: (u8, u8, u8)) -> Operation {
    Operation::SetFills {
        id,
        fills: keyed_by_position([Fill {
            brush: Brush::Solid(Color::from_rgba8(rgb.0, rgb.1, rgb.2, 255)).into(),
            visible: true,
        }]),
    }
}

fn file(out: &CodeExport, path: &str) -> String {
    let bytes = out.file(path).unwrap_or_else(|| {
        let paths: Vec<&str> = out.files.iter().map(|f| f.path.as_str()).collect();
        panic!("no {path} among {paths:?}")
    });
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// A *Button* set, *Size* (Small, Large) × *State* (Default, Hover, Pressed),
/// each variant a *Bg* rect under a *Label*, made by *Add variant* from Small ·
/// Default so their layers share lineage keys. Hover paints Bg blue, and Small ·
/// Hover's Bg is renamed *Background*; Small · Pressed paints it green and grows
/// the root; Large · Pressed puts Label under Bg. State is marked, and an
/// instance of Large · Default sits on the page.
struct Fixture {
    doc: Document,
    set: NodeId,
    inst: NodeId,
}

fn fixture() -> Fixture {
    let mut ids = IdSource::new(0x57);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let [set, rest, bg, label] = [(); 4].map(|_| ids.mint());
    doc.apply(&Transaction(vec![
        create(set, root, 0, frame(600.0, 600.0), (0.0, 0.0), "Button"),
        create(
            rest,
            set,
            0,
            frame(80.0, 30.0),
            (10.0, 10.0),
            "Small, Default",
        ),
        create(
            bg,
            rest,
            0,
            NodeKind::Rect {
                size: Size::new(80.0, 30.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            (0.0, 0.0),
            "Bg",
        ),
        create(label, rest, 1, text("Go"), (8.0, 6.0), "Label"),
        paint(bg, (200, 30, 40)),
        Operation::SetComponent {
            id: rest,
            component: true,
        },
        Operation::SetVariantSet {
            id: set,
            set: Some(VariantSet {
                props: vec![
                    VariantProp::new("Size", vec!["Small".into(), "Large".into()]),
                    VariantProp::new(
                        "State",
                        vec!["Default".into(), "Hover".into(), "Pressed".into()],
                    ),
                ],
                ..Default::default()
            }),
        },
        Operation::SetVariant {
            id: rest,
            values: vec!["Small".into(), "Default".into()],
        },
    ]))
    .expect("a set of one variant");
    let mut variants = std::collections::HashMap::new();
    for values in [
        ["Small", "Hover"],
        ["Small", "Pressed"],
        ["Large", "Default"],
        ["Large", "Hover"],
        ["Large", "Pressed"],
    ] {
        let res = Resolved::rebuild(&doc);
        let (tx, v) = variant::add_variant(&doc, &res, &mut ids, set, Some(rest)).unwrap();
        doc.apply(&tx).unwrap();
        let values: Vec<String> = values.map(String::from).to_vec();
        doc.apply(&variant::set_values(&doc, v, values.clone()).unwrap())
            .unwrap();
        variants.insert(values.join(" "), v);
    }
    let kids = |doc: &Document, v: &str| doc.get(variants[v]).unwrap().children().to_vec();
    let mut ops = vec![];
    for v in ["Large Default", "Large Hover", "Large Pressed"] {
        ops.push(Operation::SetGeometry {
            id: variants[v],
            geometry: GeometryPatch::Size(Size::new(120.0, 40.0)),
        });
    }
    ops.push(paint(kids(&doc, "Small Hover")[0], (0, 90, 200)));
    ops.push(Operation::SetName {
        id: kids(&doc, "Small Hover")[0],
        name: "Background".into(),
    });
    ops.push(paint(kids(&doc, "Large Hover")[0], (0, 90, 200)));
    ops.push(paint(kids(&doc, "Small Pressed")[0], (0, 160, 60)));
    ops.push(Operation::SetGeometry {
        id: variants["Small Pressed"],
        geometry: GeometryPatch::Size(Size::new(82.0, 30.0)),
    });
    ops.push(Operation::Reorder {
        id: kids(&doc, "Large Pressed")[1],
        index: 0,
    });
    doc.apply(&Transaction(ops)).unwrap();
    doc.apply(&variant::mark_interaction(&doc, set, 1, true).unwrap())
        .unwrap();

    let (tx, made) = ondin_core::insert_subtrees(
        &doc,
        &mut ids,
        &[Placement {
            nodes: doc.capture_subtree(variants["Large Default"]).unwrap(),
            parent: root,
            index: None,
        }],
        Default::default(),
    );
    doc.apply(&tx).unwrap();
    Fixture {
        doc,
        set,
        inst: made[0],
    }
}

/// The fixture is what it says: rest is Small · Default, and the ties are the
/// names' guesses.
#[test]
fn the_fixture_marks_its_state_property() {
    let f = fixture();
    assert_eq!(
        variant::rest_value(&f.doc, f.set, 1).as_deref(),
        Some("Default")
    );
    let vs = f.doc.get(f.set).unwrap().set().unwrap();
    assert_eq!(
        vs.props[1].state_of("Hover"),
        Some(variant::Interaction::Hover)
    );
    assert_eq!(
        vs.props[1].state_of("Pressed"),
        Some(variant::Interaction::Active)
    );
    assert!(f.doc.get(f.inst).unwrap().link().is_some());
}

/// React writes the root as a button whose live states are CSS rules: each a
/// diff against rest under its pseudo-class, also forced by `data-state`; the
/// state prop has no default, and what the component does not name reaches the
/// root.
///
/// flip: `states::write` treating a state as a plain variant → fails at "no
/// tree for Small · Hover".
#[test]
fn react_writes_states_as_pseudo_class_rules() {
    let f = fixture();
    let res = Resolved::rebuild(&f.doc);
    let out = Target::React.export(&f.doc, &res, &[f.inst]);
    let tsx = file(&out, "components/Button.tsx");
    let css = file(&out, "components/Button.module.css");
    assert!(
        tsx.contains("import type { ComponentPropsWithoutRef } from \"react\";"),
        "{tsx}"
    );
    assert!(
        tsx.contains("extends Omit<ComponentPropsWithoutRef<\"button\">, \"className\" | \"size\" | \"state\" | \"disabled\">"),
        "{tsx}"
    );
    assert!(
        tsx.contains("size = \"Small\", state, disabled = false, ...rest }"),
        "{tsx}"
    );
    assert!(tsx.contains("<button {...rest} className="), "{tsx}");
    assert!(
        tsx.contains("type=\"button\" data-state={state} disabled={disabled}>"),
        "{tsx}"
    );
    assert!(
        !tsx.contains("state === \"Hover\""),
        "no tree for Small · Hover: {tsx}"
    );
    // A forced state draws from rest's tree, so it never chooses the size.
    assert!(
        tsx.contains("className={`${size === \"Large\" ? styles[\"large-default\"]"),
        "a forced state keeps Large's class: {tsx}"
    );
    assert!(
        css.contains(":hover:not([data-state]):not(:disabled) ."),
        "{css}"
    );
    assert!(css.contains("[data-state=\"Hover\"] ."), "{css}");
    assert!(
        css.contains(":active:not([data-state]):not(:disabled) ."),
        "{css}"
    );
    assert!(css.contains("[data-state=\"Pressed\"] ."), "{css}");
    // The root's reset, before its own declarations.
    assert!(css.contains("  border: 0;\n"), "{css}");
    // Hover's blue reaches the background, renamed or not.
    let hover = css.find(":hover").expect("a hover rule");
    assert!(css[hover..].contains("#005ac8"), "{css}");

    let app = file(&out, "App.tsx");
    assert!(app.contains("<Button className="), "{app}");
    assert!(!app.contains("state="), "a use at rest passes none: {app}");
}

/// A state whose layers are reordered keeps a tree of its own behind the prop
/// alone, is reported, and the other states still merge; a state that resizes
/// the root is reported and drawn at rest's size.
///
/// flip: `states::check` accepting any order → fails at "Large · Pressed is
/// apart".
#[test]
fn a_reordered_state_falls_back_to_the_prop() {
    let f = fixture();
    let res = Resolved::rebuild(&f.doc);
    let out = Target::React.export(&f.doc, &res, &[f.inst]);
    let tsx = file(&out, "components/Button.tsx");
    assert!(
        tsx.contains("if (size === \"Large\" && state === \"Pressed\")"),
        "Large · Pressed is apart: {tsx}"
    );
    assert_eq!(tsx.matches("  if (").count(), 1, "{tsx}");
    assert!(
        out.approximated
            .iter()
            .any(|a| a.contains("Large, Pressed") && a.contains("“Label” moves before “Bg”")),
        "{:?}",
        out.approximated
    );
    assert!(
        out.approximated
            .iter()
            .any(|a| a.contains("Small, Pressed") && a.contains("2 px larger")),
        "{:?}",
        out.approximated
    );
    assert_eq!(out.states.len(), 1, "{:?}", out.states);
    assert!(
        out.states[0].contains("Hover and Pressed written as :hover and :active"),
        "{:?}",
        out.states
    );
    let report = file(&out, "report.txt");
    assert!(
        report.starts_with("Interaction states:\n- Button: "),
        "{report}"
    );
}

/// Vue binds the forced state and the behaviour of `disabled`, and its scoped
/// sheet holds the same rules.
#[test]
fn vue_writes_the_same_states() {
    let f = fixture();
    let res = Resolved::rebuild(&f.doc);
    let out = Target::Vue.export(&f.doc, &res, &[f.inst]);
    let vue = file(&out, "components/Button.vue");
    assert!(
        vue.contains(":data-state=\"state\" :disabled=\"disabled\""),
        "{vue}"
    );
    assert!(vue.contains("<button class="), "{vue}");
    assert!(vue.contains("    state: undefined,\n"), "{vue}");
    assert!(vue.contains("    disabled: false,\n"), "{vue}");
    let style = &vue[vue.find("<style scoped>").expect(&vue)..];
    assert!(style.contains(":hover:not([data-state])"), "{style}");
}

/// Plain HTML has no components and draws the page as it is; it says so.
#[test]
fn plain_html_says_it_writes_no_states() {
    let f = fixture();
    let res = Resolved::rebuild(&f.doc);
    let out = Target::Html.export(&f.doc, &res, &[f.inst]);
    assert_eq!(out.states.len(), 1, "{:?}", out.states);
    assert!(out.states[0].contains("plain HTML"), "{:?}", out.states);
}
