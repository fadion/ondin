//! Colour review (`color_review.rs`): which colours a scope holds, how they group,
//! and what a merge writes.

use ondin_core::color_review::{self, Group, NewVar, Swatch, Threshold, delta};
use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
use ondin_core::peniko::color::DynamicColor;
use ondin_core::peniko::{Color, ColorStop, Gradient};
use ondin_core::variable::{Cell, Mode, Value, VarId, VarKind, Variable};
use ondin_core::{
    Brush, BrushSource, CharAttr, Document, Effect, EffectKind, Fill, GradientBrush, History,
    IdSource, NodeId, NodeKind, Operation, Shadow, Span, Spans, TextStyle, Transaction,
    keyed_by_position,
};

const A: Color = Color::from_rgb8(0x6D, 0x8C, 0xD9);
const B: Color = Color::from_rgb8(0x6E, 0x8D, 0xDA);
const FAR: Color = Color::from_rgb8(0xE0, 0xA0, 0x4A);

struct F {
    doc: Document,
    hist: History,
    ids: IdSource,
    root: NodeId,
    board: NodeId,
}

impl F {
    fn new() -> Self {
        let mut ids = IdSource::new(0xC0DE);
        let root = ids.mint();
        let board = ids.mint();
        let mut doc = Document::new(root);
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id: board,
            parent: root,
            index: 0,
            kind: NodeKind::Artboard {
                size: Size::new(400.0, 300.0),
                corner_radii: RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::IDENTITY),
            name: None,
        }]))
        .expect("a frame");
        Self {
            doc,
            hist: History::new(),
            ids,
            root,
            board,
        }
    }

    fn add(&mut self, kind: NodeKind) -> NodeId {
        let id = self.ids.mint();
        let index = self.doc.get(self.board).unwrap().children().len();
        self.doc
            .apply(&Transaction(vec![Operation::CreateNode {
                id,
                parent: self.board,
                index,
                kind,
                transform: Some(Affine::IDENTITY),
                name: None,
            }]))
            .expect("a layer");
        id
    }

    fn rect(&mut self, fills: Vec<BrushSource>) -> NodeId {
        let id = self.add(NodeKind::Rect {
            size: Size::new(10.0, 10.0),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        });
        self.apply(vec![Operation::SetFills {
            id,
            fills: keyed_by_position(fills.into_iter().map(|brush| Fill {
                brush,
                visible: true,
            })),
        }]);
        id
    }

    fn apply(&mut self, ops: Vec<Operation>) {
        self.doc.apply(&Transaction(ops)).expect("applies");
    }

    /// A colour variable valued `c` in one mode, the mode added first.
    fn var(&mut self, name: &str, c: Color) -> VarId {
        let mut ops = Vec::new();
        let mode = match self.doc.variables().default_mode() {
            Some(m) => m,
            None => {
                let m = self.ids.mint_mode();
                ops.push(Operation::AddMode {
                    collection: ondin_core::variable::CollectionId::FIRST,
                    mode: Mode {
                        id: m,
                        name: "Mode 1".into(),
                    },
                    index: 0,
                    cells: Vec::new(),
                });
                m
            }
        };
        let id = self.ids.mint_var();
        ops.push(Operation::AddVariable {
            var: Variable {
                id,
                name: name.into(),
                kind: VarKind::Color,
                cells: [(mode, Cell::Color(c))].into_iter().collect(),
                scope: None,
                origin: None,
                forked: None,
                description: String::new(),
            },
        });
        self.apply(ops);
        id
    }

    fn census(&self) -> Vec<Swatch> {
        color_review::census(&self.doc, &[self.root])
    }

    fn fill(&self, id: NodeId) -> BrushSource {
        self.doc.get(id).unwrap().paint().fills[0].brush.clone()
    }
}

fn solid(c: Color) -> BrushSource {
    Brush::Solid(c).into()
}

fn bytes(c: Color) -> [u8; 4] {
    c.to_rgba8().to_u8_array()
}

