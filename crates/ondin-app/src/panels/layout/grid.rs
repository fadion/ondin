//! Grid's rows of the **Container** and **Item** cards (§15 D920), from the
//! mockup's screen 04 (`design/Layout Cards.dc.html`).
//!
//! **Container**, under `display: grid`: `grid-auto-flow`, the two track lists —
//! each track a row leading with its kind, a function's arguments as fields of
//! their own, a grip to reorder and a cross to remove, and under the list the
//! whole template as editable CSS for pasting — then `justify-content`,
//! `justify-items`, `align-items`, `align-content`, the gaps and the padding.
//! **Item**, in a grid: `grid-column` and `grid-row` as CSS writes them, start
//! and end, then `justify-self` and `align-self`.
//!
//! Four things differ from flex's rows on purpose:
//!
//! - **`Start` and `End` read *Start* and *End***, not *Flex start*: a grid has no
//!   reversal for `flex-start` to follow, and CSS treats the two alike there (§15
//!   D913's session call).
//! - **`justify-items`, and `normal`**: the first is not in the mockup, and is
//!   the one way to stop every item stretching sideways (§15 D914); the second is
//!   CSS's default for both items rows, holding a shape at the start of its cell
//!   (§15 D915).
//! - **`baseline` on the align rows only**: a grid draws it on its block axis —
//!   taffy shims a row's items to their first baselines, and Chrome agrees — and
//!   treats it as `start` across, its own TODO being the inline axis's alone. So
//!   `align-items` and `align-self` offer it and name it *Baseline* (§15 D939),
//!   and the justify rows leave it out; a justify row **holding** it, carried
//!   from a file, reads *Baseline · Start* with Start's picture, on a row of its
//!   own that is lit: what is held, then what is drawn (§15 D923). Until D939 all
//!   four rows left it out, on the TODO's word (§15 D919).
//! - **Every line is typed as CSS** — `auto`, `2`, `-1`, `span 2` — because a line
//!   is a number *or* a span, and one field that reads both is shorter than two
//!   that each read half.

use super::*;
use ondin_core::container::{
    GridAutoFlow, GridLines, Track, TrackBreadth, TrackSize, parse_placement, parse_tracks,
    placement_css, track_count, track_size_css, tracks_css,
};

/// A grid item's `justify-self`, in the card — `baseline` left out, the inline
/// axis treating it as `start` (§15 D919, D939); `align-self` is [`ALIGN_SELF`].
const SELF: [Option<AlignItems>; 4] = [
    Some(AlignItems::Stretch),
    Some(AlignItems::Start),
    Some(AlignItems::End),
    Some(AlignItems::Center),
];

/// A grid container's `justify-items`: `normal` first, the default (§15 D915),
/// then [`SELF`]'s four; `align-items` is [`ALIGN_ITEMS`].
const ITEMS: [Option<AlignItems>; 5] = [
    None,
    Some(AlignItems::Stretch),
    Some(AlignItems::Start),
    Some(AlignItems::End),
    Some(AlignItems::Center),
];

/// [`ITEMS`] for `align-items` — **with `baseline`, which a grid draws on its
/// block axis** (§15 D939): taffy shims a row's items to their first baselines
/// before it aligns them, and since a text leaf reports its baseline that lines
/// up letters. Only its inline axis treats `baseline` as `start`, so the justify
/// rows go on leaving it out.
const ALIGN_ITEMS: [Option<AlignItems>; 6] = [
    None,
    Some(AlignItems::Stretch),
    Some(AlignItems::Start),
    Some(AlignItems::End),
    Some(AlignItems::Center),
    Some(AlignItems::Baseline),
];

/// [`SELF`] for `align-self`, with `baseline` — [`ALIGN_ITEMS`]' reason.
const ALIGN_SELF: [Option<AlignItems>; 5] = [
    Some(AlignItems::Stretch),
    Some(AlignItems::Start),
    Some(AlignItems::End),
    Some(AlignItems::Center),
    Some(AlignItems::Baseline),
];

/// The pictures for the horizontal rows — `justify-*` — turned through the
/// diagonal: every alignment picture is drawn for a row's cross axis, which is
/// vertical, and a grid's `justify` runs across.
const ACROSS: Orient = Orient {
    transpose: true,
    flip_x: false,
    flip_y: false,
};

/// What each `grid-auto-flow` cell does, for its tooltip (§15 D886's rule).
const FLOW_TIPS: [&str; 2] = [
    "Row — auto-placed items fill each row before the next",
    "Column — auto-placed items fill each column before the next",
];

/// An item alignment as the **justify** rows name it: `normal` for none set, and
/// a held `baseline` as what it is and what grid draws for it across —
/// *Baseline · Start*, the face's *Auto · Normal* form (§15 D923). The align rows
/// name it plainly ([`align_items_name`]), the block axis drawing it (§15 D939).
fn items_name(a: Option<AlignItems>) -> &'static str {
    match a {
        None => "Normal",
        Some(AlignItems::Start) => "Start",
        Some(AlignItems::End) => "End",
        Some(AlignItems::Baseline) => "Baseline · Start",
        Some(a) => align_name(a),
    }
}

/// `normal`'s picture is `stretch`'s, which is what it does to every box; a
/// shape it holds at the start is the exception the tooltip names. A held
/// `baseline` wears `start`'s, which is what taffy's grid draws (§15 D923).
fn items_glyph(a: Option<AlignItems>) -> Glyph {
    match a {
        Some(AlignItems::Baseline) => Glyph::Align(AlignItems::Start),
        a => Glyph::Align(a.unwrap_or(AlignItems::Stretch)),
    }
}

