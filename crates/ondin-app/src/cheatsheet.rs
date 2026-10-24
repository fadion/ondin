//! The shortcut cheatsheet: the whole keymap on one sheet, a modal on either
//! screen (`Ctrl+Shift+/`).
//!
//! Its rows are the registry's (`crate::menu::Item`) that carry a chord, with the
//! label and spelling a menu row uses, so the sheet and the menus cannot disagree.
//! Two lists are kept here by hand because no registry row exists for them: the
//! mouse gestures, which are not actions, and the library's own keys, which
//! `panels::dashboard::dashboard_keys` reads outside `input::resolve`.

use crate::app::OndinApp;
use crate::input::{TextChord, ViewSwitch};
use crate::menu::Item;
use crate::theme::{self, color, icon};
use crate::tools::Tool;
use eframe::egui;
use ondin_core::{BoolOp, build};

/// The card's outer width and its greatest height, both shrunk to the window
/// less [`MARGIN`] a side.
const W: f32 = 960.0;
const MAX_H: f32 = 640.0;
const MARGIN: f32 = crate::settings::CARD_EDGE;
/// The narrowest a column gets before the sheet deals into one column fewer.
const MIN_COLUMN_W: f32 = 220.0;
const ROW_H: f32 = 24.0;
const TITLE_H: f32 = 26.0;
/// The air under a section.
const SECTION_GAP: f32 = 18.0;
const COLUMN_GAP: f32 = 32.0;
const FILTER_W: f32 = 280.0;
/// The palette's, so the two footers read alike.
const FOOTER_H: f32 = crate::palette::FOOTER_H;

/// Which screen the sheet was opened from; that screen's section comes first.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Opened {
    Editor,
    Library,
}

/// The open sheet.
pub(crate) struct Cheatsheet {
    filter: String,
    /// The section that leads: the library's, the text keys', or none.
    first: Lead,
    /// The frame that opened it, whose chord press must not also close it.
    just_opened: bool,
    /// Every modifier has been let go since it opened. Until then *hold to look*
    /// stays off, or the chord's own Ctrl+Shift would filter the sheet as it opens.
    released: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Lead {
    Library,
    Text,
    None,
}

/// One row: what it does and every spelling of its chord.
struct Row {
    /// The registry row it shows; `None` for a hand-kept one.
    #[cfg_attr(
        not(test),
        expect(dead_code, reason = "read by the sheet's chord test")
    )]
    item: Option<Item>,
    label: String,
    /// Each alternative as its keys: `[["Ctrl", "R"], ["F2"]]`.
    chords: Vec<Vec<String>>,
}

struct Section {
    title: &'static str,
    rows: Vec<Row>,
}

/// A registry row as the sheet shows it.
fn item_row(item: Item) -> Option<Row> {
    let spec = item.spec();
    let accel = spec.accel?;
    let label = match item {
        Item::ToggleHidden => "Hide / Show",
        Item::ToggleLocked => "Lock / Unlock",
        Item::PasteHere => "Paste",
        _ => spec.label,
    };
    let mut chords = vec![keys_of(accel)];
    if let Some(alt) = item.alt_accel() {
        chords.push(keys_of(alt));
    }
    Some(Row {
        item: Some(item),
        label: label.to_string(),
        chords,
    })
}

/// A hand-kept row.
fn hand_row(label: &str, chords: &[&str]) -> Row {
    Row {
        item: None,
        label: label.to_string(),
        chords: chords.iter().map(|c| keys_of(c)).collect(),
    }
}

/// `Ctrl+Shift+]` as its keys. A trailing `+` is never a key here: the registry
/// spells zoom in as `Ctrl+=`.
fn keys_of(accel: &str) -> Vec<String> {
    accel.split('+').map(str::to_string).collect()
}

fn items_section(title: &'static str, items: &[Item]) -> Section {
    let rows: Vec<Row> = items.iter().filter_map(|i| item_row(*i)).collect();
    // An item that lost its chord would drop off the sheet unseen; the chord
    // test builds every section, so this is where it goes red.
    debug_assert_eq!(
        rows.len(),
        items.len(),
        "{title}: an item on the sheet has no chord"
    );
    Section { title, rows }
}

