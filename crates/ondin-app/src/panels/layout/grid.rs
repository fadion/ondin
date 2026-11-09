//! Grid's rows of the **Container** and **Item** cards (§15 D920).
//!
//! **Container**, under `display: grid`: `grid-auto-flow`, the two track lists —
//! each track a row leading with its kind, a function's arguments as fields of
//! their own, a grip to reorder and a cross to remove, and under the list the
//! whole template as editable CSS for pasting — then the named areas as a map
//! and as CSS, then `justify-content`, `justify-items`, `align-items`,
//! `align-content`, the gaps and the padding. **Item**, in a grid: `grid-area`
//! where the grid has areas, `grid-column` and `grid-row` as CSS writes them,
//! start and end, then `justify-self` and `align-self`.
//!
//! Four things differ from flex's rows on purpose:
//!
//! - **`Start` and `End` read *Start* and *End***, not *Flex start*: a grid has no
//!   reversal for `flex-start` to follow, and CSS treats the two alike there (§15
//!   D913's session call).
//! - **`justify-items`, and `normal`**: the first is
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
    AreaName, AutoRepeat, GridArea, GridAutoFlow, GridLines, RepeatCount, Track, TrackBreadth,
    TrackSize, area_lines, areas_are_valid, areas_css, areas_without_tracks, auto_repeat_refusal,
    parse_areas, parse_placement, parse_tracks, placement_css, repeats_to_fit, track_count,
    track_size_css, tracks_css,
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
    AutoFill,
    AutoFit,
}

const KINDS: [Kind; 10] = [
    Kind::Px,
    Kind::Fr,
    Kind::Percent,
    Kind::Auto,
    Kind::MinContent,
    Kind::MaxContent,
    Kind::MinMax,
    Kind::Repeat,
    Kind::AutoFill,
    Kind::AutoFit,
];

impl Kind {
    fn of(t: &Track) -> Kind {
        match t {
            Track::Repeat {
                repeat: RepeatCount::Auto(AutoRepeat::Fill),
                ..
            } => Kind::AutoFill,
            Track::Repeat {
                repeat: RepeatCount::Auto(AutoRepeat::Fit),
                ..
            } => Kind::AutoFit,
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
            Kind::AutoFill => "auto-fill",
            Kind::AutoFit => "auto-fit",
        }
    }

    /// Whether this kind is a function, with no breadth of its own.
    fn is_function(self) -> bool {
        matches!(
            self,
            Kind::MinMax | Kind::Repeat | Kind::AutoFill | Kind::AutoFit
        )
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
            Kind::Repeat => "The same tracks, a fixed number of times",
            Kind::AutoFill => "As many as fit",
            Kind::AutoFit => "As many as fit, and close the empty ones",
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
            Kind::MinMax | Kind::Repeat | Kind::AutoFill | Kind::AutoFit => return None,
        })
    }
}