fn uses_of(census: &[Swatch], c: Color) -> usize {
    census
        .iter()
        .find(|s| s.is_raw() && bytes(s.color) == bytes(c))
        .map_or(0, |s| s.uses)
}

fn text(color: Color, run: Color) -> NodeKind {
    let style = TextStyle {
        color: Some(color),
        ..TextStyle::default()
    };
    let spans = Spans::from_parts(
        vec![Span {
            start: 0,
            end: 2,
            attr: CharAttr::Color(Some(run)),
        }],
        &style,
    );
    NodeKind::Text {
        content: "Hello".into(),
        style: Box::new(style),
        spans,
        para_spans: Default::default(),
        paragraph: Default::default(),
        block: Default::default(),
        sizing: ondin_core::TextSizing::Auto,
        on_path: None,
        on_path_flip: false,
        on_path_offset: 0.0,
    }
}

fn gradient(stops: [Color; 2]) -> BrushSource {
    let g = Gradient::new_linear((0.0, 0.0), (10.0, 0.0)).with_stops(
        stops
            .map(DynamicColor::from_alpha_color)
            .iter()
            .enumerate()
            .map(|(i, &color)| ColorStop {
                offset: i as f32,
                color,
            })
            .collect::<Vec<_>>()
            .as_slice(),
    );
    Brush::Gradient(GradientBrush {
        gradient: g,
        transform: Affine::IDENTITY,
        opacity: 1.0,
    })
    .into()
}

/// Every place a raw colour lives is counted: a solid fill, a gradient stop, a
/// shadow, a text layer's own colour and a run's.
///
/// flip: `each_place` skipping effects → fails at the shadow's count.
#[test]
fn the_census_counts_every_place_a_colour_lives() {
    let mut f = F::new();
    let r = f.rect(vec![solid(A), gradient([A, FAR])]);
    f.apply(vec![Operation::SetEffects {
        id: r,
        effects: keyed_by_position([Effect::new(EffectKind::DropShadow(Shadow {
            color: B,
            ..Shadow::default()
        }))]),
    }]);
    f.add(text(A, B));

    let census = f.census();
    assert_eq!(
        uses_of(&census, A),
        3,
        "fill, stop and the text's own colour"
    );
    assert_eq!(uses_of(&census, B), 2, "the shadow and the run");
    assert_eq!(uses_of(&census, FAR), 1, "the other stop");
    let a = census.iter().find(|s| bytes(s.color) == bytes(A)).unwrap();
    assert_eq!(a.nodes.len(), 2, "two layers carry it");
}

/// A bound fill is a member as its variable, drawn at its value.
#[test]
fn a_bound_fill_is_counted_as_its_variable() {
    let mut f = F::new();
    let v = f.var("color/accent", A);
    f.rect(vec![BrushSource::Bound { var: v, alpha: 1.0 }]);
    f.rect(vec![solid(B)]);
    let census = f.census();
    let bound = census.iter().find(|s| s.var == Some(v)).expect("listed");
    assert_eq!(bytes(bound.color), bytes(A));
    assert_eq!(uses_of(&census, A), 0, "not a raw colour as well");
}

/// Members are within reach of the anchor itself, so a chain does not pull in
/// a colour two steps away.
///
/// flip: measuring each candidate against the group's last member → fails at
/// "the far end is its own".
#[test]
fn a_group_does_not_chain() {
    let (a, b, c) = (
        Color::from_rgb8(0x64, 0x64, 0x64),
        Color::from_rgb8(0x66, 0x66, 0x66),
        Color::from_rgb8(0x68, 0x68, 0x68),
    );
    let limit = Threshold::Tight.limit();
    assert!(delta(a, b) <= limit && delta(b, c) <= limit && delta(a, c) > limit);
    let mut f = F::new();
    for _ in 0..3 {
        f.rect(vec![solid(a)]);
    }
    f.rect(vec![solid(b)]);
    f.rect(vec![solid(b)]);
    f.rect(vec![solid(c)]);
    let census = f.census();
    let groups = color_review::groups(&census, Threshold::Tight);
    assert_eq!(groups.len(), 1, "a and b; c alone is no group");
    let members: Vec<[u8; 4]> = groups[0]
        .members
        .iter()
        .map(|&m| bytes(census[m].color))
        .collect();
    assert_eq!(members, vec![bytes(a), bytes(b)], "the far end is its own");
    // Looser, the same anchor reaches it.
    let loose = color_review::groups(&census, Threshold::Close);
    assert_eq!(loose[0].members.len(), 3);
}

