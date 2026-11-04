//! Chrome text takes its size from `ui`'s type scale and its ink from
//! `theme::text`'s tiers, not from literals.
//!
//! Reads the source, outside `ui.rs` and `theme.rs` (where the scale and the
//! tiers are defined) and outside each file's `#[cfg(test)]` tail. Two holes, by
//! design: a size held in a local `const` is not read (a role with a reason of
//! its own is spelled that way, from the scale), and an off-tier ink is caught
//! only within a few lines of a text call. A line that must keep its ink says so
//! with a `text-ink:` comment giving the reason.

use std::path::{Path, PathBuf};

/// A float or integer literal and nothing else, as a call's whole argument.
fn literal(arg: &str) -> bool {
    let a = arg.trim();
    !a.is_empty()
        && a.chars()
            .all(|c| c.is_ascii_digit() || c == '.' || c == '_')
        && a.chars().next().is_some_and(|c| c.is_ascii_digit())
}

/// Each `name(` call in `line` whose argument up to the first `)` or `,` is a
/// literal.
fn literal_calls(line: &str, name: &str) -> usize {
    let mut n = 0;
    let mut rest = line;
    while let Some(i) = rest.find(name) {
        let after = &rest[i + name.len()..];
        let end = after.find([')', ',']).unwrap_or(after.len());
        if literal(&after[..end]) {
            n += 1;
        }
        rest = after;
    }
    n
}

/// Whether `line` holds a `text_a(<literal>)`.
fn text_alpha(line: &str) -> bool {
    literal_calls(line, "text_a(") > 0
}

/// What reads as a call that sets text, for the ink scan.
const TEXT_CALLS: [&str; 7] = [
    ".text(",
    "RichText::new",
    "TextFormat",
    "layout_no_wrap",
    "LayoutJob",
    "simple_singleline",
    "text_color",
];

/// The lines of `src` that build outside tests, as `(line number, text)`: each
/// `#[cfg(test)]` item is skipped to its closing brace, or to its `;` when it
/// has no body. Braces are counted naively, which a format string's balanced
/// `{}` does not disturb.
fn live(src: &str) -> Vec<(usize, &str)> {
    let mut out = Vec::new();
    let mut skip: Option<(i32, bool)> = None;
    for (i, line) in src.lines().enumerate() {
        if skip.is_none() && line.trim_start().starts_with("#[cfg(test)]") {
            skip = Some((0, false));
            continue;
        }
        let Some((depth, opened)) = skip.as_mut() else {
            out.push((i + 1, line));
            continue;
        };
        let code = line.split("//").next().unwrap_or("");
        for c in code.chars() {
            match c {
                '{' => {
                    *depth += 1;
                    *opened = true;
                }
                '}' => *depth -= 1,
                _ => {}
            }
        }
        let attribute = line.trim_start().starts_with("#[");
        if (*opened && *depth <= 0) || (!*opened && !attribute && code.trim_end().ends_with(';')) {
            skip = None;
        }
    }
    out
}

/// `(line, what)` for every literal size in `src`.
fn sizes(src: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    for (n, line) in live(src) {
        let code = line.split("//").next().unwrap_or("");
        let hits = ["proportional(", "monospace(", ".size(", "FontId::new("]
            .iter()
            .map(|c| literal_calls(code, c))
            .sum::<usize>();
        if hits > 0 {
            out.push((n, line.trim().to_string()));
        }
    }
    out
}

