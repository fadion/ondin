//! Tool definitions for `tools/list`: a write's from its registry verb, a read's
//! and a session tool's from the tables here, in the same `Param` form.

use ondin_core::verbs::{self, Param, ParamKind};
use serde_json::{Map, Value, json};

/// A tool that is not a registry verb: its line and its parameters.
pub struct Tool {
    pub name: &'static str,
    pub doc: &'static str,
    pub params: &'static [Param],
}

const fn param(name: &'static str, kind: ParamKind, required: bool, doc: &'static str) -> Param {
    Param {
        name,
        kind,
        required,
        doc,
    }
}

const SINCE: Param = param(
    "since",
    ParamKind::Index,
    false,
    "A `rev` from an earlier result: answer `unchanged`, or only the layers changed and removed since.",
);

/// The tools that read, and the ones that act on the session rather than the
/// document. A read's `since` is listed here; a write's `base_rev` is added to
/// every verb by [`definition`].
pub static TOOLS: &[Tool] = &[
    Tool {
        name: "use_document",
        doc: "Pick the document every later call acts on: an absolute path, or a file name or relative path the person used. Call it first, and again only when the person names another file. A document not open yet opens without disturbing the person.",
        params: &[param(
            "document",
            ParamKind::Text,
            true,
            "Absolute path, or a name such as `dashboard.ondin` or `dashboard`.",
        )],
    },
    Tool {
        name: "get_document",
        doc: "The whole document's snapshot: pages and every layer with world transforms, bounds, geometry and paints. Large; prefer `find_nodes` and `get_node` for one part of a page.",
        params: &[SINCE],
    },
    Tool {
        name: "get_node",
        doc: "One layer's snapshot, and with `subtree` every layer under it.",
        params: &[
            param("id", ParamKind::Node, true, "The layer."),
            param(
                "subtree",
                ParamKind::Bool,
                false,
                "Also every layer under it.",
            ),
            SINCE,
        ],
    },
    Tool {
        name: "list_children",
        doc: "A layer's or page's children in order, front last: id, name and kind. With no `id`, the pages.",
        params: &[
            param("id", ParamKind::Node, false, "The layer or page."),
            SINCE,
        ],
    },
    Tool {
        name: "find_nodes",
        doc: "Layers whose name or text contains `query`, ignoring case: id, name, kind and the names of their ancestors.",
        params: &[
            param("query", ParamKind::Text, true, "What to look for."),
            param("page", ParamKind::Node, false, "Only this page."),
            param(
                "limit",
                ParamKind::Index,
                false,
                "At most this many; 50 when absent.",
            ),
        ],
    },
    Tool {
        name: "hit_test",
        doc: "The layers under a point in page coordinates, topmost first.",
        params: &[
            param("x", ParamKind::Number, true, "Page x."),
            param("y", ParamKind::Number, true, "Page y."),
            param(
                "page",
                ParamKind::Node,
                false,
                "The page; the first when absent.",
            ),
        ],
    },
    Tool {
        name: "get_bounds",
        doc: "A layer's world bounds as [x, y, width, height].",
        params: &[param("id", ParamKind::Node, true, "The layer.")],
    },
    Tool {
        name: "get_selection",
        doc: "The layers the person has selected in the open tab; empty in a document nobody has open.",
        params: &[],
    },
    Tool {
        name: "export_svg",
        doc: "A page or a layer as SVG text.",
        params: &[param(
            "id",
            ParamKind::Node,
            false,
            "A layer or page; the first page when absent.",
        )],
    },
    Tool {
        name: "export_png",
        doc: "A page or a layer as a PNG image: how it really looks. Export, look and correct.",
        params: &[
            param(
                "id",
                ParamKind::Node,
                false,
                "A layer or page; the first page when absent.",
            ),
            param(
                "max",
                ParamKind::Index,
                false,
                "The longest side in pixels; 1600 when absent.",
            ),
        ],
    },
    Tool {
        name: "run_exports",
        doc: "Render the export settings the layers carry (the Export card's rows) and return the files with their names; nothing is written to disk. With no `ids`, every layer with settings, as Export all does.",
        params: &[param(
            "ids",
            ParamKind::Nodes,
            false,
            "The layers; all with settings when absent.",
        )],
    },
    Tool {
        name: "export_code",
        doc: "Layers as HTML/CSS, React or Vue, keeping their layout, variables and components, with what could not be said exactly. Nothing is written to disk.",
        params: &[
            param(
                "target",
                ParamKind::Choice(&["html", "react", "vue"]),
                true,
                "The code to write.",
            ),
            param(
                "ids",
                ParamKind::Nodes,
                false,
                "The layers; the selection when absent, else the first page's layers.",
            ),
        ],
    },
    Tool {
        name: "list_components",
        doc: "Every main component and variant set: id, name, page, description and documentation link; a variant's set and values, a set's properties.",
        params: &[],
    },
    Tool {
        name: "list_variables",
        doc: "Every variable collection with its modes, and every variable with its cells per mode (aliases as written) and the value each resolves to.",
        params: &[],
    },
    Tool {
        name: "list_styles",
        doc: "Every style: id, slot, name, value, description and how many layers use it.",
        params: &[],
    },
    Tool {
        name: "undo",
        doc: "Undo the last step in the document's history, whoever made it.",
        params: &[],
    },
    Tool {
        name: "redo",
        doc: "Redo the step `undo` took away.",
        params: &[],
    },
    Tool {
        name: "set_selection",
        doc: "Select these layers in the open tab, as the person would.",
        params: &[param("ids", ParamKind::Nodes, true, "The layers.")],
    },
];

