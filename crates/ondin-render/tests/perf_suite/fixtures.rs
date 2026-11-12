//! The documents the suite measures, generated from a fixed seed so every run of
//! one `VERSION` builds the same document, layer for layer.
//!
//! Each page is built in one transaction: `Document::apply` checks the whole
//! document per transaction, so a transaction per frame would make generation
//! quadratic.

use ondin_core::container::{AlignItems, Display, Flex, FlexDirection};
use ondin_core::kurbo::{Affine, BezPath, RoundedRectRadii, Size, Vec2};
use ondin_core::peniko::Color;
use ondin_core::{
    Document, Effect, EffectKind, Fill, IdSource, NodeId, NodeKind, Operation, Shadow, Stroke,
    TextSizing, TextStyle, Transaction, keyed_by_position, remap_subtree, subtree_nodes,
};
use std::collections::HashMap;

/// Bumped whenever a generator changes what it builds: a baseline is comparable
/// only with a run of the same version.
pub const VERSION: u32 = 1;

/// A generated document and the layers the suite edits.
pub struct Fixture {
    pub doc: Document,
    /// A rect on the first page that a commit recolours.
    pub leaf: NodeId,
    /// A node inside the most widely copied main, whose recolour reaches every copy.
    pub main_leaf: Option<NodeId>,
}

pub type Build = fn() -> Fixture;

/// Every fixture by name, in the order the suite runs them.
pub const ALL: &[(&str, Build)] = &[
    ("control", control),
    ("wide", wide),
    ("components", components),
];

/// Three pages of fifteen screens, 40–120 layers each: the everyday document.
pub fn control() -> Fixture {
    let mut g = Gen::new(0xC0);
    let mut doc = Document::new(g.ids.mint());
    let mut leaf = None;
    for p in 0..3 {
        let page = g.page(&mut doc, p);
        let plans: Vec<Plan> = (0..15)
            .map(|i| {
                let width = if i % 3 == 0 { 1440.0 } else { 390.0 };
                let target = g.rng.range(40, 120);
                Plan::new(&mut g.rng, width, target)
            })
            .collect();
        let found = g.screens(page, &plans, 5);
        leaf = leaf.or(found);
        g.commit(&mut doc);
    }
    Fixture {
        doc,
        leaf: leaf.expect("a screen has a rect"),
        main_leaf: None,
    }
}

/// One page of a thousand mobile screens of ~150 layers: the ceiling.
pub fn wide() -> Fixture {
    let mut g = Gen::new(0x1D);
    let mut doc = Document::new(g.ids.mint());
    let page = doc.first_page();
    let plans: Vec<Plan> = (0..1000)
        .map(|_| {
            let target = g.rng.range(130, 170);
            Plan::new(&mut g.rng, 390.0, target)
        })
        .collect();
    let leaf = g.screens(page, &plans, 40);
    g.commit(&mut doc);
    Fixture {
        doc,
        leaf: leaf.expect("a screen has a rect"),
        main_leaf: None,
    }
}

