//! The inspector's **Export** panel (§7, §9.4) — the list of files a layer
//! produces, and the button that produces them.
//!
//! **The panel is the feature, and since 2026-08-22 it is the only door.**
//! *Export as…* on the context menu asked every question every time and forgot the
//! answers, which made producing the same asset set twice a chore rather than a
//! keystroke; here the answers live on the layer, are saved with the document and
//! are read by the CLI. The two ran side by side for two days as "repeatable" and
//! "this once, somewhere else" — then the second was removed, this panel's own
//! button having made it the slower route to the same file (§15 D264).
//!
//! ⚠️ **So there is nothing behind this.** A layer with no export spec shows a
//! collapsed card and a `+`, which is one step more than the removed row asked for,
//! and that cost was taken knowingly — but it means a bug in `run_export`, in
//! `plan`, or in seeding a first spec has no fallback path a user can reach.
//!
//! **Nothing here interprets a spec.** `ondin_export::plan` turns the saved
//! settings into filenames, pixel sizes and bytes; this module is the controls
//! that edit them and the dialogs that choose where they land. That division is
//! what lets `ondin export --all` produce the same files with no window.

use crate::app::OndinApp;
use crate::theme::{self, icon};
use crate::ui::{self, Prefix, Scrub, value_field};
use ondin_core::{
    ExportBackground, ExportFormat, ExportScale, ExportSpec, NodeId, Operation, Transaction,
};
use ondin_export::{PlanOptions, PlannedFile};

/// The row is the design's `74px 1fr 28px 28px` at a 7pt pitch, in a 256pt card.
///
/// **74 is the design's and it clears the measurement**, which the 62 this started
/// at did not: a `ComboBox` spends `icon_spacing + icon_width + 2·button_padding.x`
/// = **35pt** beside its text and lays that text out in `TextStyle::Button` rather
/// than in the 11.5 the row uses, so `0.75x` wants 67 and `1024h` wants 71. A combo
/// asked for less does not clip — egui takes `max(content, width − padding)`, so it
/// comes out **wider than asked** and squeezes whatever is beside it, which is why
/// `the_export_row_fits_the_card` is a test and not a comment.
///
/// The format cell is `1fr`, so it is computed from what is left rather than
/// declared here.
const SCALE_W: f32 = 74.0;
/// The height of everything on the row, and the side of its two square buttons.
const CELL: f32 = 28.0;
/// Between two controls side by side — the design's card gap, and the one the
/// settings popover matches across, exactly as the type popup matches the card it
/// hangs from (`typography::COL_GAP`).
const PITCH: f32 = 7.0;
/// Between a row and the size line under it.
///
/// **3, which is tighter than anything else in the card**, and that is the point:
/// the line is an annotation on the row above it rather than a row of its own, so
/// it has to sit closer to what it describes than that pair sits to the next one.
/// It came down 8 → 5 → 3 by being looked at, and the last step was the one that
/// settled which of the pair should move: [`ROW_UNIT_GAP`] reads as the ordinary
/// distance between two fields, so the caption is what gets compressed against its
/// row rather than the rows being pushed apart to make room for it.
const ROW_LINE_GAP: f32 = 3.0;
/// Between one export and the next.
///
/// **6, which is *below* the card's own row gap** — so the rows are laid out in a
/// region of their own with the pitch taken to zero, because a scope can add to a
/// spacing and not subtract from one.
///
/// This started at `CARD_ROW_GAP * 2` — `paint_unit`'s doubling, and the 18 two
/// stroke entries sit at — and came down twice on the machine, to 15 and then to
/// here. The doubling was the wrong rule to borrow rather than the wrong number: it
/// is sized for an entry of two *controls*, where an export is a control and the
/// caption belonging to it, so what has to be legible is not "these are two
/// entries" but "this caption goes with the row above it". That reading is carried
/// by [`ROW_LINE_GAP`] being the tighter of the two, which is a *relationship*
/// rather than a distance — and it survives at 3 against 6.
///
/// **This is the number that stopped moving**, which is why the last adjustment
/// was made to the other one: at 6 two export rows sit the ordinary field-to-field
/// distance apart, the same as any two stacked controls elsewhere, so the list
/// reads as a list and the caption is the thing that has to earn its place inside
/// it.
///
/// **It is the gap between two *units*, so a row with no caption does not use
/// it** — it spends [`ROW_LINE_GAP`] here as well and comes to the card's own
/// pitch. That is the multi-selection case, where there is no single size to
/// report and every row is one line tall; six between two bare controls reads as
/// crowding rather than as grouping, because there is nothing left to group. See
/// `export_row`'s tail.
const ROW_UNIT_GAP: f32 = 6.0;
/// The grouping the pair above is *for*: an export's two halves have to sit closer
/// to each other than one export sits to the next, or the size line reads as
/// belonging to the row below it. Checked at compile time, because it is a
/// relationship between two constants and there is nothing to run.
const _: () = assert!(ROW_LINE_GAP < ROW_UNIT_GAP);
/// The size line's ink — the design's 34%, quieter than `text::FAINT` (38%),
/// because it annotates a control rather than labelling one.
const SIZE_INK: egui::Color32 = theme::color::text_a(87);
/// The collapse key the Preview section shares with the panel list.
///
/// A `&'static str` because `collapsed_panels` is keyed by title, and the key is
/// *not* a panel title any more — the section lives inside the Export card. Named
/// so the two places that read it cannot drift apart on a spelling.
const PREVIEW: &str = "Preview";
/// The clear space round the previewed image inside its checkerboard.
const PREVIEW_INSET: f32 = 10.0;

// --- the settings popover ---------------------------------------------------
//
// The shared popover language: 272 wide, 11 of padding, 26pt controls, 9.5pt
// small-caps section labels. The same numbers `typography.rs` and
// `stroke_menu_popup` carry, which is three copies of one card — worth
// promoting into `ui.rs`, and not inside a change that is also moving the
// panel behind it.

/// The card's width, shared with every other popover in the inspector.
const POP_W: f32 = crate::app::POPOVER_W;
const POP_PAD: f32 = 11.0;
/// The room inside the padding **and the border** — `POP_W` is the width the card
/// paints, not the room in it (§15 D307).
///
/// This one was the eighth site of eight and was found *after* the other seven
/// were converted, by reading the sweep rather than by anything failing: the
/// assertion that pins a card's painted width drives `menu_frame` directly, so it
/// cannot see a call site that computes its own inner width. Two popovers hang off
/// the export panel and only the one beside this was threaded.
const POP_INNER: f32 = ui::menu_inner_w(POP_W, POP_PAD);
/// A control inside the popover — **26, against the card's 28**.
///
/// The same bargain `typography::CELL` makes and for the same reason: a card of
/// three rows against a popover of four sections, where the compression is what
/// keeps the second from towering over the first.
const POP_CELL: f32 = 26.0;
/// Between one labelled section and the next — the shared one (§15 D275).
const SECTION_GAP: f32 = ui::POPOVER_SECTION_GAP;
/// Between a section's label and its control — the design's, for both popovers.
const LABEL_GAP: f32 = 5.0;
/// The ✕ beside the card's eyebrow, shared with the type popup's.
const CLOSE_W: f32 = 18.0;

/// One labelled block of the popover: a small-caps eyebrow, then its control.
///
/// **A `scope`, so the card's two vertical gaps are two numbers rather than one**
/// — `typography::section`'s reasoning verbatim, and the reason it is copied
/// rather than shared is that its constants are private to that module. Left to a
/// single spacing the label floats equidistant between its own control and the
/// section above, which is the reading that makes four sections look like eight
/// loose rows.
fn section<R>(ui: &mut egui::Ui, label: &str, add: impl FnOnce(&mut egui::Ui) -> R) -> R {
    ui.scope(|ui| {
        ui.spacing_mut().item_spacing.y = LABEL_GAP;
        ui.label(ui::eyebrow(label));
        add(ui)
    })
    .inner
}

/// What one row asked for.
#[derive(Default)]
struct RowOut {
    edited: Option<ExportSpec>,
    remove: bool,
}

/// Which of the two free-text parts of a filename a field is editing.
#[derive(Clone, Copy, PartialEq)]
enum NamePart {
    Prefix,
    Suffix,
}

impl NamePart {
    fn label(self) -> &'static str {
        match self {
            NamePart::Prefix => "Prefix",
            NamePart::Suffix => "Suffix",
        }
    }
    /// Distinct per part *and* per row, so two fields in one card and the same
    /// field on two rows are four widgets rather than one.
    fn id_salt(self) -> &'static str {
        match self {
            NamePart::Prefix => "export-prefix",
            NamePart::Suffix => "export-suffix",
        }
    }
}

/// What a cached preview was built from.
///
/// **Three terms, and the revision is the one that is easy to leave out.** A
/// preview keyed only by layer and spec survives every edit to the artwork it is a
/// picture of, which is worse than having no preview at all — it is a *confident*
/// picture of something that is no longer there.
#[derive(PartialEq)]
pub(crate) struct PreviewKey {
    node: NodeId,
    spec: ExportSpec,
    revision: u64,
}

/// One rendered preview, kept until something it was built from changes.
pub(crate) struct ExportPreview {
    key: PreviewKey,
    /// `None` for SVG, which has no pixels to show, and for a raster too large to
    /// rebuild per edit.
    texture: Option<egui::TextureHandle>,
    pixels: (u32, u32),
    /// The line under it: format, size, and what it weighs.
    summary: String,
}

