//! Code export's components: a main written once as a React or Vue component with
//! its properties as props, and each instance as a use of it — or written out as
//! its layers, and reported, when it differs from its main in more than its
//! properties.

use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::peniko::Color;
use ondin_core::variant::{self, PropKind, PropValue};
use ondin_core::{
    Brush, Document, Fill, IdSource, NodeId, NodeKind, Operation, Resolved, TextSizing,
    Transaction, keyed_by_position,
};
use ondin_export::code::{CodeExport, react::react_of, vue::vue_of};

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

fn create(
    id: NodeId,
    (parent, index): (NodeId, usize),
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

/// The layers of a card — a frame holding an icon and a title — under `parent`,
/// linked to `main`'s when it is given.
fn card(
    doc: &mut Document,
    ids: &mut IdSource,
    parent: NodeId,
    at: (f64, f64),
    main: Option<[NodeId; 3]>,
) -> [NodeId; 3] {
    let [frame, icon, title] = [(); 3].map(|_| ids.mint());
    let index = doc.get(parent).map_or(0, |p| p.children().len());
    let mut ops = vec![
        create(
            frame,
            (parent, index),
            NodeKind::Artboard {
                size: Size::new(200.0, 80.0),
            },
            at,
            "Card",
        ),
        create(
            icon,
            (frame, 0),
            NodeKind::Rect {
                size: Size::new(20.0, 20.0),
                corner_radii: RoundedRectRadii::default(),
            },
            (10.0, 10.0),
            "Icon",
        ),
        create(title, (frame, 1), text("Hello"), (40.0, 10.0), "Title"),
        Operation::SetFills {
            id: icon,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(Color::from_rgba8(200, 30, 40, 255)).into(),
                visible: true,
            }]),
        },
    ];
    match main {
        None => ops.push(Operation::SetComponent {
            id: frame,
            component: true,
        }),
        Some(m) => {
            for (copy, source) in [frame, icon, title].into_iter().zip(m) {
                ops.push(Operation::SetLink {
                    id: copy,
                    link: Some(source),
                });
            }
        }
    }
    doc.apply(&Transaction(ops)).expect("a card");
    [frame, icon, title]
}

struct Fixture {
    doc: Document,
    /// The instance that sets its properties and nothing else.
    clean: NodeId,
    /// The instance with its icon recoloured, which no property says.
    dirty: NodeId,
    ids: IdSource,
    /// The main's frame, icon and title.
    main: [NodeId; 3],
}

/// A main card with *Show icon* and *Title* properties, one instance that hides
/// its icon and retitles itself, and one that recolours its icon.
fn fixture() -> Fixture {
    let mut ids = IdSource::new(0xC0);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let main = card(&mut doc, &mut ids, root, (0.0, 0.0), None);
    for (name, kind, bound) in [
        ("Show icon", PropKind::Boolean, main[1]),
        ("Title", PropKind::Text, main[2]),
    ] {
        let (tx, _) =
            variant::define(&doc, &mut ids, main[0], name, kind, vec![bound]).expect("a property");
        doc.apply(&tx).unwrap();
    }
    let clean = card(&mut doc, &mut ids, root, (0.0, 100.0), Some(main))[0];
    let dirty = card(&mut doc, &mut ids, root, (0.0, 200.0), Some(main));
    let props = variant::instance_properties(&doc, clean);
    for (p, v) in props
        .iter()
        .zip([PropValue::Boolean(false), PropValue::Text("World".into())])
    {
        let ops = variant::set_property(&doc, &[clean], p, &v);
        doc.apply(&Transaction(ops)).unwrap();
    }
    doc.apply(&Transaction(vec![Operation::SetFills {
        id: dirty[1],
        fills: keyed_by_position([Fill {
            brush: Brush::Solid(Color::from_rgba8(0, 90, 200, 255)).into(),
            visible: true,
        }]),
    }]))
    .unwrap();
    Fixture {
        doc,
        clean,
        dirty: dirty[0],
        ids,
        main,
    }
}

fn file(out: &CodeExport, path: &str) -> String {
    let bytes = out.file(path).unwrap_or_else(|| {
        let paths: Vec<&str> = out.files.iter().map(|f| f.path.as_str()).collect();
        panic!("no {path} among {paths:?}")
    });
    String::from_utf8(bytes.to_vec()).unwrap()
}

/// The fixture's instances are what the test means them to be: one differing
/// from its main only in its two properties, one in a fill.
#[test]
fn the_fixture_holds_a_clean_and_a_dirty_instance() {
    let f = fixture();
    for (root, overrides) in [(f.clean, 2), (f.dirty, 1)] {
        let n: usize = ondin_core::reset::scope_nodes(&f.doc, root)
            .iter()
            .map(|id| ondin_core::reset::overrides(&f.doc, *id).len())
            .sum();
        assert_eq!(n, overrides, "overrides on {root:?}");
    }
}

