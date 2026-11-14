//! A reply's markdown: parsed into blocks, drawn into the panel. Only what a
//! short answer uses: paragraphs, headings (drawn as bold lines), lists,
//! tables, code blocks with a language bar, and the ` ```options ` block, which
//! draws as buttons. A layer id in prose (`831ddff3fec146a3:573`) draws as a
//! link naming the layer when the document has it.

use crate::theme::{self, color, icon};
use crate::ui;

/// A run of inline text.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Span {
    Text(String),
    Strong(String),
    Em(String),
    Code(String),
}

/// One block of a reply.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Block {
    Para(Vec<Span>),
    Heading(Vec<Span>),
    List {
        ordered: bool,
        /// An ordered list's first number, as written.
        start: u32,
        items: Vec<Vec<Span>>,
    },
    Code {
        lang: String,
        text: String,
    },
    Options(Vec<String>),
    Table {
        head: Vec<String>,
        rows: Vec<Vec<String>>,
    },
    Rule,
}

/// Parse a reply. An unclosed fence, as a streaming reply has, runs to the end.
pub(crate) fn parse(src: &str) -> Vec<Block> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    let mut para: Vec<&str> = Vec::new();
    let flush = |para: &mut Vec<&str>, out: &mut Vec<Block>| {
        if !para.is_empty() {
            out.push(Block::Para(inline(&para.join(" "))));
            para.clear();
        }
    };
    let mut i = 0;
    while i < lines.len() {
        let line = lines[i];
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("```") {
            flush(&mut para, &mut out);
            let lang = rest.trim().to_lowercase();
            let mut body = Vec::new();
            i += 1;
            while i < lines.len() && !lines[i].trim_start().starts_with("```") {
                body.push(lines[i]);
                i += 1;
            }
            i += 1;
            if lang == "options" {
                let opts: Vec<String> = body
                    .iter()
                    .map(|l| strip_marker(l.trim()).to_owned())
                    .filter(|l| !l.is_empty())
                    .collect();
                out.push(Block::Options(opts));
            } else {
                out.push(Block::Code {
                    lang,
                    text: body.join("\n"),
                });
            }
            continue;
        }
        if t.is_empty() {
            flush(&mut para, &mut out);
            i += 1;
            continue;
        }
        if t.starts_with('|') && lines.get(i + 1).is_some_and(|n| is_table_rule(n)) {
            flush(&mut para, &mut out);
            let head = cells(t);
            let mut rows = Vec::new();
            i += 2;
            while i < lines.len() && lines[i].trim().starts_with('|') {
                rows.push(cells(lines[i].trim()));
                i += 1;
            }
            out.push(Block::Table { head, rows });
            continue;
        }
        if matches!(t, "---" | "***" | "___") {
            flush(&mut para, &mut out);
            out.push(Block::Rule);
            i += 1;
            continue;
        }
        if t.starts_with('#') {
            let text = t.trim_start_matches('#');
            if text.starts_with(' ') {
                flush(&mut para, &mut out);
                out.push(Block::Heading(inline(text.trim())));
                i += 1;
                continue;
            }
        }
        if let Some(ordered) = list_marker(t) {
            flush(&mut para, &mut out);
            let start = t
                .chars()
                .take_while(char::is_ascii_digit)
                .collect::<String>()
                .parse()
                .unwrap_or(1);
            let mut items: Vec<String> = Vec::new();
            while i < lines.len() {
                let l = lines[i].trim();
                // A blank line between two items keeps one (loose) list.
                let next_item = || {
                    lines[i + 1..]
                        .iter()
                        .find(|l| !l.trim().is_empty())
                        .is_some_and(|l| list_marker(l.trim()) == Some(ordered))
                };
                if l.is_empty() && !items.is_empty() && next_item() {
                    i += 1;
                    continue;
                }
                match list_marker(l) {
                    Some(o) if o == ordered => items.push(strip_marker(l).to_owned()),
                    // A wrapped item's next line, indented under it.
                    None if !l.is_empty() && lines[i].starts_with("  ") && !items.is_empty() => {
                        let last = items.last_mut().expect("checked");
                        last.push(' ');
                        last.push_str(l);
                    }
                    _ => break,
                }
                i += 1;
            }
            out.push(Block::List {
                ordered,
                start,
                items: items.iter().map(|s| inline(s)).collect(),
            });
            continue;
        }
        para.push(t);
        i += 1;
    }
    flush(&mut para, &mut out);
    out
}