/// The non-verb tool called `name`.
pub fn tool(name: &str) -> Option<&'static Tool> {
    TOOLS.iter().find(|t| t.name == name)
}

/// The `tools/list` entry for `name`: a table tool, else a registry verb with
/// `base_rev` beside its parameters. `None` for a name that is neither.
pub fn definition(name: &str) -> Option<Value> {
    if let Some(t) = tool(name) {
        return Some(entry(t.name, t.doc, t.params, None));
    }
    let verb = verbs::find(name)?;
    Some(entry(verb.name, verb.doc, verb.params, Some(BASE_REV)))
}

const BASE_REV: Param = param(
    "base_rev",
    ParamKind::Index,
    false,
    "The `rev` your read was made at: the write is refused, naming them, if a layer it writes changed since.",
);

fn entry(name: &str, doc: &str, params: &[Param], extra: Option<Param>) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for p in params.iter().chain(extra.as_ref()) {
        properties.insert(p.name.to_owned(), property(p));
        if p.required {
            required.push(json!(p.name));
        }
    }
    json!({
        "name": name,
        "description": doc,
        "inputSchema": { "type": "object", "properties": properties, "required": required },
    })
}

/// One parameter's JSON Schema.
fn property(p: &Param) -> Value {
    let mut schema = match p.kind {
        ParamKind::Node => json!({ "type": "string" }),
        ParamKind::Nodes => json!({ "type": "array", "items": { "type": "string" } }),
        ParamKind::Number => json!({ "type": "number" }),
        ParamKind::Index => json!({ "type": "integer", "minimum": 0 }),
        ParamKind::Bool => json!({ "type": "boolean" }),
        ParamKind::Text => json!({ "type": "string" }),
        ParamKind::Choice(words) => json!({ "type": "string", "enum": words }),
        ParamKind::Model(_) => json!({}),
        ParamKind::Id(_) => json!({ "type": "string" }),
    };
    let kind_note = match p.kind {
        ParamKind::Node => " A layer id, \"<actor-hex>:<seq>\".".to_owned(),
        ParamKind::Nodes => " Layer ids, \"<actor-hex>:<seq>\".".to_owned(),
        ParamKind::Model(model) => format!(" A `{model}`."),
        ParamKind::Id(what) => format!(" A {what} id."),
        _ => String::new(),
    };
    schema["description"] = json!(format!("{}{kind_note}", p.doc));
    if let ParamKind::Model(model) = p.kind
        && let Some(example) = verbs::example(model)
    {
        schema["examples"] = json!([example]);
    }
    schema
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tools::{PHASE_1, PHASE_2, PHASE_3};

    /// Every Phase 1 to 3 tool has a definition.
    ///
    /// flip: dropping `find_nodes` from `TOOLS` → fails at "no definition".
    #[test]
    fn every_phase_tool_has_a_definition() {
        for name in PHASE_1.iter().chain(&PHASE_2).chain(&PHASE_3) {
            assert!(definition(name).is_some(), "`{name}` has no definition");
        }
    }

    /// A verb's definition carries its parameters, `base_rev`, and a model
    /// parameter's example.
    #[test]
    fn a_verb_definition_carries_base_rev_and_examples() {
        let d = definition("set_fills").unwrap();
        let props = &d["inputSchema"]["properties"];
        assert!(props.get("base_rev").is_some());
        let fills = props
            .as_object()
            .unwrap()
            .values()
            .find(|p| p.get("examples").is_some());
        assert!(fills.is_some(), "no model parameter shows an example: {d}");
    }
}
