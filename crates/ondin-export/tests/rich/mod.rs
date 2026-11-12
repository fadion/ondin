//! The document the editable exports (`penpot`, `sketch`) are tested on, and
//! what reading their zips back takes.

// Compiled once per test file that declares it, each using a part: `goldens`
// only reads zips back.
#![allow(dead_code)]

use ondin_core::container::{Display, Flex, LayoutItem};
use ondin_core::foreign::Archive;
use ondin_core::kurbo::{Affine, Point, RoundedRectRadii, Size};
use ondin_core::peniko::{Brush, Color, Gradient};
use ondin_core::style::{Style, StyleValue};
use ondin_core::svg_in::ImageSink;
use ondin_core::variable::{Cell, Mode, Variable};
use ondin_core::{
    BrushSource, CharAttr, CharSpans, Document, Fill, IdSource, ImageEntry, ImageFormat, ImageId,
    ImageSource, NodeId, NodeKind, Operation, Resolved, TextSizing, TextStyle, Transaction,
    VarKind, build::Placement, keyed_by_position,
};
use serde_json::Value;
use std::collections::BTreeMap;

pub struct Fixture {
    pub doc: Document,
    pub res: Resolved,
}

fn solid(id: NodeId, rgb: [u8; 3]) -> Operation {
    Operation::SetFills {
        id,
        fills: keyed_by_position([Fill {
            brush: Brush::Solid(Color::from_rgb8(rgb[0], rgb[1], rgb[2])).into(),
            visible: true,
        }]),
    }
}

fn rect(id: NodeId, parent: NodeId, index: usize, size: (f64, f64), at: Affine) -> Operation {
    Operation::CreateNode {
        id,
        parent,
        index,
        kind: NodeKind::Rect {
            size: Size::new(size.0, size.1),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        },
        transform: Some(at),
        name: None,
    }
}

fn text(content: &str, size: f64) -> NodeKind {
    NodeKind::Text {
        content: content.into(),
        style: Box::new(TextStyle {
            font_family: "Inter".into(),
            font_size: size,
            ..TextStyle::default()
        }),
        spans: CharSpans::default(),
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing: TextSizing::Auto,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    }
}

/// A 4×4 PNG, so a picture fill has bytes to carry.
fn png() -> Vec<u8> {
    let img = image::RgbaImage::from_pixel(4, 4, image::Rgba([200, 40, 90, 255]));
    let mut out = std::io::Cursor::new(Vec::new());
    img.write_to(&mut out, image::ImageFormat::Png)
        .expect("encode");
    out.into_inner()
}

/// A 20 × 20 *Icon* component whose *Art*, a plain group pinned 20% in from
/// every side, holds art its group scales: the rect *Bar*, the rect *Dot* in
/// the plain group *Inner*, and the rect *Nib* in the boolean *Badge*.
pub fn pinned_icon() -> Fixture {
    let mut ids = IdSource::new(0x1C0);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let [icon, art, bar, inner, dot, badge, nib] = [(); 7].map(|_| ids.mint());
    let pct = Some(ondin_core::LengthPct::Percent(20.0));
    let named = |id, name: &str| Operation::SetName {
        id,
        name: name.into(),
    };
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: icon,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(20.0, 20.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some("Icon".into()),
        },
        Operation::CreateNode {
            id: art,
            parent: icon,
            index: 0,
            kind: NodeKind::Group,
            transform: None,
            name: Some("Art".into()),
        },
        rect(bar, art, 0, (12.0, 4.0), Affine::translate((4.0, 4.0))),
        Operation::CreateNode {
            id: inner,
            parent: art,
            index: 1,
            kind: NodeKind::Group,
            transform: None,
            name: Some("Inner".into()),
        },
        rect(dot, inner, 0, (4.0, 4.0), Affine::translate((12.0, 12.0))),
        Operation::CreateNode {
            id: badge,
            parent: art,
            index: 2,
            kind: NodeKind::Boolean {
                op: ondin_core::node::BoolOp::Union,
            },
            transform: None,
            name: Some("Badge".into()),
        },
        rect(nib, badge, 0, (2.0, 2.0), Affine::translate((14.0, 4.0))),
        named(bar, "Bar"),
        named(dot, "Dot"),
        named(nib, "Nib"),
        solid(bar, [40, 40, 40]),
        solid(dot, [40, 40, 40]),
        solid(badge, [40, 40, 40]),
        Operation::SetInsets {
            id: art,
            insets: ondin_core::Insets {
                top: pct,
                right: pct,
                bottom: pct,
                left: pct,
                ..Default::default()
            },
        },
        Operation::SetComponent {
            id: icon,
            component: true,
        },
    ]))
    .expect("the icon");
    let res = Resolved::rebuild(&doc);
    Fixture { doc, res }
}