/// [`items_name`] for the **align** rows: `baseline` is *Baseline*, since a
/// grid's block axis draws it (§15 D939). This read *Baseline · Start* on both
/// axes, from taffy's TODO rather than from a layout — the TODO is the inline
/// axis's alone, and Chrome draws the block axis the same way taffy does.
fn align_items_name(a: Option<AlignItems>) -> &'static str {
    match a {
        Some(AlignItems::Baseline) => align_name(AlignItems::Baseline),
        a => items_name(a),
    }
}

/// [`items_glyph`] for the align rows: `baseline`'s own picture.
fn align_items_glyph(a: Option<AlignItems>) -> Glyph {
    match a {
        Some(AlignItems::Baseline) => Glyph::Align(AlignItems::Baseline),
        a => items_glyph(a),
    }
}

/// A content distribution as the grid rows name it — *Start*, *End*.
fn grid_content_name(c: AlignContent) -> &'static str {
    match c {
        AlignContent::Start => "Start",
        AlignContent::End => "End",
        c => content_name(c),
    }
}

/// What one track row is, for the kind menu that leads it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Kind {
    Px,
    Percent,
    Fr,
    Auto,
    MinContent,
    MaxContent,
    MinMax,
    Repeat,
}

const KINDS: [Kind; 8] = [
    Kind::Px,
    Kind::Fr,
    Kind::Percent,
    Kind::Auto,
    Kind::MinContent,
    Kind::MaxContent,
    Kind::MinMax,
    Kind::Repeat,
];

impl Kind {
    fn of(t: &Track) -> Kind {
        match t {
            Track::Repeat { .. } => Kind::Repeat,
            Track::Size(TrackSize::MinMax { .. }) => Kind::MinMax,
            Track::Size(TrackSize::Breadth(b)) => Kind::of_breadth(*b),
        }
    }

    fn of_breadth(b: TrackBreadth) -> Kind {
        match b {
            TrackBreadth::Px(_) => Kind::Px,
            TrackBreadth::Percent(_) => Kind::Percent,
            TrackBreadth::Fr(_) => Kind::Fr,
            TrackBreadth::Auto => Kind::Auto,
            TrackBreadth::MinContent => Kind::MinContent,
            TrackBreadth::MaxContent => Kind::MaxContent,
        }
    }

    fn word(self) -> &'static str {
        match self {
            Kind::Px => "px",
            Kind::Percent => "%",
            Kind::Fr => "fr",
            Kind::Auto => "auto",
            Kind::MinContent => "min-content",
            Kind::MaxContent => "max-content",
            Kind::MinMax => "minmax",
            Kind::Repeat => "repeat",
        }
    }

    fn describe(self) -> &'static str {
        match self {
            Kind::Px => "A fixed length",
            Kind::Percent => "A share of the grid's content box",
            Kind::Fr => "A share of the free space, by weight",
            Kind::Auto => "As wide as its content, and grows into free space",
            Kind::MinContent => "As narrow as its content can go",
            Kind::MaxContent => "As wide as its content wants",
            Kind::MinMax => "Between a minimum and a maximum",
            Kind::Repeat => "The same tracks, several times",
        }
    }

    /// A breadth of this kind — `number` where it takes one, else the kind's
    /// default. `None` for the two function kinds.
    fn breadth(self, number: Option<f64>) -> Option<TrackBreadth> {
        Some(match self {
            Kind::Px => TrackBreadth::Px(number.unwrap_or(100.0)),
            Kind::Percent => TrackBreadth::Percent(number.unwrap_or(25.0)),
            Kind::Fr => TrackBreadth::Fr(number.unwrap_or(1.0)),
            Kind::Auto => TrackBreadth::Auto,
            Kind::MinContent => TrackBreadth::MinContent,
            Kind::MaxContent => TrackBreadth::MaxContent,
            Kind::MinMax | Kind::Repeat => return None,
        })
    }
}

/// A breadth's number, where it has one.
fn number_of(b: TrackBreadth) -> Option<f64> {
    match b {
        TrackBreadth::Px(v) | TrackBreadth::Percent(v) | TrackBreadth::Fr(v) => Some(v),
        _ => None,
    }
}

/// `t` turned into a track of kind `k`, keeping what carries over: **the number
/// between px, % and fr is not kept** — 200px is not 200fr — and each kind starts
/// at its default instead; a single `fr` becomes `minmax()`'s maximum over a
/// 100px minimum and any other single size its minimum under `1fr`, and a single
/// size becomes `repeat()`'s track; a function's lead size — a `repeat()`'s first
/// track, a `minmax()`'s maximum — comes back out where it is of kind `k`.
fn converted(t: &Track, k: Kind) -> Track {
    let size = match t {
        Track::Size(s) => *s,
        Track::Repeat { tracks, .. } => tracks
            .first()
            .copied()
            .unwrap_or(TrackSize::Breadth(TrackBreadth::Auto)),
    };
    let lead = match size {
        TrackSize::Breadth(b) => b,
        TrackSize::MinMax { max, .. } => max,
    };
    let same = |b: TrackBreadth| (Kind::of_breadth(b) == k).then_some(b);
    match k {
        Kind::MinMax => Track::Size(match size {
            m @ TrackSize::MinMax { .. } => m,
            TrackSize::Breadth(b @ TrackBreadth::Fr(_)) => TrackSize::MinMax {
                min: TrackBreadth::Px(100.0),
                max: b,
            },
            TrackSize::Breadth(b) => TrackSize::MinMax {
                min: b,
                max: TrackBreadth::Fr(1.0),
            },
        }),
        Kind::Repeat => Track::Repeat {
            repeat: 2,
            tracks: vec![size],
        },
        _ => Track::Size(TrackSize::Breadth(
            same(lead)
                .or_else(|| k.breadth(None))
                .unwrap_or(TrackBreadth::Auto),
        )),
    }
}

