//! The reads an agent takes before adding to a page (§8.4, Phase 1):
//! `design_context`, what the layers under a frame or page already use, and
//! `check_layout`, the placement mistakes a render would otherwise show. Both
//! are answered by `ondin_export::context`; this reads their arguments.

use super::{Answer, node, nodes};
use crate::session::EditorSession;
use serde_json::Value;

/// The tools this module answers.
pub(super) const TOOLS: [&str; 2] = ["design_context", "check_layout"];

/// Answer `tool` on `s`.
pub(super) fn run(s: &EditorSession, tool: &str, args: &Value) -> Result<Answer, String> {
    let (doc, res) = (&s.doc, &s.resolved);
    match tool {
        "design_context" => {
            let asked = node(args, "id")?.unwrap_or(s.page);
            ondin_export::context::design_context(doc, res, asked).map(Answer::Json)
        }
        "check_layout" => {
            let roots = match nodes(args, "ids")? {
                Some(ids) => {
                    if let Some(gone) = ids.iter().find(|i| doc.get(**i).is_none()) {
                        return Err(format!("no layer `{}`", gone.to_wire()));
                    }
                    ondin_core::build::outermost(doc, &ids)
                }
                None => vec![super::page_of(s, node(args, "page")?)?],
            };
            Ok(Answer::Json(ondin_export::context::check_layout(
                doc, res, &roots,
            )))
        }
        tool => Err(format!("no tool `{tool}`")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{Affine, RoundedRectRadii, Size};
    use ondin_core::{BlockStyle, NodeId, NodeKind, TextSizing, TextStyle};
    use serde_json::json;

    fn frame(w: f64, h: f64) -> Value {
        json!(NodeKind::Artboard {
            size: Size::new(w, h),
            corner_radii: RoundedRectRadii::default(),
            corner_smoothing: 0.0,
        })
    }

    fn at(x: f64, y: f64) -> Value {
        json!(Affine::translate((x, y)))
    }

    /// A session whose page holds only `tree`.
    fn with(tree: Value) -> EditorSession {
        let mut s = EditorSession::new();
        let starter: Vec<String> = s
            .doc
            .get(s.page)
            .unwrap()
            .children()
            .iter()
            .map(|c| c.to_wire())
            .collect();
        let page = s.page.to_wire();
        let call = |s: &mut EditorSession, name: &str, args: Value| {
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
        };
        if !starter.is_empty() {
            call(&mut s, "delete_node", json!({ "nodes": starter }));
        }
        call(
            &mut s,
            "insert_subtree",
            json!({ "parent": page, "tree": tree }),
        );
        s
    }

    /// `get_outline`'s origin is where the layer's own (0, 0) lies, so a
    /// child's world box less it is the child's position in the layer.
    ///
    /// flip: `reads::origin` reading the layer's local transform for its world
    /// one → fails at the hero's origin, `[0, 200]`.
    #[test]
    fn the_outline_gives_the_origin_a_child_is_placed_from() {
        let headline = NodeKind::Text {
            content: "A headline".into(),
            style: Box::new(TextStyle {
                font_size: 56.0,
                weight: 700,
                ..TextStyle::default()
            }),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: BlockStyle::default(),
            sizing: TextSizing::Auto,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        };
        let s = with(json!({
            "kind": frame(1440.0, 1100.0), "name": "Site", "transform": at(100.0, 50.0),
            "children": [
                { "kind": frame(1440.0, 80.0), "name": "Navigation" },
                {
                    "kind": frame(1440.0, 400.0), "name": "Hero", "transform": at(0.0, 200.0),
                    "children": [{ "kind": headline, "transform": at(120.0, 40.0) }],
                },
            ],
        }));
        let site: NodeId = s.doc.get(s.page).unwrap().children()[0];
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

    /// The handler hands the page, or the outermost of the layers named, to
    /// `check_layout`, and refuses a layer that is gone.
    #[test]
    fn check_layout_reads_its_arguments() {
        let s = with(json!({ "kind": frame(100.0, 100.0), "name": "Card" }));
        let card = s.doc.get(s.page).unwrap().children()[0];
        let Answer::Json(page) = run(&s, "check_layout", &json!({})).unwrap() else {
            panic!("json")
        };
        assert_eq!(page["checked"], json!(1));
        let Answer::Json(one) =
            run(&s, "check_layout", &json!({ "ids": [card.to_wire()] })).unwrap()
        else {
            panic!("json")
        };
        assert_eq!(one["checked"], json!(1));
        let gone = run(&s, "check_layout", &json!({ "ids": ["5a5a:999"] }));
        assert!(gone.is_err());
        let Answer::Json(ctx) = run(&s, "design_context", &json!({})).unwrap() else {
            panic!("json")
        };
        assert_eq!(ctx["root"]["name"], json!("Card"));
    }
}
