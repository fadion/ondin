//! The inspector's **Export** panel (§7, §9.4) — the list of files a layer
//! produces, and the button that produces them.
//!
//! The panel is the only door to exporting: the answers live on the layer, are saved
//! with the document and are read by the CLI. So a bug in `run_export`, in `plan`,
//! or in seeding a first spec has no fallback path a user can reach.
//!
//! Nothing here interprets a spec. `ondin_export::plan` turns the saved settings
//! into filenames, pixel sizes and bytes; this module is the controls that edit them
//! and the dialogs that choose where they land. That division is what lets
//! `ondin export --all` produce the same files with no window.

use crate::app::OndinApp;
use crate::theme::{self, icon};
use crate::ui::{self, Prefix, Scrub, value_field};
use ondin_core::{
    ExportBackground, ExportFormat, ExportScale, ExportSpec, ExportText, Keyed, NodeId, Operation,
    Transaction,
};
use ondin_export::{PlanOptions, PlannedFile};

/// The scale cell of the export row (7pt pitch, 256pt card).
///
/// A `ComboBox` spends 35pt beside its text and lays it out in `TextStyle::Button`,
/// so `0.75x` wants 67 and `1024h` wants 71. A combo asked for less comes out wider
/// than asked and squeezes its neighbours (`the_export_row_fits_the_card`). The
/// format cell takes what is left.
const SCALE_W: f32 = 74.0;
/// The height of everything on the row, and the side of its two square buttons.
const CELL: f32 = 28.0;
/// Between two controls side by side — the card gap, which the settings
/// popover matches (`typography::COL_GAP`).
const PITCH: f32 = 7.0;
/// An instance's row's trailing slot, `component::item_slot`'s width (§15 D981).
const SLOT_W: f32 = 14.0;
/// Between a row and the size line under it.
///
/// Tighter than anything else in the card: the line annotates the row above it, so
/// it sits closer to that row than the pair sits to the next ([`ROW_UNIT_GAP`]).
const ROW_LINE_GAP: f32 = 3.0;
/// Between one export and the next.
///
/// Below the card's own row gap, so the rows are laid out in a region of their own
/// with the pitch taken to zero (a scope can add to a spacing, not subtract). It is
/// the gap between two units, so a row with no caption spends [`ROW_LINE_GAP`]
/// instead, coming to the card's pitch: that is the multi-selection case, where six
/// between two bare controls would read as crowding. See `export_row`'s tail.
const ROW_UNIT_GAP: f32 = 6.0;
/// An export's two halves sit closer than one export sits to the next, or the size
/// line reads as belonging to the row below it.
const _: () = assert!(ROW_LINE_GAP < ROW_UNIT_GAP);
/// The size line's ink — 34%, quieter than `text::FAINT` (38%),
/// because it annotates a control rather than labelling one.
const SIZE_INK: egui::Color32 = theme::color::text_a(87);
/// The collapse key the Preview section shares with the panel list.
///
/// `collapsed_panels` is keyed by title, but the section now lives inside the Export
/// card, so the key is named for the two places that read it.
const PREVIEW: &str = "Preview";
/// The clear space round the previewed image inside its checkerboard.
const PREVIEW_INSET: f32 = 10.0;

// --- the settings popover ---------------------------------------------------
//
// The shared popover language: 272 wide, 11 of padding, 26pt controls, 9.5pt
// small-caps section labels, as in `typography.rs` and `stroke_menu_popup`.

/// The card's width, shared with every other popover in the inspector.
const POP_W: f32 = crate::app::POPOVER_W;
const POP_PAD: f32 = 11.0;
/// The room inside the padding and the border — `POP_W` is the width the card
/// paints, not the room in it (§15 D307).
const POP_INNER: f32 = ui::menu_inner_w(POP_W, POP_PAD);
/// A control inside the popover — 26, against the card's 28, so a popover of four
/// sections does not tower over a card of three rows (as `typography::CELL`).
const POP_CELL: f32 = 26.0;
/// Between one labelled section and the next — the shared one.
const SECTION_GAP: f32 = ui::POPOVER_SECTION_GAP;
/// Between a section's label and its control, in both popovers.
const LABEL_GAP: f32 = 5.0;
/// The ✕ beside the card's eyebrow, shared with the type popup's.
const CLOSE_W: f32 = 18.0;

/// One labelled block of the popover: a small-caps eyebrow, then its control.
///
/// A `scope`, so the card's two vertical gaps are two numbers: with a single
/// spacing the label floats equidistant between its own control and the section
/// above. Copied from `typography::section`, whose constants are private.
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
    /// An instance's overridden export asked for its main's back (§15 D981).
    reset: bool,
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
/// The revision is the term that is easy to leave out: a preview keyed only by layer
/// and spec survives every edit to the artwork, a confident picture of something no
/// longer there.
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

/// What a vector export could not say exactly, as a clause to hang off a message —
/// `None` when the file is the document (§15 D780).
///
/// The same two-clause shape as the SVG paste message in `canvas.rs`, sorted because
/// the order features are met is an accident of the document. Two clauses, not one:
/// *approximated* means the drawing differs, *round-trip* means the drawing is right
/// but re-opening the file gives a different document. A PDF has only the first.
pub(crate) fn fidelity_clauses(approximated: &[String], round_trip: &[String]) -> Option<String> {
    let mut parts = Vec::new();
    let mut say = |label: &str, list: &[String]| {
        if !list.is_empty() {
            let mut items = list.to_vec();
            items.sort();
            parts.push(format!("{label} {}", items.join(", ")));
        }
    };
    say("approximated", approximated);
    say("will not re-import as drawn:", round_trip);
    (!parts.is_empty()).then(|| parts.join("; "))
}

