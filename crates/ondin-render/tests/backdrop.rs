//! Background blur through the whole CPU path: the walk repaints what lies
//! behind a layer into a blurred buffer and draws it inside the layer's outline
//! (`scene::paint_backdrop`, §5.3a).
//!
//! The oracle is the layer blur: what a background blur shows inside the glass
//! is what the same artwork draws there when it carries a layer blur itself.

use ondin_core::kurbo::{Affine, Rect, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, Effect, EffectKind, Fill, IdSource, NodeId, NodeKind, Operation, Resolved,
    Transaction, keyed_by_position,
};
use ondin_render::{ImageStore, VelloCpuRenderer, Viewport};

const SIDE: usize = 100;

struct Page {
    doc: Document,
    ids: IdSource,
    root: NodeId,
}

impl Page {
    fn new() -> Self {
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        Self {
            doc: Document::new(root),
            ids,
            root,
        }
    }

    fn apply(&mut self, ops: Vec<Operation>) {
        self.doc.apply(&Transaction(ops)).unwrap();
    }

    fn add(&mut self, parent: NodeId, kind: NodeKind, at: (f64, f64)) -> NodeId {
        let id = self.ids.mint();
        let index = self.doc.get(parent).unwrap().children().len();
        self.apply(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(Affine::translate(at)),
            name: None,
        }]);
        id
    }

    /// A `w`×`h` rect at `(x, y)` in `parent`, filled with `fill` when given.
    fn rect(
        &mut self,
        parent: NodeId,
        (x, y, w, h): (f64, f64, f64, f64),
        fill: Option<Color>,
    ) -> NodeId {
        let id = self.add(
            parent,
            NodeKind::Rect {
                size: Size::new(w, h),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            (x, y),
        );
        if let Some(c) = fill {
            self.apply(vec![Operation::SetFills {
                id,
                fills: keyed_by_position([Fill {
                    brush: Brush::Solid(c).into(),
                    visible: true,
                }]),
            }]);
        }
        id
    }

    fn group(&mut self, parent: NodeId) -> NodeId {
        self.add(parent, NodeKind::Group, (0.0, 0.0))
    }

    /// Five-unit black and white columns over the whole page, in a group of
    /// their own under `parent`.
    fn stripes(&mut self, parent: NodeId) -> NodeId {
        let g = self.group(parent);
        for i in 0..SIDE / 5 {
            let ink = if i % 2 == 0 {
                Color::BLACK
            } else {
                Color::WHITE
            };
            self.rect(g, (i as f64 * 5.0, 0.0, 5.0, SIDE as f64), Some(ink));
        }
        g
    }

    fn effects(&mut self, id: NodeId, effects: Vec<EffectKind>) {
        self.apply(vec![Operation::SetEffects {
            id,
            effects: keyed_by_position(effects.into_iter().map(Effect::new)),
        }]);
    }

    fn render(&self) -> Vec<u8> {
        self.render_of(&[self.doc.first_page()])
    }

    fn render_of(&self, origins: &[NodeId]) -> Vec<u8> {
        let res = Resolved::rebuild(&self.doc);
        let vp = Viewport {
            view: Rect::new(0.0, 0.0, SIDE as f64, SIDE as f64),
            pixel_size: (SIDE as u32, SIDE as u32),
        };
        VelloCpuRenderer::new()
            .render_subtrees_to_rgba(&self.doc, &res, &vp, &ImageStore::default(), origins)
            .0
    }
}

fn at(px: &[u8], x: usize, y: usize) -> [u8; 4] {
    let p = (y * SIDE + x) * 4;
    [px[p], px[p + 1], px[p + 2], px[p + 3]]
}

/// The glass's interior, clear of its antialiased edge: the 40×40 rect at
/// (30, 30) the tests put it in.
fn inside() -> impl Iterator<Item = (usize, usize)> {
    (32..68).flat_map(|y| (32..68).map(move |x| (x, y)))
}

const GLASS: (f64, f64, f64, f64) = (30.0, 30.0, 40.0, 40.0);
const BLUR: EffectKind = EffectKind::BackgroundBlur { radius: 8.0 };