/// Sixty screens built from instances, some overridden, on the first page, and
/// their mains, nested five deep (panel › section › row › button › icon), on
/// the second.
pub fn components() -> Fixture {
    let mut g = Gen::new(0xC5);
    let mut doc = Document::new(g.ids.mint());
    let screens = doc.first_page();
    let mains = g.page(&mut doc, 1);
    let accent = PALETTE[0];

    // Each main is its own transaction, as the next one copies it out of `doc`.
    let icon = g.main(&mut doc, (0.0, 0.0), |g, _, at| {
        let f = g.frame(mains, at, Size::new(24.0, 24.0), 0.0, None);
        let dot = g.ellipse(f, (2.0, 2.0), Size::new(20.0, 20.0), accent);
        g.rect(f, (8.0, 11.0), Size::new(8.0, 2.0), 1.0, Color::WHITE);
        (f, dot)
    });
    let (icon, icon_dot) = icon;
    let (button, _) = g.main(&mut doc, (100.0, 0.0), |g, doc, at| {
        let f = g.frame(mains, at, Size::new(120.0, 40.0), 8.0, Some(accent));
        g.flex(f, FlexDirection::Row, 8.0, [8.0, 16.0, 8.0, 16.0]);
        g.instance(doc, icon, f, (0.0, 0.0));
        g.text(f, (0.0, 0.0), "Button", 14.0, 600, Color::WHITE, None);
        (f, f)
    });
    let (row, _) = g.main(&mut doc, (300.0, 0.0), |g, doc, at| {
        let f = g.frame(mains, at, Size::new(358.0, 64.0), 0.0, Some(Color::WHITE));
        g.flex(f, FlexDirection::Row, 12.0, [12.0; 4]);
        g.stroke(f, PALETTE[6], 1.0);
        let c = g.rng.color();
        g.ellipse(f, (0.0, 0.0), Size::new(40.0, 40.0), c);
        let col = g.frame(f, (0.0, 0.0), Size::new(170.0, 40.0), 0.0, None);
        g.flex(col, FlexDirection::Column, 2.0, [0.0; 4]);
        g.text(col, (0.0, 0.0), "Jane Cooper", 15.0, 600, INK, None);
        g.text(col, (0.0, 0.0), "Last seen 2h ago", 13.0, 400, MUTED, None);
        g.instance(doc, button, f, (0.0, 0.0));
        (f, f)
    });
    let (section, _) = g.main(&mut doc, (700.0, 0.0), |g, doc, at| {
        let f = g.frame(mains, at, Size::new(382.0, 312.0), 12.0, Some(SURFACE));
        g.flex(f, FlexDirection::Column, 8.0, [12.0; 4]);
        g.text(f, (0.0, 0.0), "Recent", 17.0, 700, INK, None);
        for _ in 0..4 {
            g.instance(doc, row, f, (0.0, 0.0));
        }
        (f, f)
    });
    let (panel, _) = g.main(&mut doc, (1100.0, 0.0), |g, doc, at| {
        let f = g.frame(mains, at, Size::new(406.0, 700.0), 16.0, Some(Color::WHITE));
        g.flex(f, FlexDirection::Column, 16.0, [12.0; 4]);
        g.shadow(f);
        g.text(f, (0.0, 0.0), "Activity", 22.0, 700, INK, None);
        g.instance(doc, section, f, (0.0, 0.0));
        g.instance(doc, section, f, (0.0, 0.0));
        (f, f)
    });
    let (card, _) = g.main(&mut doc, (1600.0, 0.0), |g, doc, at| {
        let f = g.frame(mains, at, Size::new(358.0, 260.0), 12.0, Some(Color::WHITE));
        g.flex(f, FlexDirection::Column, 8.0, [12.0; 4]);
        g.shadow(f);
        let c = g.rng.color();
        g.rect(f, (0.0, 0.0), Size::new(334.0, 120.0), 8.0, c);
        g.text(f, (0.0, 0.0), "Card title", 17.0, 600, INK, None);
        g.text(f, (0.0, 0.0), BODY, 13.0, 400, MUTED, Some(334.0));
        g.instance(doc, button, f, (0.0, 0.0));
        (f, f)
    });
    let (nav, _) = g.main(&mut doc, (2000.0, 0.0), |g, doc, at| {
        let f = g.frame(mains, at, Size::new(390.0, 56.0), 0.0, Some(Color::WHITE));
        g.flex(f, FlexDirection::Row, 50.0, [16.0, 24.0, 16.0, 24.0]);
        for _ in 0..5 {
            g.instance(doc, icon, f, (0.0, 0.0));
        }
        (f, f)
    });

    for i in 0..60 {
        let at = (f64::from(i % 15) * 500.0, f64::from(i / 15) * 2000.0);
        let s = g.frame(screens, at, Size::new(390.0, 1800.0), 0.0, Some(SURFACE));
        g.clip(s);
        g.flex(s, FlexDirection::Column, 16.0, [0.0, 0.0, 16.0, 0.0]);
        g.instance(&doc, nav, s, (0.0, 0.0));
        g.instance(&doc, panel, s, (0.0, 0.0));
        for k in 0..3 {
            let copies = g.instance(&doc, card, s, (0.0, 0.0));
            // Every third card retitled, every fifth recoloured: overrides the
            // main's later edits must step around.
            if (i + k) % 3 == 0 {
                let title = card_title(&doc, card, &copies);
                g.ops.push(Operation::SetText {
                    id: title,
                    content: format!("Screen {i} · card {k}"),
                    spans: Default::default(),
                    para_spans: Default::default(),
                });
            }
            if (i + k) % 5 == 0 {
                let c = g.rng.color();
                g.fill(copies[&card], c);
            }
        }
    }
    g.commit(&mut doc);

    let leaf = doc
        .get(card)
        .expect("the card main")
        .children()
        .first()
        .copied()
        .expect("the card's picture");
    Fixture {
        doc,
        leaf,
        main_leaf: Some(icon_dot),
    }
}