/// The same RGB at another alpha is never in the group.
#[test]
fn alpha_is_compared_apart() {
    let mut f = F::new();
    f.rect(vec![solid(A)]);
    f.rect(vec![solid(A.with_alpha(0.8))]);
    let census = f.census();
    assert!(color_review::groups(&census, Threshold::Loose).is_empty());
}

/// A group of variables alone has nothing to merge; with a raw colour it shows,
/// the bound colour its target.
#[test]
fn a_group_needs_a_raw_member_and_targets_the_bound_one() {
    let mut f = F::new();
    let v = f.var("color/accent", A);
    let w = f.var("color/accent-2", B);
    f.rect(vec![BrushSource::Bound { var: v, alpha: 1.0 }]);
    f.rect(vec![BrushSource::Bound { var: w, alpha: 1.0 }]);
    assert!(color_review::groups(&f.census(), Threshold::Close).is_empty());

    for _ in 0..3 {
        f.rect(vec![solid(Color::from_rgb8(0x6C, 0x8A, 0xD6))]);
    }
    let census = f.census();
    let groups = color_review::groups(&census, Threshold::Close);
    assert_eq!(groups.len(), 1);
    let g = &groups[0];
    assert!(census[g.members[0]].is_raw(), "the most used is the anchor");
    let t = g.default_target(&census);
    assert!(
        !census[g.members[t]].is_raw(),
        "but a bound colour is the target"
    );
    assert_eq!(g.merged(&census, t).len(), 1, "the other variable is kept");
}

fn group_of(census: &[Swatch]) -> Group {
    let mut gs = color_review::groups(census, Threshold::Close);
    assert_eq!(gs.len(), 1, "one group: {gs:?}");
    gs.remove(0)
}

/// A merge into a raw colour rewrites every place, leaves a locked layer and
/// says so, and is one undo step.
///
/// flip: dropping `rewrite`'s lock check → fails at the count of places written.
#[test]
fn a_merge_rewrites_every_place_and_skips_locked_layers() {
    let mut f = F::new();
    for _ in 0..5 {
        f.rect(vec![solid(A)]);
    }
    let b1 = f.rect(vec![gradient([B, FAR])]);
    // A run equal to the text's own colour is no run, so it takes a third member.
    let t = f.add(text(B, Color::from_rgb8(0x6C, 0x8A, 0xD6)));
    let locked = f.rect(vec![solid(B)]);
    f.apply(vec![Operation::SetLocked {
        id: locked,
        locked: true,
    }]);
    let census = f.census();
    let g = group_of(&census);
    assert_eq!(
        bytes(census[g.members[0]].color),
        bytes(A),
        "A is the anchor"
    );
    let merged = color_review::merge(&f.doc, &[f.root], &census, &g, 0);
    assert_eq!(merged.uses, 3, "the stop, the text's colour and its run");
    assert_eq!(merged.layers, 2);
    assert_eq!(merged.locked, 1);
    assert_eq!(merged.unbound, 0);
    f.hist.commit(&mut f.doc, merged.tx).expect("commits");

    assert_eq!(f.fill(locked), solid(B), "the locked layer kept its colour");
    let census = f.census();
    assert_eq!(uses_of(&census, B), 1, "only the locked layer's");
    let Some(NodeKind::Text { style, spans, .. }) = f.doc.get(t).map(|n| n.kind().clone()) else {
        panic!("a text");
    };
    assert_eq!(style.color.map(bytes), Some(bytes(A)));
    assert!(
        spans.is_empty(),
        "the run now equals the default, so it goes"
    );
    let BrushSource::Literal(Brush::Gradient(g)) = f.fill(b1) else {
        panic!("a gradient");
    };
    assert_eq!(bytes(g.gradient.stops[0].color.to_alpha_color()), bytes(A));

    f.hist.undo(&mut f.doc).unwrap();
    assert_eq!(uses_of(&f.census(), B), 3, "one undo puts it all back");
}

