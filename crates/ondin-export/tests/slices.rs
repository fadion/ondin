//! What a slice exports: its own box of the page, from whatever layers lie under
//! it, cut at its edges — at its size, never trimmed, and with the box as an SVG's
//! `viewBox`.

use ondin_core::kurbo::{Affine, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, ExportFormat, ExportScale, ExportSpec, Fill, IdSource, NodeId, NodeKind,
    Operation, Resolved, Transaction, keyed_by_position,
};
use ondin_export::plan::{self, PlanOptions};

const RED: Color = Color::from_rgba8(220, 30, 40, 255);

fn solid(c: Color) -> Vec<ondin_core::Keyed<Fill>> {
    keyed_by_position([Fill {
        brush: Brush::Solid(c).into(),
        visible: true,
    }])
}

/// A white 400×300 frame holding a 100×50 red rect at (150, 125), and a 100×50
/// slice at (100, 100) over the rect's top-left corner and the frame round it.
fn frame_with_a_slice() -> (Document, Resolved, NodeId) {
    let mut ids = IdSource::new(0x511C);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (frame, rect, slice) = (ids.mint(), ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: frame,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 300.0),
                corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some("Board".into()),
        },
        Operation::SetFills {
            id: frame,
            fills: solid(Color::WHITE),
        },
        Operation::CreateNode {
            id: rect,
            parent: frame,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 50.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate((150.0, 125.0))),
            name: None,
        },
        Operation::SetFills {
            id: rect,
            fills: solid(RED),
        },
        Operation::CreateNode {
            id: slice,
            parent: frame,
            index: 1,
            kind: NodeKind::Slice {
                size: Size::new(100.0, 50.0),
            },
            transform: Some(Affine::translate((100.0, 100.0))),
            name: Some("Crop".into()),
        },
    ]))
    .expect("the fixture");
    let res = Resolved::rebuild(&doc);
    (doc, res, slice)
}

fn planned(doc: &Document, res: &Resolved, slice: NodeId, spec: ExportSpec) -> plan::PlannedFile {
    let opts = PlanOptions {
        unset: Some(spec),
        ..Default::default()
    };
    plan::plan(doc, res, &[slice], opts).remove(0)
}

/// The raster is the slice's box at its scale, the frame's white and the rect's red
/// where each lies under it, and a trim asked for is not applied.
#[test]
fn a_slice_rasters_its_box_of_the_page() {
    let (doc, res, slice) = frame_with_a_slice();
    let mut spec = ExportSpec::new(ExportFormat::Png, ExportScale::Times(1.0));
    spec.trim = true;
    let file = planned(&doc, &res, slice, spec.clone());
    assert_eq!(file.path, "Crop.png", "named after the slice");
    assert_eq!(file.pixels, Some((100, 50)));

    let (view, origins) = plan::slice_region(&doc, &res, slice).expect("a slice");
    let opts = plan::raster_opts(&doc, &res, slice, &spec);
    assert!(!opts.trim, "a slice's box is never trimmed");
    let r = ondin_export::png::raster_region(&doc, &res, &origins, view, &opts);
    assert_eq!((r.width, r.height), (100, 50));
    let px = |x: u32, y: u32| {
        let i = ((y * r.width + x) * 4) as usize;
        [r.rgba[i], r.rgba[i + 1], r.rgba[i + 2], r.rgba[i + 3]]
    };
    assert_eq!(px(75, 40), [220, 30, 40, 255], "the rect, under the slice");
    assert_eq!(
        px(20, 10),
        [255, 255, 255, 255],
        "the frame's fill round it"
    );

    spec.scale = ExportScale::Times(2.0);
    assert_eq!(planned(&doc, &res, slice, spec).pixels, Some((200, 100)));
}

/// An SVG of a slice takes the slice's box as its `viewBox`, its corner moved to
/// `0 0`, and draws the layers under it, the slice itself not at all.
/// flip: `plan::slice_region` answering `None` → fails at "the layers under it are
/// drawn".
#[test]
fn a_slice_frames_an_svg_on_its_box() {
    let (doc, res, slice) = frame_with_a_slice();
    let file = planned(
        &doc,
        &res,
        slice,
        ExportSpec::new(ExportFormat::Svg, ExportScale::Times(1.0)),
    );
    let svg = String::from_utf8(plan::render(&doc, &res, &file).expect("rendered")).unwrap();
    assert!(
        svg.contains(r#"viewBox="0 0 100 50""#),
        "framed on the slice: {svg}"
    );
    assert!(
        svg.contains("<rect"),
        "the layers under it are drawn: {svg}"
    );
}

/// A PDF of a slice is a page the slice's size.
///
/// flip: `plan::render`'s PDF arm framing on the slice's origins instead of its
/// box → fails at the media box.
#[test]
fn a_slice_frames_a_pdf_on_its_box() {
    let (doc, res, slice) = frame_with_a_slice();
    let file = planned(
        &doc,
        &res,
        slice,
        ExportSpec::new(ExportFormat::Pdf, ExportScale::Times(1.0)),
    );
    let pdf = plan::render(&doc, &res, &file).expect("rendered");
    let text = String::from_utf8_lossy(&pdf);
    assert!(text.contains("/MediaBox[0 0 100 50]"), "the media box");
}

/// A slice loose on a page sizes none of the page's code export: the page is the
/// rect's box, as its SVG and PNG are.
///
/// flip: `code::walk::page` framing on every origin, the slice's box included →
/// fails at the page's size.
#[test]
fn a_loose_slice_sizes_no_code_export() {
    let mut ids = IdSource::new(0x511D);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (rect, slice) = (ids.mint(), ids.mint());
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: rect,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 100.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        },
        Operation::SetFills {
            id: rect,
            fills: solid(RED),
        },
        Operation::CreateNode {
            id: slice,
            parent: root,
            index: 1,
            kind: NodeKind::Slice {
                size: Size::new(100.0, 100.0),
            },
            transform: Some(Affine::translate((500.0, 400.0))),
            name: Some("Crop".into()),
        },
    ]))
    .expect("the fixture");
    let res = Resolved::rebuild(&doc);
    let export = ondin_export::code::html::html_of(&doc, &res, &[rect, slice]);
    let css = String::from_utf8(export.file("styles.css").unwrap().to_vec()).unwrap();
    let page = &css[css.find(".page").expect("the page's rule")..];
    let page = &page[..page.find('}').unwrap()];
    assert!(
        page.contains("width: 100px") && page.contains("height: 100px"),
        "the page's size: {page}"
    );
}
