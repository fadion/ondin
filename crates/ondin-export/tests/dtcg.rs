//! `ondin_export::dtcg`: variables and styles as a DTCG `tokens.json`, read back
//! as JSON and asserted path by path.

mod common;

use ondin_core::peniko::Color;
use ondin_core::style::{Style, StyleValue};
use ondin_core::{
    Cell, Effect, EffectKind, IdSource, Mode, ModeId, Operation, Transaction, VarId, VarKind,
    Variable, keyed_by_position,
};
use ondin_export::dtcg::{EXTENSION, tokens};
use serde_json::Value;

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

fn add_mode(id: ModeId, name: &str, index: usize) -> Operation {
    Operation::AddMode {
        mode: Mode {
            id,
            name: name.into(),
        },
        index,
        cells: vec![],
    }
}

fn parse(json: &str) -> Value {
    serde_json::from_str(json).expect("the writer writes JSON")
}

/// The value at a dotted path, `$`-keys included.
fn at<'a>(v: &'a Value, path: &str) -> &'a Value {
    path.split('.').fold(v, |v, k| {
        v.get(k)
            .unwrap_or_else(|| panic!("no {k} on the way to {path}: {v}"))
    })
}

/// Two modes; a colour group, an alias into it, a number read as a radius, one
/// read as an opacity and one read by nothing.
struct Doc {
    f: common::Fixture,
    light: ModeId,
    dark: ModeId,
    radius: VarId,
}

fn doc() -> Doc {
    let mut f = common::fixture();
    let mut ids = IdSource::new(0xD7C6);
    let (light, dark) = (ids.mint_mode(), ids.mint_mode());
    let [brand, bg, radius, fade, loose] = [(); 5].map(|_| ids.mint_var());
    let red = Color::from_rgba8(255, 0, 0, 255);
    let navy = Color::from_rgba8(0, 0, 128, 255);
    let num = |a: f64, b: f64| vec![(light, Cell::Number(a)), (dark, Cell::Number(b))];
    f.doc
        .apply(&Transaction(vec![
            add_mode(light, "Light", 0),
            add_mode(dark, "Dark", 1),
            add_var(
                brand,
                "brand/500",
                VarKind::Color,
                vec![(light, Cell::Color(red)), (dark, Cell::Color(navy))],
            ),
            add_var(
                bg,
                "button/bg",
                VarKind::Color,
                vec![(light, Cell::Alias(brand)), (dark, Cell::Alias(brand))],
            ),
            add_var(radius, "radius/m", VarKind::Number, num(8.0, 12.0)),
            add_var(fade, "fade", VarKind::Number, num(50.0, 80.0)),
            add_var(loose, "count", VarKind::Number, num(3.0, 3.0)),
        ]))
        .unwrap();
    let mut ops = ondin_core::bind_corners_ops(&f.doc, f.rect, [Some(radius); 4]);
    ops.extend(ondin_core::bind_opacity_ops(&f.doc, f.rect, fade));
    f.doc.apply(&Transaction(ops)).unwrap();
    Doc {
        f,
        light,
        dark,
        radius,
    }
}

/// Slash paths are groups; a colour is the format's colour object; an alias is a
/// reference; a number read as a radius is a px dimension, as an opacity or by
/// nothing a plain number; the default mode is the `$value` and both modes ride
/// in the extension.
///
/// flip: `type_of` answering `number` for every number → fails at `radius/m`'s
/// `$type`.
#[test]
fn variables_are_grouped_typed_and_carry_every_mode() {
    let d = doc();
    let out = tokens(&d.f.doc, None);
    let v = parse(&out.json);

    assert_eq!(at(&v, "brand.500.$type"), "color");
    assert_eq!(at(&v, "brand.500.$value.hex"), "#ff0000");
    assert_eq!(at(&v, "brand.500.$value.colorSpace"), "srgb");
    let modes = &at(&v, "brand.500.$extensions")[EXTENSION]["modes"];
    assert_eq!(modes["Dark"]["hex"], "#000080");

    assert_eq!(at(&v, "button.bg.$value"), "{brand.500}");

    assert_eq!(at(&v, "radius.m.$type"), "dimension");
    assert_eq!(at(&v, "radius.m.$value.value"), 8.0);
    assert_eq!(at(&v, "radius.m.$value.unit"), "px");
    assert_eq!(
        at(&v, "radius.m.$extensions")[EXTENSION]["id"],
        d.radius.0.to_wire()
    );
    assert_eq!(at(&v, "fade.$type"), "number");
    assert_eq!(at(&v, "fade.$value"), 50.0);
    assert_eq!(at(&v, "count.$type"), "number");

    let ext = &v["$extensions"][EXTENSION];
    assert_eq!(ext["valueMode"], "Light");
    assert_eq!(ext["modes"][1]["name"], "Dark");
    assert!(out.notes.is_empty(), "{:?}", out.notes);
}

