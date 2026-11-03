//! `ondin_export::dtcg_in`: a `tokens.json` read back, compared with a document
//! and applied as one transaction.

mod common;

use ondin_core::peniko::Color;
use ondin_core::style::{Style, StyleValue};
use ondin_core::{
    Cell, Document, Effect, EffectKind, IdSource, Mode, ModeId, Operation, Transaction, VarId,
    VarKind, Variable, keyed_by_position,
};
use ondin_export::dtcg::tokens;
use ondin_export::dtcg_in::{Change, File, Review, parse, review};
use serde_json::{Value, json};

fn add_var(id: VarId, name: &str, kind: VarKind, cells: Vec<(ModeId, Cell)>) -> Operation {
    Operation::AddVariable {
        var: Variable {
            id,
            name: name.into(),
            kind,
            cells: cells.into_iter().collect(),
            origin: None,
            forked: None,
            description: String::new(),
        },
    }
}

fn add_style(ids: &mut IdSource, name: &str, value: StyleValue) -> Operation {
    Operation::AddStyle {
        style: Style {
            id: ids.mint_style(),
            name: name.into(),
            value,
            origin: None,
            forked: None,
            description: String::new(),
        },
    }
}

/// Two modes; colours with an alias, numbers bound to a rect's corners, a
/// string and a boolean; a fill, shadow, text and border style, the border's
/// width bound.
struct Doc {
    doc: Document,
    ids: IdSource,
    rect: ondin_core::NodeId,
    light: ModeId,
    dark: ModeId,
    brand: VarId,
    bg: VarId,
    radius: VarId,
    count: VarId,
}

fn doc() -> Doc {
    let f = common::fixture();
    let mut doc = f.doc;
    let mut ids = IdSource::new(0xD71);
    let (light, dark) = (ids.mint_mode(), ids.mint_mode());
    let [brand, bg, radius, count, label, shown] = [(); 6].map(|_| ids.mint_var());
    let num = |a: f64, b: f64| vec![(light, Cell::Number(a)), (dark, Cell::Number(b))];
    let mode = |id, name: &str, index| Operation::AddMode {
        mode: Mode {
            id,
            name: name.into(),
        },
        index,
        cells: vec![],
    };
    let mut ops = vec![
        mode(light, "Light", 0),
        mode(dark, "Dark", 1),
        add_var(
            brand,
            "brand/500",
            VarKind::Color,
            vec![
                (light, Cell::Color(Color::from_rgba8(91, 127, 214, 255))),
                (dark, Cell::Color(Color::new([0.2, 0.3, 0.7, 0.5]))),
            ],
        ),
        add_var(
            bg,
            "button/bg",
            VarKind::Color,
            vec![(light, Cell::Alias(brand)), (dark, Cell::Alias(brand))],
        ),
        add_var(radius, "radius/m", VarKind::Number, num(8.0, 12.0)),
        add_var(count, "count", VarKind::Number, num(3.0, 3.0)),
        add_var(
            label,
            "copy/label",
            VarKind::String,
            vec![
                (light, Cell::String("Buy".into())),
                (dark, Cell::String("Buy".into())),
            ],
        ),
        add_var(
            shown,
            "flags/beta",
            VarKind::Boolean,
            vec![(light, Cell::Boolean(true)), (dark, Cell::Boolean(false))],
        ),
    ];
    let drop = ondin_core::Shadow {
        offset: ondin_core::kurbo::Vec2::new(0.0, 4.0),
        blur: 12.0,
        spread: 0.0,
        color: Color::from_rgba8(0, 0, 0, 64),
        blend: ondin_core::BlendMode::Normal,
    };
    ops.push(add_style(
        &mut ids,
        "Surface/Card",
        StyleValue::Fills(keyed_by_position([ondin_core::Fill {
            brush: ondin_core::Brush::Solid(Color::from_rgba8(250, 250, 250, 255)).into(),
            visible: true,
        }])),
    ));
    ops.push(add_style(
        &mut ids,
        "Brand",
        StyleValue::Fills(keyed_by_position([ondin_core::Fill {
            brush: ondin_core::BrushSource::Bound {
                var: brand,
                alpha: 1.0,
            },
            visible: true,
        }])),
    ));
    ops.push(add_style(
        &mut ids,
        "Elevation/1",
        StyleValue::Effects(keyed_by_position([Effect {
            kind: EffectKind::DropShadow(drop),
            visible: true,
            vars: Default::default(),
        }])),
    ));
    ops.push(add_style(
        &mut ids,
        "Body",
        StyleValue::Text(Box::new(ondin_core::style::type_part(
            &ondin_core::TextStyle {
                font_family: "Inter".into(),
                font_size: 16.0,
                weight: 400,
                line_height: Some(ondin_core::Length::Px(24.0)),
                letter_spacing: ondin_core::Length::Em(0.01),
                ..Default::default()
            },
        ))),
    ));
    ops.push(add_style(
        &mut ids,
        "Hairline",
        StyleValue::Strokes(keyed_by_position([ondin_core::Stroke {
            width: 0.0,
            width_var: Some(radius),
            ..Default::default()
        }])),
    ));
    doc.apply(&Transaction(ops)).unwrap();
    let corners = ondin_core::bind_corners_ops(&doc, f.rect, [Some(radius); 4]);
    doc.apply(&Transaction(corners)).unwrap();
    Doc {
        doc,
        ids,
        rect: f.rect,
        light,
        dark,
        brand,
        bg,
        radius,
        count,
    }
}

