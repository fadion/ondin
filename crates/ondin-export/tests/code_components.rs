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
