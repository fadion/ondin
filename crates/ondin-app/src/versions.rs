//! *Version history…* — the door to the copies `Ctrl+S` and *Save named version…*
//! pin under `.versions/<document id>/` (`library::store::pin_version`): a card
//! drawing them as a timeline, newest first under the open document's own stop
//! (*Now*), grouped by day. Each row says what that version changed against the one
//! before it (`crate::version_diffs`), in the compare's signs and hues — `~`
//! changed, `+` added, `−` removed — and opens, on a click or Space, to that diff in
//! brief, with *Show on canvas* opening the full compare of the pair.
//!
//! **Nothing moves.** Rows are one fixed height: hover, naming and a summary
//! arriving shift nothing, and opening a row pushes only the rows below it.
//!
//! **A restore loses nothing.** The current state is written and pinned first, so the
//! version it replaces is the newest entry in the same list; the restored document
//! keeps the file's path and id, and is saved at once. Undo does not cross it (the
//! history is the open document's), which is why the pin is unconditional.

use crate::app::OndinApp;
use crate::compare::{self, Pick};
use crate::library::{clock, store};
use crate::theme::{self, color, icon};
use crate::version_diffs::{Brief, Counts, Later, Pair, Part, Summary};
use eframe::egui;
use ondin_core::diff::Status;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The card's width.
const CARD_W: f32 = 504.0;
/// How tall the list grows before it scrolls.
const LIST_MAX_H: f32 = 420.0;
/// A row: its name on the first line, its summary on the second.
const ROW_H: f32 = 46.0;
/// A day's heading.
const DAY_H: f32 = 30.0;
/// The time column, its text right-aligned.
const TIME_W: f32 = 44.0;
/// The rail's column, the stops centred in it.
const RAIL_W: f32 = 18.0;
/// Between the columns.
const COL_GAP: f32 = 10.0;
/// How far a lit row's ground reaches past the list on either side.
const ROW_OUTSET: f32 = 10.0;
/// The centres of a row's two lines, from its top.
const LINE1_Y: f32 = 15.5;
const LINE2_Y: f32 = 33.5;
/// A line of an opened row's list.
const LIST_LINE_H: f32 = 20.0;
/// An opened row lists this many layers a page, then says how many more.
const LINES_SHOWN: usize = 4;
/// How long a summary may take before its placeholder shows, so a fast one never
/// flickers.
const PLACEHOLDER_AFTER: f64 = 0.15;
/// One beat of a placeholder's pulse, in seconds.
const PULSE_S: f64 = 1.4;

/// One pinned copy as the card lists it.
pub(crate) struct Version {
    pub path: PathBuf,
    /// When it was pinned, read from its file name (`<unix seconds>[-n].ondin`).
    pub at: Option<u64>,
    /// The name the user gave it (`store::version_label`).
    pub label: Option<String>,
}

/// The name field open over one version's row.
pub(crate) struct Naming {
    pub path: PathBuf,
    pub text: String,
    /// Focus is requested once, on the field's first frame.
    pub focused: bool,
}

/// When a pinned copy was pinned, from its name (`store::version_stamp`).
pub(crate) fn stamp_of(path: &Path) -> Option<u64> {
    store::version_stamp(path).map(|(secs, _)| secs)
}

/// `"24 Oct 2026, 14:32"` in local time.
pub(crate) fn when_label(secs: u64) -> String {
    format!("{}, {}", clock::date_label(secs), hm(secs))
}

/// `"14:32"` in local time.
fn hm(secs: u64) -> String {
    let local = (secs as i64 + clock::local_offset(secs)).max(0) as u64;
    let (h, m) = ((local % 86_400) / 3_600, (local % 3_600) / 60);
    format!("{h:02}:{m:02}")
}

/// The local day `secs` falls on, counted from the epoch.
fn local_day(secs: u64) -> i64 {
    (secs as i64 + clock::local_offset(secs)).div_euclid(86_400)
}

/// A day's heading and the date beside it: *Today* and *Yesterday* by name, an
/// older day by its date, with the year only when it is not this one.
fn day_heading(secs: u64, today: i64) -> (String, String) {
    let date = clock::date_label(secs);
    let year = clock::date_label(clock::now());
    let short = |d: &str| d.rsplit_once(' ').map_or(d.to_string(), |(s, _)| s.into());
    match today - local_day(secs) {
        0 => ("Today".into(), short(&date)),
        1 => ("Yesterday".into(), short(&date)),
        _ if year.rsplit_once(' ').map(|y| y.1) == date.rsplit_once(' ').map(|d| d.1) => {
            (short(&date), String::new())
        }
        _ => (date, String::new()),
    }
}

/// A placeholder's opacity at `time`, `delay` into its beat.
fn pulse(time: f64, delay: f64) -> f32 {
    let phase = (time - delay) / PULSE_S * std::f64::consts::TAU;
    (0.35 + 0.65 * (0.5 - 0.5 * phase.cos())) as f32
}

/// One stop on the timeline: the open document, or a version.
struct Stop {
    key: Later,
    pair: Pair,
    /// Its place among the versions, newest first; `None` for Now.
    place: Option<usize>,
    time: String,
    /// How long ago, beside a row of today.
    ago: String,
    /// Its name as drawn.
    name: Option<String>,
    summary: Option<Arc<Summary>>,
    /// When its summary was asked for, while it is being worked out.
    asked: Option<f64>,
    /// The time of the version it is compared with.
    before: Option<String>,
}

impl Stop {
    /// Whether the row opens: its diff has something to list, or is still coming.
    fn openable(&self) -> bool {
        self.before.is_some()
            && match self.summary.as_deref() {
                None => self.asked.is_some(),
                Some(s) => matches!(s, Summary::Changed(_)),
            }
    }

    fn same(&self) -> bool {
        matches!(self.summary.as_deref(), Some(Summary::Same))
    }

    /// Its own version cannot be read, so it cannot be compared.
    fn unreadable(&self) -> bool {
        matches!(
            self.summary.as_deref(),
            Some(Summary::Failed { this: true })
        )
    }
}

/// *Compare with now*: the compare's own Then ⇄ Now grip, so it reads apart from
/// *Show on canvas*'s git-diff glyph, which opens a different pair.
const NOW_GLYPH: &str = icon::ARROWS_LEFT_RIGHT;

/// What a press on a row asked for.
enum Press {
    Toggle,
    /// Its compare with now, and whether Shift was held.
    Compare(bool),
    Name,
    Restore,
}

/// How a row is drawn beyond what its stop says.
struct Look<'a> {
    /// One of its buttons has keyboard focus, so its buttons stay.
    held: bool,
    opened: bool,
    /// Another version's row had the pointer or focus last frame, so an opened row
    /// gives its buttons up to it: one row's buttons at a time.
    elsewhere: bool,
    /// Compared, or picked to be.
    marked: bool,
    /// The card was opened to compare: every row shows its compare button lit.
    comparing: bool,
    private: bool,
    time: f64,
    first: bool,
    last: bool,
    /// Under Now with no versions: the rail below is dashed.
    dashed: bool,
    /// The row's place among the pending, for its placeholder's widths and beat.
    pending: usize,
    naming: Option<&'a mut Naming>,
}

/// What a row did this frame.
#[derive(Default)]
struct RowOut {
    press: Option<Press>,
    /// One of its buttons has keyboard focus.
    held: bool,
    /// A version's row under the pointer or with focus.
    pointed: bool,
    /// Its name field let go of focus.
    named: bool,
}

/// The name field over a version's first line, `rect` ([`crate::ui::inline_edit`]).
/// Answers `true` once the field lets go of focus, by Enter, Escape or a press
/// elsewhere; the caller tells Escape apart.
fn name_field(ui: &mut egui::Ui, rect: egui::Rect, n: &mut Naming) -> bool {
    let id = egui::Id::new(("version-name", &n.path));
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(rect));
    let resp = crate::ui::inline_edit(&mut child, rect, &mut n.text, id);
    // The store's cap, which `inline_edit` has no `char_limit` for.
    if let Some((at, _)) = n.text.char_indices().nth(store::LABEL_MAX) {
        n.text.truncate(at);
    }
    if n.text.is_empty() {
        child.painter().text(
            egui::pos2(rect.left() + crate::ui::INLINE_EDIT_PAD_X, rect.center().y),
            egui::Align2::LEFT_CENTER,
            "Name this version",
            egui::FontId::proportional(crate::ui::CONTROL_PT),
            theme::text::faint(),
        );
    }
    // Once, through the latch: requesting focus every frame would re-take the caret
    // on the frame that surrendered it, and `lost_focus` would never be true.
    if !n.focused {
        n.focused = true;
        resp.request_focus();
        let all = egui::text::CCursorRange::two(
            egui::text::CCursor::new(0),
            egui::text::CCursor::new(n.text.chars().count()),
        );
        if let Some(mut state) = egui::TextEdit::load_state(ui.ctx(), id) {
            state.cursor.set_char_range(Some(all));
            state.store(ui.ctx(), id);
        }
    }
    resp.lost_focus()
}

/// A status tag, the compare card's: its hue, and its sign in the tag's ink.
fn tag(p: &egui::Painter, at: egui::Rect, status: Status) {
    let (hue, ink, sign) = compare::look(status);
    p.rect_filled(at, 2.0, hue);
    p.text(
        at.center(),
        egui::Align2::CENTER_CENTER,
        sign,
        egui::FontId::proportional(crate::ui::CAPTION_PT),
        ink,
    );
}

