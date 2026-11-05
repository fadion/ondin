//! Code export's interaction states: a set whose property ties values to
//! browser states writes each state as a diff against rest under its
//! pseudo-class, forced by the state prop, and a state that does not line up
//! with rest's tree falls back to the prop and is reported.

use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::op::GeometryPatch;
use ondin_core::peniko::Color;
use ondin_core::variant::{self, RootElement, VariantProp, VariantSet};
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
/// the root; Large · Pressed puts Label under Bg. State ties Hover and Pressed, and an
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
    // Tying Hover ties Pressed by its name too.
    let tie = variant::tie_state(&doc, set, 1, 1, Some(variant::Interaction::Hover));
    doc.apply(&tie.unwrap()).unwrap();

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
        tsx.contains(
            "type=\"button\" data-state={state === \"Default\" ? undefined : state} disabled={disabled}>"
        ),
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
        vue.contains(
            ":data-state=\"state === 'Default' ? undefined : state\" :disabled=\"disabled\""
        ),
        "{vue}"
    );
    assert!(vue.contains("<button class="), "{vue}");
    assert!(vue.contains("    state: undefined,\n"), "{vue}");
    assert!(vue.contains("    disabled: false,\n"), "{vue}");
    let style = &vue[vue.find("<style scoped>").expect(&vue)..];
    assert!(style.contains(":hover:not([data-state])"), "{style}");
}

/// A focus state that changes only a layer inside still turns the browser's
/// focus ring off on the root: the state draws its own.
///
/// flip: `states::write` adding `outline: none` only to a root that changed →
/// fails at "the root's focus rule".
#[test]
fn a_focus_state_always_turns_the_browsers_ring_off() {
    let mut f = fixture();
    let tx = variant::add_value(&f.doc, f.set, 1, "Focus").unwrap();
    f.doc.apply(&tx).unwrap();
    let rest = variant::find_variant(&f.doc, f.set, &["Small".into(), "Default".into()]).unwrap();
    let mut ids = IdSource::new(0x58);
    let res = Resolved::rebuild(&f.doc);
    let (tx, focus) = variant::add_variant(&f.doc, &res, &mut ids, f.set, Some(rest)).unwrap();
    f.doc.apply(&tx).unwrap();
    f.doc
        .apply(&variant::set_values(&f.doc, focus, vec!["Small".into(), "Focus".into()]).unwrap())
        .unwrap();
    let bg = f.doc.get(focus).unwrap().children()[0];
    f.doc
        .apply(&Transaction(vec![paint(bg, (240, 200, 0))]))
        .unwrap();
    f.doc
        .apply(
            &variant::tie_state(
                &f.doc,
                f.set,
                1,
                3,
                Some(variant::Interaction::FocusVisible),
            )
            .unwrap(),
        )
        .unwrap();
    let res = Resolved::rebuild(&f.doc);
    let out = Target::React.export(&f.doc, &res, &[f.inst]);
    let css = file(&out, "components/Button.module.css");
    assert!(
        css.contains(":focus-visible:not([data-state]),\n")
            && css.contains("[data-state=\"Focus\"] {\n  outline: none;\n}"),
        "the root's focus rule: {css}"
    );
    assert!(css.contains("#f0c800"), "and the layer's: {css}");
}

/// A *Chip* set of one property, *State* (Default, Hover), and an instance of
/// it: Default holds a red *Bg* and a black star *Icon*, Hover is made from it
/// by *Add variant*. `edit` runs on the doc, Default and Hover before Hover is
/// tied.
fn chip(edit: impl FnOnce(&mut Document, NodeId, NodeId)) -> (Document, NodeId) {
    let mut ids = IdSource::new(0x5A);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (_, rest) = chip_set(&mut doc, &mut ids, "Chip", 0.0, edit);
    let inst = instance_of(&mut doc, &mut ids, rest);
    (doc, inst)
}

/// An instance of `main` at the top of the page, `doc` left holding it.
fn instance_of(doc: &mut Document, ids: &mut IdSource, main: NodeId) -> NodeId {
    let (tx, made) = ondin_core::insert_subtrees(
        doc,
        ids,
        &[Placement {
            nodes: doc.capture_subtree(main).unwrap(),
            parent: doc.first_page(),
            index: None,
        }],
        Default::default(),
    );
    doc.apply(&tx).unwrap();
    made[0]
}