/// Every section, in `docs/shortcuts.md`'s order, with `lead` moved first.
fn sections(lead: Lead) -> Vec<Section> {
    use build::{Axis, Edge, ZMove};
    let tools: Vec<Item> = [
        Tool::Select,
        Tool::Hand,
        Tool::Scale,
        Tool::Frame,
        Tool::Rect,
        Tool::Ellipse,
        Tool::Polygon,
        Tool::Star,
        Tool::Line,
        Tool::Pen,
        Tool::Node,
        Tool::Text,
        Tool::Eyedropper,
    ]
    .into_iter()
    .map(Item::Tool)
    .chain([Item::PlaceImage])
    .collect();
    let mut layer = items_section(
        "Layer",
        &[Item::ToggleHidden, Item::ToggleLocked, Item::Rename],
    );
    // Opacity digits are a value, not a verb; no registry row names them.
    layer.rows.push(hand_row("Opacity 10–100%", &["0–9"]));
    let view: Vec<Item> = [
        ViewSwitch::Rulers,
        ViewSwitch::Guides,
        ViewSwitch::GuideLock,
        ViewSwitch::Grid,
        ViewSwitch::Layers,
        ViewSwitch::Toolbar,
        ViewSwitch::Focus,
        ViewSwitch::SnapShapes,
        ViewSwitch::SnapGuides,
        ViewSwitch::SnapGrid,
    ]
    .into_iter()
    .map(Item::ViewSwitch)
    .collect();
    let text: Vec<Item> = [
        TextChord::Bold,
        TextChord::Italic,
        TextChord::Underline,
        TextChord::Size(1),
        TextChord::Size(-1),
        TextChord::Tracking(1),
        TextChord::Tracking(-1),
        TextChord::Leading(1),
        TextChord::Leading(-1),
        TextChord::Align(ondin_core::TextAlign::Start),
        TextChord::Align(ondin_core::TextAlign::Center),
        TextChord::Align(ondin_core::TextAlign::End),
        TextChord::Align(ondin_core::TextAlign::Justify),
    ]
    .into_iter()
    .map(Item::TextChord)
    .collect();

    let mut out = vec![
        items_section("Tools", &tools),
        items_section(
            "Zoom",
            &[
                Item::ZoomIn,
                Item::ZoomOut,
                Item::ZoomReset,
                Item::ZoomFit,
                Item::ZoomSelection,
            ],
        ),
        items_section(
            "Edit",
            &[
                Item::Undo,
                Item::Redo,
                Item::Cut,
                Item::Copy,
                Item::PasteHere,
                Item::PasteInPlace,
                Item::CopyProperties,
                Item::PasteProperties,
                Item::Duplicate,
                Item::Delete,
                Item::SelectAll,
            ],
        ),
        items_section(
            "Structure",
            &[
                Item::Group,
                Item::Ungroup,
                Item::FrameSelection,
                Item::Mask,
                Item::Flatten,
                Item::Boolean(BoolOp::Union),
                Item::Boolean(BoolOp::Subtract),
                Item::Boolean(BoolOp::Intersect),
                Item::Boolean(BoolOp::Exclude),
                Item::ReverseSubpaths,
            ],
        ),
        items_section(
            "Arrange",
            &[
                Item::Restack(ZMove::Front),
                Item::Restack(ZMove::Forward),
                Item::Restack(ZMove::Backward),
                Item::Restack(ZMove::Back),
                Item::Align(Axis::X, Edge::Min),
                Item::Align(Axis::X, Edge::Mid),
                Item::Align(Axis::X, Edge::Max),
                Item::Align(Axis::Y, Edge::Min),
                Item::Align(Axis::Y, Edge::Mid),
                Item::Align(Axis::Y, Edge::Max),
                Item::Distribute(Axis::X),
                Item::Distribute(Axis::Y),
                Item::Flip(Axis::X),
                Item::Flip(Axis::Y),
            ],
        ),
        layer,
        items_section("View", &view),
        items_section(
            "Components and kits",
            &[
                Item::CreateComponent,
                Item::DetachInstance,
                Item::InsertPicker,
                Item::VariablesBrowser,
                Item::PublishKit,
            ],
        ),
        items_section(
            "File",
            &[
                Item::Save,
                Item::NewDocument,
                Item::Library,
                Item::CloseDocument,
                Item::ExportAll,
                Item::Present,
                Item::Settings,
                Item::Palette,
                Item::Cheatsheet,
            ],
        ),
        items_section("Text editing", &text),
        Section {
            title: "Mouse",
            rows: vec![
                hand_row("Pan", &["Space+drag"]),
                hand_row("Duplicate while moving", &["Alt+drag"]),
                hand_row("Constrain, keep ratio", &["Shift+drag"]),
                hand_row("Resize from the centre", &["Alt+drag"]),
                hand_row("Rotate in 15° steps", &["Shift+drag"]),
                hand_row("Skew from a side", &["Ctrl+drag"]),
            ],
        },
        Section {
            title: "Library",
            rows: vec![
                hand_row("Search files and projects", &["Ctrl+K", "Ctrl+P"]),
                hand_row("Move the selection", &["←→↑↓"]),
                hand_row("Open the selection", &["Enter"]),
                hand_row("Rename the selection", &["F2", "Ctrl+R"]),
                hand_row("Duplicate the selection", &["Ctrl+D"]),
                hand_row("Delete the selection", &["Delete", "Backspace"]),
                hand_row("Clear the selection", &["Esc"]),
                hand_row("New file", &["Ctrl+N"]),
            ],
        },
    ];
    let lead_title = match lead {
        Lead::Library => Some("Library"),
        Lead::Text => Some("Text editing"),
        Lead::None => None,
    };
    if let Some(title) = lead_title
        && let Some(at) = out.iter().position(|s| s.title == title)
    {
        let s = out.remove(at);
        out.insert(0, s);
    }
    out
}