/// What an SVG export could not say exactly, as a clause to hang off a message —
/// `None` when the file is the document (§15 D780, `[S8.1-L7-05]`).
///
/// **Deliberately the same two-clause shape as the paste message** on the reader's
/// side (`canvas.rs`'s *"Pasted SVG — 3 layers, skipped foreignObject"*), because
/// the two ends of a round trip are read by the same person and a second vocabulary
/// for the same idea is a second thing to learn. Sorted for the same reason it is
/// sorted there: the order features are *met* is an accident of the document.
///
/// 🚨 **Two clauses and not one**, which is `Import`'s own argument carried over:
/// *approximated* means the drawing differs and *round-trip* means the drawing is
/// right but re-opening the file gives a different document. A user told their
/// image fill was "approximated", over an export that looks perfect in a browser,
/// goes looking for a bug that is not there.
pub(crate) fn fidelity_clauses(out: &ondin_export::svg::Export) -> Option<String> {
    let mut parts = Vec::new();
    let mut say = |label: &str, list: &[String]| {
        if !list.is_empty() {
            let mut items = list.to_vec();
            items.sort();
            parts.push(format!("{label} {}", items.join(", ")));
        }
    };
    say("approximated", &out.approximated);
    say("will not re-import as drawn:", &out.round_trip);
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// `84 KB`, `1.2 MB` — what a file size is worth saying to a designer.
///
/// **1024, not 1000.** Every file browser this number will be compared against on
/// Windows and macOS uses binary units under those labels, and a preview that
/// disagreed with Explorer by 2.4% would read as the preview being wrong.
fn human_bytes(n: usize) -> String {
    const KB: usize = 1024;
    match n {
        n if n < KB => format!("{n} B"),
        n if n < KB * KB => format!("{} KB", n.div_ceil(KB)),
        n => format!("{:.1} MB", n as f64 / (KB * KB) as f64),
    }
}

impl OndinApp {
    /// The layers an export acts on: the selection, in document order.
    ///
    /// **`build::in_document_order`, for both of its halves** — a selection
    /// holding a group and something inside it would plan that child twice, and a
    /// selection picked front-to-back would come out restacked in any file that
    /// held more than one of them (§15 D267). The same filter the two *Copy as*
    /// rows apply, and for the same reasons — it was *Export as…*'s too, until that
    /// row was removed (§15 D264).
    pub(crate) fn export_subjects(&self) -> Vec<NodeId> {
        ondin_core::build::in_document_order(&self.session.doc, self.session.selection.ids())
    }

    /// Whether the selection has moved since the Export panel last read it — the
    /// edge `sync_paint_collapse` re-asserts the resting collapse state on.
    ///
    /// `paint_subject_changed`'s twin, and separate for the reason its own field
    /// carries: consuming one edge from two callers loses it for one of them.
    fn export_subject_changed(&mut self) -> bool {
        let now = self.session.selection.ids();
        if self.export_synced_selection == now {
            return false;
        }
        self.export_synced_selection = now.to_vec();
        true
    }

    /// The list every subject agrees on, or `None` when they disagree.
    ///
    /// **The committed document, not `display_node`.** Every other panel reads
    /// through the preview so its controls follow a gesture in flight; this one
    /// cannot, because `Operation::SetExports` is a no-op in `RenderOverrides` —
    /// there is nothing about an export setting to preview. Reading the committed
    /// node is therefore not a shortcut but the only honest source.
    fn shared_exports(&self, subjects: &[NodeId]) -> Option<Vec<ExportSpec>> {
        let mut nodes = subjects.iter().filter_map(|id| self.session.doc.get(*id));
        let first = nodes.next()?.exports().to_vec();
        nodes
            .all(|n| n.exports() == first.as_slice())
            .then_some(first)
    }

    /// Write one list to every subject, as a single undoable step.
    ///
    /// One transaction rather than one per layer: adding a 2× PNG to thirty icons
    /// is one act and has to undo as one.
    fn write_exports(&mut self, subjects: &[NodeId], specs: &[ExportSpec]) {
        let tx = self.exports_tx(subjects, specs);
        if !tx.0.is_empty() {
            // Through the panel committer like every other inspector control, and
            // it costs nothing to be right about: `edit_note` returns early for a
            // transaction whose `changes_ink` is false, so an export edit does not
            // arm the chrome hide — which is what it should not do, since nothing
            // it changes is drawn. The exception is in the mechanism rather than in
            // a second committer.
            self.commit_edit(tx);
        }
    }

    fn exports_tx(&self, subjects: &[NodeId], specs: &[ExportSpec]) -> Transaction {
        Transaction(
            subjects
                .iter()
                .map(|id| Operation::SetExports {
                    id: *id,
                    exports: specs.to_vec(),
                })
                .collect(),
        )
    }

    /// Append `specs` to whatever each subject already has.
    ///
    /// **The `+`'s edit over a selection that *agrees***, where a disagreeing one
    /// goes through [`Self::write_exports`] instead: appending there is
    /// well-defined and still leaves the layers disagreeing, which is a state the
    /// card cannot be edited out of (`export_mixed_row`). Each subject is read
    /// separately all the same, because "whatever each already has" is the honest
    /// spelling of an append and costs nothing when they match.
    fn append_exports(&mut self, subjects: &[NodeId], specs: &[ExportSpec]) {
        let ops: Vec<Operation> = subjects
            .iter()
            .filter_map(|id| {
                let node = self.session.doc.get(*id)?;
                let mut next = node.exports().to_vec();
                next.extend(specs.iter().cloned());
                Some(Operation::SetExports {
                    id: *id,
                    exports: next,
                })
            })
            .collect();
        if !ops.is_empty() {
            // Through the panel committer, like `write_exports` beside it and every
            // other inspector control — the two halves of one button must not
            // disagree about the seam they commit on. It is the same commit either
            // way today: `note_edit` returns early for a transaction whose
            // `changes_ink` is false, and nothing a `SetExports` carries is drawn.
            self.commit_edit(Transaction(ops));
        }
    }

    /// The Export panel.
    pub(super) fn inspector_export(&mut self, ui: &mut egui::Ui) {
        let subjects = self.export_subjects();
        if subjects.is_empty() {
            return;
        }
        // The other half of `export_buttons`' rule: a collapsed panel draws no body
        // at all, so nothing in it can put the popover away.
        if self.collapsed_panels.contains("Export") {
            self.export_menu = false;
        }
        let shared = self.shared_exports(&subjects);
        // **Closed when the layer exports nothing, exactly as Fill and Stroke
        // are.** A card whose whole body is the words "No exports" is a row of
        // chrome saying nothing, and every layer in a fresh document is in that
        // state — so the resting inspector would grow a dead card per selection.
        // The `+` in the header opens it, which is why the header keeps its action
        // while collapsed.
        let fresh = self.export_subject_changed();
        self.sync_paint_collapse("Export", shared.as_ref().is_some_and(Vec::is_empty), fresh);
        // **Three tooltips, because the `+` does two different things** — and the
        // one it does over a disagreeing selection is the destructive one, so it
        // has to say so before the click rather than in the card afterwards
        // (`export_mixed_row`).
        let mixed = shared.is_none();
        let action = Some(ui::HeadAction::new(
            icon::PLUS,
            match (subjects.len(), mixed) {
                (1, _) => "Add an export",
                (_, false) => "Add an export to every selected layer",
                (_, true) => "Replace every selected layer's exports with one",
            },
        ));
        // Read before the body, because the body may edit the list under it and
        // the button below has to describe what is *there* rather than what was.
        let add = self.panel(ui, "Export", action, |app, ui| {
            let Some(specs) = shared.clone() else {
                app.export_mixed_row(ui);
                app.export_buttons(ui, &subjects);
                return;
            };
            if specs.is_empty() {
                ui.label(
                    egui::RichText::new("No exports")
                        .size(11.0)
                        .color(theme::text::FAINT),
                );
                return;
            }
            let mut next = specs.clone();
            let (mut removed, mut changed) = (None, false);
            // One subject's box sizes every row; over a multi-selection there is
            // no single answer, so the size cell says so rather than picking a
            // member's arbitrarily (`export_row`).
            let sizing = (subjects.len() == 1).then(|| subjects[0]);
            // **The rows are their own zero-spacing region**, because the gap
            // between two exports is *smaller* than the card's own row gap and a
            // scope cannot subtract: the card places its children 9 apart, so 6 can
            // only be had by taking the pitch to zero here and allocating it. The
            // trailing leak `paint_unit` warns about is zero for the same reason,
            // which is what leaves the button below at the card's ordinary 9.
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, spec) in specs.iter().enumerate() {
                    let out = app.export_row(ui, i, spec, sizing, i + 1 == specs.len());
                    if let Some(edited) = out.edited {
                        next[i] = edited;
                        changed = true;
                    }
                    if out.remove {
                        removed = Some(i);
                    }
                }
            });
            if let Some(i) = removed {
                next.remove(i);
                // The open block and the buffer are keyed by index, so both are
                // dropped rather than left pointing at whatever slid up into the
                // slot — the housekeeping `forget_stroke_chrome` does for the
                // stroke rows, in the one place this list can shrink.
                app.export_row_open = None;
                app.export_prefix_text = None;
                app.export_suffix_text = None;
                changed = true;
            }
            if changed {
                app.write_exports(&subjects, &next);
            }
            app.export_buttons(ui, &subjects);
            // **Inside the card, not a card of its own.** A preview belongs to the
            // export set it is a preview *of*, and a second floating card under it
            // read as a second feature; its own caret is what still lets the cost
            // be turned off, which is the only thing the separate card was buying.
            app.inspector_export_preview(ui, &subjects);
        });
        if add {
            let seed = self.next_export_spec(&subjects);
            match mixed {
                // **Over a disagreement the `+` overrides rather than appends**, so
                // one click leaves a selection that agrees and a list to edit.
                // Appending to each layer's own list was the careful answer and it
                // is a dead end: the layers still disagree, so the card still shows
                // the same sentence and there is no way out of the state from
                // inside it (`export_mixed_row`).
                true => self.write_exports(&subjects, &[seed]),
                false => self.append_exports(&subjects, &[seed]),
            }
            // The panel may be shut — it is, on every layer that had no exports a
            // moment ago — and a `+` that adds a row you cannot see is a button
            // that does nothing. `inspector_fill`'s call, for its reason.
            self.open_paint_panel("Export");
        }
    }

    /// The **Preview** section: what the first export actually comes out as.
    ///
    /// **Rendered only while the section is open, and at most once per edit.** A
    /// preview is a real render and a real encode — the only place in the panel
    /// that costs anything — so doing it per frame is out of the question and doing
    /// it per *change* is exactly affordable. `EditorSession::revision` is the
    /// third term of the cache key beside the layer and the spec, because a
    /// preview of a shape that has since been recoloured is worse than none.
    ///
    /// **Collapsed by default**, which Figma's is too, and here it is also the
    /// switch that turns the cost on.
    ///
    /// One layer only. Over a multi-selection there is no single subject to show,
    /// and thirty thumbnails is a different panel.
    fn inspector_export_preview(&mut self, ui: &mut egui::Ui, subjects: &[NodeId]) {
        let [subject] = subjects else { return };
        let (subject, spec) = match self.shared_exports(subjects) {
            // The row whose settings are open, else the first — so opening a row's
            // block and turning its knobs previews *that* row.
            Some(specs) if !specs.is_empty() => {
                let i = self.export_row_open.unwrap_or(0).min(specs.len() - 1);
                (*subject, specs[i].clone())
            }
            _ => return,
        };
        // **Closing the section frees the texture.** A 4-megapixel preview is 16 MB
        // of GPU memory held for a card nobody is looking at, and the section is
        // rebuilt from scratch on the next edit anyway — so there is nothing to
        // keep it for.
        let open = !self.collapsed_panels.contains(PREVIEW);
        if !open {
            self.export_preview = None;
        }
        // **A section inside the card, not a card of its own** — so the header is
        // `section_head` rather than `panel`, drawn at the card's own left edge
        // with no second ground behind it. The caret still does the one thing the
        // separate card was for: opening it is what turns the render on.
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = ROW_LINE_GAP;
            if ui::section_head(ui, "Preview", open, None, true).toggled {
                match open {
                    true => self.collapsed_panels.insert(PREVIEW),
                    false => self.collapsed_panels.remove(PREVIEW),
                };
            }
            if !open {
                return;
            }
            let key = PreviewKey {
                node: subject,
                spec: spec.clone(),
                revision: self.session.revision(),
            };
            if self.export_preview.as_ref().map(|p| &p.key) != Some(&key) {
                self.export_preview = self.build_export_preview(ui.ctx(), key);
            }
            let Some(preview) = &self.export_preview else {
                return;
            };
            if let Some(tex) = &preview.texture {
                // Fit the width, never magnify: an 8px icon blown up to 256 is a
                // picture of the resampler rather than of the export.
                let (w, h) = (preview.pixels.0 as f32, preview.pixels.1 as f32);
                let avail = ui.available_width();
                let scale = (avail / w).min(1.0);
                let shown = egui::vec2(w * scale, h * scale);
                // **On the checkerboard, inset from it** — the design's frame, and
                // it is the only way a transparent export can be told from a white
                // one. The ground spans the card so a narrow icon still reads as a
                // picture on a surface rather than as a floating thumbnail.
                let (ground, _) = ui.allocate_exact_size(
                    egui::vec2(avail, shown.y + PREVIEW_INSET * 2.0),
                    egui::Sense::empty(),
                );
                let p = ui.painter().with_clip_rect(ground);
                ui::paint_checkerboard(&p, ground, 5.0);
                p.rect_stroke(
                    ground,
                    egui::CornerRadius::same(5),
                    egui::Stroke::new(1.0, theme::color::FIELD_BORDER),
                    egui::StrokeKind::Inside,
                );
                let at = egui::Rect::from_center_size(ground.center(), shown);
                p.image(tex.id(), at, crate::thumbs::FULL_UV, egui::Color32::WHITE);
            }
            ui.label(
                egui::RichText::new(&preview.summary)
                    .size(10.5)
                    .color(SIZE_INK),
            );
        });
    }

    /// Render one spec and keep the result: its size on disk, and a texture small
    /// enough to draw in the card.
    ///
    /// `None` only when the layer has no bounds — the refusal every writer makes.
    fn build_export_preview(
        &mut self,
        ctx: &egui::Context,
        key: PreviewKey,
    ) -> Option<ExportPreview> {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        // **No raster for an SVG, deliberately.** Rasterizing the markup to show it
        // would be previewing something other than the file, and the one thing
        // worth knowing about a vector export is a number.
        if !key.spec.format.is_raster() {
            // **The reported form** (§15 D780): the card is what a user reads
            // *before* exporting, which is the one moment a fidelity note can
            // still change what they do.
            let out = ondin_export::svg::svg_of_reported(doc, res, &[key.node]);
            let note = fidelity_clauses(&out);
            let bytes = out.svg.into_bytes();
            let summary = match note {
                None => format!("SVG · {}", human_bytes(bytes.len())),
                Some(note) => format!("SVG · {} · {note}", human_bytes(bytes.len())),
            };
            return Some(ExportPreview {
                key,
                texture: None,
                pixels: (0, 0),
                summary,
            });
        }

        let (size, clamped) = ondin_export::plan::raster_size(res, key.node, &key.spec);
        let (w, h) = size?;
        // **The cap is on the preview, not on the export.** A 4× of a large frame
        // is a perfectly reasonable file and an unreasonable thing to rebuild on
        // every edit, so past this the section reports the size it *would* be and
        // renders nothing. Stating the number beats a progress bar that never ends.
        const MAX_PREVIEW_PX: u64 = 4_000_000;
        // ⚠️ **Measured against the size the raster will *be*, not the size the
        // plan reports.** `plan::raster_size` answers the framed size, and
        // `raster_of` then pads it to a square *after* this guard has passed —
        // so a 32768×120 layer with *Pad out to a square* on came through here at
        // 3.9 M pixels, well under the cap, and asked for a 32768² buffer of 4 GB
        // on the UI thread. The same raster is what a 4096×15 banner at 8× makes,
        // which is an ordinary export set rather than a contrived one.
        //
        // **An area cap cannot see either of the raster path's failures on its
        // own**, which is the shape worth remembering: they are a *per-side* and
        // a *longest-side* limit, and any area admits any aspect ratio thin
        // enough. Squaring the pair first is what turns the second one into an
        // area question. The first is now `MAX_RASTER_SIDE`'s job — it used to be
        // the value `vello_cpu` panics at, so this preview reached that too.
        let (pw, ph) = match key.spec.pad_square {
            true => (w.max(h), w.max(h)),
            false => (w, h),
        };
        //
        // ⚠️ **And a per-side test beside the area one, because a texture has
        // both.** `ctx.load_texture` panics outright past
        // `Context::max_texture_side` — *"Texture "export-preview" has size
        // 32768x120, but the maximum texture side is 2048"* — and a 32768×120
        // raster is 3.9 M pixels, under the cap, so the area test waves it
        // through. That is the same shape as the pad above and was found the same
        // way: by writing the pad's control and watching it crash. The number is
        // the **device's**, read off the context rather than written here, since
        // it is whatever wgpu reports on the machine the app is running on.
        let side = ctx.input(|i| i.max_texture_side) as u64;
        if pw as u64 * ph as u64 > MAX_PREVIEW_PX || pw as u64 > side || ph as u64 > side {
            return Some(ExportPreview {
                key,
                texture: None,
                // The padded pair, because that is the file the row is about and
                // reporting 32768×120 for a 32768² export would be describing a
                // step the export does not stop at.
                pixels: (pw, ph),
                summary: format!("{pw}×{ph} — too large to preview here"),
            });
        }

        // **Rasterized once**, then both encoded and drawn. Going through
        // `plan::render` for the byte count and `raster_of` for the texture would
        // render the same thing twice per edit, which is the one cost this section
        // has to keep down.
        let opts = ondin_export::plan::raster_opts(doc, res, key.node, &key.spec);
        let raster = ondin_export::png::raster_of(doc, res, &[key.node], &opts)?;
        let bytes = match key.spec.format {
            ExportFormat::Jpeg => ondin_export::jpeg::encode_rgba8(
                &raster.rgba,
                raster.width,
                raster.height,
                key.spec.quality,
            ),
            _ => ondin_export::png::encode_rgba8(&raster.rgba, raster.width, raster.height),
        };
        let summary = format!(
            "{} · {}×{}{} · {}",
            key.spec.format.label(),
            raster.width,
            raster.height,
            if clamped { " (clamped)" } else { "" },
            human_bytes(bytes.len())
        );
        let image = egui::ColorImage::from_rgba_unmultiplied(
            [raster.width as usize, raster.height as usize],
            &raster.rgba,
        );
        let texture = ctx.load_texture("export-preview", image, egui::TextureOptions::LINEAR);
        Some(ExportPreview {
            key,
            texture: Some(texture),
            pixels: (raster.width, raster.height),
            summary,
        })
    }

    /// What the `+` adds.
    ///
    /// **1×, 2×, 3×, then 4× for ever after** — the icon workflow, where the same
    /// drawing is wanted at several densities and the second and third clicks
    /// should not both have to be corrected. The cap is not a fallback: past 3×
    /// the list is no longer a density ladder, so climbing further would be the
    /// panel inventing sizes rather than following one.
    ///
    /// **The first row is 1× no matter what**, which is the fix this rule was
    /// re-cut around. It used to pick the first multiplier *not already spoken
    /// for*, which is a different question and answers it wrong at both ends: the
    /// first click on a fresh layer took 1× only because nothing had claimed it
    /// yet, and a list already holding a 1× and a 2× — however it got there — was
    /// handed a 3× as its *second* row. Counting the rows is what the design
    /// actually says, so that is what is counted.
    ///
    /// The **format** is inherited from the last row, because a list that is all
    /// JPEG is a decision about the whole list rather than about one entry — and
    /// PNG whenever there is nothing to inherit from, including a selection that
    /// disagrees.
    ///
    /// A repeat is not a hazard on disk: `plan::de_collide` gives a second file of
    /// the same name its ` (2)`, so nothing is overwritten if the ladder is
    /// climbed past its cap or a row is edited back onto another.
    fn next_export_spec(&self, subjects: &[NodeId]) -> ExportSpec {
        let existing = self.shared_exports(subjects).unwrap_or_default();
        let format = existing.last().map_or(ExportFormat::Png, |s| s.format);
        ExportSpec::new(format, ladder_scale(existing.len()))
    }

    /// What a selection whose layers export differently shows instead of rows.
    ///
    /// **It offers no list to edit, and one line saying what the `+` will do**,
    /// which is the same answer the Fill panel gives — the difference being that
    /// here the header's `+` still works.
    ///
    /// **"Add to override", because that is what it now does**: the `+` replaces
    /// every selected layer's list with the one export it adds, so the state
    /// resolves in one click and the rows appear. It used to *append* to each
    /// layer's own list, which is the answer that never leaves this state — thirty
    /// layers that disagreed still disagree afterwards, and the card says the same
    /// sentence back at you with one more file per layer to show for it. The
    /// destruction that reading was avoiding is real and is what Undo is for; the
    /// dead end was not.
    ///
    /// Two lines came down to one for the same reason: with the verb in the
    /// sentence — *add to override* — the second line had nothing left to say.
    fn export_mixed_row(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("Mixed exports. Add to override.")
                .size(11.0)
                .color(theme::text::FAINT),
        );
    }

    /// One export: its scale, its format, the size it will come out at, and the
    /// two buttons.
    fn export_row(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        spec: &ExportSpec,
        sizing: Option<NodeId>,
        last: bool,
    ) -> RowOut {
        let mut out = RowOut::default();
        let open = self.export_row_open == Some(index);
        let mut head = None;
        // **The row and its size line are one unit, and one unit sits further from
        // the next than its two halves sit from each other** — `paint_unit`'s
        // arrangement, so an export reads as one entry and two exports read as two.
        //
        // **`item_spacing` is zero and both gaps are allocated**, which is that
        // function's hard-won detail rather than tidiness (§15 D195): egui advances
        // its cursor past a widget by the spacing in effect *when that widget is
        // placed*, so a spacing set for the two lines is also spent after the last
        // of them — which is why the unit boundary measured 15 rather than the 9 it
        // was written as.
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = PITCH;
                // A `ComboBox` sizes its own height from `interact_size.y` plus
                // `button_padding`, so without this it comes out 26 against the 28px
                // controls beside it and sits a pixel off their baseline — the same
                // correction the stroke row's alignment combo carries, and for the same
                // reason: a combo strokes its border *inside* the rect it fills.
                ui.scope(|ui| {
                    ui.spacing_mut().interact_size.y = CELL;
                    ui.spacing_mut().button_padding.y = 0.0;
                    let mut scale = spec.scale;
                    let live = spec.scale_applies();
                    let combo = egui::ComboBox::from_id_salt(("export-scale", index))
                        .icon(ui::combo_chevron)
                        .width(SCALE_W)
                        .height(260.0)
                        .selected_text(spec.scale.label());
                    if live {
                        combo.show_ui(ui, |ui| {
                            ui::menu_rows(ui);
                            ui.spacing_mut().button_padding.y = 2.0;
                            for n in ExportScale::COMMON {
                                let s = ExportScale::Times(n);
                                ui.selectable_value(&mut scale, s, s.label());
                            }
                            // Figma's two fixed sizes, under a rule because they answer
                            // a different question: a multiplier is "the same drawing,
                            // denser", these are "whatever it takes to be this big".
                            // The exact number is typed in the settings block, which is
                            // also the only place a value not in this list can be set.
                            super::inspector::popup_rule(ui);
                            for s in [ExportScale::Width(512), ExportScale::Height(512)] {
                                ui.selectable_value(&mut scale, s, s.label());
                            }
                        });
                    } else {
                        // **Dimmed and still there, holding its value.** SVG has no
                        // pixels for a multiplier to multiply, and the two wrong
                        // answers are hiding the control — which moves everything
                        // beside it as the format changes — and silently resetting the
                        // scale, which loses a setting the moment you look at what a
                        // layer would export as vector.
                        combo
                            .show_ui(ui, |ui| {
                                ui.label(
                                    egui::RichText::new("SVG has no pixels to scale")
                                        .size(11.0)
                                        .color(theme::text::FAINT),
                                );
                            })
                            .response
                            .on_hover_text("A vector file has no resolution");
                    }
                    if scale != spec.scale {
                        out.edited = Some(ExportSpec {
                            scale,
                            ..spec.clone()
                        });
                    }

                    // **The format cell takes what is left**, where the scale's is
                    // fixed: the design's `74px 1fr 28px 28px`. It is the wider of the
                    // two because it carries a leading glyph as well as its word, and
                    // because a scale reads as a number where a format reads as a name.
                    let mut format = spec.format;
                    let format_w = ui.available_width() - CELL * 2.0 - PITCH * 2.0;
                    egui::ComboBox::from_id_salt(("export-format", index))
                        .icon(ui::combo_chevron)
                        .width(format_w.max(0.0))
                        .selected_text(ui::glyph_and_text(icon::FILE_IMAGE, spec.format.label()))
                        .show_ui(ui, |ui| {
                            ui::menu_rows(ui);
                            ui.spacing_mut().button_padding.y = 2.0;
                            for f in ExportFormat::ALL {
                                ui.selectable_value(&mut format, f, f.label());
                            }
                        });
                    if format != spec.format {
                        out.edited = Some(ExportSpec {
                            format,
                            ..out.edited.clone().unwrap_or_else(|| spec.clone())
                        });
                    }
                });

                let more = ui::field_button(
                    ui,
                    icon::SLIDERS_HORIZONTAL,
                    CELL,
                    14.0,
                    settings_button_state(spec, open),
                )
                .on_hover_text(match open {
                    true => "Hide these settings",
                    false => "Prefix, suffix, background and the rest",
                });
                if more.clicked() {
                    self.export_row_open = (!open).then_some(index);
                    self.export_prefix_text = None;
                    self.export_suffix_text = None;
                    // **The panel's two popovers are exclusive**, which the rest of the
                    // inspector's are not — and the difference is the lane. A stroke
                    // popover and a type popover hang from different cards and
                    // `popover_anchor` gives them different heights; these two hang
                    // from the same card a few points apart, so both open is one card
                    // covering the other with no way to reach the one underneath.
                    self.export_menu = false;
                }
                head = Some(more);
                // **An ✕, not a minus.** The design changed it, and the two glyphs are
                // not interchangeable here: `MINUS` is the *off* mark in this app's
                // switches (`ui::toggle_row`, the decoration track), so a row ending in
                // one reads as a state rather than as a verb.
                if ui::field_button(ui, icon::X, CELL, 14.0, ui::FieldButton::Off)
                    .on_hover_text("Remove this export")
                    .clicked()
                {
                    out.remove = true;
                }
            });
            // **The size the row will produce, on a line of its own.** It had been a
            // tooltip, because a number that can be five characters or eleven has no
            // fixed *cell* it can honestly live in — a second line has no such limit,
            // and the answer to "why is my icon 47 pixels" is worth reading without
            // hovering. Blank over a multi-selection, where there is no single answer
            // (`export_size_label`), and the line goes with it rather than sitting
            // empty.
            let size = self.export_size_label(spec, sizing);
            let lined = !size.is_empty();
            if lined {
                ui.allocate_space(egui::vec2(0.0, ROW_LINE_GAP));
                // **`interact_size.y` is a *minimum* row height, and the theme's is
                // 24.** A `horizontal` of two small labels therefore comes out 24
                // tall with 13pt of ink centred in it — five points of nothing above
                // and five below, which land on top of the gaps either side and made
                // an 8 read as 13 and an 18 as 23. Zeroed here so the line is exactly
                // as tall as its ink and the two numbers above mean what they say.
                // The row above needs no such thing: its controls are 28, so the
                // minimum never bites.
                ui.scope(|ui| {
                    ui.spacing_mut().interact_size.y = 0.0;
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 6.0;
                        ui.add_space(2.0);
                        ui.label(theme::icon_text(icon::FRAME_CORNERS, 12.0, SIZE_INK));
                        ui.label(egui::RichText::new(size).size(10.5).color(SIZE_INK));
                    });
                });
            }
            // The whole gap, not a remainder: the region these units sit in has its
            // pitch at zero (see the loop). The *last* row allocates none at all, so
            // what separates the list from the button below it is the card's own
            // row gap, like every other card's tail.
            //
            // **A row that drew no size line spends that line's gap here instead**,
            // which is the 3 a multi-selection's list was missing: [`ROW_UNIT_GAP`]
            // is the distance between two *units*, and it is tight — below the
            // card's own pitch — because a unit is two lines tall and needs the
            // contrast to read as one thing. Strip the caption and there is no unit
            // left to hold together, so six points between two bare 28pt controls
            // is just crowding. The sum is written out rather than borrowed from
            // `ui::CARD_ROW_GAP` — which it happens to equal today — because it is
            // this list's two numbers that decide it; that they land on the card's
            // own pitch is the corroboration, not the derivation.
            if !last {
                ui.allocate_space(egui::vec2(
                    0.0,
                    match lined {
                        true => ROW_UNIT_GAP,
                        false => ROW_UNIT_GAP + ROW_LINE_GAP,
                    },
                ));
            }
        });
        if let Some(head) = head
            && open
            && let Some(edited) = self.export_settings_popup(ui, index, spec, sizing, &head)
        {
            out.edited = Some(edited);
        }
        out
    }

    /// `192×96`, or the reason there is no number.
    ///
    /// **Blank over a multi-selection rather than a guess.** `Width(512)` is a
    /// different multiplier on every differently-shaped layer, so one number for
    /// thirty icons would be true of at most one of them — and the row that
    /// carries it is the same row for all thirty.
    fn export_size_label(&self, spec: &ExportSpec, sizing: Option<NodeId>) -> String {
        if !spec.format.is_raster() {
            return String::new();
        }
        let Some(id) = sizing else {
            return String::new();
        };
        match ondin_export::plan::raster_size(&self.session.resolved, id, spec) {
            (Some((w, h)), true) => format!("{w}×{h} max"),
            (Some((w, h)), false) => format!("{w}×{h}"),
            (None, _) => String::new(),
        }
    }

    /// One row's settings, in a card beside the panel — everything that did not
    /// fit on the row.
    ///
    /// **A popover, and it replaced an inline block.** The block was the call
    /// `stroke_sides_row` makes, and it was the wrong one here: this card carries
    /// four labelled sections where that row carries one, so opening it pushed the
    /// export button and every row under it down the column — the exact thing
    /// `stroke_menu_popup` cites for going the other way. It is also the shape the
    /// rest of the inspector uses for "the rest of this control's settings", and a
    /// panel that invented a second shape for it would read as a different app.
    ///
    /// The shell is the shared one: `POPOVER_W` wide, `menu_frame`'s padding,
    /// `popover_anchor`'s lane to the left of the inspector, and the six-term
    /// click-away rule every other card here obeys.
    fn export_settings_popup(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        spec: &ExportSpec,
        // The one subject, or `None` over a multi-selection — the same value
        // `export_row` reads for its size line, threaded down because the
        // background chip owes the same honesty (§15 D657).
        sizing: Option<NodeId>,
        head: &egui::Response,
    ) -> Option<ExportSpec> {
        // **Only ever `Some` on a frame the edit is *finished*.** The one
        // scrubbable control here holds its in-flight value itself
        // (`export_quality_field`), so nothing that reaches this return needs a
        // valve or a run behind it — every edit that gets out is a decision.
        let mut out: Option<ExportSpec> = None;
        let mut close = false;
        let ctx = ui.ctx().clone();
        let anchor = Self::popover_anchor(&ctx, head.rect, POP_W);
        let menu = egui::Area::new(egui::Id::new(("export-settings", index)))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            // The anchor is where the card *wants* to be; this keeps it on screen
            // when the row it hangs from is near the bottom of a long panel.
            .constrain_to(ctx.input(|i| i.content_rect()).shrink(12.0))
            .show(&ctx, |ui| {
                ui::menu_frame(POP_PAD).show(ui, |ui| {
                    ui.set_width(POP_INNER);
                    ui.spacing_mut().item_spacing.y = SECTION_GAP;

                    // --- header ------------------------------------------------
                    ui.horizontal(|ui| {
                        ui.label(ui::eyebrow("Export settings"));
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui::icon_button(ui, icon::X, CLOSE_W, 13.0, false, true)
                                .on_hover_text("Close")
                                .clicked()
                            {
                                close = true;
                            }
                        });
                    });

                    // --- prefix, then suffix -----------------------------------
                    //
                    // Both, and in the order they appear in the filename, which is
                    // the only order either could be in.
                    if let Some(text) = self.name_part_field(
                        ui,
                        index,
                        NamePart::Prefix,
                        &spec.prefix,
                        "folder/",
                        "Put in front of the layer's name. Ending it with / makes a folder",
                    ) {
                        edit(&mut out, spec).prefix = text;
                    }
                    if let Some(text) = self.name_part_field(
                        ui,
                        index,
                        NamePart::Suffix,
                        &spec.suffix,
                        // **The hint is the marker the scale derives**, so an
                        // untouched field says what the file will actually be
                        // called rather than showing a generic example. At 1× there
                        // is no marker and the field explains itself instead.
                        &match spec.effective_suffix() {
                            s if s.is_empty() => "e.g. -dark".to_string(),
                            s => s,
                        },
                        "Put after the layer's name. Left empty it follows the scale",
                    ) {
                        edit(&mut out, spec).suffix = text;
                    }

                    // --- quality, JPEG only ------------------------------------
                    //
                    // **Not in the mock's list until it was asked for, and named
                    // for its format.** "Quality" beside a Background and a Prefix
                    // reads as a general knob; only JPEG has one, and the row
                    // appears only when the format does — so the label says which.
                    // Placed after the suffix because the three above it are about
                    // the *name* and everything below is about the *picture*.
                    if spec.format == ExportFormat::Jpeg {
                        section(ui, "JPEG quality", |ui| {
                            if let Some(q) = self.export_quality_field(ui, index, spec) {
                                edit(&mut out, spec).quality = q;
                            }
                        });
                    }

                    // --- background --------------------------------------------
                    //
                    // **The row says what the setting *is*, not what it resolves
                    // to.** It read "White (no alpha)" for a Transparent JPEG,
                    // which was meant as honesty and lands as a control disagreeing
                    // with the list it was picked from — you choose Transparent and
                    // the field says White. The substitution is real and still
                    // stated, in the tooltip, where it explains rather than
                    // contradicts.
                    section(ui, "Background", |ui| {
                        ui.scope(|ui| {
                            ui.spacing_mut().interact_size.y = POP_CELL;
                            ui.spacing_mut().button_padding.y = 0.0;
                            let mut bg = spec.background;
                            let swatch = self.background_swatch(spec, sizing);
                            egui::ComboBox::from_id_salt(("export-bg", index))
                                .icon(ui::combo_chevron)
                                .width(POP_INNER)
                                .selected_text(ui::glyph_and_text_tinted(
                                    icon::SQUARE,
                                    self.background_label(spec),
                                    swatch,
                                ))
                                .show_ui(ui, |ui| {
                                    ui::menu_rows(ui);
                                    ui.spacing_mut().button_padding.y = 2.0;
                                    ui.selectable_value(
                                        &mut bg,
                                        ExportBackground::Transparent,
                                        "Transparent",
                                    );
                                    ui.selectable_value(
                                        &mut bg,
                                        ExportBackground::Frame,
                                        "Frame's own",
                                    );
                                    // **White, not "Colour".** The model carries any
                                    // colour and the CLI can write one, but there is
                                    // nowhere yet to *pick* one — the picker targets
                                    // a paint slot on a node, and an export
                                    // background is neither. A row named for a
                                    // choice that cannot be made is worse than the
                                    // one value that is actually wanted, which is
                                    // the matte a JPEG already resolves to.
                                    ui.selectable_value(
                                        &mut bg,
                                        ExportBackground::Solid(ondin_core::peniko::Color::WHITE),
                                        "White",
                                    );
                                })
                                .response
                                .on_hover_text(match (spec.background, spec.format.keeps_alpha()) {
                                    (ExportBackground::Transparent, false) => {
                                        "JPEG has no alpha — this exports on white"
                                    }
                                    // **Plural where the answer is plural** (§15
                                    // D657): over a multi-selection the chip goes
                                    // muted because there is no one colour, and a
                                    // tooltip still saying *"the nearest frame's"*
                                    // would read as the setting having failed.
                                    (ExportBackground::Frame, _) if sizing.is_none() => {
                                        "Each layer's own nearest frame — no single colour to show"
                                    }
                                    (ExportBackground::Frame, _) => {
                                        "The nearest frame's background, or nothing if it has none"
                                    }
                                    _ => "What fills the parts the layer does not cover",
                                });
                            if bg != spec.background {
                                edit(&mut out, spec).background = bg;
                            }
                        });
                    });

                    // --- the two switches --------------------------------------
                    //
                    // **At the card's own section gap, not a tighter one.** They
                    // had a scope of their own on the reasoning that two switches
                    // are one section — but they carry no shared label, so nothing
                    // on screen said which section they were, and the tighter pitch
                    // read as the pair being crowded against a card that was
                    // otherwise evenly spaced. Asked for directly: the same
                    // distance between them as between the first of them and the
                    // Background dropdown above.
                    {
                        let size = egui::vec2(POP_INNER, POP_CELL);
                        let trim = ui::toggle_row(
                            ui,
                            icon::CROP,
                            "Trim transparent edges",
                            trim_button_state(spec),
                            size,
                        )
                        .on_hover_text(match spec.format.is_raster() {
                            true => "Drop the fully transparent rows and columns",
                            false => "Trimming reads pixels, and SVG has none",
                        });
                        if trim.clicked() && spec.format.is_raster() {
                            edit(&mut out, spec).trim = !spec.trim;
                        }
                        let pad = ui::toggle_row(
                            ui,
                            icon::SQUARE,
                            "Pad out to a square",
                            pad_button_state(spec),
                            size,
                        )
                        .on_hover_text("Centre the result in a square of its longer side");
                        if pad.clicked() {
                            edit(&mut out, spec).pad_square = !spec.pad_square;
                        }
                    }
                })
            })
            .response;

        // The panel's one click-away rule (`panels::dismissed_by_click`), whole
        // rather than hand-rolled — all but one of its terms was paid for by a
        // report (§15 D82; `panels::mod`'s dismissal comment keeps the count), and
        // a popover re-deriving three of them is how the next one comes back.
        let over_overlay = ctx
            .pointer_interact_pos()
            .and_then(|p| ctx.layer_id_at(p))
            .is_some_and(|l| l.order >= egui::Order::Foreground);
        let on_picker = ctx
            .pointer_interact_pos()
            .and_then(|p| ctx.layer_id_at(p))
            .is_some_and(|l| l.id == super::picker::layer_id());
        // Recorded every frame, never from inside the short-circuiting `||`.
        let press_away = super::press_began_away(&ctx, menu.layer_id.id, menu.rect, head.rect);
        if close
            || super::dismissed_by_click(super::ClickAway {
                // The primary button only: this card holds a scrubbable field (the
                // JPEG quality), a scrub is cancelled with a right-click wherever
                // the pointer has been dragged to, and read as `any_click` that
                // cancel is also a click on the page.
                clicked: ctx.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary)),
                over_overlay,
                on_menu: menu.contains_pointer(),
                on_head: head.contains_pointer(),
                on_picker,
                in_gesture: ctx.dragged_id().is_some() || self.gesture_cancelled,
                press_away,
            })
        {
            self.export_row_open = None;
            self.export_prefix_text = None;
            self.export_suffix_text = None;
        }
        out
    }

    /// The JPEG quality field. `Some` on the frame the edit is finished — never
    /// while the pointer still holds it.
    ///
    /// **A hand-wound scrub, because neither shared committer works here.** A
    /// value field normally goes through `edit_valve`, which previews every frame
    /// and commits once on release; but `RenderOverrides` absorbs `SetExports` as
    /// a no-op by design, so there is nothing to preview — the field re-read the
    /// committed quality on every frame of the drag and the number would not move
    /// at all. `commit_run` is the other end and gives up the opposite half: it
    /// commits every frame, and `cancel_gesture` undoes a scrub by *dropping the
    /// preview*, so a right-click left the scrubbed value standing. Both were
    /// shipped and both were reported.
    ///
    /// So the in-flight value lives here instead, in `export_quality_scrub`: the
    /// field reads it while the drag is on, the release commits it once, and a
    /// cancel drops it — which is `session_scrub`'s arrangement for the one other
    /// scrub in the app that previews through something other than the overrides.
    ///
    /// **The cancel is checked before the buffer is read, not after.** On the frame
    /// a gesture is called off, egui may still report a `changed()` from the motion
    /// that frame; reading the buffer first and clearing it after would commit that
    /// last movement, which is the whole of what the cancel is for.
    fn export_quality_field(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        spec: &ExportSpec,
    ) -> Option<u8> {
        let cancelled = self.gesture_cancelled;
        if cancelled {
            self.in_flight.export_quality_scrub = None;
        }
        let held = match self.in_flight.export_quality_scrub {
            Some((i, v)) if i == index && !cancelled => Some(v),
            _ => None,
        };
        let mut q = held.unwrap_or(spec.quality as f64);
        let resp = value_field(
            ui,
            egui::vec2(POP_INNER, POP_CELL),
            Prefix::Icon(icon::GAUGE),
            &mut q,
            // **0.25 — a quarter of what the panel's other 0–100 fields ask for,
            // and deliberately.** It shipped at 1.0, which crossed the whole range
            // in 200pt ("0 to 100 in about 250px"); 0.5 matched opacity, which is
            // the consistent answer and still too fast for this one. A quality is
            // not a percentage of anything the eye is watching — nothing moves as
            // you drag it — so the useful travel is the band between about 60 and
            // 95, and a rate that crosses 1 to 100 in 800pt puts that band under a
            // hand's width. The number was set by feel, on the machine, which is
            // the only way a scrub rate ever is.
            Scrub::whole(0.25).range(1.0..=100.0),
            |d| d.custom_formatter(ui::number(0)),
        )
        .on_hover_text("Lower is a smaller file. 90 unless you have a reason");
        match quality_step(
            ScrubInput {
                cancelled,
                dragged: resp.dragged(),
                changed: resp.changed(),
                was_held: held.is_some(),
            },
            q,
            spec.quality,
        ) {
            QualityStep::Hold => {
                self.in_flight.export_quality_scrub = Some((index, q));
                None
            }
            QualityStep::Commit(v) => {
                self.in_flight.export_quality_scrub = None;
                Some(v)
            }
            QualityStep::Nothing => None,
        }
    }

    /// One of the two name fields in the settings card, and the buffering both
    /// need. `Some` on the frame the edit is finished.
    ///
    /// **One function for the pair**, because they differ in three strings and
    /// nothing else — two copies of this would be two places for the
    /// commit-on-blur rule to drift.
    fn name_part_field(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        part: NamePart,
        current: &str,
        hint: &str,
        tip: &'static str,
    ) -> Option<String> {
        let buffer = match part {
            NamePart::Prefix => &mut self.export_prefix_text,
            NamePart::Suffix => &mut self.export_suffix_text,
        };
        let editing = matches!(buffer, Some((i, _)) if *i == index);
        let mut text = match buffer {
            Some((i, t)) if *i == index => t.clone(),
            _ => current.to_string(),
        };
        let mut out = None;
        section(ui, part.label(), |ui| {
            let resp = ui::field_row(ui, egui::vec2(POP_INNER, POP_CELL), |ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                ui.label(theme::icon_text(icon::TEXT_T, 13.0, theme::text::FAINT));
                ui.add(
                    egui::TextEdit::singleline(&mut text)
                        .id(egui::Id::new((part.id_salt(), index)))
                        .desired_width(ui.available_width())
                        .frame(egui::Frame::NONE)
                        .hint_text(hint)
                        .font(egui::FontId::proportional(11.5)),
                )
            });
            if resp.gained_focus() || resp.changed() {
                *match part {
                    NamePart::Prefix => &mut self.export_prefix_text,
                    NamePart::Suffix => &mut self.export_suffix_text,
                } = Some((index, text.clone()));
            }
            // **Committed on losing focus, not per keystroke**, which is
            // `stroke_dash_text`'s reason: typing `hero/` passes through `h`, `he`,
            // `her` — four undo steps for one name.
            if editing && resp.lost_focus() {
                *match part {
                    NamePart::Prefix => &mut self.export_prefix_text,
                    NamePart::Suffix => &mut self.export_suffix_text,
                } = None;
                if text != current {
                    out = Some(text.clone());
                }
            }
            resp.on_hover_text(tip);
        });
        out
    }

    /// What the background dropdown reads — the *effective* value, so a JPEG says
    /// white rather than claiming a transparency it cannot carry.
    ///
    /// The stored setting is untouched by this: switching a JPEG back to PNG shows
    /// Transparent again, because that is what the file has always said.
    fn background_label(&self, spec: &ExportSpec) -> &'static str {
        match spec.background {
            ExportBackground::Solid(_) => "White",
            b => b.label(),
        }
    }

    /// The colour of the chip in front of that label — the colour that will
    /// actually be behind the export.
    ///
    /// **Resolved through `plan::background_color`, not read off the setting**,
    /// which is what makes it worth drawing at all: *Frame's own* is a different
    /// colour on every layer and no colour at all on one outside a frame, and a
    /// chip that showed a fixed grey for it would be a swatch of nothing. A
    /// transparent export gets the muted ink, which is this panel's word for
    /// "there is nothing here" rather than a colour claim.
    ///
    /// **Not the design's checkerboard.** That is the right drawing for
    /// transparency and it is a tiled pattern rather than a glyph, so it wants a
    /// painter and a rect where this has a `WidgetText`; a chip is what a
    /// `ComboBox`'s selected text can carry today.
    fn background_swatch(&self, spec: &ExportSpec, sizing: Option<NodeId>) -> egui::Color32 {
        // ⚠️ **Blank over a multi-selection rather than a guess** (§15 D657,
        // `[S8.3-L2-06]`), which is the panel's own rule stated twice in this file
        // and broken here. `export_size_label` one control away: *"`Width(512)` is
        // a different multiplier on every differently-shaped layer, so one number
        // for thirty icons would be true of at most one of them — and the row that
        // carries it is the same row for all thirty."* This resolved the chip from
        // `export_subjects().first()`, so two rects in two frames — one white, one
        // black — painted **white** and exported one of each, with the tooltip
        // reading *"the nearest frame's background"*, singular, and nothing on
        // screen saying the answer was per-layer.
        //
        // **Only `Frame` varies per layer**, which is why this is a `match` and not
        // a gate on `sizing`: `Solid` and `Transparent` are properties of the spec,
        // so they are exactly as true of thirty layers as of one and the chip
        // should keep showing them.
        let resolved = match (spec.background, sizing) {
            (ExportBackground::Frame, None) => None,
            (_, subject) => subject
                .or_else(|| self.export_subjects().first().copied())
                .and_then(|id| ondin_export::plan::background_color(&self.session.doc, id, spec)),
        };
        match resolved {
            Some(c) => {
                let [r, g, b, a] = c.to_rgba8().to_u8_array();
                egui::Color32::from_rgba_unmultiplied(r, g, b, a)
            }
            None => theme::text::DIM,
        }
    }

    /// The one or two buttons under the list.
    ///
    /// **The zip button appears exactly when there is more than one file** —
    /// several layers, or one layer with several specs — because a zip of one file
    /// is a thing you have to unpack to get back what you already had. That is
    /// also the boundary the destination dialog turns on: one file gets a save
    /// dialog with a name in it, several get a folder.
    fn export_buttons(&mut self, ui: &mut egui::Ui, subjects: &[NodeId]) {
        let files = self.export_plan(subjects);
        if files.is_empty() {
            // **The menu is drawn by this function, so it has to be closed by it.**
            // Anything that stops the button row appearing — a collapsed panel, a
            // selection with nothing to export — would otherwise leave the popover
            // flagged open with nothing on screen to dismiss it, and it would come
            // back the next time the row did.
            self.export_menu = false;
            return;
        }
        // No `add_space` before it: `ui::CARD_ROW_GAP` is already the distance
        // every child of a card sits from the next, and the design's export card
        // puts the button row at exactly that. A hand-rolled gap here is the same
        // number written twice, which is how a panel comes to disagree with its
        // neighbours by a point (§15 D180).
        let label = match (subjects.len(), self.session.doc.get(subjects[0])) {
            // **The layer's name, where the mock draws "Export selection".** The
            // static label is what a mock can say; naming the subject is what makes
            // the button worth reading before pressing it, and it is the thing the
            // report singled out Figma for getting right.
            (1, Some(n)) => format!("Export {}", n.name()),
            (n, _) => format!("Export {n} layers"),
        };
        let mut go = false;
        let mut head = None;
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = PITCH;
            let w = ui.available_width() - CELL - PITCH;
            go = ui::action_button(
                ui,
                icon::EXPORT,
                &label,
                ui::FieldButton::Off,
                egui::vec2(w, CELL),
            )
            // **The name again, because the label is the one place in the panel
            // that prints text the *document* supplies** — and `action_button`
            // elides it, so `Export Ellipse 11312312 3123123132` says
            // `Export Ellipse 1131…` and the button stops naming what it acts on.
            // The tooltip said only the file count, so the name was recoverable
            // nowhere: this is the affordance that was already there being asked
            // to carry the half it was missing, rather than a wider card.
            //
            // Only in the single-layer arm. The other label is `Export 3 layers`,
            // which this module wrote and which cannot elide, so repeating it
            // would be noise on the common case.
            .on_hover_text(match (subjects.len(), files.len()) {
                (1, 1) => format!("{label} — 1 file"),
                (1, n) => format!("{label} — {n} files, into a folder you pick"),
                (_, 1) => "1 file".to_string(),
                (_, n) => format!("{n} files, into a folder you pick"),
            })
            .clicked();
            let more = ui::field_button(
                ui,
                icon::DOTS_THREE,
                CELL,
                15.0,
                ui::FieldButton::on_if(self.export_menu),
            )
            .on_hover_text("Presets, and what an export does");
            if more.clicked() {
                self.export_menu = !self.export_menu;
                // The other half of the exclusivity rule — see `export_row`.
                self.export_row_open = None;
                self.export_prefix_text = None;
                self.export_suffix_text = None;
            }
            head = Some(more);
        });
        if go {
            self.run_export(files.clone(), false);
        }
        if files.len() > 1 {
            let w = ui.available_width();
            if ui::action_button(
                ui,
                icon::FILE_ZIP,
                "Export as Zip",
                ui::FieldButton::Off,
                egui::vec2(w, CELL),
            )
            .on_hover_text(format!("{} files in one archive", files.len()))
            .clicked()
            {
                self.run_export(files, true);
            }
        }
        if let Some(head) = head {
            self.export_menu_popup(ui, &head, subjects);
        }
        // **Escape closes both of this card's popovers** (§15 D801). They read no
        // key at all until 2026-09-19, which is §15 D527's failure shape in the
        // one region nobody had stood in: measured on a headless app, `Escape`
        // left the popover up **and** paid out a rung of the ladder behind it, so
        // the press did the one thing it should not and none of the thing it
        // should. `key_pressed` rather than `consume_key`, matching the three
        // inspector popovers — `OndinApp::a_popover_owns_escape` is what stops the
        // ladder seeing it, so consuming here would make the same claim twice and
        // in the place that is harder to find.
        if ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
            self.export_menu = false;
            self.export_row_open = None;
        }
    }

    /// The panel's own menu: what to do with a set of settings, and the two
    /// switches that change what *every* export does.
    ///
    /// **A popover, and so is its neighbour** — the difference between this and
    /// [`Self::export_settings_popup`] is what each *holds*, not what shape it
    /// takes. That one carries the values belonging to one row; this holds verbs
    /// and preferences that belong to no row at all, and putting eleven of them in
    /// the card would bury the button they sit under.
    ///
    /// ⚠️ **This paragraph said the opposite until 2026-08-22**, calling itself "a
    /// popover rather than rows in the card, which is the opposite of the call
    /// `Self::export_settings_block` makes". Both halves were stale: there is no
    /// `export_settings_block`, and the inline block it named became a popover
    /// itself — that function's own doc records the change ("a popover, and it
    /// replaced an inline block"). Found by the rustdoc gate, which is the class of
    /// drift it was added for (§15 D296).
    fn export_menu_popup(&mut self, ui: &mut egui::Ui, head: &egui::Response, subjects: &[NodeId]) {
        if !self.export_menu {
            return;
        }
        const MENU_W: f32 = crate::app::POPOVER_W;
        const MENU_PAD: f32 = 11.0;
        let ctx = ui.ctx().clone();
        let current = self.shared_exports(subjects);
        // Collected and applied after the `Area` closes: every one of these mutates
        // `self`, which is borrowed for the duration of the popup.
        let mut act: Option<MenuAct> = None;

        let anchor = Self::popover_anchor(&ctx, head.rect, MENU_W);
        let menu = egui::Area::new(egui::Id::new("export-menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            .constrain_to(ctx.input(|i| i.content_rect()).shrink(12.0))
            .show(&ctx, |ui| {
                ui::menu_frame(MENU_PAD).show(ui, |ui| {
                    ui.set_width(ui::menu_inner_w(MENU_W, MENU_PAD));
                    ui::menu_rows(ui);

                    if super::inspector::menu_action(
                        ui,
                        self.document_exports_anything(),
                        icon::EXPORT,
                        "Export all in document",
                        &|live| match live {
                            true => "Every layer with export settings, in one go".into(),
                            false => "No layer in this document has export settings".into(),
                        },
                    ) {
                        act = Some(MenuAct::ExportAll);
                    }
                    let has_dir = self.prefs.export_dir.as_ref().is_some_and(|d| d.is_dir());
                    if super::inspector::menu_action(
                        ui,
                        has_dir,
                        icon::FOLDER_OPEN,
                        "Show last export folder",
                        &|live| match live {
                            true => "Open it in the file browser".into(),
                            false => "Nothing has been exported yet".into(),
                        },
                    ) {
                        act = Some(MenuAct::ShowFolder);
                    }
                    super::inspector::popup_rule(ui);

                    // --- moving a set of settings around -----------------------
                    let copyable = current.as_ref().is_some_and(|s| !s.is_empty());
                    if super::inspector::menu_action(
                        ui,
                        copyable,
                        icon::COPY,
                        "Copy export settings",
                        &|live| match live {
                            true => "Take this list to another layer".into(),
                            false => "There is no single list here to copy".into(),
                        },
                    ) {
                        act = Some(MenuAct::Copy);
                    }
                    let held = self.export_clipboard.clone();
                    if super::inspector::menu_action(
                        ui,
                        held.is_some(),
                        icon::CLIPBOARD,
                        "Paste export settings",
                        &|live| match live {
                            true => "Replace this layer's list".into(),
                            false => "No export settings have been copied".into(),
                        },
                    ) {
                        act = Some(MenuAct::Paste);
                    }
                    if super::inspector::menu_action(
                        ui,
                        copyable,
                        icon::FLOPPY_DISK,
                        "Save as a preset",
                        &|live| match live {
                            true => "Keep this list for any document".into(),
                            false => "There is no single list here to save".into(),
                        },
                    ) {
                        act = Some(MenuAct::SavePreset);
                    }

                    // --- the presets themselves --------------------------------
                    if !self.prefs.export_presets.is_empty() {
                        super::inspector::popup_rule(ui);
                        for (i, preset) in self.prefs.export_presets.iter().enumerate() {
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 4.0;
                                let w = ui.available_width() - 18.0 - 4.0;
                                let row = ui.allocate_ui(egui::vec2(w, ui::MENU_ROW_H), |ui| {
                                    ui::menu_item(
                                        ui,
                                        icon::STACK_SIMPLE,
                                        &preset.name,
                                        ui::MENU_ROW_H,
                                    )
                                });
                                if row.inner.clicked() {
                                    act = Some(MenuAct::ApplyPreset(i));
                                }
                                if ui::icon_button(ui, icon::X, 18.0, 12.0, false, true)
                                    .on_hover_text("Forget this preset")
                                    .clicked()
                                {
                                    act = Some(MenuAct::ForgetPreset(i));
                                }
                            });
                        }
                    }
                    super::inspector::popup_rule(ui);

                    // --- what an export does, everywhere -----------------------
                    let folders = self.prefs.export_folders_from_names;
                    if ui::menu_row(
                        ui,
                        // `FOLDER_SIMPLE`, not the `FOLDER_OPEN` two rows above:
                        // that one is a folder being *entered*, this one a folder
                        // being *made*.
                        ui::MenuRow::new(icon::FOLDER_SIMPLE, "Slashes in names make folders")
                            .checked(folders),
                        ui::MENU_ROW_H,
                    )
                    .on_hover_text("A layer called icons/close writes icons/close.png")
                    .clicked()
                    {
                        act = Some(MenuAct::ToggleFolders);
                    }
                    let on_save = self.prefs.export_on_save;
                    if ui::menu_row(
                        ui,
                        // **`ARROW_CLOCKWISE`, the design's, though the glyph is
                        // already the Transform panel's rotation prefix.** This row
                        // is *repetition* rather than rotation, and the two never
                        // appear together — one is a menu in the export card, the
                        // other a field prefix in Transform. What it must not be is
                        // `FLOPPY_DISK`, which is *Save as a preset* four rows above
                        // it in this same menu.
                        ui::MenuRow::new(icon::ARROW_CLOCKWISE, "Re-export on save")
                            .checked(on_save),
                        ui::MENU_ROW_H,
                    )
                    .on_hover_text("Every Ctrl+S rewrites the whole export set")
                    .clicked()
                    {
                        act = Some(MenuAct::ToggleOnSave);
                    }
                })
            })
            .response;

        // The panel's one click-away rule (`panels::dismissed_by_click`). No
        // scrubbable field in here, so the primary-button term is the only one that
        // needed thought — and it is the same answer every other inspector popover
        // gives, which is the point of the rule being shared.
        let over_overlay = ctx
            .pointer_interact_pos()
            .and_then(|p| ctx.layer_id_at(p))
            .is_some_and(|l| l.order >= egui::Order::Foreground);
        let on_picker = ctx
            .pointer_interact_pos()
            .and_then(|p| ctx.layer_id_at(p))
            .is_some_and(|l| l.id == super::picker::layer_id());
        // Recorded every frame, never from inside the short-circuiting `||`.
        let press_away = super::press_began_away(&ctx, menu.layer_id.id, menu.rect, head.rect);
        // **A verb closes it, a switch does not.** Every row above the last rule is
        // an action that takes effect at once, so there is nothing left to look at;
        // the two switches at the bottom are settings, and closing the card under
        // the hand after one of them is what makes turning both a two-trip job.
        if act.is_some_and(MenuAct::closes)
            || super::dismissed_by_click(super::ClickAway {
                clicked: ctx.input(|i| i.pointer.button_clicked(egui::PointerButton::Primary)),
                over_overlay,
                on_menu: menu.contains_pointer(),
                on_head: head.contains_pointer(),
                on_picker,
                in_gesture: ctx.dragged_id().is_some() || self.gesture_cancelled,
                press_away,
            })
        {
            self.export_menu = false;
        }
        if let Some(act) = act {
            self.perform_export_menu(act, subjects, current);
        }
    }

    /// Carry out what the menu asked for, once its borrow of `self` is over.
    fn perform_export_menu(
        &mut self,
        act: MenuAct,
        subjects: &[NodeId],
        current: Option<Vec<ExportSpec>>,
    ) {
        match act {
            MenuAct::ExportAll => self.export_all(ExportAll::Asking),
            MenuAct::ShowFolder => {
                if let Some(dir) = self.prefs.export_dir.clone() {
                    super::show_in_file_browser(&dir);
                }
            }
            MenuAct::Copy => {
                if let Some(specs) = current {
                    let n = specs.len();
                    self.export_clipboard = Some(specs);
                    self.session.info(format!("Copied {n} export settings"));
                }
            }
            MenuAct::Paste => {
                if let Some(specs) = self.export_clipboard.clone() {
                    self.write_exports(subjects, &specs);
                }
            }
            MenuAct::SavePreset => {
                if let Some(specs) = current {
                    // **Named from what it holds, so nothing has to be typed.** A
                    // dialog asking for a name is the obvious design and it is a
                    // modal in the middle of a two-click action; "PNG 1x, 2x" is
                    // what the user would have typed anyway, and a second preset
                    // with the same shape is the same preset.
                    let name = preset_name(&specs);
                    if let Some(slot) = self
                        .prefs
                        .export_presets
                        .iter()
                        .position(|p| p.name == name)
                    {
                        self.prefs.export_presets[slot].specs = specs;
                    } else {
                        self.prefs.export_presets.push(crate::prefs::ExportPreset {
                            name: name.clone(),
                            specs,
                        });
                    }
                    self.prefs.save();
                    self.session.info(format!("Saved the preset “{name}”"));
                }
            }
            MenuAct::ApplyPreset(i) => {
                if let Some(preset) = self.prefs.export_presets.get(i).cloned() {
                    self.write_exports(subjects, &preset.specs);
                }
            }
            MenuAct::ForgetPreset(i) => {
                if i < self.prefs.export_presets.len() {
                    let gone = self.prefs.export_presets.remove(i);
                    self.prefs.save();
                    self.session.info(format!("Forgot “{}”", gone.name));
                }
            }
            MenuAct::ToggleFolders => {
                self.prefs.export_folders_from_names = !self.prefs.export_folders_from_names;
                self.prefs.save();
            }
            MenuAct::ToggleOnSave => {
                self.prefs.export_on_save = !self.prefs.export_on_save;
                self.prefs.save();
            }
        }
    }

    /// What these subjects would produce, under the current folder rule.
    pub(crate) fn export_plan(&self, subjects: &[NodeId]) -> Vec<PlannedFile> {
        ondin_export::plan(
            &self.session.doc,
            &self.session.resolved,
            subjects,
            PlanOptions {
                folders_from_names: self.prefs.export_folders_from_names,
                // The panel is where every row's own scale is set, so there is
                // nothing here to override — the flag exists for the CLI, which has
                // no rows (§15 D361).
                scale_override: None,
            },
        )
    }

    /// Render a plan and write it — to a file, into a folder, or into one archive.
    ///
    /// **Three destinations, one boundary.** A plan of one file gets a save dialog
    /// with the name already in it, because that is the only case where the user
    /// has a filename to choose; anything larger gets a folder, because naming
    /// each of nine files by hand is not a thing anyone wants to be offered. The
    /// zip is the third and is asked for explicitly — its whole reason is that
    /// *not* zipping is the default here, where Figma has no other option.
    ///
    /// **The committed document and `Resolved`**, as every writer here is: they are
    /// pure functions of the pair, so a gesture mid-drag cannot leak an uncommitted
    /// position into a file. *Export as…* was written the same way and is gone; the
    /// property belongs to the writers rather than to either door (§15 D264).
    pub(crate) fn run_export(&mut self, files: Vec<PlannedFile>, zip: bool) {
        if files.is_empty() {
            return;
        }
        let start = self.export_start_dir();
        let destination = match export_dialog(zip, files.len()) {
            DialogKind::Archive => rfd::FileDialog::new()
                .set_directory(&start)
                .add_filter("ZIP archive", &["zip"])
                .set_file_name(self.export_archive_name())
                .save_file()
                .map(Destination::Archive),
            DialogKind::SaveFile => {
                // **A planned path is not a file name** (§15 D656,
                // `[S8.3-L1-10]`). With *Slashes in names make folders* on, a
                // layer called `icons/close` plans `icons/close.png` — the `/` is
                // the feature — and this handed that whole string to
                // `set_file_name`, where the API wants a name. The folder half is
                // a *location*, so it goes to `set_directory`.
                let (dir, name) = save_dialog_target(&start, &files[0].path);
                rfd::FileDialog::new()
                    .set_directory(dir)
                    .add_filter(
                        files[0].spec.format.label(),
                        &[files[0].spec.format.extension()],
                    )
                    .set_file_name(name)
                    .save_file()
                    .map(Destination::File)
            }
            DialogKind::PickFolder => rfd::FileDialog::new()
                .set_directory(&start)
                .pick_folder()
                .map(Destination::Folder),
        };
        let Some(destination) = destination else {
            return;
        };
        self.write_export(files, destination);
    }

    /// Render and write, reporting what happened.
    ///
    /// **A failure stops nothing that has already succeeded**, and is counted
    /// rather than raised: half of an export set on disk with a message naming the
    /// one that failed is more use than the whole run abandoned at file three,
    /// where nothing says which three landed.
    fn write_export(&mut self, files: Vec<PlannedFile>, destination: Destination) {
        let mut rendered: Vec<(String, Vec<u8>)> = Vec::with_capacity(files.len());
        let mut skipped = 0usize;
        for f in &files {
            match ondin_export::plan::render(&self.session.doc, &self.session.resolved, f) {
                Some(bytes) => rendered.push((f.path.clone(), bytes)),
                // The one refusal both writers make: a subject with no bounds at
                // all — an empty group, a layer whose contents are all hidden.
                None => skipped += 1,
            }
        }
        if rendered.is_empty() {
            self.session
                .fail("Nothing to export — none of that has any size");
            return;
        }
        // **What the folder arm owes and the other two do not** (§15 D636,
        // `[S8.3-L5-05]`). A *File* or an *Archive* was named in a save dialog, so
        // the user has just been shown where it goes and warned by the OS if
        // something is there. A *Folder* export can arrive with no dialog at all —
        // `export_all`'s `(Some(dir), _)` arm takes `prefs.export_dir`, which is
        // one global folder set by *any* previous export, and *Re-export on save*
        // fires it on an **autosave**. Every same-named file in it is replaced by a
        // bare `fs::write`, and the message said `Exported 1 file` and named
        // nothing. *"No dialog when there is somewhere to put it"* is the feature;
        // *"no dialog" never means "no idea"* is what this function's own door
        // promises, and the two numbers below are what makes that true.
        let mut replaced = 0usize;
        let mut to_folder = false;
        let (written, failed, where_): (usize, Option<String>, std::path::PathBuf) =
            match destination {
                Destination::File(path) => {
                    let dir = path.parent().map(Into::into).unwrap_or_default();
                    // **The same line the folder arm has** (§15 D656,
                    // `[S8.3-L1-10]`), and the two arms differed by exactly it:
                    // a layer named `icons/close` exported fine as one of
                    // several and failed with *"the system cannot find the path
                    // specified"* on its own, so whether an export worked
                    // depended on how many **other** layers were selected. Every
                    // path this arm is given has just come out of a save dialog,
                    // so normally the parent exists and this is a no-op; it is
                    // not free of doubt, because the dialog was handed a name
                    // that may itself carry a folder.
                    if let Some(parent) = path.parent() {
                        let _ = std::fs::create_dir_all(parent);
                    }
                    match std::fs::write(&path, &rendered[0].1) {
                        Ok(()) => (1, None, dir),
                        Err(e) => (0, Some(e.to_string()), dir),
                    }
                }
                Destination::Archive(path) => {
                    let dir = path.parent().map(Into::into).unwrap_or_default();
                    let packed = ondin_export::plan::zip(&rendered)
                        .and_then(|bytes| std::fs::write(&path, bytes));
                    match packed {
                        Ok(()) => (rendered.len(), None, dir),
                        Err(e) => (0, Some(e.to_string()), dir),
                    }
                }
                Destination::Folder(dir) => {
                    let (mut ok, mut err) = (0, None);
                    for (name, bytes) in &rendered {
                        let path = dir.join(name);
                        // The `/` in a name is a folder the user asked for, and it
                        // may not exist yet. Created here rather than by the plan,
                        // which never touches the filesystem.
                        if let Some(parent) = path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        // **Asked before the write, which is the whole of it**:
                        // afterwards every one of these exists (§15 D636).
                        let existed = path.exists();
                        match std::fs::write(&path, bytes) {
                            Ok(()) => {
                                ok += 1;
                                if existed {
                                    replaced += 1;
                                }
                            }
                            Err(e) => err = err.or(Some(e.to_string())),
                        }
                    }
                    to_folder = true;
                    (ok, err, dir)
                }
            };

        // Remembered for the next export, and for *Show in folder*. Saved eagerly:
        // the app has no shutdown hook to rely on.
        if written > 0 {
            self.prefs.export_dir = Some(where_.clone());
            self.prefs.save();
        }
        // **The count first, the loss next, the path last**, because
        // `app::status_label` draws this with `.truncate()`: whatever falls off the
        // end is what the reader can most afford to lose, and a folder they can
        // re-derive from *Show in folder* beats a replacement count they cannot.
        let head = match (to_folder, replaced) {
            (false, _) => export_count(written),
            (true, 0) => export_count(written),
            (true, n) => format!("{} ({n} replaced)", export_count(written)),
        };
        let head = match skipped {
            0 => head,
            n => format!("{head} — {n} had no size"),
        };
        match failed {
            Some(e) => self.session.fail(format!("Could not write: {e}")),
            None if to_folder => self.session.info(format!("{head} → {}", where_.display())),
            None => self.session.info(head),
        }
    }

    /// Whether anything in the document is set up to export — what dims *Export
    /// all* on the page menu.
    ///
    /// **Stops at the first one**, unlike [`Self::exportable_layers`] which has to
    /// find them all: the menu is rebuilt every frame it is open, and this is the
    /// only question it needs answered.
    pub(crate) fn document_exports_anything(&self) -> bool {
        let mut stack = vec![self.session.doc.root()];
        while let Some(id) = stack.pop() {
            let Some(node) = self.session.doc.get(id) else {
                continue;
            };
            if !node.exports().is_empty() {
                return true;
            }
            stack.extend(node.children().iter().copied());
        }
        false
    }

    /// Every layer in the document that is set up to export, in document order.
    ///
    /// **The whole document, not the selection**, which is what makes *Export all*
    /// worth having: an asset set is spread over frames nobody wants to select
    /// first, and the export settings on the layers already say which those are.
    pub(crate) fn exportable_layers(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack = vec![self.session.doc.root()];
        while let Some(id) = stack.pop() {
            let Some(node) = self.session.doc.get(id) else {
                continue;
            };
            if !node.exports().is_empty() {
                out.push(id);
            }
            // ⚠️ **This walk's order does not matter and there used to be a
            // `.rev()` here claiming it did** (§15 D649, `[A7-L8-02]`): *"reversed,
            // so the traversal comes out in document order … which nothing
            // downstream would notice today and everything would inherit
            // tomorrow"*. Both halves were false. `all_in_document_order` below
            // collects its input into an `FxHashSet` on its first line and then
            // walks the document from the root, so this order is **discarded**,
            // and nothing inherits it today or tomorrow because the sort is
            // unconditional. The danger was not the redundant call — it was a
            // reader taking the *pattern* ("a `.rev()` on a `children()` extend is
            // what makes a stack walk come out in document order") somewhere no
            // sort follows. [`Self::document_exports_anything`] above writes the
            // same walk without one, for the same reason.
            stack.extend(node.children().iter().copied());
        }
        // **`all_in_document_order`, not `in_document_order`** (§15 D516,
        // `[S8.3-L1-02]`). The latter starts with `outermost`, which drops any id
        // that has an ancestor in the set — right for the *selection* door, where
        // a group and its child both picked would plan the child twice, and wrong
        // here: an export spec on a nested layer is a deliberate instruction, and
        // this function's own doc one line up says *every* layer in the document.
        // Measured: a `Group` exporting PNG with a `Rect` inside it exporting PNG
        // planned **one** file, and nothing said so.
        ondin_core::build::all_in_document_order(&self.session.doc, &out)
    }

    /// Re-run every export in the document, into the folder the last one went to.
    ///
    /// **No dialog when there is somewhere to put it**, which is the whole point:
    /// Figma stores the settings and still asks where every single time, so
    /// regenerating an asset set is a dialog and a folder walk rather than a
    /// keystroke. The first run of a session asks; every one after it does not,
    /// and the status line says where the files went so "no dialog" never means
    /// "no idea".
    pub(crate) fn export_all(&mut self, how: ExportAll) {
        let subjects = self.exportable_layers();
        if subjects.is_empty() {
            if how == ExportAll::Asking {
                self.session
                    .info("No layer in this document has export settings");
            }
            return;
        }
        let known = self.prefs.export_dir.clone().filter(|d| d.is_dir());
        let files = self.export_plan(&subjects);
        match export_destination(known, how) {
            ExportRoute::Write(dir) => self.write_export(files, Destination::Folder(dir)),
            ExportRoute::SayNoDestination => self
                .session
                .info("Re-export on save: no destination yet — export once to set one"),
            ExportRoute::AskForFolder => {
                let start = self.export_start_dir();
                if let Some(dir) = rfd::FileDialog::new().set_directory(&start).pick_folder() {
                    self.write_export(files, Destination::Folder(dir));
                }
            }
        }
    }

    /// Where a dialog opens: the last export's folder, else the document's own,
    /// else wherever the OS would have chosen.
    fn export_start_dir(&self) -> std::path::PathBuf {
        self.prefs
            .export_dir
            .clone()
            .filter(|d| d.is_dir())
            .or_else(|| {
                self.session
                    .path
                    .as_ref()
                    .and_then(|p| p.parent())
                    .map(Into::into)
            })
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_default())
    }

    /// What the zip is called by default — the document's name, or the layer's
    /// when a single one is being exported.
    fn export_archive_name(&self) -> String {
        let stem = self
            .session
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| "Exports".into());
        format!("{stem}.zip")
    }
}