/// `counts` as runs of a line: `~5`, `+2`, `−3`, each in its status's hue.
fn count_runs(counts: Counts) -> Vec<(String, egui::Color32)> {
    counts
        .each()
        .map(|(status, n)| {
            let (hue, _, sign) = compare::look(status);
            (format!("{sign}{n}"), hue)
        })
        .collect()
}

/// `runs` laid out as one line at `ui::SMALL_PT`, a space between each, cut at
/// `width`.
fn runs_line(ui: &egui::Ui, runs: &[(String, egui::Color32)], width: f32) -> Arc<egui::Galley> {
    let mut parts: Vec<(&str, f32, egui::Color32, bool)> = Vec::new();
    for (i, (text, ink)) in runs.iter().enumerate() {
        if i > 0 {
            parts.push((" ", crate::ui::SMALL_PT, *ink, false));
        }
        parts.push((text, crate::ui::SMALL_PT, *ink, false));
    }
    compare::one_line(ui, &parts, width)
}

/// The summary line under a row's name, or `None` while it is being worked out
/// (or failed, which draws its own line).
fn summary_runs(s: &Stop, private: bool) -> Option<Vec<(String, egui::Color32)>> {
    let before = s.before.as_deref().unwrap_or_default();
    let quiet = |t: String| vec![(t, theme::text::faint())];
    let runs = match (s.summary.as_deref()?, s.place) {
        (Summary::Failed { .. }, _) => return None,
        (Summary::Same, None) => quiet(format!("No changes since {before}")),
        (Summary::Same, Some(_)) => quiet(format!("Same as {before}")),
        (Summary::Size { pages, layers }, place) => {
            let size = format!(
                "{}, {}",
                crate::ui::count(*pages, "page"),
                crate::ui::count(*layers, "layer")
            );
            match place {
                None => vec![(size.replace(", ", " · "), theme::text::faint())],
                Some(_) => quiet(format!("First version · {size}")),
            }
        }
        (Summary::Changed(brief), place) => {
            let mut runs = Vec::new();
            if place.is_none() {
                runs.push((format!("Since {before}:"), theme::text::faint()));
            }
            runs.extend(brief.parts(private).into_iter().map(|part| match part {
                Part::Words(w) => (w, theme::text::muted()),
                Part::Count(status, n) => {
                    let (hue, _, sign) = compare::look(status);
                    (format!("{sign}{n}"), hue)
                }
            }));
            runs
        }
    };
    Some(runs)
}

/// `add` at `rect`, over what `ui` has laid out. Not `Ui::put`, which moves the
/// cursor to below `rect` — above the row's bottom, so the row would shrink.
fn placed(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    add: impl FnOnce(&mut egui::Ui) -> egui::Response,
) -> egui::Response {
    add(&mut ui.new_child(egui::UiBuilder::new().max_rect(rect)))
}

/// A [`crate::ui::action_button`] with `glyph` and `word`, [`crate::ui::CONTROL_H`]
/// tall and sized to the word, its left edge at `x` and centred on `cy`.
fn word_button(ui: &mut egui::Ui, x: f32, cy: f32, glyph: &str, word: &str) -> egui::Response {
    let rect = word_button_rect(ui, x, cy, word);
    placed(ui, rect, move |ui: &mut egui::Ui| {
        let off = crate::ui::FieldButton::Off;
        crate::ui::action_button(ui, glyph, word, off, rect.size())
    })
}

/// Where [`word_button`] lays a button for `word` from `x`, centred on `cy`.
fn word_button_rect(ui: &egui::Ui, x: f32, cy: f32, word: &str) -> egui::Rect {
    let w = crate::ui::action_button_w(ui.ctx(), word);
    egui::Rect::from_min_size(
        egui::pos2(x, cy - crate::ui::CONTROL_H / 2.0),
        egui::vec2(w, crate::ui::CONTROL_H),
    )
}

/// One stop of the timeline: the time, the rail and the stop, the name and the
/// summary, and at the right edge how long ago — or, under the pointer, with
/// keyboard focus or opened, *Compare with now*, the pencil and *Restore*.
fn stop_row(ui: &mut egui::Ui, s: &Stop, look: Look) -> RowOut {
    let mut out = RowOut::default();
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::hover(),
    );
    let id = egui::Id::new(("version-row", &s.key));
    let sense = match s.openable() {
        true => egui::Sense::click(),
        false => egui::Sense::hover(),
    };
    let resp = ui.interact(rect, id, sense);
    let editing = look.naming.is_some();
    let pointed = ui.rect_contains_pointer(rect) || resp.has_focus() || look.held;
    let version = s.place.is_some();
    let buttons = version && (pointed || (look.opened && !look.elsewhere) || editing);
    out.pointed = version && pointed;
    let lit = look.opened || look.marked || editing || (pointed && (version || s.openable()));
    let p = ui.painter().clone();
    if lit {
        let ground = rect.expand2(egui::vec2(ROW_OUTSET, 0.0));
        let r = match look.opened {
            true => egui::CornerRadius {
                nw: 6,
                ne: 6,
                sw: 0,
                se: 0,
            },
            false => egui::CornerRadius::same(6),
        };
        p.rect_filled(ground, r, color::text_a(15));
    }
    let same = s.same();
    let named = s.name.is_some();
    let lit_stop = buttons || editing;

    // The time, level with the first line.
    let time_ink = match (s.place, same, named) {
        (None, _, _) => color::accent_300(),
        (_, true, _) => theme::text::faint(),
        (_, _, true) => theme::text::muted(),
        _ => theme::text::dim(),
    };
    let y1 = rect.top() + LINE1_Y;
    p.text(
        egui::pos2(rect.left() + TIME_W, y1),
        egui::Align2::RIGHT_CENTER,
        &s.time,
        egui::FontId::proportional(crate::ui::SMALL_PT),
        time_ink,
    );

    // The rail and the stop on it.
    let rail = rect.left() + TIME_W + COL_GAP + RAIL_W / 2.0;
    let line = egui::Stroke::new(1.0, color::text_a(30));
    let r = match (s.place, named) {
        (None, _) => 5.5,
        (_, true) => 5.0,
        _ => 3.0,
    };
    if !look.first {
        p.line_segment(
            [egui::pos2(rail, rect.top()), egui::pos2(rail, y1 - r)],
            line,
        );
    }
    if look.dashed {
        dashed_rail(&p, rail, y1 + r, rect.bottom());
    } else if !look.last || look.opened {
        p.line_segment(
            [egui::pos2(rail, y1 + r), egui::pos2(rail, rect.bottom())],
            line,
        );
    }
    let c = egui::pos2(rail, y1);
    match (s.place, named) {
        (None, _) => {
            p.circle_stroke(c, r - 1.0, egui::Stroke::new(2.0, color::accent_400()));
        }
        (_, true) => {
            p.circle_filled(c, r + 3.0, color::accent_a(56));
            p.circle_filled(c, r, color::accent_400());
        }
        _ => {
            p.circle_filled(
                c,
                r,
                match (lit_stop, same) {
                    (true, _) => color::text_a(153),
                    (_, true) => color::text_a(56),
                    _ => color::text_a(81),
                },
            );
        }
    }

    // The right edge: the buttons, or how long ago.
    let mid = rect.center().y;
    let mut right = rect.right();
    let x0 = rect.left() + TIME_W + COL_GAP + RAIL_W + COL_GAP;
    if buttons {
        let rw = crate::ui::action_button_w(ui.ctx(), "Restore");
        let restore = word_button_rect(ui, right - rw, mid, "Restore");
        let pencil = egui::Rect::from_min_size(
            egui::pos2(restore.left() - 6.0 - 24.0, mid - 12.0),
            egui::vec2(24.0, 24.0),
        );
        let diff = pencil.translate(egui::vec2(-28.0, 0.0));
        let compare = placed(ui, diff, |ui: &mut egui::Ui| {
            crate::ui::icon_button(
                ui,
                NOW_GLYPH,
                24.0,
                14.0,
                look.marked || look.comparing,
                !s.unreadable(),
            )
        })
        .on_hover_text(match s.unreadable() {
            true => "This version could not be read",
            false => "Compare with now · Shift+click two to compare them",
        });
        let tip = match (look.private, named) {
            (true, _) => crate::private::DECLINE_RENAME,
            (false, true) => "Rename this version",
            (false, false) => "Name this version",
        };
        let name = placed(ui, pencil, |ui: &mut egui::Ui| {
            crate::ui::icon_button(ui, icon::PENCIL_SIMPLE, 24.0, 13.0, editing, !look.private)
        })
        .on_hover_text(tip);
        let back = word_button(
            ui,
            restore.left(),
            mid,
            icon::ARROW_COUNTER_CLOCKWISE,
            "Restore",
        )
        .on_hover_text("Make this the document — the current state is kept as the newest version");
        if compare.clicked() && !s.unreadable() {
            out.press = Some(Press::Compare(ui.input(|i| i.modifiers.shift)));
        } else if name.clicked() && !look.private && !editing {
            out.press = Some(Press::Name);
        } else if back.clicked() {
            out.press = Some(Press::Restore);
        }
        out.held = compare.has_focus() || name.has_focus() || back.has_focus();
        right = diff.left() - 8.0;
    } else if version && look.comparing {
        let diff =
            egui::Rect::from_min_size(egui::pos2(right - 24.0, mid - 12.0), egui::vec2(24.0, 24.0));
        let compare = placed(ui, diff, |ui: &mut egui::Ui| {
            crate::ui::icon_button(ui, NOW_GLYPH, 24.0, 14.0, true, !s.unreadable())
        })
        .on_hover_text("Compare with now · Shift+click two to compare them");
        if compare.clicked() && !s.unreadable() {
            out.press = Some(Press::Compare(ui.input(|i| i.modifiers.shift)));
        }
        out.held = compare.has_focus();
        right = diff.left() - 8.0;
    } else if !s.ago.is_empty() {
        let g = p.layout_no_wrap(
            s.ago.clone(),
            egui::FontId::proportional(crate::ui::SMALL_PT),
            theme::text::faint(),
        );
        right -= g.size().x;
        p.galley(
            egui::pos2(right, mid - g.size().y / 2.0),
            g,
            theme::text::faint(),
        );
        right -= 8.0;
    }

    // The name, or its field.
    let label_w = right - x0;
    match look.naming {
        Some(n) => {
            // The text where the name was drawn, the field's inset to its left.
            let pad = crate::ui::INLINE_EDIT_PAD_X;
            let at = egui::Rect::from_min_size(
                egui::pos2(x0 - pad, rect.top() + 4.0),
                egui::vec2(label_w + pad, 23.0),
            );
            out.named = name_field(ui, at, n);
        }
        None => {
            let (text, ink_) = match (s.place, &s.name) {
                (None, _) => ("Current state", theme::text::strong()),
                (_, Some(name)) if same => (name.as_str(), theme::text::muted()),
                (_, Some(name)) => (name.as_str(), color::text()),
                _ if lit_stop => ("Saved", theme::text::muted()),
                _ if same => ("Saved", theme::text::faint()),
                _ => ("Saved", theme::text::dim()),
            };
            let g = compare::one_line(ui, &[(text, crate::ui::CONTROL_PT, ink_, false)], label_w);
            p.galley(egui::pos2(x0, y1 - g.size().y / 2.0), g, ink_);
        }
    }

    // The summary.
    let y2 = rect.top() + LINE2_Y;
    match (summary_runs(s, look.private), s.summary.as_deref()) {
        (Some(runs), _) => {
            let g = runs_line(ui, &runs, label_w);
            p.galley(
                egui::pos2(x0, y2 - g.size().y / 2.0),
                g,
                theme::text::muted(),
            );
        }
        (None, Some(Summary::Failed { this })) => {
            p.text(
                egui::pos2(x0, y2),
                egui::Align2::LEFT_CENTER,
                icon::WARNING,
                theme::icon_font(12.0),
                color::warn(),
            );
            let words = match this {
                true => "Couldn’t compare: this version could not be read",
                false => "Couldn’t compare: the version before it could not be read",
            };
            let faint = theme::text::faint();
            let g = compare::one_line(
                ui,
                &[(words, crate::ui::SMALL_PT, faint, false)],
                label_w - 17.0,
            );
            p.galley(egui::pos2(x0 + 17.0, y2 - g.size().y / 2.0), g, faint);
        }
        _ if s
            .asked
            .is_some_and(|at| look.time - at >= PLACEHOLDER_AFTER) =>
        {
            const BARS: [(f32, f32); 5] = [
                (150.0, 40.0),
                (96.0, 28.0),
                (124.0, 36.0),
                (110.0, 32.0),
                (88.0, 30.0),
            ];
            let (a, b) = BARS[look.pending % BARS.len()];
            let beat = pulse(look.time, look.pending as f64 * 0.12);
            let bar = |x: f32, w: f32, alpha: u8| {
                let r = egui::Rect::from_min_size(egui::pos2(x, y2 - 3.5), egui::vec2(w, 7.0));
                p.rect_filled(r, 4.0, color::text_a(alpha).gamma_multiply(beat));
            };
            bar(x0, a.min(label_w), 35);
            bar(x0 + a + 5.0, b.min((label_w - a - 5.0).max(0.0)), 25);
        }
        _ => {}
    }
    if out.press.is_none() && resp.clicked() {
        out.press = Some(Press::Toggle);
    }
    out
}

