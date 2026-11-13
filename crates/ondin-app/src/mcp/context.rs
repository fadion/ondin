//! The reads an agent takes before adding to a page (§8.4, Phase 1):
//! `design_context`, what the layers under a frame or page already use.

use super::{Answer, node};
use crate::session::EditorSession;
use ondin_core::kurbo::{Point, Rect};
use ondin_core::peniko::Color;
use ondin_core::typography::{CharAttr, Length};
use ondin_core::{Brush, Document, NodeId, NodeKind, Resolved};
use serde_json::{Value, json};
use std::collections::HashMap;

/// The tools this module answers.
pub(super) const TOOLS: [&str; 1] = ["design_context"];

/// Answer `tool` on `s`.
pub(super) fn run(s: &EditorSession, tool: &str, args: &Value) -> Result<Answer, String> {
    match tool {
        "design_context" => design_context(s, args).map(Answer::Json),
        tool => Err(format!("no tool `{tool}`")),
    }
}

/// The most text styles, colours, radii, shadows and names listed; past it a
/// list ends and its `more` says how many were left out.
const LISTED: usize = 24;

/// The most sections listed.
const SECTIONS: usize = 40;

/// How far an edge may pass the box that holds it, in world units, before it
/// counts: a hairline stroke centred on a frame's edge is not a defect.
const SLACK: f64 = 1.0;

/// `v` at two decimal places, the precision every number here is written at.
fn r2(v: f64) -> f64 {
    (v * 100.0).round() / 100.0
}

/// A colour as `#rrggbb`, or `#rrggbbaa` when it is not opaque.
fn hex(c: Color) -> String {
    let [r, g, b, a] = c.to_rgba8().to_u8_array();
    match a {
        255 => format!("#{r:02x}{g:02x}{b:02x}"),
        a => format!("#{r:02x}{g:02x}{b:02x}{a:02x}"),
    }
}

/// `r` as `[x, y, width, height]`.
fn rect_json(r: Rect) -> Value {
    json!([r2(r.x0), r2(r.y0), r2(r.width()), r2(r.height())])
}

/// A length as the text fields show it: `2%` of the font size, or `1.5px`.
fn length(l: Length) -> String {
    match l {
        Length::Em(m) => format!("{}%", r2(m * 100.0)),
        Length::Px(v) => format!("{}px", r2(v)),
    }
}

/// Counts kept in first-seen order, so equal counts list in document order.
struct Tally<K> {
    keys: Vec<K>,
    counts: HashMap<K, usize>,
}

impl<K: std::hash::Hash + Eq + Clone> Tally<K> {
    fn new() -> Self {
        Self {
            keys: Vec::new(),
            counts: HashMap::new(),
        }
    }

    /// Count `k` once more; answers whether it is new.
    fn add(&mut self, k: K) -> bool {
        let n = self.counts.entry(k.clone()).or_insert(0);
        *n += 1;
        if *n == 1 {
            self.keys.push(k);
        }
        *n == 1
    }

    /// Every key with its count, most counted first.
    fn ranked(&self) -> Vec<(K, usize)> {
        let mut out: Vec<(K, usize)> = self
            .keys
            .iter()
            .map(|k| (k.clone(), self.counts[k]))
            .collect();
        out.sort_by_key(|(_, n)| std::cmp::Reverse(*n));
        out
    }
}

/// `items` cut to [`LISTED`], with how many were cut.
fn capped(items: Vec<Value>) -> (Vec<Value>, usize) {
    let more = items.len().saturating_sub(LISTED);
    (items.into_iter().take(LISTED).collect(), more)
}

/// The box a frame, or a group a layout sized, gives its children, in world
/// space; `None` for every other layer.
fn frame_box(doc: &Document, res: &Resolved, id: NodeId) -> Option<Rect> {
    let size = match res.used_kind(doc, id)? {
        NodeKind::Artboard { size, .. } => *size,
        NodeKind::Group => res.used_frame(id)?,
        _ => return None,
    };
    let w = res.world_transform(id)?;
    Some(ondin_core::geometry::transform_rect(
        w,
        Rect::from_origin_size(Point::ZERO, size),
    ))
}