fn exported(d: &Doc) -> Value {
    serde_json::from_str(&tokens(&d.doc, None).json).unwrap()
}

fn read(v: &Value) -> File {
    parse(&v.to_string()).unwrap_or_else(|r| panic!("refused: {r}"))
}

fn reviewed(d: &mut Doc, v: &Value) -> Review {
    review(&d.doc, &read(v), &mut d.ids).unwrap_or_else(|r| panic!("refused: {r}"))
}

/// Apply every row as it starts ticked, or as `tick` says for the named ones.
fn apply(d: &mut Doc, r: &Review, tick: &[(&str, bool)]) {
    let ticked: Vec<bool> = r
        .rows
        .iter()
        .map(|row| {
            tick.iter()
                .find(|(n, _)| *n == row.name)
                .map_or(row.ticked, |(_, on)| *on)
        })
        .collect();
    let ops = r.ops(&d.doc, &ticked).unwrap();
    d.doc.apply(&Transaction(ops)).unwrap();
}

fn row<'a>(r: &'a Review, name: &str) -> &'a ondin_export::dtcg_in::Row {
    r.rows
        .iter()
        .find(|x| x.name == name)
        .unwrap_or_else(|| panic!("no row {name}: {:?}", names(r)))
}

fn names(r: &Review) -> Vec<(String, Change)> {
    r.rows.iter().map(|x| (x.name.clone(), x.change)).collect()
}

/// The document's own export read back changes nothing: every value, alias,
/// binding, style and name the export writes is read as what it was.
#[test]
fn the_documents_own_export_reads_back_as_no_change() {
    let mut d = doc();
    let v = exported(&d);
    let r = reviewed(&mut d, &v);
    assert!(r.rows.is_empty(), "{:?}", names(&r));
    assert!(r.left_out.is_empty(), "{:?}", r.left_out);
    assert_eq!(r.columns, ["Light", "Dark"]);
}

/// A token renamed in code is a rename, matched by the id the export wrote:
/// the variable keeps its id, so the alias and the corners still read it.
#[test]
fn a_rename_in_code_is_a_rename_that_keeps_bindings() {
    let mut d = doc();
    let mut v = exported(&d);
    let token = v["radius"].as_object_mut().unwrap().remove("m").unwrap();
    v["radius"]["medium"] = token;
    let r = reviewed(&mut d, &v);
    assert_eq!(names(&r), [("radius.medium".to_string(), Change::Renamed)]);
    assert_eq!(
        row(&r, "radius.medium").sub.as_deref(),
        Some("was radius.m \u{b7} 2 bindings kept")
    );
    apply(&mut d, &r, &[]);
    let var = d.doc.variables().get(d.radius).unwrap();
    assert_eq!(var.name, "radius/medium");
    assert_eq!(
        d.doc.get(d.rect).unwrap().corner_vars(),
        [Some(d.radius); 4],
        "the corners are still bound"
    );
}