/// A page with a flex row, a component and an instance whose label is
/// overridden, a fill bound to a colour variable, a text style, a mask over a
/// picture, a turned rect, a radial gradient and a hidden, locked layer.
///
/// Past what both files hold: the row is half opaque and shadowed, as is the
/// button; a second copy, turned and mirrored, makes part of its label bold;
/// the row's first rect has a hidden fill, the mask a stroke; the screen has an
/// export setting and a layout grid, the page a guide and a slice.
pub fn fixture() -> Fixture {
    let mut ids = IdSource::new(0x7E7);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (mode, brand, heading) = (ids.mint_mode(), ids.mint_var(), ids.mint_style());
    let [
        screen,
        row,
        a,
        b,
        swatch,
        button,
        label,
        title,
        masked,
        mask,
        photo,
        turned,
        glow,
        ghost,
        slice,
    ] = [(); 15].map(|_| ids.mint());
    let guide = ondin_core::GuideId(ids.mint());
    let shadow = |id: NodeId| Operation::SetEffects {
        id,
        effects: keyed_by_position([ondin_core::Effect::new(ondin_core::EffectKind::DropShadow(
            ondin_core::Shadow {
                offset: ondin_core::kurbo::Vec2::new(0.0, 4.0),
                blur: 8.0,
                spread: 0.0,
                color: Color::from_rgba8(0, 0, 0, 80),
                blend: ondin_core::BlendMode::Normal,
            },
        ))]),
    };
    let picture = ImageId("test:photo".into());
    let mut bold = CharSpans::default();
    let title_style = TextStyle {
        font_family: "Inter".into(),
        font_size: 24.0,
        ..TextStyle::default()
    };
    // The first run keeps the style's type, so a reader binds the style by it.
    bold.set(
        0..5,
        CharAttr::Color(Some(Color::from_rgb8(200, 0, 0))),
        &title_style,
    );
    bold.set(6..11, CharAttr::Weight(700), &title_style);
    doc.apply(&Transaction(vec![
        Operation::AddMode {
            collection: ondin_core::variable::CollectionId::FIRST,
            mode: Mode {
                id: mode,
                name: "Default".into(),
            },
            index: 0,
            cells: vec![],
        },
        Operation::AddVariable {
            var: Variable {
                id: brand,
                name: "Brand/Primary".into(),
                kind: VarKind::Color,
                cells: [(mode, Cell::Color(Color::from_rgb8(30, 90, 200)))].into(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        },
        Operation::AddStyle {
            style: Style {
                id: heading,
                name: "Type/Heading".into(),
                value: StyleValue::Text(Box::new(ondin_core::style::type_part(&title_style))),
                origin: None,
                forked: None,
                description: String::new(),
            },
        },
        Operation::AddImage {
            id: picture.clone(),
            entry: ImageEntry {
                source: ImageSource::Embedded(png().into()),
                format: ImageFormat::Png,
                width: 4,
                height: 4,
            },
        },
        Operation::CreateNode {
            id: screen,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(480.0, 360.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((40.0, 30.0))),
            name: Some("Screen".into()),
        },
        solid(screen, [250, 250, 250]),
        // A flex row of two rects, the second growing.
        Operation::CreateNode {
            id: row,
            parent: screen,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(300.0, 60.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((20.0, 20.0))),
            name: Some("Row".into()),
        },
        Operation::SetDisplay {
            id: row,
            display: Some(Display::Flex(Flex {
                column_gap: 12.0,
                padding: [8.0, 10.0, 8.0, 10.0],
                ..Flex::default()
            })),
        },
        rect(a, row, 0, (40.0, 40.0), Affine::IDENTITY),
        solid(a, [10, 150, 120]),
        rect(b, row, 1, (40.0, 40.0), Affine::IDENTITY),
        solid(b, [240, 160, 20]),
        Operation::SetLayoutItem {
            id: b,
            item: LayoutItem {
                grow: 1.0,
                ..LayoutItem::default()
            },
        },
        // A swatch bound to the brand colour.
        rect(
            swatch,
            screen,
            1,
            (50.0, 50.0),
            Affine::translate((20.0, 100.0)),
        ),
        Operation::SetFills {
            id: swatch,
            fills: keyed_by_position([Fill {
                brush: BrushSource::Bound {
                    var: brand,
                    alpha: 1.0,
                },
                visible: true,
            }]),
        },
        // A main component: a frame holding a label.
        Operation::CreateNode {
            id: button,
            parent: screen,
            index: 2,
            kind: NodeKind::Artboard {
                size: Size::new(120.0, 40.0),
                corner_radii: RoundedRectRadii::from_single_radius(8.0),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((100.0, 100.0))),
            name: Some("Button".into()),
        },
        solid(button, [30, 30, 40]),
        Operation::CreateNode {
            id: label,
            parent: button,
            index: 0,
            kind: text("Press", 14.0),
            transform: Some(Affine::translate((12.0, 10.0))),
            name: Some("Label".into()),
        },
        solid(label, [255, 255, 255]),
        Operation::SetComponent {
            id: button,
            component: true,
        },
        // A title in the heading style, with a bold run and a red one.
        Operation::CreateNode {
            id: title,
            parent: screen,
            index: 3,
            kind: NodeKind::Text {
                content: "Hello world\nSecond line".into(),
                style: Box::new(title_style.clone()),
                spans: bold,
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: Some(Affine::translate((20.0, 180.0))),
            name: Some("Title".into()),
        },
        solid(title, [20, 20, 30]),
        Operation::SetTypeStyle {
            id: title,
            style: Some(heading),
        },
        // A circle masking a picture.
        Operation::CreateNode {
            id: masked,
            parent: screen,
            index: 4,
            kind: NodeKind::Group,
            transform: None,
            name: Some("Avatar".into()),
        },
        Operation::CreateNode {
            id: mask,
            parent: masked,
            index: 0,
            kind: NodeKind::Ellipse {
                size: Size::new(60.0, 60.0),
                arc: Default::default(),
            },
            transform: Some(Affine::translate((300.0, 100.0))),
            name: Some("Clip".into()),
        },
        solid(mask, [0, 0, 0]),
        Operation::SetMask {
            id: mask,
            mask: true,
        },
        rect(
            photo,
            masked,
            1,
            (80.0, 80.0),
            Affine::translate((290.0, 90.0)),
        ),
        Operation::SetFills {
            id: photo,
            fills: keyed_by_position([Fill {
                brush: ondin_core::image_brush(picture).into(),
                visible: true,
            }]),
        },
        // A rect turned a quarter of the way round, and a radial glow.
        rect(
            turned,
            screen,
            5,
            (60.0, 30.0),
            Affine::translate((380.0, 220.0)) * Affine::rotate(30f64.to_radians()),
        ),
        solid(turned, [120, 60, 200]),
        Operation::SetName {
            id: turned,
            name: "Turned".into(),
        },
        Operation::CreateNode {
            id: glow,
            parent: screen,
            index: 6,
            kind: NodeKind::Ellipse {
                size: Size::new(100.0, 60.0),
                arc: Default::default(),
            },
            transform: Some(Affine::translate((40.0, 260.0))),
            name: Some("Glow".into()),
        },
        Operation::SetFills {
            id: glow,
            fills: keyed_by_position([Fill {
                brush: Brush::Gradient(
                    Gradient::new_radial(Point::new(50.0, 30.0), 30.0)
                        .with_stops([
                            (0.0_f32, Color::from_rgb8(255, 220, 0)),
                            (1.0_f32, Color::from_rgb8(255, 80, 0)),
                        ])
                        .into(),
                )
                .into(),
                visible: true,
            }]),
        },
        rect(
            ghost,
            screen,
            7,
            (20.0, 20.0),
            Affine::translate((440.0, 20.0)),
        ),
        solid(ghost, [0, 0, 0]),
        Operation::SetVisible {
            id: ghost,
            visible: false,
        },
        Operation::SetLocked {
            id: ghost,
            locked: true,
        },
        Operation::SetOpacity {
            id: row,
            opacity: 0.5,
        },
        shadow(row),
        shadow(button),
        Operation::SetFills {
            id: a,
            fills: keyed_by_position([
                Fill {
                    brush: Brush::Solid(Color::from_rgb8(10, 150, 120)).into(),
                    visible: true,
                },
                Fill {
                    brush: Brush::Solid(Color::from_rgb8(255, 0, 0)).into(),
                    visible: false,
                },
            ]),
        },
        Operation::SetStrokes {
            id: mask,
            strokes: keyed_by_position([ondin_core::Stroke {
                brush: Brush::Solid(Color::from_rgb8(0, 0, 0)).into(),
                width: 2.0,
                ..Default::default()
            }]),
        },
        Operation::SetExports {
            id: screen,
            exports: keyed_by_position([ondin_core::ExportSpec::new(
                ondin_core::ExportFormat::Png,
                ondin_core::ExportScale::Times(2.0),
            )]),
        },
        Operation::SetLayoutGrids {
            id: screen,
            grids: keyed_by_position([ondin_core::LayoutGrid::new(ondin_core::GridAxis::Columns)]),
        },
        Operation::AddGuide {
            guide: ondin_core::Guide {
                id: guide,
                axis: ondin_core::GuideAxis::Vertical,
                position: 240.0,
                color: None,
                owner: screen,
            },
        },
        Operation::CreateNode {
            id: slice,
            parent: root,
            index: 1,
            kind: NodeKind::Slice {
                size: Size::new(100.0, 100.0),
            },
            transform: Some(Affine::translate((600.0, 30.0))),
            name: Some("Crop".into()),
        },
    ]))
    .expect("build the fixture");

    // An instance of the button, its label overridden.
    let (tx, made) = ondin_core::insert_subtrees(
        &doc,
        &mut ids,
        &[Placement {
            nodes: doc.capture_subtree(button).expect("the main"),
            parent: screen,
            index: None,
        }],
        Default::default(),
    );
    doc.apply(&tx).expect("an instance");
    let instance = made[0];
    let copy_label = doc.get(instance).unwrap().children()[0];
    doc.apply(&Transaction(vec![
        Operation::SetTransform {
            id: instance,
            transform: Affine::translate((100.0, 260.0)),
        },
        Operation::SetText {
            id: copy_label,
            content: "Go".into(),
            spans: Default::default(),
            para_spans: Default::default(),
        },
    ]))
    .expect("override the copy");

    // Another, turned 30° and mirrored, its label's first two letters bold.
    let (tx, made) = ondin_core::insert_subtrees(
        &doc,
        &mut ids,
        &[Placement {
            nodes: doc.capture_subtree(button).expect("the main"),
            parent: screen,
            index: None,
        }],
        Default::default(),
    );
    doc.apply(&tx).expect("a second instance");
    let flipped = made[0];
    let flipped_label = doc.get(flipped).unwrap().children()[0];
    let label_style = match doc.get(flipped_label).unwrap().kind() {
        NodeKind::Text { style, .. } => style.as_ref().clone(),
        _ => unreachable!("the label is a text"),
    };
    let mut strong = CharSpans::default();
    strong.set(0..2, CharAttr::Weight(700), &label_style);
    doc.apply(&Transaction(vec![
        Operation::SetTransform {
            id: flipped,
            transform: Affine::translate((330.0, 160.0))
                * Affine::rotate(30f64.to_radians())
                * Affine::scale_non_uniform(-1.0, 1.0),
        },
        Operation::SetName {
            id: flipped,
            name: "Flipped".into(),
        },
        Operation::SetText {
            id: flipped_label,
            content: "Press".into(),
            spans: strong,
            para_spans: Default::default(),
        },
    ]))
    .expect("turn and embolden the second copy");
    let res = Resolved::rebuild(&doc);
    Fixture { doc, res }
}

/// A written file's entries, JSON parsed.
pub struct Unzipped {
    pub json: BTreeMap<String, Value>,
    pub raw: BTreeMap<String, Vec<u8>>,
}

pub fn unzip(bytes: &[u8]) -> Unzipped {
    use std::io::Read;
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).expect("a zip");
    let (mut json, mut raw) = (BTreeMap::new(), BTreeMap::new());
    for i in 0..zip.len() {
        let mut f = zip.by_index(i).unwrap();
        let mut v = Vec::new();
        f.read_to_end(&mut v).unwrap();
        let name = f.name().to_owned();
        if name.ends_with(".json") {
            json.insert(name.clone(), serde_json::from_slice(&v).expect("JSON"));
        }
        raw.insert(name, v);
    }
    Unzipped { json, raw }
}

pub fn s<'a>(v: &'a Value, k: &str) -> &'a str {
    v.get(k).and_then(Value::as_str).unwrap_or("")
}