/// Inside an unfilled glass the stripes are what a layer blur of the same radius
/// draws, byte for byte; outside it they stay sharp.
///
/// flip: `paint_node_pass` never calling `paint_backdrop` → fails at the first
/// interior pixel (the stripes come through sharp).
#[test]
fn a_glass_shows_what_lies_behind_it_blurred_as_a_layer_blur_would() {
    let mut glass = Page::new();
    let root = glass.root;
    glass.stripes(root);
    let n = glass.rect(root, GLASS, None);
    glass.effects(n, vec![BLUR]);
    let got = glass.render();

    let mut oracle = Page::new();
    let root = oracle.root;
    let stripes = oracle.stripes(root);
    oracle.effects(stripes, vec![EffectKind::LayerBlur { radius: 8.0 }]);
    let want = oracle.render();

    let mut sharp = Page::new();
    let root = sharp.root;
    sharp.stripes(root);
    let sharp = sharp.render();

    assert_ne!(
        at(&want, 50, 50),
        at(&sharp, 50, 50),
        "the fixture's blur changes the middle of the glass"
    );
    for (x, y) in inside() {
        assert_eq!(
            at(&got, x, y),
            at(&want, x, y),
            "inside the glass at ({x}, {y})"
        );
    }
    for (x, y) in [(10, 50), (90, 50), (50, 10), (50, 90), (27, 50)] {
        assert_eq!(
            at(&got, x, y),
            at(&sharp, x, y),
            "outside the glass at ({x}, {y})"
        );
    }
}

/// The glass's own fill is drawn over the blurred backdrop and is no part of it,
/// and a layer drawn after the glass is not blurred by it.
///
/// flip: the repaint never stopping at `Backdrop::upto` → fails inside the
/// glass: the repaint runs on past it and blurs its own tint into the backdrop.
#[test]
fn the_backdrop_holds_what_comes_before_the_layer_and_nothing_after() {
    let tint = Color::from_rgba8(0, 0, 255, 64);
    let mut glass = Page::new();
    let root = glass.root;
    glass.stripes(root);
    let n = glass.rect(root, GLASS, Some(tint));
    glass.effects(n, vec![BLUR]);
    let red = Color::from_rgba8(255, 0, 0, 255);
    glass.rect(root, (60.0, 60.0, 30.0, 30.0), Some(red));
    let got = glass.render();

    let mut oracle = Page::new();
    let root = oracle.root;
    let stripes = oracle.stripes(root);
    oracle.effects(stripes, vec![EffectKind::LayerBlur { radius: 8.0 }]);
    oracle.rect(root, GLASS, Some(tint));
    oracle.rect(root, (60.0, 60.0, 30.0, 30.0), Some(red));
    let want = oracle.render();

    for (x, y) in inside() {
        assert_eq!(at(&got, x, y), at(&want, x, y), "at ({x}, {y})");
    }
    assert_eq!(
        at(&got, 50, 56),
        at(&want, 50, 56),
        "a later layer is not in the backdrop: no red blurred in just above it"
    );
    assert_eq!(
        at(&got, 65, 65),
        [255, 0, 0, 255],
        "and it is drawn sharp over the glass"
    );
}

/// A half-transparent glass mixes the blurred backdrop with the sharp one, half
/// and half.
#[test]
fn a_faded_glass_shows_its_blur_at_its_opacity() {
    let mut glass = Page::new();
    let root = glass.root;
    glass.stripes(root);
    let n = glass.rect(root, GLASS, None);
    glass.effects(n, vec![BLUR]);
    glass.apply(vec![Operation::SetOpacity {
        id: n,
        opacity: 0.5,
    }]);
    let got = glass.render();

    let mut oracle = Page::new();
    let root = oracle.root;
    let stripes = oracle.stripes(root);
    oracle.effects(stripes, vec![EffectKind::LayerBlur { radius: 8.0 }]);
    let blurred = oracle.render();
    let mut sharp = Page::new();
    let root = sharp.root;
    sharp.stripes(root);
    let sharp = sharp.render();

    for (x, y) in inside() {
        let (g, b, s) = (at(&got, x, y), at(&blurred, x, y), at(&sharp, x, y));
        for c in 0..3 {
            let want = (f32::from(b[c]) + f32::from(s[c])) / 2.0;
            assert!(
                (f32::from(g[c]) - want).abs() <= 1.5,
                "at ({x}, {y}) channel {c}: {} against half of {} and {}",
                g[c],
                b[c],
                s[c]
            );
        }
        assert_eq!(g[3], 255, "the page stays opaque under a faded glass");
    }
}