/// A changed `$value`, a new token, a token gone and a mode the document lacks:
/// each is its own row, the removal unticked and saying what reads it.
#[test]
fn added_changed_removed_and_a_new_mode_are_rows() {
    let mut d = doc();
    let mut v = exported(&d);
    v["radius"]["m"]["$value"] = json!({ "value": 10, "unit": "px" });
    v["space"] =
        json!({ "gutter": { "$type": "dimension", "$value": { "value": 24, "unit": "px" } } });
    v.as_object_mut().unwrap().remove("count");
    let ext = &mut v["$extensions"]["io.github.fadion.ondin"];
    ext["modes"]
        .as_array_mut()
        .unwrap()
        .push(json!({ "name": "High contrast" }));
    v["brand"]["500"]["$extensions"]["io.github.fadion.ondin"]["modes"]["High contrast"] =
        json!("#000000");
    let r = reviewed(&mut d, &v);
    assert_eq!(r.columns, ["Light", "Dark", "High contrast"]);
    let mode = row(&r, "Adds mode \u{201c}High contrast\u{201d}");
    assert_eq!((mode.change, mode.ticked), (Change::Added, true));
    let radius = row(&r, "radius.m");
    assert_eq!(radius.change, Change::Changed);
    assert_eq!(radius.cells[0].old.as_ref().unwrap().text, "8");
    assert_eq!(radius.cells[0].new.as_ref().unwrap().text, "10");
    assert!(radius.cells[1].old.is_none(), "Dark did not change");
    assert_eq!(row(&r, "space.gutter").change, Change::Added);
    let gone = row(&r, "count");
    assert_eq!((gone.change, gone.ticked), (Change::Removed, false));
    assert_eq!(gone.sub.as_deref(), Some("read by nothing"));
    // The brand colour gains a value for the new mode only: that is the mode's
    // row, not one of its own, and it still lands below.
    assert!(
        r.rows.iter().all(|x| x.name != "brand.500"),
        "{:?}",
        names(&r)
    );

    apply(&mut d, &r, &[("count", true)]);
    let t = d.doc.variables();
    assert_eq!(t.modes().len(), 3);
    let hc = t.modes()[2].id;
    assert_eq!(
        t.get(d.brand).unwrap().cells[&hc],
        Cell::Color(Color::from_rgba8(0, 0, 0, 255))
    );
    // A variable without a value for the new mode takes its value-mode one.
    assert_eq!(t.get(d.radius).unwrap().cells[&hc], Cell::Number(10.0));
    assert_eq!(t.get(d.radius).unwrap().cells[&d.light], Cell::Number(10.0));
    assert_eq!(t.get(d.radius).unwrap().cells[&d.dark], Cell::Number(12.0));
    assert!(t.get(d.count).is_none());
    assert!(t.by_name("space/gutter").is_some());
}

/// Removing a bound variable is opt-in, and once ticked its users keep the
/// value they drew, detached.
#[test]
fn a_ticked_removal_detaches_its_users() {
    let mut d = doc();
    let mut v = exported(&d);
    v.as_object_mut().unwrap().remove("radius");
    // The border style reads it by reference; it goes with it.
    v["styles"].as_object_mut().unwrap().remove("stroke");
    let r = reviewed(&mut d, &v);
    let gone = row(&r, "radius.m");
    assert_eq!(gone.sub.as_deref(), Some("read by 1 layer and 1 style"));
    apply(&mut d, &r, &[("radius.m", true)]);
    assert!(d.doc.variables().get(d.radius).is_none());
    assert_eq!(d.doc.get(d.rect).unwrap().corner_vars(), [None; 4]);
}

/// A reference between two new tokens is an alias; leaving the target unticked
/// is refused with both names.
#[test]
fn an_alias_to_an_unticked_token_is_refused() {
    let mut d = doc();
    let mut v = exported(&d);
    v["accent"] = json!({
        "base": { "$type": "color", "$value": "#ff8800" },
        "text": { "$type": "color", "$value": "{accent.base}" },
    });
    let r = reviewed(&mut d, &v);
    assert_eq!(
        row(&r, "accent.text").cells[0].new.as_ref().unwrap().text,
        "\u{2192} accent.base"
    );
    let ticked: Vec<bool> = r.rows.iter().map(|x| x.name != "accent.base").collect();
    let why = r.ops(&d.doc, &ticked).unwrap_err();
    assert_eq!(why, "accent.text reads accent.base, which is not ticked");
    apply(&mut d, &r, &[]);
    let t = d.doc.variables();
    let base = t.by_name("accent/base").unwrap().id;
    let text = t.by_name("accent/text").unwrap();
    assert!(text.cells.values().all(|c| *c == Cell::Alias(base)));
}

/// Kinds with no home are left out and named; the rest of the file still reads.
#[test]
fn a_type_with_no_home_is_left_out() {
    let mut d = doc();
    let mut v = exported(&d);
    v["motion"] =
        json!({ "ease": { "out": { "$type": "cubicBezier", "$value": [0.2, 0, 0, 1] } } });
    v["border"] = json!({ "focus": { "$type": "border", "$value": {
        "color": "#ff0000", "width": { "value": 2, "unit": "px" }, "style": "solid" } } });
    let r = reviewed(&mut d, &v);
    let left: Vec<_> = r
        .left_out
        .iter()
        .map(|l| (l.path.as_str(), l.ty.as_str()))
        .collect();
    // In the file's order, which `json!` writes sorted.
    assert_eq!(
        left,
        [
            ("border.focus", "border"),
            ("motion.ease.out", "cubicBezier")
        ]
    );
    assert!(r.rows.is_empty());
}