/// `84 KB`, `1.2 MB` — what a file size is worth saying to a designer.
///
/// 1024, not 1000: file browsers use binary units under these labels.
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
    /// `build::in_document_order` drops a child selected with its group (it would be
    /// planned twice) and restores document order (§15 D267). The two *Copy as* rows
    /// apply the same filter.
    pub(crate) fn export_subjects(&self) -> Vec<NodeId> {
        ondin_core::build::in_document_order(&self.session.doc, self.session.selection.ids())
    }

    /// Whether the selection has moved since the Export panel last read it — the
    /// edge `sync_paint_collapse` re-asserts the resting collapse state on.
    ///
    /// Its own entry in `OndinApp::synced_selection`: one edge consumed by two
    /// callers is lost to one of them.
    fn export_subject_changed(&mut self) -> bool {
        self.subject_changed("Export")
    }

    /// The list every subject agrees on, or `None` when they disagree.
    ///
    /// Reads the committed document, not `display_node`: `Operation::SetExports` is a
    /// no-op in `RenderOverrides`, so there is nothing to preview.
    fn shared_exports(&self, subjects: &[NodeId]) -> Option<Vec<ExportSpec>> {
        self.shared_exports_keyed(subjects)
            .map(|l| ondin_core::item::values(&l))
    }

    /// [`Self::shared_exports`] with the first subject's item ids — the anchor a
    /// row edit is retargeted from ([`Self::retarget_exports`]). Agreement is by
    /// value (§15 D980): two layers given one spec separately carry different ids.
    fn shared_exports_keyed(&self, subjects: &[NodeId]) -> Option<Vec<Keyed<ExportSpec>>> {
        let mut nodes = subjects.iter().filter_map(|id| self.session.doc.get(*id));
        let first = nodes.next()?.exports().to_vec();
        nodes
            .all(|n| ondin_core::same_values(n.exports(), &first))
            .then_some(first)
    }

    /// A row **edit** of the list every subject shares, written onto each subject
    /// with its own item ids (`ondin_core::item::retarget`, §15 D980), as one step.
    ///
    /// `pub(super)` for `inspector::paint_write_tests`' table of the five list
    /// writers, as are [`Self::exports_tx`] and
    /// [`Self::append_exports`].
    pub(super) fn retarget_exports(
        &mut self,
        subjects: &[NodeId],
        anchor: &[Keyed<ExportSpec>],
        edited: &[Keyed<ExportSpec>],
    ) {
        let session = &mut self.session;
        let ops: Vec<Operation> = subjects
            .iter()
            .filter_map(|id| {
                let own = session.doc.get(*id)?.exports();
                let exports = ondin_core::item::retarget(anchor, own, edited, &mut session.ids);
                (exports != own).then_some(Operation::SetExports { id: *id, exports })
            })
            .collect();
        if !ops.is_empty() {
            self.session.name_step("Export", icon::EXPORT);
            self.commit_edit(Transaction(ops));
        }
    }

    /// Write one list to every subject, as a single undoable step.
    ///
    /// One transaction rather than one per layer, so it undoes as one.
    ///
    /// A wholesale replacement (the mixed row's override, a paste, a preset): each
    /// subject keeps its own item ids by position
    /// (`ondin_core::item::rekey_by_position`); a row edit of a shared list goes
    /// through [`Self::retarget_exports`] instead.
    fn write_exports(&mut self, subjects: &[NodeId], specs: &[ExportSpec]) {
        let tx = self.exports_tx(subjects, specs);
        if !tx.0.is_empty() {
            // `edit_note` returns early for a transaction whose `changes_ink` is
            // false, so an export edit does not arm the chrome hide.
            self.session.name_step("Export", icon::EXPORT);
            self.commit_edit(tx);
        }
    }

    pub(super) fn exports_tx(&mut self, subjects: &[NodeId], specs: &[ExportSpec]) -> Transaction {
        let session = &mut self.session;
        // One id per position past the end for the whole write (§15 D1003 (6)).
        let mut fresh = ondin_core::item::PastTheEnd::default();
        Transaction(
            subjects
                .iter()
                .map(|id| {
                    let own = session.doc.get(*id).map(|n| n.exports()).unwrap_or(&[]);
                    Operation::SetExports {
                        id: *id,
                        exports: ondin_core::item::rekey_by_position(
                            own,
                            specs.iter().cloned(),
                            &mut fresh,
                            &mut session.ids,
                        ),
                    }
                })
                .collect(),
        )
    }

    /// Append `specs` to whatever each subject already has.
    ///
    /// The `+`'s edit over a selection that agrees; a disagreeing one goes through
    /// [`Self::write_exports`], since appending there would leave the layers
    /// disagreeing, a state the card cannot be edited out of (`export_mixed_row`).
    pub(super) fn append_exports(&mut self, subjects: &[NodeId], specs: &[ExportSpec]) {
        let session = &mut self.session;
        // Items are minted once for every subject (§15 D980, D1003): minted per
        // subject, a main and its instance would get two items, and the instance's
        // would be a local addition its main's later edits skip.
        let added: Vec<Keyed<ExportSpec>> = specs
            .iter()
            .map(|s| Keyed::new(session.ids.mint_item(), s.clone()))
            .collect();
        let ops: Vec<Operation> = subjects
            .iter()
            .filter_map(|id| {
                let node = session.doc.get(*id)?;
                let mut next = node.exports().to_vec();
                next.extend(added.iter().cloned());
                Some(Operation::SetExports {
                    id: *id,
                    exports: next,
                })
            })
            .collect();
        if !ops.is_empty() {
            // Through the panel committer, like `write_exports`: the two halves of
            // one button must commit on the same seam.
            self.session.name_step("Add export", icon::EXPORT);
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
        let shared = self.shared_exports_keyed(&subjects);
        // Closed when the layer exports nothing, as Fill and Stroke are; the `+` in
        // the header opens it, so the header keeps its action while collapsed.
        let fresh = self.export_subject_changed();
        // An instance's ghost rows are something to show (§15 D981).
        let ghosts = match subjects.as_slice() {
            [one] => self.has_ghosts(Some(*one), |n| n.exports()),
            _ => false,
        };
        self.sync_paint_collapse(
            "Export",
            shared.as_ref().is_some_and(Vec::is_empty) && !ghosts,
            fresh,
        );
        // Three tooltips: over a disagreeing selection the `+` is destructive, and
        // says so before the click (`export_mixed_row`).
        let mixed = shared.is_none();
        let action = Some(ui::HeadAction::new(
            icon::PLUS,
            match (subjects.len(), mixed) {
                (1, _) => "Add an export",
                (_, false) => "Add an export to every selected layer",
                (_, true) => "Replace every selected layer's exports with one",
            },
        ));
        // Read before the body, which may edit the list under it.
        let add = self.panel(ui, "Export", action, |app, ui| {
            let Some(specs) = shared.clone() else {
                app.export_mixed_row(ui);
                app.export_buttons(ui, &subjects);
                return;
            };
            // One subject's box sizes every row; over a multi-selection the size
            // cell says there is no single answer (`export_row`).
            let sizing = (subjects.len() == 1).then(|| subjects[0]);
            // One instance's list read against its source's (§15 D981): each row's
            // trailing slot, and the ghost rows of the exports it removed.
            let source = sizing.and_then(|id| app.source_list(id, |n| n.exports()));
            if specs.is_empty() && source.as_ref().is_none_or(Vec::is_empty) {
                ui.label(
                    egui::RichText::new("No exports")
                        .size(11.0)
                        .color(theme::text::FAINT),
                );
                return;
            }
            let mut next = specs.clone();
            let (mut removed, mut changed) = (None, false);
            let states = source
                .as_ref()
                .map(|s| ondin_core::reset::item_states(s, &specs));
            let mut reset = None;
            let mut restore = false;
            // A zero-spacing region: the gap between exports is smaller than the
            // card's own row gap and a scope cannot subtract, so it is allocated.
            // That also leaves no trailing leak (`paint_unit`) above the button.
            ui.scope(|ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                for (i, spec) in specs.iter().enumerate() {
                    let item = states.as_ref().map(|s| s[i]);
                    let out = app.export_row(ui, i, spec, sizing, i + 1 == specs.len(), item);
                    if let Some(edited) = out.edited {
                        next[i].value = edited;
                        changed = true;
                    }
                    if out.remove {
                        removed = Some(i);
                    }
                    if out.reset {
                        reset = Some(spec.id);
                    }
                }
                // The main's exports this instance removed, as ghost rows.
                for gone in source
                    .as_deref()
                    .map(|s| ondin_core::reset::removed_items(s, &specs))
                    .unwrap_or_default()
                {
                    let label = format!("{} {}", gone.scale.label(), gone.format.label());
                    if super::component::ghost_row(ui, super::component::GhostLead::Plain, &label) {
                        restore = true;
                    }
                }
            });
            // Written whole, not retargeted: the source exists for one subject only,
            // so there is nothing to retarget across. The rows' own edits stand down
            // that frame. A ghost row's restore is the whole list back as the main
            // has it (§15 D994); a row's ↺ is that item.
            let list = match (restore, reset, source.as_deref()) {
                (true, _, Some(src)) => Some(src.to_vec()),
                (false, Some(item), Some(src)) => {
                    Some(ondin_core::reset::reset_item(src, &specs, item))
                }
                _ => None,
            };
            if let (Some(exports), Some(id)) = (list, sizing) {
                app.session
                    .name_step("Reset", icon::ARROW_COUNTER_CLOCKWISE);
                app.commit_edit(Transaction(vec![Operation::SetExports { id, exports }]));
                (removed, changed) = (None, false);
            }
            if let Some(i) = removed {
                next.remove(i);
                // The open block and the buffers are keyed by index; drop them rather
                // than point at whatever slid up (cf. `forget_stroke_chrome`).
                app.export_row_open = None;
                app.export_prefix_text = None;
                app.export_suffix_text = None;
                changed = true;
            }
            if changed {
                app.retarget_exports(&subjects, &specs, &next);
            }
            app.export_buttons(ui, &subjects);
            // Inside the card, not a card of its own; its caret still turns the cost off.
            app.inspector_export_preview(ui, &subjects);
        });
        if add {
            let seed = self.next_export_spec(&subjects);
            match mixed {
                // Over a disagreement the `+` overrides rather than appends: the
                // layers would still disagree, with no way out (`export_mixed_row`).
                true => self.write_exports(&subjects, &[seed]),
                false => self.append_exports(&subjects, &[seed]),
            }
            // The panel may be shut (it is, on a layer with no exports); a `+` that
            // adds an unseen row does nothing.
            self.open_paint_panel("Export");
        }
    }

    /// The **Preview** section: what the first export actually comes out as.
    ///
    /// Rendered only while the section is open, and at most once per edit: a preview
    /// is a real render and encode. `EditorSession::revision` is the cache key's
    /// third term beside the layer and the spec. Collapsed by default, which is also
    /// the switch that turns the cost on.
    ///
    /// One layer only; a multi-selection has no single subject to show.
    fn inspector_export_preview(&mut self, ui: &mut egui::Ui, subjects: &[NodeId]) {
        let [subject] = subjects else { return };
        let (subject, spec) = match self.shared_exports(subjects) {
            // The row whose settings are open, else the first.
            Some(specs) if !specs.is_empty() => {
                let i = self.export_row_open.unwrap_or(0).min(specs.len() - 1);
                (*subject, specs[i].clone())
            }
            _ => return,
        };
        // Closing the section frees the texture.
        let open = !self.collapsed_panels.contains(PREVIEW);
        if !open {
            self.export_preview = None;
        }
        // A section inside the card: `section_head` rather than `panel`, with no
        // second ground behind it.
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
                // Fit the width, never magnify.
                let (w, h) = (preview.pixels.0 as f32, preview.pixels.1 as f32);
                let avail = ui.available_width();
                let scale = (avail / w).min(1.0);
                let shown = egui::vec2(w * scale, h * scale);
                // On the checkerboard, inset from it: the only
                // way a transparent export differs from a white one.
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
        // No raster for a vector file: rasterizing it would preview something other
        // than the file.
        if !key.spec.format.is_raster() {
            // The reported form (§15 D780): the card is read before exporting, when
            // a fidelity note can still change what the user does. A PDF is not
            // converted per edit, so its line has no size.
            let (head, note) = match key.spec.format {
                ExportFormat::Pdf => {
                    let approximated =
                        ondin_export::pdf::approximated(doc, res, &[key.node], key.spec.text);
                    ("PDF".to_string(), fidelity_clauses(&approximated, &[]))
                }
                _ => {
                    let out = ondin_export::svg::svg_with(doc, res, &[key.node], key.spec.text);
                    let note = fidelity_clauses(&out.approximated, &out.round_trip);
                    (format!("SVG · {}", human_bytes(out.svg.len())), note)
                }
            };
            let summary = match note {
                None => head,
                Some(note) => format!("{head} · {note}"),
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
        // The cap is on the preview, not the export: past it the section reports the
        // size it would be and renders nothing.
        const MAX_PREVIEW_PX: u64 = 4_000_000;
        // Measured against the size the raster will be, not `plan::raster_size`'s:
        // `raster_of` pads to a square after this guard, so a 32768×120 layer with
        // *Pad out to a square* would pass the area cap and ask for a 4 GB buffer.
        let (pw, ph) = match key.spec.pad_square {
            true => (w.max(h), w.max(h)),
            false => (w, h),
        };
        // A per-side test beside the area one: `ctx.load_texture` panics past
        // `max_texture_side`, and a thin raster is under any area cap. The limit is
        // the device's, read off the context.
        let side = ctx.input(|i| i.max_texture_side) as u64;
        if pw as u64 * ph as u64 > MAX_PREVIEW_PX || pw as u64 > side || ph as u64 > side {
            return Some(ExportPreview {
                key,
                texture: None,
                // The padded pair: that is the file the row is about.
                pixels: (pw, ph),
                summary: format!("{pw}×{ph} — too large to preview here"),
            });
        }

        // Rasterized once, then both encoded and drawn, not rendered twice per edit.
        let opts = ondin_export::plan::raster_opts(doc, res, key.node, &key.spec);
        let raster = ondin_export::png::raster_of(doc, res, &[key.node], &opts)?;
        let bytes = ondin_export::plan::encode(&raster, &key.spec);
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
    /// 1×, 2×, 3×, then 4× for ever after (the icon workflow); chosen by counting
    /// the rows, not by the first multiplier not yet used.
    ///
    /// The format is inherited from the last row, since a list that is all JPEG is a
    /// decision about the whole list; PNG when there is nothing to inherit,
    /// including a selection that disagrees. A repeated name is safe on disk:
    /// `plan::de_collide` gives the second file its ` (2)`.
    fn next_export_spec(&self, subjects: &[NodeId]) -> ExportSpec {
        let existing = self.shared_exports(subjects).unwrap_or_default();
        let format = existing.last().map_or(ExportFormat::Png, |s| s.format);
        ExportSpec::new(format, ladder_scale(existing.len()))
    }

    /// What a selection whose layers export differently shows instead of rows.
    ///
    /// It offers no list to edit, and one line saying what the `+` will do: it
    /// replaces every selected layer's list with the one export it adds, so the
    /// state resolves in one click. Appending would never leave this state.
    fn export_mixed_row(&mut self, ui: &mut egui::Ui) {
        ui.label(
            egui::RichText::new("Mixed exports. Add to override.")
                .size(11.0)
                .color(theme::text::FAINT),
        );
    }

    /// One export: its scale, its format, the size it will come out at, and the
    /// two buttons — and, on an instance's list, the item's reserved trailing slot
    /// before the ✕ (`item`, §15 D981): nothing while it follows its main, the dot
    /// when it differs, `+` when it is the instance's own.
    fn export_row(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        spec: &ExportSpec,
        sizing: Option<NodeId>,
        last: bool,
        item: Option<ondin_core::reset::ItemState>,
    ) -> RowOut {
        let mut out = RowOut::default();
        // The slot's room, taken from the format cell — which takes what is left.
        let slot = match item {
            Some(_) => SLOT_W + PITCH,
            None => 0.0,
        };
        // The whole row under the pointer is what turns the dot into ↺, asked
        // before it is laid out (`component::item_slot`'s caveat).
        let hot = ui.rect_contains_pointer(egui::Rect::from_min_size(
            ui.cursor().min,
            egui::vec2(ui.available_width(), CELL),
        ));
        let open = self.export_row_open == Some(index);
        let mut head = None;
        // The row and its size line are one unit, further from the next unit than
        // its halves are from each other (`paint_unit`). `item_spacing` is zero and
        // both gaps are allocated: egui spends the spacing in effect when a widget is
        // placed after it, so a spacing set for the two lines is also spent after the
        // last of them (§15 D195).
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = 0.0;
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = PITCH;
                // Without this a `ComboBox` comes out 26 against the 28px controls
                // beside it, as in the stroke row's alignment combo.
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
                            // Fixed sizes sit under a rule: a multiplier is "denser",
                            // these are "this big". Other values are typed in the
                            // settings block.
                            super::inspector::popup_rule(ui);
                            for s in [ExportScale::Width(512), ExportScale::Height(512)] {
                                ui.selectable_value(&mut scale, s, s.label());
                            }
                        });
                    } else {
                        // Dimmed and still there, holding its value: hiding it would
                        // move its neighbours, resetting it would lose the setting.
                        combo
                            .show_ui(ui, |ui| {
                                ui.label(
                                    egui::RichText::new(format!(
                                        "{} has no pixels to scale",
                                        spec.format.label()
                                    ))
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

                    // The format cell takes what is left.
                    let mut format = spec.format;
                    let format_w = ui.available_width() - CELL * 2.0 - PITCH * 2.0 - slot;
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
                    // The panel's two popovers are exclusive: they hang from the same
                    // card a few points apart, so both open would cover one another.
                    self.export_menu = false;
                }
                head = Some(more);
                if let Some(state) = item {
                    out.reset = super::component::item_slot(ui, state, hot);
                }
                // An ✕, not a minus: `MINUS` is the off mark in this app's switches, so
                // it would read as a state rather than a verb.
                if ui::field_button(ui, icon::X, CELL, 14.0, ui::FieldButton::Off)
                    .on_hover_text("Remove this export")
                    .clicked()
                {
                    out.remove = true;
                }
            });
            // The size the row will produce, on a line of its own: its width varies,
            // so it has no fixed cell. Blank, and the line absent, over a
            // multi-selection (`export_size_label`).
            let size = self.export_size_label(spec, sizing);
            let lined = !size.is_empty();
            if lined {
                ui.allocate_space(egui::vec2(0.0, ROW_LINE_GAP));
                // `interact_size.y` is a minimum row height (24 in the theme); zeroed
                // so the line is exactly as tall as its ink and the gaps mean what
                // they say.
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
            // pitch at zero (see the loop). The last row allocates none, so the card's
            // own row gap separates the list from the button below. A row with no size
            // line spends that line's gap too ([`ROW_UNIT_GAP`] is for two-line
            // units); the sum is deliberately not `ui::CARD_ROW_GAP`.
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
    /// Blank over a multi-selection rather than a guess: `Width(512)` is a different
    /// multiplier on every differently-shaped layer.
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
    /// A popover rather than an inline block, which would push the export button and
    /// every row under it down the column. The shell is the shared one: `POPOVER_W`
    /// wide, `menu_frame`'s padding, `popover_anchor`'s lane, and the click-away rule
    /// every other card obeys.
    fn export_settings_popup(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        spec: &ExportSpec,
        // The one subject, or `None` over a multi-selection (§15 D657).
        sizing: Option<NodeId>,
        head: &egui::Response,
    ) -> Option<ExportSpec> {
        // Only `Some` on a frame the edit is finished: the one scrubbable control
        // holds its in-flight value itself (`export_quality_field`).
        let mut out: Option<ExportSpec> = None;
        let mut close = false;
        let ctx = ui.ctx().clone();
        let anchor = Self::popover_anchor(&ctx, head.rect, POP_W);
        let menu = egui::Area::new(egui::Id::new(("export-settings", index)))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            // Keeps the card on screen when its row is near the bottom.
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

                    // --- prefix, then suffix (filename order) ------------------
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
                        // The hint is the marker the scale derives; at 1× there is
                        // none and the field explains itself instead.
                        &match spec.effective_suffix() {
                            s if s.is_empty() => "e.g. -dark".to_string(),
                            s => s,
                        },
                        "Put after the layer's name. Left empty it follows the scale",
                    ) {
                        edit(&mut out, spec).suffix = text;
                    }

                    // --- quality, JPEG only (labelled for its format) ----------
                    if spec.format == ExportFormat::Jpeg {
                        section(ui, "JPEG quality", |ui| {
                            if let Some(q) = self.export_quality_field(ui, index, spec) {
                                edit(&mut out, spec).quality = q;
                            }
                        });
                    }

                    // --- text, vector only ---------------------------------------
                    if !spec.format.is_raster() {
                        section(ui, "Text", |ui| {
                            ui.scope(|ui| {
                                ui.spacing_mut().interact_size.y = POP_CELL;
                                ui.spacing_mut().button_padding.y = 0.0;
                                let mut text = spec.text;
                                egui::ComboBox::from_id_salt(("export-text", index))
                                    .icon(ui::combo_chevron)
                                    .width(POP_INNER)
                                    .selected_text(ui::glyph_and_text(
                                        icon::TEXT_T,
                                        spec.text.label(spec.format),
                                    ))
                                    .show_ui(ui, |ui| {
                                        ui::menu_rows(ui);
                                        ui.spacing_mut().button_padding.y = 2.0;
                                        for t in [ExportText::Live, ExportText::Outlines] {
                                            ui.selectable_value(&mut text, t, t.label(spec.format));
                                        }
                                    })
                                    .response
                                    .on_hover_text(match spec.text {
                                        ExportText::Live => {
                                            "Selectable text, drawn with the fonts the reader has"
                                        }
                                        ExportText::Outlines => {
                                            "The glyphs as shapes: exact everywhere, no longer text"
                                        }
                                    });
                                if text != spec.text {
                                    edit(&mut out, spec).text = text;
                                }
                            });
                        });
                    }

                    // --- background --------------------------------------------
                    //
                    // The row says what the setting is, not what it resolves to; the
                    // JPEG substitution is stated in the tooltip.
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
                                    // White, not "Colour": the model carries any
                                    // colour but the picker targets a paint slot on
                                    // a node, so none can be picked here.
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
                                    // Plural where the answer is plural (§15 D657):
                                    // the chip is muted over a multi-selection.
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

                    // --- the two switches, at the card's own section gap -------
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
                            false => "Trimming reads pixels, and a vector file has none",
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

        // The panel's one click-away rule (`panels::dismissed_by_click`), whole rather
        // than hand-rolled (§15 D82).
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
                // Primary only: a scrub (the JPEG quality) is cancelled with a
                // right-click, which `any_click` would read as a click on the page.
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
    /// A hand-wound scrub, because neither shared committer works: `edit_valve`
    /// previews through `RenderOverrides`, which absorbs `SetExports` as a no-op (the
    /// number would not move), and `commit_run` commits every frame while
    /// `cancel_gesture` only drops the preview (a cancel would leave the value).
    ///
    /// The in-flight value lives in `export_quality_scrub`: the field reads it during
    /// the drag, the release commits it once, a cancel drops it (as `session_scrub`).
    /// The cancel is checked before the buffer is read: on that frame egui may still
    /// report a `changed()`, which would commit the last movement.
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
            // 0.25, slower than the panel's other 0–100 fields: nothing visibly moves
            // while dragging, and the useful band is about 60–95.
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
    /// One function for the pair, which differ in three strings: the commit-on-blur
    /// rule lives once.
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
            // Committed on losing focus, not per keystroke (one undo step per name).
            if editing && resp.lost_focus() {
                *match part {
                    NamePart::Prefix => &mut self.export_prefix_text,
                    NamePart::Suffix => &mut self.export_suffix_text,
                } = None;
                // `Escape` abandons the name (§15 D808, `crate::ui::defocus_commits`);
                // the buffer goes either way.
                if text != current && crate::ui::defocus_commits(&resp) {
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
    /// The stored setting is untouched: switching a JPEG back to PNG shows
    /// Transparent again.
    fn background_label(&self, spec: &ExportSpec) -> &'static str {
        match spec.background {
            ExportBackground::Solid(_) => "White",
            b => b.label(),
        }
    }

    /// The colour of the chip in front of that label — the colour that will
    /// actually be behind the export.
    ///
    /// Resolved through `plan::background_color`, not read off the setting: *Frame's
    /// own* differs per layer and is nothing outside a frame. A transparent export
    /// gets the muted ink. It is a chip, not the checkerboard, because a
    /// `ComboBox`'s selected text can carry a glyph but not a painter.
    fn background_swatch(&self, spec: &ExportSpec, sizing: Option<NodeId>) -> egui::Color32 {
        // Blank over a multi-selection rather than a guess (§15 D657). Only `Frame`
        // varies per layer, so this is a `match` and not a gate on `sizing`:
        // `Solid` and `Transparent` hold for any number of layers.
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
    /// The zip button appears exactly when there is more than one file (several
    /// layers, or one layer with several specs), which is also where the destination
    /// dialog turns: one file gets a save dialog, several get a folder.
    fn export_buttons(&mut self, ui: &mut egui::Ui, subjects: &[NodeId]) {
        let files = self.export_plan(subjects);
        if files.is_empty() {
            // The menu is drawn here, so it is closed here: otherwise it stays
            // flagged open with nothing on screen to dismiss it.
            self.export_menu = false;
            return;
        }
        // No `add_space` before it: `ui::CARD_ROW_GAP` already separates a card's
        // children (§15 D180).
        let label = match (subjects.len(), self.session.doc.get(subjects[0])) {
            // The layer's name, so the button is worth reading before pressing.
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
            // The name again: `action_button` elides a long layer name. Only in the
            // single-layer arm; `Export 3 layers` cannot elide.
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
        // Escape closes both of this card's popovers (§15 D801). `key_pressed` rather
        // than `consume_key`: `OndinApp::a_popover_owns_escape` is what keeps the
        // ladder from seeing it. Heard only when one is open, so a card with nothing
        // open does not vouch for a Type popover nobody draws (§15 D847).
        if self.export_menu || self.export_row_open.is_some() {
            self.popover_heard = true;
        }
        if ui.ctx().input(|i| i.key_pressed(egui::Key::Escape)) {
            self.export_menu = false;
            self.export_row_open = None;
        }
    }

    /// The panel's own menu: what to do with a set of settings, and the two
    /// switches that change what *every* export does.
    ///
    /// A popover like [`Self::export_settings_popup`], but that one holds the values
    /// of one row and this holds verbs and preferences that belong to no row;
    /// eleven of them in the card would bury the button they sit under.
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
                        // `FOLDER_SIMPLE`, not `FOLDER_OPEN` two rows above: this
                        // folder is being made, not entered.
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
                        // `ARROW_CLOCKWISE` also prefixes Transform's
                        // rotation, but the two never appear together; it must not
                        // be `FLOPPY_DISK`, which is *Save as a preset* above.
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

        // The panel's one click-away rule (`panels::dismissed_by_click`).
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
        // A verb closes it, a switch does not: closing after one switch would make
        // turning both a two-trip job.
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
                    // Named from what it holds, so no modal is needed mid-action; a
                    // second preset of the same shape is the same preset.
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
                // Every row carries its own scale; the override is for the CLI
                // (§15 D361).
                scale_override: None,
                unset: None,
            },
        )
    }

    /// Render a plan and write it — to a file, into a folder, or into one archive.
    ///
    /// Three destinations: a plan of one file gets a save dialog with the name in
    /// it, a larger one gets a folder, and the zip is asked for explicitly.
    ///
    /// Reads the committed document and `Resolved`, as every writer does, so a
    /// gesture mid-drag cannot leak an uncommitted position into a file.
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
                // A planned path is not a file name (§15 D656): with *Slashes in
                // names make folders* on, `icons/close.png`'s folder half is a
                // location for `set_directory`.
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

    /// Render and write on a worker, saying so; [`Self::poll_exports`] reports what
    /// happened when it lands.
    ///
    /// The worker gets a copy of the committed document and its `Resolved`, so an
    /// edit made while it runs is not in the files — they are what was on screen
    /// when the export was asked for.
    fn write_export(&mut self, files: Vec<PlannedFile>, destination: Destination) {
        let (doc, resolved) = (self.session.doc.clone(), self.session.resolved.clone());
        let ctx = self.egui_ctx.clone();
        self.session.info(EXPORTING);
        let job = crate::job::Job::spawn("ondin-export", &ctx, move || {
            deliver(&doc, &resolved, &files, destination)
        });
        self.exports.push(job);
    }

    /// Report the exports that have landed, and keep *Exporting…* on the status
    /// line while any is still running. Every frame.
    pub(crate) fn poll_exports(&mut self, ctx: &egui::Context) {
        let mut landed = Vec::new();
        self.exports.retain(|job| match job.poll() {
            Ok(None) => true,
            Ok(Some(d)) => {
                landed.push(Some(d));
                false
            }
            Err(()) => {
                landed.push(None);
                false
            }
        });
        for d in landed {
            match d {
                Some(d) => self.land_export(d),
                None => self.session.fail("Could not export: the writer stopped"),
            }
        }
        if !self.exports.is_empty() {
            // Said again so it outlasts the status line's hold, but only while
            // nothing newer has replaced it.
            let status = self.session.status();
            if status.text == EXPORTING && status.at.elapsed().as_secs_f32() > 1.0 {
                self.session.info(EXPORTING);
            }
            ctx.request_repaint_after(std::time::Duration::from_secs(1));
        }
    }

    /// Wait for every running export and report it (tests only).
    #[cfg(test)]
    pub(crate) fn settle_exports(&mut self) {
        for job in std::mem::take(&mut self.exports) {
            match job.wait() {
                Some(d) => self.land_export(d),
                None => self.session.fail("Could not export: the writer stopped"),
            }
        }
    }

    /// Report a finished export, and remember where it went.
    fn land_export(&mut self, d: Delivered) {
        let Delivered {
            written,
            skipped,
            replaced,
            to_folder,
            failed,
            where_,
        } = d;
        if written == 0 && failed.is_none() {
            self.session
                .fail("Nothing to export — none of that has any size");
            return;
        }
        // Remembered for the next export, and for *Show in folder*. Saved eagerly:
        // the app has no shutdown hook to rely on.
        if written > 0 {
            self.prefs.export_dir = Some(where_.clone());
            self.prefs.save();
        }
        // The count first, the loss next, the path last: `OndinApp::status_bar`
        // truncates, and the folder can be re-derived from *Show in folder*.
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
    /// all* on the canvas menu.
    ///
    /// Stops at the first one, unlike [`Self::exportable_layers`]: the menu asks
    /// every frame it is open.
    pub(crate) fn document_exports_anything(&self) -> bool {
        let mut stack: Vec<NodeId> = self.session.doc.gathered_pages().collect();
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
    /// The whole document, not the selection: the export settings on the layers
    /// already say which they are. A draft page's layers are left out.
    pub(crate) fn exportable_layers(&self) -> Vec<NodeId> {
        let mut out = Vec::new();
        let mut stack: Vec<NodeId> = self.session.doc.gathered_pages().collect();
        while let Some(id) = stack.pop() {
            let Some(node) = self.session.doc.get(id) else {
                continue;
            };
            if !node.exports().is_empty() {
                out.push(id);
            }
            // The walk's order is discarded: `all_in_document_order` below re-sorts
            // from the root.
            stack.extend(node.children().iter().copied());
        }
        // `all_in_document_order`, not `in_document_order` (§15 D516): the latter
        // drops any id with an ancestor in the set, but an export spec on a nested
        // layer is a deliberate instruction.
        ondin_core::build::all_in_document_order(&self.session.doc, &out)
    }

    /// Re-run every export in the document, into the folder the last one went to.
    ///
    /// No dialog when there is somewhere to put it. The first run of a session asks;
    /// every one after it does not, and the status line says where the files went.
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

    /// *Export all frames…* — every visible top-level frame through the ordinary
    /// dialog: its own export rows when it has any, else a 1× PNG.
    pub(crate) fn export_frames(&mut self) {
        let files = self.frames_plan();
        if files.is_empty() {
            self.session.info("No frames to export");
            return;
        }
        self.run_export(files, false);
    }

    /// The frames *Export all frames…* writes: the current page's top-level ones,
    /// less the hidden ones, which draw nothing.
    pub(crate) fn exported_frames(&self) -> Vec<NodeId> {
        self.frames_on(self.session.page)
    }

    /// `page`'s visible top-level frames, in paint order.
    fn frames_on(&self, page: NodeId) -> Vec<NodeId> {
        let doc = &self.session.doc;
        doc.get(page)
            .map(|n| n.children().to_vec())
            .unwrap_or_default()
            .into_iter()
            .filter(|id| {
                doc.get(*id).is_some_and(|n| {
                    n.visible() && matches!(n.kind(), ondin_core::NodeKind::Artboard { .. })
                })
            })
            .collect()
    }

    /// What *Export all frames…* writes.
    pub(crate) fn frames_plan(&self) -> Vec<PlannedFile> {
        self.plan_frames(&self.exported_frames())
    }

    fn plan_frames(&self, frames: &[NodeId]) -> Vec<PlannedFile> {
        ondin_export::plan(
            &self.session.doc,
            &self.session.resolved,
            frames,
            PlanOptions {
                folders_from_names: self.prefs.export_folders_from_names,
                scale_override: None,
                unset: Some(ExportSpec::new(ExportFormat::Png, ExportScale::Times(1.0))),
            },
        )
    }

    /// What *Export frames, all pages…* writes: each page that is not a
    /// draft planned as *Export all frames…* plans one, into a folder named after
    /// the page — numbered when two pages share a name — and how many drafts were
    /// left out.
    pub(crate) fn all_pages_frames_plan(&self) -> (Vec<PlannedFile>, usize) {
        let doc = &self.session.doc;
        let mut folders: Vec<String> = Vec::new();
        let mut out = Vec::new();
        for page in doc.gathered_pages() {
            // A slash in a page's name would make a folder of its own.
            let base = self.page_name(page).replace(['/', '\\'], "-");
            let folder = std::iter::once(base.clone())
                .chain((2..).map(|n| format!("{base} {n}")))
                .find(|f| !folders.contains(f))
                .expect("an unbounded range finds a free name");
            for mut file in self.plan_frames(&self.frames_on(page)) {
                file.path = format!("{folder}/{}", file.path);
                out.push(file);
            }
            folders.push(folder);
        }
        let drafts = doc.pages().iter().filter(|p| p.draft).count();
        (out, drafts)
    }

    /// *Export frames, all pages…* — [`Self::export_frames`] for every page
    /// that is not a draft, one folder per page.
    pub(crate) fn export_frames_all_pages(&mut self) {
        let (files, drafts) = self.all_pages_frames_plan();
        if files.is_empty() {
            self.session.info("No frames to export");
            return;
        }
        if drafts > 0 {
            self.session.info(match drafts {
                1 => "1 draft page is left out".to_string(),
                n => format!("{n} draft pages are left out"),
            });
        }
        self.run_export(files, false);
    }

    /// The pages *Export frames as one PDF…* writes: each of the current page's
    /// visible top-level frames, named for its bookmark.
    pub(crate) fn frames_pdf_pages(&self) -> Vec<(String, Vec<NodeId>)> {
        self.exported_frames()
            .into_iter()
            .filter_map(|id| {
                let name = self.session.doc.get(id)?.name().to_string();
                Some((name, vec![id]))
            })
            .collect()
    }

    /// *Export frames as one PDF…* — the current page's visible top-level frames
    /// as one PDF, a page and a bookmark each, into a file the person names.
    pub(crate) fn export_frames_pdf(&mut self) {
        let pages = self.frames_pdf_pages();
        if pages.is_empty() {
            self.session.info("No frames to export");
            return;
        }
        let Some(export) = ondin_export::pdf::pdf_pages(
            &self.session.doc,
            &self.session.resolved,
            &pages,
            ondin_core::ExportText::Live,
        ) else {
            self.session.fail("Could not make the PDF");
            return;
        };
        let stem = self
            .session
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().into_owned())
            .unwrap_or_else(|| self.page_name(self.session.page));
        let Some(path) = rfd::FileDialog::new()
            .set_directory(self.export_start_dir())
            .add_filter("PDF", &["pdf"])
            .set_file_name(format!("{stem}.pdf"))
            .save_file()
        else {
            return;
        };
        if let Err(e) = std::fs::write(&path, &export.pdf) {
            self.session.fail(format!("Could not write: {e}"));
            return;
        }
        if let Some(dir) = path.parent() {
            self.prefs.export_dir = Some(dir.into());
            self.prefs.save();
        }
        let n = pages.len();
        self.session.info(match export.approximated.len() {
            0 => format!("Exported {n} frames as one PDF"),
            k => format!("Exported {n} frames as one PDF — {k} drawn differently"),
        });
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

    /// The selection written as `target`, or the sentence saying why there is
    /// nothing to write. Reads the committed document, as every writer does.
    pub(crate) fn code_export(
        &self,
        target: ondin_export::code::Target,
    ) -> Result<ondin_export::code::CodeExport, &'static str> {
        let subjects = self.export_subjects();
        if subjects.is_empty() {
            return Err("Select the layers to export as code");
        }
        Ok(target.export(&self.session.doc, &self.session.resolved, &subjects))
    }

    /// Export the selection as `target` code into one zip the person names, and
    /// say what the report holds.
    pub(crate) fn export_code(&mut self, target: ondin_export::code::Target) {
        let export = match self.code_export(target) {
            Ok(e) => e,
            Err(why) => {
                self.session.fail(why);
                return;
            }
        };
        let name = match self.export_subjects().as_slice() {
            [one] => self
                .session
                .doc
                .get(*one)
                .and_then(|n| code_archive_name(n.name())),
            _ => None,
        }
        .unwrap_or_else(|| self.export_archive_name());
        let Some(path) = rfd::FileDialog::new()
            .set_directory(self.export_start_dir())
            .add_filter("ZIP archive", &["zip"])
            .set_file_name(name)
            .save_file()
        else {
            return;
        };
        let written = export.zip().and_then(|bytes| std::fs::write(&path, bytes));
        if let Err(e) = written {
            self.session.fail(format!("Could not write: {e}"));
            return;
        }
        if let Some(dir) = path.parent() {
            self.prefs.export_dir = Some(dir.into());
            self.prefs.save();
        }
        self.session.info(code_export_summary(target, &export));
    }
}

/// The zip a save dialog is seeded with for a layer named `layer`: a file name,
/// never a path (`Button / Primary` would be read as a folder), or `None` when
/// nothing of the name is usable.
fn code_archive_name(layer: &str) -> Option<String> {
    let name = ondin_export::plan::sanitize_segment(layer.trim());
    (!name.is_empty()).then(|| format!("{name}.zip"))
}

/// The status line after a code export: what was written, then what the report
/// lists, by count.
fn code_export_summary(
    target: ondin_export::code::Target,
    export: &ondin_export::code::CodeExport,
) -> String {
    let mut notes = Vec::new();
    for (n, what) in [
        (export.approximated.len(), "drawn differently"),
        (export.positioned.len(), "with fixed positions"),
        (export.inlined.len(), "written out as layers"),
    ] {
        if n > 0 {
            notes.push(format!("{n} {what}"));
        }
    }
    match notes.is_empty() {
        true => format!("Exported {} code", target.label()),
        false => format!(
            "Exported {} code — {}; see report.txt",
            target.label(),
            notes.join(", ")
        ),
    }
}

/// What *Export all* does when it has never been told where to put things.
///
/// A menu row or a chord is a person asking, so a folder dialog is right; a save
/// with the switch on must not raise a modal on Ctrl+S.
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
/// The decision "Export on save must never open a modal" lives apart from
/// `rfd::FileDialog` and `std::fs::write`, which even a headless app would drive
/// for real (§15 D626). Collapsing the `Quietly` arm into `Asking`, or dropping the
/// `is_dir` filter on the remembered folder, would pop a dialog on every Ctrl+S.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ExportRoute {
    /// Straight to the remembered folder, whoever asked.
    Write(std::path::PathBuf),
    /// One sentence and nothing else — the silent arm.
    SayNoDestination,
    /// Open the folder picker.
    AskForFolder,
}

/// Which dialog an explicit *Export* opens: [`OndinApp::run_export`]'s
/// `(zip, files.len())` table, lifted for [`ExportRoute`]'s reason (§15 D626).
///
/// One file is a *Save as* with its own name and format filter; more is a folder
/// pick. A `zip` overrides both, so it is matched first.
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
        // Silent rather than modal: it says so once, and the next explicit export
        // teaches it where.
        (None, ExportAll::Quietly) => ExportRoute::SayNoDestination,
        (None, ExportAll::Asking) => ExportRoute::AskForFolder,
    }
}

