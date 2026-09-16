//! Enforces §3's layering rules, of which invariant 1 — `ondin-core` is
//! headless — is one. This is the CI gate that keeps the core testable without
//! a graphics context and keeps the chrome out of the two crates between core
//! and the app.
//!
//! ⚠️ **It enforced exactly one of §3's four rules until 2026-09-15**
//! (§15 D775, `[A2-L7-05]`), and **two of the other three were already false with every
//! gate green** — `ondin-mcp` reaching vello through an `ondin-export` edge no
//! line of its source used (§15 D427), and §3's wgpu clause being unsatisfiable
//! (`[A2-L7-04]`). That is the argument for a table rather than a crate: an
//! unenforced layering rule in this repository does not stay true, and nothing
//! says when it stopped.
//!
//! **There are two tests here and they answer different questions.** The first
//! reads `cargo tree` rather than the manifests, so a *transitive* leak — a
//! permitted dep that itself pulls in wgpu, say — is caught and not just a
//! direct one. The second reads the manifests, because §3's vello rule is about
//! what a crate may **declare**: `ondin-app` and `ondin-export` reach vello
//! through `ondin-render` by construction, so there is no subtree in which its
//! absence could be asserted. 🚨 **Neither test can express the other's rule**,
//! and a reader who folds them together will drop one.
//!
//! ⚠️ **The tree it reads has to be the tree that is *built*, and for a long
//! time it was not.** With `resolver = "3"`, cargo unifies features across the
//! members being built, so `cargo tree -p ondin-core` and `cargo tree
//! --workspace` are two different resolutions of the same crate's closure — and
//! they demonstrably differ today, on core's own dependencies:
//!
//! ```text
//! cargo tree -p ondin-core -e normal -f "{p} {f}" → smallvec  const_generics,const_new,serde
//! cargo tree --workspace   -e normal -f "{p} {f}" → smallvec  const_generics,const_new,serde,union
//! ```
//!
//! (`union` is requested by `wgpu-hal` and reaches the build only through
//! `ondin-render`; it is also the cause of `peniko::Gradient` measuring 168
//! bytes under `-p` and 160 under `--workspace`.) Every gate that *compiles*
//! `ondin-core` — `build`, `test`, `clippy` — resolves workspace-wide. So one
//! member declaring `parley = { features = ["system"] }` would put a
//! DirectWrite binding into the closure core is compiled against, and this test,
//! reading core resolved alone, would have stayed green.
//!
//! Feature unification only ever *adds*, so the workspace-resolved closure is a
//! superset of the standalone one and scanning it strictly dominates. **One
//! `cargo tree` invocation, not two**: running cargo inside a `cargo test` is
//! already the riskiest thing this file does, and a second call is a second
//! chance to contend for the package-cache lock.

use std::process::Command;

/// Crate-name prefixes that must never appear in the dependency tree of a crate
/// that has to stay **headless** — `ondin-core` by invariant 1, `ondin-mcp` by
/// §3. No GPU, no windowing, no UI.
///
/// ⚠️ **This list is narrower than the rule it enforces, and it is matched by
/// `starts_with`, which is where several near-misses live.** §3 states the rule
/// as *"any GPU/windowing/UI crate"*; `"eframe".starts_with("egui")` is `false`,
/// and so are `emath`, `ecolor`, `epaint`, `epaint_default_fonts` and
/// `raw-window-handle` — every one of them really in the app's tree. `epaint`
/// had a partial accidental backstop (it pulls `vello_common`/`vello_cpu`, which
/// trips the `vello` prefix), but that edge is version-contingent and the others
/// had none at all. The names below are now enumerated rather than inferred.
///
/// **The right shape is an allowlist** — §3 already enumerates what core *may*
/// depend on, so the test could assert the direct list is a subset of it and
/// fall back to prefixes for the transitive tree.
///
/// ⚠️ **That change was blocked on §3's list being wrong, and it is not any
/// more** (§15 D427). §3 omitted `roxmltree`, `skrifa`, `read-fonts` and
/// `flo_curves`, all really there, and baking a wrong allowlist in would have
/// been worse than the denylist. The list was corrected on 2026-09-06, so the
/// stated blocker is gone and the inversion is now ordinary work rather than
/// work waiting on a decision.
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