/// Asked for another mode, its values are the `$value`s.
#[test]
fn another_mode_writes_its_own_values() {
    let d = doc();
    let v = parse(&tokens(&d.f.doc, Some(d.dark)).json);
    assert_eq!(at(&v, "brand.500.$value.hex"), "#000080");
    assert_eq!(at(&v, "radius.m.$value.value"), 12.0);
    assert_eq!(v["$extensions"][EXTENSION]["valueMode"], "Dark");
    // A mode the document does not have falls back to the default.
    let mut ids = IdSource::new(0x99);
    let stray = ids.mint_mode();
    let v = parse(&tokens(&d.f.doc, Some(stray)).json);
    assert_eq!(at(&v, "radius.m.$value.value"), 8.0);
    let _ = d.light;
}

/// An alias and its target share one type: a number read only through an alias
/// bound to a radius is a dimension, and so is the alias.
///
/// flip: `number_reads` pooling the alias's reads into its target only → fails at
/// `space/alias`'s `$type`, the sibling alias that nothing reads.
#[test]
fn an_alias_and_its_target_share_a_type() {
    let mut f = common::fixture();
    let mut ids = IdSource::new(0xA1);
    let m = ids.mint_mode();
    let [base, alias, unread] = [(); 3].map(|_| ids.mint_var());
    f.doc
        .apply(&Transaction(vec![
            add_mode(m, "Mode", 0),
            add_var(
                base,
                "space/4",
                VarKind::Number,
                vec![(m, Cell::Number(4.0))],
            ),
            add_var(
                unread,
                "space/alias",
                VarKind::Number,
                vec![(m, Cell::Alias(base))],
            ),
            add_var(alias, "gap", VarKind::Number, vec![(m, Cell::Alias(base))]),
        ]))
        .unwrap();
    let ops = ondin_core::bind_corners_ops(&f.doc, f.rect, [Some(alias); 4]);
    f.doc.apply(&Transaction(ops)).unwrap();
    let v = parse(&tokens(&f.doc, None).json);
    assert_eq!(at(&v, "space.4.$type"), "dimension");
    assert_eq!(at(&v, "gap.$type"), "dimension");
    assert_eq!(at(&v, "gap.$value"), "{space.4}");
    assert_eq!(at(&v, "space.alias.$type"), "dimension");
}