/// The layers under `root` that can be seen, `root` first, in document order:
/// a hidden layer's subtree and a mask, which draws nothing, are passed over.
fn shown_under(doc: &Document, root: NodeId) -> Vec<NodeId> {
    let mut out = Vec::new();
    let mut stack = vec![root];
    while let Some(id) = stack.pop() {
        let Some(n) = doc.get(id) else { continue };
        if !doc.is_page(id) && (!doc.shown(id) || n.mask()) {
            continue;
        }
        out.push(id);
        stack.extend(n.children().iter().rev().copied());
    }
    out
}

/// A text layer's drawn kind: laid out, its bound type fields read.
fn drawn_text(doc: &Document, res: &Resolved, id: NodeId) -> Option<NodeKind> {
    let node = doc.get(id)?;
    let kind = res.used_kind(doc, id)?;
    matches!(kind, NodeKind::Text { .. })
        .then(|| doc.text_drawn_in(doc.variables(), id, kind, node.text_vars()))
}

/// The colour a brush draws as one swatch, `None` for an image.
fn solid(brush: &Brush) -> Option<Color> {
    match brush {
        Brush::Solid(c) => Some(*c),
        _ => None,
    }
}

/// The colours a brush paints: a solid's, or each gradient stop's.
fn colours_of(brush: &Brush) -> Vec<Color> {
    use ondin_core::peniko::color::Srgb;
    match brush {
        Brush::Solid(c) => vec![*c],
        Brush::Gradient(g) => g
            .gradient
            .stops
            .iter()
            .map(|s| s.color.to_alpha_color::<Srgb>())
            .collect(),
        _ => Vec::new(),
    }
}