/// What *Export all* does when it has never been told where to put things.
///
/// **Two callers, two answers, and the difference is who asked.** A menu row or a
/// chord is a person asking for an export, so a folder dialog is the right way to
/// find out where; a save with the switch on is not, and a modal appearing on
/// Ctrl+S because a preference is set is the thing that would make the switch not
/// worth having.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum ExportAll {
    /// Prompt for a folder if none is remembered.
    Asking,
    /// Do nothing but say so.
    Quietly,
}

/// Where an *Export all* is going, before anything opens a dialog or writes a
/// file — [`OndinApp::export_all`]'s three-arm table, lifted.
///
/// 🚨 **The decision it carries is *"Export on save must never open a modal"*,
/// and it was fused to `rfd::FileDialog` and `std::fs::write`** (§15 D626,
/// `[A5-L6-03]`). `export_all`'s own doc states the rule: *"a modal appearing on
/// `Ctrl+S` because a preference is set is the thing that would make the switch
/// not worth having."* Nothing tested it. Collapse the `Quietly` arm into the
/// `Asking` one — or drop the `.filter(|d| d.is_dir())` so a deleted folder falls
/// through — and every `Ctrl+S` with the preference on pops a native folder
/// dialog, with the whole workspace green.
///
/// ⚠️ **`OndinApp::headless` does not dissolve this and §15 D303 says so in
/// terms**: *"a headless app makes the method callable without making the call
/// site observable … this is a door, not a licence."* Worse here — even a
/// headless call would open a **real dialog** and write **real files**, so the
/// decision cannot be observed at all until it is separated from the acting.
/// §15 D269's prescribed lift, and `panels/export.rs` was the largest genuinely
/// under-covered module in the workspace at 20.81%.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExportRoute {
    /// Straight to the remembered folder, whoever asked.
    Write(std::path::PathBuf),
    /// One sentence and nothing else — the silent arm, and the whole reason this
    /// table exists.
    SayNoDestination,
    /// Open the folder picker.
    AskForFolder,
}

