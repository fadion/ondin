//! The reads an agent takes before adding to a page (§8.4, Phase 1):
//! `design_context`, what the layers under a frame or page already use, and
//! `check_layout`, the placement mistakes a render would otherwise show.

use super::{Answer, node, nodes};
use crate::session::EditorSession;
use ondin_core::kurbo::{Point, Rect};
use ondin_core::peniko::Color;
use ondin_core::typography::{CharAttr, Length};
use ondin_core::{Brush, Document, NodeId, NodeKind, Resolved, TextSizing};
use serde_json::{Value, json};
use std::collections::HashMap;

/// The tools this module answers.
pub(super) const TOOLS: [&str; 2] = ["design_context", "check_layout"];

/// Answer `tool` on `s`.
pub(super) fn run(s: &EditorSession, tool: &str, args: &Value) -> Result<Answer, String> {
    match tool {
        "design_context" => design_context(s, args).map(Answer::Json),
        "check_layout" => check_layout(s, args).map(Answer::Json),
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

/// The most issues `check_layout` lists.
const ISSUES: usize = 100;

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

/// `id`'s corner radius as drawn: one number when its corners agree, else the
/// four from the top left clockwise; `None` when it has square corners.
fn radius(doc: &Document, res: &Resolved, id: NodeId) -> Option<Value> {
    let r = res.used_kind(doc, id).and_then(NodeKind::corner_radii)?;
    let r = doc.resolve_corners(id, r).unwrap_or(r);
    let corners = [r.top_left, r.top_right, r.bottom_right, r.bottom_left].map(r2);
    match corners {
        c if c.iter().all(|c| *c <= 0.0) => None,
        c if c.iter().all(|v| *v == c[0]) => Some(json!(c[0])),
        c => Some(json!(c)),
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
        if let Some(r) = radius(doc, res, id) {
            radii.add(r.to_string());
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
            "origin": super::reads::origin(res, root),
        },
        "layers": layers,
        "text": text,
        "colours": colour_list,
        "radii": radius_list,
        "shadows": shadow_list,
        "fonts": font_list,
        "column": column,
        "sections": sections,
        "gaps": gaps(doc, res, root),
        "controls": controls(doc, res, asked),
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

/// The least height of an empty band `gaps` lists, in world units: less is
/// the spacing between sections, not room for one.
const GAP: f64 = 24.0;

/// The most controls listed.
const CONTROLS: usize = 6;

/// The empty bands across `root` between its shown children, top to bottom,
/// each as the names of the section above and below it, its world top and its
/// height. A child covering the root's whole box is a background and is left
/// out; side-by-side children make one band edge, the lower bottom.
fn gaps(doc: &Document, res: &Resolved, root: NodeId) -> Vec<Value> {
    let Some(r) = doc.get(root) else {
        return Vec::new();
    };
    let whole = frame_box(doc, res, root);
    let mut boxes: Vec<(&str, Rect)> = r
        .children()
        .iter()
        .filter_map(|&c| {
            let n = doc.get(c)?;
            if !doc.shown(c) || n.mask() {
                return None;
            }
            let b = res.world_bounds(c)?;
            (!whole.is_some_and(|w| !passes(w, b))).then_some((n.name(), b))
        })
        .collect();
    boxes.sort_by(|a, b| a.1.y0.total_cmp(&b.1.y0));
    let mut out = Vec::new();
    let mut above: Option<(&str, f64)> = None;
    for (name, b) in boxes {
        match above {
            Some((after, bottom)) if b.y0 - bottom >= GAP && out.len() < SECTIONS => {
                out.push(json!({
                    "after": after,
                    "before": name,
                    "y": r2(bottom),
                    "h": r2(b.y0 - bottom),
                }));
            }
            _ => {}
        }
        if above.is_none_or(|(_, bottom)| b.y1 > bottom) {
            above = Some((name, b.y1));
        }
    }
    out
}

/// The first of `brushes` that puts ink down, as `#rrggbb` when it is a
/// colour and as its kind otherwise.
fn first_ink<'a>(mut brushes: impl Iterator<Item = &'a Brush>) -> Option<String> {
    brushes.find(|b| inks(b)).map(|b| match b {
        Brush::Solid(c) => hex(*c),
        Brush::Gradient(_) => "gradient".to_owned(),
        _ => "image".to_owned(),
    })
}

/// The repeated controls under `root`: a frame or group no larger than 480 ×
/// 120 holding one text and at most two other layers, painted by itself or by
/// a shape covering it, its text inside and centred down that ground. Alike
/// ones are counted together by size, radius, paint and the label's style,
/// most counted first; each gives a sample: the control, its ground and its
/// label, the last two being `like` targets.
fn controls(doc: &Document, res: &Resolved, root: NodeId) -> Vec<Value> {
    let mut found: Tally<String> = Tally::new();
    let mut samples: HashMap<String, Value> = HashMap::new();
    for id in shown_under(doc, root) {
        if doc.is_page(id) || id == root {
            continue;
        }
        let Some(n) = doc.get(id) else { continue };
        if !matches!(
            res.used_kind(doc, id),
            Some(NodeKind::Artboard { .. } | NodeKind::Group)
        ) {
            continue;
        }
        let Some(b) = frame_box(doc, res, id).or_else(|| res.world_bounds(id)) else {
            continue;
        };
        if b.width() > 480.0 || b.height() > 120.0 || b.height() < 12.0 {
            continue;
        }
        let kids: Vec<NodeId> = n
            .children()
            .iter()
            .copied()
            .filter(|k| doc.shown(*k) && doc.get(*k).is_some_and(|k| !k.mask()))
            .collect();
        let texts: Vec<NodeId> = kids
            .iter()
            .copied()
            .filter(|k| drawn_text(doc, res, *k).is_some())
            .collect();
        let ([label], 1..=3) = (texts.as_slice(), kids.len()) else {
            continue;
        };
        let paint = |at: NodeId| {
            let p = doc.resolved_paint(at, doc.get(at)?.paint());
            let fill = first_ink(
                p.fills
                    .iter()
                    .filter(|f| f.visible)
                    .map(|f| f.brush.drawn()),
            );
            let stroke = first_ink(
                p.strokes
                    .iter()
                    .filter(|s| s.visible && s.width > 0.0)
                    .map(|s| s.brush.drawn()),
            );
            (fill.is_some() || stroke.is_some()).then_some((fill, stroke))
        };
        let ground = std::iter::once(id)
            .chain(kids.iter().copied().filter(|k| k != label))
            .find_map(|g| {
                let gb = match g == id {
                    true => b,
                    false => res.world_bounds(g).filter(|gb| !passes(b, *gb))?,
                };
                Some((g, gb, paint(g)?))
            });
        let Some((ground, gb, (fill, stroke))) = ground else {
            continue;
        };
        let (Some(tb), Some(NodeKind::Text { content, style, .. })) =
            (res.world_bounds(*label), drawn_text(doc, res, *label))
        else {
            continue;
        };
        if passes(tb, gb) || (tb.center().y - gb.center().y).abs() > 0.15 * gb.height() + 1.0 {
            continue;
        }
        let ink = style.color.map(hex).or_else(|| {
            let p = doc.resolved_paint(*label, doc.get(*label)?.paint());
            first_ink(
                p.fills
                    .iter()
                    .filter(|f| f.visible)
                    .map(|f| f.brush.drawn()),
            )
        });
        let mut spec = json!({
            "size": [b.width().round(), b.height().round()],
            "label": {
                "family": style.font_family,
                "size": r2(style.font_size),
                "weight": style.weight,
            },
        });
        if let Some(r) = radius(doc, res, ground) {
            spec["radius"] = r;
        }
        if let Some(f) = fill {
            spec["fill"] = json!(f);
        }
        if let Some(s) = stroke {
            spec["stroke"] = json!(s);
        }
        if let Some(c) = ink {
            spec["label"]["colour"] = json!(c);
        }
        let key = spec.to_string();
        if found.add(key.clone()) {
            let text: String = content.chars().take(30).collect();
            samples.insert(
                key,
                json!({
                    "id": id.to_wire(),
                    "ground": ground.to_wire(),
                    "label": label.to_wire(),
                    "text": text,
                }),
            );
        }
    }
    found
        .ranked()
        .into_iter()
        .take(CONTROLS)
        .map(|(key, count)| {
            let mut v = serde_json::from_str::<Value>(&key).unwrap_or(Value::Null);
            v["count"] = json!(count);
            v["sample"] = samples.remove(&key).unwrap_or(Value::Null);
            v
        })
        .collect()
}

/// One finding of `check_layout`.
fn issue(code: &str, ids: &[NodeId], message: String) -> Value {
    json!({
        "code": code,
        "ids": ids.iter().map(|i| i.to_wire()).collect::<Vec<_>>(),
        "message": message,
    })
}

/// Whether `inner` passes `outer` on any side by more than [`SLACK`].
fn passes(inner: Rect, outer: Rect) -> bool {
    inner.x0 < outer.x0 - SLACK
        || inner.y0 < outer.y0 - SLACK
        || inner.x1 > outer.x1 + SLACK
        || inner.y1 > outer.y1 + SLACK
}

/// Whether `inner` shares nothing with `outer`. A side of no extent, a
/// horizontal line's height, counts as inside when it lies within `outer`'s
/// span on that axis: an area test would call every such layer outside.
fn apart(inner: Rect, outer: Rect) -> bool {
    let axis = |a0: f64, a1: f64, c0: f64, c1: f64| match a1 > a0 {
        true => a1 <= c0 || a0 >= c1,
        false => a0 < c0 || a0 > c1,
    };
    axis(inner.x0, inner.x1, outer.x0, outer.x1) || axis(inner.y0, inner.y1, outer.y0, outer.y1)
}

/// Whether `brush` puts any ink down: an image always, a colour or a
/// gradient when some colour of it is not wholly transparent.
fn inks(brush: &Brush) -> bool {
    match brush {
        Brush::Image(_) => true,
        b => colours_of(b).iter().any(|c| c.components[3] > 0.0),
    }
}

/// Whether shown layer `id`, of used kind `kind`, paints nothing: a shape, an
/// empty frame or an empty group with no visible fill, no stroke of width and
/// no effect. Text, booleans and slices are never one, nor is a layer a flex or
/// grid parent lays out, where an empty box is a spacer.
fn draws_nothing(doc: &Document, id: NodeId, kind: &NodeKind, laid_out: bool) -> bool {
    let Some(n) = doc.get(id) else { return false };
    let empty = n.children().is_empty();
    let shape = match kind {
        NodeKind::Rect { .. }
        | NodeKind::Ellipse { .. }
        | NodeKind::Polygon { .. }
        | NodeKind::Star { .. }
        | NodeKind::Line { .. }
        | NodeKind::Spiral { .. }
        | NodeKind::Arc { .. }
        | NodeKind::Path { .. }
        | NodeKind::Artboard { .. }
        | NodeKind::Group => empty,
        _ => false,
    };
    if !shape || (laid_out && !n.insets().is_authored()) {
        return false;
    }
    if doc
        .resolved_effects(id, n.effects())
        .iter()
        .any(|e| e.visible)
    {
        return false;
    }
    let drawn = doc.resolved_paint(id, n.paint());
    let filled = drawn
        .fills
        .iter()
        .any(|f| f.visible && inks(f.brush.drawn()));
    let stroked = drawn
        .strokes
        .iter()
        .any(|st| st.visible && st.width > 0.0 && inks(st.brush.drawn()));
    !filled && !stroked
}

/// What a walk of `check_layout` carries down to a layer.
#[derive(Clone, Copy)]
struct Above {
    /// The nearest frame that clips, and its box.
    clip: Option<(NodeId, Rect)>,
    /// The parent's box when it lays its children out with flex or grid.
    layout: Option<(NodeId, Rect)>,
    /// Whether an ancestor was reported as clipped or overflowing, which
    /// answers for everything inside it.
    reported: bool,
}

/// `check_layout`: placement lints over the shown layers of a page or of the
/// layers under `ids`. Each is a box comparison of what the session already
/// laid out, so none needs a render; see the tool's line for the list.
fn check_layout(s: &EditorSession, args: &Value) -> Result<Value, String> {
    let doc = &s.doc;
    let res = &s.resolved;
    let roots = match nodes(args, "ids")? {
        Some(ids) => {
            if let Some(gone) = ids.iter().find(|i| doc.get(**i).is_none()) {
                return Err(format!("no layer `{}`", gone.to_wire()));
            }
            ondin_core::build::outermost(doc, &ids)
        }
        None => vec![super::page_of(s, node(args, "page")?)?],
    };
    let mut issues = Vec::new();
    let mut texts: Vec<(NodeId, Rect)> = Vec::new();
    let mut checked = 0;
    for root in roots {
        if !doc.shown_through(root) {
            continue;
        }
        let mut above = Above {
            clip: None,
            layout: None,
            reported: false,
        };
        // The clip and layout a root inside a page sits in.
        let parent = doc.get(root).and_then(|n| n.parent());
        if let Some(p) = parent
            && doc.get(p).is_some_and(|n| n.display().is_some())
        {
            above.layout = frame_box(doc, res, p).map(|b| (p, b));
        }
        let mut up = parent;
        while let Some(a) = up {
            let Some(an) = doc.get(a) else { break };
            if an.kind().clips_children() && an.clip() {
                above.clip = frame_box(doc, res, a).map(|b| (a, b));
                break;
            }
            up = an.parent();
        }
        walk(doc, res, root, above, &mut issues, &mut texts, &mut checked);
    }
    // Two texts overlap when their boxes share more than a sliver: over 2
    // units across and a quarter of the shorter one's height.
    for (i, (a, ab)) in texts.iter().enumerate() {
        for (b, bb) in &texts[i + 1..] {
            let x = ab.intersect(*bb);
            if x.width() > 2.0 && x.height() > 0.25 * ab.height().min(bb.height()) {
                issues.push(issue(
                    "text_overlap",
                    &[*a, *b],
                    format!(
                        "Two texts overlap by {} × {}.",
                        r2(x.width()),
                        r2(x.height())
                    ),
                ));
            }
        }
    }
    let more = issues.len().saturating_sub(ISSUES);
    issues.truncate(ISSUES);
    let mut answer = json!({ "issues": issues, "checked": checked });
    if more > 0 {
        answer["more"] = json!(more);
    }
    Ok(answer)
}

/// Check `id` and the shown layers under it, given what lies above it.
fn walk(
    doc: &Document,
    res: &Resolved,
    id: NodeId,
    above: Above,
    issues: &mut Vec<Value>,
    texts: &mut Vec<(NodeId, Rect)>,
    checked: &mut usize,
) {
    let Some(n) = doc.get(id) else { return };
    let page = doc.is_page(id);
    if !page && (!doc.shown(id) || n.mask()) {
        return;
    }
    let kind = res.used_kind(doc, id).unwrap_or(n.kind());
    let bounds = res.world_bounds(id);
    let mut reported = above.reported;
    if !page && !matches!(kind, NodeKind::Slice { .. }) {
        *checked += 1;
        let one = |code: &str, message: String| issue(code, &[id], message);
        if let Some(size) = match kind {
            NodeKind::Text { .. } => res.text_layout(id).map(|l| l.size),
            NodeKind::Line { .. } | NodeKind::Arc { .. } | NodeKind::Spiral { .. } => None,
            k => k.box_size(),
        } && (size.width <= 0.01 || size.height <= 0.01)
        {
            issues.push(one(
                "zero_size",
                format!(
                    "It is {} × {}: nothing of it shows.",
                    r2(size.width),
                    r2(size.height)
                ),
            ));
            // A box with no area intersects nothing: the clip lint would call it
            // outside too.
            reported = true;
        } else if draws_nothing(doc, id, kind, above.layout.is_some()) {
            issues.push(one(
                "draws_nothing",
                "It draws nothing: no fill, stroke or content.".into(),
            ));
        }
        let in_flow = !n.insets().is_authored();
        if let (Some((p, pb)), Some(b)) = (above.layout, bounds)
            && in_flow
            && passes(b, pb)
        {
            issues.push(issue(
                "layout_overflow",
                &[id, p],
                "It runs past the box of the flex or grid frame that lays it out.".into(),
            ));
            reported = true;
        } else if let (Some((c, cb)), Some(b), false) = (above.clip, bounds, reported) {
            if apart(b, cb) {
                issues.push(issue(
                    "outside_clip",
                    &[id, c],
                    "It lies wholly outside the frame that clips it: none of it shows.".into(),
                ));
                reported = true;
            } else if passes(b, cb) {
                issues.push(issue(
                    "clipped",
                    &[id, c],
                    "Part of it lies outside the frame that clips it and is cut off.".into(),
                ));
                reported = true;
            }
        }
        if let NodeKind::Text { sizing, .. } = kind
            && let Some(l) = res.text_layout(id)
        {
            if l.truncated {
                issues.push(one(
                    "text_cut",
                    "Its content does not fit: lines past its box or line limit are cut.".into(),
                ));
            } else if let TextSizing::Fixed(s) = sizing
                && l.warp.is_none()
                && let (Some(first), Some(last)) = (l.baselines.first(), l.baselines.last())
                && (*last > s.height + SLACK || *first < -SLACK)
            {
                issues.push(one(
                    "text_overflow",
                    format!(
                        "Its lines run outside its fixed {} × {} box.",
                        r2(s.width),
                        r2(s.height)
                    ),
                ));
            }
            if let Some(b) = bounds
                && res
                    .world_transform(id)
                    .is_some_and(|w| w.as_coeffs()[1].abs() < 1e-9 && w.as_coeffs()[2].abs() < 1e-9)
            {
                texts.push((id, b));
            }
        }
    }
    let below = Above {
        clip: match kind.clips_children() && n.clip() {
            true => frame_box(doc, res, id).map(|b| (id, b)).or(above.clip),
            false => above.clip,
        },
        layout: n
            .display()
            .and_then(|_| frame_box(doc, res, id))
            .map(|b| (id, b)),
        reported,
    };
    for &c in n.children() {
        walk(doc, res, c, below, issues, texts, checked);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::container::{Display, Flex, LayoutItem};
    use ondin_core::effect::{Effect, EffectKind, Shadow};
    use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
    use ondin_core::{BlockStyle, Fill, TextOverflow, TextStyle};

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

    /// A page of one frame at (100, 50): a navigation bar, a hero 120 below it,
    /// a band of logos and a footer 20 below that. Three buttons of a rounded
    /// rect and a label, one in the bar and two in the hero, a pill frame
    /// painted by itself, and a card too large to be a control.
    fn site() -> (EditorSession, Vec<NodeId>) {
        let label = |s: &str| {
            json!({
                "kind": text(s, 16.0, 600, TextSizing::Fixed(Size::new(100.0, 20.0)), BlockStyle::default()),
                "name": "Label", "transform": at(10.0, 10.0), "fills": fill(255, 255, 255),
            })
        };
        let button = |x: f64, y: f64, s: &str| {
            json!({
                "kind": NodeKind::Group, "name": "Button", "transform": at(x, y),
                "children": [
                    { "kind": rect(120.0, 40.0, 8.0), "name": "Ground", "fills": fill(0x33, 0x66, 0xcc) },
                    label(s),
                ],
            })
        };
        let section = |name: &str, y: f64, h: f64, children: Value| json!({ "kind": frame(1440.0, h), "name": name, "transform": at(0.0, y), "children": children });
        with(json!({
            "kind": frame(1440.0, 1100.0), "name": "Site", "transform": at(100.0, 50.0),
            "fills": fill(255, 255, 255),
            "children": [
                section("Navigation", 0.0, 80.0, json!([
                    { "kind": text("Brand", 20.0, 700, TextSizing::Auto, BlockStyle::default()), "transform": at(120.0, 24.0) },
                    button(1200.0, 20.0, "Sign up"),
                ])),
                section("Hero", 200.0, 400.0, json!([
                    { "kind": text("A headline", 56.0, 700, TextSizing::Auto, BlockStyle::default()), "transform": at(120.0, 40.0) },
                    button(120.0, 300.0, "Start"),
                    button(260.0, 300.0, "Learn more"),
                    {
                        "kind": NodeKind::Artboard {
                            size: Size::new(140.0, 44.0),
                            corner_radii: RoundedRectRadii::from_single_radius(22.0),
                            corner_smoothing: 0.0,
                        },
                        "name": "Pill", "transform": at(420.0, 298.0), "fills": fill(255, 255, 255),
                        "children": [{
                            "kind": text("Watch", 16.0, 400, TextSizing::Fixed(Size::new(120.0, 24.0)), BlockStyle::default()),
                            "name": "Watch", "transform": at(10.0, 10.0), "fills": fill(0x11, 0x11, 0x11),
                        }],
                    },
                ])),
                section("Trusted by", 600.0, 200.0, json!([
                    {
                        "kind": NodeKind::Group, "name": "Card", "transform": at(120.0, 0.0),
                        "children": [
                            { "kind": rect(500.0, 180.0, 8.0), "fills": fill(0xf5, 0xf5, 0xf5) },
                            { "kind": text("A quote", 16.0, 400, TextSizing::Auto, BlockStyle::default()), "transform": at(20.0, 80.0) },
                        ],
                    },
                ])),
                section("Footer", 820.0, 200.0, json!([
                    { "kind": text("Footer", 14.0, 400, TextSizing::Auto, BlockStyle::default()), "transform": at(120.0, 40.0) },
                ])),
            ],
        }))
    }

    /// `design_context` gives the root's origin, the band between the bar and
    /// the hero but not the 20 above the footer, and the buttons counted as
    /// one control with the pill as another, each with its ground and label,
    /// in a small answer.
    ///
    /// flip: `controls` without its 480 × 120 limit → fails at the count of
    /// controls, which gains the card.
    #[test]
    fn design_context_finds_gaps_and_controls() {
        let (s, _) = site();
        let out = design_context(&s, &json!({})).unwrap();
        assert_eq!(out["root"]["name"], json!("Site"), "{out}");
        assert_eq!(out["root"]["origin"], json!([100, 50]));
        assert_eq!(
            out["gaps"],
            json!([{ "after": "Navigation", "before": "Hero", "y": 130.0, "h": 120.0 }])
        );
        let controls = out["controls"].as_array().unwrap();
        assert_eq!(controls.len(), 2, "{out}");
        let button = &controls[0];
        assert_eq!(
            (
                button["size"].clone(),
                button["radius"].clone(),
                button["fill"].clone(),
                button["count"].clone()
            ),
            (json!([120.0, 40.0]), json!(8.0), json!("#3366cc"), json!(3))
        );
        assert_eq!(
            button["label"],
            json!({ "family": "Inter", "size": 16.0, "weight": 600, "colour": "#ffffff" })
        );
        let name = |v: &Value| {
            let id = NodeId::from_wire(v.as_str().unwrap()).unwrap();
            s.doc.get(id).unwrap().name().to_owned()
        };
        let sample = &button["sample"];
        assert_eq!(
            [&sample["id"], &sample["ground"], &sample["label"]].map(name),
            ["Button", "Ground", "Label"]
        );
        assert_eq!(sample["text"], json!("Sign up"));
        let pill = &controls[1];
        assert_eq!(
            (pill["radius"].clone(), pill["count"].clone()),
            (json!(22.0), json!(1))
        );
        assert_eq!(pill["sample"]["ground"], pill["sample"]["id"]);
        assert!(
            out.to_string().len() < 4000,
            "{} bytes",
            out.to_string().len()
        );
    }

    /// `get_outline`'s origin is where the layer's own (0, 0) lies, so a
    /// child's world box less it is the child's position in the layer.
    ///
    /// flip: `reads::origin` reading the layer's local transform for its world
    /// one → fails at the hero's origin, `[0, 200]`.
    #[test]
    fn the_outline_gives_the_origin_a_child_is_placed_from() {
        let (s, _) = site();
        let site = s.doc.get(s.page).unwrap().children()[0];
        let hero = s.doc.get(site).unwrap().children()[1];
        let out = super::super::reads::outline(&s, hero, 1, 50);
        assert_eq!(out["origin"], json!([100, 250]), "{out}");
        let headline = &out["rows"][1];
        assert_eq!(
            (headline[3].clone(), headline[4].clone()),
            (json!(220), json!(290))
        );
        let page = super::super::reads::outline(&s, s.page, 1, 50);
        assert_eq!(page["origin"], json!([0, 0]));
    }

    /// `check_layout` finds each lint once on a card built to show it, and
    /// nothing on the layers placed well.
    ///
    /// flip: `walk` without its `truncated` check → fails at the codes, which
    /// lose `text_cut`.
    #[test]
    fn check_layout_finds_each_lint() {
        let long = "A sentence far too long for a box one line high and a hundred wide.";
        let clip = BlockStyle {
            overflow: TextOverflow::Clip,
            ..BlockStyle::default()
        };
        let fixed = TextSizing::Fixed(Size::new(100.0, 20.0));
        let item = LayoutItem {
            shrink: 0.0,
            ..LayoutItem::default()
        };
        let (s, made) = with(json!({
            "kind": frame(300.0, 300.0), "name": "Card", "clip": true,
            "children": [
                { "kind": text(long, 16.0, 400, fixed, BlockStyle::default()), "name": "Hangs", "transform": at(10.0, 10.0) },
                { "kind": text(long, 16.0, 400, fixed, clip), "name": "Cut", "transform": at(150.0, 10.0) },
                { "kind": rect(100.0, 50.0, 0.0), "name": "Half", "transform": at(250.0, 120.0), "fills": fill(9, 9, 9) },
                { "kind": rect(50.0, 50.0, 0.0), "name": "Gone", "transform": at(400.0, 10.0), "fills": fill(9, 9, 9) },
                { "kind": rect(20.0, 20.0, 0.0), "name": "Bare", "transform": at(150.0, 280.0) },
                { "kind": rect(50.0, 0.0, 0.0), "name": "Flat", "transform": at(10.0, 280.0) },
                { "kind": text("Overlap one", 16.0, 400, TextSizing::Auto, BlockStyle::default()), "name": "A", "transform": at(10.0, 200.0) },
                { "kind": text("Overlap two", 16.0, 400, TextSizing::Auto, BlockStyle::default()), "name": "B", "transform": at(20.0, 204.0) },
                {
                    "kind": frame(100.0, 40.0), "name": "Row", "transform": at(10.0, 120.0),
                    "display": Display::Flex(Flex::default()),
                    "children": [{ "kind": rect(200.0, 20.0, 0.0), "name": "Wide", "item": item }],
                },
                { "kind": rect(40.0, 40.0, 4.0), "name": "Fine", "transform": at(150.0, 240.0), "fills": fill(9, 9, 9) },
            ],
        }));
        let name = |id: &Value| {
            let id = NodeId::from_wire(id.as_str().unwrap()).unwrap();
            s.doc.get(id).unwrap().name().to_owned()
        };
        let out = check_layout(&s, &json!({})).unwrap();
        let mut found: Vec<(String, String)> = out["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| (i["code"].as_str().unwrap().to_owned(), name(&i["ids"][0])))
            .collect();
        found.sort();
        let want = [
            ("clipped", "Half"),
            ("draws_nothing", "Bare"),
            ("layout_overflow", "Wide"),
            ("outside_clip", "Gone"),
            ("text_cut", "Cut"),
            ("text_overflow", "Hangs"),
            ("text_overlap", "A"),
            ("zero_size", "Flat"),
        ]
        .map(|(c, n)| (c.to_owned(), n.to_owned()));
        assert_eq!(found, want, "{out}");
        assert_eq!(out["checked"], json!(made.len()));
        let narrowed =
            check_layout(&s, &json!({ "ids": [made.last().unwrap().to_wire()] })).unwrap();
        assert_eq!(narrowed["issues"], json!([]), "{narrowed}");
    }

    /// The codes `check_layout` finds on `s`'s page, each with its first
    /// layer's name, sorted.
    fn found(s: &EditorSession) -> Vec<(String, String)> {
        let out = check_layout(s, &json!({})).unwrap();
        let mut found: Vec<(String, String)> = out["issues"]
            .as_array()
            .unwrap()
            .iter()
            .map(|i| {
                let id = NodeId::from_wire(i["ids"][0].as_str().unwrap()).unwrap();
                let name = s.doc.get(id).unwrap().name().to_owned();
                (i["code"].as_str().unwrap().to_owned(), name)
            })
            .collect();
        found.sort();
        found
    }

    fn stroke(width: f64) -> Value {
        json!([ondin_core::Stroke {
            width,
            ..Default::default()
        }])
    }

    /// A horizontal and a vertical line without a stroke inside a clipping
    /// frame have boxes of no height or width and are inside it; a line below
    /// the frame is not. All three draw nothing, which is not asked here.
    ///
    /// flip: `apart` as `b.intersect(cb).area() <= 0.0` → fails at the codes,
    /// which gain `outside_clip` for "Across" and "Down".
    #[test]
    fn a_line_inside_a_clip_is_not_outside_it() {
        let line = |x: f64, y: f64| {
            json!(NodeKind::Line {
                end: Point::new(x, y)
            })
        };
        let (s, _) = with(json!({
            "kind": frame(300.0, 300.0), "name": "Card", "clip": true, "fills": fill(255, 255, 255),
            "children": [
                { "kind": line(100.0, 0.0), "name": "Across", "transform": at(10.0, 50.0) },
                { "kind": line(0.0, 100.0), "name": "Down", "transform": at(50.0, 10.0) },
                { "kind": line(100.0, 0.0), "name": "Below", "transform": at(10.0, 400.0) },
            ],
        }));
        let card = s.doc.get(s.page).unwrap().children()[0];
        let across = s.doc.get(card).unwrap().children()[0];
        let b = s.resolved.world_bounds(across).unwrap();
        assert_eq!(b.height(), 0.0, "the fixture's line has no height: {b:?}");
        let want = [("outside_clip", "Below")].map(|(c, n)| (c.to_owned(), n.to_owned()));
        let got: Vec<_> = found(&s)
            .into_iter()
            .filter(|(c, _)| c != "draws_nothing")
            .collect();
        assert_eq!(got, want);
    }

    /// `draws_nothing` takes a shape, an empty frame and a transparent fill,
    /// and passes over a stroke, a shadow, text, a frame with children, a
    /// hidden layer and a spacer a flex frame lays out.
    ///
    /// flip: `draws_nothing` without its `laid_out` exemption → fails at the
    /// codes, which gain "Spacer".
    #[test]
    fn check_layout_finds_what_draws_nothing() {
        let clear = json!([Fill {
            brush: Brush::Solid(Color::from_rgba8(0, 0, 0, 0)).into(),
            visible: true,
        }]);
        let (s, _) = with(json!({
            "kind": frame(400.0, 400.0), "name": "Page", "fills": fill(255, 255, 255),
            "children": [
                { "kind": rect(20.0, 20.0, 0.0), "name": "Bare", "transform": at(10.0, 10.0) },
                { "kind": rect(20.0, 20.0, 0.0), "name": "Clear", "transform": at(40.0, 10.0), "fills": clear },
                { "kind": frame(20.0, 20.0), "name": "Empty", "transform": at(70.0, 10.0) },
                { "kind": rect(20.0, 20.0, 0.0), "name": "Outlined", "transform": at(100.0, 10.0), "strokes": stroke(1.0) },
                {
                    "kind": rect(20.0, 20.0, 0.0), "name": "Shadowed", "transform": at(130.0, 10.0),
                    "effects": [Effect::new(EffectKind::DropShadow(Shadow::default()))],
                },
                { "kind": rect(20.0, 20.0, 0.0), "name": "Hidden", "transform": at(160.0, 10.0), "visible": false },
                { "kind": text("Words", 16.0, 400, TextSizing::Auto, BlockStyle::default()), "name": "Words", "transform": at(10.0, 100.0) },
                {
                    "kind": frame(100.0, 40.0), "name": "Holder", "transform": at(10.0, 200.0),
                    "children": [{ "kind": rect(20.0, 20.0, 0.0), "name": "Dot", "fills": fill(9, 9, 9) }],
                },
                {
                    "kind": frame(200.0, 40.0), "name": "Row", "transform": at(10.0, 300.0), "fills": fill(9, 9, 9),
                    "display": Display::Flex(Flex::default()),
                    "children": [{ "kind": frame(20.0, 20.0), "name": "Spacer" }],
                },
            ],
        }));
        let hidden = s.doc.get(s.page).unwrap().children()[0];
        let hidden = s.doc.get(hidden).unwrap().children()[5];
        assert!(!s.doc.shown(hidden), "the fixture hides one");
        let want = [
            ("draws_nothing", "Bare"),
            ("draws_nothing", "Clear"),
            ("draws_nothing", "Empty"),
        ]
        .map(|(c, n)| (c.to_owned(), n.to_owned()));
        assert_eq!(found(&s), want);
    }
}