/// `chip`'s set, named `name` and placed at `x` on the page: the set and its
/// rest variant.
fn chip_set(
    doc: &mut Document,
    ids: &mut IdSource,
    name: &str,
    x: f64,
    edit: impl FnOnce(&mut Document, NodeId, NodeId),
) -> (NodeId, NodeId) {
    let root = doc.first_page();
    let index = doc.get(root).unwrap().children().len();
    let [set, rest, bg, icon] = [(); 4].map(|_| ids.mint());
    doc.apply(&Transaction(vec![
        create(set, root, index, frame(300.0, 200.0), (x, 0.0), name),
        create(rest, set, 0, frame(80.0, 30.0), (10.0, 10.0), "Default"),
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
        create(
            icon,
            rest,
            1,
            NodeKind::Star {
                size: Size::new(10.0, 10.0),
                points: 5,
                inner_ratio: 0.5,
                corner_radius: 0.0,
                corner_smoothing: 0.0,
            },
            (4.0, 4.0),
            "Icon",
        ),
        paint(bg, (200, 30, 40)),
        paint(icon, (0, 0, 0)),
        Operation::SetComponent {
            id: rest,
            component: true,
        },
        Operation::SetVariantSet {
            id: set,
            set: Some(VariantSet {
                props: vec![VariantProp::new(
                    "State",
                    vec!["Default".into(), "Hover".into()],
                )],
                ..Default::default()
            }),
        },
        Operation::SetVariant {
            id: rest,
            values: vec!["Default".into()],
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(doc);
    let (tx, hover) = variant::add_variant(doc, &res, ids, set, Some(rest)).unwrap();
    doc.apply(&tx).unwrap();
    doc.apply(&variant::set_values(doc, hover, vec!["Hover".into()]).unwrap())
        .unwrap();
    edit(doc, rest, hover);
    let tie = variant::tie_state(doc, set, 0, 1, Some(variant::Interaction::Hover));
    doc.apply(&tie.unwrap()).unwrap();
    assert_eq!(
        variant::rest_value(doc, set, 0).as_deref(),
        Some("Default"),
        "the fixture's rest"
    );
    (set, rest)
}

/// A state whose star icon is recoloured draws a picture rest's cannot become
/// by declarations: rest's is hidden and the state's shown, rather than the
/// whole state falling back to the prop.
///
/// flip: `states::paired` accepting a counterpart `same_own` refuses → fails at
/// "no fallback".
#[test]
fn a_state_swaps_a_picture_that_differs() {
    let (doc, inst) = chip(|doc, _, hover| {
        let icon = doc.get(hover).unwrap().children()[1];
        doc.apply(&Transaction(vec![paint(icon, (255, 255, 255))]))
            .unwrap();
    });
    let res = Resolved::rebuild(&doc);
    let out = Target::React.export(&doc, &res, &[inst]);
    assert!(
        out.approximated.is_empty(),
        "no fallback: {:?}",
        out.approximated
    );
    let tsx = file(&out, "components/Chip.tsx");
    assert_eq!(tsx.matches("<svg").count(), 2, "both pictures: {tsx}");
    assert!(tsx.contains("fill=\"#ffffff\""), "{tsx}");
    let css = file(&out, "components/Chip.module.css");
    let hover = &css[css.find("[data-state=\"Hover\"]").expect(&css)..];
    assert!(
        hover.contains("display: none;") && hover.contains("display: revert;"),
        "rest's hidden and the state's shown: {css}"
    );
}

/// The same picture in two variants at two places on the canvas writes the same
/// markup, gradient ids and all, so a state that only recolours the background
/// pairs the icon rather than swapping it.
///
/// flip: `Walk::picture` writing the SVG in world space (`svg_linking`), or
/// keeping the node-named def ids (no `content_ids`) → each fails at "one
/// picture".
#[test]
fn the_same_picture_pairs_across_variants() {
    use ondin_core::peniko::{ColorStop, Gradient};
    let ramp = || {
        Brush::Gradient(ondin_core::GradientBrush {
            gradient: Gradient {
                stops: [
                    ColorStop {
                        offset: 0.0,
                        color: Color::from_rgba8(230, 60, 40, 255).into(),
                    },
                    ColorStop {
                        offset: 1.0,
                        color: Color::from_rgba8(40, 90, 220, 255).into(),
                    },
                ]
                .as_slice()
                .into(),
                ..Default::default()
            },
            transform: Affine::IDENTITY,
            opacity: 1.0,
        })
    };
    let (doc, inst) = chip(|doc, rest, hover| {
        let kids = |v: NodeId| doc.get(v).unwrap().children().to_vec();
        let (r, h) = (kids(rest), kids(hover));
        let fill = |id| Operation::SetFills {
            id,
            fills: keyed_by_position([Fill {
                brush: ramp().into(),
                visible: true,
            }]),
        };
        let ops = vec![fill(r[1]), fill(h[1]), paint(h[0], (0, 90, 200))];
        doc.apply(&Transaction(ops)).unwrap();
    });
    let res = Resolved::rebuild(&doc);
    let out = Target::React.export(&doc, &res, &[inst]);
    let tsx = file(&out, "components/Chip.tsx");
    assert_eq!(tsx.matches("<svg").count(), 1, "one picture: {tsx}");
    assert!(tsx.contains("viewBox=\"0 0 "), "{tsx}");
    assert!(
        tsx.contains("url(#grad-"),
        "the gradient's id is written: {tsx}"
    );
    let css = file(&out, "components/Chip.module.css");
    assert!(!css.contains("display"), "nothing swapped: {css}");
    assert!(out.approximated.is_empty(), "{:?}", out.approximated);
}

/// A state pairs with rest by the lineage key either recorded: here rest is the
/// copy, and the state's renamed background is recoloured, not swapped.
///
/// flip: `merge::pair` matching only a key the other tree's layer recorded →
/// fails at "a recolour".
#[test]
fn a_state_pairs_by_the_key_rest_recorded() {
    let (doc, inst) = chip(|doc, rest, hover| {
        let bg = doc.get(rest).unwrap().children()[0];
        doc.apply(&Transaction(vec![
            // Hover is the original from here: Default is its copy, at the top.
            Operation::SetVariant {
                id: rest,
                values: vec!["Hover".into()],
            },
            Operation::SetVariant {
                id: hover,
                values: vec!["Default".into()],
            },
            Operation::SetTransform {
                id: hover,
                transform: Affine::translate((0.0, -100.0)),
            },
            paint(bg, (0, 90, 200)),
            Operation::SetName {
                id: bg,
                name: "Background".into(),
            },
        ]))
        .unwrap();
    });
    let res = Resolved::rebuild(&doc);
    let out = Target::React.export(&doc, &res, &[inst]);
    let css = file(&out, "components/Chip.module.css");
    assert!(
        css.contains(
            "[data-state=\"Hover\"] .bg {\n  background: linear-gradient(#005ac8, #005ac8);\n}"
        ),
        "a recolour: {css}"
    );
}

/// The page drawn as plain HTML through the components reads the state prop
/// as the frameworks do: a use of Large draws Large's class, a forced state
/// writes its value, and the sheet holds the states' rules.
///
/// flip: `html::holds` comparing the encoded prop name as passed → fails at
/// "Large's class".
#[test]
fn html_of_components_reads_the_state_prop() {
    let mut f = fixture();
    let hover = variant::find_variant(&f.doc, f.set, &["Small".into(), "Hover".into()]).unwrap();
    let mut ids = IdSource::new(0x59);
    let (tx, made) = ondin_core::insert_subtrees(
        &f.doc,
        &mut ids,
        &[Placement {
            nodes: f.doc.capture_subtree(hover).unwrap(),
            parent: f.doc.first_page(),
            index: None,
        }],
        Default::default(),
    );
    f.doc.apply(&tx).unwrap();
    let res = Resolved::rebuild(&f.doc);
    let out = ondin_export::code::html::html_of_components(&f.doc, &res, &[f.inst, made[0]]);
    let html = file(&out, "index.html");
    assert!(
        html.contains("<button class=\"Button--large-default "),
        "Large's class: {html}"
    );
    assert!(html.contains("data-state=\"Hover\""), "{html}");
    assert!(!html.contains(":data-state"), "no Vue in HTML: {html}");
    let css = file(&out, "styles.css");
    assert!(css.contains("[data-state=\"Hover\"]"), "{css}");
}

/// A component property whose code name is `rest` leaves the name the root's
/// other props gather under to another.
///
/// flip: `walk` naming the spread `rest` unconditionally → fails at "two
/// bindings".
#[test]
fn a_prop_named_rest_keeps_its_name() {
    let mut f = fixture();
    let mut vs = f.doc.get(f.set).unwrap().set().unwrap().clone();
    vs.props[0].name = "Rest".into();
    f.doc
        .apply(&Transaction(vec![Operation::SetVariantSet {
            id: f.set,
            set: Some(vs),
        }]))
        .unwrap();
    let res = Resolved::rebuild(&f.doc);
    let out = Target::React.export(&f.doc, &res, &[f.inst]);
    let tsx = file(&out, "components/Button.tsx");
    assert!(
        tsx.contains("rest = \"Small\", state, disabled = false, ...rest2 }"),
        "two bindings: {tsx}"
    );
    assert!(tsx.contains("<button {...rest2} "), "{tsx}");
}

/// A state whose rest variant did not merge with the others says so, rather
/// than that no variant is at rest.
///
/// flip: `states::write` reporting every missing rest as absent → fails at the
/// one assertion.
#[test]
fn a_state_names_a_rest_that_did_not_merge() {
    let mut f = fixture();
    let large = variant::find_variant(&f.doc, f.set, &["Large".into(), "Default".into()]).unwrap();
    let label = f.doc.get(large).unwrap().children()[1];
    let mut tx = Transaction(vec![Operation::SetText {
        id: label,
        content: "Stop".into(),
        spans: Default::default(),
        para_spans: Default::default(),
    }]);
    ondin_core::propagate::owed(&f.doc, &mut tx, &mut IdSource::new(0x5B));
    f.doc.apply(&tx).unwrap();
    let res = Resolved::rebuild(&f.doc);
    let out = Target::React.export(&f.doc, &res, &[f.inst]);
    assert!(
        out.approximated
            .iter()
            .any(|a| a
                .contains("Large, Hover: Large, Default does not merge with the other variants")),
        "{:?}",
        out.approximated
    );
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

/// `set`'s variant at `values`, made from `from` by *Add variant*.
fn variant_of(
    doc: &mut Document,
    ids: &mut IdSource,
    set: NodeId,
    from: NodeId,
    values: &[&str],
) -> NodeId {
    let res = Resolved::rebuild(doc);
    let (tx, v) = variant::add_variant(doc, &res, ids, set, Some(from)).unwrap();
    doc.apply(&tx).unwrap();
    let values = values.iter().map(|v| v.to_string()).collect();
    doc.apply(&variant::set_values(doc, v, values).unwrap())
        .unwrap();
    v
}

/// The chip's set with its root written as `element`.
fn set_element(doc: &mut Document, set: NodeId, element: RootElement) {
    let mut vs = doc.get(set).unwrap().set().unwrap().clone();
    vs.element = element;
    doc.apply(&Transaction(vec![Operation::SetVariantSet {
        id: set,
        set: Some(vs),
    }]))
    .unwrap();
}

/// A Disabled tie on a Plain root: a focusable `div` whose `aria-disabled` the
/// prop or a forced Disabled sets, a rule reading it, and a use forced Disabled
/// drawing it in plain HTML.
///
/// flip: `Live::drawn_attrs` ignoring a forced Disabled → fails at "the forced
/// use".
#[test]
fn a_disabled_tie_on_a_plain_root_reads_aria_disabled() {
    let mut ids = IdSource::new(0x5C);
    let mut doc = Document::new(ids.mint());
    let (set, rest) = chip_set(&mut doc, &mut ids, "Chip", 0.0, |_, _, _| {});
    doc.apply(&variant::add_value(&doc, set, 0, "Disabled").unwrap())
        .unwrap();
    let off = variant_of(&mut doc, &mut ids, set, rest, &["Disabled"]);
    let bg = doc.get(off).unwrap().children()[0];
    doc.apply(&Transaction(vec![paint(bg, (150, 150, 150))]))
        .unwrap();
    let tie = variant::tie_state(&doc, set, 0, 2, Some(variant::Interaction::Disabled));
    doc.apply(&tie.unwrap()).unwrap();
    set_element(&mut doc, set, RootElement::Plain);
    let inst = instance_of(&mut doc, &mut ids, rest);
    let forced = instance_of(&mut doc, &mut ids, off);
    let res = Resolved::rebuild(&doc);
    let out = Target::React.export(&doc, &res, &[inst]);
    let tsx = file(&out, "components/Chip.tsx");
    assert!(
        tsx.contains("<div {...rest} className=") && tsx.contains(" tabIndex={0} data-state="),
        "a focusable div, in React's spelling: {tsx}"
    );
    assert!(
        tsx.contains("aria-disabled={disabled || state === \"Disabled\" || undefined}"),
        "{tsx}"
    );
    let css = file(&out, "components/Chip.module.css");
    let rule = &css[css.find("[aria-disabled=\"true\"] .").expect(&css)..];
    assert!(rule[..rule.find('}').unwrap()].contains("#969696"), "{css}");

    let out = ondin_export::code::html::html_of_components(&doc, &res, &[inst, forced]);
    let html = file(&out, "index.html");
    assert!(
        html.contains("data-state=\"Disabled\" aria-disabled=\"true\""),
        "the forced use: {html}"
    );
    assert_eq!(html.matches("aria-disabled").count(), 1, "{html}");
}

/// A Link root is an `<a>`; a set whose variant holds a use of an interactive
/// component is written as Plain instead, since interactive content cannot
/// nest, and says so.
///
/// flip: `Walk::component` skipping the nesting check → fails at "written as
/// Plain".
#[test]
fn a_root_holding_an_interactive_use_is_written_plain() {
    let mut ids = IdSource::new(0x5D);
    let mut doc = Document::new(ids.mint());
    let (_, chip) = chip_set(&mut doc, &mut ids, "Chip", 0.0, |_, _, _| {});
    let mut inner = IdSource::new(0x5E);
    let (row, rest) = chip_set(&mut doc, &mut ids, "Row", 400.0, |doc, rest, hover| {
        for v in [rest, hover] {
            let (tx, _) = ondin_core::insert_subtrees(
                doc,
                &mut inner,
                &[Placement {
                    nodes: doc.capture_subtree(chip).unwrap(),
                    parent: v,
                    index: None,
                }],
                Default::default(),
            );
            doc.apply(&tx).unwrap();
        }
    });
    set_element(&mut doc, row, RootElement::Link);
    let inst = instance_of(&mut doc, &mut ids, rest);
    let res = Resolved::rebuild(&doc);
    let out = Target::React.export(&doc, &res, &[inst]);
    assert!(out.inlined.is_empty(), "{:?}", out.inlined);
    assert!(
        out.approximated
            .iter()
            .any(|a| a == "Row: it holds a Chip, which is interactive, so it is written as Plain"),
        "written as Plain: {:?}",
        out.approximated
    );
    let tsx = file(&out, "components/Row.tsx");
    assert!(
        tsx.contains("<div {...rest} className=") && tsx.contains(" tabIndex={0} "),
        "{tsx}"
    );
    assert!(file(&out, "components/Chip.tsx").contains("<button "));

    // Without the use inside, the Link stands.
    let mut ids = IdSource::new(0x5F);
    let mut doc = Document::new(ids.mint());
    let (row, rest) = chip_set(&mut doc, &mut ids, "Row", 0.0, |_, _, _| {});
    set_element(&mut doc, row, RootElement::Link);
    let inst = instance_of(&mut doc, &mut ids, rest);
    let res = Resolved::rebuild(&doc);
    let out = Target::React.export(&doc, &res, &[inst]);
    assert!(
        file(&out, "components/Row.tsx").contains("<a {...rest} className="),
        "an anchor"
    );
}

/// Two variants that share a root class but differ in a state get a root class
/// each, so each state's rule reaches its own: *Tone* Primary and Ghost look
/// alike at rest, and hover blue and green.
///
/// flip: `states::write` not calling `split_roots` → fails at "two roots".
#[test]
fn variants_differing_only_in_a_state_split_their_root_class() {
    let (mut doc, inst) = chip(|doc, _, hover| {
        let bg = doc.get(hover).unwrap().children()[0];
        doc.apply(&Transaction(vec![paint(bg, (0, 90, 200))]))
            .unwrap();
    });
    let mut ids = IdSource::new(0x60);
    let set = doc.get(inst).unwrap().link().unwrap();
    let set = doc.get(set).unwrap().parent().unwrap();
    doc.apply(&variant::add_property(&doc, set, "Tone", "Primary").unwrap())
        .unwrap();
    doc.apply(&variant::add_value(&doc, set, 1, "Ghost").unwrap())
        .unwrap();
    let find = |doc: &Document, v: [&str; 2]| {
        variant::find_variant(doc, set, &v.map(String::from)).unwrap()
    };
    let rest = find(&doc, ["Default", "Primary"]);
    let hover = find(&doc, ["Hover", "Primary"]);
    variant_of(&mut doc, &mut ids, set, rest, &["Default", "Ghost"]);
    let ghost = variant_of(&mut doc, &mut ids, set, hover, &["Hover", "Ghost"]);
    let bg = doc.get(ghost).unwrap().children()[0];
    doc.apply(&Transaction(vec![paint(bg, (0, 160, 60))]))
        .unwrap();
    let res = Resolved::rebuild(&doc);
    let out = Target::React.export(&doc, &res, &[inst]);
    assert!(out.approximated.is_empty(), "{:?}", out.approximated);
    let css = file(&out, "components/Chip.module.css");
    let roots: std::collections::BTreeSet<&str> = css
        .lines()
        .filter(|l| l.contains(":hover:not("))
        .filter_map(|l| l.strip_prefix('.')?.split(':').next())
        .collect();
    assert_eq!(roots.len(), 2, "two roots: {roots:?}\n{css}");
    assert!(css.contains("#005ac8") && css.contains("#00a03c"), "{css}");
}