/// Which dialog an explicit *Export* opens: [`OndinApp::run_export`]'s
/// `(zip, files.len())` table, lifted for [`ExportRoute`]'s reason (§15 D626).
///
/// **The `files.len() == 1` boundary is the interesting one.** One file is a
/// *Save as* with the file's own name and format filter already filled in; two is
/// a folder pick, because there is no one name to offer. A `zip` overrides both,
/// which is why it is matched first.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DialogKind {
    Archive,
    SaveFile,
    PickFolder,
}

/// Where an *Export all* goes. See [`ExportRoute`].
pub(crate) fn export_destination(
    remembered: Option<std::path::PathBuf>,
    how: ExportAll,
) -> ExportRoute {
    match (remembered, how) {
        (Some(dir), _) => ExportRoute::Write(dir),
        // **Silent rather than modal**, which is the whole difference between the
        // two callers: a save that opened a folder dialog because the switch
        // happens to be on would be the surprise the switch's own doc comment says
        // it must not be. It says so once and the next explicit export teaches it
        // where.
        (None, ExportAll::Quietly) => ExportRoute::SayNoDestination,
        (None, ExportAll::Asking) => ExportRoute::AskForFolder,
    }
}

/// Where the one-file save dialog opens and what it proposes calling the file —
/// splitting a **planned path** into the two things the dialog API actually wants
/// (§15 D656, `[S8.3-L1-10]`).
///
/// A planned path may carry a `/`: with *Slashes in names make folders* on, a
/// layer called `icons/close` plans `icons/close.png` and the separator is the
/// feature (§15 D361's neighbour). `set_file_name` was given that whole string,
/// which is a relative *path* where the API wants a name — so the folder half
/// belongs in `set_directory` instead.
///
/// ⚠️ **The joined directory is used only if it already exists**, which is what
/// keeps this deterministic. Platform dialogs differ on a `set_directory` that
/// does not resolve — some fall back to the last-used folder, some to the home
/// directory — and the export's own remembered folder is a better answer than
/// whichever of those the machine picks. The folder is created on the write side
/// regardless, so nothing is lost by starting one level up.
///
/// **Lifted out of `run_export` because that function opens a modal**, which no
/// test can drive (§15 D269's shape). The arithmetic is the half worth asserting.
fn save_dialog_target(start: &std::path::Path, planned: &str) -> (std::path::PathBuf, String) {
    let p = std::path::Path::new(planned);
    let name = p
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_else(|| planned.to_string());
    match p.parent().filter(|d| !d.as_os_str().is_empty()) {
        Some(sub) => {
            let joined = start.join(sub);
            match joined.is_dir() {
                true => (joined, name),
                false => (start.to_path_buf(), name),
            }
        }
        None => (start.to_path_buf(), name),
    }
}