/// `design_context`: what the layers under a frame, group or page already use,
/// so an agent adding to it can match it without reading its trees.
///
/// Counts only what is seen: hidden layers, hidden paints and masks are left
/// out, and so are image fills, blend modes, opacity and a text's paragraph
/// settings. A text style is its layer's default, its runs' differences unread
/// but for their families and colours.
fn design_context(s: &EditorSession, args: &Value) -> Result<Value, String> {
    let doc = &s.doc;
    let res = &s.resolved;
    let asked = node(args, "id")?.unwrap_or(s.page);
    let at = doc
        .get(asked)
        .ok_or_else(|| format!("no layer `{}`", asked.to_wire()))?;
    if !doc.is_page(asked) && !matches!(at.kind(), NodeKind::Artboard { .. } | NodeKind::Group) {
        return Err(format!(
            "`{}` is not a page, frame or group: pass the one that holds it",
            asked.to_wire()
        ));
    }
    // A page holding one frame is that frame: its sections are the page's.
    let shown: Vec<NodeId> = at
        .children()
        .iter()
        .copied()
        .filter(|c| doc.shown(*c))
        .collect();
    let root = match shown.as_slice() {
        [only] if doc.is_page(asked) && frame_box(doc, res, *only).is_some() => *only,
        _ => asked,
    };

    let mut styles: Tally<(String, String, u16, bool, String, String, String)> = Tally::new();
    let mut samples: HashMap<usize, (NodeId, String)> = HashMap::new();
    let mut colours: Tally<String> = Tally::new();
    let mut roles: HashMap<String, Vec<&'static str>> = HashMap::new();
    let mut radii: Tally<String> = Tally::new();
    let mut shadows: Tally<String> = Tally::new();
    let mut fonts: Tally<String> = Tally::new();
    let mut variables: Tally<String> = Tally::new();
    let mut named_styles: Tally<String> = Tally::new();
    let mut layers = 0;

    let mut paint = |c: Color, role: &'static str| {
        let key = hex(c);
        colours.add(key.clone());
        let r = roles.entry(key).or_default();
        if !r.contains(&role) {
            r.push(role);
        }
    };

    for id in shown_under(doc, asked) {
        if doc.is_page(id) {
            continue;
        }
        let Some(n) = doc.get(id) else { continue };
        layers += 1;
        let is_text = matches!(n.kind(), NodeKind::Text { .. });
        let fill_role = if is_text { "text" } else { "fill" };
        let drawn = doc.resolved_paint(id, n.paint());
        for f in drawn.fills.iter().filter(|f| f.visible) {
            for c in colours_of(f.brush.drawn()) {
                paint(c, fill_role);
            }
        }
        for st in drawn.strokes.iter().filter(|st| st.visible) {
            for c in colours_of(st.brush.drawn()) {
                paint(c, "stroke");
            }
        }
        for e in doc.resolved_effects(id, n.effects()).iter() {
            if !e.visible {
                continue;
            }
            use ondin_core::effect::EffectKind as E;
            let spec = match &e.kind {
                E::DropShadow(sh) | E::InnerShadow(sh) => json!({
                    "kind": if matches!(e.kind, E::DropShadow(_)) { "drop" } else { "inner" },
                    "x": r2(sh.offset.x),
                    "y": r2(sh.offset.y),
                    "blur": r2(sh.blur),
                    "spread": r2(sh.spread),
                    "colour": hex(sh.color),
                }),
                E::LayerBlur { radius } => json!({ "kind": "layer_blur", "blur": r2(*radius) }),
                E::BackgroundBlur { radius } => {
                    json!({ "kind": "background_blur", "blur": r2(*radius) })
                }
                E::Filters(_) => continue,
            };
            shadows.add(spec.to_string());
        }
        if let Some(r) = res
            .used_kind(doc, id)
            .and_then(NodeKind::corner_radii)
            .map(|r| doc.resolve_corners(id, r).unwrap_or(r))
        {
            let corners = [r.top_left, r.top_right, r.bottom_right, r.bottom_left].map(r2);
            if corners.iter().any(|c| *c > 0.0) {
                radii.add(
                    match corners.iter().all(|c| *c == corners[0]) {
                        true => json!(corners[0]),
                        false => json!(corners),
                    }
                    .to_string(),
                );
            }
        }
        for b in ondin_export::snapshot::bindings(n) {
            if let Some(v) = doc.variables().get(b.variable) {
                variables.add(v.name.clone());
            }
        }
        let refs = n.styles();
        for sid in [
            refs.fills,
            refs.strokes,
            refs.effects,
            refs.text,
            refs.layout,
        ]
        .into_iter()
        .flatten()
        {
            if let Some(st) = doc.styles().get(sid) {
                named_styles.add(st.name.clone());
            }
        }
        let Some(NodeKind::Text {
            content,
            style,
            spans,
            ..
        }) = drawn_text(doc, res, id)
        else {
            continue;
        };
        let mut families = vec![style.font_family.clone()];
        for span in spans.as_slice() {
            match &span.attr {
                CharAttr::Family(f) if !families.contains(f) => families.push(f.clone()),
                CharAttr::Color(Some(c)) => paint(*c, "text"),
                _ => {}
            }
        }
        for f in families {
            fonts.add(f);
        }
        let ink = match style.color {
            Some(c) => {
                paint(c, "text");
                hex(c)
            }
            None => drawn
                .fills
                .iter()
                .filter(|f| f.visible)
                .find_map(|f| solid(f.brush.drawn()))
                .map_or_else(String::new, hex),
        };
        let key = (
            style.font_family.clone(),
            format!("{}", r2(style.font_size)),
            style.weight,
            style.italic,
            style.line_height.map_or_else(|| "auto".to_owned(), length),
            length(style.letter_spacing),
            ink,
        );
        if styles.add(key.clone()) {
            let sample: String = content.chars().take(30).collect();
            samples.insert(styles.keys.len() - 1, (id, sample));
        }
    }

    let mut text: Vec<(usize, Value, f64)> = styles
        .keys
        .iter()
        .enumerate()
        .map(|(i, k)| {
            let (id, sample) = &samples[&i];
            let size: f64 = k.1.parse().unwrap_or_default();
            let mut v = json!({
                "family": k.0,
                "size": size,
                "weight": k.2,
                "line_height": k.4,
                "count": styles.counts[k],
                "sample": { "id": id.to_wire(), "text": sample },
            });
            if k.3 {
                v["italic"] = json!(true);
            }
            if k.5 != "0px" && k.5 != "0%" {
                v["letter_spacing"] = json!(k.5);
            }
            if !k.6.is_empty() {
                v["colour"] = json!(k.6);
            }
            (styles.counts[k], v, size)
        })
        .collect();
    text.sort_by(|a, b| b.2.total_cmp(&a.2).then(b.0.cmp(&a.0)));
    let (text, text_more) = capped(text.into_iter().map(|(_, v, _)| v).collect());

    let (colour_list, colour_more) = capped(
        colours
            .ranked()
            .into_iter()
            .map(|(c, n)| json!({ "colour": c, "count": n, "as": roles[&c] }))
            .collect(),
    );
    // Radii and effects are tallied by their JSON text, and read back from it.
    let parsed = |k: &str| serde_json::from_str::<Value>(k).unwrap_or(Value::Null);
    let (radius_list, _) = capped(
        radii
            .ranked()
            .into_iter()
            .map(|(r, n)| json!({ "radius": parsed(&r), "count": n }))
            .collect(),
    );
    let (shadow_list, _) = capped(
        shadows
            .ranked()
            .into_iter()
            .map(|(spec, n)| {
                let mut v = parsed(&spec);
                v["count"] = json!(n);
                v
            })
            .collect(),
    );
    let names = |t: &Tally<String>| -> Vec<String> {
        t.ranked()
            .into_iter()
            .take(LISTED)
            .map(|(k, _)| k)
            .collect()
    };
    let font_list: Vec<Value> = fonts
        .ranked()
        .into_iter()
        .map(|(f, n)| json!({ "family": f, "count": n }))
        .collect();

    let (sections, column) = sections_and_column(doc, res, root);
    let root_node = doc.get(root).ok_or("the root is gone")?;
    let mut answer = json!({
        "root": {
            "id": root.to_wire(),
            "name": root_node.name(),
            "box": frame_box(doc, res, root).or_else(|| res.world_bounds(root)).map(rect_json),
        },
        "layers": layers,
        "text": text,
        "colours": colour_list,
        "radii": radius_list,
        "shadows": shadow_list,
        "fonts": font_list,
        "column": column,
        "sections": sections,
    });
    if text_more > 0 {
        answer["text_more"] = json!(text_more);
    }
    if colour_more > 0 {
        answer["colours_more"] = json!(colour_more);
    }
    let vars = names(&variables);
    if !vars.is_empty() {
        answer["variables"] = json!(vars);
    }
    let st = names(&named_styles);
    if !st.is_empty() {
        answer["styles"] = json!(st);
    }
    Ok(answer)
}