/// Into a variable, fills bind at the alpha that keeps their drawing; a shadow
/// cannot bind and takes the value, and the count says so.
#[test]
fn a_merge_into_a_variable_binds_what_can_bind() {
    let mut f = F::new();
    let v = f.var("color/accent", A);
    for _ in 0..3 {
        f.rect(vec![BrushSource::Bound { var: v, alpha: 1.0 }]);
    }
    let raw = f.rect(vec![solid(B)]);
    f.apply(vec![Operation::SetEffects {
        id: raw,
        effects: keyed_by_position([Effect::new(EffectKind::DropShadow(Shadow {
            color: B,
            ..Shadow::default()
        }))]),
    }]);
    let census = f.census();
    let g = group_of(&census);
    let t = g.default_target(&census);
    let merged = color_review::merge(&f.doc, &[f.root], &census, &g, t);
    assert_eq!((merged.uses, merged.unbound), (2, 1));
    f.apply(merged.tx.0);
    assert_eq!(f.fill(raw), BrushSource::Bound { var: v, alpha: 1.0 });
    let EffectKind::DropShadow(s) = &f.doc.get(raw).unwrap().effects()[0].kind else {
        panic!("a shadow");
    };
    assert_eq!(bytes(s.color), bytes(A), "the shadow took the value");
}

/// A new variable takes the target's value, a first mode when the document has
/// none, and every raw member's users — the target's too.
#[test]
fn merging_into_a_new_variable_binds_every_member() {
    let mut f = F::new();
    let a = f.rect(vec![solid(A)]);
    let a2 = f.rect(vec![solid(A)]);
    let b = f.rect(vec![solid(B)]);
    let census = f.census();
    let g = group_of(&census);
    let (var, mode) = (f.ids.mint_var(), f.ids.mint_mode());
    let merged = color_review::merge_into_new(
        &f.doc,
        &[f.root],
        &census,
        &g,
        0,
        NewVar {
            id: var,
            name: "color/blue",
            first_mode: mode,
        },
    );
    assert_eq!(merged.uses, 3);
    f.hist
        .commit(&mut f.doc, merged.tx)
        .expect("one valid transaction");
    for id in [a, a2, b] {
        assert_eq!(f.fill(id), BrushSource::Bound { var, alpha: 1.0 });
    }
    let made = f.doc.variables().get(var).expect("created");
    assert_eq!(made.name, "color/blue");
    assert_eq!(
        f.doc.variables().resolve_opt(var, None),
        Some(Value::Color(A))
    );
    f.hist.undo(&mut f.doc).unwrap();
    assert!(f.doc.variables().get(var).is_none(), "one undo step");
}

/// The eyedropper's snap: a pixel a hair from a document colour gets that
/// colour, a variable winning; one far from all gets none.
#[test]
fn a_pixel_snaps_to_a_near_document_colour() {
    let mut f = F::new();
    let v = f.var("color/accent", A);
    f.rect(vec![solid(A)]);
    f.rect(vec![BrushSource::Bound { var: v, alpha: 1.0 }]);
    let census = f.census();
    let pixel = Color::from_rgb8(0x6E, 0x8D, 0xDA);
    assert!(delta(pixel, A) <= Threshold::Tight.limit());
    assert_eq!(
        color_review::snap(&census, pixel).and_then(|s| s.var),
        Some(v)
    );
    assert!(color_review::snap(&census, FAR).is_none());
}