/// What a [`breadth_field`] did this frame — [`SizeEdit`]'s shape for a track.
struct BreadthEdit {
    resp: egui::Response,
    picked: Option<TrackBreadth>,
    typed: Option<TrackBreadth>,
}

/// One breadth — a `minmax()` end, a `repeat()`'s track — as a value field whose
/// unit is a menu of the breadth kinds ([`size_field`]'s shape): the number in
/// px, % or fr, or a keyword standing in the digits with no unit (§15 D906's
/// rule), where typing a number leaves it for px and brings the menu back — the
/// way from one keyword to another, as it is for the Item card's basis. `fr` is
/// offered only where it may go — never as a `minmax()` minimum.
///
/// ⚠️ **The first cut drew a clickable `–` under a keyword while this doc cited
/// D906**, which is the unit that entry's ruling removed; found by `arch-scribe`
/// reading the two against each other, and dropped (§15 D920's amendment).
fn breadth_field(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    prefix: Prefix,
    b: TrackBreadth,
    fr: bool,
) -> BreadthEdit {
    let kind = Kind::of_breadth(b);
    let number = number_of(b);
    let mut v = number.unwrap_or(0.0);
    let start = v;
    let word = number.is_none().then(|| kind.word());
    let (resp, unit) = ui::value_field_unit(
        ui,
        size,
        prefix,
        // No unit under a keyword (§15 D906).
        word.is_none().then_some(Suffix {
            text: kind.word(),
            clickable: true,
            tooltip: "What this size is",
        }),
        &mut v,
        match kind {
            Kind::Fr => Scrub::fine(0.05, 2).range(0.0..=f64::MAX),
            Kind::Percent => Scrub::fine(0.25, 1).range(0.0..=f64::MAX),
            _ => Scrub::whole(0.5).range(0.0..=f64::MAX),
        },
        |d| match word {
            Some(w) => d.custom_formatter(move |_, _| w.into()),
            None => d.custom_formatter(ui::number(2)),
        },
    );
    let mut picked = None;
    if let Some(unit) = unit {
        egui::Popup::menu(&unit)
            .align(egui::RectAlign::BOTTOM_END)
            .show(|ui| {
                ui::menu_rows(ui);
                ui.spacing_mut().button_padding = egui::vec2(8.0, 2.0);
                for k in KINDS
                    .into_iter()
                    .filter(|k| !matches!(k, Kind::MinMax | Kind::Repeat))
                    .filter(|k| fr || *k != Kind::Fr)
                {
                    // **The lit row picks nothing** (§15 D941, the release
                    // review's `[X6.3-L1-02]`) — `track_row`'s kind menu already
                    // said so; this one reset `minmax(200px, 1fr)` to
                    // `minmax(100px, 1fr)` on a click of its own `px`.
                    if ui
                        .selectable_label(k == kind, k.word())
                        .on_hover_text(k.describe())
                        .clicked()
                        && k != kind
                    {
                        picked = k.breadth(None);
                    }
                }
            });
    }
    // **Under a keyword, a keystroke is an edit too** (§15 D941, `[X6.3-L1-03]`):
    // the hidden number there is 0, so a typed `0` never moved it and
    // `minmax(0, 1fr)` — CSS's idiom for an `fr` track that may shrink below its
    // content — could not be typed. A click in and out types nothing, so D885's
    // rule holds. **Only what can be part of a number counts**: a letter typed
    // over `auto` leaves the hidden 0 in place, and counting it wrote `0px`.
    let keystroke = word.is_some()
        && resp.has_focus()
        && ui.input(|i| {
            i.events.iter().any(|e| {
                matches!(e, egui::Event::Text(t)
                    if !t.is_empty() && t.chars().all(|c| c.is_ascii_digit() || c == '.'))
            })
        });
    let typed = edited(&resp, v != start || keystroke).then(|| match kind {
        Kind::Percent => TrackBreadth::Percent(v.max(0.0)),
        Kind::Fr => TrackBreadth::Fr(v.max(0.0)),
        _ => TrackBreadth::Px(v.max(0.0)),
    });
    BreadthEdit {
        resp,
        picked,
        typed,
    }
}

/// A single-line CSS text field that **commits on the way out and never on
/// `Escape`** (`ui::defocus_commits`, §15 D841): the text is held in a buffer
/// while it has focus, so a half-typed value is not a live one, and handed back
/// only on a defocus that commits and only when it differs from what was shown.
/// `shown` is `None` for a selection that disagrees — the field reads empty,
/// with "Mixed" as its hint.
fn css_field(
    ui: &mut egui::Ui,
    key: egui::Id,
    size: egui::Vec2,
    shown: Option<&str>,
    tip: &str,
) -> Option<String> {
    let held = ui.ctx().data(|d| d.get_temp::<String>(key));
    let mut text = held.unwrap_or_else(|| shown.unwrap_or_default().to_owned());
    let resp = ui::text_field(
        ui,
        size,
        &mut text,
        if shown.is_none() { ui::MIXED_WORD } else { "" },
        11.0,
    )
    .on_hover_text(tip);
    ui::select_all_on_focus(ui, &resp, &text);
    if resp.has_focus() {
        ui.ctx().data_mut(|d| d.insert_temp(key, text.clone()));
    }
    // A refusal is about the text that was refused, and a new edit is a new try.
    if resp.gained_focus() {
        ui.ctx()
            .data_mut(|d| d.remove::<Refused>(key.with(REFUSED)));
    }
    let mut out = None;
    // The buffer goes on every way out, `Escape` included — a cancelled edit
    // must not sit in the field for the next look. Only the write is gated.
    if resp.lost_focus() {
        ui.ctx().data_mut(|d| d.remove::<String>(key));
        // **Against what the field was seeded with**, not against `shown` (§15
        // D941, the release review's `[X6.3-L1-01]`): a mixed selection seeds it
        // empty with `shown` `None`, and `Some("") != None` read the untouched
        // buffer as typed — a click in and out of a *Mixed* template line wrote
        // `none` to every selected grid, and an item's line field raised a
        // refusal nobody typed. D885's rule: a click in and out writes nothing.
        if ui::defocus_commits(&resp) && text != shown.unwrap_or_default() {
            out = Some(text);
        }
    }
    out
}