/// A token whose path is also a group becomes that group's `$root`; a name with a
/// reserved character is renamed and the rename noted; strings and booleans ride
/// in the root's extension.
#[test]
fn awkward_names_and_untyped_kinds_find_a_place() {
    let mut f = common::fixture();
    let mut ids = IdSource::new(0xA2);
    let m = ids.mint_mode();
    let [a, b, c, s, flag] = [(); 5].map(|_| ids.mint_var());
    f.doc
        .apply(&Transaction(vec![
            add_mode(m, "Mode", 0),
            add_var(a, "brand", VarKind::Number, vec![(m, Cell::Number(1.0))]),
            add_var(
                b,
                "brand/strong",
                VarKind::Number,
                vec![(m, Cell::Number(2.0))],
            ),
            add_var(c, "v1.5/x", VarKind::Number, vec![(m, Cell::Number(3.0))]),
            add_var(
                s,
                "copy/title",
                VarKind::String,
                vec![(m, Cell::String("Hi".into()))],
            ),
            add_var(
                flag,
                "show",
                VarKind::Boolean,
                vec![(m, Cell::Boolean(true))],
            ),
        ]))
        .unwrap();
    let out = tokens(&f.doc, None);
    let v = parse(&out.json);
    assert_eq!(at(&v, "brand.$root.$value"), 1.0);
    assert_eq!(at(&v, "brand.strong.$value"), 2.0);
    assert_eq!(at(&v, "v1_5.x.$value"), 3.0);
    assert!(
        out.notes.iter().any(|n| n.starts_with("v1.5/x")),
        "{:?}",
        out.notes
    );
    let loose = &v["$extensions"][EXTENSION]["variables"];
    assert_eq!(loose["copy/title"]["modes"]["Mode"], "Hi");
    assert_eq!(loose["show"]["kind"], "boolean");
}

/// A colour fill style is a colour token, an effects style of shadows a shadow
/// token (an inner one `inset`), a type style a typography token; a layer blur
/// has no DTCG form and its style is left out, said so.
#[test]
fn styles_become_composite_tokens_or_a_note() {
    let mut f = common::fixture();
    let mut ids = IdSource::new(0xA3);
    let style = |ids: &mut IdSource, name: &str, value: StyleValue| Operation::AddStyle {
        style: Style {
            id: ids.mint_style(),
            name: name.into(),
            value,
            origin: None,
            forked: None,
            description: String::new(),
        },
    };
    let shadow = |kind| Effect {
        kind,
        visible: true,
        vars: Default::default(),
    };
    let drop = ondin_core::Shadow {
        offset: ondin_core::kurbo::Vec2::new(0.0, 4.0),
        blur: 12.0,
        spread: 0.0,
        color: Color::from_rgba8(0, 0, 0, 64),
    };
    let ops = vec![
        style(
            &mut ids,
            "Surface/Card",
            StyleValue::Fills(keyed_by_position([ondin_core::Fill {
                brush: ondin_core::Brush::Solid(Color::from_rgba8(255, 255, 255, 255)).into(),
                visible: true,
            }])),
        ),
        style(
            &mut ids,
            "Elevation/1",
            StyleValue::Effects(keyed_by_position([
                shadow(EffectKind::DropShadow(drop.clone())),
                shadow(EffectKind::InnerShadow(drop)),
            ])),
        ),
        style(
            &mut ids,
            "Frosted",
            StyleValue::Effects(keyed_by_position([shadow(EffectKind::LayerBlur {
                radius: 8.0,
            })])),
        ),
        style(
            &mut ids,
            "Body",
            StyleValue::Text(Box::new(ondin_core::TextStyle {
                font_family: "Inter".into(),
                font_size: 16.0,
                weight: 400,
                line_height: Some(ondin_core::Length::Em(1.5)),
                letter_spacing: ondin_core::Length::Em(0.01),
                ..Default::default()
            })),
        ),
    ];
    f.doc.apply(&Transaction(ops)).unwrap();
    let out = tokens(&f.doc, None);
    let v = parse(&out.json);
    assert_eq!(at(&v, "styles.fill.Surface.Card.$type"), "color");
    assert_eq!(at(&v, "styles.fill.Surface.Card.$value.hex"), "#ffffff");
    let shadows = at(&v, "styles.effect.Elevation.1.$value");
    assert_eq!(shadows[0]["blur"]["value"], 12.0);
    assert_eq!(shadows[0]["inset"], false);
    assert_eq!(shadows[1]["inset"], true);
    assert_eq!(at(&v, "styles.type.Body.$type"), "typography");
    assert_eq!(at(&v, "styles.type.Body.$value.lineHeight"), 1.5);
    assert_eq!(at(&v, "styles.type.Body.$value.letterSpacing.value"), 0.16);
    assert!(v["styles"]["effect"].get("Frosted").is_none());
    assert!(
        out.notes.iter().any(|n| n.starts_with("Frosted")),
        "{:?}",
        out.notes
    );
}