/// `s` with a fixed part, as an automatic repeat needs: kept where it has one,
/// else given a 100px minimum under what it had as its maximum.
fn fixed(s: TrackSize) -> TrackSize {
    if s.is_fixed() {
        return s;
    }
    let max = match s {
        TrackSize::Breadth(b) => b,
        TrackSize::MinMax { max, .. } => max,
    };
    TrackSize::MinMax {
        min: TrackBreadth::Px(100.0),
        max,
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
/// track, a `minmax()`'s maximum — comes back out where it is of kind `k`. A
/// repeat keeps its tracks between a count and `auto-fill`/`auto-fit`, each
/// given a fixed part on the way to an automatic one ([`fixed`]).
fn converted(t: &Track, k: Kind) -> Track {
    let repeated = |repeat: RepeatCount| {
        let tracks = match t {
            Track::Size(s) => vec![*s],
            Track::Repeat { tracks, .. } => tracks.clone(),
        };
        Track::Repeat {
            repeat,
            tracks: match repeat {
                RepeatCount::Auto(_) => tracks.into_iter().map(fixed).collect(),
                RepeatCount::Count(_) => tracks,
            },
        }
    };
    match k {
        Kind::Repeat => return repeated(RepeatCount::Count(2)),
        Kind::AutoFill => return repeated(RepeatCount::Auto(AutoRepeat::Fill)),
        Kind::AutoFit => return repeated(RepeatCount::Auto(AutoRepeat::Fit)),
        _ => {}
    }
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
/// ⚠️ **No clickable `–` under a keyword**: a size field showing a keyword in its
/// digits has no unit (§15 D906, D920).
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
                    .filter(|k| !k.is_function())
                    .filter(|k| fr || *k != Kind::Fr)
                {
                    // **The lit row picks nothing** (§15 D941) — `track_row`'s
                    // kind menu already said so; this one reset
                    // `minmax(200px, 1fr)` to `minmax(100px, 1fr)` on a click of
                    // its own `px`.
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
    // **Under a keyword, a keystroke is an edit too** (§15 D941): the hidden
    // number there is 0, so a typed `0` never moved it and `minmax(0, 1fr)` —
    // CSS's idiom for an `fr` track that may shrink below its content — could not
    // be typed. A click in and out types nothing, so D885's rule holds. **Only
    // what can be part of a number counts**: a letter typed over `auto` leaves
    // the hidden 0 in place, and counting it wrote `0px`.
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
        // D941): a mixed selection seeds it empty with `shown` `None`, and
        // `Some("") != None` read the untouched buffer as typed — a click in and
        // out of a *Mixed* template line wrote `none` to every selected grid, and
        // an item's line field raised a refusal nobody typed. D885's rule: a
        // click in and out writes nothing.
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
fn refuse(ctx: &egui::Context, key: egui::Id, shown: Option<&str>, why: String) {
    let r: Refused = (shown.map(str::to_owned), why);
    ctx.data_mut(|d| d.insert_temp(key.with(REFUSED), r));
}

/// The key of a track list's CSS line, under which a refusal of the list is
/// shown whichever control made it.
fn css_key(rows: bool, subjects: &[NodeId]) -> egui::Id {
    egui::Id::new(("grid-track-css", rows, subjects.first().copied()))
}

/// The refusal to show under the field at `key`, **only while the field still
/// reads what it read when the text was refused** — an apply through any other
/// door (a `+`, a cross, an undo) makes it about a list no longer there — and
/// never once the field has been engaged again ([`css_field`] drops it).
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

/// The area whose name the map is editing, and the text typed so far; `fresh`
/// until the field has been given focus. `fold` is the undo step a drag just
/// made the area in, which the name joins (`EditorSession::commit_into_last`).
#[derive(Clone, Debug)]
struct Renaming {
    name: AreaName,
    text: String,
    fresh: bool,
    fold: Option<(usize, u64)>,
}

/// The columns and rows the area map draws: the template's tracks, or as many as
/// the areas reach past them — CSS sizes the explicit grid by the larger.
fn map_tracks(g: &Grid) -> (usize, usize) {
    let reach = |f: fn(&GridArea) -> u16| {
        g.areas
            .iter()
            .filter(|a| a.is_valid())
            .map(|a| usize::from(f(a)) - 1)
            .max()
            .unwrap_or(0)
    };
    (
        track_count(&g.columns).max(reach(|a| a.columns[1])),
        track_count(&g.rows).max(reach(|a| a.rows[1])),
    )
}

/// The undo step of every edit to a grid's areas.
const AREAS_STEP: &str = "Grid areas";

/// The most tracks on an axis the area map draws a cell for; past it the cells
/// are too narrow to drag across, and the CSS line edits the areas.
const MAP_TRACKS: usize = 24;

impl OndinApp {
    /// Every subject's grid with `f` applied — [`Self::display_tx`] for a grid.
    fn grid_tx(&self, subjects: &[NodeId], f: impl Fn(&mut Grid)) -> Transaction {
        self.display_tx(subjects, |d| {
            if let Display::Grid(g) = d {
                f(g);
            }
        })
    }

    /// The rows under `display: grid` — in this order, with `justify-items`
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
        // An instance's overridden grid fields (§15 D981), each its own mark.
        let flow_mark = self.grid_mark(
            subjects,
            |g| g.auto_flow,
            |g, v| g.auto_flow = v,
            move |v| ["Row", "Column"][usize::from(*v == GridAutoFlow::Column)].to_owned(),
        );
        let (pick, reset) = ui::segmented_marked(
            ui,
            full,
            ui::SEGMENT_CELL_H,
            2,
            at,
            |_| true,
            |i| FLOW_TIPS[i],
            flow_mark.as_ref().map(OverrideMark::field),
            |p, i, r, on| match flow {
                None if i == 0 => ui::segment_mixed(p, r),
                _ => ui::segment_label(p, r, ["Row", "Column"][i], on),
            },
        );
        self.reset_marked(reset, &flow_mark);
        if let Some(i) = pick {
            let tx = self.grid_tx(subjects, |g| g.auto_flow = flows[i]);
            self.session.name_step("Auto flow", icon::GRID_LAYOUT);
            self.commit_edit(tx);
        }

        // --- the tracks ------------------------------------------------------------
        self.track_list(ui, subjects, grids, false, full);
        self.track_list(ui, subjects, grids, true, full);
        self.area_map(ui, subjects, grids, full);

        // --- the alignments ---------------------------------------------------------
        let content = |c: AlignContent| Glyph::Content(c);
        let mark = self.grid_mark(
            subjects,
            |g| g.justify_content,
            |g, v| g.justify_content = v,
            |v| grid_content_name(*v).to_owned(),
        );
        let (pick, reset) = glyph_combo(
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
            mark.as_ref(),
        );
        self.reset_marked(reset, &mark);
        if let Some(Some(c)) = pick {
            let tx = self.grid_tx(subjects, |g| g.justify_content = c);
            self.session.name_step("Justify content", icon::ALIGN_LEFT);
            self.commit_edit(tx);
        }
        let mark = self.grid_mark(
            subjects,
            |g| g.justify_items,
            |g, v| g.justify_items = v,
            |v| items_name(*v).to_owned(),
        );
        let (pick, reset) = glyph_combo(
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
            mark.as_ref(),
        );
        self.reset_marked(reset, &mark);
        if let Some(Some(a)) = pick {
            let tx = self.grid_tx(subjects, |g| g.justify_items = a);
            self.session.name_step("Justify items", icon::ALIGN_LEFT);
            self.commit_edit(tx);
        }
        let mark = self.grid_mark(
            subjects,
            |g| g.align_items,
            |g, v| g.align_items = v,
            |v| align_items_name(*v).to_owned(),
        );
        let (pick, reset) = glyph_combo(
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
            mark.as_ref(),
        );
        self.reset_marked(reset, &mark);
        if let Some(Some(a)) = pick {
            let tx = self.grid_tx(subjects, |g| g.align_items = a);
            self.session.name_step("Align items", icon::ALIGN_TOP);
            self.commit_edit(tx);
        }
        let mark = self.grid_mark(
            subjects,
            |g| g.align_content,
            |g, v| g.align_content = v,
            |v| grid_content_name(*v).to_owned(),
        );
        let (pick, reset) = glyph_combo(
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
            mark.as_ref(),
        );
        self.reset_marked(reset, &mark);
        if let Some(Some(c)) = pick {
            let tx = self.grid_tx(subjects, |g| g.align_content = c);
            self.session.name_step("Align content", icon::ALIGN_TOP);
            self.commit_edit(tx);
        }

        // --- the gaps and the padding: columns first, both always live ------------
        self.gap_row(ui, subjects, laid, true, true, full);
        self.padding_rows(ui, subjects, laid, full);
    }

    /// One track list — `grid-template-columns`, or `-rows` when `rows`: a head with the property's name, how many tracks it makes
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
        // An instance's overridden track list (§15 D981), whole — the tracks carry
        // no ids to be compared one by one — on the head's label.
        let mark = self.grid_mark(
            subjects,
            get,
            |g, l| with_list(g, rows, &l),
            |l| tracks_css(l),
        );

        // --- the head ----------------------------------------------------------
        ui.horizontal(|ui| {
            let hit = super::super::component::label_mark(
                ui,
                None,
                if rows {
                    "Grid template rows"
                } else {
                    "Grid template columns"
                },
                11.0,
                theme::text::dim(),
                mark.as_ref(),
            );
            self.reset_marked(hit, &mark);
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
                    self.session.name_step(
                        if rows {
                            "Add row track"
                        } else {
                            "Add column track"
                        },
                        icon::PLUS,
                    );
                    self.commit_edit(tx);
                }
                let n = track_count(&list);
                ui.label(
                    egui::RichText::new(match (agreed, n) {
                        (false, _) => ui::MIXED_WORD.to_owned(),
                        // The count depends on the container's size.
                        (true, _) if repeats_to_fit(&list) => "repeats to fit".to_owned(),
                        (true, 0) => "None".to_owned(),
                        (true, 1) => "1 track".to_owned(),
                        (true, n) => format!("{n} tracks"),
                    })
                    .size(ui::CAPTION_PT)
                    .color(theme::text::faint()),
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
            // was drawn.
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
                        egui::Stroke::new(1.5, color::accent()),
                    );
                }
                if released {
                    ui.ctx().data_mut(|d| d.remove::<TrackDrag>(drag_key));
                    if let Some(moved) = reordered(&list, drag.from, to) {
                        let tx = self.grid_tx(subjects, |g| with_list(g, rows, &moved));
                        self.name_tracks_step(rows);
                        self.commit_edit(tx);
                    }
                }
            }
        }

        // --- the CSS line --------------------------------------------------------
        let key = css_key(rows, subjects);
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
                    self.name_tracks_step(rows);
                    self.commit_edit(tx);
                }
                Ok(parsed) => refuse(
                    ui.ctx(),
                    key,
                    shown.as_deref(),
                    auto_repeat_refusal(&parsed)
                        .unwrap_or_else(|| "CSS does not accept that list".into()),
                ),
                Err(e) => refuse(ui.ctx(), key, shown.as_deref(), e),
            }
        }
        if let Some(why) = refusal(ui, key, shown.as_deref()) {
            ui.label(
                egui::RichText::new(why)
                    .size(ui::CAPTION_PT)
                    .color(color::danger()),
            );
        } else if grids.iter().zip(subjects).any(|(g, id)| {
            repeats_to_fit(&get(g)) && self.hugs_axis(*id, rows) && self.laid_once(*id, rows)
        }) {
            // Valid, with a consequence worth naming: CSS repeats once.
            ui.label(
                egui::RichText::new(if rows {
                    "Fit content gives no height to fill — it repeats once"
                } else {
                    "Fit content gives no width to fill — it repeats once"
                })
                .size(ui::CAPTION_PT)
                .color(color::warn()),
            );
        }
    }

    /// Every subject's track list `rows` names set to `now`, replacing the shared
    /// `was` — or nothing, with the reason under the list's CSS line, where `now`
    /// holds an automatic repeat CSS refuses. Said rather than left to the
    /// operation, which refuses it in silence.
    fn list_tx(
        &self,
        ctx: &egui::Context,
        subjects: &[NodeId],
        rows: bool,
        was: &[Track],
        now: &[Track],
    ) -> Transaction {
        if let Some(why) = auto_repeat_refusal(now) {
            let shown = tracks_css(was);
            refuse(ctx, css_key(rows, subjects), Some(&shown), why);
            return Transaction(Vec::new());
        }
        self.grid_tx(subjects, |g| with_list(g, rows, now))
    }

    /// `grid-template-areas`: a head with the property's name and how many areas,
    /// a map of the explicit grid's cells while the selection agrees on its
    /// tracks and areas, and the areas as CSS under it — which is also how a
    /// selection that disagrees is edited.
    fn area_map(&mut self, ui: &mut egui::Ui, subjects: &[NodeId], grids: &[Grid], full: f32) {
        let g = grids[0].clone();
        let agreed = grids
            .iter()
            .all(|o| o.areas == g.areas && o.columns == g.columns && o.rows == g.rows);
        let (cols, rows) = map_tracks(&g);
        let mark = self.grid_mark(
            subjects,
            |g| g.areas.clone(),
            |g, v| g.areas = v,
            |v| areas_css(v, 0, 0),
        );

        // --- the head ----------------------------------------------------------
        ui.horizontal(|ui| {
            let hit = super::super::component::label_mark(
                ui,
                None,
                "Grid template areas",
                11.0,
                theme::text::dim(),
                mark.as_ref(),
            );
            self.reset_marked(hit, &mark);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.label(
                    egui::RichText::new(match (agreed, g.areas.len()) {
                        (false, _) => ui::MIXED_WORD.to_owned(),
                        (true, 0) => "None".to_owned(),
                        (true, 1) => "1 area".to_owned(),
                        (true, n) => format!("{n} areas"),
                    })
                    .size(ui::CAPTION_PT)
                    .color(theme::text::faint()),
                );
            });
        });

        // --- the map --------------------------------------------------------------
        let key = egui::Id::new(("grid-areas-css", subjects.first().copied()));
        let shown = agreed.then(|| areas_css(&g.areas, cols, rows));
        if agreed {
            let why = if repeats_to_fit(&g.columns) || repeats_to_fit(&g.rows) {
                Some("Areas need a fixed number of tracks — this axis repeats to fit")
            } else if cols == 0 || rows == 0 {
                Some("Areas are drawn on the tracks — add a column and a row")
            } else if cols > MAP_TRACKS || rows > MAP_TRACKS {
                Some("Too many tracks to draw — edit the areas as CSS")
            } else {
                None
            };
            match why {
                Some(why) => {
                    ui.label(
                        egui::RichText::new(why)
                            .size(ui::CAPTION_PT)
                            .color(theme::text::faint()),
                    );
                }
                None => self.area_cells(ui, subjects, &g, key, shown.as_deref(), full),
            }
        }

        // --- the CSS line -----------------------------------------------------------
        if let Some(text) = css_field(
            ui,
            key,
            egui::vec2(full, ui::CONTROL_H),
            shown.as_deref(),
            "The areas as CSS — a quoted row per track row, . for an empty cell, Enter to apply",
        ) {
            match parse_areas(&text) {
                Ok(parsed) if areas_are_valid(&parsed) => {
                    let tx = self.grid_tx(subjects, |g| g.areas.clone_from(&parsed));
                    self.session.name_step(AREAS_STEP, icon::GRID_LAYOUT);
                    self.commit_edit(tx);
                }
                Ok(_) => refuse(
                    ui.ctx(),
                    key,
                    shown.as_deref(),
                    "CSS does not accept those areas".into(),
                ),
                Err(e) => refuse(ui.ctx(), key, shown.as_deref(), e),
            }
        }
        if let Some(why) = refusal(ui, key, shown.as_deref()) {
            ui.label(
                egui::RichText::new(why)
                    .size(ui::CAPTION_PT)
                    .color(color::danger()),
            );
        }
    }

    /// The map itself: one cell per explicit track crossing, columns at the
    /// widths the first subject lays them, each area one tinted rectangle with its
    /// name. A drag across empty cells makes an area and opens its name; a
    /// double-click opens an area's name, and clearing it removes the area.
    /// `key` and `shown` are the CSS line's, where a refusal is said.
    fn area_cells(
        &mut self,
        ui: &mut egui::Ui,
        subjects: &[NodeId],
        g: &Grid,
        key: egui::Id,
        shown: Option<&str>,
        full: f32,
    ) {
        const GAP: f32 = 3.0;
        let (cols, rows) = map_tracks(g);
        let h = ui::CONTROL_H;
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(full, rows as f32 * (h + GAP) + GAP),
            egui::Sense::click_and_drag(),
        );
        // Column widths at the laid tracks' proportions, each at least a sliver;
        // a column with no laid track (an area past the template, nothing in it)
        // at the laid ones' mean, and all equal where the grid lays none.
        let laid = ondin_core::build::laid_grid(&self.session.doc, subjects[0]);
        let laid: Vec<Option<f32>> = (0..cols)
            .map(|c| {
                // CSS's column 1 is laid track `before`.
                laid.as_ref()
                    .and_then(|l| l.columns.spans.get(usize::from(l.columns.before) + c))
                    .map(|(a, b)| (b - a).max(0.0) as f32)
            })
            .collect();
        let known: Vec<f32> = laid.iter().flatten().copied().collect();
        let mean = match known.len() {
            0 => 1.0,
            n => known.iter().sum::<f32>() / n as f32,
        };
        let mut widths: Vec<f32> = laid.iter().map(|w| w.unwrap_or(mean)).collect();
        let total: f32 = widths.iter().sum();
        let room = rect.width() - GAP * (cols as f32 + 1.0);
        for w in &mut widths {
            *w = if total > 0.0 {
                (*w / total * room).max(12.0)
            } else {
                room / cols as f32
            };
        }
        let scale = room / widths.iter().sum::<f32>();
        let mut xs = vec![rect.left() + GAP];
        for w in &widths {
            let x = *xs.last().unwrap_or(&rect.left());
            xs.push(x + w * scale + GAP);
        }
        // The screen rectangle over 1-based lines `[c0, c1)` × `[r0, r1)`.
        let span = |c: [u16; 2], r: [u16; 2]| {
            let c = c.map(|l| usize::from(l).clamp(1, cols + 1) - 1);
            let r = r.map(|l| usize::from(l).clamp(1, rows + 1) - 1);
            egui::Rect::from_min_max(
                egui::pos2(xs[c[0]], rect.top() + GAP + r[0] as f32 * (h + GAP)),
                egui::pos2(
                    xs[c[1]] - GAP,
                    rect.top() + GAP + r[1] as f32 * (h + GAP) - GAP,
                ),
            )
        };
        let cell_at = |p: egui::Pos2| -> (u16, u16) {
            let c = xs.iter().take(cols).rposition(|x| p.x >= *x).unwrap_or(0);
            let r = (((p.y - rect.top() - GAP) / (h + GAP)).floor() as isize)
                .clamp(0, rows as isize - 1) as usize;
            (c as u16 + 1, r as u16 + 1)
        };
        let painter = ui.painter_at(rect);
        painter.rect(
            rect,
            egui::CornerRadius::same(ui::BUTTON_R),
            color::field(),
            egui::Stroke::new(1.0, color::field_border()),
            egui::StrokeKind::Inside,
        );
        let radius = egui::CornerRadius::same(3);
        for r in 1..=rows as u16 {
            for c in 1..=cols as u16 {
                if !g.areas.iter().any(|a| a.holds(c, r)) {
                    painter.rect_filled(span([c, c + 1], [r, r + 1]), radius, color::hover());
                }
            }
        }
        let renaming_key = key.with("rename");
        let renaming = ui
            .ctx()
            .data(|d| d.get_temp::<Renaming>(renaming_key))
            .filter(|n| g.area(n.name).is_some());
        for a in &g.areas {
            let r = span(a.columns, a.rows);
            painter.rect_filled(r, radius, color::accent_a(56));
            if renaming.as_ref().is_some_and(|n| n.name == a.name) {
                continue;
            }
            painter.text(
                r.left_center() + egui::vec2(6.0, 0.0),
                egui::Align2::LEFT_CENTER,
                a.name.as_str(),
                egui::FontId::proportional(ui::CAPTION_PT),
                color::text(),
            );
        }

        // --- a drag across empty cells makes an area ----------------------------
        let anchor_key = key.with("anchor");
        if resp.drag_started()
            && let Some(p) = ui.input(|i| i.pointer.press_origin())
        {
            let at = cell_at(p);
            ui.ctx().data_mut(|d| d.insert_temp(anchor_key, at));
        }
        let anchor = ui.ctx().data(|d| d.get_temp::<(u16, u16)>(anchor_key));
        if let (Some((ac, ar)), Some(p)) = (anchor, resp.interact_pointer_pos()) {
            let (pc, pr) = cell_at(p);
            let new = GridArea {
                name: AreaName::new(""),
                columns: [ac.min(pc), ac.max(pc) + 1],
                rows: [ar.min(pr), ar.max(pr) + 1],
            };
            painter.rect_stroke(
                span(new.columns, new.rows),
                radius,
                egui::Stroke::new(1.5, color::accent()),
                egui::StrokeKind::Inside,
            );
            if resp.drag_stopped() {
                ui.ctx().data_mut(|d| d.remove::<(u16, u16)>(anchor_key));
                if g.areas.iter().any(|a| a.overlaps(&new)) {
                    refuse(
                        ui.ctx(),
                        key,
                        shown,
                        "Areas cannot overlap — clear an area's name to remove it".into(),
                    );
                } else {
                    let name = (1..)
                        .map(|n| match n {
                            1 => AreaName::new("area"),
                            n => AreaName::new(&format!("area-{n}")),
                        })
                        .find(|n| g.area(*n).is_none())
                        .unwrap_or_else(|| AreaName::new("area"));
                    let area = GridArea { name, ..new };
                    let tx = self.grid_tx(subjects, |g| g.areas.push(area));
                    self.session.name_step(AREAS_STEP, icon::GRID_LAYOUT);
                    if self.commit_edit(tx) {
                        let fold =
                            Some((self.session.history.undo_depth(), self.session.commits()));
                        ui.ctx().data_mut(|d| {
                            d.insert_temp(
                                renaming_key,
                                Renaming {
                                    name,
                                    text: name.as_str().to_owned(),
                                    fresh: true,
                                    fold,
                                },
                            )
                        });
                    }
                }
            }
        } else if anchor.is_some() && !ui.input(|i| i.pointer.any_down()) {
            ui.ctx().data_mut(|d| d.remove::<(u16, u16)>(anchor_key));
        }
        let resp = resp.on_hover_text(
            "Drag across cells to make an area · double-click one to rename it, or clear its \
             name to remove it",
        );

        // --- a double-click opens an area's name ----------------------------------
        if ui::double_clicked(ui, &resp)
            && let Some(p) = resp.interact_pointer_pos()
        {
            let (c, r) = cell_at(p);
            if let Some(a) = g.areas.iter().find(|a| a.holds(c, r)) {
                ui.ctx().data_mut(|d| {
                    d.insert_temp(
                        renaming_key,
                        Renaming {
                            name: a.name,
                            text: a.name.as_str().to_owned(),
                            fresh: true,
                            fold: None,
                        },
                    )
                });
            }
        }

        // --- the name being typed ----------------------------------------------------
        let Some(mut n) = renaming else {
            return;
        };
        let Some(area) = g.area(n.name).copied() else {
            return;
        };
        let at = span(area.columns, area.rows);
        let at = egui::Rect::from_min_size(at.min, egui::vec2(at.width(), h.min(at.height())));
        let field = ui.put(at, |ui: &mut egui::Ui| {
            let resp = ui::text_field(ui, at.size(), &mut n.text, "");
            ui::select_all_on_focus(ui, &resp, &n.text);
            resp
        });
        if n.fresh {
            field.request_focus();
            ui::select_all(ui, &field, &n.text);
            n.fresh = false;
        }
        if field.lost_focus() {
            ui.ctx().data_mut(|d| d.remove::<Renaming>(renaming_key));
            let typed = n.text.trim();
            if !ui::defocus_commits(&field) || typed == area.name.as_str() {
                return;
            }
            let to = AreaName::new(typed);
            let why = if typed.is_empty() {
                None
            } else if !to.is_valid() {
                Some(format!("\"{typed}\" is not a name CSS reads"))
            } else if g.area(to).is_some() {
                Some(format!("Another area is called \"{typed}\""))
            } else {
                None
            };
            if let Some(why) = why {
                refuse(ui.ctx(), key, shown, why);
                return;
            }
            let tx = self.grid_tx(subjects, |g| {
                if typed.is_empty() {
                    g.areas.retain(|a| a.name != area.name);
                } else if let Some(a) = g.areas.iter_mut().find(|a| a.name == area.name) {
                    a.name = to;
                }
            });
            self.session.name_step(AREAS_STEP, icon::GRID_LAYOUT);
            match n.fold {
                Some(at) => {
                    self.note_edit(None, &tx);
                    self.session.commit_into_last(tx, at);
                }
                None => {
                    self.commit_edit(tx);
                }
            }
        } else {
            ui.ctx().data_mut(|d| d.insert_temp(renaming_key, n));
        }
    }

    /// Whether container `id` has no size of its own across `rows`' axis for an
    /// automatic repeat to fill: a frame asked to fit its content, or a group whose
    /// size is not typed — unless two insets stretch it on that axis.
    fn hugs_axis(&self, id: NodeId, rows: bool) -> bool {
        let Some(node) = self.session.doc.get(id) else {
            return false;
        };
        let item = node.item();
        let insets = node.insets();
        let (size, pinned) = if rows {
            (item.height, insets.top.is_some() && insets.bottom.is_some())
        } else {
            (item.width, insets.left.is_some() && insets.right.is_some())
        };
        !pinned
            && match node.kind() {
                NodeKind::Group => matches!(size, Dimension::Auto | Dimension::FitContent),
                _ => size == Dimension::FitContent,
            }
    }

    /// Whether grid container `id`'s automatic repeat across `rows`' axis was laid
    /// once — read from the layout, since a row parent can grow a frame that fits
    /// its content to a width the repeat then fills.
    fn laid_once(&self, id: NodeId, rows: bool) -> bool {
        let Some(Display::Grid(g)) = self.session.doc.get(id).and_then(|n| n.display()) else {
            return false;
        };
        let Some(laid) = ondin_core::build::laid_grid(&self.session.doc, id) else {
            return true;
        };
        let (list, axis) = match rows {
            true => (&g.rows, &laid.rows),
            false => (&g.columns, &laid.columns),
        };
        usize::from(axis.explicit) <= track_count(list)
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
            // The kind menu, the row's leading chip.
            let mut picked = None;
            ui.scope(|ui| {
                ui.spacing_mut().interact_size.y = side;
                ui.spacing_mut().button_padding.y = 0.0;
                egui::ComboBox::from_id_salt(("grid-track-kind", rows, i))
                    .icon(ui::combo_chevron)
                    .width(74.0)
                    .selected_text(ui::combo_text(kind.word()))
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
                let tx = self.list_tx(ui.ctx(), subjects, rows, list, &l);
                if !tx.0.is_empty() {
                    self.name_tracks_step(rows);
                    self.commit_edit(tx);
                }
            }
            // The fields, in what is left before the cross.
            let room = (ui.available_width() - side - gap).max(40.0);
            self.track_fields(ui, subjects, rows, list, i, &track, room);
            let remove = ui::icon_button(ui, icon::X, side, 13.0, false, true)
                .on_hover_text("Remove this track");
            if remove.clicked() {
                let mut l = list.to_vec();
                l.remove(i);
                // The tracks entry `i` made: every area crossing them shrinks, and
                // `keep_area_names` sends an emptied one's items back to their lines.
                let from = track_count(&list[..i]);
                let to = track_count(&list[..=i]);
                let tx = self.grid_tx(subjects, |g| {
                    with_list(g, rows, &l);
                    g.areas = areas_without_tracks(&g.areas, rows, from, to);
                });
                self.session.name_step(
                    if rows {
                        "Remove row track"
                    } else {
                        "Remove column track"
                    },
                    icon::X,
                );
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
        let ctx = ui.ctx().clone();
        let commit_list = |app: &mut Self, t: Track| {
            let mut l = list.to_vec();
            l[i] = t;
            app.list_tx(&ctx, subjects, rows, list, &l)
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
                    self.named_valve(tracks_label(rows), tracks_glyph(rows), &resp, tx);
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
                        self.name_tracks_step(rows);
                        self.commit_edit(tx);
                        continue;
                    }
                    let tx = match edit.typed {
                        Some(nb) => commit_list(self, to(nb)),
                        None => Transaction(Vec::new()),
                    };
                    self.named_valve(tracks_label(rows), tracks_glyph(rows), &edit.resp, tx);
                }
            }
            // An automatic repeat takes no count: the repeated size alone, so no
            // field ever shows a keyword where its digits go.
            Track::Repeat {
                repeat: repeat @ RepeatCount::Auto(_),
                tracks,
            } => self.repeated_fields(
                ui,
                rows,
                *repeat,
                tracks,
                egui::vec2(room, side),
                &commit_list,
            ),
            Track::Repeat {
                repeat: RepeatCount::Count(repeat),
                tracks,
            } => {
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
                            repeat: RepeatCount::Count(count),
                            tracks: tracks.clone(),
                        },
                    )
                } else {
                    Transaction(Vec::new())
                };
                self.named_valve(tracks_label(rows), tracks_glyph(rows), &resp, tx);
                let rest = egui::vec2((room - count_w - gap).max(20.0), side);
                self.repeated_fields(
                    ui,
                    rows,
                    RepeatCount::Count(*repeat),
                    tracks,
                    rest,
                    &commit_list,
                );
            }
        }
    }

    /// A repeat's tracks in `size`: one breadth as a field, several as their CSS,
    /// which the list's CSS line edits.
    fn repeated_fields(
        &mut self,
        ui: &mut egui::Ui,
        rows: bool,
        repeat: RepeatCount,
        tracks: &[TrackSize],
        size: egui::Vec2,
        commit_list: &dyn Fn(&mut Self, Track) -> Transaction,
    ) {
        match tracks {
            [TrackSize::Breadth(b)] => {
                let edit = breadth_field(ui, size, Prefix::Text(""), *b, true);
                let to = |nb| Track::Repeat {
                    repeat,
                    tracks: vec![TrackSize::Breadth(nb)],
                };
                if let Some(nb) = edit.picked {
                    let tx = commit_list(self, to(nb));
                    self.name_tracks_step(rows);
                    self.commit_edit(tx);
                } else {
                    let tx = match edit.typed {
                        Some(nb) => commit_list(self, to(nb)),
                        None => Transaction(Vec::new()),
                    };
                    self.named_valve(tracks_label(rows), tracks_glyph(rows), &edit.resp, tx);
                }
            }
            several => {
                let text = several
                    .iter()
                    .map(|s| track_size_css(*s))
                    .collect::<Vec<_>>()
                    .join(" ");
                ui.add_sized(
                    size,
                    egui::Label::new(
                        egui::RichText::new(text)
                            .size(ui::SMALL_PT)
                            .color(theme::text::dim()),
                    )
                    .truncate(),
                )
                .on_hover_text(if several.len() == 1 {
                    "A minmax track repeated — edit it in the CSS line"
                } else {
                    "Several tracks repeated — edit them in the CSS line"
                });
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

        // --- grid-area: offered where the grid has areas or an item names one ------
        // `parent` is the first subject's; its areas are offered only to a
        // selection that is all in that one grid.
        let doc = &self.session.doc;
        let first_parent = doc.get(subjects[0]).and_then(|n| n.parent());
        let one_grid = subjects
            .iter()
            .all(|id| doc.get(*id).and_then(|n| n.parent()) == first_parent);
        let area = shared(items, |i| i.grid_area);
        let in_area = items.iter().any(|i| i.grid_area.is_some());
        if one_grid && (!parent.areas.is_empty() || in_area) {
            let mark = self.item_mark(
                subjects,
                |i| i.grid_area,
                |i, v| i.grid_area = v,
                |v| v.map_or_else(|| "None".to_owned(), |n| n.to_string()),
            );
            let mut picked = None;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                let hit = super::super::component::label_mark(
                    ui,
                    Some(egui::vec2(LABEL_W, side)),
                    "Grid area",
                    10.5,
                    theme::text::faint(),
                    mark.as_ref(),
                );
                self.reset_marked(hit, &mark);
                ui.spacing_mut().interact_size.y = side;
                ui.spacing_mut().button_padding.y = 0.0;
                let face = match area {
                    None => ui::MIXED_WORD.to_owned(),
                    Some(None) => "None".to_owned(),
                    Some(Some(n)) => n.to_string(),
                };
                egui::ComboBox::from_id_salt(("grid-area", subjects.first().copied()))
                    .icon(ui::combo_chevron)
                    .width(full - LABEL_W - gap)
                    .selected_text(ui::combo_text(&face))
                    .show_ui(ui, |ui| {
                        ui::menu_rows(ui);
                        if ui
                            .selectable_label(area == Some(None), "None")
                            .on_hover_text("Placed by its lines")
                            .clicked()
                        {
                            picked = Some(None);
                        }
                        for a in &parent.areas {
                            if ui
                                .selectable_label(area == Some(Some(a.name)), a.name.as_str())
                                .clicked()
                            {
                                picked = Some(Some(a.name));
                            }
                        }
                    });
            });
            if let Some(to) = picked.filter(|to| area != Some(*to)) {
                let tx = self.item_tx(subjects, |i| i.grid_area = to);
                self.session.name_step("Grid area", icon::GRID_LAYOUT);
                self.commit_edit(tx);
            }
        }
        // While every item fills one area, the lines rows show the area's; while
        // any fills one, they wait, as a typed line would not move it.
        let by_area = area.flatten().filter(|_| one_grid);

        for (label, rows) in [("Grid column", false), ("Grid row", true)] {
            let lines = |i: &LayoutItem| match by_area {
                Some(name) => {
                    let (c, r) = area_lines(name, parent);
                    if rows { r } else { c }
                }
                None if rows => i.grid_row,
                None => i.grid_column,
            };
            // An instance's overridden lines (§15 D981), on the row's label: both
            // ends are one property, `grid-column` or `grid-row`.
            let mark = self.item_mark(
                subjects,
                lines,
                |i, v| {
                    if rows {
                        i.grid_row = v;
                    } else {
                        i.grid_column = v;
                    }
                },
                |v| format!("{} / {}", placement_css(v.start), placement_css(v.end)),
            );
            let mut refused = None;
            let row = ui
                .add_enabled_ui(!in_area, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = gap;
                        let hit = super::super::component::label_mark(
                            ui,
                            Some(egui::vec2(LABEL_W, side)),
                            label,
                            10.5,
                            theme::text::faint(),
                            mark.as_ref(),
                        );
                        self.reset_marked(hit, &mark);
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
                                        self.session.name_step(label, tracks_glyph(rows));
                                        self.commit_edit(tx);
                                    }
                                    // Said, not dropped: an unreadable line is never thrown
                                    // away in silence.
                                    None => refuse(
                                        ui.ctx(),
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
                    })
                })
                .response;
            match by_area {
                Some(name) => {
                    row.on_disabled_hover_text(format!(
                        "Placed by the area \u{201c}{name}\u{201d} — set Grid area to None to \
                         place by lines"
                    ));
                }
                None if in_area => {
                    row.on_disabled_hover_text(
                        "Some are placed by an area — set Grid area to None to place by lines",
                    );
                }
                None => {}
            }
            if let Some(why) = refused {
                ui.label(
                    egui::RichText::new(why)
                        .size(ui::CAPTION_PT)
                        .color(color::danger()),
                );
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
            let mark = self.item_mark(
                subjects,
                own,
                |i, v| {
                    if across {
                        i.justify_self = v;
                    } else {
                        i.align_self = v;
                    }
                },
                move |v| match v {
                    None => "Auto".to_owned(),
                    Some(_) if across => items_name(*v).to_owned(),
                    Some(_) => align_items_name(*v).to_owned(),
                },
            );
            let (pick, reset) = glyph_combo(
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
                mark.as_ref(),
            );
            self.reset_marked(reset, &mark);
            if let Some(pick) = pick {
                let to = pick.flatten();
                let tx = self.item_tx(subjects, |i| {
                    if across {
                        i.justify_self = to;
                    } else {
                        i.align_self = to;
                    }
                });
                self.session.name_step(
                    label,
                    if across {
                        icon::ALIGN_LEFT
                    } else {
                        icon::ALIGN_TOP
                    },
                );
                self.commit_edit(tx);
            }
            if flipped {
                ui.painter().rect_stroke(
                    at,
                    egui::CornerRadius::same(ui::BUTTON_R),
                    egui::Stroke::new(1.0, color::accent_700()),
                    egui::StrokeKind::Inside,
                );
            }
        }
    }

    /// Name the next step after the track list it edits.
    fn name_tracks_step(&mut self, rows: bool) {
        self.session
            .name_step(tracks_label(rows), tracks_glyph(rows));
    }
}