/// `(line, what)` for every literal alpha set on text in `src`: within the
/// statement of a text call, a tier spelled as its alpha or an alpha off them.
fn inks(src: &str) -> Vec<(usize, String)> {
    let lines = live(src);
    let mut out = Vec::new();
    for (i, &(n, line)) in lines.iter().enumerate() {
        if line.contains("text-ink:") {
            continue;
        }
        let code = line.split("//").next().unwrap_or("");
        if !text_alpha(code) {
            continue;
        }
        // Back to the statement before this one, at most six lines. Any alpha on
        // text is a tier's or exempt; a ground or a hairline keeps its own.
        for j in (i.saturating_sub(6)..=i).rev() {
            let l = lines[j].1;
            if j != i && l.trim_end().ends_with(';') {
                break;
            }
            if TEXT_CALLS.iter().any(|c| l.contains(c)) {
                out.push((n, line.trim().to_string()));
                break;
            }
        }
    }
    out
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("a readable directory") {
        let path = entry.expect("a readable entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Every source file the scale binds, with its text.
fn sources() -> Vec<(PathBuf, String)> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&src, &mut files);
    files
        .into_iter()
        .filter(|p| {
            let rel = p.strip_prefix(&src).unwrap();
            rel != Path::new("ui.rs") && rel != Path::new("theme.rs")
        })
        .map(|p| {
            let text = std::fs::read_to_string(&p).expect("a readable source file");
            (p, text)
        })
        .collect()
}

fn report(found: Vec<(PathBuf, usize, String)>) -> String {
    found
        .iter()
        .map(|(p, n, l)| format!("{}:{n}: {l}", p.display()))
        .collect::<Vec<_>>()
        .join("\n")
}

/// The scans' own fixtures: what each must flag and what it must not.
#[test]
fn the_scans_flag_literals_and_nothing_else() {
    for src in [
        "let f = egui::FontId::proportional(11.0);",
        "RichText::new(x).size(12.5)",
        "egui::FontId::monospace(10.0),",
    ] {
        assert_eq!(sizes(src).len(), 1, "must flag:\n{src}");
    }
    for src in [
        "let f = egui::FontId::proportional(ui::SMALL_PT);",
        "RichText::new(x).size(ui::CONTROL_PT * zoom)",
        "// proportional(11.0) in a comment",
        "let s = rect.size();",
        "#[cfg(test)]\nmod tests { fn f() { proportional(11.0); } }",
        "#[cfg(test)]\nuse x::y;\n#[cfg(test)]\n#[allow(z)]\nfn f() {\n    proportional(11.0);\n}",
    ] {
        assert!(sizes(src).is_empty(), "must not flag:\n{src}");
    }
    // Code after a mid-file test item is read again.
    let after = "#[cfg(test)]\nfn probe() {\n    let _ = 1;\n}\nfn live() { proportional(11.0); }";
    assert_eq!(
        sizes(after),
        [(5, "fn live() { proportional(11.0); }".to_string())]
    );
    for src in [
        "p.text(at, align, s, font, theme::color::text_a(97));",
        "p.text(at, align, s, font,\n    theme::color::text_a(140));",
        "RichText::new(s).color(text_a(87))",
    ] {
        assert_eq!(inks(src).len(), 1, "must flag:\n{src}");
    }
    for src in [
        "p.rect_filled(r, 4, theme::color::text_a(20));",
        "let hairline = theme::color::text_a(102);",
        "p.text(at, align, s, font, ink);\nlet g = text_a(12);",
        "p.text(a, b, c, d,\n    text_a(140)); // text-ink: the chip's own ink",
    ] {
        assert!(inks(src).is_empty(), "must not flag:\n{src}");
    }
}

/// No chrome text sets a literal size.
#[test]
fn text_sizes_come_from_the_scale() {
    let found: Vec<_> = sources()
        .into_iter()
        .flat_map(|(p, s)| sizes(&s).into_iter().map(move |(n, l)| (p.clone(), n, l)))
        .collect();
    assert!(
        found.is_empty(),
        "literal text sizes; use a `ui::*_PT` role:\n{}",
        report(found)
    );
}

/// No chrome text sets an ink off the tiers, or spells a tier as its alpha.
#[test]
fn text_inks_come_from_the_tiers() {
    let found: Vec<_> = sources()
        .into_iter()
        .flat_map(|(p, s)| inks(&s).into_iter().map(move |(n, l)| (p.clone(), n, l)))
        .collect();
    assert!(
        found.is_empty(),
        "text inks off `theme::text`'s tiers:\n{}",
        report(found)
    );
}
