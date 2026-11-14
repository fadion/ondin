//! What an ICO or ICNS export spec writes (`plan::render` through `icon`): one
//! square PNG per size, drawn from the vector at that size, from a frame named
//! for the size when asked, and pixel-fitted on a copy when asked.

use ondin_core::kurbo::{Affine, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, ExportBackground, ExportFormat, ExportScale, ExportSpec, Fill, IdSource,
    NodeId, NodeKind, Operation, Resolved, Transaction, keyed_by_position,
};
use ondin_export::icon;

const INK: Color = Color::from_rgba8(30, 40, 50, 255);

fn rect(
    id: NodeId,
    parent: NodeId,
    index: usize,
    at: (f64, f64),
    size: (f64, f64),
) -> [Operation; 2] {
    [
        Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Rect {
                size: Size::new(size.0, size.1),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate(at)),
            name: None,
        },
        Operation::SetFills {
            id,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(INK).into(),
                visible: true,
            }]),
        },
    ]
}

/// A 47.3 × 21 box drawn as a path named `logo` at a fractional place (a path,
/// since the renderer snaps a rect's own box to whole pixels), and a 16×16 frame
/// named `logo-16` holding a 4×4 rect at its corner. Answers the document, its
/// resolution, the path and the frame.
fn fixture() -> (Document, Resolved, NodeId, NodeId) {
    let mut ids = IdSource::new(0x1C0);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (logo, frame, dot) = (ids.mint(), ids.mint(), ids.mint());
    let mut ops: Vec<Operation> = rect(logo, root, 0, (10.3, 5.2), (47.3, 21.0)).into();
    let mut path = ondin_core::kurbo::BezPath::new();
    path.move_to((0.0, 0.0));
    path.line_to((47.3, 0.0));
    path.line_to((47.3, 21.0));
    path.line_to((0.0, 21.0));
    path.close_path();
    ops[0] = Operation::CreateNode {
        id: logo,
        parent: root,
        index: 0,
        kind: NodeKind::Path {
            path,
            corner_radii: Vec::new(),
            corner_smoothing: 0.0,
            mirror: ondin_core::symmetry::PathMirror::NONE,
        },
        transform: Some(Affine::translate((10.3, 5.2))),
        name: Some("logo".into()),
    };
    ops.push(Operation::CreateNode {
        id: frame,
        parent: root,
        index: 1,
        kind: NodeKind::Artboard {
            size: Size::new(16.0, 16.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        },
        transform: Some(Affine::translate((200.0, 0.0))),
        name: Some("logo-16".into()),
    });
    ops.extend(rect(dot, frame, 0, (0.0, 0.0), (4.0, 4.0)));
    doc.apply(&Transaction(ops)).expect("the fixture");
    let res = Resolved::rebuild(&doc);
    (doc, res, logo, frame)
}

fn ico() -> ExportSpec {
    ExportSpec::new(ExportFormat::Ico, ExportScale::Times(1.0))
}

/// Each ICO record's side, and its PNG decoded to (width, height, RGBA).
fn read_ico(bytes: &[u8]) -> Vec<(u32, u32, u32, Vec<u8>)> {
    let count = u16::from_le_bytes([bytes[4], bytes[5]]) as usize;
    (0..count)
        .map(|i| {
            let rec = &bytes[6 + 16 * i..6 + 16 * (i + 1)];
            let side = match rec[0] {
                0 => 256,
                s => u32::from(s),
            };
            let len = u32::from_le_bytes(rec[8..12].try_into().unwrap()) as usize;
            let at = u32::from_le_bytes(rec[12..16].try_into().unwrap()) as usize;
            let img = image::load_from_memory(&bytes[at..at + len])
                .expect("each entry is a PNG")
                .to_rgba8();
            (side, img.width(), img.height(), img.into_raw())
        })
        .collect()
}

fn alpha(rgba: &[u8], side: u32, x: u32, y: u32) -> u8 {
    rgba[((y * side + x) * 4 + 3) as usize]
}

/// A new ICO row writes the favicon set, each entry square at its own side, the
/// wide rect centred in it with clear rows above and below.
/// flip: `icon::square_of` answering `view` unsquared → fails at "the entry is
/// square" (the renderer is handed a 2:1 view at N × N and stretches it).
#[test]
fn an_ico_holds_each_size_square_and_centred() {
    let (doc, res, logo, _) = fixture();
    let file = &ondin_export::plan(
        &doc,
        &res,
        &[logo],
        ondin_export::PlanOptions {
            unset: Some(ico()),
            ..Default::default()
        },
    )[0];
    assert_eq!(file.path, "logo.ico");
    assert_eq!(file.pixels, Some((48, 48)), "the largest entry");
    let bytes = ondin_export::plan::render(&doc, &res, file).expect("an icon file");
    let entries = read_ico(&bytes);
    assert_eq!(
        entries.iter().map(|e| e.0).collect::<Vec<_>>(),
        [16, 32, 48]
    );
    for (side, w, h, rgba) in &entries {
        assert_eq!((w, h), (side, side), "the entry is square");
        assert_eq!(
            alpha(rgba, *side, side / 2, 0),
            0,
            "a clear top row at {side}"
        );
        assert_eq!(
            alpha(rgba, *side, side / 2, side / 2),
            255,
            "ink in the middle"
        );
        // 47.3 wide in a 47.3 square fills the row from edge to edge.
        assert!(
            alpha(rgba, *side, 0, side / 2) > 200,
            "edge to edge at {side}"
        );
    }
}

/// With *Use frames named for their size* on, the 16 comes from `logo-16` (its dot
/// in the top-left corner, the rest clear) and the 32 is still the rect.
/// flip: `icon::entries` ignoring `size_frames` → fails at the first entry's frame.
#[test]
fn a_size_with_a_frame_of_its_own_is_drawn_from_it() {
    let (doc, res, logo, frame) = fixture();
    let spec = ExportSpec {
        sizes: vec![16, 32],
        size_frames: true,
        ..ico()
    };
    assert_eq!(icon::size_frames(&doc, logo, &[16, 32]), [(16, frame)]);
    let entries = icon::entries(&doc, &res, logo, &spec).expect("entries");
    assert_eq!(entries[0].frame, Some(frame));
    let r = &entries[0].raster;
    assert_eq!(alpha(&r.rgba, 16, 1, 1), 255, "the 16 is the frame");
    assert_eq!(alpha(&r.rgba, 16, 8, 8), 0, "the 16 is the frame");
    assert_eq!(entries[1].frame, None);
    assert_eq!(alpha(&entries[1].raster.rgba, 32, 16, 16), 255);
}

/// Pixel-fitted, every pixel of the 16 is either clear or solid; drawn as it is,
/// the rect's fractional edges leave partial pixels. The document is unchanged.
/// flip: `icon::entries` skipping `fitted` → fails at "no partial pixel".
#[test]
fn a_pixel_fitted_size_has_no_partial_pixels() {
    let (doc, res, logo, _) = fixture();
    let plain = ExportSpec {
        sizes: vec![16],
        ..ico()
    };
    let partial = |spec: &ExportSpec| {
        let e = icon::entries(&doc, &res, logo, spec).expect("entries");
        e[0].raster
            .rgba
            .chunks(4)
            .filter(|p| p[3] != 0 && p[3] != 255)
            .count()
    };
    assert!(
        partial(&plain) > 0,
        "the fixture has fractional edges at 16"
    );
    let before = doc.clone();
    let fit = ExportSpec {
        pixel_fit: true,
        ..plain
    };
    assert_eq!(partial(&fit), 0, "no partial pixel");
    assert_eq!(doc, before);
}

/// A locked layer is pixel-fitted all the same: the fit is the export's copy,
/// not an edit the lock keeps off it.
/// flip: `icon::fitted` calling `pixel_fit::fit_on` → fails at "fitted though locked".
#[test]
fn a_locked_layer_is_pixel_fitted_too() {
    let (mut doc, _, logo, _) = fixture();
    doc.apply(&ondin_core::Transaction(vec![
        ondin_core::Operation::SetLocked {
            id: logo,
            locked: true,
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let spec = ExportSpec {
        sizes: vec![16],
        pixel_fit: true,
        ..ico()
    };
    let e = icon::entries(&doc, &res, logo, &spec).expect("entries");
    let partial = e[0]
        .raster
        .rgba
        .chunks(4)
        .filter(|p| p[3] != 0 && p[3] != 255)
        .count();
    assert_eq!(partial, 0, "fitted though locked");
}

/// A background fills the padding too, and trimming a box that is all ink changes
/// next to nothing.
#[test]
fn a_background_fills_the_padding() {
    let (doc, res, logo, _) = fixture();
    let spec = ExportSpec {
        sizes: vec![32],
        background: ExportBackground::Solid(Color::WHITE),
        ..ico()
    };
    let e = icon::entries(&doc, &res, logo, &spec).expect("entries");
    let r = &e[0].raster;
    assert!(r.rgba.chunks(4).all(|p| p[3] == 255), "opaque throughout");
    assert_eq!(&r.rgba[..3], &[255, 255, 255], "white in the padding");
    let trimmed = ExportSpec {
        trim: true,
        ..spec.clone()
    };
    let t = icon::entries(&doc, &res, logo, &trimmed).expect("entries");
    let diff = t[0]
        .raster
        .rgba
        .iter()
        .zip(&r.rgba)
        .filter(|(a, b)| a.abs_diff(**b) > 8)
        .count();
    assert!(
        diff < 16,
        "trimming a rect to its own ink changes next to nothing: {diff}"
    );
}

/// Trimmed, a group whose box is mostly a clear rect is drawn from its ink alone:
/// the 10 × 10 dot in a 40 × 40 box fills the 32 instead of its top-left quarter.
/// flip: `icon::entries` skipping `trimmed_view` → fails at "the trimmed dot
/// fills the square".
#[test]
fn trim_draws_the_ink_alone() {
    let mut ids = IdSource::new(0x2C0);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let (mark, clear, dot) = (ids.mint(), ids.mint(), ids.mint());
    let mut ops = vec![Operation::CreateNode {
        id: mark,
        parent: root,
        index: 0,
        kind: NodeKind::Group,
        transform: None,
        name: Some("mark".into()),
    }];
    let mut clear_rect = rect(clear, mark, 0, (0.0, 0.0), (40.0, 40.0));
    clear_rect[1] = Operation::SetFills {
        id: clear,
        fills: keyed_by_position([Fill {
            brush: Brush::Solid(Color::from_rgba8(0, 0, 0, 0)).into(),
            visible: true,
        }]),
    };
    ops.extend(clear_rect);
    ops.extend(rect(dot, mark, 1, (0.0, 0.0), (10.0, 10.0)));
    doc.apply(&Transaction(ops)).expect("the fixture");
    let res = Resolved::rebuild(&doc);
    let plain = ExportSpec {
        sizes: vec![32],
        ..ico()
    };
    let e = icon::entries(&doc, &res, mark, &plain).expect("entries");
    assert_eq!(alpha(&e[0].raster.rgba, 32, 4, 4), 255, "the dot's corner");
    assert_eq!(
        alpha(&e[0].raster.rgba, 32, 24, 24),
        0,
        "untrimmed, the clear rect holds the rest"
    );
    let trimmed = ExportSpec {
        trim: true,
        ..plain
    };
    let t = icon::entries(&doc, &res, mark, &trimmed).expect("entries");
    assert_eq!(
        alpha(&t[0].raster.rgba, 32, 24, 24),
        255,
        "the trimmed dot fills the square"
    );
}

/// A slice is drawn through its box, square and padded, and reads neither icon
/// switch: with frames and pixel-fit on, its entries are the plain ones, though a
/// frame named for it exists.
/// flip: `icon::entries` leaving the slice path when frames are on → fails at
/// "neither switch is read".
#[test]
fn a_slice_icon_is_its_box_as_the_page_shows_it() {
    let (mut doc, _, logo, frame) = fixture();
    let slice = IdSource::new(0x3C0).mint();
    let root = doc.get(logo).and_then(|n| n.parent()).expect("the root");
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id: slice,
        parent: root,
        index: 2,
        kind: NodeKind::Slice {
            size: Size::new(47.3, 21.0),
        },
        transform: Some(Affine::translate((10.3, 5.2))),
        name: Some("logo".into()),
    }]))
    .expect("the slice");
    let res = Resolved::rebuild(&doc);
    let plain = ExportSpec {
        sizes: vec![16, 32],
        ..ico()
    };
    assert_eq!(
        icon::size_frames(&doc, slice, &[16, 32]),
        [(16, frame)],
        "a frame is named for the slice"
    );
    let e = icon::entries(&doc, &res, slice, &plain).expect("entries");
    for entry in &e {
        let (r, side) = (&entry.raster, entry.size);
        assert_eq!((r.width, r.height), (side, side), "the entry is square");
        assert_eq!(alpha(&r.rgba, side, side / 2, 0), 0, "padded above");
        assert_eq!(
            alpha(&r.rgba, side, side / 2, side / 2),
            255,
            "the page's ink"
        );
        assert_eq!(entry.frame, None);
    }
    let both = ExportSpec {
        size_frames: true,
        pixel_fit: true,
        ..plain
    };
    let b = icon::entries(&doc, &res, slice, &both).expect("entries");
    for (x, y) in e.iter().zip(&b) {
        assert!(
            x.raster.rgba == y.raster.rgba,
            "neither switch is read at {}",
            x.size
        );
        assert_eq!(y.frame, None);
    }
}

/// An ICNS writes the macOS ladder under Apple's codes, its length declared.
#[test]
fn an_icns_holds_the_macos_ladder() {
    let (doc, res, logo, _) = fixture();
    let spec = ExportSpec::new(ExportFormat::Icns, ExportScale::Times(1.0));
    let file = &ondin_export::plan(
        &doc,
        &res,
        &[logo],
        ondin_export::PlanOptions {
            unset: Some(spec),
            ..Default::default()
        },
    )[0];
    assert_eq!(file.path, "logo.icns");
    let bytes = ondin_export::plan::render(&doc, &res, file).expect("an icon file");
    assert_eq!(&bytes[..4], b"icns");
    assert_eq!(
        u32::from_be_bytes(bytes[4..8].try_into().unwrap()) as usize,
        bytes.len()
    );
    let mut found = Vec::new();
    let mut at = 8;
    while at < bytes.len() {
        let code = String::from_utf8_lossy(&bytes[at..at + 4]).into_owned();
        let len = u32::from_be_bytes(bytes[at + 4..at + 8].try_into().unwrap()) as usize;
        let img = image::load_from_memory(&bytes[at + 8..at + len]).expect("a PNG");
        found.push((code, img.width()));
        at += len;
    }
    let want: Vec<(String, u32)> = [
        ("icp4", 16),
        ("icp5", 32),
        ("ic11", 32),
        ("icp6", 64),
        ("ic12", 64),
        ("ic07", 128),
        ("ic08", 256),
        ("ic13", 256),
        ("ic09", 512),
        ("ic14", 512),
        ("ic10", 1024),
    ]
    .into_iter()
    .map(|(c, s)| (c.to_string(), s))
    .collect();
    assert_eq!(found, want);
}
