//! Enforces §3's layering rules, of which invariant 1 (`ondin-core` is headless)
//! is one: core stays testable without a graphics context, and the chrome stays
//! out of the two crates between core and the app. A table rather than a single
//! check, because an unenforced layering rule does not stay true.
//!
//! Two tests answer different questions. The first reads `cargo tree`, so a
//! transitive leak is caught. The second reads the manifests, because §3's vello
//! rule is about what a crate may declare: `ondin-app` and `ondin-export` reach
//! vello through `ondin-render` by construction, so no subtree could assert its
//! absence. Neither can express the other's rule.
//!
//! The tree read must be the tree that is built. With `resolver = "3"` cargo
//! unifies features across the members being built, so `cargo tree -p ondin-core`
//! and `cargo tree --workspace` resolve core's closure differently (for example
//! `smallvec` gains `union` through `wgpu-hal` under `--workspace`), and every gate
//! that compiles core resolves workspace-wide. Unification only adds, so scanning
//! the workspace tree dominates. One `cargo tree` call, not two: running cargo
//! inside `cargo test` is risky enough without a second chance to contend for the
//! package-cache lock.

use std::process::Command;

/// Crate-name prefixes that must never appear in the dependency tree of a crate
/// that has to stay headless — `ondin-core` by invariant 1, `ondin-mcp` by §3: no
/// GPU, no windowing, no UI. The transitive half of core's rule; its direct
/// dependencies are held to `CORE_PERMITTED` below.
///
/// Matched by `starts_with`, so every name is enumerated: `"eframe"` does not
/// start with `"egui"`, nor do `emath`, `ecolor`, `epaint` or `raw-window-handle`.
const HEADLESS: &[&str] = &[
    "vello",             // GPU/CPU renderer — belongs only in ondin-render
    "wgpu",              // GPU abstraction
    "winit",             // windowing
    "egui",              // UI chrome
    "eframe",            // …and its shell, which `egui` does not prefix-match
    "emath",             // egui's maths, likewise
    "ecolor",            // egui's colour, likewise
    "epaint",            // egui's painter, likewise — and `epaint_default_fonts`
    "naga",              // shader translation (pulled by wgpu)
    "glow",              // GL bindings
    "glutin",            // GL context/windowing
    "ash",               // Vulkan bindings
    "metal",             // Metal bindings
    "raw-window-handle", // the seam every windowing crate meets a surface at
];

/// The chrome half of `HEADLESS`, for the crates that are allowed a GPU and not a
/// user interface.
///
/// Plain backticks throughout this file, not `[link]`s: no gate checks doc links
/// in an integration test.
///
/// `glutin` is deliberately absent, which is why this is a second list rather than
/// a filter over the first: `glutin_wgl_sys` legitimately sits under
/// `ondin-render` and `ondin-export` (`wgpu-hal` pulls it for the GL backend).
/// Core forbids it because core may have no GPU at all, a different rule.
/// `ash`, `metal`, `naga` and `raw-window-handle` are absent for the same reason.
const NO_UI: &[&str] = &["egui", "eframe", "emath", "ecolor", "epaint", "winit"];

/// One row per §3 layering rule that can be read off the dependency tree.
///
/// The `vello`/`vello_cpu` rule is not on this table and cannot be: it is about
/// what a manifest may declare, not what a tree may contain, since `ondin-app` and
/// `ondin-export` reach vello through `ondin-render` by construction. It has its
/// own test below, over the manifests.
const RULES: &[(&str, &[&str])] = &[
    // Invariant 1 / §3: core is headless.
    ("ondin-core", HEADLESS),
    // §3: "`ondin-mcp` contains no rendering or UI dependency." An unused
    // `ondin-export` edge once broke this by pulling in vello, wgpu and naga.
    ("ondin-mcp", HEADLESS),
    // §3: "Only `ondin-app` may depend on egui/winit" — the two crates that sit
    // between core and the app and could quietly acquire chrome.
    ("ondin-render", NO_UI),
    ("ondin-export", NO_UI),
];

/// What `ondin-core` may declare: §3's permitted list, which must stay set-equal
/// to the manifest.
const CORE_PERMITTED: &[&str] = &[
    "base64",
    "flo_curves",
    "i_overlay",
    "kurbo",
    "parley",
    "peniko",
    "read-fonts",
    "regex",
    "roxmltree",
    "rstar",
    "rustc-hash",
    "serde",
    "serde_json",
    "skrifa",
    "taffy",
    "thiserror",
];