/// Which dialog an explicit export opens. See [`DialogKind`].
pub(crate) fn export_dialog(zip: bool, files: usize) -> DialogKind {
    match (zip, files) {
        (true, _) => DialogKind::Archive,
        (false, 1) => DialogKind::SaveFile,
        (false, _) => DialogKind::PickFolder,
    }
}

/// What the panel menu asked for, deferred until its borrow of `self` is over.
#[derive(Clone, Copy)]
enum MenuAct {
    ExportAll,
    ShowFolder,
    Copy,
    Paste,
    SavePreset,
    ApplyPreset(usize),
    ForgetPreset(usize),
    ToggleFolders,
    ToggleOnSave,
}

impl MenuAct {
    /// Whether taking this one puts the menu away — true of every verb, false of
    /// the two switches, which are settings you may well want both of.
    fn closes(self) -> bool {
        !matches!(self, MenuAct::ToggleFolders | MenuAct::ToggleOnSave)
    }
}

/// A preset's name, built from what it holds.
///
/// `PNG 1x, 2x` · `PNG 2x + SVG`. Grouped by format so the common case — one
/// format at several sizes — reads as one thing rather than as a list of pairs.
fn preset_name(specs: &[ExportSpec]) -> String {
    let mut parts: Vec<String> = Vec::new();
    for f in ExportFormat::ALL {
        let scales: Vec<String> = specs
            .iter()
            .filter(|s| s.format == f)
            .map(|s| s.scale.label())
            .collect();
        if scales.is_empty() {
            continue;
        }
        parts.push(match f.is_raster() {
            true => format!("{} {}", f.label(), scales.join(", ")),
            // A vector file has no size, so listing one would be naming a setting
            // the format ignores — the same thing the row's dimmed scale says.
            false => f.label().to_string(),
        });
    }
    match parts.is_empty() {
        true => "Empty".to_string(),
        false => parts.join(" + "),
    }
}

/// Where a finished export goes.
enum Destination {
    /// One named file.
    File(std::path::PathBuf),
    /// A folder, with one file written into it per plan entry.
    Folder(std::path::PathBuf),
    /// One archive holding the lot.
    Archive(std::path::PathBuf),
}

fn export_count(n: usize) -> String {
    match n {
        1 => "Exported 1 file".to_string(),
        n => format!("Exported {n} files"),
    }
}

/// The block's controls all edit *one* spec, and each of them has to start from
/// the same copy — a second control writing a fresh clone of `spec` in the same
/// frame would silently drop the first one's change.
///
/// A free function rather than a closure over `out`, because a closure that both
/// captures the buffer and returns a borrow of it cannot be spelled: the returned
/// reference outlives the call, and the borrow checker has no way to know the
/// closure is not called again while it is held.
fn edit<'a>(out: &'a mut Option<ExportSpec>, spec: &ExportSpec) -> &'a mut ExportSpec {
    out.get_or_insert_with(|| spec.clone())
}

/// What one frame of the quality field reported.
#[derive(Clone, Copy)]
struct ScrubInput {
    /// A right-click called the gesture off (`OndinApp::cancel_gesture`).
    cancelled: bool,
    /// The pointer is holding the field this frame.
    dragged: bool,
    /// The value moved this frame — typed, nudged, or dragged.
    changed: bool,
    /// A scrub was already in flight when this frame began.
    was_held: bool,
}

