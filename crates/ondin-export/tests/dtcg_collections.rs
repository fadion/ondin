//! Collections and scope in `tokens.json`: each collection a top-level group
//! with its own modes, a number typed by its scope, and both read back.

use ondin_core::peniko::Color;
use ondin_core::variable::{Collection, CollectionId, Field, Scope};
use ondin_core::{
    Cell, Document, IdSource, Mode, ModeId, Operation, Transaction, VarId, VarKind, Variable,
};
use ondin_export::dtcg::{EXTENSION, tokens};
use ondin_export::dtcg_in::{parse, review};
use serde_json::Value;

fn mode(id: ModeId, name: &str) -> Mode {
    Mode {
        id,
        name: name.into(),
    }
}

fn var(
    id: VarId,
    name: &str,
    kind: VarKind,
    cells: Vec<(ModeId, Cell)>,
    scope: Option<Scope>,
) -> Variable {
    Variable {
        id,
        name: name.into(),
        kind,
        cells: cells.into_iter().collect(),
        scope,
        origin: None,
        forked: None,
        description: String::new(),
    }
}

/// Colour (Light, Dark) holding `brand`; Spacing (Compact, Comfortable)
/// holding `space/md`, scoped to gaps and corners, and `weight`, scoped to
/// weight.
fn doc() -> (Document, IdSource, ModeId, VarId) {
    let mut ids = IdSource::new(0xD72);
    let mut doc = Document::new(ids.mint());
    let [light, dark, compact, comfy] = [(); 4].map(|_| ids.mint_mode());
    let spacing = ids.mint_collection();
    let [brand, gap, weight] = [(); 3].map(|_| ids.mint_var());
    doc.apply(&Transaction(vec![
        Operation::SetCollectionName {
            id: CollectionId::FIRST,
            name: "Colour".into(),
        },
        Operation::AddMode {
            collection: CollectionId::FIRST,
            mode: mode(light, "Light"),
            index: 0,
            cells: vec![],
        },
        Operation::AddMode {
            collection: CollectionId::FIRST,
            mode: mode(dark, "Dark"),
            index: 1,
            cells: vec![],
        },
        Operation::AddCollection {
            collection: Collection {
                id: spacing,
                name: "Spacing".into(),
                modes: vec![mode(compact, "Compact"), mode(comfy, "Comfortable")],
                breakpoints: None,
            },
            index: 1,
        },
        Operation::AddVariable {
            var: var(
                brand,
                "brand",
                VarKind::Color,
                vec![
                    (light, Cell::Color(Color::WHITE)),
                    (dark, Cell::Color(Color::BLACK)),
                ],
                None,
            ),
        },
        Operation::AddVariable {
            var: var(
                gap,
                "space/md",
                VarKind::Number,
                vec![(compact, Cell::Number(8.0)), (comfy, Cell::Number(12.0))],
                Some(Scope::of([Field::Spacing, Field::Corner])),
            ),
        },
        Operation::AddVariable {
            var: var(
                weight,
                "weight",
                VarKind::Number,
                vec![(compact, Cell::Number(400.0)), (comfy, Cell::Number(600.0))],
                Some(Scope::of([Field::Weight])),
            ),
        },
    ]))
    .expect("two collections");
    (doc, ids, comfy, gap)
}

fn at<'a>(v: &'a Value, path: &str) -> &'a Value {
    path.split('.').fold(v, |v, k| &v[k])
}

/// **Each collection is a top-level group with its own modes**, a number is
/// typed by its scope, the scope rides in the extension, and `--mode` moves only
/// its own collection's `$value`s.
#[test]
fn collections_are_groups_and_scope_types_a_number() {
    let (d, _, comfy, _) = doc();
    let t = tokens(&d, Some(comfy));
    let v: Value = serde_json::from_str(&t.json).unwrap();
    let ext = |p: &str| &at(&v, p)["$extensions"][EXTENSION];
    assert_eq!(ext("Spacing")["collection"]["valueMode"], "Comfortable");
    assert_eq!(ext("Colour")["collection"]["valueMode"], "Light");
    assert_eq!(at(&v, "Spacing.space.md.$type"), "dimension");
    assert_eq!(at(&v, "Spacing.space.md.$value.value"), 12.0);
    assert_eq!(at(&v, "Spacing.weight.$type"), "fontWeight");
    assert_eq!(
        ext("Spacing.space.md")["scope"],
        serde_json::json!(["spacing", "corner"])
    );
    assert_eq!(at(&v, "Colour.brand.$type"), "color");
    assert!(
        v["$extensions"][EXTENSION].get("modes").is_none(),
        "the root lists no modes once collections do"
    );
}

