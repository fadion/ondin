//! A sweep gradient's start angle, through the whole CPU path: pixels, not brushes.

use ondin_core::kurbo::{Affine, Point, Rect, Size};
use ondin_core::peniko::{Color, Gradient};
use ondin_core::{
    Brush, Document, Fill, IdSource, NodeKind, Operation, Resolved, Transaction, keyed_by_position,
};
use ondin_render::{ImageStore, VelloCpuRenderer, Viewport};

/// A 100×100 square filled with a red-to-blue sweep about its centre, starting
/// `start` radians round from three o'clock, rendered at 1:1.
fn render(start: f32) -> (Vec<u8>, usize) {
    let mut ids = IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let r = ids.mint();
    let sweep = Gradient::new_sweep(Point::new(50.0, 50.0), start, start + std::f32::consts::TAU)
        .with_stops([
            (0.0_f32, Color::from_rgba8(255, 0, 0, 255)),
            (1.0_f32, Color::from_rgba8(0, 0, 255, 255)),
        ]);
    doc.apply(&Transaction(vec![
        Operation::CreateNode {
            id: r,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: Size::new(100.0, 100.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::IDENTITY),
            name: None,
        },
        Operation::SetFills {
            id: r,
            fills: keyed_by_position([Fill {
                brush: Brush::Gradient(sweep.into()).into(),
                visible: true,
            }]),
        },
    ]))
    .unwrap();
    let res = Resolved::rebuild(&doc);
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, 100.0, 100.0),
        pixel_size: (100, 100),
    };
    let (px, w, _) = VelloCpuRenderer::new().render_to_rgba(
        &doc,
        &res,
        &vp,
        &ImageStore::default(),
        doc.first_page(),
    );
    (px, w as usize)
}

fn at(px: &[u8], w: usize, x: usize, y: usize) -> (u8, u8, u8) {
    let p = (y * w + x) * 4;
    (px[p], px[p + 1], px[p + 2])
}

/// A sweep starting straight down goes round from there: just past the start is
/// the first colour, just before it the last, not the first padded over the turn.
///
/// flip: `brush_to_backend` passing a sweep's angles through unturned → fails at
/// the "just before" assertion (red where blue belongs).
#[test]
fn a_sweep_goes_round_from_its_start() {
    let (px, w) = render(std::f32::consts::FRAC_PI_2);
    let (r, _, b) = at(&px, w, 45, 90);
    assert!(r > 200 && b < 60, "just past the start is red: {r} {b}");
    let (r, _, b) = at(&px, w, 55, 90);
    assert!(b > 200 && r < 60, "just before the start is blue: {r} {b}");
    // Half a turn on, straight up, is half-way.
    let (r, _, b) = at(&px, w, 50, 10);
    assert!(
        (100..160).contains(&r) && (100..160).contains(&b),
        "{r} {b}"
    );
}

/// A negative start, which `atan2` hands the picker for the upper half, goes round
/// the same way.
#[test]
fn a_negative_start_goes_round_too() {
    let (px, w) = render(-std::f32::consts::FRAC_PI_2);
    let (r, _, b) = at(&px, w, 55, 10);
    assert!(
        r > 200 && b < 60,
        "just past an upward start is red: {r} {b}"
    );
    let (r, _, b) = at(&px, w, 45, 10);
    assert!(b > 200 && r < 60, "just before it is blue: {r} {b}");
}
