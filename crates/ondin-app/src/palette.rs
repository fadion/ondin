//! The command palette (`docs/design/app-canvas-chrome.md`, *The command
//! palette*): every app verb by name, and the layers by theirs.
//!
//! Its rows are the context-menu registry's (`crate::menu`), built against the
//! selection instead of a pointer. This module ranks them against the query, draws
//! them, and runs the chosen one through `OndinApp::perform_menu_item`, so a row
//! and its chord or menu row are one verb.

use crate::app::{ContextMenu, OndinApp, TopMenu};
use crate::input::{Action, TextChord, ViewSwitch};
use crate::menu::{self, Door, Group, Item, Kind, Target};
use crate::theme::{self, color, icon};
use crate::tools::Tool;
use eframe::egui;
use ondin_core::{NodeId, build};
use std::ops::Range;

/// The card's width, and the gap between the top of the canvas and the card.
const W: f32 = 560.0;
const TOP_GAP: f32 = 56.0;
const FIELD_H: f32 = 42.0;
const ROW_H: f32 = 30.0;
const SECTION_H: f32 = 24.0;
/// The footer's height, shared with the cheatsheet's.
pub(crate) const FOOTER_H: f32 = 30.0;
/// How far the footer's text sits above the footer's centre.
const FOOTER_TEXT_LIFT: f32 = 1.0;
/// The list scrolls past this; the field and the footer never do.
const LIST_MAX_H: f32 = 372.0;
/// The list's inset inside the card.
const PAD: f32 = 6.0;
/// The layer rows the mixed list shows before offering the rest behind `Tab`.
const LAYER_HITS: usize = 5;
/// The rows *Recent* remembers.
const RECENT: usize = 5;
/// A cap on the layers-only list, which a one-letter query over a large document
/// could otherwise make thousands of rows long.
const LAYERS_MAX: usize = 200;
/// How far Page Up and Page Down move the highlight.
const PAGE: usize = 8;

/// The open palette.
pub(crate) struct Palette {
    query: String,
    /// `Tab`'s scope: layers only, shown as a chip in the field.
    layers_only: bool,
    /// An index into this frame's selectable entries, top to bottom.
    highlight: usize,
    /// The clipboard state at open, which the rows read as a context menu's do.
    /// Its target is rewritten each frame from the selection.
    snapshot: ContextMenu,
    /// Why *Publish kit…* is dim, read at open as the document menu reads it:
    /// counting the changes takes `&mut self`, and nothing edits under the palette.
    publish_why: Option<&'static str>,
    /// The query and scope the highlight was last reset for; any change sends it
    /// back to the first live row.
    reset_for: Option<(String, bool)>,
    /// Scroll the highlighted row into view: set by a key, so a pointer hover
    /// does not scroll under the hand.
    scroll: bool,
}

/// This frame's palette keys, taken out of `Events` at the top of the frame so the
/// field never reads them (`read_keys`).
#[derive(Default, Clone, Copy)]
pub(crate) struct Keys {
    step: Option<Step>,
    enter: bool,
    shift_enter: bool,
    tab: bool,
    escape: bool,
    /// The palette's own chord again.
    close: bool,
    /// The cheatsheet's chord, which closes the palette and opens the sheet.
    cheatsheet: bool,
}

#[derive(Clone, Copy)]
enum Step {
    Next,
    Prev,
    PageDown,
    PageUp,
}

/// Take the keys the open palette answers. Called before any panel draws, as
/// `menu::read_nav` is for a context menu.
pub(crate) fn read_keys(ctx: &egui::Context) -> Keys {
    let none = egui::Modifiers::NONE;
    ctx.input_mut(|i| {
        let step = [
            (egui::Key::ArrowDown, Step::Next),
            (egui::Key::ArrowUp, Step::Prev),
            (egui::Key::PageDown, Step::PageDown),
            (egui::Key::PageUp, Step::PageUp),
        ]
        .into_iter()
        .find(|(key, _)| i.consume_key(none, *key))
        .map(|(_, step)| step);
        Keys {
            step,
            shift_enter: i.consume_key(egui::Modifiers::SHIFT, egui::Key::Enter),
            enter: i.consume_key(none, egui::Key::Enter),
            tab: i.consume_key(none, egui::Key::Tab),
            escape: i.consume_key(none, egui::Key::Escape),
            // Before `close`, whose `/` would take the shifted chord too: an extra
            // Shift still matches logically.
            cheatsheet: i.consume_key(
                egui::Modifiers::COMMAND | egui::Modifiers::SHIFT,
                egui::Key::Slash,
            ) || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Questionmark),
            close: i.consume_key(egui::Modifiers::COMMAND, egui::Key::K)
                || i.consume_key(egui::Modifiers::COMMAND, egui::Key::Slash),
        }
    })
}

/// How a row is remembered in *Recent*: by verb, so *Hide* and *Show* are one.
pub(crate) fn verb_key(item: Item) -> String {
    format!("{item:?}")
}

/// One command row, resolved for this frame.
struct Cmd {
    item: Item,
    label: String,
    glyph: &'static str,
    filled_hexagon: bool,
    accel: Option<&'static str>,
    group: Group,
    checked: Option<bool>,
    enabled: bool,
    why: Option<&'static str>,
    /// A verb the selection's menu leaves out, shown only when typed for.
    absent: bool,
    /// Built by the target's own menu: what *For the selection* lists.
    own: bool,
    /// Byte ranges of `label` the query matched.
    hits: Vec<Range<usize>>,
    rank: u8,
}

impl Cmd {
    fn of(row: menu::Row, target: Target, own: bool) -> Self {
        let spec = row.item.spec();
        // No pointer, so *Paste here* is plain paste (`run`).
        let label = if row.item == Item::PasteHere && target != Target::TextSession {
            "Paste".to_string()
        } else {
            row.label.into_owned()
        };
        Self {
            item: row.item,
            label,
            glyph: spec.glyph,
            filled_hexagon: spec.filled_hexagon,
            accel: spec.accel,
            group: row.group.unwrap_or(spec.group),
            checked: row.checked,
            enabled: row.enabled,
            why: row.why,
            absent: false,
            own,
            hits: Vec::new(),
            rank: 0,
        }
    }

    /// An app verb, live unless `why` says otherwise.
    fn verb(item: Item, why: Option<&'static str>) -> Self {
        let spec = item.spec();
        Self {
            item,
            label: spec.label.to_string(),
            glyph: spec.glyph,
            filled_hexagon: spec.filled_hexagon,
            accel: spec.accel,
            group: spec.group,
            checked: None,
            enabled: why.is_none(),
            why,
            absent: false,
            own: false,
            hits: Vec::new(),
            rank: 0,
        }
    }
}

/// One matching layer.
struct LayerHit {
    id: NodeId,
    name: String,
    glyph: &'static str,
    tint: egui::Color32,
    /// The parents, outermost first.
    path: String,
    hidden: bool,
    locked: bool,
    hits: Vec<Range<usize>>,
}

enum Entry {
    Cmd(Cmd),
    Layer(LayerHit),
    /// *N more layers*, which `Tab` and `Enter` both open.
    More(usize),
}

struct Section {
    title: Option<&'static str>,
    entries: Vec<Entry>,
}

/// What a press on an entry does.
enum Run {
    Cmd(Item),
    Layer { id: NodeId, add: bool },
    More,
}

/// The words of `s` with their byte offsets, case folded: runs of letters and
/// digits, so *Align horizontal centres* is three words and *Ctrl+K* is none of
/// the label's business.
fn words(s: &str) -> Vec<(usize, String)> {
    let mut out = Vec::new();
    let mut start: Option<usize> = None;
    for (i, ch) in s.char_indices() {
        if ch.is_alphanumeric() {
            start.get_or_insert(i);
        } else if let Some(at) = start.take() {
            out.push((at, s[at..i].to_lowercase()));
        }
    }
    if let Some(at) = start {
        out.push((at, s[at..].to_lowercase()));
    }
    out
}

