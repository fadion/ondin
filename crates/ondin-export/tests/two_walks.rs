//! The SVG writer and the scene walk agree about what gets drawn (§15 D781).
//!
//! `svg::emit_node` re-decides things `scene::build` also decides (visibility,
//! opacity, clip, mask, effects, fill rule, paint order, stroke-alignment clip,
//! image placeholder and framing) without going through `ScenePainter`, so the
//! agreement is held by convention.
//!
//! What this compares is the ink set: every node in the fixture gets a unique solid
//! colour, and the colours each walk emits are compared as a set of node
//! identities. The other decisions legitimately differ (SVG has no viewport cull,
//! geometry is a `BezPath` versus a `d` attribute, element order is not call
//! order), and an unscoped comparison would be turned off the first time it cried
//! wolf.
//!
//! flip: adding `|| node.opacity() == 0.0` to `emit_node`'s visibility guard →
//! fails all three tests, the first at the set comparison (*only in the scene*).

mod common;

use ondin_core::kurbo::{Affine, BezPath, Rect, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, Fill, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction,
    keyed_by_position,
};
use ondin_render::scene::{ClipRule, ScenePainter, StrokePaint, TextRun};
use ondin_render::{RenderOverrides, Viewport};
use std::collections::BTreeSet;

/// Records the identity of every fill the walk asks for, by its colour. Everything
/// but `fill_path` is a no-op, so nothing implies layers, strokes or text are checked.
#[derive(Default)]
struct InkSet {
    seen: BTreeSet<u32>,
}

/// A colour as a comparable key. `Color` is four `f32`s and its components round
/// -trip through `u8` exactly for the values this fixture uses.
fn key(c: Color) -> u32 {
    let [r, g, b, _] = c.to_rgba8().to_u8_array();
    u32::from_be_bytes([0, r, g, b])
}

impl ScenePainter for InkSet {
    fn has_image(&self, _id: &ondin_core::ImageId) -> bool {
        true
    }

    fn fill_path(
        &mut self,
        _transform: Affine,
        _path: &BezPath,
        brush: &Brush,
        _framing: Option<ondin_core::Framing>,
        _rule: ClipRule,
    ) {
        if let Brush::Solid(c) = brush {
            self.seen.insert(key(*c));
        }
    }

