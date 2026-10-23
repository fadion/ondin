//! MCP tool → Operation/query adapters (§8.4).
//!
//! Selection is app state, not document state, but is deliberately part of the
//! MCP surface ("the selected layer" is how humans phrase AI requests) — read
//! from / written to the owning process's session state, never the Document.
//! Handlers are implemented at M6.
//!
//! The tool names are constants rather than prose so a test can compare them with
//! §8.4; a list written in a comment is a gate nobody built, and this copy would
//! otherwise sit unread, drifting from the design, until the handlers land.

/// Phase 1 — read/export, ship first (§8.4), in the order §8.4 gives it so a
/// reordering shows as a diff.
pub const PHASE_1: [&str; 13] = [
    "get_document",
    "get_node",
    "list_children",
    "hit_test",
    "get_bounds",
    "get_selection",
    "export_svg",
    "export_png",
    "run_exports",
    "export_code",
    "list_components",
    "list_variables",
    "list_styles",
];

/// Phase 2 — write, 1:1 with operations/composites (§8.4).
///
/// `create_node`, `insert_subtree`, `duplicate` and `add_image` return the new
/// ids: the handler minted them, so it knows them.
pub const PHASE_2: [&str; 33] = [
    "create_node",
    "insert_subtree",
    "delete_node",
    "reparent",
    "reorder",
    "group",
    "ungroup",
    "duplicate",
    "set_transform",
    "set_pivot",
    "set_geometry",
    "set_name",
    "set_visible",
    "set_locked",
    "set_opacity",
    "set_clip",
    "set_mask",
    "set_text",
    "set_text_style",
    "set_text_spans",
    "set_paragraph_style",
    "set_block_style",
    "set_fills",
    "set_strokes",
    "set_effects",
    "add_image",
    "set_display",
    "set_layout_item",
    "set_insets",
    "set_exports",
    "undo",
    "redo",
    "set_selection",
];

/// Phase 3 — components, variables and styles, through the verbs rather than
/// the raw ops (§8.4).
///
/// Verbs, because the ops alone are refused or wrong: a `Set*Vars` op writes the
/// binding and not the literal beside it, and removing a variable, mode or style
/// still in use is refused at the commit. `create_instance`, `add_variable`,
/// `add_mode` and `add_style` return the new ids.
pub const PHASE_3: [&str; 23] = [
    "make_component",
    "create_instance",
    "detach_instance",
    "swap_instance",
    "combine_as_variants",
    "switch_variant",
    "define_property",
    "set_instance_property",
    "reset_overrides",
    "add_variable",
    "remove_variable",
    "rename_variable",
    "set_variable_value",
    "add_mode",
    "remove_mode",
    "rename_mode",
    "set_frame_mode",
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
/// file's path.
pub const PHASE_4: [&str; 12] = [
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
}