/// A written zip, for a reader.
pub struct Zip(pub zip::ZipArchive<std::io::Cursor<Vec<u8>>>);

impl Zip {
    pub fn of(bytes: Vec<u8>) -> Self {
        Self(zip::ZipArchive::new(std::io::Cursor::new(bytes)).expect("a zip"))
    }
}

impl Archive for Zip {
    fn entry(&mut self, name: &str) -> Option<Vec<u8>> {
        use std::io::Read;
        let mut f = self.0.by_name(name).ok()?;
        let mut v = Vec::new();
        f.read_to_end(&mut v).ok()?;
        Some(v)
    }
    fn names(&mut self) -> Vec<String> {
        self.0.file_names().map(str::to_owned).collect()
    }
}

/// Pictures read back, each taken as the fixture's 4×4.
pub struct Pictures;

impl ImageSink for Pictures {
    fn place(&mut self, bytes: Vec<u8>, format: ImageFormat) -> Option<(ImageId, ImageEntry)> {
        Some((
            ImageId(format!("back:{}", bytes.len())),
            ImageEntry {
                source: ImageSource::Embedded(bytes.into()),
                format,
                width: 4,
                height: 4,
            },
        ))
    }
}

/// The first layer named `name`, on any page.
pub fn named(doc: &Document, name: &str) -> NodeId {
    let found: Vec<NodeId> = doc
        .page_ids()
        .flat_map(|p| ondin_core::build::subtree_nodes(doc, &[p]))
        .filter(|&id| doc.get(id).is_some_and(|n| n.name() == name))
        .collect();
    assert!(!found.is_empty(), "a layer named {name:?}");
    found[0]
}

/// Whether `name`'s box in `back`, read from a file, is where it is in `f`:
/// each corner of its own box on the page, in order, so a turn of the wrong
/// sign or a mirror shows where the bounds alone would not.
pub fn same_place(f: &Fixture, back: &Document, name: &str) {
    let res = Resolved::rebuild(back);
    let corners = |doc: &Document, res: &Resolved, id: NodeId| {
        let b = ondin_core::local_box(doc, res, id).expect("a box");
        let m = res.world_transform(id).expect("placed");
        [(b.x0, b.y0), (b.x1, b.y0), (b.x1, b.y1), (b.x0, b.y1)].map(|(x, y)| m * Point::new(x, y))
    };
    let wa = corners(&f.doc, &f.res, named(&f.doc, name));
    let wb = corners(back, &res, named(back, name));
    for (p, q) in wa.iter().zip(&wb) {
        assert!(
            (*p - *q).hypot() < 0.5,
            "{name}: {wa:?} came back as {wb:?}"
        );
    }
}