/// The query's words, case folded.
fn query_words(query: &str) -> Vec<String> {
    words(query).into_iter().map(|(_, w)| w).collect()
}

/// Whether `label` (or its `aliases`) answers the query, and how well: `0` a
/// prefix of the whole label, `1` every word the start of a label word, `2` found
/// through an alias. With the byte ranges of the label to light.
///
/// Each query word takes a different label word, in any order: "al c" finds
/// *Align horizontal centres*. An alias match lights nothing, since the letters
/// typed are not in the label.
///
/// A query with no words (`🔥`, `#`) answers no command: matching every one
/// would put an arbitrary verb under Enter. The layers section still answers it.
fn rank(label: &str, aliases: &[&str], query: &[String]) -> Option<(u8, Vec<Range<usize>>)> {
    if query.is_empty() {
        return None;
    }
    let lw = words(label);
    let mut used = vec![false; lw.len()];
    let mut hits = Vec::new();
    let all = query.iter().all(|q| {
        let Some(j) = (0..lw.len()).find(|&j| !used[j] && lw[j].1.starts_with(q.as_str())) else {
            return false;
        };
        used[j] = true;
        let start = lw[j].0;
        // `q` is a prefix of the folded word; light as many characters of the original.
        let len = label[start..]
            .char_indices()
            .nth(q.chars().count())
            .map_or(label.len() - start, |(b, _)| b);
        hits.push(start..start + len);
        true
    });
    if all {
        let whole = query.join(" ");
        let prefix = words(label)
            .iter()
            .map(|(_, w)| w.as_str())
            .collect::<Vec<_>>()
            .join(" ")
            .starts_with(&whole);
        hits.sort_by_key(|r| r.start);
        return Some((if prefix { 0 } else { 1 }, hits));
    }
    let alias_words: Vec<String> = aliases
        .iter()
        .flat_map(|a| words(a).into_iter().map(|(_, w)| w))
        .collect();
    let in_label = |q: &String| lw.iter().any(|(_, w)| w.starts_with(q.as_str()));
    let in_alias = |q: &String| alias_words.iter().any(|w| w.starts_with(q.as_str()));
    // An alias must answer a word the label cannot, or "g g" would find *Group*.
    (query.iter().all(|q| in_label(q) || in_alias(q)) && query.iter().any(|q| !in_label(q)))
        .then(|| (2, Vec::new()))
}

/// The byte range of `needle` in `name`, case folded, where folding kept the
/// byte offsets (it does for every name that is not exotic Unicode).
fn substring_hit(name: &str, needle: &str) -> Vec<Range<usize>> {
    let folded = name.to_lowercase();
    if needle.is_empty() || folded.len() != name.len() {
        return Vec::new();
    }
    folded
        .find(&needle.to_lowercase())
        .filter(|at| name.is_char_boundary(*at) && name.is_char_boundary(at + needle.len()))
        .map(|at| std::iter::once(at..at + needle.len()).collect())
        .unwrap_or_default()
}

const TOOLS: [Tool; 13] = [
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
];

const TEXT_CHORDS: [TextChord; 13] = [
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
];

const SWITCHES: [ViewSwitch; 16] = [
    ViewSwitch::Rulers,
    ViewSwitch::Guides,
    ViewSwitch::GuideLock,
    ViewSwitch::Grid,
    ViewSwitch::LayoutGrid,
    ViewSwitch::Outline,
    ViewSwitch::Layers,
    ViewSwitch::History,
    ViewSwitch::Minimap,
    ViewSwitch::Toolbar,
    ViewSwitch::StatusMessages,
    ViewSwitch::Focus,
    ViewSwitch::SnapShapes,
    ViewSwitch::SnapGuides,
    ViewSwitch::SnapGrid,
    ViewSwitch::SnapBaselines,
];

/// The highlighted row's ground: the accent at the design's 18%.
fn highlight_fill() -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(109, 140, 217, 46)
}

impl OndinApp {
    /// `Ctrl+K` / `Ctrl+/`: open the palette, or close it if open.
    pub(crate) fn toggle_palette(&mut self, _ctx: &egui::Context) {
        if self.palette.take().is_some() {
            return;
        }
        // One floating thing at a time, as a context menu's open clears the rest (R4).
        self.context_menu = None;
        self.open_menu = TopMenu::None;
        self.picker = None;
        self.stroke_menu = None;
        self.effect_menu = None;
        self.export_menu = false;
        self.export_row_open = None;
        self.type_menu = None;
        let target = self.palette_target();
        let publish_why = match self.kit_changes() {
            None => Some("Make this document a kit first"),
            Some(c) if !c.any() => Some("No changes"),
            Some(_) => None,
        };
        self.palette = Some(Palette {
            query: String::new(),
            layers_only: false,
            highlight: 0,
            snapshot: self.menu_snapshot(egui::Pos2::ZERO, target, None),
            publish_why,
            reset_for: None,
            scroll: false,
        });
    }

    /// What the palette's rows act on: the text being edited, the point
    /// selection, the selected layers through the key layer (else the topmost), a
    /// selected guide, or the page.
    fn palette_target(&self) -> Target {
        if self.text.is_some() {
            return Target::TextSession;
        }
        if let Some(path) = self.edited_path()
            && (self.points.on(path).next().is_some()
                || self.points.segments(path).next().is_some())
        {
            return Target::Points;
        }
        if let Some(id) = self
            .session
            .selection
            .key()
            .or_else(|| self.topmost_selected())
        {
            return Target::Layer {
                id,
                door: Door::Panel,
            };
        }
        if let Some(guide) = self.session.selection.guides().first() {
            return Target::Guide(*guide);
        }
        Target::Canvas
    }

    /// The selected layer the layers panel lists highest.
    fn topmost_selected(&self) -> Option<NodeId> {
        let sel = self.session.selection.ids();
        if sel.len() <= 1 {
            return sel.first().copied();
        }
        let doc = &self.session.doc;
        let mut stack = vec![self.session.page];
        while let Some(id) = stack.pop() {
            if sel.contains(&id) {
                return Some(id);
            }
            if let Some(node) = doc.get(id) {
                stack.extend(node.children().iter().copied());
            }
        }
        sel.first().copied()
    }