/// The chrome half of `HEADLESS`, for the crates that are allowed a GPU and
/// are not allowed a user interface.
///
/// (Plain backticks throughout this file, not `[link]`s: an integration test is
/// its own crate root, `cargo doc` documents no test target, and there is no
/// `deny` here — so a broken link would be checked by nothing. §15 D319.)
///
/// ⚠️ **`glutin` is deliberately absent, and it is the whole reason this is a
/// second list rather than a filter over the first.** `glutin_wgl_sys` is
/// really under both `ondin-render` and `ondin-export` — `wgpu-hal` pulls it for
/// the GL backend — so a row using `HEADLESS` here would fail on a crate that
/// is doing nothing wrong. It is forbidden to `ondin-core` because core may have
/// no GPU **at all**, which is a different rule with a different reason, and
/// collapsing the two lists is what would make the stricter one unstatable.
/// `ash`, `metal`, `naga` and `raw-window-handle` are absent for the same
/// reason: every one of them arrives legitimately under `wgpu`.
const NO_UI: &[&str] = &["egui", "eframe", "emath", "ecolor", "epaint", "winit"];

/// One row per §3 layering rule that can be read off the dependency tree.
///
/// ⚠️ **This is a table because §3 states *four* rules and, until 2026-09-15,
/// exactly one of them had a test** (`[A2-L7-05]`). Two of the other three were
/// already false with every gate green, which is the argument in its strongest
/// form: an unenforced layering rule in this repository does not stay true, and
/// nothing says when it stopped.
///
/// 🚨 **The rule §3 states about `vello`/`vello_cpu` is not on this table and
/// cannot be**, because it is a rule about what a manifest may *declare*, not
/// about what a tree may contain: `ondin-app` reaches vello through
/// `ondin-render` by construction, and so does `ondin-export`. It has its own
/// test below, over the manifests. **A transitive test cannot express a direct
/// rule**, and reading it as though it could is how the row would come to be
/// omitted as "impossible".
const RULES: &[(&str, &[&str])] = &[
    // Invariant 1 / §3: core is headless.
    ("ondin-core", HEADLESS),
    // §3: "`ondin-mcp` contains no rendering or UI dependency." ⚠️ **§3 has
    // always said so about it, with nothing checking, and the sentence was
    // false**: the crate declared `ondin-export`, which reaches `ondin-render`
    // and so vello, wgpu and naga. That edge turned out to be a manifest line
    // no line of the crate's source used, so the rule is true now and this row
    // is what keeps it true.
    ("ondin-mcp", HEADLESS),
    // §3: "Only `ondin-app` may depend on egui/winit" — the two crates that sit
    // between core and the app and could quietly acquire chrome.
    ("ondin-render", NO_UI),
    ("ondin-export", NO_UI),
];

/// The packages named by `RULES`, for the vacuity guard below: a rule whose
/// subject is not in the tree at all has checked nothing, and would pass.
fn subjects() -> Vec<&'static str> {
    RULES.iter().map(|(subject, _)| *subject).collect()
}