/// What the frame should do about it.
#[derive(Clone, Copy, Debug, PartialEq)]
enum QualityStep {
    /// Keep the value in the panel's own buffer; commit nothing.
    Hold,
    /// The edit is finished — commit this, once.
    Commit(u8),
    Nothing,
}

/// The quality field's whole decision, as a function of its inputs and nothing
/// else — so the two bugs it has already had are pinned against the inputs that
/// caused them rather than against a drawn panel.
///
/// **The cancel is answered first**, before anything else is read: on the frame a
/// gesture is called off egui may still report the motion that frame as a
/// `changed()`, and letting that through commits the very movement the cancel
/// exists to discard.
///
/// **A drag holds and never commits**, which is the other bug: committing every
/// frame is what left a cancelled scrub with its scrubbed value, because
/// `cancel_gesture` undoes a scrub by dropping a preview and this field has none.
///
/// **The release is recognised by the buffer, not by `drag_stopped()`** — that is
/// not reported on every path out of a drag, and losing it strands the value
/// uncommitted.
fn quality_step(input: ScrubInput, value: f64, current: u8) -> QualityStep {
    let settled = value.round().clamp(1.0, 100.0) as u8;
    if input.cancelled {
        return QualityStep::Nothing;
    }
    if input.dragged {
        return QualityStep::Hold;
    }
    if input.was_held || input.changed {
        return match settled == current {
            true => QualityStep::Nothing,
            false => QualityStep::Commit(settled),
        };
    }
    QualityStep::Nothing
}

/// The multiplier a list of `rows` exports gets for its next one: 1×, 2×, 3×,
/// and 4× from there on.
///
/// **A function of how many rows there are, and of nothing else** — not of which
/// multipliers are free, which is the question this used to answer and the reason
/// the first row was not reliably 1× (see [`OndinApp::next_export_spec`]). Free
/// and testable because it is the whole of the rule: `OndinApp` cannot be built
/// headlessly (§15 D35), so a ladder left inside the method is a ladder nothing
/// can check.
fn ladder_scale(rows: usize) -> ExportScale {
    ExportScale::Times(rows.min(3) as f32 + 1.0)
}

/// The settings button's state — the third one is the point.
///
/// [`ui::FieldButton::Set`] is what lets the block close without either losing a
/// setting or lying about it: the border says there is something in there, the
/// fill says it is not open. Without it the only honest options are refusing to
/// collapse a row that has a suffix, or throwing the suffix away on collapse
/// (§15 D72, where the stroke panel met the same thing).
fn settings_button_state(spec: &ExportSpec, open: bool) -> ui::FieldButton {
    // **Not the quality**, which is a setting every JPEG has rather than one a
    // person went looking for — lighting the border on every JPEG row would make
    // the mark mean "this is a JPEG" instead of "there is something in here".
    let at_rest = spec.prefix.is_empty()
        && spec.suffix.is_empty()
        && spec.background == ExportBackground::Transparent
        && !spec.trim
        && !spec.pad_square;
    match (open, at_rest) {
        (true, _) => ui::FieldButton::On,
        (false, true) => ui::FieldButton::Off,
        (false, false) => ui::FieldButton::Set,
    }
}

/// Trim is measured on pixels, so it is unavailable rather than merely off for a
/// format that has none — and it keeps its stored value while it is, so switching
/// a trimmed PNG to SVG and back does not silently clear it.
fn trim_button_state(spec: &ExportSpec) -> ui::FieldButton {
    match (spec.format.is_raster(), spec.trim) {
        (false, _) => ui::FieldButton::Disabled,
        (true, true) => ui::FieldButton::On,
        (true, false) => ui::FieldButton::Off,
    }
}

/// Padding is a framing rule rather than a pixel one, so it applies to SVG too —
/// there it moves the `viewBox`.
fn pad_button_state(spec: &ExportSpec) -> ui::FieldButton {
    ui::FieldButton::on_if(spec.pad_square)
}

#[cfg(test)]
mod tests {
    //! The panel's decisions, as far as they can be reached without an app.
    //!
    //! What is pinned here is the free functions the panel's decisions were lifted
    //! into — plus the one thing about the row that is arithmetic and
    //! is otherwise checked by eye every time it changes: whether five cells and
    //! four gaps fit the card they are drawn in.
    //!
    //! *The lift was originally forced — `OndinApp` could not be built in a test.
    //! It can as of §15 D303, and the lift is kept because a decision reachable on
    //! its own terms is the better shape, not because there is no other way in.*

    use super::*;
    use ondin_core::ExportBackground;

    fn spec(format: ExportFormat, scale: ExportScale) -> ExportSpec {
        ExportSpec::new(format, scale)
    }

    /// The row's cells have to fit the inspector's 256pt content width, and the
    /// widest text each cell can hold has to fit *its* cell.
    ///
    /// **Aimed at the wrong-by-a-few-points failure, which is the one that
    /// happens.** A row that overflows does not error — egui shrinks the last
    /// item or clips it, so the symptom is a size label that quietly stops
    /// appearing on exactly the rows where the number is longest. The constants
    /// claim to have been measured; this is the measurement.
    #[test]
    fn the_export_row_fits_the_card() {
        const CARD: f32 = 284.0 - ui::CARD_MARGIN_X * 2.0;
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        // One pass so the fonts exist to lay anything out with.
        let _ = ctx.run_ui(Default::default(), |_| {});
        let text_w = |s: &str, size: f32| {
            ctx.fonts_mut(|f| {
                f.layout_no_wrap(
                    s.to_string(),
                    egui::FontId::proportional(size),
                    egui::Color32::WHITE,
                )
                .size()
                .x
            })
        };

        // **What a `ComboBox` spends beside its text, measured rather than
        // reassembled from the style.** Adding up `icon_width` and
        // `button_padding` gets 31 and leaving out `item_spacing` is the kind of
        // guess that is wrong by exactly the amount that matters here. Laying one
        // out with **no** `.width()` makes it size to its content, so the
        // difference between the rect it takes and the text in it *is* the
        // overhead, whatever egui spends it on.
        // **What a `ComboBox` spends beside its text, off egui's own arithmetic**
        // (`combo_box_dyn`): it lays the selected text out, then takes
        // `galley + icon_spacing + icon_width`, floored at `width - 2·button_padding.x`,
        // and paints that inside the padding. So the text fits its cell exactly
        // when `galley + icon_spacing + icon_width ≤ width - 2·padding` — and when
        // it does *not*, the combo comes out **wider than asked for** rather than
        // clipping, which squeezes the size label beside it. That is the failure
        // this is aimed at, and it is invisible in the source.
        //
        // **Measuring by laying one out does not work**, and the two ways it fails
        // are worth writing down: with no `.width()` a combo takes
        // `max(content, spacing.combo_width)`, so a short sample measures the
        // minimum; and the selected text is laid out in `TextStyle::Button`, not in
        // whatever font a probe reaches for, so subtracting the wrong measurement
        // from the right one reports an overhead of 62.
        let style = ctx.style_of(egui::Theme::Dark);
        let overhead = style.spacing.icon_spacing
            + style.spacing.icon_width
            + style.spacing.button_padding.x * 2.0;
        let button_font = style
            .text_styles
            .get(&egui::TextStyle::Button)
            .cloned()
            .expect("a Button text style");
        let combo_text = |s: &str| {
            ctx.fonts_mut(|f| {
                f.layout_no_wrap(s.to_string(), button_font.clone(), egui::Color32::WHITE)
                    .size()
                    .x
            })
        };

        // **What the dropdown offers, taken from the dropdown** — so adding an
        // entry to `ExportScale::COMMON` re-runs this rather than quietly
        // overflowing the cell. A value *typed* into the settings block can be
        // wider (`1024h` wants 70.9) and is allowed to push the combo out by those
        // three points; what must always fit is what a click can produce.
        let widest_scale = ExportScale::COMMON
            .iter()
            .map(|n| ExportScale::Times(*n))
            .chain([ExportScale::Width(512), ExportScale::Height(512)])
            .map(|s| combo_text(&s.label()))
            .fold(0.0_f32, f32::max);
        // **The format cell's text is not laid out in `TextStyle::Button`**, unlike
        // the scale's: `ui::glyph_and_text` builds a `LayoutJob` with its own fonts
        // — a 15pt icon, a 7pt lead, then the word at `proportional(12)`. Measuring
        // it in Button would report a cell that fits and paint one that does not.
        let widest_format = ExportFormat::ALL
            .iter()
            .map(|f| text_w(f.label(), 12.0))
            .fold(0.0_f32, f32::max);
        let format_glyph = ctx.fonts_mut(|f| {
            f.layout_no_wrap(
                icon::FILE_IMAGE.to_string(),
                crate::theme::icon_font(15.0),
                egui::Color32::WHITE,
            )
            .size()
            .x
        }) + 7.0;
        assert!(
            widest_scale + overhead <= SCALE_W,
            "the scale cell is {SCALE_W} and wants {:.1} ({widest_scale:.1} of text + {overhead:.1})",
            widest_scale + overhead
        );
        // The format cell is `1fr` — what the row has left after the scale, the two
        // buttons and the three gaps between the four cells — so what is asserted
        // is that the leftover is enough, not that a constant is.
        let format_w = CARD - SCALE_W - CELL * 2.0 - PITCH * 3.0;
        let wants = widest_format + format_glyph + overhead;
        assert!(
            wants <= format_w,
            "the format cell gets {format_w:.1} and wants {wants:.1} \
             ({widest_format:.1} of word + {format_glyph:.1} of glyph and lead + {overhead:.1} of combo)"
        );
    }

    /// The settings button's third state is the one that lets the block close
    /// without either losing a setting or lying about it (§15 D72's shape).
    ///
    /// **Every arm, because the middle one is what the state exists for**: a spec
    /// with a typed prefix and the popover shut must read differently from a spec
    /// at rest with the popover shut, and those are the two that look alike.
    #[test]
    fn a_closed_row_still_declares_a_setting() {
        let at_rest = spec(ExportFormat::Png, ExportScale::Times(2.0));
        assert_eq!(
            settings_button_state(&at_rest, false),
            ui::FieldButton::Off,
            "a scale of 2× is set on the row, not in the popover"
        );
        assert_eq!(settings_button_state(&at_rest, true), ui::FieldButton::On);

        let mut named = at_rest.clone();
        named.prefix = "hero/".into();
        assert_eq!(settings_button_state(&named, false), ui::FieldButton::Set);

        let mut suffixed = at_rest.clone();
        suffixed.suffix = "-dark".into();
        assert_eq!(
            settings_button_state(&suffixed, false),
            ui::FieldButton::Set
        );

        // **A JPEG at any quality is still at rest**: quality is a setting every
        // JPEG has, so lighting the border on it would make the mark say "this is
        // a JPEG" rather than "there is something in here".
        let mut jpeg = spec(ExportFormat::Jpeg, ExportScale::Times(1.0));
        jpeg.quality = 30;
        assert_eq!(settings_button_state(&jpeg, false), ui::FieldButton::Off);

        let mut backed = at_rest.clone();
        backed.background = ExportBackground::Frame;
        assert_eq!(settings_button_state(&backed, false), ui::FieldButton::Set);

        let mut trimmed = at_rest.clone();
        trimmed.trim = true;
        assert_eq!(settings_button_state(&trimmed, false), ui::FieldButton::Set);

        // ⚠️ **The fifth term, and it was the missing one** (§15 D653,
        // `[S8.3-L6-08]`). `at_rest` is a conjunction of five and this test built
        // four of them, so deleting `&& !spec.pad_square` from
        // `settings_button_state` left every assertion above green — and a row
        // with *Pad to a square* on, collapsed, was indistinguishable from a row
        // at rest, which is precisely what §15 D72's third state exists to
        // prevent. `pad_button_state` beside it is still tested by nothing.
        let mut padded = at_rest;
        padded.pad_square = true;
        assert_eq!(settings_button_state(&padded, false), ui::FieldButton::Set);
    }

    /// **The background chip goes muted over a multi-selection rather than
    /// painting one member's frame** — §15 D657, `[S8.3-L2-06]`.
    ///
    /// 🚨 **The panel states this rule twice and broke it one control away.**
    /// `export_size_label`: *"Blank over a multi-selection rather than a guess.
    /// `Width(512)` is a different multiplier on every differently-shaped layer,
    /// so one number for thirty icons would be true of at most one of them — and
    /// the row that carries it is the same row for all thirty."* `background_swatch`
    /// resolved from `export_subjects().first()`, so two rects in two frames — one
    /// white, one black, both set to *Frame's own* — painted **white** and exported
    /// one of each. The tooltip beside it read *"the nearest frame's background"*,
    /// singular, so nothing on screen said the answer was per-layer.
    ///
    /// ⚠️ **The last assertion is the guard against over-fixing, and it is the one
    /// worth keeping.** Only `Frame` varies per layer; `Solid` and `Transparent`
    /// are properties of the *spec* and are exactly as true of thirty layers as of
    /// one, so muting them would be a second wrong answer in the other direction —
    /// a chip that says nothing about a setting that does have a colour.
    ///
    /// ⚠️ **Flip run, two of them, both predicted right.** Deleting the
    /// `(Frame, None) => None` arm fails the multi-selection assertion, white
    /// against the muted ink. Gating the whole resolution on `sizing` instead —
    /// `sizing.and_then(…)`, the obvious one-line spelling of "be honest over a
    /// multi-selection" — leaves that green and fails the `Solid` assertion, muted
    /// ink against white. **The two flips fail different assertions, which is why
    /// both cases are here**; either one alone would bless one of the two wrong
    /// answers.
    #[test]
    fn a_frame_background_chip_says_nothing_over_a_multi_selection() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        let root = app.session.doc.root();
        let white = ondin_core::peniko::Color::WHITE;
        let black = ondin_core::peniko::Color::BLACK;
        let mut ops = Vec::new();
        let mut kids = Vec::new();
        for (i, ground) in [white, black].into_iter().enumerate() {
            let (frame, rect) = (app.session.ids.mint(), app.session.ids.mint());
            ops.push(ondin_core::Operation::CreateNode {
                id: frame,
                parent: root,
                index: i,
                kind: ondin_core::NodeKind::Artboard {
                    size: ondin_core::kurbo::Size::new(200.0, 200.0),
                },
                transform: None,
                name: None,
            });
            ops.push(ondin_core::Operation::SetFills {
                id: frame,
                fills: vec![ondin_core::Fill {
                    brush: ondin_core::Brush::Solid(ground),
                    visible: true,
                }],
            });
            ops.push(ondin_core::Operation::CreateNode {
                id: rect,
                parent: frame,
                index: 0,
                kind: ondin_core::NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(40.0, 40.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            });
            kids.push(rect);
        }
        assert!(app.session.commit(ondin_core::Transaction(ops)));
        app.session.selection.set(kids.clone());

        let mut framed = spec(ExportFormat::Png, ExportScale::Times(1.0));
        framed.background = ExportBackground::Frame;

        // The fixture is in the state this is about: the two subjects really do
        // resolve to different colours, so "one member's answer" is a wrong answer
        // for the other.
        let a = app.background_swatch(&framed, Some(kids[0]));
        let b = app.background_swatch(&framed, Some(kids[1]));
        assert_ne!(
            a, b,
            "fixture: two frames of different grounds must disagree"
        );
        assert_eq!(a, egui::Color32::WHITE, "fixture: the first frame is white");

        assert_eq!(
            app.background_swatch(&framed, None),
            theme::text::DIM,
            "and over both of them the chip says nothing rather than picking one"
        );

        // The other two backgrounds are per-spec, so a multi-selection has one
        // answer and the chip must keep showing it.
        let mut solid = spec(ExportFormat::Png, ExportScale::Times(1.0));
        solid.background = ExportBackground::Solid(white);
        assert_eq!(
            app.background_swatch(&solid, None),
            egui::Color32::WHITE,
            "White is White on thirty layers as much as on one"
        );
    }