/// **A file read back lands each collection in the one of its name**, and an
/// unchanged document reviews as no change.
#[test]
fn a_round_trip_is_no_change() {
    let (d, mut ids, _, _) = doc();
    let file = parse(&tokens(&d, None).json).unwrap();
    assert!(file.has_collections());
    let r = review(&d, &file, CollectionId::FIRST, &mut ids).unwrap();
    assert_eq!(r.changes(), 0, "{:#?}", r.rows);
}

/// **A collection picked by width keeps its widths through a file**: each
/// mode's `minWidth` rides in the collection's mode list, and a file read into
/// an empty document makes the collection picked by width again.
///
/// flip: `Review::breakpoint_ops` returning nothing → fails at "picked by width
/// again".
#[test]
fn the_breakpoints_ride_in_the_mode_list_and_come_back() {
    let (mut d, _, _, _) = doc();
    let spacing = d.variables().collections()[1].id;
    let ops = ondin_core::variable::by_width_ops(&d, spacing, true).unwrap();
    d.apply(&Transaction(ops)).unwrap();
    let json = tokens(&d, None).json;
    let v: Value = serde_json::from_str(&json).unwrap();
    let modes = &at(&v, "Spacing")["$extensions"][EXTENSION]["collection"]["modes"];
    assert_eq!(modes[0]["minWidth"], 0.0);
    assert_eq!(modes[1]["minWidth"], 768.0);

    let mut ids = IdSource::new(0xD73);
    let mut empty = Document::new(ids.mint());
    let file = parse(&json).unwrap();
    let r = review(&empty, &file, CollectionId::FIRST, &mut ids).unwrap();
    let ops = r.ops(&empty, &vec![true; r.rows.len()]).unwrap();
    empty.apply(&Transaction(ops)).expect("applies");
    let t = empty.variables();
    let back = t
        .collections()
        .iter()
        .find(|c| c.name == "Spacing")
        .unwrap();
    assert!(back.by_width(), "picked by width again");
    assert_eq!(back.min_width(back.modes[1].id), Some(768.0));
}

/// **The file's widths are a row of their own**: unticked, nothing writes them,
/// and a linked review keeps the row only when the file changed its widths, so a
/// width the document moved since is left alone.
/// flip: `ops` writing the file's widths whatever is ticked → fails at "nothing
/// ticked".
#[test]
fn the_files_widths_are_a_row_a_linked_review_keeps_only_when_changed() {
    let (mut d, _, comfy, _) = doc();
    let spacing = d.variables().collections()[1].id;
    let ops = ondin_core::variable::by_width_ops(&d, spacing, true).unwrap();
    d.apply(&Transaction(ops)).unwrap();
    let applied: Value = serde_json::from_str(&tokens(&d, None).json).unwrap();
    // The document moves Comfortable to 800; the file changes a colour.
    let mut widths = d
        .variables()
        .collection(spacing)
        .unwrap()
        .breakpoints
        .clone()
        .unwrap();
    widths.insert(comfy, 800.0);
    d.apply(&Transaction(vec![Operation::SetBreakpoints {
        id: spacing,
        breakpoints: Some(widths.clone()),
    }]))
    .unwrap();
    let mut now = applied.clone();
    now["Colour"]["brand"]["$value"] = Value::String("#ff0000".into());
    let file = parse(&now.to_string()).unwrap();
    let mut ids = IdSource::new(0xD74);
    let mut r = review(&d, &file, CollectionId::FIRST, &mut ids).unwrap();
    let names: Vec<&str> = r.rows.iter().map(|x| x.name.as_str()).collect();
    assert!(names.contains(&"Breakpoints of Spacing"), "{names:?}");
    let none = r.ops(&d, &vec![false; r.rows.len()]).unwrap();
    assert!(none.is_empty(), "nothing ticked writes nothing: {none:?}");

    r.since(&file, &parse(&applied.to_string()).unwrap());
    let names: Vec<&str> = r.rows.iter().map(|x| x.name.as_str()).collect();
    assert_eq!(names, ["Colour.brand"]);
    let mut after = d.clone();
    after
        .apply(&Transaction(r.ops(&d, &vec![true; r.rows.len()]).unwrap()))
        .unwrap();
    assert_eq!(
        after.variables().collection(spacing).unwrap().breakpoints,
        Some(widths),
        "the document's width stands"
    );
}