/// React: one `Card` with typed, defaulted props, used where the instance sets
/// only those; the other instance is its layers, and reported.
#[test]
fn react_writes_a_main_once_and_its_clean_instance_as_a_use() {
    let f = fixture();
    let res = Resolved::rebuild(&f.doc);
    let out = react_of(&f.doc, &res, &[f.clean, f.dirty]);

    let card = file(&out, "components/Card.tsx");
    assert!(
        card.contains("import styles from \"./Card.module.css\";"),
        "{card}"
    );
    assert!(
        card.contains("  showIcon?: boolean;\n  title?: string;"),
        "{card}"
    );
    assert!(
        card.contains(
            "export default function Card({ className, showIcon = true, title = \"Hello\" }: CardProps)"
        ),
        "{card}"
    );
    assert!(
        card.contains("<div className={`${styles.card} ${className ?? \"\"}`}>"),
        "{card}"
    );
    assert!(card.contains("{showIcon && ("), "{card}");
    assert!(card.contains(">{title}</p>"), "{card}");

    let app = file(&out, "App.tsx");
    assert!(
        app.contains("import Card from \"./components/Card\";"),
        "{app}"
    );
    assert!(
        app.contains("<Card className={styles.card} showIcon={false} title=\"World\" />"),
        "{app}"
    );
    assert_eq!(app.matches("<Card ").count(), 1, "one use: {app}");
    let css = file(&out, "App.module.css");
    assert!(
        css.contains(".card.card {"),
        "the use outranks the root: {css}"
    );

    assert_eq!(
        out.inlined,
        vec![
            "instance “Card”: inlined because of overrides; make it a property to keep the link"
                .to_string()
        ]
    );
    assert!(file(&out, "report.txt").contains("Written out as layers"));
    assert!(out.file("components/Card.module.css").is_some());
}

/// Vue: the same component as a single-file component, its props declared with
/// their defaults, the boolean as a `v-if` and the text interpolated.
#[test]
fn vue_writes_the_same_component_as_a_single_file_component() {
    let f = fixture();
    let res = Resolved::rebuild(&f.doc);
    let out = vue_of(&f.doc, &res, &[f.clean, f.dirty]);

    let card = file(&out, "components/Card.vue");
    assert!(
        card.contains("    showIcon?: boolean;\n    title?: string;"),
        "{card}"
    );
    assert!(
        card.contains("    showIcon: true,\n    title: \"Hello\","),
        "{card}"
    );
    assert!(card.contains(" v-if=\"showIcon\""), "{card}");
    assert!(card.contains(">{{ title }}</p>"), "{card}");
    assert!(card.contains("<style scoped>\n.card {"), "{card}");

    let app = file(&out, "App.vue");
    assert!(
        app.contains("import Card from \"./components/Card.vue\";"),
        "{app}"
    );
    assert!(
        app.contains("<Card class=\"card\" :showIcon=\"false\" title=\"World\" />"),
        "{app}"
    );
    assert!(app.contains(".card.card {"), "{app}");
    assert_eq!(out.inlined.len(), 1);
}