    /// **A planned path splits into a folder and a name before it reaches the save
    /// dialog, and the write side creates the folder either way** — §15 D656,
    /// `[S8.3-L1-10]`.
    ///
    /// 🚨 **The two destination arms of `write_export` differed by exactly one
    /// `create_dir_all` and nothing else**, so a layer named `icons/close` — which
    /// plans `icons/close.png`, the `/` being the *Slashes in names make folders*
    /// feature — exported fine as one of several (folder destination, directory
    /// created) and failed with *"the system cannot find the path specified"* on
    /// its own. Whether an export worked depended on how many **other** layers
    /// were selected.
    ///
    /// The other half is the dialog: `set_file_name(files[0].path.clone())` handed
    /// a relative *path* to an API that wants a name. What the Windows common
    /// dialog makes of that is not asserted here and cannot be from a test — the
    /// asymmetry is, and it holds however the dialog behaves.
    ///
    /// ⚠️ **The joined directory is used only when it already exists**, and that
    /// is the case worth two assertions rather than one: a subfolder that has
    /// never been exported to is the *ordinary* first run, and falling back to the
    /// remembered folder is a better start than whatever a platform dialog does
    /// with a path that does not resolve.
    ///
    /// ⚠️ **Flip run**, `(start, planned.to_string())` — the old behaviour.
    /// Predicted failing assertion: "the name". The failing *assertion* is the
    /// **existing-subfolder** one, both halves of it at once
    /// (`(…, "icons/close.png")` against `(…\icons, "close.png")`) — the bare-name
    /// case above is green under the old behaviour, which is exactly why the bug
    /// survived: the only planned paths anyone tries by hand have no `/` in them.
    #[test]
    fn a_planned_path_reaches_the_save_dialog_as_a_folder_and_a_name() {
        let start = std::env::temp_dir().join(format!("ondin-dialog-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&start);
        std::fs::create_dir_all(start.join("icons")).unwrap();

        // A bare name is unchanged, and the dialog opens where it was going to.
        assert_eq!(
            save_dialog_target(&start, "close.png"),
            (start.clone(), "close.png".to_string())
        );

        // A folder in the name is a *location*, and it exists here.
        assert_eq!(
            save_dialog_target(&start, "icons/close.png"),
            (start.join("icons"), "close.png".to_string()),
            "the `/` is a folder the user asked for, not part of the file's name"
        );

        // The same path where that folder does *not* exist yet — the ordinary
        // first run. The name is still split off; only the directory falls back.
        assert_eq!(
            save_dialog_target(&start, "badges/new/close.png"),
            (start.clone(), "close.png".to_string()),
            "a folder that is not there yet leaves the dialog where it was"
        );
        let _ = std::fs::remove_dir_all(&start);
    }

    /// Trim is unavailable for a format with no pixels and **keeps its value while
    /// it is** — so switching a trimmed PNG to SVG and back does not silently
    /// clear it.
    #[test]
    fn trim_is_unavailable_for_svg_without_being_cleared() {
        let mut s = spec(ExportFormat::Png, ExportScale::Times(1.0));
        s.trim = true;
        assert_eq!(trim_button_state(&s), ui::FieldButton::On);
        s.format = ExportFormat::Svg;
        assert_eq!(trim_button_state(&s), ui::FieldButton::Disabled);
        assert!(s.trim, "the stored value is untouched");
        s.format = ExportFormat::Png;
        assert_eq!(trim_button_state(&s), ui::FieldButton::On);
    }

    /// A preset names itself from what it holds, so saving one needs no dialog.
    #[test]
    fn a_preset_names_itself_by_format_and_size() {
        assert_eq!(
            preset_name(&[
                spec(ExportFormat::Png, ExportScale::Times(1.0)),
                spec(ExportFormat::Png, ExportScale::Times(2.0)),
            ]),
            "PNG 1x, 2x"
        );
        assert_eq!(
            preset_name(&[
                spec(ExportFormat::Png, ExportScale::Times(2.0)),
                spec(ExportFormat::Svg, ExportScale::Times(3.0)),
            ]),
            "PNG 2x + SVG",
            "a vector entry names no size, because the format ignores it"
        );
        assert_eq!(preset_name(&[]), "Empty");
    }

    /// An export row and its size line sit `ROW_LINE_GAP` apart, and one export
    /// sits twice the card's pitch from the next.
    ///
    /// **Measured through egui rather than asserted about the constants**, because
    /// the constants were right and the drawing was not. `Ui::horizontal` takes
    /// `spacing.interact_size.y` as a *minimum* row height and the theme's is 24;
    /// a line of 13pt ink therefore came out 24 tall with the ink centred, adding
    /// five points at each end — so an 8 painted as 13 and an 18 as 23. Nothing
    /// about that was visible in the constants.
    ///
    /// ⚠️ **Those two numbers are a museum piece and this paragraph asserted them
    /// in the present tense until 2026-09-09.** `ROW_LINE_GAP` is **3.0** and
    /// `ROW_UNIT_GAP` **6.0**; their own docs record the ladders 8 → 5 → 3 and
    /// 18 → 15 → 6, so the 8 and the 18 are where those ladders started. The
    /// mechanism is unchanged and the test reads both constants symbolically, so
    /// it passed either way — which is how a sentence goes on naming a value
    /// nothing in the file holds any more. Found by `arch-scribe` reading this
    /// paragraph against `export.rs:58` and `:86` while writing §15 D654.
    ///
    /// The fixture is the layout code's own shape: a card at `CARD_ROW_GAP`
    /// holding two units, each a zero-spacing scope of a 28pt row, an allocated
    /// gap, the size line, and an allocated gap for all but the last.
    #[test]
    fn an_export_row_and_its_size_line_sit_where_they_are_told() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let mut rows: Vec<egui::Rect> = Vec::new();
        let mut lines: Vec<egui::Rect> = Vec::new();
        let _ = ctx.run_ui(Default::default(), |ui| {
            // The card's pitch, then the rows' own region at zero — which is what
            // lets the gap between exports be *smaller* than the card's.
            ui.spacing_mut().item_spacing.y = ui::CARD_ROW_GAP;
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for i in 0..2 {
                    ui.scope(|ui| {
                        ui.spacing_mut().item_spacing.y = 0.0;
                        // The row: whatever it holds, it is CELL tall.
                        let row = ui.horizontal(|ui| {
                            ui.allocate_space(egui::vec2(60.0, CELL));
                        });
                        rows.push(row.response.rect);
                        ui.allocate_space(egui::vec2(0.0, ROW_LINE_GAP));
                        let line = ui.scope(|ui| {
                            ui.spacing_mut().interact_size.y = 0.0;
                            ui.horizontal(|ui| {
                                ui.spacing_mut().item_spacing.x = 6.0;
                                ui.label(theme::icon_text(icon::FRAME_CORNERS, 12.0, SIZE_INK));
                                ui.label(egui::RichText::new("100×60").size(10.5).color(SIZE_INK));
                            })
                            .response
                            .rect
                        });
                        lines.push(line.inner);
                        if i == 0 {
                            ui.allocate_space(egui::vec2(0.0, ROW_UNIT_GAP));
                        }
                    });
                }
            });
        });

        // **This one first, because it is what the other two rest on.** The gaps
        // below are measured between *rects*, and a rect taller than its ink hides
        // the difference: with the theme's 24pt minimum the line's rect still began
        // 8 below the row and ended 18 above the next, while the number inside it
        // floated five points in from each end — so both assertions passed and the
        // panel was wrong by ten. A line that is its own ink is what makes a rect
        // gap and a seen gap the same measurement.
        assert!(
            lines[0].height() < 20.0,
            "the size line is {:.1} tall — `interact_size.y`'s 24pt minimum is back, \
             and the two gaps below no longer mean what they say",
            lines[0].height()
        );
        let inside = lines[0].top() - rows[0].bottom();
        let between = rows[1].top() - lines[0].bottom();
        assert!(
            (inside - ROW_LINE_GAP).abs() < 0.5,
            "row to its size line painted {inside:.1}, not {ROW_LINE_GAP}"
        );
        assert!(
            (between - ROW_UNIT_GAP).abs() < 0.5,
            "one export to the next painted {between:.1}, not {ROW_UNIT_GAP}"
        );
    }

    /// **The gap `export_row` actually allocates after itself, measured on
    /// `export_row`** — §15 D654, `[S8.3-L6-09]`.
    ///
    /// 🚨 **The test above never calls `export_row`.** It builds its own two-unit
    /// column out of `ui.scope` / `ui.horizontal` / `ui.allocate_space` and
    /// measures *that*, so what it proves is that egui behaves as the recipe
    /// expects — it cannot see whether `export_row` follows the recipe. Swapping
    /// the two arms of `export_row`'s trailing `match lined`, which undoes exactly
    /// the correction `ROW_UNIT_GAP`'s doc records, leaves it green; so does
    /// deleting the `if !last` block outright. That is CLAUDE.md's third vacuity
    /// shape — *its fixture never reached the state it names* — with the twist
    /// that the fixture reaches an accurate **copy** of the state, which is what
    /// makes it read as covered. The replica is kept: it is the thing that caught
    /// `interact_size.y`'s 24pt minimum, and that is a fact about egui rather than
    /// about this function.
    ///
    /// ⚠️ **The finding's own cheap fix does not work and is worth saying so.** It
    /// proposed adding an un-captioned case *to the replica*, *"with the same
    /// teeth"*. It has none: the replica's arithmetic is the test's, so an
    /// assertion over it is an assertion about the fixture. The only thing that
    /// can see `export_row`'s allocation is `export_row`.
    ///
    /// **The gap is measured as a difference rather than read off**, which is what
    /// makes it independent of everything else in the row: the same row laid out
    /// with `last` false and with `last` true differs by exactly the block under
    /// test, so the row's own height, the theme's minimums and the size line all
    /// cancel.
    ///
    /// ⚠️ **Flip run, and the predicted site was wrong.** The mutation is the
    /// finding's own — `match lined`'s two arms swapped — and it was expected to
    /// bite on the *un-captioned* assertion, that being the arm the replica never
    /// builds. It bites on the **captioned** one, at 9 against 6, simply because a
    /// swap breaks both and that assertion is written first — a captioned row
    /// allocating 9 where it asks for 6. The un-captioned assertion is what makes
    /// the flip unrecoverable rather than what catches it.
    /// Run in the same command: the replica above **stays green** under it, which
    /// is the finding in one line.
    #[test]
    fn export_rows_trailing_gap_is_the_one_the_unit_asks_for() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        let parent = app.session.doc.root();
        let rect = app.session.ids.mint();
        assert!(app.session.commit(ondin_core::Transaction(vec![
            ondin_core::Operation::CreateNode {
                id: rect,
                parent,
                index: 0,
                kind: ondin_core::NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(100.0, 60.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: None,
            },
        ])));
        let s = spec(ExportFormat::Png, ExportScale::Times(1.0));

        // One `export_row` in its own frame — two in one frame would clash ids —
        // wrapped in a zero-pitch scope so the scope's rect is exactly what the
        // row allocated, trailing gap included.
        let height = |app: &mut crate::app::OndinApp, sizing: Option<NodeId>, last: bool| -> f32 {
            let mut h = 0.0;
            let _ = ctx.run_ui(Default::default(), |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                h = ui
                    .scope(|ui| {
                        app.export_row(ui, 0, &s, sizing, last);
                    })
                    .response
                    .rect
                    .height();
            });
            h
        };

        // The fixture is in both states it names: with a subject the row draws its
        // size line and without one it does not, which is the `lined` this is about.
        assert!(
            !app.export_size_label(&s, Some(rect)).is_empty(),
            "fixture: one sized subject gives the row a size line"
        );
        assert!(
            app.export_size_label(&s, None).is_empty(),
            "fixture: a multi-selection gives it none"
        );

        let lined = height(&mut app, Some(rect), false) - height(&mut app, Some(rect), true);
        let bare = height(&mut app, None, false) - height(&mut app, None, true);

        assert!(
            (lined - ROW_UNIT_GAP).abs() < 0.5,
            "a captioned row allocates {lined:.1} after itself, not {ROW_UNIT_GAP}"
        );
        // **A row that drew no size line spends that line's gap here instead** —
        // the 3 a multi-selection's list was missing.
        assert!(
            (bare - (ROW_UNIT_GAP + ROW_LINE_GAP)).abs() < 0.5,
            "an un-captioned row allocates {bare:.1}, not {:.1}",
            ROW_UNIT_GAP + ROW_LINE_GAP
        );
    }

    /// The quality field's state machine, which has been wrong twice — once in
    /// each direction — and is the reason it is a function rather than four `if`s
    /// inside a closure.
    ///
    /// **Every arm is aimed at a version that shipped.** `commit_run` committed on
    /// the drag frames (arm 2) and so could not be cancelled (arm 1); `edit_valve`
    /// held the value in a preview that a `SetExports` cannot carry, so the number
    /// never moved under the hand at all — which is arm 2 read the other way, and
    /// what the `Hold` return exists to fix.
    #[test]
    fn a_quality_scrub_holds_commits_once_and_can_be_called_off() {
        let step = |cancelled, dragged, changed, was_held, v: f64| {
            quality_step(
                ScrubInput {
                    cancelled,
                    dragged,
                    changed,
                    was_held,
                },
                v,
                90,
            )
        };

        // Mid-drag: the value is live in the buffer and nothing reaches history.
        assert_eq!(step(false, true, true, false, 62.0), QualityStep::Hold);
        assert_eq!(step(false, true, true, true, 41.0), QualityStep::Hold);

        // The release: one commit, from the buffer rather than from `drag_stopped`.
        assert_eq!(
            step(false, false, false, true, 41.4),
            QualityStep::Commit(41)
        );

        // The right-click, on the frame it lands *and* on the release after it —
        // egui reports the motion of that frame as a change, and taking it is
        // exactly the bug.
        assert_eq!(step(true, true, true, true, 41.0), QualityStep::Nothing);
        assert_eq!(step(true, false, true, true, 41.0), QualityStep::Nothing);

        // Typed or nudged, with no drag anywhere: committed on the spot.
        assert_eq!(
            step(false, false, true, false, 30.0),
            QualityStep::Commit(30)
        );
        // And a "change" that lands back on the stored value is not an edit.
        assert_eq!(step(false, false, true, false, 90.0), QualityStep::Nothing);
        assert_eq!(step(false, false, false, false, 90.0), QualityStep::Nothing);
    }

    /// The `+`'s ladder: 1×, 2×, 3×, then 4× for ever.
    ///
    /// **Both ends, because each is a version that shipped.** The bottom is the
    /// bug this was re-cut for — the rule used to pick the first multiplier not
    /// already *used*, so the row a fresh layer got was 1× by luck rather than by
    /// design, and any list that already held a 1× was handed a 3× for its second
    /// row. The top is the cap, which an uncapped `1 + n` climbs correctly for
    /// four rungs and then walks off — 5× and 6× against the six rows read here.
    #[test]
    fn the_add_button_climbs_one_two_three_and_then_stays_at_four() {
        let ladder: Vec<ExportScale> = (0..6).map(ladder_scale).collect();
        assert_eq!(
            ladder,
            [1.0, 2.0, 3.0, 4.0, 4.0, 4.0].map(ExportScale::Times),
            "an empty list must get 1×, and the fourth row onwards 4×"
        );
        // Every rung is a rung of the dropdown, so a row the `+` produced can be
        // read back off the combo it sits in rather than looking like a typed one.
        for s in &ladder {
            let ExportScale::Times(n) = s else {
                panic!("the ladder is multipliers")
            };
            assert!(
                ExportScale::COMMON.contains(n),
                "{n}× is not one of the multipliers the dropdown offers"
            );
        }
    }

    /// The units a file browser uses, because that is what the number will be
    /// compared against.
    #[test]
    fn a_file_size_reads_the_way_explorer_reads_it() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1024), "1 KB");
        assert_eq!(human_bytes(1025), "2 KB", "rounded up, never to 1");
        assert_eq!(human_bytes(1024 * 1024), "1.0 MB");
    }

    /// **`[S8.3-L1-01]`'s loss: the preview's area cap could not see the pad.**
    ///
    /// `plan::raster_size` answers the *framed* size and `raster_of` squares it
    /// afterwards, so a 32768×120 layer with *Pad out to a square* came through
    /// the guard at 3.9 M pixels — under the 4 M cap — and then asked for a
    /// 32768² buffer, 4 GB, on the UI thread inside `eframe::App::update`. The
    /// same raster is what a 4096×15 banner at 8× produces.
    ///
    /// ⚠️ **An area cap cannot guard the raster path's failures on its own**, and
    /// that is the general fact rather than this bug: they are a *per-side* and a
    /// *longest-side* limit, and any area admits any aspect ratio thin enough.
    /// The ratio needed here is about 273:1.
    ///
    /// ⚠️ **The second half of this test was found by writing the first half's
    /// control and watching it crash.** The banner *without* the pad is
    /// 32768×120 — 3.9 M pixels, under the area cap — and `ctx.load_texture`
    /// panics on it outright: *"Texture "export-preview" has size 32768x120, but
    /// the maximum texture side is 2048"*. So the intended control was a second
    /// instance of the same defect, by a second per-side limit that is the
    /// **device's** rather than the raster path's. Both are guarded now, and the
    /// control is a small layer instead.
    ///
    /// **Two flips, both run, one per half of the guard.** Dropping the pad
    /// squaring fails at *"the same strip padded is 204.8× the pixels"* — with
    /// the summary of the preview it built, `PNG · 2048×2048 · 21 KB`, in the
    /// message. Dropping the per-side test fails inside **egui**, at
    /// `context.rs:2331`, with the library's own words: *"Texture
    /// "export-preview" has size 32768x120, but the maximum texture side is
    /// 2048"*. Neither flip reaches the other's assertion.
    ///
    /// ⚠️ **Neither flip allocated anything alarming, and that is a fact about
    /// the *test environment* rather than about the bug.** A headless context
    /// reports a 2048 texture side, so the strip's square is 4.19 M pixels — 16
    /// MB. On a real device that same square is 16384², and the banner's is
    /// 32768²: 1 GB and 4 GB, on the UI thread, inside the frame callback.
    #[test]
    fn a_padded_export_is_declined_by_the_preview_on_the_size_it_would_be() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        let parent = app.session.doc.root();
        // ⚠️ **Sized off the device's own limit**, so the *pad* guard is what
        // decides this one and the side guard cannot: a `side × 10` strip is
        // inside the texture limit on both axes by construction, and its square
        // is `side²` — 4.19 M at the 2048 a headless context reports, 268 M at a
        // real device's 16384, and over the 4 M area cap either way. The banner
        // below is the realistic case and trips *both* guards, so it cannot tell
        // them apart; this one can.
        let max_side = f64::from(ctx.input(|i| i.max_texture_side) as u32);
        let (banner, strip) = (app.session.ids.mint(), app.session.ids.mint());
        let rect = |id, w: f64, h: f64, index| ondin_core::Operation::CreateNode {
            id,
            parent,
            index,
            kind: ondin_core::NodeKind::Rect {
                size: ondin_core::kurbo::Size::new(w, h),
                corner_radii: Default::default(),
            },
            transform: None,
            name: None,
        };
        // A 4096×15 banner at 8× is a 32768×120 raster — an ordinary export set,
        // not a contrived one.
        assert!(app.session.commit(ondin_core::Transaction(vec![
            rect(banner, 4096.0, 15.0, 0),
            rect(strip, max_side, 10.0, 1),
        ])));

        let at = |app: &mut crate::app::OndinApp, node, times, pad, rev| {
            let mut spec = spec(ExportFormat::Png, ExportScale::Times(times));
            spec.pad_square = pad;
            app.build_export_preview(
                &ctx,
                PreviewKey {
                    node,
                    spec,
                    revision: rev,
                },
            )
            .expect("the row has a subject")
        };

        // The pad guard on its own: `side × 10` previews, and its square does not.
        let plain = at(&mut app, strip, 1.0, false, 0);
        assert!(plain.texture.is_some(), "the control: {}", plain.summary);
        let squared = at(&mut app, strip, 1.0, true, 1);
        assert!(
            squared.texture.is_none(),
            "the same strip padded is {}× the pixels: {}",
            max_side / 10.0,
            squared.summary
        );
        assert_eq!(
            squared.pixels,
            (max_side as u32, max_side as u32),
            "and the row says the size the file would be, not the step before it"
        );

        // The realistic case, which trips both guards: a 4096×15 banner at 8×.
        let big = at(&mut app, banner, 8.0, true, 2);
        assert!(
            big.texture.is_none(),
            "a 32768² raster must not be built for a preview"
        );
        assert_eq!(big.pixels, (32_768, 32_768));
        let unpadded = at(&mut app, banner, 8.0, false, 3);
        assert!(
            unpadded.texture.is_none(),
            "and 32768×120 is past the device's texture side, whatever its area"
        );
    }
}