/// A glass inside a group drawn into a layer of its own (here, at 50% opacity)
/// reads only what that group drew before it: the stripes under the group are
/// not its backdrop, and show through sharp. In a group that draws straight
/// into the page they are, and come out blurred.
///
/// flip: `paint_backdrop` always repainting from the walk's origins (ignoring
/// `Open::isolates`) → fails at "an isolating group".
#[test]
fn the_backdrop_stops_at_the_nearest_container_that_isolates() {
    let render = |isolate: bool| {
        let mut p = Page::new();
        let root = p.root;
        p.stripes(root);
        let g = p.group(root);
        // Something in the group behind the glass, away from where it is read.
        p.rect(
            g,
            (0.0, 0.0, 10.0, 10.0),
            Some(Color::from_rgba8(255, 0, 0, 255)),
        );
        let n = p.rect(g, GLASS, None);
        p.effects(n, vec![BLUR]);
        if isolate {
            p.apply(vec![Operation::SetOpacity {
                id: g,
                opacity: 0.5,
            }]);
        }
        p.render()
    };
    let mut sharp = Page::new();
    let root = sharp.root;
    sharp.stripes(root);
    let sharp = sharp.render();

    let isolated = render(true);
    assert_eq!(
        at(&isolated, 50, 50),
        at(&sharp, 50, 50),
        "an isolating group: the stripes beneath it show through untouched"
    );
    let through = render(false);
    assert_ne!(
        at(&through, 50, 50),
        at(&sharp, 50, 50),
        "a group that is not a layer of its own: the stripes are the backdrop"
    );
}

/// Exported alone, a glass has nothing behind it: its interior is as clear as
/// its fill. Nothing outside the walk's origins is read.
#[test]
fn a_glass_exported_alone_reads_nothing_behind_it() {
    let mut p = Page::new();
    let root = p.root;
    p.stripes(root);
    let n = p.rect(root, GLASS, Some(Color::from_rgba8(0, 0, 255, 64)));
    p.effects(n, vec![BLUR]);
    let alone = p.render_of(&[n]);
    let px = at(&alone, 50, 50);
    assert_eq!(px[3], 64, "only the glass's own fill: {px:?}");
    assert_eq!(at(&alone, 10, 10)[3], 0, "and nothing else");
}

/// Damage within a blur's reach of a glass spreads into it as far as that
/// reach, and damage out of reach adds nothing. At 1:1 a radius of 8 reaches 12.
#[test]
fn damage_near_a_glass_spreads_into_it() {
    let mut p = Page::new();
    let root = p.root;
    p.stripes(root);
    let n = p.rect(root, GLASS, None);
    p.effects(n, vec![BLUR]);
    let res = Resolved::rebuild(&p.doc);
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, SIDE as f64, SIDE as f64),
        pixel_size: (SIDE as u32, SIDE as u32),
    };
    let spread = |d: Rect| {
        ondin_render::scene::spread_through_backdrops(
            &p.doc,
            &res,
            &vp,
            &Default::default(),
            p.doc.first_page(),
            vec![d],
        )
    };
    let near = spread(Rect::new(0.0, 40.0, 20.0, 50.0));
    assert_eq!(
        near,
        vec![
            Rect::new(0.0, 40.0, 20.0, 50.0),
            Rect::new(30.0, 30.0, 32.0, 62.0)
        ],
        "twelve past the change, cut to the glass"
    );
    let far = spread(Rect::new(0.0, 0.0, 10.0, 10.0));
    assert_eq!(far, vec![Rect::new(0.0, 0.0, 10.0, 10.0)], "out of reach");
}