/// `root`'s shown children as sections (name, id, world box), and the content
/// column: the most common left and right edge, rounded to whole units, of
/// what the sections hold. A section's children are its content, less any
/// child spanning the section's whole width, a background; a section with no
/// children holds none. `None` when no section holds content.
fn sections_and_column(doc: &Document, res: &Resolved, root: NodeId) -> (Vec<Value>, Value) {
    let Some(r) = doc.get(root) else {
        return (Vec::new(), Value::Null);
    };
    let mut sections = Vec::new();
    let mut lefts: Tally<i64> = Tally::new();
    let mut rights: Tally<i64> = Tally::new();
    let mut held = 0;
    for &c in r.children() {
        let Some(cn) = doc.get(c) else { continue };
        if !doc.shown(c) || cn.mask() {
            continue;
        }
        let Some(b) = res.world_bounds(c) else {
            continue;
        };
        if sections.len() < SECTIONS {
            sections.push(json!({ "id": c.to_wire(), "name": cn.name(), "box": rect_json(b) }));
        }
        let sb = frame_box(doc, res, c).unwrap_or(b);
        for &k in cn.children() {
            if !doc.shown(k) || doc.get(k).is_some_and(|n| n.mask()) {
                continue;
            }
            let Some(kb) = res.world_bounds(k) else {
                continue;
            };
            if kb.x0 <= sb.x0 + SLACK && kb.x1 >= sb.x1 - SLACK {
                continue;
            }
            held += 1;
            lefts.add(kb.x0.round() as i64);
            rights.add(kb.x1.round() as i64);
        }
    }
    let more = r.children().len().saturating_sub(SECTIONS);
    if more > 0 && sections.len() == SECTIONS {
        sections.push(json!({ "more": more }));
    }
    let column = match (lefts.ranked().first(), rights.ranked().first()) {
        (Some(&(l, nl)), Some(&(rt, nr))) => json!({
            "left": l,
            "right": rt,
            "width": rt - l,
            "of": held,
            "at_left": nl,
            "at_right": nr,
        }),
        _ => Value::Null,
    };
    (sections, column)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::effect::{Effect, EffectKind, Shadow};
    use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
    use ondin_core::{BlockStyle, Fill, TextSizing, TextStyle};

    fn fill(r: u8, g: u8, b: u8) -> Value {
        json!([Fill {
            brush: Brush::Solid(Color::from_rgb8(r, g, b)).into(),
            visible: true,
        }])
    }

    fn at(x: f64, y: f64) -> Value {
        json!(Affine::translate((x, y)))
    }

    fn frame(w: f64, h: f64) -> Value {
        json!(NodeKind::Artboard {
            size: Size::new(w, h),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        })
    }

    fn rect(w: f64, h: f64, radius: f64) -> Value {
        json!(NodeKind::Rect {
            size: Size::new(w, h),
            corner_radii: RoundedRectRadii::from_single_radius(radius),
            corner_smoothing: 0.0,
        })
    }

    fn text(content: &str, size: f64, weight: u16, sizing: TextSizing, block: BlockStyle) -> Value {
        json!(NodeKind::Text {
            content: content.into(),
            style: Box::new(TextStyle {
                font_size: size,
                weight,
                ..TextStyle::default()
            }),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block,
            sizing,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        })
    }

    /// Run verb `name` on `s` and commit it, answering the layers it made.
    fn verb(s: &mut EditorSession, name: &str, args: Value) -> Vec<NodeId> {
        let done = {
            let mut ctx = ondin_core::verbs::Ctx {
                doc: &s.doc,
                res: &s.resolved,
                ids: &mut s.ids,
                images: None,
                contrast: None,
            };
            ondin_core::verbs::call(&mut ctx, name, &args).unwrap()
        };
        s.try_commit(done.tx).unwrap();
        done.created
    }

    /// A session whose page holds only `tree`, answering the layers made.
    fn with(tree: Value) -> (EditorSession, Vec<NodeId>) {
        let mut s = EditorSession::new();
        let starter: Vec<String> = s
            .doc
            .get(s.page)
            .unwrap()
            .children()
            .iter()
            .map(|c| c.to_wire())
            .collect();
        if !starter.is_empty() {
            verb(&mut s, "delete_node", json!({ "nodes": starter }));
        }
        let page = s.page.to_wire();
        let made = verb(
            &mut s,
            "insert_subtree",
            json!({ "parent": page, "tree": tree }),
        );
        (s, made)
    }

    /// A landing page: one frame of two full-width sections whose content sits
    /// in a column from 120 to 1320, a full-width background behind the first.
    fn landing() -> (EditorSession, Vec<NodeId>) {
        let wrap = |w| TextSizing::AutoHeight(w);
        let mut shadow = Shadow::default();
        shadow.offset.y = 8.0;
        with(json!({
            "kind": frame(1440.0, 900.0), "name": "Landing", "fills": fill(255, 255, 255),
            "children": [
                {
                    "kind": frame(1440.0, 500.0), "name": "Hero",
                    "children": [
                        { "kind": rect(1440.0, 500.0, 0.0), "name": "Bg", "fills": fill(0xee, 0xee, 0xee) },
                        {
                            "kind": text("Build pages that read as one", 56.0, 700, wrap(1200.0), BlockStyle::default()),
                            "transform": at(120.0, 80.0), "fills": fill(0x11, 0x11, 0x11),
                        },
                        {
                            "kind": text("A lead paragraph under the title.", 20.0, 400, wrap(1200.0), BlockStyle::default()),
                            "transform": at(120.0, 180.0), "fills": fill(0x55, 0x55, 0x55),
                        },
                        {
                            "kind": rect(160.0, 48.0, 8.0), "name": "Button",
                            "transform": at(120.0, 300.0), "fills": fill(0x33, 0x66, 0xcc),
                            "effects": [Effect::new(EffectKind::DropShadow(shadow))],
                        },
                    ],
                },
                {
                    "kind": frame(1440.0, 400.0), "name": "Features", "transform": at(0.0, 500.0),
                    "children": [
                        {
                            "kind": text("Features", 40.0, 700, wrap(1200.0), BlockStyle::default()),
                            "transform": at(120.0, 40.0), "fills": fill(0x11, 0x11, 0x11),
                        },
                        { "kind": rect(400.0, 200.0, 8.0), "transform": at(120.0, 120.0), "fills": fill(0xf5, 0xf5, 0xf5) },
                        { "kind": rect(400.0, 200.0, 8.0), "transform": at(520.0, 120.0), "fills": fill(0xf5, 0xf5, 0xf5) },
                        { "kind": rect(400.0, 200.0, 8.0), "transform": at(920.0, 120.0), "fills": fill(0xf5, 0xf5, 0xf5) },
                    ],
                },
            ],
        }))
    }

    /// `design_context` on a page of one frame reads that frame: its two
    /// sections, the 120–1320 column past the full-width background, the type
    /// ramp largest first with its ink, the text colours as text, the radius
    /// and the shadow, in a small answer.
    ///
    /// flip: `design_context` without its one-frame descent → fails at the
    /// root's id.
    #[test]
    fn design_context_reads_a_landing_page() {
        let (s, made) = landing();
        let out = design_context(&s, &json!({})).unwrap();
        assert_eq!(out["root"]["id"], json!(made[0].to_wire()), "{out}");
        let names: Vec<&str> = out["sections"]
            .as_array()
            .unwrap()
            .iter()
            .map(|v| v["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["Hero", "Features"]);
        assert_eq!(
            out["sections"][1]["box"],
            json!([0.0, 500.0, 1440.0, 400.0])
        );
        let col = &out["column"];
        assert_eq!(
            (col["left"].clone(), col["right"].clone()),
            (json!(120), json!(1320)),
            "{out}"
        );
        let sizes: Vec<f64> = out["text"]
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["size"].as_f64().unwrap())
            .collect();
        assert_eq!(sizes, [56.0, 40.0, 20.0]);
        assert_eq!(out["text"][0]["weight"], json!(700));
        assert_eq!(out["text"][0]["colour"], json!("#111111"));
        assert_eq!(
            out["text"][0]["sample"]["text"],
            json!("Build pages that read as one")
        );
        let ink = out["colours"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["colour"] == json!("#111111"))
            .unwrap();
        assert_eq!(
            (ink["count"].clone(), ink["as"].clone()),
            (json!(2), json!(["text"]))
        );
        assert_eq!(out["radii"], json!([{ "radius": 8.0, "count": 4 }]));
        assert_eq!(out["shadows"][0]["kind"], json!("drop"));
        assert_eq!(out["shadows"][0]["y"], json!(8.0));
        assert_eq!(out["fonts"], json!([{ "family": "Inter", "count": 3 }]));
        assert!(
            out.to_string().len() < 4000,
            "{} bytes",
            out.to_string().len()
        );
        let one = design_context(&s, &json!({ "id": made[2].to_wire() }));
        assert!(one.is_err(), "a text layer is refused");
    }
}