/// Splits a planned path into the directory and the file name the one-file save
/// dialog wants (§15 D656): with *Slashes in names make folders* on, `icons/close`
/// plans `icons/close.png`, whose folder half belongs in `set_directory`.
///
/// The joined directory is used only if it already exists: platform dialogs differ
/// on a `set_directory` that does not resolve, and the folder is created on the
/// write side regardless. Lifted out of `run_export` because that opens a modal.
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
            // A vector file has no size to list, as the row's dimmed scale says.
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

/// The status line's word while an export runs.
const EXPORTING: &str = "Exporting…";

/// What an export worker did: the files written, those with no size, how many a
/// folder export replaced, and the first write error. `written` and `failed` both
/// empty means nothing had a size.
pub(crate) struct Delivered {
    written: usize,
    skipped: usize,
    replaced: usize,
    to_folder: bool,
    failed: Option<String>,
    where_: std::path::PathBuf,
}

/// Render `files` and write them to `destination`, off the UI thread.
///
/// A failure stops nothing that has already succeeded, and is reported rather
/// than raised.
fn deliver(
    doc: &ondin_core::Document,
    resolved: &ondin_core::Resolved,
    files: &[PlannedFile],
    destination: Destination,
) -> Delivered {
    let mut rendered: Vec<(String, Vec<u8>)> = Vec::with_capacity(files.len());
    let mut skipped = 0usize;
    for f in files {
        match ondin_export::plan::render(doc, resolved, f) {
            Some(bytes) => rendered.push((f.path.clone(), bytes)),
            // The one refusal both writers make: a subject with no bounds at
            // all — an empty group, a layer whose contents are all hidden.
            None => skipped += 1,
        }
    }
    let mut out = Delivered {
        written: 0,
        skipped,
        replaced: 0,
        to_folder: false,
        failed: None,
        where_: Default::default(),
    };
    if rendered.is_empty() {
        return out;
    }
    // The folder arm owes a replacement count (§15 D636): a File or Archive was
    // named in a save dialog, but a Folder export can arrive with no dialog
    // (`export_all` takes `prefs.export_dir`, and *Re-export on save* fires it
    // on an autosave), replacing same-named files with a bare `fs::write`.
    match destination {
        Destination::File(path) => {
            out.where_ = path.parent().map(Into::into).unwrap_or_default();
            // As the folder arm does (§15 D656): a name like `icons/close`
            // may carry a folder the save dialog did not create.
            if let Some(parent) = path.parent() {
                let _ = std::fs::create_dir_all(parent);
            }
            match std::fs::write(&path, &rendered[0].1) {
                Ok(()) => out.written = 1,
                Err(e) => out.failed = Some(e.to_string()),
            }
        }
        Destination::Archive(path) => {
            out.where_ = path.parent().map(Into::into).unwrap_or_default();
            let packed =
                ondin_export::plan::zip(&rendered).and_then(|bytes| std::fs::write(&path, bytes));
            match packed {
                Ok(()) => out.written = rendered.len(),
                Err(e) => out.failed = Some(e.to_string()),
            }
        }
        Destination::Folder(dir) => {
            for (name, bytes) in &rendered {
                let path = dir.join(name);
                // The `/` in a name is a folder the user asked for, and it
                // may not exist yet. Created here rather than by the plan,
                // which never touches the filesystem.
                if let Some(parent) = path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                // Asked before the write: afterwards it always exists.
                let existed = path.exists();
                match std::fs::write(&path, bytes) {
                    Ok(()) => {
                        out.written += 1;
                        if existed {
                            out.replaced += 1;
                        }
                    }
                    Err(e) => out.failed = out.failed.or(Some(e.to_string())),
                }
            }
            out.to_folder = true;
            out.where_ = dir;
        }
    }
    out
}