/// The crate names `manifest` declares under `[dependencies]` and its
/// `[target.….dependencies]` tables, or as a dotted `[dependencies.<name>]`
/// table of either — not dev or build dependencies, which are not part of the
/// shipped graph §3 is about.
fn declared(manifest: &str) -> Vec<String> {
    let mut names = Vec::new();
    let mut in_deps = false;
    for line in manifest.lines() {
        let line = line.trim();
        if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            let target = header.starts_with("target.");
            in_deps = header == "dependencies" || (target && header.ends_with(".dependencies"));
            // The table's keys below are that one crate's, not names.
            let dotted = header.strip_prefix("dependencies.").or_else(|| {
                target
                    .then(|| header.split_once(".dependencies."))
                    .flatten()
                    .map(|(_, name)| name)
            });
            if let Some(name) = dotted {
                names.push(name.trim_matches('"').to_string());
            }
            continue;
        }
        if !in_deps || line.starts_with('#') || line.is_empty() {
            continue;
        }
        // `kurbo.workspace = true`, `taffy = { … }` and `"x" = …` all put the
        // name first.
        let name = line
            .split(['.', '=', ' '])
            .next()
            .unwrap_or_default()
            .trim_matches('"');
        names.push(name.to_string());
    }
    names.sort_unstable();
    names.dedup();
    names
}

/// `ondin-core` declares exactly §3's permitted list: a dependency added to the
/// manifest without the list fails, and so does a name left on the list after
/// the manifest dropped it. Flip: adding `"image"` to `CORE_PERMITTED` →
/// fails, listing it as permitted and undeclared.
#[test]
fn ondin_core_declares_exactly_the_permitted_list() {
    let manifest = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml"),
    )
    .expect("core's manifest is readable");
    let declared = declared(&manifest);
    assert!(
        declared.iter().any(|d| d == "kurbo"),
        "the parse found core's dependencies at all: {declared:?}"
    );
    let mut permitted: Vec<String> = CORE_PERMITTED.iter().map(|s| s.to_string()).collect();
    permitted.sort_unstable();
    assert_eq!(
        declared, permitted,
        "ondin-core's [dependencies] (left) must equal §3's permitted list (right)"
    );
}

/// `declared` reads every way a manifest can name a shipped dependency, and
/// none a dev or build one. Flip: `declared`'s `dotted` arm removed → fails
/// missing `image` and `vello`.
#[test]
fn the_manifest_parse_reads_dotted_tables() {
    let manifest = r#"
[dependencies]
kurbo.workspace = true
[dependencies.image]
version = "0.25"
[target.'cfg(windows)'.dependencies.vello]
version = "0.6"
[target.'cfg(unix)'.dependencies]
rustix = "1"
[dev-dependencies.proptest]
version = "1"
[build-dependencies]
cc = "1"
"#;
    assert_eq!(declared(manifest), ["image", "kurbo", "rustix", "vello"]);
}

/// The packages named by `RULES`, for the vacuity guard below: a rule whose
/// subject is not in the tree at all has checked nothing, and would pass.
fn subjects() -> Vec<&'static str> {
    RULES.iter().map(|(subject, _)| *subject).collect()
}