/// ⚠️ **Flip-checked three ways, and the negative one is already standing.**
///
/// 1. Adding `"kurbo"` to the list above — a crate core really does depend on —
///    fails with it named, so the subtree walk finds what is under a subject.
/// 2. Restoring `ondin-export` to `ondin-mcp`'s manifest — the exact regression
///    this now guards — fails with **sixteen** crates named *(under ondin-mcp)*:
///    the whole vello/wgpu/naga closure. Two of the sixteen, `glutin_wgl_sys`
///    and `raw-window-handle`, are ones the original eight prefixes would have
///    missed, which is the widening above earning its keep on a real tree
///    rather than by argument.
/// 3. The negative: `"egui"` is *in the list already* while `egui` is really in
///    the workspace tree under `ondin-app`, and the test is green — so the walk
///    excludes what is not under a subject. Without that half the whole thing
///    could have been scanning the workspace forest and passing for the wrong
///    reason.
#[test]
fn every_layering_rule_in_section_3_holds_in_the_dependency_tree() {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_string());
    let output = Command::new(cargo)
        .args([
            "tree",
            // ⚠️ **`--workspace`, not `--package ondin-core`** — see the module
            // note. This is the resolution every gate that compiles core uses.
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

    // Every node under a subject node, at any depth, **once per rule**. The
    // whole forest is scanned rather than the first match, because
    // `--no-dedupe` repeats a package everywhere it is reached from and core
    // appears under every member that depends on it.
    //
    // ⚠️ **One pass per rule rather than one pass carrying a stack**, and the
    // reason is that the rows no longer share a list: `ondin-core` sits inside
    // `ondin-render`'s subtree, and core's forbidden list contains `vello`,
    // which render is *allowed*. A single walk that re-rooted on the inner
    // subject would report render's own vello against core; one that did not
    // would check core against render's looser list. Neither is the rule. The
    // tree is parsed once and walked four times, which costs nothing next to
    // the `cargo tree` call.
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
                // ⚠️ **The outermost occurrence wins**, deliberately: a subject
                // met *inside* its own subtree is already being scanned, and
                // re-rooting there would silently drop the outer one's
                // remaining siblings.
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

    // ⚠️ **Without this the test passes when the parse breaks.** A change to
    // cargo's `--prefix depth` format, a rename, or a `--workspace` that stopped
    // including core would leave `violations` empty and the assertion below
    // green — which is the shape of vacuity this project has found in its own
    // tests four times over.
    assert!(
        unseen.is_empty(),
        "the tree does not contain {unseen:?} at all, so nothing was checked for \
         {}:\n{tree}",
        unseen.join(", ")
    );
    // Deduped for the message only. `--no-dedupe` repeats every reachable node
    // once per path to it, so a single leak reported itself twenty-eight times
    // in the flip-check below — which buries the name it is there to give.
    violations.sort_unstable();
    violations.dedup();
    assert!(
        violations.is_empty(),
        "§3's layering rules are broken — a dependency tree pulls in forbidden \
         crate(s): {violations:?}\n\nThe rules checked were {RULES:?}\n\nFull \
         tree:\n{tree}"
    );
}

/// The one §3 rule that is about a **manifest** and not about a tree.
///
/// 🚨 **`ondin-app` and `ondin-export` both reach vello through
/// `ondin-render` by construction**, so there is no subtree anywhere in which
/// the absence of vello could be asserted — which is exactly why this rule went
/// untested while the other three did, and why a reader generalising
/// `RULES` would have concluded it was unenforceable and dropped it
/// (`[A2-L7-05]`: *"each row that cannot be made green today is a decision to
/// record, not a row to omit"*). It is enforceable; it is simply a different
/// question. **Who declares it**, not **who can reach it**.
///
/// ⚠️ **§3's `wgpu` half is deliberately not asserted here, because §3 no longer
/// claims it** (`[A2-L7-04]`). The rule read *"Only `ondin-render` may depend on
/// `vello`, `vello_cpu`, `wgpu`"* and was **unsatisfiable**: `ondin-render`'s own
/// seam takes `&Device`, `&Queue` and `&TextureView`, so the app cannot call it
/// without naming wgpu, and `ondin-app` really does declare `wgpu.workspace =
/// true`. A rule nobody can obey is a false statement rather than an aspiration.
///
/// ⚠️ **The members are read off the directory rather than listed**, so adding a
/// crate cannot quietly leave it unchecked — which is the failure mode
/// `RULES`' own table has and pays for with the `unseen` guard.
///
/// **Flip-checked**: adding `vello.workspace = true` to
/// `crates/ondin-export/Cargo.toml` fails with
/// `["ondin-export", "ondin-render"]` against `["ondin-render"]`, and the
/// positive control below — that `ondin-render` is found declaring it at all —
/// is what stops a broken parse passing as "nobody declares vello".
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

        // Section-aware, because `[dev-dependencies]` is out of scope for the
        // same reason the tree walk passes `--edges normal`: a test-only
        // dependency is not part of the shipped graph §3 is about.
        let mut in_deps = false;
        for line in text.lines() {
            let line = line.trim();
            if let Some(header) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                in_deps = header == "dependencies"
                    || (header.starts_with("target.") && header.ends_with(".dependencies"));
                continue;
            }
            if !in_deps || line.starts_with('#') || line.is_empty() {
                continue;
            }
            // `vello.workspace = true`, `vello_cpu = { … }` and
            // `ondin-core = { path = … }` all put the crate name first.
            let name = line
                .split(['.', '=', ' '])
                .next()
                .unwrap_or_default()
                .trim_matches('"');
            if name.starts_with("vello") {
                declarers.push(member.clone());
                break;
            }
        }
    }

    // ⚠️ **Without this, a parse that matched nothing would read as compliance**
    // — the shape of vacuity the sibling test's `unseen` guard exists for, and
    // the one this project has now found in its own tests five times over.
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