/// Why the last text typed into a [`css_field`] was not applied, and what the
/// field read when it was refused — `(shown, why)`.
type Refused = (Option<String>, String);

/// The salt of a [`css_field`]'s refusal, under the field's own key.
const REFUSED: &str = "refused";

/// Remember that the text typed into the field at `key` was refused, `why`, while
/// the field read `shown`.
fn refuse(ui: &egui::Ui, key: egui::Id, shown: Option<&str>, why: String) {
    let r: Refused = (shown.map(str::to_owned), why);
    ui.ctx().data_mut(|d| d.insert_temp(key.with(REFUSED), r));
}

/// The refusal to show under the field at `key`, **only while the field still
/// reads what it read when the text was refused** — an apply through any other
/// door (a `+`, a cross, an undo) makes it about a list no longer there — and
/// never once the field has been engaged again ([`css_field`] drops it).
/// `arch-scribe` read the first cut's error staying up through all of those.
fn refusal(ui: &egui::Ui, key: egui::Id, shown: Option<&str>) -> Option<String> {
    let (about, why) = ui
        .ctx()
        .data(|d| d.get_temp::<Refused>(key.with(REFUSED)))?;
    (about.as_deref() == shown).then_some(why)
}

/// A grid track list with `list` in place of the one `rows` names.
fn with_list(g: &mut Grid, rows: bool, list: &[Track]) {
    if rows {
        g.rows = list.to_vec();
    } else {
        g.columns = list.to_vec();
    }
}

/// `list` with entry `from` dropped at slot `to` — a slot being the gap before
/// entry `to`, `list.len()` the end. `None` where that is where it already is:
/// the slot before it and the slot after it both leave the list as it was.
fn reordered(list: &[Track], from: usize, to: usize) -> Option<Vec<Track>> {
    // `to` counts the entry being moved; taken out first, it lands one earlier
    // when the slot was below it.
    let at = if to > from { to - 1 } else { to };
    if at == from || from >= list.len() {
        return None;
    }
    let mut moved = list.to_vec();
    let t = moved.remove(from);
    moved.insert(at.min(moved.len()), t);
    Some(moved)
}

/// The grip reorder a track list is in the middle of: which list (by `rows`),
/// and the entry picked up.
#[derive(Clone, Copy, Debug, PartialEq)]
struct TrackDrag {
    rows: bool,
    from: usize,
}

impl OndinApp {
    /// Every subject's grid with `f` applied — [`Self::display_tx`] for a grid.
    fn grid_tx(&self, subjects: &[NodeId], f: impl Fn(&mut Grid)) -> Transaction {
        self.display_tx(subjects, |d| {
            if let Display::Grid(g) = d {
                f(g);
            }
        })
    }

    /// The rows under `display: grid` — the mockup's order, with `justify-items`
    /// after `justify-content` (§15 D914): the flow, the two track lists, the four
    /// alignments, the gaps, the padding. Not `grid_rows`, which is the layout
    /// grids' (§5.3b's chrome) in `panels::inspector` — the name "grid" is two
    /// things in this project, and this is the CSS one.
    pub(super) fn grid_container_rows(
        &mut self,
        ui: &mut egui::Ui,
        subjects: &[NodeId],
        grids: &[Grid],
        laid: &[Display],
    ) {
        if grids.is_empty() {
            return;
        }
        let full = ui.available_width();

        // --- grid-auto-flow ------------------------------------------------------
        let flow = shared(grids, |g| g.auto_flow);
        let flows = [GridAutoFlow::Row, GridAutoFlow::Column];
        let at = flow
            .and_then(|f| flows.iter().position(|x| *x == f))
            .unwrap_or(flows.len());
        if let Some(i) = ui::segmented_tipped(
            ui,
            full,
            ui::SEGMENT_CELL_H,
            2,
            at,
            |_| true,
            |i| FLOW_TIPS[i],
            |p, i, r, on| match flow {
                None if i == 0 => ui::segment_mixed(p, r),
                _ => ui::segment_label(p, r, ["Row", "Column"][i], on),
            },
        ) {
            let tx = self.grid_tx(subjects, |g| g.auto_flow = flows[i]);
            self.commit_edit(tx);
        }

        // --- the tracks ------------------------------------------------------------
        self.track_list(ui, subjects, grids, false, full);
        self.track_list(ui, subjects, grids, true, full);

        // --- the alignments ---------------------------------------------------------
        let content = |c: AlignContent| Glyph::Content(c);
        if let Some(Some(c)) = glyph_combo(
            ui,
            "grid-justify-content",
            "Justify content",
            full,
            shared(grids, |g| Some(g.justify_content)),
            &CONTENT,
            grid_content_name,
            content,
            |_| ACROSS,
            None,
            true,
            None,
        ) {
            let tx = self.grid_tx(subjects, |g| g.justify_content = c);
            self.commit_edit(tx);
        }
        if let Some(Some(a)) = glyph_combo(
            ui,
            "grid-justify-items",
            "Justify items",
            full,
            shared(grids, |g| Some(g.justify_items)),
            &ITEMS,
            items_name,
            items_glyph,
            |_| ACROSS,
            None,
            true,
            None,
        ) {
            let tx = self.grid_tx(subjects, |g| g.justify_items = a);
            self.commit_edit(tx);
        }
        if let Some(Some(a)) = glyph_combo(
            ui,
            "grid-align-items",
            "Align items",
            full,
            shared(grids, |g| Some(g.align_items)),
            &ALIGN_ITEMS,
            align_items_name,
            align_items_glyph,
            |_| Orient::default(),
            None,
            true,
            None,
        ) {
            let tx = self.grid_tx(subjects, |g| g.align_items = a);
            self.commit_edit(tx);
        }
        if let Some(Some(c)) = glyph_combo(
            ui,
            "grid-align-content",
            "Align content",
            full,
            shared(grids, |g| Some(g.align_content)),
            &CONTENT,
            grid_content_name,
            content,
            |_| Orient::default(),
            None,
            true,
            None,
        ) {
            let tx = self.grid_tx(subjects, |g| g.align_content = c);
            self.commit_edit(tx);
        }

        // --- the gaps and the padding: columns first, both always live ------------
        self.gap_row(ui, subjects, laid, true, true, full);
        self.padding_rows(ui, subjects, laid, full);
    }