/// **An added mode is matched within its own collection**: a new collection's
/// *Mode 1* is still offered when the old file's other collection had a *Mode 1*,
/// and the review applies.
/// flip: `since` matching an added mode against every old collection → fails at
/// "still offered".
#[test]
fn a_new_collections_mode_is_offered_though_a_sibling_had_its_name() {
    let mut ids = IdSource::new(0xD75);
    let mut source = Document::new(ids.mint());
    let (first, spacing_mode) = (ids.mint_mode(), ids.mint_mode());
    let (a, gap) = (ids.mint_var(), ids.mint_var());
    source
        .apply(&Transaction(vec![
            Operation::AddMode {
                collection: CollectionId::FIRST,
                mode: mode(first, "Mode 1"),
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: var(
                    a,
                    "a",
                    VarKind::Number,
                    vec![(first, Cell::Number(1.0))],
                    None,
                ),
            },
        ]))
        .unwrap();
    let old = tokens(&source, None).json;
    let spacing = ids.mint_collection();
    source
        .apply(&Transaction(vec![
            Operation::AddCollection {
                collection: Collection {
                    id: spacing,
                    name: "Spacing".into(),
                    modes: vec![mode(spacing_mode, "Mode 1")],
                    breakpoints: None,
                },
                index: 1,
            },
            Operation::AddVariable {
                var: var(
                    gap,
                    "gap",
                    VarKind::Number,
                    vec![(spacing_mode, Cell::Number(8.0))],
                    None,
                ),
            },
        ]))
        .unwrap();
    let new = tokens(&source, None).json;

    let mut product = Document::new(ids.mint());
    let old = parse(&old).unwrap();
    let first_review = review(&product, &old, CollectionId::FIRST, &mut ids).unwrap();
    let ops = first_review
        .ops(&product, &vec![true; first_review.rows.len()])
        .unwrap();
    product.apply(&Transaction(ops)).unwrap();

    let new = parse(&new).unwrap();
    let mut r = review(&product, &new, CollectionId::FIRST, &mut ids).unwrap();
    r.since(&new, &old);
    let names: Vec<&str> = r.rows.iter().map(|x| x.name.as_str()).collect();
    assert!(
        names.contains(&"Adds mode \u{201c}Mode 1\u{201d} to Spacing"),
        "still offered: {names:?}"
    );
    r.ops(&product, &vec![true; r.rows.len()])
        .expect("the review applies");
}

/// **A token's `$type` scopes a new number**, and the extension's scope wins
/// over it; a file naming no collection lands in the one it is imported into.
#[test]
fn a_new_numbers_scope_comes_from_its_type_or_its_extension() {
    let (d, mut ids, _, _) = doc();
    let spacing = d.variables().collections()[1].id;
    let json = r#"{
        "pad": { "$type": "dimension", "$value": { "value": 4, "unit": "px" } },
        "bold": { "$type": "fontWeight", "$value": 700 },
        "ratio": { "$type": "number", "$value": 2,
                   "$extensions": { "io.github.fadion.ondin": { "scope": ["opacity"] } } }
    }"#;
    let file = parse(json).unwrap();
    let r = review(&d, &file, spacing, &mut ids).unwrap();
    let ticked = vec![true; r.rows.len()];
    let ops = r.ops(&d, &ticked).unwrap();
    let mut after = d.clone();
    after.apply(&Transaction(ops)).unwrap();
    let t = after.variables();
    let scope = |name: &str| t.by_name_in(spacing, name).and_then(|v| v.scope);
    assert_eq!(
        scope("pad"),
        Some(Scope::of(
            Field::of_kind(VarKind::Number).filter(|f| f.is_length())
        ))
    );
    assert_eq!(scope("bold"), Some(Scope::of([Field::Weight])));
    assert_eq!(scope("ratio"), Some(Scope::of([Field::Opacity])));
}