/// The spread finds a glass inside a group and one a preview adds, and gives up
/// on a chained grid of glass once its boxes cover half the canvas, where the
/// canvas draws whole.
///
/// flip: `spread_through_backdrops` reading glass off `Resolved` alone → fails at
/// "a previewed glass"; without its half-canvas stop → fails at "the grid".
#[test]
fn the_spread_finds_nested_and_previewed_glass_and_stops_at_half_the_canvas() {
    let vp = Viewport {
        view: Rect::new(0.0, 0.0, SIDE as f64, SIDE as f64),
        pixel_size: (SIDE as u32, SIDE as u32),
    };
    let mut p = Page::new();
    let root = p.root;
    let g = p.group(root);
    let nested = p.rect(g, GLASS, None);
    p.effects(nested, vec![BLUR]);
    let res = Resolved::rebuild(&p.doc);
    let near = Rect::new(0.0, 40.0, 20.0, 50.0);
    let spread = |doc: &Document, res: &Resolved, ov: &ondin_render::RenderOverrides| {
        ondin_render::scene::spread_through_backdrops(
            doc,
            res,
            &vp,
            ov,
            doc.first_page(),
            vec![near],
        )
    };
    assert_eq!(
        spread(&p.doc, &res, &Default::default()).len(),
        2,
        "a nested glass"
    );

    let mut p = Page::new();
    let root = p.root;
    let plain = p.rect(root, GLASS, None);
    let res = Resolved::rebuild(&p.doc);
    let ov = ondin_render::RenderOverrides::from_transaction(
        &p.doc,
        &res,
        &Transaction(vec![Operation::SetEffects {
            id: plain,
            effects: keyed_by_position([Effect::new(BLUR)]),
        }]),
    )
    .expect("a preview");
    assert_eq!(spread(&p.doc, &res, &ov).len(), 2, "a previewed glass");

    // Twelve by twelve cards of glass, 16 apart, each in reach of the next.
    let mut p = Page::new();
    let root = p.root;
    for i in 0..12 {
        for j in 0..12 {
            let at = (f64::from(i) * 8.0, f64::from(j) * 8.0, 6.5, 6.5);
            let card = p.rect(root, at, None);
            p.effects(card, vec![EffectKind::BackgroundBlur { radius: 3.0 }]);
        }
    }
    let res = Resolved::rebuild(&p.doc);
    let out = ondin_render::scene::spread_through_backdrops(
        &p.doc,
        &res,
        &vp,
        &Default::default(),
        p.doc.first_page(),
        vec![Rect::new(0.0, 0.0, 2.0, 2.0)],
    );
    let covered: f64 = out.iter().map(|r| r.area()).sum();
    assert!(
        covered * 2.0 > (SIDE * SIDE) as f64,
        "the grid: the spread reaches half the canvas"
    );
    assert!(out.len() < 400, "the grid: {} boxes", out.len());
}

/// A hidden background blur draws nothing, as if the row were not there.
#[test]
fn a_hidden_background_blur_draws_nothing() {
    let mut p = Page::new();
    let root = p.root;
    p.stripes(root);
    let n = p.rect(root, GLASS, None);
    p.apply(vec![Operation::SetEffects {
        id: n,
        effects: keyed_by_position([Effect {
            visible: false,
            ..Effect::new(BLUR)
        }]),
    }]);
    let got = p.render();
    let mut sharp = Page::new();
    let root = sharp.root;
    sharp.stripes(root);
    assert_eq!(got, sharp.render());
}

/// A background blur on an alpha mask leaves the mask masking: its backdrop is
/// never poured into the mask layer.
///
/// flip: `paint_node_pass` drawing a mask's backdrop → fails at "masked at half".
#[test]
fn a_blur_on_a_mask_leaves_it_masking() {
    let render = |blurred: bool| {
        let mut p = Page::new();
        let root = p.root;
        p.rect(root, (0.0, 0.0, 100.0, 100.0), Some(Color::WHITE));
        let g = p.group(root);
        let mask = p.rect(
            g,
            (20.0, 20.0, 60.0, 60.0),
            Some(Color::from_rgba8(0, 0, 0, 128)),
        );
        p.apply(vec![
            Operation::SetMask {
                id: mask,
                mask: true,
            },
            Operation::SetMaskMode {
                id: mask,
                mode: ondin_core::MaskMode::Alpha,
            },
        ]);
        p.rect(
            g,
            (0.0, 0.0, 100.0, 100.0),
            Some(Color::from_rgba8(255, 0, 0, 255)),
        );
        if blurred {
            p.effects(mask, vec![BLUR]);
        }
        at(&p.render(), 50, 50)
    };
    let plain = render(false);
    assert!(
        plain[1].abs_diff(127) <= 2,
        "fixture: masked at half without a blur, {plain:?}"
    );
    assert_eq!(render(true), plain, "masked at half");
}
