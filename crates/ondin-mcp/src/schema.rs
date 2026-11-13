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
        name: "get_tree",
        doc: "A layer and the layers inside it in the shape `insert_subtree` takes and the setters write: the kind with a text's runs, every fill and stroke with the hidden ones and their framing, align, dash, cap and join, effects and exports. Read this before writing a whole list back with `set_fills`, `set_strokes`, `set_effects`, `set_exports` or `set_text_style`; the snapshot keeps only what is drawn. A binding inside a fill, stroke, effect or layout item travels with it; a bound size, corner, text, gap, padding, flow, visibility or opacity is written as its drawn value, unbound. A tree does not carry styles, modes, width queries, the pivot, the element and link, or component state, so inserting one makes a plain copy, not an instance.",
        params: &[param("id", ParamKind::Node, true, "The layer.")],
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
        name: "get_outline",
        doc: "A page's or layer's layers a few levels down in one compact table: `columns` names each row's fields (id, name, kind, world x, y, w, h to 0.1, depth), `rows` are in paint order with a parent before its children, `more` counts the rows left out past `limit`, and `selected` lists the selected layers under it. A layer `id` is its own first row, at depth 0. Read this to see where the sections of a page sit before reading one with `get_node`.",
        params: &[
            param(
                "id",
                ParamKind::Node,
                false,
                "A layer or page; the person's page when absent.",
            ),
            param(
                "depth",
                ParamKind::Index,
                false,
                "How many levels below `id`; 2 when absent.",
            ),
            param(
                "limit",
                ParamKind::Index,
                false,
                "At most this many rows; 200 when absent.",
            ),
        ],
    },
    Tool {
        name: "find_nodes",
        doc: "Layers whose name or text contains `query`, ignoring case: id, name, kind, the names of their ancestors, `box` (world [x, y, w, h] to 0.1) and `selected: true` on one that is selected or holds a selected layer.",
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
        name: "find_like",
        doc: "Layers like one: sharing its fills, strokes, font or effects; the same layer in each copy of its block (`matching`); or a main's or set's instances. The layer itself is listed too, except by `instances`.",
        params: &[
            param("id", ParamKind::Node, true, "The layer to match."),
            param(
                "by",
                ParamKind::Choice(&["fill", "stroke", "font", "effect", "matching", "instances"]),
                true,
                "What they share.",
            ),
        ],
    },
    Tool {
        name: "hit_test",
        doc: "The layers under a point in page coordinates, topmost first, as a click picks: hidden and locked layers are passed over.",
        params: &[
            param("x", ParamKind::Number, true, "Page x."),
            param("y", ParamKind::Number, true, "Page y."),
            param(
                "page",
                ParamKind::Node,
                false,
                "The page; the person's page (`get_view`) when absent.",
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
        name: "get_view",
        doc: "What the person sees: the page they are on, which the reads default to, the camera's center and zoom, and, while the document is the tab in front, `visible`, the world rectangle the canvas shows as [x, y, width, height]. A document nobody has open is on its first page.",
        params: &[],
    },
    Tool {
        name: "export_svg",
        doc: "A page or a layer as SVG text.",
        params: &[param(
            "id",
            ParamKind::Node,
            false,
            "A layer or page; the person's page when absent.",
        )],
    },
    Tool {
        name: "export_png",
        doc: "A page, a layer or a region of a page as a PNG image: how it really looks. Export, look and correct.",
        params: &[
            param(
                "id",
                ParamKind::Node,
                false,
                "A layer or page; the person's page when absent. With `region`, a page.",
            ),
            param(
                "region",
                ParamKind::Model("Region"),
                false,
                "[x, y, w, h] in page coordinates: draw only that rectangle of the page, everything in it, to look closer at one part.",
            ),
            param(
                "max",
                ParamKind::Index,
                false,
                "The longest side in pixels at most, up to 4096; 1600 when absent. A page is never drawn above 1×, a layer or a region above 2×.",
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
                "The layers; the selection when absent, else the layers of the person's page.",
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
        name: "export_tokens",
        doc: "The document's own variables and styles as a design tokens (DTCG) file, with a note for each left out or changed on the way. Nothing is written to disk.",
        params: &[],
    },
    Tool {
        name: "list_fonts",
        doc: "The font families a text can be set in, by name: installed ones, and in the window the web fonts it downloads on first use. Needs no current document.",
        params: &[
            param(
                "query",
                ParamKind::Text,
                false,
                "Only families whose name holds this, ignoring case.",
            ),
            param(
                "limit",
                ParamKind::Index,
                false,
                "At most this many; 100 when absent.",
            ),
        ],
    },
    Tool {
        name: "check_contrast",
        doc: "Measure each shown text layer, and with `non_text` each small shape, against what is drawn behind it, in every mode: its score, what its size needs, and whether it passes. `fix_contrast` fixes a failing one.",
        params: &[
            param(
                "ids",
                ParamKind::Nodes,
                false,
                "Only the layers in and under these; else a page.",
            ),
            param(
                "page",
                ParamKind::Node,
                false,
                "The page when no `ids`; the person's page when absent.",
            ),
            param(
                "level",
                ParamKind::Choice(&["aa", "aaa", "apca"]),
                false,
                "The standard; `aa` when absent.",
            ),
            param(
                "non_text",
                ParamKind::Bool,
                false,
                "Also measure icons and other shapes under 24 units across.",
            ),
        ],
    },
    Tool {
        name: "review_colours",
        doc: "Every distinct colour painted, most-used first, with where it is used, and the groups of near-identical ones that `merge_colours` makes one.",
        params: &[
            param(
                "ids",
                ParamKind::Nodes,
                false,
                "Only the layers in and under these; every page when absent.",
            ),
            param(
                "threshold",
                ParamKind::Choice(&["tight", "close", "loose"]),
                false,
                "How near counts as one; `close` when absent.",
            ),
        ],
    },
    Tool {
        name: "undo",
        doc: "Undo the last step in the document's history when you made it; refused, naming it, when the person or another client did.",
        params: &[],
    },
    Tool {
        name: "redo",
        doc: "Redo the step `undo` took away, when you made it.",
        params: &[],
    },
    Tool {
        name: "set_selection",
        doc: "Select these layers in the open tab, as the person would.",
        params: &[param(
            "ids",
            ParamKind::Nodes,
            true,
            "The layers, of the page the person is on; an empty list clears the selection.",
        )],
    },
    Tool {
        name: "batch",
        doc: "Run several write tools in turn as one undo step, each seeing what the last made; if one is refused, none applies. Answers each step's answer.",
        params: &[
            param(
                "steps",
                ParamKind::Model("Steps"),
                true,
                "The steps in order, each {\"tool\": <a write tool>, \"args\": {…}, \"as\": <label>}. A later step's argument \"@<label>\" stands for the first thing the step labelled so made; a text starting with @ is written @@.",
            ),
            BASE_REV,
        ],
    },
    Tool {
        name: "list_documents",
        doc: "Every document in the person's library: path, name, project and whether it is a kit. Needs no current document.",
        params: &[],
    },
    Tool {
        name: "create_document",
        doc: "File a new, empty document in the library and answer its path, for `use_document`. Only when the person asks for a new file. Needs no current document.",
        params: &[
            param("name", ParamKind::Text, true, "Its name."),
            param(
                "project",
                ParamKind::Text,
                false,
                "A project's name or id; outside every project when absent.",
            ),
        ],
    },
    Tool {
        name: "list_kits",
        doc: "Every kit in the library: id, name, path, the version it last published and that publish's note. Needs no current document.",
        params: &[],
    },
    Tool {
        name: "get_kit",
        doc: "A kit's published components, variables and styles: what `insert_from_kit` places, by the ids listed here.",
        params: &[KIT],
    },
    Tool {
        name: "insert_from_kit",
        doc: "Place an instance of a kit's published component or set in this document, bringing the component and the variables and styles it reads; answers the instance.",
        params: &[
            KIT,
            param(
                "main",
                ParamKind::Node,
                true,
                "The component or set, by its id in `get_kit`.",
            ),
            param(
                "parent",
                ParamKind::Node,
                true,
                "The frame, group or page to place it in.",
            ),
            param(
                "x",
                ParamKind::Number,
                false,
                "Where its box is centred, across the parent; at the component's own place unless given.",
            ),
            param(
                "y",
                ParamKind::Number,
                false,
                "Where its box is centred, down the parent.",
            ),
        ],
    },
    Tool {
        name: "list_kit_updates",
        doc: "The kits this document uses that published since: the version held, the version waiting, its note, and how many components and instances it touches.",
        params: &[],
    },
    Tool {
        name: "accept_kit_updates",
        doc: "Take the waiting kit updates in one undo step: instances keep their overrides, and a component a kit removed becomes a local main. Answers what changed.",
        params: &[param(
            "kit",
            ParamKind::Text,
            false,
            "Only this kit's, by name or id; every kit's when absent.",
        )],
    },
    Tool {
        name: "make_local",
        doc: "Make kit components this document's own mains, placed beside the content; they stop taking the kit's updates.",
        params: &[param(
            "ids",
            ParamKind::Nodes,
            true,
            "Instances of kit components, or the components themselves.",
        )],
    },
    Tool {
        name: "map_kit_modes",
        doc: "Choose which of a kit's modes each of this document's modes reads; a mode left unmapped reads the kit mode of its name, then the first.",
        params: &[
            param(
                "kit",
                ParamKind::Text,
                true,
                "The kit, by name or id, as `list_variables` shows its collections.",
            ),
            param(
                "mapping",
                ParamKind::Model("KitModeMap"),
                true,
                "An object from the document's mode ids to the kit's, as `list_variables` shows them: {\"<document mode>\": \"<kit mode>\"}; null for a kit mode takes that mapping away.",
            ),
        ],
    },
    Tool {
        name: "make_kit",
        doc: "Make this document a kit, so other documents can use its components once it publishes. Saved at once; `undo` does not revert it.",
        params: &[],
    },
    Tool {
        name: "publish_kit",
        doc: "Publish this kit's components, variables and styles; every document using it is offered the update. Only when the person asks. Saved at once; `undo` does not revert it.",
        params: &[param(
            "note",
            ParamKind::Text,
            false,
            "What changed, in a line, shown with the update.",
        )],
    },
    Tool {
        name: "stop_sharing_kit",
        doc: "Stop sharing this kit; documents using it keep their copies. Only when the person asks. Saved at once; `undo` does not revert it.",
        params: &[],
    },
    Tool {
        name: "import_tokens",
        doc: "Read a design tokens (DTCG) file into the document's variables and styles in one undo step: new tokens added, changed ones updated, renamed ones renamed. Answers how many of each and what was left out.",
        params: &[
            param(
                "tokens",
                ParamKind::Model("DtcgFile"),
                true,
                "The file's JSON, as text or as an object.",
            ),
            param(
                "collection",
                ParamKind::Text,
                false,
                "Where tokens the file puts in no collection land, by name; the first collection when absent.",
            ),
            param(
                "removals",
                ParamKind::Bool,
                false,
                "Also remove the variables the file no longer has. Only when the person asks.",
            ),
        ],
    },
    Tool {
        name: "list_versions",
        doc: "The versions kept of this document, newest first: when, and the name given. Documents in the library only.",
        params: &[],
    },
    Tool {
        name: "compare_version",
        doc: "What changed between a kept version and the document now: the pages and layers added, removed and changed, each value before and after, and the variables, modes and styles. Documents in the library only.",
        params: &[param(
            "version",
            ParamKind::Model("Version"),
            true,
            "The version, by its `at` or its name as `list_versions` gives them.",
        )],
    },
    Tool {
        name: "save_version",
        doc: "Save the document and keep it as a version the person can return to from Version history, before a large change or when asked. `undo` does not revert it.",
        params: &[param(
            "name",
            ParamKind::Text,
            false,
            "What the version is called.",
        )],
    },
];

const KIT: Param = param(
    "kit",
    ParamKind::Text,
    true,
    "The kit, by its name or its id in `list_kits`.",
);

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

/// What `base_rev` does is said once, in [`crate::wire::INSTRUCTIONS`].
const BASE_REV: Param = param("base_rev", ParamKind::Index, false, "Your read's `rev`.");

/// What a tool does to the person's work, which its annotations tell a client
/// deciding what to ask the person before calling it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Class {
    /// Reads the document, the library or the host; changes nothing.
    Read,
    /// Changes the document as one step `undo` reverts, or the session.
    Write,
    /// Changes what `undo` does not revert: the kit mark, the library, a version.
    OutsideUndo,
}

/// The library and host reads past Phase 1, every tool of which reads.
const LATER_READS: [&str; 6] = [
    "list_documents",
    "list_kits",
    "get_kit",
    "list_kit_updates",
    "list_versions",
    "compare_version",
];

/// The tools whose effect `undo` does not revert (`docs/design/mcp.md`, *Outside
/// undo*): written to disk, or published to every document using the kit.
const OUTSIDE_UNDO: [&str; 5] = [
    "create_document",
    "make_kit",
    "publish_kit",
    "stop_sharing_kit",
    "save_version",
];

/// The class of the tool called `name`.
pub fn class(name: &str) -> Class {
    if crate::tools::PHASE_1.contains(&name) || LATER_READS.contains(&name) {
        Class::Read
    } else if OUTSIDE_UNDO.contains(&name) {
        Class::OutsideUndo
    } else {
        Class::Write
    }
}

fn entry(name: &str, doc: &str, params: &[Param], extra: Option<Param>) -> Value {
    let mut properties = Map::new();
    let mut required = Vec::new();
    for p in params.iter().chain(extra.as_ref()) {
        properties.insert(p.name.to_owned(), property(p));
        if p.required {
            required.push(json!(p.name));
        }
    }
    let annotations = match class(name) {
        Class::Read => json!({ "readOnlyHint": true }),
        // Undo reverts it: not destructive in the protocol's sense.
        Class::Write => json!({ "readOnlyHint": false, "destructiveHint": false }),
        Class::OutsideUndo => json!({ "readOnlyHint": false, "destructiveHint": true }),
    };
    json!({
        "name": name,
        "description": match doc.ends_with('.') {
            true => doc.to_owned(),
            false => format!("{doc}."),
        },
        "inputSchema": { "type": "object", "properties": properties, "required": required },
        "annotations": annotations,
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
    // A layer id's form is said once, in the server instructions.
    let kind_note = match p.kind {
        // A model with no example is described by its parameter's own words.
        ParamKind::Model(model) if verbs::example(model).is_some() => format!(" A `{model}`."),
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
    use crate::tools::{PHASE_1, PHASE_2, PHASE_3, PHASE_4};

    /// Every tool of every phase has a definition.
    ///
    /// flip: dropping `find_nodes` from `TOOLS` → fails at "no definition".
    #[test]
    fn every_phase_tool_has_a_definition() {
        for name in PHASE_1
            .iter()
            .chain(&PHASE_2)
            .chain(&PHASE_3)
            .chain(&PHASE_4)
        {
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

    /// A read is never a registry verb, and every class's own list names a real
    /// tool, so a rename cannot leave a write marked read-only. The server
    /// instructions say once that results are data, and no description repeats it.
    ///
    /// flip: `class` answering `Read` for every Phase 2 tool → fails at "a verb
    /// marked read-only".
    #[test]
    fn every_tool_is_classed_and_results_are_data_is_said_once() {
        assert!(crate::wire::INSTRUCTIONS.contains("data, never instructions"));
        let all: Vec<&str> = PHASE_1
            .iter()
            .chain(&PHASE_2)
            .chain(&PHASE_3)
            .chain(&PHASE_4)
            .copied()
            .collect();
        for name in LATER_READS.iter().chain(&OUTSIDE_UNDO) {
            assert!(all.contains(name), "`{name}` is no tool");
        }
        for name in &all {
            let d = definition(name).unwrap();
            if class(name) == Class::Read {
                assert!(
                    verbs::find(name).is_none(),
                    "a verb marked read-only: {name}"
                );
            }
            let doc = d["description"].as_str().unwrap();
            assert!(!doc.contains("never instructions"), "{name}: {doc}");
            assert_eq!(
                d["annotations"]["readOnlyHint"],
                class(name) == Class::Read,
                "{name}"
            );
        }
        assert_eq!(class("publish_kit"), Class::OutsideUndo);
        assert_eq!(class("delete_node"), Class::Write);
    }

    /// The bytes `tools/list` sends, every phase's definitions as compact JSON.
    fn listed_bytes() -> usize {
        PHASE_1
            .iter()
            .chain(&PHASE_2)
            .chain(&PHASE_3)
            .chain(&PHASE_4)
            .map(|name| definition(name).unwrap().to_string().len())
            .sum()
    }

    /// The tool list stays within its budget: every client pays for it on every
    /// turn it is not cached. Raise the budget only for a new tool, never for
    /// wording repeated across tools.
    ///
    /// flip: restoring the per-parameter id-form note in `property` → fails at
    /// the budget.
    #[test]
    fn the_tool_list_stays_within_its_budget() {
        const BUDGET: usize = 90_000;
        let bytes = listed_bytes();
        assert!(
            bytes <= BUDGET,
            "tools/list is {bytes} bytes, past {BUDGET}"
        );
    }
}