/// The modifier keys, as the registry spells them.
fn is_modifier(key: &str) -> bool {
    matches!(key, "Ctrl" | "Alt" | "Shift")
}

/// Whether `keys` uses exactly the `held` modifiers — what *hold to look*
/// leaves bright.
fn uses_exactly(keys: &[String], held: [bool; 3]) -> bool {
    let has = |m: &str| keys.iter().any(|k| k == m);
    [has("Ctrl"), has("Alt"), has("Shift")] == held && keys.iter().any(|k| !is_modifier(k))
}

/// Whether a row survives the filter: every query word starts a word of the
/// label or names one of its keys.
fn row_matches(row: &Row, query: &[String]) -> bool {
    let label: Vec<String> = row
        .label
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(str::to_lowercase)
        .collect();
    query.iter().all(|q| {
        label.iter().any(|w| w.starts_with(q.as_str()))
            || row
                .chords
                .iter()
                .flatten()
                .any(|k| k.to_lowercase().starts_with(q.as_str()))
    })
}

impl OndinApp {
    /// `Ctrl+Shift+/` from either screen, and the palette's *Keyboard shortcuts* row.
    /// The chord again closes it, read in [`Self::cheatsheet_ui`].
    pub(crate) fn open_cheatsheet(&mut self, from: Opened) {
        self.palette = None;
        self.context_menu = None;
        let first = match from {
            Opened::Library => Lead::Library,
            Opened::Editor if self.text.is_some() => Lead::Text,
            Opened::Editor => Lead::None,
        };
        self.cheatsheet = Some(Cheatsheet {
            filter: String::new(),
            first,
            just_opened: true,
            released: false,
        });
    }

    /// Draw the open sheet and close it the ways a modal closes, plus its chord.
    pub(crate) fn cheatsheet_ui(&mut self, ctx: &egui::Context) {
        let Some(sheet) = self.cheatsheet.as_mut() else {
            return;
        };
        // The chord again closes, as it toggles the palette. Not on the opening
        // frame, which drew the sheet after the door that read the same press.
        let opening = std::mem::take(&mut sheet.just_opened);
        if !opening && crate::input::cheatsheet_chord(ctx) {
            self.cheatsheet = None;
            return;
        }
        let query: Vec<String> = sheet
            .filter
            .split(|c: char| !c.is_alphanumeric())
            .filter(|w| !w.is_empty())
            .map(str::to_lowercase)
            .collect();
        let held = ctx.input(|i| {
            let m = i.modifiers;
            [m.command || m.ctrl, m.alt, m.shift]
        });
        let any_held = held.iter().any(|h| *h);
        sheet.released |= !any_held;
        let looking = sheet.released && query.is_empty() && any_held;
        let lead = sheet.first;
        let mut all = sections(lead);
        for s in &mut all {
            s.rows.retain(|r| row_matches(r, &query));
        }
        all.retain(|s| !s.rows.is_empty());

        let mut closed = false;
        let mut card = crate::settings::card_modal(ctx, "cheatsheet");
        // No margin under the footer: it sits on the card's bottom edge, as the palette's does.
        card.frame = card.frame.map(|f| {
            f.inner_margin(egui::Margin {
                bottom: 0,
                ..egui::Margin::same(crate::settings::PAD as i8)
            })
        });
        let screen = ctx.content_rect();
        let pad = crate::settings::PAD;
        let inner = W.min(screen.width() - MARGIN * 2.0) - pad * 2.0;
        let card_h = MAX_H.min(screen.height() - MARGIN * 2.0);
        let n = column_count(inner);
        let modal = card.show(ctx, |ui| {
            ui.set_width(inner);
            // An `Area` lays out in last frame's size; without this a body that
            // grows back after the filter clears gains only a step per frame.
            ui.set_max_height(card_h);
            ui.horizontal(|ui| {
                ui.label(
                    egui::RichText::new("Keyboard shortcuts")
                        .size(14.0)
                        .color(theme::text::STRONG),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if crate::ui::icon_button(ui, icon::X, 18.0, 14.0, false, true).clicked() {
                        closed = true;
                    }
                    ui.add_space(12.0);
                    let field = crate::ui::search_field(
                        ui,
                        &mut sheet.filter,
                        egui::Id::new("cheatsheet-filter"),
                        "Filter by name or key",
                        // What the title and ✕ leave of a narrow card.
                        FILTER_W.min(inner - 170.0),
                    );
                    // Focused once, on open; a click elsewhere on the sheet may take it.
                    if opening {
                        field.request_focus();
                    }
                });
            });
            ui.add_space(14.0);
            // What the card has left once its top margin, the header as drawn,
            // the footer, the item spacing either side of the body and the
            // frame's two hairlines are paid.
            let used = ui.min_rect().height() + ui.spacing().item_spacing.y * 2.0;
            let body_h = (card_h - pad - used - FOOTER_H - 2.0).max(ROW_H);
            egui::ScrollArea::vertical()
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .max_height(body_h)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    if all.is_empty() {
                        ui.label(
                            egui::RichText::new(format!(
                                "No shortcut matches “{}”",
                                sheet.filter.trim()
                            ))
                            .size(12.0)
                            .color(theme::text::DIM),
                        );
                        return;
                    }
                    let columns = split_columns(&all, lead == Lead::Library, n);
                    let col_w = (inner - COLUMN_GAP * (n - 1) as f32) / n as f32;
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = COLUMN_GAP;
                        for column in columns {
                            ui.allocate_ui_with_layout(
                                egui::vec2(col_w, 0.0),
                                egui::Layout::top_down(egui::Align::Min),
                                |ui| {
                                    ui.set_width(col_w);
                                    ui.spacing_mut().item_spacing.y = 0.0;
                                    for (divider, i) in column {
                                        if divider {
                                            in_a_file(ui);
                                        }
                                        section_ui(ui, &all[i], &query, held, looking);
                                    }
                                },
                            );
                        }
                    });
                });
            footer(ui);
        });
        if closed || modal.should_close() {
            self.cheatsheet = None;
        }
    }
}