/// `Some(ordered)` when the line opens a list item.
fn list_marker(t: &str) -> Option<bool> {
    if t.starts_with("- ") || t.starts_with("* ") || t.starts_with("+ ") {
        return Some(false);
    }
    let digits = t.chars().take_while(char::is_ascii_digit).count();
    (digits > 0 && t[digits..].starts_with(". ")).then_some(true)
}

/// The line without its list marker.
fn strip_marker(t: &str) -> &str {
    match list_marker(t) {
        Some(false) => t[2..].trim_start(),
        Some(true) => {
            let digits = t.chars().take_while(char::is_ascii_digit).count();
            t[digits + 2..].trim_start()
        }
        None => t,
    }
}

fn is_table_rule(line: &str) -> bool {
    let t = line.trim();
    t.starts_with('|') && t.contains('-') && t.chars().all(|c| matches!(c, '|' | '-' | ':' | ' '))
}

fn cells(row: &str) -> Vec<String> {
    let inner = row.trim().trim_start_matches('|').trim_end_matches('|');
    inner
        .split('|')
        .map(|c| c.trim().replace("**", ""))
        .collect()
}

/// Inline spans: `**strong**`, `*em*`/`_em_`, `` `code` ``. An unmatched marker
/// is kept as text.
pub(crate) fn inline(src: &str) -> Vec<Span> {
    let mut out = Vec::new();
    let mut text = String::new();
    let mut rest = src;
    let push_text = |text: &mut String, out: &mut Vec<Span>| {
        if !text.is_empty() {
            out.push(Span::Text(std::mem::take(text)));
        }
    };
    while let Some(c) = rest.chars().next() {
        let (marker, make): (&str, fn(String) -> Span) = match c {
            '`' => ("`", Span::Code),
            '*' if rest.starts_with("**") => ("**", Span::Strong),
            '_' if rest.starts_with("__") => ("__", Span::Strong),
            '*' => ("*", Span::Em),
            '_' if text.chars().last().is_none_or(|p| !p.is_alphanumeric()) => ("_", Span::Em),
            _ => {
                text.push(c);
                rest = &rest[c.len_utf8()..];
                continue;
            }
        };
        let after = &rest[marker.len()..];
        match after.find(marker) {
            Some(end) if end > 0 => {
                push_text(&mut text, &mut out);
                out.push(make(after[..end].to_owned()));
                rest = &after[end + marker.len()..];
            }
            _ => {
                text.push_str(marker);
                rest = after;
            }
        }
    }
    push_text(&mut text, &mut out);
    out
}

/// Split text at layer ids (an actor in hex, a colon, a sequence number:
/// `831ddff3fec146a3:573`, bounded by non-word characters): the pieces in
/// order, `true` for an id. A candidate the document does not hold, such as a
/// time, stays prose when drawn.
pub(crate) fn ids_in(text: &str) -> Vec<(&str, bool)> {
    let b = text.as_bytes();
    let mut out = Vec::new();
    let mut start = 0;
    let mut i = 0;
    while i < b.len() {
        let word_before = i > 0 && (b[i - 1].is_ascii_alphanumeric() || b[i - 1] == b':');
        if b[i].is_ascii_hexdigit() && !word_before {
            let mut j = i;
            while j < b.len() && b[j].is_ascii_hexdigit() {
                j += 1;
            }
            if j < b.len() && b[j] == b':' && b.get(j + 1).is_some_and(u8::is_ascii_digit) {
                let mut k = j + 1;
                while k < b.len() && b[k].is_ascii_digit() {
                    k += 1;
                }
                let word_after = k < b.len() && (b[k].is_ascii_alphanumeric() || b[k] == b':');
                if !word_after {
                    if start < i {
                        out.push((&text[start..i], false));
                    }
                    out.push((&text[i..k], true));
                    start = k;
                    i = k;
                    continue;
                }
            }
            i = j;
            continue;
        }
        i += 1;
    }
    if start < text.len() {
        out.push((&text[start..], false));
    }
    out
}