/// The rail from `top` to `bottom` in dashes: the line under Now before any version.
fn dashed_rail(p: &egui::Painter, x: f32, top: f32, bottom: f32) {
    let stroke = egui::Stroke::new(1.0, color::text_a(45));
    let mut y = top + 2.0;
    while y < bottom {
        p.line_segment(
            [egui::pos2(x, y), egui::pos2(x, (y + 3.0).min(bottom))],
            stroke,
        );
        y += 6.0;
    }
}

/// One group of an opened row: a page, or what belongs to none.
struct Group {
    glyph: &'static str,
    name: String,
    counts: Counts,
    /// Each line's status, trail, name and what changed.
    lines: Vec<(Status, String, String, String)>,
}

/// An opened row's groups: its pages in the document's order, then *Document*.
fn groups(brief: &Brief, private: bool) -> Vec<Group> {
    let mut out: Vec<Group> = brief
        .pages
        .iter()
        .map(|page| Group {
            glyph: icon::FILE,
            name: match private {
                true => page.masked.clone(),
                false => page.name.clone(),
            },
            counts: page.counts,
            lines: page
                .lines
                .iter()
                .map(|l| match private {
                    true => (l.status, String::new(), l.masked.clone(), l.what.clone()),
                    false => (l.status, l.trail.clone(), l.name.clone(), l.what.clone()),
                })
                .collect(),
        })
        .collect();
    if !brief.document.is_empty() {
        let mut counts = Counts::default();
        for item in &brief.document {
            match item.status {
                Status::Changed => counts.changed += 1,
                Status::Added => counts.added += 1,
                Status::Removed => counts.removed += 1,
            }
        }
        out.push(Group {
            glyph: icon::SLIDERS,
            name: "Document".into(),
            counts,
            lines: brief
                .document
                .iter()
                .map(|item| {
                    let name = match private {
                        true => item.kind.clone(),
                        false => item.subject.clone(),
                    };
                    (item.status, String::new(), name, String::new())
                })
                .collect(),
        });
    }
    out
}

/// A group's height in an opened row.
fn group_h(g: &Group) -> f32 {
    let shown = g.lines.len().min(LINES_SHOWN) as f32;
    let more = if g.lines.len() > LINES_SHOWN {
        3.0 + 14.0
    } else {
        0.0
    };
    LIST_LINE_H + shown * (3.0 + LIST_LINE_H) + more
}

