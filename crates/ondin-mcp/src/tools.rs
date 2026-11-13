//! The MCP tool names by phase (§8.4), held to the design by a test that
//! parses `docs/design/mcp.md`, and to the verb registry by two more.

/// Phase 1 — read/export, ship first (§8.4), in the order §8.4 gives it so a
/// reordering shows as a diff.
pub const PHASE_1: [&str; 22] = [
    "use_document",
    "get_document",
    "get_node",
    "get_tree",
    "list_children",
    "find_nodes",
    "find_like",
    "hit_test",
    "get_bounds",
    "get_selection",
    "get_view",
    "export_svg",
    "export_png",
    "run_exports",
    "export_code",
    "list_components",
    "list_variables",
    "list_styles",
    "export_tokens",
    "list_fonts",
    "check_contrast",
    "review_colours",
];

/// Phase 2 — write, 1:1 with operations/composites (§8.4).
///
/// `create_node`, `insert_subtree`, `duplicate` and `add_image` return the new
/// ids: the handler minted them, so it knows them.
pub const PHASE_2: [&str; 76] = [
    "create_node",
    "create_text",
    "insert_subtree",
    "import_svg",
    "delete_node",
    "reparent",
    "reorder",
    "arrange",
    "group",
    "frame",
    "ungroup",
    "convert_container",
    "boolean",
    "flatten",
    "outline_shape",
    "outline_stroke",
    "outline_text",
    "divide",
    "blend_shapes",
    "duplicate",
    "join_paths",
    "repeat",
    "align",
    "distribute",
    "tidy_up",
    "slice",
    "set_transform",
    "set_pivot",
    "set_geometry",
    "set_size",
    "fit_to_content",
    "pixel_fit",
    "set_name",
    "set_visible",
    "set_locked",
    "set_opacity",
    "set_blend",
    "set_clip",
    "set_mask",
    "set_element",
    "set_link",
    "set_text",
    "replace_text",
    "replace_font",
    "set_text_style",
    "set_text_spans",
    "set_paragraph_style",
    "set_block_style",
    "style_text",
    "style_paragraphs",
    "set_fills",
    "set_strokes",
    "set_effects",
    "merge_colours",
    "add_image",
    "replace_image",
    "fix_contrast",
    "clean_up",
    "fill_content",
    "set_display",
    "set_layout_item",
    "set_insets",
    "insert_chart",
    "set_chart",
    "set_chart_data",
    "flatten_chart",
    "set_exports",
    "add_page",
    "duplicate_page",
    "delete_page",
    "move_page",
    "set_page_draft",
    "undo",
    "redo",
    "set_selection",
    "batch",
];

/// Phase 3 — components, variables and styles, through the verbs rather than
/// the raw ops (§8.4).
///
/// Verbs, because the ops alone are refused or wrong: a `Set*Vars` op writes the
/// binding and not the literal beside it, and removing a variable, mode or style
/// still in use is refused at the commit. `create_instance`, `add_variable`,
/// `add_mode`, `add_collection`, `duplicate_collection` and `add_style` return
/// the new ids.
pub const PHASE_3: [&str; 49] = [
    "make_component",
    "duplicate_as_component",
    "create_instance",
    "detach_instance",
    "swap_instance",
    "combine_as_variants",
    "add_variant",
    "set_variant_value",
    "add_variant_property",
    "rename_variant_property",
    "delete_variant_property",
    "add_variant_value",
    "rename_variant_value",
    "move_variant_value",
    "delete_variant_value",
    "tie_state",
    "switch_variant",
    "define_property",
    "edit_property",
    "delete_property",
    "show_nested_properties",
    "set_description",
    "set_doc_link",
    "set_instance_property",
    "reset_overrides",
    "push_to_main",
    "add_variable",
    "remove_variable",
    "rename_variable",
    "set_variable_value",
    "add_mode",
    "remove_mode",
    "rename_mode",
    "set_frame_mode",
    "move_mode",
    "add_collection",
    "rename_collection",
    "duplicate_collection",
    "delete_collection",
    "set_by_width",
    "set_breakpoint",
    "set_width_query",
    "set_scope",
    "bind_variable",
    "add_style",
    "remove_style",
    "rename_style",
    "set_style_value",
    "apply_style",
];

/// Phase 4 — kits and the library, the app's own actions (§8.4).
///
/// `make_kit`, `publish_kit` and `stop_sharing_kit` write the kit mark, which is
/// document metadata rather than an operation, so `undo` reverts none of them.
/// `insert_from_kit` returns the new instance's id and `create_document` the new
/// file's path. `save_version` pins a file, outside history too; `import_tokens`
/// is one undo step.
pub const PHASE_4: [&str; 15] = [
    "list_documents",
    "create_document",
    "list_kits",
    "get_kit",
    "insert_from_kit",
    "list_kit_updates",
    "accept_kit_updates",
    "make_local",
    "map_kit_modes",
    "make_kit",
    "publish_kit",
    "stop_sharing_kit",
    "import_tokens",
    "list_versions",
    "save_version",
];

