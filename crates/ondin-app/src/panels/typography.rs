//! The Type panel and the typography popup (§9.2).
//!
//! The panel holds the controls a designer reaches for constantly — family,
//! variant, size, line height, letter spacing, the two alignments — and the popup
//! is strictly overflow. No control appears in both: a duplicate would have to show
//! "mixed" twice.
//!
//! The popup's tabs are scopes ([`TypeTab`]) — Character, Paragraph, Box — labelled
//! with the scope word because that tells the user what a change hits when the
//! selection is partial.
//!
//! ## Scope, and where a write goes
//!
//! [`TypeSubject::range`] decides a character attribute's target:
//!
//! - **A live editing session with a selection** — the range is that selection, the
//!   control can read *mixed*, and the write goes into the session's own spans
//!   (`TextEdit::style_selection`), committed once when the session ends (§9.3).
//! - **Anything else** — the range is the whole node, and the write sets the node's
//!   default and drops that attribute's overrides, so "make it all 24pt" leaves no
//!   16pt run behind.
//!
//! Seven paragraph attributes (the spacing, first-line indent, `hanging`, the two
//! block indents, the list marker and its nesting level) have the same two targets
//! through [`TypeSubject::para_range`]:
//!
//! - A paragraph attribute cannot apply to half a paragraph, so the range is the
//!   selection snapped outward to hard breaks (`text::paragraph_bounds`).
//! - A bare caret is partial here: it is inside one paragraph, so an open session
//!   always writes a paragraph span, and the node's paragraph defaults are reachable
//!   only with the layer selected and no session running. A character write at a
//!   bare caret falls to the node's defaults.
//!
//! `Alignment`, `Justify`, the base direction and all of the block scope have one
//! target either way (no span type carries them); the Paragraph tab sections that
//! edit them say so on hover.

use super::component::{OverrideMark, mark_num};
use super::inspector::tip_unless_marked;
use super::paint::{CharSlot, CharWrite, DecorationSide};
use super::{ClickAway, dismissed_by_click};
use crate::app::{OndinApp, TypeTab};
use crate::expr;
use crate::input::TextChord;
use crate::theme::{self, icon};
use crate::ui::{
    self, Prefix, Scrub, Suffix, field_button, field_row, icon_button, segment_glyph,
    segment_label, segment_mixed, segmented, value_field, value_field_suffixed,
};
use eframe::egui;
use ondin_core::Brush;
use ondin_core::kurbo::Size;
use ondin_core::peniko::Color;
use ondin_core::style::StyleSlot;
use ondin_core::{
    AXES_DRIVEN_ELSEWHERE, AxisSetting, BlockStyle, BoxTrim, CharAttr, CharAttrKind, CharSpans,
    Decoration, FaceFeature, FeatureSetting, FontAxis, FontVariant, GeometryPatch, JustifyLast,
    Length, LengthUnit, LineStyle, ListMarker, MAX_FONT_SIZE, MAX_LINE_LIMIT, MAX_LIST_LEVEL,
    MIN_FONT_SIZE, NodeId, OPSZ, Operation, OverflowWrap, ParaAttr, ParaAttrKind, ParaSpans,
    ParagraphStyle, Tag, TextAlign, TextCase, TextDirection, TextOverflow, TextRef, TextSizing,
    TextStyle, Transaction, VerticalAlign, WordBreak, WrapMode, character_variant, stylistic_set,
    text,
};
use std::ops::Range;

/// Everything the Type panel edits, snapshotted while the document borrow is
/// still open so the controls can mutate afterwards.
#[derive(Clone)]
pub(crate) struct TypeSubject {
    pub id: NodeId,
    pub style: TextStyle,
    pub spans: CharSpans,
    pub paragraph: ParagraphStyle,
    pub block: BlockStyle,
    pub sizing: TextSizing,
    /// The paint the text itself is drawn in — the node's first visible fill, or
    /// `None` where it has no visible fill at all.
    ///
    /// Carried so a decoration's inherit state can be drawn. The whole brush,
    /// gradient included, because the renderer inherits it;
    /// [`TypeSubject::text_color`] collapses it to one colour.
    pub text_paint: Option<Brush>,
    /// What the shaped layout's first line measures, in px, so the line height
    /// field can show what `Auto` is worth. `None` for a node with nothing laid out.
    pub line_height: Option<f64>,
    /// The face's own underline and strikethrough thicknesses in px
    /// (`TextLayout::decoration_sizes`), so the thickness chip seeds from the font
    /// instead of zero, which erased the line.
    pub decoration_sizes: Option<(f64, f64)>,
    /// The bytes a character control acts on — see the module docs.
    pub range: Range<usize>,
    /// Where [`Self::shown`] resolves an attribute when the range disagrees with
    /// itself; `None` means the node's defaults.
    ///
    /// For a selection it is `range.start`; for a bare caret the byte to the caret's
    /// left, because an insertion joins the run that ends there (byte 0 has none).
    /// Readout only: a bare caret's character write still sets the node's default.
    pub resolve_at: Option<usize>,
    /// Whether a live session has a non-empty selection (select-all included) —
    /// the only case in which a character control can read mixed.
    pub partial: bool,
    /// The node's per-paragraph overrides — the live session's own while one is
    /// open, since the document's copy is a commit behind.
    pub para_spans: ParaSpans,
    /// The bytes a paragraph control acts on: the selection snapped outward to
    /// hard breaks.
    pub para_range: Range<usize>,
    /// Whether the write goes into a live session's overrides rather than the
    /// node's paragraph defaults — the paragraph scope's twin of `partial`.
    ///
    /// True for any open session with a paragraph to name, false for an empty node.
    pub para_partial: bool,
    /// The other selected text layers a several-layer Type card writes beside `id`
    /// — each a whole-node, never-partial subject. Empty on a single layer's card.
    ///
    /// [`Self::mixed`] reads them, and [`char_attrs_tx`] writes each its own
    /// defaults with only the one attribute changed.
    pub others: Vec<TypeSubject>,
    /// The node's text bindings: a whole-node write keeps the span lists against
    /// a bound default's literal, not what it draws (`variable::span_defaults`).
    pub text_vars: ondin_core::TextVars,
}

impl TypeSubject {
    /// The panel's subject for `id`, or `None` if it is not a text node, read
    /// through the preview so every control tracks a live drag.
    pub(crate) fn of(app: &OndinApp, id: NodeId) -> Option<Self> {
        Self::read(app, id, app.session.display_node(id)?)
    }

    /// [`Self::of`] over the **committed** document, for a write that must not
    /// compare itself against its own preview — see `Session::committed_node`.
    pub(crate) fn committed(app: &OndinApp, id: NodeId) -> Option<Self> {
        Self::read(app, id, app.session.committed_node(id)?)
    }

    fn read(app: &OndinApp, id: NodeId, node: crate::session::DisplayNode<'_>) -> Option<Self> {
        let kind = node.kind().clone();
        let text_paint = node
            .paint()
            .fills
            .iter()
            .find(|f| f.visible)
            .map(|f| app.session.doc.resolve_brush(id, &f.brush).into_owned());
        let parts = TextRef::of(&kind)?;
        let len = parts.content.len();
        // A live session's selection and spans win: the document's copy is a commit behind.
        let live = app.text.as_ref().filter(|s| s.id == id);
        let (range, spans, partial, resolve_at) =
            match live.map(|s| (s.editor.selected_range(), s.editor.spans())) {
                Some((sel, spans)) if !sel.is_empty() => {
                    let at = sel.start;
                    (sel, spans.clone(), true, Some(at))
                }
                // The bare caret: the readout describes the run to its left.
                Some((sel, spans)) => (0..len, spans.clone(), false, sel.start.checked_sub(1)),
                None => (0..len, parts.spans.clone(), false, Some(0)),
            };
        // The editor's content is a commit ahead of the document's string.
        let (para_range, para_spans, para_partial) = match live {
            Some(s) => {
                let range = s.editor.paragraph_range();
                let partial = !range.is_empty();
                (range, s.editor.para_spans().clone(), partial)
            }
            None => (0..len, parts.para_spans.clone(), false),
        };
        Some(TypeSubject {
            id,
            style: parts.style.clone(),
            spans,
            paragraph: parts.paragraph.clone(),
            block: *parts.block,
            sizing: *parts.sizing,
            text_paint,
            line_height: app
                .session
                .preview_text_layout(id)
                .map(|l| l.line_height)
                .filter(|h| *h > 0.0),
            decoration_sizes: app
                .session
                .preview_text_layout(id)
                .and_then(|l| l.decoration_sizes),
            range,
            resolve_at,
            partial,
            para_spans,
            para_range,
            para_partial,
            others: Vec::new(),
            text_vars: app
                .session
                .doc
                .get(id)
                .map(|n| n.text_vars())
                .unwrap_or_default(),
        })
    }

    /// The subject of a several-layer Type card: the first text layer of `ids`,
    /// the rest as [`Self::others`]. `None` under two text layers or while a live
    /// session runs, whose selection is one layer's.
    pub(crate) fn several(app: &OndinApp, ids: &[NodeId]) -> Option<Self> {
        if ids.len() < 2 || app.text.is_some() {
            return None;
        }
        let mut subjects = ids.iter().filter_map(|id| Self::of(app, *id));
        let mut first = subjects.next()?;
        first.others = subjects.collect();
        (!first.others.is_empty()).then_some(first)
    }

    /// Every layer the subject writes — `id`, then [`Self::others`]'.
    pub(crate) fn ids(&self) -> Vec<NodeId> {
        std::iter::once(self.id)
            .chain(self.others.iter().map(|o| o.id))
            .collect()
    }

    /// The one colour the text's paint collapses to, for controls that hold only
    /// one: the first stop, and black where there is no fill. Both match
    /// [`OndinApp::write_char_slot`]'s collapse and what `scene.rs` paints.
    pub fn text_color(&self) -> Color {
        match &self.text_paint {
            Some(brush) => super::paint::stops_of(brush)
                .first()
                .map_or(Color::BLACK, |(_, c)| *c),
            None => Color::BLACK,
        }
    }

    /// The stops to *draw* the inherited paint with, or `None` when it is a plain
    /// colour that [`Self::text_color`] already covers, built through
    /// `picker::ramp` like every other gradient chip.
    fn text_ramp(&self) -> Option<Vec<(f32, egui::Color32)>> {
        let brush = self.text_paint.as_ref()?;
        if matches!(brush, Brush::Solid(_)) {
            return None;
        }
        Some(super::picker::ramp(brush))
    }

    /// The single value of one character attribute over the subject's range, or
    /// `None` when the range disagrees with itself.
    ///
    /// Two zeroes in different units are one value here: `Spans::shared_in`
    /// compares structurally, but `Em(0.0)` and `Px(0.0)` must not read *Mixed*.
    fn shared(&self, kind: CharAttrKind) -> Option<CharAttr> {
        let direct = self.spans.shared_in(&self.style, kind, self.range.clone());
        direct.or_else(|| {
            agreed_zero(
                self.spans.values_in(&self.style, kind, self.range.clone()),
                |a| match a {
                    CharAttr::LetterSpacing(l)
                    | CharAttr::WordSpacing(l)
                    | CharAttr::BaselineShift(l) => Some(*l),
                    _ => None,
                },
            )
        })
    }

    /// The value to show for an attribute: the shared one, or the value at
    /// [`Self::resolve_at`] where the range disagrees, so a drag has somewhere to
    /// start. Callers pair this with [`Self::shared`] to decide on "mixed".
    fn shown(&self, kind: CharAttrKind) -> CharAttr {
        self.shared(kind).unwrap_or_else(|| match self.resolve_at {
            Some(at) => self.spans.resolve(&self.style, at).get(kind),
            None => self.style.get(kind),
        })
    }

    /// Whether a control may not name one value: a partial range that disagrees
    /// with itself, or — on a several-layer card — layers that disagree (two
    /// zeroes in different units agree, as in [`Self::shared`]).
    fn mixed(&self, kind: CharAttrKind) -> bool {
        (self.partial && self.shared(kind).is_none())
            || self.others.iter().any(|o| {
                let (a, b) = (self.shown(kind), o.shown(kind));
                a != b && agreed_zero(vec![a, b], char_length).is_none()
            })
    }

    /// Whether the range disagrees with itself about one attribute.
    ///
    /// Not [`Self::mixed`], which is false over the whole node however ragged the
    /// spans. A reset asks whether there is anything to put back, and `shown` reads
    /// byte 0 only.
    fn ragged(&self, kind: CharAttrKind) -> bool {
        self.shared(kind).is_none()
    }

    /// Whether the range disagrees about one OpenType tag.
    ///
    /// Per tag, not per list: [`Self::mixed`] on `Features` is true once any tag
    /// differs, which would dim every row. An absent tag is `0`.
    fn feature_mixed(&self, tag: Tag) -> bool {
        if !self.partial {
            return false;
        }
        let mut seen: Option<u16> = None;
        for attr in self
            .spans
            .values_in(&self.style, CharAttrKind::Features, self.range.clone())
        {
            let CharAttr::Features(list) = attr else {
                continue;
            };
            let v = list.iter().find(|f| f.tag == tag).map_or(0, |f| f.value);
            match seen {
                None => seen = Some(v),
                Some(prev) if prev == v => {}
                Some(_) => return true,
            }
        }
        false
    }

    /// Whether the line height field may not name one value: [`Self::mixed`],
    /// and — on a several-layer card all on Auto — layers whose laid lines differ,
    /// since Auto is one stored value but the field shows what it is worth in px.
    fn line_height_mixed(&self) -> bool {
        if self.mixed(CharAttrKind::LineHeight) {
            return true;
        }
        let auto = |s: &TypeSubject| {
            matches!(
                s.shown(CharAttrKind::LineHeight),
                CharAttr::LineHeight(None)
            )
        };
        auto(self)
            && self
                .others
                .iter()
                .any(|o| match (self.line_height, o.line_height) {
                    (Some(a), Some(b)) => (a - b).abs() > 0.005,
                    (a, b) => a.is_some() != b.is_some(),
                })
    }

    fn font_size(&self) -> f64 {
        match self.shown(CharAttrKind::Size) {
            CharAttr::Size(v) => v,
            _ => self.style.font_size,
        }
    }

    // --- the paragraph scope, mirroring the four above ---------------------

    /// [`Self::shared`] for the paragraph scope, including its zero rule over the
    /// four `ParaAttr` lengths.
    fn para_shared(&self, kind: ParaAttrKind) -> Option<ParaAttr> {
        let direct = self
            .para_spans
            .shared_in(&self.paragraph, kind, self.para_range.clone());
        direct.or_else(|| {
            agreed_zero(
                self.para_spans
                    .values_in(&self.paragraph, kind, self.para_range.clone()),
                |a| match a {
                    ParaAttr::Spacing(l)
                    | ParaAttr::Indent(l)
                    | ParaAttr::IndentStart(l)
                    | ParaAttr::IndentEnd(l) => Some(*l),
                    _ => None,
                },
            )
        })
    }

    fn para_mixed(&self, kind: ParaAttrKind) -> bool {
        self.para_partial && self.para_shared(kind).is_none()
    }

    /// Whether the range disagrees with itself about one attribute — [`Self::ragged`]
    /// for the paragraph scope.
    ///
    /// The hanging pair's click needs it: its change test compares against
    /// [`Self::shown_paragraph`], which over a non-partial range reads the first
    /// paragraph, so an override on a later one made the click write nothing.
    fn para_ragged(&self, kind: ParaAttrKind) -> bool {
        self.para_shared(kind).is_none()
    }

    /// The paragraph style the tab's spannable controls **show**: the node's
    /// defaults with the current paragraph's overrides applied.
    ///
    /// Not what the node-level controls write from: writing a resolved value back
    /// would promote one paragraph's indent to the node's default. It resolves at
    /// the range's start; [`Self::para_ragged`] answers what this does not.
    fn shown_paragraph(&self) -> ParagraphStyle {
        self.para_spans
            .resolve(&self.paragraph, self.para_range.start)
    }
}

/// Unpack one attribute's value, panicking only on a programming error (the kind
/// asked for and the kind returned always match, by construction of
/// [`TextStyle::get`]).
macro_rules! attr {
    ($subject:expr, $kind:ident, $pat:ident) => {
        match $subject.shown(CharAttrKind::$kind) {
            CharAttr::$pat(v) => v,
            other => unreachable!("asked for {:?}, got {other:?}", CharAttrKind::$kind),
        }
    };
}

/// Panel geometry. The card's own width, minus its margins, is what everything
/// here divides up.
const ROW_H: f32 = ui::CONTROL_H;
const GAP: f32 = ui::CARD_COL_GAP;
/// The cell height that makes a [`ui::segmented`] track paint [`ROW_H`] tall; the
/// track adds its own 2pt of padding above and below.
const SEG_CELL_H: f32 = ui::SEGMENT_CELL_H;

/// What one alignment-track cell comes out at on a 284pt card.
#[cfg(test)]
const SEG_CELL: f32 = 28.0;

/// The cell width of a `ui::segmented` track of `n` cells: a 2pt gap between each
/// pair, and 3pt of padding plus hairline at each end.
#[cfg(test)]
fn seg_cell_of(track: f32, n: usize) -> f32 {
    (track - 6.0 - 2.0 * (n as f32 - 1.0)) / n as f32
}
/// Height of one row in the family list. Fixed because `ScrollArea::show_rows`
/// virtualizes by dividing the scroll offset by it.
const FAMILY_ROW_H: f32 = 24.0;

/// Room a family row keeps at its right edge for the loading dot, reserved on every
/// row so the name does not reflow when its face arrives.
const LOADING_DOT_SLOT: f32 = 14.0;
/// The loading dot's radius; it pulses rather than spins, as many are on screen.
const LOADING_DOT_R: f32 = 2.5;
/// Seconds for one full breath of the loading dot.
const LOADING_DOT_PERIOD: f64 = 1.2;

/// Popup geometry. Every tab draws into the same box, so the width is pinned: a
/// card that resized itself would jump on each tab change.
const MENU_W: f32 = crate::app::POPOVER_W;
const MENU_PAD: f32 = 11.0;
/// The room inside the padding and the border, which everything below divides up.
const MENU_INNER: f32 = crate::ui::menu_inner_w(MENU_W, MENU_PAD);
/// A field, and the side of the square buttons that sit beside one — the same
/// height as the card the popup hangs beside.
const CELL: f32 = ui::CONTROL_H;
/// A segmented cell. The track it sits in paints 2px larger on every side.
const SEG_H: f32 = ui::SEGMENT_CELL_H;
/// The gutter turning a list marker on opens, when the paragraph has none, in px
/// (see [`OndinApp::list_section`]). A marker is right-aligned on the paragraph's
/// start edge, so with no indent it draws outside the box.
const DEFAULT_LIST_GUTTER: f64 = 24.0;
/// What the Paragraph tab's two node-level sections say on hover: `Layout::align`
/// and the base direction are one value for the whole layout.
const WHOLE_LAYER: &str = "The whole text layer — alignment and direction cannot vary by paragraph";

/// Between one section and the next; one number for every popover in the app.
const SECTION_GAP: f32 = ui::POPOVER_SECTION_GAP;
/// Between a section's label and its controls; control-to-control is
/// [`ui::CARD_ROW_GAP`].
const LABEL_GAP: f32 = ui::SECTION_LABEL_GAP;
/// Between two controls side by side in a row — the Type panel's own [`GAP`], so the
/// popup and the card it hangs from agree on both axes.
const COL_GAP: f32 = GAP;
/// How tall a list inside the popup may grow before it scrolls — one limit for the
/// axes and the features, so the card's height does not depend on the family.
const LIST_MAX_H: f32 = 104.0;
/// The numeric field on an axis row. Wide enough for the tag prefix and four
/// digits, which covers every registered axis's range.
const AXIS_FIELD_W: f32 = 74.0;
/// One row of the OpenType feature list.
const FEATURE_ROW_H: f32 = 22.0;
/// The ✕ beside the tab strip, and so the width the strip gives up to it.
const CLOSE_BUTTON_W: f32 = 18.0;
/// The magnifier in the feature search field, and the gap after it — the field
/// gap the value fields use at both ends of their number.
const SEARCH_ICON_PT: f32 = 14.0;
const SEARCH_GAP: f32 = 7.0;
/// The tab strip's own height — the segmented track, which paints 2px larger than
/// its cells on every side.
const TAB_STRIP_H: f32 = SEG_H + 4.0;
/// How close to the window's edge the card is allowed to come.
const SCREEN_MARGIN: f32 = 12.0;

/// Whether a click on a family row is an edit — `None` for a re-pick of the family
/// the subject is already in.
///
/// Picking a family clears every variable-axis setting and OpenType feature, which
/// is destructive for a re-pick, and `apply_char_attrs`' equality guard cannot
/// catch it. `mixed` is an argument because over a disagreeing selection,
/// re-picking the shown family makes the selection agree.
fn family_actually_changed(chosen: Option<String>, current: &str, mixed: bool) -> Option<String> {
    chosen.filter(|f| mixed || f != current)
}

/// The Type card's override marks — `OndinApp::type_marks`.
#[derive(Default)]
struct TypeMarks {
    family: Option<OverrideMark>,
    variant: Option<OverrideMark>,
    size: Option<OverrideMark>,
    line_height: Option<OverrideMark>,
    letter_spacing: Option<OverrideMark>,
    align: Option<OverrideMark>,
    valign: Option<OverrideMark>,
    content: Option<OverrideMark>,
}

/// A `Length` as a mark's tooltip names it: *120%*, *4*.
fn length_words(l: &Length) -> String {
    match *l {
        Length::Em(m) => format!("{}%", mark_num(m * 100.0)),
        Length::Px(v) => mark_num(v),
    }
}

impl OndinApp {
    /// One text style field's override mark on text `id`, compared on the node's
    /// own style (the one `reset::overrides` compares), not a run's.
    fn style_mark<V: PartialEq + Clone>(
        &self,
        id: NodeId,
        get: impl Fn(&TextStyle) -> V,
        put: impl Fn(&mut TextStyle, V),
        say: impl Fn(&V) -> String,
    ) -> Option<OverrideMark> {
        self.sub_mark(
            &[id],
            |op| match op {
                Operation::SetTextStyle { style, .. } => Some(style.clone()),
                _ => None,
            },
            |n| match n.kind() {
                ondin_core::NodeKind::Text { style, .. } => Some((**style).clone()),
                _ => None,
            },
            get,
            put,
            |id, style| Operation::SetTextStyle {
                id,
                style,
                spans: None,
            },
            say,
        )
    }

    /// The family's override mark on text `id`, comparing the binding with the
    /// literal: a bound family's literal is empty on both sides, so a copy bound
    /// apart from its main differs by its binding alone, and the reset writes the
    /// main's binding beside its literal.
    fn family_mark(&self, id: NodeId) -> Option<OverrideMark> {
        let doc = &self.session.doc;
        let resets = self.resets_of(id);
        let node = doc.get(id)?;
        let ondin_core::NodeKind::Text { style, .. } = node.kind() else {
            return None;
        };
        let theirs_family = resets.iter().find_map(|op| match op {
            Operation::SetTextStyle { style, .. } => Some(style.font_family.clone()),
            _ => None,
        });
        let theirs_var = resets.iter().find_map(|op| match op {
            Operation::SetTextVars { vars, .. } => Some(vars[1]),
            _ => None,
        });
        if theirs_family.is_none() && theirs_var.is_none() {
            return None;
        }
        let family = theirs_family.unwrap_or_else(|| style.font_family.clone());
        let var = theirs_var.unwrap_or(node.text_vars()[1]);
        if family == style.font_family && var == node.text_vars()[1] {
            return None;
        }
        let shown = match var {
            Some(v) => doc
                .variables()
                .get(v)
                .map_or_else(|| "a variable".to_owned(), |x| x.name.clone()),
            None => family.clone(),
        };
        let mut next = (**style).clone();
        next.font_family = family;
        let mut vars = node.text_vars();
        vars[1] = var;
        Some(OverrideMark {
            tip: format!("Reset to main · {shown}"),
            tx: Transaction(vec![
                Operation::SetTextStyle {
                    id,
                    style: next,
                    spans: None,
                },
                Operation::SetTextVars { id, vars },
            ]),
        })
    }

    /// A type number's override mark on text `id` — [`Self::family_mark`]'s rule
    /// for `variable::TextVars` slot `slot`: where the copy is bound apart from its
    /// main, a mark resetting the main's binding beside its literal; else
    /// `literal`, the field's own mark on the stored value.
    fn binding_mark(
        &self,
        id: NodeId,
        slot: usize,
        literal: Option<OverrideMark>,
    ) -> Option<OverrideMark> {
        let doc = &self.session.doc;
        let resets = self.resets_of(id);
        let node = doc.get(id)?;
        let own = node.text_vars();
        let theirs = resets.iter().find_map(|op| match op {
            Operation::SetTextVars { vars, .. } => Some(vars[slot]),
            _ => None,
        });
        let Some(var) = theirs.filter(|v| *v != own[slot]) else {
            return literal;
        };
        // The main's literal for this slot alone, over the copy's own values.
        let mut main = node.kind().clone();
        for op in resets.iter() {
            if let ondin_core::NodeKind::Text {
                style, paragraph, ..
            } = &mut main
            {
                match op {
                    Operation::SetTextStyle { style: s, .. } => **style = s.clone(),
                    Operation::SetParagraphStyle { paragraph: p, .. } => *paragraph = p.clone(),
                    _ => {}
                }
            }
        }
        let mut kind = node.kind().clone();
        ondin_core::variable::copy_text_slot(slot, &main, &mut kind);
        let ondin_core::NodeKind::Text {
            style, paragraph, ..
        } = kind
        else {
            return literal;
        };
        let shown = match var {
            Some(v) => doc
                .variables()
                .get(v)
                .map_or_else(|| "a variable".to_owned(), |x| x.name.clone()),
            None => ondin_core::variable::text_slot_number(slot, &style, &paragraph)
                .map_or_else(|| "auto".to_owned(), mark_num),
        };
        let mut vars = own;
        vars[slot] = var;
        let write = match slot {
            ondin_core::variable::TEXT_PARAGRAPH_SPACING => Operation::SetParagraphStyle {
                id,
                paragraph,
                spans: None,
            },
            _ => Operation::SetTextStyle {
                id,
                style: *style,
                spans: None,
            },
        };
        Some(OverrideMark {
            tip: format!("Reset to main · {shown}"),
            tx: Transaction(vec![write, Operation::SetTextVars { id, vars }]),
        })
    }

    /// The *Content* row's override mark on text `id` — [`Self::family_mark`]'s
    /// binding-with-the-literal rule for the other half of `TextVars`. The reset is
    /// the main's `SetText` (its spans with it) and its content binding.
    fn content_mark(&self, id: NodeId) -> Option<OverrideMark> {
        let doc = &self.session.doc;
        let resets = self.resets_of(id);
        let node = doc.get(id)?;
        let ondin_core::NodeKind::Text { content, .. } = node.kind() else {
            return None;
        };
        let theirs_text = resets
            .iter()
            .find(|op| matches!(op, Operation::SetText { .. }))
            .cloned();
        let theirs_var = resets.iter().find_map(|op| match op {
            Operation::SetTextVars { vars, .. } => Some(vars[0]),
            _ => None,
        });
        let own_var = node.text_vars()[0];
        let var = theirs_var.unwrap_or(own_var);
        let text_differs = matches!(
            &theirs_text,
            Some(Operation::SetText { content: c, .. }) if c != content
        );
        if !text_differs && var == own_var {
            return None;
        }
        let shown = match (var, &theirs_text) {
            (Some(v), _) => doc
                .variables()
                .get(v)
                .map_or_else(|| "a variable".to_owned(), |x| x.name.clone()),
            (None, Some(Operation::SetText { content: c, .. })) => c.clone(),
            (None, _) => content.clone(),
        };
        let shown: String = match shown.chars().count() > 32 {
            true => shown.chars().take(31).chain(['…']).collect(),
            false => shown,
        };
        let mut tx = Vec::new();
        if let Some(op) = theirs_text {
            tx.push(op);
        }
        let mut vars = node.text_vars();
        vars[0] = var;
        tx.push(Operation::SetTextVars { id, vars });
        Some(OverrideMark {
            tip: format!("Reset to main · {}", shown.replace(['\r', '\n'], " ")),
            tx: Transaction(tx),
        })
    }

    /// One paragraph style field's override mark on text `id`, the paragraph twin
    /// of [`Self::style_mark`].
    fn para_mark<V: PartialEq + Clone>(
        &self,
        id: NodeId,
        get: impl Fn(&ParagraphStyle) -> V,
        put: impl Fn(&mut ParagraphStyle, V),
        say: impl Fn(&V) -> String,
    ) -> Option<OverrideMark> {
        self.sub_mark(
            &[id],
            |op| match op {
                Operation::SetParagraphStyle { paragraph, .. } => Some(paragraph.clone()),
                _ => None,
            },
            |n| match n.kind() {
                ondin_core::NodeKind::Text { paragraph, .. } => Some(paragraph.clone()),
                _ => None,
            },
            get,
            put,
            |id, paragraph| Operation::SetParagraphStyle {
                id,
                paragraph,
                spans: None,
            },
            say,
        )
    }

    /// One text box field's override mark on text `id`.
    fn block_mark<V: PartialEq + Clone>(
        &self,
        id: NodeId,
        get: impl Fn(&BlockStyle) -> V,
        put: impl Fn(&mut BlockStyle, V),
        say: impl Fn(&V) -> String,
    ) -> Option<OverrideMark> {
        self.sub_mark(
            &[id],
            |op| match op {
                Operation::SetBlockStyle { block, .. } => Some(*block),
                _ => None,
            },
            |n| match n.kind() {
                ondin_core::NodeKind::Text { block, .. } => Some(*block),
                _ => None,
            },
            get,
            put,
            |id, block| Operation::SetBlockStyle { id, block },
            say,
        )
    }

    /// The Sizing section's override mark on text `id`: compared on the mode (auto
    /// width, auto height, fixed) and reset to the main's whole sizing. The widths
    /// and heights are marked on the Transform card (`OndinApp::transform_marks`);
    /// comparing them here would mark one difference twice.
    fn sizing_mark(&self, id: NodeId) -> Option<OverrideMark> {
        // The sizing as the mark compares it: equal when the modes are.
        #[derive(Clone)]
        struct Mode(TextSizing);
        impl PartialEq for Mode {
            fn eq(&self, other: &Self) -> bool {
                self.0.cell() == other.0.cell()
            }
        }
        self.sub_mark(
            &[id],
            |op| match op {
                Operation::SetGeometry {
                    geometry: GeometryPatch::TextSizing(s),
                    ..
                } => Some(*s),
                _ => None,
            },
            |n| match n.kind() {
                ondin_core::NodeKind::Text { sizing, .. } => Some(*sizing),
                _ => None,
            },
            |s| Mode(*s),
            |s, Mode(from)| *s = from,
            |id, sizing| Operation::SetGeometry {
                id,
                geometry: GeometryPatch::TextSizing(sizing),
            },
            |Mode(s)| TextSizing::LABELS[s.cell()].to_owned(),
        )
    }

    /// The Type card's override marks for text `id`: the five style fields, the
    /// paragraph's alignment and the box's vertical alignment.
    ///
    /// On a text-styled layer the five style fields carry no mark: the card's own
    /// mark resets a difference, and a field's reset would write the whole
    /// `SetTextStyle` and make `style::settle` detach it.
    fn type_marks(&self, id: NodeId) -> TypeMarks {
        let styled = self
            .session
            .doc
            .get(id)
            .is_some_and(|n| n.styles().get(StyleSlot::Text).is_some());
        if styled {
            return TypeMarks {
                family: None,
                variant: None,
                size: None,
                line_height: None,
                letter_spacing: None,
                ..self.unstyled_marks(id)
            };
        }
        self.unstyled_marks(id)
    }

    /// [`Self::type_marks`] whatever the layer's text style. A type number bound
    /// apart from its main is marked by its binding ([`Self::binding_mark`]).
    fn unstyled_marks(&self, id: NodeId) -> TypeMarks {
        use ondin_core::variable::{TEXT_LETTER_SPACING, TEXT_LINE_HEIGHT, TEXT_SIZE, TEXT_WEIGHT};
        let bound = |slot, literal| self.binding_mark(id, slot, literal);
        TypeMarks {
            family: self.family_mark(id),
            content: self.content_mark(id),
            // The face: weight, italic and the axes they drive, as one.
            variant: bound(
                TEXT_WEIGHT,
                self.style_mark(
                    id,
                    |s| AxesOf {
                        shown: (
                            s.weight,
                            s.italic,
                            s.variations
                                .iter()
                                .copied()
                                .filter(|a| AXES_DRIVEN_ELSEWHERE.contains(&a.tag))
                                .collect::<Vec<_>>(),
                        ),
                        whole: s.variations.clone(),
                    },
                    |s, v| {
                        (s.weight, s.italic) = (v.shown.0, v.shown.1);
                        s.variations = axes_reset(&s.variations, &v.whole, |t| {
                            AXES_DRIVEN_ELSEWHERE.contains(&t)
                        });
                    },
                    |AxesOf {
                         shown: (w, i, _), ..
                     }| match i {
                        true => format!("{w} italic"),
                        false => w.to_string(),
                    },
                ),
            ),
            size: bound(
                TEXT_SIZE,
                self.style_mark(
                    id,
                    |s| s.font_size,
                    |s, v| s.font_size = v,
                    |v| mark_num(*v),
                ),
            ),
            line_height: bound(
                TEXT_LINE_HEIGHT,
                self.style_mark(
                    id,
                    |s| s.line_height,
                    |s, v| s.line_height = v,
                    |v| v.as_ref().map_or_else(|| "auto".to_owned(), length_words),
                ),
            ),
            letter_spacing: bound(
                TEXT_LETTER_SPACING,
                self.style_mark(
                    id,
                    |s| s.letter_spacing,
                    |s, v| s.letter_spacing = v,
                    length_words,
                ),
            ),
            align: self.sub_mark(
                &[id],
                |op| match op {
                    Operation::SetParagraphStyle { paragraph, .. } => Some(paragraph.clone()),
                    _ => None,
                },
                |n| match n.kind() {
                    ondin_core::NodeKind::Text { paragraph, .. } => Some(paragraph.clone()),
                    _ => None,
                },
                |p| p.align,
                |p, v| p.align = v,
                |id, paragraph| Operation::SetParagraphStyle {
                    id,
                    paragraph,
                    spans: None,
                },
                |v| format!("{v:?}"),
            ),
            valign: self.sub_mark(
                &[id],
                |op| match op {
                    Operation::SetBlockStyle { block, .. } => Some(*block),
                    _ => None,
                },
                |n| match n.kind() {
                    ondin_core::NodeKind::Text { block, .. } => Some(*block),
                    _ => None,
                },
                |b| b.vertical_align,
                |b, v| b.vertical_align = v,
                |id, block| Operation::SetBlockStyle { id, block },
                |v| format!("{v:?}"),
            ),
        }
    }

    /// The Type panel: family, variant and size, the two `Length` fields, and the
    /// alignment row with the popup's button on the end.
    pub(crate) fn inspector_type(&mut self, ui: &mut egui::Ui, subject: TypeSubject) {
        // The button's rect reaches the popup so the dismiss test forgives the click
        // that opens it.
        let mut menu_button: Option<egui::Rect> = None;
        // Over several layers the card has no marks, *Content* row or popup: each is one layer's.
        let several = !subject.others.is_empty();
        let marks = match several {
            false => self.type_marks(subject.id),
            true => TypeMarks::default(),
        };
        // Over a session's selection a write is spans, which keep the style.
        let targets = subject.ids();
        let live = subject.partial;
        self.style_card_live(
            ui,
            "Type",
            StyleSlot::Text,
            &targets,
            None,
            live,
            |app, ui| {
                app.type_family_row(ui, &subject, marks.family.as_ref());
                let half = egui::vec2((ui.available_width() - GAP) / 2.0, ROW_H);
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    app.type_variant_field(ui, &subject, half, marks.variant.as_ref());
                    let (_, hit) = app.type_size_field(ui, &subject, half, marks.size.as_ref());
                    app.reset_marked(hit, &marks.size);
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    app.type_line_height_field(ui, &subject, half, marks.line_height.as_ref());
                    app.type_letter_spacing_field(
                        ui,
                        &subject,
                        half,
                        marks.letter_spacing.as_ref(),
                    );
                });
            },
            // No text style holds the content, paragraph or box, so they stay live.
            |app, ui| {
                if !several {
                    app.type_content_row(ui, &subject, &marks.content);
                }
                menu_button = app.type_alignment_row(ui, &subject, [&marks.align, &marks.valign]);
            },
        );
        self.type_menu_popup(ui, &subject, menu_button);
    }

    /// The *Content* row: the text itself, one line, and the door to binding it to
    /// a string variable.
    ///
    /// Unbound, `--` opens the variable list, `Enter` or a click away writes, `Escape`
    /// abandons. The write is the one edit the typing made (common head and tail
    /// kept), so spans over the rest are re-based. Read-only over several lines and
    /// while the canvas edits this layer. `mark` is [`Self::content_mark`]'s.
    fn type_content_row(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        mark: &Option<OverrideMark>,
    ) {
        let id = subject.id;
        let key = super::binding::FieldKey::Content { id };
        let w = ui.available_width();
        let live = self.text.as_ref().is_some_and(|s| s.id == id);
        if !live && self.bound_field_ui(ui, key, w, mark) {
            return;
        }
        let Some(ondin_core::NodeKind::Text {
            content,
            style,
            spans,
            para_spans,
            paragraph,
            ..
        }) = self.session.doc.get(id).map(|n| n.kind().clone())
        else {
            return;
        };
        let shown = match self.session.doc.resolved_kind(id).map(|k| k.into_owned()) {
            Some(ondin_core::NodeKind::Text { content, .. }) => content,
            _ => content.clone(),
        };
        let read_only = live || self.field_var(key).is_some() || shown.contains('\n');
        let edit_id = egui::Id::new(("type-content", id));
        let focused = ui.memory(|m| m.has_focus(edit_id));
        let mut text = match focused {
            true => ui
                .data(|d| d.get_temp::<String>(edit_id))
                .unwrap_or_else(|| shown.clone()),
            false => shown.replace(['\r', '\n'], " "),
        };
        let mut field = None;
        let mut reset_hit = false;
        let row = field_row(ui, egui::vec2(w, ROW_H), |ui| {
            let over = ui.rect_contains_pointer(ui.max_rect());
            match mark {
                None => {
                    ui.label(theme::icon_text(key.badge(), 14.0, theme::text::FAINT));
                }
                Some(m) => {
                    let glyph = if over {
                        icon::ARROW_COUNTER_CLOCKWISE
                    } else {
                        key.badge()
                    };
                    let badge = ui.add(
                        egui::Label::new(theme::icon_text(glyph, 14.0, theme::text::STRONG))
                            .sense(egui::Sense::click()),
                    );
                    if !over {
                        crate::ui::override_dot(
                            ui.painter(),
                            badge.rect.right_top() + egui::vec2(1.5, 3.0),
                        );
                    }
                    reset_hit = badge.on_hover_text(&m.tip).clicked();
                }
            }
            let out = ui.add_enabled(
                !read_only,
                egui::TextEdit::singleline(&mut text)
                    .id(edit_id)
                    .frame(egui::Frame::NONE)
                    .desired_width(f32::INFINITY)
                    .font(egui::FontId::proportional(12.0)),
            );
            field = Some(out);
            ui.max_rect()
        });
        let Some(field) = field else { return };
        // The ↺ is the user's last act and wins over the field's own typing.
        if reset_hit {
            ui.data_mut(|d| d.remove::<String>(edit_id));
            self.reset_marked(true, mark);
            return;
        }
        // A disabled field reports no hover, so the reason hangs off its own sense.
        let hover = ui.interact(row, edit_id.with("hover"), egui::Sense::click());
        if read_only {
            let why = if live {
                "Being edited on the canvas"
            } else {
                "Text of several lines — edit it on the canvas"
            };
            let _ = hover.clone().on_hover_text(why);
        }
        if (hover.secondary_clicked() || field.secondary_clicked())
            && let Some(at) = ui.input(|i| i.pointer.interact_pos())
        {
            self.field_menu = Some((key, at, row));
        }
        if field.has_focus() {
            ui.data_mut(|d| d.insert_temp(edit_id, text.clone()));
            // `--` hands the text to the retype buffer, which binds.
            self.bind_typing(ui.ctx(), key, &text, field.rect);
            if text.starts_with("--") {
                self.var_retype = Some(super::binding::Retype {
                    key,
                    text,
                    focus: true,
                });
            }
            return;
        }
        if ui::defocus_commits(&field) && text != shown && !text.starts_with("--") {
            let (old, new) = (content.as_bytes(), text.as_bytes());
            let mut head = old.iter().zip(new).take_while(|(a, b)| a == b).count();
            while !content.is_char_boundary(head) || !text.is_char_boundary(head) {
                head -= 1;
            }
            let room = old.len().min(new.len()) - head;
            let mut tail = old
                .iter()
                .rev()
                .zip(new.iter().rev())
                .take(room)
                .take_while(|(a, b)| a == b)
                .count();
            while !content.is_char_boundary(old.len() - tail)
                || !text.is_char_boundary(new.len() - tail)
            {
                tail -= 1;
            }
            let removed = old.len() - head - tail;
            let inserted = new.len() - head - tail;
            self.commit_edit(Transaction(vec![Operation::SetText {
                id,
                content: text,
                spans: spans.edited(head, removed, inserted, &style),
                para_spans: para_spans.edited(head, removed, inserted, &paragraph),
            }]));
        }
        if field.lost_focus() {
            ui.data_mut(|d| d.remove::<String>(edit_id));
        }
    }

    // --- main panel -------------------------------------------------------

    /// The family dropdown — virtualized, with every row set in its own face (§9.2).
    ///
    /// Only the rows in view are built (the catalog is ~1900 families), which is why
    /// [`FAMILY_ROW_H`] is a constant. Drawing a row asks for that family's regular
    /// face, so scrolling prefetches what picking needs.
    fn type_family_row(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        mark: Option<&OverrideMark>,
    ) {
        // Bound, the row is the chip, unless a session's selection is what it edits.
        let key = super::binding::FieldKey::Family { id: subject.id };
        // Over several layers the row is the picker whatever is bound.
        let several = !subject.others.is_empty();
        if !subject.partial
            && !several
            && let Some(var) = self.field_var(key)
        {
            let w = ui.available_width();
            if self.number_chip(ui, key, var, w, mark)
                && let Some(m) = mark
            {
                self.commit_reset(m.tx.clone());
            }
            return;
        }
        let shown = attr!(subject, Family, Family);
        let mixed = subject.mixed(CharAttrKind::Family);
        let current = shown.clone();
        // A family still downloading gets an ellipsis; one that can never be had
        // wears its glyph in `WARN`.
        let status = (!mixed).then(|| self.fonts.family_status(&shown));
        let label = if mixed {
            "Mixed".to_string()
        } else if self.fonts.is_loading(&shown) {
            format!("{shown}…")
        } else {
            shown
        };
        // Only the glyph is tinted: a dim name would read as the pick not taking.
        let head = match status {
            Some(crate::fonts::FamilyStatus::Missing) => {
                ui::glyph_and_text_tinted(icon::TEXT_AA, &label, theme::color::WARN)
            }
            _ => ui::glyph_and_text(icon::TEXT_AA, &label),
        };
        let mut chosen: Option<String> = None;
        let mut reset = false;
        let mut menu_at: Option<(egui::Pos2, egui::Rect)> = None;
        let width = ui.available_width();
        // A combo paints exactly `interact_size.y`; the `scope` keeps that off the fields below.
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = ROW_H;
            ui.spacing_mut().button_padding.y = 0.0;
            let combo = egui::ComboBox::from_id_salt("font-family")
                .icon(ui::combo_chevron)
                .width(width)
                // The popup holds a search field, so a click inside must not dismiss it.
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .selected_text(head)
                .show_ui(ui, |ui| {
                    ui::menu_rows(ui);
                    ui.spacing_mut().item_spacing.y = 3.0;
                    if let Some(m) = mark
                        && super::component::menu_reset_row(ui, m)
                    {
                        reset = true;
                        ui.close();
                    }
                    chosen = self.family_list(ui, &current);
                })
                .response;
            if mark.is_some() {
                super::component::combo_dot(ui.painter(), combo.rect);
            }
            // The door to a binding is the field menu (a picker takes no `--`).
            if combo.secondary_clicked()
                && !subject.partial
                && !several
                && let Some(at) = combo.interact_pointer_pos()
            {
                menu_at = Some((at, combo.rect));
            }
            // The tooltip must hang off the combo's response; the `ui.scope`'s never fires.
            if status == Some(crate::fonts::FamilyStatus::Missing) {
                combo.on_hover_text(
                    "This font is not installed and cannot be downloaded — \
                     the text is drawn in the fallback face",
                );
            }
        });
        if let Some((at, rect)) = menu_at
            && self
                .session
                .doc
                .get(subject.id)
                .is_some_and(|n| n.styles().text.is_none())
        {
            self.field_menu = Some((key, at, rect));
        }
        if reset && let Some(m) = mark {
            self.commit_reset(m.tx.clone());
            return;
        }
        if let Some(family) = family_actually_changed(chosen, &current, mixed) {
            self.fonts.ensure_loaded(&family);
            // Axis and feature tags mean something different in the next typeface.
            self.apply_char_attrs(
                subject,
                vec![
                    CharAttr::Family(family),
                    CharAttr::Variations(Vec::new()),
                    CharAttr::Features(Vec::new()),
                ],
            );
        }
    }

    /// The family dropdown's body: a search field over every family, each row in
    /// its own face. Returns the row clicked, having closed the popup.
    pub(crate) fn family_list(&mut self, ui: &mut egui::Ui, current: &str) -> Option<String> {
        let mut chosen = None;
        ui.add(
            egui::TextEdit::singleline(&mut self.font_filter)
                .margin(ui::SEARCH_PAD)
                .hint_text("Search…"),
        );
        let total = self.fonts.match_count(&self.font_filter);
        egui::ScrollArea::vertical().max_height(320.0).show_rows(
            ui,
            FAMILY_ROW_H,
            total,
            |ui, rows| {
                let names = self.fonts.matched_names(&self.font_filter, rows);
                for name in names {
                    if self.family_row(ui, &name, current) {
                        chosen = Some(name);
                        ui.close();
                    }
                }
            },
        );
        chosen
    }

    /// One row of the family list: the name, set in the family it names, falling
    /// back to the UI font when there is no truthful preview (bytes not arrived, or
    /// a symbol font). Returns whether it was clicked; [`family_actually_changed`]
    /// decides whether that is an edit.
    fn family_row(&mut self, ui: &mut egui::Ui, family: &str, current: &str) -> bool {
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), FAMILY_ROW_H),
            egui::Sense::click(),
        );
        let selected = family == current;
        let p = ui.painter();
        let r5 = egui::CornerRadius::same(5);
        if selected {
            p.rect_filled(rect, r5, theme::color::SELECT_ROW);
        } else if resp.hovered() {
            p.rect_filled(rect, r5, theme::color::text_a(12));
        }

        let ink = if selected {
            theme::color::TEXT
        } else {
            theme::text::STRONG
        };
        let text = rect
            .shrink2(egui::vec2(7.0, 0.0))
            .with_max_x(rect.right() - LOADING_DOT_SLOT);
        // Also the LRU touch that keeps an on-screen family from being evicted.
        self.fonts.touch(family);
        if !self.previews.paint(ui, text, family, ink) {
            self.fonts.ensure_preview(family);
            ui.painter().text(
                egui::pos2(text.left(), rect.center().y),
                egui::Align2::LEFT_CENTER,
                family,
                egui::FontId::proportional(12.0),
                theme::text::MUTED,
            );
        }
        // Asked after `ensure_preview`, which is what puts the face in flight.
        if self.fonts.is_loading(family) {
            paint_loading_dot(
                ui,
                egui::pos2(rect.right() - LOADING_DOT_SLOT * 0.5, rect.center().y),
            );
        }
        resp.clicked()
    }

    /// The variant dropdown: the family's named instances, or its faces, plus an
    /// inert `Custom` row when the settings match no variant — classified from the
    /// values, so a hand-tuned axis cannot leave the field claiming "Bold".
    fn type_variant_field(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        size: egui::Vec2,
        mark: Option<&OverrideMark>,
    ) {
        // Bound, the field is the weight's chip; picking a face would detach it.
        let key = self.type_key(subject, ondin_core::variable::TEXT_WEIGHT);
        if self.type_bound(key) {
            self.type_bound_ui(ui, key, size.x, mark);
            return;
        }
        let family = attr!(subject, Family, Family);
        let variants = text::family_variants(&family);
        let weight = attr!(subject, Weight, Weight);
        let italic = attr!(subject, Italic, Italic);
        let coords = attr!(subject, Variations, Variations);
        let current = variants
            .iter()
            .position(|v| matches_variant(v, weight, italic, &coords));
        // Over several layers a layer on another face makes the field *Mixed*.
        let faces_differ = faces_disagree(&variants, subject);
        let current = current.filter(|_| !faces_differ);
        let label = if faces_differ
            || subject.mixed(CharAttrKind::Weight)
            || subject.mixed(CharAttrKind::Italic)
        {
            "Mixed".to_string()
        } else {
            match current {
                Some(i) => variants[i].name.clone(),
                None => "Custom".to_string(),
            }
        };
        // A family with one face, or families that disagree across layers, fall
        // back to the bold/italic pair.
        if variants.len() < 2 || subject.mixed(CharAttrKind::Family) {
            self.type_weight_toggles(ui, subject, size);
            return;
        }
        let mut pick: Option<usize> = None;
        let mut reset = false;
        let mut menu_at = None;
        ui.scope(|ui| {
            // The combo paints its border inside its rect, so its height is `size.y` whole.
            ui.spacing_mut().interact_size.y = size.y;
            ui.spacing_mut().button_padding.y = 0.0;
            let face = egui::ComboBox::from_id_salt("font-variant")
                .icon(ui::combo_chevron)
                .width(size.x)
                .selected_text(ui::glyph_and_text(icon::TEXT_BOLD, &label))
                .show_ui(ui, |ui| {
                    ui::menu_rows(ui);
                    ui.spacing_mut().button_padding.y = 2.0;
                    if let Some(m) = mark {
                        reset = super::component::menu_reset_row(ui, m);
                    }
                    for (i, v) in variants.iter().enumerate() {
                        if ui.selectable_label(current == Some(i), &v.name).clicked() {
                            pick = Some(i);
                        }
                    }
                    // Inert: a readout of "none of the above", not a choice.
                    if current.is_none() && !faces_differ {
                        ui.add_enabled(false, egui::Button::new("Custom"));
                    }
                })
                .response;
            if mark.is_some() {
                super::component::combo_dot(ui.painter(), face.rect);
            }
            // The door to a binding is the field menu (a picker takes no `--`).
            if let Some(key) = key
                && face.secondary_clicked()
                && let Some(at) = face.interact_pointer_pos()
            {
                menu_at = Some((key, at, face.rect));
            }
        });
        if let Some(menu) = menu_at {
            self.field_menu = Some(menu);
        }
        if reset && let Some(m) = mark {
            self.commit_reset(m.tx.clone());
            return;
        }
        if let Some(i) = pick {
            self.apply_variant(subject, &variants[i]);
        }
    }

    /// Variant `v` picked over `subject`: its weight, slope and axes, each
    /// layer's own other axes kept.
    fn apply_variant(&mut self, subject: &TypeSubject, v: &text::FontVariant) {
        // So the first layer's hand-set axes do not land on the rest.
        self.apply_char_attrs_each(subject, |m| {
            variant_attrs(v, &attr!(m, Variations, Variations))
        });
    }

    /// *Bold* (`bold`) or *Italic* over `subject`, as `Ctrl+B`/`Ctrl+I` and the
    /// panel's two toggles do it: the family's own face in that direction
    /// (`text::bold_step`, `text::italic_step`), or a status line saying it has none.
    ///
    /// Each layer steps from its own weight and slope, the on/off decision shared.
    /// Over a range whose other attribute is ragged only the toggled one is
    /// written, so Italic never flattens the weights and Bold never the slopes.
    pub(crate) fn toggle_face(&mut self, subject: &TypeSubject, bold: bool) {
        let family = attr!(subject, Family, Family);
        let weight = attr!(subject, Weight, Weight);
        let italic = attr!(subject, Italic, Italic);
        // Over several families one family's faces would land on the others' layers,
        // so mixed is the bare weight or flag, as for a family core does not know.
        let offer = match subject.mixed(CharAttrKind::Family) {
            true => text::FaceOffer::default(),
            false => text::FaceOffer::of(&family),
        };
        let mixed_weight = subject.mixed(CharAttrKind::Weight);
        let mixed_italic = subject.mixed(CharAttrKind::Italic);
        let step = |weight, italic| match bold {
            true => text::bold_step(&offer, weight, italic, mixed_weight),
            false => text::italic_step(&offer, weight, italic, mixed_italic),
        };
        match step(weight, italic) {
            Some(_) => self.apply_char_attrs_each(subject, |m| {
                let Some(v) = step(attr!(m, Weight, Weight), attr!(m, Italic, Italic)) else {
                    return Vec::new();
                };
                match bold {
                    true if m.ragged(CharAttrKind::Italic) => vec![CharAttr::Weight(v.weight)],
                    false if m.ragged(CharAttrKind::Weight) => vec![CharAttr::Italic(v.italic)],
                    _ => variant_attrs(&v, &attr!(m, Variations, Variations)),
                }
            }),
            None => self.session.info(match bold {
                true if weight >= text::BOLD_FROM && !mixed_weight => {
                    format!("{family} has nothing lighter")
                }
                true => format!("{family} has no bold"),
                false => format!("{family} has no italic"),
            }),
        }
    }

    /// The bold/italic pair, for a family that offers no variant list.
    ///
    /// Each button carries an instance's override mark on its own value (weight on
    /// Bold, slant on Italic), and a click on a marked one is its reset
    /// (`ui::field_button_marked`).
    fn type_weight_toggles(&mut self, ui: &mut egui::Ui, subject: &TypeSubject, size: egui::Vec2) {
        let weight = attr!(subject, Weight, Weight);
        let italic = attr!(subject, Italic, Italic);
        let cell = (size.x - 2.0) / 2.0;
        let bold_mark = self.style_mark(
            subject.id,
            |s| s.weight,
            |s, v| s.weight = v,
            |w| w.to_string(),
        );
        let italic_mark = self.style_mark(
            subject.id,
            |s| s.italic,
            |s, v| s.italic = v,
            |i| match i {
                true => "italic".to_owned(),
                false => "upright".to_owned(),
            },
        );
        // One layer's override marks cannot speak for several.
        let (bold_mark, italic_mark) = match subject.others.is_empty() {
            true => (bold_mark, italic_mark),
            false => (None, None),
        };
        let side = egui::Vec2::splat(cell.min(size.y));
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let bold_on = bold_lit(weight, subject.mixed(CharAttrKind::Weight));
            let (resp, reset) = ui::field_button_marked(
                ui,
                icon::TEXT_BOLD,
                side,
                14.0,
                ui::FieldButton::on_if(bold_on),
                bold_mark.as_ref().map(OverrideMark::field),
            );
            if reset {
                self.reset_marked(true, &bold_mark);
            } else if tip_unless_marked(resp, &bold_mark, "Bold").clicked() {
                self.toggle_face(subject, true);
            }
            let italic_on = italic && !subject.mixed(CharAttrKind::Italic);
            let (resp, reset) = ui::field_button_marked(
                ui,
                icon::TEXT_ITALIC,
                side,
                14.0,
                ui::FieldButton::on_if(italic_on),
                italic_mark.as_ref().map(OverrideMark::field),
            );
            if reset {
                self.reset_marked(true, &italic_mark);
            } else if tip_unless_marked(resp, &italic_mark, "Italic").clicked() {
                self.toggle_face(subject, false);
            }
        });
    }

    /// The binding key of `subject`'s type number `slot` (`variable::TextVars`),
    /// where one binds: one layer's default, not a session's selection or
    /// paragraphs, and a character number not under a text style, which holds
    /// plain values.
    fn type_key(&self, subject: &TypeSubject, slot: usize) -> Option<super::binding::FieldKey> {
        let styled = self
            .session
            .doc
            .get(subject.id)
            .is_some_and(|n| n.styles().text.is_some());
        let refused = styled && ondin_core::variable::TEXT_STYLED.contains(&slot);
        // The paragraph spacing's scope is the session's paragraphs, a caret's too.
        let partial = match slot {
            ondin_core::variable::TEXT_PARAGRAPH_SPACING => subject.para_partial,
            _ => subject.partial,
        };
        (!partial && subject.others.is_empty() && !refused).then_some(
            super::binding::FieldKey::Type {
                id: subject.id,
                slot,
            },
        )
    }

    /// Whether `key`'s field draws its bound face: bound, or its chip typed over
    /// (a typed field only — the weight is picked).
    fn type_bound(&self, key: Option<super::binding::FieldKey>) -> bool {
        key.is_some_and(|k| self.typed_over(k) || self.field_var(k).is_some())
    }

    fn typed_over(&self, key: super::binding::FieldKey) -> bool {
        key.typed() && self.var_retype.as_ref().is_some_and(|r| r.key == key)
    }

    /// A type number's bound face in a field `w` wide — the buffer a key on its
    /// chip began, else the chip — `true` when it drew one and the plain field is
    /// not drawn. `mark` is the field's override mark, whose ↺ resets.
    fn type_bound_ui(
        &mut self,
        ui: &mut egui::Ui,
        key: Option<super::binding::FieldKey>,
        w: f32,
        mark: Option<&OverrideMark>,
    ) -> bool {
        let Some(key) = key else { return false };
        if self.typed_over(key)
            && let Some(rt) = self.var_retype.clone()
        {
            self.number_retype(ui, key, rt, w);
            return true;
        }
        let Some(var) = self.field_var(key) else {
            return false;
        };
        if self.number_chip(ui, key, var, w, mark)
            && let Some(m) = mark
        {
            self.commit_reset(m.tx.clone());
        }
        true
    }

    /// Size: `T` prefix, px only, no unit toggle (a size in ems would be relative
    /// to itself).
    ///
    /// Returns the field's own `Response` so a test can find its rect; the second
    /// value is whether the instance override mark's ↺ was clicked.
    fn type_size_field(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        size: egui::Vec2,
        mark: Option<&OverrideMark>,
    ) -> (egui::Response, bool) {
        let key = self.type_key(subject, ondin_core::variable::TEXT_SIZE);
        if self.type_bound(key) {
            let r = ui.scope(|ui| self.type_bound_ui(ui, key, size.x, mark));
            return (r.response, false);
        }
        let mut value = subject.font_size();
        let (resp, hit) = ui::value_field_marked(
            ui,
            size,
            Prefix::Icon(icon::TEXT_T),
            mark.map(OverrideMark::field),
            &mut value,
            Scrub::whole(0.5).range(MIN_FONT_SIZE..=MAX_FONT_SIZE),
            |d| {
                let d = d.custom_formatter(ui::number(2));
                mixed_text(d, subject.mixed(CharAttrKind::Size))
            },
        );
        self.char_valve(&resp, subject, CharAttr::Size(value));
        if let Some(key) = key {
            self.field_doors(ui, key, &resp);
        }
        (resp, hit)
    }

    /// Line height: `Auto`, a percentage of the font size, or absolute px.
    ///
    /// Three states on one field, cycled by the unit suffix. Auto is the font's own
    /// line height (CSS `normal`).
    fn type_line_height_field(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        size: egui::Vec2,
        mark: Option<&OverrideMark>,
    ) {
        let key = self.type_key(subject, ondin_core::variable::TEXT_LINE_HEIGHT);
        if self.type_bound(key) {
            self.type_bound_ui(ui, key, size.x, mark);
            return;
        }
        let current = match subject.shown(CharAttrKind::LineHeight) {
            CharAttr::LineHeight(v) => v,
            _ => None,
        };
        let mixed = subject.line_height_mixed();
        let font_size = subject.font_size();
        let (unit_label, tooltip) = match current {
            // A number typed over layers that disagree is px for them all.
            _ if mixed => (
                "px",
                "Mixed — a number typed sets every layer in px; click to move each to the first layer's next unit",
            ),
            None => ("auto", "The font's own line height — click for %"),
            Some(Length::Em(_)) => ("%", "Percent of the font size — click for px"),
            Some(Length::Px(_)) => ("px", "Absolute — click for auto"),
        };
        let suffix = Some(Suffix {
            text: unit_label,
            clickable: true,
            tooltip,
        });
        // In Auto the digits show what Auto is worth in px; only a node with nothing
        // laid out falls back to the word "Auto".
        let resolved = subject.line_height;
        let (mut shown, scrub, typed) = match current {
            _ if mixed => (
                resolved.unwrap_or(0.0),
                Scrub::whole(0.5).range(Bounds::LINE_HEIGHT.px(font_size)),
                expr::Unit::Px,
            ),
            // Auto cannot be typed into: a zero-width scrub range, and the write
            // below is gated on `current` being `Some`.
            None => (
                resolved.unwrap_or(0.0),
                Scrub::whole(1.0).range(0.0..=0.0),
                expr::Unit::Px,
            ),
            Some(Length::Em(m)) => (
                m * 100.0,
                Scrub::whole(0.5).range(Bounds::LINE_HEIGHT.pct),
                expr::Unit::Pct,
            ),
            Some(Length::Px(v)) => (
                v,
                Scrub::whole(0.5).range(Bounds::LINE_HEIGHT.px(font_size)),
                expr::Unit::Px,
            ),
        };
        let (resp, unit, hit) = ui::value_field_unit_marked(
            ui,
            size,
            Prefix::Icon(icon::ARROWS_VERTICAL),
            suffix,
            mark.map(OverrideMark::field),
            &mut shown,
            scrub,
            // Its own parser: px and % share one field, so the unit not shown is refused.
            |d| {
                let d = d
                    .custom_formatter(ui::number(2))
                    .custom_parser(move |t| expr::eval_in(t, typed));
                if mixed {
                    return d.custom_formatter(|_, _| "–".into());
                }
                // Nothing laid out to report, so the state is all there is to say.
                if current.is_none() && resolved.is_none() {
                    return d.custom_formatter(|_, _| "Auto".into());
                }
                d
            },
        );
        if let Some(key) = key {
            self.field_doors(ui, key, &resp);
        }
        // The valve below still runs: it owns the latch.
        if hit && let Some(m) = mark {
            self.commit_reset(m.tx.clone());
        }
        let unit_clicked = unit.is_some_and(|u| u.clicked());
        if unit_clicked {
            // The first layer's next state, and every layer into it from its own value.
            let like = next_line_height(current, font_size, resolved);
            self.apply_char_attrs_each(subject, |m| {
                let own = match m.shown(CharAttrKind::LineHeight) {
                    CharAttr::LineHeight(v) => v,
                    _ => None,
                };
                vec![CharAttr::LineHeight(line_height_as(
                    own,
                    m.font_size(),
                    m.line_height,
                    like,
                ))]
            });
            return;
        }
        let next = match current {
            _ if mixed => Some(Length::Px(shown)),
            Some(Length::Em(_)) => Some(Length::Em(shown / 100.0)),
            Some(Length::Px(_)) => Some(Length::Px(shown)),
            None => None,
        };
        let Some(next) = next else { return };
        // A Mixed field writes only once its number has moved, or a click in and out
        // would put the first layer's value on every other.
        if mixed {
            let key = resp.id.with("line-height-moved");
            if resp.changed() {
                ui.ctx().data_mut(|d| d.insert_temp(key, true));
            }
            if !ui.ctx().data(|d| d.get_temp::<bool>(key)).unwrap_or(false) {
                return;
            }
            if !(resp.dragged() || resp.has_focus()) {
                ui.ctx().data_mut(|d| d.insert_temp(key, false));
            }
        }
        self.char_valve(&resp, subject, CharAttr::LineHeight(Some(next)));
    }

    /// Letter spacing: `%` by default, with a `px` alternative.
    ///
    /// `%` is the default because tracking has to scale with size: one style used
    /// at 12pt and 48pt wants the same proportional tracking.
    fn type_letter_spacing_field(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        size: egui::Vec2,
        mark: Option<&OverrideMark>,
    ) {
        self.length_char_field_marked(
            ui,
            subject,
            size,
            CharAttrKind::LetterSpacing,
            Prefix::Icon(icon::TEXT_AA),
            Bounds::TRACKING,
            "Letter spacing",
            mark,
        );
    }

    /// Horizontal and vertical alignment, and the button that opens the popup.
    ///
    /// Both alignments live here and nowhere else; they are daily controls.
    ///
    /// Returns the popup button's rect, which [`Self::type_menu_popup`] needs.
    fn type_alignment_row(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        marks: [&Option<OverrideMark>; 2],
    ) -> Option<egui::Rect> {
        let [align_mark, valign_mark] = marks;
        // The row sits on the column grid of the fields above it, not on its own module.
        let full = ui.available_width();
        let horizontal_w = (full - GAP) / 2.0;
        // The remainder puts the popup button's right edge on the margin.
        let vertical_w = full - horizontal_w - ROW_H - GAP * 2.0;
        let gap = GAP;
        let mut button = None;
        // Over several layers a track lights only the cell they all agree on, and
        // with no popup the tracks take the button's room.
        let members: Vec<&TypeSubject> = std::iter::once(subject).chain(&subject.others).collect();
        let several = members.len() > 1;
        let vertical_w = if several {
            full - horizontal_w - GAP
        } else {
            vertical_w
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let align = subject.paragraph.align;
            let align_cell = match members.iter().all(|m| m.paragraph.align == align) {
                true => align.cell(),
                false => TextAlign::OFFERED.len(),
            };
            let (pick, reset) = ui::segmented_marked(
                ui,
                horizontal_w,
                SEG_CELL_H,
                4,
                align_cell,
                |_| true,
                |_| "",
                align_mark.as_ref().map(OverrideMark::field),
                |p, i, rect, on| {
                    segment_glyph(p, rect, ALIGN_GLYPHS[i], on);
                },
            );
            self.reset_marked(reset, align_mark);
            if several && let Some(i) = pick {
                let to = TextAlign::OFFERED[i];
                let ops: Vec<Operation> = members
                    .iter()
                    .filter(|m| m.paragraph.align != to)
                    .map(|m| Operation::SetParagraphStyle {
                        id: m.id,
                        paragraph: ParagraphStyle {
                            align: to,
                            ..m.paragraph.clone()
                        },
                        spans: None,
                    })
                    .collect();
                if !ops.is_empty() {
                    self.commit_edit(Transaction(ops));
                }
            } else if let Some(i) = pick
                && TextAlign::OFFERED[i] != align
            {
                let mut next = subject.paragraph.clone();
                next.align = TextAlign::OFFERED[i];
                self.commit_paragraph(subject.id, next);
            }
            let valign = subject.block.vertical_align;
            let current = match members.iter().all(|m| m.block.vertical_align == valign) {
                true => VerticalAlign::ALL
                    .iter()
                    .position(|v| *v == valign)
                    .unwrap_or(0),
                false => VerticalAlign::ALL.len(),
            };
            let enabled = members.iter().all(|m| m.sizing.height_is_authored());
            let resp = ui.scope(|ui| {
                // An auto-height box is as tall as its text: no free space to align in.
                if !enabled {
                    ui.disable();
                }
                let (pick, reset) = ui::segmented_marked(
                    ui,
                    vertical_w,
                    SEG_CELL_H,
                    3,
                    current,
                    |_| true,
                    |_| "",
                    valign_mark.as_ref().map(OverrideMark::field),
                    |p, i, rect, on| {
                        segment_glyph(p, rect, VALIGN_GLYPHS[i], on);
                    },
                );
                self.reset_marked(reset, valign_mark);
                if several && let Some(i) = pick {
                    let to = VerticalAlign::ALL[i];
                    let ops: Vec<Operation> = members
                        .iter()
                        .filter(|m| m.block.vertical_align != to)
                        .map(|m| Operation::SetBlockStyle {
                            id: m.id,
                            block: BlockStyle {
                                vertical_align: to,
                                ..m.block
                            },
                        })
                        .collect();
                    if !ops.is_empty() {
                        self.commit_edit(Transaction(ops));
                    }
                } else if let Some(i) = pick
                    && VerticalAlign::ALL[i] != valign
                {
                    let next = BlockStyle {
                        vertical_align: VerticalAlign::ALL[i],
                        ..subject.block
                    };
                    self.commit_block(subject.id, next);
                }
            });
            // A `Ui::scope`'s response never reports a hover, hence a claimant of its own.
            if !enabled {
                ui.interact(
                    resp.response.rect,
                    ui.id().with("dimmed-valign-row"),
                    egui::Sense::empty(),
                )
                .on_hover_text("Only a fixed-height box has room to align in");
            }
            if several {
                return;
            }
            let open = self.type_menu.is_some();
            let head = field_button(
                ui,
                icon::SLIDERS_HORIZONTAL,
                ROW_H,
                14.0,
                ui::FieldButton::on_if(open),
            )
            .on_hover_text("Typography options");
            if head.clicked() {
                self.type_menu = if open { None } else { Some(TypeTab::default()) };
            }
            button = Some(head.rect);
        });
        button
    }

    // --- the popup --------------------------------------------------------

    /// The three-tab overflow panel, an `Area` rather than inline rows so it does
    /// not push every panel below it down the page.
    ///
    /// `head` is the rect of the button that opens it, which the dismiss test needs.
    fn type_menu_popup(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        head: Option<egui::Rect>,
    ) {
        let Some(tab) = self.type_menu else { return };
        // The popup goes with its button: a collapsed Type panel draws none.
        let Some(head) = head else {
            self.type_menu = None;
            return;
        };
        let ctx = ui.ctx().clone();
        let mut close = false;
        // On a styled layer, outside a selection and the style's own *Edit*, the
        // Character tab is read-only: its writes would detach the style.
        let char_locked = !subject.partial
            && self
                .session
                .doc
                .get(subject.id)
                .and_then(|n| n.styles().text)
                .is_some_and(|s| self.session.style_edit != Some(s));
        let anchor = OndinApp::popover_anchor(&ctx, head, MENU_W);
        // The combo lists inside this popup are their own `Area`s, so a click in one
        // misses `menu.contains_pointer()`.
        let over_overlay = ctx
            .pointer_interact_pos()
            .and_then(|p| ctx.layer_id_at(p))
            .is_some_and(|l| l.order >= egui::Order::Foreground);
        let screen = ctx.input(|i| i.content_rect());
        // The window less the chrome around the body; the border counts because a
        // `Frame`'s stroke grows its outer rect on both axes.
        let body_max_h = (screen.height()
            - SCREEN_MARGIN * 2.0
            - crate::ui::MENU_BORDER * 2.0
            - MENU_PAD * 2.0
            - TAB_STRIP_H
            - SECTION_GAP)
            .max(0.0);
        let menu = egui::Area::new(egui::Id::new("type-menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            .constrain_to(screen.shrink(SCREEN_MARGIN))
            .show(&ctx, |ui| {
                ui::menu_frame(MENU_PAD).show(ui, |ui| {
                    ui.set_width(MENU_INNER);
                    ui.spacing_mut().item_spacing.y = SECTION_GAP;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = COL_GAP;
                        let track = MENU_INNER - CLOSE_BUTTON_W - COL_GAP;
                        let current = TypeTab::ALL.iter().position(|t| *t == tab).unwrap_or(0);
                        if let Some(i) =
                            segmented(ui, track, SEG_H, 3, current, |p, i, rect, on| {
                                segment_label(p, rect, TypeTab::ALL[i].label(), on)
                            })
                        {
                            self.type_menu = Some(TypeTab::ALL[i]);
                        }
                        if icon_button(ui, icon::X, CLOSE_BUTTON_W, 13.0, false, true)
                            .on_hover_text("Close")
                            .clicked()
                        {
                            close = true;
                        }
                    });
                    // An explicit rect: a `ScrollArea` inside an `Area` sizes from the
                    // area's last-frame size, so the height would ratchet down per tab switch.
                    let body = egui::Rect::from_min_size(
                        ui.cursor().min,
                        egui::vec2(MENU_INNER, body_max_h),
                    );
                    ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| {
                        egui::ScrollArea::vertical()
                            .id_salt("type-menu-body")
                            .auto_shrink([true, true])
                            .show(ui, |ui| {
                                ui.set_width(MENU_INNER);
                                ui.spacing_mut().item_spacing.y = SECTION_GAP;
                                match tab {
                                    TypeTab::Character => {
                                        if char_locked {
                                            ui.label(
                                                egui::RichText::new(
                                                    "The text style holds the greyed rows. Detach to edit",
                                                )
                                                .size(10.0)
                                                .color(theme::text::MUTED),
                                            );
                                        }
                                        self.type_character_tab(ui, subject, char_locked);
                                    }
                                    TypeTab::Paragraph => self.type_paragraph_tab(ui, subject),
                                    TypeTab::Block => self.type_block_tab(ui, subject),
                                }
                            });
                    });
                });
            })
            .response;
        let on_head = ctx.pointer_interact_pos().is_some_and(|p| head.contains(p));
        // The picker is an `egui::Window` at `Order::Middle`, neither the popup's rect
        // nor an overlay; asked by layer identity because `picker_ui` runs later.
        let on_picker = ctx
            .pointer_interact_pos()
            .and_then(|p| ctx.layer_id_at(p))
            .is_some_and(|l| l.id == super::picker::layer_id());
        // Recorded every frame, never from inside a short-circuiting `&&`.
        let press_away = super::press_began_away(&ctx, menu.layer_id.id, menu.rect, head);
        if close
            || dismissed_by_click(ClickAway {
                // Primary only: a right-click cancels a gesture and must not close the popup.
                clicked: ctx.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary)),
                over_overlay,
                on_menu: menu.contains_pointer(),
                on_head,
                on_picker,
                // A drag, or a cancel waiting for its button to come up.
                in_gesture: ctx.dragged_id().is_some() || self.gesture_cancelled,
                press_away,
            })
        {
            self.type_menu = None;
        }
        // The router may spend next frame's `Escape` on this popover.
        self.popover_heard = true;
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.type_menu = None;
        }
    }

    // --- tab 1: Character -------------------------------------------------

    /// Case, the one decoration, the two spacings — and everything the family
    /// offers: the optical-size mode, the language, the variable axes and the
    /// OpenType features.
    ///
    /// The lower half builds itself from the font, and comes last so a family
    /// change does not move the top of the tab. `locked` greys the sections a text
    /// style holds and leaves Colour and Language live (`style::type_part` clears both).
    fn type_character_tab(&mut self, ui: &mut egui::Ui, subject: &TypeSubject, locked: bool) {
        let held = |app: &mut Self, ui: &mut egui::Ui, f: &dyn Fn(&mut Self, &mut egui::Ui)| {
            if locked {
                ui.scope(|ui| {
                    super::styles::lock(ui);
                    f(app, ui);
                });
            } else {
                f(app, ui);
            }
        };
        self.color_section(ui, subject);
        held(self, ui, &|app, ui| {
            app.case_section(ui, subject);
            app.decoration_section(ui, subject);
            app.spacing_section(ui, subject);
        });

        let family = attr!(subject, Family, Family);
        let axes = text::family_axes(&family);
        let coords = attr!(subject, Variations, Variations);
        held(self, ui, &|app, ui| {
            app.optical_size_section(ui, subject, &axes, &coords);
        });
        self.language_section(ui, subject);
        held(self, ui, &|app, ui| {
            app.axes_section(ui, subject, &axes, &coords);
            app.features_section(ui, subject, &text::family_features(&family));
        });
    }

    /// The text's own colour, over whatever range a character control acts on.
    ///
    /// Absence means the layer's fill stack, so the swatch can show a gradient while
    /// the slot holds one colour: a run cannot be filled with a ramp but can inherit one.
    fn color_section(&mut self, ui: &mut egui::Ui, subject: &TypeSubject) {
        let mark = self.style_mark(
            subject.id,
            |s| s.color,
            |s, v| s.color = v,
            |v| v.map_or_else(|| "the layer's fill".to_owned(), ui::hex_of),
        );
        let (_, hit) = section_marked(ui, "Colour", mark.as_ref(), |ui| {
            self.char_color_row(ui, subject, CharSlot::Color);
        });
        self.reset_marked(hit, &mark);
    }

    /// Case is a display transform; small caps is not here because drawn small
    /// capitals are the `smcp` OpenType feature, a different thing.
    fn case_section(&mut self, ui: &mut egui::Ui, subject: &TypeSubject) {
        let case = attr!(subject, Case, Case);
        let mixed = subject.mixed(CharAttrKind::Case);
        let current = TextCase::ALL.iter().position(|c| *c == case).unwrap_or(0);
        let mark = self.style_mark(
            subject.id,
            |s| s.case,
            |s, v| s.case = v,
            |v| v.label().to_owned(),
        );
        let (_, hit) = section_marked(ui, "Case", mark.as_ref(), |ui| {
            if let Some(i) = segmented(
                ui,
                MENU_INNER,
                SEG_H,
                4,
                if mixed { usize::MAX } else { current },
                |p, i, rect, on| {
                    if mixed && i == 0 {
                        segment_mixed(p, rect);
                    } else {
                        segment_label(p, rect, TextCase::ALL[i].label(), on);
                    }
                },
            ) && TextCase::ALL[i] != case
            {
                self.apply_char_attrs(subject, vec![CharAttr::Case(TextCase::ALL[i])]);
            }
        });
        self.reset_marked(hit, &mark);
    }

    /// The decoration, and everything under it: thickness, offset, line style and
    /// colour, shown only when there is one.
    ///
    /// One of three, though the model can hold both an underline and a
    /// strikethrough. Picking a decoration clears the other and carries the tuning
    /// across; each number's `None` state, "the font decides", survives a font swap.
    fn decoration_section(&mut self, ui: &mut egui::Ui, subject: &TypeSubject) {
        let under = decoration_of(subject, CharAttrKind::Underline);
        let strike = decoration_of(subject, CharAttrKind::Strikethrough);
        let mixed =
            subject.mixed(CharAttrKind::Underline) || subject.mixed(CharAttrKind::Strikethrough);
        let current = decoration_cell(under, strike);
        // Both lines and everything in the `Decoration` under them.
        let mark = self.style_mark(
            subject.id,
            |s| (s.underline, s.strikethrough),
            |s, (u, k)| {
                s.underline = u;
                s.strikethrough = k;
            },
            |(u, k)| match (u, k) {
                (Some(_), _) => "underline".to_owned(),
                (None, Some(_)) => "strikethrough".to_owned(),
                (None, None) => "none".to_owned(),
            },
        );
        let (_, hit) = section_marked(ui, "Decoration", mark.as_ref(), |ui| {
            let picked = segmented(
                ui,
                MENU_INNER,
                SEG_H,
                3,
                if mixed { usize::MAX } else { current },
                |p, i, rect, on| {
                    if mixed && i == 1 {
                        segment_mixed(p, rect);
                    } else {
                        segment_glyph(p, rect, DECORATION_GLYPHS[i], on);
                    }
                },
            );
            if let Some(i) = picked.filter(|i| mixed || *i != current) {
                let carried = under.or(strike).unwrap_or_default();
                self.apply_char_attrs(subject, decoration_write(i, carried));
                return;
            }
            // Mixed has no one decoration to detail.
            let Some(d) = under.or(strike).filter(|_| !mixed) else {
                return;
            };
            let kind = if under.is_some() {
                CharAttrKind::Underline
            } else {
                CharAttrKind::Strikethrough
            };
            let font_size = subject.font_size();
            let half = (MENU_INNER - COL_GAP) / 2.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = COL_GAP;
                let e = optional_length_field(
                    ui,
                    egui::vec2(half, CELL),
                    Prefix::Icon(icon::LINE_VERTICAL),
                    d.thickness,
                    font_size,
                    Bounds::DECORATION_THICKNESS,
                    // What the face draws, so leaving *Font* keeps the line; zero would erase it.
                    subject
                        .decoration_sizes
                        .map(|(u, s)| if under.is_some() { u } else { s })
                        .filter(|v| *v > 0.0),
                    "Thickness — the font's own unless set",
                );
                if let Some(next) = e.next {
                    let mut d = d;
                    d.thickness = next;
                    self.decoration_write(&e, subject, decoration_attr(kind, Some(d)));
                }
                let e = optional_length_field(
                    ui,
                    egui::vec2(half, CELL),
                    Prefix::Icon(icon::ARROW_UP),
                    d.offset,
                    font_size,
                    Bounds::DECORATION_OFFSET,
                    // Nothing to seed from: the face's offset sign is parley's, not ours.
                    None,
                    "Offset from the baseline, positive upwards — so a lower line is \
                     a negative number. The font's own unless set",
                );
                if let Some(next) = e.next {
                    let mut d = d;
                    d.offset = next;
                    self.decoration_write(&e, subject, decoration_attr(kind, Some(d)));
                }
            });
            // The line style is ours, not parley's (§6.3). The skip-ink toggle is
            // underline-only (`skip_ink` is never read for a strikethrough), so the
            // dropdown takes the width back there.
            let skips = under.is_some();
            let style_w = if skips {
                MENU_INNER - COL_GAP - CELL
            } else {
                MENU_INNER
            };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = COL_GAP;
                if let Some(next) = enum_combo(
                    ui,
                    egui::vec2(style_w, CELL),
                    "type-line-style",
                    icon::WAVE_SINE,
                    &LineStyle::ALL,
                    d.style,
                    LineStyle::label,
                ) {
                    let mut d = d;
                    d.style = next;
                    self.apply_char_attrs(subject, vec![decoration_attr(kind, Some(d))]);
                }
                if skips
                    && field_button(
                        ui,
                        icon::LINK_SIMPLE_HORIZONTAL_BREAK,
                        CELL,
                        14.0,
                        ui::FieldButton::on_if(d.skip_ink),
                    )
                    .on_hover_text(
                        "Skip-ink — break the line around the letters it crosses, \
                         as a browser does",
                    )
                    .clicked()
                {
                    let mut d = d;
                    d.skip_ink = !d.skip_ink;
                    self.apply_char_attrs(subject, vec![decoration_attr(kind, Some(d))]);
                }
            });
            if let Some(side) = DecorationSide::of(kind) {
                self.char_color_row(ui, subject, CharSlot::Decoration(side));
            }
        });
        self.reset_marked(hit, &mark);
    }

    /// The decoration's colour: a swatch, its hex, its opacity, and a reset.
    ///
    /// Absence means the text's own colour ([`TypeSubject::text_color`]).
    ///
    /// The hex is buffered like the paint rows' (half of `9184D9` is a legal colour,
    /// so per-keystroke parsing would leave several undo steps); the opacity goes
    /// through the drag valve. The swatch opens the picker through a `PaintSlot` of
    /// its own ([`crate::panels::paint::PaintSlot::TextDecoration`]).
    fn char_color_row(&mut self, ui: &mut egui::Ui, subject: &TypeSubject, slot: CharSlot) {
        let kind = slot.kind();
        let own = match slot {
            CharSlot::Color => attr!(subject, Color, Color),
            CharSlot::Decoration(side) => decoration_of(subject, side.kind()).and_then(|d| d.color),
        };
        let targeted = self
            .picker
            .as_ref()
            .is_some_and(|p| p.node == subject.id && p.slot == slot.paint_slot());
        let shown = own.unwrap_or_else(|| subject.text_color());
        let ramp = own.is_none().then(|| subject.text_ramp()).flatten();
        // Committed at once.
        let mut now: Option<Option<Color>> = None;
        // Valved: a scrub is one undo step.
        let mut opacity: Option<(egui::Response, f64)> = None;
        let mut open = false;
        let anchor = ui.cursor().min;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = COL_GAP;
            ui::field_row_active(
                ui,
                egui::vec2(MENU_INNER - CELL - COL_GAP, CELL),
                targeted,
                |ui| {
                    ui.spacing_mut().item_spacing.x = 7.0;
                    if ui::swatch(
                        ui,
                        14.0,
                        theme::color::FIELD,
                        match &ramp {
                            Some(stops) => ui::Swatch::Ramp(stops),
                            None => ui::Swatch::Solid(ui::color_to_egui(shown)),
                        },
                    )
                    .on_hover_text(slot.swatch_tooltip(own.is_some(), ramp.is_some()))
                    .clicked()
                    {
                        open = true;
                    }
                    let mut text = match &self.char_hex {
                        Some((s, buf)) if *s == slot => buf.clone(),
                        _ => ui::hex_of(shown),
                    };
                    let resp = ui.add(
                        egui::TextEdit::singleline(&mut text)
                            .frame(egui::Frame::NONE)
                            .desired_width(62.0)
                            .char_limit(ui::COLOUR_CHAR_LIMIT)
                            .font(egui::FontId::proportional(11.5)),
                    );
                    ui::select_all_on_focus(ui, &resp, &text);
                    if resp.gained_focus() || resp.changed() {
                        self.char_hex = Some((slot, text.clone()));
                    }
                    // `Escape` drops the buffer and writes nothing: the clear is outside
                    // the guard, the write inside.
                    if resp.lost_focus() {
                        self.char_hex = None;
                        if ui::defocus_commits(&resp) {
                            // A typed alpha sets the opacity; a colour without one keeps it.
                            // Compared as bytes, so a click through the field writes nothing.
                            match crate::colour_text::parse(&text) {
                                Some(_) if crate::colour_text::matches(&text, shown) => {}
                                Some(t) => {
                                    let c = t.color(shown.components[3]);
                                    self.remember_colour(c);
                                    now = Some(Some(c));
                                }
                                None => {
                                    if let Some(why) = crate::colour_text::refusal(&text) {
                                        self.session.info(why);
                                    }
                                }
                            }
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut pct = f64::from(shown.components[3]) * 100.0;
                        let resp = ui::bare_drag_value(
                            ui,
                            egui::DragValue::new(&mut pct)
                                .suffix("%")
                                .speed(ui::scrub(0.5))
                                .max_decimals(0)
                                .range(0.0..=100.0),
                        );
                        opacity = Some((resp, pct));
                    });
                },
            );
            // Live while there is anything to put back (`TypeSubject::ragged`).
            if reset_slot(ui, own.is_some() || subject.ragged(kind), slot.reset_tip()) {
                now = Some(None);
            }
        });
        if let Some(color) = now {
            if let Some(attr) = char_color_attr(subject, slot, color) {
                self.apply_char_attrs(subject, vec![attr]);
            }
        } else if let Some((resp, pct)) = opacity
            // Only while the field is in use: an idle frame would build a transaction
            // from a stale snapshot whose `SetTextStyle` could undo another commit.
            //
            // valve-gate-not-a-commit: the commit is `char_valve`'s, on its own
            // engagement latch; this only decides whether to reach it.
            && (resp.dragged()
                || resp.drag_stopped()
                || resp.changed()
                || resp.lost_focus())
        {
            // Fading an inherited colour adopts it: the alpha lives in the slot's `color`.
            let faded = Some(shown.with_alpha((pct / 100.0) as f32));
            if let Some(attr) = char_color_attr(subject, slot, faded) {
                self.char_valve(&resp, subject, attr);
            }
        }
        // The picker needs a colour in the slot to point at.
        if open {
            if own.is_none()
                && let Some(attr) = char_color_attr(subject, slot, Some(shown))
            {
                self.apply_char_attrs(subject, vec![attr]);
            }
            self.open_picker(ui, subject.id, slot.paint_slot(), anchor);
        }
    }

    /// Word spacing and baseline shift, side by side.
    ///
    /// Baseline shift is signed and a draw offset: it moves the run without changing
    /// the line box.
    fn spacing_section(&mut self, ui: &mut egui::Ui, subject: &TypeSubject) {
        let word = self.style_mark(
            subject.id,
            |s| s.word_spacing,
            |s, v| s.word_spacing = v,
            length_words,
        );
        let shift = self.style_mark(
            subject.id,
            |s| s.baseline_shift,
            |s, v| s.baseline_shift = v,
            length_words,
        );
        section(ui, "Baseline & word spacing", |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = COL_GAP;
                let half = (MENU_INNER - COL_GAP) / 2.0;
                self.length_char_field_marked(
                    ui,
                    subject,
                    egui::vec2(half, CELL),
                    CharAttrKind::WordSpacing,
                    Prefix::Icon(icon::ARROWS_IN_LINE_HORIZONTAL),
                    Bounds::TRACKING,
                    "Word spacing",
                    word.as_ref(),
                );
                self.length_char_field_marked(
                    ui,
                    subject,
                    egui::vec2(half, CELL),
                    CharAttrKind::BaselineShift,
                    Prefix::Icon(icon::TEXT_SUPERSCRIPT),
                    Bounds::SIGNED_TRACKING,
                    "Baseline shift",
                    shift.as_ref(),
                );
            });
        });
    }

    /// `opsz`, above the axis list and with a mode of its own. Auto writes no
    /// coordinate, so the shaper takes the font size; Manual pins a value.
    fn optical_size_section(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        axes: &[FontAxis],
        coords: &[AxisSetting],
    ) {
        let Some(opsz) = axes.iter().find(|a| a.tag == OPSZ) else {
            return;
        };
        let auto = !coords.iter().any(|a| a.tag == OPSZ);
        // The one coordinate, absent for Auto; the Axes section marks the rest.
        let mark = self.style_mark(
            subject.id,
            |s| AxesOf {
                shown: opsz_of(&s.variations),
                whole: s.variations.clone(),
            },
            |s, v| s.variations = axes_reset(&s.variations, &v.whole, |t| t == OPSZ),
            |v| v.shown.map_or_else(|| "auto".to_owned(), mark_num),
        );
        let (_, hit) = section_marked(ui, "Optical size", mark.as_ref(), |ui| {
            if let Some(i) = segmented(
                ui,
                MENU_INNER,
                SEG_H,
                2,
                usize::from(!auto),
                |p, i, rect, on| segment_label(p, rect, if i == 0 { "Auto" } else { "Manual" }, on),
            ) && (i == 0) != auto
            {
                let mut next: Vec<AxisSetting> =
                    coords.iter().copied().filter(|a| a.tag != OPSZ).collect();
                if i == 1 {
                    // `FontAxis::clamp`, not `f64::clamp`, which panics on reversed `fvar` ends.
                    next.push(AxisSetting::new(OPSZ, opsz.clamp(subject.font_size())));
                }
                self.apply_char_attrs(subject, vec![CharAttr::Variations(next)]);
                return;
            }
            if auto {
                return;
            }
            // No name and no reset: the section label names the axis and Auto is the reset.
            self.opsz_row(ui, subject, opsz, coords);
        });
        self.reset_marked(hit, &mark);
    }

    /// The list marker: a dropdown of `None` plus the eight kinds.
    ///
    /// Turning a marker on also opens a gutter for it, once: the marker is
    /// right-aligned on the paragraph's start edge, so at `indent_start` 0 it draws
    /// outside the box. The nesting level sits under the dropdown, where there is a list.
    fn list_section(&mut self, ui: &mut egui::Ui, subject: &TypeSubject, shown: &ParagraphStyle) {
        let current = shown.marker;
        let mixed = subject.para_mixed(ParaAttrKind::Marker);
        let label = match (mixed, current) {
            (true, _) => "Mixed".to_string(),
            (false, None) => "None".to_string(),
            (false, Some(m)) => m.label().to_string(),
        };
        let mut pick: Option<Option<ListMarker>> = None;
        let mut nest: Option<(egui::Response, u8)> = None;
        // The marker and its level; the gutter a pick opens is marked by Spacing & indent.
        let mark = self.para_mark(
            subject.id,
            |p| (p.marker, p.level),
            |p, (m, l)| {
                p.marker = m;
                p.level = l;
            },
            |(m, _)| m.map_or_else(|| "None".to_owned(), |m| m.label().to_owned()),
        );
        let (_, hit) = section_marked(ui, "List", mark.as_ref(), |ui| {
            ui.scope(|ui| {
                // `CELL` whole: a combo strokes inside its own rect.
                ui.spacing_mut().interact_size.y = CELL;
                ui.spacing_mut().button_padding.y = 0.0;
                egui::ComboBox::from_id_salt("type-list-marker")
                    .icon(ui::combo_chevron)
                    .width(MENU_INNER)
                    .selected_text(ui::glyph_and_text(
                        match current {
                            Some(m) if m.ordered() => icon::LIST_NUMBERS,
                            Some(_) => icon::LIST_DASHES,
                            None => icon::LIST,
                        },
                        &label,
                    ))
                    .show_ui(ui, |ui| {
                        ui::menu_rows(ui);
                        if ui
                            .selectable_label(!mixed && current.is_none(), "None")
                            .clicked()
                        {
                            pick = Some(None);
                        }
                        for m in ListMarker::ALL {
                            if ui
                                .selectable_label(!mixed && current == Some(m), m.label())
                                .clicked()
                            {
                                pick = Some(Some(m));
                            }
                        }
                    });
            });
            if mixed || current.is_some() {
                let level_mixed = subject.para_mixed(ParaAttrKind::Level);
                let mut level = f64::from(shown.level);
                let resp = value_field(
                    ui,
                    egui::vec2(MENU_INNER, CELL),
                    // A word: every "indent" glyph is taken by the section above.
                    Prefix::Text("Level"),
                    &mut level,
                    Scrub::whole(0.05).range(0.0..=f64::from(MAX_LIST_LEVEL)),
                    |d| mixed_text(d.max_decimals(0), level_mixed),
                )
                .on_hover_text("Nesting depth — each level steps in by one more start indent");
                nest = Some((
                    resp,
                    level.round().clamp(0.0, f64::from(MAX_LIST_LEVEL)) as u8,
                ));
            }
        });
        if hit {
            self.reset_marked(hit, &mark);
            return;
        }
        // The frame the scrub ended on carries no new value and is the one the valve commits on.
        if let Some((resp, level)) = nest.filter(|(r, l)| *l != shown.level || r.drag_stopped()) {
            self.valve_paragraph(&resp, subject, ParaAttr::Level(level));
        }
        let Some(next) = pick else {
            return;
        };
        // `para_ragged`: over a non-partial range `shown` reads the first paragraph,
        // so picking the shown value must still write when another disagrees.
        if !subject.para_ragged(ParaAttrKind::Marker) && current == next {
            return;
        }
        let attrs = marker_attrs(next, shown.indent_start, subject.font_size());
        self.apply_para_attrs(subject, attrs);
    }

    fn language_section(&mut self, ui: &mut egui::Ui, subject: &TypeSubject) {
        let current = match subject.shown(CharAttrKind::Locale) {
            CharAttr::Locale(v) => v,
            _ => None,
        };
        // An unlisted tag (an imported `xml:lang`) shows as itself, not *None*.
        let label = match current.as_deref() {
            None => "None".to_string(),
            Some(tag) => LANGUAGES
                .iter()
                .find(|(t, _)| *t == tag)
                .map_or_else(|| tag.to_string(), |(_, name)| (*name).to_string()),
        };
        let mut pick: Option<Option<String>> = None;
        let mark = self.style_mark(
            subject.id,
            |s| s.locale.clone(),
            |s, v| s.locale = v,
            |v| match v.as_deref() {
                None => "None".to_owned(),
                Some(tag) => LANGUAGES
                    .iter()
                    .find(|(t, _)| *t == tag)
                    .map_or_else(|| tag.to_owned(), |(_, name)| (*name).to_owned()),
            },
        );
        let (_, hit) = section_marked(ui, "Language", mark.as_ref(), |ui| {
            ui.scope(|ui| {
                // `CELL` whole: a combo strokes inside its own rect.
                ui.spacing_mut().interact_size.y = CELL;
                ui.spacing_mut().button_padding.y = 0.0;
                egui::ComboBox::from_id_salt("type-language")
                    .icon(ui::combo_chevron)
                    .width(MENU_INNER)
                    .selected_text(ui::glyph_and_text(icon::TRANSLATE, &label))
                    .show_ui(ui, |ui| {
                        ui::menu_rows(ui);
                        if ui.selectable_label(current.is_none(), "None").clicked() {
                            pick = Some(None);
                        }
                        for (tag, name) in LANGUAGES {
                            let on = current.as_deref() == Some(*tag);
                            if ui.selectable_label(on, *name).clicked() {
                                pick = Some(Some((*tag).to_string()));
                            }
                        }
                    });
            });
        });
        if hit {
            self.reset_marked(hit, &mark);
        } else if let Some(next) = pick {
            self.apply_char_attrs(subject, vec![CharAttr::Locale(next)]);
        }
    }

    /// Every `fvar` axis with a value of its own, minus the ones a dedicated
    /// control already drives, in a list that scrolls rather than growing the card.
    fn axes_section(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        axes: &[FontAxis],
        coords: &[AxisSetting],
    ) {
        let listed: Vec<&FontAxis> = axes
            .iter()
            .filter(|a| a.tag != OPSZ && !AXES_DRIVEN_ELSEWHERE.contains(&a.tag) && !a.hidden)
            .collect();
        if listed.is_empty() {
            return;
        }
        // Not `opsz` (Optical size's) nor the axes the variant drives.
        let own = |t: Tag| t != OPSZ && !AXES_DRIVEN_ELSEWHERE.contains(&t);
        let mark = self.style_mark(
            subject.id,
            move |s| AxesOf {
                shown: s
                    .variations
                    .iter()
                    .copied()
                    .filter(|a| own(a.tag))
                    .collect::<Vec<_>>(),
                whole: s.variations.clone(),
            },
            move |s, v| s.variations = axes_reset(&s.variations, &v.whole, own),
            |v| match v.shown.len() {
                0 => "the font's defaults".to_owned(),
                1 => "1 axis set".to_owned(),
                n => format!("{n} axes set"),
            },
        );
        let (_, hit) = section_marked(ui, "Axes", mark.as_ref(), |ui| {
            egui::ScrollArea::vertical()
                .id_salt("type-axes")
                .max_height(LIST_MAX_H)
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 9.0;
                    for axis in listed {
                        self.axis_row(ui, subject, axis, coords, MENU_INNER - LIST_GUTTER);
                    }
                });
        });
        self.reset_marked(hit, &mark);
    }

    /// The face's OpenType features as curated switches over a search field; a
    /// `cvXX` with more than one named value draws a [`feature_choice_row`] instead.
    ///
    /// The well-known tags (`tnum` first) sort to the top. The list leaves out the
    /// tags the shaper requires or applies per script, since a switch for them only
    /// mis-shapes text; [`REGISTRY`] carries the verdict per tag and *Show all
    /// features* restores the rest. The search field is unconditional so the card's
    /// layout does not depend on the family.
    fn features_section(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        available: &[FaceFeature],
    ) {
        if available.is_empty() {
            return;
        }
        // `set` resolves at the first run, so a disagreeing tag draws
        // `ui::switch_row_mixed` and a toggle writes into each run (`write_feature_tag`).
        let set = attr!(subject, Features, Features);
        let mut ordered: Vec<(Tag, FeatureLabel)> = available
            .iter()
            .map(|f| (f.tag, feature_label(f)))
            .collect();
        ordered.sort_by_key(|(tag, _)| (feature_rank(*tag), tag.to_string()));
        // A withheld feature the document sets is shown anyway: a state nobody can undo is a trap.
        let shown = |tag: &Tag, l: &FeatureLabel| l.offered || set.iter().any(|f| f.tag == *tag);
        // One writer for both kinds of row: two would make two `apply_char_attrs`
        // calls off one `set` clone, dropping the first.
        let mut written: Option<(Tag, u16)> = None;
        let mark = self.style_mark(
            subject.id,
            |s| s.features.clone(),
            |s, v| s.features = v,
            |v| match v.len() {
                0 => "none set".to_owned(),
                1 => "1 feature set".to_owned(),
                n => format!("{n} features set"),
            },
        );
        let (_, hit) = section_marked(ui, "OpenType", mark.as_ref(), |ui| {
            field_row(ui, egui::vec2(MENU_INNER, CELL), |ui| {
                ui.spacing_mut().item_spacing.x = SEARCH_GAP;
                ui.label(theme::icon_text(
                    icon::MAGNIFYING_GLASS,
                    SEARCH_ICON_PT,
                    theme::text::FAINT,
                ));
                // A `TextEdit` asking for more than its row grows the row, pinning the card's width.
                ui.add(
                    egui::TextEdit::singleline(&mut self.feature_filter)
                        .frame(egui::Frame::NONE)
                        .desired_width(
                            MENU_INNER - ui::FIELD_PAD_X * 2.0 - SEARCH_ICON_PT - SEARCH_GAP * 2.0,
                        )
                        .hint_text("Search features"),
                );
            });
            let needle = self.feature_filter.to_lowercase();
            // Written once: the row loop and the reveal's count must agree.
            let matches = |tag: &Tag, l: &FeatureLabel| {
                needle.is_empty()
                    || l.name.to_lowercase().contains(&needle)
                    || tag.to_string().contains(&needle)
            };
            // Exactly the rows *Show all features* would add: withheld, and matching.
            let hidden = ordered
                .iter()
                .filter(|(t, l)| !shown(t, l) && matches(t, l))
                .count();
            egui::ScrollArea::vertical()
                .id_salt("type-features")
                .max_height(LIST_MAX_H)
                .show(ui, |ui| {
                    ui.set_width(MENU_INNER - LIST_GUTTER);
                    ui.spacing_mut().item_spacing.y = 1.0;
                    for (tag, label) in &ordered {
                        if !self.show_all_features && !shown(tag, label) {
                            continue;
                        }
                        if !matches(tag, label) {
                            continue;
                        }
                        let value = set.iter().find(|f| f.tag == *tag).map_or(0, |f| f.value);
                        if label.values.len() > 1 {
                            if let Some(v) = feature_choice_row(ui, *tag, label, value) {
                                written = Some((*tag, v));
                            }
                            continue;
                        }
                        let mixed = subject.feature_mixed(*tag);
                        let row = if mixed {
                            ui::switch_row_mixed(ui, &label.name, FEATURE_ROW_H)
                        } else {
                            ui::switch_row(ui, &label.name, value > 0, FEATURE_ROW_H)
                        };
                        // A mixed row has no position, so the tooltip says what a click does.
                        let row = if mixed {
                            row.on_hover_text(format!(
                                "{}\n\nMixed — some of the selection has this on. \
                                 Click to turn it on for all of it.",
                                label.hover.as_str()
                            ))
                        } else {
                            row.on_hover_text(label.hover.as_str())
                        };
                        if row.clicked() {
                            // An explicit `0`, not a removal: absent means the font's default.
                            // A mixed row turns the feature on.
                            written = Some((*tag, if mixed { 1 } else { u16::from(value == 0) }));
                        }
                    }
                });
            // Under the list, not beside the search, so it does not read as a filter.
            if let Some(text) = features_reveal(self.show_all_features, hidden)
                && ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new(text)
                                .size(ui::SEGMENT_LABEL_PT)
                                .color(theme::text::FAINT),
                        )
                        .sense(egui::Sense::click())
                        // A selectable `Label` reports the I-beam cursor.
                        .selectable(false),
                    )
                    .on_hover_text(
                        "The rest are required, applied per script by the shaper, or belong to \
                         vertical writing — switching one by hand mis-shapes text",
                    )
                    .clicked()
            {
                self.show_all_features = !self.show_all_features;
            }
        });
        if hit {
            self.reset_marked(hit, &mark);
        } else if let Some((tag, value)) = written {
            self.write_feature_tag(subject, tag, value);
        }
    }

    /// Set one OpenType tag across the selection, leaving every other tag in each
    /// run as it was.
    ///
    /// Over a selection each run keeps its own list
    /// (`EditorSession::style_selection_per_run`); over a whole node a write
    /// flattens the spans. The merge lives here because core knows nothing about
    /// OpenType.
    fn write_feature_tag(&mut self, subject: &TypeSubject, tag: Tag, value: u16) {
        let merge = move |list: &[FeatureSetting]| {
            let mut next = list.to_vec();
            match next.iter_mut().find(|f| f.tag == tag) {
                Some(existing) => existing.value = value,
                None => next.push(FeatureSetting { tag, value }),
            }
            next
        };
        if subject.partial {
            if let Some(s) = self.text.as_mut().filter(|s| s.id == subject.id) {
                s.editor
                    .style_selection_per_run(CharAttrKind::Features, |old| {
                        let CharAttr::Features(list) = old else {
                            return old.clone();
                        };
                        CharAttr::Features(merge(list))
                    });
            }
            self.preview_session();
            return;
        }
        let set = attr!(subject, Features, Features);
        self.apply_char_attrs(subject, vec![CharAttr::Features(merge(&set))]);
    }

    /// One row of the axis list: the axis's name and its value on one line, its
    /// slider under them, since the slider needs the whole width to be aimable.
    fn axis_row(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        axis: &FontAxis,
        coords: &[AxisSetting],
        width: f32,
    ) {
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = LABEL_GAP;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = COL_GAP;
                ui.allocate_ui_with_layout(
                    egui::vec2(width - AXIS_FIELD_W - CELL - COL_GAP * 2.0, CELL),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(axis.name.as_str())
                                    .size(11.5)
                                    .color(theme::text::MUTED),
                            )
                            .truncate(),
                        )
                        .on_hover_text(match axis_sign_hint(axis.tag) {
                            Some(hint) => format!("{} ({}) — {hint}", axis.name, axis.tag),
                            None => format!("{} ({})", axis.name, axis.tag),
                        });
                    },
                );
                self.axis_field(ui, subject, axis, coords, AXIS_FIELD_W, true);
            });
            self.axis_slider(ui, subject, axis, coords, width, true);
        });
    }

    /// `opsz`'s slider and its readout on one line; the section's label names the axis.
    fn opsz_row(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        axis: &FontAxis,
        coords: &[AxisSetting],
    ) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = COL_GAP;
            // `false`: an absent `opsz` coordinate is the Auto state.
            self.axis_slider(
                ui,
                subject,
                axis,
                coords,
                MENU_INNER - AXIS_FIELD_W - COL_GAP,
                false,
            );
            self.axis_field(ui, subject, axis, coords, AXIS_FIELD_W, false);
        });
    }

    /// The numeric field of one axis, and — when `resettable` — the 26×26 slot
    /// that puts it back to the font's own value.
    fn axis_field(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        axis: &FontAxis,
        coords: &[AxisSetting],
        width: f32,
        resettable: bool,
    ) {
        let current = axis_value(axis, coords);
        let mut value = current;
        // The slider keeps its knob: it has nowhere to mean "mixed".
        let mixed = subject.mixed(CharAttrKind::Variations);
        let resp = value_field(
            ui,
            egui::vec2(width, CELL),
            Prefix::Text(axis_prefix(axis.tag)),
            &mut value,
            Scrub::fine(0.5, 1).range(axis.min..=axis.max),
            |d| mixed_text(d.custom_formatter(ui::number(1)), mixed),
        );
        if resettable
            && reset_slot(
                ui,
                current != axis.default || subject.ragged(CharAttrKind::Variations),
                "Back to the font's default",
            )
        {
            self.write_axis(subject, axis, coords, current, axis.default, resettable);
            return;
        }
        // Valved: the prefix strip is a scrub, and `write_axis`'s `value == current`
        // guard cannot absorb it (`apply_char_attrs` sets no preview).
        self.axis_valve(&resp, subject, axis, coords, value, resettable);
    }

    fn axis_slider(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        axis: &FontAxis,
        coords: &[AxisSetting],
        width: f32,
        clear_at_default: bool,
    ) {
        let current = axis_value(axis, coords);
        let mut value = current;
        let resp = ui::slider(ui, width, &mut value, axis.min..=axis.max);
        // Handed over every frame: `current` reads through the preview, so a
        // `value != current` guard would be false on the release frame.
        self.axis_valve(&resp, subject, axis, coords, value, clear_at_default);
    }

    /// Write one axis coordinate, if it moved.
    ///
    /// The decision is [`axis_coords_after`]; [`Self::axis_valve`] valves it instead.
    fn write_axis(
        &mut self,
        subject: &TypeSubject,
        axis: &FontAxis,
        coords: &[AxisSetting],
        current: f64,
        value: f64,
        clear_at_default: bool,
    ) {
        if value == current {
            return;
        }
        let next = axis_coords_after(axis, coords, value, clear_at_default);
        self.apply_char_attrs(subject, vec![CharAttr::Variations(next)]);
    }

    /// The same write, routed through the drag valve: a preview while the control
    /// is held, one commit on release.
    ///
    /// `clear_at_default` means what it means in [`Self::write_axis`]: an absent
    /// coordinate differs from one equal to the font's value (for `opsz`, a
    /// different mode), and both controls must save the same file for the same number.
    fn axis_valve(
        &mut self,
        resp: &egui::Response,
        subject: &TypeSubject,
        axis: &FontAxis,
        coords: &[AxisSetting],
        value: f64,
        clear_at_default: bool,
    ) {
        let next = axis_coords_after(axis, coords, value, clear_at_default);
        self.char_valve(resp, subject, CharAttr::Variations(next));
    }

    // --- tab 2: Paragraph -------------------------------------------------

    /// Spacing, the three indents and which lines the first-line one reaches, the
    /// last line of a justified paragraph, and the base direction.
    ///
    /// Two scopes in one tab: the first section acts on the paragraphs the selection
    /// touches and can read *mixed*; the two below act on the whole layer. `shown`
    /// is what the spannable controls display and `p` the node's own defaults;
    /// building a node-level transaction from `shown` would promote one
    /// paragraph's indent.
    fn type_paragraph_tab(&mut self, ui: &mut egui::Ui, subject: &TypeSubject) {
        let p = subject.paragraph.clone();
        let shown = subject.shown_paragraph();
        let font_size = subject.font_size();
        let spacing_mark = self.para_mark(
            subject.id,
            |p| (p.spacing, p.indent, p.hanging, p.indent_start, p.indent_end),
            |p, (s, i, h, a, b)| {
                p.spacing = s;
                p.indent = i;
                p.hanging = h;
                p.indent_start = a;
                p.indent_end = b;
            },
            |(s, i, h, a, b)| {
                let i = match h {
                    true => format!("hanging {}", length_words(i)),
                    false => format!("first line {}", length_words(i)),
                };
                format!(
                    "space {}, {i}, start {}, end {}",
                    length_words(s),
                    length_words(a),
                    length_words(b)
                )
            },
        );
        let spacing_mark = self.binding_mark(
            subject.id,
            ondin_core::variable::TEXT_PARAGRAPH_SPACING,
            spacing_mark,
        );
        let (_, hit) = section_marked(ui, "Spacing & indent", spacing_mark.as_ref(), |ui| {
            let half = (MENU_INNER - COL_GAP) / 2.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = COL_GAP;
                // The section's mark is the chip's reset too.
                let key = self.type_key(subject, ondin_core::variable::TEXT_PARAGRAPH_SPACING);
                if self.type_bound(key) {
                    self.type_bound_ui(ui, key, half, None);
                } else {
                    let e = length_field(
                        ui,
                        egui::vec2(half, CELL),
                        Prefix::Icon(icon::PARAGRAPH),
                        shown.spacing,
                        subject.para_mixed(ParaAttrKind::Spacing),
                        font_size,
                        Bounds::PARAGRAPH_SPACING,
                        "Space before this paragraph",
                    );
                    self.paragraph_length(subject, &e, ParaAttr::Spacing);
                    if let Some(key) = key {
                        self.field_doors(ui, key, &e.resp);
                    }
                }
                let e = length_field(
                    ui,
                    egui::vec2(half, CELL),
                    Prefix::Icon(if shown.hanging {
                        icon::TEXT_OUTDENT
                    } else {
                        icon::TEXT_INDENT
                    }),
                    shown.indent,
                    subject.para_mixed(ParaAttrKind::Indent),
                    font_size,
                    Bounds::INDENT,
                    if shown.hanging {
                        "Hanging indent — how far the continuation lines come in"
                    } else {
                        "First-line indent"
                    },
                );
                self.paragraph_length(subject, &e, ParaAttr::Indent);
            });
            // One value, two buttons that press as a segmented control would. See
            // `ParagraphStyle::indent`.
            let hanging_mixed = subject.para_mixed(ParaAttrKind::Hanging);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = COL_GAP;
                for (hanging, glyph, tip) in [
                    (
                        false,
                        icon::TEXT_INDENT,
                        "Indent the first line of each paragraph",
                    ),
                    (
                        true,
                        icon::TEXT_OUTDENT,
                        "Hanging — indent the continuation lines instead",
                    ),
                ] {
                    // Neither lit when the range disagrees: this pair's mixed state.
                    if field_button(
                        ui,
                        glyph,
                        CELL,
                        14.0,
                        ui::FieldButton::on_if(!hanging_mixed && shown.hanging == hanging),
                    )
                    .on_hover_text(tip)
                    .clicked()
                        && (subject.para_ragged(ParaAttrKind::Hanging) || shown.hanging != hanging)
                    {
                        self.apply_para_attrs(subject, vec![ParaAttr::Hanging(hanging)]);
                    }
                }
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = COL_GAP;
                let e = length_field(
                    ui,
                    egui::vec2(half, CELL),
                    Prefix::Icon(icon::ARROW_LINE_LEFT),
                    shown.indent_start,
                    subject.para_mixed(ParaAttrKind::IndentStart),
                    font_size,
                    Bounds::INDENT,
                    "Indent from the start edge — every line",
                );
                self.paragraph_length(subject, &e, ParaAttr::IndentStart);
                let e = length_field(
                    ui,
                    egui::vec2(half, CELL),
                    Prefix::Icon(icon::ARROW_LINE_RIGHT),
                    shown.indent_end,
                    subject.para_mixed(ParaAttrKind::IndentEnd),
                    font_size,
                    Bounds::INDENT,
                    // An auto-width node has no wrap width for an end indent to narrow.
                    if matches!(subject.sizing, TextSizing::Auto) {
                        "Indent from the end edge — needs a box to measure from"
                    } else {
                        "Indent from the end edge — every line"
                    },
                );
                self.paragraph_length(subject, &e, ParaAttr::IndentEnd);
            });
        });
        if hit {
            self.reset_marked(hit, &spacing_mark);
            return;
        }

        self.list_section(ui, subject, &shown);

        if p.align == TextAlign::Justify {
            let mark = self.para_mark(
                subject.id,
                |p| p.justify_last,
                |p, v| p.justify_last = v,
                |v| v.label().to_owned(),
            );
            let (row, hit) = section_marked(ui, "Last line", mark.as_ref(), |ui| {
                ui.scope(|ui| {
                    let current = JustifyLast::ALL
                        .iter()
                        .position(|j| *j == p.justify_last)
                        .unwrap_or(0);
                    if let Some(i) =
                        segmented(ui, MENU_INNER, SEG_H, 3, current, |painter, i, rect, on| {
                            segment_label(painter, rect, JustifyLast::ALL[i].label(), on)
                        })
                        && JustifyLast::ALL[i] != p.justify_last
                    {
                        self.commit_paragraph(
                            subject.id,
                            ParagraphStyle {
                                justify_last: JustifyLast::ALL[i],
                                ..p.clone()
                            },
                        );
                    }
                })
                .response
            });
            row.on_hover_text(WHOLE_LAYER);
            if hit {
                self.reset_marked(hit, &mark);
                return;
            }
        }

        let mark = self.para_mark(
            subject.id,
            |p| p.direction,
            |p, v| p.direction = v,
            |v| v.label().to_owned(),
        );
        let (row, hit) = section_marked(ui, "Text direction", mark.as_ref(), |ui| {
            ui.scope(|ui| {
                let current = TextDirection::ALL
                    .iter()
                    .position(|d| *d == p.direction)
                    .unwrap_or(0);
                if let Some(i) =
                    segmented(ui, MENU_INNER, SEG_H, 3, current, |painter, i, rect, on| {
                        segment_label(painter, rect, TextDirection::ALL[i].label(), on)
                    })
                    && TextDirection::ALL[i] != p.direction
                {
                    self.commit_paragraph(
                        subject.id,
                        ParagraphStyle {
                            direction: TextDirection::ALL[i],
                            ..p.clone()
                        },
                    );
                }
            })
            .response
        });
        row.on_hover_text(WHOLE_LAYER);
        self.reset_marked(hit, &mark);
    }

    /// Route one of the four spannable `Length` fields: a unit click commits now, a
    /// scrub goes through the valve.
    ///
    /// `into` builds the [`ParaAttr`], the only thing the four fields differ in.
    fn paragraph_length(
        &mut self,
        subject: &TypeSubject,
        e: &LengthEdit,
        into: fn(Length) -> ParaAttr,
    ) {
        let Some(next) = e.next else {
            return;
        };
        if e.immediate {
            self.apply_para_attrs(subject, vec![into(next)]);
        } else {
            self.valve_paragraph(&e.resp, subject, into(next));
        }
    }

    // --- tab 3: Box -------------------------------------------------------

    /// Put `id`'s box into the panel's sizing cell `i` — auto width, auto height,
    /// fixed.
    ///
    /// A method because the context menu is a second door onto the same states
    /// (`context-menus.md` §5.6). The switch adopts the shaped size, read from the
    /// preview layout, so a mode change does not move the text, and goes through
    /// `text_session_restyled` so a live editor re-shapes.
    pub(crate) fn set_text_sizing(&mut self, id: NodeId, i: usize) {
        let Some(sizing) = self.session.display_node(id).and_then(|n| match n.kind() {
            ondin_core::NodeKind::Text { sizing, .. } => Some(*sizing),
            _ => None,
        }) else {
            return;
        };
        if sizing.cell() == i {
            return;
        }
        let shaped = self
            .session
            .preview_text_layout(id)
            .map(|l| l.size)
            .unwrap_or(Size::new(200.0, 60.0));
        self.commit_edit(Transaction(vec![Operation::SetGeometry {
            id,
            geometry: GeometryPatch::TextSizing(sizing.to_cell(i, shaped)),
        }]));
        self.text_session_restyled();
    }

    /// The box itself: how it is sized, what it is measured to, what happens to
    /// text that does not fit, and where lines may break.
    fn type_block_tab(&mut self, ui: &mut egui::Ui, subject: &TypeSubject) {
        let b = subject.block;
        // A reset ends the tab's frame so no later section acts on the `b` it replaced.
        let mark = self.sizing_mark(subject.id);
        let (_, hit) = section_marked(ui, "Sizing", mark.as_ref(), |ui| {
            if let Some(i) = segmented(
                ui,
                MENU_INNER,
                SEG_H,
                3,
                subject.sizing.cell(),
                |p, i, rect, on| segment_label(p, rect, TextSizing::LABELS[i], on),
            ) && i != subject.sizing.cell()
            {
                self.set_text_sizing(subject.id, i);
            }
        });
        if hit {
            self.reset_marked(hit, &mark);
            return;
        }

        let trim_index = BoxTrim::ALL.iter().position(|t| *t == b.trim).unwrap_or(0);
        let mark = self.block_mark(
            subject.id,
            |b| b.trim,
            |b, v| b.trim = v,
            |v| v.label().to_owned(),
        );
        let (trim_row, hit) = section_marked(ui, "Box trim", mark.as_ref(), |ui| {
            ui.scope(|ui| {
                if let Some(i) =
                    segmented(ui, MENU_INNER, SEG_H, 3, trim_index, |p, i, rect, on| {
                        segment_label(p, rect, BoxTrim::ALL[i].label(), on)
                    })
                    && BoxTrim::ALL[i] != b.trim
                {
                    self.commit_trim(
                        subject.id,
                        BlockStyle {
                            trim: BoxTrim::ALL[i],
                            ..b
                        },
                    );
                }
            })
            .response
        });
        trim_row.on_hover_text(b.trim.hint());
        if hit {
            self.reset_marked(hit, &mark);
            return;
        }

        let mark = self.block_mark(
            subject.id,
            |b| b.overflow,
            |b, v| b.overflow = v,
            |v| v.label().to_owned(),
        );
        let (_, hit) = section_marked(ui, "Overflow", mark.as_ref(), |ui| {
            let over_index = TextOverflow::ALL
                .iter()
                .position(|o| *o == b.overflow)
                .unwrap_or(0);
            if let Some(i) = segmented(ui, MENU_INNER, SEG_H, 3, over_index, |p, i, rect, on| {
                segment_label(p, rect, TextOverflow::ALL[i].label(), on)
            }) && TextOverflow::ALL[i] != b.overflow
            {
                self.commit_block(
                    subject.id,
                    BlockStyle {
                        overflow: TextOverflow::ALL[i],
                        ..b
                    },
                );
            }
        });
        if hit {
            self.reset_marked(hit, &mark);
            return;
        }

        let mark = self.block_mark(
            subject.id,
            |b| b.max_lines,
            |b, v| b.max_lines = v,
            |v| match v {
                0 => "no limit".to_owned(),
                n => n.to_string(),
            },
        );
        let (_, hit) = section_marked(ui, "Max lines", mark.as_ref(), |ui| {
            let mut lines = f64::from(b.max_lines);
            let resp = value_field(
                ui,
                egui::vec2(MENU_INNER, CELL),
                Prefix::Icon(icon::LIST_NUMBERS),
                &mut lines,
                Scrub::whole(0.25).range(0.0..=f64::from(MAX_LINE_LIMIT)),
                |d| {
                    if b.max_lines == 0 {
                        return d.custom_formatter(|_, _| "No limit".into());
                    }
                    d.max_decimals(0)
                },
            )
            .on_hover_text("Most lines to lay out; 0 is no limit");
            let next = BlockStyle {
                max_lines: lines.round().clamp(0.0, f64::from(MAX_LINE_LIMIT)) as u32,
                ..b
            };
            self.edit_valve(
                &resp,
                Transaction(vec![Operation::SetBlockStyle {
                    id: subject.id,
                    block: next,
                }]),
            );
            if b.overflow == TextOverflow::Ellipsis
                && b.max_lines == 0
                && !subject.sizing.height_is_authored()
            {
                ui.label(
                    egui::RichText::new("An ellipsis needs a line limit or a fixed height.")
                        .size(10.0)
                        .color(theme::text::FAINT),
                );
            }
        });
        if hit {
            self.reset_marked(hit, &mark);
            return;
        }

        self.wrap_section(ui, subject);
        self.optical_margin_section(ui, subject);
    }

    /// The three wrapping controls, stacked, each with its own label.
    ///
    /// Word break and Long words dim under `NoWrap`, which suppresses line breaking
    /// outright. They are dimmed, not reset: a stored `word_break` survives a trip
    /// through `NoWrap`. The gate stops there, since hard breaks still break.
    fn wrap_section(&mut self, ui: &mut egui::Ui, subject: &TypeSubject) {
        let p = subject.paragraph.clone();
        let wraps = p.wrap != WrapMode::NoWrap;
        // The two inside the gate dim with it and take no click: under `NoWrap`
        // their reset is the card header's.
        let wrap_mark = self.para_mark(
            subject.id,
            |p| p.wrap,
            |p, v| p.wrap = v,
            |v| v.label().to_owned(),
        );
        let break_mark = self.para_mark(
            subject.id,
            |p| p.word_break,
            |p, v| p.word_break = v,
            |v| v.label().to_owned(),
        );
        let long_mark = self.para_mark(
            subject.id,
            |p| p.overflow_wrap,
            |p, v| p.overflow_wrap = v,
            |v| v.label().to_owned(),
        );
        let mut inner_hit = None;
        let (_, hit) = section_marked(ui, "Wrap", wrap_mark.as_ref(), |ui| {
            let current = WrapMode::ALL.iter().position(|w| *w == p.wrap).unwrap_or(0);
            if let Some(i) = segmented(ui, MENU_INNER, SEG_H, 2, current, |painter, i, rect, on| {
                segment_label(painter, rect, WrapMode::ALL[i].label(), on)
            }) && WrapMode::ALL[i] != p.wrap
            {
                self.commit_paragraph(
                    subject.id,
                    ParagraphStyle {
                        wrap: WrapMode::ALL[i],
                        ..p.clone()
                    },
                );
            }

            let gated = ui.scope(|ui| {
                if !wraps {
                    ui.disable();
                }
                // The two sections share this scope only for the dimming rule.
                ui.spacing_mut().item_spacing.y = SECTION_GAP;
                let (_, hit) = section_marked(ui, "Word break", break_mark.as_ref(), |ui| {
                    let current = WordBreak::ALL
                        .iter()
                        .position(|w| *w == p.word_break)
                        .unwrap_or(0);
                    if let Some(i) =
                        segmented(ui, MENU_INNER, SEG_H, 3, current, |painter, i, rect, on| {
                            segment_label(painter, rect, WordBreak::ALL[i].label(), on)
                        })
                        && WordBreak::ALL[i] != p.word_break
                    {
                        self.commit_paragraph(
                            subject.id,
                            ParagraphStyle {
                                word_break: WordBreak::ALL[i],
                                ..p.clone()
                            },
                        );
                    }
                });
                if hit {
                    inner_hit = Some(&break_mark);
                }
                let (_, hit) = section_marked(ui, "Long words", long_mark.as_ref(), |ui| {
                    let current = OverflowWrap::ALL
                        .iter()
                        .position(|w| *w == p.overflow_wrap)
                        .unwrap_or(0);
                    if let Some(i) =
                        segmented(ui, MENU_INNER, SEG_H, 3, current, |painter, i, rect, on| {
                            segment_label(painter, rect, OverflowWrap::ALL[i].label(), on)
                        })
                        && OverflowWrap::ALL[i] != p.overflow_wrap
                    {
                        self.commit_paragraph(
                            subject.id,
                            ParagraphStyle {
                                overflow_wrap: OverflowWrap::ALL[i],
                                ..p.clone()
                            },
                        );
                    }
                });
                if hit {
                    inner_hit = Some(&long_mark);
                }
            });
            // A claimant of its own: a scope's response never hovers.
            if !wraps {
                ui.interact(
                    gated.response.rect,
                    ui.id().with("dimmed-wrap-section"),
                    egui::Sense::empty(),
                )
                .on_hover_text("Nothing to break: the lines do not wrap");
            }
        });
        if hit {
            self.reset_marked(hit, &wrap_mark);
        } else if let Some(mark) = inner_hit {
            self.reset_marked(true, mark);
        }
    }

    /// Optical margins: place each line by its ink rather than by its advance.
    ///
    /// Not inside `wrap_section`'s gate: it is most useful on a node that never
    /// wraps, such as a single-line label.
    fn optical_margin_section(&mut self, ui: &mut egui::Ui, subject: &TypeSubject) {
        let p = subject.paragraph.clone();
        // A switch row's mark is the dot after its label and no ↺: its toggle is the reset.
        let mark = self.para_mark(
            subject.id,
            |p| p.optical_margins,
            |p, v| p.optical_margins = v,
            |v| match v {
                true => "on".to_owned(),
                false => "off".to_owned(),
            },
        );
        section(ui, "Margins", |ui| {
            let resp = ui::switch_row_marked(
                ui,
                "Optical margins",
                p.optical_margins,
                SEG_H,
                mark.is_some(),
            );
            if resp.clicked() {
                self.commit_paragraph(
                    subject.id,
                    ParagraphStyle {
                        optical_margins: !p.optical_margins,
                        ..p.clone()
                    },
                );
            }
            match &mark {
                Some(m) => resp.on_hover_text(&m.tip),
                None => resp.on_hover_text(
                    "Hang the first and last glyph of every line out to the edges, so \
                     the text lines up by its ink instead of by its spacing",
                ),
            };
        });
    }

    // --- writing ----------------------------------------------------------

    /// One `Length` character attribute as a field with a `%`/`px` suffix, with
    /// an instance's override mark.
    #[allow(clippy::too_many_arguments)]
    fn length_char_field_marked(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        size: egui::Vec2,
        kind: CharAttrKind,
        prefix: Prefix,
        bounds: Bounds,
        tooltip: &str,
        mark: Option<&OverrideMark>,
    ) {
        // Letter spacing binds; word spacing and baseline shift do not.
        let key = match kind {
            CharAttrKind::LetterSpacing => {
                self.type_key(subject, ondin_core::variable::TEXT_LETTER_SPACING)
            }
            _ => None,
        };
        if self.type_bound(key) {
            self.type_bound_ui(ui, key, size.x, mark);
            return;
        }
        let pct_range = bounds.pct.clone();
        let current = match subject.shown(kind) {
            CharAttr::LetterSpacing(l) | CharAttr::WordSpacing(l) | CharAttr::BaselineShift(l) => l,
            _ => Length::ZERO,
        };
        let mixed = subject.mixed(kind);
        let font_size = subject.font_size();
        let (mut shown, scrub, typed) = match current {
            Length::Em(m) => (
                m * 100.0,
                Scrub::whole(0.5).range(pct_range),
                expr::Unit::Pct,
            ),
            Length::Px(v) => (
                v,
                Scrub::whole(0.5).range(bounds.px(font_size)),
                expr::Unit::Px,
            ),
        };
        let (resp, unit, hit) = ui::value_field_unit_marked(
            ui,
            size,
            prefix,
            Some(Suffix {
                text: current.unit().label(),
                clickable: true,
                tooltip: "Click to switch unit",
            }),
            mark.map(OverrideMark::field),
            &mut shown,
            scrub,
            // Its own parser: two units share one field, so the unit not shown is refused.
            |d| {
                let d = d
                    .custom_formatter(ui::number(2))
                    .custom_parser(move |t| expr::eval_in(t, typed));
                mixed_text(d, mixed)
            },
        );
        if let Some(key) = key {
            self.field_doors(ui, key, &resp);
        }
        // A marked field's tip names the main's value instead.
        let resp = match mark {
            None => resp.on_hover_text(tooltip),
            Some(m) => {
                if hit {
                    self.commit_reset(m.tx.clone());
                }
                resp
            }
        };
        if unit.is_some_and(|u| u.clicked()) {
            let to = current.unit().other();
            self.apply_char_attrs_each(subject, |m| {
                let own = char_length(&m.shown(kind)).unwrap_or(Length::ZERO);
                vec![length_attr(kind, own.in_unit(to, m.font_size()))]
            });
            return;
        }
        let next = match current {
            Length::Em(_) => Length::Em(shown / 100.0),
            Length::Px(_) => Length::Px(shown),
        };
        self.char_valve(&resp, subject, length_attr(kind, next));
    }

    /// Write a decoration's thickness or offset, through the valve unless the edit
    /// was a unit click.
    ///
    /// The same fork [`Self::paragraph_length`] makes: a unit click has no drag for
    /// a valve to wait on.
    fn decoration_write(&mut self, e: &OptionalLengthEdit, subject: &TypeSubject, attr: CharAttr) {
        if e.immediate {
            self.apply_char_attrs(subject, vec![attr]);
        } else {
            self.char_valve(&e.resp, subject, attr);
        }
    }

    /// Route a character-attribute *field* through the preview→commit valve, or
    /// straight into a live editing session that owns the range (its commit is owed
    /// when it ends, §9.3).
    ///
    /// Outside a session it is an engagement-latched valve: it previews while the
    /// control is engaged and commits once on the falling edge, so typing `40`
    /// does not commit `4` on its way. No arm commits on a bare `changed()`.
    /// (§15 D316, D812)
    fn char_valve(&mut self, resp: &egui::Response, subject: &TypeSubject, attr: CharAttr) {
        if subject.partial {
            self.arm_session_scrub(resp, subject.id);
            if resp.changed() {
                self.apply_char_attrs(subject, vec![attr]);
                // A partial subject commits no `commit_edit` until the session ends.
                self.note_live_edit(resp.dragged());
            }
            return;
        }
        // Built unconditionally: `subject` reads through the preview, so an equality
        // guard would withhold the release-frame commit. The engagement is read before
        // the `gesture_cancelled` bail so a cancelled frame spends the transition.
        let engaged = resp.dragged() || resp.has_focus();
        let was_engaged = resp.ctx.data_mut(|d| {
            let id = resp.id.with("char-valve-engaged");
            let was = d.get_temp::<bool>(id).unwrap_or(false);
            d.insert_temp(id, engaged);
            was
        });
        if self.gesture_cancelled {
            return;
        }
        // The edit in flight between the last keystroke and the committing frame;
        // no test needs it, but the valve must not depend on when egui writes back.

        let pending_key = resp.id.with("char-valve-pending");
        let (tx, changes) = char_attrs_tx(subject, vec![attr.clone()]);
        let tx = if changes {
            resp.ctx
                .data_mut(|d| d.insert_temp(pending_key, Some(attr)));
            tx
        } else {
            match resp
                .ctx
                .data(|d| d.get_temp::<Option<CharAttr>>(pending_key))
                .flatten()
            {
                Some(held) => char_attrs_tx(subject, vec![held]).0,
                None => tx,
            }
        };
        // Every frame, as `edit_valve` does.
        self.note_edit(Some(resp), &tx);
        if engaged {
            self.session.set_preview(&tx);
        } else if was_engaged {
            // `Escape` abandons a typed value; egui merely surrenders focus.
            if !resp.ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.commit_edit(tx.clone());
                self.text_session_restyled_after(&tx);
            }
            self.session.clear_gesture_preview();
            resp.ctx
                .data_mut(|d| d.insert_temp::<Option<CharAttr>>(pending_key, None));
        }
        // No arm commits on a bare `changed()`: egui's clamp of an out-of-range
        // stored value marks an untouched field changed.
    }

    /// Remember what a session-scoped scrub is about to overwrite, on `drag_started`.
    ///
    /// The write goes into the editor, so this snapshot is all `cancel_gesture` can
    /// wind back to — see [`InFlight::session_scrub`].
    ///
    /// [`InFlight::session_scrub`]: crate::app::InFlight::session_scrub
    fn arm_session_scrub(&mut self, resp: &egui::Response, id: NodeId) {
        if !resp.drag_started() {
            return;
        }
        if let Some(s) = self.text.as_ref().filter(|s| s.id == id) {
            self.in_flight.session_scrub =
                Some((id, s.editor.spans().clone(), s.editor.para_spans().clone()));
        }
    }

    /// Apply one of `docs/shortcuts.md` §10's styling chords to the live session.
    ///
    /// Every chord goes through the same `apply_char_attrs` / `commit_paragraph` the
    /// panel's controls use, so a chord and a click produce identical edits.
    /// Silent with no session open: [`Action::TextStyle`] resolves only in
    /// `Mode::TextInsert`.
    ///
    /// [`Action::TextStyle`]: crate::input::Action::TextStyle
    pub(crate) fn text_chord(&mut self, chord: TextChord) {
        let Some(id) = self.text.as_ref().map(|s| s.id) else {
            return;
        };
        let Some(subject) = TypeSubject::of(self, id) else {
            return;
        };
        match chord {
            TextChord::Bold => self.toggle_face(&subject, true),
            TextChord::Italic => self.toggle_face(&subject, false),
            // Exclusive with strikethrough, as clicking the track is.
            TextChord::Underline => {
                let under = decoration_of(&subject, CharAttrKind::Underline);
                let strike = decoration_of(&subject, CharAttrKind::Strikethrough);
                let attrs = if under.is_some() {
                    vec![CharAttr::Underline(None)]
                } else {
                    vec![
                        CharAttr::Underline(Some(strike.unwrap_or_default())),
                        CharAttr::Strikethrough(None),
                    ]
                };
                self.apply_char_attrs(&subject, attrs);
            }
            TextChord::Size(step) => {
                // `CharAttr::Size` clamps itself, so held keys stop at the ends.
                let next = subject.font_size() + f64::from(step) * FONT_SIZE_STEP;
                self.apply_char_attrs(&subject, vec![CharAttr::Size(next)]);
            }
            TextChord::Tracking(step) => {
                let current = match subject.shown(CharAttrKind::LetterSpacing) {
                    CharAttr::LetterSpacing(l) => l,
                    _ => Length::ZERO,
                };
                // A held key stops where the field's [`Bounds`] stop. The cap is on
                // the control, not the model (a file may hold an out-of-range value),
                // hence [`stepped_into`] and not `clamp` (§15 D425).
                let next = match (
                    current,
                    step_length(current, step, TRACKING_STEP_EM, TRACKING_STEP_PX),
                ) {
                    (Length::Em(c), Length::Em(v)) => {
                        Length::Em(stepped_into(c, v, Bounds::TRACKING.em()))
                    }
                    (Length::Px(c), Length::Px(v)) => {
                        Length::Px(stepped_into(c, v, Bounds::TRACKING.px(subject.font_size())))
                    }
                    // `step_length` keeps the unit, so a mixed pair cannot arise.
                    (_, stepped) => stepped,
                }
                .canonical();
                self.apply_char_attrs(&subject, vec![CharAttr::LetterSpacing(next)]);
            }
            TextChord::Leading(step) => {
                // Auto seeds from what Auto is worth in px, as the field does.
                let current = match subject.shown(CharAttrKind::LineHeight) {
                    CharAttr::LineHeight(v) => v,
                    _ => None,
                };
                let from = current
                    .or_else(|| subject.line_height.map(Length::Px))
                    .unwrap_or(Length::Em(1.0));
                // `Length::canonical` does not bound, so a held key would go negative.
                let next = match (
                    from,
                    step_length(from, step, LEADING_STEP_EM, LEADING_STEP_PX),
                ) {
                    (Length::Em(c), Length::Em(v)) => {
                        Length::Em(stepped_into(c, v, Bounds::LINE_HEIGHT.em()))
                    }
                    (Length::Px(c), Length::Px(v)) => Length::Px(stepped_into(
                        c,
                        v,
                        Bounds::LINE_HEIGHT.px(subject.font_size()),
                    )),
                    (_, stepped) => stepped,
                }
                .canonical();
                self.apply_char_attrs(&subject, vec![CharAttr::LineHeight(Some(next))]);
            }
            // Node-level (see [`TextChord::Align`]); a no-op press leaves no undo step.
            TextChord::Align(align) => {
                if subject.paragraph.align == align {
                    return;
                }
                let mut next = subject.paragraph.clone();
                next.align = align;
                self.commit_paragraph(id, next);
            }
        }
    }

    /// Apply character attributes now — for clicks and unit toggles, which have no
    /// drag for a valve to wait on.
    pub(super) fn apply_char_attrs(&mut self, subject: &TypeSubject, attrs: Vec<CharAttr>) {
        if subject.partial {
            if let Some(s) = self.text.as_mut().filter(|s| s.id == subject.id) {
                for attr in attrs {
                    s.editor.style_selection(attr);
                }
            }
            self.preview_session();
            return;
        }
        // No preview is in play here, so equality is the right question: a no-op
        // click must leave no undo step.
        let (tx, changes) = char_attrs_tx(subject, attrs);
        if changes {
            self.commit_edit(tx.clone());
            // A session on a bare caret reaches here, so the editor must adopt the
            // cleared span list.
            self.text_session_restyled_after(&tx);
        }
    }

    /// [`Self::apply_char_attrs`] with each layer's own attributes — `each` is asked
    /// once per layer, for writes whose value depends on the layer (a unit click, a
    /// variant merged into each layer's own axes).
    fn apply_char_attrs_each(
        &mut self,
        subject: &TypeSubject,
        each: impl Fn(&TypeSubject) -> Vec<CharAttr>,
    ) {
        if subject.others.is_empty() {
            self.apply_char_attrs(subject, each(subject));
            return;
        }
        let mut tx = Transaction(Vec::new());
        let mut changes = false;
        for m in std::iter::once(subject).chain(&subject.others) {
            let (more, moved) = one_char_attrs_tx(m, each(m));
            tx.0.extend(more.0);
            changes |= moved;
        }
        if changes {
            self.commit_edit(tx);
        }
    }

    /// The colour the detached picker should show for a decoration: its own if it
    /// has one, otherwise the text's, which is what the decoration is drawn in.
    ///
    /// `None` when there is no decoration to colour, which is how the picker learns
    /// to close itself.
    pub(crate) fn char_slot_color(&self, id: NodeId, slot: CharSlot) -> Option<Color> {
        let subject = TypeSubject::of(self, id)?;
        match slot {
            CharSlot::Color => Some(match attr!(subject, Color, Color) {
                Some(c) => c,
                None => subject.text_color(),
            }),
            CharSlot::Decoration(side) => {
                let d = decoration_of(&subject, side.kind())?;
                Some(d.color.unwrap_or_else(|| subject.text_color()))
            }
        }
    }

    /// Write a colour the detached picker is editing back into a decoration.
    ///
    /// Routes to `apply_char_attrs` for a click and `char_valve` for a drag, for both
    /// character-scoped slots ([`crate::panels::paint::PaintSlot::char_scoped`]);
    /// while a live session owns the range the write belongs in its spans (§9.3).
    ///
    /// Returns whether it had anything to write, which is `preview_slot`'s answer.
    /// A gradient collapses to its first stop; the picker disables its gradient tabs
    /// for these slots ([`crate::panels::paint::PaintSlot::takes_gradients`]).
    pub(crate) fn write_char_slot(
        &mut self,
        id: NodeId,
        slot: CharSlot,
        brush: &Brush,
        how: CharWrite<'_>,
    ) -> bool {
        // A commit reads the committed document: the press frame has just put this
        // colour in the preview, so `apply_char_attrs` would see no change.
        let subject = match how {
            CharWrite::Commit => TypeSubject::committed(self, id),
            CharWrite::Valve(_) | CharWrite::Preview => TypeSubject::of(self, id),
        };
        let Some(subject) = subject else {
            return false;
        };
        let Some((_, color)) = super::paint::stops_of(brush).first().copied() else {
            return false;
        };
        let Some(attr) = char_color_attr(&subject, slot, Some(color)) else {
            return false;
        };
        // A live selection writes straight into the editor's spans (§9.3);
        // `char_valve`'s `changed()` gate would miss the picker's raw regions.
        if subject.partial {
            // `valve_slot` runs every frame, and writing on every call would overwrite
            // the control under the pointer.
            //
            // valve-gate-not-a-commit: the write goes into the live session, not a valve.
            if let CharWrite::Valve(resp) = how {
                if !(resp.dragged() || resp.drag_stopped() || resp.changed() || resp.lost_focus()) {
                    return false;
                }
                self.note_live_edit(resp.dragged());
            }
            self.apply_char_attrs(&subject, vec![attr]);
            return true;
        }
        match how {
            CharWrite::Commit => self.apply_char_attrs(&subject, vec![attr]),
            CharWrite::Valve(resp) => self.char_valve(resp, &subject, attr),
            // Not through `char_valve`: a press that never becomes a drag has no
            // release frame to clear the preview.
            CharWrite::Preview => {
                let (tx, _) = char_attrs_tx(&subject, vec![attr]);
                self.session.set_preview(&tx);
            }
        }
        true
    }

    /// A **node-level** paragraph write: alignment, the last line of a justified
    /// paragraph, the base direction. Nothing spannable comes through here.
    fn commit_paragraph(&mut self, id: NodeId, paragraph: ParagraphStyle) {
        if self.commit_edit(Transaction(vec![Operation::SetParagraphStyle {
            id,
            paragraph,
            spans: None,
        }])) {
            self.text_session_restyled();
        }
    }

    /// Apply one spannable paragraph attribute now, for clicks and unit chips.
    ///
    /// [`apply_char_attrs`]'s twin, with the same two targets: a live session gets it
    /// in its own `ParaSpans` (committed when it ends, §9.3); anything else sets the
    /// node's defaults and drops that attribute's overrides.
    ///
    /// [`apply_char_attrs`]: Self::apply_char_attrs
    fn apply_para_attrs(&mut self, subject: &TypeSubject, attrs: Vec<ParaAttr>) {
        if subject.para_partial {
            if let Some(s) = self.text.as_mut().filter(|s| s.id == subject.id) {
                for attr in attrs {
                    s.editor.style_paragraph(attr);
                }
            }
            self.preview_session();
            return;
        }
        let (tx, changes) = para_attrs_tx(subject, attrs);
        if changes {
            self.commit_edit(tx);
            self.text_session_restyled();
        }
    }

    /// A spannable paragraph write through the drag valve.
    ///
    /// The transaction is built unconditionally, as in `char_valve`: `subject` reads
    /// through the preview, so a change guard would withhold the release-frame commit.
    fn valve_paragraph(&mut self, resp: &egui::Response, subject: &TypeSubject, attr: ParaAttr) {
        if subject.para_partial {
            self.arm_session_scrub(resp, subject.id);
            if resp.changed() {
                self.apply_para_attrs(subject, vec![attr]);
                self.note_live_edit(resp.dragged());
            }
            return;
        }
        let (tx, _) = para_attrs_tx(subject, vec![attr]);
        self.edit_valve(resp, tx);
        self.text_session_restyled();
    }

    fn commit_block(&mut self, id: NodeId, block: BlockStyle) {
        if self.commit_edit(Transaction(vec![Operation::SetBlockStyle { id, block }])) {
            self.text_session_restyled();
        }
    }

    /// [`Self::commit_block`] for box trim, whose only visible result is the
    /// selection box itself: trimming never moves the ink, so the chrome hide would
    /// remove the only thing to see.
    fn commit_trim(&mut self, id: NodeId, block: BlockStyle) {
        if self.commit_chrome_edit(Transaction(vec![Operation::SetBlockStyle { id, block }])) {
            self.text_session_restyled();
        }
    }
}

// --- free helpers ----------------------------------------------------------

/// One labelled block of the popup: a small-caps eyebrow, then its controls.
///
/// The popup's `item_spacing.y` separates sections ([`SECTION_GAP`]);
/// [`ui::labelled`] sets the label gap ([`LABEL_GAP`]) and the control gap.
fn section<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui::labelled(ui, label, add)
}

/// A [`section`] whose fields an instance overrides: the mark sits on the section's
/// label (full brightness with a dot, ↺ under the pointer), and a click on it is the
/// reset, which the second return value reports. Unmarked, exactly [`section`].
///
/// One mark per section; the two prefixed spacing fields and the *Margins* switch
/// row carry their own.
fn section_marked<R>(
    ui: &mut egui::Ui,
    label: &str,
    mark: Option<&OverrideMark>,
    add: impl FnOnce(&mut egui::Ui) -> R,
) -> (R, bool) {
    if mark.is_none() {
        return (section(ui, label, add), false);
    }
    let mut hit = false;
    let r = ui
        .scope(|ui| {
            // `ui::labelled`'s two gaps, with the eyebrow drawn by `label_mark`.
            ui.spacing_mut().item_spacing.y = 0.0;
            hit = super::component::label_mark(
                ui,
                None,
                &label.to_uppercase(),
                ui::EYEBROW_PT,
                theme::text::FAINT,
                mark,
            );
            ui.add_space(ui::SECTION_LABEL_GAP);
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
                add(ui)
            })
            .inner
        })
        .inner;
    (r, hit)
}

/// The square button at the end of a row that puts a value back to the font's
/// own. Returns whether it was pressed.
///
/// Always drawn, dimmed when there is nothing to put back: an empty slot would read
/// as the row stopping short.
fn reset_slot(ui: &mut egui::Ui, active: bool, tooltip: &str) -> bool {
    let resp = field_button(
        ui,
        icon::ARROW_COUNTER_CLOCKWISE,
        CELL,
        14.0,
        ui::FieldButton::enabled_if(active),
    );
    if !active {
        return false;
    }
    resp.on_hover_text(tooltip.to_string()).clicked()
}

/// The transaction that sets character attributes over the *whole* node: a new
/// default per attribute, plus the spans with that attribute's overrides dropped.
///
/// Both halves, always: setting the default alone leaves an override span behind.
/// The transaction always comes back, with the equality beside it: on a drag's
/// release frame the new style equals the preview, and a valve given `None` would
/// never commit. A several-layer subject writes every layer in one transaction.
fn char_attrs_tx(subject: &TypeSubject, attrs: Vec<CharAttr>) -> (Transaction, bool) {
    let (mut tx, mut changes) = one_char_attrs_tx(subject, attrs.clone());
    for other in &subject.others {
        let (more, moved) = one_char_attrs_tx(other, attrs.clone());
        tx.0.extend(more.0);
        changes |= moved;
    }
    (tx, changes)
}

/// The writes that put face `v` on text whose axes are `coords`: its weight,
/// slope and axes, the other axes kept.
fn variant_attrs(v: &text::FontVariant, coords: &[AxisSetting]) -> Vec<CharAttr> {
    let mut merged = coords.to_vec();
    for axis in &v.coords {
        match merged.iter_mut().find(|a| a.tag == axis.tag) {
            Some(existing) => existing.value = axis.value,
            None => merged.push(*axis),
        }
    }
    vec![
        CharAttr::Weight(v.weight),
        CharAttr::Italic(v.italic),
        CharAttr::Variations(merged),
    ]
}

/// [`char_attrs_tx`] for `subject`'s own layer alone.
fn one_char_attrs_tx(subject: &TypeSubject, attrs: Vec<CharAttr>) -> (Transaction, bool) {
    let mut style = subject.style.clone();
    let mut spans = subject.spans.clone();
    let (mut kept, _) =
        ondin_core::variable::span_defaults(subject.text_vars, &style, &subject.paragraph);
    for attr in attrs {
        let kind = attr.kind();
        style.set(attr.clone());
        kept.set(attr);
        spans.clear(subject.range.clone(), kind, &kept);
    }
    let changes = style != subject.style || spans != subject.spans;
    (
        Transaction(vec![
            Operation::SetTextStyle {
                id: subject.id,
                style,
                spans: None,
            },
            Operation::SetTextSpans {
                id: subject.id,
                spans,
            },
        ]),
        changes,
    )
}

/// The whole-node write for a spannable paragraph attribute: set the **defaults**
/// and drop the overrides of that one attribute over the range.
///
/// [`char_attrs_tx`]'s twin, down to the order of the two operations: the style op
/// re-states the span list (`Document::op_set_paragraph_style`), then the spans op
/// installs the list computed here. Only the click path may act on the changed flag.
fn para_attrs_tx(subject: &TypeSubject, attrs: Vec<ParaAttr>) -> (Transaction, bool) {
    let mut paragraph = subject.paragraph.clone();
    let mut spans = subject.para_spans.clone();
    let (_, mut kept) =
        ondin_core::variable::span_defaults(subject.text_vars, &subject.style, &paragraph);
    for attr in attrs {
        let kind = attr.kind();
        paragraph.set(attr);
        kept.set(attr);
        spans.clear(subject.para_range.clone(), kind, &kept);
    }
    let changes = paragraph != subject.paragraph || spans != subject.para_spans;
    (
        Transaction(vec![
            Operation::SetParagraphStyle {
                id: subject.id,
                paragraph,
                spans: None,
            },
            Operation::SetParagraphSpans {
                id: subject.id,
                spans,
            },
        ]),
        changes,
    )
}

/// One axis's current coordinate, or the font's own default where nothing has
/// been set.
fn axis_value(axis: &FontAxis, coords: &[AxisSetting]) -> f64 {
    coords
        .iter()
        .find(|a| a.tag == axis.tag)
        .map(|a| a.value)
        .unwrap_or(axis.default)
}

/// The `opsz` coordinate in `coords`, or `None` where the optical size is Auto.
fn opsz_of(coords: &[AxisSetting]) -> Option<f64> {
    coords.iter().find(|a| a.tag == OPSZ).map(|a| a.value)
}

/// What a `variations` mark compares — `shown`, the coordinates its section draws —
/// carrying the `whole` list so the mark's `put` can rebuild the copy's list in the
/// source's order. Equal when the shown parts are.
#[derive(Clone)]
struct AxesOf<V> {
    shown: V,
    whole: Vec<AxisSetting>,
}

impl<V: PartialEq> PartialEq for AxesOf<V> {
    fn eq(&self, other: &Self) -> bool {
        self.shown == other.shown
    }
}

/// `cur`'s coordinates with the ones `owned` picks reset to `src`'s, in `src`'s
/// order; the copy's own value stays at each tag `owned` does not pick, and a tag
/// only the copy has follows.
///
/// The order matters: `reset::overrides` and `propagate`'s merge compare
/// `variations` as an ordered list.
fn axes_reset(
    cur: &[AxisSetting],
    src: &[AxisSetting],
    owned: impl Fn(Tag) -> bool,
) -> Vec<AxisSetting> {
    let mine = |tag: Tag| cur.iter().find(|a| a.tag == tag).copied();
    let mut out: Vec<AxisSetting> = src
        .iter()
        .filter_map(|a| if owned(a.tag) { Some(*a) } else { mine(a.tag) })
        .collect();
    out.extend(
        cur.iter()
            .filter(|a| !owned(a.tag) && !src.iter().any(|s| s.tag == a.tag))
            .copied(),
    );
    out
}

/// The attribute that puts `color` in `slot` — `None` meaning "inherit".
///
/// One builder for every write to a character-scoped slot, so the row, the picker
/// and the slot verbs agree. `None` for a decoration slot with no decoration on it.
fn char_color_attr(
    subject: &TypeSubject,
    slot: CharSlot,
    color: Option<Color>,
) -> Option<CharAttr> {
    match slot {
        CharSlot::Color => Some(CharAttr::Color(color)),
        CharSlot::Decoration(side) => {
            let kind = side.kind();
            let mut d = decoration_of(subject, kind)?;
            d.color = color;
            Some(decoration_attr(kind, Some(d)))
        }
    }
}

/// One decoration attribute's value over the subject's range.
fn decoration_of(subject: &TypeSubject, kind: CharAttrKind) -> Option<Decoration> {
    match subject.shown(kind) {
        CharAttr::Underline(d) | CharAttr::Strikethrough(d) => d,
        _ => None,
    }
}

/// The glyphs the three decoration cells show: none, underline, strikethrough.
const DECORATION_GLYPHS: [&str; 3] = [icon::MINUS, icon::TEXT_UNDERLINE, icon::TEXT_STRIKETHROUGH];

/// The column a list inside the popup gives up to its scrollbar, so the bar does
/// not overlap a slider that reaches the same edge.
const LIST_GUTTER: f32 = 10.0;

/// Which of the Decoration strip's three cells is lit; cell 0 is no decoration.
/// Lifted out of `decoration_section` so a test can reach it.
fn decoration_cell(under: Option<Decoration>, strike: Option<Decoration>) -> usize {
    match (under, strike) {
        (Some(_), _) => 1,
        (None, Some(_)) => 2,
        (None, None) => 0,
    }
}

/// What picking cell `i` writes. `carried` is the decoration's own styling, kept
/// when switching between underline and strikethrough.
fn decoration_write(i: usize, carried: Decoration) -> Vec<CharAttr> {
    match i {
        1 => vec![
            CharAttr::Underline(Some(carried)),
            CharAttr::Strikethrough(None),
        ],
        2 => vec![
            CharAttr::Strikethrough(Some(carried)),
            CharAttr::Underline(None),
        ],
        _ => vec![CharAttr::Underline(None), CharAttr::Strikethrough(None)],
    }
}

/// Whether the Bold toggle is lit: at `text::BOLD_FROM` and up, the line
/// `text::bold_step` turns on. A mixed selection is never lit, or the next click
/// would un-bold runs that were never bold.
fn bold_lit(weight: u16, mixed: bool) -> bool {
    weight >= text::BOLD_FROM && !mixed
}

/// Whether a variant describes the current weight, slope and axis settings.
fn matches_variant(v: &FontVariant, weight: u16, italic: bool, coords: &[AxisSetting]) -> bool {
    if v.weight != weight || v.italic != italic {
        return false;
    }
    // Every axis the variant names has to agree; axes it does not name are the
    // user's business (a named instance says nothing about `opsz`).
    v.coords.iter().all(|a| {
        coords
            .iter()
            .find(|c| c.tag == a.tag)
            .is_none_or(|c| (c.value - a.value).abs() < 0.05)
    })
}

/// Whether a several-layer subject's layers sit on different faces of `variants`,
/// each classified by [`matches_variant`] (a layer on no face differs from one on a
/// face). False on a single layer.
fn faces_disagree(variants: &[FontVariant], subject: &TypeSubject) -> bool {
    let face = |m: &TypeSubject| {
        variants.iter().position(|v| {
            matches_variant(
                v,
                attr!(m, Weight, Weight),
                attr!(m, Italic, Italic),
                &attr!(m, Variations, Variations),
            )
        })
    };
    let first = face(subject);
    subject.others.iter().any(|m| face(m) != first)
}

/// Replace a `DragValue`'s digits with a dash when the range disagrees with
/// itself.
///
/// A dash, not a blank (reads as broken) and not the first value (reads as a claim).
fn mixed_text(d: egui::DragValue<'_>, mixed: bool) -> egui::DragValue<'_> {
    if mixed {
        return d.custom_formatter(|_, _| "–".into());
    }
    d
}

/// What picking list marker `next` writes — the marker, and a gutter for it if the
/// paragraph has none.
///
/// The gutter opens only where the indent resolves to zero (hence `font_size`),
/// and turning a marker off never touches the indent.
fn marker_attrs(next: Option<ListMarker>, indent_start: Length, font_size: f64) -> Vec<ParaAttr> {
    let mut attrs = vec![ParaAttr::Marker(next)];
    if next.is_some() && indent_start.resolve(font_size) == 0.0 {
        attrs.push(ParaAttr::IndentStart(Length::Px(DEFAULT_LIST_GUTTER)));
    }
    attrs
}

/// The variation coordinates after setting `axis` to `value`.
///
/// `clear_at_default` drops the coordinate rather than writing the font's own value.
/// It is off for `opsz`, where an absent coordinate is the Auto state.
fn axis_coords_after(
    axis: &FontAxis,
    coords: &[AxisSetting],
    value: f64,
    clear_at_default: bool,
) -> Vec<AxisSetting> {
    let mut next: Vec<AxisSetting> = coords
        .iter()
        .copied()
        .filter(|a| a.tag != axis.tag)
        .collect();
    if !(clear_at_default && value == axis.default) {
        next.push(AxisSetting::new(axis.tag, value));
    }
    next
}

/// The one value to show for a set of attribute values that are all zero lengths
/// in whatever units, or `None` when they are not.
///
/// It stops at zero because zero is the only amount where the unit carries no
/// information about the ink (`Em(0.02)` and `Px(0.32)` differ at 24pt). It hands
/// back the first value that cannot be the default, so the unit chip does not flip
/// to `px` under a user who set `%`.
fn agreed_zero<A>(values: Vec<A>, length_of: impl Fn(&A) -> Option<Length>) -> Option<A> {
    if !values
        .iter()
        .all(|v| length_of(v).is_some_and(|l| l.is_zero()))
    {
        return None;
    }
    // `Length::ZERO` is `Px(0.0)`, these attributes' default, so a `Px(0.0)` may be
    // nobody's choice while an `Em(0.0)` always is.
    let pick = values
        .iter()
        .position(|v| length_of(v) != Some(Length::ZERO))
        .unwrap_or(0);
    values.into_iter().nth(pick)
}

/// A character attribute's `Length`, for [`agreed_zero`] — the three spacings.
fn char_length(a: &CharAttr) -> Option<Length> {
    match a {
        CharAttr::LetterSpacing(l) | CharAttr::WordSpacing(l) | CharAttr::BaselineShift(l) => {
            Some(*l)
        }
        _ => None,
    }
}

fn length_attr(kind: CharAttrKind, l: Length) -> CharAttr {
    match kind {
        CharAttrKind::LetterSpacing => CharAttr::LetterSpacing(l),
        CharAttrKind::WordSpacing => CharAttr::WordSpacing(l),
        CharAttrKind::BaselineShift => CharAttr::BaselineShift(l),
        other => unreachable!("{other:?} is not a length attribute"),
    }
}

fn decoration_attr(kind: CharAttrKind, d: Option<Decoration>) -> CharAttr {
    match kind {
        CharAttrKind::Underline => CharAttr::Underline(d),
        CharAttrKind::Strikethrough => CharAttr::Strikethrough(d),
        other => unreachable!("{other:?} is not a decoration"),
    }
}

/// What a [`length_field`] produced.
///
/// The response comes back too, because these fields owe a valve.
struct LengthEdit {
    /// The value to write: on the frame it changed, and again on the frame a scrub
    /// ended, which is the one the valve commits on.
    next: Option<Length>,
    /// Whether it must commit now. A unit toggle is a click with no drag for a
    /// valve to wait on.
    immediate: bool,
    resp: egui::Response,
}

/// A `Length` field with a clickable unit, for the paragraph scope.
///
/// `mixed` replaces the digits with a dash and nothing else ([`mixed_text`]): a
/// scrub starts from the value at the range's start and makes the range uniform.
#[allow(clippy::too_many_arguments)]
fn length_field(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    prefix: Prefix,
    current: Length,
    mixed: bool,
    font_size: f64,
    bounds: Bounds,
    tooltip: &str,
) -> LengthEdit {
    let (mut shown, scrub, typed) = match current {
        Length::Em(m) => (
            m * 100.0,
            Scrub::whole(0.5).range(bounds.pct.clone()),
            expr::Unit::Pct,
        ),
        Length::Px(v) => (
            v,
            Scrub::whole(0.5).range(bounds.px(font_size)),
            expr::Unit::Px,
        ),
    };
    let before = shown;
    let (resp, unit_clicked) = value_field_suffixed(
        ui,
        size,
        prefix,
        Some(Suffix {
            text: current.unit().label(),
            clickable: true,
            tooltip: "Click to switch unit",
        }),
        &mut shown,
        scrub,
        // `mixed_text` last, so its dash wins. The parser is replaced because two
        // units share the field: a `px` typed into its `%` face is a rejection.
        |d| {
            mixed_text(
                d.custom_formatter(ui::number(2))
                    .custom_parser(move |t| expr::eval_in(t, typed)),
                mixed,
            )
        },
    );
    let resp = resp.on_hover_text(tooltip.to_string());
    if unit_clicked {
        return LengthEdit {
            next: Some(current.in_unit(current.unit().other(), font_size)),
            immediate: true,
            resp,
        };
    }
    LengthEdit {
        // `edit_valve` commits on the release frame, when `value_field` has already
        // settled the number. Pinned by
        // `a_scrub_reports_its_value_on_the_release_frame_and_a_click_reports_nothing`.
        next: (shown != before || resp.drag_stopped()).then(|| match current {
            Length::Em(_) => Length::Em(shown / 100.0),
            Length::Px(_) => Length::Px(shown),
        }),
        immediate: false,
        resp,
    }
}

/// [`LengthEdit`] for a field whose absent state means "the font decides".
struct OptionalLengthEdit {
    /// The value to write. The outer `Option` is whether there is one at all (a field
    /// showing "auto" has nothing to write); the inner is the field's absent state.
    next: Option<Option<Length>>,
    /// Whether it must commit now, as [`LengthEdit::immediate`].
    immediate: bool,
    resp: egui::Response,
}

/// A `Length` field whose absent state means "the font decides".
///
/// The suffix cycles `auto → px → % → auto`. Reports as [`length_field`] does.
/// `bounds` is the caller's because the offset is signed and the thickness is not;
/// the model clamps too (`typography::canonical_decoration`).
///
/// `resolved` is what leaving "auto" seeds: a zero thickness is no line. `None` for
/// the offset field, whose sign convention differs from parley's.
#[allow(clippy::too_many_arguments)]
fn optional_length_field(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    prefix: Prefix,
    current: Option<Length>,
    font_size: f64,
    bounds: Bounds,
    resolved: Option<f64>,
    tooltip: &str,
) -> OptionalLengthEdit {
    let (mut shown, scrub, unit, typed) = match current {
        // The parse unit is arbitrary: the scrub range is zero-width and `next` is
        // withheld while `current` is `None`.
        None => (
            0.0,
            Scrub::whole(1.0).range(0.0..=0.0),
            "auto",
            expr::Unit::Px,
        ),
        Some(Length::Px(v)) => (
            v,
            Scrub::fine(0.1, 2).range(bounds.px(font_size)),
            "px",
            expr::Unit::Px,
        ),
        Some(Length::Em(m)) => (
            m * 100.0,
            Scrub::whole(0.5).range(bounds.pct.clone()),
            "%",
            expr::Unit::Pct,
        ),
    };
    let before = shown;
    let (resp, unit_clicked) = value_field_suffixed(
        ui,
        size,
        prefix,
        Some(Suffix {
            text: unit,
            clickable: true,
            tooltip: "Click to cycle: px, %, or the font's own",
        }),
        &mut shown,
        scrub,
        // Its own parser, as [`length_field`]'s.
        |d| {
            let d = d.custom_parser(move |t| expr::eval_in(t, typed));
            if current.is_none() {
                return d.custom_formatter(|_, _| "Font".into());
            }
            d.custom_formatter(ui::number(2))
        },
    );
    let resp = resp.on_hover_text(tooltip.to_string());
    if unit_clicked {
        return OptionalLengthEdit {
            next: Some(next_optional_length(current, font_size, resolved)),
            immediate: true,
            resp,
        };
    }
    OptionalLengthEdit {
        // Nothing to write while the field shows "auto"; otherwise `length_field`'s rule.
        next: (current.is_some() && (shown != before || resp.drag_stopped())).then(|| {
            Some(match current {
                Some(Length::Em(_)) => Length::Em(shown / 100.0),
                _ => Length::Px(shown),
            })
        }),
        immediate: false,
        resp,
    }
}

/// A combo over a small `Copy` enum, returning the pick when it changed.
///
/// Exposes the enum's own variants verbatim, so a variant added to the model
/// reaches the panel. Used by the line-style picker.
fn enum_combo<T: Copy + PartialEq>(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    salt: &str,
    glyph: &'static str,
    all: &[T],
    current: T,
    label: fn(T) -> &'static str,
) -> Option<T> {
    let mut pick = None;
    ui.scope(|ui| {
        // The combo paints its border inside its rect, so its height is `size.y` whole.
        ui.spacing_mut().interact_size.y = size.y;
        ui.spacing_mut().button_padding.y = 0.0;
        egui::ComboBox::from_id_salt(salt)
            .icon(ui::combo_chevron)
            .width(size.x)
            .selected_text(ui::glyph_and_text(glyph, label(current)))
            .show_ui(ui, |ui| {
                ui::menu_rows(ui);
                ui.spacing_mut().button_padding.y = 2.0;
                for v in all {
                    if ui.selectable_label(*v == current, label(*v)).clicked() {
                        pick = Some(*v);
                    }
                }
            });
    });
    pick.filter(|p| *p != current)
}

/// The next state of [`optional_length_field`]'s three-way unit cycle:
/// auto → px → % → auto.
///
/// Leaving "the font decides" seeds what the font was deciding
/// ([`next_line_height`]'s rule), since a zero seed would remove the line;
/// `resolved` is `None` for the offset field. Unlike line height's
/// `auto → % → px → auto`, the seeded face here is `Px`.
fn next_optional_length(
    current: Option<Length>,
    font_size: f64,
    resolved: Option<f64>,
) -> Option<Length> {
    match current {
        None => Some(Length::Px(resolved.unwrap_or(0.0))),
        Some(l @ Length::Px(_)) => Some(l.in_unit(LengthUnit::Em, font_size)),
        Some(Length::Em(_)) => None,
    }
}

/// The next state of the line-height field's three-way unit cycle:
/// auto → % → px → auto.
///
/// Leaving Auto seeds what Auto was worth (`resolved` is the shaped first line's
/// height), so the click changes the unit and not the type; `120%` is the fallback
/// for a node with nothing laid out.
fn next_line_height(
    current: Option<Length>,
    font_size: f64,
    resolved: Option<f64>,
) -> Option<Length> {
    match current {
        None => Some(Length::Em(match resolved {
            Some(px) if font_size > 0.0 => (px / font_size).max(0.0),
            _ => 1.2,
        })),
        Some(l @ Length::Em(_)) => Some(l.in_unit(LengthUnit::Px, font_size)),
        Some(Length::Px(_)) => None,
    }
}

/// One layer's line height `current` moved into the state `like` names (Auto, a
/// multiple or px), keeping what it draws where the state allows: the several-layer
/// card's unit click, where `like` is the first layer's [`next_line_height`]. Auto
/// reads the laid line as [`next_line_height`] does.
fn line_height_as(
    current: Option<Length>,
    font_size: f64,
    resolved: Option<f64>,
    like: Option<Length>,
) -> Option<Length> {
    let unit = like?.unit();
    let from = current.unwrap_or_else(|| match resolved {
        Some(px) => Length::Px(px.max(0.0)),
        None => Length::Em(1.2),
    });
    Some(from.in_unit(unit, font_size))
}

/// A short prefix for an axis field.
fn axis_prefix(tag: Tag) -> &'static str {
    // `Prefix::Text` wants a `'static str`: the rest share a marker rather than
    // leaking a string per frame.
    match tag.as_str() {
        "opsz" => "OS",
        "wdth" => "WD",
        "GRAD" => "GR",
        "slnt" => "SL",
        "ital" => "IT",
        "wght" => "WT",
        "XTRA" => "XT",
        "YOPQ" => "YO",
        _ => "AX",
    }
}

/// The sign convention of an axis that does not run the way a reader expects,
/// for the axis row's hover — `None` for the ones that need no warning.
///
/// Only `slnt`: `fvar` measures slant counter-clockwise, so a right-leaning
/// oblique is `slnt −10`.
fn axis_sign_hint(tag: Tag) -> Option<&'static str> {
    match tag.as_str() {
        "slnt" => Some("counter-clockwise degrees, so a right-leaning oblique is negative"),
        _ => None,
    }
}

/// Sort key for the feature list — the tags worth finding first.
///
/// The stylistic-set arm asks `typography::stylistic_set` for a number, so `ssty`
/// (a shaping mechanic) is not filed under stylistic sets.
fn feature_rank(tag: Tag) -> u8 {
    match tag.as_str() {
        "tnum" | "pnum" | "lnum" | "onum" => 0,
        "liga" | "dlig" | "clig" | "calt" => 1,
        "frac" | "ordn" | "zero" | "sups" | "subs" => 2,
        "smcp" | "c2sc" | "case" => 3,
        _ if stylistic_set(tag).is_some() => 4,
        _ if character_variant(tag).is_some() => 5,
        _ if registered(tag).is_some_and(|r| r.offered) => 6,
        _ => 7,
    }
}

/// One tag from the OpenType feature registry: its name, one line saying what it
/// does, and whether the registry's *UI suggestion* says a user should choose it.
struct Registered {
    name: &'static str,
    /// For the row's hover. `""` for every mechanic.
    what: &'static str,
    offered: bool,
}

/// The registry entry for `tag`, or `None` for a tag the registry does not have.
///
/// `None` means a private tag: four letters the font made up.
fn registered(tag: Tag) -> Option<Registered> {
    REGISTRY
        .iter()
        .find(|(t, ..)| *t == tag.as_str())
        .map(|&(_, name, what, offered)| Registered {
            name,
            what,
            offered,
        })
}

/// `(tag, name, what it does, offered to the user)`.
///
/// Every individually registered tag (OpenType 1.9.1) is here; one missing would be
/// reported to the user as private. `ss01`–`ss20` and `cv01`–`cv99` are font-defined
/// ([`ondin_core::text::FaceFeature`]) and resolved by [`feature_label`] first.
///
/// Names are the registry's *Friendly name* in sentence case. `offered` follows the
/// registry's UI suggestion: withheld are the required features (`ccmp`, `rvrn`),
/// those a shaper applies per script (`init`, `medi`, `fina`, `isol`, `mark`,
/// `mkmk`, `curs`, `dist`, `locl`, `rlig`, `rclt`) and corollaries (`numr`, `dnom`
/// under `frac`). Four departures, each because the switch would be a lie:
///
/// - The vertical-writing features (`vert`, `vrt2`, `vrtr`, `valt`, `vhal`, `vpal`,
///   `vchw`, `vkrn`, `vapk`, `vkna`): there is no vertical writing mode.
/// - `aalt` feeds a glyph palette and means nothing as a run-wide switch.
/// - `ital` is applied from the italic style, which the panel already controls.
/// - `size` carries a tracking curve; optical size lives on the `opsz` axis row.
const REGISTRY: &[(&str, &str, &str, bool)] = &[
    // ---- Offered: the registry says a user may choose these ----------------
    // Figures and numbers.
    (
        "afrc",
        "Alternative fractions",
        "Fractions stacked over a horizontal bar rather than a diagonal one",
        true,
    ),
    ("dnom", "Denominators", "", false),
    (
        "frac",
        "Fractions",
        "Sets digits either side of a slash as one fraction glyph",
        true,
    ),
    (
        "lnum",
        "Lining figures",
        "Digits all at cap height, so they sit with capitals",
        true,
    ),
    ("numr", "Numerators", "", false),
    (
        "onum",
        "Oldstyle figures",
        "Digits with ascenders and descenders, for running text",
        true,
    ),
    (
        "ordn",
        "Ordinals",
        "Raised letters after a figure — 1st, 2.º",
        true,
    ),
    (
        "pnum",
        "Proportional figures",
        "Digits with their own widths, for running text",
        true,
    ),
    (
        "sinf",
        "Scientific inferiors",
        "Small lowered digits, for chemical formulae",
        true,
    ),
    (
        "subs",
        "Subscript",
        "Small lowered figures and letters",
        true,
    ),
    (
        "sups",
        "Superscript",
        "Small raised figures and letters",
        true,
    ),
    (
        "tnum",
        "Tabular figures",
        "Every digit the same width, so columns of numbers line up",
        true,
    ),
    (
        "zero",
        "Slashed zero",
        "A slash through the zero, so it cannot read as a capital O",
        true,
    ),
    // Ligatures and letterform alternates.
    (
        "calt",
        "Contextual alternates",
        "Forms that fit their neighbours better",
        true,
    ),
    (
        "clig",
        "Contextual ligatures",
        "Ligatures the design applies only in certain combinations",
        true,
    ),
    (
        "cswh",
        "Contextual swash",
        "Swash forms where the neighbouring letters leave room",
        true,
    ),
    (
        "dlig",
        "Discretionary ligatures",
        "Ligatures offered for effect rather than applied by default",
        true,
    ),
    (
        "hist",
        "Historical forms",
        "Period letterforms, such as the long s",
        true,
    ),
    (
        "hlig",
        "Historical ligatures",
        "Ligatures that read as archaic today",
        true,
    ),
    (
        "liga",
        "Standard ligatures",
        "The ligatures the design expects in ordinary text",
        true,
    ),
    (
        "rand",
        "Randomize",
        "Varies the glyph for repeated characters, for a written look",
        true,
    ),
    (
        "salt",
        "Stylistic alternates",
        "The designer's alternates that no other feature covers",
        true,
    ),
    (
        "swsh",
        "Swashes",
        "Flourished forms, usually for a first or last letter",
        true,
    ),
    (
        "titl",
        "Titling",
        "Lighter, wider capitals drawn for large sizes",
        true,
    ),
    // Case.
    (
        "c2pc",
        "Capitals to petite capitals",
        "Capitals as petite capitals, which sit at x-height",
        true,
    ),
    (
        "c2sc",
        "Capitals to small capitals",
        "Capitals as small capitals",
        true,
    ),
    (
        "case",
        "Case-sensitive forms",
        "Raises punctuation and lowers accents to suit all capitals",
        true,
    ),
    (
        "cpsp",
        "Capital spacing",
        "A little tracking added to runs of capitals",
        true,
    ),
    (
        "pcap",
        "Petite capitals",
        "Lowercase as capitals at x-height, smaller than small capitals",
        true,
    ),
    (
        "smcp",
        "Small capitals",
        "Lowercase as small capitals",
        true,
    ),
    (
        "unic",
        "Unicase",
        "One case throughout, mixing lowercase and small-capital shapes",
        true,
    ),
    // Symbols and notation.
    (
        "mgrk",
        "Mathematical Greek",
        "Greek letters as mathematical notation draws them",
        true,
    ),
    (
        "nalt",
        "Alternate annotation forms",
        "Circled, boxed or parenthesised forms",
        true,
    ),
    (
        "ornm",
        "Ornaments",
        "Fleurons, dingbats and border pieces in place of the bullet",
        true,
    ),
    (
        "ruby",
        "Ruby notation forms",
        "Glyphs drawn small enough to set as ruby annotation",
        true,
    ),
    // Line and paragraph fitting; nothing in this app applies them for you.
    (
        "falt",
        "Final glyph on line alternates",
        "Alternate forms for a line's last glyph, to help justification",
        true,
    ),
    (
        "jalt",
        "Justification alternates",
        "Wider or narrower forms a justification pass can use",
        true,
    ),
    (
        "lfbd",
        "Left bounds",
        "Aligns a line's left edge by the letters' apparent edge",
        true,
    ),
    (
        "opbd",
        "Optical bounds",
        "Aligns both edges by the letters' apparent edges. Deprecated",
        true,
    ),
    (
        "rtbd",
        "Right bounds",
        "Aligns a line's right edge by the letters' apparent edge",
        true,
    ),
    // CJK glyph widths and spacing, and the kerning that goes with them.
    (
        "apkn",
        "Kerning for alternate proportional widths",
        "Kerns the glyphs Proportional alternate widths has made proportional",
        true,
    ),
    (
        "chws",
        "Contextual half-width spacing",
        "Closes the gap between adjacent CJK punctuation",
        true,
    ),
    (
        "cpct",
        "Centered CJK punctuation",
        "Punctuation centred in its em box, as older Chinese setting sets it",
        true,
    ),
    (
        "fwid",
        "Full widths",
        "Every glyph on a full em width",
        true,
    ),
    (
        "halt",
        "Alternate half widths",
        "Re-spaces full-width glyphs onto half-em widths",
        true,
    ),
    (
        "hkna",
        "Horizontal kana alternates",
        "Kana drawn for horizontal setting",
        true,
    ),
    ("hwid", "Half widths", "Glyphs on half-em (en) widths", true),
    (
        "kern",
        "Kerning",
        "Closes or opens the space in specific letter pairs",
        true,
    ),
    (
        "palt",
        "Proportional alternate widths",
        "Re-spaces full-width glyphs onto their own widths",
        true,
    ),
    (
        "pkna",
        "Proportional kana",
        "Kana on their own widths rather than full-width",
        true,
    ),
    (
        "pwid",
        "Proportional widths",
        "Full-width glyphs on their own widths",
        true,
    ),
    (
        "qwid",
        "Quarter widths",
        "Glyphs on quarter-em widths",
        true,
    ),
    ("twid", "Third widths", "Glyphs on third-em widths", true),
    // CJK character sets — which form of a character, rather than which shape.
    (
        "expt",
        "Expert forms",
        "The JIS expert set of Japanese forms",
        true,
    ),
    (
        "hngl",
        "Hangul",
        "Hanja replaced by the matching hangul. Deprecated",
        true,
    ),
    (
        "hojo",
        "Hojo kanji forms",
        "The JIS X 0212-1990 form of a kanji",
        true,
    ),
    (
        "jp04",
        "JIS2004 forms",
        "Kanji as the JIS X 0213:2004 standard draws them",
        true,
    ),
    (
        "jp78",
        "JIS78 forms",
        "Kanji as the JIS C 6226-1978 standard drew them",
        true,
    ),
    (
        "jp83",
        "JIS83 forms",
        "Kanji as the JIS X 0208-1983 standard drew them",
        true,
    ),
    (
        "jp90",
        "JIS90 forms",
        "Kanji as the JIS X 0208-1990 standard drew them",
        true,
    ),
    (
        "nlck",
        "NLC kanji forms",
        "Kanji as Japan's National Language Council redrew them",
        true,
    ),
    (
        "smpl",
        "Simplified forms",
        "The simplified Chinese form of a character",
        true,
    ),
    (
        "tnam",
        "Traditional name forms",
        "Traditional forms of the characters used in Japanese names",
        true,
    ),
    (
        "trad",
        "Traditional forms",
        "The traditional Chinese form of a character",
        true,
    ),
    // ---- Withheld: required, or applied by the shaper, or by another feature -
    // No prose: these exist so that *Show all features* is a complete list,
    // not to talk anyone into switching one.
    ("aalt", "Access all alternates", "", false),
    ("abvf", "Above-base forms", "", false),
    ("abvm", "Above-base mark positioning", "", false),
    ("abvs", "Above-base substitutions", "", false),
    ("akhn", "Akhand", "", false),
    ("blwf", "Below-base forms", "", false),
    ("blwm", "Below-base mark positioning", "", false),
    ("blws", "Below-base substitutions", "", false),
    ("ccmp", "Glyph composition / decomposition", "", false),
    ("cfar", "Conjunct form after Ro", "", false),
    ("cjct", "Conjunct forms", "", false),
    ("curs", "Cursive positioning", "", false),
    ("dist", "Distances", "", false),
    ("dtls", "Dotless forms", "", false),
    ("fin2", "Terminal forms #2", "", false),
    ("fin3", "Terminal forms #3", "", false),
    ("fina", "Terminal forms", "", false),
    ("flac", "Flattened accent forms", "", false),
    ("half", "Half forms", "", false),
    ("haln", "Halant forms", "", false),
    ("init", "Initial forms", "", false),
    ("isol", "Isolated forms", "", false),
    ("ital", "Italics", "", false),
    ("ljmo", "Leading jamo forms", "", false),
    ("locl", "Localized forms", "", false),
    ("ltra", "Left-to-right alternates", "", false),
    ("ltrm", "Left-to-right mirrored forms", "", false),
    ("mark", "Mark positioning", "", false),
    ("med2", "Medial forms #2", "", false),
    ("medi", "Medial forms", "", false),
    ("mkmk", "Mark to mark positioning", "", false),
    ("mset", "Mark positioning via substitution", "", false),
    ("nukt", "Nukta forms", "", false),
    ("pref", "Pre-base forms", "", false),
    ("pres", "Pre-base substitutions", "", false),
    ("pstf", "Post-base forms", "", false),
    ("psts", "Post-base substitutions", "", false),
    ("rclt", "Required contextual alternates", "", false),
    ("rkrf", "Rakar forms", "", false),
    ("rlig", "Required ligatures", "", false),
    ("rphf", "Reph form", "", false),
    ("rtla", "Right-to-left alternates", "", false),
    ("rtlm", "Right-to-left mirrored forms", "", false),
    ("rvrn", "Required variation alternates", "", false),
    ("size", "Optical size", "", false),
    ("ssty", "Math script-style alternates", "", false),
    ("stch", "Stretching glyph decomposition", "", false),
    ("tjmo", "Trailing jamo forms", "", false),
    ("vatu", "Vattu variants", "", false),
    ("vjmo", "Vowel jamo forms", "", false),
    // Vertical writing, which this app does not have: switching one on in
    // horizontal text only mis-sets it.
    ("valt", "Alternate vertical metrics", "", false),
    (
        "vapk",
        "Kerning for alternate proportional vertical metrics",
        "",
        false,
    ),
    ("vchw", "Vertical contextual half-width spacing", "", false),
    ("vert", "Vertical alternates", "", false),
    ("vhal", "Alternate vertical half metrics", "", false),
    ("vkna", "Vertical kana alternates", "", false),
    ("vkrn", "Vertical kerning", "", false),
    ("vpal", "Proportional alternate vertical metrics", "", false),
    ("vrt2", "Vertical alternates and rotation", "", false),
    ("vrtr", "Vertical alternates for rotation", "", false),
];

/// What the list draws for one feature the face offers.
struct FeatureLabel {
    /// The row's label.
    name: String,
    /// The row's hover: the name, the tag, and whatever else is known.
    hover: String,
    /// Whether it belongs in the list, or behind *Show all features*.
    offered: bool,
    /// The feature's named values in order: index `k` is `FeatureSetting::value =
    /// k + 1`. Empty for on/off features; more than one entry makes a dropdown.
    values: Vec<String>,
}

/// How lit the loading dot is at `t` seconds, on 0.0–1.0.
///
/// A raised cosine rather than `sin`, so the breath starts at the trough.
fn loading_dot_phase(t: f64) -> f32 {
    let turns = t / LOADING_DOT_PERIOD * std::f64::consts::TAU;
    ((1.0 - turns.cos()) * 0.5) as f32
}

/// The pulsing dot that says a family's face is still on its way.
///
/// Never fully dark: a dot that blinks out would strobe across a scrolling list.
fn paint_loading_dot(ui: &egui::Ui, centre: egui::Pos2) {
    let phase = loading_dot_phase(ui.input(|i| i.time));
    let alpha = (90.0 + 150.0 * phase) as u8;
    ui.painter().circle_filled(
        centre,
        LOADING_DOT_R,
        theme::color::ACCENT.gamma_multiply(alpha as f32 / 255.0),
    );
    // The app repaints reactively; without this the dot freezes at its last phase.
    ui.ctx().request_repaint();
}

/// The *Show all features* / *Show fewer features* reveal: its text, or `None`
/// where it would be a control that cannot change anything.
///
/// `hidden` is not a property of the face: touching a withheld feature moves it
/// out of the count, so the way back when expanded is unconditional.
fn features_reveal(show_all: bool, hidden: usize) -> Option<String> {
    match (show_all, hidden) {
        (true, _) => Some("Show fewer features".to_string()),
        (false, 0) => None,
        (false, n) => Some(format!("Show all features ({n} more)")),
    }
}

/// One row of the feature list for a feature that offers more than one named
/// value (a `cvXX` choice). Returns the value to write when the user picked a
/// different one.
///
/// A dropdown under the name, since font-supplied value names are unbounded. `Off`
/// is an item in the list: `FeatureSetting::value` is one number (`0` off, `k` the
/// k-th named value), so two controls would write one field.
fn feature_choice_row(
    ui: &mut egui::Ui,
    tag: Tag,
    label: &FeatureLabel,
    value: u16,
) -> Option<u16> {
    let mut pick = None;
    ui.scope(|ui| {
        // Every gap is allocated and `item_spacing` is zero: egui pays a widget's
        // spacing on both sides, so it cannot be tighter inside than outside.
        ui.spacing_mut().item_spacing.y = 0.0;
        // Painted, to share the switch rows' baseline and left edge.
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), FEATURE_ROW_H),
            egui::Sense::empty(),
        );
        ui.painter().text(
            egui::pos2(rect.left(), rect.center().y),
            egui::Align2::LEFT_CENTER,
            &label.name,
            egui::FontId::proportional(ui::SWITCH_ROW_LABEL_PT),
            ui::switch_row_ink(value > 0),
        );
        resp.on_hover_text(label.hover.as_str());
        ui.allocate_space(egui::vec2(0.0, CHOICE_LABEL_GAP));
        ui.spacing_mut().interact_size.y = CELL;
        ui.spacing_mut().button_padding.y = 0.0;
        egui::ComboBox::from_id_salt(("type-feature-value", tag.to_string()))
            .icon(ui::combo_chevron)
            .width(rect.width())
            .selected_text(feature_value_label(label, value))
            .show_ui(ui, |ui| {
                ui::menu_rows(ui);
                if ui.selectable_label(value == 0, FEATURE_OFF).clicked() {
                    pick = Some(0);
                }
                for (v, name) in feature_values(label) {
                    if ui.selectable_label(value == v, name).clicked() {
                        pick = Some(v);
                    }
                }
            });
        ui.allocate_space(egui::vec2(0.0, CHOICE_ROW_TAIL));
    });
    pick.filter(|p| *p != value)
}

/// The gap between an n-way row's name and the dropdown under it; smaller than
/// [`CHOICE_ROW_TAIL`] so the two lines read as one row.
const CHOICE_LABEL_GAP: f32 = 2.0;

/// The gap under an n-way row's dropdown, before the next feature.
const CHOICE_ROW_TAIL: f32 = 6.0;

/// The values a [`feature_choice_row`]'s list offers, as `(value, name)`.
///
/// The only place the `+ 1` lives: index `k` of the labels is the value `k + 1`.
fn feature_values(label: &FeatureLabel) -> impl Iterator<Item = (u16, &str)> {
    label
        .values
        .iter()
        .enumerate()
        .map(|(k, name)| (k as u16 + 1, name.as_str()))
}

/// What the closed dropdown of a [`feature_choice_row`] shows.
///
/// A value the face does not offer shows as its number, so the row never
/// misreports the file.
fn feature_value_label(label: &FeatureLabel, value: u16) -> String {
    if value == 0 {
        return FEATURE_OFF.to_string();
    }
    feature_values(label)
        .find(|(v, _)| *v == value)
        .map_or_else(|| format!("Value {value}"), |(_, name)| name.to_string())
}

/// The off state of a feature, read by both the item and the closed dropdown.
const FEATURE_OFF: &str = "Off";

/// The three-tier answer to "what is this tag", in order of authority:
///
/// 1. `ssXX` / `cvXX`: the font's own strings, with "Stylistic set 7" as fallback.
/// 2. A registered tag: [`REGISTRY`].
/// 3. Anything else: the raw tag, with no invented prose.
fn feature_label(f: &FaceFeature) -> FeatureLabel {
    let tag = f.tag;
    let font_defined = |generic: String| {
        (
            f.name.clone().unwrap_or(generic),
            f.tooltip.clone().unwrap_or_default(),
            true,
        )
    };
    let (name, what, offered) = match (stylistic_set(tag), character_variant(tag)) {
        (Some(n), _) => font_defined(format!("Stylistic set {n}")),
        (_, Some(n)) => font_defined(format!("Character variant {n}")),
        _ => match registered(tag) {
            Some(r) => (r.name.to_string(), r.what.to_string(), r.offered),
            None => (
                tag.to_string(),
                "Not a registered feature — the font's own tag".to_string(),
                false,
            ),
        },
    };
    let mut hover = format!("{name} ({tag})");
    if !what.is_empty() {
        hover.push_str(" — ");
        hover.push_str(&what);
    }
    // A declared-but-unnamed value is "Alternate 2", not a gap.
    let values = f
        .values
        .iter()
        .enumerate()
        .map(|(k, v)| v.clone().unwrap_or_else(|| format!("Alternate {}", k + 1)))
        .collect();
    FeatureLabel {
        name,
        hover,
        offered,
        values,
    }
}

/// The glyphs the four alignment cells show, in [`TextAlign::OFFERED`] order.
const ALIGN_GLYPHS: [&str; 4] = [
    icon::TEXT_ALIGN_LEFT,
    icon::TEXT_ALIGN_CENTER,
    icon::TEXT_ALIGN_RIGHT,
    icon::TEXT_ALIGN_JUSTIFY,
];

/// The glyphs the three vertical-alignment cells show, in [`VerticalAlign::ALL`] order.
const VALIGN_GLYPHS: [&str; 3] = [
    icon::ALIGN_TOP,
    icon::ALIGN_CENTER_VERTICAL,
    icon::ALIGN_BOTTOM,
];

/// Languages worth offering by name: a short list of those that visibly change shaping.
const LANGUAGES: &[(&str, &str)] = &[
    ("en", "English"),
    ("de", "German"),
    ("fr", "French"),
    ("es", "Spanish"),
    ("it", "Italian"),
    ("nl", "Dutch"),
    ("pl", "Polish"),
    ("cs", "Czech"),
    ("tr", "Turkish"),
    ("ro", "Romanian"),
    ("sr", "Serbian"),
    ("ru", "Russian"),
    ("uk", "Ukrainian"),
    ("el", "Greek"),
    ("he", "Hebrew"),
    ("ar", "Arabic"),
    ("fa", "Persian"),
    ("hi", "Hindi"),
    ("th", "Thai"),
    ("vi", "Vietnamese"),
    ("ja", "Japanese"),
    ("ko", "Korean"),
    ("zh-Hans", "Chinese (Simplified)"),
    ("zh-Hant", "Chinese (Traditional)"),
];

/// What a `Length` field accepts in each of its two units.
///
/// Only the `%` end is a number; the `px` end is derived from it
/// (`px = pct/100 × font_size`, exactly [`ondin_core::Length::in_unit`]), so a
/// unit click cannot change a legal value.
#[derive(Clone)]
struct Bounds {
    pct: std::ops::RangeInclusive<f64>,
}

impl Bounds {
    /// Letter and word spacing: −50%…200%, the range CSS tracking is useful over.
    const TRACKING: Bounds = Bounds {
        pct: MIN_TRACKING_PCT..=MAX_TRACKING_PCT,
    };
    /// Baseline shift, which runs both ways from zero.
    const SIGNED_TRACKING: Bounds = Bounds {
        pct: -MAX_TRACKING_PCT..=MAX_TRACKING_PCT,
    };
    /// Space before a paragraph. Unsigned, as the field always was.
    const PARAGRAPH_SPACING: Bounds = Bounds {
        pct: 0.0..=MAX_PARAGRAPH_SPACING_PCT,
    };
    /// The three indents (first-line, start, end), each running both ways.
    const INDENT: Bounds = Bounds {
        pct: -MAX_INDENT_PCT..=MAX_INDENT_PCT,
    };
    /// A decoration's thickness. Unsigned; the model floors a negative one anyway.
    const DECORATION_THICKNESS: Bounds = Bounds {
        pct: 0.0..=MAX_DECORATION_THICKNESS_PCT,
    };
    /// A decoration's offset from the baseline, positive-up, so pushing an
    /// underline clear of the descenders is a negative number.
    const DECORATION_OFFSET: Bounds = Bounds {
        pct: -MAX_DECORATION_OFFSET_PCT..=MAX_DECORATION_OFFSET_PCT,
    };
    /// Line height, which is never negative and runs to ten lines.
    const LINE_HEIGHT: Bounds = Bounds {
        pct: MIN_LINE_HEIGHT_PCT..=MAX_LINE_HEIGHT_PCT,
    };

    /// The same range in px, at this font size.
    fn px(&self, font_size: f64) -> std::ops::RangeInclusive<f64> {
        px_range_for(self.pct.clone(), font_size)
    }

    /// The same range as an em multiple; the `Alt`+arrow chords stop at these ends.
    fn em(&self) -> std::ops::RangeInclusive<f64> {
        (self.pct.start() / 100.0)..=(self.pct.end() / 100.0)
    }
}

/// A `%` field range expressed in px at `font_size`.
///
/// The font size is floored at 1: a mixed selection can report 0
/// (`TypeSubject::font_size`), and a `0.0..=0.0` range cannot be typed into.
fn px_range_for(
    pct: std::ops::RangeInclusive<f64>,
    font_size: f64,
) -> std::ops::RangeInclusive<f64> {
    let fs = font_size.max(1.0);
    (pct.start() / 100.0 * fs)..=(pct.end() / 100.0 * fs)
}

/// How much one press of `Ctrl+Shift+.` / `,` moves the font size, in px
/// (`docs/shortcuts.md` §10).
const FONT_SIZE_STEP: f64 = 2.0;
/// One press of `Alt+←`/`→` on letter spacing, per unit: `0.01em` is the `1%` the
/// field displays; px tracking is the finer adjustment.
const TRACKING_STEP_EM: f64 = 0.01;
const TRACKING_STEP_PX: f64 = 0.1;
/// One press of `Alt+↑`/`↓` on line height, per unit.
const LEADING_STEP_EM: f64 = 0.1;
const LEADING_STEP_PX: f64 = 1.0;

/// Step a [`Length`] in whatever unit it already carries; a chord must not
/// silently convert the value.
fn step_length(current: Length, step: i8, em: f64, px: f64) -> Length {
    let d = f64::from(step);
    match current {
        Length::Em(v) => Length::Em(v + d * em),
        Length::Px(v) => Length::Px(v + d * px),
    }
    .canonical()
}

/// A chord's stepped value, bounded by `range` widened to admit `from`.
///
/// A bare `clamp` would let the increase key decrease a stored out-of-range value
/// (500% tracking snapping to the 200% cap). Widened, it moves freely toward the
/// range from outside and is pinned at `from` going away.
fn stepped_into(from: f64, next: f64, range: std::ops::RangeInclusive<f64>) -> f64 {
    next.clamp(range.start().min(from), range.end().max(from))
}

/// Field bounds, all `%`: caps on what can be typed or scrubbed, not on the model.
/// One constant per quantity, even where several hold the same number, so changing
/// one field's cap moves only that field.
const MIN_LINE_HEIGHT_PCT: f64 = 0.0;
const MAX_LINE_HEIGHT_PCT: f64 = 1000.0;
const MIN_TRACKING_PCT: f64 = -50.0;
const MAX_TRACKING_PCT: f64 = 200.0;
const MAX_PARAGRAPH_SPACING_PCT: f64 = 200.0;
const MAX_INDENT_PCT: f64 = 200.0;
const MAX_DECORATION_THICKNESS_PCT: f64 = 200.0;
const MAX_DECORATION_OFFSET_PCT: f64 = 200.0;

#[cfg(test)]
mod tests {
    use super::*;

    /// Decisions lifted out of `&mut self` bodies so tests can reach them.
    mod lifted_decisions {
        use super::*;

        /// The decoration strip's read and write halves round-trip each cell.
        ///
        /// flip: swapping `(Some(_), _) => 1` and `(None, Some(_)) => 2` → fails at the underline round trip
        #[test]
        fn the_decoration_strip_reads_and_writes_the_same_three_cells() {
            let d = Decoration::default();
            assert_eq!(decoration_cell(None, None), 0, "neither is cell 0");
            assert_eq!(decoration_cell(Some(d), None), 1, "underline is cell 1");
            assert_eq!(decoration_cell(None, Some(d)), 2, "strikethrough is cell 2");
            // Both at once is reachable through a paste; the strip has no fourth cell.
            assert_eq!(decoration_cell(Some(d), Some(d)), 1);

            for cell in [0usize, 1, 2] {
                let written = decoration_write(cell, d);
                let under = written.iter().find_map(|a| match a {
                    CharAttr::Underline(u) => Some(*u),
                    _ => None,
                });
                let strike = written.iter().find_map(|a| match a {
                    CharAttr::Strikethrough(s) => Some(*s),
                    _ => None,
                });
                // Flatten: `find_map` yields `Option<Option<_>>`, whose outer `is_some()`
                // only says the attribute was written.
                let (under, strike) = (under.flatten(), strike.flatten());
                assert_eq!(
                    under.is_some() || strike.is_some(),
                    cell != 0,
                    "cell {cell} wrote the wrong presence: {written:?}"
                );
                assert_eq!(
                    decoration_cell(under, strike),
                    cell,
                    "picking cell {cell} must light cell {cell} again: {written:?}"
                );
                // Both attributes are always written, so switching sides clears
                // the other rather than leaving two decorations on one run.
                assert_eq!(written.len(), 2, "cell {cell}: {written:?}");
            }

            // The styling is carried across a switch rather than re-made.
            let styled = Decoration {
                style: LineStyle::Dashed,
                ..Default::default()
            };
            assert!(
                matches!(
                    decoration_write(2, styled).first(),
                    Some(CharAttr::Strikethrough(Some(s))) if s.style == LineStyle::Dashed
                ),
                "the decoration's own styling must survive moving sides"
            );
        }

        /// The bold toggle lights from semibold up and is never lit when mixed.
        ///
        /// flip: `>= BOLD_FROM` → `>= 100` in `bold_lit` → fails at the 400 case
        #[test]
        fn the_bold_toggle_lights_at_semibold() {
            assert!(!bold_lit(400, false), "regular offers bold");
            assert!(!bold_lit(599, false), "one under the line");
            assert!(bold_lit(600, false), "semibold reads bold");
            assert!(bold_lit(700, false), "and so does bold");
            assert!(
                !bold_lit(900, true),
                "a mixed selection is never lit, however heavy its runs"
            );
        }

        /// A variant matches when weight and slope are equal and every axis it names agrees within tolerance; axes it does not name are free.
        ///
        /// flip: widening the tolerance in `matches_variant` → fails at the `wght` 400-vs-700 case
        #[test]
        fn a_variant_matches_only_when_every_axis_it_names_agrees() {
            let wght = Tag::parse("wght").unwrap();
            let opsz = Tag::parse("opsz").unwrap();
            let v = |w: u16, italic: bool, coords: Vec<AxisSetting>| FontVariant {
                name: "x".into(),
                weight: w,
                italic,
                coords,
            };
            let at = |tag: Tag, value: f64| AxisSetting { tag, value };

            let bold = v(700, false, vec![at(wght, 700.0)]);
            assert!(matches_variant(&bold, 700, false, &[at(wght, 700.0)]));
            assert!(
                !matches_variant(&bold, 400, false, &[at(wght, 700.0)]),
                "the weight is checked before the axes"
            );
            assert!(
                !matches_variant(&bold, 700, true, &[at(wght, 700.0)]),
                "and so is the slope"
            );
            assert!(
                !matches_variant(&bold, 700, false, &[at(wght, 400.0)]),
                "an axis the variant names has to agree"
            );
            assert!(
                matches_variant(&bold, 700, false, &[at(wght, 700.04)]),
                "within the tolerance, which exists because a slider is a float"
            );
            assert!(
                !matches_variant(&bold, 700, false, &[at(wght, 700.06)]),
                "and outside it, which is what says the tolerance is not a wildcard"
            );
            assert!(
                matches_variant(&bold, 700, false, &[at(wght, 700.0), at(opsz, 14.0)]),
                "an axis the variant does not name is the user's business"
            );
            // A static face names no axes, so only weight and slope can disqualify it.
            let static_bold = v(700, false, Vec::new());
            assert!(matches_variant(
                &static_bold,
                700,
                false,
                &[at(wght, 123.0), at(opsz, 99.0)]
            ));
        }

        /// A marker opens a gutter only over a zero indent (`Em(0.0)` counts as `Px(0.0)`), and turning it off leaves the indent alone.
        ///
        /// flip: dropping `next.is_some() &&` in `marker_attrs` → fails at the marker-off assertion
        #[test]
        fn a_marker_opens_a_gutter_only_when_there_is_none() {
            let dot = Some(ListMarker::Disc);
            let em = 16.0;

            assert_eq!(
                marker_attrs(dot, Length::ZERO, em),
                vec![
                    ParaAttr::Marker(dot),
                    ParaAttr::IndentStart(Length::Px(DEFAULT_LIST_GUTTER))
                ],
                "a marker over no indent opens a gutter for itself"
            );
            assert_eq!(
                marker_attrs(dot, Length::Px(40.0), em),
                vec![ParaAttr::Marker(dot)],
                "and leaves an indent the paragraph already has"
            );
            assert_eq!(
                marker_attrs(None, Length::ZERO, em),
                vec![ParaAttr::Marker(None)],
                "turning the marker off writes no gutter — the rule is one-way"
            );
            assert_eq!(
                marker_attrs(dot, Length::Em(0.0), em),
                vec![
                    ParaAttr::Marker(dot),
                    ParaAttr::IndentStart(Length::Px(DEFAULT_LIST_GUTTER))
                ],
                "and a zero in the other unit is still no gutter — the test is on \
                 the resolved value, not on the variant"
            );
        }

        /// A default axis value is dropped except on `opsz`, where absence means Auto; setting a present axis replaces it.
        ///
        /// flip: dropping `clear_at_default &&` in `axis_coords_after` → fails at the `opsz` assertion
        #[test]
        fn a_default_axis_value_is_dropped_except_on_opsz() {
            let wght = Tag::parse("wght").unwrap();
            let opsz = Tag::parse("opsz").unwrap();
            let at = |tag: Tag, value: f64| AxisSetting { tag, value };
            let axis = |tag: Tag, default: f64| FontAxis {
                tag,
                min: 0.0,
                default,
                max: 1000.0,
                name: tag.to_string(),
                hidden: false,
            };

            let w = axis(wght, 400.0);
            assert_eq!(
                axis_coords_after(&w, &[], 400.0, true),
                Vec::new(),
                "a value equal to the font's own default is dropped, not written — \
                 that is what reset means"
            );
            assert_eq!(
                axis_coords_after(&w, &[], 700.0, true),
                vec![at(wght, 700.0)],
                "and any other value is written"
            );

            let o = axis(opsz, 14.0);
            assert_eq!(
                axis_coords_after(&o, &[], 14.0, false),
                vec![at(opsz, 14.0)],
                "for optical size the absence of a coordinate is the Auto state, \
                 so a default value is written out like any other — dropping it \
                 would change the mode rather than reset the value"
            );

            let before = [at(wght, 100.0), at(opsz, 14.0)];
            let after = axis_coords_after(&w, &before, 700.0, true);
            assert_eq!(
                after,
                vec![at(opsz, 14.0), at(wght, 700.0)],
                "the axis being set is replaced and its neighbour is untouched"
            );
            assert_eq!(
                axis_coords_after(&w, &before, 400.0, true),
                vec![at(opsz, 14.0)],
                "and clearing it removes the coordinate while leaving the rest"
            );
        }
    }

    fn away() -> ClickAway {
        ClickAway {
            clicked: true,
            over_overlay: false,
            on_menu: false,
            on_head: false,
            on_picker: false,
            in_gesture: false,
            press_away: true,
        }
    }

    /// The reveal label shows an arrow, not an I-beam, under `selectable(false)`.
    #[test]
    fn the_features_reveal_asks_for_an_arrow_and_a_bare_label_asks_for_an_i_beam() {
        let hovered = |selectable: bool, hover_cursor: bool| {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let _ = ctx.run_ui(Default::default(), |_| {});
            let mut icon = egui::CursorIcon::Default;
            let mut at = egui::Pos2::ZERO;
            for _ in 0..5 {
                let input = egui::RawInput {
                    events: vec![egui::Event::PointerMoved(at)],
                    ..Default::default()
                };
                let out = ctx.run_ui(input, |ui| {
                    let resp = ui.add(
                        egui::Label::new(
                            egui::RichText::new("Show all features (7 more)")
                                .size(ui::SEGMENT_LABEL_PT),
                        )
                        .sense(egui::Sense::click())
                        .selectable(selectable),
                    );
                    at = resp.rect.center();
                    if hover_cursor {
                        resp.on_hover_cursor(egui::CursorIcon::PointingHand);
                    }
                });
                icon = out.platform_output.cursor_icon;
            }
            icon
        };
        assert_eq!(
            hovered(true, false),
            egui::CursorIcon::Text,
            "the reported symptom: a selectable label claims the I-beam"
        );
        assert_eq!(
            hovered(false, false),
            egui::CursorIcon::Default,
            "and `selectable(false)` alone is the arrow the row now ships — this \
             is the production configuration"
        );
        for selectable in [true, false] {
            assert_eq!(
                hovered(selectable, true),
                egui::CursorIcon::PointingHand,
                "a hover cursor outranks the label's own (selectable={selectable}) \
                 — which is why removing it, rather than adding `selectable(false)` \
                 on top of it, is what changed the row"
            );
        }
    }

    fn face_feature(tag: &str) -> FaceFeature {
        FaceFeature {
            tag: Tag::parse(tag).unwrap(),
            name: None,
            tooltip: None,
            values: Vec::new(),
        }
    }

    /// The way back from *Show all* survives the last hidden feature being switched.
    #[test]
    fn the_way_back_from_show_all_survives_the_last_hidden_feature_being_switched() {
        assert_eq!(
            features_reveal(true, 0).as_deref(),
            Some("Show fewer features"),
            "the reported symptom: expanded, nothing left hidden, and no way back"
        );
        assert_eq!(
            features_reveal(true, 3).as_deref(),
            Some("Show fewer features")
        );
        assert_eq!(features_reveal(false, 0), None);
        assert_eq!(
            features_reveal(false, 7).as_deref(),
            Some("Show all features (7 more)")
        );
    }

    /// A combo with a search field above rows survives a click in the field under
    /// `CloseOnClickOutside` (egui's default closes it) and still closes on a pick.
    #[test]
    fn a_combo_survives_a_click_in_its_search_field_but_not_a_pick() {
        let still_open = |close_outside: bool, hit_row: bool| {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let _ = ctx.run_ui(Default::default(), |_| {});
            let click = |pos: egui::Pos2| {
                let button = |pressed| egui::Event::PointerButton {
                    pos,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                };
                vec![egui::Event::PointerMoved(pos), button(true), button(false)]
            };
            let mut text = String::new();
            let (mut head, mut field, mut row) =
                (egui::Pos2::ZERO, egui::Pos2::ZERO, egui::Pos2::ZERO);
            let mut next: Vec<egui::Event> = Vec::new();
            let mut drew = Vec::new();
            for step in 0..5 {
                let mut open = false;
                let input = egui::RawInput {
                    events: std::mem::take(&mut next),
                    ..Default::default()
                };
                let _ = ctx.run_ui(input, |ui| {
                    let mut combo = egui::ComboBox::from_id_salt("probe")
                        .width(200.0)
                        .selected_text("Inter");
                    if close_outside {
                        combo = combo.close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside);
                    }
                    head = combo
                        .show_ui(ui, |ui| {
                            open = true;
                            field = ui
                                .add(egui::TextEdit::singleline(&mut text).hint_text("Search…"))
                                .rect
                                .center();
                            // Inside a `ScrollArea`, as the real list is: `ui.close()` must reach the `Area`.
                            egui::ScrollArea::vertical()
                                .max_height(80.0)
                                .show(ui, |ui| {
                                    let picked = ui.selectable_label(false, "Inter");
                                    row = picked.rect.center();
                                    if picked.clicked() && close_outside {
                                        ui.close();
                                    }
                                });
                        })
                        .response
                        .rect
                        .center();
                });
                drew.push(open);
                next = match step {
                    0 => click(head),
                    2 => click(if hit_row { row } else { field }),
                    _ => Vec::new(),
                };
            }
            assert!(
                drew[2],
                "the popup never opened, so the probe proves nothing"
            );
            drew[4]
        };
        assert!(
            !still_open(false, false),
            "the reported symptom: egui's default closes the popup on a click in its own field"
        );
        assert!(
            still_open(true, false),
            "with `CloseOnClickOutside` the search field is a click inside, and the list stays"
        );
        assert!(
            !still_open(true, true),
            "picking a row must still dismiss the list — that is what `ui.close()` is for"
        );
    }

    /// Charis SIL's `cv43`: three named values, the longest of which sets the layout.
    fn capital_eng() -> FeatureLabel {
        feature_label(&FaceFeature {
            tag: Tag::parse("cv43").unwrap(),
            name: Some("Capital Eng".into()),
            tooltip: None,
            values: vec![
                Some("Lowercase no descender".into()),
                Some("Capital form".into()),
                Some("Lowercase short stem".into()),
            ],
        })
    }

    /// The list and the closed dropdown agree on value numbering; the first value is `1` because `0` is off.
    #[test]
    fn a_named_value_and_the_value_written_for_it_are_the_same_number() {
        let label = capital_eng();
        let offered: Vec<(u16, &str)> = feature_values(&label).collect();
        assert_eq!(
            offered,
            vec![
                (1, "Lowercase no descender"),
                (2, "Capital form"),
                (3, "Lowercase short stem"),
            ],
        );
        for (v, name) in offered {
            assert_eq!(
                feature_value_label(&label, v),
                name,
                "picking {name} shows something else"
            );
        }
        assert_eq!(feature_value_label(&label, 0), "Off");
    }

    /// A value the face does not offer keeps its number rather than being clamped or hidden.
    #[test]
    fn a_value_past_the_end_is_shown_as_the_number_it_is() {
        assert_eq!(feature_value_label(&capital_eng(), 7), "Value 7");
    }

    /// A declared value the font did not name still gets a row, labelled "Alternate n".
    #[test]
    fn an_unnamed_declared_value_is_offered_as_an_alternate() {
        let label = feature_label(&FaceFeature {
            tag: Tag::parse("cv12").unwrap(),
            name: Some("Ampersand".into()),
            tooltip: None,
            values: vec![Some("Straight".into()), None],
        });
        assert_eq!(
            feature_values(&label).collect::<Vec<_>>(),
            vec![(1, "Straight"), (2, "Alternate 2")],
        );
    }

    /// One named value is still a switch, not a dropdown of "Off" and one other thing.
    #[test]
    fn one_named_value_does_not_make_a_choice_row() {
        let one = feature_label(&FaceFeature {
            tag: Tag::parse("cv01").unwrap(),
            name: Some("Alternate one".into()),
            tooltip: None,
            values: vec![Some("Serifed".into())],
        });
        assert_eq!(one.values.len(), 1, "a switch row, by `values.len() > 1`");
        assert!(feature_label(&face_feature("liga")).values.is_empty());
    }

    /// A choice row is tighter inside than outside, measured off the ink and the control.
    #[test]
    fn a_choice_row_groups_its_label_with_its_control() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let width = MENU_INNER - LIST_GUTTER;
        let out = ctx.run_ui(Default::default(), |ui| {
            let at = ui.max_rect().left_top();
            ui.scope_builder(
                egui::UiBuilder::new()
                    .max_rect(egui::Rect::from_min_size(at, egui::vec2(width, 400.0))),
                |ui| {
                    ui.spacing_mut().item_spacing.y = 1.0;
                    feature_choice_row(ui, Tag::parse("cv43").unwrap(), &capital_eng(), 0);
                    ui::switch_row(ui, "Small caps", false, FEATURE_ROW_H);
                },
            );
        });
        let galley = |text: &str| {
            out.shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::epaint::Shape::Text(t) if t.galley.job.text == text => {
                        Some(t.galley.rect.translate(t.pos.to_vec2()))
                    }
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no galley for {text:?}"))
        };
        let combo = out
            .shapes
            .iter()
            .find_map(|s| match &s.shape {
                egui::epaint::Shape::Rect(r) if (r.rect.width() - width).abs() < 0.5 => {
                    Some(r.rect)
                }
                _ => None,
            })
            .expect("the dropdown paints a frame the width of the list");
        let inside = combo.top() - galley("Capital Eng").bottom();
        let below = galley("Small caps").top() - combo.bottom();
        assert!(
            below >= inside * 1.5,
            "the row's halves are {inside} apart and the next row is {below} away: \
             too close to call, which is how the first version read"
        );
    }

    /// A choice row's name sits at a switch row's left edge in the same size, and its dropdown fits the list width.
    ///
    /// flip: `rect.width() + 1` in `feature_choice_row`'s combo width → fails at the fit assertion
    #[test]
    fn a_choice_row_lines_up_with_the_switch_rows_and_fits_the_list() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let width = MENU_INNER - LIST_GUTTER;
        let mut used = 0.0;
        let out = ctx.run_ui(Default::default(), |ui| {
            let at = ui.max_rect().left_top();
            used = ui
                .scope_builder(
                    egui::UiBuilder::new()
                        .max_rect(egui::Rect::from_min_size(at, egui::vec2(width, 400.0))),
                    |ui| {
                        feature_choice_row(ui, Tag::parse("cv43").unwrap(), &capital_eng(), 0);
                        ui::switch_row(ui, "Small caps", false, FEATURE_ROW_H);
                    },
                )
                .response
                .rect
                .width();
        });
        let galley = |text: &str| {
            out.shapes
                .iter()
                .find_map(|s| match &s.shape {
                    egui::epaint::Shape::Text(t) if t.galley.job.text == text => Some(t.clone()),
                    _ => None,
                })
                .unwrap_or_else(|| panic!("no galley for {text:?}"))
        };
        let (mine, theirs) = (galley("Capital Eng"), galley("Small caps"));
        assert_eq!(mine.pos.x, theirs.pos.x, "the two labels start apart");
        assert_eq!(
            mine.galley.job.sections[0].format.font_id,
            theirs.galley.job.sections[0].format.font_id,
            "the two labels are different sizes"
        );
        assert!(
            used <= width,
            "the row wants {used} of a {width} list, so the dropdown overhangs it"
        );
    }

    /// `REGISTRY` holds exactly the individually registered OpenType tags, each once; the set difference is the failure message.
    ///
    /// flip: retyping `qwid` as `qwix` in `REGISTRY` → fails at the set-difference assertion
    #[test]
    fn the_feature_registry_is_the_whole_registry_and_says_each_tag_once() {
        /// Every tag the table must hold, sorted.
        const EXPECTED: [&str; 124] = [
            "aalt", "abvf", "abvm", "abvs", "afrc", "akhn", "apkn", "blwf", "blwm", "blws", "c2pc",
            "c2sc", "calt", "case", "ccmp", "cfar", "chws", "cjct", "clig", "cpct", "cpsp", "cswh",
            "curs", "dist", "dlig", "dnom", "dtls", "expt", "falt", "fin2", "fin3", "fina", "flac",
            "frac", "fwid", "half", "haln", "halt", "hist", "hkna", "hlig", "hngl", "hojo", "hwid",
            "init", "isol", "ital", "jalt", "jp04", "jp78", "jp83", "jp90", "kern", "lfbd", "liga",
            "ljmo", "lnum", "locl", "ltra", "ltrm", "mark", "med2", "medi", "mgrk", "mkmk", "mset",
            "nalt", "nlck", "nukt", "numr", "onum", "opbd", "ordn", "ornm", "palt", "pcap", "pkna",
            "pnum", "pref", "pres", "pstf", "psts", "pwid", "qwid", "rand", "rclt", "rkrf", "rlig",
            "rphf", "rtbd", "rtla", "rtlm", "ruby", "rvrn", "salt", "sinf", "size", "smcp", "smpl",
            "ssty", "stch", "subs", "sups", "swsh", "titl", "tjmo", "tnam", "tnum", "trad", "twid",
            "unic", "valt", "vapk", "vatu", "vchw", "vert", "vhal", "vjmo", "vkna", "vkrn", "vpal",
            "vrt2", "vrtr", "zero",
        ];
        assert_eq!(REGISTRY.len(), 124, "the registry has 124 single tags");
        let mut tags: Vec<&str> = REGISTRY.iter().map(|(t, ..)| *t).collect();
        tags.sort_unstable();
        let before = tags.len();
        tags.dedup();
        assert_eq!(before, tags.len(), "a tag is listed twice");
        let gone: Vec<&str> = EXPECTED
            .iter()
            .copied()
            .filter(|t| !tags.contains(t))
            .collect();
        let arrived: Vec<&str> = tags
            .iter()
            .copied()
            .filter(|t| !EXPECTED.contains(t))
            .collect();
        assert!(
            gone.is_empty() && arrived.is_empty(),
            "the registered set has changed — a tag that leaves this table is \
             reported to the user as the font's own private tag. \
             gone: {gone:?}, arrived: {arrived:?}"
        );
        for (tag, name, what, offered) in REGISTRY {
            assert!(
                tag.len() == 4 && tag.is_ascii(),
                "{tag} is not a four-byte tag"
            );
            assert!(!name.is_empty(), "{tag} has no name");
            assert_eq!(
                *offered,
                !what.is_empty(),
                "{tag}: offered and described must agree"
            );
            assert!(
                stylistic_set(Tag::parse(tag).unwrap()).is_none()
                    && character_variant(Tag::parse(tag).unwrap()).is_none(),
                "{tag} is font-defined and must not be in the table"
            );
        }
    }

    /// Leaving Auto seeds the multiple the face was already giving, so the unit click does not re-space the lines.
    #[test]
    fn leaving_auto_seeds_the_line_height_the_font_was_giving() {
        let next = next_line_height(None, 20.0, Some(24.2));
        assert_eq!(next, Some(Length::Em(1.21)));
        assert_eq!(next_line_height(None, 20.0, None), Some(Length::Em(1.2)));
        assert_eq!(
            next_line_height(None, 0.0, Some(24.2)),
            Some(Length::Em(1.2))
        );
        assert_eq!(
            next_line_height(Some(Length::Em(1.5)), 20.0, Some(30.0)),
            Some(Length::Px(30.0))
        );
        assert_eq!(
            next_line_height(Some(Length::Px(30.0)), 20.0, Some(30.0)),
            None
        );
    }

    /// A chord steps a `Length` in the unit it carries, by the step constants' magnitudes.
    ///
    /// flip: `Length::Em(v) => Length::Px(v + d * em)` in `step_length` → fails at the first assertion
    #[test]
    fn a_chord_steps_a_length_without_changing_its_unit() {
        assert_eq!(
            step_length(Length::Em(0.02), 1, TRACKING_STEP_EM, TRACKING_STEP_PX),
            Length::Em(0.03),
            "a tracking of 2% nudged up stays a percentage"
        );
        assert_eq!(
            step_length(Length::Px(0.4), 1, TRACKING_STEP_EM, TRACKING_STEP_PX),
            Length::Px(0.5),
            "and one of 0.4px stays absolute"
        );

        assert_eq!(
            step_length(Length::Em(0.01), -2, TRACKING_STEP_EM, TRACKING_STEP_PX),
            Length::Em(-0.01),
            "a negative tracking is a legal one and stays in its unit"
        );
        assert_eq!(
            step_length(Length::Px(0.0), -1, TRACKING_STEP_EM, TRACKING_STEP_PX),
            Length::Px(-0.1),
            "zero carries no unit of its own and must not be the door a \
             conversion comes through"
        );

        assert_eq!(
            step_length(Length::Em(1.2), 1, LEADING_STEP_EM, LEADING_STEP_PX),
            Length::Em(1.3),
            "a tenth of an em is a tenth of a line"
        );
        assert_eq!(
            step_length(Length::Px(24.0), 1, LEADING_STEP_EM, LEADING_STEP_PX),
            Length::Px(25.0),
            "and a px is a px"
        );

        for l in [Length::Em(0.02), Length::Px(0.4)] {
            assert_eq!(
                step_length(l, 0, TRACKING_STEP_EM, TRACKING_STEP_PX),
                l,
                "no press, no change, no conversion"
            );
        }
    }

    /// Leaving *Font* on a decoration thickness seeds the face's own thickness in px, not a zero-height band.
    ///
    /// flip: seeding `Px(0.0)` in `next_optional_length` → fails at the first assertion
    #[test]
    fn leaving_the_font_seeds_the_thickness_the_face_was_drawing() {
        let inter = 1.367_187_5;
        assert_eq!(
            next_optional_length(None, 20.0, Some(inter)),
            Some(Length::Px(inter)),
            "leaving Font shows what the font was giving, in px — not zero"
        );
        assert_eq!(
            next_optional_length(None, 20.0, None),
            Some(Length::Px(0.0)),
            "a caller with nothing to seed from still seeds zero, deliberately"
        );
        assert_eq!(
            next_optional_length(Some(Length::Px(10.0)), 20.0, Some(inter)),
            Some(Length::Em(0.5))
        );
        assert_eq!(
            next_optional_length(Some(Length::Em(0.5)), 20.0, Some(inter)),
            None
        );
    }

    /// The three label tiers: mechanics withheld, font-defined sets named, private tags neither.
    #[test]
    fn a_mechanic_is_withheld_a_stylistic_set_is_named_and_a_private_tag_is_neither() {
        for mechanic in [
            "aalt", "ccmp", "locl", "mark", "mkmk", "numr", "vert", "ssty",
        ] {
            let label = feature_label(&face_feature(mechanic));
            assert!(!label.offered, "{mechanic} is offered as a choice");
        }
        for offered in ["tnum", "liga", "kern", "smcp"] {
            assert!(feature_label(&face_feature(offered)).offered, "{offered}");
        }

        let named = FaceFeature {
            name: Some("Open digits".to_string()),
            ..face_feature("ss01")
        };
        assert_eq!(feature_label(&named).name, "Open digits");
        assert_eq!(feature_label(&face_feature("ss01")).name, "Stylistic set 1");
        assert_eq!(
            feature_label(&face_feature("cv14")).name,
            "Character variant 14"
        );

        let private = feature_label(&face_feature("Zork"));
        assert!(!private.offered);
        assert_eq!(private.name, "Zork");
        assert!(
            private.hover.contains("Not a registered feature"),
            "{private:?}",
            private = private.hover
        );
    }

    /// `ssty` (a shaping mechanic) is not ranked with the stylistic sets.
    #[test]
    fn the_sort_does_not_file_ssty_under_stylistic_sets() {
        let sets = feature_rank(Tag::parse("ss01").unwrap());
        assert_eq!(sets, 4);
        assert_ne!(feature_rank(Tag::parse("ssty").unwrap()), sets);
        assert_eq!(feature_rank(Tag::parse("cv13").unwrap()), 5);
    }

    /// The opening click lands outside the not-yet-existing popup, so the button's own rect is forgiven.
    #[test]
    fn the_click_that_opens_the_popup_does_not_dismiss_it() {
        assert!(!dismissed_by_click(ClickAway {
            on_head: true,
            ..away()
        }));
    }

    /// The popup's own combo lists are separate `Area`s, so a pick lands outside both rects.
    #[test]
    fn choosing_an_item_in_one_of_the_popups_own_lists_does_not_dismiss_it() {
        assert!(!dismissed_by_click(ClickAway {
            over_overlay: true,
            ..away()
        }));
    }

    #[test]
    fn a_click_inside_the_popup_does_not_dismiss_it() {
        assert!(!dismissed_by_click(ClickAway {
            on_menu: true,
            ..away()
        }));
    }

    /// The release that ends or cancels a drag (a right-click cancel, then a primary release) is not a click away.
    #[test]
    fn ending_or_cancelling_a_drag_does_not_dismiss_the_popup() {
        assert!(!dismissed_by_click(ClickAway {
            in_gesture: true,
            ..away()
        }));
    }

    /// `in_gesture` misses the frame a drag ends, so the release is caught by where the press was.
    #[test]
    fn the_release_that_ends_a_scrub_started_on_the_card_does_not_dismiss_it() {
        assert!(!dismissed_by_click(ClickAway {
            press_away: false,
            ..away()
        }));
    }

    /// The detached colour picker is a `Window` below the popup in layer order, so it needs its own exemption.
    #[test]
    fn a_click_in_the_picker_this_popup_opened_does_not_dismiss_it() {
        assert!(!dismissed_by_click(ClickAway {
            on_picker: true,
            ..away()
        }));
    }

    /// A click on the page still dismisses, so the exemptions leave it dismissable.
    #[test]
    fn a_click_on_the_page_dismisses_the_popup() {
        assert!(dismissed_by_click(away()));
        assert!(
            !dismissed_by_click(ClickAway {
                clicked: false,
                ..away()
            }),
            "a frame with no click at all is not a dismissal"
        );
    }

    /// The alignment row returns a non-empty button rect, and the popup anchors to it.
    #[test]
    fn the_alignment_row_hands_back_the_rect_the_popup_anchors_to() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut got: Option<egui::Rect> = None;
        let _ = ctx.run_ui(Default::default(), |ui| {
            ui.set_width(268.0);
            let head = crate::ui::field_button(
                ui,
                crate::theme::icon::SLIDERS_HORIZONTAL,
                26.0,
                14.0,
                crate::ui::FieldButton::Off,
            );
            got = Some(head.rect);
        });
        let rect = got.expect("the row lays a button out");
        assert!(
            rect.width() > 0.0 && rect.height() > 0.0,
            "an empty rect would forgive every click, or none: {rect:?}"
        );
        let anchor = egui::pos2(rect.right() - MENU_W, rect.bottom() + 6.0);
        assert!(anchor.y > rect.top(), "the popup hangs below the button");
        assert!(
            anchor.x < rect.left(),
            "a {MENU_W}px popup right-aligned on a 26px button reaches left of it"
        );
    }

    /// The alignment row's track and button edges sit on the field edges and the
    /// margin above it, measured off the painted rects.
    #[test]
    fn the_alignment_row_lines_up_with_the_field_columns_above_it() {
        const CARD_INNER: f32 = 284.0 - ui::CARD_MARGIN_X * 2.0;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let half = (CARD_INNER - GAP) / 2.0;
        let field_a_right = half;
        let field_b_left = half + GAP;

        let mut tracks: Vec<egui::Rect> = Vec::new();
        let mut cells: Vec<egui::Rect> = Vec::new();
        let out = ctx.run_ui(Default::default(), |ui| {
            ui.set_width(CARD_INNER);
            let horizontal_w = (CARD_INNER - GAP) / 2.0;
            let vertical_w = CARD_INNER - horizontal_w - ROW_H - GAP * 2.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                // Cell 0 selected, so `segmented` paints a rect.
                segmented(ui, horizontal_w, SEG_CELL_H, 4, 0, |_, _, _, _| {});
                segmented(ui, vertical_w, SEG_CELL_H, 3, 0, |_, _, _, _| {});
                ui::field_button(
                    ui,
                    icon::SLIDERS_HORIZONTAL,
                    ROW_H,
                    14.0,
                    ui::FieldButton::Off,
                );
            });
        });
        for c in &out.shapes {
            if let egui::Shape::Rect(r) = &c.shape {
                if (r.rect.height() - (SEG_CELL_H - 2.0)).abs() < 0.5 {
                    cells.push(r.rect);
                } else if (r.rect.height() - ROW_H).abs() < 0.5
                    // The button and a track each paint a fill and a stroke over one rect.
                    && !tracks.contains(&r.rect)
                {
                    tracks.push(r.rect);
                }
            }
        }
        assert_eq!(tracks.len(), 3, "grounds at row height: {tracks:?}");
        let (h_track, v_track, button) = (tracks[0], tracks[1], tracks[2]);

        assert!(
            (h_track.right() - field_a_right).abs() < 0.5,
            "the four-cell track ends at {} and the field column above it at \
             {field_a_right}",
            h_track.right()
        );
        assert!(
            (v_track.left() - field_b_left).abs() < 0.5,
            "the three-cell track starts at {} and the field column above it at \
             {field_b_left}",
            v_track.left()
        );
        assert!(
            (button.right() - CARD_INNER).abs() < 0.5,
            "the row ends at {} on a {CARD_INNER}pt card",
            button.right()
        );
        assert_eq!(cells.len(), 2, "one raised cell in each track: {cells:?}");
        for (i, (cell, n)) in [(cells[0], 4), (cells[1], 3)].iter().enumerate() {
            let derived = seg_cell_of(if i == 0 { h_track } else { v_track }.width(), *n);
            assert!(
                (cell.width() - derived).abs() < 0.5,
                "track {i}'s painted cell is {}pt where its width implies {derived}pt",
                cell.width()
            );
            assert!(
                (cell.width() - SEG_CELL).abs() < 2.0,
                "track {i}'s cell came out {}pt, more than two points off the \
                 {SEG_CELL}pt button",
                cell.width()
            );
        }
    }

    /// The card's height does not ratchet down as tabs are visited: pinning an
    /// explicit `max_rect` cuts the loop. Driven tall, short, tall, since an
    /// `Area`'s first frame is always right.
    #[test]
    fn a_short_tab_does_not_shorten_the_card_for_the_next_tall_one() {
        const CAP: f32 = 400.0;
        const TALL: usize = 30;
        const SHORT: usize = 3;
        const ROW: f32 = 20.0;

        fn frame(ctx: &egui::Context, rows: usize, pinned: bool) -> f32 {
            let mut got = 0.0;
            let _ = ctx.run_ui(Default::default(), |ui| {
                egui::Area::new(egui::Id::new("ratchet"))
                    .fixed_pos(egui::pos2(0.0, 0.0))
                    .show(ui.ctx(), |ui| {
                        ui.set_width(MENU_INNER);
                        let body =
                            egui::Rect::from_min_size(ui.cursor().min, egui::vec2(MENU_INNER, CAP));
                        let mut add = |ui: &mut egui::Ui| {
                            let area = egui::ScrollArea::vertical().id_salt("body");
                            let area = if pinned {
                                area.auto_shrink([true, true])
                            } else {
                                area.max_height(CAP)
                            };
                            got = area
                                .show(ui, |ui| {
                                    ui.set_width(MENU_INNER);
                                    for _ in 0..rows {
                                        ui.allocate_exact_size(
                                            egui::vec2(MENU_INNER, ROW),
                                            egui::Sense::empty(),
                                        );
                                    }
                                })
                                .inner_rect
                                .height();
                        };
                        if pinned {
                            ui.scope_builder(egui::UiBuilder::new().max_rect(body), |ui| add(ui));
                        } else {
                            add(ui);
                        }
                    });
            });
            got
        }

        for pinned in [false, true] {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            let mut tall_first = 0.0;
            for _ in 0..2 {
                tall_first = frame(&ctx, TALL, pinned);
            }
            for _ in 0..2 {
                frame(&ctx, SHORT, pinned);
            }
            let mut tall_again = 0.0;
            for _ in 0..2 {
                tall_again = frame(&ctx, TALL, pinned);
            }
            let short_h = SHORT as f32 * ROW;
            assert!(
                tall_first > short_h * 2.0,
                "the tall tab has to actually be tall for this test to mean \
                 anything (pinned = {pinned}): {tall_first}"
            );
            if pinned {
                assert!(
                    (tall_again - tall_first).abs() < 1.0,
                    "the tall tab came back at {tall_again} having first drawn at \
                     {tall_first} — the short tab in between shortened it"
                );
            } else {
                assert!(
                    tall_again < tall_first - 1.0,
                    "expected the unpinned form to ratchet ({tall_first} → \
                     {tall_again}); if it no longer does, egui changed and this \
                     test is measuring nothing"
                );
            }
        }
    }

    /// The unit brightens on hover and the colour is the one drawn: `Painter::galley`'s
    /// colour is only a fallback for `Color32::PLACEHOLDER` sections, so both are asserted.
    #[test]
    fn hovering_the_unit_changes_the_colour_it_is_actually_drawn_in() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        const W: f32 = 130.0;

        let ink = |at: Option<egui::Pos2>| -> (egui::Color32, egui::Color32) {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(W, 60.0),
                )),
                ..Default::default()
            };
            input.events = at.map(egui::Event::PointerMoved).into_iter().collect();
            let mut found = None;
            for _ in 0..2 {
                let mut v = 120.0_f64;
                let out = ctx.run_ui(input.clone(), |ui| {
                    ui.set_width(W);
                    ui::value_field_suffixed(
                        ui,
                        egui::vec2(W, ROW_H),
                        Prefix::Icon(icon::ARROWS_VERTICAL),
                        Some(ui::Suffix {
                            text: "px",
                            clickable: true,
                            tooltip: "t",
                        }),
                        &mut v,
                        Scrub::whole(0.5).range(0.0..=1000.0),
                        |d| d,
                    );
                });
                for c in &out.shapes {
                    if let egui::Shape::Text(t) = &c.shape
                        && t.galley.job.text == "px"
                    {
                        found = Some((t.fallback_color, t.galley.job.sections[0].format.color));
                    }
                }
            }
            found.expect("the unit is drawn")
        };

        let (rest_fallback, rest_baked) = ink(None);
        let (hot_fallback, hot_baked) = ink(Some(egui::pos2(W - 8.0, ROW_H / 2.0)));

        assert_eq!(
            rest_baked,
            egui::Color32::PLACEHOLDER,
            "the galley must defer its colour to paint time, or the fallback below \
             is decoration"
        );
        assert_eq!(hot_baked, egui::Color32::PLACEHOLDER);
        assert_ne!(
            rest_fallback, hot_fallback,
            "hovering the unit has to change the colour it is painted in"
        );
        assert_eq!(rest_fallback, theme::text::FAINT, "at rest");
    }

    /// The hand-painted slider reaches both ends of its range despite the knob inset, and the middle.
    #[test]
    fn the_slider_reaches_both_ends_of_its_range() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        const W: f32 = 200.0;
        let dragged_to = |x: f32| -> f64 {
            let mut value = 400.0_f64;
            let y = ui::SLIDER_H / 2.0;
            let frame = |input: egui::RawInput, value: &mut f64| {
                let _ = ctx.run_ui(input, |ui| {
                    ui.set_width(W);
                    ui::slider(ui, W, value, 100.0..=900.0);
                });
            };
            let start = egui::pos2(W / 2.0, y);
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(W, 40.0),
                )),
                ..Default::default()
            };
            input.events = vec![egui::Event::PointerMoved(start)];
            frame(input.clone(), &mut value);
            input.events = vec![egui::Event::PointerButton {
                pos: start,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            }];
            frame(input.clone(), &mut value);
            input.events = vec![egui::Event::PointerMoved(egui::pos2(x, y))];
            frame(input, &mut value);
            value
        };
        assert_eq!(dragged_to(-40.0), 100.0, "dragged past the left end");
        assert_eq!(dragged_to(W + 40.0), 900.0, "dragged past the right end");
        let mid = dragged_to(W / 2.0);
        assert!(
            (mid - 500.0).abs() < 12.0,
            "the middle of the rail should be near the middle of the range, got {mid}"
        );
    }

    /// A scrub reports its value again on the frame it ends (the frame the valve
    /// commits on), and a bare click reports nothing.
    #[test]
    fn a_scrub_reports_its_value_on_the_release_frame_and_a_click_reports_nothing() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        const W: f32 = 200.0;
        let y = CELL / 2.0;
        let frame = |events: Vec<egui::Event>, current: &mut Length| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(W, 60.0),
                )),
                events,
                ..Default::default()
            };
            let mut out = None;
            let _ = ctx.run_ui(input, |ui| {
                ui.set_width(W);
                let e = length_field(
                    ui,
                    egui::vec2(W, CELL),
                    Prefix::Text("I"),
                    *current,
                    false,
                    16.0,
                    Bounds { pct: 0.0..=1000.0 },
                    "indent",
                );
                out = Some((e.next, e.resp.dragged(), e.resp.drag_stopped()));
            });
            let (next, dragged, drag_stopped) = out.unwrap();
            if let Some(n) = next {
                *current = n;
            }
            (next, dragged, drag_stopped)
        };
        let at = |x: f32| egui::pos2(x, y);
        let button = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let commits = |(next, dragged, drag_stopped): (Option<Length>, bool, bool)| {
            next.is_some() && !dragged && drag_stopped
        };

        let start = at(W / 2.0);
        let mut current = Length::Px(10.0);
        frame(vec![egui::Event::PointerMoved(start)], &mut current);
        frame(vec![button(start, true)], &mut current);
        for i in 1..=4 {
            frame(
                vec![egui::Event::PointerMoved(at(W / 2.0 + (i * 6) as f32))],
                &mut current,
            );
        }
        assert_ne!(
            current,
            Length::Px(10.0),
            "the drag never moved the value, so there is no release to test"
        );
        let reached = current;
        let release = frame(vec![button(at(W / 2.0 + 24.0), false)], &mut current);
        assert!(
            release.2,
            "the release frame is the one the valve commits on"
        );
        assert_eq!(
            release.0,
            Some(reached),
            "the release frame reports the value the drag reached"
        );
        assert!(commits(release), "so the last drag of a scrub commits");

        let mut clicked = Length::Px(10.0);
        for (tag, events) in [
            ("hover", vec![egui::Event::PointerMoved(start)]),
            ("press", vec![button(start, true)]),
            ("release", vec![button(start, false)]),
            ("settle", vec![]),
        ] {
            let r = frame(events, &mut clicked);
            assert!(r.0.is_none(), "{tag}: a click has no value to report");
            assert!(!commits(r), "{tag}: a click must not commit a transaction");
        }
        assert_eq!(clicked, Length::Px(10.0), "a click moved the value");
    }

    /// `optional_length_field` reports on a change and on the release, not on idle frames.
    ///
    /// flip: restoring `current.is_some().then(…)` in `optional_length_field` → fails at the idle-frame assertion
    #[test]
    fn a_decoration_field_reports_on_a_change_and_on_the_release_and_not_when_idle() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        const W: f32 = 200.0;
        let y = CELL / 2.0;
        let frame = |events: Vec<egui::Event>, current: &mut Option<Length>| {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(W, 60.0),
                )),
                events,
                ..Default::default()
            };
            let mut out = None;
            let _ = ctx.run_ui(input, |ui| {
                ui.set_width(W);
                let e = optional_length_field(
                    ui,
                    egui::vec2(W, CELL),
                    Prefix::Text("T"),
                    *current,
                    16.0,
                    Bounds::DECORATION_THICKNESS,
                    Some(2.0),
                    "thickness",
                );
                out = Some((e.next, e.resp.drag_stopped()));
            });
            let (next, drag_stopped) = out.unwrap();
            if let Some(n) = next {
                *current = n;
            }
            (next, drag_stopped)
        };
        let at = |x: f32| egui::pos2(x, y);
        let button = |pos: egui::Pos2, pressed: bool| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };

        let start = at(W / 2.0);
        let mut current = Some(Length::Px(4.0));

        frame(vec![egui::Event::PointerMoved(start)], &mut current);
        let idle = frame(Vec::new(), &mut current);
        assert!(
            idle.0.is_none(),
            "an idle frame has nothing to say — this is the assertion the \
             every-frame gate fails and a scrub-only test would never reach"
        );
        assert_eq!(
            current,
            Some(Length::Px(4.0)),
            "and the idle frame left the value alone"
        );

        frame(vec![button(start, true)], &mut current);
        for i in 1..=4 {
            frame(
                vec![egui::Event::PointerMoved(at(W / 2.0 + (i * 6) as f32))],
                &mut current,
            );
        }
        assert_ne!(
            current,
            Some(Length::Px(4.0)),
            "the drag never moved the value, so there is no release to test"
        );
        let reached = current;
        let release = frame(vec![button(at(W / 2.0 + 24.0), false)], &mut current);
        assert!(
            release.1,
            "the release frame is the one the valve commits on"
        );
        assert_eq!(
            release.0,
            Some(reached),
            "and it reports the value the drag reached — dropping the \
             `drag_stopped` term is §15 D109"
        );
    }

    /// A field with a unit paints the width it was given, with the unit inside its border.
    #[test]
    fn a_field_with_a_unit_fills_its_width_and_keeps_the_unit_inside() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        const W: f32 = 268.0;
        let half = (W - GAP) / 2.0;
        let mut frames: Vec<egui::Rect> = Vec::new();
        let mut units: Vec<egui::Rect> = Vec::new();
        let out = ctx.run_ui(Default::default(), |ui| {
            ui.set_width(W);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                for (unit, prefix) in [
                    ("%", Prefix::Icon(icon::ARROWS_VERTICAL)),
                    ("auto", Prefix::Icon(icon::TEXT_AA)),
                ] {
                    let mut v = 120.0_f64;
                    ui::value_field_suffixed(
                        ui,
                        egui::vec2(half, ROW_H),
                        prefix,
                        Some(ui::Suffix {
                            text: unit,
                            clickable: true,
                            tooltip: "t",
                        }),
                        &mut v,
                        Scrub::whole(0.5).range(0.0..=1000.0),
                        |d| d,
                    );
                }
            });
        });
        for c in &out.shapes {
            let r = c.shape.visual_bounding_rect();
            match &c.shape {
                egui::Shape::Rect(_) if (r.height() - ROW_H).abs() < 0.5 => frames.push(r),
                egui::Shape::Text(t) if matches!(t.galley.job.text.as_str(), "%" | "auto") => {
                    units.push(r)
                }
                _ => {}
            }
        }
        assert_eq!(frames.len(), 2, "two field grounds: {frames:?}");
        assert_eq!(units.len(), 2, "two units drawn: {units:?}");
        for (i, (frame, unit)) in frames.iter().zip(&units).enumerate() {
            assert!(
                (frame.width() - half).abs() < 0.5,
                "field {i} was given {half}pt and painted {}pt",
                frame.width()
            );
            let inset = frame.right() - unit.right();
            assert!(
                unit.right() < frame.right(),
                "field {i}'s unit ends at {} with the field's border at {}",
                unit.right(),
                frame.right()
            );
            assert!(
                (2.0..12.0).contains(&inset),
                "field {i}'s unit is {inset}pt from the border — it should tuck \
                 against the right edge, not float in the middle"
            );
        }
        let right = frames[1].right() - frames[0].left();
        assert!(
            (right - W).abs() < 0.5,
            "the two fields spanned {right}pt of a {W}pt row"
        );
    }

    /// No ink lands right of the card's inner edge, asserted on the painted shapes.
    #[test]
    fn nothing_in_the_popup_paints_past_the_card() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let out = ctx.run_ui(Default::default(), |ui| {
            ui.set_width(MENU_INNER);

            let mut needle = String::new();
            field_row(ui, egui::vec2(MENU_INNER, CELL), |ui| {
                ui.spacing_mut().item_spacing.x = SEARCH_GAP;
                ui.label(theme::icon_text(
                    icon::MAGNIFYING_GLASS,
                    SEARCH_ICON_PT,
                    theme::text::FAINT,
                ));
                ui.add(
                    egui::TextEdit::singleline(&mut needle)
                        .frame(egui::Frame::NONE)
                        .desired_width(
                            MENU_INNER - ui::FIELD_PAD_X * 2.0 - SEARCH_ICON_PT - SEARCH_GAP * 2.0,
                        )
                        .hint_text("Search features"),
                );
            });

            let mut hex = "9184D9".to_string();
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = COL_GAP;
                field_row(ui, egui::vec2(MENU_INNER - CELL - COL_GAP, CELL), |ui| {
                    ui.spacing_mut().item_spacing.x = 7.0;
                    ui::swatch(
                        ui,
                        14.0,
                        theme::color::FIELD,
                        ui::Swatch::Solid(egui::Color32::RED),
                    );
                    ui.add(
                        egui::TextEdit::singleline(&mut hex)
                            .frame(egui::Frame::NONE)
                            .desired_width(62.0)
                            .char_limit(ui::COLOUR_CHAR_LIMIT)
                            .font(egui::FontId::proportional(11.5)),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        let mut pct = 100.0_f64;
                        ui::bare_drag_value(
                            ui,
                            egui::DragValue::new(&mut pct).suffix("%").max_decimals(0),
                        );
                    });
                });
                let _ = reset_slot(ui, true, "Back to the text's own colour");
            });

            let mut v = 480.0_f64;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = COL_GAP;
                ui.allocate_ui_with_layout(
                    egui::vec2(
                        MENU_INNER - LIST_GUTTER - AXIS_FIELD_W - CELL - COL_GAP * 2.0,
                        CELL,
                    ),
                    egui::Layout::left_to_right(egui::Align::Center),
                    |ui| {
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new("Optical size")
                                    .size(11.5)
                                    .color(theme::text::MUTED),
                            )
                            .truncate(),
                        );
                    },
                );
                value_field(
                    ui,
                    egui::vec2(AXIS_FIELD_W, CELL),
                    Prefix::Text("WT"),
                    &mut v,
                    Scrub::fine(0.5, 1).range(100.0..=900.0),
                    |d| d.custom_formatter(ui::number(1)),
                );
                let _ = reset_slot(ui, true, "Back to the font's default");
            });
        });
        let edge = MENU_INNER + 0.5;
        let over: Vec<f32> = out
            .shapes
            .iter()
            .map(|c| c.shape.visual_bounding_rect())
            .filter(|r| r.is_positive() && r.right() > edge)
            .map(|r| r.right())
            .collect();
        assert!(
            over.is_empty(),
            "ink reaching past the card's {MENU_INNER}pt inner edge, at x = {over:?}"
        );
    }

    /// A subject over the whole of an eight-byte node.
    fn subject_of(style: TextStyle, spans: CharSpans) -> TypeSubject {
        TypeSubject {
            id: NodeId::from_wire("1:1").expect("a well-formed id"),
            style,
            spans,
            paragraph: ParagraphStyle::default(),
            block: BlockStyle::default(),
            sizing: TextSizing::Auto,
            text_paint: Some(Brush::Solid(Color::BLACK)),
            line_height: None,
            decoration_sizes: None,
            range: 0..8,
            resolve_at: Some(0),
            partial: false,
            para_spans: ParaSpans::default(),
            para_range: 0..8,
            para_partial: false,
            others: Vec::new(),
            text_vars: Default::default(),
        }
    }

    /// `subject_of` over some of a node's paragraphs; `para_range` is already snapped.
    fn para_subject_of(
        paragraph: ParagraphStyle,
        para_spans: ParaSpans,
        para_range: Range<usize>,
    ) -> TypeSubject {
        TypeSubject {
            paragraph,
            para_spans,
            para_partial: para_range != (0..8),
            para_range,
            ..subject_of(TextStyle::default(), CharSpans::default())
        }
    }

    /// A whole-node paragraph write sets the defaults and drops the overrides it replaces.
    #[test]
    fn a_whole_node_paragraph_write_flattens_the_overrides_it_replaces() {
        let defaults = ParagraphStyle::default();
        let mut spans = ParaSpans::default();
        spans.set(0..4, ParaAttr::IndentStart(Length::Px(40.0)), &defaults);
        spans.set(4..8, ParaAttr::Spacing(Length::Px(12.0)), &defaults);
        let subject = para_subject_of(defaults, spans, 0..8);
        assert!(!subject.para_partial, "the precondition: no live session");

        let (tx, changes) = para_attrs_tx(&subject, vec![ParaAttr::IndentStart(Length::Px(10.0))]);
        assert!(changes);
        let (paragraph, written) = paragraph_ops(&tx);
        assert_eq!(
            paragraph.indent_start,
            Length::Px(10.0),
            "the default moved"
        );
        assert_eq!(
            written.as_slice().len(),
            1,
            "the start-indent override should be gone and the spacing one kept: {:?}",
            written.as_slice()
        );
        assert_eq!(
            written.as_slice()[0].attr,
            ParaAttr::Spacing(Length::Px(12.0))
        );
    }

    /// A write that changes nothing reports so, since the click path commits only when it does.
    #[test]
    fn a_paragraph_write_of_the_value_already_there_changes_nothing() {
        let defaults = ParagraphStyle {
            indent: Length::Px(20.0),
            ..ParagraphStyle::default()
        };
        let subject = para_subject_of(defaults, ParaSpans::default(), 0..8);
        let (_, changes) = para_attrs_tx(&subject, vec![ParaAttr::Indent(Length::Px(20.0))]);
        assert!(!changes);
        let (_, changes) =
            para_attrs_tx(&subject, vec![ParaAttr::Indent(Length::Px(20.000000001))]);
        assert!(
            !changes,
            "a drag's float noise must not commit — that is what fragments a span list"
        );
    }

    /// Over a mixed range `shown_paragraph` gives the start's value and `para_mixed` says it is not the only one.
    #[test]
    fn a_paragraph_field_reads_mixed_over_paragraphs_that_disagree() {
        let defaults = ParagraphStyle::default();
        let mut spans = ParaSpans::default();
        spans.set(4..8, ParaAttr::IndentStart(Length::Px(40.0)), &defaults);

        let both = para_subject_of(defaults.clone(), spans.clone(), 0..8);
        assert!(!both.para_mixed(ParaAttrKind::IndentStart));

        let first = para_subject_of(defaults.clone(), spans.clone(), 0..4);
        assert!(first.para_partial);
        assert!(!first.para_mixed(ParaAttrKind::IndentStart));
        assert_eq!(first.shown_paragraph().indent_start, Length::ZERO);

        let second = para_subject_of(defaults.clone(), spans.clone(), 4..8);
        assert!(!second.para_mixed(ParaAttrKind::IndentStart));
        assert_eq!(second.shown_paragraph().indent_start, Length::Px(40.0));

        let mut across = para_subject_of(defaults, spans, 0..8);
        across.para_partial = true;
        assert!(across.para_mixed(ParaAttrKind::IndentStart));
        assert!(
            !across.para_mixed(ParaAttrKind::Spacing),
            "an attribute nothing overrides is not mixed just because its neighbour is"
        );
        assert_eq!(across.shown_paragraph().indent_start, Length::ZERO);
    }

    /// A letter spacing of zero everywhere is not *Mixed* whatever the zeroes' units, and
    /// comes back in the unit the user chose so the chip does not flip.
    ///
    /// flip: deleting `shared`'s `or_else(agreed_zero(…))` arm → fails at the `mixed` assertion
    /// flip: `.last()` in place of the non-default pick → fails at the `Px(0.0)` against `Em(0.0)` assertion
    #[test]
    fn a_zero_letter_spacing_in_two_units_is_not_mixed() {
        assert_ne!(
            Length::Em(0.0),
            Length::Px(0.0),
            "the fixture rests on these being different values — if they ever \
             compare equal this test proves nothing"
        );

        for at in [0..4, 4..8] {
            let style = TextStyle::default();
            let mut spans = CharSpans::default();
            spans.set(at.clone(), CharAttr::LetterSpacing(Length::Em(0.0)), &style);

            let mut across = subject_of(style, spans);
            across.partial = true;
            across.range = 0..8;

            assert!(
                !across.mixed(CharAttrKind::LetterSpacing),
                "zero over here and zero over there is zero: the ink is \
                 identical and the field must show a number, not a dash \
                 (explicit run at {at:?})"
            );
            assert_eq!(
                across.shared(CharAttrKind::LetterSpacing),
                Some(CharAttr::LetterSpacing(Length::Em(0.0))),
                "and it comes back in the unit the user chose, because the chip \
                 reads its suffix off this — handing back the Px default would \
                 flip a percent document to px under them, which is D537's \
                 symptom by a new road (explicit run at {at:?})"
            );
        }
    }

    /// A real disagreement still reads *Mixed*: two amounts, and a zero beside a number.
    ///
    /// flip: dropping `agreed_zero`'s `is_zero` test → fails at the two-amounts assertion
    #[test]
    fn two_different_letter_spacings_still_read_mixed() {
        let style = TextStyle::default();

        let mut two_numbers = CharSpans::default();
        two_numbers.set(0..4, CharAttr::LetterSpacing(Length::Em(0.02)), &style);
        two_numbers.set(4..8, CharAttr::LetterSpacing(Length::Px(3.0)), &style);
        let mut across = subject_of(style.clone(), two_numbers);
        across.partial = true;
        assert!(
            across.mixed(CharAttrKind::LetterSpacing),
            "two different amounts disagree however the zero rule reads"
        );

        let mut zero_and_number = CharSpans::default();
        zero_and_number.set(0..4, CharAttr::LetterSpacing(Length::Em(0.0)), &style);
        zero_and_number.set(4..8, CharAttr::LetterSpacing(Length::Px(3.0)), &style);
        let mut across = subject_of(style, zero_and_number);
        across.partial = true;
        assert!(
            across.mixed(CharAttrKind::LetterSpacing),
            "a zero beside a number is the mixed case the zero rule must not \
             swallow — one of these is ink and the other is not"
        );
    }

    /// The hanging pair's click guard asks `para_ragged`, not `shown_paragraph`
    /// (the first paragraph), so an override on a later paragraph still flattens.
    #[test]
    fn the_hanging_pair_has_something_to_do_when_the_paragraphs_disagree() {
        let defaults = ParagraphStyle::default();
        let mut spans = ParaSpans::default();
        spans.set(4..8, ParaAttr::Hanging(true), &defaults);
        let subject = para_subject_of(defaults.clone(), spans, 0..8);

        assert!(
            !subject.shown_paragraph().hanging,
            "the first paragraph is what is shown, and it does not hang"
        );
        assert!(
            !subject.para_mixed(ParaAttrKind::Hanging),
            "a whole-node range never reads mixed — that is the convention `ragged` \
             exists beside, not a bug"
        );
        assert!(
            subject.para_ragged(ParaAttrKind::Hanging),
            "but there *is* something to flatten, which is what the click must fire on"
        );
        let (tx, changes) = para_attrs_tx(&subject, vec![ParaAttr::Hanging(false)]);
        assert!(
            changes,
            "the span list changed even though the default did not"
        );
        let (paragraph, written) = paragraph_ops(&tx);
        assert!(!paragraph.hanging);
        assert!(
            written.is_empty(),
            "the second paragraph's override should be gone: {:?}",
            written.as_slice()
        );

        let plain = para_subject_of(defaults, ParaSpans::default(), 0..8);
        assert!(!plain.para_ragged(ParaAttrKind::Hanging));
        let (_, changes) = para_attrs_tx(&plain, vec![ParaAttr::Hanging(false)]);
        assert!(!changes);
    }

    /// The paragraph style and span list out of a `para_attrs_tx` transaction.
    fn paragraph_ops(tx: &Transaction) -> (ParagraphStyle, ParaSpans) {
        let mut style = None;
        let mut spans = None;
        for op in &tx.0 {
            match op {
                Operation::SetParagraphStyle { paragraph, .. } => style = Some(paragraph.clone()),
                Operation::SetParagraphSpans { spans: s, .. } => spans = Some(s.clone()),
                other => panic!("unexpected op in a paragraph write: {other:?}"),
            }
        }
        (
            style.expect("a SetParagraphStyle"),
            spans.expect("a SetParagraphSpans"),
        )
    }

    /// Every character-scoped slot round-trips through `char_scoped`, and no other slot claims to be one;
    /// the three write verbs branch on it, so a miss would write an empty document transaction.
    #[test]
    fn every_character_scoped_slot_round_trips_and_no_other_claims_to_be_one() {
        use crate::panels::PaintSlot;
        for cs in [
            CharSlot::Color,
            CharSlot::Decoration(DecorationSide::Underline),
            CharSlot::Decoration(DecorationSide::Strikethrough),
        ] {
            assert_eq!(
                cs.paint_slot().char_scoped(),
                Some(cs),
                "{cs:?} must survive the round trip the picker makes"
            );
        }
        for slot in [
            PaintSlot::Fill(0),
            PaintSlot::Stroke(1),
            PaintSlot::Canvas,
            PaintSlot::Guide,
            PaintSlot::GroupColor([1, 2, 3, 4]),
        ] {
            assert!(
                slot.char_scoped().is_none(),
                "{slot:?} writes a transaction and must not be diverted"
            );
        }
    }

    /// The one colour builder covers both slots; `None` is the inherited state the reset writes.
    #[test]
    fn the_colour_builder_covers_both_slots_and_the_inherited_state() {
        let subject = subject_of(TextStyle::default(), CharSpans::default());
        let c = Color::from_rgba8(9, 8, 7, 255);
        assert_eq!(
            char_color_attr(&subject, CharSlot::Color, Some(c)),
            Some(CharAttr::Color(Some(c)))
        );
        assert_eq!(
            char_color_attr(&subject, CharSlot::Color, None),
            Some(CharAttr::Color(None)),
            "the reset writes the inherited state, not a missing attribute"
        );
        assert!(
            char_color_attr(
                &subject,
                CharSlot::Decoration(DecorationSide::Underline),
                Some(c)
            )
            .is_none()
        );
    }

    fn red() -> Decoration {
        Decoration {
            color: Some(Color::from_rgba8(255, 0, 0, 255)),
            ..Decoration::default()
        }
    }

    /// A gradient-filled text node shows its decoration inheriting the ramp, not black;
    /// the hex and picker collapse to the first stop, as `write_char_slot` does.
    #[test]
    fn a_gradient_text_fill_is_inherited_as_a_ramp_not_as_black() {
        use ondin_core::peniko::Gradient;
        let (first, last) = (
            Color::from_rgba8(255, 0, 0, 255),
            Color::from_rgba8(0, 0, 255, 255),
        );
        let gradient = Gradient::new_linear(
            ondin_core::kurbo::Point::ZERO,
            ondin_core::kurbo::Point::new(100.0, 0.0),
        )
        .with_stops([(0.0_f32, first), (1.0_f32, last)]);
        let mut subject = subject_of(TextStyle::default(), CharSpans::default());
        subject.text_paint = Some(Brush::Gradient(gradient.into()));

        let ramp = subject.text_ramp().expect("a gradient fill has a ramp");
        assert_eq!(ramp.len(), 2, "both stops reach the chip: {ramp:?}");
        assert_eq!(
            subject.text_color(),
            first,
            "the collapse is the first stop, not black"
        );

        subject.text_paint = Some(Brush::Solid(last));
        assert!(subject.text_ramp().is_none());
        assert_eq!(subject.text_color(), last);

        subject.text_paint = None;
        assert!(subject.text_ramp().is_none());
        assert_eq!(subject.text_color(), Color::BLACK);
    }

    /// The underline's colour as the transaction leaves it: the node's default, and
    /// whether any span still overrides it.
    fn underline_after(tx: &Transaction) -> (Option<Color>, bool) {
        let mut default = None;
        let mut overridden = false;
        for op in &tx.0 {
            match op {
                Operation::SetTextStyle { style, .. } => {
                    default = match style.get(CharAttrKind::Underline) {
                        CharAttr::Underline(d) => d.and_then(|d| d.color),
                        other => unreachable!("{other:?}"),
                    };
                }
                Operation::SetTextSpans { spans, .. } => {
                    overridden =
                        spans.shared_in(&TextStyle::default(), CharAttrKind::Underline, 0..8)
                            != Some(CharAttr::Underline(None));
                }
                _ => {}
            }
        }
        (default, overridden)
    }

    /// Clearing a decoration's colour clears it from both the node default
    /// (`TextStyle::set`) and a span override (`CharSpans::clear`).
    #[test]
    fn clearing_an_underline_colour_clears_the_default_and_the_spans() {
        let mut styled = TextStyle::default();
        styled.set(CharAttr::Underline(Some(red())));

        let subject = subject_of(styled.clone(), CharSpans::default());
        let (tx, changes) = char_attrs_tx(
            &subject,
            vec![CharAttr::Underline(Some(Decoration::default()))],
        );
        assert!(changes, "clearing a colour that is set is not a no-op");
        assert_eq!(underline_after(&tx), (None, false));

        // Red as a span `shown()` cannot see, since it reads byte 0.
        let mut spans = CharSpans::default();
        spans.set(
            4..8,
            CharAttr::Underline(Some(red())),
            &TextStyle::default(),
        );
        let subject = subject_of(TextStyle::default(), spans);
        let (tx, changes) = char_attrs_tx(
            &subject,
            vec![CharAttr::Underline(Some(Decoration::default()))],
        );
        assert!(
            changes,
            "clearing a colour a span still holds is not a no-op"
        );
        assert_eq!(underline_after(&tx), (None, false));
    }

    /// A colour in a span that misses byte 0 still lights the reset (`ragged`).
    #[test]
    fn a_colour_hiding_in_a_span_still_lights_the_reset() {
        let mut spans = CharSpans::default();
        spans.set(
            4..8,
            CharAttr::Underline(Some(red())),
            &TextStyle::default(),
        );
        let subject = subject_of(TextStyle::default(), spans);

        let shown = decoration_of(&subject, CharAttrKind::Underline);
        assert_eq!(
            shown.and_then(|d| d.color),
            None,
            "the premise: byte 0 has no colour, so `shown` reports none"
        );
        assert!(
            subject.ragged(CharAttrKind::Underline),
            "but the range disagrees, which is what the reset has to notice"
        );
    }

    /// The widest tab label fits its cell, since `ui::segment_label` clips nothing.
    #[test]
    fn the_spelled_out_tab_labels_fit_their_cells() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let track = MENU_INNER - CLOSE_BUTTON_W - COL_GAP;
        let cells = TypeTab::ALL.len() as f32;
        let cell_w = (track - 4.0 - 2.0 * (cells - 1.0)) / cells;

        let font = egui::FontId::proportional(ui::SEGMENT_LABEL_PT);
        for tab in TypeTab::ALL {
            let w = ctx.fonts_mut(|f| {
                f.layout_no_wrap(tab.label().to_string(), font.clone(), theme::color::TEXT)
                    .size()
                    .x
            });
            assert!(
                w <= cell_w,
                "`{}` inks {w:.1}pt in a {cell_w:.1}pt cell — it would paint over its neighbour",
                tab.label()
            );
        }
    }

    /// The segmented tracks inside a tab (full inner width, no close button) fit their labels.
    #[test]
    fn the_segmented_labels_inside_a_tab_fit_their_cells() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        let font = egui::FontId::proportional(ui::SEGMENT_LABEL_PT);
        let widest = |labels: &[&str]| {
            labels
                .iter()
                .map(|s| {
                    ctx.fonts_mut(|f| {
                        f.layout_no_wrap((*s).to_string(), font.clone(), theme::color::TEXT)
                            .size()
                            .x
                    })
                })
                .fold(0.0_f32, f32::max)
        };
        for (what, labels) in [
            (
                "Case",
                TextCase::ALL.iter().map(|c| c.label()).collect::<Vec<_>>(),
            ),
            (
                "Word break",
                WordBreak::ALL.iter().map(|w| w.label()).collect(),
            ),
            (
                "Long words",
                OverflowWrap::ALL.iter().map(|w| w.label()).collect(),
            ),
            ("Sizing", TextSizing::LABELS.to_vec()),
        ] {
            let n = labels.len() as f32;
            let cell_w = (MENU_INNER - 4.0 - 2.0 * (n - 1.0)) / n;
            let w = widest(&labels);
            assert!(
                w <= cell_w,
                "the widest `{what}` label inks {w:.1}pt in a {cell_w:.1}pt cell"
            );
        }
    }

    /// Every `Length` field's two ends bound the same quantity, so the unit chip
    /// cannot change the value it converts; scope is the ranges, not the call sites.
    ///
    /// flip: hard-coding `Bounds::px` to `-10_000.0..=10_000.0` → fails at the `Bounds` derivation assertion
    #[test]
    fn both_ends_of_every_length_field_bound_the_same_quantity() {
        for (name, b) in [
            ("TRACKING", &Bounds::TRACKING),
            ("SIGNED_TRACKING", &Bounds::SIGNED_TRACKING),
            ("PARAGRAPH_SPACING", &Bounds::PARAGRAPH_SPACING),
            ("INDENT", &Bounds::INDENT),
            ("DECORATION_THICKNESS", &Bounds::DECORATION_THICKNESS),
            ("DECORATION_OFFSET", &Bounds::DECORATION_OFFSET),
            ("LINE_HEIGHT", &Bounds::LINE_HEIGHT),
        ] {
            for font_size in [8.0, 16.0, 72.0] {
                assert_eq!(
                    b.px(font_size),
                    px_range_for(b.pct.clone(), font_size),
                    "{name} @{font_size}pt must derive its px face, not carry one"
                );
            }
            // flip: `em` returning the `%` range undivided → fails here on `TRACKING`.
            let em = b.em();
            assert_eq!(
                (*em.start() * 100.0, *em.end() * 100.0),
                (*b.pct.start(), *b.pct.end()),
                "{name}'s em face must be its % face"
            );
        }

        let ranges: Vec<(&str, std::ops::RangeInclusive<f64>)> = vec![
            ("letter/word spacing", Bounds::TRACKING.pct.clone()),
            ("baseline shift", Bounds::SIGNED_TRACKING.pct.clone()),
            (
                "decoration thickness",
                Bounds::DECORATION_THICKNESS.pct.clone(),
            ),
            ("decoration offset", Bounds::DECORATION_OFFSET.pct.clone()),
            ("paragraph spacing", Bounds::PARAGRAPH_SPACING.pct.clone()),
            (
                "first-line indent, indent start, indent end",
                Bounds::INDENT.pct.clone(),
            ),
            ("line height", Bounds::LINE_HEIGHT.pct.clone()),
        ];

        for (name, pct) in ranges {
            for font_size in [8.0, 16.0, 72.0] {
                let px = px_range_for(pct.clone(), font_size);
                for end in [*pct.start(), *pct.end()] {
                    let converted = Length::Em(end / 100.0).in_unit(LengthUnit::Px, font_size);
                    let Length::Px(v) = converted else {
                        panic!("{name}: in_unit(Px) must answer in px")
                    };
                    assert!(
                        px.contains(&v),
                        "{name} @{font_size}pt: {end}% converts to {v}px, \
                         outside the px face's {:?}..={:?}",
                        px.start(),
                        px.end()
                    );
                }

                for end in [*px.start(), *px.end()] {
                    let converted = Length::Px(end).in_unit(LengthUnit::Em, font_size);
                    let Length::Em(m) = converted else {
                        panic!("{name}: in_unit(Em) must answer in em")
                    };
                    let as_pct = m * 100.0;
                    assert!(
                        pct.contains(&as_pct) || (as_pct - pct.end()).abs() < 1e-6,
                        "{name} @{font_size}pt: {end}px converts to {as_pct}%, \
                         outside the % face's {:?}..={:?}",
                        pct.start(),
                        pct.end()
                    );
                }
            }
        }
    }

    /// The px face never collapses to a point, even at a reported font size of 0.
    #[test]
    fn a_derived_px_range_survives_a_font_size_of_zero() {
        let r = px_range_for(MIN_TRACKING_PCT..=MAX_TRACKING_PCT, 0.0);
        assert!(r.end() > r.start(), "{r:?}");
        assert!(
            r.contains(&1.0),
            "and something ordinary can still be typed"
        );
    }

    /// Each bound carries its field's sign rule; the model's own floor
    /// (`typography::canonical_decoration`) is the second defence.
    #[test]
    fn an_unsigned_bound_starts_at_zero_and_a_signed_one_runs_both_ways() {
        for (name, b) in [
            ("PARAGRAPH_SPACING", &Bounds::PARAGRAPH_SPACING),
            ("DECORATION_THICKNESS", &Bounds::DECORATION_THICKNESS),
        ] {
            assert_eq!(*b.pct.start(), 0.0, "{name} is unsigned: {:?}", b.pct);
            assert!(*b.pct.end() > 0.0, "{name} must admit a value: {:?}", b.pct);
        }
        for (name, b) in [
            ("INDENT", &Bounds::INDENT),
            ("DECORATION_OFFSET", &Bounds::DECORATION_OFFSET),
            ("SIGNED_TRACKING", &Bounds::SIGNED_TRACKING),
        ] {
            assert_eq!(
                *b.pct.start(),
                -*b.pct.end(),
                "{name} runs both ways from zero: {:?}",
                b.pct
            );
            assert!(*b.pct.end() > 0.0, "{name} must admit a value: {:?}", b.pct);
        }
    }
}

/// The egui behaviour `cancel_gesture` works around: a right-click whose press and
/// release land in one frame takes the drag away before the cancel sees it. The gate
/// is pinned in `canvas::headless_app_tests::cancelling_needs_a_witness_and_our_two_do_not_blink`.
#[cfg(test)]
mod cancel_gate {
    use eframe::egui;

    fn button(b: egui::PointerButton, pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: b,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// Drive a `DragValue` into a real drag, then hand it `events` and report
    /// whether egui still calls it dragged.
    fn dragged_after(events: Vec<egui::Event>) -> bool {
        let ctx = egui::Context::default();
        let mut v = 50.0_f64;
        let mut t = 0.0_f64;
        let at = egui::pos2(40.0, 20.0);
        let frame = |events: Vec<egui::Event>, v: &mut f64, t: &mut f64| {
            *t += 0.1;
            let _ = ctx.run_ui(
                egui::RawInput {
                    time: Some(*t),
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(400.0, 200.0),
                    )),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.add(egui::DragValue::new(v).speed(1.0));
                },
            );
        };
        frame(vec![egui::Event::PointerMoved(at)], &mut v, &mut t);
        frame(
            vec![button(egui::PointerButton::Primary, at, true)],
            &mut v,
            &mut t,
        );
        let mut p = at;
        for _ in 0..3 {
            p.x += 25.0;
            frame(vec![egui::Event::PointerMoved(p)], &mut v, &mut t);
        }
        assert!(
            ctx.dragged_id().is_some() && v != 50.0,
            "the fixture has to reach a real drag before it can say anything about \
             losing one: dragged={}, value={v}",
            ctx.dragged_id().is_some()
        );
        let events = events
            .into_iter()
            .map(|e| match e {
                egui::Event::PointerButton {
                    button: b, pressed, ..
                } => button(b, p, pressed),
                other => other,
            })
            .collect();
        frame(events, &mut v, &mut t);
        ctx.dragged_id().is_some()
    }

    #[test]
    fn a_right_click_short_enough_to_fit_one_frame_takes_the_drag_away() {
        let zero = egui::Pos2::ZERO;
        assert!(
            dragged_after(vec![button(egui::PointerButton::Secondary, zero, true)]),
            "a press on its own leaves the drag in place — this is the case that \
             always worked"
        );
        assert!(
            !dragged_after(vec![
                button(egui::PointerButton::Secondary, zero, true),
                button(egui::PointerButton::Secondary, zero, false),
            ]),
            "and a press with its release in the same frame does not — if this ever \
             passes, egui has changed and `cancel_gesture`'s extra tests are no \
             longer load-bearing (they stay harmless)"
        );
    }
}

#[cfg(test)]
mod loading_dot_tests {
    //! The loading dot's pulse, as a function of time; its placement and ink are checked by eye.

    use super::*;

    /// The dot starts dark, reaches full at the half period, and returns: a pulse, not a fade-in.
    ///
    /// flip: plain `sin` in `loading_dot_phase` → fails at the full-brightness assertion; `sin().abs()` passes here and fails the seam test
    #[test]
    fn the_dot_starts_dark_reaches_full_and_returns() {
        assert!(
            loading_dot_phase(0.0) < 0.001,
            "the dot is already lit the instant it appears, so it reads as \
             something switching off rather than as something pending"
        );
        assert!(
            loading_dot_phase(LOADING_DOT_PERIOD / 2.0) > 0.999,
            "it never reaches full brightness"
        );
        assert!(
            loading_dot_phase(LOADING_DOT_PERIOD) < 0.001,
            "it does not come back down, so this is a fade-in and not a pulse"
        );
    }

    /// The pulse is periodic with no jump at the wrap and no frame step big enough to flicker.
    /// The bound `π/P` per second is the exact derivative maximum of the raised cosine.
    #[test]
    fn the_pulse_wraps_without_a_seam() {
        let step = 1.0 / 60.0;
        let max_jump = std::f32::consts::PI / LOADING_DOT_PERIOD as f32 * step as f32;
        let mut t = 0.0;
        let mut prev = loading_dot_phase(t);
        while t < LOADING_DOT_PERIOD * 2.5 {
            t += step;
            let now = loading_dot_phase(t);
            assert!(
                (now - prev).abs() <= max_jump,
                "the phase jumped {:.4} between two frames at t={t:.3}, which is a \
                 visible flicker rather than a breath",
                (now - prev).abs()
            );
            assert!(
                (0.0..=1.0).contains(&now),
                "the phase left 0..=1 at t={t:.3}, so the alpha arithmetic on it \
                 wraps rather than clamps"
            );
            prev = now;
        }
    }

    /// `FontService::is_loading`, which `family_row` reads for the dot, is false for a family nothing is fetching.
    #[test]
    fn the_predicate_the_dot_reads_is_false_for_a_family_nothing_is_fetching() {
        let service = crate::fonts::FontService::inert();
        assert!(
            !service.is_loading("Inter"),
            "a bundled family that was never fetched must not wear the dot"
        );
        assert!(
            !service.is_loading("Nothing At All"),
            "nor may a family the service has never heard of"
        );
    }
}

/// The panel tests' one text fixture: a headless app holding a single `Auto`
/// text node of `content` in Inter at 20pt, selected, and its id.
#[cfg(test)]
fn app_with_text_of(content: &str) -> (egui::Context, crate::app::OndinApp, ondin_core::NodeId) {
    use ondin_core::{Document, Operation, Transaction};
    let ctx = egui::Context::default();
    crate::theme::install(&ctx);
    let mut app = crate::app::OndinApp::headless(&ctx);
    let mut ids = ondin_core::IdSource::new(1);
    let root = ids.mint();
    let mut doc = Document::new(root);
    let id = ids.mint();
    doc.apply(&Transaction(vec![Operation::CreateNode {
        id,
        parent: root,
        index: 0,
        kind: ondin_core::NodeKind::Text {
            content: content.into(),
            style: Box::new(TextStyle {
                font_family: "Inter".into(),
                font_size: 20.0,
                ..TextStyle::default()
            }),
            spans: CharSpans::default(),
            para_spans: ParaSpans::default(),
            paragraph: ParagraphStyle::default(),
            block: BlockStyle::default(),
            sizing: TextSizing::Auto,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        },
        transform: None,
        name: None,
    }]))
    .expect("a text node");
    app.session.adopt_document(doc, None);
    app.session.selection.set_one(id);
    (ctx, app, id)
}

/// `app_with_text_of("hello")`.
#[cfg(test)]
fn app_with_text() -> (egui::Context, crate::app::OndinApp, ondin_core::NodeId) {
    app_with_text_of("hello")
}

/// The Type card over several text layers: what its fields read and what their
/// writes reach.
#[cfg(test)]
mod several_layer_tests {
    use super::*;
    use crate::app::OndinApp;
    use ondin_core::{Document, IdSource};

    /// Three selected text layers: sizes 20, 20, 32; weights 400, 700, 400.
    fn three() -> (egui::Context, OndinApp, Vec<NodeId>) {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let mut texts = Vec::new();
        for (i, (size, weight)) in [(20.0, 400), (20.0, 700), (32.0, 400)]
            .into_iter()
            .enumerate()
        {
            let id = ids.mint();
            doc.apply(&Transaction(vec![Operation::CreateNode {
                id,
                parent: root,
                index: i,
                kind: ondin_core::NodeKind::Text {
                    content: "hello".into(),
                    style: Box::new(TextStyle {
                        font_family: "Inter".into(),
                        font_size: size,
                        weight,
                        ..TextStyle::default()
                    }),
                    spans: CharSpans::default(),
                    para_spans: ParaSpans::default(),
                    paragraph: ParagraphStyle::default(),
                    block: BlockStyle::default(),
                    sizing: TextSizing::Auto,
                    on_path: None,
                    on_path_flip: false,
                    on_path_offset: 0.0,
                },
                transform: None,
                name: None,
            }]))
            .expect("a text node");
            texts.push(id);
        }
        app.session.adopt_document(doc, None);
        app.session.selection.set(texts.clone());
        (ctx, app, texts)
    }

    fn style(app: &OndinApp, id: NodeId) -> TextStyle {
        match app.session.doc.get(id).map(|n| n.kind()) {
            Some(ondin_core::NodeKind::Text { style, .. }) => (**style).clone(),
            other => panic!("not a text: {other:?}"),
        }
    }

    /// Size reads Mixed where the layers disagree and not where they agree; the
    /// shared family never does.
    ///
    /// flip: `mixed` without its `others` clause → fails at the first size assertion.
    #[test]
    fn a_field_over_layers_that_disagree_reads_mixed() {
        let (_ctx, app, texts) = three();
        let all = TypeSubject::several(&app, &texts).expect("a several-layer subject");
        assert_eq!(all.ids(), texts);
        assert!(
            all.mixed(CharAttrKind::Size),
            "20, 20 and 32 read as one size"
        );
        assert!(!all.mixed(CharAttrKind::Family));
        let pair = TypeSubject::several(&app, &texts[..2]).expect("a pair");
        assert!(!pair.mixed(CharAttrKind::Size), "20 and 20 read as mixed");
        assert!(pair.mixed(CharAttrKind::Weight));
        assert!(
            TypeSubject::several(&app, &texts[..1]).is_none(),
            "one layer is the single card's"
        );
    }

    /// **Italic over layers of three weights keeps each layer's weight**.
    ///
    /// flip: `toggle_face` stepping every layer from the first's weight → fails
    /// at *"each layer keeps its weight"* (`[400, 400, 400]`).
    #[test]
    fn italic_over_layers_of_several_weights_keeps_each_weight() {
        let (_ctx, mut app, texts) = three();
        let all = TypeSubject::several(&app, &texts).expect("a several-layer subject");
        app.toggle_face(&all, false);
        let faces: Vec<_> = texts
            .iter()
            .map(|id| (style(&app, *id).weight, style(&app, *id).italic))
            .collect();
        assert_eq!(
            faces,
            vec![(400, true), (700, true), (400, true)],
            "each layer keeps its weight"
        );
    }

    /// Two layers of one weight told apart only by `wdth` read as two faces;
    /// both at the same `wdth`, one face.
    ///
    /// flip: `faces_disagree` answering false → fails at "regular and condensed".
    #[test]
    fn layers_on_different_faces_of_one_weight_disagree() {
        let (_ctx, mut app, texts) = three();
        let wdth = Tag::new(*b"wdth");
        let face = |name: &str, w: f64| FontVariant {
            name: name.into(),
            weight: 400,
            italic: false,
            coords: vec![AxisSetting::new(wdth, w)],
        };
        let variants = [face("Regular", 100.0), face("Condensed", 75.0)];
        let pair = [texts[0], texts[2]];
        for (id, w) in pair.iter().zip([75.0, 100.0]) {
            let one = TypeSubject::of(&app, *id).expect("a subject");
            app.apply_char_attrs(
                &one,
                vec![CharAttr::Variations(vec![AxisSetting::new(wdth, w)])],
            );
        }
        let both = TypeSubject::several(&app, &pair).expect("a pair");
        assert!(!both.mixed(CharAttrKind::Weight), "the fixture: one weight");
        assert!(
            faces_disagree(&variants, &both),
            "regular and condensed read as one face"
        );
        let one = TypeSubject::of(&app, texts[2]).expect("a subject");
        app.apply_char_attrs(
            &one,
            vec![CharAttr::Variations(vec![AxisSetting::new(wdth, 75.0)])],
        );
        let both = TypeSubject::several(&app, &pair).expect("a pair");
        assert!(!faces_disagree(&variants, &both), "one face read as two");
    }

    /// Auto layers laid at different heights read mixed; an agreeing pair reads one value.
    ///
    /// flip: `line_height_mixed` answering `self.mixed(..)` alone → fails at "24 and 38".
    #[test]
    fn auto_over_lines_of_different_heights_reads_mixed() {
        let (_ctx, app, texts) = three();
        let mut all = TypeSubject::several(&app, &texts).expect("a subject");
        all.line_height = Some(24.0);
        for (o, h) in all.others.iter_mut().zip([24.0, 38.0]) {
            o.line_height = Some(h);
        }
        assert!(
            !all.mixed(CharAttrKind::LineHeight),
            "the fixture: all Auto"
        );
        assert!(all.line_height_mixed(), "24 and 38 read as one line");
        all.others.truncate(1);
        assert!(!all.line_height_mixed(), "24 and 24 read as mixed");
    }

    /// A size write reaches every layer, keeps each its own weight, and undoes as one step.
    ///
    /// flip: `char_attrs_tx` returning `one_char_attrs_tx(subject, attrs)` → fails at the second layer's size.
    #[test]
    fn a_write_reaches_every_layer_and_keeps_their_own_values() {
        let (_ctx, mut app, texts) = three();
        let all = TypeSubject::several(&app, &texts).expect("a several-layer subject");
        app.apply_char_attrs(&all, vec![CharAttr::Size(24.0)]);
        for (id, weight) in texts.iter().zip([400, 700, 400]) {
            let s = style(&app, *id);
            assert_eq!(s.font_size, 24.0, "{id:?} never took the size");
            assert_eq!(s.weight, weight, "{id:?} lost its own weight");
        }
        app.undo();
        let sizes: Vec<f64> = texts.iter().map(|id| style(&app, *id).font_size).collect();
        assert_eq!(sizes, [20.0, 20.0, 32.0]);
    }

    /// A tracking unit click converts each layer at its own size (10% over 20, 20, 32 → 2, 2, 3.2px).
    ///
    /// flip: converting at `subject.font_size()` → fails on the third layer.
    #[test]
    fn a_unit_click_converts_each_layer_at_its_own_size() {
        let (ctx, mut app, texts) = three();
        let all = TypeSubject::several(&app, &texts).expect("a subject");
        app.apply_char_attrs(&all, vec![CharAttr::LetterSpacing(Length::Em(0.1))]);
        let frame = |app: &mut OndinApp, events: Vec<egui::Event>| -> egui::Rect {
            let mut field = egui::Rect::NOTHING;
            let all = TypeSubject::several(app, &texts).expect("a subject");
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(TEST_AREA),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let top = ui.cursor().min;
                    let size = egui::vec2(130.0, ROW_H);
                    app.type_letter_spacing_field(ui, &all, size, None);
                    field = egui::Rect::from_min_size(top, size);
                },
            );
            field
        };
        let field = frame(&mut app, Vec::new());
        let at = egui::pos2(field.right() - 8.0, field.center().y);
        frame(&mut app, vec![egui::Event::PointerMoved(at)]);
        for pressed in [true, false] {
            frame(
                &mut app,
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                }],
            );
        }
        let tracking: Vec<Length> = texts
            .iter()
            .map(|id| style(&app, *id).letter_spacing)
            .collect();
        assert_eq!(
            tracking,
            [Length::Px(2.0), Length::Px(2.0), Length::Px(3.2)],
            "each layer at its own size"
        );
    }

    /// A variant picked over several layers keeps each layer's own other axes.
    ///
    /// flip: `apply_variant` merging into the first layer's coords for every layer → fails at the second layer.
    #[test]
    fn a_variant_pick_keeps_each_layers_own_axes() {
        use ondin_core::typography::{AxisSetting, Tag};
        let (_ctx, mut app, texts) = three();
        let axis = |tag: &[u8; 4], value| AxisSetting {
            tag: Tag::new(*tag),
            value,
        };
        for (i, coords) in [
            (0, vec![axis(b"opsz", 14.0)]),
            (2, vec![axis(b"wdth", 75.0)]),
        ] {
            let mut s = style(&app, texts[i]);
            s.variations = coords;
            app.commit_edit(Transaction(vec![Operation::SetTextStyle {
                id: texts[i],
                style: s,
                spans: None,
            }]));
        }
        let all = TypeSubject::several(&app, &texts).expect("a subject");
        app.apply_variant(
            &all,
            &text::FontVariant {
                name: "Graded".into(),
                weight: 500,
                italic: false,
                coords: vec![axis(b"GRAD", 50.0)],
            },
        );
        let has = |i: usize, want: &[AxisSetting]| {
            let got = style(&app, texts[i]).variations;
            got.len() == want.len() && want.iter().all(|a| got.contains(a))
        };
        assert!(
            has(0, &[axis(b"opsz", 14.0), axis(b"GRAD", 50.0)]),
            "the first keeps its size"
        );
        assert!(has(1, &[axis(b"GRAD", 50.0)]));
        assert!(
            has(2, &[axis(b"wdth", 75.0), axis(b"GRAD", 50.0)]),
            "the third keeps its width"
        );
        assert!(texts.iter().all(|id| style(&app, *id).weight == 500));
    }

    /// The line-height unit click converts each layer at its own size (120% → 24, 24, 38.4px).
    ///
    /// flip: converting at `subject.font_size()` → fails on the third layer.
    #[test]
    fn a_line_height_unit_click_converts_each_layer_at_its_own_size() {
        let (ctx, mut app, texts) = three();
        for id in &texts {
            let mut s = style(&app, *id);
            s.line_height = Some(Length::Em(1.2));
            app.commit_edit(Transaction(vec![Operation::SetTextStyle {
                id: *id,
                style: s,
                spans: None,
            }]));
        }
        let frame = |app: &mut OndinApp, events: Vec<egui::Event>| -> egui::Rect {
            let mut field = egui::Rect::NOTHING;
            let all = TypeSubject::several(app, &texts).expect("a subject");
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(TEST_AREA),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let top = ui.cursor().min;
                    let size = egui::vec2(130.0, ROW_H);
                    app.type_line_height_field(ui, &all, size, None);
                    field = egui::Rect::from_min_size(top, size);
                },
            );
            field
        };
        let field = frame(&mut app, Vec::new());
        let at = egui::pos2(field.right() - 8.0, field.center().y);
        frame(&mut app, vec![egui::Event::PointerMoved(at)]);
        for pressed in [true, false] {
            frame(
                &mut app,
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                }],
            );
        }
        let heights: Vec<Option<Length>> = texts
            .iter()
            .map(|id| style(&app, *id).line_height)
            .collect();
        let near = |l: Option<Length>, want: f64| matches!(l, Some(Length::Px(v)) if (v - want).abs() < 1e-3);
        assert!(
            near(heights[0], 24.0) && near(heights[1], 24.0) && near(heights[2], 38.4),
            "each layer at its own size: {heights:?}"
        );
    }

    /// Picking Bottom over three fixed-height layers writes each its own block with only the alignment changed.
    ///
    /// flip: the vertical track's several-layer arm switched off → fails at the second layer.
    #[test]
    fn a_vertical_alignment_pick_writes_every_layer() {
        let (ctx, mut app, texts) = three();
        let mut ops: Vec<Operation> = texts
            .iter()
            .map(|id| Operation::SetGeometry {
                id: *id,
                geometry: ondin_core::GeometryPatch::TextSizing(TextSizing::Fixed(
                    ondin_core::kurbo::Size::new(120.0, 80.0),
                )),
            })
            .collect();
        ops.push(Operation::SetBlockStyle {
            id: texts[2],
            block: BlockStyle {
                overflow: ondin_core::TextOverflow::Clip,
                ..BlockStyle::default()
            },
        });
        app.commit_edit(Transaction(ops));
        let frame = |app: &mut OndinApp, events: Vec<egui::Event>| -> egui::Rect {
            let mut row = egui::Rect::NOTHING;
            let all = TypeSubject::several(app, &texts).expect("a subject");
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(TEST_AREA),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(268.0);
                    let top = ui.cursor().min;
                    app.type_alignment_row(ui, &all, [&None, &None]);
                    row = egui::Rect::from_min_size(top, egui::vec2(268.0, ROW_H));
                },
            );
            row
        };
        let row = frame(&mut app, Vec::new());
        // The vertical track's third cell is *Bottom*.
        let start = row.left() + (row.width() - GAP) / 2.0 + GAP;
        let at = egui::pos2(start + (row.right() - start) * 5.0 / 6.0, row.center().y);
        frame(&mut app, vec![egui::Event::PointerMoved(at)]);
        for pressed in [true, false] {
            frame(
                &mut app,
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                }],
            );
        }
        let blocks: Vec<BlockStyle> = texts
            .iter()
            .map(|id| match app.session.doc.get(*id).map(|n| n.kind()) {
                Some(ondin_core::NodeKind::Text { block, .. }) => *block,
                _ => panic!("not a text"),
            })
            .collect();
        for (i, b) in blocks.iter().enumerate() {
            assert_eq!(b.vertical_align, VerticalAlign::Bottom, "layer {i}");
        }
        assert_eq!(
            blocks[2].overflow,
            ondin_core::TextOverflow::Clip,
            "the third kept its overflow"
        );
    }

    /// A Mixed line height takes a typed px on every layer, and a click in and out writes nothing.
    ///
    /// flips: the `_ if mixed` arms removed → fails "every layer at 30px"; the Mixed field's moved gate removed → fails "a click in and out writes nothing".
    #[test]
    fn a_mixed_line_height_over_auto_takes_a_typed_px() {
        let (ctx, mut app, texts) = three();
        let mut set = style(&app, texts[1]);
        set.line_height = Some(Length::Px(24.0));
        app.commit_edit(Transaction(vec![Operation::SetTextStyle {
            id: texts[1],
            style: set,
            spans: None,
        }]));
        let frame = |app: &mut OndinApp, events: Vec<egui::Event>| -> egui::Rect {
            let mut field = egui::Rect::NOTHING;
            let all = TypeSubject::several(app, &texts).expect("a subject");
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(TEST_AREA),
                    events,
                    ..Default::default()
                },
                |ui| {
                    let top = ui.cursor().min;
                    let size = egui::vec2(130.0, ROW_H);
                    app.type_line_height_field(ui, &all, size, None);
                    field = egui::Rect::from_min_size(top, size);
                },
            );
            field
        };
        let heights = |app: &OndinApp| -> Vec<Option<Length>> {
            texts.iter().map(|id| style(app, *id).line_height).collect()
        };
        let before = heights(&app);
        assert_eq!(
            before,
            [None, Some(Length::Px(24.0)), None],
            "the fixture: Mixed"
        );
        let field = frame(&mut app, Vec::new());
        let at = field.center();
        let click = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        let away = egui::pos2(at.x, at.y + 200.0);
        let click_away = |pressed| egui::Event::PointerButton {
            pos: away,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(&mut app, vec![egui::Event::PointerMoved(at)]);
        frame(&mut app, vec![click(true)]);
        frame(&mut app, vec![click(false)]);
        frame(&mut app, Vec::new());
        frame(
            &mut app,
            vec![egui::Event::PointerMoved(away), click_away(true)],
        );
        frame(&mut app, vec![click_away(false)]);
        frame(&mut app, Vec::new());
        assert_eq!(heights(&app), before, "a click in and out writes nothing");

        let key = |key, pressed| egui::Event::Key {
            key,
            physical_key: None,
            pressed,
            repeat: false,
            modifiers: Default::default(),
        };
        let type_in = |app: &mut OndinApp, text: &str| {
            frame(app, vec![egui::Event::PointerMoved(at)]);
            frame(app, vec![click(true)]);
            frame(app, vec![click(false)]);
            frame(app, Vec::new());
            frame(
                app,
                vec![egui::Event::Key {
                    key: egui::Key::A,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers: egui::Modifiers::COMMAND,
                }],
            );
            frame(app, vec![egui::Event::Text(text.into())]);
            frame(
                app,
                vec![key(egui::Key::Enter, true), key(egui::Key::Enter, false)],
            );
            frame(app, Vec::new());
            frame(app, Vec::new());
        };
        type_in(&mut app, "30");
        assert_eq!(
            heights(&app),
            [Some(Length::Px(30.0)); 3],
            "every layer at 30px"
        );

        let mut first = style(&app, texts[0]);
        first.line_height = Some(Length::Em(1.5));
        app.commit_edit(Transaction(vec![Operation::SetTextStyle {
            id: texts[0],
            style: first,
            spans: None,
        }]));
        type_in(&mut app, "40");
        assert_eq!(
            heights(&app),
            [Some(Length::Px(40.0)); 3],
            "px over a first layer in %"
        );
    }

    /// `line_height_as` moves each layer into the first layer's next state, keeping what it draws.
    #[test]
    fn a_line_height_moves_into_the_first_layers_next_state() {
        let px = Some(Length::Px(1.0));
        let em = Some(Length::Em(1.0));
        assert_eq!(
            line_height_as(Some(Length::Em(1.5)), 32.0, None, px),
            Some(Length::Px(48.0))
        );
        assert_eq!(
            line_height_as(None, 20.0, Some(30.0), em),
            Some(Length::Em(1.5))
        );
        assert_eq!(line_height_as(None, 20.0, None, em), Some(Length::Em(1.2)));
        assert_eq!(
            line_height_as(Some(Length::Px(24.0)), 20.0, None, None),
            None
        );
    }

    /// Picking Center over three layers (one already centred) writes the other two and leaves the centred one's spacing.
    ///
    /// flip: the row's several-layer arm switched off → fails at the second layer, still `Start`.
    #[test]
    fn an_alignment_pick_writes_every_layer() {
        let (ctx, mut app, texts) = three();
        app.commit_edit(Transaction(vec![Operation::SetParagraphStyle {
            id: texts[2],
            paragraph: ParagraphStyle {
                align: TextAlign::Center,
                spacing: Length::Px(6.0),
                ..ParagraphStyle::default()
            },
            spans: None,
        }]));
        let frame = |app: &mut OndinApp, events: Vec<egui::Event>| -> egui::Rect {
            let mut track = egui::Rect::NOTHING;
            let all = TypeSubject::several(app, &texts).expect("a subject");
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(TEST_AREA),
                    events,
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(268.0);
                    let top = ui.cursor().min;
                    let menu = app.type_alignment_row(ui, &all, [&None, &None]);
                    assert!(menu.is_none(), "a several-layer row offers the popup");
                    track = egui::Rect::from_min_size(top, egui::vec2(130.0, ROW_H));
                },
            );
            track
        };
        let track = frame(&mut app, Vec::new());
        let aligns: Vec<TextAlign> = texts
            .iter()
            .filter_map(|id| match app.session.doc.get(*id).map(|n| n.kind()) {
                Some(ondin_core::NodeKind::Text { paragraph, .. }) => Some(paragraph.align),
                _ => None,
            })
            .collect();
        assert_eq!(aligns[2], TextAlign::Center);
        assert!(
            aligns[..2].iter().all(|a| *a != TextAlign::Center),
            "{aligns:?}"
        );
        let at = egui::pos2(track.left() + track.width() * 0.375, track.center().y);
        frame(&mut app, vec![egui::Event::PointerMoved(at)]);
        for pressed in [true, false] {
            frame(
                &mut app,
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: Default::default(),
                }],
            );
        }
        for id in &texts {
            let Some(ondin_core::NodeKind::Text { paragraph, .. }) =
                app.session.doc.get(*id).map(|n| n.kind())
            else {
                panic!("not a text");
            };
            assert_eq!(paragraph.align, TextAlign::Center, "{id:?} not centred");
        }
        let Some(ondin_core::NodeKind::Text { paragraph, .. }) =
            app.session.doc.get(texts[2]).map(|n| n.kind())
        else {
            panic!("not a text");
        };
        assert_eq!(
            paragraph.spacing,
            Length::Px(6.0),
            "the third lost its spacing"
        );
    }
}

/// The screen the panel tests lay a `Ui` out in, 400×600.
#[cfg(test)]
const TEST_AREA: egui::Rect = egui::Rect {
    min: egui::pos2(0.0, 0.0),
    max: egui::pos2(400.0, 600.0),
};

/// The Type panel's size field, driven through real pointer and key events to pin
/// which frame the valve commits on.
#[cfg(test)]
mod char_valve_tests {
    use super::*;
    use crate::app::OndinApp;
    use ondin_core::NodeId;

    /// One frame with the size field alone in a `Ui`, returning the rect it was laid out in.
    fn frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        id: NodeId,
        events: Vec<egui::Event>,
    ) -> egui::Rect {
        let subject = TypeSubject::of(app, id).expect("a subject");
        let mut out = egui::Rect::NOTHING;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(280.0, 600.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                ui.set_width(240.0);
                out = app
                    .type_size_field(ui, &subject, egui::vec2(120.0, 28.0), None)
                    .0
                    .rect;
            },
        );
        out
    }

    fn click(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// The fixture with the field laid out twice, returning its centre.
    fn settled() -> (egui::Context, OndinApp, NodeId, egui::Pos2) {
        let (ctx, mut app, id) = app_with_text();
        let mut at = egui::Pos2::ZERO;
        for _ in 0..2 {
            at = frame(&ctx, &mut app, id, Vec::new()).center();
        }
        (ctx, app, id, at)
    }

    /// Click into the field and type `text`, leaving it focused.
    fn type_into(ctx: &egui::Context, app: &mut OndinApp, id: NodeId, at: egui::Pos2, text: &str) {
        frame(ctx, app, id, vec![egui::Event::PointerMoved(at)]);
        frame(ctx, app, id, vec![click(at, true)]);
        frame(ctx, app, id, vec![click(at, false)]);
        frame(ctx, app, id, Vec::new());
        for ch in text.chars() {
            frame(ctx, app, id, vec![egui::Event::Text(ch.to_string())]);
        }
    }

    fn font_size(app: &OndinApp, id: NodeId) -> f64 {
        match app.session.doc.get(id).map(|n| n.kind()) {
            Some(ondin_core::NodeKind::Text { style, .. }) => style.font_size,
            _ => -1.0,
        }
    }

    /// The size field's binding doors (`FieldKey::Type`): a right-click opens the
    /// field menu on it; bound, the field is the chip at the field's own width and
    /// the text draws the variable; a number typed over the chip writes the size
    /// and detaches.
    ///
    /// flip: `type_size_field`'s `field_doors` call dropped → fails at *"the
    /// size's field menu"*; `number_retype`'s `Type` arm returning early → fails
    /// at *"the typed size"*.
    #[test]
    fn the_size_field_binds_shows_its_chip_and_takes_a_number_back() {
        use crate::panels::binding::{FieldKey, Retype};
        use ondin_core::variable::TEXT_SIZE;
        let (ctx, mut app, id, at) = settled();
        let key = FieldKey::Type {
            id,
            slot: TEXT_SIZE,
        };
        frame(&ctx, &mut app, id, vec![egui::Event::PointerMoved(at)]);
        for pressed in [true, false] {
            frame(
                &ctx,
                &mut app,
                id,
                vec![egui::Event::PointerButton {
                    pos: at,
                    button: egui::PointerButton::Secondary,
                    pressed,
                    modifiers: Default::default(),
                }],
            );
        }
        assert_eq!(
            app.field_menu.map(|(k, ..)| k),
            Some(key),
            "the size's field menu"
        );
        app.field_menu = None;

        let (mode, var) = (app.session.ids.mint_mode(), app.session.ids.mint_var());
        assert!(app.session.commit(Transaction(vec![
            Operation::AddMode {
                mode: ondin_core::Mode {
                    id: mode,
                    name: "Mode 1".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: ondin_core::Variable {
                    id: var,
                    name: "type/size".into(),
                    kind: ondin_core::VarKind::Number,
                    cells: [(mode, ondin_core::Cell::Number(32.0))].into(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ])));
        app.bind_field(key, var);
        assert_eq!(font_size(&app, id), 0.0, "the literal 0");
        let drawn = match app.session.doc.resolved_kind(id).map(|k| k.into_owned()) {
            Some(ondin_core::NodeKind::Text { style, .. }) => style.font_size,
            _ => -1.0,
        };
        assert_eq!(drawn, 32.0, "the text draws the variable");
        let chip = frame(&ctx, &mut app, id, Vec::new());
        assert_eq!(chip.width(), 120.0, "the chip at the field's width");

        app.var_retype = Some(Retype {
            key,
            text: "30".into(),
            focus: true,
        });
        for _ in 0..2 {
            frame(&ctx, &mut app, id, Vec::new());
        }
        frame(
            &ctx,
            &mut app,
            id,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        assert_eq!(font_size(&app, id), 30.0, "the typed size");
        assert_eq!(app.field_var(key), None, "detached");
    }

    /// A typed font size is one undo step, and the sizes on the way to it never reach the document.
    ///
    /// flip: restoring `drag_stopped() || lost_focus() || changed()` → fails at the during-typing assertion, before the count.
    #[test]
    fn typing_a_font_size_is_one_step_and_writes_no_size_on_the_way() {
        let (ctx, mut app, id, at) = settled();
        let depth0 = app.session.history.undo_depth();
        assert_eq!(font_size(&app, id), 20.0, "the fixture's size");

        type_into(&ctx, &mut app, id, at, "40");
        assert_eq!(
            font_size(&app, id),
            20.0,
            "the keystrokes preview: `4` is never a committed state of the document"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            depth0,
            "and nothing is in the history yet"
        );

        frame(
            &ctx,
            &mut app,
            id,
            vec![click(egui::pos2(5.0, 500.0), true)],
        );
        frame(
            &ctx,
            &mut app,
            id,
            vec![click(egui::pos2(5.0, 500.0), false)],
        );
        for _ in 0..2 {
            frame(&ctx, &mut app, id, Vec::new());
        }

        assert_eq!(font_size(&app, id), 40.0, "the typed size landed");
        assert_eq!(
            app.session.history.undo_depth() - depth0,
            1,
            "one number is one step"
        );
        app.session.undo();
        assert_eq!(
            font_size(&app, id),
            20.0,
            "and one undo puts it all the way back, not part of the way"
        );
    }

    /// `Escape` abandons a typed size.
    ///
    /// flip: dropping the `key_pressed(Escape)` arm → fails at the size (40 for 20); the count stays green.
    #[test]
    fn escape_abandons_a_typed_font_size() {
        let (ctx, mut app, id, at) = settled();
        let depth0 = app.session.history.undo_depth();

        type_into(&ctx, &mut app, id, at, "40");
        frame(
            &ctx,
            &mut app,
            id,
            vec![egui::Event::Key {
                key: egui::Key::Escape,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: Default::default(),
            }],
        );
        for _ in 0..3 {
            frame(&ctx, &mut app, id, Vec::new());
        }

        assert_eq!(font_size(&app, id), 20.0, "the typed size was abandoned");
        assert_eq!(
            app.session.history.undo_depth(),
            depth0,
            "and nothing reached the history to be undone"
        );
    }

    // The idle `changed()` frame is pinned by
    // `valve_arm_tests::a_changed_frame_with_no_engagement_commits_nothing`.
}

#[cfg(test)]
mod skip_ink_tests {
    //! The skip-ink toggle beside the underline's line style, driven through the
    //! real `decoration_section` on a headless app.

    use super::*;
    use crate::app::OndinApp;
    use ondin_core::Document;

    const AREA: egui::Rect = egui::Rect {
        min: egui::pos2(0.0, 0.0),
        max: egui::pos2(400.0, 400.0),
    };

    /// A headless app holding one text node whose defaults carry `attr`, and its id.
    fn app_with(attr: CharAttr) -> (egui::Context, OndinApp, NodeId) {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        let mut style = TextStyle {
            font_family: "Inter".into(),
            font_size: 20.0,
            ..TextStyle::default()
        };
        style.set(attr);
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: ondin_core::NodeKind::Text {
                content: "gypsy".into(),
                style: Box::new(style),
                spans: CharSpans::default(),
                para_spans: ParaSpans::default(),
                paragraph: ParagraphStyle::default(),
                block: BlockStyle::default(),
                sizing: TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("a text node");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        (ctx, app, id)
    }

    /// On an instance with size and alignment overridden, only those two are marked, and the size's reset writes the main's 20 and nothing else.
    ///
    /// flip: `style_mark` comparing the whole style → fails at "the family follows".
    #[test]
    fn the_type_card_marks_an_instances_own_fields_and_no_others() {
        let (_ctx, mut app, text) = app_with(CharAttr::Size(20.0));
        let group = app.session.ids.mint();
        let root = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: ondin_core::NodeKind::Group,
                transform: None,
                name: Some("Label".into()),
            },
            Operation::Reparent {
                id: text,
                new_parent: group,
                index: 0,
            },
            Operation::SetComponent {
                id: group,
                component: true,
            },
        ])));
        let inst = {
            let doc = &app.session.doc;
            let (tx, made) = ondin_core::insert_subtrees(
                doc,
                &mut app.session.ids,
                &[ondin_core::Placement {
                    nodes: doc.capture_subtree(group).unwrap(),
                    parent: root,
                    index: None,
                }],
                Default::default(),
            );
            assert!(app.session.commit(tx));
            made[0]
        };
        let copy = app.session.doc.get(inst).unwrap().children()[0];
        let (style, paragraph) = match app.session.doc.get(copy).unwrap().kind() {
            ondin_core::NodeKind::Text {
                style, paragraph, ..
            } => ((**style).clone(), paragraph.clone()),
            _ => unreachable!("a text"),
        };
        assert!(app.session.commit(Transaction(vec![
            Operation::SetTextStyle {
                id: copy,
                style: TextStyle {
                    font_size: 30.0,
                    ..style.clone()
                },
                spans: None,
            },
            Operation::SetParagraphStyle {
                id: copy,
                paragraph: ParagraphStyle {
                    align: TextAlign::Center,
                    ..paragraph
                },
                spans: None,
            },
        ])));
        app.session.selection.set_one(copy);
        app.gather_card_overrides();
        let marks = app.type_marks(copy);
        assert!(marks.family.is_none(), "the family follows");
        assert!(marks.line_height.is_none() && marks.valign.is_none());
        assert!(marks.align.is_some(), "the alignment is overridden");
        let size = marks.size.expect("the size is overridden");
        assert_eq!(size.tip, "Reset to main · 20");
        assert_eq!(
            size.tx,
            Transaction(vec![Operation::SetTextStyle {
                id: copy,
                style,
                spans: None,
            }]),
            "the main's size into the copy's own style, nothing else moved"
        );
    }

    /// The Content row marks a copy whose content is bound to a variable the main's is not; its reset leaves the copy unbound, drawing the main's string.
    ///
    /// flip: `content_mark` reading the copy's own binding (`let var = own_var`) → fails at the tip "Reset to main · copy/cta".
    #[test]
    fn the_content_row_marks_a_copys_bound_content() {
        use ondin_core::variable::{Cell, Mode, VarKind, Variable};
        let (_ctx, mut app, text) = app_with(CharAttr::Size(20.0));
        let group = app.session.ids.mint();
        let root = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: ondin_core::NodeKind::Group,
                transform: None,
                name: Some("Label".into()),
            },
            Operation::Reparent {
                id: text,
                new_parent: group,
                index: 0,
            },
            Operation::SetComponent {
                id: group,
                component: true,
            },
        ])));
        let inst = {
            let doc = &app.session.doc;
            let (tx, made) = ondin_core::insert_subtrees(
                doc,
                &mut app.session.ids,
                &[ondin_core::Placement {
                    nodes: doc.capture_subtree(group).unwrap(),
                    parent: root,
                    index: None,
                }],
                Default::default(),
            );
            assert!(app.session.commit(tx));
            made[0]
        };
        let copy = app.session.doc.get(inst).unwrap().children()[0];
        let drawn =
            |app: &OndinApp, id| match app.session.doc.resolved_kind(id).map(|k| k.into_owned()) {
                Some(ondin_core::NodeKind::Text { content, .. }) => content,
                _ => unreachable!("a text"),
            };
        let main_string = drawn(&app, text);
        assert!(!main_string.is_empty(), "the fixture's text has content");
        app.session.selection.set_one(copy);
        app.gather_card_overrides();
        assert!(app.type_marks(copy).content.is_none(), "unbound: no mark");

        let (mode, var) = (app.session.ids.mint_mode(), app.session.ids.mint_var());
        assert!(app.session.commit(Transaction(vec![
            Operation::AddMode {
                mode: Mode {
                    id: mode,
                    name: "Light".into(),
                },
                index: 0,
                cells: vec![],
            },
            Operation::AddVariable {
                var: Variable {
                    id: var,
                    name: "copy/cta".into(),
                    kind: VarKind::String,
                    cells: [(mode, Cell::String("Sign in".into()))].into(),
                    origin: None,
                    forked: None,
                    description: String::new(),
                },
            },
        ])));
        let vars = ondin_core::variable::text_vars_at(ondin_core::variable::TEXT_CONTENT, var);
        let ops = ondin_core::build::bind_text_ops(&app.session.doc, copy, vars);
        assert!(app.session.commit(Transaction(ops)));
        app.gather_card_overrides();
        let mark = app.type_marks(copy).content.expect("bound apart: marked");
        assert_eq!(mark.tip, format!("Reset to main · {main_string}"));
        assert!(app.session.commit(mark.tx));
        assert_eq!(drawn(&app, copy), main_string, "the main's string");
        assert_eq!(
            app.session.doc.get(copy).unwrap().text_vars()[0],
            None,
            "unbound"
        );
        app.gather_card_overrides();
        assert!(app.type_marks(copy).content.is_none(), "and unmarked");
    }

    /// A copy bound to a text style carries no mark on the style's fields; the alignment, which no text style holds, keeps its mark.
    ///
    /// flip: `type_marks` without its `styled` arm → fails at "the size is the style's".
    #[test]
    fn a_text_styled_copys_style_fields_carry_no_mark() {
        let (_ctx, mut app, text) = app_with(CharAttr::Size(20.0));
        let group = app.session.ids.mint();
        let root = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: ondin_core::NodeKind::Group,
                transform: None,
                name: Some("Label".into()),
            },
            Operation::Reparent {
                id: text,
                new_parent: group,
                index: 0,
            },
            Operation::SetComponent {
                id: group,
                component: true,
            },
        ])));
        let inst = {
            let doc = &app.session.doc;
            let (tx, made) = ondin_core::insert_subtrees(
                doc,
                &mut app.session.ids,
                &[ondin_core::Placement {
                    nodes: doc.capture_subtree(group).unwrap(),
                    parent: root,
                    index: None,
                }],
                Default::default(),
            );
            assert!(app.session.commit(tx));
            made[0]
        };
        let copy = app.session.doc.get(inst).unwrap().children()[0];
        let (style, paragraph) = match app.session.doc.get(copy).unwrap().kind() {
            ondin_core::NodeKind::Text {
                style, paragraph, ..
            } => ((**style).clone(), paragraph.clone()),
            _ => unreachable!("a text"),
        };
        assert!(app.session.commit(Transaction(vec![
            Operation::SetTextStyle {
                id: copy,
                style: TextStyle {
                    font_size: 30.0,
                    ..style
                },
                spans: None,
            },
            Operation::SetParagraphStyle {
                id: copy,
                paragraph: ParagraphStyle {
                    align: TextAlign::Center,
                    ..paragraph
                },
                spans: None,
            },
        ])));
        let s = app.session.ids.mint_style();
        let ops = ondin_core::style::create_ops(
            &app.session.doc,
            s,
            "Heading/H2",
            copy,
            StyleSlot::Text,
            &[copy],
        )
        .unwrap();
        assert!(app.session.commit(Transaction(ops)));
        assert_eq!(app.session.doc.get(copy).unwrap().styles().text, Some(s));
        app.session.selection.set_one(copy);
        app.gather_card_overrides();
        let marks = app.type_marks(copy);
        assert!(marks.size.is_none(), "the size is the style's");
        assert!(
            marks.align.is_some(),
            "the alignment is still the copy's own"
        );

        let old = app.unstyled_marks(copy).size.expect("the size differs");
        assert!(app.session.commit(old.tx));
        assert_eq!(
            app.session.doc.get(copy).unwrap().styles().text,
            None,
            "the old reset detached the style — D1038 (1)'s defect"
        );
    }

    /// One frame of the decoration section alone, with `events` delivered to it.
    fn frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        id: NodeId,
        events: Vec<egui::Event>,
    ) -> egui::FullOutput {
        let subject = TypeSubject::of(app, id).expect("a text node has a subject");
        let input = egui::RawInput {
            screen_rect: Some(AREA),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| {
            ui.set_width(MENU_INNER);
            app.decoration_section(ui, &subject);
        })
    }

    /// A stored out-of-range tracking (`Em(3.0)` against a 200% cap) is shown, not rewritten, on a frame with no events; an in-range control proves the field still reports changes.
    ///
    /// Two defences hold it, so no single flip reddens it: `value_field_f64`'s
    /// `.clamp_existing_to_range(false)` and `char_valve` committing nothing on an idle `changed()` frame.
    #[test]
    fn a_stored_tracking_outside_the_fields_range_is_not_rewritten_on_an_idle_frame() {
        fn tracking(app: &OndinApp, id: NodeId) -> Length {
            match app.session.doc.get(id).map(|n| n.kind()) {
                Some(ondin_core::NodeKind::Text { style, .. }) => style.letter_spacing,
                other => panic!("the fixture is a text node, got {other:?}"),
            }
        }

        for (stored, name) in [
            (Length::Em(3.0), "above the 200% cap"),
            (Length::Em(1.0), "well inside it — the control"),
        ] {
            let (ctx, mut app, id) = app_with(CharAttr::LetterSpacing(stored));
            assert_eq!(tracking(&app, id), stored, "fixture: {name}");
            let depth = app.session.history.undo_depth();

            // Two idle frames: the first could not commit what it had not yet drawn.
            for _ in 0..2 {
                let subject = TypeSubject::of(&app, id).expect("a subject");
                let input = egui::RawInput {
                    screen_rect: Some(AREA),
                    ..Default::default()
                };
                let _ = ctx.run_ui(input, |ui| {
                    ui.set_width(MENU_INNER);
                    app.type_letter_spacing_field(ui, &subject, egui::vec2(120.0, 28.0), None);
                });
            }

            assert_eq!(
                tracking(&app, id),
                stored,
                "{name}: a frame with no events must not rewrite the document"
            );
            assert_eq!(
                app.session.history.undo_depth(),
                depth,
                "{name}: and must not be an undo step"
            );
            assert!(
                !app.session.is_dirty(),
                "{name}: nor make the document dirty"
            );
        }
    }

    /// Where the section painted the skip-ink glyph, or `None` if it did not.
    fn toggle_ink(out: &egui::FullOutput) -> Option<egui::Rect> {
        out.shapes.iter().find_map(|c| match &c.shape {
            egui::Shape::Text(t) if t.galley.job.text == icon::LINK_SIMPLE_HORIZONTAL_BREAK => {
                Some(egui::Rect::from_min_size(t.pos, t.galley.size()))
            }
            _ => None,
        })
    }

    /// The smallest rectangle painted under `at`, with its fill: the button's box
    /// and ground, which is what `FieldButton::On` changes. `min_by` keeps the fill
    /// over the same-rect stroke `button_face` paints after it.
    fn button_box(out: &egui::FullOutput, at: egui::Pos2) -> Option<(egui::Rect, egui::Color32)> {
        out.shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Rect(r) if r.rect.contains(at) => Some(r),
                _ => None,
            })
            .min_by(|a, b| a.rect.area().total_cmp(&b.rect.area()))
            .map(|r| (r.rect, r.fill))
    }

    fn underline_of(app: &OndinApp, id: NodeId) -> Decoration {
        let subject = TypeSubject::of(app, id).expect("still a text node");
        decoration_of(&subject, CharAttrKind::Underline).expect("still underlined")
    }

    /// The toggle is there for an underline, lit while skipping is on, drawn in a different ground when off, and clicking it reaches the document.
    ///
    /// The grounds are sampled on the frame after each click, since `frame` reads the subject before running the UI.
    ///
    /// flips: `FieldButton::Off` passed unconditionally → fails at the two grounds being equal; `d.skip_ink = false` instead of negating → only the second click fails; the dropdown left at the full `MENU_INNER` → fails at the box inside the card.
    #[test]
    fn the_skip_ink_toggle_reports_its_state_and_writes_it() {
        let (ctx, mut app, id) = app_with(CharAttr::Underline(Some(Decoration::default())));

        let out = frame(&ctx, &mut app, id, Vec::new());
        let ink = toggle_ink(&out).expect("an underline gets the skip-ink toggle");
        let at = ink.center();
        let (rect, _) = button_box(&out, at).expect("the button paints a ground");
        assert!(
            rect.right() <= MENU_INNER + 0.5,
            "the whole row has to fit the card: the toggle's box is {rect:?} against \
             an inner edge of {MENU_INNER}"
        );
        assert!(
            underline_of(&app, id).skip_ink,
            "the fixture starts with skipping on, which is the default"
        );
        let out = frame(&ctx, &mut app, id, vec![egui::Event::PointerMoved(at)]);
        let lit = button_box(&out, at).expect("the button paints a ground").1;

        let _ = frame(&ctx, &mut app, id, click(at));
        assert!(
            !underline_of(&app, id).skip_ink,
            "the click has to reach the document, not only the panel"
        );
        let out = frame(&ctx, &mut app, id, vec![egui::Event::PointerMoved(at)]);
        let dark = button_box(&out, at)
            .expect("the button still paints a ground")
            .1;
        assert_ne!(
            lit, dark,
            "on and off have to look different or the toggle is invisible"
        );

        let _ = frame(&ctx, &mut app, id, click(at));
        assert!(
            underline_of(&app, id).skip_ink,
            "clicking it again turns skipping back on"
        );
        let d = underline_of(&app, id);
        assert_eq!(
            (d.thickness, d.offset, d.style, d.color),
            (None, None, LineStyle::Solid, None),
            "the toggle carried something it should not have"
        );
    }

    /// A strikethrough gets no skip-ink toggle, since `text::skip_ink` ignores the flag there.
    ///
    /// flip: dropping the `under.is_some()` guard → fails at the toggle being present.
    #[test]
    fn a_strikethrough_is_offered_no_skip_ink_toggle() {
        let (ctx, mut app, id) = app_with(CharAttr::Strikethrough(Some(Decoration::default())));
        let out = frame(&ctx, &mut app, id, Vec::new());
        let glyphs: Vec<&str> = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some(t.galley.job.text.as_str()),
                _ => None,
            })
            .collect();
        // The dropdown lays prefix and word out as one galley, hence `starts_with`.
        assert!(
            glyphs.iter().any(|g| g.starts_with(icon::WAVE_SINE)),
            "the fixture has to reach the line-style row for its absence to mean \
             anything: {glyphs:?}"
        );
        assert!(
            !glyphs.contains(&icon::LINK_SIMPLE_HORIZONTAL_BREAK),
            "a line-through was never skipped, so the control would be inert: \
             {glyphs:?}"
        );
    }

    fn click(at: egui::Pos2) -> Vec<egui::Event> {
        vec![
            egui::Event::PointerMoved(at),
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: true,
                modifiers: Default::default(),
            },
            egui::Event::PointerButton {
                pos: at,
                button: egui::PointerButton::Primary,
                pressed: false,
                modifiers: Default::default(),
            },
        ]
    }
}

#[cfg(test)]
mod family_repick_tests {
    //! Re-picking the family you are already in is a no-op, not a reset.
    //!
    //! Asserted on `family_actually_changed`; its `mixed` arm is the case a bare `!=`
    //! gets wrong.

    use super::family_actually_changed;

    /// flips: the body replaced with `chosen` → fails at the first assertion; `chosen.filter(|f| f != current)` (no `mixed` term) → only the fourth fails.
    #[test]
    fn a_repick_is_not_an_edit_and_a_mixed_repick_is() {
        let pick = |name: &str| Some(name.to_string());

        assert_eq!(
            family_actually_changed(pick("Inter"), "Inter", false),
            None,
            "the highlighted row: the family is unchanged, so nothing may be \
             written — the write would clear every axis and every feature"
        );
        assert_eq!(
            family_actually_changed(pick("Georgia"), "Inter", false),
            pick("Georgia"),
            "a different family is the edit the clearing was written for"
        );
        assert_eq!(
            family_actually_changed(None, "Inter", false),
            None,
            "and no click is no edit"
        );
        assert_eq!(
            family_actually_changed(pick("Inter"), "Inter", true),
            pick("Inter"),
            "but over a mixed selection `current` is one member's, so picking it \
             is the edit that makes the selection agree"
        );
    }
}

/// The variable-axis number field, driven through real pointer events; `undo_depth`
/// counts a gesture's commits.
#[cfg(test)]
mod axis_valve_tests {
    use super::*;
    use crate::app::OndinApp;
    use crate::theme;
    use ondin_core::{Document, NodeId, Operation, Transaction};

    /// `wght`, 100–900, defaulting to 400 — the axis every one of these drives.
    fn wght() -> FontAxis {
        FontAxis {
            tag: Tag::parse("wght").expect("a four-byte tag"),
            min: 100.0,
            default: 400.0,
            max: 900.0,
            name: "Weight".into(),
            hidden: false,
        }
    }

    /// A selected text node whose `wght` coordinate is already off the default (`403`).
    fn app_with_wght(value: f64) -> (egui::Context, OndinApp, NodeId, Vec<AxisSetting>) {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(3);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        let coords = vec![AxisSetting::new(wght().tag, value)];
        let style = TextStyle {
            font_family: "Inter".into(),
            font_size: 20.0,
            variations: coords.clone(),
            ..TextStyle::default()
        };
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: ondin_core::NodeKind::Text {
                content: "hello".into(),
                style: Box::new(style),
                spans: CharSpans::default(),
                para_spans: ParaSpans::default(),
                paragraph: ParagraphStyle::default(),
                block: BlockStyle::default(),
                sizing: TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("a text node");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        (ctx, app, id, coords)
    }

    /// The node's stored `wght`, or `None` when there is no coordinate at all.
    fn stored_wght(app: &OndinApp, id: NodeId) -> Option<f64> {
        let ondin_core::NodeKind::Text { style, .. } = app.session.doc.get(id)?.kind() else {
            panic!("a text node");
        };
        style
            .variations
            .iter()
            .find(|a| a.tag == wght().tag)
            .map(|a| a.value)
    }

    fn click(pos: egui::Pos2, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        }
    }

    /// One frame with the axis field alone in a `Ui`, returning the rect it drew in.
    fn frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        id: NodeId,
        resettable: bool,
        events: Vec<egui::Event>,
    ) -> egui::Rect {
        let subject = TypeSubject::of(app, id).expect("a subject");
        let coords = attr!(&subject, Variations, Variations);
        let mut out = egui::Rect::NOTHING;
        let _ = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(280.0, 600.0),
                )),
                events,
                ..Default::default()
            },
            |ui| {
                ui.set_width(240.0);
                out = ui
                    .scope(|ui| {
                        ui.horizontal(|ui| {
                            app.axis_field(ui, &subject, &wght(), &coords, 120.0, resettable);
                        });
                    })
                    .response
                    .rect;
            },
        );
        out
    }

    /// A prefix-strip scrub on an axis field is one undo step, not one per frame; the coordinate moving at all is the in-run control.
    ///
    /// flip: `axis_field`'s `axis_valve` call put back to the immediate `self.write_axis(..)` → fails at "a scrub is one undo step".
    #[test]
    fn a_prefix_strip_scrub_on_an_axis_field_is_one_undo_step() {
        let (ctx, mut app, id, _) = app_with_wght(403.0);
        let mut at = egui::Pos2::ZERO;
        for _ in 0..2 {
            at = frame(&ctx, &mut app, id, true, Vec::new()).center();
        }
        let depth0 = app.session.history.undo_depth();

        frame(
            &ctx,
            &mut app,
            id,
            true,
            vec![egui::Event::PointerMoved(at)],
        );
        frame(&ctx, &mut app, id, true, vec![click(at, true)]);
        for step in 1..=5 {
            let to = at + egui::vec2(step as f32 * 6.0, 0.0);
            frame(
                &ctx,
                &mut app,
                id,
                true,
                vec![egui::Event::PointerMoved(to)],
            );
        }
        let end = at + egui::vec2(30.0, 0.0);
        frame(&ctx, &mut app, id, true, vec![click(end, false)]);
        frame(&ctx, &mut app, id, true, Vec::new());
        frame(&ctx, &mut app, id, true, Vec::new());

        let moved = stored_wght(&app, id).expect("the coordinate is still there");
        assert_ne!(
            moved, 403.0,
            "the control: the scrub has to have moved the coordinate at all, or the \
             undo-depth assertion below is about a dead field"
        );
        assert_eq!(
            app.session.history.undo_depth() - depth0,
            1,
            "a scrub is one undo step, not one per frame of it"
        );
    }

    /// Typing the font's own value drops the coordinate, as the reset slot does; the same run at 500 keeps it.
    ///
    /// flip: `axis_valve`'s `clear_at_default` guard removed → fails at "typing the font's own value means no coordinate" (`Some(400.0)`); the 500 control stays green.
    #[test]
    fn typing_the_default_into_the_field_drops_the_coordinate_too() {
        for (typed, want, name) in [
            (400.0_f64, None, "the font's own value"),
            (500.0, Some(500.0), "a value that is not — the control"),
        ] {
            let (ctx, mut app, id, _) = app_with_wght(403.0);
            let mut at = egui::Pos2::ZERO;
            for _ in 0..2 {
                at = frame(&ctx, &mut app, id, true, Vec::new()).center();
            }
            frame(
                &ctx,
                &mut app,
                id,
                true,
                vec![egui::Event::PointerMoved(at)],
            );
            frame(&ctx, &mut app, id, true, vec![click(at, true)]);
            frame(&ctx, &mut app, id, true, vec![click(at, false)]);
            frame(&ctx, &mut app, id, true, Vec::new());
            for ch in format!("{typed:.0}").chars() {
                frame(&ctx, &mut app, id, true, vec![egui::Event::Text(ch.into())]);
            }
            let away = egui::pos2(5.0, 560.0);
            frame(&ctx, &mut app, id, true, vec![click(away, true)]);
            frame(&ctx, &mut app, id, true, vec![click(away, false)]);
            frame(&ctx, &mut app, id, true, Vec::new());
            frame(&ctx, &mut app, id, true, Vec::new());

            assert_eq!(
                stored_wght(&app, id),
                want,
                "typing {name} — `Some(400.0)` is the valve-shaped failure"
            );
        }
    }

    /// The reset slot writes immediately and drops the coordinate rather than writing the default (`Some(400.0)`).
    ///
    /// Coverage gap: removing the reset arm's `return` stays green; it guards a reset clicked during a scrub, which is not staged here.
    #[test]
    fn the_reset_slot_drops_the_coordinate_rather_than_writing_the_default() {
        let (ctx, mut app, id, _) = app_with_wght(403.0);
        let mut at = egui::Pos2::ZERO;
        for _ in 0..2 {
            at = frame(&ctx, &mut app, id, true, Vec::new()).center();
        }
        assert_eq!(
            stored_wght(&app, id),
            Some(403.0),
            "the fixture: there is a coordinate to drop"
        );

        let slot = egui::pos2(at.x + 76.0, at.y);
        frame(
            &ctx,
            &mut app,
            id,
            true,
            vec![egui::Event::PointerMoved(slot)],
        );
        frame(&ctx, &mut app, id, true, vec![click(slot, true)]);
        frame(&ctx, &mut app, id, true, vec![click(slot, false)]);
        frame(&ctx, &mut app, id, true, Vec::new());

        assert_eq!(
            stored_wght(&app, id),
            None,
            "reset means the coordinate is gone, not that it holds the font's own \
             value — `Some(400.0)` is the valve-shaped failure"
        );
    }

    /// An axis field over a selection whose runs disagree shows a dash; the agreeing half is the control.
    ///
    /// flip: dropping `mixed_text` from the formatter → fails at the dash.
    #[test]
    fn an_axis_field_over_two_disagreeing_runs_shows_a_dash() {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0x4A5);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        let style = TextStyle {
            font_family: "Inter".into(),
            font_size: 20.0,
            ..TextStyle::default()
        };
        let at = |v: f64| CharAttr::Variations(vec![AxisSetting::new(wght().tag, v)]);
        let mut spans = CharSpans::default();
        spans.set(0..5, at(300.0), &style);
        spans.set(5..11, at(800.0), &style);
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: ondin_core::NodeKind::Text {
                content: "hello world".into(),
                style: Box::new(style),
                spans,
                para_spans: ParaSpans::default(),
                paragraph: ParagraphStyle::default(),
                block: BlockStyle::default(),
                sizing: TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("a text node whose two words carry two weights");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        app.begin_edit_text(Some(id), None);

        /// Every galley the axis field paints for the current selection.
        fn drawn(ctx: &egui::Context, app: &mut OndinApp, id: NodeId) -> Vec<String> {
            let subject = TypeSubject::of(app, id).expect("a subject");
            let coords = attr!(&subject, Variations, Variations);
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(280.0, 600.0),
                    )),
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(240.0);
                    ui.horizontal(|ui| {
                        app.axis_field(ui, &subject, &wght(), &coords, 120.0, false);
                    });
                },
            );
            fn walk(s: &egui::Shape, v: &mut Vec<String>) {
                match s {
                    egui::Shape::Text(t) => v.push(t.galley.text().to_string()),
                    egui::Shape::Vec(x) => x.iter().for_each(|s| walk(s, v)),
                    _ => {}
                }
            }
            let mut v = Vec::new();
            for cs in &out.shapes {
                walk(&cs.shape, &mut v);
            }
            v
        }

        // Bytes 3..8 span the word boundary and hold both weights.
        {
            let editor = &mut app.text.as_mut().expect("a live session").editor;
            editor.select_all();
            editor.move_line_start(false);
            for _ in 0..3 {
                editor.move_right(false, false);
            }
            for _ in 0..5 {
                editor.move_right(true, false);
            }
        }
        let subject = TypeSubject::of(&app, id).expect("a subject");
        assert!(
            subject.partial,
            "the fixture: a sub-range, which is the only case a character control \
             can read mixed"
        );
        assert!(
            subject.mixed(CharAttrKind::Variations),
            "the fixture: and the two runs disagree about wght"
        );
        let mixed = drawn(&ctx, &mut app, id);
        assert!(
            mixed.iter().any(|t| t == "–"),
            "the field says it does not know, in {mixed:?}"
        );
        assert!(
            !mixed.iter().any(|t| t.contains("300")),
            "and does not name the first run's weight, in {mixed:?}"
        );

        // The control: a selection inside one run, where nothing disagrees.
        {
            let editor = &mut app.text.as_mut().expect("a live session").editor;
            editor.select_all();
            editor.move_line_start(false);
            for _ in 0..4 {
                editor.move_right(true, false);
            }
        }
        let one_run = TypeSubject::of(&app, id).expect("a subject");
        assert!(
            one_run.partial && !one_run.mixed(CharAttrKind::Variations),
            "the control: a sub-range inside one run agrees with itself"
        );
        let agreed = drawn(&ctx, &mut app, id);
        assert!(
            !agreed.iter().any(|t| t == "–"),
            "so the field names its number, in {agreed:?}"
        );
        assert!(
            agreed.iter().any(|t| t.contains("300")),
            "and it is that run's weight, in {agreed:?}"
        );
    }
}

#[cfg(test)]
mod bare_caret_readout_tests {
    //! A bare caret's character fields describe the run typing will inherit; the
    //! write still sets the node's default.

    use super::*;
    use crate::app::OndinApp;
    use crate::theme;
    use ondin_core::{Document, NodeId, Operation, Transaction};

    /// `"hello world"` with `Size(40)` over its first five bytes and a node default of 16.
    fn app_with_ragged_text() -> (egui::Context, OndinApp, NodeId) {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0x51E);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        let style = TextStyle {
            font_family: "Inter".into(),
            font_size: 16.0,
            ..TextStyle::default()
        };
        let mut spans = CharSpans::default();
        spans.set(0..5, CharAttr::Size(40.0), &style);
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: ondin_core::NodeKind::Text {
                content: "hello world".into(),
                style: Box::new(style),
                spans,
                para_spans: ParaSpans::default(),
                paragraph: ParagraphStyle::default(),
                block: BlockStyle::default(),
                sizing: TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("a text node");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        (ctx, app, id)
    }

    /// The size the Size field would show with the caret at `at`.
    fn shown_size(app: &mut OndinApp, id: NodeId, at: usize) -> f64 {
        let editor = &mut app.text.as_mut().expect("a live session").editor;
        editor.select_all();
        editor.move_line_start(false);
        for _ in 0..at {
            editor.move_right(false, false);
        }
        assert_eq!(
            editor.selected_range(),
            at..at,
            "the fixture: the caret must be a bare caret at {at}"
        );
        let subject = TypeSubject::of(app, id).expect("a subject");
        assert!(
            !subject.partial,
            "the fixture: a bare caret is not partial — that is the designed half"
        );
        match subject.shown(CharAttrKind::Size) {
            CharAttr::Size(v) => v,
            other => panic!("the Size field showed {other:?}"),
        }
    }

    /// The field shows what the next keystroke will produce: inside the styled run, at its boundary (`Spans::edited` joins the run ending there), past it, and at byte 0.
    ///
    /// flip: `resolve_at` returning `Some(self.range.start)` for every case → fails at "the caret is in 16pt text" (40 against 16).
    #[test]
    fn a_bare_caret_reads_the_run_typing_will_join() {
        let (_ctx, mut app, id) = app_with_ragged_text();
        app.begin_edit_text(Some(id), None);

        assert_eq!(
            shown_size(&mut app, id, 11),
            16.0,
            "the caret is in 16pt text and the field named the 40pt run at byte 0"
        );
        assert_eq!(
            shown_size(&mut app, id, 5),
            40.0,
            "at the boundary an insertion joins the run that ends there, so the \
             field must read 40"
        );
        assert_eq!(
            shown_size(&mut app, id, 3),
            40.0,
            "and inside the run it reads the run"
        );
        assert_eq!(
            shown_size(&mut app, id, 0),
            16.0,
            "at byte 0 there is nothing to the left, and an insertion there takes \
             the node's defaults"
        );
    }

    /// Control: a selection still reads its own start, and a node with no session still reads byte 0.
    #[test]
    fn a_selection_and_an_unedited_node_are_unmoved() {
        let (_ctx, mut app, id) = app_with_ragged_text();

        let subject = TypeSubject::of(&app, id).expect("a subject");
        assert_eq!(subject.resolve_at, Some(0));
        assert!(matches!(
            subject.shown(CharAttrKind::Size),
            CharAttr::Size(v) if v == 40.0
        ));

        app.begin_edit_text(Some(id), None);
        let editor = &mut app.text.as_mut().expect("a live session").editor;
        editor.select_all();
        editor.move_line_start(false);
        for _ in 0..3 {
            editor.move_right(false, false);
        }
        for _ in 0..5 {
            editor.move_right(true, false);
        }
        let subject = TypeSubject::of(&app, id).expect("a subject");
        assert_eq!(subject.range, 3..8, "the fixture: a ragged selection");
        assert!(subject.partial);
        assert_eq!(
            subject.resolve_at,
            Some(3),
            "a selection resolves at its own start, as it always did"
        );
        assert!(matches!(
            subject.shown(CharAttrKind::Size),
            CharAttr::Size(v) if v == 40.0
        ));
        assert!(
            subject.mixed(CharAttrKind::Size),
            "and a ragged selection can still say so, which is the thing a bare \
             caret structurally cannot"
        );
    }
}

#[cfg(test)]
mod type_popup_readout_tests {
    //! Two Character-tab controls: the *Show all features* offer counts only rows
    //! the click adds (the search needle applies), and the Language field reads its
    //! name, not its tag.
    //!
    //! `features_section` is driven with a hand-built `&[FaceFeature]`, since
    //! `text::family_features` is empty headlessly.

    use super::*;
    use crate::app::OndinApp;
    use ondin_core::NodeId;

    /// A face offering `liga`, plus 43 withheld private features (`Zk01`..`Zk43`;
    /// `cvNN` tags would be offered).
    fn a_face_with_43_withheld() -> Vec<FaceFeature> {
        let mut out = vec![FaceFeature {
            tag: Tag::parse("liga").unwrap(),
            name: None,
            tooltip: None,
            values: Vec::new(),
        }];
        for i in 1..=43 {
            out.push(FaceFeature {
                tag: Tag::parse(&format!("Zk{i:02}")).unwrap(),
                name: None,
                tooltip: None,
                values: Vec::new(),
            });
        }
        out
    }

    /// A headless app holding one plain text node, and its id.
    fn app_with_gypsy() -> (egui::Context, OndinApp, NodeId) {
        app_with_text_of("gypsy")
    }

    /// The reveal's text this frame, if it drew one.
    fn reveal(
        ctx: &egui::Context,
        app: &mut OndinApp,
        id: NodeId,
        available: &[FaceFeature],
    ) -> Option<String> {
        let subject = TypeSubject::of(app, id).expect("a text node has a subject");
        let out = ctx.run_ui(
            egui::RawInput {
                screen_rect: Some(TEST_AREA),
                ..Default::default()
            },
            |ui| {
                ui.set_width(MENU_INNER);
                app.features_section(ui, &subject, available);
            },
        );
        out.shapes.iter().find_map(|c| match &c.shape {
            egui::epaint::Shape::Text(t)
                if t.galley.job.text.starts_with("Show all features")
                    || t.galley.job.text == "Show fewer features" =>
            {
                Some(t.galley.job.text.clone())
            }
            _ => None,
        })
    }

    /// With a needle only an offered feature matches, the reveal is absent, since clicking it would add nothing.
    ///
    /// flip: counting `hidden` over `!shown(t, l)` alone → fails the second assertion (`Some("Show all features (43 more)")`).
    #[test]
    fn a_needle_that_hides_every_withheld_feature_hides_the_offer() {
        let (ctx, mut app, id) = app_with_gypsy();
        let available = a_face_with_43_withheld();

        assert_eq!(
            reveal(&ctx, &mut app, id, &available).as_deref(),
            Some("Show all features (43 more)"),
            "the fixture must reach the state: 43 withheld features and an offer to show them"
        );

        app.feature_filter = "liga".into();
        assert_eq!(
            reveal(&ctx, &mut app, id, &available),
            None,
            "with a needle no withheld feature matches, the offer would add nothing and is absent"
        );
    }

    /// The closed Language field reads the locale's name, as the open list does; an unlisted locale falls back to its tag, not to None.
    ///
    /// flip: `current.clone().unwrap_or_else(…)` restored → fails at the second assertion ("German" wanted, `de` read).
    #[test]
    fn the_language_field_reads_its_name_rather_than_its_tag() {
        let (ctx, mut app, id) = app_with_gypsy();

        let field = |app: &mut OndinApp, locale: Option<&str>| -> Vec<String> {
            app.apply_char_attrs(
                &TypeSubject::of(app, id).expect("a subject"),
                vec![CharAttr::Locale(locale.map(str::to_string))],
            );
            let subject = TypeSubject::of(app, id).expect("a subject");
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(TEST_AREA),
                    ..Default::default()
                },
                |ui| {
                    ui.set_width(MENU_INNER);
                    app.language_section(ui, &subject);
                },
            );
            out.shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::epaint::Shape::Text(t) => Some(t.galley.job.text.clone()),
                    _ => None,
                })
                .collect()
        };

        let said = field(&mut app, None);
        assert!(
            said.iter().any(|t| t.contains("None")),
            "the fixture must reach the state: the closed field is drawn at all: {said:?}"
        );

        let said = field(&mut app, Some("de"));
        assert!(
            said.iter().any(|t| t.contains("German")),
            "the field names the language, not its tag: {said:?}"
        );
        assert!(
            !said
                .iter()
                .any(|t| t.contains("de") && !t.contains("German")),
            "and the raw tag is nowhere on it: {said:?}"
        );

        let said = field(&mut app, Some("qxx-Zzzz"));
        assert!(
            said.iter().any(|t| t.contains("qxx-Zzzz")),
            "a locale the table does not offer falls back to itself — it is a \
             real setting and must not read as None: {said:?}"
        );
    }

    /// A needle that matches withheld rows still offers them, at the number it would add.
    ///
    /// flip: the `!shown(t, l)`-only count → fails with "(43 more)" against "(4 more)".
    #[test]
    fn a_needle_that_matches_withheld_features_offers_exactly_those() {
        let (ctx, mut app, id) = app_with_gypsy();
        let available = a_face_with_43_withheld();

        app.feature_filter = "zk4".into();
        assert_eq!(
            reveal(&ctx, &mut app, id, &available).as_deref(),
            Some("Show all features (4 more)"),
            "four withheld features survive this needle, and four is what the click adds"
        );
    }
}

#[cfg(test)]
mod feature_mixed_tests {
    //! The OpenType section over a selection whose runs disagree: a feature row
    //! reads "Mixed" per tag (`TypeSubject::feature_mixed`) and a toggle writes only
    //! the tag it names.

    use super::*;
    use crate::app::OndinApp;
    use crate::theme;
    use ondin_core::{Document, NodeId, Operation, Transaction};

    fn tag(s: &str) -> Tag {
        Tag::parse(s).expect("a four-byte tag")
    }

    /// `"alpha bravo"` with `liga 1` over the whole node and `tnum 1` over bytes 6..11:
    /// the runs agree about `liga` and disagree about `tnum`.
    fn app_with_two_feature_runs() -> (egui::Context, OndinApp, NodeId) {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0x0F7);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        let style = TextStyle {
            font_family: "Inter".into(),
            font_size: 16.0,
            ..TextStyle::default()
        };
        let mut spans = CharSpans::default();
        spans.set(
            0..11,
            CharAttr::Features(vec![FeatureSetting {
                tag: tag("liga"),
                value: 1,
            }]),
            &style,
        );
        spans.set(
            6..11,
            CharAttr::Features(vec![
                FeatureSetting {
                    tag: tag("liga"),
                    value: 1,
                },
                FeatureSetting {
                    tag: tag("tnum"),
                    value: 1,
                },
            ]),
            &style,
        );
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: ondin_core::NodeKind::Text {
                content: "alpha bravo".into(),
                style: Box::new(style),
                spans,
                para_spans: ParaSpans::default(),
                paragraph: ParagraphStyle::default(),
                block: BlockStyle::default(),
                sizing: TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("a text node");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        (ctx, app, id)
    }

    /// Open a session and select everything.
    fn select_all_in_a_session(app: &mut OndinApp, id: NodeId) {
        app.begin_edit_text(Some(id), None);
        app.text
            .as_mut()
            .expect("a live session")
            .editor
            .select_all();
    }

    /// The feature value each *run* of the node carries for `t`, in order.
    fn per_run(app: &OndinApp, id: NodeId, t: Tag) -> Vec<u16> {
        let node = app.session.doc.get(id).expect("the node");
        let ondin_core::NodeKind::Text { spans, style, .. } = node.kind() else {
            panic!("a text node")
        };
        spans
            .runs_of(style, CharAttrKind::Features, 0..11)
            .into_iter()
            .map(|(_, attr)| match attr {
                CharAttr::Features(list) => list.iter().find(|f| f.tag == t).map_or(0, |f| f.value),
                _ => 0,
            })
            .collect()
    }

    /// A tag the runs disagree about is mixed; one they agree about is not.
    ///
    /// flip: `feature_mixed` returning `self.mixed(CharAttrKind::Features)` → fails at the `liga` assertion; `tnum` stays green.
    #[test]
    fn a_tag_is_mixed_only_where_the_runs_disagree_about_that_tag() {
        let (_ctx, mut app, id) = app_with_two_feature_runs();
        select_all_in_a_session(&mut app, id);
        let subject = TypeSubject::of(&app, id).expect("a subject");

        assert!(
            subject.partial,
            "fixture: a non-empty selection, or `mixed` is structurally false"
        );
        assert!(
            subject.mixed(CharAttrKind::Features),
            "fixture: the range really does disagree about its feature list"
        );
        assert!(
            subject.feature_mixed(tag("tnum")),
            "tnum is on in the second run and absent in the first"
        );
        assert!(
            !subject.feature_mixed(tag("liga")),
            "liga is 1 in both runs — a per-attribute answer would call this mixed \
             too, and mark every row in the list"
        );
    }

    /// Toggling one tag leaves every other tag in every run alone.
    ///
    /// flip: `write_feature_tag`'s partial arm replaced by `apply_char_attrs(subject, vec![CharAttr::Features(merge(&set))])` → fails at `tnum` (`[0]` for `[0, 0]`: the two runs collapse into one).
    #[test]
    fn toggling_one_feature_leaves_the_other_runs_features_alone() {
        let (_ctx, mut app, id) = app_with_two_feature_runs();
        select_all_in_a_session(&mut app, id);
        let subject = TypeSubject::of(&app, id).expect("a subject");

        assert_eq!(
            per_run(&app, id, tag("tnum")),
            vec![0, 1],
            "fixture: the second run carries tnum and the first does not"
        );

        // Spans live in the editor until the session closes.
        app.write_feature_tag(&subject, tag("liga"), 0);
        app.finish_text_edit();

        assert_eq!(
            per_run(&app, id, tag("tnum")),
            vec![0, 1],
            "the second run still carries tnum — the tag nobody named"
        );
        assert_eq!(
            per_run(&app, id, tag("liga")),
            vec![0, 0],
            "control: and the tag that was named really did change in both runs"
        );
    }
}

#[cfg(test)]
mod valve_arm_tests {
    //! `char_valve` has two arms, and a held tracking chord stops where its field stops.
    use super::*;
    use crate::app::OndinApp;

    /// A control reporting `changed()` while unfocused and undragged commits nothing.
    /// The fixture is a ranged `DragValue` without the app's `clamp_existing_to_range(false)`
    /// opt-out, so egui's clamp reports a change.
    ///
    /// flip: restoring the `else if resp.changed() && !resp.lost_focus()` arm → fails at "no undo step" (1 against 0).
    #[test]
    fn a_changed_frame_with_no_engagement_commits_nothing() {
        let (ctx, mut app, id) = app_with_text();
        let before = app.session.history.undo_depth();

        let mut out_of_range = 5000.0_f64;
        let mut states = Vec::new();
        for _ in 0..3 {
            let subject = TypeSubject::of(&app, id).expect("a subject");
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(TEST_AREA),
                    ..Default::default()
                },
                |ui| {
                    let resp = ui.add(egui::DragValue::new(&mut out_of_range).range(
                        ondin_core::typography::MIN_FONT_SIZE
                            ..=ondin_core::typography::MAX_FONT_SIZE,
                    ));
                    states.push((resp.changed(), resp.has_focus(), resp.dragged()));
                    app.char_valve(&resp, &subject, CharAttr::Size(out_of_range));
                },
            );
        }

        assert!(
            states
                .iter()
                .any(|(changed, focused, dragged)| *changed && !focused && !dragged),
            "the fixture must reach the state the arm fired on, on some frame: {states:?}"
        );
        assert_eq!(
            app.session.history.undo_depth(),
            before,
            "a control nobody touched spends no undo step"
        );
        let size = match app.session.doc.get(id).expect("the node").kind() {
            ondin_core::NodeKind::Text { style, .. } => style.font_size,
            other => panic!("the fixture is a text node, not {other:?}"),
        };
        assert_eq!(size, 20.0, "and does not write its rewritten value either");
    }

    /// `Ctrl+B` and `Ctrl+I` in a session on bundled Inter reach its Bold and
    /// Italic faces and come back.
    ///
    /// flip: the `Bold` arm of `text_chord` calling `toggle_face(.., false)` →
    /// fails at "Ctrl+B makes it bold".
    #[test]
    fn bold_and_italic_chords_reach_the_familys_faces_and_back() {
        let (_ctx, mut app, id) = app_with_text();
        app.begin_edit_text(Some(id), None);
        assert!(
            app.text.as_ref().is_some_and(|s| s.id == id),
            "a live session"
        );
        let face = |app: &OndinApp| {
            let subject = TypeSubject::of(app, id).expect("a subject");
            (
                attr!(subject, Weight, Weight),
                attr!(subject, Italic, Italic),
            )
        };
        assert_eq!(face(&app), (400, false), "the fixture: Inter Regular");

        app.text_chord(TextChord::Bold);
        assert_eq!(face(&app), (700, false), "Ctrl+B makes it bold");
        app.text_chord(TextChord::Italic);
        assert_eq!(face(&app), (700, true), "Ctrl+I keeps the weight");
        app.text_chord(TextChord::Bold);
        assert_eq!(face(&app), (400, true), "and Ctrl+B again un-bolds");
        app.text_chord(TextChord::Italic);
        assert_eq!(face(&app), (400, false));
    }

    /// **Italic over a range of two weights keeps each weight, and Bold over a
    /// range of two slopes keeps each slope**: "hello" 400 and "world" 700.
    ///
    /// flip: `toggle_face` writing the whole variant over a ragged range (the
    /// `ragged` arms dropped) → fails at *"world stays bold"*.
    #[test]
    fn bold_and_italic_over_a_mixed_range_keep_the_other_attribute() {
        let (_ctx, mut app, id) = app_with_text_of("hello world");
        let subject = TypeSubject::of(&app, id).expect("a subject");
        let mut spans = subject.spans.clone();
        spans.set(6..11, CharAttr::Weight(700), &subject.style);
        app.session
            .commit(Transaction(vec![Operation::SetTextSpans { id, spans }]));
        app.begin_edit_text(Some(id), None);
        assert!(
            app.text.as_ref().is_some_and(|s| s.id == id),
            "a live session"
        );
        let at = |app: &OndinApp, byte: usize| {
            let s = TypeSubject::of(app, id).expect("a subject");
            let attrs = s.spans.resolve(&s.style, byte);
            (
                attrs.get(CharAttrKind::Weight),
                attrs.get(CharAttrKind::Italic),
            )
        };
        assert_eq!(
            at(&app, 7).0,
            CharAttr::Weight(700),
            "fixture: world is bold"
        );
        app.text_chord(TextChord::Italic);
        assert_eq!(
            at(&app, 7),
            (CharAttr::Weight(700), CharAttr::Italic(true)),
            "world stays bold"
        );
        assert_eq!(
            at(&app, 1),
            (CharAttr::Weight(400), CharAttr::Italic(true)),
            "hello stays regular"
        );

        let (_ctx, mut app, id) = app_with_text_of("hello world");
        let subject = TypeSubject::of(&app, id).expect("a subject");
        let mut spans = subject.spans.clone();
        spans.set(6..11, CharAttr::Italic(true), &subject.style);
        app.session
            .commit(Transaction(vec![Operation::SetTextSpans { id, spans }]));
        app.begin_edit_text(Some(id), None);
        app.text_chord(TextChord::Bold);
        let at = |app: &OndinApp, byte: usize| {
            let s = TypeSubject::of(app, id).expect("a subject");
            let attrs = s.spans.resolve(&s.style, byte);
            (
                attrs.get(CharAttrKind::Weight),
                attrs.get(CharAttrKind::Italic),
            )
        };
        assert_eq!(
            at(&app, 7),
            (CharAttr::Weight(700), CharAttr::Italic(true)),
            "world stays italic"
        );
        assert_eq!(
            at(&app, 1),
            (CharAttr::Weight(700), CharAttr::Italic(false)),
            "hello stays upright"
        );
    }

    /// A held `Alt`+`→` stops at `MAX_TRACKING_PCT` and `Alt`+`←` at the min, in px and em.
    /// The bound sits at the chord arm, not in `TextStyle::set`: a file may hold any finite value.
    ///
    /// flip: the `(Em, Em)` arm of `OndinApp::text_chord` returning the stepped value instead of `stepped_into` → fails at "the em face stops at the same percentage" (`Em(4.0)` for `Em(2.0)`); the px assertions do not bite.
    #[test]
    fn a_held_tracking_chord_stops_at_the_fields_cap() {
        let (_ctx, mut app, id) = app_with_text();
        app.begin_edit_text(Some(id), None);
        assert!(
            app.text.as_ref().map(|s| s.id) == Some(id),
            "the fixture must reach the state: `text_chord` returns early with no \
             live session, so without this the test would be about nothing"
        );

        // With a live session the write lands in the editor, not the node.
        let tracking = |app: &OndinApp| {
            let subject = TypeSubject::of(app, id).expect("a subject");
            match subject.shown(CharAttrKind::LetterSpacing) {
                CharAttr::LetterSpacing(l) => l,
                other => panic!("the letter-spacing slot answered {other:?}"),
            }
        };

        let px_cap = px_range_for(MIN_TRACKING_PCT..=MAX_TRACKING_PCT, 20.0);
        assert!(
            matches!(tracking(&app), Length::Px(v) if v == 0.0),
            "the fixture must reach the state: tracking starts at Px(0), not {:?}",
            tracking(&app)
        );

        for _ in 0..400 {
            app.text_chord(TextChord::Tracking(1));
        }
        assert_eq!(
            tracking(&app),
            Length::Px(*px_cap.end()),
            "a held key stops at the cap the field stops at"
        );
        for _ in 0..800 {
            app.text_chord(TextChord::Tracking(-1));
        }
        assert_eq!(
            tracking(&app),
            Length::Px(*px_cap.start()),
            "and at the other end, which is not the negation of the first"
        );

        let (_ctx, mut app, id) = app_with_text();
        app.begin_edit_text(Some(id), None);
        let subject = TypeSubject::of(&app, id).expect("a subject");
        app.apply_char_attrs(&subject, vec![CharAttr::LetterSpacing(Length::Em(0.0))]);
        assert!(
            matches!(tracking(&app), Length::Em(_)),
            "the fixture must reach the state: tracking is in em now, not {:?}",
            tracking(&app)
        );
        for _ in 0..400 {
            app.text_chord(TextChord::Tracking(1));
        }
        assert_eq!(
            tracking(&app),
            Length::Em(MAX_TRACKING_PCT / 100.0),
            "the em face stops at the same percentage"
        );
        for _ in 0..800 {
            app.text_chord(TextChord::Tracking(-1));
        }
        assert_eq!(
            tracking(&app),
            Length::Em(MIN_TRACKING_PCT / 100.0),
            "and at the em floor, which is not the negation of the em cap"
        );

        let (_ctx, mut app, id) = app_with_text();
        app.begin_edit_text(Some(id), None);
        app.text_chord(TextChord::Tracking(1));
        assert_eq!(
            tracking(&app),
            Length::Px(TRACKING_STEP_PX).canonical(),
            "control: an ordinary press still steps by one step"
        );
    }

    /// A held leading chord stops at the field's floor and at its px and em ceilings; zero is one press from the seeded value.
    ///
    /// flips: `stepped_into` replaced by the bare `step_length` → fails at the floor (negative line height); `Bounds::em` returning the `%` range undivided → fails at the em ceiling (`Em(21.0)` for `Em(10.0)`).
    #[test]
    fn a_held_leading_chord_stops_at_the_fields_ends() {
        let (_ctx, mut app, id) = app_with_text();
        app.begin_edit_text(Some(id), None);
        let leading = |app: &OndinApp| {
            let subject = TypeSubject::of(app, id).expect("a subject");
            match subject.shown(CharAttrKind::LineHeight) {
                CharAttr::LineHeight(v) => v,
                other => panic!("the line-height slot answered {other:?}"),
            }
        };

        for _ in 0..600 {
            app.text_chord(TextChord::Leading(-1));
        }
        let at_floor = leading(&app).expect("the chord writes a line height");
        let px_floor = px_range_for(MIN_LINE_HEIGHT_PCT..=MAX_LINE_HEIGHT_PCT, 20.0);
        assert!(
            match at_floor {
                Length::Px(v) => v >= *px_floor.start(),
                Length::Em(v) => v >= MIN_LINE_HEIGHT_PCT / 100.0,
            },
            "a held key must not walk line height below the field's floor, got \
             {at_floor:?}"
        );

        let before = leading(&app);
        app.text_chord(TextChord::Leading(1));
        assert_ne!(
            leading(&app),
            before,
            "control: the chord still moves away from the floor"
        );

        // From the floor, 400 presses of `LEADING_STEP_PX` pass the px cap at 20pt (200).
        assert!(
            matches!(leading(&app), Some(Length::Px(_))),
            "the fixture must reach the state: the floor left line height in px, \
             not {:?}",
            leading(&app)
        );
        for _ in 0..400 {
            app.text_chord(TextChord::Leading(1));
        }
        assert_eq!(
            leading(&app),
            Some(Length::Px(*Bounds::LINE_HEIGHT.px(20.0).end())),
            "a held key stops at the field's px ceiling"
        );
        let subject = TypeSubject::of(&app, id).expect("a subject");
        app.apply_char_attrs(&subject, vec![CharAttr::LineHeight(Some(Length::Em(1.0)))]);
        assert!(
            matches!(leading(&app), Some(Length::Em(_))),
            "the fixture must reach the state: line height is in em now, not {:?}",
            leading(&app)
        );
        for _ in 0..200 {
            app.text_chord(TextChord::Leading(1));
        }
        assert_eq!(
            leading(&app),
            Some(Length::Em(MAX_LINE_HEIGHT_PCT / 100.0)),
            "the em face stops at the same percentage"
        );
    }

    /// A chord does not rewrite a stored out-of-range tracking: `Alt`+`→` on a stored `Em(5.0)` leaves it, `Alt`+`←` walks it back toward the range.
    ///
    /// flips: an unconditional clamp → fails at the second assertion (`Em(2.0)` for `Em(5.0)`); skipping the clamp when the value starts out of range → also fails there (`Em(5.01)`). The third assertion is reached by neither.
    #[test]
    fn a_chord_does_not_rewrite_a_stored_out_of_range_tracking() {
        let (_ctx, mut app, id) = app_with_text();
        app.begin_edit_text(Some(id), None);
        let tracking = |app: &OndinApp| {
            let subject = TypeSubject::of(app, id).expect("a subject");
            match subject.shown(CharAttrKind::LetterSpacing) {
                CharAttr::LetterSpacing(l) => l,
                other => panic!("the letter-spacing slot answered {other:?}"),
            }
        };

        let stored = Length::Em(5.0);
        let subject = TypeSubject::of(&app, id).expect("a subject");
        app.apply_char_attrs(&subject, vec![CharAttr::LetterSpacing(stored)]);
        assert_eq!(
            tracking(&app),
            stored,
            "the fixture must reach the state: a value well outside the field's \
             range, which D425 says the panel shows rather than rewrites"
        );

        app.text_chord(TextChord::Tracking(1));
        assert_eq!(
            tracking(&app),
            stored,
            "the increase key must not drag a stored 500% down to the cap — it \
             asked for more and there is no more, so nothing moves"
        );

        app.text_chord(TextChord::Tracking(-1));
        assert!(
            matches!(tracking(&app), Length::Em(v) if v < 5.0),
            "and the decrease key walks it back toward the range, which is the \
             direction that has somewhere to go"
        );

        for _ in 0..1200 {
            app.text_chord(TextChord::Tracking(-1));
        }
        assert_eq!(
            tracking(&app),
            Length::Em(MIN_TRACKING_PCT / 100.0),
            "and having come back into range it stops at the field's floor like \
             any other value — the widening is only ever by the value it started \
             with"
        );
    }

    /// The tracking test's twin for the leading chord, in both units: a stored `Em(15.0)` or `Px(300.0)` at 20pt is left alone by `Alt`+`↑` and walked back by `Alt`+`↓`.
    ///
    /// flips: a bare `clamp` on the `(Em, Em)` arm → fails at "the increase key leaves a stored 1500%"; on the `(Px, Px)` arm → fails at "and a stored 300px".
    #[test]
    fn a_chord_does_not_rewrite_a_stored_out_of_range_leading() {
        let (_ctx, mut app, id) = app_with_text();
        app.begin_edit_text(Some(id), None);
        let leading = |app: &OndinApp| {
            let subject = TypeSubject::of(app, id).expect("a subject");
            match subject.shown(CharAttrKind::LineHeight) {
                CharAttr::LineHeight(v) => v,
                other => panic!("the line-height slot answered {other:?}"),
            }
        };
        let store = |app: &mut OndinApp, v: Length| {
            let subject = TypeSubject::of(app, id).expect("a subject");
            app.apply_char_attrs(&subject, vec![CharAttr::LineHeight(Some(v))]);
        };

        store(&mut app, Length::Em(15.0));
        assert_eq!(
            leading(&app),
            Some(Length::Em(15.0)),
            "the fixture must reach the state: a line height well outside the \
             field's range"
        );
        app.text_chord(TextChord::Leading(1));
        assert_eq!(
            leading(&app),
            Some(Length::Em(15.0)),
            "the increase key leaves a stored 1500% — it asked for more and \
             there is no more"
        );
        app.text_chord(TextChord::Leading(-1));
        assert!(
            matches!(leading(&app), Some(Length::Em(v)) if v < 15.0 && v > 10.0),
            "and the decrease key walks it one step back toward the range, got {:?}",
            leading(&app)
        );

        let cap = *Bounds::LINE_HEIGHT.px(20.0).end();
        assert_eq!(cap, 200.0, "the fixture: 1000% of 20pt");
        store(&mut app, Length::Px(300.0));
        app.text_chord(TextChord::Leading(1));
        assert_eq!(
            leading(&app),
            Some(Length::Px(300.0)),
            "and a stored 300px is left alone by the increase key too"
        );
        app.text_chord(TextChord::Leading(-1));
        assert!(
            matches!(leading(&app), Some(Length::Px(v)) if v < 300.0 && v > cap),
            "and walked one step back, got {:?}",
            leading(&app)
        );
    }
}

#[cfg(test)]
mod wrap_gate_tests {
    //! Word break and Long words are dead under `NoWrap`.
    use super::*;
    use crate::app::OndinApp;
    use crate::theme;
    use ondin_core::{Document, IdSource, NodeId, Operation, Transaction};

    fn app_wrapping(wrap: WrapMode) -> (egui::Context, OndinApp, NodeId) {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: ondin_core::NodeKind::Text {
                content: "gypsy".into(),
                style: Box::default(),
                spans: CharSpans::default(),
                para_spans: ParaSpans::default(),
                paragraph: ParagraphStyle {
                    wrap,
                    ..ParagraphStyle::default()
                },
                block: BlockStyle::default(),
                sizing: TextSizing::Auto,
                on_path: None,
                on_path_flip: false,
                on_path_offset: 0.0,
            },
            transform: None,
            name: None,
        }]))
        .expect("a text node");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        (ctx, app, id)
    }

    fn frame(ctx: &egui::Context, app: &mut OndinApp, id: NodeId, events: Vec<egui::Event>) {
        let subject = TypeSubject::of(app, id).expect("a text node has a subject");
        let input = egui::RawInput {
            screen_rect: Some(TEST_AREA),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| {
            ui.set_width(MENU_INNER);
            app.wrap_section(ui, &subject);
        });
    }

    fn para(app: &OndinApp, id: NodeId) -> ParagraphStyle {
        match app.session.doc.get(id).map(|n| n.kind()) {
            Some(ondin_core::NodeKind::Text { paragraph, .. }) => paragraph.clone(),
            other => panic!("the fixture is a text node, got {other:?}"),
        }
    }

    fn click(ctx: &egui::Context, app: &mut OndinApp, id: NodeId, at: egui::Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(ctx, app, id, Vec::new());
        frame(ctx, app, id, vec![egui::Event::PointerMoved(at)]);
        frame(ctx, app, id, vec![button(true)]);
        frame(ctx, app, id, vec![button(false)]);
        frame(ctx, app, id, Vec::new());
    }

    /// Cells of the three strips with the section drawn at `MENU_INNER` in a 400×600 area, each not the current one.
    const WRAP_OFF: egui::Pos2 = egui::pos2(120.0, 30.0);
    const WRAP_ON: egui::Pos2 = egui::pos2(200.0, 30.0);
    const WORD_BREAK_BREAK_ALL: egui::Pos2 = egui::pos2(120.0, 84.0);
    const LONG_WORDS_BREAK_WORD: egui::Pos2 = egui::pos2(120.0, 124.0);

    /// `NoWrap` dims Word break and Long words without resetting them, and nothing else.
    /// Clicking the Wrap strip itself is the control, or the "click did nothing" assertions pass against an empty `ui`.
    ///
    /// flips: removing `if !wraps { ui.disable(); }` → the two `NoWrap` click assertions fail, Word break first; inverting it to `if wraps` → the `Wrap` half fails at the first assertion.
    #[test]
    fn nowrap_kills_word_break_and_long_words_and_nothing_else() {
        let (ctx, mut app, id) = app_wrapping(WrapMode::Wrap);
        click(&ctx, &mut app, id, WORD_BREAK_BREAK_ALL);
        assert_eq!(
            para(&app, id).word_break,
            WordBreak::BreakAll,
            "with wrapping on, Word break takes a click — if this fails the \
             coordinate is wrong and the negative half below proves nothing"
        );
        click(&ctx, &mut app, id, LONG_WORDS_BREAK_WORD);
        assert_eq!(
            para(&app, id).overflow_wrap,
            OverflowWrap::BreakWord,
            "and so does Long words"
        );

        click(&ctx, &mut app, id, WRAP_OFF);
        assert_eq!(para(&app, id).wrap, WrapMode::NoWrap, "the mode switched");
        assert_eq!(
            para(&app, id).word_break,
            WordBreak::BreakAll,
            "and the stored value survived the trip — dimmed is not reset"
        );

        let (ctx, mut app, id) = app_wrapping(WrapMode::NoWrap);
        let before = para(&app, id);
        click(&ctx, &mut app, id, WORD_BREAK_BREAK_ALL);
        assert_eq!(
            para(&app, id).word_break,
            before.word_break,
            "under NoWrap a Word break click reaches nothing"
        );
        click(&ctx, &mut app, id, LONG_WORDS_BREAK_WORD);
        assert_eq!(
            para(&app, id).overflow_wrap,
            before.overflow_wrap,
            "and neither does a Long words click"
        );

        click(&ctx, &mut app, id, WRAP_ON);
        assert_eq!(
            para(&app, id).wrap,
            WrapMode::Wrap,
            "the Wrap strip itself is outside the gate and still works — without \
             this assertion the two above pass against a section nobody drew"
        );
    }
}

/// Override marks on the bold/italic pair a family without variants shows, and on the popup's sections.
#[cfg(test)]
mod instance_mark_tests {
    use super::*;
    use crate::app::OndinApp;
    use ondin_core::{NodeId, Operation, Transaction};

    /// A text in a main group *Label* and the instance's copy of it:
    /// `(ctx, app, main's text, copy)`.
    fn instance() -> (egui::Context, OndinApp, NodeId, NodeId) {
        let (ctx, mut app, text) = super::app_with_text();
        let group = app.session.ids.mint();
        let root = app.session.doc.first_page();
        assert!(app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id: group,
                parent: root,
                index: 0,
                kind: ondin_core::NodeKind::Group,
                transform: None,
                name: Some("Label".into()),
            },
            Operation::Reparent {
                id: text,
                new_parent: group,
                index: 0,
            },
            Operation::SetComponent {
                id: group,
                component: true,
            },
        ])));
        let inst = {
            let doc = &app.session.doc;
            let (tx, made) = ondin_core::insert_subtrees(
                doc,
                &mut app.session.ids,
                &[ondin_core::Placement {
                    nodes: doc.capture_subtree(group).unwrap(),
                    parent: root,
                    index: None,
                }],
                Default::default(),
            );
            assert!(app.session.commit(tx));
            made[0]
        };
        let copy = app.session.doc.get(inst).unwrap().children()[0];
        app.session.selection.set_one(copy);
        (ctx, app, text, copy)
    }

    fn style(app: &OndinApp, id: NodeId) -> TextStyle {
        match app.session.doc.get(id).unwrap().kind() {
            ondin_core::NodeKind::Text { style, .. } => (**style).clone(),
            _ => unreachable!("a text"),
        }
    }

    /// Write `edit` into `id`'s own style.
    fn restyle(app: &mut OndinApp, id: NodeId, edit: impl FnOnce(&mut TextStyle)) {
        let mut s = style(app, id);
        edit(&mut s);
        assert!(
            app.session
                .commit(Transaction(vec![Operation::SetTextStyle {
                    id,
                    style: s,
                    spans: None,
                }]))
        );
    }

    /// One frame of `draw` over the copy's subject, with `events` delivered.
    fn frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        id: NodeId,
        events: Vec<egui::Event>,
        draw: impl Fn(&mut OndinApp, &mut egui::Ui, &TypeSubject),
    ) -> egui::FullOutput {
        app.gather_card_overrides();
        let subject = TypeSubject::of(app, id).expect("a text node has a subject");
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(400.0, 600.0),
            )),
            events,
            ..Default::default()
        };
        ctx.run_ui(input, |ui| {
            ui.set_width(MENU_INNER);
            draw(app, ui, &subject);
        })
    }

    /// Every painted text with its rect and colour, and every override dot's centre.
    #[allow(clippy::type_complexity)]
    fn painted(
        out: &egui::FullOutput,
    ) -> (Vec<(String, egui::Rect, egui::Color32)>, Vec<egui::Pos2>) {
        fn walk(shape: &egui::Shape, texts: &mut Vec<(String, egui::Rect, egui::Color32)>) {
            match shape {
                egui::Shape::Text(t) => texts.push((
                    t.galley.text().to_string(),
                    t.visual_bounding_rect(),
                    t.fallback_color,
                )),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, texts)),
                _ => {}
            }
        }
        let mut texts = Vec::new();
        for s in &out.shapes {
            walk(&s.shape, &mut texts);
        }
        (texts, crate::ui::painted_override_dots(out))
    }

    /// The pointer moved to `at`, then a press and a release there, each a frame.
    fn click(
        ctx: &egui::Context,
        app: &mut OndinApp,
        id: NodeId,
        at: egui::Pos2,
        draw: impl Fn(&mut OndinApp, &mut egui::Ui, &TypeSubject),
    ) {
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        frame(ctx, app, id, vec![egui::Event::PointerMoved(at)], &draw);
        frame(ctx, app, id, vec![press(true)], &draw);
        frame(ctx, app, id, vec![press(false)], &draw);
    }

    /// The bold/italic pair marks an overridden slant (one dot at Italic's top right), and a click on it writes the main's slant back.
    ///
    /// flip: the Italic mark comparing the weight (`|s| s.weight`) → fails at "one dot".
    #[test]
    fn the_weight_toggles_mark_the_slant_an_instance_overrides() {
        let (ctx, mut app, main, copy) = instance();
        restyle(&mut app, copy, |s| s.italic = true);
        let draw = |app: &mut OndinApp, ui: &mut egui::Ui, s: &TypeSubject| {
            app.type_weight_toggles(ui, s, egui::vec2(58.0, 28.0));
        };
        let out = frame(&ctx, &mut app, copy, Vec::new(), draw);
        let (texts, dots) = painted(&out);
        let italic = texts
            .iter()
            .find(|(t, ..)| t == icon::TEXT_ITALIC)
            .map(|(_, r, _)| *r)
            .expect("the Italic button");
        assert_eq!(dots.len(), 1, "one dot: {dots:?}");
        assert!(
            dots[0].x > italic.center().x,
            "Italic's, not Bold's: {dots:?}"
        );
        click(&ctx, &mut app, copy, italic.center(), draw);
        assert_eq!(
            style(&app, copy),
            style(&app, main),
            "the main's slant is back"
        );
    }

    /// A popup section carries its mark on its label (one dot on *CASE*, Language not overridden), and a click on the label resets it.
    ///
    /// flip: `section_marked` drawing the plain `section` → fails at "one dot".
    #[test]
    fn a_popup_section_marks_its_own_fields_and_its_label_resets_them() {
        let (ctx, mut app, main, copy) = instance();
        restyle(&mut app, copy, |s| s.case = TextCase::Upper);
        let draw = |app: &mut OndinApp, ui: &mut egui::Ui, s: &TypeSubject| {
            app.case_section(ui, s);
            app.language_section(ui, s);
        };
        let out = frame(&ctx, &mut app, copy, Vec::new(), draw);
        let (texts, dots) = painted(&out);
        let ink = |s: &str| {
            texts
                .iter()
                .find(|(t, ..)| t == s)
                .map(|(_, r, c)| (*r, *c))
                .unwrap_or_else(|| panic!("no {s} in {texts:?}"))
        };
        assert_eq!(dots.len(), 1, "one dot: {dots:?}");
        let (case_at, case_ink) = ink("CASE");
        assert_eq!(case_ink, theme::text::STRONG, "Case is overridden");
        // Language's ink is in the galley, not `fallback_color`.
        assert!(
            (dots[0].y - case_at.center().y).abs() < 8.0 && ink("LANGUAGE").0.top() > dots[0].y,
            "the dot is Case's: {dots:?} beside {case_at:?}"
        );
        click(&ctx, &mut app, copy, case_at.center(), draw);
        assert_eq!(
            style(&app, copy),
            style(&app, main),
            "the main's case is back"
        );
    }

    /// The Sizing mark compares the mode, not the width: auto-height copies at different widths are unmarked; a fixed copy is marked *Auto H*, resetting the main's whole sizing.
    ///
    /// flip: `Mode`'s equality comparing the whole sizing → fails at "the width is W's".
    #[test]
    fn the_sizing_mark_compares_the_mode_and_not_the_width() {
        let (_ctx, mut app, main, copy) = instance();
        let sizing = |id, s| Operation::SetGeometry {
            id,
            geometry: GeometryPatch::TextSizing(s),
        };
        assert!(app.session.commit(Transaction(vec![sizing(
            main,
            TextSizing::AutoHeight(100.0)
        )])));
        assert!(app.session.commit(Transaction(vec![sizing(
            copy,
            TextSizing::AutoHeight(150.0)
        )])));
        app.gather_card_overrides();
        assert!(app.sizing_mark(copy).is_none(), "the width is W's");
        let fixed = TextSizing::Fixed(Size::new(150.0, 40.0));
        assert!(app.session.commit(Transaction(vec![sizing(copy, fixed)])));
        app.gather_card_overrides();
        let mark = app.sizing_mark(copy).expect("the mode is overridden");
        assert_eq!(mark.tip, "Reset to main · Auto H");
        assert_eq!(
            mark.tx,
            Transaction(vec![sizing(copy, TextSizing::AutoHeight(100.0))]),
            "the main's whole sizing"
        );
    }

    // --- the popup's other resets, clicked through ---------
    //
    // Each test clicks the real section's reset and asserts the copy equals its main
    // and `reset::overrides` finds nothing left.

    /// A synthetic `fvar` axis, so the axis sections draw without a font loaded.
    fn font_axis(tag: Tag, min: f64, default: f64, max: f64) -> FontAxis {
        FontAxis {
            tag,
            min,
            default,
            max,
            name: tag.to_string(),
            hidden: false,
        }
    }

    /// Write `edit` into `id`'s own paragraph.
    fn repara(app: &mut OndinApp, id: NodeId, edit: impl FnOnce(&mut ParagraphStyle)) {
        let mut p = match app.session.doc.get(id).unwrap().kind() {
            ondin_core::NodeKind::Text { paragraph, .. } => paragraph.clone(),
            _ => unreachable!("a text"),
        };
        edit(&mut p);
        assert!(
            app.session
                .commit(Transaction(vec![Operation::SetParagraphStyle {
                    id,
                    paragraph: p,
                    spans: None,
                }]))
        );
    }

    fn paragraph(app: &OndinApp, id: NodeId) -> ParagraphStyle {
        match app.session.doc.get(id).unwrap().kind() {
            ondin_core::NodeKind::Text { paragraph, .. } => paragraph.clone(),
            _ => unreachable!("a text"),
        }
    }

    /// The overrides the core still finds on `id`.
    fn left(app: &OndinApp, id: NodeId) -> usize {
        ondin_core::reset::overrides(&app.session.doc, id).len()
    }

    /// Where `label` is painted, after one frame of `draw`.
    fn painted_at(
        ctx: &egui::Context,
        app: &mut OndinApp,
        id: NodeId,
        label: &str,
        draw: impl Fn(&mut OndinApp, &mut egui::Ui, &TypeSubject),
    ) -> egui::Pos2 {
        let out = frame(ctx, app, id, Vec::new(), draw);
        let (texts, _) = painted(&out);
        texts
            .iter()
            .find(|(t, ..)| t.contains(label))
            .map(|(_, r, _)| r.center())
            .unwrap_or_else(|| panic!("no {label} in {texts:?}"))
    }

    fn axes(list: &[(Tag, f64)]) -> Vec<AxisSetting> {
        list.iter().map(|(t, v)| AxisSetting::new(*t, *v)).collect()
    }

    const WGHT: Tag = Tag::new(*b"wght");
    const GRAD: Tag = Tag::new(*b"GRAD");

    /// The Optical size reset writes the main's coordinates in the main's order, not the copy's appended one, so the copy follows again.
    ///
    /// flip: the `put` back to `retain` + `push` → fails at "the main's list, in its order".
    #[test]
    fn the_optical_size_reset_keeps_the_mains_order_and_the_copy_follows_again() {
        let (ctx, mut app, main, copy) = instance();
        restyle(&mut app, main, |s| {
            s.variations = axes(&[(OPSZ, 14.0), (WGHT, 700.0)]);
        });
        restyle(&mut app, copy, |s| {
            s.variations = axes(&[(WGHT, 700.0), (OPSZ, 20.0)]);
        });
        let draw = |app: &mut OndinApp, ui: &mut egui::Ui, s: &TypeSubject| {
            let coords = style(app, s.id).variations;
            app.optical_size_section(ui, s, &[font_axis(OPSZ, 6.0, 14.0, 72.0)], &coords);
        };
        let at = painted_at(&ctx, &mut app, copy, "OPTICAL SIZE", draw);
        click(&ctx, &mut app, copy, at, draw);
        assert_eq!(
            style(&app, copy).variations,
            style(&app, main).variations,
            "the main's list, in its order"
        );
        assert_eq!(left(&app, copy), 0, "nothing is left overridden");
        restyle(&mut app, main, |s| {
            s.variations = axes(&[(OPSZ, 30.0), (WGHT, 700.0)]);
        });
        assert_eq!(
            style(&app, copy).variations,
            style(&app, main).variations,
            "the copy follows the main again"
        );
    }

    /// The Axes reset does the same for the axes it lists.
    ///
    /// flip: the `put` back to `retain` + `extend` → fails at "the main's list, in its order".
    #[test]
    fn the_axes_reset_keeps_the_mains_order() {
        let (ctx, mut app, main, copy) = instance();
        restyle(&mut app, main, |s| {
            s.variations = axes(&[(GRAD, 50.0), (WGHT, 700.0)]);
        });
        restyle(&mut app, copy, |s| {
            s.variations = axes(&[(WGHT, 700.0), (GRAD, 80.0)]);
        });
        let draw = |app: &mut OndinApp, ui: &mut egui::Ui, s: &TypeSubject| {
            let coords = style(app, s.id).variations;
            app.axes_section(ui, s, &[font_axis(GRAD, 0.0, 0.0, 100.0)], &coords);
        };
        let at = painted_at(&ctx, &mut app, copy, "AXES", draw);
        click(&ctx, &mut app, copy, at, draw);
        assert_eq!(
            style(&app, copy).variations,
            style(&app, main).variations,
            "the main's list, in its order"
        );
        assert_eq!(left(&app, copy), 0, "nothing is left overridden");
    }

    /// The face mark compares weight, slant and the axes the variant drives; its reset leaves the other sections' axes alone (a copy differing only in `opsz` has no face mark).
    ///
    /// flip: the face mark comparing the whole `variations` → fails at "opsz is Optical size's".
    #[test]
    fn the_face_mark_is_the_faces_and_leaves_the_other_axes() {
        let (_ctx, mut app, main, copy) = instance();
        restyle(&mut app, main, |s| {
            s.weight = 400;
            s.variations = axes(&[(WGHT, 400.0)]);
        });
        restyle(&mut app, copy, |s| {
            s.variations = axes(&[(WGHT, 400.0), (OPSZ, 20.0)]);
        });
        app.gather_card_overrides();
        assert_eq!(
            app.type_marks(copy).variant.map(|m| m.tip),
            None,
            "opsz is Optical size's"
        );
        restyle(&mut app, main, |s| {
            s.variations = axes(&[(GRAD, 50.0), (WGHT, 400.0)]);
        });
        restyle(&mut app, copy, |s| {
            s.weight = 700;
            s.variations = axes(&[(GRAD, 80.0), (WGHT, 700.0)]);
        });
        app.gather_card_overrides();
        let mark = app.type_marks(copy).variant.expect("the face differs");
        assert_eq!(mark.tip, "Reset to main · 400");
        assert!(app.session.commit(mark.tx));
        let s = style(&app, copy);
        assert_eq!(s.weight, 400, "the main's weight");
        assert_eq!(
            s.variations,
            axes(&[(GRAD, 80.0), (WGHT, 400.0)]),
            "the main's wght, the copy's own GRAD"
        );
    }

    /// A click on *Word break* resets only the word break, and one on *Long words* only the long-word rule.
    ///
    /// flip: the two `inner_hit = Some(&…)` swapped → fails at "Word break's, not Long words'".
    #[test]
    fn the_wrap_sections_inner_labels_reset_their_own_fields() {
        let (ctx, mut app, main, copy) = instance();
        let mine = (WordBreak::ALL[1], OverflowWrap::ALL[1]);
        repara(&mut app, copy, |p| {
            p.word_break = mine.0;
            p.overflow_wrap = mine.1;
        });
        assert_ne!(
            paragraph(&app, main).word_break,
            mine.0,
            "the fixture differs"
        );
        assert_ne!(
            paragraph(&app, main).overflow_wrap,
            mine.1,
            "the fixture differs"
        );
        let draw = |app: &mut OndinApp, ui: &mut egui::Ui, s: &TypeSubject| {
            app.wrap_section(ui, s);
        };
        let at = painted_at(&ctx, &mut app, copy, "WORD BREAK", draw);
        click(&ctx, &mut app, copy, at, draw);
        let p = paragraph(&app, copy);
        assert_eq!(
            (p.word_break, p.overflow_wrap),
            (paragraph(&app, main).word_break, mine.1),
            "Word break's, not Long words'"
        );
        let at = painted_at(&ctx, &mut app, copy, "LONG WORDS", draw);
        click(&ctx, &mut app, copy, at, draw);
        assert_eq!(paragraph(&app, copy), paragraph(&app, main));
        assert_eq!(left(&app, copy), 0, "nothing is left overridden");
    }

    /// Spacing & indent resets all its values as one.
    ///
    /// flip: the mark's `put` skipping `indent_start` → fails the paragraph equality (`indent_start: Px(12.0)` left).
    #[test]
    fn the_spacing_and_indent_label_resets_all_five() {
        let (ctx, mut app, main, copy) = instance();
        repara(&mut app, copy, |p| {
            p.spacing = Length::Px(24.0);
            p.indent_start = Length::Px(12.0);
            p.hanging = true;
        });
        let draw = |app: &mut OndinApp, ui: &mut egui::Ui, s: &TypeSubject| {
            app.type_paragraph_tab(ui, s);
        };
        let at = painted_at(&ctx, &mut app, copy, "SPACING & INDENT", draw);
        click(&ctx, &mut app, copy, at, draw);
        assert_eq!(paragraph(&app, copy), paragraph(&app, main));
        assert_eq!(left(&app, copy), 0, "nothing is left overridden");
    }

    /// List resets the marker and its level as one.
    ///
    /// flip: the mark's `put` skipping `level` → fails the paragraph equality (`level: 2` left).
    #[test]
    fn the_list_label_resets_the_marker_and_its_level() {
        let (ctx, mut app, main, copy) = instance();
        repara(&mut app, copy, |p| {
            p.marker = Some(ListMarker::ALL[0]);
            p.level = 2;
        });
        let draw = |app: &mut OndinApp, ui: &mut egui::Ui, s: &TypeSubject| {
            let shown = s.shown_paragraph();
            app.list_section(ui, s, &shown);
        };
        let at = painted_at(&ctx, &mut app, copy, "LIST", draw);
        click(&ctx, &mut app, copy, at, draw);
        assert_eq!(paragraph(&app, copy), paragraph(&app, main));
        assert_eq!(left(&app, copy), 0, "nothing is left overridden");
    }

    /// The line height's ↺ is the prefix strip's press and commits the main's value (auto) before the valve is told.
    ///
    /// flip: dropping the field's `commit_reset` → fails at "the main's line height" (`Some(Px(30.0))` left).
    #[test]
    fn the_line_height_prefix_resets_the_line_height() {
        let (ctx, mut app, main, copy) = instance();
        restyle(&mut app, copy, |s| s.line_height = Some(Length::Px(30.0)));
        let draw = |app: &mut OndinApp, ui: &mut egui::Ui, s: &TypeSubject| {
            let marks = app.type_marks(s.id);
            app.type_line_height_field(ui, s, egui::vec2(120.0, 28.0), marks.line_height.as_ref());
        };
        let at = painted_at(&ctx, &mut app, copy, icon::ARROWS_VERTICAL, draw);
        click(&ctx, &mut app, copy, at, draw);
        assert_eq!(
            style(&app, copy),
            style(&app, main),
            "the main's line height"
        );
        assert_eq!(left(&app, copy), 0, "nothing is left overridden");
    }

    /// The family dropdown's first row (*Reset to main · Inter*) puts the main's family back.
    /// The variant dropdown's reset row is not driven: it needs a family with two faces.
    ///
    /// flip: the row's `commit_reset` dropped (early `return` kept) → fails at "the main's family".
    #[test]
    fn the_family_dropdowns_reset_row_puts_the_family_back() {
        let (ctx, mut app, main, copy) = instance();
        restyle(&mut app, copy, |s| s.font_family = "Elsewhere Sans".into());
        let draw = |app: &mut OndinApp, ui: &mut egui::Ui, s: &TypeSubject| {
            let marks = app.type_marks(s.id);
            app.type_family_row(ui, s, marks.family.as_ref());
        };
        let at = painted_at(&ctx, &mut app, copy, "Elsewhere Sans", draw);
        click(&ctx, &mut app, copy, at, draw);
        let row = painted_at(&ctx, &mut app, copy, "Reset to main · Inter", draw);
        click(&ctx, &mut app, copy, row, draw);
        assert_eq!(style(&app, copy), style(&app, main), "the main's family");
        assert_eq!(left(&app, copy), 0, "nothing is left overridden");
    }
}