/// The copy of the card main's title text among `copies` (source → copy).
fn card_title(doc: &Document, card: NodeId, copies: &HashMap<NodeId, NodeId>) -> NodeId {
    let title = doc.get(card).expect("the card main").children()[1];
    copies[&title]
}

const INK: Color = Color::from_rgba8(0x1c, 0x1f, 0x26, 0xff);
const MUTED: Color = Color::from_rgba8(0x6b, 0x72, 0x80, 0xff);
const SURFACE: Color = Color::from_rgba8(0xf4, 0xf5, 0xf7, 0xff);
const PALETTE: [Color; 8] = [
    Color::from_rgba8(0x3b, 0x5b, 0xdb, 0xff),
    Color::from_rgba8(0x12, 0xb8, 0x86, 0xff),
    Color::from_rgba8(0xf5, 0x9f, 0x00, 0xff),
    Color::from_rgba8(0xe0, 0x31, 0x31, 0xff),
    Color::from_rgba8(0x7b, 0x2c, 0xbf, 0xff),
    Color::from_rgba8(0x0b, 0x72, 0x85, 0xff),
    Color::from_rgba8(0xde, 0xe2, 0xe6, 0xff),
    Color::from_rgba8(0xff, 0x8a, 0x65, 0xff),
];
const BODY: &str = "A short description that wraps onto a second line, as card copy does.";

/// splitmix64: a seeded generator with no dependency behind it.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9e37_79b9_7f4a_7c15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xbf58_476d_1ce4_e5b9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94d0_49bb_1331_11eb);
        z ^ (z >> 31)
    }

    /// A number in `lo..=hi`.
    fn range(&mut self, lo: usize, hi: usize) -> usize {
        lo + (self.next() % (hi - lo + 1) as u64) as usize
    }

    fn color(&mut self) -> Color {
        PALETTE[self.range(0, PALETTE.len() - 1)]
    }
}

/// The blocks a screen stacks under its header, and how tall they make it.
#[derive(Clone, Copy)]
enum Block {
    /// A list row: avatar, two lines of text, a chevron.
    Row,
    /// A card: picture, title, wrapped body, button.
    Card,
    /// A bar chart: a dozen bars on an axis, with a caption.
    Chart,
}

impl Block {
    fn layers(self) -> usize {
        match self {
            Block::Row => 6,
            Block::Card => 7,
            Block::Chart => 15,
        }
    }

    fn height(self) -> f64 {
        match self {
            Block::Row => 64.0,
            Block::Card => 280.0,
            Block::Chart => 220.0,
        }
    }
}

const HEADER: f64 = 64.0;
const GAP: f64 = 16.0;

struct Plan {
    width: f64,
    blocks: Vec<Block>,
}

impl Plan {
    /// Blocks drawn at random until the screen holds about `target` layers.
    fn new(rng: &mut Rng, width: f64, target: usize) -> Self {
        let (mut blocks, mut layers) = (Vec::new(), 7);
        while layers < target {
            let b = match rng.range(0, 9) {
                0..=4 => Block::Row,
                5..=7 => Block::Card,
                _ => Block::Chart,
            };
            layers += b.layers();
            blocks.push(b);
        }
        Self { width, blocks }
    }

    fn height(&self) -> f64 {
        HEADER + self.blocks.iter().map(|b| b.height() + GAP).sum::<f64>() + GAP
    }
}