/// How many columns a body `inner` wide deals into: three on a full-width card,
/// fewer as the window narrows, so no column drops under [`MIN_COLUMN_W`].
fn column_count(inner: f32) -> usize {
    (1..=3)
        .rev()
        .find(|&n| inner >= n as f32 * MIN_COLUMN_W + (n - 1) as f32 * COLUMN_GAP)
        .unwrap_or(1)
}

/// The sections dealt into `n` columns in order, each column filled to about
/// its share of the rows before the next starts, so the sheet reads down and
/// across as CSS columns do. A section is never split. `divide` marks the first
/// section after the library's, which gets the *In a file* divider.
fn split_columns(all: &[Section], divide: bool, n: usize) -> Vec<Vec<(bool, usize)>> {
    let height = |s: &Section| s.rows.len() as f32 * ROW_H + TITLE_H + SECTION_GAP;
    let total: f32 = all.iter().map(height).sum();
    let target = total / n as f32;
    let mut out: Vec<Vec<(bool, usize)>> = vec![Vec::new()];
    let mut filled = 0.0;
    for (i, s) in all.iter().enumerate() {
        let h = height(s);
        let last = out.len() == n;
        if !last && !out.last().is_some_and(Vec::is_empty) && filled + h / 2.0 > target {
            out.push(Vec::new());
            filled = 0.0;
        }
        out.last_mut()
            .expect("one column at least")
            .push((divide && i == 1, i));
        filled += h;
    }
    out
}

/// The quiet line between the library's keys and the editor's.
fn in_a_file(ui: &mut egui::Ui) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 22.0), egui::Sense::empty());
    let p = ui.painter();
    let g = p.layout_no_wrap(
        "In a file".into(),
        egui::FontId::proportional(10.5),
        theme::text::FAINT,
    );
    let w = g.size().x;
    p.galley(
        egui::pos2(rect.left(), rect.center().y - g.size().y / 2.0),
        g,
        theme::text::FAINT,
    );
    p.hline(
        (rect.left() + w + 10.0)..=rect.right(),
        rect.center().y,
        egui::Stroke::new(1.0, theme::color::text_a(20)),
    );
}

/// One section: its title over a rule, then its rows.
fn section_ui(ui: &mut egui::Ui, s: &Section, query: &[String], held: [bool; 3], looking: bool) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), TITLE_H),
        egui::Sense::empty(),
    );
    let p = ui.painter();
    let mut job = egui::text::LayoutJob::single_section(
        s.title.to_uppercase(),
        egui::TextFormat {
            font_id: egui::FontId::proportional(11.0),
            color: theme::color::text_a(87),
            extra_letter_spacing: 0.5,
            ..Default::default()
        },
    );
    job.wrap.max_rows = 1;
    let g = p.layout_job(job);
    let w = g.size().x;
    let y = rect.top() + 4.0;
    p.galley(egui::pos2(rect.left(), y), g, theme::text::FAINT);
    p.hline(
        (rect.left() + w + 10.0)..=rect.right(),
        y + 7.0,
        egui::Stroke::new(1.0, theme::color::text_a(23)),
    );
    for row in &s.rows {
        let bright = !looking || row.chords.iter().any(|c| uses_exactly(c, held));
        row_ui(ui, row, query, held, looking, bright);
    }
    ui.add_space(SECTION_GAP);
}