/// Under an opened row: its diff in brief, by page, a line per changed layer, and
/// *Show on canvas* — or, while the diff is worked out, which pair it is comparing
/// and three pulsing lines. Answers whether *Show on canvas* was pressed.
fn opened_rows(ui: &mut egui::Ui, s: &Stop, private: bool, time: f64, last: bool) -> bool {
    let brief = match s.summary.as_deref() {
        Some(Summary::Changed(b)) => Some(b),
        _ => None,
    };
    let groups = brief.map(|b| groups(b, private)).unwrap_or_default();
    let body = match brief {
        Some(_) => {
            groups.iter().map(group_h).sum::<f32>() + 10.0 * groups.len().saturating_sub(1) as f32
        }
        None => LIST_LINE_H + 3.0 * (3.0 + LIST_LINE_H),
    };
    let h = 6.0 + body + 10.0 + crate::ui::CONTROL_H + 10.0;
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), h), egui::Sense::hover());
    let p = ui.painter().clone();
    p.rect_filled(
        rect.expand2(egui::vec2(ROW_OUTSET, 0.0)),
        egui::CornerRadius {
            nw: 0,
            ne: 0,
            sw: 6,
            se: 6,
        },
        color::text_a(15),
    );
    let rail = rect.left() + TIME_W + COL_GAP + RAIL_W / 2.0;
    if !last {
        p.line_segment(
            [
                egui::pos2(rail, rect.top()),
                egui::pos2(rail, rect.bottom()),
            ],
            egui::Stroke::new(1.0, color::text_a(30)),
        );
    }
    let x0 = rect.left() + TIME_W + COL_GAP + RAIL_W + COL_GAP;
    let w = rect.right() - x0;
    let before = s.before.clone().unwrap_or_default();
    let mut y = rect.top() + 6.0;
    let centre = |y: f32| y + LIST_LINE_H / 2.0;
    if brief.is_none() {
        p.text(
            egui::pos2(x0, centre(y)),
            egui::Align2::LEFT_CENTER,
            icon::CIRCLE_NOTCH,
            theme::icon_font(12.0),
            color::accent_400(),
        );
        p.text(
            egui::pos2(x0 + 18.0, centre(y)),
            egui::Align2::LEFT_CENTER,
            format!("Comparing with {before}…"),
            egui::FontId::proportional(crate::ui::SMALL_PT),
            theme::text::dim(),
        );
        y += LIST_LINE_H;
        for (i, frac) in [0.78, 0.62, 0.70].into_iter().enumerate() {
            y += 3.0;
            let beat = pulse(time, i as f64 * 0.12);
            let cy = centre(y);
            let dot = egui::Rect::from_center_size(egui::pos2(x0 + 7.0, cy), egui::vec2(6.0, 6.0));
            p.rect_filled(dot, 2.0, color::text_a(35).gamma_multiply(beat));
            let bar = egui::Rect::from_min_size(
                egui::pos2(x0 + 20.0, cy - 3.5),
                egui::vec2((w - 20.0) * frac, 7.0),
            );
            p.rect_filled(bar, 4.0, color::text_a(25).gamma_multiply(beat));
            y += LIST_LINE_H;
        }
    }
    for (gi, g) in groups.iter().enumerate() {
        if gi > 0 {
            y += 10.0;
        }
        // The heading: glyph, name, counts.
        let cy = centre(y);
        p.text(
            egui::pos2(x0, cy),
            egui::Align2::LEFT_CENTER,
            g.glyph,
            theme::icon_font(12.0),
            theme::text::dim(),
        );
        let mut runs = vec![(g.name.clone(), theme::text::strong())];
        runs.extend(count_runs(g.counts));
        let head = runs_line(ui, &runs, w - 18.0);
        p.galley(
            egui::pos2(x0 + 18.0, cy - head.size().y / 2.0),
            head,
            theme::text::strong(),
        );
        y += LIST_LINE_H;
        for (status, trail, name, what) in g.lines.iter().take(LINES_SHOWN) {
            y += 3.0;
            let cy = centre(y);
            let at = egui::Rect::from_center_size(egui::pos2(x0 + 7.0, cy), egui::vec2(12.0, 12.0));
            tag(&p, at, *status);
            let what = match what.is_empty() {
                true => String::new(),
                false => format!(" · {what}"),
            };
            let line = compare::one_line(
                ui,
                &[
                    (trail, crate::ui::SMALL_PT, theme::text::faint(), false),
                    (name, crate::ui::SMALL_PT, theme::text::strong(), false),
                    (&what, crate::ui::SMALL_PT, theme::text::dim(), false),
                ],
                w - 20.0,
            );
            p.galley(
                egui::pos2(x0 + 20.0, cy - line.size().y / 2.0),
                line,
                theme::text::strong(),
            );
            y += LIST_LINE_H;
        }
        if g.lines.len() > LINES_SHOWN {
            y += 3.0;
            let more = g.lines.len() - LINES_SHOWN;
            let words = match g.glyph == icon::FILE {
                true => format!("and {more} more on {}", g.name),
                false => format!("and {more} more"),
            };
            let faint = theme::text::faint();
            let line = compare::one_line(
                ui,
                &[(&words, crate::ui::CAPTION_PT, faint, false)],
                w - 20.0,
            );
            p.galley(egui::pos2(x0 + 20.0, y), line, faint);
            y += 14.0;
        }
    }
    // The footer: the pair as a compare, and which pair it is.
    y += 10.0;
    let cy = y + crate::ui::CONTROL_H / 2.0;
    let show = word_button(ui, x0, cy, icon::GIT_DIFF, "Show on canvas");
    p.text(
        egui::pos2(show.rect.right() + 8.0, cy),
        egui::Align2::LEFT_CENTER,
        format!("{} against {before}", s.time),
        egui::FontId::proportional(crate::ui::CAPTION_PT),
        theme::text::faint(),
    );
    show.clicked()
}

/// A day's heading on the rail: the day, and its date dim beside it.
fn day_row(ui: &mut egui::Ui, day: &str, date: &str) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), DAY_H),
        egui::Sense::hover(),
    );
    let p = ui.painter();
    let rail = rect.left() + TIME_W + COL_GAP + RAIL_W / 2.0;
    p.line_segment(
        [
            egui::pos2(rail, rect.top()),
            egui::pos2(rail, rect.bottom()),
        ],
        egui::Stroke::new(1.0, color::text_a(30)),
    );
    let x0 = rect.left() + TIME_W + COL_GAP + RAIL_W + COL_GAP;
    let y = rect.bottom() - 6.0;
    let head = crate::ui::paint_eyebrow(p, egui::pos2(x0, y), egui::Align2::LEFT_BOTTOM, day);
    p.text(
        egui::pos2(head.right() + 7.0, y),
        egui::Align2::LEFT_BOTTOM,
        date,
        egui::FontId::proportional(crate::ui::CAPTION_PT),
        theme::text::faint(),
    );
}

/// Under Now with no versions: why there are none, beside the dashed rail. `pins`
/// is the version-history preference: off, `Ctrl+S` pins nothing.
fn empty_rows(ui: &mut egui::Ui, filed: bool, pins: bool) {
    let w = ui.available_width();
    let x0 = TIME_W + COL_GAP + RAIL_W + COL_GAP;
    let words = match (filed, pins) {
        (true, true) => {
            "Each Ctrl+S adds a stop here, with what it changed. Name one to find it again."
        }
        (true, false) => "Save named version… adds a stop here, with what it changed.",
        (false, _) => "Save this document to the library to keep its versions.",
    };
    let body = ui.fonts_mut(|f| {
        f.layout(
            words.into(),
            egui::FontId::proportional(crate::ui::SMALL_PT),
            theme::text::dim(),
            w - x0,
        )
    });
    let h = 10.0 + 17.0 + 4.0 + body.size().y + 4.0;
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, h), egui::Sense::hover());
    let p = ui.painter();
    let rail = rect.left() + TIME_W + COL_GAP + RAIL_W / 2.0;
    dashed_rail(p, rail, rect.top() - 2.0, rect.bottom());
    let x = rect.left() + x0;
    if filed {
        p.text(
            egui::pos2(x, rect.top() + 10.0),
            egui::Align2::LEFT_TOP,
            "No versions yet",
            egui::FontId::proportional(crate::ui::CONTROL_PT),
            color::text(),
        );
    }
    let top = if filed {
        rect.top() + 31.0
    } else {
        rect.top() + 10.0
    };
    p.galley(egui::pos2(x, top), body, theme::text::dim());
}

/// The card's head: the glyph, the title, the document and its count dim beside it,
/// how far the summaries have got while they are worked out, and the close button.
/// Answers whether the close button was pressed.
fn card_head(ui: &mut egui::Ui, title: &str, progress: Option<(usize, usize)>) -> bool {
    let close = ui
        .horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            ui.label(theme::icon_text(
                icon::CLOCK_COUNTER_CLOCKWISE,
                16.0,
                color::accent_400(),
            ));
            ui.label(
                egui::RichText::new("Version history")
                    .size(crate::ui::TITLE_PT)
                    .color(theme::text::strong()),
            );
            ui.add(
                egui::Label::new(
                    egui::RichText::new(title)
                        .size(crate::ui::SMALL_PT)
                        .color(theme::text::dim()),
                )
                .truncate(),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let close = crate::ui::close_button(ui).clicked();
                if let Some((done, all)) = progress {
                    ui.label(
                        egui::RichText::new(format!("Comparing {done} of {all}"))
                            .size(crate::ui::SMALL_PT)
                            .color(theme::text::faint()),
                    );
                }
                close
            })
            .inner
        })
        .inner;
    // A thin bar under the head across the whole card, while summaries are coming.
    if let Some((done, all)) = progress {
        let at = ui.cursor().min.y + 4.0;
        let full = ui.max_rect().x_range();
        let (x0, x1) = (
            full.min - crate::settings::PAD,
            full.max + crate::settings::PAD,
        );
        let p = ui.painter();
        p.rect_filled(
            egui::Rect::from_x_y_ranges(x0..=x1, at..=at + 2.0),
            0.0,
            color::text_a(15),
        );
        let to = x0 + (x1 - x0) * done as f32 / all.max(1) as f32;
        p.rect_filled(
            egui::Rect::from_x_y_ranges(x0..=to, at..=at + 2.0),
            0.0,
            color::accent_400(),
        );
    }
    close
}

/// A key's name on a faint chip, set a point low: centred on the row, the chip's
/// small caps-height text reads high against the words beside it.
fn key_chip(ui: &mut egui::Ui, key: &str) {
    let text = ui.fonts_mut(|f| {
        f.layout_no_wrap(
            key.into(),
            egui::FontId::proportional(crate::ui::CAPTION_PT),
            theme::text::muted(),
        )
    });
    let size = text.size() + egui::vec2(10.0, 2.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    let rect = rect.translate(egui::vec2(0.0, 1.0));
    let p = ui.painter();
    p.rect_filled(rect, 3.0, color::text_a(20));
    p.galley(
        rect.center() - text.size() / 2.0,
        text,
        theme::text::muted(),
    );
}

/// The card's foot: how a version is saved — said only while `Ctrl+S` pins one
/// (`pins`) — *Save named version…* and *Close*. Answers which was pressed:
/// `Some(true)` to save a named version, `Some(false)` to close.
fn card_foot(ui: &mut egui::Ui, pins: bool) -> Option<bool> {
    let rule = ui.cursor().min.y;
    let full = ui.max_rect().x_range();
    ui.painter().line_segment(
        [
            egui::pos2(full.min - crate::settings::PAD, rule),
            egui::pos2(full.max + crate::settings::PAD, rule),
        ],
        egui::Stroke::new(1.0, color::text_a(20)),
    );
    ui.add_space(12.0);
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 4.0;
        if pins {
            key_chip(ui, "Ctrl+S");
            ui.label(
                egui::RichText::new("saves a version")
                    .size(crate::ui::SMALL_PT)
                    .color(theme::text::dim()),
            );
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            ui.spacing_mut().item_spacing.x = 8.0;
            let off = crate::ui::FieldButton::Off;
            if crate::settings::text_button(ui, "Close", off).clicked() {
                return Some(false);
            }
            if crate::settings::text_button(ui, "Save named version…", off).clicked() {
                return Some(true);
            }
            None
        })
        .inner
    })
    .inner
}