    /// Every command row for this frame, in registry order: the target's menu, the
    /// page's, the app verbs, then the verbs the menu left out.
    fn palette_commands(&self, pal: &Palette) -> Vec<Cmd> {
        let target = pal.snapshot.target;
        let doc = &self.session.doc;
        let sel = self.session.selection.ids();
        let kinds: Vec<Kind> = sel
            .iter()
            .filter_map(|id| doc.get(*id).map(|n| Kind::of(n.kind())))
            .collect();
        let mut out: Vec<Cmd> = Vec::new();
        let push = |out: &mut Vec<Cmd>, cmd: Cmd| {
            if !out.iter().any(|c| c.item == cmd.item) {
                out.push(cmd);
            }
        };
        // A point the keyboard cannot supply, and a "this" with no pointer to name it.
        let pointless = |item: Item| matches!(item, Item::AddPointHere | Item::MakeKeyLayer);

        let mut cx = self.menu_context(&pal.snapshot, &kinds);
        for row in menu::build(&cx).into_iter().flatten() {
            if !pointless(row.item) {
                push(&mut out, Cmd::of(row, target, true));
            }
        }
        if target == Target::TextSession {
            for chord in TEXT_CHORDS {
                push(&mut out, Cmd::verb(Item::TextChord(chord), None));
            }
        }
        // The page's rows are the selection's too: Select all, Zoom to fit, Export all.
        cx.target = Target::Canvas;
        cx.state = menu::LayerState::default();
        for row in menu::build(&cx).into_iter().flatten() {
            if !pointless(row.item) {
                push(&mut out, Cmd::of(row, target, false));
            }
        }

        let none = sel.is_empty();
        let need_sel = none.then_some(menu::NEEDS_SELECTION);
        let history = &self.session.history;
        push(
            &mut out,
            Cmd::verb(
                Item::Undo,
                (!history.can_undo()).then_some("Nothing to undo"),
            ),
        );
        push(
            &mut out,
            Cmd::verb(
                Item::Redo,
                (!history.can_redo()).then_some("Nothing to redo"),
            ),
        );
        for tool in TOOLS {
            push(&mut out, Cmd::verb(Item::Tool(tool), None));
        }
        push(&mut out, Cmd::verb(Item::PlaceImage, None));
        // Pages: the one way to reach another page with the layers pane hidden.
        push(&mut out, Cmd::verb(Item::NewPage, None));
        let at = doc.page_index(self.session.page).unwrap_or(0);
        let last = doc.pages().len() - 1;
        push(
            &mut out,
            Cmd::verb(
                Item::StepPage(true),
                (at == last).then_some("This is the last page"),
            ),
        );
        push(
            &mut out,
            Cmd::verb(
                Item::StepPage(false),
                (at == 0).then_some("This is the first page"),
            ),
        );
        for page in doc.page_ids() {
            let name = self.page_name(page);
            let mut go = Cmd::verb(
                Item::GoToPage(page),
                (page == self.session.page).then_some("This is the current page"),
            );
            go.label = format!("Go to page: {name}");
            push(&mut out, go);
            if page != self.session.page {
                let mut send = Cmd::verb(Item::MoveTo(page), need_sel);
                send.label = format!("Move to page: {name}");
                push(&mut out, send);
            }
        }
        for item in [Item::ZoomIn, Item::ZoomOut, Item::ZoomReset] {
            push(&mut out, Cmd::verb(item, None));
        }
        for axis in [build::Axis::X, build::Axis::Y] {
            for edge in [build::Edge::Min, build::Edge::Mid, build::Edge::Max] {
                push(&mut out, Cmd::verb(Item::Align(axis, edge), need_sel));
            }
        }
        for axis in [build::Axis::X, build::Axis::Y] {
            push(
                &mut out,
                Cmd::verb(
                    Item::Distribute(axis),
                    (sel.len() < 3).then_some("Needs three or more layers"),
                ),
            );
        }
        push(
            &mut out,
            Cmd::verb(
                Item::TidyUp,
                (sel.len() < 2).then_some("Needs two or more layers"),
            ),
        );
        for sw in SWITCHES {
            let mut cmd = Cmd::verb(Item::ViewSwitch(sw), None);
            cmd.checked = Some(self.view_switch(sw));
            push(&mut out, cmd);
        }
        for item in [Item::InsertPicker, Item::VariablesBrowser] {
            push(&mut out, Cmd::verb(item, None));
        }
        push(&mut out, Cmd::verb(Item::PublishKit, pal.publish_why));
        // The Document menu's rows, each refused where the menu would not offer it.
        let shared = self.shared_kit().is_some();
        push(
            &mut out,
            Cmd::verb(
                Item::MakeKit,
                shared.then_some("This document is already a kit"),
            ),
        );
        push(
            &mut out,
            Cmd::verb(
                Item::StopSharingKit,
                (!shared).then_some("This document is not a kit"),
            ),
        );
        push(
            &mut out,
            Cmd::verb(
                Item::KitsUsed,
                (!self.uses_kits()).then_some("This document uses no kit"),
            ),
        );
        push(&mut out, Cmd::verb(Item::ShowInLibrary, None));
        push(&mut out, Cmd::verb(Item::VersionHistory, None));
        for clockwise in [false, true] {
            push(
                &mut out,
                Cmd::verb(Item::RotateQuarter(clockwise), need_sel),
            );
        }
        for same in crate::input::Same::ALL {
            push(
                &mut out,
                Cmd::verb(Item::SelectSame(same), self.select_same_source(same).err()),
            );
        }
        push(&mut out, Cmd::verb(Item::SelectInverse, need_sel));
        for t in [
            ondin_export::code::Target::Html,
            ondin_export::code::Target::React,
            ondin_export::code::Target::Vue,
        ] {
            push(
                &mut out,
                Cmd::verb(
                    Item::ExportCode(t),
                    none.then_some("Select the layers to export as code"),
                ),
            );
        }
        for item in [
            Item::Save,
            Item::NewDocument,
            Item::CloseDocument,
            Item::Library,
            Item::Settings,
            Item::Cheatsheet,
        ] {
            push(&mut out, Cmd::verb(item, None));
        }
        let no_frames = self.presentable().is_empty();
        push(
            &mut out,
            Cmd::verb(Item::Present, no_frames.then_some("No frames to present")),
        );
        let mut contrast = Cmd::verb(Item::CheckContrast, None);
        if self.contrast.is_some() {
            contrast.label = "Stop checking contrast".into();
        }
        push(&mut out, contrast);
        // The cards the inspector drew last frame, then the ones it can draw for
        // another selection.
        let focus = self
            .focus
            .then_some("Leave focus mode to see the inspector");
        let shown: Vec<&'static str> = self.cards_shown.clone();
        for title in &shown {
            let mut cmd = Cmd::verb(Item::ShowCard(title), focus);
            cmd.label = format!("Show {title}");
            push(&mut out, cmd);
        }
        for title in self.cards_seen.iter().filter(|t| !shown.contains(t)) {
            let mut cmd = Cmd::verb(Item::ShowCard(title), Some("Not shown for this selection"));
            cmd.label = format!("Show {title}");
            push(&mut out, cmd);
        }

        for (item, needs) in menu::KIND_VERBS {
            if out.iter().any(|c| c.item == *item) {
                continue;
            }
            let mut cmd = Cmd::verb(
                *item,
                Some(if none { menu::NEEDS_SELECTION } else { needs }),
            );
            cmd.absent = true;
            push(&mut out, cmd);
        }
        out
    }

    /// The layers whose names match, as the palette lists them.
    fn palette_layers(&self, needle: &str, cap: usize) -> (Vec<LayerHit>, usize) {
        let doc = &self.session.doc;
        let root = self.session.page;
        let ids: Vec<NodeId> = crate::panels::layers::name_hits(doc, root, needle)
            .into_iter()
            .filter(|id| *id != root)
            .collect();
        let total = ids.len();
        let hits = ids
            .into_iter()
            .take(cap)
            .filter_map(|id| {
                let node = doc.get(id)?;
                let (glyph, tint) = crate::panels::layers::node_icon(node.kind());
                let mut path = Vec::new();
                let mut cursor = node.parent();
                while let Some(p) = cursor.filter(|p| *p != root) {
                    let parent = doc.get(p)?;
                    path.push(parent.name().to_string());
                    cursor = parent.parent();
                }
                path.reverse();
                Some(LayerHit {
                    id,
                    name: node.name().to_string(),
                    glyph,
                    tint,
                    path: path.join(" / "),
                    hidden: !doc.shown(id),
                    locked: node.locked(),
                    hits: substring_hit(node.name(), needle),
                })
            })
            .collect();
        (hits, total)
    }