#[cfg(test)]
mod tests {
    use super::{PHASE_1, PHASE_2, PHASE_3, PHASE_4};

    /// `docs/design/mcp.md` (§8), embedded at compile time. `include_str!` rather
    /// than a runtime read, so cargo knows the test depends on the document and
    /// editing §8.4 rebuilds and re-runs it.
    const ARCHITECTURE: &str = include_str!("../../../docs/design/mcp.md");

    /// The tool names §8.4 lists under a paragraph starting with `prefix`.
    ///
    /// Parsed from the backticks, with any signature dropped at the `(`: §8.4
    /// writes `group(ids)` where this crate writes bare names. The paragraph ends
    /// at the first blank line, which keeps the *Selection* and *Notes* paragraphs
    /// below it, which also contain backticked names, out of the answer.
    fn listed(prefix: &str) -> Vec<String> {
        let section = ARCHITECTURE
            .split_once("\n### 8.4 Tools\n")
            .expect("§8.4 is in the document")
            .1;
        let para = section
            .split_once(prefix)
            .unwrap_or_else(|| panic!("§8.4 has a paragraph starting {prefix:?}"))
            .1
            .split_once("\n\n")
            .expect("the paragraph ends")
            .0;
        para.split('`')
            .skip(1)
            .step_by(2)
            .map(|name| name.split('(').next().unwrap_or(name).trim().to_string())
            .filter(|name| !name.is_empty())
            .collect()
    }

    /// The crate's tool list is §8.4's tool list, compared in order (§8.4 gives the
    /// order it wants shipped; a set comparison would hide a reordering). The
    /// parsed list is asserted non-empty first: a renamed heading or reflowed
    /// paragraph yields an empty list, and empty against empty is a green test
    /// about nothing.
    ///
    /// flip: renaming one entry (`set_name` → `set_name_TYPO`) → fails at the
    /// `assert_eq!`; rewording the heading (`ship first` → `shipped first`) →
    /// fails at `listed`'s panic, loudly rather than quietly.
    #[test]
    fn the_tool_list_here_is_the_one_the_design_specifies() {
        for (prefix, ours) in [
            ("Phase 1 — read/export (ship first):", &PHASE_1[..]),
            (
                "Phase 2 — write (1:1 with operations/composites):",
                &PHASE_2[..],
            ),
            (
                "Phase 3 — components, variables and styles (through the verbs, never the raw ops):",
                &PHASE_3[..],
            ),
            (
                "Phase 4 — kits and the library (the app's own actions, some outside undo):",
                &PHASE_4[..],
            ),
        ] {
            let theirs = listed(prefix);
            assert!(
                !theirs.is_empty(),
                "§8.4's {prefix:?} paragraph parsed to nothing — the heading or \
                 the layout moved, and this gate is about to go quiet rather than \
                 red"
            );
            assert_eq!(
                theirs,
                ours.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                "this crate's list and §8.4's have drifted under {prefix:?}"
            );
        }
    }

    /// Phase 2 tools no verb answers: the session's, and `batch`, which runs verbs.
    const SESSION: [&str; 4] = ["undo", "redo", "set_selection", "batch"];

    /// Every Phase 2 tool is a verb of `ondin_core::verbs` under the same name or a
    /// host's tool, never both.
    ///
    /// flip: renaming the registry's `set_fills` → fails at "neither a verb".
    #[test]
    fn every_write_tool_is_a_registry_verb() {
        use ondin_core::verbs;
        for tool in PHASE_2 {
            let verb = verbs::find(tool).is_some();
            let session = SESSION.contains(&tool);
            assert!(
                verb || session,
                "`{tool}` is neither a verb nor a session tool"
            );
            assert!(!(verb && session), "`{tool}` is a verb and a session tool");
        }
        for name in SESSION {
            assert!(PHASE_2.contains(&name), "`{name}` is no Phase 2 tool");
        }
    }

    /// Every Phase 3 tool is a verb of `ondin_core::verbs` under the same name.
    ///
    /// flip: renaming the registry's `make_component` → fails at "is no verb".
    #[test]
    fn every_phase_3_tool_is_a_registry_verb() {
        for tool in PHASE_3 {
            assert!(
                ondin_core::verbs::find(tool).is_some(),
                "`{tool}` is no verb"
            );
        }
    }
}