impl OndinApp {
    /// Whether the open document is a file the library lists, the one place
    /// `Ctrl+S` pins versions. A file elsewhere may carry a minted id, a library
    /// document's copy even, and is still not.
    pub(crate) fn in_library(&self) -> bool {
        self.session
            .path
            .as_deref()
            .is_some_and(|p| self.library.entry_at(p).is_some())
    }

    /// The open document's pinned versions, newest first; empty when it is not in
    /// the library or has no id to keep them under.
    pub(crate) fn document_versions(&self) -> Vec<Version> {
        if !self.in_library() {
            return Vec::new();
        }
        let Some(id) = self
            .session
            .doc
            .meta()
            .id
            .as_deref()
            .filter(|id| crate::library::ids::is_minted(id))
        else {
            return Vec::new();
        };
        store::versions(&self.library.root, id)
            .into_iter()
            .map(|path| Version {
                at: stamp_of(&path),
                label: store::version_label(&path),
                path,
            })
            .collect()
    }

    /// *Save named version…*: save, pin a version whatever the version-history
    /// preference says, since the ask is explicit, and open the card on that
    /// version's name field.
    pub(crate) fn save_named_version(&mut self) {
        self.finish_text_first();
        if !self.save_file(false) {
            return;
        }
        if !self.in_library() {
            return self
                .session
                .fail("Save this document to the library to keep its versions");
        }
        if let Some(path) = self.pin_saved_version() {
            self.versions_open = true;
            self.version_naming = Some(Naming {
                path,
                text: String::new(),
                focused: false,
            });
        }
    }

    /// Keep the name typed for `naming`'s version, or report why it was not kept.
    pub(crate) fn commit_version_name(&mut self, naming: Naming) {
        if let Err(e) = store::set_version_label(&naming.path, &naming.text) {
            let e = crate::private::io_error(&e);
            self.session.fail(format!("Naming the version failed: {e}"));
        }
    }

    /// The stops of the timeline, Now first, and each version's pair asked for in
    /// that order — the row opened first of all.
    fn timeline(&mut self, ctx: &egui::Context, versions: &[Version]) -> Vec<Stop> {
        let time = ctx.input(|i| i.time);
        let today = local_day(clock::now());
        let mut stops = vec![Stop {
            key: Later::Now(self.session.revision()),
            pair: Pair {
                older: versions.first().map(|v| v.path.clone()),
                newer: Later::Now(self.session.revision()),
            },
            place: None,
            time: "Now".into(),
            ago: String::new(),
            name: None,
            summary: None,
            asked: None,
            before: versions
                .first()
                .map(|v| v.at.map_or_else(|| "—".into(), hm)),
        }];
        for (place, v) in versions.iter().enumerate() {
            let key = Later::Version(v.path.clone());
            let older = versions.get(place + 1);
            stops.push(Stop {
                pair: Pair {
                    older: older.map(|o| o.path.clone()),
                    newer: key.clone(),
                },
                key,
                place: Some(place),
                time: v.at.map_or_else(|| "—".into(), hm),
                ago: match v.at {
                    Some(at) if local_day(at) == today => clock::relative_label(clock::since(at)),
                    _ => String::new(),
                },
                name: v
                    .label
                    .as_ref()
                    .map(|l| crate::private::version(l, place).into_owned()),
                summary: None,
                asked: None,
                before: older.map(|o| o.at.map_or_else(|| "—".into(), hm)),
            });
        }
        let pairs: Vec<Pair> = stops.iter().map(|s| s.pair.clone()).collect();
        let first = self
            .version_opened
            .as_ref()
            .and_then(|k| stops.iter().find(|s| &s.key == k))
            .map(|s| s.pair.clone());
        let doc = &self.session.doc;
        self.version_diffs
            .want(&pairs, first.as_ref(), time, || doc.clone());
        self.version_diffs.pump(ctx);
        for s in &mut stops {
            s.summary = self.version_diffs.get(&s.pair);
            s.asked = self.version_diffs.asked(&s.pair);
        }
        stops
    }

