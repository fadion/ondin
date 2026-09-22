//! The Type panel and the typography popup (§9.2).
//!
//! **The panel is the four things a designer reaches for every minute** — family,
//! variant, size, line height, letter spacing, and the two alignments — and the
//! popup is strictly *overflow*. No control appears in both: a duplicated control
//! has to show "mixed" twice, which is two chances to disagree, and it makes the
//! question "which one of these is the real setting" answerable only by trying it.
//!
//! **The popup's tabs are scopes** ([`TypeTab`]) — Character, Paragraph, Box —
//! labelled with the scope word, because the label is what tells the user what a
//! change will hit when the selection is partial. There is no fourth: the axes,
//! the OpenType features and the language used to have a `Font` tab of their own
//! on the grounds that they are *generated from the family*, which is a fact about
//! how they are built and not about what they do. All of it is character-scoped,
//! so the strip named three scopes and one implementation detail. See [`TypeTab`].
//!
//! ## Scope, and where a write goes
//!
//! A character attribute has two possible targets and [`TypeSubject::range`] is
//! what decides between them:
//!
//! - **A live editing session with a selection** — the range is that selection,
//!   the control can read *mixed*, and the write goes into the session's own spans
//!   (`TextEdit::style_selection`), which previews itself and commits once when
//!   the session ends (§9.3: one editing session is one undo step).
//! - **Anything else** — the range is the whole node, and the write sets the
//!   node's *default* and drops the overrides of that one attribute. Setting a
//!   default while leaving the spans alone is how "make it all 24pt" leaves a
//!   16pt run behind.
//!
//! **Seven paragraph attributes have the same two targets** — the spacing, the
//! first-line indent, `hanging`, the two block indents, the list marker and its
//! nesting level — reached through
//! [`TypeSubject::para_range`] rather than `range`, because the two scopes do not
//! agree about what a selection *is*:
//!
//! - A paragraph attribute cannot apply to half a paragraph, so the range is the
//!   selection **snapped outward to hard breaks** (`text::paragraph_bounds`).
//! - **A bare caret is partial here**, where for a character attribute it is not.
//!   A caret with nothing selected has no characters to restyle, so the character
//!   scope falls to its *"anything else"* arm above and writes the **node's
//!   defaults** over the whole range — but a caret is unambiguously *inside* one
//!   paragraph, which is the case this whole feature is for. So an open session
//!   always writes a paragraph span and the node's paragraph *defaults* are
//!   reachable only with the layer selected and no session running: while you are
//!   editing you are editing paragraphs.
//!
//!   ⚠️ **That sentence used to read *"hence `TextEdit::pending`, which holds the
//!   attribute for the next one typed"*, and it named a mechanism the app never
//!   runs** (§15 D733, `[S6.2-L3-08]`). `TextEdit::style_selection` is
//!   `pending`'s only door, it has exactly **one** production caller — the
//!   `subject.partial` arm of `apply_char_attrs` — and `TypeSubject::read` sets
//!   `partial` **only** for `Some((sel, _)) if !sel.is_empty()`. So
//!   `style_selection` is never called with an empty range and its
//!   `if range.is_empty()` arm is dead in production. The feature is real, has two
//!   core tests and `architecture.md` §5.4's boundary-rule bullet behind it — **not
//!   §15 D217, which is the clipboard entry and *depends* on the feature rather
//!   than deciding it** — and describes nothing this panel does —
//!   which is worse than an absent feature, because **a reader of this doc goes
//!   looking for a mechanism that is not wired up.**
//!
//!   ⚠️ **The hinge is already asserted and no new test was written.**
//!   `bare_caret_readout_tests::shown_size` guards its own fixture with
//!   *"a bare caret is not partial — that is the designed half"*, which is
//!   exactly the condition that makes `pending` unreachable. So the code was
//!   pinned and only the prose had drifted — **a test can hold a fact firmly
//!   while a doc three thousand lines away denies it, and nothing compares
//!   them.**
//!
//! The rest of the paragraph scope, and all of the block scope, has one target
//! either way — `Alignment`, `Justify` and the base direction are one value per
//! `Layout` and no span type carries them (§15 D77). The two sections of the
//! Paragraph tab that edit them say so on hover, because a control that quietly
//! ignores the selection reads as broken rather than as limited.

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
    /// **Carried so a decoration's "inherit" state can be *drawn* rather than
    /// merely named.** `Decoration::color` is `None` by default, meaning "the
    /// text's own", and a swatch showing grey for that is a claim about a colour
    /// the panel would not know. With this it shows the real one, and adopting it
    /// explicitly starts from the paint that was already on screen instead of
    /// from an arbitrary black.
    ///
    /// **The whole brush, not a colour, because the renderer inherits the whole
    /// brush.** `scene.rs` gives an uncoloured decoration `inks.first()` — the
    /// gradient included — so flattening a gradient fill to black here (which is
    /// what this field used to do) made the panel claim a flat black underline
    /// where the canvas drew a gradient one, with nothing on the row to show the
    /// difference. [`TypeSubject::text_color`] is the collapse, kept for the
    /// controls that genuinely need one colour and stated in one place.
    pub text_paint: Option<Brush>,
    /// What the shaped layout's first line actually measures, in px — so the line
    /// height field can *show* what `Auto` is worth and seed from it rather than
    /// from a round number (§15 D162). `None` for a node with nothing laid out.
    pub line_height: Option<f64>,
    /// What the face's own underline and strikethrough thicknesses are worth, in
    /// px — [`Self::line_height`]'s field one along, and read for the same reason
    /// (§15 D570).
    ///
    /// `TextLayout::decoration_sizes`, so the decoration's thickness chip can seed
    /// what the font was drawing instead of seeding **zero**, which erased the line.
    pub decoration_sizes: Option<(f64, f64)>,
    /// The bytes a character control acts on — see the module docs.
    pub range: Range<usize>,
    /// Where [`Self::shown`] resolves an attribute when the range disagrees with
    /// itself: the byte whose style a character control should be **describing**.
    ///
    /// 🚨 **`None` means "the node's defaults", and it is a real answer rather
    /// than a missing one** (§15 D589, `[S6.2-L1-06]`). For a selection this is
    /// `range.start` and always was. For a **bare caret** it is the byte to the
    /// caret's *left*, because that is what typing there inherits: `Spans::edited`
    /// maps a span whose `end == at` to `at + inserted` and a span whose
    /// `start == at` to `at + inserted` as well, so an insertion joins the run
    /// that ends at the caret and never the one that begins there. A caret at byte
    /// 0 has no run to its left, and its `None` is what says so.
    ///
    /// It used to be `range.start`, which for a bare caret is **byte 0** whatever
    /// the caret is doing. Measured on `"hello world"` with `Size(40)` over the
    /// first five bytes and a node default of 16: caret at the end, the Size field
    /// read **40**, `mixed` was structurally `false`, and the next keystroke came
    /// out at **16**. The field named a size the very next character contradicted
    /// — and a scrub started from it would have flattened the whole node to 40,
    /// which nobody asked for.
    ///
    /// ⚠️ **The write is unchanged and must be**: §9.4 and §15 D164 settle that a
    /// bare caret's character write sets the node's *default* and drops that
    /// attribute's overrides. This is the readout only, which the design has no
    /// passage on.
    pub resolve_at: Option<usize>,
    /// Whether a live session has a **non-empty selection**. **The only case in
    /// which a *character* control can read mixed.**
    ///
    /// 🚨 **This said "a sub-range rather than the whole node" and that is not
    /// what `read` computes** (§15 D636). It is `!sel.is_empty()`, so a
    /// **select-all is partial** — which is the case a designer meets first, and
    /// the one `[S6.2-L1-05]` measured. The two states that are *not* partial are
    /// the bare caret and no session at all, and for both of those `shown`
    /// describes the run a keystroke will join (§15 D589) rather than a range that
    /// could disagree with itself. The old sentence made a select-all sound
    /// exempt from `mixed`, which would have made that finding unreproducible.
    pub partial: bool,
    /// The node's per-paragraph overrides — the live session's own while one is
    /// open, for the same reason `spans` is: the document's copy is a commit behind.
    pub para_spans: ParaSpans,
    /// The bytes a paragraph control acts on: the selection snapped outward to
    /// hard breaks. See the module docs for why this is not `range`.
    pub para_range: Range<usize>,
    /// Whether the write goes into a live session's overrides rather than into the
    /// node's paragraph defaults — the paragraph scope's twin of `partial`.
    ///
    /// True for any open session with a paragraph to name, a select-all included,
    /// and false for an empty node, where there is no paragraph to tell apart from
    /// the node itself.
    pub para_partial: bool,
}

impl TypeSubject {
    /// The panel's subject for `id`, or `None` if it is not a text node.
    ///
    /// Read through the **preview**, so every control tracks a live drag (§9.3).
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
        // The *first* visible fill, matching the renderer: a decoration takes the
        // bottom of the fill stack and is not repeated per fill.
        let text_paint = node
            .paint()
            .fills
            .iter()
            .find(|f| f.visible)
            .map(|f| f.brush.clone());
        let parts = TextRef::of(&kind)?;
        let len = parts.content.len();
        // A live session's selection is the range, and its *own* spans are the
        // truth — the document's copy is a commit behind while the session runs.
        let live = app.text.as_ref().filter(|s| s.id == id);
        let (range, spans, partial, resolve_at) =
            match live.map(|s| (s.editor.selected_range(), s.editor.spans())) {
                Some((sel, spans)) if !sel.is_empty() => {
                    let at = sel.start;
                    (sel, spans.clone(), true, Some(at))
                }
                // The bare caret. `sel.start == sel.end` is the caret byte, and the
                // readout describes the run to its **left** — see `resolve_at`.
                Some((sel, spans)) => (0..len, spans.clone(), false, sel.start.checked_sub(1)),
                None => (0..len, parts.spans.clone(), false, Some(0)),
            };
        // **The paragraph scope asks a different question of the same selection, and
        // asks it of the *editor's* content.** A live session's string is a commit
        // ahead of the document's, so snapping against `len` here would name the
        // wrong paragraph the moment anything had been typed.
        let (para_range, para_spans, para_partial) = match live {
            Some(s) => {
                let range = s.editor.paragraph_range();
                // **Partial whenever there is a paragraph to name**, which for a
                // select-all is every paragraph and still a span write — the same
                // bargain the character scope makes, and it is what keeps the
                // *defaults* out of a session's reach. Empty only for an empty node,
                // where there is no paragraph to tell apart from the node itself.
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
            // From the *preview* layout, so the readout tracks a live edit rather
            // than the last commit — the same source every other measured thing in
            // this panel reads. Zero for a node with no line to measure.
            line_height: app
                .session
                .preview_text_layout(id)
                .map(|l| l.line_height)
                .filter(|h| *h > 0.0),
            // The same layout, and `None` for a node with nothing shaped — which
            // the thickness chip reads as "nothing resolved" and falls back on.
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
        })
    }

    /// The one colour the text's paint collapses to, for the controls that can
    /// only hold one — a hex field, an opacity, the colour a decoration adopts.
    ///
    /// **The first stop, and black only where there is no fill.** Both halves
    /// match something that already exists rather than being chosen here: the
    /// first stop is [`OndinApp::write_char_slot`]'s stated collapse for
    /// this very slot, and black is what `scene.rs` paints a decoration on a
    /// fill-less node with, so the swatch and the canvas agree in both cases. It
    /// used to be black for a *gradient* too, which agreed with neither.
    pub fn text_color(&self) -> Color {
        match &self.text_paint {
            Some(brush) => super::paint::stops_of(brush)
                .first()
                .map_or(Color::BLACK, |(_, c)| *c),
            None => Color::BLACK,
        }
    }

    /// The stops to *draw* the inherited paint with, or `None` when it is a plain
    /// colour that [`Self::text_color`] already covers.
    ///
    /// This is what puts the difference on screen: a decoration inheriting a
    /// gradient gets the ramp in its swatch, where before it got a black chip
    /// indistinguishable from black text. Through `picker::ramp`, so the chip is
    /// built exactly the way every other gradient chip in the app is.
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
    /// **Two zeroes in different units are one value here** (§15 D799,
    /// `[S6.1-L1-03]`). `Spans::shared_in` compares structurally and has to:
    /// `Em(0.0)` and `Px(0.0)` are different values to the *model*, and §15 D537
    /// exists to keep them so, because that difference is where a user's choice
    /// of unit lives when the amount is zero. But they are the same **ink**, so
    /// text whose letter spacing is zero throughout was reading *Mixed* —
    /// reliably, for anyone who sets the unit before the number.
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

    /// The value to *show* for an attribute: the shared one, or — where the range
    /// disagrees with itself — the value at [`Self::resolve_at`], so a field still
    /// has digits in it and a drag has somewhere to start from. Callers pair this
    /// with [`Self::shared`] to decide whether to say "mixed".
    ///
    /// **The resolve point is not `range.start`** (§15 D589) — see `resolve_at`
    /// for why a bare caret describes the byte to its left and why byte 0 is the
    /// one case with no byte to describe.
    fn shown(&self, kind: CharAttrKind) -> CharAttr {
        self.shared(kind).unwrap_or_else(|| match self.resolve_at {
            Some(at) => self.spans.resolve(&self.style, at).get(kind),
            None => self.style.get(kind),
        })
    }

    fn mixed(&self, kind: CharAttrKind) -> bool {
        self.partial && self.shared(kind).is_none()
    }

    /// Whether the range disagrees with itself about one attribute.
    ///
    /// **Not the same question as [`Self::mixed`]**, and the difference is what a
    /// reset button has to know. `mixed` is about what a control may *say*, and
    /// over the whole node it is false by design however ragged the spans are:
    /// a whole-node write flattens them, so [`Self::shown`] is showing the value
    /// the next write will produce and is not lying about it.
    ///
    /// A reset asks something else — *is there anything here to put back* — and
    /// answering that from `shown` reads byte 0 and calls it the node. That was a
    /// reported bug: a red underline colour living in a span that did not reach
    /// byte 0 left the reset dimmed and inert with the red plainly on screen.
    fn ragged(&self, kind: CharAttrKind) -> bool {
        self.shared(kind).is_none()
    }

    /// Whether the range disagrees about **one OpenType tag** (§15 D752).
    ///
    /// 🚨 **Per tag, not per attribute, and the difference is the whole readout.**
    /// [`Self::mixed`] answers about `CharAttrKind::Features` as a *list*, so it
    /// is true the moment any one tag differs anywhere — which would dim all
    /// forty rows because one of them disagreed. `CharSpans::values_in` hands back
    /// every distinct list the range holds, so the value each list gives *this*
    /// tag can be compared directly, and only the rows that actually disagree say
    /// so.
    ///
    /// **Absent is `0`, exactly as the row reads it.** A tag missing from a list
    /// means *whatever the font does by default*, and the switch draws that as
    /// off; treating absence and an explicit `0` as different here would mark a
    /// row mixed that reads identically in both runs.
    ///
    /// ⚠️ **Only a partial selection can be mixed**, which is [`Self::mixed`]'s
    /// rule and holds for the same reason: a whole-node write flattens the spans,
    /// so what the row shows is what the next write will produce and is not a lie
    /// about it.
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

    fn font_size(&self) -> f64 {
        match self.shown(CharAttrKind::Size) {
            CharAttr::Size(v) => v,
            _ => self.style.font_size,
        }
    }

    // --- the paragraph scope, mirroring the four above ---------------------

    /// [`Self::shared`] for the paragraph scope, including its zero rule — D537
    /// kept the unit on four `ParaAttr` lengths too, so the readout was wrong in
    /// the same way on all four (§15 D799).
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
    /// for the paragraph scope, and the same distinction from [`Self::para_mixed`].
    ///
    /// **What the hanging pair's click has to know.** Its "did anything change?" test
    /// is a value comparison against [`Self::shown_paragraph`], which over a
    /// non-partial range reads the *first* paragraph — so with the layer selected, a
    /// hanging override on the second paragraph and none on the first, the pair drew
    /// "first-line" lit and clicking it wrote nothing while the second paragraph
    /// plainly hung. That is the reported bug `ragged` was added for in the other
    /// scope, arriving here through the same door: `mixed` is about what a control may
    /// *say*, this is about whether there is anything to flatten.
    ///
    /// The four length fields do not need it — a drag changes the number, so their
    /// test fires whatever the range holds. Typing the value already shown into a
    /// ragged field is the one gap left, and it is a much smaller one than a lit
    /// button that does nothing.
    fn para_ragged(&self, kind: ParaAttrKind) -> bool {
        self.para_shared(kind).is_none()
    }

    /// The paragraph style the tab's spannable controls **show**: the node's
    /// defaults with the current paragraph's overrides applied.
    ///
    /// **Not what the node-level controls write from.** `justify_last` and
    /// `direction` build their transaction from `self.paragraph`, because writing a
    /// resolved value back would silently promote one paragraph's indent to the
    /// node's default — a control changing something it does not name.
    ///
    /// Over a range that disagrees this resolves at the range's *start*, which is
    /// [`Self::shown`]'s bargain in the other scope: a field still has digits in it
    /// and a drag has somewhere to start from, and [`Self::para_mixed`] is what
    /// stops it claiming the number is *the* value.
    ///
    /// **With no session open the range is the whole node, so this reads the first
    /// paragraph** — not the defaults. Deliberate, and the same convention the
    /// character fields follow: `para_mixed` is false there however ragged the spans
    /// are, because the next write flattens them, so the number shown is the one that
    /// write will produce. [`Self::para_ragged`] is the question that is *not*
    /// answered by this, and the hanging pair needs it.
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
/// The cell height that makes a [`ui::segmented`] track paint [`ROW_H`] tall — so
/// the two alignment tracks are the same height as the fields above them and the
/// button beside them. The track adds its own 2pt of padding above and below.
const SEG_CELL_H: f32 = ui::SEGMENT_CELL_H;

/// What one cell of an alignment track comes out at on a 284pt card: the design's
/// module, near enough, and reached from the *columns* rather than pinned. Used
/// only to state the tolerance a test holds the row to — see
/// [`OndinApp::type_alignment_row`] for why the grid is what is pinned instead.
///
/// [`OndinApp::type_alignment_row`]: crate::app::OndinApp
#[cfg(test)]
const SEG_CELL: f32 = 28.0;

/// How wide a [`ui::segmented`] track of `n` cells is, given the cell width: the
/// cells, a 2pt gap between each pair, and the recessed track's own 2pt padding
/// plus 1pt hairline at each end. The inverse of what `segmented` does to a width
/// it is handed — **six**, not four, since the track gained the design's border
/// (§15 D386) and the cells give it up on both sides.
#[cfg(test)]
fn seg_cell_of(track: f32, n: usize) -> f32 {
    (track - 6.0 - 2.0 * (n as f32 - 1.0)) / n as f32
}
/// Height of one row in the family list. Fixed, because `ScrollArea::show_rows`
/// virtualizes by dividing the scroll offset by it — it is what lets the list
/// build twenty widgets instead of nineteen hundred. Tall enough for a 15pt
/// preview of the name plus a little air.
const FAMILY_ROW_H: f32 = 24.0;

/// Room a family row keeps at its right edge for the loading dot (§15 D353).
///
/// **Reserved on every row, lit or not.** The alternative — taking the room only
/// while a row is loading — reflows the name at the exact moment its face
/// arrives, so a list settling after a scroll twitches once per row. The dot is
/// the smaller change of the two, and it is the one the eye is being pointed at.
const LOADING_DOT_SLOT: f32 = 14.0;
/// The dot itself, and the beat it breathes on.
///
/// **A pulse rather than a spinner**, because there are up to twenty of these on
/// screen at once during a scroll and twenty spinners is a fairground. A dot that
/// breathes reads as *pending* at a glance and disappears into the row when it is
/// not there.
const LOADING_DOT_R: f32 = 2.5;
/// Seconds for one full breath. Slow enough to read as waiting rather than as an
/// alarm; egui's own `Spinner` turns at about this rate.
const LOADING_DOT_PERIOD: f64 = 1.2;

/// Inner padding of the family list's search field. Its own numbers rather than
/// `button_padding`, which is 4x2 across the chrome and belongs to controls a
/// third this width.
const FILTER_PAD_X: i8 = 7;
const FILTER_PAD_Y: i8 = 5;

/// Popup geometry, from `design/Editor.dc.html`.
///
/// **One card, 272 wide, and every tab draws into the same box.** The mockup lays
/// the three tabs out side by side so all their states can be seen at once; in the
/// app it is one card that swaps its contents, which is why the width is pinned
/// rather than derived — the tabs do not need the same room and a card that
/// resized itself would jump every time one was picked. The same reason the
/// picker's width is pinned.
const MENU_W: f32 = crate::app::POPOVER_W;
const MENU_PAD: f32 = 11.0;
/// The room inside the padding **and the border**, which everything below divides
/// up — `MENU_W` is the width the card paints, not the room in it (§15 D307).
const MENU_INNER: f32 = crate::ui::menu_inner_w(MENU_W, MENU_PAD);
/// A field, and the side of the square buttons that sit beside one.
///
/// ⚠️ **It was 26, and the sentence that stood here — *shorter than the card's 28px
/// fields, which is what keeps a popup of nine sections from towering over the
/// panel it belongs to* — is the argument that was overruled** (§15 D386). The
/// popup hangs beside the card it opened from, at the same width, so two points
/// shorter reads as a wobble rather than as a decision. It is [`ui::CONTROL_H`],
/// like every other control in the chrome.
const CELL: f32 = ui::CONTROL_H;
/// A segmented cell. The track it sits in paints 2px larger on every side.
const SEG_H: f32 = ui::SEGMENT_CELL_H;
/// The gutter turning a list marker on opens, when the paragraph has none.
///
/// **In px, and only ever written once** — see [`OndinApp::list_section`]. A marker
/// is right-aligned on the paragraph's start edge, so with no indent it draws outside
/// the box; 24 is a little wider than `10.` at the panel's own default 16pt type,
/// which is the case where a too-narrow gutter would show first.
const DEFAULT_LIST_GUTTER: f64 = 24.0;
/// What the Paragraph tab's two node-level sections say on hover.
///
/// **Because a control that quietly ignores the selection reads as broken.** The
/// five controls above them take the paragraphs the selection touches; these two
/// cannot, and the reason is parley's rather than a shortcut here — `Layout::align`
/// takes one `Alignment` for the whole layout and the bidi base direction is one
/// value with it (§15 D77). Saying so costs a hover and buys the difference between
/// a limit and a bug.
const WHOLE_LAYER: &str = "The whole text layer — alignment and direction cannot vary by paragraph";

/// Between one section and the next — **10, and it is now one number for every
/// popover in the app** (`ui::POPOVER_SECTION_GAP`).
///
/// **This was 15, and the 15 was asked for.** The mock draws each tab as its own
/// card with three or four sections; the real Character tab has seven, and at 10
/// against an in-section gap of 5 the ratio is 2:1, which read as one long list
/// rather than as sections — reported as "a bit too tight", with 5 more asked for
/// by name. It went back to 10 on 2026-08-21, by the same person and against a
/// wider view: by then five popovers each carried their own number (15 here, 11 in
/// the export card, 10 in the stroke card and the picker), and *"section gap on
/// the popovers should be 10 on all of them"*. Consistency across five cards
/// outranks the ratio inside one of them, which is this project's usual answer
/// (§15 D275). Kept as a named alias rather than replaced everywhere, so what the
/// Character tab spends is still readable as a decision that was made about *it*.
const SECTION_GAP: f32 = ui::POPOVER_SECTION_GAP;
/// Between a section's label and its controls.
///
/// ⚠️ **Its second job is gone.** It used to hold two controls of one section to
/// each other as well — the mock draws every section as a `column` at 5, label
/// included — and that put two stacked fields 5 apart inside a popover hanging off
/// a card whose own rows sit at 9. The Transform card is the baseline now, on both
/// axes and in every panel, so control-to-control is [`ui::CARD_ROW_GAP`] and this
/// holds only the label (§15 D386). [`ui::labelled`] is what keeps the two apart.
const LABEL_GAP: f32 = ui::SECTION_LABEL_GAP;
/// Between two controls side by side in a row.
///
/// **[`GAP`], the Type panel's own, not the 6 the design's popup mock uses.** The
/// popup and the card it hangs from are read one after the other and their fields
/// are the same fields; a one-point difference between them is not a distinction,
/// it is a wobble. Reported after the first pass, and the mock's 6 was never a
/// decision — the card behind it was drawn at 7 in the same file.
///
/// ⚠️ **The paragraph that used to stand here is wrong now, and worth stating so
/// rather than deleting.** It said the popup takes the design's *vertical*
/// compression whole — 5 down against the card's 9 — while matching the card
/// exactly across, and called "one axis compressed and one matched" the rule. The
/// argument for it was the mock. The instruction that replaced it was explicitly
/// not the mock: the Transform card's X↔Y and X/Y↔W/H gaps are the baseline every
/// panel and popover follows, so both axes are matched now and the popup is taller
/// for it (§15 D386). [`CELL`]'s 26 against [`ROW_H`]'s 28 is the remaining
/// compression and is untouched — it is a height, not a gap.
const COL_GAP: f32 = GAP;
/// How tall a list inside the popup may grow before it scrolls.
///
/// The same number for the axes and for the features, and the popup is the
/// reason: both are font-dependent and either can be long, so two lists with
/// their own limits would make the card's height a function of which family is
/// selected.
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

/// Whether a click on a family row is an **edit** — `None` for a re-pick of the
/// family the subject is already in (§15 D467).
///
/// ⚠️ **Because picking a family clears every variable-axis setting and every
/// OpenType feature on the layer**, and that clearing is right for a *change* and
/// destructive for a *re-pick*. `[S6.2-L1-02]`: a text layer in Inter with
/// `wght 620`, `opsz 28`, `tnum 1` and `ss01 1`; open the dropdown to check what
/// family it is in, click the row the list draws as **selected**, and — measured
/// headless — `variations [opsz 28, wght 620] → []`,
/// `features [ss01 1, tnum 1] → []`, `undo_depth 0 → 1`, the family unchanged.
/// No confirmation, and nothing on screen distinguishes that click from the no-op
/// the user meant it to be.
///
/// ⚠️ **`apply_char_attrs`' equality guard cannot save it, and fires exactly as
/// designed.** `char_attrs_tx` reports a change because clearing two non-empty
/// vectors *is* one. This is not the no-op family (`[S14.1-L1-01]`) and no guard
/// at the commit can see it: the write really does change something.
///
/// ⚠️ **`mixed` is why this is three arguments rather than a `!=`.** `current` is
/// `attr!(subject, Family, Family)`, one member's family over a disagreeing
/// selection — so re-picking *that* family is a real edit, the one that makes the
/// selection agree, and the clearing is right for every layer it moves. The row's
/// own label reads the same fact to print the word *"Mixed"*.
///
/// `family_row` returns `clicked()` for any row and gates nothing: `selected`
/// decides only the ground it paints. That is deliberate — the list is drawn by
/// hand for `show_rows` — so the decision has to live here.
fn family_actually_changed(chosen: Option<String>, current: &str, mixed: bool) -> Option<String> {
    chosen.filter(|f| mixed || f != current)
}