    /// The list for this frame: ranked commands, then layers, then the verbs not
    /// available here; or *Recent* and the target's own rows for an empty query.
    fn palette_sections(&self, pal: &Palette) -> Vec<Section> {
        let needle = pal.query.trim();
        if pal.layers_only {
            let (hits, _) = self.palette_layers(needle, LAYERS_MAX);
            return vec![Section {
                title: None,
                entries: hits.into_iter().map(Entry::Layer).collect(),
            }];
        }
        let cmds = self.palette_commands(pal);
        let recents = &self.prefs.palette_recent;
        let recency = |c: &Cmd| {
            recents
                .iter()
                .position(|k| *k == verb_key(c.item))
                .unwrap_or(usize::MAX)
        };
        if needle.is_empty() {
            let mut recent: Vec<(usize, Cmd)> = Vec::new();
            let mut own = Vec::new();
            for cmd in cmds {
                if cmd.absent {
                    continue;
                }
                let r = recency(&cmd);
                if r < RECENT {
                    recent.push((r, cmd));
                } else if cmd.own {
                    own.push(cmd);
                }
            }
            recent.sort_by_key(|(r, _)| *r);
            let mut out = Vec::new();
            if !recent.is_empty() {
                out.push(Section {
                    title: Some("Recent"),
                    entries: recent.into_iter().map(|(_, c)| Entry::Cmd(c)).collect(),
                });
            }
            if !own.is_empty() {
                out.push(Section {
                    title: Some(match pal.snapshot.target {
                        Target::Canvas => "For the page",
                        Target::TextSession => "For the text",
                        Target::Points => "For the points",
                        Target::Guide(_) => "For the guide",
                        _ => "For the selection",
                    }),
                    entries: own.into_iter().map(Entry::Cmd).collect(),
                });
            }
            return out;
        }

        let query = query_words(needle);
        let mut live = Vec::new();
        let mut absent = Vec::new();
        for (order, mut cmd) in cmds.into_iter().enumerate() {
            let Some((r, hits)) = rank(&cmd.label, cmd.item.aliases(), &query) else {
                continue;
            };
            cmd.rank = r;
            cmd.hits = hits;
            if cmd.absent {
                absent.push((order, cmd));
            } else {
                live.push((order, cmd));
            }
        }
        // Dim rows rank with the live ones, so the list does not reshuffle as the
        // selection changes what is dim; ties keep registry order.
        live.sort_by_key(|(order, c)| (c.rank, recency(c), *order));
        absent.sort_by_key(|(order, c)| (c.rank, *order));
        let mut out = Vec::new();
        if !live.is_empty() {
            out.push(Section {
                title: None,
                entries: live.into_iter().map(|(_, c)| Entry::Cmd(c)).collect(),
            });
        }
        let (hits, total) = self.palette_layers(needle, LAYER_HITS);
        if !hits.is_empty() {
            let mut entries: Vec<Entry> = hits.into_iter().map(Entry::Layer).collect();
            if total > LAYER_HITS {
                entries.push(Entry::More(total - LAYER_HITS));
            }
            out.push(Section {
                title: Some("Layers"),
                entries,
            });
        }
        if !absent.is_empty() {
            out.push(Section {
                title: Some("Not available here"),
                entries: absent.into_iter().map(|(_, c)| Entry::Cmd(c)).collect(),
            });
        }
        out
    }

    /// The footer's left half: what the rows act on.
    fn palette_target_note(&self, target: Target) -> String {
        match target {
            Target::TextSession => "Editing text".into(),
            Target::Points => "Points on the path".into(),
            Target::Guide(_) => "A guide".into(),
            Target::Page(p) => self.page_name(p),
            Target::Canvas | Target::PanelBackground => "Nothing selected".into(),
            Target::Layer { id, .. } => {
                let name = self
                    .session
                    .doc
                    .get(id)
                    .map(|n| n.name().to_string())
                    .unwrap_or_default();
                match self.session.selection.ids().len() {
                    0 | 1 => name,
                    2 => format!("{name} and 1 more"),
                    n => format!("{name} and {} more", n - 1),
                }
            }
        }
    }

    /// Run a command row: remembered in *Recent*, then dispatched as its menu row
    /// would be. The palette is already closed, so a dialog never opens under it.
    fn run_palette_item(&mut self, ctx: &egui::Context, item: Item, target: Target) {
        let key = verb_key(item);
        let recent = &mut self.prefs.palette_recent;
        recent.retain(|k| *k != key);
        recent.insert(0, key);
        recent.truncate(RECENT);
        self.prefs.save();
        // No pointer: a paste lands where the chord's does.
        if item == Item::PasteHere && target != Target::TextSession {
            self.dispatch(ctx, Action::Paste);
        } else {
            self.perform_menu_item(ctx, item, target, None);
        }
    }

    /// Select a layer as a click on its layers-panel row does, or add it with
    /// Shift, and bring it into view without changing the zoom. The panel opens
    /// its parents itself, on the selection change.
    fn pick_palette_layer(&mut self, id: NodeId, add: bool) {
        if add {
            if !self.session.selection.ids().contains(&id) {
                self.session.selection.toggle(id);
            }
        } else {
            self.session.selection.set_one(id);
        }
        self.layers_range_anchor = Some(id);
        self.reveal_selection();
    }