/// What a reply's drawing asks of the panel.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Act {
    /// A chip was clicked: select the layer and zoom to it.
    Select(String),
    /// A chip is hovered: outline the layer.
    Hover(String),
    /// An option was clicked: send its line as the next question.
    Choose(String),
}

/// What drawing a reply needs to know.
pub(crate) struct Look<'a> {
    /// A layer's name by id, `None` when the document has no such layer.
    pub layer: &'a dyn Fn(&str) -> Option<String>,
    /// Whether this reply's options can be clicked: only the last turn's.
    pub options_live: bool,
    /// Prose ink.
    pub ink: egui::Color32,
}

/// The gap between two blocks.
const BLOCK_GAP: f32 = 7.0;

/// A code block's language bar.
const CODE_BAR_H: f32 = 24.0;

/// Draw a reply's blocks.
pub(crate) fn show(ui: &mut egui::Ui, blocks: &[Block], look: &Look<'_>, acts: &mut Vec<Act>) {
    ui.spacing_mut().item_spacing.y = BLOCK_GAP;
    for (n, b) in blocks.iter().enumerate() {
        let salt = ui.id().with(("md", n));
        match b {
            Block::Para(spans) => spans_row(ui, spans, look, acts, false),
            Block::Heading(spans) => spans_row(ui, spans, look, acts, true),
            Block::List {
                ordered,
                start,
                items,
            } => {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    for (k, item) in items.iter().enumerate() {
                        ui.horizontal_top(|ui| {
                            let mark = match ordered {
                                true => format!("{}.", u64::from(*start) + k as u64),
                                false => "•".to_owned(),
                            };
                            ui.add_sized(
                                egui::vec2(14.0, 0.0),
                                egui::Label::new(
                                    egui::RichText::new(mark)
                                        .size(ui::CONTROL_PT)
                                        .color(theme::text::dim()),
                                ),
                            );
                            ui.vertical(|ui| spans_row(ui, item, look, acts, false));
                        });
                    }
                });
            }
            Block::Code { lang, text } => code_block(ui, salt, lang, text),
            Block::Options(opts) => {
                ui.vertical(|ui| {
                    ui.spacing_mut().item_spacing.y = 4.0;
                    for o in opts {
                        let w = ui.available_width();
                        let state = match look.options_live {
                            true => ui::FieldButton::Off,
                            false => ui::FieldButton::Disabled,
                        };
                        let resp =
                            ui::label_button(ui, o, egui::vec2(w, ui::CONTROL_H - 4.0), state);
                        if look.options_live && resp.clicked() {
                            acts.push(Act::Choose(o.clone()));
                        }
                    }
                });
            }
            Block::Table { head, rows } => table(ui, salt, head, rows, look),
            Block::Rule => {
                ui::card_rule(ui);
            }
        }
    }
}