/// A track list's name as an undo step.
fn tracks_label(rows: bool) -> &'static str {
    if rows { "Grid rows" } else { "Grid columns" }
}

/// The glyph beside a track list's undo step.
fn tracks_glyph(rows: bool) -> &'static str {
    if rows { icon::ROWS } else { icon::COLUMNS }
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
                repeat: RepeatCount::Count(2),
                tracks: vec![TrackSize::Breadth(TrackBreadth::Fr(2.0))],
            }
        );
        let rep = converted(&fr, Kind::Repeat);
        assert_eq!(converted(&rep, Kind::Fr), fr, "its track back out");
        // A maximum of 3fr, so its coming back out is not the default standing
        // (a 1fr maximum could not tell the two apart).
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

    /// **An automatic repeat gets a fixed part on the way in, and keeps its
    /// tracks between a count and `auto-fit`**: `1fr` becomes `minmax(100px,
    /// 1fr)`, a px track stays as it is, and a repeat's tracks travel whole.
    /// flip: `fixed` returning its input → fails at the first assert.
    #[test]
    fn an_automatic_repeat_gets_a_fixed_part() {
        let fr = b(TrackBreadth::Fr(1.0));
        let fill = converted(&fr, Kind::AutoFill);
        assert_eq!(
            fill,
            Track::Repeat {
                repeat: RepeatCount::Auto(AutoRepeat::Fill),
                tracks: vec![TrackSize::MinMax {
                    min: TrackBreadth::Px(100.0),
                    max: TrackBreadth::Fr(1.0),
                }],
            }
        );
        assert_eq!(auto_repeat_refusal(std::slice::from_ref(&fill)), None);
        let px = b(TrackBreadth::Px(160.0));
        assert_eq!(
            converted(&px, Kind::AutoFit),
            Track::Repeat {
                repeat: RepeatCount::Auto(AutoRepeat::Fit),
                tracks: vec![TrackSize::Breadth(TrackBreadth::Px(160.0))],
            }
        );
        let Track::Repeat { tracks, .. } = converted(&fill, Kind::Repeat) else {
            panic!("a repeat");
        };
        assert_eq!(tracks.len(), 1);
        assert!(matches!(tracks[0], TrackSize::MinMax { .. }), "kept whole");
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