/// Mints ids and collects one transaction's operations.
struct Gen {
    ids: IdSource,
    rng: Rng,
    ops: Vec<Operation>,
    /// How many children each parent has been given in `ops`.
    filled: HashMap<NodeId, usize>,
    pages: usize,
}

impl Gen {
    fn new(seed: u64) -> Self {
        Self {
            ids: IdSource::new(0x5eed_0000 | seed),
            rng: Rng(seed),
            ops: Vec::new(),
            filled: HashMap::new(),
            pages: 1,
        }
    }

    fn commit(&mut self, doc: &mut Document) {
        let tx = Transaction(std::mem::take(&mut self.ops));
        doc.apply(&tx).expect("the generated transaction applies");
        self.filled.clear();
    }

    /// The `n`th page: the first one the document came with, or a new one.
    fn page(&mut self, doc: &mut Document, n: usize) -> NodeId {
        if n == 0 {
            return doc.first_page();
        }
        let id = self.ids.mint();
        self.ops.push(Operation::AddPage {
            id,
            index: self.pages,
            name: format!("Page {}", n + 1),
            ground: None,
            draft: false,
            modes: Vec::new(),
        });
        self.pages += 1;
        self.commit(doc);
        id
    }

    fn next_index(&mut self, parent: NodeId) -> usize {
        let n = self.filled.entry(parent).or_default();
        *n += 1;
        *n - 1
    }