/// A paragraph's spans, wrapped, with each layer id the document holds drawn
/// as a link naming the layer.
fn spans_row(
    ui: &mut egui::Ui,
    spans: &[Span],
    look: &Look<'_>,
    acts: &mut Vec<Act>,
    strong: bool,
) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        for s in spans {
            let (text, base): (&str, fn(String) -> egui::RichText) = match s {
                Span::Text(t) => (t, egui::RichText::new),
                Span::Strong(t) => (t, |t| ui::bold(egui::RichText::new(t))),
                Span::Em(t) => (t, |t| egui::RichText::new(t).italics()),
                Span::Code(t) => (t, |t| egui::RichText::new(t).monospace()),
            };
            for (piece, is_id) in ids_in(text) {
                if is_id && let Some(name) = (look.layer)(piece) {
                    let resp = ui::word_link_pt(ui, &name, false, ui::CONTROL_PT)
                        .on_hover_text("Select and zoom to it");
                    if resp.hovered() {
                        acts.push(Act::Hover(piece.to_owned()));
                    }
                    if resp.clicked() {
                        acts.push(Act::Select(piece.to_owned()));
                    }
                    continue;
                }
                let strong_ink = strong || matches!(s, Span::Strong(_));
                let ink = if strong_ink { color::text() } else { look.ink };
                let mut rt = base(piece.to_owned()).size(ui::CONTROL_PT).color(ink);
                if strong {
                    rt = ui::bold(rt);
                }
                ui.label(rt);
            }
        }
    });
}

/// A code block: a language bar with *Copy*, then the text in mono.
fn code_block(ui: &mut egui::Ui, salt: egui::Id, lang: &str, text: &str) {
    egui::Frame::NONE
        .fill(color::field())
        .stroke(egui::Stroke::new(1.0, color::field_border()))
        .corner_radius(egui::CornerRadius::same(6))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            let w = ui.available_width();
            let (bar, _) = ui.allocate_exact_size(egui::vec2(w, CODE_BAR_H), egui::Sense::hover());
            let p = ui.painter();
            p.text(
                egui::pos2(bar.left() + 10.0, bar.center().y),
                egui::Align2::LEFT_CENTER,
                if lang.is_empty() { "code" } else { lang },
                egui::FontId::proportional(ui::CAPTION_PT),
                theme::text::faint(),
            );
            p.hline(
                bar.x_range(),
                bar.bottom() - 0.5,
                egui::Stroke::new(1.0, color::divider()),
            );
            let copy = egui::Rect::from_center_size(
                egui::pos2(bar.right() - 8.0 - 9.0, bar.center().y),
                egui::vec2(18.0, 18.0),
            );
            let mut child = ui.new_child(egui::UiBuilder::new().max_rect(copy).id_salt(salt));
            if ui::icon_button(&mut child, icon::COPY, 18.0, 13.0, false, true)
                .on_hover_text("Copy")
                .clicked()
            {
                ui.ctx().copy_text(text.to_owned());
            }
            egui::Frame::NONE
                .inner_margin(egui::Margin::symmetric(10, 8))
                .show(ui, |ui| {
                    egui::ScrollArea::horizontal()
                        .id_salt(salt.with("code"))
                        .show(ui, |ui| {
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(text)
                                        .monospace()
                                        .size(ui::SMALL_PT - 0.5)
                                        .color(theme::text::muted()),
                                )
                                .extend(),
                            );
                        });
                });
        });
}