/// What is not DTCG, or cannot be read, is refused with the first reason and
/// where it is.
#[test]
fn unreadable_files_are_refused_with_a_path() {
    assert!(
        parse("not json")
            .unwrap_err()
            .reason
            .starts_with("this is not JSON")
    );
    assert_eq!(
        parse("[1]").unwrap_err().reason,
        "a token file is a JSON object"
    );
    assert_eq!(
        parse("{}").unwrap_err().reason,
        "the file holds no design token"
    );
    let r =
        parse(r#"{"space": {"s": {"$type": "dimension", "$value": {"value": 4, "unit": "em"}}}}"#)
            .unwrap_err();
    assert_eq!(
        r.to_string(),
        "space.s: a dimension in em cannot be read; px or rem"
    );
    let r = parse(r#"{"a": {"$value": "{b}"}}"#).unwrap_err();
    assert_eq!(r.to_string(), "a: the token has no $type");
    let mut d = doc();
    let dangling = parse(r#"{"a": {"$type": "color", "$value": "{nowhere.at.all}"}}"#).unwrap();
    let r = review(&d.doc, &dangling, &mut d.ids).unwrap_err();
    assert_eq!(
        r.to_string(),
        "a: refers to {nowhere.at.all}, which is not in the file"
    );
}

/// A file from elsewhere — no modes, no ids — is matched by path into the
/// default mode; group types are inherited, rem is 16px, and a typography token
/// becomes a text style.
#[test]
fn a_foreign_file_matches_by_path_into_the_default_mode() {
    let mut d = doc();
    let v = json!({
        "radius": { "$type": "dimension", "m": { "$value": "0.75rem" } },
        "brand": { "500": { "$type": "color", "$value": {
            "colorSpace": "oklch", "components": [0.7, 0.1, 250] } } },
        "heading": { "$type": "typography", "$value": {
            "fontFamily": ["Inter", "sans-serif"], "fontSize": "{radius.m}",
            "fontWeight": "bold", "letterSpacing": { "value": 0, "unit": "px" }, "lineHeight": 1.25 } },
    });
    let r = reviewed(&mut d, &v);
    assert_eq!(
        row(&r, "radius.m").cells[0].new.as_ref().unwrap().text,
        "12"
    );
    assert!(
        row(&r, "radius.m").cells[1].old.is_none(),
        "Dark keeps its value"
    );
    assert_eq!(row(&r, "brand.500").change, Change::Changed);
    assert_eq!(row(&r, "heading").change, Change::Added);
    // Everything of the document's own the file lacks is offered for removal.
    assert_eq!(row(&r, "count").change, Change::Removed);
    apply(&mut d, &r, &[]);
    let heading = d
        .doc
        .styles()
        .iter()
        .find(|s| s.name == "heading")
        .expect("the style was added");
    let StyleValue::Text(t) = &heading.value else {
        panic!("a text style")
    };
    assert_eq!(
        (t.font_family.as_str(), t.font_size, t.weight),
        ("Inter", 12.0, 700)
    );
    assert!(
        d.doc.variables().get(d.count).is_some(),
        "an unticked removal keeps it"
    );
    assert_eq!(
        d.doc.variables().get(d.radius).unwrap().cells[&d.dark],
        Cell::Number(12.0)
    );
}

/// A linked file's review holds only what changed since the file was last
/// applied: a value the document changed meanwhile is not offered back.
#[test]
fn since_keeps_only_what_the_file_changed() {
    let mut d = doc();
    let applied = exported(&d);
    // The document moves on; so does the file, elsewhere.
    d.doc
        .apply(&Transaction(vec![Operation::SetVariableCell {
            id: d.count,
            mode: d.light,
            cell: Cell::Number(4.0),
        }]))
        .unwrap();
    let mut now = applied.clone();
    now["radius"]["m"]["$value"] = json!({ "value": 6, "unit": "px" });
    let mut r = reviewed(&mut d, &now);
    assert_eq!(r.rows.len(), 2, "{:?}", names(&r));
    r.since(&read(&now), &read(&applied));
    assert_eq!(names(&r), [("radius.m".to_string(), Change::Changed)]);
    assert_eq!(r.changes(), 1);
}

/// A border's width read by reference binds the stroke style to the variable,
/// and a fill bound to a colour variable stays bound — both from the export's
/// `styles` group.
#[test]
fn style_references_bind_variables() {
    let mut d = doc();
    let mut v = exported(&d);
    v["styles"]["fill"]["Brand"]["$value"] = json!("{button.bg}");
    let r = reviewed(&mut d, &v);
    let brand = row(&r, "styles.fill.Brand");
    assert_eq!(brand.change, Change::Changed);
    assert_eq!(
        brand.cells[0].new.as_ref().unwrap().text,
        "\u{2192} button.bg"
    );
    apply(&mut d, &r, &[]);
    let style = d.doc.styles().iter().find(|s| s.name == "Brand").unwrap();
    let StyleValue::Fills(f) = &style.value else {
        panic!("a fill style")
    };
    assert_eq!(f[0].brush.var(), Some(d.bg));
    let _ = d.dark;
}