/// One row: the label left, the keycaps right, "or" between alternatives.
fn row_ui(
    ui: &mut egui::Ui,
    row: &Row,
    query: &[String],
    held: [bool; 3],
    looking: bool,
    bright: bool,
) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::empty(),
    );
    let p = ui.painter();
    let fade = |c: egui::Color32| if bright { c } else { c.gamma_multiply(0.28) };
    let font = egui::FontId::proportional(10.0);
    let mut right = rect.right();
    // Laid right to left, so the last alternative's last key is at the margin.
    for (n, chord) in row.chords.iter().enumerate().rev() {
        for key in chord.iter().rev() {
            let lit_held = looking
                && bright
                && match key.as_str() {
                    "Ctrl" => held[0],
                    "Alt" => held[1],
                    "Shift" => held[2],
                    _ => false,
                };
            let lit_query = query
                .iter()
                .any(|q| key.to_lowercase().starts_with(q.as_str()));
            let (fg, bg) = match () {
                _ if lit_held => (
                    color::ACCENT_100,
                    egui::Color32::from_rgba_unmultiplied(109, 140, 217, 77),
                ),
                _ if lit_query => (color::ACCENT_300, theme::color::text_a(20)),
                _ => (theme::color::text_a(184), theme::color::text_a(20)),
            };
            let word = key == "drag";
            let g = p.layout_no_wrap(key.clone(), font.clone(), fade(fg));
            if word {
                let x = right - g.size().x - 2.0;
                p.galley(
                    egui::pos2(x, rect.center().y - g.size().y / 2.0),
                    g,
                    fade(theme::text::FAINT),
                );
                right = x - 3.0;
                continue;
            }
            let size = egui::vec2((g.size().x + 10.0).max(18.0), 18.0);
            let cap = egui::Rect::from_min_size(
                egui::pos2(right - size.x, rect.center().y - size.y / 2.0),
                size,
            );
            p.rect_filled(cap, egui::CornerRadius::same(4), fade(bg));
            // The keycap's lower lip.
            p.hline(
                (cap.left() + 2.0)..=(cap.right() - 2.0),
                cap.bottom() - 0.5,
                egui::Stroke::new(1.0, fade(egui::Color32::from_black_alpha(90))),
            );
            p.galley(
                egui::pos2(
                    cap.center().x - g.size().x / 2.0,
                    cap.center().y - g.size().y / 2.0,
                ),
                g,
                fade(fg),
            );
            right = cap.left() - 3.0;
        }
        if n > 0 {
            let g = p.layout_no_wrap("or".into(), font.clone(), fade(theme::text::FAINT));
            let x = right - g.size().x - 2.0;
            p.galley(
                egui::pos2(x, rect.center().y - g.size().y / 2.0),
                g,
                fade(theme::text::FAINT),
            );
            right = x - 5.0;
        }
    }
    let base = fade(theme::color::text_a(204));
    let mut job = egui::text::LayoutJob::default();
    let plain = egui::TextFormat::simple(egui::FontId::proportional(11.5), base);
    let lit = egui::TextFormat::simple(egui::FontId::proportional(11.5), fade(color::ACCENT_300));
    // Light each label word a query word starts.
    let mut at = 0;
    let label = &row.label;
    for (i, ch) in label.char_indices() {
        let starts = ch.is_alphanumeric()
            && label[..i]
                .chars()
                .next_back()
                .is_none_or(|c| !c.is_alphanumeric());
        if !starts || i < at {
            continue;
        }
        let word_end = label[i..]
            .char_indices()
            .find(|(_, c)| !c.is_alphanumeric())
            .map_or(label.len(), |(b, _)| i + b);
        let word = label[i..word_end].to_lowercase();
        if let Some(q) = query
            .iter()
            .filter(|q| word.starts_with(q.as_str()))
            .max_by_key(|q| q.len())
            && word.len() == label[i..word_end].len()
        {
            job.append(&label[at..i], 0.0, plain.clone());
            job.append(&label[i..i + q.len()], 0.0, lit.clone());
            at = i + q.len();
        }
    }
    job.append(&label[at..], 0.0, plain);
    job.wrap = egui::text::TextWrapping {
        max_width: (right - rect.left() - 8.0).max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let g = p.layout_job(job);
    p.galley(
        egui::pos2(rect.left(), rect.center().y - g.size().y / 2.0),
        g,
        base,
    );
}