/// flips: adding `"kurbo"` to a forbidden list → fails naming it, so the walk
/// finds what is under a subject; restoring `ondin-export` to `ondin-mcp`'s
/// manifest → fails naming the vello/wgpu/naga closure (under ondin-mcp). The
/// negative control stands already: `egui` is really in the tree under
/// `ondin-app` and the test is green, so the walk excludes what is not under a
/// subject.
#[test]
fn every_layering_rule_in_section_3_holds_in_the_dependency_tree() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(cargo)
        .args([
            "tree",
            // `--workspace`, not `--package ondin-core` (see the module doc).
            "--workspace",
            "--edges",
            "normal", // ignore dev/build deps — the test harness itself is fine
            "--prefix",
            "depth", // so the subtree under each subject node can be cut out
            "--no-dedupe",
        ])
        .output()
        .expect("failed to run `cargo tree`");

    assert!(
        output.status.success(),
        "cargo tree failed:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );

    let tree = String::from_utf8(output.stdout).expect("cargo tree output is not UTF-8");

    // With `--prefix depth`, each line is "<depth><name> v<version> [..]".
    fn parse(line: &str) -> Option<(usize, &str)> {
        let digits = line.len() - line.trim_start_matches(|c: char| c.is_ascii_digit()).len();
        if digits == 0 {
            return None;
        }
        let depth = line[..digits].parse().ok()?;
        Some((depth, line[digits..].split_whitespace().next()?))
    }

    // Every node under a subject node, at any depth, once per rule. The whole
    // forest is scanned, since `--no-dedupe` repeats a package under every member
    // that reaches it.
    //
    // One pass per rule, not one carrying a stack: `ondin-core` sits inside
    // `ondin-render`'s subtree and core forbids `vello`, which render may use. A
    // single walk would either report render's vello against core or check core
    // against render's looser list.
    let mut unseen: Vec<&str> = subjects();
    let mut violations: Vec<String> = Vec::new();
    for (subject, forbidden) in RULES {
        let mut inside: Option<usize> = None;
        for line in tree.lines() {
            let Some((depth, name)) = parse(line) else {
                continue;
            };
            if let Some(root_depth) = inside
                && depth <= root_depth
            {
                inside = None;
            }
            if name == *subject {
                unseen.retain(|s| s != subject);
                // The outermost occurrence wins: re-rooting inside its own
                // subtree would drop the outer one's remaining siblings.
                if inside.is_none() {
                    inside = Some(depth);
                }
                continue;
            }
            if inside.is_some() && forbidden.iter().any(|prefix| name.starts_with(prefix)) {
                violations.push(format!("{name} (under {subject})"));
            }
        }
    }

    // Without this the test passes when the parse breaks: a changed `--prefix
    // depth` format or a rename would leave `violations` empty and the assertion
    // below green.
    assert!(
        unseen.is_empty(),
        "the tree does not contain {unseen:?} at all, so nothing was checked for \
         {}:\n{tree}",
        unseen.join(", ")
    );
    // Deduped for the message only: `--no-dedupe` repeats a node once per path to
    // it, which buries the name.
    violations.sort_unstable();
    violations.dedup();
    assert!(
        violations.is_empty(),
        "§3's layering rules are broken — a dependency tree pulls in forbidden \
         crate(s): {violations:?}\n\nThe rules checked were {RULES:?}\n\nFull \
         tree:\n{tree}"
    );
}

/// The one §3 rule that is about a manifest and not a tree: only `ondin-render`
/// may declare `vello`/`vello_cpu`. The question is who declares it, not who can
/// reach it.
///
/// §3's `wgpu` half is deliberately not asserted: `ondin-render`'s seam takes
/// `&Device`, `&Queue` and `&TextureView`, so the app cannot call it without
/// naming wgpu and `ondin-app` really declares it.
///
/// The members are read off the directory rather than listed, so a new crate
/// cannot be left unchecked.
///
/// flip: adding `vello.workspace = true` to `crates/ondin-export/Cargo.toml` →
/// fails at the `assert_eq!` (`["ondin-export", "ondin-render"]`); the
/// `manifests` guard stops a broken parse passing as "nobody declares vello".
#[test]
fn only_ondin_render_declares_vello() {
    let crates = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ondin-core has a parent")
        .to_path_buf();

    let mut declarers: Vec<String> = Vec::new();
    let mut manifests = 0;
    for entry in std::fs::read_dir(&crates).expect("crates/ is readable") {
        let dir = entry.expect("readable entry").path();
        let manifest = dir.join("Cargo.toml");
        if !manifest.is_file() {
            continue;
        }
        manifests += 1;
        let member = dir
            .file_name()
            .and_then(|n| n.to_str())
            .expect("a crate directory has a UTF-8 name")
            .to_string();
        let text = std::fs::read_to_string(&manifest).expect("manifest is readable");
        if declared(&text).iter().any(|n| n.starts_with("vello")) {
            declarers.push(member);
        }
    }

    // Without this, a parse that matched nothing would read as compliance (the
    // sibling test's `unseen` guard, again).
    assert!(
        manifests >= 4,
        "only {manifests} manifest(s) found under {}; the scan is not reading \
         the workspace",
        crates.display()
    );

    declarers.sort_unstable();
    declarers.dedup();
    assert_eq!(
        declarers,
        vec!["ondin-render".to_string()],
        "§3: only `ondin-render` may declare vello/vello_cpu"
    );
}