    /// Draw the open palette and answer this frame's keys and clicks.
    ///
    /// A modal with a clear backdrop: a click outside closes it and is not passed
    /// on, and nothing behind it dims.
    pub(crate) fn palette_ui(&mut self, ctx: &egui::Context, canvas: egui::Rect, keys: Keys) {
        let Some(mut pal) = self.palette.take() else {
            return;
        };
        if keys.cheatsheet {
            self.open_cheatsheet(crate::cheatsheet::Opened::Editor);
            return;
        }
        if self.modal_is_up() || keys.escape || keys.close {
            return;
        }
        if keys.tab {
            pal.layers_only = !pal.layers_only;
        }
        let target = self.palette_target();
        pal.snapshot.target = target;

        // Backspace on an empty field takes the Layers chip off; read before the
        // field, which would see nothing to delete.
        if pal.layers_only
            && pal.query.is_empty()
            && ctx.input(|i| i.key_pressed(egui::Key::Backspace))
        {
            pal.layers_only = false;
        }

        let sections = self.palette_sections(&pal);
        let count: usize = sections.iter().map(|s| s.entries.len()).sum();
        let now = (pal.query.clone(), pal.layers_only);
        if pal.reset_for.as_ref() != Some(&now) {
            pal.reset_for = Some(now);
            pal.highlight = sections
                .iter()
                .flat_map(|s| &s.entries)
                .position(|e| !matches!(e, Entry::Cmd(c) if !c.enabled))
                .unwrap_or(0);
        }
        if count == 0 {
            pal.highlight = 0;
        } else {
            pal.highlight = pal.highlight.min(count - 1);
            if let Some(step) = keys.step {
                let last = count - 1;
                pal.highlight = match step {
                    Step::Next if pal.highlight == last => 0,
                    Step::Next => pal.highlight + 1,
                    Step::Prev if pal.highlight == 0 => last,
                    Step::Prev => pal.highlight - 1,
                    Step::PageDown => (pal.highlight + PAGE).min(last),
                    Step::PageUp => pal.highlight.saturating_sub(PAGE),
                };
                pal.scroll = true;
            }
        }

        let run_of = |e: &Entry, add: bool| match e {
            Entry::Cmd(c) => c.enabled.then_some(Run::Cmd(c.item)),
            Entry::Layer(l) => Some(Run::Layer { id: l.id, add }),
            Entry::More(_) => Some(Run::More),
        };
        let mut run: Option<Run> = None;
        if keys.enter || keys.shift_enter {
            run = sections
                .iter()
                .flat_map(|s| &s.entries)
                .nth(pal.highlight)
                .and_then(|e| run_of(e, keys.shift_enter));
        }

        let note = self.palette_target_note(target);
        let screen = ctx.content_rect();
        let area = egui::Modal::default_area(egui::Id::new("command-palette")).anchor(
            egui::Align2::CENTER_TOP,
            egui::vec2(
                canvas.center().x - screen.center().x,
                canvas.top() - screen.top() + TOP_GAP,
            ),
        );
        let moved = ctx.input(|i| i.pointer.delta() != egui::Vec2::ZERO);
        let mut clicked: Option<usize> = None;
        let mut hovered: Option<usize> = None;
        let modal = egui::Modal::new(egui::Id::new("command-palette"))
            .area(area)
            .backdrop_color(egui::Color32::TRANSPARENT)
            .frame(
                egui::Frame::NONE
                    .fill(color::CARD)
                    .stroke(egui::Stroke::new(1.0, theme::color::DIVIDER))
                    .corner_radius(egui::CornerRadius::same(8))
                    .shadow(egui::epaint::Shadow {
                        offset: [0, 14],
                        blur: 30,
                        spread: 0,
                        color: egui::Color32::from_black_alpha(102),
                    }),
            )
            .show(ctx, |ui| {
                ui.set_width(W);
                ui.spacing_mut().item_spacing = egui::vec2(0.0, 0.0);
                palette_field(ui, &mut pal);
                divider(ui);
                let mut index = 0usize;
                egui::ScrollArea::vertical()
                    .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                    .max_height(LIST_MAX_H)
                    .auto_shrink([false, true])
                    .show(ui, |ui| {
                        egui::Frame::NONE
                            .inner_margin(egui::Margin::same(PAD as i8))
                            .show(ui, |ui| {
                                ui.spacing_mut().item_spacing.y = 1.0;
                                if count == 0 {
                                    no_match(ui, &pal);
                                }
                                for section in &sections {
                                    if let Some(title) = section.title {
                                        section_label(ui, title);
                                    }
                                    for entry in &section.entries {
                                        let lit = index == pal.highlight;
                                        let resp = match entry {
                                            Entry::Cmd(c) => cmd_row(ui, c, lit),
                                            Entry::Layer(l) => layer_row(ui, l, lit),
                                            Entry::More(n) => more_row(ui, *n, lit),
                                        };
                                        if lit && pal.scroll {
                                            resp.scroll_to_me(None);
                                        }
                                        if resp.clicked() {
                                            clicked = Some(index);
                                        }
                                        if resp.hovered() && moved {
                                            hovered = Some(index);
                                        }
                                        index += 1;
                                    }
                                }
                            });
                    });
                divider(ui);
                let flat = sections.iter().flat_map(|s| &s.entries);
                let why = match flat.clone().nth(pal.highlight) {
                    Some(Entry::Cmd(c)) if !c.enabled => c.why,
                    _ => None,
                };
                footer(ui, why, &note, pal.layers_only);
            });
        pal.scroll = false;
        if let Some(i) = hovered {
            pal.highlight = i;
        }
        if let Some(i) = clicked {
            pal.highlight = i;
            let add = ctx.input(|i| i.modifiers.shift);
            run = sections
                .iter()
                .flat_map(|s| &s.entries)
                .nth(i)
                .and_then(|e| run_of(e, add));
        }
        if modal.backdrop_response.clicked() {
            return;
        }
        match run {
            Some(Run::Cmd(item)) => {
                // Closed first: the verb may open a dialog or a modal.
                self.run_palette_item(ctx, item, target);
                ctx.request_repaint();
            }
            Some(Run::Layer { id, add }) => {
                self.pick_palette_layer(id, add);
                ctx.request_repaint();
            }
            Some(Run::More) => {
                pal.layers_only = true;
                self.palette = Some(pal);
            }
            None => self.palette = Some(pal),
        }
    }
}

/// The search field, with the *Layers* chip in front of it in that scope.
fn palette_field(ui: &mut egui::Ui, pal: &mut Palette) {
    ui.allocate_ui_with_layout(
        egui::vec2(W, FIELD_H),
        egui::Layout::left_to_right(egui::Align::Center),
        |ui| {
            ui.spacing_mut().item_spacing.x = 10.0;
            // The glyph sits where the rows' glyphs do: the list's inset plus a row's.
            ui.add_space(PAD + 8.0);
            ui.label(
                egui::RichText::new(icon::MAGNIFYING_GLASS)
                    .font(theme::icon_font(16.0))
                    .color(theme::text::FAINT),
            );
            if pal.layers_only {
                layers_chip(ui);
            }
            let resp = ui.add(
                egui::TextEdit::singleline(&mut pal.query)
                    .id(egui::Id::new("command-palette-field"))
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .font(egui::FontId::proportional(13.0))
                    .text_color(color::TEXT)
                    .hint_text(
                        egui::RichText::new(if pal.layers_only {
                            "Search layers"
                        } else {
                            "Search commands and layers"
                        })
                        .size(13.0)
                        .color(theme::text::FAINT),
                    ),
            );
            // Every frame: the palette owns the keyboard while it is open.
            if !resp.has_focus() {
                resp.request_focus();
            }
        },
    );
}

/// The hairline between the field, the list and the footer.
fn divider(ui: &mut egui::Ui) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W, 1.0), egui::Sense::empty());
    ui.painter()
        .rect_filled(rect, egui::CornerRadius::ZERO, theme::color::text_a(20));
}

/// A section's small-caps title.
fn section_label(ui: &mut egui::Ui, title: &str) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), SECTION_H),
        egui::Sense::empty(),
    );
    let mut job = egui::text::LayoutJob::single_section(
        title.to_uppercase(),
        egui::TextFormat {
            font_id: egui::FontId::proportional(9.0),
            color: theme::text::FAINT,
            extra_letter_spacing: 0.9,
            ..Default::default()
        },
    );
    job.wrap.max_rows = 1;
    let galley = ui.painter().layout_job(job);
    ui.painter().galley(
        egui::pos2(rect.left() + 8.0, rect.bottom() - 5.0 - galley.size().y),
        galley,
        theme::text::FAINT,
    );
}

/// `text` with `hits` in the accent, for one row's label.
fn lit_label(
    text: &str,
    hits: &[Range<usize>],
    size: f32,
    base: egui::Color32,
    hit: egui::Color32,
) -> egui::text::LayoutJob {
    let mut job = egui::text::LayoutJob::default();
    let plain = egui::TextFormat::simple(egui::FontId::proportional(size), base);
    let lit = egui::TextFormat::simple(egui::FontId::proportional(size), hit);
    let mut at = 0;
    for r in hits {
        if r.start < at || r.end > text.len() {
            continue;
        }
        job.append(&text[at..r.start], 0.0, plain.clone());
        job.append(&text[r.clone()], 0.0, lit.clone());
        at = r.end;
    }
    job.append(&text[at..], 0.0, plain);
    job.wrap = egui::text::TextWrapping {
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
        ..Default::default()
    };
    job
}

/// A chord as keycaps would read it in one chip: `Ctrl + K`.
fn chip(ui: &egui::Ui, right: f32, y: f32, accel: &str, live: bool) -> f32 {
    let p = ui.painter();
    let text = accel.replace('+', " + ");
    let galley = p.layout_no_wrap(
        text,
        egui::FontId::proportional(10.0),
        theme::color::text_a(if live { 125 } else { 70 }),
    );
    let size = galley.size() + egui::vec2(10.0, 4.0);
    let rect = egui::Rect::from_min_size(egui::pos2(right - size.x, y - size.y / 2.0), size);
    p.rect_filled(
        rect,
        egui::CornerRadius::same(4),
        theme::color::text_a(if live { 18 } else { 10 }),
    );
    p.galley(rect.min + egui::vec2(5.0, 2.0), galley, theme::text::FAINT);
    size.x
}