#[cfg(test)]
mod export_all_scope_tests {
    //! **What *Export all* actually collects** — `[S8.3-L1-02]`, §15 D516.

    use super::*;

    /// **A layer with export settings inside another layer with export settings
    /// is exported too.**
    ///
    /// `exportable_layers` walks the whole tree and collects every node with a
    /// non-empty `exports()` — correctly — and then handed the result to
    /// `build::in_document_order`, whose first line is `outermost(doc, ids)`.
    /// That filter drops any id with an ancestor in the set. It is right for
    /// `export_subjects`, the *selection* door, where a group and its child both
    /// picked would plan the child twice (§15 D267); it is wrong here, because an
    /// export spec on a nested layer is a deliberate instruction and exporting
    /// the group produces a **different file** from exporting the child. A banner
    /// frame with an icon inside it exported separately is the workflow the panel
    /// exists for.
    ///
    /// ⚠️ **Nothing reported the loss.** `write_export`'s `skipped` counter only
    /// counts layers with no *bounds*, so the status line said *"Exported 1
    /// file"* with no qualification, while the rect's settings sat in its own
    /// Export card producing nothing.
    ///
    /// ⚠️ **The documentation was the correct half, which is why this is a code
    /// fix.** `exportable_layers`' own doc says *"Every layer in the document
    /// that is set up to export"*, the menu row says *"Every layer with export
    /// settings, in one go"*, and `main.rs`'s `run_export_all` says *"the same
    /// set and the same order the panel's Export all collects"*. All three
    /// describe what a user expects; only `outermost` disagreed — **and the CLI
    /// had the same defect through an independent copy of the walk, so the two
    /// front ends agreed about the wrong answer, which is why nothing noticed.**
    ///
    /// The planned **filenames** are asserted rather than the count alone: two
    /// files that collide on a name are a different failure from one file, and
    /// `plan` is where that would show.
    ///
    /// **Flip run**, `in_document_order` restored: fails on *"both layers are
    /// collected"* at one id against two — the predicted site.
    #[test]
    fn a_nested_layers_export_settings_are_not_dropped_by_export_all() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        let parent = app.session.doc.root();
        let (group, inner) = (app.session.ids.mint(), app.session.ids.mint());
        let png = vec![ExportSpec::new(ExportFormat::Png, ExportScale::Times(1.0))];
        assert!(app.session.commit(ondin_core::Transaction(vec![
            ondin_core::Operation::CreateNode {
                id: group,
                parent,
                index: 0,
                kind: ondin_core::NodeKind::Group,
                transform: None,
                name: Some("Banner".into()),
            },
            ondin_core::Operation::CreateNode {
                id: inner,
                parent: group,
                index: 0,
                kind: ondin_core::NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(40.0, 40.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: Some("Icon".into()),
            },
            ondin_core::Operation::SetExports {
                id: group,
                exports: png.clone(),
            },
            ondin_core::Operation::SetExports {
                id: inner,
                exports: png,
            },
        ])));

        assert!(
            app.document_exports_anything(),
            "fixture: the row is live, which is what promises the files"
        );
        let layers = app.exportable_layers();
        assert_eq!(
            layers,
            vec![group, inner],
            "both layers are collected, outermost first — document order"
        );
        let plan = app.export_plan(&layers);
        let names: Vec<&str> = plan.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(
            names.len(),
            2,
            "and both are planned as their own file: {names:?}"
        );
        assert_ne!(names[0], names[1], "under distinct names: {names:?}");
    }

    /// **Document order among *siblings* comes from the sort, not from the walk**
    /// — §15 D649, `[A7-L8-02]`.
    ///
    /// The walk in `exportable_layers` used to push each parent's children
    /// reversed, under a comment saying that is *"what makes the traversal come
    /// out in document order"* and that everything *"would inherit"* it. Neither
    /// held: `build::all_in_document_order` collects its input into an
    /// `FxHashSet` on its first line and re-walks the document, so the order the
    /// stack produced was thrown away. The `.rev()` was removed with the comment.
    ///
    /// ⚠️ **Siblings, not a parent and a child.** The test above already asserts a
    /// nested pair comes out outermost-first, and that pair is in the same order
    /// whichever way the children are pushed — so it could not have seen this at
    /// all. Three siblings is the smallest fixture where the two orders differ.
    ///
    /// ⚠️ **Flip run, and the first one deliberately does not bite.** Putting the
    /// `.rev()` back leaves this green, which is the claim: the walk's order is
    /// unobservable. What bites is returning `out` from `exportable_layers`
    /// instead of the sort — `[c, b, a]` against `[a, b, c]`, on the one
    /// assertion — which is what says the sort is load-bearing rather than
    /// decorative, and therefore what the removed comment should have said.
    #[test]
    fn export_all_collects_siblings_in_document_order() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        let parent = app.session.doc.root();
        let png = vec![ExportSpec::new(ExportFormat::Png, ExportScale::Times(1.0))];
        let ids: Vec<_> = (0..3).map(|_| app.session.ids.mint()).collect();
        let mut ops = Vec::new();
        for (i, id) in ids.iter().enumerate() {
            ops.push(ondin_core::Operation::CreateNode {
                id: *id,
                parent,
                index: i,
                kind: ondin_core::NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(40.0, 40.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: Some(format!("Sibling {i}")),
            });
            ops.push(ondin_core::Operation::SetExports {
                id: *id,
                exports: png.clone(),
            });
        }
        assert!(app.session.commit(ondin_core::Transaction(ops)));

        assert_eq!(
            app.exportable_layers(),
            ids,
            "three exporting siblings come out in the order the document holds them"
        );
    }

    /// **Export on save never opens a modal** (§15 D626, `[A5-L6-03]`).
    ///
    /// 🚨 **The whole GUI export path had zero tests**, and this decision — the
    /// one `export_all`'s own doc calls out, *"a modal appearing on `Ctrl+S`
    /// because a preference is set is the thing that would make the switch not
    /// worth having"* — was fused to `rfd::FileDialog`. Collapse the `Quietly` arm
    /// into the `Asking` one and every save with the preference on pops a native
    /// folder dialog, with nothing in the workspace failing. `facts.md` put
    /// `panels/export.rs` at **20.81% over 1,432 lines** — the largest genuinely
    /// under-covered module in the workspace.
    ///
    /// ⚠️ **`OndinApp::headless` does not reach this and §15 D303 says so**: even a
    /// headless call would open a real dialog and write real files. The decision
    /// had to be *lifted* before it could be observed at all, which is §15 D269's
    /// prescription and is the shape this session has now applied four times.
    ///
    /// **The remembered-folder arm is asserted for both `how` values**, because
    /// that is the arm's actual claim — *whoever asked* — and a table checked one
    /// row per input would pass against a version that only wrote for `Asking`.
    ///
    /// **Flip-check, run**: `(None, Quietly)` mapped to `AskForFolder` fails at
    /// *"a save must never ask"*; and dropping the `.filter(|d| d.is_dir())` at the
    /// call site is the other half of the same failure — **it cannot be flipped
    /// here**, because it lives in `export_all` and is a filesystem test rather
    /// than a decision. That half is still uncovered, and this test does not
    /// pretend to reach it.
    #[test]
    fn export_on_save_never_asks_for_a_folder() {
        let dir = std::path::PathBuf::from("/tmp/exports");
        assert_eq!(
            export_destination(Some(dir.clone()), ExportAll::Quietly),
            ExportRoute::Write(dir.clone()),
            "a remembered folder is written to without asking"
        );
        assert_eq!(
            export_destination(Some(dir), ExportAll::Asking),
            ExportRoute::Write(std::path::PathBuf::from("/tmp/exports")),
            "and the same folder serves an explicit export too"
        );
        assert_eq!(
            export_destination(None, ExportAll::Quietly),
            ExportRoute::SayNoDestination,
            "a save must never ask — it says so once and moves on"
        );
        assert_eq!(
            export_destination(None, ExportAll::Asking),
            ExportRoute::AskForFolder,
            "and an explicit export with nowhere to go is the one that may ask"
        );
    }

    /// **Which dialog an explicit export opens, at the boundary that decides it**
    /// (§15 D626, `[A5-L6-03]`).
    ///
    /// One file is a *Save as* with that file's own name and format filter already
    /// filled in; two is a folder pick, because there is no one name to offer. ⚠️
    /// **Asserted from both sides of `files.len() == 1`**, which is the whole of
    /// the second arm — a test at one count alone says nothing about a boundary.
    ///
    /// **A `zip` overrides both and is matched first**, so it is asserted at each
    /// of the three counts: an archive of one file is still an archive, which is
    /// the case a reordered match would get wrong and the only one where the two
    /// tables disagree.
    #[test]
    fn one_file_saves_and_two_pick_a_folder_unless_it_is_an_archive() {
        assert_eq!(export_dialog(false, 1), DialogKind::SaveFile);
        assert_eq!(export_dialog(false, 2), DialogKind::PickFolder);
        assert_eq!(export_dialog(false, 9), DialogKind::PickFolder);
        for n in [1usize, 2, 9] {
            assert_eq!(
                export_dialog(true, n),
                DialogKind::Archive,
                "an archive of {n} file(s) is still an archive"
            );
        }
        // `run_export` returns before this on an empty plan, so zero is not a
        // reachable input — asserted anyway, because the arm it falls into is the
        // folder pick and a reader should not have to work that out.
        assert_eq!(export_dialog(false, 0), DialogKind::PickFolder);
    }

    /// **`write_export` writes the files and reports what happened** (§15 D626,
    /// `[S8.3-L6-04]`).
    ///
    /// 🚨 **None of the 27 `OndinApp` methods in this file had a test caller
    /// anywhere in the workspace**, and the *tested* export walk is the wrong one:
    /// `main.rs`'s `export_all_runs_the_documents_own_settings` drives
    /// `run_export_all`, which **re-implements** the same walk. The panel's door
    /// is the one a user reaches.
    ///
    /// **This one needs no dialog at all**, which is why the finding names it as
    /// the place to start: `Destination` is constructed directly, and the whole
    /// `(failed, skipped)` reporting table becomes assertable against a temp
    /// folder.
    ///
    /// **Three things are asserted and each is a separate claim.** The bytes reach
    /// disk under the planned names — the point of the function. The `skipped`
    /// counter appears in the sentence — *"the one refusal both writers make"*, a
    /// subject with no bounds at all, and it is reported rather than silent, which
    /// is exactly what `[S8.3-L1-02]` found *missing* for a different loss. And
    /// `prefs.export_dir` is remembered, which is what makes the *next* export
    /// silent and is therefore the input to `export_destination` above.
    ///
    /// ⚠️ **The empty group is the fixture's own point.** It has export settings
    /// and no extent, so `plan::render` refuses it — that is the only way to reach
    /// the `(None, n)` arm, and a fixture of two ordinary rects would assert the
    /// `(None, 0)` arm twice and call it coverage.
    ///
    /// ⚠️ **`write_export` calls `prefs.save()` and this test does not touch the
    /// user's `prefs.json`**, because `OndinApp::headless` sets `Prefs::ephemeral`
    /// and `save` returns on its first line. 🚨 **That is the injection point
    /// `library::cache::LocalIndex` still lacks** — §15 D619 records the suite
    /// editing the person's own *Recent searches* for exactly the want of it, and
    /// `Prefs::save`'s own first-line comment gives the reason in the words that
    /// entry needed: *"the damage is silent and permanent — the file it writes outlives
    /// the test by however long it takes somebody to notice"*. **The shape already
    /// exists in this codebase; one module has it and one does not.**
    ///
    /// **Flip-check, run**: the `None` arm counted as a success — `rendered.push`
    /// with empty bytes instead of `skipped += 1` — fails at **`["Icon.png",
    /// "Nothing.png"]`, 2 against 1**. ⚠️ **The predicted site was the sentence
    /// and it bites two assertions earlier, on the files**, which is the sharper
    /// result: the wrong version does not merely mis-report, it writes a
    /// **zero-byte PNG** for a layer that has nothing in it. *A miscount here is
    /// a file on the user's disk, not a wording.* Dropping the `if written > 0`
    /// guard around the prefs write is invisible to this fixture, which always
    /// writes something, and is called out rather than asserted.
    #[test]
    fn writing_an_export_lands_the_files_and_says_what_had_no_size() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        let parent = app.session.doc.root();
        let (rect, empty) = (app.session.ids.mint(), app.session.ids.mint());
        let png = vec![ExportSpec::new(ExportFormat::Png, ExportScale::Times(1.0))];
        assert!(app.session.commit(ondin_core::Transaction(vec![
            ondin_core::Operation::CreateNode {
                id: rect,
                parent,
                index: 0,
                kind: ondin_core::NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(40.0, 30.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: Some("Icon".into()),
            },
            // Exports set, nothing inside — `plan::render` refuses it, which is
            // the `skipped` arm and the only way to reach it.
            ondin_core::Operation::CreateNode {
                id: empty,
                parent,
                index: 1,
                kind: ondin_core::NodeKind::Group,
                transform: None,
                name: Some("Nothing".into()),
            },
            ondin_core::Operation::SetExports {
                id: rect,
                exports: png.clone(),
            },
            ondin_core::Operation::SetExports {
                id: empty,
                exports: png,
            },
        ])));

        let dir = std::env::temp_dir().join(format!("ondin-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let layers = app.exportable_layers();
        assert_eq!(layers.len(), 2, "fixture: both layers are collected");
        let files = app.export_plan(&layers);
        app.write_export(files, Destination::Folder(dir.clone()));

        let on_disk: Vec<String> = std::fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| Some(e.ok()?.file_name().to_string_lossy().into_owned()))
            .collect();
        assert_eq!(
            on_disk.len(),
            1,
            "the one layer with a size is written and the empty one is not: {on_disk:?}"
        );
        assert!(
            on_disk[0].ends_with(".png"),
            "under its planned name: {on_disk:?}"
        );

        let said = app.session.status().text.clone();
        assert!(
            said.contains("had no size"),
            "the refusal is reported rather than silent: {said:?}"
        );
        assert_eq!(
            app.prefs.export_dir.as_deref(),
            Some(dir.as_path()),
            "and the folder is remembered, which is what makes the next export silent"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A folder export says how many files it replaced, and where** (§15 D636,
    /// `[S8.3-L5-05]`).
    ///
    /// The defect is the silence rather than the write. `export_all` takes
    /// `prefs.export_dir` — **one global folder**, set by any previous export
    /// including a single-file save dialog's parent — and when it is still a
    /// directory it writes there with **no dialog**; *Re-export on save* then
    /// fires that on an **autosave**. A designer who exported once into a folder
    /// of hand-made assets has that folder's `logo.png` replaced every time a
    /// different document saves itself, and the message said `Exported 1 file`
    /// and named nothing.
    ///
    /// ⚠️ **Only the folder arm gains this**, and the asymmetry is the decision: a
    /// *File* or an *Archive* was named in a save dialog a moment earlier, so the
    /// user has been shown the path and warned by the OS. A folder export can
    /// arrive with no dialog at all, which is what `export_all`'s doc calls the
    /// feature — *"No dialog when there is somewhere to put it"* — and what makes
    /// the report the whole of the promise.
    ///
    /// **The same plan written into the same folder twice, because the count is a
    /// difference and not a property.** It must say `0 replaced` and then
    /// `1 replaced`; asserting only the second would pass for a message that says
    /// "replaced" always, and asserting only the first for one that never does. A
    /// third write follows, into a `Destination::File`, and it is the control for
    /// the *other two arms* rather than a third folder run.
    ///
    /// ⚠️ Flip-checked by moving the `path.exists()` read to **after** the write,
    /// and **the failure is one run earlier than predicted**: red on the *first*
    /// run, at `"Exported 1 file (1 replaced)"` for a file that was not there a
    /// moment before. Every path exists once `fs::write` has returned, so the
    /// count comes back as the *file* count and the report is wrong from the very
    /// first export rather than drifting into wrongness. *That is the whole of why
    /// the read is where it is*, and it is the one line in this change a reader
    /// could move without noticing — which is why the `!contains("replaced")`
    /// assertion is here at all.
    #[test]
    fn a_folder_export_says_what_it_replaced_and_where() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        let parent = app.session.doc.root();
        let rect = app.session.ids.mint();
        assert!(app.session.commit(ondin_core::Transaction(vec![
            ondin_core::Operation::CreateNode {
                id: rect,
                parent,
                index: 0,
                kind: ondin_core::NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(40.0, 30.0),
                    corner_radii: Default::default(),
                },
                transform: None,
                name: Some("Icon".into()),
            },
            ondin_core::Operation::SetExports {
                id: rect,
                exports: vec![ExportSpec::new(ExportFormat::Png, ExportScale::Times(1.0))],
            },
        ])));

        let dir = std::env::temp_dir().join(format!("ondin-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let plan = |app: &crate::app::OndinApp| app.export_plan(&app.exportable_layers());

        app.write_export(plan(&app), Destination::Folder(dir.clone()));
        let first = app.session.status().text.clone();
        assert!(
            first.starts_with("Exported 1 file"),
            "the first run writes it: {first:?}"
        );
        assert!(
            !first.contains("replaced"),
            "and replaces nothing, so it says nothing about replacing: {first:?}"
        );
        assert!(
            first.contains(&dir.display().to_string()),
            "and it names the folder, which is the half no dialog was shown for: \
             {first:?}"
        );

        app.write_export(plan(&app), Destination::Folder(dir.clone()));
        let again = app.session.status().text.clone();
        assert!(
            again.contains("(1 replaced)"),
            "the second run overwrites and says so: {again:?}"
        );
        assert!(
            again.contains(&dir.display().to_string()),
            "still naming where: {again:?}"
        );

        // And a *File* export is unchanged, because its dialog already said where.
        let one = dir.join("named.png");
        app.write_export(plan(&app), Destination::File(one));
        let file = app.session.status().text.clone();
        assert_eq!(
            file, "Exported 1 file",
            "a save-dialog destination reports as it always did"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }
}