    /// One track list — `grid-template-columns`, or `-rows` when `rows` — as the
    /// mockup draws it: a head with the property's name, how many tracks it makes
    /// and a `+`; a row per entry while the selection agrees on the list; and the
    /// whole list as CSS under it, which is also how a selection that disagrees is
    /// edited (it reads *Mixed* and takes a typed or pasted list for all of them).
    fn track_list(
        &mut self,
        ui: &mut egui::Ui,
        subjects: &[NodeId],
        grids: &[Grid],
        rows: bool,
        full: f32,
    ) {
        let get = |g: &Grid| -> Vec<Track> {
            if rows {
                g.rows.clone()
            } else {
                g.columns.clone()
            }
        };
        let list = get(&grids[0]);
        let agreed = grids.iter().all(|g| get(g) == list);
        let side = ui::CONTROL_H;

        // --- the head ----------------------------------------------------------
        ui.horizontal(|ui| {
            ui.label(
                egui::RichText::new(if rows {
                    "Grid template rows"
                } else {
                    "Grid template columns"
                })
                .size(11.0)
                .color(theme::text::DIM),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let add = ui::icon_button(ui, icon::PLUS, 20.0, 13.0, false, true).on_hover_text(
                    if rows {
                        "Add a row track"
                    } else {
                        "Add a column track"
                    },
                );
                if add.clicked() {
                    let one = Track::Size(TrackSize::Breadth(TrackBreadth::Fr(1.0)));
                    let tx = self.grid_tx(subjects, |g| {
                        let mut l = get(g);
                        l.push(one.clone());
                        with_list(g, rows, &l);
                    });
                    self.commit_edit(tx);
                }
                let n = track_count(&list);
                ui.label(
                    egui::RichText::new(match (agreed, n) {
                        (false, _) => ui::MIXED_WORD.to_owned(),
                        (true, 0) => "None".to_owned(),
                        (true, 1) => "1 track".to_owned(),
                        (true, n) => format!("{n} tracks"),
                    })
                    .size(10.5)
                    .color(theme::text::FAINT),
                );
            });
        });

        // --- a row per entry --------------------------------------------------------
        if agreed {
            let drag_key = egui::Id::new(("grid-track-drag", subjects.first().copied()));
            let dragging = ui
                .ctx()
                .data(|d| d.get_temp::<TrackDrag>(drag_key))
                .filter(|d| d.rows == rows);
            let mut rects = Vec::with_capacity(list.len());
            let mut released = false;
            for i in 0..list.len() {
                let (rect, grip) = self.track_row(ui, subjects, rows, &list, i);
                rects.push(rect);
                if grip.drag_started() {
                    ui.ctx()
                        .data_mut(|d| d.insert_temp(drag_key, TrackDrag { rows, from: i }));
                }
                released |= grip.drag_stopped();
            }
            // A drag that ended where no grip saw it — the selection changed under
            // it, or the list turned mixed — is over once no button is held;
            // otherwise its line would follow the pointer the next time this list
            // was drawn (`arch-scribe`'s reading of the first cut).
            let dragging = dragging.filter(|_| {
                let held = ui.input(|i| i.pointer.any_down());
                if !held && !released {
                    ui.ctx().data_mut(|d| d.remove::<TrackDrag>(drag_key));
                }
                held || released
            });
            if let Some(drag) = dragging {
                // Where the entry would land: before the first row whose middle the
                // pointer is above, or at the end — drawn as a line between rows.
                let y = ui.ctx().pointer_latest_pos().map_or(f32::INFINITY, |p| p.y);
                let to = rects
                    .iter()
                    .position(|r| y < r.center().y)
                    .unwrap_or(rects.len());
                let line_y = match rects.get(to) {
                    Some(r) => r.top() - 2.0,
                    None => rects.last().map_or(0.0, |r| r.bottom() + 2.0),
                };
                if let Some(first) = rects.first() {
                    ui.painter().hline(
                        first.x_range(),
                        line_y,
                        egui::Stroke::new(1.5, color::ACCENT),
                    );
                }
                if released {
                    ui.ctx().data_mut(|d| d.remove::<TrackDrag>(drag_key));
                    if let Some(moved) = reordered(&list, drag.from, to) {
                        let tx = self.grid_tx(subjects, |g| with_list(g, rows, &moved));
                        self.commit_edit(tx);
                    }
                }
            }
        }