/// One command row: glyph or tick, the label with the query lit, the group it
/// comes from (or, for a row not available here, what it needs), the chord.
fn cmd_row(ui: &mut egui::Ui, c: &Cmd, lit: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::click(),
    );
    let p = ui.painter().clone();
    if lit {
        let fill = if c.enabled {
            highlight_fill()
        } else {
            theme::color::text_a(20)
        };
        p.rect_filled(rect, egui::CornerRadius::same(5), fill);
    }
    let live = c.enabled;
    let glyph_col = match () {
        _ if !live => theme::text::DISABLED,
        _ if lit => color::ACCENT_200,
        _ => theme::text::DIM,
    };
    let glyph_at = egui::pos2(rect.left() + 8.0 + 8.0, rect.center().y);
    match c.checked {
        Some(true) => {
            p.text(
                glyph_at,
                egui::Align2::CENTER_CENTER,
                icon::CHECK,
                theme::icon_font(14.0),
                if live {
                    color::ACCENT_300
                } else {
                    theme::text::DISABLED
                },
            );
        }
        Some(false) => {}
        None if c.filled_hexagon => crate::ui::paint_hexagon_filled(&p, glyph_at, 15.0, glyph_col),
        None => {
            p.text(
                glyph_at,
                egui::Align2::CENTER_CENTER,
                c.glyph,
                theme::icon_font(15.0),
                glyph_col,
            );
        }
    }

    // Right to left: chord, then the group or the need.
    let mut right = rect.right() - 8.0;
    if let Some(accel) = c.accel.filter(|_| !c.absent) {
        right -= chip(ui, right, rect.center().y, accel, live) + 8.0;
    }
    let side = if c.absent {
        c.why.unwrap_or_default()
    } else {
        c.group.label()
    };
    let side_g = p.layout_no_wrap(
        side.to_string(),
        egui::FontId::proportional(10.5),
        theme::color::text_a(if live { 97 } else { 79 }),
    );
    let side_w = side_g.size().x;
    p.galley(
        egui::pos2(right - side_w, rect.center().y - side_g.size().y / 2.0),
        side_g,
        theme::text::FAINT,
    );
    right -= side_w + 10.0;

    let label_x = rect.left() + 8.0 + 16.0 + 10.0;
    let (base, hit) = if live {
        (
            if lit {
                color::TEXT
            } else {
                theme::text::STRONG
            },
            color::ACCENT_300,
        )
    } else {
        (
            theme::color::text_a(97),
            color::ACCENT_300.gamma_multiply(0.6),
        )
    };
    let mut job = lit_label(&c.label, &c.hits, 12.0, base, hit);
    job.wrap.max_width = (right - label_x).max(0.0);
    let galley = p.layout_job(job);
    p.galley(
        egui::pos2(label_x, rect.center().y - galley.size().y / 2.0),
        galley,
        base,
    );
    resp
}

/// One layer row: its kind glyph, its name with the match lit, its parents
/// (cut from the left when long), and the panel's hidden or locked mark.
fn layer_row(ui: &mut egui::Ui, l: &LayerHit, lit: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H),
        egui::Sense::click(),
    );
    let p = ui.painter().clone();
    if lit {
        p.rect_filled(rect, egui::CornerRadius::same(5), highlight_fill());
    }
    p.text(
        egui::pos2(rect.left() + 16.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        l.glyph,
        theme::icon_font(15.0),
        l.tint,
    );
    let mut right = rect.right() - 8.0;
    let mark = if l.hidden {
        Some(icon::EYE_SLASH)
    } else if l.locked {
        Some(icon::LOCK_SIMPLE)
    } else {
        None
    };
    if let Some(mark) = mark {
        p.text(
            egui::pos2(right, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            mark,
            theme::icon_font(13.0),
            theme::text::FAINT,
        );
        right -= 13.0 + 8.0;
    }
    let label_x = rect.left() + 8.0 + 16.0 + 10.0;
    let mut job = lit_label(
        &l.name,
        &l.hits,
        12.0,
        if lit {
            color::TEXT
        } else {
            theme::text::STRONG
        },
        color::ACCENT_300,
    );
    // The name keeps at least half the row; the path takes what is left.
    job.wrap.max_width = ((right - label_x) * 0.6).max(0.0);
    let name = p.layout_job(job);
    let name_w = name.size().x;
    p.galley(
        egui::pos2(label_x, rect.center().y - name.size().y / 2.0),
        name,
        color::TEXT,
    );
    let room = right - (label_x + name_w + 12.0);
    if room > 20.0 && !l.path.is_empty() {
        let font = egui::FontId::proportional(10.5);
        // Cut from the left, a whole parent at a time: the nearest parent is the
        // one that tells two same-named layers apart.
        let mut parts: Vec<&str> = l.path.split(" / ").collect();
        let mut text = l.path.clone();
        let mut galley = p.layout_no_wrap(text.clone(), font.clone(), theme::text::FAINT);
        while galley.size().x > room && parts.len() > 1 {
            parts.remove(0);
            text = format!("… / {}", parts.join(" / "));
            galley = p.layout_no_wrap(text.clone(), font.clone(), theme::text::FAINT);
        }
        if galley.size().x <= room {
            p.galley(
                egui::pos2(
                    right - galley.size().x,
                    rect.center().y - galley.size().y / 2.0,
                ),
                galley,
                theme::text::FAINT,
            );
        }
    }
    resp
}

/// *N more layers*, which opens the layers-only scope.
fn more_row(ui: &mut egui::Ui, n: usize, lit: bool) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), ROW_H - 2.0),
        egui::Sense::click(),
    );
    let p = ui.painter().clone();
    if lit {
        p.rect_filled(rect, egui::CornerRadius::same(5), highlight_fill());
    }
    p.text(
        egui::pos2(rect.left() + 8.0 + 16.0 + 10.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        if n == 1 {
            "1 more layer".to_string()
        } else {
            format!("{n} more layers")
        },
        egui::FontId::proportional(11.0),
        theme::text::MUTED,
    );
    chip(ui, rect.right() - 8.0, rect.center().y, "Tab", true);
    resp
}

/// The list's place when nothing matches: one line naming the query back.
fn no_match(ui: &mut egui::Ui, pal: &Palette) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 40.0), egui::Sense::empty());
    let q = pal.query.trim();
    let text = match (pal.layers_only, q.is_empty()) {
        (true, true) => "This document has no layers".to_string(),
        (true, false) => format!("No layer matches “{q}”"),
        (false, _) => format!("No command or layer matches “{q}”"),
    };
    ui.painter().text(
        egui::pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(12.0),
        theme::text::DIM,
    );
}

/// The *Layers* chip in the field.
fn layers_chip(ui: &mut egui::Ui) {
    let font = egui::FontId::proportional(11.0);
    let galley = ui
        .painter()
        .layout_no_wrap("Layers".into(), font, color::ACCENT_100);
    let size = egui::vec2(galley.size().x + 7.0 + 12.0 + 5.0 + 7.0, 20.0);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::empty());
    let p = ui.painter();
    p.rect(
        rect,
        egui::CornerRadius::same(5),
        egui::Color32::from_rgba_unmultiplied(109, 140, 217, 56),
        egui::Stroke::new(
            1.0,
            egui::Color32::from_rgba_unmultiplied(109, 140, 217, 128),
        ),
        egui::StrokeKind::Inside,
    );
    p.text(
        egui::pos2(rect.left() + 7.0 + 6.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        icon::STACK,
        theme::icon_font(12.0),
        color::ACCENT_100,
    );
    p.galley(
        egui::pos2(
            rect.left() + 7.0 + 12.0 + 5.0,
            rect.center().y - galley.size().y / 2.0,
        ),
        galley,
        color::ACCENT_100,
    );
}

