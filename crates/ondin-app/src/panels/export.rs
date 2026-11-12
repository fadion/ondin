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
    ExportBackground, ExportFormat, ExportScale, ExportSpec, ExportText, IconSet, Keyed, NodeId,
    Operation, SvgColour, SvgStrokes, SvgView, Transaction,
};
use ondin_export::{PlanOptions, PlannedFile};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

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
/// The collapse key the Preview section shares with the panel list.
///
/// `collapsed_panels` is keyed by title, but the section now lives inside the Export
/// card, so the key is named for the two places that read it.
const PREVIEW: &str = "Preview";
/// The clear space round the previewed image inside its checkerboard.
const PREVIEW_INSET: f32 = 10.0;

// --- the settings popover ---------------------------------------------------
//
// The shared popover language: 272 wide, 11 of padding, `ui::CONTROL_H` controls,
// `ui::labelled` sections, as in `typography.rs` and `stroke_menu_popup`.

/// The card's width, shared with every other popover in the inspector.
const POP_W: f32 = crate::app::POPOVER_W;
/// The room inside the padding and the border — `POP_W` is the width the card
/// paints, not the room in it (§15 D307).
const POP_INNER: f32 = ui::menu_inner_w(POP_W, ui::POPOVER_PAD);
/// A control inside the popover: every control's height.
const POP_CELL: f32 = ui::CONTROL_H;
/// Between one labelled section and the next — the shared one.
const SECTION_GAP: f32 = ui::POPOVER_SECTION_GAP;

/// The settings card's two scrubbable numbers, which share one in-flight slot
/// (`InFlight::export_scrub`): only one field is dragged at a time.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ExportNumber {
    /// A JPEG's quality, 1–100.
    Quality,
    /// An SVG's decimal places, 0–4.
    Precision,
}

impl ExportNumber {
    fn range(self) -> std::ops::RangeInclusive<f64> {
        match self {
            ExportNumber::Quality => 1.0..=100.0,
            ExportNumber::Precision => 0.0..=f64::from(ondin_core::export::MAX_PRECISION),
        }
    }

    fn of(self, spec: &ExportSpec) -> u8 {
        match self {
            ExportNumber::Quality => spec.quality,
            ExportNumber::Precision => spec.precision,
        }
    }
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
    /// An icon file's sizes, each its own texture, smallest first; empty for any
    /// other format.
    icons: Vec<IconTile>,
    /// The line under it: format, size, and what it weighs.
    summary: String,
}

/// One size of an icon file's preview.
pub(crate) struct IconTile {
    size: u32,
    texture: egui::TextureHandle,
    /// Drawn from a frame of its own, which the label marks.
    framed: bool,
}

