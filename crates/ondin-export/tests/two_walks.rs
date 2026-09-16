//! The SVG writer and the scene walk agree about **what gets drawn** (§15 D781,
//! `[A1-L7-07]`).
//!
//! `svg::emit_node` re-decides eleven things `scene::build` also decides —
//! visibility, opacity, clip, mask and mask mode, effects, fill rule, paint order,
//! the stroke-alignment clip, the missing-image placeholder and image framing —
//! without going through `scene::build` or `ScenePainter`. `scene::build_of`'s own
//! doc calls itself *"the mirror of `ondin_export::svg::svg_of` … which is why this
//! one is written to look like it"*, and invariant 11 says document→**scene**
//! walk, which SVG is not. So the agreement is held by convention, and it has
//! **diverged twice on record**: §15 D179's frame background reaching the
//! placeholder rule a step late, so *"a frame whose picture had gone drew the cross
//! on canvas and exported a hole"*, and a `fill="none"` written on the same path.
//! Both were found by a human reading two files against each other.
//!
//! # What this compares, and what it does not
//!
//! **The ink set**: for each node, did this walk decide to put paint down for it at
//! all. Read on both sides by giving every node in the fixture a **unique solid
//! colour** and collecting the colours each walk emits — so the comparison is a set
//! of node identities, arrived at without either walk being asked to report on
//! itself.
//!
//! That is deliberately one of the eleven and not all eleven, and the reason is the
//! finding's own: the two walks *legitimately* differ elsewhere. SVG has no
//! viewport cull. Geometry is a `BezPath` on one side and a `d` attribute on the
//! other. Element ordering is not call ordering. A comparison that reached for
//! those would need a scope argument per decision, and an unscoped one would be
//! turned off the first time it cried wolf.
//!
//! **Both recorded divergences were ink-set bugs**, which is why this is the one
//! worth having: each was a node one walk drew and the other did not.
//!
//! # The answer, which the finding could not give
//!
//! `[A1-L7-07]` is explicit that it is *"not a claim that they disagree right
//! now"*, and says establishing it by reading was not possible. **They agree**, on
//! this tree, today. That is the second thing this file is for: a negative result
//! somebody can re-run, rather than a structural worry nobody can close.
//!
//! **Flipped.** Adding `|| node.opacity() == 0.0` to `emit_node`'s visibility
//! guard — the plausible "optimisation", and one that changes no picture at all —
//! fails all three tests at their predicted sites, the first naming the one colour
//! that went missing: *only in the scene: [328708]*.
//!
//! (Plain backticks throughout per §15 D319.)

mod common;

use ondin_core::kurbo::{Affine, BezPath, Rect, Size};
use ondin_core::peniko::Color;
use ondin_core::{
    Brush, Document, Fill, IdSource, NodeId, NodeKind, Operation, Resolved, Transaction,
};
use ondin_render::scene::{ClipRule, ScenePainter, StrokePaint, TextRun};
use ondin_render::{RenderOverrides, Viewport};
use std::collections::BTreeSet;

/// Records the identity of every fill the walk asks for, by its colour.
///
/// **A `ScenePainter` that draws nothing.** Everything below `fill_path` is a
/// no-op: the layer stack, strokes and text are not what this test is about, and
/// implementing them would invite the reader to think they were checked.
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
/// **Out of `fill="#rrggbb"` only.** A colour that reached the file some other way
/// — a gradient stop, a `stop-color` — is not this fixture's doing: every node here
/// is a solid fill, so anything else appearing would be a defect in the *test*, and
/// the count assertion below is what would catch it.
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
            // **Index 0 every time, not the loop counter.** `index` is within the
            // *parent*, and `inside` is the first child of a group that is empty at
            // that point — the loop counter says 2 and the op is refused
            // (`IndexOutOfRange`). Order does not matter to an ink *set*.
            index: 0,
            kind: match id == group {
                true => NodeKind::Group,
                false => NodeKind::Rect {
                    size: Size::new(30.0, 20.0),
                    corner_radii: Default::default(),
                },
            },
            transform: Some(Affine::translate((10.0 * i as f64, 10.0 * i as f64))),
            name: None,
        });
    }

    let mut painted = Vec::new();
    // The frame's own background is a fill like any other since §15 D400, and it
    // is one of the two nodes a recorded divergence was about.
    for (n, id) in [frame, plain, inside, hidden, clear]
        .into_iter()
        .enumerate()
    {
        let c = colour(n as u32);
        painted.push((id, key(c)));
        ops.push(Operation::SetFills {
            id,
            fills: vec![Fill {
                brush: Brush::Solid(c),
                visible: true,
            }],
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

/// A viewport far larger than the tree, so the scene walk's cull — which the SVG
/// writer has no equivalent of — cannot be what the two disagree about.
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

/// The whole page: the two walks put paint down for the same nodes.
#[test]
fn the_two_walks_draw_the_same_nodes() {
    let t = tree();
    let scene = scene_ink(&t, &[t.root]);
    let svg = svg_ink(&ondin_export::svg::svg_of(&t.doc, &t.res, &[t.root]));

    // **The fixture is asserted before the comparison.** Two empty sets are equal,
    // and a fixture that painted nothing would make this test green for ever.
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

/// And for a **subtree** export, which is the form with two origins' worth of rules
/// in it — `build_of` and `svg_of` are each other's mirror and this is where they
/// were once a transform apart (§15 D258).
#[test]
fn the_two_walks_agree_on_a_subtree_too() {
    let t = tree();
    let scene = scene_ink(&t, &[t.frame]);
    let svg = svg_ink(&ondin_export::svg::svg_of(&t.doc, &t.res, &[t.frame]));
    assert!(!scene.is_empty(), "fixture: {scene:?}");
    assert_eq!(scene, svg);
}

/// A hidden node is in neither, and a zero-opacity node is in **both**.
///
/// 🚨 **The second half is the one worth asserting**, and it is not obvious: zero
/// opacity is not invisibility. The node is drawn and composited away, so both
/// walks emit it — and a reader "fixing" either walk to skip it would make the two
/// disagree in a way no picture would show, because the ink is invisible either
/// way. It is exactly the shape of the two recorded divergences: a difference of
/// *decision* with no difference of appearance, until something downstream reads
/// the file.
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