/// A table: a header row in dim ink, then the rows, hairlines between.
fn table(
    ui: &mut egui::Ui,
    salt: egui::Id,
    head: &[String],
    rows: &[Vec<String>],
    look: &Look<'_>,
) {
    egui::ScrollArea::horizontal().id_salt(salt).show(ui, |ui| {
        egui::Grid::new(salt.with("grid"))
            .spacing(egui::vec2(16.0, 5.0))
            .striped(false)
            .show(ui, |ui| {
                for h in head {
                    ui.label(
                        egui::RichText::new(h)
                            .size(ui::SMALL_PT)
                            .color(theme::text::dim()),
                    );
                }
                ui.end_row();
                for r in rows {
                    for c in r {
                        ui.label(egui::RichText::new(c).size(ui::SMALL_PT).color(look.ink));
                    }
                    ui.end_row();
                }
            });
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Blocks come apart at blank lines, fences, tables and lists.
    /// flip: `parse` not flushing at a fence → fails at the block count.
    #[test]
    fn a_reply_parses_into_its_blocks() {
        let src = "Three styles:\n\n| Style | Size |\n|---|---|\n| Caption | 12 |\n\n- one\n- two\n\n```css\na { }\n```\nend";
        let b = parse(src);
        assert_eq!(b.len(), 5, "{b:?}");
        assert!(matches!(&b[1], Block::Table { rows, .. } if rows.len() == 1));
        assert!(matches!(&b[2], Block::List { ordered: false, items, .. } if items.len() == 2));
        assert!(matches!(&b[3], Block::Code { lang, .. } if lang == "css"));
    }

    /// An options fence becomes its lines, list markers stripped.
    /// flip: `parse` treating `options` as code → fails at the match.
    #[test]
    fn an_options_block_is_its_lines() {
        let b = parse("Which way?\n```options\n- Align both on the baseline\n2. Centre both\n```");
        assert_eq!(
            b[1],
            Block::Options(vec![
                "Align both on the baseline".into(),
                "Centre both".into()
            ])
        );
    }

    /// A streaming reply's open fence runs to the end.
    #[test]
    fn an_unclosed_fence_runs_to_the_end() {
        let b = parse("```rust\nfn a() {}");
        assert_eq!(
            b,
            vec![Block::Code {
                lang: "rust".into(),
                text: "fn a() {}".into()
            }]
        );
    }

    /// Inline markers, and an unmatched one kept as text.
    #[test]
    fn inline_spans() {
        assert_eq!(
            inline("Set **Fill** on `w` *now* 2 * 3"),
            vec![
                Span::Text("Set ".into()),
                Span::Strong("Fill".into()),
                Span::Text(" on ".into()),
                Span::Code("w".into()),
                Span::Text(" ".into()),
                Span::Em("now".into()),
                Span::Text(" 2 * 3".into()),
            ]
        );
    }

    /// Layer ids split out of prose; a time or a ratio inside a word does not.
    /// flip: dropping the `word_after` check → fails at "1:2:3".
    #[test]
    fn layer_ids_split_out_of_prose() {
        assert_eq!(
            ids_in("Set 4:31 to Fill, then 12:7."),
            vec![
                ("Set ", false),
                ("4:31", true),
                (" to Fill, then ", false),
                ("12:7", true),
                (".", false)
            ]
        );
        assert_eq!(ids_in("ratio 1:2:3 x"), vec![("ratio 1:2:3 x", false)]);
        assert_eq!(ids_in("v4:31"), vec![("v4:31", false)]);
        // flip: `ids_in` scanning decimal digits for the actor → fails here.
        assert_eq!(
            ids_in("fills 831ddff3fec146a3:771 blue"),
            vec![
                ("fills ", false),
                ("831ddff3fec146a3:771", true),
                (" blue", false)
            ],
            "the actor is hex"
        );
    }

    /// A loose ordered list, its items parted by blank lines, is one list, and
    /// a list that starts at 3 counts from 3.
    /// flip: `parse` without its blank-line continuation → fails at "one list".
    #[test]
    fn a_loose_list_is_one_list_numbered_as_written() {
        let b = parse("1. Pick\n\n2. Place\n\n3. Check");
        assert_eq!(b.len(), 1, "one list: {b:?}");
        assert!(
            matches!(&b[0], Block::List { ordered: true, start: 1, items } if items.len() == 3)
        );
        let b = parse("3. Third\n4. Fourth");
        assert!(matches!(&b[0], Block::List { start: 3, .. }), "{b:?}");
        // A blank line before a paragraph still ends the list.
        let b = parse("- a\n\nAfter");
        assert_eq!(b.len(), 2, "{b:?}");
    }
}