    fn create(&mut self, parent: NodeId, at: (f64, f64), kind: NodeKind) -> NodeId {
        let id = self.ids.mint();
        let index = self.next_index(parent);
        self.ops.push(Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(Affine::translate(at)),
            name: None,
        });
        id
    }

    fn fill(&mut self, id: NodeId, color: Color) {
        self.ops.push(Operation::SetFills {
            id,
            fills: keyed_by_position([Fill {
                brush: color.into(),
                visible: true,
            }]),
        });
    }

    fn stroke(&mut self, id: NodeId, color: Color, width: f64) {
        self.ops.push(Operation::SetStrokes {
            id,
            strokes: keyed_by_position([Stroke {
                brush: color.into(),
                width,
                ..Stroke::default()
            }]),
        });
    }

    fn shadow(&mut self, id: NodeId) {
        let shadow = Shadow {
            offset: Vec2::new(0.0, 4.0),
            blur: 12.0,
            spread: 0.0,
            color: Color::from_rgba8(0, 0, 0, 0x22),
            ..Shadow::default()
        };
        self.ops.push(Operation::SetEffects {
            id,
            effects: keyed_by_position([Effect {
                kind: EffectKind::DropShadow(shadow),
                visible: true,
                vars: Default::default(),
            }]),
        });
    }

    fn clip(&mut self, id: NodeId) {
        self.ops.push(Operation::SetClip { id, clip: true });
    }

    fn flex(&mut self, id: NodeId, direction: FlexDirection, gap: f64, padding: [f64; 4]) {
        self.ops.push(Operation::SetDisplay {
            id,
            display: Some(Display::Flex(Flex {
                direction,
                align_items: AlignItems::Center,
                row_gap: gap,
                column_gap: gap,
                padding,
                ..Flex::default()
            })),
        });
    }

    fn frame(
        &mut self,
        parent: NodeId,
        at: (f64, f64),
        size: Size,
        radius: f64,
        fill: Option<Color>,
    ) -> NodeId {
        let id = self.create(
            parent,
            at,
            NodeKind::Artboard {
                size,
                corner_radii: RoundedRectRadii::from_single_radius(radius),
                corner_smoothing: 0.0,
            },
        );
        if let Some(c) = fill {
            self.fill(id, c);
        }
        id
    }

    fn rect(
        &mut self,
        parent: NodeId,
        at: (f64, f64),
        size: Size,
        radius: f64,
        c: Color,
    ) -> NodeId {
        let id = self.create(
            parent,
            at,
            NodeKind::Rect {
                size,
                corner_radii: RoundedRectRadii::from_single_radius(radius),
                corner_smoothing: 0.0,
            },
        );
        self.fill(id, c);
        id
    }

    fn ellipse(&mut self, parent: NodeId, at: (f64, f64), size: Size, c: Color) -> NodeId {
        let id = self.create(
            parent,
            at,
            NodeKind::Ellipse {
                size,
                arc: Default::default(),
            },
        );
        self.fill(id, c);
        id
    }

    /// A line of text, or a paragraph wrapped at `wrap` wide.
    #[allow(clippy::too_many_arguments)]
    fn text(
        &mut self,
        parent: NodeId,
        at: (f64, f64),
        content: &str,
        size: f64,
        weight: u16,
        c: Color,
        wrap: Option<f64>,
    ) -> NodeId {
        let id = self.create(
            parent,
            at,
            NodeKind::Text {
                content: content.into(),
                style: Box::new(TextStyle {
                    font_family: "Inter".into(),
                    font_size: size,
                    weight,
                    ..Default::default()
                }),
                spans: Default::default(),
                para_spans: Default::default(),
                paragraph: Default::default(),
                block: Default::default(),
                sizing: wrap.map_or(TextSizing::Auto, TextSizing::AutoHeight),
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
        );
        self.fill(id, c);
        id
    }

    fn chevron(&mut self, parent: NodeId, at: (f64, f64)) -> NodeId {
        let mut path = BezPath::new();
        path.move_to((0.0, 0.0));
        path.line_to((6.0, 6.0));
        path.line_to((0.0, 12.0));
        let id = self.create(
            parent,
            at,
            NodeKind::Path {
                path,
                corner_radii: Vec::new(),
                corner_smoothing: 0.0,
                mirror: ondin_core::symmetry::PathMirror::NONE,
            },
        );
        self.stroke(id, MUTED, 2.0);
        id
    }

    /// `plans` as screens on `page`, `columns` to a row; the first rect laid.
    fn screens(&mut self, page: NodeId, plans: &[Plan], columns: usize) -> Option<NodeId> {
        let (mut x, mut y, mut row_height) = (0.0, 0.0, 0.0_f64);
        let mut leaf = None;
        for (i, plan) in plans.iter().enumerate() {
            if i > 0 && i % columns == 0 {
                (x, y) = (0.0, y + row_height + 200.0);
                row_height = 0.0;
            }
            let found = self.screen(page, (x, y), plan);
            leaf = leaf.or(found);
            x += plan.width + 100.0;
            row_height = row_height.max(plan.height());
        }
        leaf
    }

    /// One screen: a header bar and `plan`'s blocks stacked under it.
    fn screen(&mut self, page: NodeId, at: (f64, f64), plan: &Plan) -> Option<NodeId> {
        let w = plan.width;
        let s = self.frame(
            page,
            at,
            Size::new(w, plan.height()),
            0.0,
            Some(Color::WHITE),
        );
        self.clip(s);
        let accent = self.rng.color();
        let header = self.frame(s, (0.0, 0.0), Size::new(w, HEADER), 0.0, Some(accent));
        self.flex(header, FlexDirection::Row, 16.0, [16.0; 4]);
        self.ellipse(header, (0.0, 0.0), Size::new(32.0, 32.0), Color::WHITE);
        self.text(
            header,
            (0.0, 0.0),
            "Dashboard",
            18.0,
            700,
            Color::WHITE,
            None,
        );
        for _ in 0..3 {
            self.ellipse(header, (0.0, 0.0), Size::new(24.0, 24.0), SURFACE);
        }

        let inner = w - 2.0 * GAP;
        let mut y = HEADER + GAP;
        let mut leaf = None;
        for &b in &plan.blocks {
            match b {
                Block::Row => {
                    let r = self.frame(s, (GAP, y), Size::new(inner, 64.0), 8.0, Some(SURFACE));
                    self.flex(r, FlexDirection::Row, 12.0, [12.0; 4]);
                    let c = self.rng.color();
                    self.ellipse(r, (0.0, 0.0), Size::new(40.0, 40.0), c);
                    let col = self.frame(r, (0.0, 0.0), Size::new(inner - 100.0, 40.0), 0.0, None);
                    self.flex(col, FlexDirection::Column, 2.0, [0.0; 4]);
                    self.text(col, (0.0, 0.0), "Jane Cooper", 15.0, 600, INK, None);
                    self.text(col, (0.0, 0.0), "Last seen 2h ago", 13.0, 400, MUTED, None);
                    self.chevron(r, (0.0, 0.0));
                }
                Block::Card => {
                    let card = self.frame(s, (GAP, y), Size::new(inner, 280.0), 12.0, None);
                    self.fill(card, Color::WHITE);
                    self.shadow(card);
                    let c = self.rng.color();
                    let pic = self.rect(card, (12.0, 12.0), Size::new(inner - 24.0, 140.0), 8.0, c);
                    leaf = leaf.or(Some(pic));
                    self.text(card, (12.0, 164.0), "Card title", 17.0, 600, INK, None);
                    self.text(
                        card,
                        (12.0, 190.0),
                        BODY,
                        13.0,
                        400,
                        MUTED,
                        Some(inner - 24.0),
                    );
                    let button = self.frame(card, (12.0, 228.0), Size::new(120.0, 40.0), 8.0, None);
                    self.fill(button, accent);
                    self.flex(button, FlexDirection::Row, 0.0, [8.0, 16.0, 8.0, 16.0]);
                    self.text(button, (0.0, 0.0), "Open", 14.0, 600, Color::WHITE, None);
                }
                Block::Chart => {
                    let chart = self.create(s, (GAP, y), NodeKind::Group);
                    self.text(chart, (0.0, 0.0), "Weekly", 15.0, 600, INK, None);
                    let line = self.create(
                        chart,
                        (0.0, 200.0),
                        NodeKind::Line {
                            end: (inner, 0.0).into(),
                        },
                    );
                    self.stroke(line, PALETTE[6], 1.0);
                    let bar = (inner - 11.0 * 6.0) / 12.0;
                    for k in 0..12 {
                        let h = self.rng.range(20, 160) as f64;
                        let c = self.rng.color();
                        let x = f64::from(k) * (bar + 6.0);
                        self.rect(chart, (x, 200.0 - h), Size::new(bar, h), 2.0, c);
                    }
                }
            }
            y += b.height() + GAP;
        }
        leaf
    }

    /// A main built by `body` (returning the main's frame and a node of interest)
    /// at `at`, committed so later mains can copy it.
    fn main(
        &mut self,
        doc: &mut Document,
        at: (f64, f64),
        body: impl FnOnce(&mut Self, &Document, (f64, f64)) -> (NodeId, NodeId),
    ) -> (NodeId, NodeId) {
        let (main, inner) = body(self, doc, at);
        self.ops.push(Operation::SetComponent {
            id: main,
            component: true,
        });
        self.commit(doc);
        (main, inner)
    }

    /// An instance of `main` under `parent`, each of its nodes linked to the one
    /// it was copied from, as the app's copy does. Answers source → copy.
    fn instance(
        &mut self,
        doc: &Document,
        main: NodeId,
        parent: NodeId,
        at: (f64, f64),
    ) -> HashMap<NodeId, NodeId> {
        let template: Vec<_> = subtree_nodes(doc, &[main])
            .into_iter()
            .map(|id| doc.get(id).expect("in the main").clone())
            .collect();
        let (nodes, root) = remap_subtree(&template, &mut self.ids).expect("a well-formed main");
        // `remap_subtree` keeps the template's order, so the lists pair up.
        let pairs: Vec<(NodeId, NodeId)> = template
            .iter()
            .zip(&nodes)
            .map(|(src, copy)| (src.id(), copy.id()))
            .collect();
        let index = self.next_index(parent);
        self.ops.push(Operation::InsertSubtree {
            nodes,
            parent,
            index,
        });
        self.ops.push(Operation::SetComponent {
            id: root,
            component: false,
        });
        for &(src, copy) in &pairs {
            self.ops.push(Operation::SetLink {
                id: copy,
                link: Some(src),
            });
        }
        self.ops.push(Operation::SetTransform {
            id: root,
            transform: Affine::translate(at),
        });
        pairs.into_iter().collect()
    }
}
