//! No attribute is written twice on one item, anywhere in the workspace.
//!
//! `clippy::duplicated_attributes` covers this and is silent on any item inside
//! an `impl` block, inherent or trait — most of this workspace — so an
//! `#[allow(clippy::too_many_arguments)]` written twice on consecutive lines
//! stood with every gate green. This scan reads the source instead.

use std::path::{Path, PathBuf};

/// The attribute lines repeated within one item's run of attributes in `src`, as
/// `(line number, attribute)`. A run is the attribute lines above an item, doc
/// and line comments between them included; anything else ends it. Only an
/// attribute that closes on its own line is compared, so two multi-line
/// attributes sharing a first line are not mistaken for one.
fn duplicated(src: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut run: Vec<&str> = Vec::new();
    for (i, line) in src.lines().enumerate() {
        let t = line.trim();
        if t.starts_with("#[") && t.ends_with(']') {
            if run.contains(&t) {
                out.push((i + 1, t.to_string()));
            }
            run.push(t);
        } else if !(t.starts_with("//") || t.starts_with("#[")) {
            run.clear();
        }
    }
    out
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("a readable directory") {
        let path = entry.expect("a readable entry").path();
        if path.is_dir() {
            if path.file_name().is_some_and(|n| n != "target") {
                rust_files(&path, out);
            }
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// The scan's own fixtures: what it must flag and what it must not.
#[test]
fn the_scan_flags_a_repeat_and_nothing_else() {
    let must = [
        "#[allow(x)]\n#[allow(x)]\nfn f() {}",
        "    #[allow(x)]\n    /// doc\n    #[inline]\n    #[allow(x)]\n    fn f() {}",
        "#[test]\n// note\n#[test]\nfn t() {}",
    ];
    for src in must {
        assert_eq!(duplicated(src).len(), 1, "must flag:\n{src}");
    }
    let must_not = [
        "#[allow(x)]\nfn f() {}\n#[allow(x)]\nfn g() {}",
        "#[allow(x)]\n#[allow(y)]\nfn f() {}",
        "#[cfg_attr(\n    a,\n)]\n#[cfg_attr(\n    b,\n)]\nfn f() {}",
        "#[test]\nfn a() {}\n\n#[test]\nfn b() {}",
    ];
    for src in must_not {
        assert_eq!(duplicated(src), [], "must not flag:\n{src}");
    }
}

/// Every `.rs` file under `crates/` is free of a repeated attribute.
/// Flip: an `#[inline]` written twice above a method in an `impl` → fails
/// naming the file and line.
#[test]
fn no_item_in_the_workspace_repeats_an_attribute() {
    let crates = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("crates/ondin-core has a parent");
    let mut files = Vec::new();
    rust_files(crates, &mut files);
    assert!(
        files.len() > 50,
        "only {} files found under {}; the scan is not reading the workspace",
        files.len(),
        crates.display()
    );
    let mut found = Vec::new();
    let mut attributes = 0;
    for f in &files {
        let src = std::fs::read_to_string(f).expect("a readable source file");
        attributes += src
            .lines()
            .filter(|l| l.trim().starts_with("#[test]"))
            .count();
        for (line, attr) in duplicated(&src) {
            found.push(format!("{}:{line}: {attr}", f.display()));
        }
    }
    assert!(
        attributes > 1000,
        "only {attributes} #[test] attributes seen"
    );
    assert!(
        found.is_empty(),
        "attributes written twice on one item:\n{}",
        found.join("\n")
    );
}