        // --- the CSS line --------------------------------------------------------
        let key = egui::Id::new(("grid-track-css", rows, subjects.first().copied()));
        let shown = agreed.then(|| tracks_css(&list));
        if let Some(text) = css_field(
            ui,
            key,
            egui::vec2(full, side),
            shown.as_deref(),
            "The whole list as CSS — type or paste a template, Enter to apply",
        ) {
            match parse_tracks(&text) {
                Ok(parsed)
                    if Grid {
                        columns: parsed.clone(),
                        ..Grid::default()
                    }
                    .is_valid() =>
                {
                    let tx = self.grid_tx(subjects, |g| with_list(g, rows, &parsed));
                    self.commit_edit(tx);
                }
                Ok(_) => refuse(
                    ui,
                    key,
                    shown.as_deref(),
                    "CSS does not accept that list".into(),
                ),
                Err(e) => refuse(ui, key, shown.as_deref(), e),
            }
        }
        if let Some(why) = refusal(ui, key, shown.as_deref()) {
            ui.label(egui::RichText::new(why).size(10.5).color(color::DANGER));
        }
    }

    /// Entry `i` of a track list: the grip, the kind menu, the kind's fields and
    /// the cross. Hands back the row's rect and the grip's response, for the
    /// list's reorder.
    fn track_row(
        &mut self,
        ui: &mut egui::Ui,
        subjects: &[NodeId],
        rows: bool,
        list: &[Track],
        i: usize,
    ) -> (egui::Rect, egui::Response) {
        let gap = ui::CARD_COL_GAP;
        let side = ui::CONTROL_H;
        let track = list[i].clone();
        let kind = Kind::of(&track);
        let put = |t: Track| -> Vec<Track> {
            let mut l = list.to_vec();
            l[i] = t;
            l
        };
        let row = ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let grip = ui::grip(ui, list.len() > 1, side);
            // The kind menu, the mockup's leading chip.
            let mut picked = None;
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = side;
                egui::ComboBox::from_id_salt(("grid-track-kind", rows, i))
                    .icon(ui::combo_chevron)
                    .width(74.0)
                    .selected_text(egui::RichText::new(kind.word()).size(11.5))
                    .show_ui(ui, |ui| {
                        ui::menu_rows(ui);
                        for k in KINDS {
                            if ui
                                .selectable_label(k == kind, k.word())
                                .on_hover_text(k.describe())
                                .clicked()
                            {
                                picked = Some(k);
                            }
                        }
                    });
            });
            if let Some(k) = picked.filter(|k| *k != kind) {
                let l = put(converted(&track, k));
                let tx = self.grid_tx(subjects, |g| with_list(g, rows, &l));
                self.commit_edit(tx);
            }
            // The fields, in what is left before the cross.
            let room = (ui.available_width() - side - gap).max(40.0);
            self.track_fields(ui, subjects, rows, list, i, &track, room);
            let remove = ui::icon_button(ui, icon::X, side, 13.0, false, true)
                .on_hover_text("Remove this track");
            if remove.clicked() {
                let mut l = list.to_vec();
                l.remove(i);
                let tx = self.grid_tx(subjects, |g| with_list(g, rows, &l));
                self.commit_edit(tx);
            }
            grip
        });
        (row.response.rect, row.inner)
    }

    /// The fields of entry `i`, `track`, in `room` points: a number for a length,
    /// a share or `fr`; nothing for a keyword; `minmax()`'s two ends; `repeat()`'s
    /// count and its track — or, for a repeat of several, the tracks as CSS, which
    /// the list's CSS line edits.
    #[allow(clippy::too_many_arguments)]
    fn track_fields(
        &mut self,
        ui: &mut egui::Ui,
        subjects: &[NodeId],
        rows: bool,
        list: &[Track],
        i: usize,
        track: &Track,
        room: f32,
    ) {
        let gap = ui::CARD_COL_GAP;
        let side = ui::CONTROL_H;
        let commit_list = |app: &mut Self, t: Track| {
            let mut l = list.to_vec();
            l[i] = t;
            app.grid_tx(subjects, |g| with_list(g, rows, &l))
        };
        match track {
            Track::Size(TrackSize::Breadth(b)) => match number_of(*b) {
                Some(n) => {
                    let mut v = n;
                    let kind = Kind::of_breadth(*b);
                    let (resp, _) = ui::value_field_suffixed(
                        ui,
                        egui::vec2(room, side),
                        Prefix::Text(""),
                        Some(Suffix {
                            text: kind.word(),
                            clickable: false,
                            tooltip: "",
                        }),
                        &mut v,
                        match kind {
                            Kind::Fr => Scrub::fine(0.05, 2).range(0.0..=f64::MAX),
                            Kind::Percent => Scrub::fine(0.25, 1).range(0.0..=f64::MAX),
                            _ => Scrub::whole(0.5).range(0.0..=f64::MAX),
                        },
                        |d| d.custom_formatter(ui::number(2)),
                    );
                    let tx = if edited(&resp, v != n) {
                        commit_list(
                            self,
                            Track::Size(TrackSize::Breadth(
                                kind.breadth(Some(v.max(0.0))).unwrap_or(*b),
                            )),
                        )
                    } else {
                        Transaction(Vec::new())
                    };
                    self.edit_valve(&resp, tx);
                }
                None => {
                    ui.allocate_exact_size(egui::vec2(room, side), egui::Sense::hover());
                }
            },
            Track::Size(TrackSize::MinMax { min, max }) => {
                let half = egui::vec2((room - gap) / 2.0, side);
                for is_min in [true, false] {
                    let b = if is_min { *min } else { *max };
                    let edit = breadth_field(ui, half, Prefix::Text(""), b, !is_min);
                    let to = |nb: TrackBreadth| {
                        Track::Size(if is_min {
                            TrackSize::MinMax { min: nb, max: *max }
                        } else {
                            TrackSize::MinMax { min: *min, max: nb }
                        })
                    };
                    if let Some(nb) = edit.picked {
                        let tx = commit_list(self, to(nb));
                        self.commit_edit(tx);
                        continue;
                    }
                    let tx = match edit.typed {
                        Some(nb) => commit_list(self, to(nb)),
                        None => Transaction(Vec::new()),
                    };
                    self.edit_valve(&edit.resp, tx);
                }
            }
            Track::Repeat { repeat, tracks } => {
                let count_w = 64.0;
                let mut v = f64::from(*repeat);
                let (resp, _) = ui::value_field_suffixed(
                    ui,
                    egui::vec2(count_w, side),
                    Prefix::Text(""),
                    Some(Suffix {
                        text: "×",
                        clickable: false,
                        tooltip: "",
                    }),
                    &mut v,
                    Scrub::whole(0.1).range(1.0..=1000.0),
                    |d| d.custom_formatter(ui::number(0)),
                );
                let count = v.round().clamp(1.0, 1000.0) as u16;
                let tx = if edited(&resp, count != *repeat) {
                    commit_list(
                        self,
                        Track::Repeat {
                            repeat: count,
                            tracks: tracks.clone(),
                        },
                    )
                } else {
                    Transaction(Vec::new())
                };
                self.edit_valve(&resp, tx);
                let rest = egui::vec2((room - count_w - gap).max(20.0), side);
                match tracks.as_slice() {
                    [TrackSize::Breadth(b)] => {
                        let edit = breadth_field(ui, rest, Prefix::Text(""), *b, true);
                        let to = |nb| Track::Repeat {
                            repeat: *repeat,
                            tracks: vec![TrackSize::Breadth(nb)],
                        };
                        if let Some(nb) = edit.picked {
                            let tx = commit_list(self, to(nb));
                            self.commit_edit(tx);
                        } else {
                            let tx = match edit.typed {
                                Some(nb) => commit_list(self, to(nb)),
                                None => Transaction(Vec::new()),
                            };
                            self.edit_valve(&edit.resp, tx);
                        }
                    }
                    several => {
                        let text = several
                            .iter()
                            .map(|s| track_size_css(*s))
                            .collect::<Vec<_>>()
                            .join(" ");
                        ui.add_sized(
                            rest,
                            egui::Label::new(
                                egui::RichText::new(text).size(11.0).color(theme::text::DIM),
                            )
                            .truncate(),
                        )
                        .on_hover_text("Several tracks repeated — edit them in the CSS line");
                    }
                }
            }
        }
    }

    /// The Item card's rows for an item of a grid (§15 D920): its lines, then
    /// `justify-self` and `align-self`, each `auto` reading what the container's
    /// items row resolves to. Where flex has grow, shrink and basis.
    ///
    /// `flipped` is justify, align: the two self-alignments the last resize's
    /// receipt names, each accented while it does, as flex's are (§15 D880) —
    /// the receipt said *"justify self"* over a row nothing marked until §15
    /// D922.
    pub(super) fn grid_item_rows(
        &mut self,
        ui: &mut egui::Ui,
        subjects: &[NodeId],
        items: &[LayoutItem],
        parent: &Grid,
        full: f32,
        flipped: [bool; 2],
    ) {
        let gap = ui::CARD_COL_GAP;
        let side = ui::CONTROL_H;
        const LABEL_W: f32 = 74.0;
        for (label, rows) in [("Grid column", false), ("Grid row", true)] {
            let lines = |i: &LayoutItem| if rows { i.grid_row } else { i.grid_column };
            let mut refused = None;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                ui.add_sized(
                    egui::vec2(LABEL_W, side),
                    egui::Label::new(
                        egui::RichText::new(label)
                            .size(10.5)
                            .color(theme::text::FAINT),
                    ),
                );
                let half = egui::vec2((full - LABEL_W - gap * 2.0) / 2.0, side);
                for is_start in [true, false] {
                    let end = |l: GridLines| if is_start { l.start } else { l.end };
                    let shown = shared(items, |i| end(lines(i))).map(placement_css);
                    let key = egui::Id::new((
                        "grid-item-line",
                        rows,
                        is_start,
                        subjects.first().copied(),
                    ));
                    let tip = if is_start {
                        "Where it starts — auto, a line (2, or -1 from the end), or span n"
                    } else {
                        "Where it ends — auto, a line (2, or -1 from the end), or span n"
                    };
                    if let Some(text) = css_field(ui, key, half, shown.as_deref(), tip) {
                        match parse_placement(&text) {
                            Some(p) => {
                                let tx = self.item_tx(subjects, |i| {
                                    let l = if rows {
                                        &mut i.grid_row
                                    } else {
                                        &mut i.grid_column
                                    };
                                    if is_start {
                                        l.start = p;
                                    } else {
                                        l.end = p;
                                    }
                                });
                                self.commit_edit(tx);
                            }
                            // Said, not dropped — `arch-scribe` read the first cut
                            // throwing an unreadable line away in silence.
                            None => refuse(
                                ui,
                                key,
                                shown.as_deref(),
                                format!(
                                    "\"{}\" is not a line — auto, 2, -1 or span 2",
                                    text.trim()
                                ),
                            ),
                        }
                    }
                    if refused.is_none() {
                        refused = refusal(ui, key, shown.as_deref());
                    }
                }
            });
            if let Some(why) = refused {
                ui.label(egui::RichText::new(why).size(10.5).color(color::DANGER));
            }
        }

        // --- the self-alignments ----------------------------------------------------
        for (salt, label, across, flipped) in [
            ("grid-justify-self", "Justify self", true, flipped[0]),
            ("grid-align-self", "Align self", false, flipped[1]),
        ] {
            let at = egui::Rect::from_min_size(ui.cursor().min, egui::vec2(full, side));
            let own = |i: &LayoutItem| if across { i.justify_self } else { i.align_self };
            let inherited = if across {
                parent.justify_items
            } else {
                parent.align_items
            };
            // `T` is the row's value with `normal` in it, so the face can read
            // *Auto · Normal*; an item's own value is never `normal`.
            let shown = shared(items, |i| own(i).map(Some));
            if let Some(pick) = glyph_combo(
                ui,
                salt,
                label,
                full,
                shown,
                if across { &SELF } else { &ALIGN_SELF },
                if across { items_name } else { align_items_name },
                move |a| {
                    if across {
                        items_glyph(a)
                    } else {
                        align_items_glyph(a)
                    }
                },
                move |_| if across { ACROSS } else { Orient::default() },
                Some(inherited),
                true,
                None,
            ) {
                let to = pick.flatten();
                let tx = self.item_tx(subjects, |i| {
                    if across {
                        i.justify_self = to;
                    } else {
                        i.align_self = to;
                    }
                });
                self.commit_edit(tx);
            }
            if flipped {
                ui.painter().rect_stroke(
                    at,
                    egui::CornerRadius::same(ui::BUTTON_R),
                    egui::Stroke::new(1.0, color::ACCENT),
                    egui::StrokeKind::Inside,
                );
            }
        }
    }
}