/// Between two icon sizes side by side in the preview, and between their lines.
const ICON_TILE_GAP: f32 = 10.0;

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
                        .size(ui::SMALL_PT)
                        .color(theme::text::faint()),
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
                    match super::component::ghost_row(
                        ui,
                        super::component::GhostLead::Plain,
                        &label,
                    ) {
                        Some(ui::ResetOrPush::Reset) => restore = true,
                        Some(ui::ResetOrPush::Push) => {
                            if let Some(id) = sizing {
                                let list = Operation::SetExports {
                                    id,
                                    exports: Vec::new(),
                                };
                                app.push_item_removal(id, &list, gone.id);
                            }
                        }
                        None => {}
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
            if !preview.icons.is_empty() {
                icon_preview(ui, &preview.icons);
            }
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
                    egui::Stroke::new(1.0, theme::color::field_border()),
                    egui::StrokeKind::Inside,
                );
                let at = egui::Rect::from_center_size(ground.center(), shown);
                p.image(tex.id(), at, crate::thumbs::FULL_UV, egui::Color32::WHITE);
            }
            ui.label(
                egui::RichText::new(&preview.summary)
                    .size(ui::CAPTION_PT)
                    .color(theme::text::faint()),
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
        // A slice previews its box of the page, as its file is (`plan::render`).
        let region = ondin_export::plan::slice_region(doc, res, key.node);
        let origins = match &region {
            Some((_, origins)) => origins.clone(),
            None => vec![key.node],
        };
        if key.spec.format.is_icon() {
            let entries = ondin_export::icon::entries(doc, res, key.node, &key.spec)?;
            let bytes = ondin_export::icon::encode(key.spec.format, &entries).unwrap_or_default();
            let summary = format!(
                "{} · {} · {}",
                key.spec.format.label(),
                ui::count(entries.len(), "size"),
                human_bytes(bytes.len())
            );
            let icons = entries
                .into_iter()
                .map(|e| {
                    let side = e.size as usize;
                    let image =
                        egui::ColorImage::from_rgba_unmultiplied([side, side], &e.raster.rgba);
                    IconTile {
                        size: e.size,
                        texture: ctx.load_texture(
                            format!("export-icon-{}", e.size),
                            image,
                            egui::TextureOptions::LINEAR,
                        ),
                        framed: e.frame.is_some(),
                    }
                })
                .collect();
            return Some(ExportPreview {
                key,
                texture: None,
                pixels: (0, 0),
                icons,
                summary,
            });
        }
        // No raster for a vector file: rasterizing it would preview something other
        // than the file.
        if !key.spec.format.is_raster() {
            // The reported form (§15 D780): the card is read before exporting, when
            // a fidelity note can still change what the user does. A PDF is not
            // converted per edit, so its line has no size.
            let (head, note) = match key.spec.format {
                ExportFormat::Pdf => {
                    let approximated =
                        ondin_export::pdf::approximated(doc, res, &origins, key.spec.text);
                    ("PDF".to_string(), fidelity_clauses(&approximated, &[]))
                }
                _ => {
                    let out = ondin_export::svg::svg_spec(doc, res, key.node, &key.spec);
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
                icons: Vec::new(),
                summary,
            });
        }

        let (size, clamped) = ondin_export::plan::raster_size(doc, res, key.node, &key.spec);
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
                icons: Vec::new(),
                summary: format!("{pw}×{ph} — too large to preview here"),
            });
        }

        // Rasterized once, then both encoded and drawn, not rendered twice per edit.
        let opts = ondin_export::plan::raster_opts(doc, res, key.node, &key.spec);
        let raster = match &region {
            Some((view, origins)) => {
                ondin_export::png::raster_region(doc, res, origins, *view, &opts)
            }
            None => ondin_export::png::raster_of(doc, res, &[key.node], &opts)?,
        };
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
            icons: Vec::new(),
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
                .size(ui::SMALL_PT)
                .color(theme::text::faint()),
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
                        .selected_text(ui::combo_text(&spec.scale.label()));
                    if spec.format.is_icon() {
                        // The row's "how big" cell holds the size set instead.
                        if let Some(sizes) = icon_set_cell(ui, index, spec) {
                            out.edited = Some(ExportSpec {
                                sizes,
                                ..spec.clone()
                            });
                        }
                    } else if live {
                        combo.show_ui(ui, |ui| {
                            ui::menu_rows(ui);
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
                                    .size(ui::SMALL_PT)
                                    .color(theme::text::faint()),
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
                            for f in ExportFormat::ALL {
                                // The icon files under a rule: the formats above
                                // make one picture, these a set of them.
                                if f == ExportFormat::Ico {
                                    super::inspector::popup_rule(ui);
                                }
                                ui.selectable_value(&mut format, f, f.label());
                            }
                        });
                    if format != spec.format {
                        let mut next = out.edited.clone().unwrap_or_else(|| spec.clone());
                        // ICO's ticks are not ICNS's sizes: the other icon format
                        // starts on its own set. Any other format keeps them inert.
                        if format.is_icon() && spec.format.is_icon() {
                            next.sizes.clear();
                        }
                        out.edited = Some(ExportSpec { format, ..next });
                    }
                });

                let more = ui::field_button(
                    ui,
                    icon::SLIDERS_HORIZONTAL,
                    CELL,
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
                if ui::field_button(ui, icon::X, CELL, ui::FieldButton::Off)
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
            let folder = spec.each_variant
                && sizing.is_some_and(|id| ondin_core::variant::is_set(&self.session.doc, id));
            let glyph = match folder {
                true => icon::FOLDER_SIMPLE,
                false => icon::FRAME,
            };
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
                        ui.label(theme::icon_text(glyph, 12.0, theme::text::faint()));
                        ui.label(
                            egui::RichText::new(size)
                                .size(ui::CAPTION_PT)
                                .color(theme::text::faint()),
                        );
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
    /// multiplier on every differently-shaped layer. A set exporting each variant
    /// says how many files go into which folder instead.
    fn export_size_label(&self, spec: &ExportSpec, sizing: Option<NodeId>) -> String {
        let Some(id) = sizing else {
            return String::new();
        };
        let opts = PlanOptions {
            folders_from_names: self.prefs.export_folders_from_names,
            ..Default::default()
        };
        if let Some(files) = ondin_export::plan::variant_files(&self.session.doc, id, spec, &opts) {
            let folder = files.first().and_then(|(_, p)| p.rsplit_once('/'));
            return match folder {
                Some((folder, _)) => format!("{} in {folder}/", ui::count(files.len(), "file")),
                None => "No variants to export".to_string(),
            };
        }
        if spec.format.is_icon() {
            return self.icon_size_line(spec, id);
        }
        if !spec.format.is_raster() {
            return String::new();
        }
        match ondin_export::plan::raster_size(&self.session.doc, &self.session.resolved, id, spec) {
            (Some((w, h)), true) => format!("{w}×{h} max"),
            (Some((w, h)), false) => format!("{w}×{h}"),
            (None, _) => String::new(),
        }
    }

    /// What an icon row's file will hold — `16, 32, 48 px` — and what changes it:
    /// a layer that is not square is padded, sizes drawn from their own frames are
    /// counted, and a fit is named. Empty for a layer with no box.
    fn icon_size_line(&self, spec: &ExportSpec, id: NodeId) -> String {
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        let Some(b) = res.ink_bounds(id) else {
            return String::new();
        };
        let sizes = spec.icon_sizes();
        let mut line = sizes_text(&sizes, true);
        let slice = ondin_export::plan::slice_region(doc, res, id).map(|(v, _)| v);
        let b = slice.unwrap_or(b);
        // A hundredth of a unit is no padding anyone would see.
        if (b.width() - b.height()).abs() > 0.01 {
            line.push_str(" · padded to square");
        }
        if spec.size_frames {
            let n = ondin_export::icon::size_frames(doc, id, &sizes).len();
            match n {
                0 => {}
                1 => line.push_str(" · 1 from its own frame"),
                n => line.push_str(&format!(" · {n} from their own frames")),
            }
        }
        if spec.pixel_fit {
            line.push_str(" · pixel-fitted");
        }
        line
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
            .constrain_to(ctx.input(|i| i.content_rect()).shrink(ui::SCREEN_MARGIN))
            .show(&ctx, |ui| {
                ui::hold_tab(ui);
                ui::menu_frame(ui::POPOVER_PAD).show(ui, |ui| {
                    ui.set_width(POP_INNER);
                    ui.spacing_mut().item_spacing.y = SECTION_GAP;

                    // --- header ------------------------------------------------
                    if ui::popover_head(ui, "Export settings") {
                        close = true;
                    }

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

                    // --- sizes, icon files only ----------------------------------
                    if spec.format.is_icon() {
                        ui::labelled(ui, "Sizes", |ui| {
                            if let Some(sizes) = icon_size_chips(ui, spec) {
                                edit(&mut out, spec).sizes = sizes;
                            }
                        });
                    }

                    // --- quality, JPEG only (labelled for its format) ----------
                    if spec.format == ExportFormat::Jpeg {
                        ui::labelled(ui, "JPEG quality", |ui| {
                            let quality = ExportNumber::Quality;
                            if let Some(q) =
                                self.export_number_field(ui, index, quality, spec, POP_INNER)
                            {
                                edit(&mut out, spec).quality = q;
                            }
                        });
                    }

                    // --- colour, strokes, viewBox and precision, SVG only ------
                    if spec.format == ExportFormat::Svg {
                        self.export_svg_settings(ui, index, spec, sizing, &mut out);
                    }

                    // --- text, vector only ---------------------------------------
                    if !has_pixels(spec.format) {
                        ui::labelled(ui, "Text", |ui| {
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
                    ui::labelled(ui, "Background", |ui| {
                        ui.scope(|ui| {
                            ui.spacing_mut().interact_size.y = POP_CELL;
                            ui.spacing_mut().button_padding.y = 0.0;
                            let mut bg = spec.background;
                            let (glyph, swatch) = self.background_swatch(spec, sizing);
                            egui::ComboBox::from_id_salt(("export-bg", index))
                                .icon(ui::combo_chevron)
                                .width(POP_INNER)
                                .selected_text(ui::glyph_and_text_tinted(
                                    glyph,
                                    self.background_label(spec),
                                    swatch,
                                ))
                                .show_ui(ui, |ui| {
                                    ui::menu_rows(ui);
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
                                    // Plural where the answer is plural: over a
                                    // multi-selection each layer has its own frame.
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
                    // A slice's box is the point of it, so it offers no trim
                    // (`plan::raster_opts` ignores one).
                    let slices = self.session.selection.ids().iter().all(|id| {
                        self.session
                            .doc
                            .get(*id)
                            .is_some_and(|n| matches!(n.kind(), ondin_core::NodeKind::Slice { .. }))
                    });
                    {
                        let size = egui::vec2(POP_INNER, POP_CELL);
                        if !slices {
                            let trim = ui::toggle_row(
                                ui,
                                icon::CROP,
                                "Trim transparent edges",
                                trim_button_state(spec),
                                size,
                            )
                            .on_hover_text(
                                match has_pixels(spec.format) {
                                    true => "Drop the fully transparent rows and columns",
                                    false => "Trimming reads pixels, and a vector file has none",
                                },
                            );
                            if trim.clicked() && has_pixels(spec.format) {
                                edit(&mut out, spec).trim = !spec.trim;
                            }
                        }
                        if spec.format.is_icon() {
                            // Held on, the stored value kept for the other formats.
                            ui::toggle_row_held(ui, icon::PAD_SQUARE, "Pad out to a square", size)
                                .on_hover_text("Icon files are always square");
                        } else {
                            let pad = ui::toggle_row(
                                ui,
                                icon::PAD_SQUARE,
                                "Pad out to a square",
                                pad_button_state(spec),
                                size,
                            )
                            .on_hover_text("Centre the result in a square of its longer side");
                            if pad.clicked() {
                                edit(&mut out, spec).pad_square = !spec.pad_square;
                            }
                        }
                        if spec.format.is_icon() {
                            self.icon_switches(ui, spec, sizing, &mut out);
                        }
                        // Only where it means something: every selected layer is a
                        // component set.
                        let sets = self
                            .export_subjects()
                            .iter()
                            .all(|id| ondin_core::variant::is_set(&self.session.doc, *id));
                        if sets {
                            let each = ui::toggle_row(
                                ui,
                                icon::FOLDER_SIMPLE,
                                "Each variant as its own file",
                                ui::FieldButton::on_if(spec.each_variant),
                                size,
                            )
                            .on_hover_text(
                                "A file per variant, named by its values, in a folder named for the set",
                            );
                            if each.clicked() {
                                edit(&mut out, spec).each_variant = !spec.each_variant;
                            }
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

    /// An icon row's two switches, each with its caption under it: sizes drawn
    /// from frames named for them, saying what it finds on the page, and a fit
    /// per size.
    fn icon_switches(
        &mut self,
        ui: &mut egui::Ui,
        spec: &ExportSpec,
        sizing: Option<NodeId>,
        out: &mut Option<ExportSpec>,
    ) {
        let size = egui::vec2(POP_INNER, POP_CELL);
        let caption = |ui: &mut egui::Ui, text: &str| {
            ui.label(
                egui::RichText::new(text)
                    .size(ui::CAPTION_PT)
                    .color(theme::text::faint()),
            );
        };
        // The caption annotates its switch, so it sits as close as a size line.
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = ROW_LINE_GAP;
            let frames = ui::toggle_row(
                ui,
                icon::FRAME,
                "Use frames named for their size",
                ui::FieldButton::on_if(spec.size_frames),
                size,
            )
            .on_hover_text("Draw a size from its own frame on this page instead of scaling");
            if frames.clicked() {
                edit(out, spec).size_frames = !spec.size_frames;
            }
            let doc = &self.session.doc;
            let text = match sizing.and_then(|id| Some((id, doc.get(id)?.name()))) {
                Some((id, name)) => {
                    let sizes = spec.icon_sizes();
                    let found = ondin_export::icon::size_frames(doc, id, &sizes);
                    size_frames_caption(name, &sizes, &found)
                }
                None => "Each layer's own frames, named for the layer and the size".to_string(),
            };
            caption(ui, &text);
        });
        ui.scope(|ui| {
            ui.spacing_mut().item_spacing.y = ROW_LINE_GAP;
            let fit = ui::toggle_row(
                ui,
                icon::PIXEL_FIT,
                "Pixel-fit each size",
                ui::FieldButton::on_if(spec.pixel_fit),
                size,
            )
            .on_hover_text("Text and turned layers are drawn as they are");
            if fit.clicked() {
                edit(out, spec).pixel_fit = !spec.pixel_fit;
            }
            caption(
                ui,
                "Fits a copy of the art to each size's pixel grid before drawing it. \
                 The document is not changed.",
            );
        });
    }

    /// The settings card's four SVG rows: *Colour*, *Strokes* and *viewBox* as
    /// segmented pairs, then *Precision*. Under `currentColor` a single layer whose
    /// solids are more than one colour says how many will merge, so the choice is
    /// never silent. *From the frame* is unavailable on a layer that is no frame.
    fn export_svg_settings(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        spec: &ExportSpec,
        sizing: Option<NodeId>,
        out: &mut Option<ExportSpec>,
    ) {
        let pair = |ui: &mut egui::Ui,
                    labels: [&'static str; 2],
                    lit: usize,
                    enabled: [bool; 2],
                    tips: [&'static str; 2]| {
            ui::segmented_tipped(
                ui,
                POP_INNER,
                ui::SEGMENT_CELL_H,
                2,
                lit,
                |i| enabled[i],
                |i| tips[i],
                |p, i, r, on| ui::segment_label(p, r, labels[i], on),
            )
        };

        ui::labelled(ui, "Colour", |ui| {
            let lit = usize::from(spec.colour == SvgColour::CurrentColor);
            let tips = [
                "Each paint keeps its own colour",
                "Solid fills and strokes take the page's colour; opacity is kept",
            ];
            if let Some(i) = pair(ui, ["As painted", "currentColor"], lit, [true; 2], tips) {
                edit(out, spec).colour = [SvgColour::AsPainted, SvgColour::CurrentColor][i];
            }
            let merged = match (spec.colour, sizing) {
                (SvgColour::CurrentColor, Some(node)) => self.export_merged_colours(node, spec),
                _ => 0,
            };
            if merged > 1 {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let warn = theme::color::warn();
                    ui.label(theme::icon_text(icon::WARNING, 13.0, warn));
                    ui.label(
                        egui::RichText::new(format!(
                            "{merged} colours will all become currentColor"
                        ))
                        .size(ui::CAPTION_PT)
                        .color(warn),
                    );
                });
            }
        });

        ui::labelled(ui, "Strokes", |ui| {
            let lit = usize::from(spec.strokes == SvgStrokes::Outline);
            let tips = [
                "Strokes stay strokes, which CSS can restyle",
                "Each stroke becomes the filled shape it draws; the layer is left alone",
            ];
            if let Some(i) = pair(ui, ["Keep", "Outline"], lit, [true; 2], tips) {
                edit(out, spec).strokes = [SvgStrokes::Keep, SvgStrokes::Outline][i];
            }
        });

        ui::labelled(ui, "viewBox", |ui| {
            // Over a multi-selection a frame among the layers is enough: each layer
            // that is no frame takes its artwork on its own.
            let framed = self.export_subjects().iter().any(|id| {
                matches!(
                    self.session.doc.get(*id).map(|n| n.kind()),
                    Some(ondin_core::NodeKind::Artboard { .. })
                )
            });
            let lit = match (spec.view, framed) {
                (SvgView::Frame, true) => 0,
                _ => 1,
            };
            let tips = [
                match framed {
                    true => "The frame's own box, whatever its artwork reaches",
                    false => "Only a frame has its own viewBox",
                },
                "Trimmed to the artwork's ink",
            ];
            let labels = ["From the frame", "Fit the artwork"];
            if let Some(i) = pair(ui, labels, lit, [framed, true], tips) {
                edit(out, spec).view = [SvgView::Frame, SvgView::Artwork][i];
            }
        });

        ui::labelled(ui, "Precision", |ui| {
            let precision = ExportNumber::Precision;
            if let Some(p) = self.export_number_field(ui, index, precision, spec, POP_INNER) {
                edit(out, spec).precision = p;
            }
        });
    }

    /// How many solid colours `currentColor` would merge on `node`, counted by
    /// writing the file once per edit of the layer or the spec.
    fn export_merged_colours(&mut self, node: NodeId, spec: &ExportSpec) -> usize {
        let key = PreviewKey {
            node,
            spec: spec.clone(),
            revision: self.session.revision(),
        };
        if let Some((k, n)) = &self.export_merged
            && *k == key
        {
            return *n;
        }
        let (doc, res) = (&self.session.doc, &self.session.resolved);
        let n = ondin_export::svg::svg_spec(doc, res, node, spec).merged;
        self.export_merged = Some((key, n));
        n
    }

    /// The JPEG quality or SVG precision field. `Some` on the frame the edit is
    /// finished — never while the pointer still holds it.
    ///
    /// A hand-wound scrub, because neither shared committer works: `edit_valve`
    /// previews through `RenderOverrides`, which absorbs `SetExports` as a no-op (the
    /// number would not move), and `commit_run` commits every frame while
    /// `cancel_gesture` only drops the preview (a cancel would leave the value).
    ///
    /// The in-flight value lives in `export_scrub`: the field reads it during the
    /// drag, the release commits it once, a right-click's cancel drops it (as
    /// `session_scrub`). The cancel is checked before the buffer is read: on that
    /// frame egui may still report a `changed()`, which would commit the last movement.
    fn export_number_field(
        &mut self,
        ui: &mut egui::Ui,
        index: usize,
        number: ExportNumber,
        spec: &ExportSpec,
        width: f32,
    ) -> Option<u8> {
        let cancelled = self.gesture_cancelled;
        if cancelled {
            self.in_flight.export_scrub = None;
        }
        let held = match self.in_flight.export_scrub {
            Some((n, i, v)) if n == number && i == index && !cancelled => Some(v),
            _ => None,
        };
        let mut v = held.unwrap_or(number.of(spec) as f64);
        let size = egui::vec2(width, POP_CELL);
        let resp = match number {
            ExportNumber::Quality => value_field(
                ui,
                size,
                Prefix::Icon(icon::GAUGE),
                &mut v,
                // 0.25, slower than the panel's other 0–100 fields: nothing visibly
                // moves while dragging, and the useful band is about 60–95.
                Scrub::whole(0.25).range(number.range()),
                |d| d.custom_formatter(ui::number(0)),
            )
            .on_hover_text("Lower is a smaller file. 90 unless you have a reason"),
            ExportNumber::Precision => {
                let unit = ui::Suffix {
                    text: "decimals",
                    clickable: false,
                    tooltip: "",
                };
                ui::value_field_suffixed(
                    ui,
                    size,
                    Prefix::Icon(icon::HASH),
                    Some(unit),
                    &mut v,
                    // Five values across the field: a slow hand per step.
                    Scrub::whole(0.05).range(number.range()),
                    |d| d.custom_formatter(ui::number(0)),
                )
                .0
                .on_hover_text("Fewer places is a smaller file. 2 is past what an icon shows")
            }
        };
        match scrub_step(
            ScrubInput {
                cancelled,
                dragged: resp.dragged(),
                changed: resp.changed(),
                was_held: held.is_some(),
            },
            number,
            v,
            number.of(spec),
        ) {
            ScrubStep::Hold => {
                self.in_flight.export_scrub = Some((number, index, v));
                None
            }
            ScrubStep::Commit(v) => {
                self.in_flight.export_scrub = None;
                Some(v)
            }
            ScrubStep::Nothing => None,
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
        ui::labelled(ui, part.label(), |ui| {
            // `ui::text_field` has no leading glyph, so its numbers are kept here.
            #[expect(
                clippy::disallowed_methods,
                reason = "the entry inside a composed field row, which paints the ground and ui::typing_edge itself"
            )]
            let resp = ui::field_row(ui, egui::vec2(POP_INNER, POP_CELL), |ui| {
                ui.spacing_mut().item_spacing.x = 7.0;
                ui.label(theme::icon_text(
                    icon::TEXT_T,
                    ui::CONTROL_GLYPH_PT,
                    theme::text::faint(),
                ));
                let font = egui::FontId::proportional(ui::CONTROL_PT);
                let pad = ui::edit_frame(ui, &font, POP_CELL - ui::FIELD_BORDER_H, 0.0);
                pad.add(
                    ui,
                    f32::INFINITY,
                    egui::TextEdit::singleline(&mut text)
                        .id(egui::Id::new((part.id_salt(), index)))
                        .hint_text(egui::RichText::new(hint).size(ui::CONTROL_PT))
                        .font(font),
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

    /// The glyph in front of that label and its ink: *Frame's own* is the frame
    /// glyph in the row's dim ink, since it differs per layer; any other is a chip
    /// in the colour that will actually be behind the export, or the checkerboard
    /// when nothing will.
    ///
    /// Resolved through `plan::background_color`, not read off the setting. Glyphs,
    /// because a `ComboBox`'s selected text can carry a glyph but not a painter.
    fn background_swatch(
        &self,
        spec: &ExportSpec,
        sizing: Option<NodeId>,
    ) -> (&'static str, egui::Color32) {
        if spec.background == ExportBackground::Frame {
            return (icon::FRAME, theme::text::dim());
        }
        // `Solid` and `Transparent` hold for any number of layers.
        let resolved = sizing
            .or_else(|| self.export_subjects().first().copied())
            .and_then(|id| ondin_export::plan::background_color(&self.session.doc, id, spec));
        match resolved {
            Some(c) => {
                let [r, g, b, a] = c.to_rgba8().to_u8_array();
                let ink = egui::Color32::from_rgba_unmultiplied(r, g, b, a);
                (icon::SQUARE_FILL, ink)
            }
            None => (icon::CHECKERBOARD, theme::text::dim()),
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
            (1, Some(n)) => format!(
                "Export {}",
                crate::private::layer(&self.session.doc, n.id())
            ),
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
            .on_hover_text(format!("{} in one archive", ui::count(files.len(), "file")))
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
        let ctx = ui.ctx().clone();
        let current = self.shared_exports(subjects);
        // Collected and applied after the `Area` closes: every one of these mutates
        // `self`, which is borrowed for the duration of the popup.
        let mut act: Option<MenuAct> = None;

        let anchor = Self::popover_anchor(&ctx, head.rect, MENU_W);
        let menu = egui::Area::new(egui::Id::new("export-menu"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            .constrain_to(ctx.input(|i| i.content_rect()).shrink(ui::SCREEN_MARGIN))
            .show(&ctx, |ui| {
                ui::hold_tab(ui);
                ui::menu_frame(ui::POPOVER_PAD).show(ui, |ui| {
                    ui.set_width(ui::menu_inner_w(MENU_W, ui::POPOVER_PAD));
                    ui::popup_rows(ui);

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
                                        icon::FLOPPY_DISK,
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
                        ui::MenuRow::new(icon::EXPORT_SYNC, "Re-export on save").checked(on_save),
                        ui::MENU_ROW_H,
                    )
                    .on_hover_text(crate::input::keys(
                        "Every Ctrl+S rewrites the whole export set",
                    ))
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
        let turn = self.export_turns.ticket();
        let asker = self.session.path.clone();
        let label = match files.as_slice() {
            [one] => file_name_of(&one.path),
            many => format!("{} files", many.len()),
        };
        self.spawn_export(label, move |cancel| {
            // SVG and PDF shape live text again, with the faces the canvas used.
            ondin_core::text::register_font_data(&resolved.text_fonts());
            Delivered {
                asker,
                ..deliver(&doc, &resolved, &files, destination, &turn, cancel)
            }
        });
    }

    /// Run `work` on an export worker, the status bar saying *Exporting ‹label›…*
    /// with *Cancel* until it lands. `work` reads the flag
    /// [`Self::cancel_exports`] sets, before each render and before it writes.
    fn spawn_export(
        &mut self,
        label: String,
        work: impl FnOnce(&ExportFlags) -> Delivered + Send + 'static,
    ) {
        let ctx = self.egui_ctx.clone();
        let flags = Arc::new(ExportFlags::default());
        let theirs = flags.clone();
        let job = crate::job::Job::spawn("ondin-export", &ctx, move || work(&theirs));
        self.exports.push(RunningExport { job, flags, label });
    }

    /// What the status bar says while exports run, `None` when none does: the
    /// file of one, the count of several. A cancelled one no longer counts.
    pub(crate) fn exporting(&self) -> Option<String> {
        let live: Vec<&RunningExport> = self.exports.iter().filter(|e| !e.cancelled()).collect();
        match live.as_slice() {
            [] => None,
            // The label is a file name, made from a layer's.
            [_] if crate::private::on() => Some("Exporting 1 export…".into()),
            [one] => Some(format!("Exporting {}…", one.label)),
            many => Some(format!("Exporting {} exports…", many.len())),
        }
    }

    /// Cancel every running export: none writes a file it has not started
    /// writing. A render already under way finishes on its worker, unwritten, and
    /// stays in [`Self::exports`] until it does; quitting waits only for a write
    /// in progress.
    pub(crate) fn cancel_exports(&mut self) {
        let mut n = 0;
        for e in self.exports.iter().filter(|e| !e.cancelled()) {
            e.flags.cancel.store(true, Ordering::SeqCst);
            n += 1;
        }
        match n {
            0 => {}
            1 => self.session.info("Export cancelled"),
            n => self.session.info(format!("{n} exports cancelled")),
        }
    }

    /// Wait for every export that will still write: quitting mid-write would
    /// leave a file torn. A cancelled one that has not begun writing never will,
    /// so its render is left to die with the process. Their reports have nowhere
    /// left to go.
    pub(crate) fn drain_exports(&mut self) {
        for e in std::mem::take(&mut self.exports) {
            if !e.cancelled() || e.flags.writing.load(Ordering::SeqCst) {
                let _ = e.job.wait();
            }
        }
    }

    /// Report the exports that have landed. Every frame.
    pub(crate) fn poll_exports(&mut self) {
        let mut landed = Vec::new();
        self.exports.retain(|e| match e.job.poll() {
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
    }

    /// Wait for every running export and report it (tests only).
    #[cfg(test)]
    pub(crate) fn settle_exports(&mut self) {
        for e in std::mem::take(&mut self.exports) {
            match e.job.wait() {
                Some(d) => self.land_export(d),
                None => self.session.fail("Could not export: the writer stopped"),
            }
        }
    }

    /// Report a finished export, and remember where it went; a cancelled one
    /// says nothing, its cancel having said it.
    fn land_export(&mut self, d: Delivered) {
        if d.cancelled {
            return;
        }
        let Delivered {
            written,
            skipped,
            replaced,
            to_folder,
            failed,
            where_,
            head,
            asker,
            name,
            cancelled: _,
        } = d;
        // A report landing on another tab says which document it was for.
        let whose = match &asker {
            Some(path) if self.session.path.as_ref() != Some(path) => path
                .file_stem()
                .map(|s| format!("{}: ", crate::private::document(&s.to_string_lossy(), 0)))
                .unwrap_or_default(),
            _ => String::new(),
        };
        if written == 0 && failed.is_none() {
            self.session.fail(format!(
                "{whose}Nothing to export — none of that has any size"
            ));
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
        // One file by its name, so a report is never mistaken for another export's.
        let head = match (head, to_folder, replaced) {
            (Some(head), ..) => head,
            // A file is named after its layer, which private mode masks.
            (None, false, _) | (None, true, 0)
                if written == 1 && name.is_some() && !crate::private::on() =>
            {
                format!("Exported “{}”", name.unwrap_or_default())
            }
            (None, false, _) | (None, true, 0) => export_count(written),
            (None, true, n) => format!("{} ({n} replaced)", export_count(written)),
        };
        let head = format!("{whose}{head}");
        let head = match skipped {
            0 => head,
            n => format!("{head} — {n} had no size"),
        };
        match failed {
            Some(e) => self.session.fail(format!("{whose}Could not write: {e}")),
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
            if ondin_core::build::exported_in_bulk(&self.session.doc, id) {
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
                doc.shown(*id)
                    && doc
                        .get(*id)
                        .is_some_and(|n| matches!(n.kind(), ondin_core::NodeKind::Artboard { .. }))
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
            // A legal folder name (no separator, no `..`), else the page's number;
            // `Home` and `home` are one folder on Windows and macOS.
            let mut base = ondin_export::plan::sanitize_segment(&self.page_name(page));
            if base.is_empty() {
                base = format!("Page {}", doc.page_index(page).map_or(0, |i| i + 1));
            }
            let folder = std::iter::once(base.clone())
                .chain((2..).map(|n| format!("{base} {n}")))
                .find(|f| !folders.iter().any(|t| t.to_lowercase() == f.to_lowercase()))
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
        // On the worker, as every image export: a busy page takes seconds.
        let (doc, resolved) = (self.session.doc.clone(), self.session.resolved.clone());
        let turn = self.export_turns.ticket();
        let asker = self.session.path.clone();
        let label = path
            .file_name()
            .map_or_else(|| "PDF".into(), |n| n.to_string_lossy().into_owned());
        self.spawn_export(label, move |cancel| {
            ondin_core::text::register_font_data(&resolved.text_fonts());
            let mut out = Delivered {
                asker,
                ..Delivered::empty(path.parent().map(Into::into).unwrap_or_default())
            };
            if cancel.cancelled() {
                return Delivered::cancelled();
            }
            let made =
                ondin_export::pdf::pdf_pages(&doc, &resolved, &pages, ondin_core::ExportText::Live);
            turn.wait();
            if !cancel.begin_write() {
                return Delivered::cancelled();
            }
            let Some(export) = made else {
                out.failed = Some("the PDF could not be made".into());
                return out;
            };
            match std::fs::write(&path, &export.pdf) {
                Ok(()) => out.written = 1,
                Err(e) => out.failed = Some(e.to_string()),
            }
            let n = pages.len();
            out.head = Some(match export.approximated.len() {
                0 => format!("Exported {n} frames as one PDF"),
                k => format!("Exported {n} frames as one PDF — {k} drawn differently"),
            });
            out
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

    /// The layers a code export writes, or the sentence saying why there is
    /// nothing to write.
    pub(crate) fn code_subjects(&self) -> Result<Vec<NodeId>, &'static str> {
        let subjects = self.export_subjects();
        if subjects.is_empty() {
            return Err("Select the layers to export as code");
        }
        Ok(subjects)
    }

    /// Export the selection as `target` code into one zip the person names, and
    /// say what the report holds. Generated, zipped and written on an export
    /// worker ([`Self::spawn_export`]) from a copy of the committed document.
    pub(crate) fn export_code(&mut self, target: ondin_export::code::Target) {
        let subjects = match self.code_subjects() {
            Ok(s) => s,
            Err(why) => {
                self.session.fail(why);
                return;
            }
        };
        let name = match subjects.as_slice() {
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
        let (doc, resolved) = (self.session.doc.clone(), self.session.resolved.clone());
        let asker = self.session.path.clone();
        let label = path
            .file_name()
            .map_or_else(|| "code".into(), |n| n.to_string_lossy().into_owned());
        self.spawn_export(label, move |cancel| Delivered {
            asker,
            ..write_code(&doc, &resolved, &subjects, target, &path, cancel)
        });
    }

    /// Write the whole document as `format` into one file the person names, and
    /// say what it draws differently. Written on an export worker
    /// ([`Self::spawn_export`]) from a copy of the committed document.
    pub(crate) fn export_editable(&mut self, format: EditableFormat) {
        let stem = self
            .session
            .path
            .as_ref()
            .and_then(|p| p.file_stem())
            .map_or_else(|| "Untitled".into(), |s| s.to_string_lossy().into_owned());
        let Some(path) = rfd::FileDialog::new()
            .set_directory(self.export_start_dir())
            .add_filter(format!("{} file", format.name()), &[format.extension()])
            .set_file_name(format!("{stem}.{}", format.extension()))
            .save_file()
        else {
            return;
        };
        let (doc, resolved) = (self.session.doc.clone(), self.session.resolved.clone());
        let asker = self.session.path.clone();
        let label = path
            .file_name()
            .map_or_else(|| stem.clone(), |n| n.to_string_lossy().into_owned());
        self.spawn_export(label, move |cancel| Delivered {
            asker,
            ..write_editable(&doc, &resolved, format, &stem, &path, cancel)
        });
    }
}

/// Another tool's editable file the whole document can be written as.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum EditableFormat {
    Penpot,
    Sketch,
}

impl EditableFormat {
    pub(crate) const ALL: [EditableFormat; 2] = [EditableFormat::Penpot, EditableFormat::Sketch];

    /// The palette row.
    pub(crate) fn label(self) -> &'static str {
        match self {
            EditableFormat::Penpot => "Export as Penpot file…",
            EditableFormat::Sketch => "Export as Sketch file…",
        }
    }

    pub(crate) fn aliases(self) -> &'static [&'static str] {
        match self {
            EditableFormat::Penpot => &["penpot", "editable", "save as"],
            EditableFormat::Sketch => &["sketch", "lunacy", "editable", "save as"],
        }
    }

    fn name(self) -> &'static str {
        match self {
            EditableFormat::Penpot => "Penpot",
            EditableFormat::Sketch => "Sketch",
        }
    }

    fn extension(self) -> &'static str {
        match self {
            EditableFormat::Penpot => "penpot",
            EditableFormat::Sketch => "sketch",
        }
    }

    /// The file's bytes and what it draws differently.
    fn write(
        self,
        doc: &ondin_core::Document,
        resolved: &ondin_core::Resolved,
        name: &str,
    ) -> std::io::Result<(Vec<u8>, Vec<String>)> {
        match self {
            EditableFormat::Penpot => {
                ondin_export::penpot::export(doc, resolved, name).map(|e| (e.bytes, e.approximated))
            }
            EditableFormat::Sketch => {
                ondin_export::sketch::export(doc, resolved, name).map(|e| (e.bytes, e.approximated))
            }
        }
    }
}

/// Write the whole document as `format` to `path`, named `name` inside, off the
/// UI thread; nothing is written once `cancel` is set.
fn write_editable(
    doc: &ondin_core::Document,
    resolved: &ondin_core::Resolved,
    format: EditableFormat,
    name: &str,
    path: &std::path::Path,
    cancel: &ExportFlags,
) -> Delivered {
    if cancel.cancelled() {
        return Delivered::cancelled();
    }
    // Text is set again on this thread, line by line, in the document's faces.
    ondin_core::text::register_font_data(&resolved.text_fonts());
    let made = format.write(doc, resolved, name);
    if !cancel.begin_write() {
        return Delivered::cancelled();
    }
    let written = made.and_then(|(bytes, notes)| std::fs::write(path, bytes).map(|()| notes));
    let head = match &written {
        Ok(notes) if notes.is_empty() => format!("Exported {} file", format.name()),
        Ok(notes) => format!(
            "Exported {} file — drawn differently: {}",
            format.name(),
            notes.join("; ")
        ),
        Err(_) => format!("Could not export the {} file", format.name()),
    };
    Delivered {
        written: usize::from(written.is_ok()),
        failed: written.err().map(|e| e.to_string()),
        head: Some(head),
        ..Delivered::empty(path.parent().map(Into::into).unwrap_or_default())
    }
}

/// The zip a save dialog is seeded with for a layer named `layer`: a file name,
/// never a path (`Button / Primary` would be read as a folder), or `None` when
/// nothing of the name is usable.
fn code_archive_name(layer: &str) -> Option<String> {
    let name = ondin_export::plan::sanitize_segment(layer.trim());
    (!name.is_empty()).then(|| format!("{name}.zip"))
}

/// Write `subjects` as `target` code into the zip at `path`, off the UI thread,
/// reported as an export is; nothing is written once `cancel` is set.
fn write_code(
    doc: &ondin_core::Document,
    resolved: &ondin_core::Resolved,
    subjects: &[NodeId],
    target: ondin_export::code::Target,
    path: &std::path::Path,
    cancel: &ExportFlags,
) -> Delivered {
    if cancel.cancelled() {
        return Delivered::cancelled();
    }
    // A kit's components are laid out again on this thread, in the document's faces.
    ondin_core::text::register_font_data(&resolved.text_fonts());
    let export = target.export(doc, resolved, subjects);
    if !cancel.begin_write() {
        return Delivered::cancelled();
    }
    let written = export.zip().and_then(|bytes| std::fs::write(path, bytes));
    Delivered {
        written: usize::from(written.is_ok()),
        failed: written.err().map(|e| e.to_string()),
        head: Some(code_export_summary(target, &export)),
        ..Delivered::empty(path.parent().map(Into::into).unwrap_or_default())
    }
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
        // An icon row by its sizes, or by its name alone when it holds them all.
        if f.is_icon() {
            for s in specs.iter().filter(|s| s.format == f) {
                let sizes = s.icon_sizes();
                parts.push(match sizes.as_slice() == f.icon_sizes() {
                    true => f.label().to_string(),
                    false => format!("{} {}", f.label(), sizes_text(&sizes, false)),
                });
            }
            continue;
        }
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

/// The order export workers write in: the order they were asked for. They render
/// side by side, but each writes only once every earlier one has, so two
/// re-exports on save leave the later save's files, never a mix.
#[derive(Default)]
pub(crate) struct ExportTurns {
    /// The ticket whose turn it is to write.
    now: std::sync::Arc<(std::sync::Mutex<u64>, std::sync::Condvar)>,
    /// The next ticket to hand out.
    next: u64,
}

impl ExportTurns {
    fn ticket(&mut self) -> Turn {
        self.next += 1;
        Turn {
            now: self.now.clone(),
            n: self.next - 1,
        }
    }
}

/// One export's place in [`ExportTurns`]. Dropped, it passes the turn on, after
/// waiting for its own, so a worker that fails early still keeps the order.
struct Turn {
    now: std::sync::Arc<(std::sync::Mutex<u64>, std::sync::Condvar)>,
    n: u64,
}

impl Turn {
    /// Block until every earlier export has written.
    fn wait(&self) {
        let (lock, ready) = &*self.now;
        let mut now = lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while *now < self.n {
            now = ready
                .wait(now)
                .unwrap_or_else(std::sync::PoisonError::into_inner);
        }
    }
}

impl Drop for Turn {
    fn drop(&mut self) {
        self.wait();
        let (lock, ready) = &*self.now;
        *lock
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = self.n + 1;
        ready.notify_all();
    }
}

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
    /// The status line's report when the export words its own.
    head: Option<String>,
    /// The file of the document that asked, so a report landing on another tab
    /// names it.
    asker: Option<std::path::PathBuf>,
    /// The one file written by name, which the report shows instead of a count.
    name: Option<String>,
    /// Cancelled before it wrote anything ([`OndinApp::cancel_exports`]).
    cancelled: bool,
}

impl Delivered {
    /// Nothing written yet, to `where_`.
    fn empty(where_: std::path::PathBuf) -> Self {
        Self {
            written: 0,
            skipped: 0,
            replaced: 0,
            to_folder: false,
            failed: None,
            where_,
            head: None,
            asker: None,
            name: None,
            cancelled: false,
        }
    }

    /// An export cancelled before it wrote.
    fn cancelled() -> Self {
        Self {
            cancelled: true,
            ..Self::empty(Default::default())
        }
    }
}

/// An export on its worker, with the flags it shares with it and what the status
/// bar calls it.
pub(crate) struct RunningExport {
    job: crate::job::Job<Delivered>,
    flags: Arc<ExportFlags>,
    label: String,
}

impl RunningExport {
    fn cancelled(&self) -> bool {
        self.flags.cancelled()
    }
}

/// What the UI thread and an export's worker share: the UI sets `cancel`, the
/// worker sets `writing` once it has committed to writing, so quitting knows
/// which cancelled exports it must still wait for.
#[derive(Default)]
pub(crate) struct ExportFlags {
    cancel: AtomicBool,
    writing: AtomicBool,
}

impl ExportFlags {
    fn cancelled(&self) -> bool {
        self.cancel.load(Ordering::SeqCst)
    }

    /// Claim the write: `false` when the export is cancelled, which then writes
    /// nothing. Marked before the check, so a cancel that lands between the two
    /// either stops the write or finds it marked.
    fn begin_write(&self) -> bool {
        self.writing.store(true, Ordering::SeqCst);
        if self.cancelled() {
            self.writing.store(false, Ordering::SeqCst);
            return false;
        }
        true
    }
}

/// A planned path's file name, folders and all left off.
fn file_name_of(path: &str) -> String {
    path.rsplit('/').next().unwrap_or(path).to_string()
}

/// Render `files` and write them to `destination`, off the UI thread, writing
/// when `turn` comes; nothing is rendered further or written once `cancel` is
/// set.
///
/// A failure stops nothing that has already succeeded, and is reported rather
/// than raised.
fn deliver(
    doc: &ondin_core::Document,
    resolved: &ondin_core::Resolved,
    files: &[PlannedFile],
    destination: Destination,
    turn: &Turn,
    cancel: &ExportFlags,
) -> Delivered {
    let mut rendered: Vec<(String, Vec<u8>)> = Vec::with_capacity(files.len());
    let mut skipped = 0usize;
    for f in files {
        if cancel.cancelled() {
            return Delivered::cancelled();
        }
        match ondin_export::plan::render(doc, resolved, f) {
            Some(bytes) => rendered.push((f.path.clone(), bytes)),
            // The one refusal both writers make: a subject with no bounds at
            // all — an empty group, a layer whose contents are all hidden.
            None => skipped += 1,
        }
    }
    let mut out = Delivered {
        skipped,
        ..Delivered::empty(Default::default())
    };
    if rendered.is_empty() {
        return out;
    }
    // Asked after the wait too: an earlier export can hold the turn a while.
    turn.wait();
    if !cancel.begin_write() {
        return Delivered::cancelled();
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
            out.name = path.file_name().map(|n| n.to_string_lossy().into_owned());
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
            if let [(name, _)] = rendered.as_slice() {
                out.name = Some(file_name_of(name));
            }
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
enum ScrubStep {
    /// Keep the value in the panel's own buffer; commit nothing.
    Hold,
    /// The edit is finished — commit this, once.
    Commit(u8),
    Nothing,
}

/// A scrubbed number's whole decision, as a function of its inputs alone.
///
/// The cancel is answered first: on that frame egui may still report a `changed()`,
/// which would commit the movement the cancel discards. A drag holds and never
/// commits, since `cancel_gesture` undoes a scrub by dropping a preview and this
/// field has none. The release is recognised by the buffer, not `drag_stopped()`,
/// which is not reported on every path out of a drag.
fn scrub_step(input: ScrubInput, number: ExportNumber, value: f64, current: u8) -> ScrubStep {
    let range = number.range();
    let settled = value.round().clamp(*range.start(), *range.end()) as u8;
    if input.cancelled {
        return ScrubStep::Nothing;
    }
    if input.dragged {
        return ScrubStep::Hold;
    }
    if input.was_held || input.changed {
        return match settled == current {
            true => ScrubStep::Nothing,
            false => ScrubStep::Commit(settled),
        };
    }
    ScrubStep::Nothing
}

/// The multiplier a list of `rows` exports gets for its next one: 1×, 2×, 3×,
/// and 4× from there on.
///
/// A function of how many rows there are, not of which multipliers are free (see
/// [`OndinApp::next_export_spec`]); free so the rule is testable on its own.
fn ladder_scale(rows: usize) -> ExportScale {
    ExportScale::Times(rows.min(3) as f32 + 1.0)
}

/// An icon file's sizes on the preview's checkerboard, each at actual size with its
/// number under it, in order, as many to a line as fit; a size wider than the
/// column takes a line of its own, drawn down to the column and labelled
/// `256, shown at 216`. A size from its own frame wears the frame glyph.
fn icon_preview(ui: &mut egui::Ui, tiles: &[IconTile]) {
    let avail = ui.available_width();
    let inner = avail - PREVIEW_INSET * 2.0;
    let label_h = ui::CAPTION_PT + 4.0;
    // Lines of (tile, drawn side), packed left to right.
    let mut lines: Vec<Vec<(&IconTile, f32)>> = Vec::new();
    let mut used = f32::INFINITY;
    for t in tiles {
        let side = t.size as f32;
        if side > inner {
            lines.push(vec![(t, inner.floor())]);
            used = f32::INFINITY;
            continue;
        }
        if used + ICON_TILE_GAP + side > inner {
            lines.push(Vec::new());
            used = -ICON_TILE_GAP;
        }
        used += ICON_TILE_GAP + side;
        if let Some(line) = lines.last_mut() {
            line.push((t, side));
        }
    }
    let line_h = |l: &[(&IconTile, f32)]| l.iter().map(|(_, s)| *s).fold(0.0, f32::max) + label_h;
    let h: f32 = lines.iter().map(|l| line_h(l)).sum::<f32>()
        + ICON_TILE_GAP * lines.len().saturating_sub(1) as f32
        + PREVIEW_INSET * 2.0;
    let (ground, _) = ui.allocate_exact_size(egui::vec2(avail, h), egui::Sense::empty());
    let p = ui.painter().with_clip_rect(ground);
    ui::paint_checkerboard(&p, ground, 5.0);
    p.rect_stroke(
        ground,
        egui::CornerRadius::same(5),
        egui::Stroke::new(1.0, theme::color::field_border()),
        egui::StrokeKind::Inside,
    );
    let mut y = ground.top() + PREVIEW_INSET;
    for line in &lines {
        let tall = line_h(line) - label_h;
        let mut x = ground.left() + PREVIEW_INSET;
        for (t, side) in line {
            // Bottom-aligned on whole points, so a 16 is not drawn across pixels.
            let at = egui::Rect::from_min_size(
                egui::pos2(x, y + tall - side).round(),
                egui::vec2(*side, *side),
            );
            p.image(
                t.texture.id(),
                at,
                crate::thumbs::FULL_UV,
                egui::Color32::WHITE,
            );
            let text = match *side < t.size as f32 {
                true => format!("{}, shown at {}", t.size, *side as u32),
                false => t.size.to_string(),
            };
            let label = egui::pos2(at.center().x, y + tall + 2.0);
            let ink = theme::text::faint();
            let galley = p.layout_no_wrap(text, egui::FontId::proportional(ui::CAPTION_PT), ink);
            let glyph_w = if t.framed { ui::CAPTION_PT + 3.0 } else { 0.0 };
            let left = label.x - (galley.size().x + glyph_w) / 2.0;
            if t.framed {
                p.text(
                    egui::pos2(left, label.y),
                    egui::Align2::LEFT_TOP,
                    icon::FRAME,
                    theme::icon_font(ui::CAPTION_PT),
                    ink,
                );
            }
            p.galley(egui::pos2(left + glyph_w, label.y), galley, ink);
            x += side + ICON_TILE_GAP;
        }
        y += line_h(line) + ICON_TILE_GAP;
    }
}

/// Whether `format` is drawn as pixels — one raster or an icon file's several —
/// and so whether a trim means anything for it.
fn has_pixels(format: ExportFormat) -> bool {
    format.is_raster() || format.is_icon()
}

/// The *Sizes* chips: every size the format can hold, five to a line, lit when
/// ticked. The last ticked size refuses to go out. Answers the new ticks.
fn icon_size_chips(ui: &mut egui::Ui, spec: &ExportSpec) -> Option<Vec<u32>> {
    let allowed = spec.format.icon_sizes();
    let ticked = spec.icon_sizes();
    let labels: Vec<String> = allowed.iter().map(u32::to_string).collect();
    let last = ticked.len() == 1;
    let i = ui::toggle_grid(
        ui,
        POP_INNER,
        5,
        &labels,
        |i| ticked.contains(&allowed[i]),
        |i| match last && ticked.contains(&allowed[i]) {
            true => "An icon file needs one size",
            false => "",
        },
    )?;
    toggled_size(&ticked, allowed[i])
}

/// `ticked` with `size` ticked or unticked, smallest first; `None` when `size` is
/// the last one ticked, which an icon file cannot lose.
fn toggled_size(ticked: &[u32], size: u32) -> Option<Vec<u32>> {
    let mut next = ticked.to_vec();
    match next.iter().position(|s| *s == size) {
        Some(_) if next.len() == 1 => return None,
        Some(at) => {
            next.remove(at);
        }
        None => {
            next.push(size);
            next.sort_unstable();
        }
    }
    Some(next)
}

/// `a`, `a and b`, `a, b or c`: a list read as a sentence, joined by `word`.
fn spoken_list(items: &[String], word: &str) -> String {
    match items {
        [] => String::new(),
        [one] => one.clone(),
        [rest @ .., last] => format!("{} {word} {last}", rest.join(", ")),
    }
}

/// What *Use frames named for their size* finds for the layer called `name`:
/// which sizes come from which frames, and which are drawn from the layer.
fn size_frames_caption(name: &str, sizes: &[u32], found: &[(u32, NodeId)]) -> String {
    let named = |s: u32| format!("{name}-{s}");
    if found.is_empty() {
        let names: Vec<String> = sizes.iter().map(|s| named(*s)).collect();
        return format!("No frame named {} on this page", spoken_list(&names, "or"));
    }
    let from: Vec<String> = found.iter().map(|(s, _)| s.to_string()).collect();
    let frames: Vec<String> = found.iter().map(|(s, _)| named(*s)).collect();
    let verb = if from.len() == 1 { "comes" } else { "come" };
    let mut line = format!(
        "{} {verb} from {}",
        spoken_list(&from, "and"),
        spoken_list(&frames, "and")
    );
    let rest: Vec<String> = sizes
        .iter()
        .filter(|s| !found.iter().any(|(f, _)| f == *s))
        .map(u32::to_string)
        .collect();
    if !rest.is_empty() {
        let verb = if rest.len() == 1 { "is" } else { "are" };
        line.push_str(&format!(
            "; {} {verb} drawn from {name}",
            spoken_list(&rest, "and")
        ));
    }
    line
}

/// Up to this many sizes are listed one by one; more read as a range.
const SIZES_LISTED: usize = 4;

/// `16, 32, 48`, or past [`SIZES_LISTED`] `16 – 1024`. With `unit`, the size
/// line's form: `16, 32, 48 px`, `16 – 1024 px · 7 sizes`.
fn sizes_text(sizes: &[u32], unit: bool) -> String {
    let px = if unit { " px" } else { "" };
    match (sizes, sizes.len() > SIZES_LISTED) {
        ([first, .., last], true) => match unit {
            true => format!("{first} – {last}{px} · {} sizes", sizes.len()),
            false => format!("{first} – {last}"),
        },
        _ => {
            let list: Vec<String> = sizes.iter().map(u32::to_string).collect();
            format!("{}{px}", list.join(", "))
        }
    }
}

/// An icon row's size cell: the format's named sets, each naming its sizes, and
/// *Custom*, lit by itself when the ticks match no set and never picked. Answers
/// the sizes of a set picked.
fn icon_set_cell(ui: &mut egui::Ui, index: usize, spec: &ExportSpec) -> Option<Vec<u32>> {
    let set = spec.icon_set();
    let mut picked = None;
    egui::ComboBox::from_id_salt(("export-scale", index))
        .icon(ui::combo_chevron)
        .width(SCALE_W)
        .selected_text(ui::combo_text(set.map_or("Custom", IconSet::label)))
        .show_ui(ui, |ui| {
            ui::menu_rows(ui);
            for s in IconSet::of(spec.format) {
                let note = sizes_text(s.sizes(), false);
                let row = ui::MenuRow::new("", s.label())
                    .note(&note)
                    .selected(set == Some(*s));
                if ui::menu_row(ui, row, ui::MENU_ROW_H).clicked() {
                    picked = Some(s.sizes().to_vec());
                }
            }
            let custom = ui::MenuRow::new("", "Custom")
                .note("in settings")
                .selected(set.is_none())
                .enabled(false);
            ui::menu_row(ui, custom, ui::MENU_ROW_H)
                .on_hover_text("Tick sizes in this row's settings");
        })
        .response
        .on_hover_text("The sizes this file holds");
    picked
}

/// The settings button's state. [`ui::FieldButton::Set`] says there is something
/// in a closed popover: the border marks content, the fill marks open (§15 D72).
fn settings_button_state(spec: &ExportSpec, open: bool) -> ui::FieldButton {
    // Not the quality: every JPEG has one, so the mark would mean "this is a JPEG".
    let at_rest = spec.prefix.is_empty()
        && spec.suffix.is_empty()
        && spec.background == ExportBackground::Transparent
        && !spec.trim
        // An icon file's pad is held on, not set; its switches are its own.
        && (spec.format.is_icon() || !spec.pad_square)
        && (!spec.format.is_icon() || !(spec.size_frames || spec.pixel_fit))
        // Outlines kept from a vector format are inert on a raster, as the
        // popover hides them; so are the SVG settings on any other format.
        && (has_pixels(spec.format) || spec.text == ExportText::Live)
        && (spec.format != ExportFormat::Svg || {
            let plain = ExportSpec::new(spec.format, spec.scale);
            (spec.colour, spec.strokes, spec.view, spec.precision)
                == (plain.colour, plain.strokes, plain.view, plain.precision)
        });
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
    match (has_pixels(spec.format), spec.trim) {
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
        // `max(content, spacing.combo_width)`.
        let style = ctx.style_of(egui::Theme::Dark);
        let overhead = style.spacing.icon_spacing
            + style.spacing.icon_width
            + style.spacing.button_padding.x * 2.0;

        // Taken from the dropdown, so a new `ExportScale::COMMON` entry re-runs this.
        // A typed value can be wider (`1024h` wants 70.9); what a click produces
        // must fit. The face is `ui::combo_text`, at `ui::CONTROL_PT`.
        let widest_scale = ExportScale::COMMON
            .iter()
            .map(|n| ExportScale::Times(*n))
            .chain([ExportScale::Width(512), ExportScale::Height(512)])
            .map(|s| text_w(&s.label(), ui::CONTROL_PT))
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

    /// *Frame's own* wears the frame glyph in the dim ink on one layer or many;
    /// a `Solid` colour is a chip in that colour on any number; nothing behind
    /// the export wears the checkerboard.
    ///
    /// flip: `background_swatch` resolving *Frame's own* like the others → fails
    /// at "a white frame"; gating the resolution on `sizing` → fails at the `Solid`
    /// assertion.
    #[test]
    fn a_frame_background_wears_the_frame_glyph_and_a_solid_its_colour() {
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
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
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

        assert_eq!(
            app.background_swatch(&framed, Some(kids[0])),
            (icon::FRAME, theme::text::dim()),
            "a white frame"
        );
        assert_eq!(
            app.background_swatch(&framed, None),
            (icon::FRAME, theme::text::dim()),
            "and both frames at once"
        );

        // The other backgrounds are per-spec: the chip keeps showing them.
        let mut solid = spec(ExportFormat::Png, ExportScale::Times(1.0));
        solid.background = ExportBackground::Solid(white);
        assert_eq!(
            app.background_swatch(&solid, None),
            (icon::SQUARE_FILL, egui::Color32::WHITE),
            "White is White on thirty layers as much as on one"
        );
        let clear = spec(ExportFormat::Png, ExportScale::Times(1.0));
        assert_eq!(
            app.background_swatch(&clear, None).0,
            icon::CHECKERBOARD,
            "fixture's default is transparent, which wears the checkerboard"
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
                                ui.label(theme::icon_text(icon::FRAME, 12.0, theme::text::faint()));
                                ui.label(
                                    egui::RichText::new("100×60")
                                        .size(crate::ui::CAPTION_PT)
                                        .color(theme::text::faint()),
                                );
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
            scrub_step(
                ScrubInput {
                    cancelled,
                    dragged,
                    changed,
                    was_held,
                },
                ExportNumber::Quality,
                v,
                90,
            )
        };

        // Mid-drag: the value is live in the buffer and nothing reaches history.
        assert_eq!(step(false, true, true, false, 62.0), ScrubStep::Hold);
        assert_eq!(step(false, true, true, true, 41.0), ScrubStep::Hold);

        // The release: one commit, from the buffer rather than from `drag_stopped`.
        assert_eq!(step(false, false, false, true, 41.4), ScrubStep::Commit(41));

        // The right-click, on the frame it lands and on the release after it.
        assert_eq!(step(true, true, true, true, 41.0), ScrubStep::Nothing);
        assert_eq!(step(true, false, true, true, 41.0), ScrubStep::Nothing);

        // Typed or nudged, with no drag anywhere: committed on the spot.
        assert_eq!(step(false, false, true, false, 30.0), ScrubStep::Commit(30));
        // And a "change" that lands back on the stored value is not an edit.
        assert_eq!(step(false, false, true, false, 90.0), ScrubStep::Nothing);
        assert_eq!(step(false, false, false, false, 90.0), ScrubStep::Nothing);
    }

    /// Each number settles into its own range: a typed 9 places is the most an SVG
    /// writes, a typed 0 quality the least a JPEG takes.
    #[test]
    fn a_scrubbed_number_settles_into_its_own_range() {
        let typed = ScrubInput {
            cancelled: false,
            dragged: false,
            changed: true,
            was_held: false,
        };
        let precision = ExportNumber::Precision;
        assert_eq!(scrub_step(typed, precision, 9.0, 2), ScrubStep::Commit(4));
        assert_eq!(scrub_step(typed, precision, 0.4, 2), ScrubStep::Commit(0));
        let quality = ExportNumber::Quality;
        assert_eq!(scrub_step(typed, quality, 0.0, 90), ScrubStep::Commit(1));
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

    /// An export writes only once every export asked before it has written, even
    /// when it finishes rendering first.
    ///
    /// flip: `Turn::wait` returning at once → fails at "held behind the earlier".
    #[test]
    fn exports_write_in_the_order_asked() {
        let mut turns = ExportTurns::default();
        let (first, second, third) = (turns.ticket(), turns.ticket(), turns.ticket());
        let (tx, rx) = std::sync::mpsc::channel();
        let worker = std::thread::spawn(move || {
            third.wait();
            tx.send(()).unwrap();
        });
        let held = rx.recv_timeout(std::time::Duration::from_millis(200));
        assert!(held.is_err(), "held behind the earlier");
        drop(first);
        drop(second);
        rx.recv().expect("its turn came");
        worker.join().unwrap();
    }

    /// *Export frames, all pages…* names each page's folder legally: no `..` that
    /// climbs out of the chosen folder, no `:`, and two names one apart only in
    /// case get two folders.
    ///
    /// flip: `all_pages_frames_plan` without `sanitize_segment` → fails at "`..`".
    #[test]
    fn page_folders_are_legal_and_distinct() {
        let ctx = egui::Context::default();
        let mut app = crate::app::OndinApp::headless(&ctx);
        let mut ids = ondin_core::IdSource::new(0xF01D);
        let first = ids.mint();
        let mut doc = ondin_core::Document::new(first);
        let pages = [first, ids.mint(), ids.mint(), ids.mint()];
        let mut ops = Vec::new();
        for (i, (page, name)) in pages.iter().zip(["..", "a:b", "Home", "home"]).enumerate() {
            if i > 0 {
                ops.push(Operation::AddPage {
                    id: *page,
                    index: i,
                    name: name.into(),
                    ground: None,
                    draft: false,
                    modes: Vec::new(),
                });
            } else {
                ops.push(Operation::SetName {
                    id: *page,
                    name: name.into(),
                });
            }
            ops.push(Operation::CreateNode {
                id: ids.mint(),
                parent: *page,
                index: 0,
                kind: ondin_core::NodeKind::Artboard {
                    size: ondin_core::kurbo::Size::new(10.0, 10.0),
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Frame".into()),
            });
        }
        doc.apply(&ondin_core::Transaction(ops)).unwrap();
        app.session.adopt_document(doc, None);
        let (files, _) = app.all_pages_frames_plan();
        let folders: Vec<String> = files
            .iter()
            .map(|f| f.path.split('/').next().unwrap().to_lowercase())
            .collect();
        assert_eq!(folders.len(), 4, "a frame per page: {folders:?}");
        assert!(!folders.iter().any(|f| f == ".."), "`..`: {folders:?}");
        assert!(!folders.iter().any(|f| f.contains(':')), "{folders:?}");
        let distinct: std::collections::HashSet<_> = folders.iter().collect();
        assert_eq!(distinct.len(), 4, "{folders:?}");
    }

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
                corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                corner_smoothing: 0.0,
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
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
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
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
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
        assert_eq!(
            app.exporting().as_deref(),
            Some("Exporting 2 files…"),
            "it says it started"
        );
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
            first.starts_with("Exported “Icon"),
            "the first run writes it, by name: {first:?}"
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
            file, "Exported “named.png”",
            "a save-dialog destination names its file and not its folder"
        );

        let _ = std::fs::remove_dir_all(&dir);
    }

    /// *Cancel* stops an export from writing: the worker, held at its turn here,
    /// finds the flag set and leaves the folder empty, the status bar no longer
    /// counts it, and its landing says nothing over *Export cancelled*.
    ///
    /// flip: both of `deliver`'s flag checks removed → fails at "nothing written".
    /// Either alone does not bite: which one the worker meets first depends on
    /// whether its render started before the cancel, and the test cannot hold it
    /// between the two without a sleep.
    #[test]
    fn a_cancelled_export_writes_nothing() {
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
        let dir = std::env::temp_dir().join(format!("ondin-cancel-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();

        // An earlier ticket, held: the export renders, then waits to write.
        let held = app.export_turns.ticket();
        app.write_export(
            app.export_plan(&app.exportable_layers()),
            Destination::Folder(dir.clone()),
        );
        assert_eq!(app.exporting().as_deref(), Some("Exporting Icon.png…"));
        app.cancel_exports();
        assert_eq!(app.exporting(), None, "a cancelled export is not counted");
        assert_eq!(app.session.status().text, "Export cancelled");
        drop(held);
        app.settle_exports();
        assert_eq!(
            std::fs::read_dir(&dir).unwrap().count(),
            0,
            "nothing written"
        );
        assert_eq!(
            app.session.status().text,
            "Export cancelled",
            "landed unsaid"
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
            app.poll_exports();
            std::thread::yield_now();
        }
        assert!(
            app.session.status().text.starts_with("Exported “Icon"),
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
    /// an empty zip; the worker's zip lands as the code export's own report.
    ///
    /// flip: `write_code` leaving `head` unset → fails at "Exported React code".
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
                    corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Hero".into()),
            }
        ])));
        app.session.selection.set(vec![]);
        assert!(app.code_subjects().is_err());

        app.session.selection.set(vec![frame]);
        let subjects = app.code_subjects().expect("a selection to write");
        for target in Target::ALL {
            let out = target.export(&app.session.doc, &app.session.resolved, &subjects);
            let page = match target {
                Target::Html => "index.html",
                Target::React => "App.tsx",
                Target::Vue => "App.vue",
            };
            let text = String::from_utf8(out.file(page).expect(page).to_vec()).unwrap();
            assert!(text.contains("hero"), "{target:?} writes the frame: {text}");
        }

        // The worker's half: the zip on disk, landing as the export's report and
        // the folder remembered.
        let dir = std::env::temp_dir().join(format!("ondin-code-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let zip = dir.join("Hero.zip");
        let d = write_code(
            &app.session.doc,
            &app.session.resolved,
            &subjects,
            Target::React,
            &zip,
            &ExportFlags::default(),
        );
        app.land_export(d);
        assert!(
            std::fs::metadata(&zip).is_ok_and(|m| m.len() > 0),
            "the zip"
        );
        // Cancelled before it starts, it writes nothing.
        let early = dir.join("Early.zip");
        let flags = ExportFlags::default();
        flags.cancel.store(true, Ordering::SeqCst);
        let d = write_code(
            &app.session.doc,
            &app.session.resolved,
            &subjects,
            Target::React,
            &early,
            &flags,
        );
        assert!(d.cancelled && !early.exists(), "nothing written");
        assert_eq!(app.session.status().text, "Exported React code");
        assert_eq!(app.prefs.export_dir.as_deref(), Some(dir.as_path()));
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Quitting does not wait for a cancelled export's render, which will write
    /// nothing, but does for one that has begun writing.
    /// flip: `drain_exports` waiting on every job → fails at "not waited for".
    #[test]
    fn quitting_skips_a_cancelled_render() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let (_keep, never) = std::sync::mpsc::channel::<()>();
        app.spawn_export("slow.pdf".into(), move |_| {
            let _ = never.recv_timeout(std::time::Duration::from_secs(5));
            Delivered::cancelled()
        });
        app.cancel_exports();
        let start = std::time::Instant::now();
        app.drain_exports();
        assert!(
            start.elapsed() < std::time::Duration::from_secs(2),
            "not waited for"
        );

        let flags = ExportFlags::default();
        assert!(flags.begin_write(), "a live export writes");
        flags.cancel.store(true, Ordering::SeqCst);
        assert!(flags.writing.load(Ordering::SeqCst), "and stays waited for");
        let late = ExportFlags::default();
        late.cancel.store(true, Ordering::SeqCst);
        assert!(!late.begin_write() && !late.writing.load(Ordering::SeqCst));
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

    /// A headless app whose new document's frame is selected and exports one SVG,
    /// that row's settings open. Answers the app and the frame.
    fn svg_settings_app(ctx: &egui::Context) -> (crate::app::OndinApp, NodeId) {
        crate::theme::install(ctx);
        let mut app = crate::app::OndinApp::headless(ctx);
        let page = app.session.doc.first_page();
        let frame = app.session.doc.get(page).expect("the page").children()[0];
        let spec = ExportSpec::new(ExportFormat::Svg, ExportScale::Times(1.0));
        assert!(app.session.commit(Transaction(vec![Operation::SetExports {
            id: frame,
            exports: ondin_core::keyed_by_position([spec]),
        }])));
        app.session.selection.set_one(frame);
        app.export_row_open = Some(0);
        (app, frame)
    }

    fn precision_of(app: &crate::app::OndinApp, frame: NodeId) -> u8 {
        app.session.doc.get(frame).expect("the frame").exports()[0]
            .value
            .precision
    }

    /// A right-click mid-scrub of *Precision* puts the number back, writes nothing,
    /// and leaves the card open, with the pointer on the card or out over the canvas.
    /// flip: `export_number_field` ignoring `gesture_cancelled` → fails at "the
    /// precision is back".
    #[test]
    fn a_right_click_winds_a_precision_scrub_back() {
        for outside in [false, true] {
            let ctx = egui::Context::default();
            let (mut app, frame) = svg_settings_app(&ctx);
            let steps = app.session.history.undo_depth();
            crate::repeat::tests::scrub_then_right_click(
                &ctx,
                &mut app,
                egui::Id::new(("export-settings", 0usize)),
                "Precision",
                |at| match outside {
                    true => egui::pos2(500.0, 400.0),
                    false => at,
                },
                |app| {
                    let held = app.in_flight.export_scrub.map(|(_, _, v)| v);
                    assert!(
                        held.is_some_and(|v| v != 2.0),
                        "the drag scrubbed it: {held:?}"
                    );
                },
            );
            assert_eq!(
                precision_of(&app, frame),
                2,
                "the precision is back ({outside})"
            );
            assert!(app.in_flight.export_scrub.is_none(), "nothing held");
            assert_eq!(app.session.history.undo_depth(), steps, "nothing written");
            assert_eq!(
                app.export_row_open,
                Some(0),
                "the right-click closed nothing"
            );
        }
    }

    /// A set whose SVG row exports each variant plans a file per variant, and the
    /// row's caption counts them into the set's folder.
    #[test]
    fn a_set_row_says_how_many_files_go_into_its_folder() {
        use ondin_core::variant::{VariantProp, VariantSet};
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = crate::app::OndinApp::headless(&ctx);
        let page = app.session.doc.first_page();
        let [set, a, b] = [(); 3].map(|_| app.session.ids.mint());
        let frame = |id, parent, index| Operation::CreateNode {
            id,
            parent,
            index,
            kind: ondin_core::NodeKind::Artboard {
                size: ondin_core::kurbo::Size::new(24.0, 24.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: Some("Arrow Icons".into()),
        };
        let mut ops = vec![
            frame(set, page, 0),
            Operation::SetVariantSet {
                id: set,
                set: Some(VariantSet {
                    props: vec![VariantProp::new("Size", vec!["16".into(), "24".into()])],
                }),
            },
        ];
        for (i, (v, value)) in [(a, "16"), (b, "24")].into_iter().enumerate() {
            ops.push(frame(v, set, i));
            ops.push(Operation::SetComponent {
                id: v,
                component: true,
            });
            ops.push(Operation::SetVariant {
                id: v,
                values: vec![value.into()],
            });
        }
        let spec = ExportSpec {
            each_variant: true,
            ..ExportSpec::new(ExportFormat::Svg, ExportScale::Times(1.0))
        };
        ops.push(Operation::SetExports {
            id: set,
            exports: ondin_core::keyed_by_position([spec.clone()]),
        });
        assert!(app.session.commit(Transaction(ops)));
        let paths: Vec<String> = app
            .export_plan(&[set])
            .into_iter()
            .map(|f| f.path)
            .collect();
        assert_eq!(paths, ["arrow-icons/16.svg", "arrow-icons/24.svg"]);
        assert_eq!(
            app.export_size_label(&spec, Some(set)),
            "2 files in arrow-icons/"
        );
    }

    /// An icon row's words: the size line, the menu's ranges, the frames caption and
    /// the preset name; the last size cannot be unticked; the held pad does not mark
    /// the settings button and a fit does.
    #[test]
    fn an_icon_row_names_its_sizes_and_frames_in_words() {
        assert_eq!(sizes_text(&[16, 32, 48], true), "16, 32, 48 px");
        assert_eq!(
            sizes_text(ExportFormat::Icns.icon_sizes(), true),
            "16 – 1024 px · 7 sizes"
        );
        assert_eq!(sizes_text(&[16, 24, 32, 48, 64, 256], false), "16 – 256");
        let mut source = ondin_core::IdSource::new(1);
        let ids = [source.mint(), source.mint()];
        assert_eq!(
            size_frames_caption("logo", &[16, 32, 48], &[(16, ids[0]), (32, ids[1])]),
            "16 and 32 come from logo-16 and logo-32; 48 is drawn from logo"
        );
        assert_eq!(
            size_frames_caption("logo", &[16, 32, 48], &[]),
            "No frame named logo-16, logo-32 or logo-48 on this page"
        );
        assert_eq!(
            size_frames_caption("logo", &[16, 32], &[(16, ids[0])]),
            "16 comes from logo-16; 32 is drawn from logo"
        );
        assert_eq!(toggled_size(&[16, 48], 32), Some(vec![16, 32, 48]));
        assert_eq!(toggled_size(&[16, 48], 48), Some(vec![16]));
        assert_eq!(toggled_size(&[16], 16), None, "the last size stays");
        let ico = ExportSpec::new(ExportFormat::Ico, ExportScale::Times(1.0));
        let icns = ExportSpec::new(ExportFormat::Icns, ExportScale::Times(1.0));
        assert_eq!(preset_name(&[ico.clone(), icns]), "ICO 16, 32, 48 + ICNS");
        // An icon file's held pad is no setting of the row's; a fit is.
        let padded = ExportSpec {
            pad_square: true,
            ..ico.clone()
        };
        assert_eq!(settings_button_state(&padded, false), ui::FieldButton::Off);
        let fitted = ExportSpec {
            pixel_fit: true,
            ..ico
        };
        assert_eq!(settings_button_state(&fitted, false), ui::FieldButton::Set);
    }

    /// An ICO row's settings card offers its sizes as chips and holds the pad on;
    /// a click on the 256 chip ticks it; the open preview draws one tile per size.
    /// flip: `icon_size_chips` dropping what `toggled_size` answers → fails at "256
    /// is ticked".
    #[test]
    fn an_ico_rows_card_ticks_a_size_and_previews_each() {
        let ctx = egui::Context::default();
        let (mut app, frame) = svg_settings_app(&ctx);
        let ico = ExportSpec::new(ExportFormat::Ico, ExportScale::Times(1.0));
        assert!(app.session.commit(Transaction(vec![Operation::SetExports {
            id: frame,
            exports: ondin_core::keyed_by_position([ico]),
        }])));
        app.collapsed_panels.remove(PREVIEW);
        let mut out = crate::repeat::tests::pass(&ctx, &mut app, Vec::new());
        for _ in 0..10 {
            out = crate::repeat::tests::pass(&ctx, &mut app, Vec::new());
        }
        let card = ctx
            .memory(|m| m.area_rect(egui::Id::new(("export-settings", 0usize))))
            .expect("the card is up");
        let text_at = |out: &egui::FullOutput, words: &str| {
            out.shapes.iter().find_map(|c| match &c.shape {
                egui::Shape::Text(t) if t.galley.text() == words && card.contains(t.pos) => {
                    Some(egui::Rect::from_min_size(t.pos, t.galley.size()).center())
                }
                _ => None,
            })
        };
        assert!(text_at(&out, "SIZES").is_some(), "the Sizes section");
        assert!(text_at(&out, "Pad out to a square").is_some());
        assert!(text_at(&out, "Use frames named for their size").is_some());
        assert!(
            text_at(&out, "Text").is_none(),
            "no text setting on an icon"
        );
        let tiles = app.export_preview.as_ref().map(|p| p.icons.len());
        assert_eq!(tiles, Some(3), "a tile per favicon size");
        let chip = text_at(&out, "256").expect("the 256 chip");
        let button = |pressed| egui::Event::PointerButton {
            pos: chip,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        crate::repeat::tests::pass(&ctx, &mut app, vec![egui::Event::PointerMoved(chip)]);
        crate::repeat::tests::pass(&ctx, &mut app, vec![button(true)]);
        crate::repeat::tests::pass(&ctx, &mut app, vec![button(false)]);
        let now = app.session.doc.get(frame).expect("the frame").exports()[0]
            .value
            .icon_sizes();
        assert_eq!(now, [16, 32, 48, 256], "256 is ticked");
        assert_eq!(app.export_row_open, Some(0), "the card stays open");
    }

    /// A closed SVG row's settings button is marked by any SVG setting off its
    /// default, and a PNG's is not by SVG settings it keeps but does not use.
    #[test]
    fn an_svg_setting_marks_the_settings_button() {
        let svg = ExportSpec::new(ExportFormat::Svg, ExportScale::Times(1.0));
        assert_eq!(settings_button_state(&svg, false), ui::FieldButton::Off);
        for set in [
            ExportSpec {
                colour: SvgColour::CurrentColor,
                ..svg.clone()
            },
            ExportSpec {
                strokes: SvgStrokes::Outline,
                ..svg.clone()
            },
            ExportSpec {
                view: SvgView::Artwork,
                ..svg.clone()
            },
            ExportSpec {
                precision: 0,
                ..svg.clone()
            },
        ] {
            assert_eq!(settings_button_state(&set, false), ui::FieldButton::Set);
            let png = ExportSpec {
                format: ExportFormat::Png,
                ..set
            };
            assert_eq!(settings_button_state(&png, false), ui::FieldButton::Off);
        }
    }
}