/// An exported instance whose ancestors set a mode keeps it: a use writes no
/// attributes, so the mode is on a `display: contents` element around it.
/// flip: `walk::page` pushing `data-mode` onto the use → fails at "around the use".
#[test]
fn a_use_keeps_the_mode_its_ancestors_set() {
    use ondin_core::variable::Mode;
    let mut f = fixture();
    let (light, dark) = (f.ids.mint_mode(), f.ids.mint_mode());
    let board = f.ids.mint();
    let root = f.doc.root();
    let index = f.doc.get(root).unwrap().children().len();
    f.doc
        .apply(&Transaction(vec![
            Operation::AddMode {
                mode: Mode {
                    id: light,
                    name: "Light".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddMode {
                mode: Mode {
                    id: dark,
                    name: "Dark".into(),
                },
                index: 1,
                cells: vec![],
            },
            create(
                board,
                (root, index),
                NodeKind::Artboard {
                    size: Size::new(400.0, 400.0),
                },
                (300.0, 0.0),
                "Board",
            ),
            Operation::SetMode {
                id: board,
                mode: Some(dark),
            },
        ]))
        .unwrap();
    let inside = card(&mut f.doc, &mut f.ids, board, (10.0, 10.0), Some(f.main))[0];
    let res = Resolved::rebuild(&f.doc);
    let app = file(&react_of(&f.doc, &res, &[inside]), "App.tsx");
    assert!(
        app.contains("<div className={styles.mode} data-mode=\"dark\">\n        <Card "),
        "around the use: {app}"
    );
    let css = file(&react_of(&f.doc, &res, &[inside]), "App.module.css");
    assert!(css.contains(".mode {\n  display: contents;\n}"), "{css}");
}

/// An instance whose root sets a mode is a use inside an element carrying it,
/// while its main sets none; once the main sets one of its own, which the
/// component's root carries, the instance is its layers.
/// flip: `why_inline` refusing any root mode unlike its main's → fails at "a use".
#[test]
fn an_instance_setting_a_mode_is_a_use_inside_it() {
    use ondin_core::variable::Mode;
    let mut f = fixture();
    let (light, dark) = (f.ids.mint_mode(), f.ids.mint_mode());
    f.doc
        .apply(&Transaction(vec![
            Operation::AddMode {
                mode: Mode {
                    id: light,
                    name: "Light".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddMode {
                mode: Mode {
                    id: dark,
                    name: "Dark".into(),
                },
                index: 1,
                cells: vec![],
            },
            Operation::SetMode {
                id: f.clean,
                mode: Some(dark),
            },
        ]))
        .unwrap();
    assert_eq!(f.doc.get(f.clean).unwrap().mode(), Some(dark));
    let res = Resolved::rebuild(&f.doc);
    let out = react_of(&f.doc, &res, &[f.clean]);
    let app = file(&out, "App.tsx");
    assert!(
        app.contains("<div className={styles.mode} data-mode=\"dark\">\n        <Card "),
        "a use: {app}"
    );
    assert!(out.inlined.is_empty(), "{:?}", out.inlined);

    f.doc
        .apply(&Transaction(vec![Operation::SetMode {
            id: f.main[0],
            mode: Some(light),
        }]))
        .unwrap();
    assert_eq!(f.doc.get(f.clean).unwrap().mode(), Some(dark), "still dark");
    let res = Resolved::rebuild(&f.doc);
    let out = react_of(&f.doc, &res, &[f.clean]);
    assert!(!file(&out, "App.tsx").contains("<Card "));
    assert_eq!(out.inlined.len(), 1, "{:?}", out.inlined);
}

/// In Vue a child component's root also wears its user's scope, so no element of
/// the user may share the child root's class: the inlined card, met first, would
/// otherwise be `card` and its rule would reach the component's root.
/// flip: `vue::avoid` returning at once → fails at "no layer of the page".
#[test]
fn a_vue_page_never_shares_a_class_with_a_component_root_it_uses() {
    let f = fixture();
    let res = Resolved::rebuild(&f.doc);
    let out = vue_of(&f.doc, &res, &[f.dirty, f.clean]);
    let card = file(&out, "components/Card.vue");
    assert!(
        card.contains("<div class=\"card\">"),
        "the root is `card`: {card}"
    );
    let app = file(&out, "App.vue");
    assert!(
        !app.contains("<div class=\"card\">"),
        "no layer of the page is `card`: {app}"
    );
    assert!(app.contains("<Card class=\"card-2\""), "{app}");
    assert!(app.contains("<div class=\"card-3\">"), "{app}");
    assert!(app.contains("\n.card-3 {"), "its rule moved with it: {app}");
}

/// Where [`button_set`]'s *Badge* is.
#[derive(Clone, Copy, PartialEq)]
enum Badge {
    None,
    /// In *Large* alone, above its label.
    InLarge,
    /// In both, under *Small*'s label and above *Large*'s: an order no one
    /// tree keeps.
    UnderInSmall,
}

/// A *Button* set with a *Size* property (*Small*, the default as the topmost,
/// and *Large*) and a *Label* text property bound in both, and an instance of
/// *Large* labelled *Go*, and a *Badge* rect as `badge` says.
fn button_set(badge: Badge) -> (Document, NodeId) {
    use ondin_core::variant::{VariantProp, VariantSet};
    let mut ids = IdSource::new(0xB7);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let [
        set,
        small,
        small_label,
        large,
        large_label,
        large_badge,
        small_badge,
    ] = [(); 7].map(|_| ids.mint());
    let frame = |w, h| NodeKind::Artboard {
        size: Size::new(w, h),
    };
    doc.apply(&Transaction(vec![
        create(set, (root, 0), frame(200.0, 120.0), (0.0, 0.0), "Button"),
        Operation::SetVariantSet {
            id: set,
            set: Some(VariantSet {
                props: vec![VariantProp {
                    name: "Size".into(),
                    values: vec!["Small".into(), "Large".into()],
                }],
            }),
        },
        create(small, (set, 0), frame(80.0, 30.0), (10.0, 10.0), "Small"),
        create(small_label, (small, 0), text("Label"), (8.0, 6.0), "Label"),
        create(large, (set, 1), frame(120.0, 40.0), (10.0, 60.0), "Large"),
        create(
            large_label,
            (large, 0),
            text("Label"),
            (12.0, 10.0),
            "Label",
        ),
    ]))
    .unwrap();
    let rect = NodeKind::Rect {
        size: Size::new(8.0, 8.0),
        corner_radii: RoundedRectRadii::default(),
    };
    if badge != Badge::None {
        doc.apply(&Transaction(vec![
            create(large_badge, (large, 1), rect.clone(), (100.0, 4.0), "Badge"),
            paint(large_badge, RED),
        ]))
        .unwrap();
    }
    if badge == Badge::UnderInSmall {
        doc.apply(&Transaction(vec![
            create(small_badge, (small, 0), rect.clone(), (60.0, 4.0), "Badge"),
            paint(small_badge, RED),
        ]))
        .unwrap();
    }
    for (v, values) in [(small, "Small"), (large, "Large")] {
        doc.apply(&Transaction(vec![
            Operation::SetComponent {
                id: v,
                component: true,
            },
            Operation::SetVariant {
                id: v,
                values: vec![values.into()],
            },
        ]))
        .unwrap();
    }
    let (tx, _) = variant::define(
        &doc,
        &mut ids,
        set,
        "Label",
        PropKind::Text,
        vec![small_label, large_label],
    )
    .expect("a property");
    doc.apply(&tx).unwrap();

    let [inst, inst_label] = [(); 2].map(|_| ids.mint());
    doc.apply(&Transaction(vec![
        create(inst, (root, 1), frame(120.0, 40.0), (0.0, 200.0), "Large"),
        create(inst_label, (inst, 0), text("Label"), (12.0, 10.0), "Label"),
        Operation::SetLink {
            id: inst,
            link: Some(large),
        },
        Operation::SetLink {
            id: inst_label,
            link: Some(large_label),
        },
    ]))
    .unwrap();
    if badge != Badge::None {
        let inst_badge = ids.mint();
        doc.apply(&Transaction(vec![
            create(inst_badge, (inst, 1), rect, (100.0, 4.0), "Badge"),
            paint(inst_badge, RED),
            Operation::SetLink {
                id: inst_badge,
                link: Some(large_badge),
            },
        ]))
        .unwrap();
    }
    let p = &variant::instance_properties(&doc, inst)[0];
    let ops = variant::set_property(&doc, &[inst], p, &PropValue::Text("Go".into()));
    doc.apply(&Transaction(ops)).unwrap();
    (doc, inst)
}

/// A set whose variants draw the same elements is one tree, each element
/// choosing its class by the variant property, the default's class the fallback.
/// flip: `merge::same_shape` answering false → fails at "one tree".
#[test]
fn a_set_whose_variants_match_is_one_tree_choosing_classes() {
    let (doc, inst) = button_set(Badge::None);
    let res = Resolved::rebuild(&doc);
    let out = react_of(&doc, &res, &[inst]);
    assert!(out.inlined.is_empty(), "{:?}", out.inlined);
    let button = file(&out, "components/Button.tsx");
    assert!(!button.contains("if (size"), "one tree: {button}");
    assert!(
        button.contains(
            "<div className={`${size === \"Large\" ? styles.large : styles.small} ${className ?? \"\"}`}>"
        ),
        "{button}"
    );
    assert!(
        button.contains("className={size === \"Large\" ? styles[\"label-2\"] : styles.label}"),
        "{button}"
    );
    assert_eq!(button.matches(">{label}</p>").count(), 1, "{button}");
    let css = file(&out, "components/Button.module.css");
    for rule in [".small {", ".large {", ".label {", ".label-2 {"] {
        assert!(css.contains(rule), "{rule}: {css}");
    }
    let app = file(&out, "App.tsx");
    assert!(
        app.contains("<Button className={styles.large} size=\"Large\" label=\"Go\" />"),
        "{app}"
    );

    let out = vue_of(&doc, &res, &[inst]);
    let button = file(&out, "components/Button.vue");
    assert!(
        button.contains("<div :class=\"size === 'Large' ? 'large' : 'small'\">"),
        "{button}"
    );
    assert!(!button.contains("v-if"), "{button}");
    // The root may wear `large`, so the use's class keeps off it.
    let app = file(&out, "App.vue");
    assert!(app.contains("<Button class=\"large-2\""), "{app}");
}

/// A layer only some variants draw is in the merged tree, shown under those.
/// flip: `merge::children` leaving `shown_when` unset → fails at "under Large".
#[test]
fn a_layer_only_some_variants_draw_is_shown_under_them() {
    let (doc, inst) = button_set(Badge::InLarge);
    let res = Resolved::rebuild(&doc);
    let out = react_of(&doc, &res, &[inst]);
    assert!(out.inlined.is_empty(), "{:?}", out.inlined);
    let button = file(&out, "components/Button.tsx");
    assert!(!button.contains("if (size"), "one tree: {button}");
    assert!(
        button.contains(
            "      {size === \"Large\" && (\n        <div className={styles.badge}>\n        </div>\n      )}"
        ),
        "under Large: {button}"
    );
    let label = button.find("{label}").expect(&button);
    assert!(
        label < button.find("styles.badge").expect(&button),
        "above the label, as Large orders them: {button}"
    );

    let out = vue_of(&doc, &res, &[inst]);
    let button = file(&out, "components/Button.vue");
    assert!(
        button.contains("<div class=\"badge\" v-if=\"size === 'Large'\">\n"),
        "{button}"
    );
}

/// A set is one component: its variant property a string union defaulting to the
/// default variant's value, and where its variants' trees do not merge — here,
/// two layers ordered each way round — each other variant's tree behind its
/// condition and the default's last; the use names the variant it shows.
/// flip: `vue::keep_uses_apart` doing nothing → fails at "a use's class is none
/// of the used file's".
#[test]
fn a_set_is_one_component_choosing_its_variant_by_a_union_prop() {
    let (doc, inst) = button_set(Badge::UnderInSmall);
    let res = Resolved::rebuild(&doc);
    let out = react_of(&doc, &res, &[inst]);
    assert!(out.inlined.is_empty(), "{:?}", out.inlined);

    let button = file(&out, "components/Button.tsx");
    assert!(
        button.contains("  size?: \"Small\" | \"Large\";\n  label?: string;"),
        "{button}"
    );
    assert!(
        button.contains("function Button({ className, size = \"Small\", label = \"Label\" }"),
        "{button}"
    );
    let large = button
        .find("  if (size === \"Large\") {\n    return (\n      <div className={`${styles.large}")
        .expect(&button);
    let small = button
        .find("  return (\n    <div className={`${styles.small}")
        .expect(&button);
    assert!(
        large < small,
        "the default's tree is the fallback: {button}"
    );
    assert_eq!(button.matches(">{label}</p>").count(), 2, "{button}");
    let css = file(&out, "components/Button.module.css");
    assert!(
        css.contains(".small {") && css.contains(".large {"),
        "{css}"
    );

    let app = file(&out, "App.tsx");
    assert!(
        app.contains("<Button className={styles.large} size=\"Large\" label=\"Go\" />"),
        "{app}"
    );

    let out = vue_of(&doc, &res, &[inst]);
    let button = file(&out, "components/Button.vue");
    assert!(
        button.contains("<div class=\"large\" v-if=\"size === 'Large'\">"),
        "{button}"
    );
    assert!(button.contains("<div class=\"small\" v-else>"), "{button}");
    assert!(button.contains("    size: \"Small\",\n"), "{button}");
    // `large` is a class of the Large tree, and a use's class lands on whichever
    // tree is drawn, where Button's own `.large` rule would reach it.
    let app = file(&out, "App.vue");
    assert!(
        app.contains("<Button class=\"large-2\" size=\"Large\" label=\"Go\" />"),
        "a use's class is none of the used file's: {app}"
    );
}

const RED: Color = Color::from_rgba8(200, 30, 40, 255);
const BLUE: Color = Color::from_rgba8(0, 90, 200, 255);

/// One solid fill on `id`.
fn paint(id: NodeId, c: Color) -> Operation {
    Operation::SetFills {
        id,
        fills: keyed_by_position([Fill {
            brush: Brush::Solid(c).into(),
            visible: true,
        }]),
    }
}

/// Mains *Star* (a red dot) and *Heart* (a blue one, placed apart), a *Card*
/// main whose *Icon* slot shows Star and is bound to a swap property, and an
/// instance of Card whose slot shows Heart.
fn swap_card() -> (Document, NodeId) {
    let mut ids = IdSource::new(0x5A);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let frame = |w, h| NodeKind::Artboard {
        size: Size::new(w, h),
    };
    let dot = || NodeKind::Rect {
        size: Size::new(10.0, 10.0),
        corner_radii: RoundedRectRadii::default(),
    };
    let [star, star_dot, heart, heart_dot] = [(); 4].map(|_| ids.mint());
    let [card, slot, slot_dot] = [(); 3].map(|_| ids.mint());
    doc.apply(&Transaction(vec![
        create(star, (root, 0), frame(20.0, 20.0), (0.0, 0.0), "Star"),
        create(star_dot, (star, 0), dot(), (5.0, 5.0), "Dot"),
        create(heart, (root, 1), frame(20.0, 20.0), (40.0, 0.0), "Heart"),
        create(heart_dot, (heart, 0), dot(), (2.0, 2.0), "Dot"),
        create(card, (root, 2), frame(100.0, 40.0), (0.0, 50.0), "Card"),
        create(slot, (card, 0), frame(20.0, 20.0), (10.0, 10.0), "Star"),
        create(slot_dot, (slot, 0), dot(), (5.0, 5.0), "Dot"),
        Operation::SetComponent {
            id: star,
            component: true,
        },
        Operation::SetComponent {
            id: heart,
            component: true,
        },
        Operation::SetComponent {
            id: card,
            component: true,
        },
        Operation::SetLink {
            id: slot,
            link: Some(star),
        },
        Operation::SetLink {
            id: slot_dot,
            link: Some(star_dot),
        },
        paint(star_dot, RED),
        paint(slot_dot, RED),
        paint(heart_dot, BLUE),
    ]))
    .unwrap();
    let (tx, _) = variant::define(&doc, &mut ids, card, "Icon", PropKind::Swap, vec![slot])
        .expect("a property");
    doc.apply(&tx).unwrap();

    let [inst, inst_slot, inst_dot] = [(); 3].map(|_| ids.mint());
    doc.apply(&Transaction(vec![
        create(inst, (root, 3), frame(100.0, 40.0), (0.0, 100.0), "Card"),
        create(
            inst_slot,
            (inst, 0),
            frame(20.0, 20.0),
            (10.0, 10.0),
            "Star",
        ),
        create(inst_dot, (inst_slot, 0), dot(), (5.0, 5.0), "Dot"),
        Operation::SetLink {
            id: inst,
            link: Some(card),
        },
        Operation::SetLink {
            id: inst_slot,
            link: Some(slot),
        },
        Operation::SetLink {
            id: inst_dot,
            link: Some(slot_dot),
        },
        paint(inst_dot, RED),
    ]))
    .unwrap();
    // The swap, with the rewrite of the slot's contents the commit adds to it.
    let p = &variant::instance_properties(&doc, inst)[0];
    let mut tx = Transaction(variant::set_property(
        &doc,
        &[inst],
        p,
        &PropValue::Swap(heart),
    ));
    ondin_core::propagate::owed(&doc, &mut tx, &mut ids);
    doc.apply(&tx).unwrap();
    (doc, inst)
}

/// A swap property is a component-typed prop defaulting to the slot's main; the
/// slot draws whichever component it is given, and a use passes its swap.
/// flip: `Walk::why_inline` refusing every `SetSwap` → fails at "the swapped
/// instance is a use".
#[test]
fn a_swap_property_is_a_component_prop() {
    let (doc, inst) = swap_card();
    let drift = ondin_core::reset::drift(&doc, inst);
    assert_eq!(
        (drift.fields, drift.removed, drift.order, drift.local),
        (1, 0, 0, 0),
        "the instance differs from its main in its swap alone"
    );
    let res = Resolved::rebuild(&doc);
    let out = react_of(&doc, &res, &[inst]);
    assert!(
        out.inlined.is_empty(),
        "the swapped instance is a use: {:?}",
        out.inlined
    );

    let card = file(&out, "components/Card.tsx");
    assert!(
        card.starts_with("import type { ComponentType } from \"react\";\n"),
        "{card}"
    );
    assert!(card.contains("import Star from \"./Star\";"), "{card}");
    assert!(
        card.contains("  icon?: ComponentType<{ className?: string }>;"),
        "{card}"
    );
    assert!(
        card.contains("function Card({ className, icon: IconSlot = Star }: CardProps)"),
        "{card}"
    );
    assert!(
        card.contains("<IconSlot className={styles.star} />"),
        "{card}"
    );

    let app = file(&out, "App.tsx");
    assert!(
        app.contains("import Heart from \"./components/Heart\";"),
        "{app}"
    );
    assert!(
        app.contains("<Card className={styles.card} icon={Heart} />"),
        "{app}"
    );
    assert!(out.file("components/Heart.tsx").is_some());
    assert!(out.file("components/Star.tsx").is_some());

    let out = vue_of(&doc, &res, &[inst]);
    let card = file(&out, "components/Card.vue");
    assert!(
        card.contains("import type { Component } from \"vue\";"),
        "{card}"
    );
    assert!(card.contains("    icon?: Component;"), "{card}");
    assert!(card.contains("    icon: () => Star,"), "{card}");
    assert!(
        card.contains("<component :is=\"icon\" class=\"star-2\" />"),
        "the slot keeps off every component's classes: {card}"
    );
    let app = file(&out, "App.vue");
    assert!(
        app.contains("<Card class=\"card\" :icon=\"Heart\" />"),
        "{app}"
    );
}

/// A *Button* main with a *Label* text property, a *Card* main showing the
/// Button slot inside it, which labels it *OK*, and two instances of Card: one
/// labelling its Button *Buy*, one leaving it.
fn shown_card() -> (Document, [NodeId; 2]) {
    let mut ids = IdSource::new(0x4E);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let frame = |w, h| NodeKind::Artboard {
        size: Size::new(w, h),
    };
    let [button, button_label, card, slot, slot_label] = [(); 5].map(|_| ids.mint());
    let [button_icon, slot_icon] = [(); 2].map(|_| ids.mint());
    let icon = || NodeKind::Rect {
        size: Size::new(12.0, 12.0),
        corner_radii: RoundedRectRadii::default(),
    };
    doc.apply(&Transaction(vec![
        create(button, (root, 0), frame(80.0, 30.0), (0.0, 0.0), "Button"),
        create(
            button_label,
            (button, 0),
            text("Label"),
            (8.0, 6.0),
            "Label",
        ),
        create(card, (root, 1), frame(200.0, 80.0), (0.0, 50.0), "Card"),
        create(slot, (card, 0), frame(80.0, 30.0), (10.0, 10.0), "Button"),
        create(slot_label, (slot, 0), text("Label"), (8.0, 6.0), "Label"),
        create(button_icon, (button, 1), icon(), (60.0, 9.0), "Icon"),
        create(slot_icon, (slot, 1), icon(), (60.0, 9.0), "Icon"),
        paint(button, BLUE),
        paint(slot, BLUE),
        paint(button_icon, RED),
        paint(slot_icon, RED),
        Operation::SetLink {
            id: slot_icon,
            link: Some(button_icon),
        },
        Operation::SetComponent {
            id: button,
            component: true,
        },
        Operation::SetComponent {
            id: card,
            component: true,
        },
        Operation::SetLink {
            id: slot,
            link: Some(button),
        },
        Operation::SetLink {
            id: slot_label,
            link: Some(button_label),
        },
    ]))
    .unwrap();
    let (tx, _) = variant::define(
        &doc,
        &mut ids,
        button,
        "Label",
        PropKind::Text,
        vec![button_label],
    )
    .expect("a property");
    doc.apply(&tx).unwrap();
    let (tx, _) = variant::define(
        &doc,
        &mut ids,
        button,
        "Show icon",
        PropKind::Boolean,
        vec![button_icon],
    )
    .expect("a property");
    doc.apply(&tx).unwrap();
    let label = |doc: &Document, root| variant::instance_properties(doc, root)[0].clone();
    let shows_icon = |doc: &Document, root| variant::instance_properties(doc, root)[1].clone();
    let ops = variant::set_property(
        &doc,
        &[slot],
        &label(&doc, slot),
        &PropValue::Text("OK".into()),
    );
    doc.apply(&Transaction(ops)).unwrap();
    let tx = variant::set_shown(&doc, &mut ids, slot, true).expect("the slot is shown");
    doc.apply(&tx).unwrap();

    let mut insts = [root; 2];
    for (n, inst) in insts.iter_mut().enumerate() {
        let [r, s, l, i] = [(); 4].map(|_| ids.mint());
        let index = doc.get(root).unwrap().children().len();
        let y = 100.0 + 90.0 * n as f64;
        doc.apply(&Transaction(vec![
            create(r, (root, index), frame(200.0, 80.0), (0.0, y), "Card"),
            create(s, (r, 0), frame(80.0, 30.0), (10.0, 10.0), "Button"),
            create(l, (s, 0), text("OK"), (8.0, 6.0), "Label"),
            create(i, (s, 1), icon(), (60.0, 9.0), "Icon"),
            paint(s, BLUE),
            paint(i, RED),
            Operation::SetLink {
                id: i,
                link: Some(slot_icon),
            },
            Operation::SetLink {
                id: r,
                link: Some(card),
            },
            Operation::SetLink {
                id: s,
                link: Some(slot),
            },
            Operation::SetLink {
                id: l,
                link: Some(slot_label),
            },
        ]))
        .unwrap();
        if n == 0 {
            let ops =
                variant::set_property(&doc, &[s], &label(&doc, s), &PropValue::Text("Buy".into()));
            doc.apply(&Transaction(ops)).unwrap();
            let ops =
                variant::set_property(&doc, &[s], &shows_icon(&doc, s), &PropValue::Boolean(false));
            doc.apply(&Transaction(ops)).unwrap();
        }
        *inst = r;
    }
    (doc, insts)
}

/// A shown nested instance's own properties are the outer component's,
/// forwarded under the slot's name with no default of their own: the slot
/// passes the prop on, its own value standing in when it is left unset.
/// flip: `Walk::fields` answering nothing for a forwarded parameter → fails at
/// "both instances are uses".
#[test]
fn a_shown_nested_instances_properties_are_forwarded() {
    let (doc, [labelled, plain]) = shown_card();
    for (inst, fields) in [(labelled, 2), (plain, 0)] {
        let drift = ondin_core::reset::drift(&doc, inst);
        assert_eq!(
            (drift.fields, drift.removed, drift.order, drift.local),
            (fields, 0, 0, 0),
            "the fixture's instances differ only in the shown label and icon"
        );
    }
    let res = Resolved::rebuild(&doc);
    let out = react_of(&doc, &res, &[labelled, plain]);
    assert!(
        out.inlined.is_empty(),
        "both instances are uses: {:?}",
        out.inlined
    );

    let card = file(&out, "components/Card.tsx");
    assert!(
        card.contains("  buttonLabel?: string;\n  buttonShowIcon?: boolean;"),
        "{card}"
    );
    assert!(
        card.contains("function Card({ className, buttonLabel, buttonShowIcon }: CardProps)"),
        "a forwarded prop has no default: {card}"
    );
    assert!(
        card.contains(
            "<Button className={styles.button} label={buttonLabel ?? \"OK\"} \
             showIcon={buttonShowIcon} />"
        ),
        "{card}"
    );
    let app = file(&out, "App.tsx");
    assert!(
        app.contains("<Card className={styles.card} buttonLabel=\"Buy\" buttonShowIcon={false} />"),
        "{app}"
    );
    assert!(
        app.contains("<Card className={styles[\"card-2\"]} />"),
        "the plain instance passes nothing: {app}"
    );

    let out = vue_of(&doc, &res, &[labelled, plain]);
    let card = file(&out, "components/Card.vue");
    // Unset rather than cast to `false`, which Vue does to a boolean with no
    // default, so the slot's own `true` stands.
    assert!(
        card.contains("    buttonLabel: undefined,\n    buttonShowIcon: undefined,"),
        "{card}"
    );
    assert!(card.contains(":label=\"buttonLabel ?? 'OK'\""), "{card}");
    let app = file(&out, "App.vue");
    assert!(app.contains("buttonLabel=\"Buy\""), "{app}");
}

/// A kit's *Badge*, a frame holding a filled dot and a label, published by a kit named
/// *Acme UI*, and a consumer holding one instance of it at its root.
fn kit_badge() -> (Document, NodeId) {
    use ondin_core::meta::{DocumentMeta, KitMark};
    const KIT: &str = "5eed00000000000000000000000000c0";
    let mut ids = IdSource::new(0xC4);
    let root = ids.mint();
    let mut kit = Document::new(root);
    let [badge, dot, label] = [(); 3].map(|_| ids.mint());
    kit.apply(&Transaction(vec![
        create(
            badge,
            (root, 0),
            NodeKind::Artboard {
                size: Size::new(40.0, 20.0),
            },
            (0.0, 0.0),
            "Badge",
        ),
        create(
            dot,
            (badge, 0),
            NodeKind::Rect {
                size: Size::new(10.0, 10.0),
                corner_radii: RoundedRectRadii::default(),
            },
            (5.0, 5.0),
            "Dot",
        ),
        create(label, (badge, 1), text("New"), (18.0, 2.0), "Label"),
        paint(dot, Color::from_rgba8(200, 30, 40, 255)),
        Operation::SetComponent {
            id: badge,
            component: true,
        },
    ]))
    .unwrap();
    kit.set_meta(DocumentMeta {
        id: Some(KIT.into()),
        name: Some("Acme UI".into()),
        kit: Some(KitMark::default()),
        ..Default::default()
    });
    let published = ondin_core::kit::freeze(&kit).unwrap();
    let mut dids = IdSource::new(0xC5);
    let mut d = Document::new(dids.mint());
    let droot = d.root();
    let (tx, inst) =
        ondin_core::kit::insert(&d, &mut dids, KIT, 1, &published, badge, droot).unwrap();
    d.apply(&tx).unwrap();
    (d, inst)
}

/// An instance of a kit's main is a use of a component written from the vault,
/// in a folder named for the kit, its imports reaching back two folders.
/// flip: `walk::page` walking the document as given, its vault unresolved →
/// fails at "laid out from the vault".
#[test]
fn a_kit_main_is_a_component_in_the_kits_folder() {
    let (doc, inst) = kit_badge();
    let main = ondin_core::component::main_of(&doc, inst).unwrap();
    assert!(doc.in_vault(main), "the main is the vault's");
    let res = Resolved::rebuild(&doc);

    let out = react_of(&doc, &res, &[inst]);
    assert!(out.inlined.is_empty(), "{:?}", out.inlined);
    let app = file(&out, "App.tsx");
    assert!(
        app.contains("import Badge from \"./components/AcmeUI/Badge\";"),
        "{app}"
    );
    assert!(app.contains("<Badge className={styles.badge} />"), "{app}");
    let badge = file(&out, "components/AcmeUI/Badge.tsx");
    assert!(badge.starts_with("import \"../../base.css\";"), "{badge}");
    let css = file(&out, "components/AcmeUI/Badge.module.css");
    assert!(css.contains(".dot {"), "the badge's dot: {css}");
    // A text's automatic line height is the one its resolved layout laid.
    assert!(
        css.lines()
            .any(|l| l.trim_start().starts_with("line-height:") && l.ends_with("px;")),
        "laid out from the vault: {css}"
    );

    let out = vue_of(&doc, &res, &[inst]);
    let app = file(&out, "App.vue");
    assert!(
        app.contains("import Badge from \"./components/AcmeUI/Badge.vue\";"),
        "{app}"
    );
    let badge = file(&out, "components/AcmeUI/Badge.vue");
    assert!(badge.contains("import \"../../base.css\";"), "{badge}");
}