    /// The *Version history* card, while it is open.
    pub(crate) fn version_history_dialog(&mut self, ctx: &egui::Context) {
        if !self.versions_open {
            return;
        }
        let versions = self.document_versions();
        let filed = self.in_library();
        let private = crate::private::on();
        let stops = self.timeline(ctx, &versions);
        let time = ctx.input(|i| i.time);
        let done = stops.iter().filter(|s| s.summary.is_some()).count();
        let progress = (done < stops.len()).then_some((done, stops.len()));
        if progress.is_some() {
            // The placeholders pulse.
            ctx.request_repaint();
        }
        let title = format!(
            "{} · {}",
            self.shown_title(),
            crate::ui::count(versions.len(), "version")
        );
        let mut close = false;
        let mut save_named = false;
        let mut restore: Option<PathBuf> = None;
        // A compare button's row, and whether Shift was held.
        let mut compare: Option<(usize, bool)> = None;
        let mut toggle: Option<Later> = None;
        let mut show: Option<Option<usize>> = None;
        let comparing = self.versions_compare;
        let compared: Vec<PathBuf> = self.session.compare.as_ref().map_or(Vec::new(), |c| {
            std::iter::once(c.pick.path.clone())
                .chain(c.newer.as_ref().map(|n| n.pick.path.clone()))
                .collect()
        });
        let picked = self.versions_pick.clone();
        let opened = self.version_opened.clone();
        // What the compare bar names each by: its name and when beside it, or its time.
        let picks: Vec<Pick> = versions
            .iter()
            .enumerate()
            .map(|(place, v)| {
                let when = v.at.map_or_else(|| "Unknown time".into(), when_label);
                match &v.label {
                    Some(label) => Pick {
                        path: v.path.clone(),
                        title: crate::private::version(label, place).into_owned(),
                        when: Some(when),
                    },
                    None => Pick {
                        path: v.path.clone(),
                        title: when,
                        when: None,
                    },
                }
            })
            .collect();
        let mut naming = self.version_naming.take();
        // Private mode draws no field, so a name cannot be typed blind; one open as
        // it came on keeps what was typed.
        if private && let Some(n) = naming.take() {
            self.commit_version_name(n);
        }
        // A name field open at the frame's start takes Escape from the card.
        let was_naming = naming.is_some();
        let mut name: Option<Naming> = None;
        let mut kept: Option<Naming> = None;
        let held_id = egui::Id::new("version-row-held");
        let held: Option<Later> = ctx.data(|d| d.get_temp(held_id));
        let mut held_now: Option<Later> = None;
        let pointed_id = egui::Id::new("version-row-pointed");
        let pointed: Option<Later> = ctx.data(|d| d.get_temp(pointed_id));
        let mut pointed_now: Option<Later> = None;
        let pins = self.prefs.version_history;
        let today = local_day(clock::now());
        let modal = crate::settings::card("version-history", ctx, |ui| {
            ui.set_width(crate::ui::menu_inner_w(CARD_W, crate::settings::PAD));
            if card_head(ui, &title, progress) {
                close = true;
            }
            ui.add_space(10.0);
            egui::ScrollArea::vertical()
                .max_height(LIST_MAX_H)
                .auto_shrink([false, true])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    let mut day: Option<Option<i64>> = None;
                    let mut pending = 0;
                    let last = stops.len() - 1;
                    for (i, s) in stops.iter().enumerate() {
                        if let Some(place) = s.place {
                            let at = versions[place].at;
                            let key = at.map(local_day);
                            if day != Some(key) {
                                day = Some(key);
                                let (head, date) = match at {
                                    Some(at) => day_heading(at, today),
                                    None => ("Unknown time".into(), String::new()),
                                };
                                day_row(ui, &head, &date);
                            }
                        }
                        let editing = s.place.is_some_and(|p| {
                            naming.as_ref().is_some_and(|n| n.path == versions[p].path)
                        });
                        let path = s.place.map(|p| versions[p].path.clone());
                        let marked = path
                            .as_ref()
                            .is_some_and(|p| picked.as_ref() == Some(p) || compared.contains(p));
                        let is_open = opened.as_ref() == Some(&s.key);
                        let out = stop_row(
                            ui,
                            s,
                            Look {
                                held: held.as_ref() == Some(&s.key),
                                opened: is_open,
                                elsewhere: pointed.as_ref().is_some_and(|k| k != &s.key),
                                marked,
                                comparing,
                                private,
                                time,
                                first: i == 0,
                                last: i == last,
                                dashed: versions.is_empty(),
                                pending,
                                naming: naming.as_mut().filter(|_| editing),
                            },
                        );
                        if s.summary.is_none() {
                            pending += 1;
                        }
                        if out.held {
                            held_now = Some(s.key.clone());
                        }
                        if out.pointed {
                            pointed_now = Some(s.key.clone());
                        }
                        if out.named {
                            kept = naming.take();
                        }
                        match out.press {
                            Some(Press::Toggle) => toggle = Some(s.key.clone()),
                            Some(Press::Compare(shift)) => {
                                compare = s.place.map(|p| (p, shift));
                            }
                            Some(Press::Name) => {
                                if let Some(p) = s.place {
                                    name = Some(Naming {
                                        path: versions[p].path.clone(),
                                        text: versions[p].label.clone().unwrap_or_default(),
                                        focused: false,
                                    });
                                }
                            }
                            Some(Press::Restore) => restore = path.clone(),
                            None => {}
                        }
                        if is_open && s.openable() && opened_rows(ui, s, private, time, i == last) {
                            show = Some(s.place);
                        }
                    }
                    if versions.is_empty() {
                        empty_rows(ui, filed, pins);
                    }
                });
            ui.add_space(12.0);
            match card_foot(ui, pins) {
                Some(true) => save_named = true,
                Some(false) => close = true,
                None => {}
            }
        });
        ctx.data_mut(|d| {
            for (id, key) in [(held_id, held_now), (pointed_id, pointed_now)] {
                match key {
                    Some(k) => {
                        d.insert_temp(id, k);
                    }
                    None => d.remove::<Later>(id),
                }
            }
        });
        // Escape in the field is the field's: it surrendered focus this frame, so
        // `name_field` answered and `naming` is gone, and the card stays.
        let escaped = was_naming && ctx.input(|i| i.key_pressed(egui::Key::Escape));
        if let Some(n) = kept.filter(|_| !escaped) {
            self.commit_version_name(n);
        }
        // A press on another row's pencil while a field is open keeps the first name.
        if let Some(n) = name {
            if let Some(open) = naming.take() {
                self.commit_version_name(open);
            }
            naming = Some(n);
        }
        self.version_naming = naming;
        // One row open at a time: opening another closes the first.
        if let Some(key) = toggle {
            self.version_opened = match self.version_opened.as_ref() == Some(&key) {
                true => None,
                false => Some(key),
            };
        }
        let chosen = compare.and_then(|(place, shift)| self.compare_pick(&picks, place, shift));
        // *Show on canvas*: Now against the newest, or a version against the one before.
        let shown = show.and_then(|place| match place {
            None => picks.first().map(|p| (p.clone(), None)),
            Some(p) => picks
                .get(p + 1)
                .map(|older| (older.clone(), Some(picks[p].clone()))),
        });
        let leaving = restore.is_some()
            || chosen.is_some()
            || shown.is_some()
            || close
            || (modal.should_close() && !escaped);
        // Leaving the card keeps what was typed, as a press elsewhere does.
        if leaving && let Some(n) = self.version_naming.take() {
            self.commit_version_name(n);
        }
        if leaving {
            self.versions_open = false;
            self.versions_compare = false;
            self.versions_pick = None;
            self.version_opened = None;
            self.version_diffs.stop();
        }
        if let Some(path) = restore {
            self.restore_version(&path);
        } else if let Some((then, newer)) = chosen.or(shown) {
            self.compare_with(then, newer);
        } else if save_named {
            self.save_named_version();
        }
    }

    /// What a press on row `place`'s compare button asks for, `picks` being every
    /// row's newest first. Plain, that version with now. With Shift, it is the second
    /// of two — after a Shift+click on another row, or another row's compare with now
    /// being open — and answers the older with the newer; else it picks the row and
    /// answers nothing, and a second Shift+click on it drops the pick.
    pub(crate) fn compare_pick(
        &mut self,
        picks: &[Pick],
        place: usize,
        shift: bool,
    ) -> Option<(Pick, Option<Pick>)> {
        let pick = picks.get(place)?;
        if !shift {
            self.versions_pick = None;
            return Some((pick.clone(), None));
        }
        let open = self
            .session
            .compare
            .as_ref()
            .filter(|c| c.newer.is_none())
            .map(|c| c.pick.path.clone());
        let first = self.versions_pick.take().or(open);
        match first.and_then(|p| picks.iter().position(|k| k.path == p)) {
            Some(other) if other == place => None,
            Some(other) => {
                let (newer, older) = (place.min(other), place.max(other));
                Some((picks[older].clone(), Some(picks[newer].clone())))
            }
            None => {
                self.versions_pick = Some(pick.path.clone());
                self.session
                    .info("Shift+click another version's compare to compare the two");
                None
            }
        }
    }

    /// Make the pinned copy at `version` the open document: write and pin what is
    /// open now, then adopt the copy at the same path and save it. Reports what it
    /// did, or why it did nothing.
    pub(crate) fn restore_version(&mut self, version: &Path) {
        self.finish_text_first();
        let loaded = std::fs::read(version)
            .map_err(|e| e.to_string())
            .and_then(|b| ondin_core::io::load(&b).map_err(|e| e.to_string()));
        let mut doc = match loaded {
            Ok(doc) => doc,
            Err(e) => return self.session.fail(format!("Restore failed: {e}")),
        };
        let Some(path) = self.session.path.clone() else {
            return self
                .session
                .fail("Restore failed: this document is not saved");
        };
        // The current state first, written and pinned whatever the version-history
        // preference says: after the adopt it has no other record.
        if !self.save_file(false) {
            return;
        }
        self.library.refresh();
        let pinned = self
            .library
            .entry_at(&path)
            .cloned()
            .map(|entry| store::pin_version(&self.library.root, &entry));
        if !matches!(pinned, Some(Ok(Some(_)))) {
            return self
                .session
                .fail("Restore failed: the current state could not be kept as a version");
        }
        // The file's identity, whatever the copy says.
        let mut meta = doc.meta().clone();
        meta.id = self.session.doc.meta().id.clone();
        doc.set_meta(meta);
        // A restore is not an open: the user keeps looking where they were.
        let (page, camera) = (self.session.page, self.session.camera);
        self.adopt_opened(&path, doc);
        if self.session.page == page {
            self.session.camera = camera;
            self.session.home_pending = false;
        }
        self.session.mark_unsaved();
        if !self.save_file(false) {
            return;
        }
        let when = stamp_of(version).map(when_label).unwrap_or_default();
        self.session.info(format!(
            "Restored the version from {when} — the state before it is the newest version"
        ));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A restore puts the pinned copy's content in the file under the same path and
    /// id, keeps the view where it was, and keeps the state it replaced as the
    /// newest version, so restoring that one comes back.
    /// flip: `restore_version` without its pin of the current state → fails at the
    /// version count; without putting the camera back → fails at "and is not homed".
    #[test]
    fn a_restore_keeps_the_state_it_replaces_as_the_newest_version() {
        let ctx = egui::Context::default();
        let root = std::env::temp_dir().join(format!("ondin-versions-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut app = OndinApp::headless(&ctx);
        app.prefs.base_folder = Some(root.clone());
        app.library = crate::library::state::Library::open(root.clone());
        let name = |app: &OndinApp| app.session.doc.meta().name.clone();
        let rename = |app: &mut OndinApp, to: &str| {
            let mut meta = app.session.doc.meta().clone();
            meta.name = Some(to.into());
            app.session.doc.set_meta(meta);
            app.session.mark_unsaved();
        };
        rename(&mut app, "First");
        assert!(app.save_file(true), "filed and pinned");
        let path = app.session.path.clone().unwrap();
        let id = app.session.doc.meta().id.clone().unwrap();
        rename(&mut app, "Second");
        // Unsaved work at the moment of the restore: it must survive too.
        let first = app.document_versions();
        assert_eq!(first.len(), 1, "the fixture: one pin");
        // Zoomed in on a detail: the restore leaves the view there.
        app.session.home_pending = false;
        app.session.camera.center = ondin_core::kurbo::Point::new(321.0, 123.0);
        app.session.camera.set_zoom(4.0);
        let camera = app.session.camera;

        app.restore_version(&first[0].path);
        assert_eq!(app.session.camera, camera, "the view stays");
        assert!(!app.session.home_pending, "and is not homed");
        assert_eq!(name(&app).as_deref(), Some("First"), "the copy's content");
        assert_eq!(
            app.session.path.as_deref(),
            Some(path.as_path()),
            "same file"
        );
        assert_eq!(
            app.session.doc.meta().id.as_deref(),
            Some(id.as_str()),
            "same id"
        );
        assert!(!app.session.is_dirty(), "saved at once");
        let on_disk = ondin_core::io::load(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(on_disk.meta().name.as_deref(), Some("First"));

        let now = app.document_versions();
        assert_eq!(now.len(), 2, "the replaced state was pinned");
        app.restore_version(&now[0].path);
        assert_eq!(name(&app).as_deref(), Some("Second"), "and comes back");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A copy of a library document opened from outside the library, its id and
    /// all, is not in the library: no versions are listed for it, since `Ctrl+S`
    /// there pins none.
    ///
    /// flip: `in_library` answering for any saved document with an id → fails at
    /// "not in the library".
    #[test]
    fn a_document_outside_the_library_has_no_versions() {
        let ctx = egui::Context::default();
        let base = std::env::temp_dir().join(format!("ondin-versions-out-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (root, outside) = (base.join("library"), base.join("downloads"));
        std::fs::create_dir_all(&outside).unwrap();
        let mut app = OndinApp::headless(&ctx);
        app.prefs.base_folder = Some(root.clone());
        app.library = crate::library::state::Library::open(root.clone());
        assert!(app.save_file(true), "filed and pinned");
        assert!(app.in_library(), "fixture: the original is in the library");
        assert_eq!(app.document_versions().len(), 1, "fixture: one pin");

        let copy = outside.join("brief.ondin");
        std::fs::copy(app.session.path.as_ref().unwrap(), &copy).unwrap();
        let doc = ondin_core::io::load(&std::fs::read(&copy).unwrap()).unwrap();
        assert!(doc.meta().id.is_some(), "fixture: the copy keeps the id");
        app.adopt_opened(&copy, doc);
        assert!(!app.in_library(), "not in the library");
        assert!(app.document_versions().is_empty(), "so no versions");
        let _ = std::fs::remove_dir_all(&base);
    }

    /// An app over a fresh library at a temporary `root`, its document filed there
    /// and no version pinned yet.
    fn filed(ctx: &egui::Context, tag: &str) -> (OndinApp, PathBuf) {
        crate::theme::install(ctx);
        let root =
            std::env::temp_dir().join(format!("ondin-versions-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let mut app = OndinApp::headless(ctx);
        app.prefs.base_folder = Some(root.clone());
        app.library = crate::library::state::Library::open(root.clone());
        assert!(app.save_file(false), "fixture: filed");
        assert!(app.document_versions().is_empty(), "fixture: no pin yet");
        (app, root)
    }

    /// Add a rectangle named `name` to the open document's page, unsaved.
    fn add_rect(app: &mut OndinApp, seed: u64, name: &str) {
        let id = ondin_core::IdSource::new(seed).mint();
        let page = app.session.page;
        app.session
            .doc
            .apply(&ondin_core::Transaction(vec![
                ondin_core::Operation::CreateNode {
                    id,
                    parent: page,
                    index: 0,
                    kind: ondin_core::NodeKind::Rect {
                        size: ondin_core::kurbo::Size::new(20.0, 20.0),
                        corner_radii: Default::default(),
                        corner_smoothing: 0.0,
                    },
                    transform: None,
                    name: Some(name.into()),
                },
            ]))
            .expect("the rectangle");
        app.session.mark_unsaved();
    }

    /// One frame of the card alone, answering the strings it drew.
    fn card_frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        events: Vec<egui::Event>,
    ) -> Vec<String> {
        card_texts(ctx, app, events)
            .into_iter()
            .map(|(t, _)| t)
            .collect()
    }

    /// `card_frame` with where each string was drawn.
    fn card_texts(
        ctx: &egui::Context,
        app: &mut OndinApp,
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Pos2)> {
        let time = ctx.input(|i| i.time) + 0.1;
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1200.0, 800.0),
            )),
            time: Some(time),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| app.version_history_dialog(ui.ctx()))
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_string(), t.pos)),
                _ => None,
            })
            .collect()
    }

    /// A few frames, the summaries worked out between them, answering the last's
    /// strings and where they were drawn.
    fn settled(ctx: &egui::Context, app: &mut OndinApp) -> Vec<(String, egui::Pos2)> {
        card_texts(ctx, app, Vec::new());
        app.version_diffs.settle();
        for _ in 0..3 {
            card_texts(ctx, app, Vec::new());
        }
        card_texts(ctx, app, Vec::new())
    }

    /// Whether `t` is a row's time, `HH:MM`.
    fn is_time(t: &str) -> bool {
        t.len() == 5 && t.as_bytes()[2] == b':'
    }

    /// How far below the card's title each string `keep` takes was drawn: the card
    /// is centred, so it moves as a whole when it grows.
    fn below_title(drawn: &[(String, egui::Pos2)], keep: impl Fn(&str) -> bool) -> Vec<f32> {
        let title = drawn
            .iter()
            .find(|(t, _)| t == "Version history")
            .map(|(_, p)| p.y)
            .expect("the title is drawn");
        drawn
            .iter()
            .filter(|(t, _)| keep(t))
            .map(|(_, p)| p.y - title)
            .collect()
    }

    /// Opening a row's name field moves no row: every time sits where it sat.
    ///
    /// flip: `stop_row` allocating the field's rect instead of placing it over the
    /// first line → fails at "no row moves".
    #[test]
    fn opening_a_name_field_moves_no_row() {
        let ctx = egui::Context::default();
        let (mut app, root) = filed(&ctx, "steady");
        assert!(app.save_file(true), "fixture: one pin");
        assert!(app.pin_saved_version().is_some(), "fixture: two pins");
        let versions = app.document_versions();
        assert_eq!(versions.len(), 2, "fixture: two rows");
        app.versions_open = true;
        let times =
            |drawn: Vec<(String, egui::Pos2)>| below_title(&drawn, |t| is_time(t) || t == "Now");
        let before = times(settled(&ctx, &mut app));
        app.version_naming = Some(Naming {
            path: versions[0].path.clone(),
            text: String::new(),
            focused: false,
        });
        for _ in 0..4 {
            card_texts(&ctx, &mut app, Vec::new());
        }
        let after = times(card_texts(&ctx, &mut app, Vec::new()));
        assert!(app.version_naming.is_some(), "fixture: the field is open");
        assert_eq!(before.len(), 3, "fixture: Now and both rows drawn");
        assert_eq!(before, after, "no row moves");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Each row says what its version changed against the one before: a layer added
    /// is `+1` on its page, a save of nothing new is *Same as* the one before, and
    /// the oldest gives the document's size. Now, saved, has no changes since.
    ///
    /// flip: `Brief::parts` leaving out the counts → fails at "the added layer".
    #[test]
    fn a_row_says_what_its_version_changed() {
        let ctx = egui::Context::default();
        let (mut app, root) = filed(&ctx, "summary");
        assert!(app.save_file(true), "fixture: the first pin");
        // `Ctrl+S` skips a save of nothing new; a named version still pins one.
        assert!(app.pin_saved_version().is_some(), "fixture: the same again");
        add_rect(&mut app, 0x51, "Badge");
        assert!(app.save_file(true), "fixture: a rectangle more");
        let versions = app.document_versions();
        assert_eq!(versions.len(), 3, "fixture: three rows");
        app.versions_open = true;
        let drawn: Vec<String> = settled(&ctx, &mut app)
            .into_iter()
            .map(|(t, _)| t)
            .collect();
        let page = crate::private::page(&app.session.doc, app.session.page).into_owned();
        let same = hm(versions[2].at.unwrap());
        assert!(
            drawn.iter().any(|t| t == &format!("{page} +1")),
            "the added layer: {drawn:?}"
        );
        assert!(
            drawn.iter().any(|t| t == &format!("Same as {same}")),
            "the save of nothing new: {drawn:?}"
        );
        assert!(
            drawn
                .iter()
                .any(|t| t.starts_with("First version · 1 page, ")),
            "the oldest: {drawn:?}"
        );
        assert!(
            drawn.iter().any(|t| t.starts_with("No changes since ")),
            "Now: {drawn:?}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `Ctrl+S` with nothing new since the newest version pins none, and with a
    /// change it pins again.
    ///
    /// flip: `save_file` pinning without asking `saved_is_newest_version` → fails at
    /// "nothing new, no pin".
    #[test]
    fn a_save_of_nothing_new_pins_no_version() {
        let ctx = egui::Context::default();
        let (mut app, root) = filed(&ctx, "nothing-new");
        assert!(app.save_file(true), "fixture: the first pin");
        assert_eq!(app.document_versions().len(), 1, "fixture: one version");
        assert!(app.save_file(true), "the save itself still succeeds");
        assert_eq!(app.document_versions().len(), 1, "nothing new, no pin");
        add_rect(&mut app, 0x5a, "Badge");
        assert!(app.save_file(true), "fixture: a change saved");
        assert_eq!(app.document_versions().len(), 2, "a change pins");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Opening a row lists its changes under it and pushes only the rows below; the
    /// rows above stay where they were.
    ///
    /// flip: `version_history_dialog` drawing `opened_rows` under every openable row
    /// → fails at "the row below is pushed".
    #[test]
    fn opening_a_row_moves_only_the_rows_below() {
        let ctx = egui::Context::default();
        let (mut app, root) = filed(&ctx, "open");
        assert!(app.save_file(true), "fixture: the first pin");
        add_rect(&mut app, 0x52, "Badge");
        assert!(app.save_file(true), "fixture: the second");
        add_rect(&mut app, 0x53, "Chip");
        assert!(app.save_file(true), "fixture: the third");
        let versions = app.document_versions();
        app.versions_open = true;
        let saved = |drawn: &[(String, egui::Pos2)]| below_title(drawn, |t| t == "Saved");
        let before = saved(&settled(&ctx, &mut app));
        assert_eq!(before.len(), 3, "fixture: three rows");
        app.version_opened = Some(Later::Version(versions[1].path.clone()));
        let drawn = settled(&ctx, &mut app);
        let after = saved(&drawn);
        assert_eq!(after[..2], before[..2], "the rows above stay");
        assert!(after[2] > before[2] + 40.0, "the row below is pushed");
        assert!(
            drawn.iter().any(|(t, _)| t.starts_with("Badge")),
            "the change is listed: {drawn:?}"
        );
        assert!(drawn.iter().any(|(t, _)| t == "Show on canvas"));
        let _ = std::fs::remove_dir_all(&root);
    }

    /// One row's buttons at a time: an opened row shows its buttons at rest, and gives
    /// them up to another row under the pointer.
    ///
    /// flip: `stop_row` showing an opened row's buttons whatever `Look::elsewhere`
    /// says → fails at "one row's buttons".
    #[test]
    fn an_opened_row_gives_its_buttons_up_to_the_hovered_one() {
        let ctx = egui::Context::default();
        let (mut app, root) = filed(&ctx, "buttons");
        assert!(app.save_file(true), "fixture: the first pin");
        add_rect(&mut app, 0x56, "Badge");
        assert!(app.save_file(true), "fixture: the second");
        add_rect(&mut app, 0x57, "Chip");
        assert!(app.save_file(true), "fixture: the third");
        let versions = app.document_versions();
        app.versions_open = true;
        app.version_opened = Some(Later::Version(versions[1].path.clone()));
        let drawn = settled(&ctx, &mut app);
        let restores =
            |drawn: &[(String, egui::Pos2)]| drawn.iter().filter(|(t, _)| t == "Restore").count();
        assert_eq!(restores(&drawn), 1, "the opened row's, at rest");
        let (_, top) = drawn
            .iter()
            .find(|(t, _)| t == "Saved")
            .cloned()
            .expect("fixture: the newest row");
        let at = top + egui::vec2(4.0, 4.0);
        let mut last = Vec::new();
        for _ in 0..4 {
            last = card_texts(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        }
        assert_eq!(restores(&last), 1, "one row's buttons");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// *Show on canvas* on an opened row compares its version with the one before
    /// it, the newer standing for Now, and leaves the card.
    ///
    /// flip: `version_history_dialog` answering *Show on canvas* with the version
    /// against now → fails at "with the version before it".
    #[test]
    fn show_on_canvas_compares_a_version_with_the_one_before() {
        let ctx = egui::Context::default();
        let (mut app, root) = filed(&ctx, "show");
        assert!(app.save_file(true), "fixture: the first pin");
        add_rect(&mut app, 0x54, "Badge");
        assert!(app.save_file(true), "fixture: the second");
        add_rect(&mut app, 0x55, "Chip");
        let versions = app.document_versions();
        app.versions_open = true;
        app.version_opened = Some(Later::Version(versions[0].path.clone()));
        let drawn = settled(&ctx, &mut app);
        let (_, at) = drawn
            .iter()
            .find(|(t, _)| t == "Show on canvas")
            .cloned()
            .expect("fixture: the row is open");
        let at = at + egui::vec2(4.0, 4.0);
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        card_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        card_frame(&ctx, &mut app, vec![button(true)]);
        card_frame(&ctx, &mut app, vec![button(false)]);
        let c = app.session.compare.as_ref().expect("a compare opens");
        assert_eq!(c.pick.path, versions[1].path, "with the version before it");
        assert_eq!(
            c.newer.as_ref().map(|n| n.pick.path.clone()),
            Some(versions[0].path.clone()),
            "the version standing for Now"
        );
        assert!(!app.versions_open, "and the card closes");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A collapsed row under the pointer shows one compare, and it compares that
    /// version with now, not with the version before it.
    ///
    /// flip: `version_history_dialog` answering `Press::Compare` with the pair *Show
    /// on canvas* opens → fails at "of the hovered version".
    #[test]
    fn a_rows_one_compare_is_with_now() {
        let ctx = egui::Context::default();
        let (mut app, root) = filed(&ctx, "with-now");
        assert!(app.save_file(true), "fixture: the first pin");
        add_rect(&mut app, 0x58, "Badge");
        assert!(app.save_file(true), "fixture: the second");
        // Unsaved work, so a compare with now would open too.
        add_rect(&mut app, 0x59, "Chip");
        let versions = app.document_versions();
        app.versions_open = true;
        let drawn = settled(&ctx, &mut app);
        let (_, saved) = drawn
            .iter()
            .find(|(t, _)| t == "Saved")
            .cloned()
            .expect("fixture: the newest row");
        let mut drawn = Vec::new();
        for _ in 0..3 {
            drawn = card_texts(
                &ctx,
                &mut app,
                vec![egui::Event::PointerMoved(saved + egui::vec2(4.0, 4.0))],
            );
        }
        let glyph = |g: &str| drawn.iter().filter(|(t, _)| t == g).count();
        assert_eq!(glyph(NOW_GLYPH), 1, "compare with now, collapsed");
        assert_eq!(
            glyph(icon::ARROW_DOWN),
            0,
            "and no compare with the previous"
        );
        let (_, at) = drawn.iter().find(|(t, _)| t == NOW_GLYPH).cloned().unwrap();
        let at = at + egui::vec2(3.0, 3.0);
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        card_frame(&ctx, &mut app, vec![egui::Event::PointerMoved(at)]);
        card_frame(&ctx, &mut app, vec![button(true)]);
        card_frame(&ctx, &mut app, vec![button(false)]);
        let c = app.session.compare.as_ref().expect("a compare opens");
        assert_eq!(c.pick.path, versions[0].path, "of the hovered version");
        assert!(c.newer.is_none(), "with now");
        let _ = std::fs::remove_dir_all(&root);
    }

    fn key(key: egui::Key) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        }
    }

    /// *Save named version…* pins a version even with the version-history
    /// preference off, opens the card on that version's focused name field, and
    /// Enter keeps the name, which the row then leads with.
    ///
    /// flip: `version_history_dialog` dropping the name `name_field` let go of →
    /// fails at "the name is kept".
    #[test]
    fn a_named_version_is_pinned_and_named_through_its_field() {
        let ctx = egui::Context::default();
        let (mut app, root) = filed(&ctx, "named");
        app.prefs.version_history = false;
        app.save_named_version();
        let versions = app.document_versions();
        assert_eq!(versions.len(), 1, "pinned whatever the preference says");
        assert!(app.versions_open, "the card opens");
        let field = egui::Id::new(("version-name", &versions[0].path));
        card_frame(&ctx, &mut app, Vec::new());
        card_frame(&ctx, &mut app, Vec::new());
        assert_eq!(
            ctx.memory(|m| m.focused()),
            Some(field),
            "on its name field"
        );

        card_frame(
            &ctx,
            &mut app,
            vec![egui::Event::Text("Sent to client".into())],
        );
        card_frame(&ctx, &mut app, vec![key(egui::Key::Enter)]);
        let drawn = card_frame(&ctx, &mut app, Vec::new());
        assert_eq!(
            store::version_label(&versions[0].path).as_deref(),
            Some("Sent to client"),
            "the name is kept"
        );
        assert!(app.version_naming.is_none(), "the field closes");
        assert!(app.versions_open, "and the card stays");
        assert!(
            drawn.iter().any(|t| t == "Sent to client"),
            "the row leads with it"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Escape in the name field drops what was typed and closes only the field; a
    /// second Escape closes the card.
    ///
    /// flip: `version_history_dialog` letting the card close on the field's Escape
    /// → fails at "the card stays".
    #[test]
    fn escape_in_the_name_field_closes_only_the_field() {
        let ctx = egui::Context::default();
        let (mut app, root) = filed(&ctx, "escape");
        app.save_named_version();
        let path = app.document_versions()[0].path.clone();
        card_frame(&ctx, &mut app, Vec::new());
        card_frame(&ctx, &mut app, Vec::new());
        card_frame(&ctx, &mut app, vec![egui::Event::Text("Draft".into())]);
        assert!(app.version_naming.is_some(), "fixture: the field is open");
        card_frame(&ctx, &mut app, vec![key(egui::Key::Escape)]);
        card_frame(&ctx, &mut app, Vec::new());
        assert_eq!(store::version_label(&path), None, "nothing typed is kept");
        assert!(app.version_naming.is_none(), "the field closes");
        assert!(app.versions_open, "the card stays");
        card_frame(&ctx, &mut app, vec![key(egui::Key::Escape)]);
        assert!(!app.versions_open, "a second Escape closes it");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// In private mode a version's name is drawn as its place, and no name field
    /// opens, so a name cannot be typed blind; one open as it came on is kept.
    ///
    /// flip: `timeline` naming the row by `v.label` without `private::version` →
    /// fails at "the name is masked".
    #[test]
    fn private_mode_masks_a_versions_name() {
        let ctx = egui::Context::default();
        let (mut app, root) = filed(&ctx, "private");
        assert!(app.save_file(true), "fixture: one pin");
        let path = app.document_versions()[0].path.clone();
        store::set_version_label(&path, "Acme pitch").unwrap();
        app.versions_open = true;
        crate::private::set(true);
        app.version_naming = Some(Naming {
            path: path.clone(),
            text: "Acme".into(),
            focused: false,
        });
        let mut drawn = Vec::new();
        for _ in 0..2 {
            drawn.extend(card_frame(&ctx, &mut app, Vec::new()));
        }
        crate::private::set(false);
        assert!(
            !drawn.iter().any(|t| t.contains("Acme")),
            "the name is masked: {drawn:?}"
        );
        assert!(drawn.iter().any(|t| t == "Version 1"), "drawn as its place");
        assert!(app.version_naming.is_none(), "no field opens");
        assert_eq!(
            store::version_label(&path).as_deref(),
            Some("Acme"),
            "the field open as it came on keeps what was typed"
        );
        let _ = std::fs::remove_dir_all(&root);
    }
}