    fn stroke_path(&mut self, _transform: Affine, _path: &BezPath, _stroke: &StrokePaint<'_>) {}
    fn draw_text(&mut self, _transform: Affine, _run: &TextRun<'_>) {}
    fn push_layer(
        &mut self,
        _transform: Affine,
        _clip: Option<&BezPath>,
        _rule: ClipRule,
        _opacity: f32,
    ) {
    }
    fn push_mask_layer(&mut self) {}
    fn pop_layer(&mut self) {}
}

/// The colours the SVG writer put in the file, read back out of the markup.
///
/// Reads `fill="#rrggbb"` only: every node here is a solid fill, and the count
/// assertion below would catch anything else.
fn svg_ink(markup: &str) -> BTreeSet<u32> {
    let mut out = BTreeSet::new();
    for (at, _) in markup.match_indices("fill=\"#") {
        let hex = &markup[at + 7..at + 13];
        if let Ok(v) = u32::from_str_radix(hex, 16) {
            out.insert(v);
        }
    }
    out
}

/// One colour per node, dense from `0x010000` so no two collide and none is black
/// (which is a default several things fall back to).
fn colour(n: u32) -> Color {
    let [_, r, g, b] = (0x01_00_00u32 + n * 0x01_01_01).to_be_bytes();
    Color::from_rgba8(r, g, b, 255)
}

/// A tree wide enough to make the two walks disagree if either forgets a rule:
/// a frame with a background, a plain rect, a group, a rect inside the group, a
/// **hidden** rect, and a rect at zero opacity.
struct Tree {
    doc: Document,
    res: Resolved,
    root: NodeId,
    frame: NodeId,
    /// Every node given a colour, in mint order, with the colour it wears.
    painted: Vec<(NodeId, u32)>,
}

fn tree() -> Tree {
    let mut ids = IdSource::new(0x7A11);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let frame = ids.mint();
    let plain = ids.mint();
    let group = ids.mint();
    let inside = ids.mint();
    let hidden = ids.mint();
    let clear = ids.mint();

    let mut ops = vec![Operation::CreateNode {
        id: frame,
        parent: root,
        index: 0,
        kind: NodeKind::Artboard {
            size: Size::new(400.0, 300.0),
        },
        transform: None,
        name: None,
    }];
    for (i, (id, parent)) in [
        (plain, frame),
        (group, frame),
        (inside, group),
        (hidden, frame),
        (clear, frame),
    ]
    .into_iter()
    .enumerate()
    {
        ops.push(Operation::CreateNode {
            id,
            parent,
            // Index 0 every time: `index` is within the parent, and the loop counter
            // would be refused (`IndexOutOfRange`) for the group's first child.
            index: 0,
            kind: match id == group {
                true => NodeKind::Group,
                false => NodeKind::Rect {
                    size: Size::new(30.0, 20.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
            },
            transform: Some(Affine::translate((10.0 * i as f64, 10.0 * i as f64))),
            name: None,
        });
    }

    let mut painted = Vec::new();
    // The frame's background is a fill like any other (§15 D400).
    for (n, id) in [frame, plain, inside, hidden, clear]
        .into_iter()
        .enumerate()
    {
        let c = colour(n as u32);
        painted.push((id, key(c)));
        ops.push(Operation::SetFills {
            id,
            fills: keyed_by_position([Fill {
                brush: Brush::Solid(c).into(),
                visible: true,
            }]),
        });
    }
    ops.push(Operation::SetVisible {
        id: hidden,
        visible: false,
    });
    ops.push(Operation::SetOpacity {
        id: clear,
        opacity: 0.0,
    });

    doc.apply(&Transaction(ops)).expect("build the tree");
    let res = Resolved::rebuild(&doc);
    Tree {
        doc,
        res,
        root,
        frame,
        painted,
    }
}

/// A viewport far larger than the tree, so the scene walk's cull cannot differ
/// from the SVG writer.
fn everything() -> Viewport {
    Viewport {
        view: Rect::new(-10_000.0, -10_000.0, 10_000.0, 10_000.0),
        pixel_size: (2000, 2000),
    }
}

fn scene_ink(t: &Tree, origins: &[NodeId]) -> BTreeSet<u32> {
    let mut ink = InkSet::default();
    ondin_render::scene::build_of(
        &t.doc,
        &t.res,
        &everything(),
        &RenderOverrides::default(),
        origins,
        &mut ink,
    );
    ink.seen
}

/// The two walks put paint down for the same nodes.
#[test]
fn the_two_walks_draw_the_same_nodes() {
    let t = tree();
    let scene = scene_ink(&t, &[t.root]);
    let svg = svg_ink(&ondin_export::svg::svg_of(&t.doc, &t.res, &[t.root]));

    // Fixture first: two empty sets are equal.
    assert!(
        scene.len() >= 3,
        "the scene walk drew almost nothing — the fixture is wrong, not the writers: {scene:?}"
    );
    assert_eq!(
        scene,
        svg,
        "the scene walk and the SVG writer disagree about which nodes get ink.\n\
         only in the scene: {:?}\nonly in the SVG: {:?}\n\
         (colours are per-node; `Tree::painted` says which is which)",
        scene.difference(&svg).collect::<Vec<_>>(),
        svg.difference(&scene).collect::<Vec<_>>(),
    );
}

/// The same for a subtree export, where `build_of` and `svg_of` once differed by a
/// transform (§15 D258).
#[test]
fn the_two_walks_agree_on_a_subtree_too() {
    let t = tree();
    let scene = scene_ink(&t, &[t.frame]);
    let svg = svg_ink(&ondin_export::svg::svg_of(&t.doc, &t.res, &[t.frame]));
    assert!(!scene.is_empty(), "fixture: {scene:?}");
    assert_eq!(scene, svg);
}

/// A hidden node is in neither walk, and a zero-opacity node is in both: it is
/// drawn and composited away, so "fixing" one walk to skip it would diverge in a
/// way no picture shows.
#[test]
fn the_agreement_is_about_decisions_and_not_about_appearance() {
    let t = tree();
    let scene = scene_ink(&t, &[t.root]);
    let svg = svg_ink(&ondin_export::svg::svg_of(&t.doc, &t.res, &[t.root]));

    let hidden = t.painted[3].1;
    let clear = t.painted[4].1;
    assert!(
        !scene.contains(&hidden) && !svg.contains(&hidden),
        "a hidden node is in neither walk"
    );
    assert!(
        scene.contains(&clear) && svg.contains(&clear),
        "a zero-opacity node is in both: scene {} / svg {}",
        scene.contains(&clear),
        svg.contains(&clear)
    );
}