fn export_count(n: usize) -> String {
    match n {
        1 => "Exported 1 file".to_string(),
        n => format!("Exported {n} files"),
    }
}

/// The popover's controls all edit one spec and must start from the same copy, or a
/// second control's fresh clone would drop the first one's change.
///
/// A free function because a closure that captures `out` and returns a borrow of it
/// cannot be spelled.
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

/// The quality field's whole decision, as a function of its inputs alone.
///
/// The cancel is answered first: on that frame egui may still report a `changed()`,
/// which would commit the movement the cancel discards. A drag holds and never
/// commits, since `cancel_gesture` undoes a scrub by dropping a preview and this
/// field has none. The release is recognised by the buffer, not `drag_stopped()`,
/// which is not reported on every path out of a drag.
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
/// A function of how many rows there are, not of which multipliers are free (see
/// [`OndinApp::next_export_spec`]); free so the rule is testable on its own.
fn ladder_scale(rows: usize) -> ExportScale {
    ExportScale::Times(rows.min(3) as f32 + 1.0)
}

/// The settings button's state. [`ui::FieldButton::Set`] says there is something
/// in a closed popover: the border marks content, the fill marks open (§15 D72).
fn settings_button_state(spec: &ExportSpec, open: bool) -> ui::FieldButton {
    // Not the quality: every JPEG has one, so the mark would mean "this is a JPEG".
    let at_rest = spec.prefix.is_empty()
        && spec.suffix.is_empty()
        && spec.background == ExportBackground::Transparent
        && !spec.trim
        && !spec.pad_square
        // Outlines kept from a vector format are inert on a raster, as the
        // popover hides them.
        && (spec.format.is_raster() || spec.text == ExportText::Live);
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
    //! The free functions the panel's decisions were lifted into, plus whether the
    //! row's cells and gaps fit the card they are drawn in.

    use super::*;
    use ondin_core::ExportBackground;

    fn spec(format: ExportFormat, scale: ExportScale) -> ExportSpec {
        ExportSpec::new(format, scale)
    }

    /// The row's cells fit the inspector's 256pt content width, and the widest text
    /// each cell can hold fits its cell. An overflowing row does not error: egui
    /// widens the combo and squeezes the size label.
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

        // What a `ComboBox` spends beside its text, off egui's own arithmetic
        // (`combo_box_dyn`): the text fits its cell when `galley + icon_spacing +
        // icon_width ≤ width - 2·padding`, else the combo comes out wider than asked.
        // Measuring by laying one out does not work: with no `.width()` a combo takes
        // `max(content, spacing.combo_width)`, and its text uses `TextStyle::Button`.
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

        // Taken from the dropdown, so a new `ExportScale::COMMON` entry re-runs this.
        // A typed value can be wider (`1024h` wants 70.9); what a click produces
        // must fit.
        let widest_scale = ExportScale::COMMON
            .iter()
            .map(|n| ExportScale::Times(*n))
            .chain([ExportScale::Width(512), ExportScale::Height(512)])
            .map(|s| combo_text(&s.label()))
            .fold(0.0_f32, f32::max);
        // The format cell's text is not in `TextStyle::Button`: `ui::glyph_and_text`
        // builds its own `LayoutJob` (15pt icon, 7pt lead, word at `proportional(12)`).
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
        // The format cell is `1fr`: what is asserted is that the leftover is enough.
        let format_w = CARD - SCALE_W - CELL * 2.0 - PITCH * 3.0;
        let wants = widest_format + format_glyph + overhead;
        assert!(
            wants <= format_w,
            "the format cell gets {format_w:.1} and wants {wants:.1} \
             ({widest_format:.1} of word + {format_glyph:.1} of glyph and lead + {overhead:.1} of combo)"
        );
    }

    /// A closed row with a typed setting reads differently from a row at rest, one
    /// assertion per term of `at_rest`.
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

        // A JPEG at any quality is still at rest.
        let mut jpeg = spec(ExportFormat::Jpeg, ExportScale::Times(1.0));
        jpeg.quality = 30;
        assert_eq!(settings_button_state(&jpeg, false), ui::FieldButton::Off);

        let mut backed = at_rest.clone();
        backed.background = ExportBackground::Frame;
        assert_eq!(settings_button_state(&backed, false), ui::FieldButton::Set);

        let mut trimmed = at_rest.clone();
        trimmed.trim = true;
        assert_eq!(settings_button_state(&trimmed, false), ui::FieldButton::Set);

        // flip: delete `&& !spec.pad_square` in `settings_button_state` → fails at
        // this assertion.
        let mut padded = at_rest;
        padded.pad_square = true;
        assert_eq!(settings_button_state(&padded, false), ui::FieldButton::Set);

        // Outlined text marks a PDF, and not a PNG it was carried over to.
        let mut outlined = spec(ExportFormat::Pdf, ExportScale::Times(1.0));
        assert_eq!(
            settings_button_state(&outlined, false),
            ui::FieldButton::Off
        );
        outlined.text = ExportText::Outlines;
        assert_eq!(
            settings_button_state(&outlined, false),
            ui::FieldButton::Set
        );
        outlined.format = ExportFormat::Png;
        assert_eq!(
            settings_button_state(&outlined, false),
            ui::FieldButton::Off
        );
    }

    /// The background chip goes muted over a multi-selection of differing frames
    /// rather than painting one member's, but keeps showing a `Solid` colour (§15
    /// D657).
    ///
    /// flip: delete the `(Frame, None) => None` arm of `background_swatch` → fails at
    /// the multi-selection assertion; gate the whole resolution on `sizing` → fails
    /// at the `Solid` assertion.
    #[test]
    fn a_frame_background_chip_says_nothing_over_a_multi_selection() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        let root = app.session.doc.first_page();
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
                fills: ondin_core::keyed_by_position([ondin_core::Fill {
                    brush: ondin_core::Brush::Solid(ground).into(),
                    visible: true,
                }]),
            });
            ops.push(ondin_core::Operation::CreateNode {
                id: rect,
                parent: frame,
                index: 0,
                kind: ondin_core::NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(40.0, 40.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
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

        // The two subjects really do resolve to different colours.
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

        // The other backgrounds are per-spec: the chip keeps showing them.
        let mut solid = spec(ExportFormat::Png, ExportScale::Times(1.0));
        solid.background = ExportBackground::Solid(white);
        assert_eq!(
            app.background_swatch(&solid, None),
            egui::Color32::WHITE,
            "White is White on thirty layers as much as on one"
        );
    }

    /// A planned path splits into a folder and a name before it reaches the save
    /// dialog, and the folder is used only when it already exists (§15 D656).
    ///
    /// flip: return `(start, planned.to_string())` from `save_dialog_target` → fails
    /// at the existing-subfolder assertion; the bare-name case stays green.
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

        // A folder not there yet (the ordinary first run): only the directory falls back.
        assert_eq!(
            save_dialog_target(&start, "badges/new/close.png"),
            (start.clone(), "close.png".to_string()),
            "a folder that is not there yet leaves the dialog where it was"
        );
        let _ = std::fs::remove_dir_all(&start);
    }

    /// Trim is unavailable for a format with no pixels and keeps its value meanwhile.
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

    /// An export row and its size line sit `ROW_LINE_GAP` apart, and one export sits
    /// `ROW_UNIT_GAP` from the next.
    ///
    /// Measured through egui, since `Ui::horizontal` takes the theme's 24pt
    /// `interact_size.y` as a minimum height and pads a small line with ink centred.
    /// The fixture replicates the layout code's shape (a card at `CARD_ROW_GAP`
    /// holding two zero-spacing units); it cannot see `export_row` itself, which the
    /// next test measures.
    #[test]
    fn an_export_row_and_its_size_line_sit_where_they_are_told() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let _ = ctx.run_ui(Default::default(), |_| {});

        let mut rows: Vec<egui::Rect> = Vec::new();
        let mut lines: Vec<egui::Rect> = Vec::new();
        let _ = ctx.run_ui(Default::default(), |ui| {
            // The card's pitch, then the rows' own region at zero.
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

        // First, because the gaps below are measured between rects: a rect taller
        // than its ink would hide the difference.
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

    /// The gap `export_row` allocates after itself, measured on `export_row`
    /// (§15 D654): the same row laid out with `last` false and true differs by
    /// exactly the trailing block, so the row's height and minimums cancel.
    ///
    /// flip: swap the arms of `export_row`'s trailing `match lined` → fails at the
    /// captioned assertion (9 against 6); the replica test above stays green.
    #[test]
    fn export_rows_trailing_gap_is_the_one_the_unit_asks_for() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        let parent = app.session.doc.first_page();
        let rect = app.session.ids.mint();
        assert!(app.session.commit(ondin_core::Transaction(vec![
            ondin_core::Operation::CreateNode {
                id: rect,
                parent,
                index: 0,
                kind: ondin_core::NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(100.0, 60.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: None,
            },
        ])));
        let s = spec(ExportFormat::Png, ExportScale::Times(1.0));

        // One `export_row` per frame (two would clash ids), in a zero-pitch scope so
        // its rect is exactly what the row allocated.
        let height = |app: &mut crate::app::OndinApp, sizing: Option<NodeId>, last: bool| -> f32 {
            let mut h = 0.0;
            let _ = ctx.run_ui(Default::default(), |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                h = ui
                    .scope(|ui| {
                        app.export_row(ui, 0, &s, sizing, last, None);
                    })
                    .response
                    .rect
                    .height();
            });
            h
        };

        // Both states: with a subject the row draws a size line, without one not.
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
        // A row with no size line spends that line's gap too.
        assert!(
            (bare - (ROW_UNIT_GAP + ROW_LINE_GAP)).abs() < 0.5,
            "an un-captioned row allocates {bare:.1}, not {:.1}",
            ROW_UNIT_GAP + ROW_LINE_GAP
        );
    }

    /// The quality field's state machine: a drag holds, a release commits once, a
    /// cancel commits nothing.
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

        // The right-click, on the frame it lands and on the release after it.
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
    /// Both ends: the first row is 1×, and the cap holds where an uncapped `1 + n`
    /// would walk off to 5× and 6×.
    #[test]
    fn the_add_button_climbs_one_two_three_and_then_stays_at_four() {
        let ladder: Vec<ExportScale> = (0..6).map(ladder_scale).collect();
        assert_eq!(
            ladder,
            [1.0, 2.0, 3.0, 4.0, 4.0, 4.0].map(ExportScale::Times),
            "an empty list must get 1×, and the fourth row onwards 4×"
        );
        // Every rung is in the dropdown, so a `+` row is not mistaken for a typed one.
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

    /// The preview declines a padded export on the size the file would be, and an
    /// over-wide one on the device's texture side, whatever its area.
    ///
    /// flip: drop the pad squaring in `build_export_preview` → fails at "the same
    /// strip padded is …× the pixels"; drop the per-side test → panics inside egui's
    /// `load_texture`. Neither flip reaches the other's assertion.
    #[test]
    fn a_padded_export_is_declined_by_the_preview_on_the_size_it_would_be() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        let parent = app.session.doc.first_page();
        // Sized off the device's own limit so the pad guard alone decides: a
        // `side × 10` strip is inside the texture limit, and its square is over the
        // area cap. The banner below trips both guards.
        let max_side = f64::from(ctx.input(|i| i.max_texture_side) as u32);
        let (banner, strip) = (app.session.ids.mint(), app.session.ids.mint());
        let rect = |id, w: f64, h: f64, index| ondin_core::Operation::CreateNode {
            id,
            parent,
            index,
            kind: ondin_core::NodeKind::Rect {
                size: ondin_core::kurbo::Size::new(w, h),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        };
        // A 4096×15 banner at 8× is a 32768×120 raster.
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
    //! What *Export all* collects and how it writes (§15 D516).

    use super::*;

    /// A layer with export settings inside another with export settings is exported
    /// too: its spec is a deliberate instruction, and the group is a different file.
    /// The planned names are asserted too, since a name collision is its own failure.
    ///
    /// flip: restore `in_document_order` in `exportable_layers` → fails at "both
    /// layers are collected" (one id against two).
    #[test]
    fn a_nested_layers_export_settings_are_not_dropped_by_export_all() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        let parent = app.session.doc.first_page();
        let (group, inner) = (app.session.ids.mint(), app.session.ids.mint());
        let png = ondin_core::keyed_by_position([ExportSpec::new(
            ExportFormat::Png,
            ExportScale::Times(1.0),
        )]);
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
                    corner_smoothing: 0.0,
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

    /// Document order among siblings comes from the sort, not from the walk (§15
    /// D649); three siblings is the smallest fixture where the orders differ.
    ///
    /// flip: return `out` from `exportable_layers` instead of the sorted list →
    /// fails at the one assertion (`[c, b, a]` against `[a, b, c]`).
    #[test]
    fn export_all_collects_siblings_in_document_order() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        let parent = app.session.doc.first_page();
        let png = ondin_core::keyed_by_position([ExportSpec::new(
            ExportFormat::Png,
            ExportScale::Times(1.0),
        )]);
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
                    corner_smoothing: 0.0,
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

    /// *Export all frames…* plans each visible top-level frame — its own rows, else
    /// a 1× PNG — and neither a hidden frame nor one nested in another.
    ///
    /// flip: `frames_plan` dropping the `visible` filter → fails at the paths.
    #[test]
    fn export_frames_plans_the_visible_top_level_frames() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        let root = app.session.doc.first_page();
        let ids: Vec<_> = (0..4).map(|_| app.session.ids.mint()).collect();
        let frame = |id, parent, index, name: &str| ondin_core::Operation::CreateNode {
            id,
            parent,
            index,
            kind: ondin_core::NodeKind::Artboard {
                size: ondin_core::kurbo::Size::new(40.0, 30.0),
            },
            transform: None,
            name: Some(name.into()),
        };
        let ops = vec![
            frame(ids[0], root, 0, "Home"),
            frame(ids[1], root, 1, "Hidden"),
            frame(ids[2], root, 2, "Icons"),
            frame(ids[3], ids[0], 0, "Nested"),
            ondin_core::Operation::SetVisible {
                id: ids[1],
                visible: false,
            },
            ondin_core::Operation::SetExports {
                id: ids[2],
                exports: ondin_core::keyed_by_position([ExportSpec::new(
                    ExportFormat::Svg,
                    ExportScale::Times(1.0),
                )]),
            },
        ];
        assert!(app.session.commit(ondin_core::Transaction(ops)));

        let paths: Vec<String> = app.frames_plan().into_iter().map(|f| f.path).collect();
        // `Frame 1` is the new document's own frame, now last.
        assert_eq!(paths, ["Home.png", "Icons.svg", "Frame 1.png"]);
    }

    /// *Export frames as one PDF…* takes the frames *Export all frames…* takes, a
    /// page each named for its bookmark, and the PDF has that many pages.
    #[test]
    fn the_frames_pdf_is_a_page_per_visible_top_level_frame() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        let root = app.session.doc.first_page();
        let ids: Vec<_> = (0..2).map(|_| app.session.ids.mint()).collect();
        let ops = vec![
            ondin_core::Operation::CreateNode {
                id: ids[0],
                parent: root,
                index: 0,
                kind: ondin_core::NodeKind::Artboard {
                    size: ondin_core::kurbo::Size::new(40.0, 30.0),
                },
                transform: None,
                name: Some("Home".into()),
            },
            ondin_core::Operation::CreateNode {
                id: ids[1],
                parent: root,
                index: 1,
                kind: ondin_core::NodeKind::Artboard {
                    size: ondin_core::kurbo::Size::new(40.0, 30.0),
                },
                transform: None,
                name: Some("Hidden".into()),
            },
            ondin_core::Operation::SetVisible {
                id: ids[1],
                visible: false,
            },
        ];
        assert!(app.session.commit(ondin_core::Transaction(ops)));
        let pages = app.frames_pdf_pages();
        let names: Vec<&str> = pages.iter().map(|(n, _)| n.as_str()).collect();
        assert_eq!(names, ["Home", "Frame 1"]);
        let pdf = ondin_export::pdf::pdf_pages(
            &app.session.doc,
            &app.session.resolved,
            &pages,
            ondin_core::ExportText::Live,
        )
        .expect("a pdf");
        let text = String::from_utf8_lossy(&pdf.pdf);
        let count = text.matches("/Type/Page").count() - text.matches("/Type/Pages").count();
        assert_eq!(count, 2);
    }

    /// Export on save never opens a modal (§15 D626); the remembered-folder arm is
    /// asserted for both `how` values, as it holds whoever asked.
    ///
    /// flip: map `(None, Quietly)` to `AskForFolder` → fails at "a save must never
    /// ask". The `is_dir` filter in `export_all` is not reached by this test.
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

    /// Which dialog an explicit export opens, asserted from both sides of
    /// `files.len() == 1`; a `zip` is an archive at every count, which a reordered
    /// match would get wrong for one file.
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
        // Unreachable (`run_export` returns on an empty plan); it falls into the pick.
        assert_eq!(export_dialog(false, 0), DialogKind::PickFolder);
    }

    /// `write_export` lands the bytes under the planned names, reports the layer
    /// with no bounds as skipped, and remembers `prefs.export_dir` (§15 D626).
    /// The empty group has settings and no extent, the only way to reach the
    /// skipped arm. `OndinApp::headless` uses ephemeral prefs, so the user's
    /// `prefs.json` is untouched.
    ///
    /// flip: count the `None` arm of `deliver` as a success (push empty bytes
    /// instead of `skipped += 1`) → fails at the files assertion (2 against 1), as
    /// it writes a zero-byte PNG. The `if written > 0` guard is not reached.
    #[test]
    fn writing_an_export_lands_the_files_and_says_what_had_no_size() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        let parent = app.session.doc.first_page();
        let (rect, empty) = (app.session.ids.mint(), app.session.ids.mint());
        let png = ondin_core::keyed_by_position([ExportSpec::new(
            ExportFormat::Png,
            ExportScale::Times(1.0),
        )]);
        assert!(app.session.commit(ondin_core::Transaction(vec![
            ondin_core::Operation::CreateNode {
                id: rect,
                parent,
                index: 0,
                kind: ondin_core::NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(40.0, 30.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Icon".into()),
            },
            // Exports set, nothing inside: `plan::render` refuses it.
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
        assert_eq!(app.session.status().text, EXPORTING, "it says it started");
        app.settle_exports();

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

    /// A folder export says how many files it replaced and where, and only the
    /// folder arm does (§15 D636). The same plan is written twice, since the count
    /// is a difference: it must read no replacement, then one. A `Destination::File`
    /// write is the control for the other arms.
    ///
    /// flip: move the `path.exists()` read in `deliver` after the write → fails
    /// on the first run at the `!contains("replaced")` assertion.
    #[test]
    fn a_folder_export_says_what_it_replaced_and_where() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        let parent = app.session.doc.first_page();
        let rect = app.session.ids.mint();
        assert!(app.session.commit(ondin_core::Transaction(vec![
            ondin_core::Operation::CreateNode {
                id: rect,
                parent,
                index: 0,
                kind: ondin_core::NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(40.0, 30.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Icon".into()),
            },
            ondin_core::Operation::SetExports {
                id: rect,
                exports: ondin_core::keyed_by_position([ExportSpec::new(
                    ExportFormat::Png,
                    ExportScale::Times(1.0),
                )]),
            },
        ])));

        let dir = std::env::temp_dir().join(format!("ondin-replace-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        let plan = |app: &crate::app::OndinApp| app.export_plan(&app.exportable_layers());

        app.write_export(plan(&app), Destination::Folder(dir.clone()));
        app.settle_exports();
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
        app.settle_exports();
        let again = app.session.status().text.clone();
        assert!(
            again.contains("(1 replaced)"),
            "the second run overwrites and says so: {again:?}"
        );
        assert!(
            again.contains(&dir.display().to_string()),
            "still naming where: {again:?}"
        );

        // A *File* export is unchanged: its dialog already said where.
        let one = dir.join("named.png");
        app.write_export(plan(&app), Destination::File(one));
        app.settle_exports();
        let file = app.session.status().text.clone();
        assert_eq!(
            file, "Exported 1 file",
            "a save-dialog destination reports as it always did"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// An export renders on a worker: `write_export` returns before a file exists
    /// is not something a test can pin, so this pins the other half — the frame
    /// loop's `poll_exports` reports it when it lands. flip: `poll_exports` keeping
    /// a landed job → fails at "the frame loop reports it".
    #[test]
    fn an_export_lands_through_the_frame_loop() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        let parent = app.session.doc.first_page();
        let rect = app.session.ids.mint();
        assert!(app.session.commit(ondin_core::Transaction(vec![
            ondin_core::Operation::CreateNode {
                id: rect,
                parent,
                index: 0,
                kind: ondin_core::NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(40.0, 30.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Icon".into()),
            },
            ondin_core::Operation::SetExports {
                id: rect,
                exports: ondin_core::keyed_by_position([ExportSpec::new(
                    ExportFormat::Svg,
                    ExportScale::Times(1.0),
                )]),
            },
        ])));
        let dir = std::env::temp_dir().join(format!("ondin-worker-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        app.write_export(
            app.export_plan(&app.exportable_layers()),
            Destination::Folder(dir.clone()),
        );
        assert_eq!(app.exports.len(), 1, "on a worker");
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        while !app.exports.is_empty() && std::time::Instant::now() < deadline {
            app.poll_exports(&ctx);
            std::thread::yield_now();
        }
        assert!(
            app.session.status().text.starts_with("Exported 1 file"),
            "the frame loop reports it: {:?}",
            app.session.status().text
        );
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            1,
            "and the file is there"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}

#[cfg(test)]
mod code_export_tests {
    //! *Export as HTML / React / Vue…*, short of its save dialog.

    use super::*;
    use ondin_export::code::Target;

    /// The selection is what is written, and nothing selected is a sentence, not
    /// an empty zip.
    #[test]
    fn code_export_writes_the_selection_and_refuses_none() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        let parent = app.session.doc.first_page();
        let frame = app.session.ids.mint();
        assert!(app.session.commit(ondin_core::Transaction(vec![
            ondin_core::Operation::CreateNode {
                id: frame,
                parent,
                index: 0,
                kind: ondin_core::NodeKind::Artboard {
                    size: ondin_core::kurbo::Size::new(40.0, 30.0),
                },
                transform: None,
                name: Some("Hero".into()),
            }
        ])));
        app.session.selection.set(vec![]);
        assert!(app.code_export(Target::React).is_err());

        app.session.selection.set(vec![frame]);
        for target in Target::ALL {
            let out = app.code_export(target).expect("a selection to write");
            let page = match target {
                Target::Html => "index.html",
                Target::React => "App.tsx",
                Target::Vue => "App.vue",
            };
            let text = String::from_utf8(out.file(page).expect(page).to_vec()).unwrap();
            assert!(text.contains("hero"), "{target:?} writes the frame: {text}");
        }
    }

    /// The status line names the target and counts each list the report holds.
    #[test]
    fn a_code_export_says_what_its_report_lists() {
        let clean = ondin_export::code::CodeExport::default();
        assert_eq!(
            code_export_summary(Target::Vue, &clean),
            "Exported Vue code"
        );
        let noted = ondin_export::code::CodeExport {
            approximated: vec!["a".into()],
            inlined: vec!["b".into(), "c".into()],
            ..Default::default()
        };
        assert_eq!(
            code_export_summary(Target::React, &noted),
            "Exported React code — 1 drawn differently, 2 written out as layers; see report.txt"
        );
    }

    /// The save dialog is seeded with a file name, never a path or a name Windows
    /// refuses, and a name with nothing usable falls back.
    ///
    /// flip: `code_archive_name` without `sanitize_segment` → `Button / Primary.zip`.
    #[test]
    fn the_code_archive_is_named_as_a_file() {
        assert_eq!(
            code_archive_name("Button / Primary").as_deref(),
            Some("Button - Primary.zip")
        );
        assert_eq!(code_archive_name("a:b?c").as_deref(), Some("a-b-c.zip"));
        assert_eq!(code_archive_name("Con").as_deref(), Some("_Con.zip"));
        assert_eq!(code_archive_name("  "), None);
    }
}
