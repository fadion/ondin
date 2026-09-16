//! MCP tool → Operation/query adapters (§8.4).
//!
//! Selection is app state, not document state, but is deliberately part of the
//! MCP surface ("the selected layer" is how humans phrase AI requests) — read
//! from / written to the owning process's session state, never the Document.
//! Handlers are implemented at M6.
//!
//! 🚨 **The tool names below used to be prose here, and nothing compared them
//! with §8.4** (§15 D726, `[S22-L3-06]`). Two copies of one list, identical name
//! for name and phase for phase on the day the finding was written — and the
//! window in which they can drift is the *whole* of the time nobody is reading
//! this file: `roadmap.md` files M6 under *Later · Post-v1* and §15 D4 defers
//! `serve`/`mcp-proxy` until the editor is feature-complete. So the copy that is
//! **not** the design would sit here unread across every milestone in which the
//! tool list is most likely to change, and would read as the design to anyone who
//! opened the crate.
//!
//! ⚠️ **`cargo doc` cannot help, and it is worth saying why**, because this crate
//! *is* one of the targets it reads: a prose list of tool names contains no
//! `[link]`, so the one gate that reads doc comments has nothing to resolve. The
//! list had to stop being prose before anything could check it. **A count — or a
//! list — written in a comment is a gate nobody built.**

/// Phase 1 — read/export, ship first (§8.4).
///
/// **The list, in the order §8.4 gives it**, so a diff against the design is a
/// diff and not a set comparison that hides a reordering.
pub const PHASE_1: [&str; 8] = [
    "get_document",
    "get_node",
    "list_children",
    "hit_test",
    "get_bounds",
    "get_selection",
    "export_svg",
    "export_png",
];

/// Phase 2 — write, 1:1 with operations/composites (§8.4).
///
/// `create_node`, `insert_subtree` and `duplicate` return the new ids: the
/// handler minted them, so it knows them.
pub const PHASE_2: [&str; 20] = [
    "create_node",
    "insert_subtree",
    "delete_node",
    "reparent",
    "reorder",
    "set_transform",
    "set_geometry",
    "set_text",
    "set_text_style",
    "set_fills",
    "set_strokes",
    "set_opacity",
    "set_visible",
    "set_name",
    "group",
    "ungroup",
    "duplicate",
    "undo",
    "redo",
    "set_selection",
];

#[cfg(test)]
mod tests {
    use super::{PHASE_1, PHASE_2};

    /// `docs/architecture.md`, embedded at compile time.
    ///
    /// **`include_str!` rather than a path read, and that is the half that makes
    /// this a gate.** A runtime `fs::read` would depend on the working directory
    /// and, worse, would not tell cargo that this test target depends on the
    /// document — so editing §8.4 would leave a stale binary passing. `include_str!`
    /// registers the dependency, so a change to the design rebuilds and re-runs
    /// this.
    const ARCHITECTURE: &str = include_str!("../../../docs/architecture.md");

    /// The tool names §8.4 lists under a paragraph starting with `prefix`.
    ///
    /// **Parsed from the backticks, with any signature dropped at the `(`** —
    /// §8.4 writes `get_document() -> Snapshot` and `group(ids)` where this crate
    /// writes bare names, and a comparison that demanded the same *spelling*
    /// would fail on a difference that is not one. What has to agree is the set of
    /// names and their order.
    ///
    /// The paragraph ends at the first blank line, which is what keeps the
    /// *Selection* and *Notes* paragraphs below — both of which contain backticked
    /// names — out of the answer.
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

    /// **The crate's tool list is §8.4's tool list** (§15 D726, `[S22-L3-06]`).
    ///
    /// Two copies of one list with nothing between them is the shape this project
    /// keeps meeting — *a rule with one statement and more than one
    /// implementation* — and here the second copy sits in a crate no handler will
    /// be written in for three milestones. This is the comparison that did not
    /// exist.
    ///
    /// ⚠️ **Ordered, not set-compared.** §8.4 gives Phase 1 in the order it wants
    /// shipped; a set comparison would call a reordered list identical and lose
    /// exactly the information the phases carry.
    ///
    /// **The fixture is asserted non-empty first**, because every failure mode of
    /// the parser above — a renamed heading, a reflowed paragraph, a `###` that
    /// became `####` — produces an *empty* list, and an empty list compared with
    /// an empty list is a green test about nothing. That is this project's
    /// vacuity trap in its purest form: the gate would go quiet at the exact
    /// moment the document it watches was edited.
    ///
    /// ⚠️ **Flip-checked in both directions, and the second is the one that
    /// matters.** Renaming one entry (`set_name` → `set_name_TYPO`) is red at the
    /// comparison, as predicted. Rewording the *heading* this parser looks for
    /// (`ship first` → `shipped first`) is red at `listed`'s own panic —
    /// **loudly, not quietly** — which is what the `expect` and the non-empty
    /// assertion are between them for. A gate that reads a prose document has two
    /// ways to fail and only one of them is the interesting one; the other is
    /// silence.
    #[test]
    fn the_tool_list_here_is_the_one_the_design_specifies() {
        for (prefix, ours) in [
            ("Phase 1 — read/export (ship first):", &PHASE_1[..]),
            (
                "Phase 2 — write (1:1 with operations/composites):",
                &PHASE_2[..],
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