impl OndinApp {
    /// The Type panel: family, variant and size, the two `Length` fields, and the
    /// alignment row with the popup's button on the end.
    pub(crate) fn inspector_type(&mut self, ui: &mut egui::Ui, subject: TypeSubject) {
        // **The button's own rect has to reach the popup.** The click that opens
        // the popup is a click *outside* it — the popup does not exist yet when it
        // happens — so the dismiss test below has to know which rect to forgive,
        // or the popup opens and closes on the same frame (D82). Carried out of
        // the panel closure as a plain local, since `panel` hands the closure
        // `&mut Self` and nothing else.
        let mut menu_button: Option<egui::Rect> = None;
        self.panel(ui, "Type", None, |app, ui| {
            app.type_family_row(ui, &subject);
            let half = egui::vec2((ui.available_width() - GAP) / 2.0, ROW_H);
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                app.type_variant_field(ui, &subject, half);
                app.type_size_field(ui, &subject, half);
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                app.type_line_height_field(ui, &subject, half);
                app.type_letter_spacing_field(ui, &subject, half);
            });
            menu_button = app.type_alignment_row(ui, &subject);
        });
        self.type_menu_popup(ui, &subject, menu_button);
    }

    // --- main panel -------------------------------------------------------

    /// The family dropdown — **virtualized, and every row set in its own face**
    /// (§9.2).
    ///
    /// Two things about the list are load-bearing rather than decorative:
    ///
    /// - **Only the rows in view are built.** With the system fonts and the whole
    ///   Google Fonts catalog the list is ~1900 families, and a widget per family
    ///   cost about a second of layout, id hashing and glyph rasterization every
    ///   time the list opened — for the twenty rows a scroll area then showed.
    ///   `show_rows` needs a fixed row height, which is why [`FAMILY_ROW_H`] is a
    ///   constant and the rows are drawn by hand rather than by `selectable_label`.
    /// - **Drawing a row asks for that family's regular face**, so scrolling the
    ///   list warms exactly the files picking a family needs. The row is where the
    ///   prefetch comes from; the preview is what makes it worth doing anyway.
    fn type_family_row(&mut self, ui: &mut egui::Ui, subject: &TypeSubject) {
        let shown = attr!(subject, Family, Family);
        let mixed = subject.mixed(CharAttrKind::Family);
        let current = shown.clone();
        // **Three states, and the third one is a warning** (§15 D227). A family whose
        // bytes are still coming says so with an ellipsis — text on canvas is in the
        // fallback face until they land, and without that the wait reads as the pick
        // not having taken — and a family that can never be had wears its glyph in
        // `WARN`, the way a layers row whose picture cannot be drawn does (§15 D179).
        //
        // **The middle state is what made this buildable at all.** Asked of
        // `is_family_available` alone, "still downloading" and "nobody has this font"
        // are the same answer, so the warning would have accused a font that was on
        // its way — which is why §5.4a carried this as a gap rather than a promise.
        // `family_status` is the distinction.
        let status = (!mixed).then(|| self.fonts.family_status(&shown));
        let label = if mixed {
            "Mixed".to_string()
        } else if self.fonts.is_loading(&shown) {
            format!("{shown}…")
        } else {
            shown
        };
        // Only the glyph. The family's *name* is the identity this row exists to
        // report, and dimming or annotating it would read as the pick not having
        // taken — which is the same mistake the ellipsis above was added to avoid.
        let head = match status {
            Some(crate::fonts::FamilyStatus::Missing) => {
                ui::glyph_and_text_tinted(icon::TEXT_AA, &label, theme::color::WARN)
            }
            _ => ui::glyph_and_text(icon::TEXT_AA, &label),
        };
        let mut chosen: Option<String> = None;
        let width = ui.available_width();
        // **A combo paints exactly `interact_size.y`, so the row states it** — and
        // in a `scope`, or the height would leak into the fields below. Left to the
        // theme this one came out 24px under the 28px fields beside it; the measured
        // symptom was 24 / 26 / 28 down a single card (§15 D85).
        ui.scope(|ui| {
            ui.spacing_mut().interact_size.y = ROW_H;
            ui.spacing_mut().button_padding.y = 0.0;
            let combo = egui::ComboBox::from_id_salt("font-family")
                .icon(ui::combo_chevron)
                .width(width)
                // **This is the one combo whose popup you are meant to click into**,
                // and egui's default does not allow it: `PopupCloseBehavior` for a
                // `ComboBox` is `CloseOnClick`, which is a click *anywhere, inside or
                // outside*. So clicking the search field this list exists to be
                // filtered by dismissed the list. Reported.
                //
                // The rest of the app's combos are lists of rows and want the
                // default. The cost here is that picking a row no longer closes the
                // popup either — nothing does, since every click is now "inside" —
                // which is what the explicit `ui.close()` below is for, and it makes
                // the row the only thing that dismisses this list.
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .selected_text(head)
                .show_ui(ui, |ui| {
                    // The list is its own rhythm: rows draw at `FAMILY_ROW_H` by hand,
                    // and the search field must not inherit a 28px control height.
                    ui::menu_rows(ui);
                    // egui's default text-edit margin is 4x2, which next to 24px rows
                    // reads as a cramped afterthought rather than the field you type
                    // into first. The gap below it is tightened at the same time: the
                    // theme's 6px item spacing was set for stacked panel rows, and
                    // here it separates a field from a list that belongs to it.
                    ui.spacing_mut().item_spacing.y = 3.0;
                    ui.add(
                        egui::TextEdit::singleline(&mut self.font_filter)
                            .margin(egui::Margin::symmetric(FILTER_PAD_X, FILTER_PAD_Y))
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
                                if self.family_row(ui, &name, &current) {
                                    chosen = Some(name);
                                    // The popup's own `Area` is `closable`, so this
                                    // reaches it from inside the scroll area.
                                    ui.close();
                                }
                            }
                        },
                    );
                })
                .response;
            // **The sentence hangs off the combo's own head response**, which is the
            // one place it fires: a tooltip on the wrapping `ui.scope` is silently
            // dead, the scope's response being a placeholder over the rect rather
            // than a registered widget (`inspector::menu_action`). A colour on its
            // own says *something* is wrong and never says what, and a tooltip is the
            // only text this row has room for.
            if status == Some(crate::fonts::FamilyStatus::Missing) {
                combo.on_hover_text(
                    "This font is not installed and cannot be downloaded — \
                     the text is drawn in the fallback face",
                );
            }
        });
        if let Some(family) = family_actually_changed(chosen, &current, mixed) {
            self.fonts.ensure_loaded(&family);
            // **The axis and feature settings go with the family.** They are named
            // by tag, and a tag means something different in the next typeface —
            // `wdth 75` on a family with no width axis is silently nothing, and
            // `ss01` is a different alternate everywhere. Carrying them over is
            // how a font swap comes out looking wrong in a way nothing on screen
            // explains.
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

    /// One row of the family list: the name, set in the family it names.
    ///
    /// See [`family_actually_changed`], which decides whether a click on this row
    /// is an edit at all.
    ///
    /// Returns whether it was clicked. Falls back to the UI font when there is no
    /// truthful preview to draw — the bytes have not arrived, or the family is a
    /// symbol font whose Latin name would come out as a row of empty boxes.
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
        // **The dot's room is reserved on every row, lit or not** (§15 D353), so a
        // name never reflows at the moment its face arrives — a list of 1,976 rows
        // twitching as downloads land is worse than the uncertainty the dot is
        // there to remove.
        let text = rect
            .shrink2(egui::vec2(7.0, 0.0))
            .with_max_x(rect.right() - LOADING_DOT_SLOT);
        // Asked for on every row that draws, not only on the ones that miss: it
        // is also the LRU touch that keeps a family on screen from being evicted
        // while it is being looked at.
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
        // **After the name, and asked *after* `ensure_preview`**: that call is what
        // puts the face in flight, so on the first frame a row appears the dot
        // would otherwise be a frame late.
        if self.fonts.is_loading(family) {
            paint_loading_dot(
                ui,
                egui::pos2(rect.right() - LOADING_DOT_SLOT * 0.5, rect.center().y),
            );
        }
        resp.clicked()
    }

    /// The variant dropdown: the family's named instances, or its faces.
    ///
    /// **Plus a `Custom` row, shown only when the current settings match no
    /// variant** — the same shape as the stroke panel's `DashStyle::Custom`, and
    /// classified from the values rather than stored beside them, so a hand-tuned
    /// axis cannot leave the field claiming "Bold".
    fn type_variant_field(&mut self, ui: &mut egui::Ui, subject: &TypeSubject, size: egui::Vec2) {
        let family = attr!(subject, Family, Family);
        let variants = text::family_variants(&family);
        let weight = attr!(subject, Weight, Weight);
        let italic = attr!(subject, Italic, Italic);
        let coords = attr!(subject, Variations, Variations);
        let current = variants
            .iter()
            .position(|v| matches_variant(v, weight, italic, &coords));
        let label = if subject.mixed(CharAttrKind::Weight) || subject.mixed(CharAttrKind::Italic) {
            "Mixed".to_string()
        } else {
            match current {
                Some(i) => variants[i].name.clone(),
                None => "Custom".to_string(),
            }
        };
        // A family with no faces to choose between has nothing to offer here, so
        // the slot falls back to the bold/italic pair — which is still the fastest
        // way to ask for those two and is what every keyboard shortcut means.
        if variants.len() < 2 {
            self.type_weight_toggles(ui, subject, size);
            return;
        }
        let mut pick: Option<usize> = None;
        ui.scope(|ui| {
            // The combo paints its border *inside* its rect, so its height is `size.y`
            // whole — subtracting the hairline (right for content inside a `field_row`)
            // left it 2px short of the fields beside it (§15 D85).
            ui.spacing_mut().interact_size.y = size.y;
            ui.spacing_mut().button_padding.y = 0.0;
            egui::ComboBox::from_id_salt("font-variant")
                .icon(ui::combo_chevron)
                .width(size.x)
                .selected_text(ui::glyph_and_text(icon::TEXT_BOLD, &label))
                .show_ui(ui, |ui| {
                    ui::menu_rows(ui);
                    ui.spacing_mut().button_padding.y = 2.0;
                    for (i, v) in variants.iter().enumerate() {
                        if ui.selectable_label(current == Some(i), &v.name).clicked() {
                            pick = Some(i);
                        }
                    }
                    // Shown, and inert: it is a readout of "none of the above",
                    // not a thing to choose. Picking it could only mean "leave the
                    // axes as they are", which is what not opening the list does.
                    if current.is_none() {
                        ui.add_enabled(false, egui::Button::new("Custom"));
                    }
                });
        });
        if let Some(i) = pick {
            let v = &variants[i];
            let mut merged = coords.clone();
            for axis in &v.coords {
                match merged.iter_mut().find(|a| a.tag == axis.tag) {
                    Some(existing) => existing.value = axis.value,
                    None => merged.push(*axis),
                }
            }
            self.apply_char_attrs(
                subject,
                vec![
                    CharAttr::Weight(v.weight),
                    CharAttr::Italic(v.italic),
                    CharAttr::Variations(merged),
                ],
            );
        }
    }

    /// The bold/italic pair, for a family that offers no variant list.
    fn type_weight_toggles(&mut self, ui: &mut egui::Ui, subject: &TypeSubject, size: egui::Vec2) {
        let weight = attr!(subject, Weight, Weight);
        let italic = attr!(subject, Italic, Italic);
        let cell = (size.x - 2.0) / 2.0;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 2.0;
            let (bold_on, bold_next) = bold_toggle(weight, subject.mixed(CharAttrKind::Weight));
            if field_button(
                ui,
                icon::TEXT_BOLD,
                cell.min(size.y),
                14.0,
                ui::FieldButton::on_if(bold_on),
            )
            .on_hover_text("Bold")
            .clicked()
            {
                self.apply_char_attrs(subject, vec![CharAttr::Weight(bold_next)]);
            }
            let italic_on = italic && !subject.mixed(CharAttrKind::Italic);
            if field_button(
                ui,
                icon::TEXT_ITALIC,
                cell.min(size.y),
                14.0,
                ui::FieldButton::on_if(italic_on),
            )
            .on_hover_text("Italic")
            .clicked()
            {
                self.apply_char_attrs(subject, vec![CharAttr::Italic(!italic_on)]);
            }
        });
    }

    /// Size: `T` prefix, px only, no unit toggle.
    ///
    /// **No `Length` here on purpose.** A font size in ems would be a size
    /// relative to itself; px is the only unit it can have, so offering a suffix
    /// to click would be offering a state that cannot exist.
    ///
    /// Returns the field's own `Response`, as `multi_number` and `scale_row` do
    /// and for the same reason: the widget's rect is not predictable from
    /// outside, so a test that wants to click into it has to draw it once to
    /// find where it is. Nothing in production reads the value.
    fn type_size_field(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        size: egui::Vec2,
    ) -> egui::Response {
        let mut value = subject.font_size();
        let resp = value_field(
            ui,
            size,
            Prefix::Icon(icon::TEXT_T),
            &mut value,
            Scrub::whole(0.5).range(MIN_FONT_SIZE..=MAX_FONT_SIZE),
            |d| {
                let d = d.custom_formatter(ui::number(2));
                mixed_text(d, subject.mixed(CharAttrKind::Size))
            },
        );
        self.char_valve(&resp, subject, CharAttr::Size(value));
        resp
    }

    /// Line height: `Auto`, a percentage of the font size, or absolute px.
    ///
    /// **Three states on one field**, cycled by the unit suffix. Auto is
    /// `MetricsRelative(1.0)` — the font's own line height, CSS `normal` — and is
    /// a genuinely different value rather than a collapsed control, which is why
    /// it lives on the suffix beside the other two units rather than in a fourth
    /// widget.
    fn type_line_height_field(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        size: egui::Vec2,
    ) {
        let current = match subject.shown(CharAttrKind::LineHeight) {
            CharAttr::LineHeight(v) => v,
            _ => None,
        };
        let mixed = subject.mixed(CharAttrKind::LineHeight);
        let font_size = subject.font_size();
        let (unit_label, tooltip) = match current {
            None => ("auto", "The font's own line height — click for %"),
            Some(Length::Em(_)) => ("%", "Percent of the font size — click for px"),
            Some(Length::Px(_)) => ("px", "Absolute — click for auto"),
        };
        let suffix = Some(Suffix {
            text: unit_label,
            clickable: true,
            tooltip,
        });
        // **In Auto the digits show what Auto is worth right now**, in px, and the
        // suffix is what says it is automatic — the field's own three-state idiom,
        // so the number needs no decoration to be read as derived. It printed the
        // word "Auto" over an unscrubbable zero until 2026-08-04, which said the
        // state twice and the value never (§15 D162). Only a node with nothing laid
        // out falls back to that.
        let resolved = subject.line_height;
        let (mut shown, scrub, typed) = match current {
            // Auto shows px and cannot be typed into — a zero-width scrub range,
            // and the write below is gated on `current` being `Some` — so its unit
            // is the honest one rather than a reachable one.
            None => (
                resolved.unwrap_or(0.0),
                Scrub::whole(1.0).range(0.0..=0.0),
                expr::Unit::Px,
            ),
            Some(Length::Em(m)) => (
                m * 100.0,
                Scrub::whole(0.5).range(MIN_LINE_HEIGHT_PCT..=MAX_LINE_HEIGHT_PCT),
                expr::Unit::Pct,
            ),
            // ⚠️ **Derived, like every other `Length` face in this panel.** This
            // is the field `MAX_LINE_HEIGHT_PX` was actually named for, and even
            // here the two ends did not describe one quantity: 1000% against
            // 10 000px is 125× apart at 8pt. The sign rule always agreed (both
            // floored at zero); the magnitudes never did. See [`Bounds`].
            Some(Length::Px(v)) => (
                v,
                Scrub::whole(0.5).range(px_range_for(
                    MIN_LINE_HEIGHT_PCT..=MAX_LINE_HEIGHT_PCT,
                    font_size,
                )),
                expr::Unit::Px,
            ),
        };
        let (resp, unit_clicked) = value_field_suffixed(
            ui,
            size,
            Prefix::Icon(icon::ARROWS_VERTICAL),
            suffix,
            &mut shown,
            scrub,
            // Its own parser, for [`length_field`]'s reason: px and % on one
            // field, so the one it is not showing is refused (§15 D359).
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
        if unit_clicked {
            self.apply_char_attrs(
                subject,
                vec![CharAttr::LineHeight(next_line_height(
                    current, font_size, resolved,
                ))],
            );
            return;
        }
        if let Some(l) = current {
            let next = match l {
                Length::Em(_) => Length::Em(shown / 100.0),
                Length::Px(_) => Length::Px(shown),
            };
            self.char_valve(&resp, subject, CharAttr::LineHeight(Some(next)));
        }
    }

    /// Letter spacing: `%` by default, with a `px` alternative.
    ///
    /// **`%` is the default because tracking has to scale with size**, or a type
    /// token cannot carry it: the same style used at 12pt and 48pt wants the same
    /// *proportional* tracking, and a stored px value is wrong at one of the two.
    fn type_letter_spacing_field(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        size: egui::Vec2,
    ) {
        self.length_char_field(
            ui,
            subject,
            size,
            CharAttrKind::LetterSpacing,
            Prefix::Icon(icon::TEXT_AA),
            Bounds::TRACKING,
            "Letter spacing",
        );
    }

    /// Horizontal and vertical alignment, and the button that opens the popup.
    ///
    /// **Both alignments live here and nowhere else.** They are the controls a
    /// designer reaches for without thinking, and putting either in the popup
    /// would put a daily control two clicks away to make room for ones that are
    /// consulted monthly.
    ///
    /// Returns the popup button's rect, which [`Self::type_menu_popup`] needs in
    /// order not to dismiss itself on the click that opened it.
    fn type_alignment_row(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
    ) -> Option<egui::Rect> {
        // **The row is on the *column* grid of the fields above it, not on its own
        // module.** Two passes got this wrong in opposite directions. First the
        // horizontal set took the row's remainder while the vertical set was pinned
        // at 22 a cell: 37 wide against 20, two alignment controls side by side
        // drawn to two different sizes. Then both were pinned at the design's 28,
        // which made the cells equal and left the tracks off the grid — measured
        // and reported, the horizontal track ended 2pt short of the field above it
        // and the vertical track started 3pt left of the field above *that*.
        //
        // Those two goals do not both fit: four cells across a half-card and three
        // across the rest is not an equal division at any card width. The grid
        // wins, because the row is read against the two rows of paired fields
        // directly above it, and a 1pt difference between the two tracks' cells is
        // not something the eye can see where a 2pt step in a column edge is.
        let full = ui.available_width();
        // The same `half` the field rows use, so the first track's right edge lands
        // on the first field's — and the second track therefore starts on the
        // second field's left edge, since both gaps are the card's own.
        let horizontal_w = (full - GAP) / 2.0;
        // The remainder, which puts the popup button's right edge on the margin.
        let vertical_w = full - horizontal_w - ROW_H - GAP * 2.0;
        let gap = GAP;
        let mut button = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = gap;
            let align = subject.paragraph.align;
            if let Some(i) = segmented(
                ui,
                horizontal_w,
                SEG_CELL_H,
                4,
                align.cell(),
                |p, i, rect, on| {
                    segment_glyph(p, rect, ALIGN_GLYPHS[i], on);
                },
            ) && TextAlign::OFFERED[i] != align
            {
                let mut next = subject.paragraph.clone();
                next.align = TextAlign::OFFERED[i];
                self.commit_paragraph(subject.id, next);
            }
            let valign = subject.block.vertical_align;
            let current = VerticalAlign::ALL
                .iter()
                .position(|v| *v == valign)
                .unwrap_or(0);
            let enabled = subject.sizing.height_is_authored();
            let resp = ui.scope(|ui| {
                // **Dimmed where there is no free space to distribute.** An
                // auto-height box is exactly as tall as its text, so all three
                // cells are the same drawing — the same per-kind gate the stroke
                // panel puts on `Join`.
                if !enabled {
                    ui.disable();
                }
                if let Some(i) =
                    segmented(ui, vertical_w, SEG_CELL_H, 3, current, |p, i, rect, on| {
                        segment_glyph(p, rect, VALIGN_GLYPHS[i], on);
                    })
                    && VerticalAlign::ALL[i] != valign
                {
                    let next = BlockStyle {
                        vertical_align: VerticalAlign::ALL[i],
                        ..subject.block
                    };
                    self.commit_block(subject.id, next);
                }
            });
            // Through a claimant of its own: a `Ui::scope`'s response is a
            // placeholder rather than a registered widget and never reports a
            // hover, and `on_hover_text` is gated on `Response::enabled()`, which
            // is false inside a disabled scope. Hung on the scope it showed
            // nothing, silently (`inspector::menu_action` is the pattern).
            if !enabled {
                ui.interact(
                    resp.response.rect,
                    ui.id().with("dimmed-valign-row"),
                    egui::Sense::empty(),
                )
                .on_hover_text("Only a fixed-height box has room to align in");
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

    /// The three-tab overflow panel.
    ///
    /// An `Area` rather than rows in the card, for the same reason the stroke
    /// options are one: the card is 296px wide and this is up to nine controls,
    /// so opening it inline would push every panel below it down the page every
    /// time it was consulted.
    ///
    /// `head` is the rect of the button that opens it — see the dismiss test at
    /// the bottom for why the popup cannot do without it.
    fn type_menu_popup(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        head: Option<egui::Rect>,
    ) {
        let Some(tab) = self.type_menu else { return };
        // **The popup belongs to the button that opens it, so it goes away when
        // that button does.** A collapsed Type panel draws no button, and a
        // popover with no opener is a panel quietly editing something the card no
        // longer shows — the same rule the detached picker follows when its target
        // stops being selected.
        let Some(head) = head else {
            self.type_menu = None;
            return;
        };
        let ctx = ui.ctx().clone();
        let mut close = false;
        // Clear of the inspector, to its left, aligned with the row that opened it
        // — see `OndinApp::popover_anchor` for why it is not under the button.
        let anchor = OndinApp::popover_anchor(&ctx, head, MENU_W);
        // Whether the pointer is over an overlay rather than over the page — the
        // combo lists inside this popup are their own `Area`s at the same order,
        // so a click in one is a click that misses `menu.contains_pointer()`. Read
        // as a question about layer order rather than about popups, exactly as the
        // stroke menu does it.
        let over_overlay = ctx
            .pointer_interact_pos()
            .and_then(|p| ctx.layer_id_at(p))
            .is_some_and(|l| l.order >= egui::Order::Foreground);
        // `content_rect`, as the picker's placement reads it — the window minus
        // whatever the platform has taken off it.
        let screen = ctx.input(|i| i.content_rect());
        // **The tallest the tab body may become, which is the window less the
        // chrome around it.** With a variable font, a decoration showing its four
        // controls and `opsz` on Manual the Character tab measures a little over
        // 700pt — which is what folding the Font tab into it bought, and is taller
        // than the anchor leaves below a Type panel sitting anywhere but the top of
        // the inspector. The design draws one long card and says nothing about the
        // case, so the card scrolls: invisible whenever it fits, which is every
        // other tab and most fonts (§15 D104).
        //
        // **The border is in the subtraction too**, since 2026-08-23 — the same two
        // points the widths lost (§15 D307), on the axis nobody was looking at. A
        // `Frame`'s stroke grows its outer rect on *both* axes, so a budget that
        // counted only the padding let the tallest card stand 2pt past the margin it
        // was computed to respect. Spelled out rather than routed through
        // `ui::menu_inner_w`: that one is about a width the design names, and this is
        // a height taken from whatever the screen has left.
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
            // The anchor is where the card *wants* to be; this is what keeps it on
            // screen when the button it hangs from is near the bottom.
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
                    // No line of hint text under the tabs. There was one, spelling
                    // out the scope the abbreviated label could not — and the
                    // labels are whole words now, so it would be the same sentence
                    // twice.
                    //
                    // The tab strip stays outside the scroll: it is how you leave a
                    // tab, and a tab you have scrolled to the bottom of is exactly
                    // where you need it.
                    //
                    // **The body is given an explicit rect, not the room the `Area`
                    // has.** A `ScrollArea` takes its outer size from
                    // `available_rect_before_wrap()`, and inside an `Area` that is
                    // the area's own size *from the previous frame* — so the height
                    // ratcheted downwards and never came back: open on Character
                    // and the card is tall, switch to Box and it shrinks to fit,
                    // switch back and Character is now stuck at Box's height,
                    // Paragraph shrinks it again, and so on. Reported exactly that
                    // way. Pinning `max_rect` breaks the feedback loop — the cap is
                    // the same number every frame whatever the last tab did — and
                    // `auto_shrink` on the y axis is then free to do the part that
                    // *is* wanted, which is a short tab drawing a short card
                    // (§15 D106). The same trap `inspector_ui` documents, and the
                    // same fix.
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
                                    TypeTab::Character => self.type_character_tab(ui, subject),
                                    TypeTab::Paragraph => self.type_paragraph_tab(ui, subject),
                                    TypeTab::Block => self.type_block_tab(ui, subject),
                                }
                            });
                    });
                });
            })
            .response;
        let on_head = ctx.pointer_interact_pos().is_some_and(|p| head.contains(p));
        // **The picker this popup opens is a third thing to forgive.** It is an
        // `egui::Window` at `Order::Middle`, so it is neither the popup's rect nor
        // an overlay above it — a click in it satisfies every term of the dismissal
        // and would close the popup the picker belongs to, taking the picker with
        // it (see the orphan rule in `picker_ui`). Asked by layer *identity* rather
        // than by rect, because the window's rect is not known here: `picker_ui`
        // runs after the inspector, so this frame's rect does not exist yet and
        // last frame's would be a frame stale on the first click.
        let on_picker = ctx
            .pointer_interact_pos()
            .and_then(|p| ctx.layer_id_at(p))
            .is_some_and(|l| l.id == super::picker::layer_id());
        // The sixth term, recorded every frame rather than only on the release —
        // never from inside the short-circuiting `&&`.
        let press_away = super::press_began_away(&ctx, menu.layer_id.id, menu.rect, head);
        if close
            || dismissed_by_click(ClickAway {
                // **The primary button only.** Right-click is spent on cancelling
                // a gesture everywhere in this app, and a scrub started in the
                // popup is cancelled with the pointer wherever it has been dragged
                // to — usually out over the canvas. Read as `any_click`, that
                // cancel was also a click on the page, so cancelling a drag closed
                // the popup you were dragging in. Reported exactly that way, along
                // with the tell: cancelling *inside* the popup worked, because
                // there the click landed on `on_menu`.
                clicked: ctx.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary)),
                over_overlay,
                on_menu: menu.contains_pointer(),
                on_head,
                on_picker,
                // Something is still being dragged, or a cancel is waiting for its
                // button to come up — `gesture_cancelled` is cleared at the end of
                // the frame the release arrives in, so it is still true here.
                in_gesture: ctx.dragged_id().is_some() || self.gesture_cancelled,
                press_away,
            })
        {
            self.type_menu = None;
        }
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.type_menu = None;
        }
    }

    // --- tab 1: Character -------------------------------------------------

    /// Case, the one decoration, the two spacings — and everything the *family*
    /// offers: the optical-size mode, the language, the variable axes and the
    /// OpenType features.
    ///
    /// **The lower half builds itself from the font.** Which axes exist, their
    /// ranges and which features are available are the typeface's decisions; a
    /// hardcoded list is wrong the first time a family ships a `GRAD`, and a
    /// switch for a feature the face does not have is a control that shapes to
    /// nothing. The generated sections come last, after the ones that are always
    /// there, so choosing a different family does not move the top of the tab.
    fn type_character_tab(&mut self, ui: &mut egui::Ui, subject: &TypeSubject) {
        self.color_section(ui, subject);
        self.case_section(ui, subject);
        self.decoration_section(ui, subject);
        self.spacing_section(ui, subject);

        let family = attr!(subject, Family, Family);
        let axes = text::family_axes(&family);
        let coords = attr!(subject, Variations, Variations);
        self.optical_size_section(ui, subject, &axes, &coords);
        self.language_section(ui, subject);
        self.axes_section(ui, subject, &axes, &coords);
        self.features_section(ui, subject, &text::family_features(&family));
    }

    /// The text's own colour, over whatever range a character control acts on
    /// (§15 D154).
    ///
    /// **First in the tab, because it is the attribute a selection is most often
    /// made to change**, and because the two rows below it — Case and Decoration —
    /// both describe ink that inherits from here. Reading downwards then goes from
    /// what the letters are drawn in to what is done to them.
    ///
    /// **Absence means the layer's fill stack**, which is what keeps the Fill panel
    /// meaningful for text and needs no migration: a document with no run colours is
    /// a document where every run inherits, exactly as before. It is also why the
    /// swatch can show a *gradient* while the slot itself holds only one colour — a
    /// run cannot be filled with a ramp (D154), but it can inherit a layer that is.
    fn color_section(&mut self, ui: &mut egui::Ui, subject: &TypeSubject) {
        section(ui, "Colour", |ui| {
            self.char_color_row(ui, subject, CharSlot::Color);
        });
    }

    /// **Case is a display transform, and small caps is not here.** Real drawn
    /// small capitals are a *feature* (`smcp`, in the OpenType list at the bottom
    /// of this same tab); a case transform is a pre-shaping substitution.
    /// Offering them side by side would suggest they are two ways of asking for
    /// one thing.
    fn case_section(&mut self, ui: &mut egui::Ui, subject: &TypeSubject) {
        let case = attr!(subject, Case, Case);
        let mixed = subject.mixed(CharAttrKind::Case);
        let current = TextCase::ALL.iter().position(|c| *c == case).unwrap_or(0);
        section(ui, "Case", |ui| {
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
    }

    /// The decoration, and everything under it: thickness, offset, line style and
    /// colour, shown only when there is one.
    ///
    /// **One of three, though the model can hold both.** `TextStyle` carries an
    /// underline and a strikethrough independently and the renderer draws
    /// whichever are set; the control offers a choice between them. That is a
    /// deliberate narrowing, not a limitation leaking through: a line under *and*
    /// through the same run is a mistake far more often than a request, and
    /// supporting the pair costs two toggles, two collapsible blocks of four
    /// controls each, and a reader who has to check both to know what is on. One
    /// track says the whole state at a glance. Picking a decoration clears the
    /// other and **carries the tuning across**, so switching between them is a
    /// change of which line, not a reset of how it is drawn.
    ///
    /// **Each of the three numbers has a `None` state meaning "the font
    /// decides"** — the same "absence is a state" pattern as `Option<Pivot>`, and
    /// what keeps the value correct across a font swap: a stored 1.2px underline
    /// is wrong the moment the family changes, where the face's own `post` entry
    /// is right in both.
    fn decoration_section(&mut self, ui: &mut egui::Ui, subject: &TypeSubject) {
        let under = decoration_of(subject, CharAttrKind::Underline);
        let strike = decoration_of(subject, CharAttrKind::Strikethrough);
        let mixed =
            subject.mixed(CharAttrKind::Underline) || subject.mixed(CharAttrKind::Strikethrough);
        let current = decoration_cell(under, strike);
        section(ui, "Decoration", |ui| {
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
            // Mixed has no one decoration to detail, and showing the first run's
            // would be a claim the next edit would make true of all of them.
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
                    // **Unsigned**: a negative thickness has no meaning (§15 D546).
                    false,
                    // What the face is drawing, so leaving *Font* changes the unit
                    // and not the line (§15 D570). Filtered above zero because a
                    // seed of zero is the bug this closes — a face that reports no
                    // thickness has nothing to offer and falls back to the old
                    // behaviour rather than pretending to.
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
                // **An arrow that points, rather than one that spans.** The offset
                // is positive-*up*, so pushing an underline clear of the descenders
                // means typing a negative number — and a symmetric double-headed
                // arrow (which is also line height's prefix, two fields away) said
                // "vertical" while saying nothing about which way. One glyph is the
                // whole hint; the tooltip carries the rest.
                let e = optional_length_field(
                    ui,
                    egui::vec2(half, CELL),
                    Prefix::Icon(icon::ARROW_UP),
                    d.offset,
                    font_size,
                    // **Signed**, and this is the field the shared range was
                    // written for.
                    true,
                    // Nothing to seed from — the face's own offset is on the same
                    // metrics, but its sign is parley's and this field's is not
                    // (§15 D151, D570).
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
            // **Solid, dashed, dotted, wavy — ours, not parley's.** parley reports
            // where a decoration goes and how thick it is; how the ink is broken
            // up reuses the stroke dash machinery at the render boundary (§6.3).
            //
            // A dropdown, not a four-cell track: the four names are words rather
            // than pictures, and four words in 250px is the width at which a
            // segmented control starts abbreviating.
            //
            // **Beside it, on an underline only, the skip-ink toggle** (§15 D357).
            // It belongs on this row because it answers the same question the line
            // style does — what the band *looks like* — and not the two rows above,
            // which are where the band is and how thick. `Decoration::skip_ink` is
            // read for underlines and never for a strikethrough (a `line-through` is
            // meant to cross the letters, css-text-decor-4), so the button is absent
            // rather than disabled on one and the dropdown takes the whole width back:
            // a control that cannot do anything is worse than no control, and the
            // model field it would write is one nothing reads.
            //
            // **Lit is the default state here**, which is unusual for this chrome and
            // is the honest reading: the button says what the line is doing, as
            // `TEXT_BOLD` does, and skipping is on unless it is switched off. The
            // alternative — lighting the button for the *non*-default — would mean a
            // dark toggle beside a broken line and a lit one beside a continuous
            // rule, which is the state backwards.
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
    }

    /// The decoration's colour: a swatch, its hex, its opacity, and a reset.
    ///
    /// **Absence means the text's own colour**, which is what makes it the
    /// default — recolour the text and the decoration follows. The field draws
    /// that state with the *resolved* colour ([`TypeSubject::text_color`]) rather
    /// than a placeholder grey, because a grey chip would be a claim about a
    /// colour the panel does not know; and the reset button is what tells the two
    /// states apart, present only while the colour is this decoration's own.
    ///
    /// The hex is buffered like the paint rows', and for the same reason: half of
    /// `9184D9` is itself a legal colour, so parsing per keystroke would recolour
    /// the line four times on the way and leave four undo steps behind. The
    /// opacity goes through the drag valve, so a scrub is one step.
    ///
    /// **The swatch opens the detached picker**, as every other swatch in the app
    /// does, through a `PaintSlot` of its own
    /// ([`crate::panels::paint::PaintSlot::TextDecoration`]). It
    /// was a bare preview in the first pass, on the grounds that the picker is
    /// written against a paint list and a decoration is not one — which is true of
    /// `slot_transaction` and of nothing else, so the fix was one diverted write
    /// rather than a second colour panel.
    fn char_color_row(&mut self, ui: &mut egui::Ui, subject: &TypeSubject, slot: CharSlot) {
        let kind = slot.kind();
        // The colour stored *in this slot*, or `None` for the inherited state. Both
        // slots have one, which is what lets one row drive both.
        let own = match slot {
            CharSlot::Color => attr!(subject, Color, Color),
            CharSlot::Decoration(side) => decoration_of(subject, side.kind()).and_then(|d| d.color),
        };
        let targeted = self
            .picker
            .as_ref()
            .is_some_and(|p| p.node == subject.id && p.slot == slot.paint_slot());
        let shown = own.unwrap_or_else(|| subject.text_color());
        // The inherited paint's ramp, and only while it is being inherited: once the
        // slot has a colour of its own, the node's gradient has nothing to do with
        // what is drawn. The same question for both slots — a decoration inherits the
        // run's ink and a run inherits the node's fill, and `text_paint` is where that
        // chain bottoms out.
        let ramp = own.is_none().then(|| subject.text_ramp()).flatten();
        // Committed at once — a hex is finished when it loses focus, and a reset
        // is a click.
        let mut now: Option<Option<Color>> = None;
        // Valved — a scrub is one undo step, not one per frame.
        let mut opacity: Option<(egui::Response, f64)> = None;
        let mut open = false;
        // Where the row lands, so the picker opens beside it rather than beside
        // the whole popup.
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
                            .char_limit(ui::HEX_CHAR_LIMIT)
                            .font(egui::FontId::proportional(11.5)),
                    );
                    ui::select_all_on_focus(ui, &resp, &text);
                    if resp.gained_focus() || resp.changed() {
                        self.char_hex = Some((slot, text.clone()));
                    }
                    // Dropping the buffer here is what strips a typed or pasted
                    // `#`: the field falls back to `hex_of`, which never writes one.
                    // 🚨 **`Escape` drops the buffer and writes nothing** (§15
                    // D841). This read `if let Some(…) = parse_hex(&text)` with
                    // no `Escape` clause, so the key that cancels everywhere
                    // else committed the typed colour — `[X6-L1-02]`'s second
                    // site, and the same input §15 D808 records as the bug.
                    //
                    // ⚠️ **The clear is outside the guard and the write inside
                    // it.** Both have to happen on `Escape` except the write: a
                    // cancelled edit that kept its buffer would leave the
                    // abandoned text in the field.
                    if resp.lost_focus() {
                        self.char_hex = None;
                        if let Some([r, g, b]) =
                            ui::parse_hex(&text).filter(|_| ui::defocus_commits(&resp))
                        {
                            now = Some(Some(
                                Color::from_rgba8(r, g, b, 255).with_alpha(shown.components[3]),
                            ));
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
            // Live whenever there is *anything* to put back: this slot's own colour,
            // or a range that disagrees about it at all — see `TypeSubject::ragged`
            // for the bug the second term fixes.
            if reset_slot(ui, own.is_some() || subject.ragged(kind), slot.reset_tip()) {
                now = Some(None);
            }
        });
        if let Some(color) = now {
            if let Some(attr) = char_color_attr(subject, slot, color) {
                self.apply_char_attrs(subject, vec![attr]);
            }
        } else if let Some((resp, pct)) = opacity
            // **Only while the field is actually being used.** The valve gates on
            // the response, so an idle frame writes nothing either way — but it
            // would still *build* a transaction from a frame-start snapshot every
            // frame, and this row runs before the axes and the OpenType list. One
            // stale `SetTextStyle` reaching the valve on a frame where something
            // else had already committed would undo it.
            //
            // valve-gate-not-a-commit: the commit is `char_valve`'s, on its own
            // engagement latch; this only decides whether to reach it.
            && (resp.dragged()
                || resp.drag_stopped()
                || resp.changed()
                || resp.lost_focus())
        {
            // Fading an inherited colour adopts it: the alpha has to live somewhere,
            // and the only place either slot has for it is its own `color`.
            let faded = Some(shown.with_alpha((pct / 100.0) as f32));
            if let Some(attr) = char_color_attr(subject, slot, faded) {
                self.char_valve(&resp, subject, attr);
            }
        }
        // **Opening the picker adopts the colour first.** The picker writes one
        // colour into the slot, so a slot still inheriting has nothing for it to
        // point at — and the value it adopts is the one already on screen, so the
        // click changes what is *editable* and not what is drawn.
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
    /// Baseline shift is signed and is a *draw* offset — it moves the run without
    /// changing the line box, which is what keeps neighbouring baselines aligned.
    fn spacing_section(&mut self, ui: &mut egui::Ui, subject: &TypeSubject) {
        section(ui, "Baseline & word spacing", |ui| {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = COL_GAP;
                let half = (MENU_INNER - COL_GAP) / 2.0;
                self.length_char_field(
                    ui,
                    subject,
                    egui::vec2(half, CELL),
                    CharAttrKind::WordSpacing,
                    Prefix::Icon(icon::ARROWS_IN_LINE_HORIZONTAL),
                    Bounds::TRACKING,
                    "Word spacing",
                );
                self.length_char_field(
                    ui,
                    subject,
                    egui::vec2(half, CELL),
                    CharAttrKind::BaselineShift,
                    Prefix::Icon(icon::TEXT_SUPERSCRIPT),
                    Bounds::SIGNED_TRACKING,
                    "Baseline shift",
                );
            });
        });
    }

    /// `opsz`, above the axis list and with a mode of its own — **the one axis
    /// that has one**. Auto writes no coordinate at all, so the shaper takes the
    /// font size; Manual pins a value.
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
        section(ui, "Optical size", |ui| {
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
                    // Manual starts from what Auto was giving, so the switch
                    // itself changes nothing on screen.
                    // `FontAxis::clamp`, not `f64::clamp`, and the difference is
                    // a panic: an `fvar` axis record whose ends are reversed is
                    // something a font can simply contain, and `f64::clamp`
                    // asserts on the ordering in release as well as in debug.
                    next.push(AxisSetting::new(OPSZ, opsz.clamp(subject.font_size())));
                }
                self.apply_char_attrs(subject, vec![CharAttr::Variations(next)]);
                return;
            }
            if auto {
                return;
            }
            // A slider and its readout, with no name and no reset. The section
            // label already names the axis, and **Auto is the reset** — a button
            // beside it meaning "the font's default" would be a second way to
            // spell the cell directly above.
            self.opsz_row(ui, subject, opsz, coords);
        });
    }

    /// The list marker: a dropdown of `None` plus the eight kinds.
    ///
    /// **A `ComboBox` rather than a segmented track**, which is what the rest of this
    /// tab uses — nine cells will not fit the card's width, and the shape of the
    /// choice is the *same* as Language's, `None` plus a list of named options. The
    /// panel already answers that shape one way, and consistency of interaction beats
    /// matching the section above it.
    ///
    /// **Turning a marker on also opens a gutter for it, once.** The marker is
    /// right-aligned on the paragraph's own start edge (`ParagraphStyle::marker`), so
    /// at `indent_start` 0 it draws at a negative x — outside the box, which is the
    /// honest reading of "no gutter" and a poor first impression. A default is written
    /// only when the value is still zero, so a user who chose their own indent, or who
    /// deliberately zeroed it, is not overruled.
    ///
    /// **The nesting level sits under the dropdown, and only where there is a list.**
    /// `ParagraphStyle::level` indents any paragraph, marker or not — deliberately, so
    /// the field is never inert — but the level of a paragraph that is not an item is
    /// not a thing anyone comes to this card to set, and the row would be dead weight
    /// on every text node that is not a list. Reached only through an item, it also
    /// needs no gutter default of its own: the pick above has already written one.
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
        section(ui, "List", |ui| {
            ui.scope(|ui| {
                // `CELL` whole: a combo strokes inside its own rect (§15 D85).
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
                    // **A word, where every other field in this popup has a glyph.**
                    // Phosphor's nesting glyphs are trees and file hierarchies, which
                    // is the layers panel's language rather than this one's, and the
                    // three glyphs that would read as "indent" are all spoken for by
                    // the controls in the section above — the first-line pair and the
                    // two block indents. A fourth arrow-against-an-edge here would be
                    // the fifth member of a set of four.
                    Prefix::Text("Level"),
                    &mut level,
                    // Slow: nine values over the whole range, so a scrub that means to
                    // go in one step does not go in three. Typing is unaffected.
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
        // **Or the frame the scrub ended on**, which carries no new value and is the
        // one the valve commits on — the same rule the `Length` fields state at
        // [`length_field`]'s return, and for the same reason. Without it the last drag
        // of this field stayed in the preview.
        if let Some((resp, level)) = nest.filter(|(r, l)| *l != shown.level || r.drag_stopped()) {
            // Through the valve, like the four `Length` fields above it and for D108's
            // reason: a scrub previews and commits once, rather than once per frame.
            self.valve_paragraph(&resp, subject, ParaAttr::Level(level));
        }
        let Some(next) = pick else {
            return;
        };
        // `para_ragged` for the same reason the hanging pair needs it (§15 D164):
        // over a non-partial range `shown` reads the *first* paragraph, so picking the
        // value it already shows must still write when another paragraph disagrees.
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
        // **The friendly name closed as well as open** (§15 D704,
        // `[S6.3-L3-08]`). This was `current` verbatim, so the control
        // disagreed with itself between its two states: the list offered
        // *Chinese (Simplified)* and, once picked, the field read `zh-Hans`;
        // *German* became `de`. `LANGUAGES` is a `(tag, name)` table whose
        // stated job — *"languages worth offering by name"* — is exactly this
        // mapping, and it was honoured on one side of the control only. It was
        // also the one control in this file showing the user a raw wire value;
        // the two combos beside it, `enum_combo` and `feature_choice_row`, both
        // resolve their closed text through the same table their list uses.
        // 🚨 `feature_values`, which is the second of those, states the rule
        // in as many words: *"The list and the closed dropdown are two readings
        // of the same mapping."* So this was a rule written down in one place
        // and broken in the one beside it, which is the shape to look for.
        //
        // ⚠️ **An unlisted tag falls back to itself rather than to *None*.** A
        // document can carry a locale this table does not offer — an imported
        // `xml:lang`, or a file written against a longer list — and that is a
        // real setting the panel must not describe as absent.
        let label = match current.as_deref() {
            None => "None".to_string(),
            Some(tag) => LANGUAGES
                .iter()
                .find(|(t, _)| *t == tag)
                .map_or_else(|| tag.to_string(), |(_, name)| (*name).to_string()),
        };
        let mut pick: Option<Option<String>> = None;
        section(ui, "Language", |ui| {
            ui.scope(|ui| {
                // `CELL` whole: a combo strokes inside its own rect (§15 D85).
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
        if let Some(next) = pick {
            self.apply_char_attrs(subject, vec![CharAttr::Locale(next)]);
        }
    }

    /// Every `fvar` axis with a value of its own, minus the ones a dedicated
    /// control already drives — two controls for one axis is two chances to
    /// disagree — in a list that scrolls rather than growing the card.
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
        section(ui, "Axes", |ui| {
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
    }

    /// The face's OpenType features as switches, named, described on hover, and
    /// curated — over a search field.
    ///
    /// **Nearly all of them are switches; a `cvXX` the font declares more than one
    /// named value for is not**, and draws a [`feature_choice_row`] instead. That
    /// is the only row here that is not a toggle, and `values.len()` — the count
    /// out of the font's own `FeatureParams` — is the whole of what decides it.
    ///
    /// **Tabular figures (`tnum`) is the single highest-value feature in the
    /// list** — it is the difference between a column of numbers lining up and
    /// not — so the well-known tags sort to the top rather than being buried in
    /// `ss01`–`ss20` alphabetical order.
    ///
    /// **What the list leaves out is the reason it is readable at all.** A face
    /// with a full `GSUB`/`GPOS` offers `aalt`, `ccmp`, `locl`, `mark`, `mkmk`,
    /// `numr`, `dnom` and friends beside its stylistic sets, and every one of
    /// those is either required, applied per script by the shaper, or applied as
    /// a corollary of another feature — so a switch for it does not offer a
    /// choice, it offers a way to mis-shape text. [`REGISTRY`] carries the
    /// registry's own verdict per tag and *Show all features* is the way back to
    /// the full list; nothing about the **model** changes either way, since
    /// `CharAttr::Features` still carries any tag and a document that has one set
    /// keeps it.
    ///
    /// The search field is unconditional. It used to appear only past ten
    /// features, on the reasoning that below that it was more chrome than the list
    /// it filtered; but the section's *height* is already fixed by the scroll area
    /// under it, so the only thing the threshold bought was a card whose layout
    /// depended on which family happened to be selected.
    fn features_section(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        available: &[FaceFeature],
    ) {
        if available.is_empty() {
            return;
        }
        // 🚨 **This section still has no mixed state, and that half of
        // `[S6.2-L1-05]` is deliberately open** (§15 D636). Over a selection whose
        // runs disagree, every switch below draws **off** — because `set` resolves
        // at `range.start`, i.e. the first run — so a selection half of which has
        // `tnum` on reads as a column of dead switches. The Axes section beside it
        // was the same and is fixed, because a *number* already had this panel's
        // word for "I do not know": `mixed_text`'s dash, whose four other callers
        // cover about ten fields in this file. **A switch does not**, and
        // inventing a third knob state is a
        // visual decision rather than a consistency repair — which is why it is
        // named here and left. `ui::switch_row` and `ui::paint_switch` take `on:
        // bool` and would both have to grow the state.
        //
        // ⚠️ **The write half is a separate ruling and is *not* what this comment
        // is about**: a toggle over a mixed range writes `set.clone()` — the first
        // run's whole list — plus the change, over the entire selection, so the
        // other runs' features are dropped. Whether it should write only the tag
        // it names or say that it flattens is the maintainer's call.
        let set = attr!(subject, Features, Features);
        let mut ordered: Vec<(Tag, FeatureLabel)> = available
            .iter()
            .map(|f| (f.tag, feature_label(f)))
            .collect();
        ordered.sort_by_key(|(tag, _)| (feature_rank(*tag), tag.to_string()));
        // **A withheld feature the document has an opinion about is shown anyway**,
        // whichever way that opinion goes: a switch nobody can see is a state
        // nobody can undo. So the curation hides tags this text does not mention,
        // and a file that arrives with `locl 0` in it — or one written before the
        // list was curated — still shows the row it would take to change.
        let shown = |tag: &Tag, l: &FeatureLabel| l.offered || set.iter().any(|f| f.tag == *tag);
        // ⚠️ **The hidden count is *not* taken here** (§15 D691). It used to be,
        // and it was wrong by however many withheld features the search box was
        // excluding: two filters decide what the list contains and only one of
        // them is knowable at this point, because the needle is not read until
        // the field below has been drawn. It is counted next to the loop it has
        // to agree with instead.
        // **One writer for both kinds of row.** A switch and a choice row produce
        // the same thing — a tag and the value to write — and two `Option`s would
        // be two `apply_char_attrs` calls off one `set` clone in a frame where both
        // fired, which silently drops the first.
        let mut written: Option<(Tag, u16)> = None;
        section(ui, "OpenType", |ui| {
            field_row(ui, egui::vec2(MENU_INNER, CELL), |ui| {
                ui.spacing_mut().item_spacing.x = SEARCH_GAP;
                ui.label(theme::icon_text(
                    icon::MAGNIFYING_GLASS,
                    SEARCH_ICON_PT,
                    theme::text::FAINT,
                ));
                // **The room left, spelled out.** A `TextEdit` that asks for more
                // than its row has grows the row, and the row is what pins the
                // card's width — so this is the field's content box minus the
                // magnifier and the gap after it, not a round number that happened
                // to fit at the width the popup used to be.
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
            // **The search test, written once** (§15 D691), because the row loop
            // and the reveal's count have to answer it the same way — and the
            // whole of `[S6.3-L1-07]` is that they did not. The list applies
            // *both* this and `shown`; the count applied only `shown`, so with a
            // needle typed the collapsed reveal offered "(43 more)" and the click
            // added nothing, every one of the 43 being filtered out. That is the
            // "control that cannot change anything" state [`features_reveal`]
            // exists to suppress, reached by the door it does not test for.
            let matches = |tag: &Tag, l: &FeatureLabel| {
                needle.is_empty()
                    || l.name.to_lowercase().contains(&needle)
                    || tag.to_string().contains(&needle)
            };
            // Exactly the rows a click on *Show all features* would add: withheld
            // by the curation, and surviving the needle.
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
                        // 🚨 **The mixed readout, per tag** (§15 D752). `value`
                        // above resolves at `range.start` — the *first run* — so
                        // over a selection whose runs disagree this row used to
                        // draw the first run's answer as though it were the
                        // selection's, which is the one thing `mixed_text`'s doc
                        // says a control must not do: *"showing one of the several
                        // values reads as a claim that it is **the** value — which
                        // the next drag would then make true for all of them."*
                        let mixed = subject.feature_mixed(*tag);
                        let row = if mixed {
                            ui::switch_row_mixed(ui, &label.name, FEATURE_ROW_H)
                        } else {
                            ui::switch_row(ui, &label.name, value > 0, FEATURE_ROW_H)
                        };
                        // **The tooltip says what a click will do, because a mixed
                        // row is the one place the answer is not obvious.** On an
                        // ordinary row the switch's position says it; here there is
                        // no position, so the sentence carries it.
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
                            // Written as an explicit `0` rather than removed, because
                            // the two are not the same request: absent means "whatever
                            // the font does by default", where `liga 0` means "off" —
                            // and most faces have ligatures on by default.
                            //
                            // ⚠️ **A mixed row turns the feature *on*** rather than
                            // toggling off the first run's value: `value` is run 1's
                            // and a click on a row that shows no position has to mean
                            // something the user can predict from the row alone. The
                            // tooltip says which.
                            written = Some((*tag, if mixed { 1 } else { u16::from(value == 0) }));
                        }
                    }
                });
            // **The reveal, under the list rather than beside the search.** It
            // changes what the list *contains*, which is the scroll area's
            // business, and putting it in the field row would have it read as a
            // filter — the one thing it is not.
            //
            // Absent when the face has nothing withheld, because then it is a
            // control that cannot change anything — see [`features_reveal`], which
            // owns that question and is where the reported disappearance was.
            if let Some(text) = features_reveal(self.show_all_features, hidden)
                && ui
                    .add(
                        egui::Label::new(
                            egui::RichText::new(text)
                                .size(ui::SEGMENT_LABEL_PT)
                                .color(theme::text::FAINT),
                        )
                        .sense(egui::Sense::click())
                        // **A clickable label is not text.** egui's `Label` is
                        // selectable by default, which is what put an I-beam over a
                        // control that toggles the list — reported here first, and
                        // later reported again about every card eyebrow, which is
                        // why the app now turns the whole style off
                        // (`theme::install_style_into`). This is therefore no longer
                        // load-bearing and is kept as a local statement of what the
                        // row is: not text, whatever the style says.
                        //
                        // ⚠️ **And it *is* the fix now, because the `PointingHand`
                        // that used to sit under this line is gone.** The app shows
                        // the arrow over chrome everywhere (§9.2); the cursor
                        // changes only where the change means something. Measured
                        // when this row was written and still true: a selectable
                        // label reports `CursorIcon::Text` and `selectable(false)`
                        // reports `Default` — so the I-beam this row was reported
                        // for stays fixed by the call above, and the hand it also
                        // acquired was the part that broke the rule.
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
        if let Some((tag, value)) = written {
            self.write_feature_tag(subject, tag, value);
        }
    }

    /// Set one OpenType tag across the selection, leaving every **other** tag in
    /// each run exactly as it was (§15 D752, `[S6.2-L1-05]`).
    ///
    /// 🚨 **This used to be four lines and they flattened the selection.** It read
    /// `set.clone()` — the list `attr!` resolves at `range.start`, i.e. the *first
    /// run* — edited the one tag into it and applied that list to everything. So
    /// toggling `liga` over a range whose second half had `tnum` on wrote the
    /// first run's list over both, and the `tnum` was gone with nothing said. The
    /// maintainer's ruling: *"Write only the tag you named and leave each run's
    /// other features alone."*
    ///
    /// **Two arms, because a partial selection and a whole node are different
    /// questions rather than two spellings of one.** Over a *selection* the runs
    /// are real and each keeps its own list, which is what
    /// `EditorSession::style_selection_per_run` is for. Over a **whole node**
    /// there is nothing to preserve: a whole-node write flattens the spans by
    /// design, and `TypeSubject::mixed` is false there for exactly that reason —
    /// what the row shows is what the write will produce, so it is not lying.
    /// Routing the whole-node case through the per-run path would be a change to
    /// D130's rule rather than to this bug.
    ///
    /// ⚠️ **The merge is here rather than in core** — `style_selection_per_run`
    /// takes a closure over each run's own value and knows nothing about
    /// OpenType. A tag is absent-means-default, so a run that does not name the
    /// tag gains it and a run that does has its value replaced; both are the same
    /// two lines and neither disturbs a neighbouring tag.
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
    /// slider under them.
    ///
    /// **The name and the value share a line because the slider needs the whole
    /// width.** A 250px row split between a label, a field, a reset and a slider
    /// leaves the slider about 90px, which for a `wght` axis of 100–900 is under
    /// nine pixels per hundred units — a control you cannot aim. Two lines cost
    /// height in a list that scrolls anyway.
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
                // A fixed column, so the fields line up down a list of axes whose
                // names are all different lengths.
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
            // `true`, the same as the field above it — the reset slot is on this
            // row, so "back to the font's own value" has to mean *no coordinate*
            // whichever of the two controls arrives at it (§15 D569).
            self.axis_slider(ui, subject, axis, coords, width, true);
        });
    }

    /// `opsz`'s slider and its readout on one line — the shape the section's own
    /// label makes possible, the axis being named already.
    fn opsz_row(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        axis: &FontAxis,
        coords: &[AxisSetting],
    ) {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = COL_GAP;
            // `false` for `opsz`, where an absent coordinate is the **Auto** state
            // and dropping one at the default would silently change the mode —
            // `write_axis`'s own rule, now reaching the valve too.
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
        // **A dash where the selection disagrees**, which is this panel's own word
        // for it and was the one section that did not use it (§15 D636,
        // `[S6.2-L1-05]`). `subject.mixed` *was* asked **zero** times across
        // `axes_section` and `features_section` — this call is the first;
        // `ragged` is asked once, one line below, and its answer spent on the
        // reset button, so the code had the question right there and put the
        // answer somewhere else. Over a selection whose first word is `wght 300`
        // and whose second is `wght 800`, the field read **300** with no mark,
        // which is `mixed_text`'s own objection: *"showing one of the several
        // values reads as a claim that it is **the** value — which the next drag
        // would then make true for all of them"*.
        //
        // 🚨 **This is a fifth caller of a *departure* from §15 D130, and the
        // departure is the panel's rather than this line's.** D130 is *Resolved*
        // and says a control spells the word **"Mixed"** wherever five letters
        // fit, with the dash confined to *"exactly two"* places — a segmented cell
        // and a 16px swatch. `mixed_text` paints a dash into a `DragValue`'s
        // digits, and it had **four** callers before this one — Size, the list
        // *Level*, `length_char_field` (three fields) and `length_field` (five) —
        // plus the line-height field's hand-rolled copy. So the Type panel already
        // spells mixed differently from the inspector, which says the word at
        // three sites. **The dash is chosen here anyway, on D130's own argument
        // against itself**: *"The alternative is one field spelling the state
        // differently from the three beside it."* Matching D130's letter on this
        // one field would do exactly that, nine times over.
        //
        // ⚠️ **The panel-wide question is one ruling and is not taken here**:
        // either D130's *"exactly two"* is amended to name `mixed_text`, or the
        // Type panel's ten fields learn the word — and that second one carries
        // D130's own recorded trap, since egui appends a `suffix` *after* a custom
        // formatter and `"Mixed%"` is what the paint row already had to be fixed
        // for. **Do not cite this comment as the argument for a sixth dash.**
        //
        // ⚠️ **The slider below keeps its knob and that is deliberate**, the same
        // fork `multi_number` makes: a control has to operate from something real
        // even while the field beside it is *saying* it does not know, and a
        // knob has nowhere to be that means "mixed". What the dash buys is that
        // the *number* stops making a claim; the drag is relative either way.
        let mixed = subject.mixed(CharAttrKind::Variations);
        let resp = value_field(
            ui,
            egui::vec2(width, CELL),
            Prefix::Text(axis_prefix(axis.tag)),
            &mut value,
            // Tenths, matching the quantum `AxisSetting` stores at: a slider that
            // produced sixteen digits would fill the file with them.
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
            // **The reset is a click, so it takes the immediate write and not the
            // valve** — there is no drag for a falling edge to wait on, and a valve
            // handed a click would hold the edit until the *next* frame the field
            // happens to be engaged on. The same fork `decoration_write` and
            // `paragraph_length` already make.
            self.write_axis(subject, axis, coords, current, axis.default, resettable);
            return;
        }
        // **Valved, exactly as `axis_slider` below is** (§15 D569). The prefix strip
        // is a scrub, and routed through the immediate `write_axis` it committed one
        // undo step *per frame* of it — five frames of a `wght` drag gave
        // `undo_depth 0 → 5` and five values the user never stopped on. §9.3's *"one
        // gesture = one transaction = one undo step"*, and §15 D108's bug, still live
        // on the one control the paragraph fields' fix never reached.
        //
        // ⚠️ **`write_axis`'s own `value == current` guard could never absorb it**,
        // which is why nothing here looked wrong: `apply_char_attrs` is the *click*
        // path and sets no preview, so `current` is the last committed value on
        // every frame of the drag and the two are unequal every time.
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
        // **Valved, so a sweep across the rail is one undo step**, and handed over
        // on every frame rather than only when the value moved. `current` is read
        // through the preview, so during a drag it already *is* `value` — a
        // `value != current` guard here would be false on the release frame and the
        // commit would never arrive, which is the same trap `char_valve` documents.
        self.axis_valve(&resp, subject, axis, coords, value, clear_at_default);
    }

    /// Write one axis coordinate, if it moved.
    ///
    /// The decision is [`axis_coords_after`]; this is the arm that commits it
    /// outright, and [`Self::axis_valve`] is the arm that valves it.
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
    /// is under the hand, one commit when it is let go.
    ///
    /// `clear_at_default` means what it means in [`Self::write_axis`], and it is
    /// here for the reason it is there — an absent coordinate is not the same
    /// document as a coordinate that happens to equal the font's own value, and for
    /// `opsz` it is a different *mode*. ⚠️ **It was missing, and that was visible
    /// as a disagreement between two controls for the same number** (§15 D569):
    /// sliding `wght` exactly onto 400 stored `wght=400` while typing 400 into the
    /// field beside it dropped the coordinate, so the same axis at the same value
    /// saved two different files depending on which control set it.
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
    /// **Horizontal alignment is not here** — it is inline in the main panel, and
    /// nothing appears in both places. **Nor are the three wrapping controls**,
    /// which moved to the Box tab: where a line may break is a question about the
    /// box the text is being fitted into, and every one of them is inert when the
    /// box is auto-width.
    ///
    /// **Two scopes in one tab, and the split is not arbitrary.** The first section
    /// acts on the paragraphs the selection touches and can read *mixed*; the two
    /// below it act on the whole layer, because `Justify` and the base direction are
    /// one value per `Layout` (§15 D77). Hence the two locals: `shown` is what the
    /// spannable controls display, `p` is the node's own defaults, and a node-level
    /// control that built its transaction from `shown` would promote one paragraph's
    /// indent to the node's default without saying so.
    fn type_paragraph_tab(&mut self, ui: &mut egui::Ui, subject: &TypeSubject) {
        let p = subject.paragraph.clone();
        let shown = subject.shown_paragraph();
        let font_size = subject.font_size();
        section(ui, "Spacing & indent", |ui| {
            let half = (MENU_INNER - COL_GAP) / 2.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = COL_GAP;
                // Paragraph spacing is **absolute by default**: it belongs to the
                // layout's spacing scale, not to the type, so a 24px gap stays
                // 24px when the type size is tuned.
                let e = length_field(
                    ui,
                    egui::vec2(half, CELL),
                    Prefix::Icon(icon::PARAGRAPH),
                    shown.spacing,
                    subject.para_mixed(ParaAttrKind::Spacing),
                    font_size,
                    0.0..=MAX_TRACKING_PCT,
                    "Space before this paragraph",
                );
                self.paragraph_length(subject, &e, ParaAttr::Spacing);
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
                    -MAX_TRACKING_PCT..=MAX_TRACKING_PCT,
                    // **The tooltip follows the state, because the field does.**
                    // One value, two meanings, and the only other thing that says
                    // which is the prefix glyph flipping between two icons that
                    // are near-mirrors of each other.
                    if shown.hanging {
                        "Hanging indent — how far the continuation lines come in"
                    } else {
                        "First-line indent"
                    },
                );
                self.paragraph_length(subject, &e, ParaAttr::Indent);
            });
            // **Which lines the indent reaches — one value, two buttons.** It is
            // not a pair of independent toggles: an indent is either on the first
            // line of each paragraph or on the ones after it, never both and never
            // neither, so the two press as a segmented control would while keeping
            // the 26×26 square the rest of the popup is built from.
            //
            // (There was a third button here, CSS `each-line`, and it is gone: with
            // it off the indent reached only the first line of the whole *layout*,
            // so a three-paragraph node indented one line and left two flush. See
            // `ParagraphStyle::indent`.)
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
                    // **Neither lit when the range disagrees**, which is this pair's
                    // whole mixed state: there is no third square to light and a
                    // dash cannot be drawn on a glyph, so "not one of the two" is
                    // what says the selection has both in it. A press then makes it
                    // uniform, as a drag on a mixed field does.
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
            // **The two block indents, below the first-line pair they qualify.** They
            // hold every line of the paragraph where the field above holds one, which
            // is why they are arrows against an edge rather than a third and fourth
            // text glyph — see `icon::ARROW_LINE_LEFT`.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = COL_GAP;
                let e = length_field(
                    ui,
                    egui::vec2(half, CELL),
                    Prefix::Icon(icon::ARROW_LINE_LEFT),
                    shown.indent_start,
                    subject.para_mixed(ParaAttrKind::IndentStart),
                    font_size,
                    -MAX_TRACKING_PCT..=MAX_TRACKING_PCT,
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
                    -MAX_TRACKING_PCT..=MAX_TRACKING_PCT,
                    // Says what it does *and* where it does nothing: an auto-width
                    // node has no wrap width for an end indent to narrow (§15 D163),
                    // and "inert" is indistinguishable from "broken" unnamed.
                    if matches!(subject.sizing, TextSizing::Auto) {
                        "Indent from the end edge — needs a box to measure from"
                    } else {
                        "Indent from the end edge — every line"
                    },
                );
                self.paragraph_length(subject, &e, ParaAttr::IndentEnd);
            });
        });

        self.list_section(ui, subject, &shown);

        // **Only under Justify**, because it only means anything there: a
        // left-aligned paragraph's last line is already at the start.
        if p.align == TextAlign::Justify {
            let row = section(ui, "Last line", |ui| {
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
        }

        let row = section(ui, "Text direction", |ui| {
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
    }

    /// Route one of the four spannable `Length` fields: a unit click commits now, a
    /// scrub goes through the valve.
    ///
    /// One helper rather than four copies of the same eight lines, and `into` is what
    /// lets it be one: the four differ only in which [`ParaAttr`] they build.
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
    /// **A method rather than the body of the segmented control**, because the
    /// context menu is a second door onto the same three states
    /// (`context-menus.md` §5.6) and §8's rule is that a row and the control it
    /// duplicates must be the same verb. What it carries that a call site would
    /// have to remember: the switch **adopts the shaped size**, so changing mode
    /// does not move the text, and it goes through `text_session_restyled` so a
    /// live editor re-shapes rather than drawing to the old width.
    ///
    /// Reads the **preview** layout, unlike most verbs: the size being adopted is
    /// the one on screen, and during a live text session that is the uncommitted
    /// one. Adopting the committed size instead would snap the box back to what it
    /// measured before the last keystroke.
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
    ///
    /// **Vertical alignment is not here** — it is inline in the main panel.
    fn type_block_tab(&mut self, ui: &mut egui::Ui, subject: &TypeSubject) {
        let b = subject.block;
        section(ui, "Sizing", |ui| {
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

        let trim_index = BoxTrim::ALL.iter().position(|t| *t == b.trim).unwrap_or(0);
        let trim_row = section(ui, "Box trim", |ui| {
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

        section(ui, "Overflow", |ui| {
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

        // **Directly under Overflow because neither means much alone.** An
        // ellipsis needs somewhere to stop, and a line limit with nothing to say
        // about the overflow just cuts the text off.
        section(ui, "Max lines", |ui| {
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

        self.wrap_section(ui, subject);
        self.optical_margin_section(ui, subject);
    }

    /// The three wrapping controls, stacked, each with its own label.
    ///
    /// **Three segmented tracks rather than three dropdowns.** Every option is
    /// one or two words and there are eight of them altogether; a dropdown hides
    /// seven of those behind a click each and says nothing about what the
    /// alternatives are, which for controls consulted this rarely is exactly the
    /// wrong trade. The tracks also make the gate below visible — a whole row of
    /// dimmed cells reads as "this does not apply", where a dimmed dropdown just
    /// reads as broken.
    ///
    /// **Word break and Long words dim under `NoWrap`.** That mode reaches parley
    /// as `TextWrapMode(NoWrap)` and suppresses line breaking outright, so both of
    /// them are rules about breaks that cannot happen. Dimmed and **not reset**,
    /// the same rule the stroke panel's Join follows: a stored `word_break`
    /// survives a trip through `NoWrap` and back. The gate stops there — hard
    /// breaks still break under `NoWrap`, so the paragraph spacing and indent
    /// controls in the tab beside this one go on working and must not be caught by
    /// the same condition.
    fn wrap_section(&mut self, ui: &mut egui::Ui, subject: &TypeSubject) {
        let p = subject.paragraph.clone();
        let wraps = p.wrap != WrapMode::NoWrap;
        section(ui, "Wrap", |ui| {
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
                // **Two sections in one scope, so the scope's own pitch is the
                // *section* gap** — the two are labelled blocks like any other and
                // only share a parent because they share one dimming rule. Written
                // flat at [`LABEL_GAP`] they had 5 between one section's control
                // and the next section's label, which is the gap that belongs
                // between a label and the thing it names (§15 D386).
                ui.spacing_mut().item_spacing.y = SECTION_GAP;
                section(ui, "Word break", |ui| {
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
                section(ui, "Long words", |ui| {
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
            });
            // A claimant of its own, for the reason `type_alignment_row` spells
            // out: a scope's response never hovers, and a disabled one could not
            // fire `on_hover_text` even if it did.
            if !wraps {
                ui.interact(
                    gated.response.rect,
                    ui.id().with("dimmed-wrap-section"),
                    egui::Sense::empty(),
                )
                .on_hover_text("Nothing to break: the lines do not wrap");
            }
        });
    }

    /// **Optical margins: place each line by its ink rather than by its advance**
    /// (§15 D830).
    ///
    /// **Not inside `wrap_section`'s gate, and that is the point.** Everything in
    /// that scope is dimmed under `NoWrap` because it states a rule about breaks
    /// that cannot happen; this one is most useful on exactly the node that never
    /// wraps — a single-line label, which is the case `roadmap.md` raised it for.
    /// A gate copied from the neighbour would have dimmed the control precisely
    /// where it earns its keep.
    ///
    /// **One switch and no preview text.** The effect is a fraction of the font
    /// size — measured 0.98% to 10.06% of it on the default face — so a sample
    /// string in a 264px popup would show nothing a reader could trust. The
    /// canvas is where it is visible, and the tooltip says what to look at.
    fn optical_margin_section(&mut self, ui: &mut egui::Ui, subject: &TypeSubject) {
        let p = subject.paragraph.clone();
        section(ui, "Margins", |ui| {
            let resp = ui::switch_row(ui, "Optical margins", p.optical_margins, SEG_H);
            if resp.clicked() {
                self.commit_paragraph(
                    subject.id,
                    ParagraphStyle {
                        optical_margins: !p.optical_margins,
                        ..p.clone()
                    },
                );
            }
            resp.on_hover_text(
                "Hang the first and last glyph of every line out to the edges, so \
                 the text lines up by its ink instead of by its spacing",
            );
        });
    }

    // --- writing ----------------------------------------------------------

    /// One `Length` character attribute as a field with a `%`/`px` suffix.
    #[allow(clippy::too_many_arguments)]
    fn length_char_field(
        &mut self,
        ui: &mut egui::Ui,
        subject: &TypeSubject,
        size: egui::Vec2,
        kind: CharAttrKind,
        prefix: Prefix,
        bounds: Bounds,
        tooltip: &str,
    ) {
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
            // Its own parser, for [`length_field`]'s reason: two units on one
            // field, so the one it is not showing is refused (§15 D359).
            |d| {
                let d = d
                    .custom_formatter(ui::number(2))
                    .custom_parser(move |t| expr::eval_in(t, typed));
                mixed_text(d, mixed)
            },
        );
        // 🚨 **The field had no hover text at all until §15 D765**, on the same
        // argument `length_field` makes for having one: *"the field is labelled by
        // a 14pt glyph and nothing else, so the tooltip is the whole of what says
        // which number this is"*. Three fields here wear that argument and none of
        // them had the tooltip — and two of the three, word spacing and baseline
        // shift, sit **side by side in one row** behind two icons a reader has to
        // already know. The chip had its own text the whole time, which is the
        // shape `[S6.2-L3-09]`'s third divergence was about from the other side.
        let resp = resp.on_hover_text(tooltip);
        if unit_clicked {
            let next = current.in_unit(current.unit().other(), font_size);
            self.apply_char_attrs(subject, vec![length_attr(kind, next)]);
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
    /// The same fork [`Self::paragraph_length`] makes for the paragraph fields, and
    /// it exists here for the same two reasons: a scrub must preview and commit once
    /// rather than commit per frame, and a unit click has no drag for a valve to
    /// wait on.
    fn decoration_write(&mut self, e: &OptionalLengthEdit, subject: &TypeSubject, attr: CharAttr) {
        if e.immediate {
            self.apply_char_attrs(subject, vec![attr]);
        } else {
            self.char_valve(&e.resp, subject, attr);
        }
    }

    /// Route a character-attribute *field* through the preview→commit valve, or —
    /// while a live editing session owns the range — straight into that session.
    ///
    /// The session is its own preview (it re-shapes and the canvas draws from the
    /// editor), so a valve there would be a second preview of the same edit; and
    /// the commit is owed at the end of the session rather than at the end of the
    /// drag, because one editing session is one undo step (§9.3).
    ///
    /// ⚠️ **The third valve D316 never reached, found by writing a doc comment
    /// about the second** (§15 D523). This committed on `drag_stopped() ||
    /// lost_focus() || changed()`, so typing `40` into the Type panel's size
    /// field left **two** entries in the history — and, worse than the multi
    /// card's version, *wrote the intermediate values to the document*: measured
    /// frame by frame, the keystrokes committed `4` and then `40`, so a size
    /// typed as `100` puts a one-point line through the artwork on its way. The
    /// engagement latch is D316's, spelled here for the same reason it is
    /// spelled in `multi_valve`.
    ///
    /// 🚨 **There were three arms and there are two** (§15 D812, ruled by the
    /// maintainer; the body carries the full account at the site of the
    /// deletion). This paragraph described the third in the present tense for
    /// as long as it did not exist — *"a `changed()` frame on a control that was
    /// never engaged commits at once"*, *"whether anything reaches this arm is
    /// open"*, *"deleting the arm breaks no test today"* — and all three
    /// sentences were false at `HEAD`: there is no third arm, the question was
    /// ruled on, and deleting or restoring it now breaks two tests (§15 D840).
    ///
    /// ⚠️ **The commit that removed the arm rewrote the comment *inside* the
    /// function and not the doc above it**, which is the shape worth naming:
    /// the body and the doc of one function drifted apart in one edit, and the
    /// doc is the half a reader meets first. A reader repairing the code to
    /// match this would have restored the arm.
    fn char_valve(&mut self, resp: &egui::Response, subject: &TypeSubject, attr: CharAttr) {
        if subject.partial {
            self.arm_session_scrub(resp, subject.id);
            if resp.changed() {
                self.apply_char_attrs(subject, vec![attr]);
                // **Armed here, because nothing downstream will.** A partial subject
                // previews through the live text session and commits nothing until
                // the session ends (§9.3), so there is no `commit_edit` on this path
                // — and the selection outline would sit over the very text being
                // recoloured (§15 D128).
                self.note_live_edit(resp.dragged());
            }
            return;
        }
        // **Built unconditionally, and the valve is spelled out rather than
        // borrowed.** `edit_valve` would do, but only given a transaction — and the
        // guard that used to sit here withheld one on exactly the frame that
        // matters. `char_attrs_tx` compares its result against `subject`, and
        // `subject` is read through the *preview*: during a drag that is the value
        // on screen, so on the release frame the two are equal, the tx came back
        // `None`, and the commit never happened. The edit lived in the preview and
        // nowhere else, until the next commit anywhere cleared it and the control
        // snapped back to the last committed value. Reported as "I can slide a
        // slider and it will reset immediately to the value it had" (§15 D109).
        // **Read before the `gesture_cancelled` bail**, which is §15 D317's
        // ordering: a cancelled frame has to spend the transition, or the frame
        // after it finds `was_engaged` without `engaged`, the `Escape` long gone
        // from `input`, and commits the edit the cancel threw away.
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
        // The edit this control has in flight, remembered across the frames
        // between the last keystroke and the frame the valve commits on.
        // `changes` is `char_attrs_tx`'s own answer to *"is this different from
        // the subject"*, so it is what decides whether there is an edit worth
        // holding.
        //
        // ⚠️ **This is insurance rather than the load-bearing line here, and the
        // flip says so** (§15 D523). In `multi_valve` the equivalent memory is
        // what makes the falling-edge commit work at all: a `DragValue` there
        // writes its parsed edit string back one frame *after* it surrenders
        // focus, so the committing frame carries no number and dropping the
        // memory leaves the typed value on the floor — measured, red. Flipped
        // the same way here, **nothing goes red**: the size field's widget has
        // written 40 back by the frame the commit lands on, so the held value is
        // never the one used. Why the two differ has not been established, which
        // is the argument for keeping this rather than against it — the valve
        // must not depend on which frame egui happens to choose, and this valve
        // has *seven* call sites of which the tests drive one.

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
        // Every frame, as `edit_valve` does it: a drag's hold has no deadline until
        // the button comes up, and noting only on the committing frame would leave
        // the box over the artwork for the whole scrub (§15 D128).
        self.note_edit(Some(resp), &tx);
        if engaged {
            self.session.set_preview(&tx);
        } else if was_engaged {
            // `Escape` abandons a typed value; every other way out of the field
            // confirms it. egui hands back what was typed and merely surrenders
            // focus, so without this the key that cancels everywhere else would
            // apply the edit.
            if !resp.ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
                self.commit_edit(tx.clone());
                // As in `apply_char_attrs`: the transaction carries the cleared list, so
                // the editor adopts it instead of putting its own back (§15 D164).
                self.text_session_restyled_after(&tx);
            }
            self.session.clear_gesture_preview();
            resp.ctx
                .data_mut(|d| d.insert_temp::<Option<CharAttr>>(pending_key, None));
        }
        // 🚨 **There was a third arm and it is gone** (§15 D812, ruled by the
        // maintainer). `else if resp.changed() && !resp.lost_focus()` committed
        // outright, on the reasoning that this valve — unlike the app's other two —
        // serves controls with **no engagement to latch**, and that without it a
        // click on the picker's hue strip would commit nothing, ever.
        //
        // **Both halves turned out to be false, eleven days after the arm was
        // kept and within hours of each other.**
        // §15 D802 wrote the test D523 asked for: `picker::pointer_slot` answers a
        // click through `write_slot` before it could reach `valve_slot`, so the raw
        // sensed regions never enter this valve at all — a drag enters it as the
        // engaged arm and its release as the falling edge, which are the two arms
        // every other valve has, and `changed()` is never true on those frames.
        // §15 D803 then found the one thing that *did* reach it, and it was a
        // hazard: egui's per-frame clamp of a stored value outside a **ranged**
        // field marks a control holding no focus as `changed()`, so the arm fired
        // on a field nobody had touched, committed it and spent an undo step. That
        // is `[S6.2-L1-01]`.
        //
        // ⚠️ **What decided it was the asymmetry, not the count.** All seven call
        // sites are engagement-bearing — `ui::value_field`, `ui::bare_drag_value`,
        // the axis field and slider, the length fields — and **both** of this app's
        // `DragValue` helpers now opt out of egui's clamp (§15 D425, D552), so the
        // arm had no live user of any kind. If some unmeasured caller did need it,
        // its symptom is *"this control commits nothing"*, which is loud and gets
        // reported; the symptom of keeping it is *"a field nobody touched spent an
        // undo step"*, which is silent. D425's one line is no longer the only thing
        // standing between that bug and the document.
    }

    /// Remember what a **session-scoped** scrub is about to overwrite, on the frame
    /// its drag begins.
    ///
    /// The write goes into the editor rather than into a preview, so this snapshot is
    /// the only thing `cancel_gesture` can wind a right-click back to — see
    /// [`InFlight::session_scrub`]. Taken on `drag_started` alone: taking it every
    /// frame would snapshot the value the drag had already reached.
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
    /// **Every one of these goes through the same `apply_char_attrs` /
    /// `commit_paragraph` the panel's own controls use**, so a chord and a click
    /// produce byte-identical edits — including the scope rule, where a chord
    /// with a selection styles the selection and one on a bare caret sets the
    /// node's defaults. Duplicating that logic here is how the two would come to
    /// disagree about a mixed run.
    ///
    /// Silent with no session open: [`Action::TextStyle`] is only resolved in
    /// `Mode::TextInsert`, so this is belt and braces rather than a live path.
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
            // **Exclusive with strikethrough**, which is the whole reason this is
            // not a one-line `Underline(Some(_))`. The 2026-07-31 redesign offers
            // one decoration rather than two independent ones, so turning
            // underline on has to turn strikethrough off — and the *colour and
            // thickness carry over*, exactly as clicking the segmented track
            // does, because switching which line is drawn is not a reset of how
            // it is drawn.
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
                // `CharAttr::Size` clamps itself to `MIN_FONT_SIZE..=MAX_FONT_SIZE`,
                // so held keys stop at the ends without a second bound here.
                let next = subject.font_size() + f64::from(step) * FONT_SIZE_STEP;
                self.apply_char_attrs(&subject, vec![CharAttr::Size(next)]);
            }
            TextChord::Tracking(step) => {
                let current = match subject.shown(CharAttrKind::LetterSpacing) {
                    CharAttr::LetterSpacing(l) => l,
                    _ => Length::ZERO,
                };
                // **A held key stops where the field stops** (§15 D817, ruled by
                // the maintainer). `TextStyle::set` canonicalizes `letter_spacing`
                // without bounding it, so before this a held `Alt`+`→` walked
                // tracking past `MAX_TRACKING_PCT` and out of what the field can be
                // scrubbed to — where the `Size` arm four lines up stops at its
                // ends and says so.
                //
                // ⚠️ **Here and not at `TextStyle::set`, which is where `Size`'s
                // bound lives**, and the difference is deliberate. These caps are
                // on the *controls*: any finite value is a legal attribute, a file
                // may hold one, and the field **shows** an out-of-range stored
                // value instead of rewriting it — which is **§15 D425**'s
                // decision, not D475's. (D475 is this panel's *test* of it, and
                // says in its own body that D425 carries the argument and it
                // deliberately does not restate it. Citing D475 for the behaviour
                // is the substitution `CLAUDE.md` records happening at seven
                // sites, and this comment made it an eighth.) A bound in
                // the model would make that unreachable for tracking and would be
                // this panel refusing a document it is able to display. So the two
                // chords agree about *behaviour* — held keys stop at the field's
                // ends — and deliberately not about where the bound is written.
                //
                // The px face is derived from the `%` face at the current font
                // size, through the same [`px_range_for`] the field itself uses, so
                // the chord and the scrub cannot come to disagree about the cap in
                // the unit the user happens to be in.
                // ⚠️ **[`stepped_into`] and not `clamp`**, which is the whole of
                // §15 D840: a bare clamp here honoured D817 and broke D425, by
                // rewriting a stored out-of-range value the paragraph above says
                // this panel deliberately shows rather than rewrites.
                let next = match (
                    current,
                    step_length(current, step, TRACKING_STEP_EM, TRACKING_STEP_PX),
                ) {
                    (Length::Em(c), Length::Em(v)) => Length::Em(stepped_into(
                        c,
                        v,
                        MIN_TRACKING_PCT / 100.0..=MAX_TRACKING_PCT / 100.0,
                    )),
                    (Length::Px(c), Length::Px(v)) => {
                        let r =
                            px_range_for(MIN_TRACKING_PCT..=MAX_TRACKING_PCT, subject.font_size());
                        Length::Px(stepped_into(c, v, r))
                    }
                    // `step_length` returns the unit it was handed, so a mixed
                    // pair cannot arise; taking the step unbounded is the honest
                    // answer if it ever does, since the bound would be in the
                    // wrong unit.
                    (_, stepped) => stepped,
                }
                .canonical();
                self.apply_char_attrs(&subject, vec![CharAttr::LetterSpacing(next)]);
            }
            TextChord::Leading(step) => {
                // **Auto seeds from what Auto is currently worth**, in px, rather
                // than from a round number — the same rule the field follows
                // (§15 D162). Stepping from a nominal 0 would collapse the lines
                // on the first press and read as the key being broken.
                let current = match subject.shown(CharAttrKind::LineHeight) {
                    CharAttr::LineHeight(v) => v,
                    _ => None,
                };
                let from = current
                    .or_else(|| subject.line_height.map(Length::Px))
                    .unwrap_or(Length::Em(1.0));
                // 🚨 **The identical defect §15 D817 fixed three arms up, left
                // standing here** (§15 D840). This arm ended in `step_length`
                // with no range expression at all, and `Length::canonical`
                // rounds and normalises `-0.0` without bounding — so a held
                // `Alt`+`↓` walked line height **negative**, straight out of the
                // field's `0..=1000%` and into `parley_line_height`. D817's own
                // entry is about the tracking arm and this one was never read
                // beside it; the fix that closes both is one predicate, which is
                // the argument for [`stepped_into`] existing rather than each
                // arm carrying its own bound.
                let next = match (
                    from,
                    step_length(from, step, LEADING_STEP_EM, LEADING_STEP_PX),
                ) {
                    (Length::Em(c), Length::Em(v)) => Length::Em(stepped_into(
                        c,
                        v,
                        MIN_LINE_HEIGHT_PCT / 100.0..=MAX_LINE_HEIGHT_PCT / 100.0,
                    )),
                    (Length::Px(c), Length::Px(v)) => {
                        let r = px_range_for(
                            MIN_LINE_HEIGHT_PCT..=MAX_LINE_HEIGHT_PCT,
                            subject.font_size(),
                        );
                        Length::Px(stepped_into(c, v, r))
                    }
                    (_, stepped) => stepped,
                }
                .canonical();
                self.apply_char_attrs(&subject, vec![CharAttr::LineHeight(Some(next))]);
            }
            // Node-level, so this is a paragraph-style commit rather than a span
            // — see [`TextChord::Align`]. Nothing to do when it is already set:
            // an alignment chord pressed twice must not leave a second undo step
            // saying nothing, which is the rule the segmented track states as
            // `!= align`.
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

    /// Apply character attributes now — for the controls that are a click rather
    /// than a drag, and for the unit toggles, which change the value without any
    /// drag for a valve to wait on.
    fn apply_char_attrs(&mut self, subject: &TypeSubject, attrs: Vec<CharAttr>) {
        if subject.partial {
            if let Some(s) = self.text.as_mut().filter(|s| s.id == subject.id) {
                for attr in attrs {
                    s.editor.style_selection(attr);
                }
            }
            self.preview_session();
            return;
        }
        // Here the equality *is* the right question: this path is for clicks, there
        // is no preview in play, and a click that changes nothing must not leave an
        // undo step behind.
        let (tx, changes) = char_attrs_tx(subject, attrs);
        if changes {
            self.commit_edit(tx.clone());
            // `_after`, not the plain re-style: this transaction *clears* the
            // attribute's spans over the whole node, and a session sitting on a bare
            // caret reaches here — so the editor has to adopt the cleared list rather
            // than win with its own (§15 D164).
            self.text_session_restyled_after(&tx);
        }
    }

    /// The colour the detached picker should show for a decoration: its own if it
    /// has one, otherwise the text's, which is what the decoration is drawn in.
    ///
    /// `None` when there is no decoration to colour — which is also how the
    /// picker learns to close itself, since `slot_brush` returning `None` is
    /// already the signal that a slot has stopped existing.
    pub(crate) fn char_slot_color(&self, id: NodeId, slot: CharSlot) -> Option<Color> {
        let subject = TypeSubject::of(self, id)?;
        match slot {
            // Its own colour if it has one, otherwise the text's — which is what the
            // run is actually drawn in, so the picker opens on the colour already on
            // screen rather than on an arbitrary black.
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
    /// **The paint slots whose write is not a document transaction.** Every other
    /// slot ends in `slot_transaction`; a character attribute cannot, because while a
    /// live editing session owns the range the write belongs in that session's spans
    /// (§9.3). The three cases are already decided in one place each —
    /// `apply_char_attrs` for a click, `char_valve` for a drag — so this routes there
    /// rather than teaching the paint plumbing about text.
    ///
    /// **One route for both character-scoped slots**, which is what made the third
    /// verb possible. With only a decoration to divert, each of `write_slot`,
    /// `valve_slot` and `preview_slot` carried its own branch, and the third of them
    /// simply declined — the press-preview gap §15 D129 left open. A run colour is
    /// the second caller that branch was waiting for; hoisting it turned three
    /// branches into one predicate ([`crate::panels::paint::PaintSlot::char_scoped`])
    /// and one function, and
    /// the gap closed as a side effect rather than as a fourth special case.
    ///
    /// Returns whether it had anything to write, which is `preview_slot`'s answer.
    ///
    /// A gradient collapses to its first stop. The picker shows its gradient tabs
    /// *disabled* for these slots
    /// ([`crate::panels::paint::PaintSlot::takes_gradients`]) so one cannot be
    /// built here, and this is the belt to that brace rather than a second policy.
    pub(crate) fn write_char_slot(
        &mut self,
        id: NodeId,
        slot: CharSlot,
        brush: &Brush,
        how: CharWrite<'_>,
    ) -> bool {
        // **A commit reads the *committed* document; the other two verbs read the
        // preview.** `TypeSubject::of` goes through `display_node`, which applies the
        // render override — and the press frame of a click on the picker's plane has
        // just put this very colour there (`CharWrite::Preview`). Comparing against it
        // makes `apply_char_attrs` see no change and commit nothing, so the colour
        // lives in the preview until something clears it: §15 D109's shape arriving by
        // the route D129's own fix opened.
        //
        // **Reading committed truth, not clearing the preview** — which is what this
        // tried first and is a worse bug than the one it fixed. `clear_preview` also
        // drops `session_preview`, and a preview may belong to a **live text
        // session** rather than to the gesture in hand, so clearing it took the
        // uncommitted text off the canvas on every press: reported as the wheel and
        // the strip going completely dead. The two verbs that *should* see the preview
        // still do.
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
        // **A live selection collapses the three verbs into one.** Nothing a session
        // styles reaches history until the session ends (§9.3), so "show this colour"
        // and "commit it" are the same write — into the editor's own spans — and
        // there is no undo step for a repeat to leave behind.
        //
        // Spelling that out here is what fixes a picker that did **nothing at all**
        // over a text selection. Two gates conspired: `Preview` declined a partial
        // subject, so the press showed nothing, and `char_valve`'s partial branch
        // gates on `resp.changed()` — which a `DragValue` reports and a raw sensed
        // region like the picker's wheel or its opacity strip never does. Press, drag
        // and release therefore all fell through. Reported as the picker's UI being
        // frozen with only the close button alive, which is exactly how it looked.
        //
        // `char_valve` is deliberately left alone: its `changed()` gate is right for
        // the popup's *own* fields, which do report it, and it is D109's code.
        if subject.partial {
            // **The gate the valve was providing, and dropping it was a bug.**
            // `valve_slot` is called *unconditionally, every frame* by several of the
            // picker's controls — the hue slider, the alpha strip, the hex row — on
            // the understanding that the valve itself decides whether anything is
            // happening. Writing on every call therefore had each of them stamp its
            // own frame-start value into the editor every frame, overwriting whatever
            // the control actually under the pointer had just written. The colour
            // landed and was undone in the same frame, which is why the picker looked
            // frozen rather than wrong.
            //
            // `Commit` and `Preview` need no gate: their callers reach them only on a
            // click and on a press respectively. `Valve` takes the same test
            // `char_valve` applies, and it has to include the pointer terms — a raw
            // sensed region reports neither `changed` nor `lost_focus`.
            //
            // valve-gate-not-a-commit: `inspector::interacting` again, on the
            // partial-subject path where the write goes into the live editing
            // session rather than through a valve at all.
            if let CharWrite::Valve(resp) = how {
                if !(resp.dragged() || resp.drag_stopped() || resp.changed() || resp.lost_focus()) {
                    return false;
                }
                // The selection chrome owes the same retreat every other live edit
                // gets (§15 D128); `dragged()` holds it while the button is down.
                self.note_live_edit(resp.dragged());
            }
            self.apply_char_attrs(&subject, vec![attr]);
            return true;
        }
        match how {
            CharWrite::Commit => self.apply_char_attrs(&subject, vec![attr]),
            CharWrite::Valve(resp) => self.char_valve(resp, &subject, attr),
            // **Preview only, and deliberately not through `char_valve`.** The valve
            // clears its own preview by committing on release; a press that never
            // becomes a drag has no such frame, so routing this through the valve
            // would leave a stale preview on every press-and-release of the popup's
            // *other* valved controls — which is why D129's gap was left open rather
            // than papered over.
            //
            // A live selection never reaches here — it is answered above, where the
            // three verbs are one write.
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

    /// Apply one spannable paragraph attribute now — for the controls that are a
    /// click rather than a drag, and for the unit chips, which change the value
    /// with no drag for a valve to wait on.
    ///
    /// [`apply_char_attrs`]'s twin, and the same two targets: a live session that
    /// owns *some* of the paragraphs gets it in its own `ParaSpans`, which previews
    /// itself and commits once when the session ends (§9.3); anything else sets the
    /// node's defaults and drops the overrides of that attribute.
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
        // As in `apply_char_attrs`: this path is for clicks with no preview in play,
        // so a click that changes nothing must not leave an undo step behind.
        let (tx, changes) = para_attrs_tx(subject, attrs);
        if changes {
            self.commit_edit(tx);
            self.text_session_restyled();
        }
    }

    /// A spannable paragraph write through the drag valve: previewed while the
    /// control is under the hand, committed once when it is released.
    ///
    /// **The transaction is built unconditionally** — D109's lesson, and it applies
    /// here for the same reason it does in `char_valve`: `para_attrs_tx` compares its
    /// result against `subject`, `subject` is read through the *preview*, so on the
    /// release frame of a drag the two are equal. A guard on "did anything change"
    /// therefore withholds the commit on exactly the frame that owes one, and the
    /// edit lives in the preview until something else clears it.
    fn valve_paragraph(&mut self, resp: &egui::Response, subject: &TypeSubject, attr: ParaAttr) {
        if subject.para_partial {
            self.arm_session_scrub(resp, subject.id);
            if resp.changed() {
                self.apply_para_attrs(subject, vec![attr]);
                // The chrome hide, armed here because nothing downstream will: a
                // partial write previews through the live session and commits
                // nothing until it ends, so there is no `commit_edit` on this path
                // (§15 D128).
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

    /// [`Self::commit_block`] for **box trim**, the one block edit whose only visible
    /// result is the selection box itself.
    ///
    /// §15 D78 guarantees that trimming never moves the ink, so what it moves is the
    /// box the selection outline draws — and the chrome hide would take away the only
    /// thing there was to see (§15 D128). The exception reads here, at the edit it is
    /// about, rather than as a flag threaded through the shared committer.
    fn commit_trim(&mut self, id: NodeId, block: BlockStyle) {
        if self.commit_chrome_edit(Transaction(vec![Operation::SetBlockStyle { id, block }])) {
            self.text_session_restyled();
        }
    }
}

// --- free helpers ----------------------------------------------------------

/// One labelled block of the popup: a small-caps eyebrow, then its controls.
///
/// **Three gaps, three numbers.** The popup's own `item_spacing.y` separates one
/// section from the next ([`SECTION_GAP`]); [`ui::labelled`] then holds the label
/// to its control at [`LABEL_GAP`] and two controls of one section to each other
/// at [`ui::CARD_ROW_GAP`] — the Transform card's own row pitch, which is the
/// baseline for every panel and popover (§15 D386).
///
/// It was one number for the last two of those, and that is what put two stacked
/// fields 5 apart in a popover hanging off a card that stacks its own at 9. One
/// number for all three is worse still and was the state before that: the label
/// floats equidistant between its own control and the section above, which is the
/// reading that makes a nine-section card look like eighteen loose rows.
fn section<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui::labelled(ui, label, add)
}

/// The square button at the end of a row that puts a value back to the font's
/// own. Returns whether it was pressed.
///
/// **Always drawn, dimmed when there is nothing to put back.** The handoff asked
/// for it "shown only when the axis is off default", and the first pass did that —
/// allocating the square either way so the field beside it would not change width.
/// Which left a 28pt hole at the end of the row, reported as the colour field not
/// filling its container, and it was: an empty slot at the end of a row reads as
/// the row stopping short, not as an absent button.
///
/// So the third state does the work instead, as it does on a [`ui::FieldButton`]
/// everywhere else in the chrome: the ground and the glyph say "nothing to undo"
/// while the geometry never moves. `Disabled` also refuses the hover, so it does
/// not invite the click it would ignore.
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
/// **Both halves, always.** Setting the default alone is how "make it all 24pt"
/// leaves a 16pt run behind — the span was there, it was not the default, and
/// nothing asked it to go.
///
/// A free function rather than a method, because it touches nothing but its
/// arguments and because the panel's most easily-broken write is the one that
/// clears a value rather than sets one: it is the case where "the new style equals
/// the old" is *nearly* true, and a guard that catches a no-op edit is one line
/// away from swallowing a real one.
///
/// **The transaction always comes back; the equality is reported beside it.** It
/// used to be an `Option`, and every caller read the `None` as "nothing to do" —
/// which is only true for a *click*. `subject` is read through the render
/// override, so during a drag it holds the previewed value; on the release frame
/// the new style therefore equals it, and a valve given `None` never commits. The
/// two callers want different answers to the same comparison, so it is theirs to
/// make (§15 D109).
fn char_attrs_tx(subject: &TypeSubject, attrs: Vec<CharAttr>) -> (Transaction, bool) {
    let mut style = subject.style.clone();
    let mut spans = subject.spans.clone();
    for attr in attrs {
        let kind = attr.kind();
        style.set(attr);
        spans.clear(subject.range.clone(), kind, &style);
    }
    let changes = style != subject.style || spans != subject.spans;
    (
        Transaction(vec![
            Operation::SetTextStyle {
                id: subject.id,
                style,
                // The list below is what the document's own re-statement would
                // produce and then some — it also clears the range — so this asks
                // for the re-statement and lets the next op overwrite it (§15 D163).
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
/// [`char_attrs_tx`]'s twin, down to the order of the two operations — the style op
/// re-states the span list against the new defaults (`Document::op_set_paragraph_style`)
/// and the spans op then installs the list computed here, which was normalized
/// against those same new defaults. Also returns whether anything changed, which
/// only the click path may act on.
fn para_attrs_tx(subject: &TypeSubject, attrs: Vec<ParaAttr>) -> (Transaction, bool) {
    let mut paragraph = subject.paragraph.clone();
    let mut spans = subject.para_spans.clone();
    for attr in attrs {
        let kind = attr.kind();
        paragraph.set(attr);
        spans.clear(subject.para_range.clone(), kind, &paragraph);
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

/// The attribute that puts `color` in `slot` — `None` meaning "inherit".
///
/// **One builder for every write to a character-scoped slot**, so the row, the
/// picker and the three slot verbs cannot disagree about what setting a colour
/// means. `None` for a decoration slot with no decoration on it: there is nothing to
/// colour, which is also how the picker learns to close itself when one is switched
/// off underneath it.
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

/// The column a list inside the popup gives up to its scrollbar.
///
/// Taken out of the rows rather than laid over them: a bar drawn on top of a
/// slider that reaches the same edge is a bar you cannot grab without moving the
/// axis you were trying to scroll past.
const LIST_GUTTER: f32 = 10.0;

/// Which of the Decoration strip's three cells is lit, and what picking cell `i`
/// writes — the pair, because they are one rule read in two directions.
///
/// 🚨 **Lifted out of `decoration_section` so a test can reach it** (§15 D624,
/// `[S6.2-L6-07]`, §15 D269's move). The review swapped the `(under, strike)` →
/// cell mapping so the strip lights the wrong decoration, and the **whole
/// workspace stayed green** — including `skip_ink_tests`, which is one of the few
/// modules in this file that touches `decoration_section` at all and which reads
/// `under.is_some()` rather than the cell, so *which* cell lights was unpinned
/// even there.
///
/// **`carried` is the decoration's own styling, moved rather than re-made.**
/// Switching from underline to strikethrough keeps the colour, thickness and
/// style the user chose — the `unwrap_or_default()` only fires when there was
/// nothing to carry.
///
/// The `None` arm is cell 0, *no decoration*, and it is the arm a wrong mapping
/// reaches by accident: `_ =>` in the write half means anything not 1 or 2 clears
/// both, so a read half that answered 1 where it means 2 still writes *something*
/// and looks like it works.
fn decoration_cell(under: Option<Decoration>, strike: Option<Decoration>) -> usize {
    match (under, strike) {
        (Some(_), _) => 1,
        (None, Some(_)) => 2,
        (None, None) => 0,
    }
}

/// What picking cell `i` writes, given whatever styling was already there.
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

/// Whether the **Bold** toggle is lit, and the weight a click on it writes.
///
/// 🚨 **Lifted for `[S6.2-L6-07]`'s second flip** (§15 D624): `weight >= 600`
/// became `>= 100` — every face reading bold — with the whole workspace green.
///
/// **600 and not 700**, because the threshold is a *reading* of an arbitrary
/// weight and the click is a *choice* between two: a 600 Semibold face should
/// show the button lit, and turning the button on from anywhere below writes 700.
/// ⚠️ **A mixed selection is never lit**, whatever its weights: the button is a
/// claim about the whole subject, and lighting it would make the next click mean
/// *un*-bold for runs that were never bold.
fn bold_toggle(weight: u16, mixed: bool) -> (bool, u16) {
    let on = weight >= 600 && !mixed;
    (on, if on { 400 } else { 700 })
}

/// Whether a variant describes the current weight, slope and axis settings.
///
/// ⚠️ **This line was on `decoration_cell` for the length of one commit**, which
/// is `CLAUDE.md`'s *"anchor above"* trap, instance **sixteen** — and committed by
/// a session that had the rule in front of it and had already run the neighbour
/// grep four times. The anchor was `fn matches_variant(…)`, the **item** line, so
/// the block went in between this function and its own doc. The merged run was 30
/// lines against the length ranking's 52-line floor, so that detector could not
/// see it; what found it was running the neighbour check over *every* insertion
/// site at the close of the session rather than over the ones that felt risky.
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

/// Replace a `DragValue`'s digits with a dash when the range disagrees with
/// itself.
///
/// **A dash, not a blank and not the first value.** Blank reads as a broken
/// field, and showing one of the several values reads as a claim that it is *the*
/// value — which the next drag would then make true for all of them.
fn mixed_text(d: egui::DragValue<'_>, mixed: bool) -> egui::DragValue<'_> {
    if mixed {
        return d.custom_formatter(|_, _| "–".into());
    }
    d
}

/// What picking list marker `next` writes — the marker, and a gutter for it if the
/// paragraph has none (§15 D804).
///
/// **Turning a marker on opens a gutter once.** A marker is right-aligned on the
/// paragraph's start edge, so with no indent it draws outside the box — an honest
/// reading of "no gutter" and a poor first impression. ⚠️ **Only where the indent
/// resolves to zero**, so a paragraph that already has one keeps it, and turning a
/// marker off never touches the indent at all: a gutter the user can see is theirs
/// once it exists.
///
/// ⚠️ **The zero test is on the *resolved* value, which is why `font_size` is a
/// parameter** and not a detail — an `Em(0.0)` indent is zero at every size, but so
/// is `Px(0.0)`, and the field can hold either (§15 D537).
fn marker_attrs(next: Option<ListMarker>, indent_start: Length, font_size: f64) -> Vec<ParaAttr> {
    let mut attrs = vec![ParaAttr::Marker(next)];
    if next.is_some() && indent_start.resolve(font_size) == 0.0 {
        attrs.push(ParaAttr::IndentStart(Length::Px(DEFAULT_LIST_GUTTER)));
    }
    attrs
}

/// The variation coordinates after setting `axis` to `value` — the whole of what
/// `clear_at_default` decides (§15 D804).
///
/// `clear_at_default` **drops** the coordinate rather than writing the font's own
/// value into it, which is what "reset" means and what keeps the file from
/// accumulating settings that say nothing. It is **off for `opsz`**, where the
/// absence of a coordinate is the Auto state and dropping one would silently
/// change the mode — so for that axis a default value is written out like any
/// other.
///
/// 🚨 **This was the same eight lines in two places** — `OndinApp::write_axis`
/// and `OndinApp::axis_valve` — and only the valve's copy was reached by a test
/// (§15 D569). `roadmap.md` warned about exactly that shape: *"a grep for the
/// name reads as though the decision were reached"*. It also listed this as a
/// decision needing *"a live text session and a laid-out panel"*, which was true
/// of its two callers and never of the rule: every operand here is a parameter,
/// so the lift §15 D269 prescribes reaches it after all.
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

/// The one value to show for a set of attribute values that are **all zero
/// lengths**, whatever units they are in — or `None` when they are not (§15 D799).
///
/// **This is the comparison on the *resolved* value that `Length::is_default_zero`
/// says belongs at the reader.** `Length::resolve` turns a length into a number
/// given a font size, and a zero is zero at every font size, so for this case
/// "resolved" needs no size and cannot be wrong.
///
/// 🚨 **And that is exactly why it stops at zero.** The general resolved
/// comparison — two lengths that resolve alike *at the current size* — was
/// considered and not taken: `Em(0.02)` and `Px(0.32)` are the same ink at 16pt
/// and different ink at 24, so a field would flip between a number and a dash as
/// the user resized the text, and the unit chip would have to name one of two
/// units the document actually holds. Zero is the only amount where the unit
/// carries no information about the ink, which is the whole reason it was the
/// only amount that read wrong. **Widening this is a decision, not an
/// oversight.**
///
/// ⚠️ **It hands back the first value that cannot be the default.** The unit chip
/// reads its suffix off whatever comes back, so returning `Px(0.0)` for a
/// document the user had set to `%` flips the chip back to `px` under them —
/// D537's own symptom arriving by a new road.
///
/// 🚨 **This paragraph used to say *"the first value, not the default"*, and
/// that was a coincidence rather than a guarantee** (§15 D840). `values_in`
/// answers in document order, and for every selection whose explicit run is not
/// at the head the first value **is** the default — so the sentence described
/// the one case it was not protecting. It is a rule now, and the body says how.
///
/// ⚠️ **This doc was stolen and put back on 2026-09-19** (§15 D804). Inserting
/// `marker_attrs` above it, anchored on this `fn` line rather than on the line
/// above the block, moved all of the above onto that function and left this one
/// bare — `CLAUDE.md`'s insertion-anchor trap, done by a session that had spent
/// the day on this class, and green through every gate. ⚠️ **The running count of
/// this class lives in `CLAUDE.md` and deliberately not in a comment** (§15 D697);
/// what caught this one was the neighbour grep run once more at the end, which is
/// the argument for running it as a routine rather than on suspicion.
fn agreed_zero<A>(values: Vec<A>, length_of: impl Fn(&A) -> Option<Length>) -> Option<A> {
    if !values
        .iter()
        .all(|v| length_of(v).is_some_and(|l| l.is_zero()))
    {
        return None;
    }
    // 🚨 **The first value that *cannot* be the default, and only then the
    // first** (§15 D840). `Spans::values_in` answers in **document** order, so
    // "the first value" is the one at the start of the selection — and for any
    // selection whose explicit run is not at the head, that is the node default.
    // A layer with `%` typed over characters 4..8 and the default over 0..4 read
    // back `px` for the whole word, and the same document with the run at the
    // head read `%`: the unit chip flipped under the user on the strength of
    // where they happened to start selecting.
    //
    // **`Length::ZERO` is `Px(0.0)` and is what every one of these six
    // attributes defaults to**, so a `Px(0.0)` in this set *may* be nobody's
    // choice while an `Em(0.0)` is always somebody's. That is what makes this a
    // rule rather than a preference between two units. Where both are explicit —
    // px typed on one run, `%` on another, both zero — the two are equally
    // defensible and this is a tie-break; the alternative the review raised, the
    // value at the caret, is the better answer to *that* case and needs a caret
    // this function is not given.
    let pick = values
        .iter()
        .position(|v| length_of(v) != Some(Length::ZERO))
        .unwrap_or(0);
    values.into_iter().nth(pick)
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
/// **The response comes back too, because these fields owe a valve.** They used to
/// return a bare `Option<Length>` and their callers committed it on the spot, which
/// made them the only numeric controls in the panel that did not preview — one
/// history entry per frame of a drag, and a commit built from a snapshot taken
/// before the frame started. That was the reported bug: scrubbing either paragraph
/// field did nothing at all (§15 D108).
struct LengthEdit {
    /// The value to write: on the frame it changed, and again on the frame a scrub
    /// ended — see the note at [`length_field`]'s own return for why the second is
    /// the one the valve needs.
    next: Option<Length>,
    /// Whether it must commit **now**. A unit toggle is a click: there is no drag
    /// for a valve to wait on, and `edit_valve` would sit on it forever.
    immediate: bool,
    resp: egui::Response,
}

/// A `Length` field with a clickable unit, for the paragraph scope.
///
/// **`mixed` replaces the digits with a dash and nothing else** — the number under
/// it is still `current`, so a scrub starts from the value at the range's start and
/// makes the range uniform, which is what the character fields do
/// ([`mixed_text`]). A unit click behaves the same way: it converts the value
/// shown and writes it to every paragraph in the range.
#[allow(clippy::too_many_arguments)]
fn length_field(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    prefix: Prefix,
    current: Length,
    mixed: bool,
    font_size: f64,
    pct_range: std::ops::RangeInclusive<f64>,
    tooltip: &str,
) -> LengthEdit {
    // ⚠️ **One range in, not two, and the px face is derived** — see [`Bounds`]
    // for the class this closes. The four callers used to pass a `px_range` of
    // `±MAX_LINE_HEIGHT_PX` beside a `pct_range` of `±MAX_TRACKING_PCT`, which
    // are the same two numbers every other `Length` field in this file was
    // passing and describe no quantity in common.
    let (mut shown, scrub, typed) = match current {
        Length::Em(m) => (
            m * 100.0,
            Scrub::whole(0.5).range(pct_range),
            expr::Unit::Pct,
        ),
        Length::Px(v) => (
            v,
            Scrub::whole(0.5).range(px_range_for(pct_range, font_size)),
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
        // `mixed_text` last, so its dash wins over the number formatter rather than
        // being replaced by it.
        //
        // **The parser goes on here rather than being the one every field gets**,
        // which `value_field_f64` sets before calling this closure precisely so a
        // caller can take it back: this is a field with two units, so a `px` typed
        // into its `%` face is a rejection instead of a wrong number (§15 D359).
        |d| {
            mixed_text(
                d.custom_formatter(ui::number(2))
                    .custom_parser(move |t| expr::eval_in(t, typed)),
                mixed,
            )
        },
    );
    // The field is labelled by a 14pt glyph and nothing else, so the tooltip is
    // the whole of what says which number this is. It was taken and dropped on the
    // floor here, which is why the paragraph fields have never had one.
    let resp = resp.on_hover_text(tooltip.to_string());
    if unit_clicked {
        return LengthEdit {
            next: Some(current.in_unit(current.unit().other(), font_size)),
            immediate: true,
            resp,
        };
    }
    LengthEdit {
        // **A scrub reports on the frame it changes the value and again on the frame
        // it ends**, and the second half is not redundant: the release is the frame
        // `edit_valve` commits on, and it is the one frame a changed-only field has
        // nothing to say, because `value_field` has already settled the number to
        // whole units on every dragged frame. Gating on the change alone therefore
        // withheld the value on exactly the frame that owed one, and the last drag of
        // a paragraph field lived in the preview until the next commit anywhere
        // cleared it — §15 D109's shape, one level up from where D109 was fixed.
        //
        // It costs nothing on a click: a stationary press-release is a click and
        // never a drag, so `drag_stopped` does not fire and there is no identical
        // transaction to commit. Both halves are pinned by
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
    /// The value to write. The outer `Option` is whether there is one at all — a
    /// field showing "auto" has nothing to write and no drag that could produce
    /// one — and the inner is the field's own absent state.
    next: Option<Option<Length>>,
    /// Whether it must commit **now**, for [`LengthEdit::immediate`]'s reason: a
    /// unit click has no drag for the valve to wait on.
    immediate: bool,
    resp: egui::Response,
}

/// A `Length` field whose absent state means "the font decides".
///
/// The suffix cycles `auto → px → % → auto`, so the `None` state is one of the
/// units rather than a separate switch — the same three-states-on-one-field shape
/// as line height, and for the same reason: it is a value, not a collapsed
/// control.
///
/// **Reports on a frame the value changed, and again on the frame the drag ends**
/// — [`length_field`]'s rule, adopted here by §15 D765. The gate this function
/// *originally* had cost these two fields both halves of a valve: they committed on
/// *every* changed frame of a drag, one undo step each, and then said nothing at all
/// on the release frame — D108 and D109 respectively, on the two fields that were
/// never moved onto the valve with the rest. The `drag_stopped` term is what stops
/// that happening again, and it is why the rule is not a plain `shown != before`.
///
/// 🚨 **It reported on *every* frame it had a value between then and D765, on an
/// argument that cited a method which does not exist.** The sentence read *"the
/// shape `OndinApp::char_length` uses for the other character-scope `Length`
/// fields, and the house rule for anything that ends at a valve"* — there is no
/// `char_length` anywhere in the tree, and the link reference under this doc
/// pointed the name at the **type**, so `cargo doc` resolved it and the gate stayed
/// green. **A citation that resolves to the wrong item is worse than one that
/// dangles**, and this one was the whole of the argument for the divergence
/// `[S6.2-L3-09]` was filed about.
///
/// ⚠️ **The two `char_valve` callers are *not* a third answer to this question.**
/// `length_char_field` and `type_line_height_field` hand `char_valve` a value
/// unconditionally because that function takes a `CharAttr` rather than an
/// `Option`, and its own doc records D109 being caused by a guard in that position.
/// They have a different contract, not a different opinion.
///
/// ⚠️ **`signed` is the caller's, because this one function serves a signed
/// field and an unsigned one** (§15 D546). `[S6.3-L1-02]`: it chose a symmetric
/// range for both, and the range was chosen for the **offset** — which is
/// legitimately signed, positive-up, so pushing an underline clear of the
/// descenders means typing a negative number. **Thickness inherited that sign
/// for no reason but sharing the function**, so `−5 px` was accepted, shown and
/// saved, and then rendered three different ways: the canvas floors it and drew
/// nothing, the SVG writer emitted `text-decoration-thickness="-5px"` verbatim,
/// and re-importing that file gave a third answer because `svg_in` reads no
/// decoration thickness at all.
///
/// **The model clamps too** (`typography::canonical_decoration`), and that is
/// the half that matters for files already saved; this half is what stops the
/// field offering the value in the first place.
///
/// ⚠️ **`resolved` is what leaving "auto" seeds, and it is not decoration** (§15
/// D570). `[S6.3-L1-01]`: the `None` arm used to seed `Px(0.0)` on the argument
/// that *"the font's own value is not knowable here"*, which was true and is no
/// longer — and the conclusion never followed from it anyway. **A zero thickness is
/// not a thin line, it is no line**: `render::scene::decoration_path` and
/// `core::text::crossings` each open by refusing a band of zero height, so one
/// click on the *Font* chip — the click that means *"show me this in px"* — erased
/// the underline, with `0.00 px` on screen and no way to guess what to type back.
/// `next_line_height` had solved exactly this two fields away (§15 D162) and this
/// one did not know.
///
/// `None` for a caller with nothing to seed from, which is still the **offset**
/// field: the face's own offset is available on the same metrics, but the panel's
/// offset is positive-*up* against parley's own convention (§15 D151) and getting
/// that sign wrong silently moves every underline. Seeding zero there puts the band
/// on the baseline, which is visible and recoverable; seeding zero on a thickness is
/// not.
///
#[allow(clippy::too_many_arguments)]
fn optional_length_field(
    ui: &mut egui::Ui,
    size: egui::Vec2,
    prefix: Prefix,
    current: Option<Length>,
    font_size: f64,
    signed: bool,
    resolved: Option<f64>,
    tooltip: &str,
) -> OptionalLengthEdit {
    let floor = |lo: f64| if signed { lo } else { 0.0 };
    // `length_field`'s `before`, for the release-frame rule at the foot of this
    // function (§15 D765): the value as the field was handed it, so `shown != before`
    // means *this frame moved it*.
    let (mut shown, scrub, unit, typed) = match current {
        // The unit the "auto" face parses in is arbitrary and unreachable: its
        // scrub range is zero-width and `next` is withheld while `current` is
        // `None`, so nothing typed here can reach the document whatever it says.
        None => (
            0.0,
            Scrub::whole(1.0).range(0.0..=0.0),
            "auto",
            expr::Unit::Px,
        ),
        // ⚠️ **Derived from the `%` face below rather than hard-coded**, which is
        // the whole of [`Bounds`]' argument at the one field that took no
        // `Bounds` at all: these two ends used to be `±10 000 px` against
        // `±200 %`, so `Px(50)` at 16pt converted to `Em(3.125)` and came back
        // as 32px — a measured loss on one click of the unit chip.
        Some(Length::Px(v)) => (
            v,
            Scrub::fine(0.1, 2).range(px_range_for(
                floor(-MAX_TRACKING_PCT)..=MAX_TRACKING_PCT,
                font_size,
            )),
            "px",
            expr::Unit::Px,
        ),
        Some(Length::Em(m)) => (
            m * 100.0,
            Scrub::whole(0.5).range(floor(-MAX_TRACKING_PCT)..=MAX_TRACKING_PCT),
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
            // **What clicking does, not what the field is** — the third of the three
            // divergences `[S6.2-L3-09]` measured across this file's three `Length`
            // fields, and the last one still open (the `%` cap closed with
            // [`Bounds`], and the release-frame rule is a ruling rather than a slip).
            //
            // 🚨 **This chip wore the *field's* label, and the field wore nothing.**
            // A decoration's thickness chip hovered as *"Thickness — the font's own
            // unless set"*, which describes the number beside it and says nothing
            // about the click — on the one chip in this file whose click is
            // dangerous enough to have earned a §15 entry of its own: D570, where it
            // used to seed `Px(0.0)` and make the underline vanish. The two
            // arguments are `length_field`'s comment below, from the other side —
            // *"it was taken and dropped on the floor here"* — happening again, with
            // the tooltip relocated onto the wrong control rather than lost.
            //
            // ⚠️ **Three faces, not two**, which is why this is not
            // `length_field`'s literal string: `next_optional_length` cycles
            // px → % → the font's own, so *"switch unit"* would be two-thirds true.
            tooltip: "Click to cycle: px, %, or the font's own",
        }),
        &mut shown,
        scrub,
        // Its own parser for [`length_field`]'s reason — px and % on one field
        // (§15 D359) — and on the "auto" face too, where it is inert but not worth
        // a second spelling.
        |d| {
            let d = d.custom_parser(move |t| expr::eval_in(t, typed));
            if current.is_none() {
                return d.custom_formatter(|_, _| "Font".into());
            }
            d.custom_formatter(ui::number(2))
        },
    );
    // `length_field`'s line, for its stated reason: the field is labelled by a 14pt
    // glyph and nothing else, so this is the whole of what says *which number* this
    // is. It had been spent on the chip instead.
    let resp = resp.on_hover_text(tooltip.to_string());
    if unit_clicked {
        return OptionalLengthEdit {
            next: Some(next_optional_length(current, font_size, resolved)),
            immediate: true,
            resp,
        };
    }
    OptionalLengthEdit {
        // Nothing to write while the field shows "auto": its scrub is clamped to a
        // zero-width range, so there is no value the drag could have produced, and
        // writing the state back would only give an unfocused field a way to commit
        // an identical transaction.
        //
        // 🚨 **`length_field`'s release-frame rule, which this did not have** (§15
        // D765). This read `current.is_some().then(…)` — a value on **every** frame
        // the field is not `auto`, idle ones included — under a doc claiming that
        // was *"the house rule for anything that ends at a valve"*. ⚠️ **That claim
        // cited `OndinApp::char_length`, a method which does not exist anywhere in
        // the tree**, and the link reference beside it redirected to the *type*, so
        // `cargo doc` resolved it and the invented name never surfaced. It was the
        // whole argument for the divergence.
        //
        // The rule that survives reading is `length_field`'s: report on a frame the
        // value **changed**, and again on the frame the drag **ends**. The second
        // half is the one D109 is about and is why this is not a plain
        // `shown != before` — the release is the frame `edit_valve` commits on, and
        // a changed-only gate withholds a value on exactly it.
        //
        // ⚠️ **The `current.is_some()` term stays and is a different question**:
        // *is there a value at all*, against *has it anything to say this frame*.
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
/// Read the enum's own variants and expose them verbatim, so a variant added to
/// the model reaches the panel without a second list to keep in step. One caller
/// today — the **line-style** picker in `decoration_section`.
///
/// ⚠️ **This is not what draws the wrapping controls**, and this comment said it
/// was until 2026-09-06 (*"the three wrapping controls … are one function rather
/// than three blocks that could drift"*). Those three live in `wrap_section` and
/// are hand-painted segmented tracks, which a combo cannot be — so a reader
/// adding a fourth wrapping control wants `segment_label` there, not this.
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
        // The combo paints its border *inside* its rect, so its height is `size.y`
        // whole — subtracting the hairline (right for content inside a `field_row`)
        // left it 2px short of the fields beside it (§15 D85).
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
/// **Leaving "the font decides" seeds what the font was deciding** —
/// [`next_line_height`]'s rule, one field along and arrived at four months later
/// (§15 D570). This arm used to seed `Px(0.0)` on the argument that *"the font's
/// own value is not knowable here, and a made-up number would move the ink"*. The
/// premise was true when it was written and is not now (`TextLayout::
/// decoration_sizes`), and the conclusion never followed from it either: **zero
/// moves the ink further than any guess could — it removes it.**
///
/// `resolved` is `None` for a caller with nothing measured, and the zero fallback
/// is then what it always was. The **offset** field is that caller today, for the
/// reason [`optional_length_field`]'s own doc gives.
///
/// ⚠️ **The `Px` face is the seeded one and the cycle is not line height's.** These
/// two run `auto → px → % → auto` where line height runs `auto → % → px → auto`, so
/// the arm that reads `None` here produces a `Px` and the equivalent there produces
/// an `Em`. A fix copied across without reading the cycle seeds the wrong unit and
/// nothing says so — the number is still right.
///
/// A free function for [`next_line_height`]'s reason: it is the decision, and the
/// field around it needs a `Ui`.
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
/// **Leaving Auto seeds what Auto was worth**, `resolved` being the shaped first
/// line's own height, so the click changes the *unit* and not the type. It seeded a
/// round `120%` while the font's leading was unknowable in the panel, which moved
/// the ink of anything whose face disagreed — Inter's is nearer 121%, and a display
/// face can be anywhere from 100% to 150%. The round number is still the fallback
/// for a node with nothing laid out.
///
/// A free function because it is the decision, and `OndinApp` cannot be built
/// headlessly to test the method around it.
fn next_line_height(
    current: Option<Length>,
    font_size: f64,
    resolved: Option<f64>,
) -> Option<Length> {
    match current {
        None => Some(Length::Em(match resolved {
            // The multiple the face is actually giving, canonicalized like every
            // other stored `Length` so the field and the file agree.
            Some(px) if font_size > 0.0 => (px / font_size).max(0.0),
            _ => 1.2,
        })),
        Some(l @ Length::Em(_)) => Some(l.in_unit(LengthUnit::Px, font_size)),
        Some(Length::Px(_)) => None,
    }
}

/// A short prefix for an axis field — the tag itself, which is what a type
/// designer calls it and what the file records.
fn axis_prefix(tag: Tag) -> &'static str {
    // `Prefix::Text` wants a `'static str` and a tag is four bytes of anything,
    // so the well-known ones get their name and the rest share a marker. Better
    // than leaking a string per frame for a label the slider already names.
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
/// **Only `slnt`, and it needs one badly.** `fvar` measures slant
/// *counter-clockwise*, so the right-leaning oblique everybody means by "slant
/// 10°" is `slnt −10`: a spec transcribed straight into this field leans the text
/// the wrong way, and there is nothing else on the row to say so. The row already
/// carries the axis's own name and tag on hover, which is where a rule this
/// narrow belongs — it is a fact about the format, not about this control, and a
/// permanent mark on the row would be noise for every other axis.
fn axis_sign_hint(tag: Tag) -> Option<&'static str> {
    match tag.as_str() {
        "slnt" => Some("counter-clockwise degrees, so a right-leaning oblique is negative"),
        _ => None,
    }
}

/// Sort key for the feature list — the tags worth finding first.
///
/// **The stylistic-set arm asks for a *number*, and that is a fix.** It used to
/// be `t.starts_with("ss")`, which also caught `ssty` — math script-style
/// alternates, a shaping mechanic — and filed it under stylistic sets;
/// `typography::stylistic_set` answers the question the arm was asking.
fn feature_rank(tag: Tag) -> u8 {
    match tag.as_str() {
        "tnum" | "pnum" | "lnum" | "onum" => 0,
        "liga" | "dlig" | "clig" | "calt" => 1,
        "frac" | "ordn" | "zero" | "sups" | "subs" => 2,
        "smcp" | "c2sc" | "case" => 3,
        _ if stylistic_set(tag).is_some() => 4,
        _ if character_variant(tag).is_some() => 5,
        // Everything else the registry offers, then the mechanics and private
        // tags — which are only on screen at all under *Show all features*.
        _ if registered(tag).is_some_and(|r| r.offered) => 6,
        _ => 7,
    }
}

/// One tag from the OpenType feature registry: the registry's own name for it,
/// one line saying what it does, and whether the registry's *UI suggestion* says
/// a user should be choosing it.
struct Registered {
    name: &'static str,
    /// For the row's hover. `""` where there is nothing to say that is not
    /// already in the name — every mechanic, since a switch nobody should be
    /// touching does not want prose talking them into it.
    what: &'static str,
    offered: bool,
}

/// The registry entry for `tag`, or `None` for a tag the registry does not have.
///
/// `None` is the third tier and means *private tag*: four letters the font made
/// up, which is then the only honest thing to show for it.
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
/// **The tags are registered, not invented by the font**, which is what makes a
/// static table completable rather than a guess: the registry fixes both the tag
/// and its meaning, so a font cannot make `ccmp` mean something else. All 124
/// individually registered tags are here — OpenType 1.9.1, the `featurelist`
/// index and the five `features_*` description pages — and that completeness is
/// load-bearing: a tag *missing* from this table is reported to the user as
/// private, so an omission is a wrong answer rather than a gap.
///
/// `ss01`–`ss20` and `cv01`–`cv99` are the registry's two **font-defined** ranges
/// and are deliberately absent: the font names those itself, in a `FeatureParams`
/// table ([`ondin_core::text::FaceFeature`]), and [`feature_label`] resolves them
/// before consulting this table.
///
/// **Names are the registry's own *Friendly name*, lowered to sentence case.**
/// The case is the only thing about them that is ours, and it is not gratuitous:
/// the `ssXX`/`cvXX` labels beside them come out of a font's `name` table, where
/// sentence case is what type designers write ("Open digits", "Alternate one").
///
/// **`offered` is the registry's answer too.** A tag is withheld when its own UI
/// suggestion says control "should not generally be exposed to the user", which
/// covers the required features (`ccmp`, `rvrn`), the ones a shaper applies per
/// script (`init`, `medi`, `fina`, `isol`, `mark`, `mkmk`, `curs`, `dist`,
/// `locl`, `rlig`, `rclt`) and the ones an application applies as a corollary of
/// another (`numr` and `dnom` under `frac`). Four departures, each because the
/// switch would be a lie rather than a mechanic:
///
/// - **The vertical-writing features** (`vert`, `vrt2`, `vrtr`, `valt`, `vhal`,
///   `vpal`, `vchw`, `vkrn`, `vapk`, `vkna`) are "active by default in vertical
///   writing mode", and this app has no vertical writing mode — so switching one
///   on can only mis-shape horizontal text. The `init`/`medi` argument, one axis
///   over.
/// - **`aalt`** feeds a *glyph palette*: its UI suggestion is to indicate which
///   glyphs have alternatives and let the user pick one. As a run-wide switch it
///   means nothing.
/// - **`ital`** is applied from the italic style, and the panel already has the
///   control the registry asks for — two ways to say italic is two ways to
///   disagree.
/// - **`size`** carries a tracking curve rather than an on/off state; the `opsz`
///   axis row is where optical size lives here.
const REGISTRY: &[(&str, &str, &str, bool)] = &[
    // ---- Offered: the registry says a user may choose these ----------------
    // Figures and numbers, which is where the list's highest-value entries are.
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
    // Line and paragraph fitting. The registry offers all three; nothing in this
    // app applies them for you, which is the difference between these and `numr`.
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
    // No prose on any of these: the row exists so that *Show all features* is a
    // complete list, not to talk anyone into switching one.
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
    // Vertical writing, which this app does not have. Switching one of these on
    // in horizontal text substitutes rotated or re-spaced glyphs into it.
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
    /// The row's hover: the name, the tag, and whatever else is known — the same
    /// shape an axis row's hover has, and the only place the four-letter tag
    /// appears now that the label is prose.
    hover: String,
    /// Whether it belongs in the list, or behind *Show all features*.
    offered: bool,
    /// What the feature's named values are called, in value order: index `k` is
    /// `FeatureSetting::value = k + 1`. Empty for the on/off features, which is
    /// nearly all of them.
    ///
    /// **Past one, the row is not a switch**: `values` is what a switch cannot
    /// say, and the length is what decides which control the row draws.
    values: Vec<String>,
}

/// How lit the loading dot is at `t` seconds, on 0.0–1.0 (§15 D353).
///
/// **Split out from the painting** — `tools::resized_box`'s division, and here
/// because a pulse is the one part of a drawing that a still frame cannot show:
/// what has to be right is that it *moves*, and moves smoothly, which is a claim
/// about a function of time rather than about any one frame's shapes.
///
/// A raised cosine rather than `sin`, so the breath starts at the trough: a dot
/// that appears at full brightness and fades reads as something switching off.
fn loading_dot_phase(t: f64) -> f32 {
    let turns = t / LOADING_DOT_PERIOD * std::f64::consts::TAU;
    ((1.0 - turns.cos()) * 0.5) as f32
}

/// The pulsing dot that says a family's face is still on its way (§15 D353).
///
/// **Never fully dark.** The floor is what keeps the dot a *thing on the row*
/// rather than something that blinks in and out of existence twice a second —
/// twenty of those in a scrolling list is a strobe. It breathes between a dim
/// accent and a bright one.
fn paint_loading_dot(ui: &egui::Ui, centre: egui::Pos2) {
    let phase = loading_dot_phase(ui.input(|i| i.time));
    let alpha = (90.0 + 150.0 * phase) as u8;
    ui.painter().circle_filled(
        centre,
        LOADING_DOT_R,
        theme::color::ACCENT.gamma_multiply(alpha as f32 / 255.0),
    );
    // **The animation's own clock.** The app repaints reactively, so without this
    // the dot freezes at whatever phase the last input left it on — which looks
    // like a static dot and says the opposite of what it is for. Bounded by the
    // popup being open and by there being something in flight.
    ui.ctx().request_repaint();
}

/// The *Show all features* / *Show fewer features* reveal: its text, or `None`
/// where it would be a control that cannot change anything.
///
/// **`hidden` is not a property of the face, and reading it as one is the bug this
/// function exists to hold.** A withheld feature the document has an opinion about
/// is shown anyway, so *touching* one — which writes it into the set, even to turn
/// it off — moves it out of the hidden count. Gating the control on that count
/// alone therefore made it vanish under the hand: switch the last hidden feature
/// while the list was expanded and the way back disappeared with it. Reported as
/// "sometimes the *Show fewer features* just disappears… on one font it was the
/// last one, on another it was the second one" — which is the same bug twice, the
/// ordinal being however many withheld features that face had.
///
/// So the two states ask different questions. **Expanded, the way back is
/// unconditional**: the user is in a mode and must be able to leave it. Collapsed,
/// the offer is only real if something is actually hidden — and if every withheld
/// feature is set in the document, nothing is, because they are all on screen
/// already.
fn features_reveal(show_all: bool, hidden: usize) -> Option<String> {
    match (show_all, hidden) {
        (true, _) => Some("Show fewer features".to_string()),
        (false, 0) => None,
        (false, n) => Some(format!("Show all features ({n} more)")),
    }
}

/// One row of the feature list for a feature that offers **more than one named
/// value** — a `cvXX` whose `numNamedParameters` is past one, which the OpenType
/// registry defines as a 1..n choice rather than an on/off feature. Returns the
/// value to write when the user picked a different one.
///
/// **A dropdown, and it takes the whole width under the name.** Measured, because
/// the alternative was arithmetic away: the list is 240 wide, and the widest real
/// pairing on hand — Charis SIL's `cv43` "Capital Eng" against its "Lowercase no
/// descender" — wants 240.1 on one line with the label in its own column. That is
/// not a near miss to shave a gap off; three faces is the whole evidence base, and
/// the *next* face's names are not bounded by anything. So the name keeps the row
/// it would have had as a switch, flush with every other label in the column, and
/// the control goes under it with all 240 to spend. [`OndinApp::axis_row`] made
/// the same trade for the same reason.
///
/// **A dropdown rather than a segmented track**, which is what the wrap controls
/// use: a track is for a handful of one- or two-word options, and these are named
/// by the *font* — "Straight with low hook" — with `numNamedParameters` free to
/// declare dozens of them.
///
/// **`Off` is an item in the list rather than a switch beside it.** The model has
/// one number for both questions (`FeatureSetting::value`: `0` off, `k` the k-th
/// named value), so a switch *and* a dropdown would be two controls writing one
/// field, and every pair of them in this app has eventually disagreed. It also
/// means the row does not reflow when the feature is turned on.
fn feature_choice_row(
    ui: &mut egui::Ui,
    tag: Tag,
    label: &FeatureLabel,
    value: u16,
) -> Option<u16> {
    let mut pick = None;
    ui.scope(|ui| {
        // **Every gap in this row is allocated, and `item_spacing` is zero** (§15
        // D195). Not style: a spacing set here is banked *after* the dropdown as well as
        // before it — egui advances the cursor past a widget by the spacing in
        // effect when the widget is placed, so it is paid on both sides and
        // zeroing it afterwards is too late. Measured: with the tail held at 6,
        // raising this row's spacing from 2 to 5 to 10 moved the gap *below* the
        // dropdown to 13, 16, 21. So a spacing cannot express "tighter inside than
        // outside" at all — it tightens both ends together, which is exactly how
        // the first version came out 5 within against 6 between and was reported
        // as "hard to understand where the row ends".
        ui.spacing_mut().item_spacing.y = 0.0;
        // The name line, painted rather than a `Label` so it sits on the same
        // baseline and the same left edge as the switch rows around it.
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
        // `CELL` whole: a combo strokes inside its own rect (§15 D85).
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
        // The other half of the same point: the row ends here, and the list's own
        // point of spacing is not enough to say so under a 26px control.
        ui.allocate_space(egui::vec2(0.0, CHOICE_ROW_TAIL));
    });
    pick.filter(|p| *p != value)
}

/// The gap between an n-way row's name and the dropdown under it.
///
/// Deliberately smaller than [`CHOICE_ROW_TAIL`], and the pair is the whole of
/// what makes a two-line row read as one row —
/// `a_choice_row_groups_its_label_with_its_control` asserts the *measured* gaps
/// rather than these two numbers, so the rule survives either being changed.
const CHOICE_LABEL_GAP: f32 = 2.0;

/// The gap under an n-way row's dropdown, before the next feature. The list adds
/// its own point on top of this; nothing else does, since the row's own
/// `item_spacing` is zero — see [`feature_choice_row`] for why that is load
/// bearing rather than tidiness.
const CHOICE_ROW_TAIL: f32 = 6.0;

/// The values a [`feature_choice_row`]'s list offers, as `(value, name)`.
///
/// **The `+ 1` lives here and nowhere else.** The list and the closed dropdown are
/// two readings of the same mapping — index `k` of the labels is the value `k + 1`,
/// because the OpenType spec spends the `ParamUILabelNameID` ids in value order
/// from one and `0` is off — and the way that breaks is not a crash: the row shows
/// the name of a neighbouring value and the document gets the one you did not pick.
fn feature_values(label: &FeatureLabel) -> impl Iterator<Item = (u16, &str)> {
    label
        .values
        .iter()
        .enumerate()
        .map(|(k, name)| (k as u16 + 1, name.as_str()))
}

/// What the closed dropdown of a [`feature_choice_row`] shows.
///
/// **A value the face does not offer is shown as the number it is**, rather than
/// clamped into the list or hidden behind "Off": a document written against a
/// different face — or a later version of this one — can carry `cv43 7` where this
/// face names three values, and the row that cannot say so is a row that silently
/// misreports the file. It is the same reasoning as the withheld-but-set feature
/// the list shows anyway.
fn feature_value_label(label: &FeatureLabel, value: u16) -> String {
    if value == 0 {
        return FEATURE_OFF.to_string();
    }
    feature_values(label)
        .find(|(v, _)| *v == value)
        .map_or_else(|| format!("Value {value}"), |(_, name)| name.to_string())
}

/// The off state of a feature, in the one place both the item and the closed
/// dropdown read it from.
const FEATURE_OFF: &str = "Off";

/// The three-tier answer to "what is this tag".
///
/// In order, because each tier is a different kind of authority:
///
/// 1. **`ssXX` / `cvXX` — the font's own strings**, which is the only case where
///    the file is the authority, and the generic "Stylistic set 7" is a fallback
///    rather than the answer: plenty of fonts document their `cvXX` set on a web
///    page and leave `FeatureParams` NULL in the binary.
/// 2. **A registered tag — [`REGISTRY`]**, where the *spec* is the authority.
/// 3. **Anything else — the raw tag**, with no invented prose, because a private
///    tag means whatever the foundry decided and nothing here can know.
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
    // **The generic label for a value the font declared and did not name is the
    // panel's**, exactly as "Stylistic set 7" above it is: core reports what the
    // font says and `None` where it says nothing. A font can declare a count and
    // leave the whole `ParamUILabelNameID` range NULL, and the control still has
    // to offer that many values — an unnamed one is "Alternate 2", not a gap in
    // the list, because the *number* is what gets written to the document.
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

/// The glyphs the three vertical-alignment cells show, in [`VerticalAlign::ALL`]
/// order. **The layer-align glyphs, deliberately**: these push the *box's*
/// contents to an edge, which is the same gesture the Align panel performs on
/// layers, where the text-align set describes ragged lines.
const VALIGN_GLYPHS: [&str; 3] = [
    icon::ALIGN_TOP,
    icon::ALIGN_CENTER_VERTICAL,
    icon::ALIGN_BOTTOM,
];

/// Languages worth offering by name. **A short list, not the whole of BCP-47**:
/// the tag only changes shaping where a font has language-specific rules, and
/// these are the ones that visibly do.
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
/// The pair travels together because a field switches between them at a click, so
/// a caller that named only one would have to remember which — and the two ends of
/// the same control disagreeing about its limits is exactly the sort of thing that
/// only shows up as a value that cannot be typed back.
///
/// ⚠️ **Only the `%` end is a number here; the `px` end is *derived* from it, and
/// that is the whole fix for a class of nine** (§15 D425). Before this, every `Length` field
/// in the panel took a hand-written px cap, and there was exactly one px cap in
/// the file: `MAX_LINE_HEIGHT_PX`, named for line height and used as the bound
/// for tracking, baseline shift, both decoration fields, the four paragraph
/// fields and line height. Nothing had ever computed a px cap for any quantity
/// but the one it is named after — so **no field's two ends bounded the same
/// quantity**, and the click that converts between them destroyed values:
/// measured, decoration thickness `Px(50)` at 16pt came back as 32px, and letter
/// spacing `Px(-100)` at 16pt came back as −8px, a **625×** disagreement at 8pt
/// between the two ends of one control.
///
/// The derivation is the only spelling under which a unit click cannot change
/// the value: `px = pct/100 × font_size` is exactly what
/// [`ondin_core::Length::in_unit`] computes, so a value legal at one end
/// converts to a value legal at the other, by construction rather than by two
/// constants happening to agree.
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
    ///
    /// ⚠️ **The only difference from [`Self::TRACKING`] is the `%` floor**, and
    /// it used to be less than that: both carried the same *symmetric* px range,
    /// so letter spacing refused `−51%` and accepted `−10 000 px` — a doc saying
    /// "runs both ways from zero" about a pair whose other half already did.
    /// With the px end derived, the sign rule is stated once and holds in both
    /// units.
    const SIGNED_TRACKING: Bounds = Bounds {
        pct: -MAX_TRACKING_PCT..=MAX_TRACKING_PCT,
    };

    /// The same range in px, at this font size.
    fn px(&self, font_size: f64) -> std::ops::RangeInclusive<f64> {
        px_range_for(self.pct.clone(), font_size)
    }
}

/// A `%` field range expressed in px at `font_size` — the one derivation every
/// `Length` field in this panel bounds its px face with.
///
/// ⚠️ **The font size is floored at 1, and it is not defensive noise.** A
/// multi-selection whose sizes disagree can report 0 (`TypeSubject::font_size`),
/// and a range of `0.0..=0.0` is a field that cannot be typed into at all —
/// which reads as a broken control rather than as a bound. One px of headroom is
/// meaningless as a limit and keeps the field alive.
fn px_range_for(
    pct: std::ops::RangeInclusive<f64>,
    font_size: f64,
) -> std::ops::RangeInclusive<f64> {
    let fs = font_size.max(1.0);
    (pct.start() / 100.0 * fs)..=(pct.end() / 100.0 * fs)
}

/// How much one press of `Ctrl+Shift+.` / `,` moves the font size
/// (`docs/shortcuts.md` §10).
///
/// Two, which is Illustrator's and InDesign's step — in this app's px rather
/// than in their points, since px is the unit the size field itself shows and a
/// chord that disagreed with the field beside it would be the worse surprise.
const FONT_SIZE_STEP: f64 = 2.0;
/// One press of `Alt+←`/`→` on letter spacing, per unit.
///
/// `0.01em` is one whole unit of what the field displays — it shows an em ×100,
/// so this is the `1%` step a designer would type. The px step is a tenth,
/// because px tracking is a fine adjustment where em tracking is a coarse one.
const TRACKING_STEP_EM: f64 = 0.01;
const TRACKING_STEP_PX: f64 = 0.1;
/// One press of `Alt+↑`/`↓` on line height, per unit. A tenth of an em is a
/// tenth of a line, and a px is a px.
const LEADING_STEP_EM: f64 = 0.1;
const LEADING_STEP_PX: f64 = 1.0;

/// Step a [`Length`] **in whatever unit it already carries**.
///
/// A chord must not silently convert the value: a tracking of `2%` nudged up
/// stays a percentage, and one of `0.4px` stays absolute. Converting would make
/// the same key mean different things depending on a unit chip the user set
/// deliberately, and would strand the value the next time the font size changed.
fn step_length(current: Length, step: i8, em: f64, px: f64) -> Length {
    let d = f64::from(step);
    match current {
        Length::Em(v) => Length::Em(v + d * em),
        Length::Px(v) => Length::Px(v + d * px),
    }
    .canonical()
}

/// A chord's stepped value, bounded by `range` **widened to admit `from`**.
///
/// 🚨 **Two decisions have to hold at once here and a bare `clamp` breaks one of
/// them** (§15 D840). §15 D817 says a held key stops where the field stops;
/// §15 D425 says a control **shows** a stored out-of-range value rather than
/// rewriting it, which is why these caps sit on the chord and not in
/// `TextStyle::set`. An unconditional `clamp` honours the first and violates the
/// second: with a stored `Em(5.0)` — 500%, legal, and a file may hold one —
/// `Alt`+`→` stepped to `5.0 + step` and then clamped to `MAX_TRACKING_PCT`,
/// so **the increase key decreased tracking by 300 percentage points** in one
/// press, and the value the user had was gone.
///
/// Widening the range by `from` gives both: inside the range this is exactly
/// `clamp`, so a held key still stops at the field's ends; outside it, the step
/// may move **toward** the range freely and is pinned at `from` going away from
/// it. So the out-of-range value is never rewritten by a key that was asked to
/// nudge it, and a user who holds the key in the sensible direction walks it
/// back into range and then stops there.
///
/// ⚠️ **Not the same as "skip the clamp when out of range"**, which is the
/// other obvious repair and is wrong in the away direction: it lets `Alt`+`→`
/// on a stored 500% walk to 600% and further, which is a control with no cap at
/// all for exactly the documents that most need one.
fn stepped_into(from: f64, next: f64, range: std::ops::RangeInclusive<f64>) -> f64 {
    next.clamp(range.start().min(from), range.end().max(from))
}

/// Field bounds. Caps on the *controls*, not on the model — any finite value is a
/// legal attribute and a file may hold one; what these bound is what can be typed
/// or scrubbed, past which the number has stopped describing type.
///
/// ⚠️ **All four are `%`, and there is deliberately no px constant any more.**
/// There used to be one — `MAX_LINE_HEIGHT_PX = 10_000.0`, named for line height
/// and used as the px cap of all *nine* `Length` fields in this panel, including
/// six that measure nothing like a line height. Every px face is now derived
/// from its own `%` face at the current font size ([`px_range_for`]), which is
/// the only arrangement under which the unit chip cannot change the value it
/// converts. If a px cap is wanted that is *not* the `%` cap in the other unit,
/// it is a second quantity and wants naming as one.
const MIN_LINE_HEIGHT_PCT: f64 = 0.0;
const MAX_LINE_HEIGHT_PCT: f64 = 1000.0;
const MIN_TRACKING_PCT: f64 = -50.0;
const MAX_TRACKING_PCT: f64 = 200.0;

#[cfg(test)]
mod tests {
    use super::*;

    /// **The six decisions `[S6.2-L6-07]` broke without a single test noticing**
    /// — five of them pinned here, the sixth in `wrap_gate_tests` (§15 D624, D804).
    ///
    /// 🚨 **37 of 38 production items in the panel's write half had zero test
    /// callers** — `char_valve`, the panel's central write seam with fifteen
    /// occurrences in production, among them — and the review flipped six
    /// documented decisions **simultaneously** to **975 passed / 0 failed**. A
    /// control flip (constant-folding `next_line_height`'s Auto seed) did bite at
    /// its predicted assertion, so the harness is live and the six greens mean
    /// *nothing reaches these decisions*.
    ///
    /// **Two of the three had to be lifted before a test could reach them at
    /// all** (§15 D269's move): the `(under, strike)` → cell mapping out of
    /// `decoration_section`, and the bold reading out of `type_weight_toggles`.
    /// The third, `matches_variant`, was already free and simply had no caller.
    ///
    /// 🚨 **The other three were called out of reach and two of them were not**
    /// (§15 D804). `list_section`'s gutter default, `wrap_section`'s
    /// `!= WrapMode::NoWrap` gate and `write_axis`'s `clear_at_default` were named
    /// as sitting inside `&mut self` bodies needing a live text session and a
    /// laid-out panel — *a fixture rather than a lift*. That was true of the wrap
    /// gate, which `wrap_gate_tests` drives with real pointer events. The other two
    /// read nothing but their parameters: what needed the panel was their
    /// **callers**, so `marker_attrs` and `axis_coords_after` lifted like the rest
    /// and are pinned in this module. ⚠️ **`clear_at_default` was also the same
    /// eight lines in two places**, only `axis_valve`'s copy covered (§15 D569),
    /// which is what made a grep for the name read as coverage.
    mod lifted_decisions {
        use super::*;

        /// **Flip 6, the sharpest of the six.** The strip's read and write halves
        /// are one rule, so they are asserted against each other: whatever cell a
        /// state lights, picking that cell must produce that state again.
        ///
        /// ⚠️ **The round trip is what has teeth, not the literals.** A mapping
        /// swapped in *both* halves would satisfy a table of expected numbers and
        /// still light the wrong glyph — so cell 1's meaning is pinned to
        /// `Underline` by name as well.
        ///
        /// **Flip-check, run**: swapping `(Some(_), _) => 1` and
        /// `(None, Some(_)) => 2` fails at the underline round trip with cell 2
        /// where 1 is wanted — the predicted site.
        #[test]
        fn the_decoration_strip_reads_and_writes_the_same_three_cells() {
            let d = Decoration::default();
            assert_eq!(decoration_cell(None, None), 0, "neither is cell 0");
            assert_eq!(decoration_cell(Some(d), None), 1, "underline is cell 1");
            assert_eq!(decoration_cell(None, Some(d)), 2, "strikethrough is cell 2");
            // ⚠️ Both at once is *underline*, which is the arm the model can reach
            // through a paste and the panel cannot: the strip has three cells and
            // no fourth, so it has to name one.
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
                // ⚠️ **Flattened, and the first draft was not.** `find_map` over
                // `Underline(Option<Decoration>)` yields `Option<Option<_>>`, so
                // the outer `is_some()` means *"the attribute was written"* — true
                // for `Underline(None)` as well — and asserted nothing about
                // whether a decoration is set. It failed on cell 0, which is the
                // one arm where the two readings differ.
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

        /// **Flip 2.** `weight >= 600` became `>= 100` — every face reading bold —
        /// with the whole workspace green.
        ///
        /// **Asserted at the boundary**, 599 against 600, because a threshold is
        /// wrong only at its ends; and the *written* weight is asserted beside the
        /// lit state, because they are one rule and a click that writes 700 from
        /// an already-bold face is the failure a state-only test misses.
        ///
        /// ⚠️ **Mixed is never lit whatever its weight**, which is the arm that
        /// makes the toggle a claim about the whole subject rather than about its
        /// first run.
        ///
        /// **Flip-check, run**: `>= 600` → `>= 100` fails on the 400 case, lit
        /// where it must not be — the predicted site. Dropping `&& !mixed` fails
        /// the mixed case.
        #[test]
        fn the_bold_toggle_lights_at_semibold_and_writes_the_other_end() {
            assert_eq!(bold_toggle(400, false), (false, 700), "regular offers bold");
            assert_eq!(bold_toggle(599, false), (false, 700), "one under the line");
            assert_eq!(bold_toggle(600, false), (true, 400), "semibold reads bold");
            assert_eq!(bold_toggle(700, false), (true, 400), "and so does bold");
            assert_eq!(
                bold_toggle(900, true),
                (false, 700),
                "a mixed selection is never lit, however heavy its runs"
            );
        }

        /// **Flip 3.** The axis-agreement tolerance `< 0.05` became `< 1000.0`, so
        /// every variant matched, with the whole workspace green.
        ///
        /// **Three separate rules in one predicate**, and each has its own case
        /// here: weight and slope must be equal, an axis the variant *names* must
        /// agree within tolerance, and an axis it does **not** name is the user's
        /// business — a named instance says nothing about `opsz`, which is the
        /// clause a naive "all coords equal" would break.
        ///
        /// **Flip-check, run**: widening the tolerance fails the `wght` 400-vs-700
        /// case; narrowing it to `== 0.0` fails the within-tolerance case, which is
        /// the other side and the one that says the tolerance is not decoration.
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
            // A static face names no axes, so nothing about the user's coords can
            // disqualify it — only its weight and slope can.
            let static_bold = v(700, false, Vec::new());
            assert!(matches_variant(
                &static_bold,
                700,
                false,
                &[at(wght, 123.0), at(opsz, 99.0)]
            ));
        }

        /// **Flip 5, and the gutter is the second of three said to need a
        /// fixture** (§15 D804).
        ///
        /// ⚠️ **Turning a marker *off* must not touch the indent**, which is the
        /// asymmetry the rule's own doc states and the one an "if the indent is
        /// zero, set it" reading would lose. A gutter the user can see is theirs
        /// once it exists.
        ///
        /// ⚠️ **The zero test is on the resolved value**, so an `Em(0.0)` indent
        /// counts as no gutter exactly as `Px(0.0)` does — the two are different
        /// values to the model and the same ink (§15 D537, D799), and a test that
        /// only tried `Px` would pass against a predicate comparing variants.
        ///
        /// ⚠️ **Flipped**: dropping the `next.is_some() &&` writes a gutter when
        /// the marker is turned off — red on the third assertion. Dropping the
        /// `== 0.0` test writes one over an indent the user set — red on the
        /// second. Neither is caught by the other.
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

        /// **Flip 4, and it was said to be out of reach** (§15 D804).
        ///
        /// `roadmap.md` listed `write_axis`'s `clear_at_default` with two other
        /// decisions as needing *"a live text session and a laid-out panel to
        /// enter"* — *"a fixture rather than a lift"*. That was true of its
        /// **callers** and never of the rule: every operand is a parameter, so
        /// D269's move reaches it and this is the lift, not a fixture.
        ///
        /// 🚨 **It was also eight lines in two places**, `write_axis` and
        /// `axis_valve`, of which only the valve's copy was covered (§15 D569) —
        /// the exact shape the roadmap's own ⚠️ warns about, *"a grep for the
        /// name reads as though the decision were reached"*. Both call
        /// `axis_coords_after` now, so one assertion covers both arms.
        ///
        /// ⚠️ **`opsz` is the case the rule exists for and it is asserted first.**
        /// For every other axis a value equal to the font's default is dropped,
        /// because a stored coordinate that says what the font already says is
        /// noise in the file. For `opsz` the *absence* of a coordinate is the Auto
        /// state, so dropping one would silently change the mode rather than reset
        /// a value — which is why the flag exists at all rather than the rule
        /// being unconditional.
        ///
        /// ⚠️ **Flipped three ways, all run, and each lands on a different
        /// assertion** — which is the argument for four of them rather than one.
        ///
        /// - Dropping the `clear_at_default &&` so the rule is unconditional:
        ///   red on the **`opsz`** assertion, `[]` against a written coordinate.
        ///   That is the only assertion that can see this flip, and it is the one
        ///   saying the flag has a job at all.
        /// - Dropping the `value == axis.default` so the flag alone decides: red
        ///   on the **second** assertion, "any other value is written". ⚠️ The
        ///   prediction said the first; the first is about a *default* value,
        ///   which this flip still drops, so it stays green.
        /// - Inverting the `!` so only defaults are kept: red on the **first**.
        ///
        /// So no two of the three share a site, and dropping any assertion here
        /// would let one of the three through.
        ///
        /// ⚠️ **And the filter is asserted, not just the push.** Setting an axis
        /// that is already present must *replace* it and not append a second
        /// coordinate for the same tag, which no assertion about length alone
        /// would catch if the filter also dropped a neighbour.
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

            // **The `opsz` case, and the reason the flag is a parameter.**
            let o = axis(opsz, 14.0);
            assert_eq!(
                axis_coords_after(&o, &[], 14.0, false),
                vec![at(opsz, 14.0)],
                "for optical size the absence of a coordinate is the Auto state, \
                 so a default value is written out like any other — dropping it \
                 would change the mode rather than reset the value"
            );

            // **Replacement, not accumulation.**
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

    /// **The *Show all features* reveal is a button, and a `Label` is text.**
    /// Reported as a text cursor over it, and this is the reading that both
    /// reproduces that and settles which call fixes it: hovering the same label
    /// with `selectable(true)` and no hover cursor reports `CursorIcon::Text`;
    /// with `selectable(false)` alone it reports `Default`, an arrow.
    ///
    /// ⚠️ **`selectable(false)` is now the whole fix, and the third case is why
    /// that is a choice rather than a limitation.** The row also carried an
    /// `on_hover_cursor(PointingHand)`, which works and outranks the label's own
    /// whichever way `selectable` goes — and it has been removed, because the app
    /// shows the arrow over chrome everywhere and changes the cursor only where the
    /// change means something (§9.2). Keeping that case asserted is what stops the
    /// arrow reading as an accident of egui to the next person here: the hand was
    /// available, it was measured, and it was declined.
    ///
    /// **The pointer is aimed at the response's own rect from the frame before**,
    /// because a `Ui` with no panel around it does not put the label where a guess
    /// would: aiming at a fixed point reported `Default` for every configuration —
    /// a probe that discriminated nothing (§15 D96).
    #[test]
    fn the_features_reveal_asks_for_an_arrow_and_a_bare_label_asks_for_an_i_beam() {
        let hovered = |selectable: bool, hover_cursor: bool| {
            let ctx = egui::Context::default();
            crate::theme::install(&ctx);
            // One pass to make the fonts available, since the label measures a galley.
            let _ = ctx.run_ui(Default::default(), |_| {});
            let mut icon = egui::CursorIcon::Default;
            let mut at = egui::Pos2::ZERO;
            // A widget's interaction state is last frame's, so pump a few.
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
        // The declined alternative, kept measured: the hand is reachable and beats
        // the label's own cursor either way, so the arrow above is a decision.
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

    /// **The way out of a mode cannot be gated on the mode having anything left to
    /// do.** Reported: toggling a feature sometimes made *Show fewer features*
    /// disappear — because touching a withheld feature writes it into the set,
    /// which makes it shown, which takes it out of the hidden count the control was
    /// gated on. The ordinal differed per font because the count did.
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
        // Collapsed, the offer has to be real: with every withheld feature set in
        // the document they are all on screen already, so there is nothing to show.
        assert_eq!(features_reveal(false, 0), None);
        assert_eq!(
            features_reveal(false, 7).as_deref(),
            Some("Show all features (7 more)")
        );
    }

    /// **Clicking the font list's search field closed the font list.** Reported, and
    /// the cause is egui's own default rather than anything here: a `ComboBox` gets
    /// `PopupCloseBehavior::CloseOnClick`, and that is a click *anywhere, inside or
    /// outside* the popup — so the field the list exists to be filtered by dismissed
    /// it on the way in.
    ///
    /// The claim is about **egui's** popup behaviour rather than about this row, so
    /// this drives the *shape* it uses — a combo whose popup
    /// holds a text field above rows — and pins all three states: the symptom under
    /// the default, the fix under `CloseOnClickOutside`, and that picking a row
    /// still closes the popup, which after the fix only `ui.close()` does.
    #[test]
    fn a_combo_survives_a_click_in_its_search_field_but_not_a_pick() {
        // `true` when the popup was still drawing two frames after the click.
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
                            // **Inside a `ScrollArea`, as the real list is.** A
                            // `ui.close()` has to reach the popup's `Area` from
                            // in here, and a probe that skipped the scroll area
                            // would be proving it for a shape the code does not
                            // have.
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

    /// Charis SIL's `cv43`, which is the face this control was built against:
    /// three named values, the longest of which is what settled the layout.
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

    /// **The list and the closed dropdown are one mapping read two ways**, and the
    /// way it breaks is silent: an off-by-one shows the name of a neighbouring
    /// value and writes the one you did not pick. So the round trip is the
    /// assertion, and the first value is pinned to `1` explicitly — `0` is off,
    /// which is what an index would have collided with.
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

    /// **A value the face does not offer keeps its number rather than being
    /// clamped or hidden.** A document written against another face can carry
    /// `cv43 7` where this one names three values, and the row that quietly showed
    /// "Off" — or the third value — would be misreporting the file it is editing.
    #[test]
    fn a_value_past_the_end_is_shown_as_the_number_it_is() {
        assert_eq!(feature_value_label(&capital_eng(), 7), "Value 7");
    }

    /// **A declared value the font did not name still gets a row**, because the
    /// number is what reaches the document: `numNamedParameters` is the count, and
    /// a NULL label id means the font declined to name that one, not that it does
    /// not exist. The generic text is the panel's, like "Stylistic set 7" above it.
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

    /// **One named value is still a switch.** Across the three faces that name any
    /// value at all — Charis SIL, Andika, Gentium Plus, since no *installed* face
    /// names one — 106 `cvXX` features name a value, and **97 of them name exactly
    /// one** against 9 naming more. So the common case must not grow a dropdown
    /// listing "Off" and one other thing.
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

    /// **A two-line row in a list of one-line rows has to be tighter inside than
    /// outside**, or it reads as a stray label above someone else's control.
    /// Reported that way — "hard to understand where the row ends" — and the first
    /// version had it exactly backwards: 5 within against the list's 1 between.
    ///
    /// Measured off the ink and the control rather than off the two constants,
    /// which would be a tautology: the label's baseline, the dropdown's top and the
    /// next row's label are three positions the user can actually see. The list's
    /// own 1px spacing is set here because the row is only ever drawn inside it —
    /// without that the outer gap under test is not the one on screen.
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
        // The dropdown's own painted frame — the one shape as wide as the list.
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
        // **Half again, not merely more.** Measured, the version reported as
        // unreadable is 9 inside against 10 below — it *satisfies* "outside is
        // bigger" and still looks like two loose halves, because one point of
        // difference is nothing at this size. That is not a hypothetical: the
        // obvious assertion was written first and passed against it. The shipped
        // pair measures 6 and 11.
        assert!(
            below >= inside * 1.5,
            "the row's halves are {inside} apart and the next row is {below} away: \
             too close to call, which is how the first version read"
        );
    }

    /// **The n-way row is a row of the same list**, and the only way to know is to
    /// measure both: its name has to sit at the left edge a switch row's does, in
    /// the same size, or a column of a dozen features has one row that has slipped.
    ///
    /// The width assertion is the other half, and it is the one reading could not
    /// settle: `ComboBox::width` is not documented as the *whole* widget's width,
    /// so whether a full-width dropdown fits the 240 the list has is a question
    /// about egui rather than about us. Flipping the call to `rect.width() + 1`
    /// fails it, which is the check that it is asserting anything at all.
    #[test]
    fn a_choice_row_lines_up_with_the_switch_rows_and_fits_the_list() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        // One pass so the fonts exist; a galley cannot be laid out before that.
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

    /// **The table's completeness is what makes the third tier mean anything.**
    /// `feature_label` reports a tag that misses `REGISTRY` as the font's own
    /// private tag, so a dropped row is a wrong answer rather than a gap — and a
    /// duplicated row is a silent one, since the lookup takes the first.
    ///
    /// 124 is the count of *individually* registered tags in OpenType 1.9.1: the
    /// `featurelist` index has 126 entries, two of which are the `ss01`–`ss20` and
    /// `cv01`–`cv99` ranges this table deliberately leaves to the font.
    ///
    /// ⚠️ **This asserted the table's *cardinality* until §15 D599, and a count
    /// cannot see a substitution** — which in a table of 124 four-letter literals is
    /// the likeliest edit there is. Measured: `("qwid", …)` retyped as
    /// `("qwix", …)` left `cargo test -p ondin-app` at **970 passed / 0 failed**,
    /// with `qwid` then reported to the user as the font's own private tag, hidden
    /// behind *Show all features*, and carrying the raw four letters where a name and
    /// a description belonged. The control — `kern` retyped as `kzrn` — went red, but
    /// only in
    /// `a_mechanic_is_withheld_a_stylistic_set_is_named_and_a_private_tag_is_neither`
    /// below, whose `["tnum", "liga", "kern", "smcp"]` loop names **four** of the 124
    /// by hand. So the machinery worked and the coverage was four. `[S6.3-L6-04]`.
    ///
    /// **The set, sorted, as a literal** is what closes it: a typo is a one-line diff
    /// naming both strings, which is the failure message this table's own doc asks
    /// for (*"a tag **missing** from this table is reported to the user as private,
    /// so an omission is a wrong answer rather than a gap"*). The dedup and the
    /// length assertions are kept — they are what says the list below is not being
    /// satisfied by a duplicate — but they are no longer the only thing holding the
    /// data.
    ///
    /// ⚠️ **This is `[A8-L6-04]`'s mechanism — "the test asserts a count" — at its
    /// second site**, `theme.rs`'s icon catalogue being the first. Worth reading the
    /// two together.
    ///
    /// Flip-check, run: `qwid` → `qwix` now fails here with
    /// `gone: ["qwid"], arrived: ["qwix"]`. ⚠️ **That message is the second draft and
    /// the first one is the lesson**: `assert_eq!(tags, EXPECTED)` is the same
    /// assertion and it printed both 124-element lists, burying the one string that
    /// moved in 248 that had not. **A set assertion needs the set *difference* in its
    /// message, or it reports the haystack.** The control is that the unmodified
    /// table passes, which it must, since the data was checked against the registered
    /// list when it was written.
    #[test]
    fn the_feature_registry_is_the_whole_registry_and_says_each_tag_once() {
        /// Every tag the table must hold, sorted, so a substitution names both
        /// strings. Not `REGISTRY`'s own order: the table is grouped by what the
        /// features *do*, which is the right order to read it in and the wrong one
        /// to diff it in.
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
        // **The two differences, not the two lists.** `assert_eq!(tags, EXPECTED)`
        // is the same assertion and prints 124 strings twice, which buries the one
        // that moved — measured under the flip below before this was written.
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
            // The two halves of the curation, each with its own rule: a feature
            // offered to the user says what it does, and a withheld one carries
            // no prose talking anyone into switching it (see `REGISTRY`).
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

    /// **Leaving Auto must not move the type.** The cycle's first step seeded a
    /// round 120%, so clicking the unit on a face whose leading is anything else
    /// re-spaced every line — the click says "show me this in percent", not "change
    /// it". With the resolved height in hand the seed is the multiple the face was
    /// already giving.
    #[test]
    fn leaving_auto_seeds_the_line_height_the_font_was_giving() {
        // 24.2 on a 20px face is 121%, which is about Inter's.
        let next = next_line_height(None, 20.0, Some(24.2));
        assert_eq!(next, Some(Length::Em(1.21)));
        // Nothing laid out: the round number is still the fallback, since the
        // alternative is seeding a zero.
        assert_eq!(next_line_height(None, 20.0, None), Some(Length::Em(1.2)));
        // A zero font size cannot be divided by, and the *state* is still what the
        // click asked to change.
        assert_eq!(
            next_line_height(None, 0.0, Some(24.2)),
            Some(Length::Em(1.2))
        );
        // The rest of the cycle is unchanged: % → px resolves, px → auto clears.
        assert_eq!(
            next_line_height(Some(Length::Em(1.5)), 20.0, Some(30.0)),
            Some(Length::Px(30.0))
        );
        assert_eq!(
            next_line_height(Some(Length::Px(30.0)), 20.0, Some(30.0)),
            None
        );
    }

    /// **A chord steps a `Length` in whatever unit it already carries** — the rule
    /// `step_length`'s doc states in the imperative and nothing asserted
    /// (§15 D718, `[S6.3-L6-05]`).
    ///
    /// The doc: *"A chord must not silently convert the value: a tracking of `2%`
    /// nudged up stays a percentage, and one of `0.4px` stays absolute. Converting
    /// would make the same key mean different things depending on a unit chip the
    /// user set deliberately, and would strand the value the next time the font
    /// size changed."*
    ///
    /// 🚨 **The finding's flip was predicted green and *was* green.** Rewriting the
    /// `Em` arm as `Length::Px(v + d * em)` — precisely the silent conversion the
    /// doc forbids, and the slip a careless edit makes — left the whole workspace
    /// at exit 0, all 31 suites. One `Alt+→` on a `2%` tracking would have stored
    /// `0.01px` and flipped the field's unit chip under the user's hand, with every
    /// gate green.
    ///
    /// ⚠️ **`step_length` is the shape §15 D269 says to lift logic into** — free,
    /// pure, `Copy` in and `Copy` out — and it had **zero** test callers, while
    /// `next_line_height`, the same shape on the same type, has five in the test
    /// directly above this one. *Being the ideal test subject is not the same as
    /// being tested*, and the two tests sitting side by side is what makes that
    /// legible. (It is the *tests* that are neighbours — the two functions are
    /// nowhere near each other in this file.)
    ///
    /// The step sizes are asserted with the unit because they are the other half of
    /// the same promise: `TRACKING_STEP_PX` is a tenth of `TRACKING_STEP_EM`'s
    /// magnitude on purpose (px tracking is a fine adjustment, em a coarse one), so
    /// a test that only checked the *unit* would pass against arms that swapped
    /// their step constants.
    ///
    /// ⚠️ **Flip-check, run: the finding's own mutation, `Length::Em(v) =>
    /// Length::Px(v + d * em)`.** Red here at the first assertion, `Px(0.03)`
    /// against `Em(0.03)` — the same edit that was **exit 0 across the whole
    /// workspace** before this test existed.
    #[test]
    fn a_chord_steps_a_length_without_changing_its_unit() {
        // The doc's own two examples. `2%` is `Em(0.02)` — the field shows an em
        // ×100 — so one step up is `Em(0.03)`.
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

        // Down, and across zero, which is where a sign slip would show.
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

        // Leading's constants through the same function: the unit rule is the
        // function's and the magnitudes are the caller's.
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

        // A step of zero is a step of zero — the identity, in the same unit.
        for l in [Length::Em(0.02), Length::Px(0.4)] {
            assert_eq!(
                step_length(l, 0, TRACKING_STEP_EM, TRACKING_STEP_PX),
                l,
                "no press, no change, no conversion"
            );
        }
    }

    /// **Leaving *Font* on a decoration thickness must not erase the line** — the
    /// test above, one field along and four months later (`[S6.3-L1-01]`, §15 D570).
    ///
    /// The chip means *"show me this in px"*. It wrote `Px(0.0)`, and a zero-height
    /// band is refused by `render::scene::decoration_path` and by
    /// `core::text::crossings`, so the underline vanished with `0.00 px` on screen
    /// and no number to guess back. The core half — that the seed draws ink and zero
    /// does not — is `text::tests::the_seeded_thickness_draws_a_band_where_zero_draws_none`;
    /// this is the panel's half.
    ///
    /// ⚠️ **The `None` fallback is still zero and that is deliberate, not an
    /// oversight**: the *offset* field passes `None` and its face metric has the
    /// opposite sign convention (§15 D151). Asserted so a later reader does not
    /// "finish the job" by seeding an offset it has not checked the sign of.
    ///
    /// ⚠️ **And the cycle is not line height's.** These fields run `auto → px → %`
    /// where line height runs `auto → % → px`, so the seeded unit here is `Px` and
    /// there is `Em`. The two `1.3671875` assertions below are what pins that: a fix
    /// transplanted from the twin without reading the cycle produces the right
    /// number in the wrong unit, and at 20pt `Em(1.37)` is a 27px underline.
    ///
    /// **Flip run**, the seed put back to `Px(0.0)`: fails on the first assertion,
    /// `Some(Px(0.0))` against `Some(Px(1.3671875))` — the predicted site.
    #[test]
    fn leaving_the_font_seeds_the_thickness_the_face_was_drawing() {
        // Inter's underline size at 20pt, measured — `text::tests::
        // a_shaped_node_reports_the_faces_own_decoration_thicknesses` is where that
        // number comes from.
        let inter = 1.367_187_5;
        assert_eq!(
            next_optional_length(None, 20.0, Some(inter)),
            Some(Length::Px(inter)),
            "leaving Font shows what the font was giving, in px — not zero"
        );
        // Nothing measured: the old behaviour, kept for the offset field.
        assert_eq!(
            next_optional_length(None, 20.0, None),
            Some(Length::Px(0.0)),
            "a caller with nothing to seed from still seeds zero, deliberately"
        );
        // The rest of the cycle is untouched: px → % converts, % → auto clears.
        assert_eq!(
            next_optional_length(Some(Length::Px(10.0)), 20.0, Some(inter)),
            Some(Length::Em(0.5))
        );
        assert_eq!(
            next_optional_length(Some(Length::Em(0.5)), 20.0, Some(inter)),
            None
        );
    }

    /// The three tiers, one assertion each, and the middle one is the reported
    /// problem: `aalt` / `ccmp` / `locl` are exactly the column of mechanics that
    /// made the list unreadable.
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

        // A font's own name wins, and the generic is a fallback rather than the
        // answer — plenty of faces leave `FeatureParams` NULL.
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

        // Tier three invents nothing: the four letters, and a hover saying why.
        let private = feature_label(&face_feature("Zork"));
        assert!(!private.offered);
        assert_eq!(private.name, "Zork");
        assert!(
            private.hover.contains("Not a registered feature"),
            "{private:?}",
            private = private.hover
        );
    }

    /// `feature_rank`'s stylistic-set arm was `t.starts_with("ss")`, which filed
    /// `ssty` — math script-style alternates, a shaping mechanic — under stylistic
    /// sets, next to the sets a designer chooses between.
    #[test]
    fn the_sort_does_not_file_ssty_under_stylistic_sets() {
        let sets = feature_rank(Tag::parse("ss01").unwrap());
        assert_eq!(sets, 4);
        assert_ne!(feature_rank(Tag::parse("ssty").unwrap()), sets);
        // And `cvXX` is its own tier, one below the sets.
        assert_eq!(feature_rank(Tag::parse("cv13").unwrap()), 5);
    }

    /// The reported bug: the popup showed for a frame and vanished.
    ///
    /// **The opening click is a click outside the popup**, because the popup does
    /// not exist when it happens — so on its first frame `clicked` is true and
    /// `on_menu` is false. Forgiving the button's own rect is the whole fix, and
    /// this case is the one a dismiss test forgets to write.
    #[test]
    fn the_click_that_opens_the_popup_does_not_dismiss_it() {
        assert!(!dismissed_by_click(ClickAway {
            on_head: true,
            ..away()
        }));
    }

    /// The other exemption, and a different miss: the popup's own combo lists are
    /// separate `Area`s, so choosing a language is a click that lands on neither
    /// the popup's rect nor the button's.
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

    /// **The release that ends a drag is not a click away from anything.** A scrub
    /// started in the popup is finished — or cancelled with a right-click — with
    /// the pointer wherever it has been dragged to, which is usually out over the
    /// canvas. Reported twice: reading the primary button alone was not enough,
    /// because the *cancel* is the right-click while the left button is still down,
    /// and the release that follows it is a primary event.
    #[test]
    fn ending_or_cancelling_a_drag_does_not_dismiss_the_popup() {
        assert!(!dismissed_by_click(ClickAway {
            in_gesture: true,
            ..away()
        }));
    }

    /// The fifth exemption, and the other half of the fourth: `in_gesture` catches
    /// a drag **in flight**, and on the frame it ends `ctx.dragged_id()` has
    /// already gone back to `None` (measured). So the release that finishes a
    /// scrub started on the card is caught by where the *press* was instead.
    #[test]
    fn the_release_that_ends_a_scrub_started_on_the_card_does_not_dismiss_it() {
        assert!(!dismissed_by_click(ClickAway {
            press_away: false,
            ..away()
        }));
    }

    /// The third exemption, and the one that made a control unusable rather than
    /// merely awkward: the decoration swatch opens the detached colour picker,
    /// which is an `egui::Window` **below** this popup in layer order — so it is
    /// neither `on_menu` nor `over_overlay`, and the first click in it closed the
    /// popup that owned it, which orphaned and closed the picker too.
    #[test]
    fn a_click_in_the_picker_this_popup_opened_does_not_dismiss_it() {
        assert!(!dismissed_by_click(ClickAway {
            on_picker: true,
            ..away()
        }));
    }

    /// And the case the whole test exists to preserve: a click on the page really
    /// does close it, so the exemptions above have not made it undismissable.
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

    /// **The button's rect has to reach the popup**, which the decision table
    /// above cannot check: it would pass with `on_head` computed from nothing.
    ///
    /// So this asserts the seam instead — that the row *returns* a rect at all,
    /// and that it is the popup's anchor. Driven through `run_ui` rather than by
    /// calling the row directly, because the rect only exists once something has
    /// laid the button out.
    #[test]
    fn the_alignment_row_hands_back_the_rect_the_popup_anchors_to() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut got: Option<egui::Rect> = None;
        let _ = ctx.run_ui(Default::default(), |ui| {
            ui.set_width(268.0);
            // A bare button in the same allocation the row makes, so the test
            // measures the seam rather than reimplementing the row.
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
        // The anchor the popup computes from it: right-aligned, just below. The
        // real constant, not a copy of it — a copy is what let this go on
        // asserting "276px" after the card was redrawn at 272.
        let anchor = egui::pos2(rect.right() - MENU_W, rect.bottom() + 6.0);
        assert!(anchor.y > rect.top(), "the popup hangs below the button");
        assert!(
            anchor.x < rect.left(),
            "a {MENU_W}px popup right-aligned on a 26px button reaches left of it"
        );
    }

    /// **The alignment row sits on the column grid of the fields above it.**
    ///
    /// Reported twice, from opposite directions. First "horizontal text align and
    /// vertical text align have different per-button sizing" — 37 against 20,
    /// because one took the row's remainder and the other was pinned at 22 a cell.
    /// Then, once both were pinned at the design's 28: "the horizontal align
    /// control is 2px narrower than the field above, while the vertical align
    /// control is 3px to the left of the field's above left edge."
    ///
    /// So the thing to pin is the **grid**, not the cell: the first track's right
    /// edge on the first field's, the second track's left edge on the second
    /// field's, the button's right edge on the margin. Equal cells cannot also
    /// hold — four across a half-card and three across the rest is not an equal
    /// division — and the tolerance below is what is left of that goal: the two
    /// cells still land within a point of each other and of the button.
    ///
    /// Measured off the **painted** rects, because the arithmetic is what is under
    /// test and restating it would pass whatever it became.
    #[test]
    fn the_alignment_row_lines_up_with_the_field_columns_above_it() {
        // The Type card's content width: 284 wide, `ui::CARD_MARGIN_X` a side.
        const CARD_INNER: f32 = 284.0 - ui::CARD_MARGIN_X * 2.0;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        // The column edges the paired field rows above produce: `inspector_type`
        // gives each field `half` and separates them by `GAP`.
        let half = (CARD_INNER - GAP) / 2.0;
        let field_a_right = half;
        let field_b_left = half + GAP;

        let mut tracks: Vec<egui::Rect> = Vec::new();
        let mut cells: Vec<egui::Rect> = Vec::new();
        let out = ctx.run_ui(Default::default(), |ui| {
            ui.set_width(CARD_INNER);
            // The alignment row, by the same arithmetic the row itself uses.
            let horizontal_w = (CARD_INNER - GAP) / 2.0;
            let vertical_w = CARD_INNER - horizontal_w - ROW_H - GAP * 2.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = GAP;
                // Cell 0 selected in each, so `segmented` paints a rect for it —
                // the resting cells draw only their glyph.
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
                // A raised cell is the track's `SEG_CELL_H` less the hairline it
                // now gives up on each side (§15 D386) — 22 inside a 28pt track,
                // which is the design's `1 + 2 + 22 + 2 + 1`.
                if (r.rect.height() - (SEG_CELL_H - 2.0)).abs() < 0.5 {
                    cells.push(r.rect);
                } else if (r.rect.height() - ROW_H).abs() < 0.5
                    // The button paints a fill and a stroke over one rect, and so
                    // does a track: ground, then hairline.
                    && !tracks.contains(&r.rect)
                {
                    tracks.push(r.rect);
                }
            }
        }
        // Two tracks and the button's ground, all `ROW_H` tall.
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
        // What is left of "one module": within a point of each other and of 28.
        assert_eq!(cells.len(), 2, "one raised cell in each track: {cells:?}");
        for (i, (cell, n)) in [(cells[0], 4), (cells[1], 3)].iter().enumerate() {
            let derived = seg_cell_of(if i == 0 { h_track } else { v_track }.width(), *n);
            assert!(
                (cell.width() - derived).abs() < 0.5,
                "track {i}'s painted cell is {}pt where its width implies {derived}pt",
                cell.width()
            );
            // **Two points, not one and a half, since the track gained its
            // hairline** (§15 D386): the border takes a point off each end of the
            // track, so a three-cell strip's cells each lose two thirds of one and
            // the wider of the two now sits 1.5 off the button rather than 0.83.
            // The number is a *sanity* bound on the module — the assertion above
            // is the one that pins the arithmetic — so it moves with the geometry
            // rather than the geometry being bent to keep it.
            assert!(
                (cell.width() - SEG_CELL).abs() < 2.0,
                "track {i}'s cell came out {}pt, more than two points off the \
                 {SEG_CELL}pt button",
                cell.width()
            );
        }
    }

    /// **The card's height does not ratchet down as tabs are visited.**
    ///
    /// Reported as: open on Character and the card is tall; switch to Box and it
    /// shrinks to fit; switch back to Character and it *stays* at Box's height;
    /// go to Paragraph and it shrinks again, and Character is now stuck at that.
    /// "Some smallest-opened-height kind of memory."
    ///
    /// The memory is a feedback loop, not a cache. A `ScrollArea` takes its outer
    /// size from `available_rect_before_wrap()`; inside an `Area` that is the
    /// area's size *from the previous frame*; and the area's size is what the
    /// scroll area produced. So each short tab teaches the loop a smaller number
    /// and nothing ever teaches it a larger one. Pinning an explicit `max_rect`
    /// cuts the loop.
    ///
    /// Driven through the same three phases the report describes — tall, short,
    /// tall — because a single pass of each proves nothing: the first frame of any
    /// `Area` has no previous size and so is always right.
    #[test]
    fn a_short_tab_does_not_shorten_the_card_for_the_next_tall_one() {
        const CAP: f32 = 400.0;
        const TALL: usize = 30;
        const SHORT: usize = 3;
        const ROW: f32 = 20.0;

        // One frame of a card whose body is `rows` tall, returning the height the
        // body actually took. `pinned` is the fix under test.
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
            // Two frames per phase: the first settles the `Area`'s size, the
            // second is what the user is looking at.
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
                // The bug, kept as an assertion so the fix cannot be quietly
                // reverted to "it was never broken".
                assert!(
                    tall_again < tall_first - 1.0,
                    "expected the unpinned form to ratchet ({tall_first} → \
                     {tall_again}); if it no longer does, egui changed and this \
                     test is measuring nothing"
                );
            }
        }
    }

    /// **The unit brightens on hover, and the colour it brightens to is the one
    /// that gets drawn.**
    ///
    /// Reported as "doesn't have a hover color", and the hover was working the whole
    /// time — `value_field_suffixed` computed it and passed it to
    /// `Painter::galley`, whose colour argument is a **fallback**: the tessellator
    /// substitutes it only where a section left `Color32::PLACEHOLDER`. The galley
    /// was laid out with `.color(theme::text::FAINT)` baked in, so the fallback was
    /// ignored and the ink never moved.
    ///
    /// **Both halves are asserted, because either alone passes while the control is
    /// broken.** A probe that read `fallback_color` back said the hover worked; one
    /// that read only the section colour would have missed that omitting the colour
    /// makes `into_galley` substitute the style's own `text_color` — full-strength
    /// white at rest, which was the second attempt.
    #[test]
    fn hovering_the_unit_changes_the_colour_it_is_actually_drawn_in() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        const W: f32 = 130.0;

        // The unit's `(fallback, baked)` pair, with the pointer wherever `at` says.
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
            // Twice: a widget's interaction state is last frame's, so the first pass
            // is the one that registers the hover and the second is what it draws.
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

        // The unit sits against the right edge: the field is 130 wide, its content
        // box ends 9pt inside that, and the unit hangs 4pt further out.
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

    /// **The hand-painted slider tracks the pointer and reaches both ends.**
    ///
    /// `ui::slider` replaced `egui::Slider` to get the design's 3pt rail and 11pt
    /// knob, and in doing so took over the arithmetic egui was doing: the knob's
    /// centre travels `half..width - half` so it cannot hang off the rail, which is
    /// exactly the sort of inset that silently costs you the last few percent of
    /// the range. So the ends are what is asserted, along with the middle.
    #[test]
    fn the_slider_reaches_both_ends_of_its_range() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});
        const W: f32 = 200.0;
        // Press in the middle of the rail, drag to `x`, and read where it settled.
        // The press has to land *on* the slider — one outside is not a grab, which
        // is why it starts at the middle every time.
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
        // Hard against each end, and the middle. A knob inset from the rail's ends
        // is the whole reason the travel is computed rather than assumed.
        assert_eq!(dragged_to(-40.0), 100.0, "dragged past the left end");
        assert_eq!(dragged_to(W + 40.0), 900.0, "dragged past the right end");
        let mid = dragged_to(W / 2.0);
        assert!(
            (mid - 500.0).abs() < 12.0,
            "the middle of the rail should be near the middle of the range, got {mid}"
        );
    }

    /// **A scrub reports its value again on the frame it ends, and a bare click
    /// reports nothing at all.**
    ///
    /// The release frame is the one the valve commits on (`edit_valve` acts on
    /// `drag_stopped() || lost_focus() || changed()`), and it is the one frame a
    /// *changed-only* field has nothing to say: `ui::value_field` settles the
    /// number to whole units on every dragged frame, so by the time the button comes
    /// up there is nothing left to round and nothing to report. The four paragraph
    /// `Length` fields and the `Level` beside them gated on exactly that, so the last
    /// drag of one never committed — it lived in the preview until the next commit
    /// anywhere cleared it. That is §15 D109's shape, one level up from where D109
    /// was fixed: the valve was made unconditional and the *field* in front of it
    /// still had the guard.
    ///
    /// Measured rather than read, because reading is what got it wrong: before the
    /// fix the release frame came back `next=None, drag_stopped=true`, and every
    /// frame of a four-step scrub scored `commits=false`.
    ///
    /// **Both halves, because either alone passes for the wrong reason.** Reporting
    /// the value unconditionally satisfies the release frame too — and would commit
    /// an identical transaction after a bare click, which is what the fix was priced
    /// at in `docs/roadmap.md` before anyone measured it. It turns out to cost nothing: a
    /// stationary press-release is a click and never a drag, so `drag_stopped` never
    /// fires and there is no junk undo step to trade for. The click half is what says
    /// so, and it is the half that fails against the over-eager version.
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
                    0.0..=1000.0,
                    "indent",
                );
                out = Some((e.next, e.resp.dragged(), e.resp.drag_stopped()));
            });
            let (next, dragged, drag_stopped) = out.unwrap();
            // What the preview does: the panel reads its subject back through it, so
            // the next frame's `current` is whatever this frame reported.
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
        // `paragraph_length` → `valve_paragraph` → `edit_valve`, spelled out: the
        // field has to report a value for the valve to be reached at all, and the
        // valve commits on any frame that is not a dragged one.
        let commits = |(next, dragged, drag_stopped): (Option<Length>, bool, bool)| {
            next.is_some() && !dragged && drag_stopped
        };

        // A real scrub: press, four frames of motion, release without moving —
        // which is the release egui reports, and the one the value has nothing to
        // add to.
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
        // The fixture is in the state this test is about, or the test is about
        // nothing: a scrub that never moved the number would satisfy every
        // assertion below by never reaching them.
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

        // A bare click, which is the whole cost of the rule above.
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

    /// **The decoration fields now follow the same rule, and nothing pinned them
    /// before** (§15 D765).
    ///
    /// `optional_length_field` reported on **every** frame it held a value — idle
    /// ones included — where `length_field` reports on a changed frame and again on
    /// the release. The divergence was one of the three `[S6.2-L3-09]` measured,
    /// and it was argued in that function's doc by a citation to
    /// `OndinApp::char_length`, **a method that does not exist**: its link reference
    /// redirected the name to the type, so `cargo doc` resolved it and nobody read
    /// it back.
    ///
    /// ⚠️ **The load-bearing assertion is the idle frame.** The release and changed
    /// frames report under *either* rule — the old every-frame gate is a superset —
    /// so a test that only drove a scrub would be green for both. The frame that
    /// tells them apart is a settled one with the value untouched.
    ///
    /// ⚠️ **And the release is asserted too**, because that is the half the gate
    /// must not lose: it is the frame `edit_valve` commits on, and a plain
    /// `shown != before` withholds a value there. §15 D109 is that bug.
    ///
    /// **Flip, run:** restoring `current.is_some().then(…)` fails at the idle-frame
    /// assertion — predicted correctly. Dropping the `|| resp.drag_stopped()` term
    /// fails at the release assertion, which is D109's shape and the reason both are
    /// here.
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
                    false,
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

        // 🚨 The frame that tells the two rules apart: settled, nothing touched, a
        // value present. The old gate reported here; the rule reports nothing.
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

        // A scrub still reports on the frames that matter.
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

    /// **A field with a unit paints the width it was given, and the unit lands
    /// inside it.**
    ///
    /// Reported as *"the %/px toggle is still outside the control"*, and it was
    /// one bug wearing two faces. `field_frame` shrink-wraps its content; the
    /// unit's strip was subtracted from the number's width and then *painted*
    /// over, never allocated — so the frame came out `trail_w` short of the size
    /// asked for while the unit was still placed against the full content box, and
    /// landed astride the border it should have been inside. Measured before the
    /// fix: a 130.5pt field painting 110.5, its `%` spanning 108.8–119.8 across a
    /// border at 110.5. The second symptom was the same number — a row of two such
    /// fields stopped 42pt short of the card ("line height and character spacing
    /// should both fill the full width").
    ///
    /// Both halves are asserted, because fixing either alone still looks wrong.
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
                    // The widest unit any field shows, and the one most likely to
                    // push past the strip reserved for it.
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
                // The field's own recessed ground, at the row's full height.
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
            // The chip carries `SUFFIX_CHIP_PAD` of its own past the text, so the
            // *text* sits a little further in than the chip's own 5pt.
            assert!(
                (2.0..12.0).contains(&inset),
                "field {i}'s unit is {inset}pt from the border — it should tuck \
                 against the right edge, not float in the middle"
            );
        }
        // And the pair fills the row it was given, which is the same bug's other
        // face: `half` is computed from `W`, so a frame that shrank left a gap.
        let right = frames[1].right() - frames[0].left();
        assert!(
            (right - W).abs() < 0.5,
            "the two fields spanned {right}pt of a {W}pt row"
        );
    }

    /// **No ink lands right of the card's inner edge.**
    ///
    /// The popup's width is pinned so that switching tabs does not resize it, and
    /// every one of its rows divides that width up by arithmetic — a field beside
    /// a 26px button, a label column beside a field beside a button, a `TextEdit`
    /// given whatever the magnifier leaves. Get one of those subtractions wrong
    /// and nothing errors: the row's contents simply reach past its border.
    ///
    /// **Asserted on the shapes, not on any `Ui`'s rect**, and that distinction is
    /// what makes the test worth having. The first version of it measured
    /// `ui.min_rect()` inside the row and passed with the swatch inflated from 14px
    /// to 120 and the hex field from 62 to 140: `allocate_ui_with_layout` bounds
    /// `max_rect`, so `min_rect` reports the allocation back rather than what the
    /// contents did with it, and a `TextEdit`'s `desired_width` is clamped to the
    /// room available in the first place. Both readings are the *budget*. The ink
    /// is the only thing that overflows, so the ink is what is measured.
    #[test]
    fn nothing_in_the_popup_paints_past_the_card() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        // One pass to make the fonts available, since both rows measure galleys.
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
                            .char_limit(ui::HEX_CHAR_LIMIT)
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
        // The rows are laid out from the origin, so the card's inner edge is
        // `MENU_INNER` across. A half-point of tolerance for the hairlines, which
        // egui strokes on the boundary rather than inside it.
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

    /// A subject over the whole of an eight-byte node, with `style` as its
    /// defaults and `spans` as its overrides.
    fn subject_of(style: TextStyle, spans: CharSpans) -> TypeSubject {
        TypeSubject {
            id: NodeId::from_wire("1:1").expect("a well-formed id"),
            style,
            spans,
            paragraph: ParagraphStyle::default(),
            block: BlockStyle::default(),
            sizing: TextSizing::Auto,
            text_paint: Some(Brush::Solid(Color::BLACK)),
            // Nothing shaped in a unit test, which is the `None` the field falls
            // back on rather than a case these tests are avoiding.
            line_height: None,
            decoration_sizes: None,
            range: 0..8,
            // Byte 0, which is what `read` gives a subject with no live session —
            // these are whole-node fixtures.
            resolve_at: Some(0),
            partial: false,
            para_spans: ParaSpans::default(),
            para_range: 0..8,
            para_partial: false,
        }
    }

    /// `subject_of` over **some** of a node's paragraphs — the state a live
    /// session with the caret in one of them produces.
    ///
    /// `para_range` is the snapped range the panel would have read off the editor,
    /// so a test states the paragraph it means rather than the selection that found
    /// it: the snapping itself is core's and is tested there
    /// (`a_paragraph_write_snaps_outward_to_whole_paragraphs`).
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

    /// **A whole-node paragraph write sets the defaults *and* drops the overrides**,
    /// which is the half a `SetParagraphStyle` alone would miss: an override left
    /// behind is a paragraph that ignores the control, and it is the same shape
    /// `char_attrs_tx` exists for.
    #[test]
    fn a_whole_node_paragraph_write_flattens_the_overrides_it_replaces() {
        let defaults = ParagraphStyle::default();
        let mut spans = ParaSpans::default();
        spans.set(0..4, ParaAttr::IndentStart(Length::Px(40.0)), &defaults);
        // And one of a *different* attribute, which must survive.
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

    /// A write that changes nothing reports so, because the click path commits only
    /// when it does — an undo step for a click that moved nothing is the bug this
    /// answers, and it is why `para_attrs_tx` returns the flag at all.
    #[test]
    fn a_paragraph_write_of_the_value_already_there_changes_nothing() {
        let defaults = ParagraphStyle {
            indent: Length::Px(20.0),
            ..ParagraphStyle::default()
        };
        let subject = para_subject_of(defaults, ParaSpans::default(), 0..8);
        let (_, changes) = para_attrs_tx(&subject, vec![ParaAttr::Indent(Length::Px(20.0))]);
        assert!(!changes);
        // And the canonicalization is in the path, so a value that only *looks*
        // different does not count either (`Length::canonical`, four places).
        let (_, changes) =
            para_attrs_tx(&subject, vec![ParaAttr::Indent(Length::Px(20.000000001))]);
        assert!(
            !changes,
            "a drag's float noise must not commit — that is what fragments a span list"
        );
    }

    /// **What the tab shows over a mixed range, and what it says about it.** The two
    /// halves are separate on purpose: `shown_paragraph` gives the value at the
    /// range's start so a field has digits and a drag has somewhere to start from,
    /// and `para_mixed` is what stops the field claiming that number is *the* value.
    #[test]
    fn a_paragraph_field_reads_mixed_over_paragraphs_that_disagree() {
        let defaults = ParagraphStyle::default();
        let mut spans = ParaSpans::default();
        // Paragraph two is indented, paragraph one is not.
        spans.set(4..8, ParaAttr::IndentStart(Length::Px(40.0)), &defaults);

        let both = para_subject_of(defaults.clone(), spans.clone(), 0..8);
        // Over the whole node with no session, `partial` is false — so however
        // ragged the spans are the field is not lying, because the next write
        // flattens them. That is `ragged` versus `mixed` in the other scope.
        assert!(!both.para_mixed(ParaAttrKind::IndentStart));

        let first = para_subject_of(defaults.clone(), spans.clone(), 0..4);
        assert!(first.para_partial);
        assert!(!first.para_mixed(ParaAttrKind::IndentStart));
        assert_eq!(first.shown_paragraph().indent_start, Length::ZERO);

        let second = para_subject_of(defaults.clone(), spans.clone(), 4..8);
        assert!(!second.para_mixed(ParaAttrKind::IndentStart));
        assert_eq!(second.shown_paragraph().indent_start, Length::Px(40.0));

        // A live selection reaching both is the mixed one, and it still shows the
        // value at its start.
        let mut across = para_subject_of(defaults, spans, 0..8);
        across.para_partial = true;
        assert!(across.para_mixed(ParaAttrKind::IndentStart));
        assert!(
            !across.para_mixed(ParaAttrKind::Spacing),
            "an attribute nothing overrides is not mixed just because its neighbour is"
        );
        assert_eq!(across.shown_paragraph().indent_start, Length::ZERO);
    }

    /// **A letter spacing of zero everywhere is not *Mixed*, whatever units the
    /// zeroes are in** (§15 D799, `[S6.1-L1-03]`).
    ///
    /// The reported move is the ordinary one for anyone who works in percent:
    /// click the `%` chip before typing a number. That writes `Em(0.0)` over the
    /// selection and leaves `Px(0.0)` — the default — everywhere else, and the
    /// two are different values to `Spans::shared_in`, which compares
    /// structurally and **must**: §15 D537 keeps that difference precisely
    /// because it is where the unit choice lives at amount zero. So the field
    /// went to a dash over text whose letter spacing is zero throughout.
    ///
    /// ⚠️ **The fixture guard is the first assertion, and it is not decoration.**
    /// If `set` ever canonicalized `Em(0.0)` to `Px(0.0)` on the way in, the
    /// spans would agree structurally, `shared_in` would answer on its own, and
    /// every assertion below would pass **without `agreed_zero` existing**. That
    /// is the vacuous version of this test and `assert_ne!` is what refuses it.
    ///
    /// ⚠️ **Flipped** by deleting `shared`'s `or_else(agreed_zero(…))` arm: red
    /// on the `mixed` assertion, the predicted site.
    ///
    /// ⚠️ **And flipped a second way, which is why there are two assertions and
    /// not one.** Returning a different one of the agreeing values — `.last()`
    /// in place of the non-default pick — leaves the `mixed` assertion **green**
    /// and fails the second at `Px(0.0)` against `Em(0.0)`. Nothing about the
    /// dash-or-digits question can see that, and what it breaks is the unit
    /// chip: it reads its suffix off this value, so a `%` document would flip to
    /// `px` under the user. **A test that only asked "is it mixed" would have
    /// shipped D537's own symptom by a new road.**
    ///
    /// 🚨 **And it shipped it anyway, because the fixture only ever put the
    /// explicit run at the head** (§15 D840). Both flips above were run against
    /// the one ordering document order answers correctly. Restoring
    /// `values.into_iter().next()` is now red on the `4..8` case and **green on
    /// the `0..4` case** — the two arms of one loop disagreeing, which is what
    /// says the ordering is the subject rather than the units.
    #[test]
    fn a_zero_letter_spacing_in_two_units_is_not_mixed() {
        assert_ne!(
            Length::Em(0.0),
            Length::Px(0.0),
            "the fixture rests on these being different values — if they ever \
             compare equal this test proves nothing"
        );

        // 🚨 **Both orderings** (§15 D840). The explicit run at the head was the
        // only one covered, and it is the one case document order gets right:
        // with the run at 4..8 the first value is the *default*, and this test
        // reported `Px(0.0)` in its own words while claiming to prevent exactly
        // that. One character of the fixture separated a passing test from the
        // defect it was written to catch.
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

    /// **A real disagreement still reads *Mixed*** — the control for the test
    /// above, and the reason it is a separate one.
    ///
    /// `agreed_zero` is reached only where `shared_in` has already answered
    /// `None`, so the risk it introduces is that it says "agreed" too often. A
    /// non-zero amount on one side is the case that must not collapse, and a
    /// zero against a non-zero is the case that is genuinely mixed and shares a
    /// zero with the test above — so it is the one an over-broad predicate would
    /// swallow.
    ///
    /// ⚠️ **Flipped** by dropping `agreed_zero`'s `is_zero` test — accepting any
    /// two lengths as agreed — and **the predicted site was wrong**. It is red on
    /// the **first** assertion, two non-zero amounts, and it never reaches the
    /// second. The prediction assumed a zero beside a number is the harder case
    /// for an over-broad predicate to get right; it is not, because that
    /// predicate stopped looking at amounts at all, and the first pair it meets
    /// is the one that fails. **Both assertions are kept** — the first is what
    /// this flip catches, and the second is the one that would survive a
    /// predicate that checked *one* side for zero rather than all of them.
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

    /// **The lit button that did nothing.** The hanging pair's click compares against
    /// `shown_paragraph`, which over the whole node reads the *first* paragraph — so an
    /// override on the second one left "first-line" lit, the guard saw no change and
    /// wrote nothing while that paragraph plainly hung. `para_ragged` is the question
    /// the guard actually needed; this pins the two apart, because they agree in every
    /// case except the one that was broken.
    #[test]
    fn the_hanging_pair_has_something_to_do_when_the_paragraphs_disagree() {
        let defaults = ParagraphStyle::default();
        let mut spans = ParaSpans::default();
        // Not the first paragraph — that is the whole point.
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
        // And the write it fires does flatten: the override goes, the default stays.
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

        // With nothing overridden the two questions agree again, and a click on the
        // lit button must still commit nothing.
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

    /// **Every character-scoped slot must round-trip through `char_scoped`, and no
    /// other slot may claim to be one.**
    ///
    /// That predicate is what all three write verbs branch on (§15 D154), so a slot
    /// it misses would be written as a document transaction — which for a character
    /// attribute is an empty one, i.e. a control that silently does nothing. This is
    /// the guard the old shape could not have: with the branch spelled out three
    /// times, `preview_slot` disagreed with its siblings and that was D129's gap.
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

    /// **One builder for every write to a character-scoped slot.** The row, the
    /// picker and the three verbs all go through it, so "set this colour" cannot mean
    /// two things — and `None` is the inherited state rather than a missing value,
    /// which is what the reset button writes.
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
        // A decoration slot on text carrying no decoration has nothing to colour,
        // which is also how the picker learns to close itself when one is switched
        // off underneath it.
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

    /// **A gradient-filled text node has to show its decoration inheriting the
    /// gradient.** `scene.rs` gives an uncoloured decoration `inks.first()`, so the
    /// canvas draws that underline with the gradient; the panel flattened the same
    /// fill to `Color::BLACK`, which put a flat black chip and `000000` on the row
    /// with nothing to distinguish it from black text. The chip is now the ramp,
    /// and the one colour the hex and the picker still need is the **first stop** —
    /// the collapse `write_char_slot` already performs when the gradient is
    /// adopted, so what the row promises is what clicking it produces.
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

        // A solid fill keeps the single chip it always had — the ramp is the extra
        // state, not a replacement.
        subject.text_paint = Some(Brush::Solid(last));
        assert!(subject.text_ramp().is_none());
        assert_eq!(subject.text_color(), last);

        // And a node with no visible fill still lands on black, matching the
        // stand-in `scene.rs` paints an unfilled node's decoration with.
        subject.text_paint = None;
        assert!(subject.text_ramp().is_none());
        assert_eq!(subject.text_color(), Color::BLACK);
    }

    /// The underline's colour as the resulting transaction leaves it: `Ok` of the
    /// node's default, and whether any span still overrides it.
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
                    // A span that still disagrees with the default about the
                    // underline is exactly what "the red survived" looks like.
                    overridden =
                        spans.shared_in(&TextStyle::default(), CharAttrKind::Underline, 0..8)
                            != Some(CharAttr::Underline(None));
                }
                _ => {}
            }
        }
        (default, overridden)
    }

    /// **Clearing a decoration's colour has to clear it everywhere.** Reported as
    /// "I set the underline's colour to red; clicking the reset button does
    /// nothing", and the write is the half of that worth pinning: an edit whose
    /// new value is *nearly* the old one is where `char_attrs_tx`'s no-op guard
    /// earns its keep or swallows the edit.
    ///
    /// Both shapes the red can be in, because they take different routes out: the
    /// node's own default (which `TextStyle::set` replaces) and a span override
    /// (which only `CharSpans::clear` removes).
    #[test]
    fn clearing_an_underline_colour_clears_the_default_and_the_spans() {
        let mut styled = TextStyle::default();
        styled.set(CharAttr::Underline(Some(red())));

        // Red as the node default.
        let subject = subject_of(styled.clone(), CharSpans::default());
        let (tx, changes) = char_attrs_tx(
            &subject,
            vec![CharAttr::Underline(Some(Decoration::default()))],
        );
        assert!(changes, "clearing a colour that is set is not a no-op");
        assert_eq!(underline_after(&tx), (None, false));

        // Red as a span over part of the node, the default plain — the shape the
        // panel cannot see through `shown()`, since that reads byte 0.
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

    /// **And the half that was actually broken: whether the button is live.**
    ///
    /// The write above was correct all along. What failed was `d.color.is_some()`
    /// as the enabled test — `d` is `shown()`, which over a whole node reads byte 0
    /// — so a colour living in a span that did not reach byte 0 left the button
    /// dimmed and inert with the red plainly on screen. `ragged` is the second
    /// term, and this is the case that needs it: nothing about the *shown* value
    /// distinguishes it from a decoration that never had a colour.
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

    /// **A tab label that does not fit its cell simply paints over the next
    /// one.** `ui::segment_label` centres a galley in a rect and clips nothing, so
    /// spelling the tabs out — `Character` where the strip used to say `Char` —
    /// is a change whose failure mode is invisible in the source and obvious on
    /// screen. The widest of the three, measured against the cell arithmetic the
    /// popup actually performs.
    #[test]
    fn the_spelled_out_tab_labels_fit_their_cells() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        // What `type_menu_popup` hands `segmented`, and what `segmented` then
        // divides it into: 2px of track padding on each side, 2px between cells.
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

    /// The same question of the two widest segmented tracks inside a tab, which
    /// have no close button to give up and so get the full inner width. `By
    /// script` and `Break word` are the labels the redesign introduced, and the
    /// four-cell Case track is the tightest geometry in the popup.
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

    /// Every `Length` field's two ends describe the **same quantity**, so the
    /// unit chip cannot change the value it converts.
    ///
    /// ⚠️ **This is what the panel's own `Bounds` doc has always claimed and
    /// what nothing checked.** That doc: *"the two ends of the same control
    /// disagreeing about its limits is exactly the sort of thing that only shows
    /// up as a value that cannot be typed back."* Every pair in the panel was
    /// such a disagreement, because there was exactly one px cap in the file —
    /// `MAX_LINE_HEIGHT_PX`, named for line height — and all nine fields used
    /// it. Worst case **625× at 8pt**; two measured losses on one click:
    /// decoration thickness `Px(50)` at 16pt came back 32px, letter spacing
    /// `Px(-100)` at 16pt came back −8px.
    ///
    /// **Driven through the real conversion**, `Length::in_unit`, rather than by
    /// comparing the two ranges as numbers: the arithmetic the field performs is
    /// the thing that has to land inside the bound, and asserting on the
    /// constants would pass against a derivation that used a different formula
    /// from the one the chip uses.
    ///
    /// **Three font sizes, because the disagreement is a function of font size**
    /// and a single size can be coincidentally fine — at 16pt line height's old
    /// pair was only 62× apart, which still reads as "large" rather than as
    /// "wrong".
    ///
    /// ⚠️ **The flip did not bite the first time, and that was a finding about
    /// this test rather than about the code.** With `Bounds::px` replaced by a
    /// hard-coded `-10_000.0..=10_000.0`, every assertion stayed green — because
    /// the loop below builds its px face by calling `px_range_for` *directly*
    /// and so never went through `Bounds::px` at all. The first block now asserts
    /// that the `Bounds` faces **are** the shared derivation, which is what makes
    /// the flip fail; without it this test was about arithmetic nobody uses.
    ///
    /// Flip-check, run (after that correction): the same one-line change fails on
    /// `Bounds::TRACKING`'s px face at 8pt, printing both ranges.
    ///
    /// ⚠️ **Its honest scope**: this asserts the *ranges* are coherent and that
    /// `Bounds` derives its own. It does not drive `length_field`,
    /// `optional_length_field` or `type_line_height_field`, each of which calls
    /// `px_range_for` inline — so a future field that hard-codes a px range again
    /// is not caught here. The thing that would catch it is `px_range_for` being
    /// the only way to build one, and it is not, because a `RangeInclusive` is a
    /// literal anyone can write.
    #[test]
    fn both_ends_of_every_length_field_bound_the_same_quantity() {
        // `Bounds` must have no px face of its own — see the ⚠️ above.
        for (name, b) in [
            ("TRACKING", &Bounds::TRACKING),
            ("SIGNED_TRACKING", &Bounds::SIGNED_TRACKING),
        ] {
            for font_size in [8.0, 16.0, 72.0] {
                assert_eq!(
                    b.px(font_size),
                    px_range_for(b.pct.clone(), font_size),
                    "{name} @{font_size}pt must derive its px face, not carry one"
                );
            }
        }

        // Every pct range the panel hands a `Length` field, with the name of the
        // field or fields it belongs to.
        let ranges: Vec<(&str, std::ops::RangeInclusive<f64>)> = vec![
            ("letter/word spacing", Bounds::TRACKING.pct.clone()),
            ("baseline shift", Bounds::SIGNED_TRACKING.pct.clone()),
            (
                "decoration thickness/offset",
                -MAX_TRACKING_PCT..=MAX_TRACKING_PCT,
            ),
            ("paragraph spacing", 0.0..=MAX_TRACKING_PCT),
            (
                "first-line indent, indent start, indent end",
                -MAX_TRACKING_PCT..=MAX_TRACKING_PCT,
            ),
            ("line height", MIN_LINE_HEIGHT_PCT..=MAX_LINE_HEIGHT_PCT),
        ];

        for (name, pct) in ranges {
            for font_size in [8.0, 16.0, 72.0] {
                let px = px_range_for(pct.clone(), font_size);
                for end in [*pct.start(), *pct.end()] {
                    // What the unit chip actually does to this value.
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

                // And the other direction: a px end converted to % has to land
                // inside the % face, or the chip destroys the value coming back.
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

    /// ⚠️ **The px face never collapses to a point**, however small the reported
    /// font size is.
    ///
    /// `TypeSubject::font_size` answers 0 for a multi-selection whose sizes
    /// disagree, and a derived range of `0.0..=0.0` is a field that cannot be
    /// typed into at all — which reads as a broken control rather than as a
    /// bound, and is the one way this derivation could be worse than the
    /// constants it replaced. `px_range_for` floors the size at 1 for that
    /// reason and this is the assertion that says so.
    #[test]
    fn a_derived_px_range_survives_a_font_size_of_zero() {
        let r = px_range_for(MIN_TRACKING_PCT..=MAX_TRACKING_PCT, 0.0);
        assert!(r.end() > r.start(), "{r:?}");
        assert!(
            r.contains(&1.0),
            "and something ordinary can still be typed"
        );
    }
}

/// **The egui behaviour `cancel_gesture` has to work around**, pinned because the
/// workaround looks like belt-and-braces without it.
///
/// `Context::dragged_id` is the natural answer to "is a field being scrubbed", and it
/// is the wrong one to ask on the frame a right-click arrives: egui clears `dragged`
/// on the release of **any** button (`interaction.rs`, `PointerEvent::Released`, whose
/// `button` is `_`), so a right-click short enough for its press and release to land
/// in one rendered frame takes the drag away before the cancel can see it. At 60fps
/// that is any click under ~16ms — reported from use as right-click resetting a value
/// "almost always", failing at random, with no pattern in where or how.
///
/// The two rows below are the whole diagnosis: a press on its own leaves the drag
/// alone, a press *and release* together do not. `cancel_gesture` therefore also asks
/// two questions of its own state, which do not blink.
///
/// **This is the premise, not the fix**: it pins what egui reports for a press and
/// for a press-and-release, which is what `cancel_gesture` is read against. The
/// gate itself is pinned separately now, in
/// `canvas::headless_app_tests::cancelling_needs_a_witness_and_our_two_do_not_blink`
/// (§15 D303) — which asserts the two witnesses that *do not* blink arm it without
/// egui's, since egui's is the one this module is about losing.
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
    //! The picker's "this row is still arriving" dot (§15 D353).
    //!
    //! **Reported from the machine**: *"Right now you load the font list, and
    //! you're not sure if the font you're seeing is loaded yet or not."* A row
    //! whose face has not arrived draws its name in the UI font, which is a real
    //! signal and an unreadable one — plenty of families look like the UI font,
    //! and a *failed* row looks identical to a *pending* one.
    //!
    //! What is pinned here is the pulse, as a function of time. The dot's
    //! placement and its ink are one `circle_filled` and are better checked by
    //! eye; whether it *moves*, and moves without a strobe, is arithmetic.

    use super::*;

    /// **The dot breathes: it starts dark, reaches full, and comes back.**
    ///
    /// ⚠️ Sampled at the trough, the crest and the return rather than at one
    /// point, which is the caret-blink trap again: any monotonic ramp is "bright
    /// at half a period", and a constant is "bright" everywhere. Only the round
    /// trip separates a pulse from a fade-in.
    ///
    /// ⚠️ Flipped to plain `sin`, the obvious spelling. Predicted to fail on the
    /// **first** assertion and it fails on the **second** — `sin(0)` really is 0,
    /// so the dot does start dark; what breaks is that it peaks a *quarter* of the
    /// way through and is back at zero by the half, so "it never reaches full
    /// brightness" is what fires. The correction matters because it says which
    /// assertion is load-bearing: the trough is satisfied by both shapes and the
    /// **crest** is what picks the raised cosine out.
    ///
    /// ⚠️ `sin().abs()` — the fix somebody reaches for next, to stop the phase
    /// going negative — passes both of those and fails the *seam* test below at
    /// 0.0872 per frame, because folding the negative half doubles the rate. Three
    /// spellings, three different assertions: worth knowing that no single one of
    /// these tests pins the shape on its own.
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

    /// **It is periodic and smooth** — no jump at the seam, and no frame-to-frame
    /// step big enough to read as a flicker.
    ///
    /// The seam is the half worth asserting: a phase built from `t % PERIOD`
    /// scaled linearly is also "dark at 0, full at half", and it snaps from full
    /// back to dark at the period boundary. Sampling across the wrap is what tells
    /// the two apart.
    ///
    /// ⚠️ **The bound is derived, not picked, and picking it got it wrong first.**
    /// `phase(t) = (1 − cos(2πt/P))/2` differentiates to `(π/P)·sin(2πt/P)`, so the
    /// steepest it can move is `π/P` per second and — by the mean value theorem —
    /// no 1/60s step can exceed `(π/P)/60`. That is exact rather than generous.
    /// This first said `3.0/P` as a round stand-in for π and failed at t=0.267
    /// with a jump of 0.0426 against a bound of 0.0417: the *test* was wrong and
    /// the pulse was fine, which is worth knowing before reading a failure here as
    /// a flicker. Spelled from `LOADING_DOT_PERIOD` so slowing the pulse tightens
    /// the bound with it.
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

    /// **The row asks the service, and the service is now able to answer.**
    ///
    /// ⚠️ This is a note as much as a test. `family_row` needs a whole
    /// `TypographyPanel` and is checked by eye; what can be said here is that the
    /// predicate it reads — `FontService::is_loading` — is *truthful*, which it was
    /// not until the same change added the dot. A preview the backlog pushed out
    /// used to leave its family reporting `is_loading` for the rest of the session
    /// (`fonts::tests::a_preview_dropped_by_the_backlog_can_be_asked_for_again`),
    /// so a dot built on it would have pulsed for ever on exactly the rows a scroll
    /// had given up on. **The bug was older than the dot and invisible until
    /// something drew it** — which is the thing worth remembering about adding a
    /// visible state to a value nothing was displaying.
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

/// The Type panel's size field, driven through real pointer and key events
/// (§15 D523).
///
/// **Events, not calls**, because everything here is about *when* the valve
/// fires: which frame a `DragValue` reports `changed()` on, which frame it
/// writes its parsed edit string back, and which of those the commit lands on.
/// None of that is visible from `char_attrs_tx`.
#[cfg(test)]
mod char_valve_tests {
    use super::*;
    use crate::app::OndinApp;
    use crate::theme;
    use ondin_core::{Document, NodeId, Operation, Transaction};

    fn app_with_text() -> (egui::Context, OndinApp, NodeId) {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        let style = TextStyle {
            font_family: "Inter".into(),
            font_size: 20.0,
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
        (ctx, app, id)
    }

    /// One frame with the size field alone in a `Ui`, returning the rect it was
    /// laid out in — found by drawing rather than guessed, since a `Ui` with no
    /// panel around it does not put a widget where a guess would.
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
                    .type_size_field(ui, &subject, egui::vec2(120.0, 28.0))
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

    /// **A typed font size is one undo step, and the sizes on the way to it
    /// never reach the document** (§15 D523).
    ///
    /// Measured before the fix, frame by frame: typing `40` over a 20pt node
    /// committed on **both** keystroke frames, leaving `undo_depth` at 2 with
    /// the document having been at `4` in between. That second half is what
    /// makes this worse than `multi_valve`'s version of the same defect — a size
    /// typed as `100` puts a one-point line through the artwork and then a
    /// ten-point one, each of them a real committed state, each of them an
    /// autosave and a crash-snapshot write.
    ///
    /// ⚠️ The assertion *during* typing is the one that says so, and it is
    /// checked before the count. Flipped by restoring the old condition
    /// (`drag_stopped() || lost_focus() || changed()`): red there, and **reading
    /// 40, not 4** — the assertion runs after both keystrokes, so what it catches
    /// is that the document has been written to at all, and the `4` that was
    /// there a frame earlier is visible only frame by frame. The count assertion
    /// three lines down is never reached.
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

    /// **`Escape` abandons a typed size**, which this valve had no refusal for
    /// at all — `edit_valve` has carried one since §15 D316 and `multi_valve`
    /// since D519.
    ///
    /// ⚠️ Flipped by dropping the `key_pressed(Escape)` arm: red at the size,
    /// 40 for 20, and green at the count — abandoning and committing both spend
    /// the engagement exactly once, so the count cannot tell them apart.
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

    // 🚨 **The third arm is gone and the question was answered** (§15 D812,
    // ruled by the maintainer). This comment stood through the deletion saying
    // *"the third arm … has no test here"* and *"what is owed is not a test but
    // a question"* — both true when written, both false from the moment the arm
    // was removed later in the same range (§15 D840).
    //
    // The history, because it is the useful half: §15 D523 kept the arm for the
    // picker's raw sensed regions arriving through `CharWrite::Valve`, on the
    // reasoning that *"without it a click on the hue strip would commit nothing,
    // ever"*. §15 D802 measured that and found `picker::pointer_slot` answers
    // through `write_slot` before it could reach `valve_slot`, so a click never
    // entered `char_valve` at all. §15 D803 then found the one thing that did
    // reach it and it was a hazard rather than a user. D812 deleted it.
    //
    // **The test is `valve_arm_tests::a_changed_frame_with_no_engagement_commits_nothing`**,
    // about 1,600 lines below this in the same file — which is why a reader here
    // would conclude there is none.
}

#[cfg(test)]
mod skip_ink_tests {
    //! The skip-ink toggle beside the underline's line style (§15 D357).
    //!
    //! Driven through the real `decoration_section` on a headless app (§15 D303)
    //! rather than through `char_attrs_tx`, because what is new here is the
    //! *wiring*: the write itself is one field of a `Decoration` and could not go
    //! wrong quietly, while "the button is there, it says which state it is in, and
    //! clicking it reaches the document" is three things that can each be absent
    //! with everything green.

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

    /// **A stored typography value outside its field's range is shown, not
    /// rewritten — on a frame with no events at all** (§15 D475).
    ///
    /// `[S6.2-L1-01]`: a text node whose letter spacing is `Em(3.0)` — 300%,
    /// against `Bounds::TRACKING`'s 200% cap — selected, one frame of the panel
    /// drawn with an **empty** `RawInput`, and it came back `Em(2.0)` with
    /// `undo_depth 0 → 1` and `is_dirty()` true. At 16pt that is 48px of tracking
    /// silently reflowed to 32px, with nobody touching anything.
    ///
    /// ⚠️ **The mechanism was closed before this test existed**, by
    /// `ui::value_field_f64`'s `.clamp_existing_to_range(false)` — one line for
    /// all 82 numeric fields, landed for a blur radius rather than for this. What
    /// was missing is a test on **this** panel, which reaches the widget by three
    /// different routes and is the panel the finding measured:
    /// `length_char_field` through `char_valve`, `type_line_height_field`, and
    /// `axis_field`, which discards the `Response` entirely and writes through
    /// `write_axis` whose only guard is `if value == current { return; }` — a
    /// guard the clamp had just made false.
    ///
    /// ⚠️ **This panel has no second guard, and the widget's opt-out is the
    /// whole of what stops a library default writing to the document here** —
    /// where the passing control one function away, `paragraph_length`, routes
    /// through `edit_valve` and its latch. That is exactly why it wants a test
    /// of its own rather than inheriting `ui.rs`'s. 🚨 **The reason given here
    /// was that `char_valve` is "still the pre-D316 shape … with no engagement
    /// latch", and it has been false since §15 D523** gave the valve the latch;
    /// the verdict survives the correction because the latch is not what would
    /// absorb this. An idle egui-initiated rewrite is a `changed()` frame on a
    /// control holding no focus, which was the valve's **third** arm — it
    /// committed at once, having no engagement to wait on — so the opt-out was
    /// then the only thing in the way (§9.2 said the same).
    ///
    /// 🚨 **That arm was deleted later in this same range (§15 D812), and it is
    /// what this test now rests on** (§15 D840). With no third arm an idle
    /// `changed()` frame commits nothing, so the panel has **two** independent
    /// defences where it had one, and the opt-out is no longer *"the only thing
    /// in the way"* — the sentence above is kept in the past tense because the
    /// argument it carries is what the deletion replaced.
    ///
    /// ⚠️ **The in-range control is what keeps it honest**: without it, a field
    /// that had stopped reporting *any* change would pass.
    ///
    /// 🚨 **Flip-check: it no longer bites, and that is the finding rather than
    /// a failed experiment** (§15 D840). Removing
    /// `.clamp_existing_to_range(false)` from `value_field_f64` used to fail the
    /// first assertion with `Em(2.0)` where `Em(3.0)` was — the finding's own
    /// numbers, re-measured 2026-09-19 under §15 D803. Re-run after D812
    /// deleted the third arm, **this test stays green** while
    /// `ui::tests::a_stored_value_outside_a_fields_range_is_shown_rather_than_rewritten`
    /// and the inspector's grid-panel test go red. D425's line is still
    /// guarded — but from `ui.rs` and the inspector, which is precisely the
    /// inheritance §15 D475 exists to prevent.
    ///
    /// **Measured, both defences removed together**: with the opt-out gone *and*
    /// the third arm restored, it fails at `Em(2.0)` against `Em(3.0)` exactly
    /// as before. So neither single mutation can redden it, and the reason is
    /// defence in depth rather than a vacuous assertion — **a flip that does not
    /// bite because two things must fail at once is a different animal from one
    /// that does not bite because nothing is being tested**, and the two are
    /// indistinguishable without running the pair. This test is kept for the
    /// behaviour it states, and what it can no longer do is stand in for a
    /// single-mutation guard.
    ///
    /// 🚨 **`ui.rs` makes that call at four production sites and they are not
    /// interchangeable — a re-run spent on the wrong one came back green and was
    /// nearly written up as "the flip does not bite".** The one this test turns
    /// on is `value_field_f64`'s. `badge_field`'s is **inert**, and says so on
    /// its own line: *"both production callers pass an unranged `Scrub`, so
    /// there is nothing to clamp against"* — removing it changes nothing while
    /// that stays true. `plain_drag_value`'s and `bare_drag_value`'s (§15 D552)
    /// are the other two. **The confusable pair is `badge_field`'s and this
    /// one's, which are byte-identical** — same indentation, same chained call,
    /// no semicolon — so naming the mutation as *"removing
    /// `.clamp_existing_to_range(false)`"* does not identify it; naming the
    /// **function** does (§15 D803).
    ///
    /// 🚨 **And the flip said which arm of `char_valve` did the damage, which is
    /// how that arm came to be deleted.** Instrumented on the failing run, the
    /// third arm fired — the `changed() && !lost_focus()` one D802 found the
    /// picker cannot reach. So its only **known** user was this egui-initiated
    /// rewrite, and it was the mechanism by which `[S6.2-L1-01]`'s clamp became
    /// a committed document change and an undo step. This paragraph argued that
    /// deleting it would be *"a second defence against the same bug rather than
    /// tidying"*; §15 D812 deleted it, so the second defence is built and the
    /// argument is spent. Written in the past tense for that reason — a case
    /// made for a change that has since landed is history, not a proposal
    /// (§15 D840).
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

            // Two idle frames, because a widget's state is last frame's: the first
            // could not have committed anything it had not yet drawn.
            for _ in 0..2 {
                let subject = TypeSubject::of(&app, id).expect("a subject");
                let input = egui::RawInput {
                    screen_rect: Some(AREA),
                    ..Default::default()
                };
                let _ = ctx.run_ui(input, |ui| {
                    ui.set_width(MENU_INNER);
                    app.type_letter_spacing_field(ui, &subject, egui::vec2(120.0, 28.0));
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

    /// The smallest rectangle painted under `at`, with its fill — the button's own
    /// box and ground. The ground is what `FieldButton::On` changes and the only
    /// thing on screen that says which state the toggle is in; the box is what says
    /// the control fits, where the glyph's ink would fit inside an overflowing button.
    ///
    /// `button_face` paints the fill and then strokes the *same* rect, and `min_by`
    /// keeps the first of two equal minima — the fill — which is why this can ask for
    /// a colour and a rect in one call.
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

    /// **The toggle is there for an underline, it is lit while skipping is on, and
    /// clicking it reaches the document.**
    ///
    /// The lit assertion is the one worth explaining. Skip-ink defaults to *on*, so
    /// this button is lit the first time anybody sees it — deliberately, since it
    /// reports what the line is doing — and the failure that reading invites is a
    /// toggle that draws the same ground in both states, which is a control that
    /// silently does nothing you can see. Comparing the ground before and against
    /// after is what says the state is drawn at all; the exact colours are
    /// `button_face`'s and are not this test's business.
    ///
    /// ⚠️ **The two grounds have to be read on frames that differ only in the
    /// state**, which is `ctx.run_ui`'s own trap and cost this test its first
    /// version: `frame` reads the subject *before* running the UI, so the frame the
    /// click lands on still draws the value that was there when it started. Read
    /// there, the two colours came back equal — `#212C46` twice — and the toggle
    /// looked broken while the document had already changed. The ground is therefore
    /// sampled on the frame *after* each click, and both samples have the pointer
    /// resting on the button so the hover treatment is not the difference.
    ///
    /// ⚠️ Flip-checked by passing `FieldButton::Off` unconditionally: fails on the
    /// two grounds being equal, which is where it was predicted. Flipped a second
    /// way — writing `d.skip_ink = false` instead of negating it — leaves the first
    /// click green, because the click under test is the one that turns it off; it is
    /// the second click below that catches it, and that is the whole reason the
    /// second click is here.
    ///
    /// ⚠️ **The box, not the glyph.** Flipped by leaving the dropdown at the full
    /// `MENU_INNER` — forgetting to make room, the likeliest slip in a row that used
    /// to hold one control — the button lands at `255..281` against an inner edge of
    /// **248**, and the assertion bites. The glyph's own ink would have caught *that*
    /// one too, since 33px of overflow is more than a 26pt box has slack in it; what
    /// the box buys is the six-odd points either side of a 14pt picture, which is an
    /// overflow the ink reading cannot see and the eye can. Worth saying because
    /// `nothing_in_the_popup_paints_past_the_card` — the whole-card version of this
    /// check — builds its rows by hand and never sees this one at all.
    #[test]
    fn the_skip_ink_toggle_reports_its_state_and_writes_it() {
        let (ctx, mut app, id) = app_with(CharAttr::Underline(Some(Decoration::default())));

        // Warm-up: egui resolves a press against the previous frame's rects.
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

        // And back, which is what says this is a toggle rather than a switch that
        // only goes one way — and is the assertion a `= false` write fails.
        let _ = frame(&ctx, &mut app, id, click(at));
        assert!(
            underline_of(&app, id).skip_ink,
            "clicking it again turns skipping back on"
        );
        // Nothing else about the decoration moved: the toggle writes one field.
        let d = underline_of(&app, id);
        assert_eq!(
            (d.thickness, d.offset, d.style, d.color),
            (None, None, LineStyle::Solid, None),
            "the toggle carried something it should not have"
        );
    }

    /// **A strikethrough gets no toggle at all**, because `text::skip_ink` never
    /// looks at its flag: css-text-decor-4 skips underlines and overlines and never
    /// a `line-through`.
    ///
    /// The fixture check is the load-bearing half — an absent glyph is also what a
    /// section that bailed out before the line-style row looks like, so the wavy
    /// prefix beside the dropdown has to be *present* for the absence to mean
    /// anything.
    ///
    /// ⚠️ Flip-checked by dropping the `under.is_some()` guard: fails on the toggle
    /// being present, as predicted. The dropdown's own width is not asserted here —
    /// `nothing_in_the_popup_paints_past_the_card` is where a row that overflowed
    /// the card would be caught, and it would be caught for either kind.
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
        // `starts_with`, because the dropdown lays its prefix and its word out as one
        // galley (`ui::glyph_and_text`) — which is worth knowing here: the toggle
        // beside it is its own galley, so a whole-string match would have made the
        // *absence* below unfalsifiable for the wrong reason.
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
    //! **Re-picking the family you are already in is a no-op, not a reset**
    //! (§15 D467) — `[S6.2-L1-02]`.
    //!
    //! ⚠️ **Asserted on the predicate rather than by clicking the row**, and the
    //! reason is worth stating: the family list lives inside a `ComboBox` popup
    //! drawn by `show_rows`, so reaching a row means opening the popup and hunting
    //! a coordinate inside a scroll area — which
    //! `inspector::mixed_opacity_tests` shows is a hazard in its own right, the
    //! first aim there landing on a destructive neighbour. The decision was
    //! extracted into `family_actually_changed` so it could be asserted directly;
    //! its one call site is a single line and was read.
    //!
    //! ⚠️ **The `mixed` arm is the case a bare `!=` gets wrong**, and it is why
    //! the predicate takes three arguments. `current` over a disagreeing selection
    //! is one member's family, so re-picking *that* family is the real edit that
    //! makes the selection agree.

    use super::family_actually_changed;

    /// ⚠️ **Flipped** by replacing the body with `chosen`: the first assertion
    /// fails, `Some("Inter")` against `None`. Flipped the other way — `chosen
    /// .filter(|f| f != current)`, the obvious spelling and the one that drops the
    /// `mixed` term — the first three stay green and the fourth fails, which is
    /// what says that term is carrying its own case rather than decorating.
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

/// The variable-axis **number field**, driven through real pointer events
/// (`[S6.2-L1-04]`, §15 D569).
///
/// **Events, not calls**, for `char_valve_tests`' reason and one of its own: the
/// defect is entirely about *how many* commits a gesture makes, which is invisible
/// from any single call. Reading `undo_depth` after a frame is the only witness.
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

    /// A selected text node whose `wght` coordinate is already `403` — off the
    /// default, so the field has somewhere to scrub *from* and the `clear_at_default`
    /// arm is not in play.
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

    /// The node's stored `wght`, or `None` when there is no coordinate at all —
    /// which is a different document from one holding the font's own value, and is
    /// what the Auto/reset assertions are about.
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

    /// One frame with the axis field alone in a `Ui`, returning the rect it drew
    /// in — found by drawing rather than guessed, `char_valve_tests`' rule.
    fn frame(
        ctx: &egui::Context,
        app: &mut OndinApp,
        id: NodeId,
        resettable: bool,
        events: Vec<egui::Event>,
    ) -> egui::Rect {
        let subject = TypeSubject::of(app, id).expect("a subject");
        // Read the way the panel reads it — through `shown`, so during a drag it is
        // the *preview*'s coordinate and not the last committed one. A test that
        // took `style.variations` straight off the document would hand the field a
        // stale `current` on every frame of the scrub and prove nothing about the
        // valve.
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

    /// **A prefix-strip scrub is one undo step, not one per frame** — the finding's
    /// own measurement, which read `undo_depth 0 → 1 → 2 → 3 → 4 → 5` over five
    /// frames of pointer motion while the coordinate walked `403 → 406 → 409 → 412
    /// → 415`.
    ///
    /// §9.3's *"one gesture = one transaction = one undo step"*, and §15 D108's bug
    /// — *"one history entry per frame of a drag"* — which was fixed for the
    /// paragraph fields and never reached this one.
    ///
    /// **The in-run control is the coordinate moving at all.** Without it a field
    /// that had stopped responding to the drag entirely would pass the undo-depth
    /// assertion perfectly.
    ///
    /// **Flip run**, `axis_field`'s `axis_valve` call put back to the immediate
    /// `self.write_axis(subject, axis, coords, current, value, resettable)`: fails
    /// on *"a scrub is one undo step"* — see the recorded numbers in the assertion.
    #[test]
    fn a_prefix_strip_scrub_on_an_axis_field_is_one_undo_step() {
        let (ctx, mut app, id, _) = app_with_wght(403.0);
        // Two settling frames: a widget's interaction state is last frame's, so the
        // first could not report a press it had not yet been drawn for.
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

    /// **Typing the font's own value into the field drops the coordinate, exactly
    /// as the reset slot does** — the half of `clear_at_default` the valve had to
    /// be given, and the one that is *not* reachable from the reset button.
    ///
    /// ⚠️ **This is what said the fix was incomplete.** Routing the field through
    /// `axis_valve` moved the write off `write_axis`, and `axis_valve` pushed a
    /// coordinate unconditionally — so a field that had always dropped `wght=400`
    /// would have started storing it, silently, in the save format. The finding
    /// warned about it and the warning is the reason this test exists.
    ///
    /// **The control is the same run at 500**, which must keep its coordinate: an
    /// `axis_valve` that dropped *everything* would pass the assertion above.
    ///
    /// **Flip run**, `axis_valve`'s `clear_at_default` guard removed so it always
    /// pushes: fails on *"typing the font's own value means no coordinate"* with
    /// `Some(400.0)` against `None`, the control at 500 staying green.
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
            // Focus the field the way a hand does: click, release, then type over
            // the selected digits and click away to confirm.
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

    /// **The reset slot still writes immediately, and still *drops* the
    /// coordinate.**
    ///
    /// The two halves the fix had to keep. A click has no drag for a falling edge
    /// to wait on, so routing the reset through the valve would hold the edit until
    /// some later frame the field happened to be engaged on; and `write_axis`'s
    /// `clear_at_default` is what makes "back to the font's default" mean *no
    /// coordinate* rather than a coordinate that says the same thing — which is a
    /// different file, and for `opsz` a different mode.
    ///
    /// ⚠️ **`Some(400.0)` is the failure to watch for here, not `None` vs some other
    /// number**: it is what a valve without `clear_at_default` writes, and it looks
    /// right everywhere except on disk.
    ///
    /// **Flip run**, the reset arm's `return` removed so the valve runs too: green.
    /// A *finding*, not a failure — the frame the reset lands on is not an engaged
    /// one, so `char_valve`'s falling edge never fires and the immediate write is
    /// the only one either way. The `return` is insurance against a reset clicked
    /// *during* a scrub, which is reachable and which this test does not stage.
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

        // The reset slot sits to the right of the 120pt field, inside the row's
        // rect — found by walking the row rather than guessed at.
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

    /// **An axis field over a selection whose runs disagree shows a dash**
    /// (§15 D636, `[S6.2-L1-05]`).
    ///
    /// `subject.mixed` was asked **zero** times across the whole of
    /// `axes_section` and `features_section`; `ragged` was asked once and its
    /// answer spent on the reset button one line above the readout. So the field
    /// drew the *first run's* number with no mark, which is `mixed_text`'s own
    /// objection — *"showing one of the several values reads as a claim that it is
    /// the value"* — while `mixed_text`'s four other callers, about ten fields in
    /// this same file, already used it. (⚠️ That dash is itself a departure from
    /// §15 D130, which is recorded at `axis_field` and is a panel-wide ruling
    /// rather than this field's.)
    ///
    /// 🚨 **`TypeSubject::partial`'s own doc is wrong, and it nearly cost this
    /// test its fixture.** The field says *"whether `range` is a sub-range rather
    /// than the whole node. **The only case in which a character control can read
    /// mixed.**"* — so a select-all reads as `partial == false`, which is what the
    /// finding's *select all* fixture was written against and what made me
    /// predict the control below would be a whole-node selection. `read` sets it
    /// from `!sel.is_empty()`: **any** non-empty selection is partial, a
    /// select-all included, and only the *bare caret* and the no-session case are
    /// not. The finding was right and the doc was not; the assertion that caught
    /// it is `assert!(one_run.partial)` on the fixture rather than anything about
    /// the claim. Corrected at the field.
    ///
    /// ⚠️ Flip-checked by dropping `mixed_text` from the formatter: red at the
    /// dash. The agreeing half is the control and stays green under it, which is
    /// what says the dash comes from `mixed` rather than from the field generally.
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

        // A sub-range spanning the word boundary: bytes 3..8 hold both weights.
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

        // The control: a selection *inside* one run, where there is nothing to
        // disagree about. Without it "shows a dash" would be satisfied by a field
        // that dashes whenever anything is selected.
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
    //! **A bare caret's character fields describe the run typing will inherit**
    //! (§15 D589, `[S6.2-L1-06]`).
    //!
    //! `TypeSubject::read` fell back to `0..len` for a caret with no selection —
    //! which is right, and is what §9.4 and §15 D164 settle about the *write* —
    //! and `shown` then resolved at `range.start`, i.e. **byte 0**, whatever the
    //! caret was doing. `mixed` is structurally `false` over a whole-node range,
    //! so nothing on the row said otherwise. The Size field read 40 with the caret
    //! in 16pt text and the next keystroke came out 16.
    //!
    //! ⚠️ **The write is untouched.** A bare-caret character write still sets the
    //! node's default and drops that attribute's overrides; only what the field
    //! *shows* has moved.
    //!
    //! (Plain backticks per §15 D319 — `cargo doc` builds without the `test` cfg.)

    use super::*;
    use crate::app::OndinApp;
    use crate::theme;
    use ondin_core::{Document, NodeId, Operation, Transaction};

    /// `"hello world"` with `Size(40)` over its first five bytes and a node
    /// default of 16 — the fixture the finding measured.
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

    /// **The field shows what the next keystroke will produce.**
    ///
    /// The three carets are the three cases the resolve point has: inside the
    /// styled run, at the boundary — where `Spans::edited` joins the run that
    /// *ends* at the caret and never the one that begins there — and past it.
    ///
    /// ⚠️ **Byte 0 is its own case and is here as more than a bounds check.**
    /// There is no run to the caret's left, and an insertion at 0 takes the node's
    /// defaults: `map_start` shifts a span starting at 0 to the right rather than
    /// growing it. So the honest answer at byte 0 is 16, the default, even though
    /// byte 0 itself carries the 40pt span — which is the one caret where "the
    /// value at the caret" and "the value typing inherits" disagree in the other
    /// direction, and `resolve_at`'s `None` is what states it.
    ///
    /// **Flip run**, `resolve_at` put back to `Some(self.range.start)` for every
    /// case: fails on the first assertion, *"the caret is in 16pt text"*, at 40
    /// against 16 — which is the finding's own measurement.
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

    /// **The control: a selection still reads its own start, and a node with no
    /// session still reads byte 0.**
    ///
    /// Both were already right and both go through the changed line, so this is
    /// where a fix that moved more than it meant to would show. The selection case
    /// is the one that matters: `range.start` is the resolve point there *and* the
    /// first byte of the write, and nothing about the caret's left-hand rule
    /// applies to it.
    #[test]
    fn a_selection_and_an_unedited_node_are_unmoved() {
        let (_ctx, mut app, id) = app_with_ragged_text();

        // No session: the whole node, resolved at byte 0, which is the 40pt run.
        let subject = TypeSubject::of(&app, id).expect("a subject");
        assert_eq!(subject.resolve_at, Some(0));
        assert!(matches!(
            subject.shown(CharAttrKind::Size),
            CharAttr::Size(v) if v == 40.0
        ));

        // A selection over bytes 6..11, all of it 16pt, so `shared` answers and the
        // resolve point is not even consulted — and one over 3..8, which is ragged
        // and does consult it.
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
    //! **Two controls in the Character tab that told the user something the
    //! panel's own tables contradict** — §15 D691 (`[S6.3-L1-07]`) and D704
    //! (`[S6.3-L3-08]`).
    //!
    //! They share a fixture rather than a subject: both need a headless app
    //! holding one text node and a hand-built `available`, and duplicating
    //! thirty lines of that to keep two names apart is the shape this codebase
    //! keeps filing against itself.
    //!
    //! ## D691 — the *Show all features* offer counts the rows the click adds
    //!
    //! Two filters decide what the feature list contains — the curation
    //! (`shown`) and the search box (`matches`) — and the count behind the
    //! reveal applied only the first, because it was taken before the search
    //! field had been drawn. So with a needle typed, the collapsed reveal
    //! offered every withheld feature on the face and a click added none of
    //! them: the exact "control that cannot change anything" state
    //! `features_reveal` was written to suppress, reached by the one door it
    //! does not ask about.
    //!
    //! ⚠️ **`features_section` is driven directly with a hand-built
    //! `&[FaceFeature]`.** Its real caller reads `text::family_features`, which
    //! is empty headlessly — no fonts are registered — so it returns at the
    //! first line and the section never draws. The finding said as much and
    //! this is the probe it asked for.
    //!
    //! ## D704 — the Language field reads its name, not its tag
    //!
    //! `LANGUAGES` is a `(tag, name)` table and the closed control used the
    //! stored locale verbatim, so the two states of one combo disagreed:
    //! *German* in the list, `de` in the field. That control needs no
    //! `available`, only the app and the subject, which is the half of the
    //! fixture it shares.
    //!
    //! (Plain backticks per §15 D319 — `cargo doc` builds without the `test` cfg.)

    use super::*;
    use crate::app::OndinApp;
    use crate::theme;
    use ondin_core::{CharSpans, Document, NodeId, Operation, ParaSpans, Transaction};

    const AREA: egui::Rect = egui::Rect {
        min: egui::pos2(0.0, 0.0),
        max: egui::pos2(400.0, 600.0),
    };

    /// A face offering `liga`, plus 43 withheld private features — the shape the
    /// finding measured, whose reveal read "Show all features (43 more)".
    ///
    /// ⚠️ **`Zk01`..`Zk43` and not `cv01`..`cv43`**, which the first draft of
    /// this fixture used and which is wrong in the way that matters:
    /// `feature_label` routes a character variant through `font_defined`, so
    /// every `cvNN` is **offered**. A face of 43 of them has nothing withheld,
    /// `features_reveal(false, 0)` is `None`, and the probe reads exactly like
    /// the fix working. A private four-letter tag is tier three —
    /// `a_mechanic_is_withheld_a_stylistic_set_is_named_and_a_private_tag_is_neither`
    /// pins that — and is the only shape here that is really withheld.
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
    fn app_with_text() -> (egui::Context, OndinApp, NodeId) {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        doc.apply(&Transaction(vec![Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind: ondin_core::NodeKind::Text {
                content: "gypsy".into(),
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
                screen_rect: Some(AREA),
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

    /// **With a needle that only an *offered* feature matches, the reveal is
    /// absent** — because clicking it would add nothing.
    ///
    /// ⚠️ **The first assertion is the control and the fixture check in one.**
    /// It says the section really does draw a reveal, and really does see 43
    /// withheld features, before the interesting assertion claims it stops. A
    /// green second assertion on its own is equally consistent with
    /// `features_section` having returned at its `available.is_empty()` line
    /// and drawn nothing whatever — which is exactly how this probe fails if
    /// the fixture is wrong, so it has to be ruled out rather than assumed.
    ///
    /// ⚠️ **Flip:** counting `hidden` over `!shown(t, l)` alone — the shipped
    /// spelling — fails the second assertion with
    /// `left: Some("Show all features (43 more)"), right: None`, which is the
    /// reported symptom verbatim.
    #[test]
    fn a_needle_that_hides_every_withheld_feature_hides_the_offer() {
        let (ctx, mut app, id) = app_with_text();
        let available = a_face_with_43_withheld();

        assert_eq!(
            reveal(&ctx, &mut app, id, &available).as_deref(),
            Some("Show all features (43 more)"),
            "the fixture must reach the state: 43 withheld features and an offer to show them"
        );

        // `liga` is offered, so it is on screen already and none of the 43
        // survives the needle.
        app.feature_filter = "liga".into();
        assert_eq!(
            reveal(&ctx, &mut app, id, &available),
            None,
            "with a needle no withheld feature matches, the offer would add nothing and is absent"
        );
    }

    /// **The Language control reads the same closed as open** (§15 D704,
    /// `[S6.3-L3-08]`).
    ///
    /// The closed field used the stored locale verbatim, so picking *German*
    /// from a list that says "German" left a field reading `de`, and *Chinese
    /// (Simplified)* left `zh-Hans`. `LANGUAGES`' second column exists for
    /// exactly this and was used on one side of the control only.
    ///
    /// ⚠️ **The third case is the one a lookup gets wrong**: a locale the table
    /// does not offer is a real setting, and must fall back to the tag rather
    /// than to *None*. A `.find(…).map(…).unwrap_or("None")` passes the first
    /// two assertions and fails this one, which is why it is here.
    ///
    /// ⚠️ **Flip:** restoring `current.clone().unwrap_or_else(…)` fails the
    /// second assertion — the field reads `de` where "German" is wanted — and
    /// leaves the first and third green, `None` and an unlisted tag being the
    /// two inputs the old spelling happened to get right.
    #[test]
    fn the_language_field_reads_its_name_rather_than_its_tag() {
        let (ctx, mut app, id) = app_with_text();

        let field = |app: &mut OndinApp, locale: Option<&str>| -> Vec<String> {
            app.apply_char_attrs(
                &TypeSubject::of(app, id).expect("a subject"),
                vec![CharAttr::Locale(locale.map(str::to_string))],
            );
            let subject = TypeSubject::of(app, id).expect("a subject");
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(AREA),
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

    /// **And a needle that *does* match withheld rows still offers them, at the
    /// number it would really add.** The other side of the same rule: the
    /// repair must narrow the count, not suppress the control.
    ///
    /// ⚠️ **Flip:** the shipped `!shown(t, l)`-only count fails here with
    /// `left: Some("Show all features (43 more)")` against a right of
    /// `(4 more)` — which is the same defect as the test above reported as a
    /// *wrong number* rather than as a dead control, and is why both are
    /// asserted. A repair that simply hid the reveal whenever a needle was
    /// typed would pass that one and fail this.
    #[test]
    fn a_needle_that_matches_withheld_features_offers_exactly_those() {
        let (ctx, mut app, id) = app_with_text();
        let available = a_face_with_43_withheld();

        // `zk4` matches `Zk40`..`Zk43` by name and nothing else.
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
    //! **The OpenType section over a selection whose runs disagree** — §15 D752,
    //! `[S6.2-L1-05]`, both halves, on the maintainer's rulings.
    //!
    //! 🚨 **The section had the answer and spent it on the reset button.**
    //! `subject.mixed(CharAttrKind::Features)` was called **zero** times across
    //! the whole of `axes_section`, `features_section` and their rows, while
    //! `subject.ragged` was called exactly once — one line above the readout, to
    //! decide whether a reset was lit. So a selection half of which had `tnum` on
    //! drew a column of dead switches, and toggling any one of them wrote the
    //! *first run's whole list* over everything and dropped the rest.
    //!
    //! **The readout is the word "Mixed"**, which is §15 D130's rule rather than a
    //! new invention — *"a control says Mixed; if it is too small to hold five
    //! letters it says a dash, and there are exactly two"* — and the maintainer
    //! ruled out the alternative independently: *"A three way switch would be
    //! visually weird (thumb in the middle?)"*. **The write is per tag**: *"Write
    //! only the tag you named and leave each run's other features alone."*
    //!
    //! ⚠️ **Per *tag*, not per attribute.** `mixed(Features)` is true the moment
    //! any one tag disagrees anywhere, so a readout built on it would mark forty
    //! rows because one of them differed. `TypeSubject::feature_mixed` asks about
    //! one tag through `CharSpans::values_in`.
    //!
    //! (Plain backticks per §15 D319 — `cargo doc` builds without the `test` cfg.)

    use super::*;
    use crate::app::OndinApp;
    use crate::theme;
    use ondin_core::{Document, NodeId, Operation, Transaction};

    fn tag(s: &str) -> Tag {
        Tag::parse(s).expect("a four-byte tag")
    }

    /// `"alpha bravo"`, with `liga 1` over the whole node, `tnum 1` added over
    /// bytes 6..11 — so the two runs agree about `liga` and disagree about
    /// `tnum`. The finding's fixture, one attribute simpler.
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

    /// Open a session and select everything, which is what the finding does.
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

    /// **A tag the runs disagree about is mixed; one they agree about is not.**
    ///
    /// ⚠️ **The `liga` half is the control and it is load-bearing.** Without it
    /// this passes against a `feature_mixed` that answered `subject.mixed(Features)`
    /// — true for every tag the moment one of them disagrees — which is the
    /// obvious wrong implementation and the one a reader would tidy this into.
    ///
    /// ⚠️ **Flip, run:** returning `self.mixed(CharAttrKind::Features)` from
    /// `feature_mixed` fails at the `liga` assertion and leaves the `tnum` one
    /// green, which is the whole distinction in one line.
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

    /// **Toggling one tag leaves every other tag in every run alone.**
    ///
    /// 🚨 **This is the failure the maintainer ruled on.** The old write cloned
    /// the list `attr!` resolves at `range.start` — the first run's, which has no
    /// `tnum` — edited `liga` into it and applied it to the whole selection. The
    /// second run's `tnum` went with it, silently, one undo step.
    ///
    /// ⚠️ **The assertion order puts the loss first.** `tnum` surviving is what
    /// the user loses; `liga` having actually changed is the control that says the
    /// click did anything at all. A test asserting only the second passes against
    /// the flattening version.
    ///
    /// ⚠️ **Flip, run — the site was right and the *value* is worse than
    /// predicted.** Replacing `write_feature_tag`'s partial arm with the old
    /// `apply_char_attrs(subject, vec![CharAttr::Features(merge(&set))])` fails at
    /// the `tnum` assertion, and the reading is `left: [0]` where `[0, 0]` was
    /// expected: flattening does not merely set both runs to the first one's list,
    /// it **collapses the two runs into one**, because identical neighbouring
    /// spans have no boundary left to keep them apart. So the shape the user loses
    /// is not just a value — it is the fact that the second word was ever
    /// different, and no undo-free edit can put that back.
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

        // Turn `liga` off across the whole selection, then land the session on the
        // document — the spans live in the editor until it closes, and `per_run`
        // reads the node.
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
    //! **`char_valve` has two arms, not three** (§15 D812), and **a held
    //! tracking chord stops where its field stops** (§15 D817).
    use super::*;
    use crate::app::OndinApp;
    use crate::theme;
    use ondin_core::{CharSpans, Document, NodeId, Operation, ParaSpans, Transaction};

    const AREA: egui::Rect = egui::Rect {
        min: egui::pos2(0.0, 0.0),
        max: egui::pos2(400.0, 600.0),
    };

    /// A headless app holding one plain text node at 20pt, and its id.
    fn app_with_text() -> (egui::Context, OndinApp, NodeId) {
        let ctx = egui::Context::default();
        theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let id = ids.mint();
        let style = TextStyle {
            font_family: "Inter".into(),
            font_size: 20.0,
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
        .expect("build the fixture");
        app.session.adopt_document(doc, None);
        app.session.selection.set_one(id);
        (ctx, app, id)
    }

    /// **A control reporting `changed()` while it holds no focus and is not
    /// dragged commits nothing** — `char_valve` has two arms, not three
    /// (§15 D812).
    ///
    /// 🚨 **This is `[S6.2-L1-01]` reproduced at the valve.** The deleted arm was
    /// `else if resp.changed() && !resp.lost_focus()`, kept for *"a control with no
    /// engagement to latch"*. §15 D802 found the controls it was kept for never
    /// reach this valve; §15 D803 found the one thing that does, and it is a
    /// hazard — egui's per-frame clamp of a stored value outside a **ranged**
    /// field reports an untouched control as changed, so the arm committed an edit
    /// nobody made and spent an undo step on it.
    ///
    /// ⚠️ **The fixture is a ranged `DragValue` *without* the app's opt-out**, and
    /// that is the whole of what makes this reproducible. `ui::value_field_f64`
    /// and `ui::bare_drag_value` both pass `clamp_existing_to_range(false)`
    /// (§15 D425, D552), so no production control can get into this state today —
    /// which is exactly why the arm's removal is a *second* defence rather than
    /// tidying, and why the condition has to be built here by hand. A test that
    /// drove a real field would assert nothing.
    ///
    /// ⚠️ **The response is the assertion's subject and the value is not.** What
    /// is being pinned is that a frame reporting `changed()` with no engagement
    /// does not reach a commit; whether egui chose to clamp on this particular
    /// frame is egui's business, so the fixture asserts it reached the state
    /// (`changed()` true, `has_focus()` false, `dragged()` false) before asserting
    /// what the valve did about it.
    ///
    /// **Flip-check, run** by restoring the third arm: fails at *"no undo step"*
    /// with **1 against 0**, the predicted site. ⚠️ The size assertion under it
    /// was predicted to fail as well and never runs, the first `assert_eq!`
    /// ending the test — *a prediction about a second assertion is a prediction
    /// about an assertion that may not execute.*
    #[test]
    fn a_changed_frame_with_no_engagement_commits_nothing() {
        let (ctx, mut app, id) = app_with_text();
        let before = app.session.history.undo_depth();

        // 20 is the fixture's font size; 5000 is outside the field's range, which
        // is what makes egui rewrite it and call that a change.
        //
        // ⚠️ **The first draft used 999 and `MAX_FONT_SIZE` is 1000**, so the value
        // was *in* range, nothing was rewritten, and the fixture assertion below
        // failed with `changed=false` on every frame. That assertion is the only
        // reason this test is not silently about nothing — which is exactly what
        // it is there for.
        let mut out_of_range = 5000.0_f64;
        // ⚠️ **Every frame, not the last one.** The rewrite happens on the frame
        // egui first meets the stored value; by the frame after it the value is
        // already in range and reports nothing. Reading only the final frame
        // measured `changed=false` and the fixture never reached the state it
        // names — which is the *"assert the fixture is in the state you think"*
        // rule biting on its own first draft.
        let mut states = Vec::new();
        for _ in 0..3 {
            let subject = TypeSubject::of(&app, id).expect("a subject");
            let _ = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(AREA),
                    ..Default::default()
                },
                |ui| {
                    // No `clamp_existing_to_range(false)`: egui's default, which is
                    // what every field in the app opts out of.
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

    /// **A held `Alt`+`→` stops at `MAX_TRACKING_PCT`, and `Alt`+`←` at the
    /// min** — the tracking chord stops where its field stops (§15 D817).
    ///
    /// `TextStyle::set` canonicalizes `letter_spacing` without bounding it, so
    /// before this the chord walked past the cap and out of what the field can be
    /// scrubbed to, where the `Size` arm in the same `match` stops at its ends.
    /// Nothing was *lost* — the field shows an out-of-range stored value rather
    /// than rewriting it (§15 **D425**; D475 is this panel's test of that, and
    /// citing it for the behaviour is the substitution `CLAUDE.md` records at
    /// seven sites) — so what this fixes is the two chords disagreeing about
    /// whether a held key has an end.
    ///
    /// ⚠️ **The bound is at the arm and not at `TextStyle::set`**, which is where
    /// `Size`'s is, and that asymmetry is the decision rather than an oversight:
    /// these caps are on the *controls*, a file may legally hold any finite
    /// value, and a bound in the model would make D425's behaviour unreachable
    /// for this attribute.
    ///
    /// ⚠️ **200 presses, not 2.** The step is `TRACKING_STEP_EM`, so a test that
    /// pressed twice would be green against no clamp at all — it has to actually
    /// reach the cap, which is what the *"the fixture must reach the state"*
    /// assertion below demands before the interesting one runs.
    ///
    /// **Flip-check, run** by removing the `clamp` from the `Em` arm: fails at
    /// *"the em face stops at the same percentage"* with **`Em(4.0)`** against
    /// `Em(2.0)` — 400%, twice the field's own maximum. ⚠️ The predicted site was
    /// the *first* cap assertion, and that one is the **px** arm, which the flip
    /// does not touch: the two arms are two clamps, and a flip of one cannot be
    /// caught by an assertion about the other. *The stored default being `Px` is
    /// why the em face needs a fixture of its own at all.*
    #[test]
    fn a_held_tracking_chord_stops_at_the_fields_cap() {
        let (_ctx, mut app, id) = app_with_text();
        app.begin_edit_text(Some(id), None);
        assert!(
            app.text.as_ref().map(|s| s.id) == Some(id),
            "the fixture must reach the state: `text_chord` returns early with no \
             live session, so without this the test would be about nothing"
        );

        // ⚠️ **Read through `TypeSubject`, not off the node.** With a live session
        // the write can land in the editor rather than in the document
        // (`apply_char_attrs`'s `partial` arm), so a probe reading
        // `style.letter_spacing` measured `Px(0.0)` after four hundred presses and
        // would have been green against a chord that did nothing. The subject is
        // what the chord itself reads and what the field shows, which makes it the
        // honest place to ask.
        let tracking = |app: &OndinApp| {
            let subject = TypeSubject::of(app, id).expect("a subject");
            match subject.shown(CharAttrKind::LetterSpacing) {
                CharAttr::LetterSpacing(l) => l,
                other => panic!("the letter-spacing slot answered {other:?}"),
            }
        };

        // ⚠️ **The stored default is `Px(0.0)`, not `Em`**, which the first draft
        // of this test assumed and which is why the px face is asserted first. The
        // px cap is derived from the `%` one at the current font size through the
        // same `px_range_for` the field uses — at the fixture's 20pt that is
        // `-10..=40`.
        let px_cap = px_range_for(MIN_TRACKING_PCT..=MAX_TRACKING_PCT, 20.0);
        assert!(
            matches!(tracking(&app), Length::Px(v) if v == 0.0),
            "the fixture must reach the state: tracking starts at Px(0), not {:?}",
            tracking(&app)
        );

        // Far past the cap, in both directions, and the count is the point: the
        // step is small, so a test that pressed twice would be green against no
        // clamp at all.
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

        // **The em face, which is the other arm of the clamp.** A unit click puts
        // the same quantity in `%`, and the cap there is the constant itself
        // rather than a derived number.
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
        // ⚠️ **The em *floor*, which nothing asserted** (§15 D840). Two arms with
        // two ends each is four clamps and this test pinned one of them twice
        // and another once; deleting half of D817's em clamp left the suite
        // green. The floor is not the negation of the cap — `MIN_TRACKING_PCT`
        // is −50 against a maximum of 200 — so an assertion that assumed
        // symmetry would pass against the wrong number.
        for _ in 0..800 {
            app.text_chord(TextChord::Tracking(-1));
        }
        assert_eq!(
            tracking(&app),
            Length::Em(MIN_TRACKING_PCT / 100.0),
            "and at the em floor, which is not the negation of the em cap"
        );

        // Control: one press from zero is an ordinary step and is not clamped to
        // anything. Without this the test would pass against a chord that did
        // nothing at all.
        let (_ctx, mut app, id) = app_with_text();
        app.begin_edit_text(Some(id), None);
        app.text_chord(TextChord::Tracking(1));
        assert_eq!(
            tracking(&app),
            Length::Px(TRACKING_STEP_PX).canonical(),
            "control: an ordinary press still steps by one step"
        );
    }

    /// **A held leading chord stops where the field stops** (§15 D840).
    ///
    /// 🚨 **The identical defect §15 D817 fixed for tracking, three arms up in
    /// the same `match`, and it went unread beside it.** The `Leading` arm ended
    /// in `step_length` with no range expression at all, so a held `Alt`+`↓`
    /// walked line height **negative** — out of the field's `0..=1000%` and into
    /// `parley_line_height` — and `Length::canonical` normalises `-0.0` without
    /// bounding anything.
    ///
    /// ⚠️ **The floor is the interesting end.** The cap is 1000%, which takes a
    /// great many presses to reach and is not where the damage was; zero is one
    /// press away from the seeded value and is what a user holding the key
    /// actually hits.
    ///
    /// Flip: [`stepped_into`] replaced by the bare `step_length` this arm used to
    /// end in. Red at the floor, the predicted site, with a negative line height.
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

        // Control: one press up from there is an ordinary step, so the floor is
        // a clamp rather than the chord having stopped working.
        let before = leading(&app);
        app.text_chord(TextChord::Leading(1));
        assert_ne!(
            leading(&app),
            before,
            "control: the chord still moves away from the floor"
        );
    }

    /// 🚨 **The increase key decreased tracking by 300 percentage points**
    /// (§15 D840).
    ///
    /// §15 D817's clamp was unconditional, and §15 D425 says a control **shows**
    /// a stored out-of-range value rather than rewriting it — which is the whole
    /// reason these caps sit on the chord and not in `TextStyle::set`. So with a
    /// stored `Em(5.0)` (500%, legal, and a file may hold one) a single
    /// `Alt`+`→` stepped to `5.0 + step` and clamped to `MAX_TRACKING_PCT`: the
    /// user asked for *more* and lost 300 percentage points in one press, with
    /// no way back to the value they had.
    ///
    /// **Two flips, both run, and both land on the *second* assertion** — which
    /// is not where this doc predicted either of them. The shipped
    /// unconditional clamp gives `Em(2.0)` against `Em(5.0)`, the 300-point loss
    /// itself; the obvious repair, skipping the clamp whenever the value starts
    /// out of range, gives `Em(5.01)` — it lets the *away* direction run free,
    /// so `Alt`+`→` on a stored 500% walks to 600% and beyond, a control with no
    /// cap at all for exactly the documents that most need one. ⚠️ **The
    /// prediction here was that the second repair would survive to the third
    /// assertion**, and it does not: *"nothing moves"* is strict enough to catch
    /// both wrong versions, at values 3.0 apart. The third assertion is
    /// therefore not the discriminator it was written as — it covers the
    /// return-to-range behaviour, which neither flip reaches.
    ///
    /// ⚠️ **The fixture writes through `apply_char_attrs`, not the chord**,
    /// because the chord is the thing under test: seeding with it would clamp on
    /// the way in and the fixture would never reach the state this is about.
    /// Asserted, for that reason.
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

        // The third behaviour: once inside, the ordinary cap applies again.
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
}

#[cfg(test)]
mod wrap_gate_tests {
    //! **Word break and Long words are dead under `NoWrap`** (§15 D804).
    use super::*;
    use crate::app::OndinApp;
    use crate::theme;
    use ondin_core::{Document, IdSource, NodeId, Operation, Transaction};

    const AREA: egui::Rect = egui::Rect {
        min: egui::pos2(0.0, 0.0),
        max: egui::pos2(400.0, 600.0),
    };

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
            screen_rect: Some(AREA),
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

    /// Where the three strips sit with the section drawn at `MENU_INNER` in a
    /// 400×600 area, measured by a 4pt sweep over x ∈ {40, 120, 200, 280},
    /// y ∈ [0, 260]. Each is a cell that is *not* the current one, so a click
    /// there has something to do.
    ///
    /// ⚠️ **The Wrap strip reads off-then-on, so its left cell is `NoWrap`** —
    /// `WrapMode::ALL` puts *"the cell that does nothing"* first. Naming these the
    /// other way round is what the first draft did, and the failure was the
    /// honest one: a click on the cell already current changes nothing, and the
    /// "the mode switched" assertion caught it.
    const WRAP_OFF: egui::Pos2 = egui::pos2(120.0, 30.0);
    const WRAP_ON: egui::Pos2 = egui::pos2(200.0, 30.0);
    const WORD_BREAK_BREAK_ALL: egui::Pos2 = egui::pos2(120.0, 84.0);
    const LONG_WORDS_BREAK_WORD: egui::Pos2 = egui::pos2(120.0, 124.0);

    /// **`NoWrap` makes Word break and Long words dead, and nothing else**
    /// (§15 D804).
    ///
    /// The section's own doc states the rule and its boundary: both controls are
    /// *"rules about breaks that cannot happen"* under `NoWrap`, dimmed and **not
    /// reset**, while *"hard breaks still break … so the paragraph spacing and
    /// indent controls in the tab beside this one go on working and must not be
    /// caught by the same condition."* `roadmap.md` listed this among three
    /// decisions reachable by no test, needing *"a live text session and a
    /// laid-out panel"* — which is right, and is what this is.
    ///
    /// 🚨 **The control is the whole test.** Every assertion under `NoWrap` is
    /// that a click changed *nothing*, which is exactly what a section that was
    /// never drawn, or a coordinate that missed, also produces. So the same
    /// fixture clicks the Wrap strip itself — the one control the gate must
    /// **not** catch — and requires it to still work. Without that, this passes
    /// against an empty `ui`.
    ///
    /// ⚠️ **Dimmed and not reset is asserted too.** The stored `word_break`
    /// survives the trip: it is set under `Wrap`, the mode is switched to
    /// `NoWrap`, and the value is still there. A gate that cleared the fields
    /// instead of disabling them would satisfy every "the click did nothing"
    /// assertion and still lose the user's setting.
    ///
    /// ⚠️ **Flipped**: removing the `if !wraps { ui.disable(); }` makes the two
    /// `NoWrap` click assertions red — Word break first. Inverting it to
    /// `if wraps` instead makes the **`Wrap`** half red, at the first assertion,
    /// which is the half that proves the coordinates are live at all.
    #[test]
    fn nowrap_kills_word_break_and_long_words_and_nothing_else() {
        // The positive half: with wrapping on, both strips take a click.
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

        // **Dimmed, not reset**: switching to NoWrap keeps what was stored.
        click(&ctx, &mut app, id, WRAP_OFF);
        assert_eq!(para(&app, id).wrap, WrapMode::NoWrap, "the mode switched");
        assert_eq!(
            para(&app, id).word_break,
            WordBreak::BreakAll,
            "and the stored value survived the trip — dimmed is not reset"
        );

        // The negative half, on a fixture that starts in the mode.
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

        // 🚨 **The control.** The gate must stop at those two.
        click(&ctx, &mut app, id, WRAP_ON);
        assert_eq!(
            para(&app, id).wrap,
            WrapMode::Wrap,
            "the Wrap strip itself is outside the gate and still works — without \
             this assertion the two above pass against a section nobody drew"
        );
    }
}