/// The track row's two pure pieces — the kind menu's conversion and the grip's
/// reorder — pinned apart from the card, which `layout`'s tests drive whole.
///
/// Plain backticks throughout, per §15 D319 — a `#[cfg(test)]` module.
#[cfg(test)]
mod tests {
    use super::*;

    fn b(b: TrackBreadth) -> Track {
        Track::Size(TrackSize::Breadth(b))
    }

    /// **A kind picked from the menu turns the track into one of that kind, and
    /// carries over what means the same** — a single size becomes `minmax()`'s
    /// maximum when it is `fr` and its minimum otherwise, and `repeat()`'s track;
    /// a `repeat()`'s track comes back out; between px, %
    /// and fr the number does not travel (200px is not 200fr) and the kind's
    /// default stands; a keyword kind is the keyword.
    #[test]
    fn a_picked_kind_keeps_what_means_the_same() {
        let px = b(TrackBreadth::Px(200.0));
        let fr = b(TrackBreadth::Fr(2.0));
        assert_eq!(converted(&px, Kind::Fr), b(TrackBreadth::Fr(1.0)));
        assert_eq!(converted(&px, Kind::Auto), b(TrackBreadth::Auto));
        assert_eq!(
            converted(&px, Kind::MinMax),
            Track::Size(TrackSize::MinMax {
                min: TrackBreadth::Px(200.0),
                max: TrackBreadth::Fr(1.0),
            })
        );
        assert_eq!(
            converted(&fr, Kind::MinMax),
            Track::Size(TrackSize::MinMax {
                min: TrackBreadth::Px(100.0),
                max: TrackBreadth::Fr(2.0),
            }),
            "a share is the maximum, never an fr minimum"
        );
        assert_eq!(
            converted(&fr, Kind::Repeat),
            Track::Repeat {
                repeat: 2,
                tracks: vec![TrackSize::Breadth(TrackBreadth::Fr(2.0))],
            }
        );
        let rep = converted(&fr, Kind::Repeat);
        assert_eq!(converted(&rep, Kind::Fr), fr, "its track back out");
        // A maximum of 3fr, so its coming back out is not the default standing
        // (`arch-scribe`: a 1fr maximum could not tell the two apart).
        let mm = Track::Size(TrackSize::MinMax {
            min: TrackBreadth::Px(200.0),
            max: TrackBreadth::Fr(3.0),
        });
        assert_eq!(
            converted(&mm, Kind::Fr),
            b(TrackBreadth::Fr(3.0)),
            "its maximum back out"
        );
        assert_eq!(converted(&mm, Kind::MinMax), mm, "the same kind is itself");
    }

    /// **A grip drop moves the entry to the slot it lands in** — a slot is the gap
    /// before an entry, the list's length the end — and the two slots either side
    /// of the entry are where it already is, so they move nothing.
    #[test]
    fn a_grip_drop_moves_the_entry_to_its_slot() {
        let [x, y, z] = [1.0, 2.0, 3.0].map(|v| b(TrackBreadth::Px(v)));
        let list = vec![x.clone(), y.clone(), z.clone()];
        assert_eq!(
            reordered(&list, 0, 3),
            Some(vec![y.clone(), z.clone(), x.clone()])
        );
        assert_eq!(
            reordered(&list, 2, 0),
            Some(vec![z.clone(), x.clone(), y.clone()])
        );
        assert_eq!(
            reordered(&list, 0, 2),
            Some(vec![y.clone(), x.clone(), z.clone()])
        );
        assert_eq!(reordered(&list, 1, 1), None, "the slot before it");
        assert_eq!(reordered(&list, 1, 2), None, "the slot after it");
        assert_eq!(reordered(&list, 5, 0), None, "an entry that is not there");
    }
}