/// The target (or the highlighted row's reason) and the keys.
fn footer(ui: &mut egui::Ui, why: Option<&str>, note: &str, layers_only: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(W, FOOTER_H), egui::Sense::empty());
    let p = ui.painter();
    let font = egui::FontId::proportional(10.5);
    // The text's optical centre, a point above the box's.
    let y = rect.center().y - FOOTER_TEXT_LIFT;
    let keys = if layers_only {
        "Enter select    Shift+Enter add    Tab commands    Esc close"
    } else {
        "↑↓ move    Enter run    Tab layers    Esc close"
    };
    let keys_g = p.layout_no_wrap(keys.into(), font.clone(), theme::text::DIM);
    let keys_w = keys_g.size().x;
    p.galley(
        egui::pos2(rect.right() - 14.0 - keys_w, y - keys_g.size().y / 2.0),
        keys_g,
        theme::text::DIM,
    );
    let (text, col) = match why {
        Some(why) => (why, theme::text::MUTED),
        None => (note, theme::text::MUTED),
    };
    let mut job = egui::text::LayoutJob::single_section(
        text.to_string(),
        egui::TextFormat::simple(font, col),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: (rect.width() - 14.0 * 2.0 - keys_w - 16.0).max(0.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = p.layout_job(job);
    p.galley(
        egui::pos2(rect.left() + 14.0, y - galley.size().y / 2.0),
        galley,
        col,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn q(s: &str) -> Vec<String> {
        query_words(s)
    }

    /// A rectangle named *Card* at the top of the document's root.
    pub(crate) fn add_rect(app: &mut OndinApp) -> NodeId {
        add_named(app, "Card")
    }

    /// A rectangle named `name` at the top of the document's root.
    fn add_named(app: &mut OndinApp, name: &str) -> NodeId {
        use ondin_core::{NodeKind, Operation, Transaction};
        let s = &mut app.session;
        let id = s.ids.mint();
        let parent = s.doc.first_page();
        let index = s.doc.get(parent).map_or(0, |n| n.children().len());
        assert!(s.commit(Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Rect {
                size: ondin_core::kurbo::Size::new(10.0, 10.0),
                corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some(name.into()),
        }])));
        id
    }

    /// Query words match word starts in any order, each taking a different word.
    #[test]
    fn words_match_word_starts_in_any_order() {
        let (r, hits) = rank("Align horizontal centres", &[], &q("al c")).expect("matches");
        assert_eq!(r, 1);
        assert_eq!(hits, vec![0..2, 17..18]);
        assert!(rank("Align horizontal centres", &[], &q("c al")).is_some());
        // One label word cannot answer two query words.
        assert!(rank("Group selection", &[], &q("g g")).is_none());
        // Inside a word is not a start.
        assert!(rank("Ungroup", &[], &q("group")).is_none());
        // A query of no words matches nothing.
        assert!(rank("Cut", &[], &q("🔥")).is_none());
    }

    /// A prefix of the whole label outranks scattered word starts, which outrank
    /// an alias.
    #[test]
    fn a_whole_prefix_outranks_word_starts_and_aliases() {
        assert_eq!(rank("Bring to front", &[], &q("bring to f")).unwrap().0, 0);
        assert_eq!(rank("Bring to front", &[], &q("bring f")).unwrap().0, 1);
        assert_eq!(rank("Bring to front", &[], &q("front")).unwrap().0, 1);
        let (r, hits) = rank("Copy as SVG", &["svg", "vector"], &q("vector")).unwrap();
        assert_eq!((r, hits.len()), (2, 0));
    }

    /// One frame of the whole app, with these events and modifiers held.
    fn pass(
        ctx: &egui::Context,
        app: &mut OndinApp,
        events: Vec<egui::Event>,
        modifiers: egui::Modifiers,
    ) {
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
    }

    fn key(key: egui::Key, modifiers: egui::Modifiers) -> egui::Event {
        egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers,
        }
    }

    const CTRL: egui::Modifiers = egui::Modifiers {
        alt: false,
        ctrl: true,
        shift: false,
        mac_cmd: false,
        command: true,
    };

    /// Through real frames: `Ctrl+K` opens the palette, typing filters it, and
    /// `Enter` closes it and runs the top row — here *Show rulers*.
    ///
    /// flip: dropping `discovery_chord` from `normal_mode` → fails at "Ctrl+K opens".
    #[test]
    fn ctrl_k_typing_and_enter_run_the_top_row() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let none = egui::Modifiers::NONE;
        pass(&ctx, &mut app, Vec::new(), none);
        pass(&ctx, &mut app, vec![key(egui::Key::K, CTRL)], CTRL);
        assert!(app.palette.is_some(), "Ctrl+K opens the palette");
        // A frame for the field to take focus before the text arrives.
        pass(&ctx, &mut app, Vec::new(), none);
        pass(
            &ctx,
            &mut app,
            vec![egui::Event::Text("show rul".into())],
            none,
        );
        assert_eq!(
            app.palette.as_ref().map(|p| p.query.as_str()),
            Some("show rul")
        );
        let before = app.show_rulers;
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, none)], none);
        assert!(app.palette.is_none(), "Enter closes the palette");
        assert_ne!(app.show_rulers, before, "and runs Show rulers");
        assert_eq!(
            app.prefs.palette_recent.first().map(String::as_str),
            Some("ViewSwitch(Rulers)")
        );
    }

    /// *Show Transform* opens the card and puts the keyboard in its first field.
    ///
    /// flip: dropping the `request_focus` in `panel_full`'s reveal → fails at
    /// "a field has the keyboard".
    #[test]
    fn show_card_focuses_the_cards_first_field() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let id = add_rect(&mut app);
        app.session.selection.set_one(id);
        app.collapsed_panels.insert("Transform");
        let none = egui::Modifiers::NONE;
        for _ in 0..3 {
            pass(&ctx, &mut app, Vec::new(), none);
        }
        assert!(
            app.cards_shown.contains(&"Transform"),
            "the fixture shows a Transform card: {:?}",
            app.cards_shown
        );
        app.toggle_palette(&ctx);
        pass(&ctx, &mut app, Vec::new(), none);
        pass(
            &ctx,
            &mut app,
            vec![egui::Event::Text("show tra".into())],
            none,
        );
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, none)], none);
        assert!(app.palette.is_none());
        for _ in 0..3 {
            pass(&ctx, &mut app, Vec::new(), none);
        }
        assert!(
            !app.collapsed_panels.contains("Transform"),
            "the card opened"
        );
        let focused = ctx.memory(|m| m.focused());
        assert!(focused.is_some(), "a field has the keyboard");
        assert_ne!(focused, Some(egui::Id::new("command-palette-field")));
    }

    /// `Escape` closes the palette and nothing else: the selection stays.
    #[test]
    fn escape_closes_only_the_palette() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let id = add_rect(&mut app);
        app.session.selection.set_one(id);
        let none = egui::Modifiers::NONE;
        pass(&ctx, &mut app, Vec::new(), none);
        app.toggle_palette(&ctx);
        pass(&ctx, &mut app, Vec::new(), none);
        pass(&ctx, &mut app, vec![key(egui::Key::Escape, none)], none);
        assert!(app.palette.is_none());
        assert_eq!(app.session.selection.ids(), &[id]);
    }

    /// A layer's match lights the substring, case folded.
    #[test]
    fn a_layer_hit_lights_its_substring() {
        assert_eq!(substring_hit("Pricing Card", "card"), vec![8..12]);
        assert!(substring_hit("Pricing", "x").is_empty());
    }

    /// The palette's rows on a rectangle: its own menu's, with *Paste here*
    /// renamed, and the verbs its menu leaves out kept apart as absent.
    ///
    /// flip: dropping `cmd.absent = true` in `palette_commands` → fails at the
    /// *Ungroup* assertion.
    #[test]
    fn a_rectangles_rows_keep_the_left_out_verbs_apart() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let id = add_rect(&mut app);
        app.session.selection.set_one(id);
        app.toggle_palette(&ctx);
        let pal = app.palette.take().expect("the palette opened");
        assert_eq!(
            pal.snapshot.target,
            Target::Layer {
                id,
                door: Door::Panel
            }
        );
        let cmds = app.palette_commands(&pal);
        let find = |item| cmds.iter().find(|c| c.item == item);
        let paste = find(Item::PasteHere).expect("a paste row");
        assert_eq!(paste.label, "Paste");
        assert!(paste.own);
        let ungroup = find(Item::Ungroup).expect("ungroup is offered as absent");
        assert!(ungroup.absent && !ungroup.enabled);
        assert_eq!(ungroup.why, Some("Needs a group or a boolean"));
        assert!(find(Item::MakeKeyLayer).is_none());
        assert!(
            find(Item::ExportCode(ondin_export::code::Target::React)).is_some_and(|c| c.enabled)
        );
    }

    /// Open the palette through real frames and type `text` into it.
    fn type_into_palette(ctx: &egui::Context, app: &mut OndinApp, text: &str) {
        let none = egui::Modifiers::NONE;
        pass(ctx, app, Vec::new(), none);
        app.toggle_palette(ctx);
        pass(ctx, app, Vec::new(), none);
        pass(ctx, app, vec![egui::Event::Text(text.into())], none);
        // The rows are built before the field reads its text: one more frame
        // builds them for the query.
        pass(ctx, app, Vec::new(), none);
        assert_eq!(
            app.palette.as_ref().map(|p| p.query.as_str()),
            Some(text),
            "fixture: the query was typed"
        );
    }

    /// **A query with no words answers no command**: `🔥` lists the layer it
    /// names and Enter selects it, rather than running the first verb (*Cut*).
    /// flip: `rank` answering an empty word list with a match → fails at "the
    /// card was not cut".
    #[test]
    fn a_wordless_query_finds_its_layer_and_runs_no_command() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let card = add_rect(&mut app);
        let hero = add_named(&mut app, "🔥 Hero");
        app.session.selection.set_one(card);
        type_into_palette(&ctx, &mut app, "🔥");
        let none = egui::Modifiers::NONE;
        pass(&ctx, &mut app, vec![key(egui::Key::Enter, none)], none);
        assert!(app.session.doc.get(card).is_some(), "the card was not cut");
        assert_eq!(app.session.selection.ids(), &[hero]);
    }

    /// **The cheatsheet chord swaps the palette for the sheet**, in both
    /// spellings a keyboard can send it.
    /// flip: `read_keys` not reading the cheatsheet chord → fails at "Slash".
    #[test]
    fn the_cheatsheet_chord_swaps_the_palette_for_the_sheet() {
        let shift_ctrl = CTRL | egui::Modifiers::SHIFT;
        for (spelling, k) in [
            ("Slash", egui::Key::Slash),
            ("Questionmark", egui::Key::Questionmark),
        ] {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let mut app = OndinApp::headless(&ctx);
            type_into_palette(&ctx, &mut app, "x");
            pass(&ctx, &mut app, vec![key(k, shift_ctrl)], shift_ctrl);
            pass(&ctx, &mut app, Vec::new(), egui::Modifiers::NONE);
            assert!(app.palette.is_none(), "{spelling}: the palette closed");
            assert!(app.cheatsheet.is_some(), "{spelling}: the sheet opened");
        }
    }

    /// **The rows act on the topmost selected layer** in panel order when there
    /// is no key layer, whichever order the layers were selected in.
    /// flip: `topmost_selected` answering `sel.first()` → fails here.
    #[test]
    fn the_target_is_the_topmost_selected_layer() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let (a, b, c) = (
            add_named(&mut app, "A"),
            add_named(&mut app, "B"),
            add_named(&mut app, "C"),
        );
        app.session.selection.set(vec![a, c, b]);
        assert_eq!(app.session.selection.key(), None, "fixture: no key layer");
        assert_eq!(
            app.palette_target(),
            Target::Layer {
                id: c,
                door: Door::Panel
            }
        );
    }

    /// **The name rule lists layers top to bottom as the panel does**, case
    /// folded, parents before their children, the root included.
    /// flip: `name_hits` pushing children reversed → fails at the order.
    #[test]
    fn name_hits_lists_in_panel_order_case_folded() {
        use ondin_core::{NodeKind, Operation, Transaction};
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let low = add_named(&mut app, "card low");
        let s = &mut app.session;
        let (group, inner) = (s.ids.mint(), s.ids.mint());
        let root = s.doc.first_page();
        let top = s.doc.get(root).map_or(0, |n| n.children().len());
        assert!(s.commit(Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: top,
                kind: NodeKind::Group,
                transform: None,
                name: Some("Card group".into()),
            },
            Operation::CreateNode {
                id: inner,
                parent: group,
                index: 0,
                kind: NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(10.0, 10.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("CARD inner".into()),
            },
        ])));
        let hits = crate::panels::layers::name_hits(&app.session.doc, app.session.page, "Card");
        assert_eq!(hits, vec![group, inner, low]);
    }

    /// **The highlight starts on the first live row** when the best-ranked row
    /// is dim, so Enter runs something.
    /// flip: the `reset_for` block highlighting index 0 → fails here.
    #[test]
    fn the_highlight_skips_a_dim_top_row() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        type_into_palette(&ctx, &mut app, "re");
        let pal = app.palette.take().expect("open");
        let sections = app.palette_sections(&pal);
        let entries: Vec<&Entry> = sections.iter().flat_map(|s| &s.entries).collect();
        let live = entries
            .iter()
            .position(|e| !matches!(e, Entry::Cmd(c) if !c.enabled))
            .expect("fixture: a live row");
        assert!(live > 0, "fixture: the top row is dim");
        assert_eq!(pal.highlight, live);
    }

    /// **Publish kit… dims outside a kit**, as the document menu's row does.
    /// flip: `palette_commands` passing no reason for it → fails here.
    #[test]
    fn publish_kit_is_dim_outside_a_kit() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.toggle_palette(&ctx);
        let pal = app.palette.take().expect("open");
        let cmds = app.palette_commands(&pal);
        let publish = cmds
            .iter()
            .find(|c| c.item == Item::PublishKit)
            .expect("a publish row");
        assert!(!publish.enabled);
        assert_eq!(publish.why, Some("Make this document a kit first"));
    }

    /// The Document menu's rows and the quarter turns are commands: *Make this a kit*
    /// and *Show in library* live in a plain document, the two that need a kit or a
    /// vendored main dim, the turns dim with nothing selected, and a turn runs.
    /// flip: `palette_commands` without the quarter turns → fails at the turn rows.
    #[test]
    fn the_document_menu_and_the_quarter_turns_are_in_the_palette() {
        use ondin_core::{IdSource, NodeKind, Operation, Transaction};
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.toggle_palette(&ctx);
        let pal = app.palette.take().expect("open");
        let cmds = app.palette_commands(&pal);
        let row = |item: Item| {
            cmds.iter()
                .find(|c| c.item == item)
                .unwrap_or_else(|| panic!("no {item:?} row"))
        };
        assert!(row(Item::MakeKit).enabled && row(Item::ShowInLibrary).enabled);
        assert!(row(Item::VersionHistory).enabled);
        assert!(!row(Item::StopSharingKit).enabled && !row(Item::KitsUsed).enabled);
        for clockwise in [false, true] {
            assert!(
                !row(Item::RotateQuarter(clockwise)).enabled,
                "nothing selected"
            );
        }
        assert_eq!(
            row(Item::MakeKit).label,
            "Make this a kit",
            "the menu's own words"
        );

        let mut ids = IdSource::new(0x9A1);
        let id = ids.mint();
        let root = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: NodeKind::Rect {
                size: ondin_core::kurbo::Size::new(20.0, 10.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        }])));
        app.session.selection.set_one(id);
        app.dispatch(&ctx, crate::input::Action::RotateQuarter(true));
        let c = app.session.doc.get(id).unwrap().transform().as_coeffs();
        assert!(
            (c[0]).abs() < 1e-9 && (c[1] - 1.0).abs() < 1e-9,
            "a quarter turn clockwise, got {c:?}"
        );
    }
}