/// The two other ways in, and the way out.
fn footer(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), FOOTER_H),
        egui::Sense::empty(),
    );
    let p = ui.painter();
    p.hline(
        rect.x_range(),
        rect.top() + 0.5,
        egui::Stroke::new(1.0, theme::color::text_a(20)),
    );
    let font = egui::FontId::proportional(10.5);
    let y = rect.center().y;
    let mut job = egui::text::LayoutJob::default();
    let dim = egui::TextFormat::simple(font.clone(), theme::text::DIM);
    let strong = egui::TextFormat::simple(font.clone(), theme::text::MUTED);
    for (text, fmt) in [
        ("Run any command by name with ", &dim),
        ("Ctrl + K", &strong),
        ("        Hold ", &dim),
        ("Ctrl", &strong),
        (", ", &dim),
        ("Alt", &strong),
        (" or ", &dim),
        ("Shift", &strong),
        (" to see what they do", &dim),
    ] {
        job.append(text, 0.0, fmt.clone());
    }
    let esc = p.layout_no_wrap("Esc close".into(), font, theme::text::DIM);
    // On a narrow card the hints give way to "Esc close", never run under it.
    job.wrap = egui::text::TextWrapping {
        max_width: (rect.width() - esc.size().x - 16.0).max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let g = p.layout_job(job);
    p.galley(
        egui::pos2(rect.left(), y - g.size().y / 2.0),
        g,
        theme::text::DIM,
    );
    p.galley(
        egui::pos2(rect.right() - esc.size().x, y - esc.size().y / 2.0),
        esc,
        theme::text::DIM,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every registry chord the sheet shows resolves to its row's action and to
    /// nothing else — walking the sheet's own sections, so a row added there is
    /// driven too, and each spelling as `egui_winit` delivers it (`resolve_chord`).
    ///
    /// flip: spelling `Item::ZoomOut`'s accel as `Ctrl+0` → fails at "Zoom out".
    /// flip: spelling `Item::Boolean(Exclude)`'s accel as `Ctrl+Alt+E` → fails at
    /// "Exclude" (that chord is *Export all*).
    /// flip: `item_row` dropping `Item::Flip(Axis::Y)` → fails at
    /// `items_section`'s assertion, which a lost accel trips the same way.
    #[test]
    fn every_sheet_chord_resolves_to_its_rows_action() {
        let mut checked = 0;
        let items = sections(Lead::None)
            .into_iter()
            .flat_map(|s| s.rows)
            .filter_map(|r| r.item);
        for item in items {
            let spec = item.spec();
            let action = item
                .action()
                .unwrap_or_else(|| panic!("{} is on the sheet with no keymap action", spec.label));
            let accel = spec.accel.expect("a sheet item has a chord");
            let text_mode = matches!(item, Item::TextChord(_));
            for spelling in std::iter::once(accel).chain(item.alt_accel()) {
                let mut got = resolve_chord(spelling, text_mode);
                // The image-only clipboard's witness, which both pastes' releases
                // push and `dispatch` stands down whenever the clipboard holds text.
                if matches!(
                    action,
                    crate::input::Action::Paste | crate::input::Action::PasteInPlace
                ) {
                    got.retain(|a| *a != crate::input::Action::PasteRelease);
                }
                assert_eq!(
                    got,
                    vec![action],
                    "{} ({spelling}) resolved to {got:?}, not {action:?}",
                    spec.label
                );
                checked += 1;
            }
        }
        assert!(checked > 90, "only {checked} chords were driven");
    }

    /// The other way round: every action any key and modifier set resolves to,
    /// in either mode, is on the sheet, or one the design keeps off it.
    ///
    /// flip: dropping `Item::Delete` from the Edit section → fails at `Delete`.
    #[test]
    fn every_bound_action_is_on_the_sheet() {
        use crate::input::Action as A;
        let shown: Vec<A> = sections(Lead::None)
            .into_iter()
            .flat_map(|s| s.rows)
            .filter_map(|r| r.item?.action())
            .collect();
        let kept_off = |a: &A| {
            matches!(
                a,
                A::Nudge(_)
                    | A::SizeStep(_)
                    | A::StepPoint(_)
                    | A::Escape
                    | A::Enter
                    | A::PasteRelease
                    | A::OpacityDigit(_)
            )
        };
        let m = |ctrl, alt, shift| egui::Modifiers {
            alt,
            ctrl,
            shift,
            mac_cmd: false,
            command: ctrl,
        };
        let mut missing = Vec::new();
        for text_mode in [false, true] {
            for &key in egui::Key::ALL {
                for (ctrl, alt, shift) in [
                    (false, false, false),
                    (true, false, false),
                    (false, true, false),
                    (false, false, true),
                    (true, true, false),
                    (true, false, true),
                    (false, true, true),
                    (true, true, true),
                ] {
                    for a in resolve_press(key, m(ctrl, alt, shift), text_mode) {
                        if !kept_off(&a) && !shown.contains(&a) && !missing.contains(&a) {
                            missing.push(a);
                        }
                    }
                }
            }
        }
        assert!(
            missing.is_empty(),
            "bound but not on the sheet: {missing:?}"
        );
    }

    /// Press `spelling` and let go, through `input::resolve`, as `egui_winit`
    /// delivers it: a command chord on C, X or V arrives as `Event::Copy` / `Cut` /
    /// `Paste` instead of a key press, whatever Alt and Shift are doing, and its
    /// release still arrives as a key. Two frames, the actions of both.
    fn resolve_chord(spelling: &str, text_mode: bool) -> Vec<crate::input::Action> {
        let keys = keys_of(spelling);
        let mut modifiers = egui::Modifiers::NONE;
        let mut key = None;
        for k in &keys {
            match k.as_str() {
                "Ctrl" => {
                    modifiers.ctrl = true;
                    modifiers.command = true;
                }
                "Alt" => modifiers.alt = true,
                "Shift" => modifiers.shift = true,
                "→" => key = Some(egui::Key::ArrowRight),
                "←" => key = Some(egui::Key::ArrowLeft),
                "↑" => key = Some(egui::Key::ArrowUp),
                "↓" => key = Some(egui::Key::ArrowDown),
                "Del" => key = Some(egui::Key::Delete),
                other => {
                    key =
                        Some(egui::Key::from_name(other).unwrap_or_else(|| {
                            panic!("`{other}` in `{spelling}` names no egui key")
                        }))
                }
            }
        }
        resolve_press(key.expect("a chord has a key"), modifiers, text_mode)
    }

    /// Press `key` with `modifiers` held and let go, as `resolve_chord` does.
    fn resolve_press(
        key: egui::Key,
        modifiers: egui::Modifiers,
        text_mode: bool,
    ) -> Vec<crate::input::Action> {
        let event = |pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers,
        };
        let press = match key {
            _ if !modifiers.command => event(true),
            egui::Key::C => egui::Event::Copy,
            egui::Key::X => egui::Event::Cut,
            egui::Key::V => egui::Event::Paste("text".into()),
            _ => event(true),
        };
        let ctx = egui::Context::default();
        let mode = if text_mode {
            crate::input::Mode::TextInsert
        } else {
            crate::input::Mode::Normal
        };
        let mut out = Vec::new();
        for events in [vec![press], vec![event(false)]] {
            let input = egui::RawInput {
                modifiers,
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| {
                out.extend(crate::input::resolve(
                    ui.ctx(),
                    mode,
                    crate::input::NudgeStep::default(),
                ));
            });
        }
        out
    }

    /// Through real frames, on both screens: the chord opens the sheet and it stays
    /// open, the chord again closes it.
    ///
    /// flip: dropping the `just_opened` guard in `cheatsheet_ui` → fails at "stays
    /// open" (the opening press also closes it).
    #[test]
    fn the_chord_opens_the_sheet_and_closes_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let chord = egui::Modifiers {
            ctrl: true,
            command: true,
            shift: true,
            ..Default::default()
        };
        let press = |app: &mut OndinApp, events: Vec<egui::Event>, modifiers| {
            let mut frame = eframe::Frame::_new_kittest();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1320.0, 820.0),
                )),
                modifiers,
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
        };
        let slash = || {
            vec![egui::Event::Key {
                key: egui::Key::Slash,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: chord,
            }]
        };
        for library in [false, true] {
            if library {
                app.view = crate::app::View::Dashboard;
            }
            press(&mut app, Vec::new(), egui::Modifiers::NONE);
            press(&mut app, slash(), chord);
            assert!(app.cheatsheet.is_some(), "stays open (library: {library})");
            press(&mut app, Vec::new(), egui::Modifiers::NONE);
            press(&mut app, slash(), chord);
            assert!(
                app.cheatsheet.is_none(),
                "the chord again closes (library: {library})"
            );
        }
    }

    /// Holding Ctrl+Alt leaves exactly the Ctrl+Alt chords bright.
    #[test]
    fn holding_modifiers_keeps_only_their_chords() {
        let keys = |s: &str| keys_of(s);
        let held = [true, true, false];
        assert!(uses_exactly(&keys("Ctrl+Alt+U"), held));
        assert!(!uses_exactly(&keys("Ctrl+Alt+Shift+U"), held));
        assert!(!uses_exactly(&keys("Ctrl+G"), held));
    }

    /// A filter word finds a row by its label's word starts or by a key's name.
    #[test]
    fn the_filter_reads_labels_and_keys() {
        let row = hand_row("Group selection", &["Ctrl+G"]);
        assert!(row_matches(&row, &["sel".into()]));
        assert!(row_matches(&row, &["ctrl".into()]));
        assert!(!row_matches(&row, &["alt".into()]));
    }

    /// One whole app frame at `size`.
    fn app_frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        size: egui::Vec2,
        events: Vec<egui::Event>,
    ) {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, size)),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
    }

    fn card_rect(ctx: &egui::Context) -> egui::Rect {
        ctx.memory(|m| m.area_rect(egui::Id::new("cheatsheet")))
            .expect("the card was drawn")
    }

    /// **The card fits the window**: full width on a large one, narrowed with
    /// fewer columns on a small one, never past an edge.
    /// flip: `cheatsheet_ui` sizing the card to `W` whatever the window → fails
    /// at 800×560.
    #[test]
    fn the_card_fits_the_window() {
        for (w, h, columns) in [(1320.0, 820.0, 3), (800.0, 560.0, 2), (520.0, 420.0, 1)] {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let mut app = OndinApp::headless(&ctx);
            let size = egui::vec2(w, h);
            app_frame(&ctx, &mut app, size, Vec::new());
            app.open_cheatsheet(Opened::Editor);
            for _ in 0..4 {
                app_frame(&ctx, &mut app, size, Vec::new());
            }
            let card = card_rect(&ctx);
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
            assert!(
                screen.shrink(MARGIN - 1.0).contains_rect(card),
                "{w}×{h}: the card {card:?} leaves the window"
            );
            if w > W + MARGIN * 2.0 {
                // The frame's hairline sits outside the width it was given.
                assert!(
                    (card.width() - W).abs() <= 2.0,
                    "{w}×{h}: full width, got {}",
                    card.width()
                );
            }
            let inner = card.width() - 2.0 - crate::settings::PAD * 2.0;
            assert_eq!(column_count(inner), columns, "{w}×{h}: columns");
        }
    }

    /// Clearing the filter brings the card back to its full height on the next
    /// frame, not a step at a time over the frames after.
    /// flip: drop `cheatsheet_ui`'s `set_max_height` → fails at the height, 241 of 544.
    #[test]
    fn clearing_the_filter_restores_the_height_at_once() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let size = egui::vec2(1320.0, 820.0);
        app_frame(&ctx, &mut app, size, Vec::new());
        app.open_cheatsheet(Opened::Editor);
        for _ in 0..4 {
            app_frame(&ctx, &mut app, size, Vec::new());
        }
        let full = card_rect(&ctx).height();

        app.cheatsheet.as_mut().unwrap().filter = "group".into();
        for _ in 0..4 {
            app_frame(&ctx, &mut app, size, Vec::new());
        }
        let filtered = card_rect(&ctx).height();
        assert!(
            filtered < full - 100.0,
            "the filter has to shrink the card ({filtered} of {full})"
        );

        app.cheatsheet.as_mut().unwrap().filter.clear();
        app_frame(&ctx, &mut app, size, Vec::new());
        app_frame(&ctx, &mut app, size, Vec::new());
        assert_eq!(card_rect(&ctx).height(), full);
    }

    /// **The sheet holds the keyboard over a text session**: a click on its body
    /// takes the focus off the filter, and the typing, Backspace and Escape that
    /// follow still never reach the text behind the backdrop.
    /// flip: `text_mode_input` reading events under a modal → fails at the text.
    #[test]
    fn the_sheet_sends_no_keys_to_the_text_behind_it() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let id = app.session.ids.mint();
        let parent = app.session.doc.root();
        let parts = crate::fonts::FontService::default_text_parts();
        app.session
            .try_commit(ondin_core::Transaction(vec![
                ondin_core::Operation::CreateNode {
                    id,
                    parent,
                    index: 0,
                    kind: ondin_core::NodeKind::Text {
                        content: "hello".into(),
                        style: Box::new(parts.style),
                        spans: parts.spans,
                        para_spans: parts.para_spans,
                        paragraph: parts.paragraph,
                        block: parts.block,
                        sizing: parts.sizing,
                        on_path: None,
                        on_path_flip: false,
                        on_path_offset: 0.0,
                    },
                    transform: None,
                    name: None,
                },
            ]))
            .expect("the fixture's text node");
        let size = egui::vec2(1320.0, 820.0);
        app_frame(&ctx, &mut app, size, Vec::new());
        app.begin_edit_text(Some(id), None);
        app.open_cheatsheet(Opened::Editor);
        for _ in 0..3 {
            app_frame(&ctx, &mut app, size, Vec::new());
        }
        let at = egui::pos2(card_rect(&ctx).center().x, card_rect(&ctx).top() + 60.0);
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        app_frame(&ctx, &mut app, size, vec![egui::Event::PointerMoved(at)]);
        app_frame(&ctx, &mut app, size, vec![button(true)]);
        app_frame(&ctx, &mut app, size, vec![button(false)]);
        assert!(
            app.cheatsheet.is_some(),
            "fixture: the click kept the sheet"
        );
        assert_eq!(
            ctx.memory(|m| m.focused()),
            None,
            "fixture: the filter lost focus"
        );
        let key = |key| egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        app_frame(&ctx, &mut app, size, vec![egui::Event::Text("Z".into())]);
        app_frame(&ctx, &mut app, size, vec![key(egui::Key::Backspace)]);
        let content = |app: &OndinApp| app.text.as_ref().map(|s| s.editor.content().to_string());
        assert_eq!(content(&app).as_deref(), Some("hello"), "the text");
        app_frame(&ctx, &mut app, size, vec![key(egui::Key::Escape)]);
        assert!(app.cheatsheet.is_none(), "Escape closed the sheet");
        assert_eq!(
            content(&app).as_deref(),
            Some("hello"),
            "and only the sheet"
        );
    }

    /// Opened from the library, the library's section leads.
    #[test]
    fn the_screen_it_opened_from_leads() {
        assert_eq!(sections(Lead::Library)[0].title, "Library");
        assert_eq!(sections(Lead::Text)[0].title, "Text editing");
        assert_eq!(sections(Lead::None)[0].title, "Tools");
    }
}
