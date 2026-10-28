//! Variants and component properties in the inspector (§5.3d build step 7, §15
//! D982): a set's **Variants** card, a variant's face inside the Component card,
//! an instance's variant dropdowns and property rows between the card's heading (the
//! main's name, its *Instance of* caption gone since §15 D993) and *Reset all*, the
//! **Properties** card on a main or a set, and the binding line on
//! a layer inside a main.
//!
//! **A variant choice is never an override**: its dropdowns
//! carry no dot, and choosing a value *switches* the instance to another variant
//! (`variant::switch`) rather than writing a field. A component property is a view
//! over fields, so its row's dot is the bound field's override and its ↺ that
//! field's reset.
//!
//! ⚠️ **Settled by the maintainer (§15 D982):** a value
//! is renamed, moved and deleted from a popup on its chip rather than edited in
//! place and dragged; a layer is bound from one line on the Component card —
//! *Visibility* and *Content*, each a dropdown of the owner's properties — rather
//! than from a `{}` button in the layers row, the Appearance card and the Type card,
//! so no other card had to learn about properties; and a property's
//! default is the main's own bound field, edited where that field is, so the define
//! popover has no *Default* row; and a set's property name is a text field
//! with a ×.

use crate::app::OndinApp;
use crate::kits::index::KitHeading;
use crate::theme::{self, icon};
use crate::ui::{self, FieldButton};
use eframe::egui;
use ondin_core::variant::{self, PropKind, PropValue, Property};
use ondin_core::{Keyed, NodeId, Transaction, component, swap};

/// A row's label column, the width the Component card's dropdowns sit after.
const LABEL_W: f32 = 76.0;
/// The capsule's ground around a bound boolean property's switch, each side
/// (§15 D1060): 38×22 around the 26×16 switch, a chip's height.
const BOUND_SWITCH_PAD: egui::Vec2 = egui::vec2(6.0, 3.0);
/// A value chip's height.
const CHIP_H: f32 = 22.0;
/// A text property's binding: the content's slot of `variable::TextVars` alone.
const CONTENT_ONLY: [bool; 7] = [true, false, false, false, false, false, false];
/// The inner width of a value chip's menu — the top bar's View and Snap menus'
/// 160 less their frame.
const CHIP_MENU_W: f32 = 150.0;
/// The widest the Properties card's `+` menu grows (§15 D1005) — it is fitted to
/// its rows, and a nested instance with a long name would otherwise run it across
/// the canvas; past this the row's label is cut, never what it binds.
const ADD_MENU_MAX_W: f32 = 280.0;
/// The air between a `+` menu row's label and what it binds, at its widest row
/// — the ⋯ reset menu's between a label and its count.
pub(super) const ADD_MENU_GAP: f32 = 24.0;

/// An edit the Variants or Properties card asks for while it draws, **built only
/// once the card is drawn** (§15 D1009): against the document every earlier edit
/// of the frame has already changed, and naming what it edits by a key that
/// survives them.
///
/// **Why not a transaction taken while drawing** (D1008's *Fix* on these two
/// cards): a field commits on the frame it loses focus, which is the frame a click
/// elsewhere on the card lands, and every write here rewrites the owner's whole
/// property list or the whole `VariantSet`. One slot kept only the click's write;
/// committing both in turn would have the second, built from the frame's document,
/// put back what the first changed — argued from the writes, not run. An
/// instance's rows write separate fields, and D1008's `Vec` was enough there.
type Deferred = Box<dyn FnOnce(&mut EditCx<'_>) -> Result<Option<Transaction>, &'static str>>;

/// What a [`Deferred`] edit is built against: the document as the frame's earlier
/// edits left it, the id source, and the names they changed.
struct EditCx<'a> {
    doc: &'a ondin_core::Document,
    ids: &'a mut ondin_core::IdSource,
    renamed: &'a mut Renames,
}

/// The variant properties and values a frame's earlier edits renamed, old name to
/// new (§15 D1009). A set's property and value have no id — they are named, and
/// placed by index — so a later edit of the frame finds its target by the name it
/// was drawn with, followed through these: a value typed to a new name and then
/// *Move later* in its own menu moves the renamed value.
#[derive(Default)]
struct Renames {
    props: Vec<(String, String)>,
    /// The property's name when the value was renamed, the old value, the new.
    values: Vec<(String, String, String)>,
}

impl Renames {
    /// The name the property drawn as `name` has now.
    fn prop(&self, name: &str) -> String {
        let mut now = name.to_string();
        for (from, to) in &self.props {
            if *from == now {
                now = to.clone();
            }
        }
        now
    }

    /// The name the value drawn as `value`, under the property drawn as `prop`,
    /// has now.
    fn value(&self, prop: &str, value: &str) -> String {
        let prop = self.prop(prop);
        let mut now = value.to_string();
        for (p, from, to) in &self.values {
            if self.prop(p) == prop && *from == now {
                now = to.clone();
            }
        }
        now
    }
}

impl EditCx<'_> {
    /// Where the set's property drawn as `name` is now.
    fn prop_at(&self, set: NodeId, name: &str) -> Option<usize> {
        let name = self.renamed.prop(name);
        self.doc
            .get(set)?
            .set()?
            .props
            .iter()
            .position(|p| p.name == name)
    }

    /// Where the value drawn as `value`, under the property drawn as `prop`, is
    /// now: the property's place and the value's.
    fn value_at(&self, set: NodeId, prop: &str, value: &str) -> Option<(usize, usize)> {
        let pi = self.prop_at(set, prop)?;
        let value = self.renamed.value(prop, value);
        let vi = self.doc.get(set)?.set()?.props[pi]
            .values
            .iter()
            .position(|v| *v == value)?;
        Some((pi, vi))
    }

    /// The set's variant properties as they are now.
    fn set_now(&self, set: NodeId) -> Option<variant::VariantSet> {
        self.doc.get(set)?.set().cloned()
    }
}

/// The value drawn as `value`, under the property drawn as `prop`, renamed to
/// `to` — the chip's menu field and its commit on close (§15 D1003 (11)) — and
/// the rename recorded for the frame's later edits (§15 D1009).
fn rename_value_edit(set: NodeId, prop: String, value: String, to: String) -> Deferred {
    defer(move |cx| {
        let Some((pi, vi)) = cx.value_at(set, &prop, &value) else {
            return Ok(None);
        };
        let now = cx.renamed.value(&prop, &value);
        let tx = variant::rename_value(cx.doc, set, pi, vi, &to).ok_or("That value is taken")?;
        let p = cx.renamed.prop(&prop);
        cx.renamed.values.push((p, now, to));
        Ok(Some(tx))
    })
}

/// A [`Deferred`] edit from a closure.
fn defer(
    f: impl FnOnce(&mut EditCx<'_>) -> Result<Option<Transaction>, &'static str> + 'static,
) -> Deferred {
    Box::new(f)
}

/// What a row of the Properties card's `+` menu adds (§15 D1005).
#[derive(Clone, Copy)]
enum Add {
    /// A property of this kind bound to nothing, named for it.
    Kind(PropKind, &'static str),
    /// A swap property bound to this nested instance.
    Swap(NodeId),
    /// The lone main combined into a set.
    Variant,
}

/// A value the user asked to delete while variants use it, awaiting the
/// confirmation modal ([`OndinApp::value_delete_confirmation`], §15 D996): which
/// value, what it is called, and the counts the modal states.
pub(crate) struct ValueDelete {
    pub(super) set: NodeId,
    pub(super) prop: usize,
    pub(super) value: usize,
    pub(super) name: String,
    pub(super) variants: usize,
    pub(super) instances: usize,
}

/// What a [`committed_field`] holds its text to.
#[derive(Clone, Copy)]
struct Rules {
    /// Shown greyed while the field is empty.
    hint: &'static str,
    /// Names are trimmed and never empty (a layer name typed empty is discarded
    /// too, §15 D54); content is committed as typed, empty allowed (§15 D1003
    /// (10): content is not a name, and ↺ is the way back to the main).
    name: bool,
    /// The selection's values differ: the field starts **empty**, the word
    /// *Mixed* its hint, and only typing commits anything.
    mixed: bool,
}

impl Rules {
    const NAME: Self = Self {
        hint: "",
        name: true,
        mixed: false,
    };

    fn commits(&self, buf: &str, current: &str) -> Option<String> {
        let typed = if self.name { buf.trim() } else { buf };
        if self.name && typed.is_empty() {
            return None;
        }
        let seed = if self.mixed { "" } else { current };
        (buf != seed && (self.mixed || typed != current)).then(|| typed.to_string())
    }
}

/// A text field that edits `current` and answers the new text once, when the
/// field gives its focus up with something new in it — `Escape` abandons, as every
/// committed text field here does (`ui::defocus_commits`). The typed text lives in
/// egui's memory under `id` while the field has focus — and after, only when the
/// field stopped being drawn with focus still on it ([`pending`]).
///
/// **One protocol for every field of these cards**: a property's name, a
/// value's, a swap filter, and an instance's text content, each with its own
/// [`Rules`]. The content row borrowed the name field's, which seeded its buffer
/// with the word *Mixed* — so typing over a mixed selection wrote "MixOKed" to
/// every instance — and trimmed the content and refused it empty.
fn committed_field(
    ui: &mut egui::Ui,
    id: egui::Id,
    current: &str,
    size: egui::Vec2,
    pt: f32,
    rules: Rules,
) -> (egui::Response, Option<String>) {
    let seed = if rules.mixed { "" } else { current };
    let mut buf = ui
        .data(|d| d.get_temp::<String>(id))
        .unwrap_or_else(|| seed.to_string());
    let hint = if rules.mixed {
        ui::MIXED_WORD
    } else {
        rules.hint
    };
    let resp = ui
        .push_id(id, |ui| ui::text_field(ui, size, &mut buf, hint, pt))
        .inner;
    let focused = resp.has_focus();
    ui.data_mut(|d| {
        if focused {
            d.insert_temp(id, buf.clone());
        } else {
            d.remove::<String>(id);
        }
    });
    let done =
        ui::defocus_commits(&resp) || (focused && ui.input(|i| i.key_pressed(egui::Key::Enter)));
    let out = if done {
        rules.commits(&buf, current)
    } else {
        None
    };
    (resp, out)
}

/// Ask the inspector to hold the selection it drew this frame (§15 D999) for a
/// field on a popover's own layer — which D999's layer test lets go of, so a
/// popover can never pin the inspector — because this one commits (§15 D1003
/// (11)). Read by `OndinApp::inspector_panel` at the end of the frame.
pub(crate) fn hold_inspector(ctx: &egui::Context) {
    ctx.data_mut(|d| d.insert_temp(hold_id(), true));
}

/// Where [`hold_inspector`] leaves its field for the frame.
pub(crate) fn hold_id() -> egui::Id {
    egui::Id::new("inspector-popover-hold")
}

/// [`committed_field`] by name rules.
fn name_field(
    ui: &mut egui::Ui,
    id: egui::Id,
    current: &str,
    size: egui::Vec2,
    pt: f32,
) -> Option<String> {
    committed_field(ui, id, current, size, pt, Rules::NAME).1
}

/// What a [`committed_field`] was holding when it stopped being drawn with the
/// focus still on it — a popover closed by a click elsewhere — taken, so it is
/// answered once. `Escape` abandons it, as it abandons a field still drawn.
fn pending(ui: &egui::Ui, id: egui::Id, current: &str, rules: Rules) -> Option<String> {
    let buf = ui.data_mut(|d| d.remove_temp::<String>(id))?;
    if ui.input(|i| i.key_pressed(egui::Key::Escape)) {
        return None;
    }
    rules.commits(&buf, current)
}

/// A closed dropdown's face at [`ui::CONTROL_H`], in the `Ui` it is about to be
/// laid out in — the font family's two lines (`typography.rs`). The popup sets its
/// own row heights (`ui::menu_rows`), so this reaches the face alone.
fn control_height(ui: &mut egui::Ui) {
    ui.spacing_mut().interact_size.y = ui::CONTROL_H;
    ui.spacing_mut().button_padding.y = 0.0;
}

/// One option of a [`dropdown`]: its label, and why it is greyed when it is.
struct Choice {
    label: String,
    refused: Option<String>,
}

/// A dropdown of `choices` showing `shown`, answering the index picked.
///
/// **[`ui::CONTROL_H`] tall, as every other card's dropdown** (§15 D996): a combo
/// paints exactly `interact_size.y`, and left to the theme these came out 24 under
/// the cards' 28 — the font family's case (§15 D85), measured again here. Set in
/// the gate's own `Ui` so the height does not leak into the rows below.
fn dropdown(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug,
    width: f32,
    shown: &str,
    choices: &[Choice],
    current: Option<usize>,
    enabled: Option<&str>,
) -> Option<usize> {
    let mut pick = None;
    ui::disable_unless(ui, enabled.is_none(), |ui| {
        control_height(ui);
        let resp = egui::ComboBox::from_id_salt(salt)
            .icon(ui::combo_chevron)
            .width(width)
            .selected_text(egui::RichText::new(shown).size(12.0))
            .show_ui(ui, |ui| {
                ui::menu_rows(ui);
                for (i, c) in choices.iter().enumerate() {
                    let row = ui.add_enabled(
                        c.refused.is_none(),
                        egui::Button::selectable(current == Some(i), c.label.as_str()),
                    );
                    if let Some(why) = &c.refused {
                        row.on_disabled_hover_text(why);
                    } else if row.clicked() {
                        pick = Some(i);
                    }
                }
            })
            .response;
        if let Some(why) = enabled {
            resp.on_disabled_hover_text(why);
        }
    });
    pick
}

/// The **swap picker** (§15 D983): a dropdown of the mains `options` answers for
/// the search typed at its top, showing `shown`, answering the main picked and
/// whether the dropdown is open — the caller asks for the library's kits while it
/// is, so a kit's mains not yet used here can be listed (§15 D1019). The search
/// lives in egui's memory under the dropdown's id while it is open and is cleared
/// when it closes, so every opening starts from the whole filtered list. A row
/// hovers its main's description, read in `doc` or the kit's published state.
#[allow(clippy::too_many_arguments)]
fn swap_picker(
    ui: &mut egui::Ui,
    salt: impl std::hash::Hash + std::fmt::Debug + Copy,
    width: f32,
    shown: &str,
    current: Option<NodeId>,
    doc: &ondin_core::Document,
    options: impl Fn(&str) -> Vec<SwapOption>,
    enabled: Option<&str>,
) -> (Option<SwapTarget>, bool) {
    let mut pick = None;
    let mut open = false;
    let search_id = egui::Id::new(("swap-search", salt));
    ui::disable_unless(ui, enabled.is_none(), |ui| {
        control_height(ui);
        let resp = egui::ComboBox::from_id_salt(salt)
            .icon(ui::combo_chevron)
            .width(width)
            // A long main's name is cut, or the combo grows past `width` and the
            // card with it.
            .truncate()
            // A popup you click into, the font family's case (`typography.rs`):
            // `CloseOnClick` would dismiss it on the click into the search field,
            // so a pick closes it by hand below.
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .selected_text(egui::RichText::new(shown).size(12.0))
            .show_ui(ui, |ui| {
                ui::menu_rows(ui);
                ui.spacing_mut().item_spacing.y = 3.0;
                let mut search = ui
                    .data(|d| d.get_temp::<String>(search_id))
                    .unwrap_or_default();
                // The family list's search field (§15 D1085).
                ui.add(
                    egui::TextEdit::singleline(&mut search)
                        .margin(ui::SEARCH_PAD)
                        .desired_width(width.max(160.0))
                        .hint_text("Search…"),
                );
                ui.data_mut(|d| d.insert_temp(search_id, search.clone()));
                let found = options(&search);
                if found.is_empty() {
                    ui.label(
                        egui::RichText::new("No main matches")
                            .size(12.0)
                            .color(theme::text::DIM),
                    );
                }
                egui::ScrollArea::vertical()
                    .max_height(240.0)
                    .show(ui, |ui| {
                        for (heading, group) in swap_groups(found) {
                            if let Some(h) = &heading {
                                // A kit's heading wears the kit glyph, *Preferred*
                                // a star; *This document*, which is no kit, none.
                                let glyph = match h.kit.as_str() {
                                    "" => None,
                                    PREFERRED => Some(icon::STAR),
                                    _ => Some(icon::PACKAGE),
                                };
                                swap_heading(ui, glyph, &h.name, &h.status);
                            }
                            for (target, name, _) in group {
                                let on =
                                    matches!(target, SwapTarget::Here(id) if current == Some(id));
                                let mut row = ui.add(egui::Button::selectable(on, name));
                                let about = target.description(doc);
                                if !about.is_empty() {
                                    row = row.on_hover_text(about);
                                }
                                if row.clicked() {
                                    pick = Some(target);
                                    ui.close();
                                }
                            }
                        }
                    });
            });
        open = resp.inner.is_some();
        if !open {
            ui.data_mut(|d| d.remove::<String>(search_id));
        }
        if let Some(why) = enabled {
            resp.response.on_disabled_hover_text(why);
        }
    });
    (pick, open)
}

/// What a swap picker's row swaps to.
#[derive(Clone)]
pub(crate) enum SwapTarget {
    /// A main the document holds — its own, or a kit's vendored copy.
    Here(NodeId),
    /// A main of `kit`'s published state the document has never used, by its id
    /// in that state: vendored at the swap, in the swap's own transaction (§15
    /// D1019).
    Kit {
        kit: crate::kits::index::KitEntry,
        published: std::sync::Arc<ondin_core::kit::Published>,
        main: NodeId,
    },
}

impl SwapTarget {
    /// The main's description, read in `doc` or the kit's published state;
    /// empty when it has none.
    fn description(&self, doc: &ondin_core::Document) -> String {
        let (d, id) = match self {
            SwapTarget::Here(m) => (doc, *m),
            SwapTarget::Kit {
                published, main, ..
            } => (published.document(), *main),
        };
        d.get(id)
            .map(|n| n.description().to_string())
            .unwrap_or_default()
    }
}

/// A main the swap picker offers: what it swaps to, its name, and the kit it
/// comes from as a heading — or `None` for one of the document's own (§5.3e).
type SwapOption = (SwapTarget, String, Option<KitHeading>);

/// The heading key of a swap property's preferred mains, which no kit id can be.
const PREFERRED: &str = "\u{1}preferred";

/// A swap picked from a kit main not yet vendored, kept until the card is drawn:
/// building it takes the id source, which the card's borrow of the document
/// holds (§15 D1019).
struct KitSwap {
    kit: crate::kits::index::KitEntry,
    published: std::sync::Arc<ondin_core::kit::Published>,
    main: NodeId,
    roots: Vec<NodeId>,
    /// The swap property the pick was made in, or `None` for the nested copy's
    /// own *Swap* row.
    prop: Option<Keyed<Property>>,
}

/// A library kit and its published state as the shelf holds it, current.
type Shelved = (
    crate::kits::index::KitEntry,
    std::sync::Arc<ondin_core::kit::Published>,
);

/// Every swap row's list at `copy` (§15 D1019): the mains `swap::options` gives
/// — the document's own and its vault's — and each shelved kit's that the
/// document does not hold yet (`kit::swap_offers`), sorted by name.
///
/// **One heading per kit, whatever its rows** (`arch-scribe`'s reading of the
/// first cut): `kit_heading` reads a vault copy's own version and an unused main
/// would read the library's, so while an update waited a group shared by both
/// read one or the other by which name sorted first. A kit the document holds
/// anything of reads the **oldest version held** — the version the update pill
/// counts from (`Waiting::held`) — on every row; one it holds nothing of reads
/// the library's, the version a pick would vendor.
fn swap_list(
    doc: &ondin_core::Document,
    kits: &[crate::kits::index::KitEntry],
    shelves: &[Shelved],
    copy: NodeId,
    filter: &str,
    preferred: &[NodeId],
    search: &str,
) -> Vec<SwapOption> {
    let name_of = |d: &ondin_core::Document, id: NodeId| {
        d.get(id).map(|n| n.name().to_string()).unwrap_or_default()
    };
    // A swap property's preferred mains lead, in their own order and once.
    let first = swap::preferred(doc, copy, preferred, search);
    let lead: Vec<SwapOption> = first
        .iter()
        .map(|&m| {
            let heading = KitHeading {
                kit: PREFERRED.into(),
                name: "Preferred".into(),
                status: String::new(),
            };
            (SwapTarget::Here(m), name_of(doc, m), Some(heading))
        })
        .collect();
    let mut out: Vec<SwapOption> = swap::options(doc, copy, filter, search)
        .into_iter()
        .filter(|m| !first.contains(m))
        .map(|m| {
            let h = crate::kits::index::kit_heading(doc, kits, m);
            (SwapTarget::Here(m), name_of(doc, m), h)
        })
        .collect();
    for (k, p) in shelves {
        for m in ondin_core::kit::swap_offers(doc, &k.id, p, copy, filter, search) {
            let heading = KitHeading {
                kit: k.id.clone(),
                name: k.name.clone(),
                status: String::new(),
            };
            let target = SwapTarget::Kit {
                kit: k.clone(),
                published: p.clone(),
                main: m,
            };
            out.push((target, name_of(p.document(), m), Some(heading)));
        }
    }
    // The oldest version held of each kit, read off the vault's tops.
    let mut held: std::collections::HashMap<&str, u32> = std::collections::HashMap::new();
    if let Some(v) = doc.vault().and_then(|v| doc.get(v)) {
        for o in v
            .children()
            .iter()
            .filter_map(|t| doc.get(*t).and_then(|n| n.origin()))
        {
            let e = held.entry(o.kit.as_str()).or_insert(o.version);
            *e = (*e).min(o.version);
        }
    }
    for (_, _, h) in &mut out {
        let Some(h) = h else { continue };
        let Some(k) = kits.iter().find(|k| k.id == h.kit) else {
            continue; // *unavailable*, `kit_heading`'s own word.
        };
        h.status = format!(
            "v{}",
            held.get(h.kit.as_str()).copied().unwrap_or(k.version)
        );
    }
    out.sort_by(|a, b| a.1.cmp(&b.1));
    lead.into_iter().chain(out).collect()
}

/// The swap picker's list grouped as 4F has it (§15 D1014): the document's own
/// mains first, under *This document* only when a kit group follows, then one
/// group per kit by name, each in the name order `swap::options` gave. A swap
/// property's *Preferred* mains lead them all, under their own heading.
///
/// **Grouped by the kit's id, not its heading's name**: two kits may share a
/// name, and two the library has lost may both read *Kit* (`arch-scribe`'s
/// reading of the first cut, which keyed on the name).
fn swap_groups(found: Vec<SwapOption>) -> Vec<(Option<KitHeading>, Vec<SwapOption>)> {
    let mut preferred: Option<(KitHeading, Vec<SwapOption>)> = None;
    let mut own = Vec::new();
    let mut kits: Vec<(KitHeading, Vec<SwapOption>)> = Vec::new();
    for o in found {
        match o.2.clone() {
            None => own.push(o),
            Some(h) if h.kit == PREFERRED => preferred.get_or_insert((h, Vec::new())).1.push(o),
            Some(h) => match kits.iter_mut().find(|(k, _)| k.kit == h.kit) {
                Some((_, g)) => g.push(o),
                None => kits.push((h, vec![o])),
            },
        }
    }
    kits.sort_by_key(|(h, _)| (h.name.to_lowercase(), h.kit.clone()));
    // *Preferred* leads, in the property's order.
    let named = preferred.is_some() || !kits.is_empty();
    let mut out: Vec<(Option<KitHeading>, Vec<SwapOption>)> =
        preferred.into_iter().map(|(h, g)| (Some(h), g)).collect();
    if !own.is_empty() {
        let heading = named.then(|| KitHeading {
            kit: String::new(),
            name: "This document".into(),
            status: String::new(),
        });
        out.push((heading, own));
    }
    out.extend(kits.into_iter().map(|(h, g)| (Some(h), g)));
    out
}

/// A group's heading in the swap picker: the kit glyph, the name, and its status
/// on the right.
fn swap_heading(ui: &mut egui::Ui, glyph: Option<&str>, name: &str, status: &str) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 20.0), egui::Sense::hover());
    let p = ui.painter();
    let y = rect.center().y;
    let mut x = rect.left() + 4.0;
    if let Some(g) = glyph {
        p.text(
            egui::pos2(x, y),
            egui::Align2::LEFT_CENTER,
            g,
            theme::icon_font(12.0),
            theme::text::DIM,
        );
        x += 16.0;
    }
    p.text(
        egui::pos2(x, y),
        egui::Align2::LEFT_CENTER,
        name,
        egui::FontId::proportional(11.0),
        theme::text::MUTED,
    );
    p.text(
        egui::pos2(rect.right() - 4.0, y),
        egui::Align2::RIGHT_CENTER,
        status,
        egui::FontId::proportional(10.5),
        theme::text::FAINT,
    );
}

/// A row's label in the label column, **right-aligned against its control**
/// (§15 D996, `component::label_mark_in`) — every label in the column ending the
/// same distance from the field it names, whatever its length.
fn row_label(ui: &mut egui::Ui, text: &str, mark: Option<&super::component::OverrideMark>) -> bool {
    super::component::label_mark_in(
        ui,
        Some(egui::vec2(LABEL_W, ui::CONTROL_H)),
        egui::Align::Max,
        text,
        12.0,
        theme::text::MUTED,
        mark,
    )
}

/// A boolean property's row on an instance's card: the name in the label column
/// as every other property row has it, and the switch where those rows' fields
/// begin — `None` the word *Mixed* there (§15 D130). Answers the response of the
/// switch's side of the row, whose click is the toggle, and whether the marked
/// label's ↺ was clicked.
///
/// **Not [`ui::switch_row`]** (§15 D996, the maintainer's look): that is the
/// toggle-list row of the OpenType features — its label at the content edge in
/// 11.5pt, lit while on, the switch against the right margin — and among the
/// card's labelled fields it read as a row from another panel: smaller, brighter,
/// out of the label column, and its switch nowhere near the fields' edge.
///
/// **`bound`, the switch sits in the capsule** (§15 D1060): every layer the
/// property views bound to a boolean variable, the property shows the chip
/// (D1038 (d), *"the property is a view over the field (D982), so it shows
/// the chip"*) — the identity card's bound eye's capsule (§15 D1059), `text_a(28)`
/// and 44 under the pointer, around the switch, its left edge on the fields'
/// edge. The switch's own click still writes past the binding, which detaches it.
fn toggle_row(
    ui: &mut egui::Ui,
    name: &str,
    on: Option<bool>,
    mark: Option<&super::component::OverrideMark>,
    bound: bool,
) -> (egui::Response, bool) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let reset = row_label(ui, name, mark);
        let (rect, resp) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), ui::CONTROL_H),
            egui::Sense::click(),
        );
        let p = ui.painter();
        let inset = if bound { BOUND_SWITCH_PAD.x } else { 0.0 };
        if bound {
            let cap = egui::Rect::from_min_size(
                egui::pos2(
                    rect.left(),
                    rect.center().y - ui::SWITCH_SIZE.y / 2.0 - BOUND_SWITCH_PAD.y,
                ),
                ui::SWITCH_SIZE + BOUND_SWITCH_PAD * 2.0,
            );
            let fill = if resp.hovered() { 44 } else { 28 };
            p.rect_filled(cap, cap.height() / 2.0, theme::color::text_a(fill));
        }
        match on {
            Some(on) => ui::paint_switch(
                p,
                egui::Rect::from_min_size(
                    egui::pos2(
                        rect.left() + inset,
                        rect.center().y - ui::SWITCH_SIZE.y / 2.0,
                    ),
                    ui::SWITCH_SIZE,
                ),
                on,
                resp.hovered(),
            ),
            None => {
                p.text(
                    egui::pos2(rect.left() + inset, rect.center().y),
                    egui::Align2::LEFT_CENTER,
                    ui::MIXED_WORD,
                    egui::FontId::proportional(12.0),
                    theme::text::FAINT,
                );
            }
        }
        (resp, reset)
    })
    .inner
}

/// A leading glyph and a line of text, the card's one-line notes (a clash).
fn note(ui: &mut egui::Ui, glyph: &str, text: &str) -> egui::Response {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        ui.label(
            egui::RichText::new(glyph)
                .font(theme::icon_font(13.0))
                .color(theme::text::STRONG),
        );
        ui.label(
            egui::RichText::new(text)
                .size(12.0)
                .color(theme::text::STRONG),
        );
    })
    .response
}

/// *2 variants are Large, Hover* — or `None` without a clash.
pub(crate) fn clash_text(doc: &ondin_core::Document, set: NodeId) -> Option<String> {
    let clashing = variant::clashes(doc, set);
    let first = doc.get(*clashing.first()?)?;
    let same = clashing
        .iter()
        .filter(|c| doc.get(**c).is_some_and(|n| n.variant() == first.variant()))
        .count();
    Some(format!(
        "{same} variants are {}",
        variant::derived_name(first.variant())
    ))
}

impl OndinApp {
    /// Build and commit a card's [`Deferred`] edits in the order it asked for
    /// them, each against the document the one before left and each its own undo
    /// step; a refusal says why on the status line (§15 D1009).
    ///
    /// Answers the names the frame's edits changed, for what the card keeps
    /// past the frame to follow ([`ValueDelete`]).
    fn run_edits(&mut self, edits: Vec<Deferred>) -> Renames {
        let mut renamed = Renames::default();
        for edit in edits {
            let mut cx = EditCx {
                doc: &self.session.doc,
                ids: &mut self.session.ids,
                renamed: &mut renamed,
            };
            match edit(&mut cx) {
                Ok(Some(tx)) if !tx.0.is_empty() => {
                    self.session
                        .name_step("Properties", icon::SLIDERS_HORIZONTAL);
                    self.commit_edit(tx);
                }
                Ok(_) => {}
                Err(why) => self.session.info(why),
            }
        }
        renamed
    }

    /// The **Variants** card's body for the set `set`: its name and
    /// instance count, a clash note, each property with its values as chips, and the
    /// verbs that add to it.
    pub(super) fn set_body(&mut self, ui: &mut egui::Ui, set: NodeId) {
        let doc = &self.session.doc;
        let Some(s) = doc.get(set) else { return };
        let Some(vs) = s.set().cloned() else { return };
        let name = s.name().to_string();
        let variants = variant::variants(doc, set);
        let instances: usize = variants
            .iter()
            .map(|v| component::instances_of(doc, *v).len())
            .sum();
        let clash = clash_text(doc, set);
        // Every write the card asks for, built after it is drawn (§15 D1009).
        let mut edits: Vec<Deferred> = Vec::new();
        let mut select_all = false;
        let mut add_variant = false;
        // With the property's name as drawn, for the frame's renames to follow.
        let mut confirm: Option<(String, ValueDelete)> = None;

        // **One line, the set's name alone** (§15 D1000, the maintainer's look):
        // the *Component set* caption over it went as a main's and an instance's
        // did (§15 D993) — the glyph and the card's *Variants* title say it — and
        // the line is a main's face, glyph slot and all, so the name sits at the
        // height theirs do.
        // **The count of every variant's instances after the name, selecting
        // them** (§15 D1007, the maintainer's look): the *N variants · M
        // instances* row under the name, with *Select all* beside it, made a
        // stepped row; the variant count went to the card's title.
        if super::component::name_line(ui, Some(icon::SQUARES_FOUR), &name, Some(instances)) {
            select_all = true;
        }
        self.description_field(ui, set);
        let doc = &self.session.doc;
        if let Some(text) = &clash {
            note(ui, icon::WARNING, text);
        }
        for (pi, p) in vs.props.iter().enumerate() {
            // **A rule between two properties** (the maintainer's look, §15
            // D1006): a property's chips wrapped under its name sat as close to
            // the next property's name as to their own, so which chips were whose
            // took reading. The Properties card's hairline (`card_rule`, D1000).
            if pi > 0 {
                card_rule(ui);
            }
            ui.add_space(2.0);
            // The property's name, renamed in place, with its delete beside it.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
                let w = ui.available_width() - ui::CONTROL_H - ui::CARD_COL_GAP;
                let id = ui.id().with(("variant-prop", set, pi));
                if let Some(n) = name_field(ui, id, &p.name, egui::vec2(w, ui::CONTROL_H), 12.0) {
                    let drawn = p.name.clone();
                    edits.push(defer(move |cx| {
                        let Some(pi) = cx.prop_at(set, &drawn) else {
                            return Ok(None);
                        };
                        let now = cx.renamed.prop(&drawn);
                        let tx = variant::rename_property(cx.doc, set, pi, &n)
                            .ok_or("That name is taken")?;
                        cx.renamed.props.push((now, n));
                        Ok(Some(tx))
                    }));
                }
                let last = vs.props.len() < 2;
                let state = if last {
                    FieldButton::Disabled
                } else {
                    FieldButton::Off
                };
                let resp = ui::field_button(ui, icon::X, ui::CONTROL_H, 13.0, state).on_hover_text(
                    if last {
                        "A set needs at least one property"
                    } else {
                        "Delete this property"
                    },
                );
                if resp.clicked() && !last {
                    let drawn = p.name.clone();
                    edits.push(defer(move |cx| {
                        Ok(cx
                            .prop_at(set, &drawn)
                            .and_then(|pi| variant::delete_property(cx.doc, set, pi)))
                    }));
                }
            });
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(5.0, 5.0);
                for (vi, v) in p.values.iter().enumerate() {
                    let chip = ui.add(
                        egui::Button::new(egui::RichText::new(v).size(12.0))
                            .min_size(egui::vec2(0.0, CHIP_H)),
                    );
                    // The rename field's id is the value's place, and the menu
                    // closes on a move so a place never names another value
                    // under an open menu (a second *Move later* moved the
                    // neighbour back, and *Delete* deleted it).
                    let field = egui::Id::new(("variant-value", set, pi, vi));
                    let shown = egui::Popup::menu(&chip)
                        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                        .gap(ui::MENU_GAP)
                        .show(|ui| {
                            // **A width, not a minimum** (§15 D996): `menu_row`
                            // takes the available width, which in a popup's `Area`
                            // is however far the screen goes, so with only a
                            // floor the menu ran most of the way across the
                            // canvas — the ⋯ menu's case (§15 D993).
                            ui.set_width(CHIP_MENU_W);
                            ui::menu_rows(ui);
                            let (resp, renamed) = committed_field(
                                ui,
                                field,
                                v,
                                egui::vec2(CHIP_MENU_W, ui::CONTROL_H),
                                12.0,
                                Rules::NAME,
                            );
                            if resp.has_focus() {
                                hold_inspector(ui.ctx());
                            }
                            if let Some(n) = renamed {
                                edits.push(rename_value_edit(set, p.name.clone(), v.clone(), n));
                            }
                            // A move or a delete names the value as drawn, and
                            // finds it renamed if the field above committed on
                            // this frame (§15 D1009).
                            let at = move |cx: &EditCx<'_>, prop: &str, value: &str| {
                                cx.value_at(set, prop, value)
                            };
                            let (pname, vname) = (p.name.clone(), v.clone());
                            let n = p.values.len();
                            let row = |ui: &mut egui::Ui, label: &str, ok: bool| {
                                ui::menu_row(
                                    ui,
                                    ui::MenuRow::new("", label).enabled(ok),
                                    ui::MENU_ROW_H,
                                )
                                .clicked()
                                    && ok
                            };
                            for (label, ok, later) in [
                                ("Move earlier", vi > 0, false),
                                ("Move later", vi + 1 < n, true),
                            ] {
                                if row(ui, label, ok) {
                                    let (pname, vname) = (pname.clone(), vname.clone());
                                    edits.push(defer(move |cx| {
                                        Ok(at(cx, &pname, &vname).and_then(|(pi, vi)| {
                                            let to =
                                                if later { vi + 1 } else { vi.checked_sub(1)? };
                                            variant::move_value(cx.doc, set, pi, vi, to)
                                        }))
                                    }));
                                    ui.close();
                                }
                            }
                            ui::menu_sep(ui);
                            // Confirm only when variants use it, with exact
                            // counts — and the instances **detach** (§15 D982,
                            // D979 (c)). **In a
                            // modal, not in the menu** (§15 D996, the maintainer's
                            // look): the warning was a sentence inside the menu
                            // over a row that did it, so one click deleted
                            // variants.
                            let using = variant::using_value(doc, set, pi, vi);
                            if row(ui, "Delete value", n >= 2) {
                                if using.is_empty() {
                                    edits.push(defer(move |cx| {
                                        Ok(at(cx, &pname, &vname).and_then(|(pi, vi)| {
                                            variant::delete_value(cx.doc, set, pi, vi)
                                        }))
                                    }));
                                } else {
                                    confirm = Some((
                                        pname.clone(),
                                        ValueDelete {
                                            set,
                                            prop: pi,
                                            value: vi,
                                            name: v.clone(),
                                            variants: using.len(),
                                            instances: using
                                                .iter()
                                                .map(|u| component::instances_of(doc, *u).len())
                                                .sum(),
                                        },
                                    ));
                                }
                                ui.close();
                            }
                        });
                    // **A rename typed and left commits when the menu closes**
                    // (§15 D1003 (11)): a click on the canvas closes the popup
                    // before its field can see its focus go, and the inspector
                    // holds the set on screen for the frame that takes
                    // (`hold_inspector`, D999's promise extended to a popover
                    // field that commits).
                    if shown.is_none()
                        && let Some(n) = pending(ui, field, v, Rules::NAME)
                    {
                        edits.push(rename_value_edit(set, p.name.clone(), v.clone(), n));
                    }
                }
                let add = ui.add(
                    egui::Button::new(
                        egui::RichText::new(icon::PLUS)
                            .font(theme::icon_font(12.0))
                            .color(theme::text::MUTED),
                    )
                    .min_size(egui::vec2(CHIP_H, CHIP_H)),
                );
                if add.on_hover_text("Add a value").clicked() {
                    let drawn = p.name.clone();
                    edits.push(defer(move |cx| {
                        let (Some(pi), Some(vs)) = (cx.prop_at(set, &drawn), cx.set_now(set))
                        else {
                            return Ok(None);
                        };
                        Ok(variant::add_value(
                            cx.doc,
                            set,
                            pi,
                            &variant::next_value_name(&vs, pi),
                        ))
                    }));
                }
            });
        }
        ui.add_space(2.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
            let half = (ui.available_width() - ui::CARD_COL_GAP) / 2.0;
            if ui::action_button(
                ui,
                icon::PLUS,
                "Property",
                FieldButton::Off,
                egui::vec2(half, ui::CONTROL_H),
            )
            .on_hover_text("Add a variant property")
            .clicked()
            {
                edits.push(defer(move |cx| {
                    Ok(cx.set_now(set).and_then(|vs| {
                        variant::add_property(
                            cx.doc,
                            set,
                            &variant::next_property_name(&vs),
                            "Default",
                        )
                    }))
                }));
            }
            if ui::action_button(
                ui,
                icon::HEXAGON,
                "Add variant",
                FieldButton::Off,
                egui::vec2(half, ui::CONTROL_H),
            )
            .on_hover_text("A copy of the last variant, at the next free combination")
            .clicked()
            {
                add_variant = true;
            }
        });
        let renamed = self.run_edits(edits);
        // 🚨 **The confirmation follows the frame's renames** (D1009's ⚠️ run):
        // a used value's name typed in its menu commits on the frame *Delete
        // value* is clicked, after this was built from the name as drawn — so
        // the modal named a value that no longer existed, and *Delete* found no
        // value of that name and deleted nothing, silently. It takes the value's
        // name and place now.
        if let Some((drawn_prop, mut c)) = confirm {
            let prop = renamed.prop(&drawn_prop);
            let value = renamed.value(&drawn_prop, &c.name);
            let at = self
                .session
                .doc
                .get(set)
                .and_then(|s| s.set())
                .and_then(|vs| {
                    let pi = vs.props.iter().position(|p| p.name == prop)?;
                    let vi = vs.props[pi].values.iter().position(|v| *v == value)?;
                    Some((pi, vi))
                });
            if let Some((pi, vi)) = at {
                (c.prop, c.value, c.name) = (pi, vi, value);
                self.deleting_value = Some(c);
            }
        }
        if select_all {
            self.select_all_instances();
        }
        if add_variant {
            self.add_variant();
        }
    }

    /// The confirmation for deleting a value variants use (§15 D996), a dialog
    /// rather than a warning inside the chip's menu: what goes and what detaches, then
    /// *Cancel* and *Delete*. `Escape`, the backdrop and the ✕ cancel — the safe
    /// arm, §15 D377's reading and the close confirmation's (§15 D525) — and a
    /// value that is no longer there by the time *Delete* is pressed deletes
    /// nothing.
    pub(crate) fn value_delete_confirmation(&mut self, ctx: &egui::Context) {
        let Some(pending) = &self.deleting_value else {
            return;
        };
        let mut delete = false;
        let mut cancel = false;
        let detach = match pending.instances {
            0 => String::new(),
            n => format!(" {} will detach.", plural(n, "instance")),
        };
        let body = format!(
            "{} {} “{}”, and {} deleted with it.{detach}",
            plural(pending.variants, "variant"),
            if pending.variants == 1 { "uses" } else { "use" },
            pending.name,
            if pending.variants == 1 { "is" } else { "are" },
        );
        let modal = crate::settings::card("confirm-value-delete", ctx, |ui| {
            ui.set_width(ui::menu_inner_w(320.0, crate::settings::PAD));
            if crate::settings::modal_title(ui, "Delete value") {
                cancel = true;
            }
            ui.add_space(8.0);
            ui.add(
                egui::Label::new(
                    egui::RichText::new(body)
                        .size(12.0)
                        .color(theme::text::MUTED),
                )
                .wrap(),
            );
            ui.add_space(crate::settings::FOOTER_GAP);
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                if crate::settings::text_button(ui, "Cancel", FieldButton::Off).clicked() {
                    cancel = true;
                }
                if crate::settings::text_button(ui, "Delete", FieldButton::Danger).clicked() {
                    delete = true;
                }
            });
        });
        if modal.should_close() {
            cancel = true;
        }
        if delete && let Some(p) = self.deleting_value.take() {
            let still = self
                .session
                .doc
                .get(p.set)
                .and_then(|s| s.set())
                .and_then(|s| s.props.get(p.prop))
                .and_then(|q| q.values.get(p.value))
                .is_some_and(|v| *v == p.name);
            if still
                && let Some(tx) = variant::delete_value(&self.session.doc, p.set, p.prop, p.value)
            {
                self.session.name_step("Delete value", icon::TRASH);
                self.commit_edit(tx);
            }
        } else if cancel {
            self.deleting_value = None;
        }
    }

    /// A variant's rows inside the Component card: one dropdown per
    /// property — choosing a value moves this main to that combination, a taken
    /// one showing the clash rather than being refused — and the clash note.
    pub(super) fn variant_rows(&mut self, ui: &mut egui::Ui, main: NodeId) {
        let doc = &self.session.doc;
        let (Some(set), Some(m)) = (variant::set_of(doc, main), doc.get(main)) else {
            return;
        };
        let Some(vs) = doc.get(set).and_then(|s| s.set()).cloned() else {
            return;
        };
        let values = m.variant().to_vec();
        let mut out = None;
        let mut select = None;
        for (pi, p) in vs.props.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                row_label(ui, &p.name, None);
                let choices: Vec<Choice> = p
                    .values
                    .iter()
                    .map(|v| Choice {
                        label: v.clone(),
                        refused: None,
                    })
                    .collect();
                let current = p.values.iter().position(|v| Some(v) == values.get(pi));
                let w = ui.available_width();
                let shown = values.get(pi).map(String::as_str).unwrap_or("");
                if let Some(i) = dropdown(
                    ui,
                    ("variant-of", main, pi),
                    w,
                    shown,
                    &choices,
                    current,
                    None,
                ) {
                    let mut next = values.clone();
                    next[pi] = p.values[i].clone();
                    out = variant::set_values(doc, main, next);
                }
            });
        }
        let other = variant::clashes(doc, set)
            .into_iter()
            .find(|c| *c != main && doc.get(*c).is_some_and(|n| n.variant() == values));
        if let Some(o) = other {
            ui.horizontal(|ui| {
                // **Two words, the sentence in the tooltip** (§15 D1000, the
                // maintainer's look): *Another variant is also Large, Primary*
                // ran under *Select it* and was cut. A scope's own response is
                // never hovered, so the note's rect is sensed by hand.
                let row = note(ui, icon::WARNING, "Variant clash");
                ui.interact(row.rect, ui.id().with("clash-tip"), egui::Sense::hover())
                    .on_hover_text(format!(
                        "Another variant in this set is also {} — give each variant \
                         a combination of its own",
                        variant::derived_name(&values)
                    ));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    let label = "Select it";
                    let w = ui::action_button_w(ui.ctx(), label);
                    if ui::action_button(
                        ui,
                        icon::ARROW_SQUARE_OUT,
                        label,
                        FieldButton::Off,
                        egui::vec2(w, ui::CONTROL_H),
                    )
                    .clicked()
                    {
                        select = Some(o);
                    }
                });
            });
        }
        if let Some(tx) = out {
            self.session.name_step("Variant", icon::DIAMOND);
            self.commit_edit(tx);
        }
        if let Some(o) = select {
            self.session.selection.set_one(o);
        }
    }

    /// The rows an instance — or several sharing one owner — gets between the
    /// card's heading and *Reset all*: a dropdown per variant property,
    /// switching the instances (a missing combination greyed, with the reason),
    /// then each component property, with its override mark — and then **the
    /// nested instances it shows**, each under a sub-heading of its own with the
    /// same rows over its copies (§15 D988, 4M–4S).
    pub(super) fn instance_rows(&mut self, ui: &mut egui::Ui, roots: &[NodeId]) {
        self.own_rows(ui, roots, false);
        self.shown_groups(ui, roots);
    }

    /// The groups of [`Self::instance_rows`] (4M–4S): one per nested instance the
    /// roots' owner shows, flattened across levels and named by the › path of
    /// the shown layers and the main the copy shows now (4P, 4Q). Over several
    /// roots a group matches by slot — its layers' name path — and a slot that
    /// shows different mains in them says so rather than guess which properties
    /// apply (4S). A group the instance hides dims, its rows read-only (4R — the
    /// line naming what hides it gone since §15 D1000); its dot still shows, and
    /// *Reset all* and ⋯ still reach it.
    ///
    /// **No Swap row inside a group**: the copy's swap is an outer swap
    /// property's row where one drives it, and otherwise is made from the copy's
    /// own card — the group holds the copy's own properties alone, a
    /// *Badge · Count* group drawn with *Count* and *Dot only* and nothing else. The
    /// main is named as its set where it is a variant — *Icon · Arrow right*, not
    /// the variant's derived *Regular* (4M).
    fn shown_groups(&mut self, ui: &mut egui::Ui, roots: &[NodeId]) {
        struct Group {
            path: Vec<String>,
            main: Option<String>,
            copies: Vec<NodeId>,
            hidden: Option<String>,
        }
        if roots.is_empty() {
            return;
        }
        let groups: Vec<Group> = {
            let doc = &self.session.doc;
            let per_root: Vec<Vec<variant::ShownNested>> = roots
                .iter()
                .map(|r| variant::shown_nested(doc, *r))
                .collect();
            let name_of = |id: NodeId| {
                doc.get(id)
                    .map(|n| n.name().to_string())
                    .unwrap_or_default()
            };
            per_root[0]
                .iter()
                .filter_map(|g| {
                    let matched: Vec<&variant::ShownNested> = per_root
                        .iter()
                        .map(|gs| gs.iter().find(|h| h.key == g.key))
                        .collect::<Option<_>>()?;
                    let copies: Vec<NodeId> = matched.iter().map(|h| h.copy).collect();
                    let mains: Vec<Option<NodeId>> =
                        copies.iter().map(|c| component::main_of(doc, *c)).collect();
                    let same = mains.iter().all(|m| *m == mains[0]);
                    let hidden = roots
                        .iter()
                        .zip(&matched)
                        .find_map(|(r, h)| variant::hidden_by(doc, *r, h));
                    // A variant reads as its set.
                    let shown_as = |m: NodeId| name_of(variant::set_of(doc, m).unwrap_or(m));
                    Some(Group {
                        path: g.path.iter().map(|c| name_of(*c)).collect(),
                        main: same.then(|| mains[0].map(shown_as)).flatten(),
                        copies,
                        hidden,
                    })
                })
                .collect()
        };
        for g in groups {
            ui.add_space(4.0);
            // 4M's sub-heading: the instance's hexagon, then the path in the row
            // labels' type with its › and the main a step fainter.
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                ui.label(
                    egui::RichText::new(icon::HEXAGON)
                        .font(theme::icon_font(14.0))
                        .color(theme::text::MUTED),
                );
                for (i, name) in g.path.iter().enumerate() {
                    if i > 0 {
                        ui.label(egui::RichText::new("›").size(12.0).color(theme::text::DIM));
                    }
                    ui.label(
                        egui::RichText::new(name)
                            .size(12.0)
                            .color(theme::text::MUTED),
                    );
                }
                if let Some(main) = &g.main {
                    ui.label(
                        egui::RichText::new(format!("· {main}"))
                            .size(12.0)
                            .color(theme::text::DIM),
                    );
                }
            });
            if g.main.is_none() {
                note(ui, icon::STACK, "Different mains");
                ui.label(
                    egui::RichText::new("Select one to edit its properties")
                        .size(11.5)
                        .color(theme::text::DIM),
                );
                continue;
            }
            match &g.hidden {
                None => self.own_rows(ui, &g.copies, true),
                // **Dimmed and disabled, with no line saying so** (§15 D1000, the
                // maintainer's look: the faded rows are affordance enough). 4R's
                // *Hidden by …* caption went.
                Some(_) => {
                    ui.scope(|ui| {
                        ui.disable();
                        self.own_rows(ui, &g.copies, true);
                    });
                }
            }
        }
    }

    /// *Chosen by Icon*: a nested instance whose main a swap property
    /// sets says which, the main's own property inside a main, and *on Sign up*
    /// after it for a copy inside an outer instance. Nothing for an instance no
    /// swap property drives.
    pub(super) fn chosen_by_note(&mut self, ui: &mut egui::Ui, root: NodeId) {
        let doc = &self.session.doc;
        let by_prop = |owner: NodeId, of: NodeId| {
            variant::bound_to(doc, owner, of, PropKind::Swap).map(|p| p.name.clone())
        };
        let found = match variant::owner_above(doc, root) {
            Some(owner) => by_prop(owner, root).map(|p| (p, None)),
            None => doc
                .get(root)
                .and_then(|n| n.parent())
                .and_then(|p| component::instance_root(doc, p))
                .and_then(|outer| {
                    variant::instance_properties(doc, outer)
                        .into_iter()
                        .find(|p| {
                            p.kind == PropKind::Swap
                                && variant::counterparts(doc, outer, p).contains(&root)
                        })
                        .map(|p| {
                            let on = doc.get(outer).map(|o| o.name().to_string());
                            (p.name.clone(), on)
                        })
                }),
        };
        let Some((prop, on)) = found else { return };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            for (text, ink) in [
                ("Chosen by".to_string(), theme::text::DIM),
                (prop, theme::text::STRONG),
            ]
            .into_iter()
            .chain(on.map(|o| (format!("on {o}"), theme::text::DIM)))
            {
                ui.label(egui::RichText::new(text).size(11.5).color(ink));
            }
        });
    }

    /// *Show properties on instances*: on a nested instance inside a main,
    /// the switch that shows its own properties on every instance of the main —
    /// every variant's, in a set ([`variant::set_shown`]) — with a caption naming
    /// them. Nothing for an instance that is not a slot inside a main.
    pub(super) fn show_switch(&mut self, ui: &mut egui::Ui, root: NodeId) {
        let doc = &self.session.doc;
        let Some(owner) = variant::owner_above(doc, root) else {
            return;
        };
        if !doc
            .get(root)
            .and_then(|n| n.link())
            .and_then(|l| doc.get(l))
            .is_some_and(|l| l.component())
        {
            return;
        }
        let on = variant::slot_is_shown(doc, root);
        let names = shown_names(doc, root);
        let owner_name = doc
            .get(owner)
            .map(|o| o.name().to_string())
            .unwrap_or_default();
        let caption = match names.len() {
            0 => "Nothing to show yet: its main has no properties".to_string(),
            1 => format!("{} appears on every {owner_name}", names[0]),
            _ => format!("{} appear on every {owner_name}", names.join(", ")),
        };
        let resp = ui::switch_row(ui, "Show properties on instances", on, ui::CONTROL_H)
            .on_hover_text(
                "Show this instance's own properties on the card of every instance of its main",
            );
        ui.label(
            egui::RichText::new(caption)
                .size(11.0)
                .color(theme::text::DIM),
        );
        if resp.clicked()
            && let Some(tx) = variant::set_shown(doc, &mut self.session.ids, root, !on)
        {
            self.session
                .name_step("Show properties on instances", icon::EYE);
            self.commit_edit(tx);
        }
    }

    /// The rows of [`Self::instance_rows`] for `roots` themselves. `group` when
    /// they are a shown group's copies, which draw no Swap row (4O, see
    /// [`Self::shown_groups`]).
    fn own_rows(&mut self, ui: &mut egui::Ui, roots: &[NodeId], group: bool) {
        let rev = self.session.revision();
        let doc = &self.session.doc;
        let Some(&first) = roots.first() else { return };
        let Some(main) = component::main_of(doc, first) else {
            return;
        };
        let Some(owner) = variant::owner_of_main(doc, main) else {
            return;
        };
        // **Every row's write, not the last one** (§15 D1008, the maintainer's
        // report): a text field commits on the frame it loses focus, which is the
        // frame a click on the switch below it lands, and one `Option` here kept
        // the switch's write and dropped the text typed into the field. Each is
        // committed in turn, its own undo step — the two touch different fields,
        // but for a text row's ↺ and its own field, where the ↺ wins (below).
        let mut out: Vec<Transaction> = Vec::new();
        let name_of = |id: NodeId| {
            doc.get(id)
                .map(|n| n.name().to_string())
                .unwrap_or_default()
        };
        // The library's kits, for the swap pickers' headings: which kit a
        // vendored main is from, and whether the library can still find it.
        let kits = crate::kits::index::kits_in(&self.library, doc.meta().id.as_deref());
        // And their published states, for the mains a swap could vendor (§15
        // D1019) — only those read at the kit file's current time, as an accept
        // reads them, so a pick vendors what the version it records published.
        let shelves: Vec<Shelved> = kits
            .iter()
            .filter_map(|k| self.kit_shelf.current(k).map(|p| (k.clone(), p)))
            .collect();
        let swap_list = |copy: NodeId, filter: &str, preferred: &[NodeId], search: &str| {
            swap_list(doc, &kits, &shelves, copy, filter, preferred, search)
        };
        let mut kit_swap: Option<KitSwap> = None;
        let mut swap_open = false;
        // **Swap**, on a nested copy inside an outer instance (§15 D983) — every
        // main of its kind, the session's: a swap property narrows this list for
        // an instance's users, and this row is the nested copy's own, with no
        // property to filter it. A swap is an override, marked and reset as one.
        // Not inside a shown group (4O).
        if !group && roots.iter().all(|r| swap::can_swap(doc, *r)) {
            let shows: Vec<Option<NodeId>> =
                roots.iter().map(|r| component::main_of(doc, *r)).collect();
            let mixed = shows.iter().any(|m| *m != shows[0]);
            let shown = match (mixed, shows[0]) {
                (true, _) => "Mixed".to_string(),
                (false, Some(m)) => name_of(m),
                (false, None) => String::new(),
            };
            let resets: Vec<ondin_core::Operation> = roots
                .iter()
                .filter(|r| doc.get(**r).is_some_and(|n| n.swap().is_some()))
                .map(|r| ondin_core::Operation::SetSwap { id: *r, swap: None })
                .collect();
            let slot = swap::slot_main(doc, first).map(name_of).unwrap_or_default();
            let mark = (!resets.is_empty()).then(|| super::component::OverrideMark {
                tip: format!("Reset to main · {slot}"),
                tx: Transaction(resets),
            });
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                if row_label(ui, "Swap", mark.as_ref()) {
                    out.extend(mark.as_ref().map(|m| m.tx.clone()));
                }
                let w = ui.available_width();
                let current = (!mixed).then_some(shows[0]).flatten();
                let options = |search: &str| swap_list(first, "", &[], search);
                let (pick, open) = swap_picker(
                    ui,
                    ("swap-pick", first),
                    w,
                    &shown,
                    current,
                    doc,
                    options,
                    None,
                );
                swap_open |= open;
                match pick {
                    Some(SwapTarget::Here(m)) => {
                        // A shown copy carries its variant values by name (4Q).
                        let ops: Vec<ondin_core::Operation> = roots
                            .iter()
                            .filter_map(|r| variant::swap_carrying(doc, *r, m))
                            .flat_map(|tx| tx.0)
                            .collect();
                        out.push(Transaction(ops));
                    }
                    Some(SwapTarget::Kit {
                        kit,
                        published,
                        main,
                    }) => {
                        kit_swap = Some(KitSwap {
                            kit,
                            published,
                            main,
                            roots: roots.to_vec(),
                            prop: None,
                        });
                    }
                    None => {}
                }
            });
        }
        // **Main**, on an instance linked straight to a main: the same picker, but the
        // pick relinks (`variant::switch`), carrying overrides by name path. Not an
        // override, so no mark; not inside a shown group.
        if !group && roots.iter().all(|r| swap::can_relink(doc, *r)) {
            let shows: Vec<Option<NodeId>> =
                roots.iter().map(|r| component::main_of(doc, *r)).collect();
            let mixed = shows.iter().any(|m| *m != shows[0]);
            let shown = match (mixed, shows[0]) {
                (true, _) => "Mixed".to_string(),
                (false, Some(m)) => name_of(m),
                (false, None) => String::new(),
            };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 0.0;
                row_label(ui, "Main", None);
                let w = ui.available_width();
                let current = (!mixed).then_some(shows[0]).flatten();
                let options = |search: &str| swap_list(first, "", &[], search);
                let (pick, open) = swap_picker(
                    ui,
                    ("main-pick", first),
                    w,
                    &shown,
                    current,
                    doc,
                    options,
                    None,
                );
                swap_open |= open;
                if let Some(SwapTarget::Here(m)) = pick {
                    let ops: Vec<ondin_core::Operation> = roots
                        .iter()
                        .filter(|r| swap::can_relink_to(doc, **r, m))
                        .filter_map(|r| variant::switch(doc, *r, m, &mut self.session.ids))
                        .flat_map(|tx| tx.0)
                        .collect();
                    // The main it already shows relinks nothing.
                    if !ops.is_empty() {
                        out.push(Transaction(ops));
                    }
                }
            });
        }
        let switchable = roots.iter().all(|r| variant::can_switch(doc, *r));
        if let Some(set) = variant::set_of(doc, main)
            && let Some(vs) = doc.get(set).and_then(|s| s.set()).cloned()
        {
            let mains: Vec<NodeId> = roots
                .iter()
                .filter_map(|r| component::main_of(doc, *r))
                .collect();
            for (pi, p) in vs.props.iter().enumerate() {
                let shown_of = |m: &NodeId| doc.get(*m).and_then(|n| n.variant().get(pi).cloned());
                let first_value = shown_of(&mains[0]);
                let mixed = mains.iter().any(|m| shown_of(m) != first_value);
                let shown = if mixed {
                    "Mixed".to_string()
                } else {
                    first_value.clone().unwrap_or_default()
                };
                // A value is offered where every instance has somewhere to go.
                let choices: Vec<Choice> = p
                    .values
                    .iter()
                    .map(|v| {
                        let missing = mains
                            .iter()
                            .find(|m| variant::switch_target(doc, **m, pi, v).is_none());
                        Choice {
                            label: v.clone(),
                            refused: missing.map(|m| {
                                let mut want = doc
                                    .get(*m)
                                    .map(|n| n.variant().to_vec())
                                    .unwrap_or_default();
                                if let Some(slot) = want.get_mut(pi) {
                                    *slot = v.clone();
                                }
                                format!(
                                    "No {} variant in {}",
                                    variant::derived_name(&want),
                                    doc.get(set).map(|s| s.name()).unwrap_or("")
                                )
                            }),
                        }
                    })
                    .collect();
                let current = (!mixed)
                    .then(|| {
                        p.values
                            .iter()
                            .position(|v| Some(v) == first_value.as_ref())
                    })
                    .flatten();
                // **A nested copy's choice is a swap, so an override, and its
                // dropdown draws the dot** (§15 D988, reversing D983's build,
                // which drew none): against the slot's own main's value for
                // the property of that name, reset within the set the copy shows
                // now. A root linked straight to a main has none — *"a variant
                // choice is never an override"* there (§15 D982).
                let states: Vec<(String, bool)> = roots
                    .iter()
                    .filter_map(|r| variant::nested_variant_state(doc, *r, pi))
                    .collect();
                let mark = states.iter().any(|(_, d)| *d).then(|| {
                    let source = states
                        .iter()
                        .find(|(_, d)| *d)
                        .map(|(s, _)| s.clone())
                        .unwrap_or_default();
                    super::component::OverrideMark {
                        tip: format!("Reset to main · {source}"),
                        tx: Transaction(
                            roots
                                .iter()
                                .filter_map(|r| variant::nested_variant_reset(doc, *r, pi))
                                .flat_map(|t| t.0)
                                .collect(),
                        ),
                    }
                });
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    if row_label(ui, &p.name, mark.as_ref()) {
                        out.extend(mark.as_ref().map(|m| m.tx.clone()));
                    }
                    let w = ui.available_width();
                    let refuse =
                        (!switchable).then_some("Only an instance or a nested copy switches");
                    if let Some(i) = dropdown(
                        ui,
                        ("variant-pick", first, pi),
                        w,
                        &shown,
                        &choices,
                        current,
                        refuse,
                    ) {
                        let value = p.values[i].clone();
                        let mut ops = Vec::new();
                        for (r, m) in roots.iter().zip(&mains) {
                            let Some(to) = variant::switch_target(doc, *m, pi, &value) else {
                                continue;
                            };
                            if to == *m {
                                continue;
                            }
                            if let Some(tx) = variant::switch(doc, *r, to, &mut self.session.ids) {
                                ops.extend(tx.0);
                            }
                        }
                        out.push(Transaction(ops));
                    }
                });
            }
        }
        // Component properties, from the owner — not its showing, which is the
        // groups after these rows (`shown_groups`).
        let props: Vec<Keyed<Property>> = doc
            .get(owner)
            .map(|o| o.props().to_vec())
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.kind != PropKind::Nested)
            .collect();
        // Each row's readings from `PropCache` while the document is unchanged,
        // and its reset built only for a row that is overridden — the mark and
        // its ↺ are the reset's only readers. Uncached and unconditional, the two
        // weighed on every frame with hundreds of instances selected.
        let cache = self.prop_cache.at(rev);
        for p in &props {
            let states: Vec<(PropValue, bool)> = roots
                .iter()
                .filter_map(|r| {
                    cache
                        .states
                        .entry((*r, p.id))
                        .or_insert_with(|| {
                            #[cfg(test)]
                            super::component::count(|c| c.state += 1);
                            variant::property_state(doc, *r, p)
                        })
                        .clone()
                })
                .collect();
            let Some((value, _)) = states.first().cloned() else {
                continue;
            };
            let mixed = states.iter().any(|(v, _)| *v != value);
            let overridden = states.iter().any(|(_, o)| *o);
            let mark = overridden.then(|| {
                let (tip, reset) = cache
                    .marks
                    .entry((roots.to_vec(), p.id))
                    .or_insert_with(|| {
                        #[cfg(test)]
                        super::component::count(|c| c.reset += 1);
                        let default = main_default(doc, main, p);
                        (
                            format!("Reset to main · {}", say(doc, &default)),
                            variant::reset_property(doc, roots, p),
                        )
                    })
                    .clone();
                super::component::OverrideMark {
                    tip,
                    tx: Transaction(reset),
                }
            });
            let reset = || mark.as_ref().map(|m| m.tx.clone());
            match p.kind {
                // Filtered out above: a showing has no value to draw.
                PropKind::Nested => {}
                PropKind::Boolean => {
                    let on = matches!(value, PropValue::Boolean(true));
                    // **Every layer the property views bound, the switch sits in
                    // the capsule** (§15 D1060), the *Mixed* rule as the fields
                    // keep it (§15 D1045): one variable, the chip's tooltip —
                    // *name · value · Mode*, or *name · Mixed* where modes give
                    // the layers different values; several, the capsule over the
                    // switch or *Mixed* with their names listed. A binding on
                    // some and not others draws none, D130's plain row.
                    let chip = cache
                        .chips
                        .entry((roots.to_vec(), p.id))
                        .or_insert_with(|| bound_chip(doc, roots, p, mixed, |n| n.visible_var()))
                        .clone()
                        .map(|(_, tip)| tip);
                    let (resp, reset_hit) = toggle_row(
                        ui,
                        &p.name,
                        (!mixed).then_some(on),
                        mark.as_ref(),
                        chip.is_some(),
                    );
                    if reset_hit {
                        out.extend(reset());
                    }
                    // **What the click does, said truly**: over a *Mixed* row it
                    // shows every layer, and only those it changes — the hidden
                    // ones — detach; the shown keep their binding
                    // (`variant::set_property` writes against what shows).
                    let verb = if mixed {
                        "Click to show all; the hidden ones detach"
                    } else {
                        "Click to toggle and detach"
                    };
                    let tip = match (&chip, &mark) {
                        (Some(c), Some(m)) => Some(format!("{c}\n{verb}\n{}", m.tip)),
                        (Some(c), None) => Some(format!("{c}\n{verb}")),
                        (None, Some(m)) => Some(m.tip.clone()),
                        (None, None) => None,
                    };
                    let resp = match tip {
                        Some(t) => resp.on_hover_text(t),
                        None => resp,
                    };
                    if resp.clicked() {
                        let next = if mixed { true } else { !on };
                        // Each bound layer the click changes detached in the edit
                        // itself, the detach the tooltip promises — asked for, so
                        // the floating status says nothing (§15 D1071).
                        let mut ops: Vec<_> = roots
                            .iter()
                            .flat_map(|r| variant::counterparts(doc, *r, p))
                            .filter(|c| {
                                doc.get(*c).is_some_and(|n| n.visible_var().is_some())
                                    && doc.shown(*c) != next
                            })
                            .flat_map(|c| ondin_core::build::detach_visible_ops(doc, c))
                            .collect();
                        ops.extend(variant::set_property(
                            doc,
                            roots,
                            p,
                            &PropValue::Boolean(next),
                        ));
                        out.push(Transaction(ops));
                    }
                }
                PropKind::Text => {
                    let text_chip = cache
                        .chips
                        .entry((roots.to_vec(), p.id))
                        .or_insert_with(|| bound_chip(doc, roots, p, mixed, |n| n.text_vars()[0]))
                        .clone();
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        // 🚨 **The ↺ is the user's last act, and wins**:
                        // clicking it with text typed into this row's own field
                        // commits that field on the same frame, after the
                        // reset, and put the typing back over it — two undo
                        // steps whose sum was the reset lost. The two are one field, the
                        // case D1008's "different fields" left out.
                        let reset_clicked = row_label(ui, &p.name, mark.as_ref());
                        if reset_clicked {
                            out.extend(reset());
                        }
                        let text = match &value {
                            PropValue::Text(t) => t.clone(),
                            _ => String::new(),
                        };
                        let w = ui.available_width();
                        let id = ui.id().with(("prop-text", first, p.name.as_str()));
                        // **Every layer it views bound, the row is the chip**
                        // (D1038 (d), §15 D1070's amendment) — the property a view
                        // over the field (D982), so it shows what the Type card's
                        // Content row shows: `Backspace` or the × detaches each
                        // to the string it draws, and a key typed starts the
                        // text that replaces it, held under `retype` until the
                        // field commits it (which detaches, `keep_text_bindings`)
                        // or `Escape` gives the chip back.
                        let retype_id = id.with("retype");
                        let retype = ui.data(|d| d.get_temp::<String>(retype_id));
                        if let (Some((label, tip)), None) = (&text_chip, &retype) {
                            let (rect, _) = ui.allocate_exact_size(
                                egui::vec2(w, ui::CONTROL_H),
                                egui::Sense::hover(),
                            );
                            let tip = match &mark {
                                Some(m) => format!("{tip}\n{}", m.tip),
                                None => tip.clone(),
                            };
                            let got =
                                super::binding::chip(ui, rect, id.with("chip"), label, &tip, true);
                            if reset_clicked {
                                return;
                            }
                            if got.detach {
                                let ops: Vec<_> = roots
                                    .iter()
                                    .flat_map(|r| variant::counterparts(doc, *r, p))
                                    .flat_map(|c| {
                                        ondin_core::build::detach_text_ops(doc, c, CONTENT_ONLY)
                                    })
                                    .collect();
                                out.push(Transaction(ops));
                            } else if let Some(t) = got.typed {
                                ui.data_mut(|d| d.insert_temp(retype_id, t));
                            }
                            return;
                        }
                        // Content, not a name: as typed, empty allowed (§15
                        // D1003 (10)); over a mixed selection *Mixed* is the
                        // hint and the field starts empty.
                        let rules = Rules {
                            hint: "",
                            name: false,
                            mixed: mixed && retype.is_none(),
                        };
                        let seed = retype.clone().unwrap_or(text);
                        let (resp, committed) = committed_field(
                            ui,
                            id,
                            &seed,
                            egui::vec2(w, ui::CONTROL_H),
                            12.0,
                            rules,
                        );
                        if retype.is_some() {
                            if !resp.has_focus() && !resp.lost_focus() {
                                resp.request_focus();
                                super::binding::caret_to_end(ui.ctx(), resp.id, &seed);
                            } else if resp.lost_focus() {
                                ui.data_mut(|d| d.remove::<String>(retype_id));
                            }
                        }
                        if let Some(t) = committed
                            && !reset_clicked
                        {
                            // **Detached first, as the Content chip's retype is**
                            // (§15 D1071's amendment): a bound copy this text
                            // changes is detached in the edit itself, so the
                            // detach is the one asked for and the floating status
                            // says nothing — left to `keep_text_bindings`, the
                            // same key said *Detached copy/cta* here and nothing
                            // on the Type card.
                            let mut ops: Vec<_> = roots
                                .iter()
                                .flat_map(|r| variant::counterparts(doc, *r, p))
                                .filter(|c| {
                                    doc.get(*c).is_some_and(|n| n.text_vars()[0].is_some())
                                        && !matches!(
                                            doc.resolved_kind(*c).as_deref(),
                                            Some(ondin_core::NodeKind::Text { content, .. })
                                                if *content == t
                                        )
                                })
                                .flat_map(|c| {
                                    ondin_core::build::detach_text_ops(doc, c, CONTENT_ONLY)
                                })
                                .collect();
                            ops.extend(variant::set_property(doc, roots, p, &PropValue::Text(t)));
                            out.push(Transaction(ops));
                        }
                    });
                }
                // An instance swap (§15 D983): the mains the property's filter
                // offers, searched; a pick swaps each instance's copy.
                PropKind::Swap => {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 0.0;
                        if row_label(ui, &p.name, mark.as_ref()) {
                            out.extend(reset());
                        }
                        let shown = match (&value, mixed) {
                            (_, true) => "Mixed".to_string(),
                            (PropValue::Swap(m), false) => name_of(*m),
                            _ => String::new(),
                        };
                        let current = match (&value, mixed) {
                            (PropValue::Swap(m), false) => Some(*m),
                            _ => None,
                        };
                        let copy = variant::counterparts(doc, first, p).first().copied();
                        let w = ui.available_width();
                        let options = |search: &str| {
                            copy.map(|c| swap_list(c, &p.filter, &p.preferred, search))
                                .unwrap_or_default()
                        };
                        let salt = ("prop-swap", first, p.id);
                        let (pick, open) =
                            swap_picker(ui, salt, w, &shown, current, doc, options, None);
                        swap_open |= open;
                        match pick {
                            Some(SwapTarget::Here(m)) => {
                                out.push(Transaction(variant::set_property(
                                    doc,
                                    roots,
                                    p,
                                    &PropValue::Swap(m),
                                )));
                            }
                            Some(SwapTarget::Kit {
                                kit,
                                published,
                                main,
                            }) => {
                                kit_swap = Some(KitSwap {
                                    kit,
                                    published,
                                    main,
                                    roots: roots.to_vec(),
                                    prop: Some(p.clone()),
                                });
                            }
                            None => {}
                        }
                    });
                }
            }
        }
        for tx in out.into_iter().filter(|t| !t.0.is_empty()) {
            self.session.name_step("Property", icon::SLIDERS_HORIZONTAL);
            self.commit_edit(tx);
        }
        // While a swap list is open, the library's kits are read, so their mains
        // not used here join it once loaded (§15 D1019); free when held.
        if swap_open {
            for k in &kits {
                self.kit_shelf.ask(k);
            }
        }
        if let Some(s) = kit_swap {
            self.swap_to_kit(s);
        }
    }

    /// Swap `s.roots` to a kit main this document has never used (§15 D1019):
    /// vendor it, then the swap the row would have made — the nested copy's own,
    /// carrying its variant values (4Q), or the property's — against the document
    /// the vendoring leaves, all one transaction and one undo step.
    fn swap_to_kit(&mut self, s: KitSwap) {
        let built = ondin_core::kit::vendored(
            &self.session.doc,
            &mut self.session.ids,
            &s.kit.id,
            s.kit.version,
            &s.published,
            s.main,
        );
        let (mut ops, scratch, shown) = match built {
            Ok(b) => b,
            // A main, or a kit's modes, held at an older version — a held kit
            // lacking a mode the publish read (§15 D1064).
            Err(
                ondin_core::kit::KitError::NeedsUpdate(_) | ondin_core::kit::KitError::Modes(_),
            ) => {
                self.session.fail(
                    "This kit has an update to accept before that component can be swapped in"
                        .to_string(),
                );
                return;
            }
            // §15 D1036 (2): a kit built on this one.
            Err(ondin_core::kit::KitError::Circular(own)) => {
                let own = self.session.doc.get(own).map_or("component", |n| n.name());
                self.session.fail(format!(
                    "That component uses this kit's {own}, so it cannot be swapped in here"
                ));
                return;
            }
            Err(e) => {
                self.session
                    .fail(format!("That component could not be swapped in ({e:?})"));
                return;
            }
        };
        let swap: Vec<ondin_core::Operation> = match &s.prop {
            None => s
                .roots
                .iter()
                .filter_map(|r| variant::swap_carrying(&scratch, *r, shown))
                .flat_map(|tx| tx.0)
                .collect(),
            Some(p) => variant::set_property(&scratch, &s.roots, p, &PropValue::Swap(shown)),
        };
        if swap.is_empty() {
            self.session
                .fail("That component cannot be swapped in here".to_string());
            return;
        }
        ops.extend(swap);
        self.session.name_step("Swap", icon::SWAP);
        if self.commit_edit(Transaction(ops))
            && let Some(doc) = self.session.doc.meta().id.clone()
        {
            // A kit used now, first in the insert picker's order.
            let order = self.library.local.kit_use.entry(doc).or_default();
            order.retain(|k| *k != s.kit.id);
            order.insert(0, s.kit.id.clone());
            self.library.local.save();
        }
    }

    /// The **Properties** card on a main or a set: each property's kind,
    /// name and default, and what it is bound to; a row's name renames it and its ×
    /// deletes it. **One is added from the header's `+`** (§15 D1005), a menu
    /// of *Boolean*, *Text*, an *Instance swap* for each nested instance no swap
    /// property drives, and on a lone main *Variant*, which wraps it in a set.
    pub(super) fn inspector_properties(&mut self, ui: &mut egui::Ui) {
        // **Asked before the card decides whether to draw** (§15 D1005's
        // 2026-10-08 amendment, `arch-scribe`'s find): a property is bound from a
        // layer inside the main, a selection this card skips, and an edge that
        // only moved when the card drew would read the main as the subject it
        // already was — so the card stayed shut over the property just made.
        let fresh = self.subject_changed("Properties");
        let Some(id) = self.session.selection.single() else {
            return;
        };
        let doc = &self.session.doc;
        let Some(owner) = variant::owner_of_main(doc, id) else {
            return;
        };
        // A variant's card is its set's, shown on the set.
        if owner != id {
            return;
        }
        // The showing is listed apart, under *Shown from nested*.
        let props: Vec<Keyed<Property>> = doc
            .get(owner)
            .map(|o| o.props().to_vec())
            .unwrap_or_default()
            .into_iter()
            .filter(|p| p.kind != PropKind::Nested)
            .collect();
        let slots = variant::nested_slots(doc, owner);
        let lone_main = doc.get(owner).is_some_and(|n| n.component());
        // **A nested instance no swap property drives is offered as one**, in the
        // `+` menu — the way back after a swap property is deleted (§15 D1000,
        // where it was a row of its own with a `+` on the card; D1005 moved it).
        let dormant: Vec<(NodeId, String)> = slots
            .iter()
            .map(|(s, _)| *s)
            .filter(|s| {
                !props
                    .iter()
                    .any(|p| p.kind == PropKind::Swap && p.bound.contains(s))
            })
            .map(|s| {
                let name = doc.get(s).map(|n| n.name().to_string()).unwrap_or_default();
                (s, name)
            })
            .collect();
        let no_slots = slots.is_empty();
        // Every write the card asks for, built after it is drawn (§15 D1009).
        let mut edits: Vec<Deferred> = Vec::new();
        let mut combine = false;
        let mut select = None;
        // **Collapsed while empty, opened by an add** — Fill's and Stroke's rule
        // (`sync_paint_collapse`, the maintainer's ask, §15 D1005): an empty card
        // was a header, a `+` and a line of grey text. Empty is no property and
        // nothing shown from a nested instance, the card's two lists.
        let empty = props.is_empty() && !slots.iter().any(|(_, shown)| *shown);
        self.sync_paint_collapse("Properties", empty, fresh);
        let action = ui::HeadAction::new(icon::PLUS, "Add a property");
        let clicks = self.panel_head(ui, "Properties", None, Some(action), None, |app, ui| {
            let doc = &app.session.doc;
            if props.is_empty() {
                ui.label(
                    egui::RichText::new(
                        // Short on purpose (§15 D993): the sentence it replaced
                        // named all three kinds of binding, and ran too long to
                        // be read in the card.
                        "Bind layers from their Component line",
                    )
                    .size(11.5)
                    .color(theme::text::DIM),
                );
            }
            // **Three groups with a rule between each** (§15 D1000, the
            // maintainer's look — the card read as one undivided list): the
            // boolean and text properties; the swap properties, a rule between
            // each; and what is shown from nested instances.
            let (swaps, plain): (Vec<&Keyed<Property>>, Vec<&Keyed<Property>>) =
                props.iter().partition(|p| p.kind == PropKind::Swap);
            for p in &plain {
                property_row(ui, doc, owner, p, &mut edits);
            }
            let mut first = plain.is_empty();
            for p in &swaps {
                if !std::mem::take(&mut first) {
                    card_rule(ui);
                }
                property_row(ui, doc, owner, p, &mut edits);
            }
            // **Shown from nested**: the nested instances whose properties
            // the main's instances show, with what they show and a − to stop — a
            // click on a name selects it. **Only those, and only when there are
            // any** (§15 D1000): a heading over a list of *Not shown* rows read
            // as something the card wanted done.
            let shown: Vec<NodeId> = slots.iter().filter(|(_, s)| *s).map(|(s, _)| *s).collect();
            if !shown.is_empty() {
                card_rule(ui);
                ui.label(ui::eyebrow("Shown from nested"));
                for slot in &shown {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        // The instance's own mark (§15 D1005): the swap glyph
                        // is a swap property's, and this row is not one.
                        ui.label(
                            egui::RichText::new(icon::HEXAGON)
                                .font(theme::icon_font(14.0))
                                .color(theme::text::MUTED),
                        );
                        let name = doc
                            .get(*slot)
                            .map(|n| n.name().to_string())
                            .unwrap_or_default();
                        let name_resp = ui
                            .add(
                                egui::Label::new(
                                    egui::RichText::new(&name)
                                        .size(12.0)
                                        .color(theme::text::STRONG),
                                )
                                .sense(egui::Sense::click()),
                            )
                            .on_hover_text("Select it in the main");
                        if name_resp.clicked() {
                            select = Some(*slot);
                        }
                        ui.label(
                            egui::RichText::new(format!(
                                "· {}",
                                shown_names(doc, *slot).join(", ")
                            ))
                            .size(12.0)
                            .color(theme::text::DIM),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                            if ui::field_button(
                                ui,
                                icon::MINUS,
                                ui::CONTROL_H,
                                13.0,
                                FieldButton::Off,
                            )
                            .on_hover_text("Stop showing its properties on instances")
                            .clicked()
                            {
                                let slot = *slot;
                                edits.push(defer(move |cx| {
                                    Ok(variant::set_shown(cx.doc, cx.ids, slot, false))
                                }));
                            }
                        });
                    });
                }
            }
        });
        // **The `+` menu** (§15 D1005), overturning D995's *Bool* / *Text* /
        // *Var* buttons on the card — the maintainer's call. Each row's kind in
        // full, and in the accelerator column what it binds: the row is wide enough for both, where a third of the card was not
        // (D995's reason for the short words).
        let mut add = None;
        if let Some(anchor) = clicks.action_rect {
            egui::Popup::new(
                ui.id().with("props-add"),
                ui.ctx().clone(),
                anchor,
                ui.layer_id(),
            )
            .kind(egui::PopupKind::Menu)
            .layout(egui::Layout::top_down_justified(egui::Align::Min))
            .style(egui::containers::menu::menu_style)
            .open_memory(clicks.acted.then_some(egui::SetOpenCommand::Toggle))
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .align(egui::RectAlign::BOTTOM_END)
            .gap(ui::MENU_GAP)
            .show(|ui| {
                ui::menu_rows(ui);
                // **A width, not a minimum** (§15 D996), and **fitted to the widest
                // row** as the ⋯ reset menu is (§15 D993): `menu_row`'s 8pt insets,
                // its 24pt glyph column, the label at 11.5 and what it binds at
                // 10.5. A fixed 200 left *Instance swap* and *nested instance*
                // touching (the maintainer's look).
                let ctx = ui.ctx().clone();
                let w = |t: &str, size: f32| {
                    ctx.fonts_mut(|f| {
                        f.layout_no_wrap(
                            t.to_string(),
                            egui::FontId::proportional(size),
                            egui::Color32::WHITE,
                        )
                        .size()
                        .x
                    })
                };
                let swap_named: Vec<&str> = if dormant.is_empty() {
                    vec!["nested instance"]
                } else {
                    dormant.iter().map(|(_, n)| n.as_str()).collect()
                };
                let widest = [("Boolean", "visibility"), ("Text", "text content")]
                    .into_iter()
                    .chain(lone_main.then_some(("Variant", "makes a set")))
                    .chain(swap_named.into_iter().map(|n| ("Instance swap", n)))
                    .map(|(label, what)| w(label, 11.5) + w(what, 10.5))
                    .fold(0.0, f32::max);
                ui.set_width(
                    (8.0 + 24.0 + widest + ADD_MENU_GAP + 8.0)
                        .ceil()
                        .min(ADD_MENU_MAX_W),
                );
                let row = |ui: &mut egui::Ui, glyph: &str, label: &str, what: &str, ok: bool| {
                    ui::menu_row(
                        ui,
                        ui::MenuRow::new(glyph, label).accel(Some(what)).enabled(ok),
                        ui::MENU_ROW_H,
                    )
                };
                if row(ui, icon::EYE, "Boolean", "visibility", true).clicked() {
                    add = Some(Add::Kind(PropKind::Boolean, "Boolean"));
                    ui.close();
                }
                if row(ui, icon::TEXT_T, "Text", "text content", true).clicked() {
                    add = Some(Add::Kind(PropKind::Text, "Text"));
                    ui.close();
                }
                // One row per nested instance it can bind, named in the
                // accelerator column; one dimmed row saying why when there is none.
                for (slot, name) in &dormant {
                    if row(ui, icon::SWAP, "Instance swap", name, true).clicked() {
                        add = Some(Add::Swap(*slot));
                        ui.close();
                    }
                }
                if dormant.is_empty() {
                    row(ui, icon::SWAP, "Instance swap", "nested instance", false).on_hover_text(
                        if no_slots {
                            "This main has no nested instance to swap"
                        } else {
                            "Every nested instance already has a swap property"
                        },
                    );
                }
                if lone_main {
                    ui::menu_sep(ui);
                    if row(ui, icon::SQUARES_FOUR, "Variant", "makes a set", true)
                        .on_hover_text("Make a set — this main becomes its first variant")
                        .clicked()
                    {
                        add = Some(Add::Variant);
                        ui.close();
                    }
                }
            });
        }
        if let Some(add) = add {
            // A `+` on a collapsed header is a legal press, and what it adds is
            // shown (the Effects card's rule, `open_paint_panel`).
            self.open_paint_panel("Properties");
            match add {
                // The property keeps the kind's full name (§15 D995), made
                // fresh against the list as the frame's earlier edits left it.
                Add::Kind(kind, name) => edits.push(defer(move |cx| {
                    let now = cx
                        .doc
                        .get(owner)
                        .map(|o| o.props().to_vec())
                        .unwrap_or_default();
                    Ok(variant::define(
                        cx.doc,
                        cx.ids,
                        owner,
                        &fresh_name(&now, name),
                        kind,
                        Vec::new(),
                    )
                    .map(|(tx, _)| tx))
                })),
                Add::Swap(slot) => edits.push(defer(move |cx| {
                    Ok(new_swap_property(cx.doc, cx.ids, owner, slot))
                })),
                Add::Variant => combine = true,
            }
        }
        self.run_edits(edits);
        if combine {
            self.combine_as_variants();
        }
        if let Some(s) = select {
            self.session.selection.set_one(s);
        }
    }

    /// The binding line on a layer inside a main, drawn as one line: its
    /// visibility, and for text its content, each a dropdown of the owner's
    /// properties of that kind — *None*, each property, and *New property*.
    pub(super) fn bind_line(&mut self, ui: &mut egui::Ui, node: NodeId) {
        let doc = &self.session.doc;
        let Some(owner) = variant::owner_above(doc, node) else {
            return;
        };
        let Some(n) = doc.get(node) else { return };
        let is_text = matches!(n.kind(), ondin_core::NodeKind::Text { .. });
        // A nested instance inside the main: its main can be a swap property
        // (§15 D983), with the filter suggested from the main it shows.
        let shows = n
            .link()
            .filter(|l| doc.get(*l).is_some_and(|l| l.component()));
        let layer = n.name().to_string();
        let filter = shows
            .and_then(|m| doc.get(m))
            .map(|m| swap::suggested_filter(m.name()))
            .unwrap_or_default();
        let props = doc
            .get(owner)
            .map(|o| o.props().to_vec())
            .unwrap_or_default();
        let mut out: Option<Transaction> = None;
        let kinds: &[(PropKind, &str)] = match (is_text, shows.is_some()) {
            (true, _) => &[
                (PropKind::Boolean, "Visibility"),
                (PropKind::Text, "Content"),
            ],
            (false, true) => &[
                (PropKind::Boolean, "Visibility"),
                (PropKind::Swap, "Instance"),
            ],
            (false, false) => &[(PropKind::Boolean, "Visibility")],
        };
        // **Rows of the identity card, not a card of their own** (§15 D995): the
        // heading has no curly-brace glyph, and the rows sit under the layer's name
        // and its boolean row.
        ui.scope(|ui| {
            ui.label(
                egui::RichText::new("Bind to a component property")
                    .size(12.0)
                    .color(theme::text::STRONG),
            );
            for (kind, label) in kinds {
                let of_kind: Vec<&Keyed<Property>> =
                    props.iter().filter(|p| p.kind == *kind).collect();
                let bound = of_kind.iter().position(|p| p.bound.contains(&node));
                let mut choices = vec![Choice {
                    label: "None".into(),
                    refused: None,
                }];
                choices.extend(of_kind.iter().map(|p| Choice {
                    label: p.name.clone(),
                    refused: None,
                }));
                choices.push(Choice {
                    label: "New property…".into(),
                    refused: None,
                });
                let shown = bound
                    .map(|b| of_kind[b].name.clone())
                    .unwrap_or_else(|| "None".into());
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    row_label(ui, label, None);
                    let w = ui.available_width();
                    let current = Some(bound.map_or(0, |b| b + 1));
                    let Some(i) = dropdown(
                        ui,
                        ("bind", node, *label),
                        w,
                        &shown,
                        &choices,
                        current,
                        None,
                    ) else {
                        return;
                    };
                    let pick = match i {
                        0 => Bind::None,
                        i if i == choices.len() - 1 => Bind::New(self.session.ids.mint_item()),
                        i => Bind::To(of_kind[i - 1].id),
                    };
                    let next = rebind(&props, *kind, node, pick, &layer, &filter);
                    out = Some(Transaction(vec![ondin_core::Operation::SetProperties {
                        id: owner,
                        props: next,
                    }]));
                });
            }
        });
        if let Some(tx) = out {
            self.session.name_step("Bind property", icon::LINK_SIMPLE);
            self.commit_edit(tx);
        }
    }
}

/// What a binding dropdown was set to ([`rebind`]).
#[derive(Clone, Copy, Debug)]
enum Bind {
    None,
    /// The property with this item id.
    To(ondin_core::ItemId),
    /// *New property…*, made under this item id.
    New(ondin_core::ItemId),
}

/// An owner's properties with `node` bound, for `kind`, as `pick` says — the
/// decision [`OndinApp::bind_line`] commits, lifted out so a test can ask it.
/// The layer comes off **every** property of the kind first, so it is never
/// bound twice for one field (which `variant::settle` would prune silently,
/// losing the new binding); a new property is named for the layer — `Show
/// {layer}`, `{layer} text`, the layer's name — made fresh among the others,
/// and a swap property takes `filter`, the suggestion from the main the layer
/// shows.
fn rebind(
    props: &[Keyed<Property>],
    kind: PropKind,
    node: NodeId,
    pick: Bind,
    layer: &str,
    filter: &str,
) -> Vec<Keyed<Property>> {
    let mut next: Vec<Keyed<Property>> = props.to_vec();
    for p in next.iter_mut().filter(|p| p.kind == kind) {
        p.value.bound.retain(|b| *b != node);
    }
    match pick {
        Bind::None => {}
        Bind::To(id) => {
            if let Some(p) = next.iter_mut().find(|p| p.id == id && p.kind == kind) {
                p.value.bound.push(node);
            }
        }
        Bind::New(item) => {
            let name = fresh_name(
                &next,
                &match kind {
                    PropKind::Boolean => format!("Show {layer}"),
                    PropKind::Text => format!("{layer} text"),
                    PropKind::Swap | PropKind::Nested => layer.to_string(),
                },
            );
            next.push(Keyed::new(
                item,
                Property {
                    name,
                    kind,
                    bound: vec![node],
                    filter: match kind {
                        PropKind::Swap => filter.to_string(),
                        PropKind::Boolean | PropKind::Text | PropKind::Nested => String::new(),
                    },
                    preferred: Vec::new(),
                },
            ));
        }
    }
    next
}

/// The properties a shown nested instance `slot` brings to the card (the
/// switch's caption, the *Shown from nested* rows): its main's set's variant properties, then its own
/// component properties — what [`OndinApp::instance_rows`] draws for it.
fn shown_names(doc: &ondin_core::Document, slot: NodeId) -> Vec<String> {
    let mut names: Vec<String> = Vec::new();
    if let Some(main) = component::main_of(doc, slot) {
        if let Some(vs) = variant::set_of(doc, main).and_then(|s| doc.get(s)?.set().cloned()) {
            names.extend(vs.props.into_iter().map(|p| p.name));
        }
        names.extend(
            variant::instance_properties(doc, slot)
                .into_iter()
                .filter(|p| p.kind != PropKind::Nested)
                .map(|p| p.value.name),
        );
    }
    names
}

/// One row of the Properties card: the kind's glyph, the name (renamed in
/// place), the default, the ×, and what it is bound to under it.
fn property_row(
    ui: &mut egui::Ui,
    doc: &ondin_core::Document,
    owner: NodeId,
    p: &Keyed<Property>,
    edits: &mut Vec<Deferred>,
) {
    // Each write names the property by its item id and is built after the card
    // is drawn (§15 D1009), so a rename typed here and a click on another row's
    // × on the frame it commits both land.
    let item = p.id;
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
        let glyph = match p.kind {
            PropKind::Boolean => icon::EYE,
            PropKind::Text => icon::TEXT_T,
            PropKind::Swap => icon::SWAP,
            // Never drawn as a row: `inspector_properties` lists it apart.
            PropKind::Nested => icon::STACK,
        };
        ui.label(
            egui::RichText::new(glyph)
                .font(theme::icon_font(14.0))
                .color(theme::text::MUTED),
        );
        let w = ui.available_width() - ui::CONTROL_H - ui::CARD_COL_GAP;
        let id = ui.id().with(("prop-name", owner, p.id));
        if let Some(n) = name_field(ui, id, &p.name, egui::vec2(w, ui::CONTROL_H), 12.0) {
            edits.push(defer(move |cx| {
                // Another property's name, or one of a set's variant properties
                // — `variant::check`'s `PropertyName`, asked before the commit is.
                let taken = cx.doc.get(owner).is_some_and(|o| {
                    o.props().iter().any(|q| q.id != item && q.name == n)
                        || o.set().is_some_and(|s| s.props.iter().any(|v| v.name == n))
                });
                if taken {
                    return Err("That name is taken");
                }
                Ok(variant::edit_property(cx.doc, owner, item, |q| {
                    Some(Property {
                        name: n.clone(),
                        ..q.clone()
                    })
                }))
            }));
        }
        if ui::field_button(ui, icon::X, ui::CONTROL_H, 13.0, FieldButton::Off)
            .on_hover_text("Delete this property — its layers keep their values")
            .clicked()
        {
            edits.push(defer(move |cx| {
                Ok(variant::edit_property(cx.doc, owner, item, |_| None))
            }));
        }
    });
    let bound: Vec<String> = p
        .bound
        .iter()
        .filter_map(|b| doc.get(*b))
        .map(|n| {
            format!(
                "{} · {}",
                n.name(),
                match p.kind {
                    PropKind::Boolean => "visibility",
                    PropKind::Text => "content",
                    // *main*, the field a swap sets (§15 D1005): *Icon · main*.
                    PropKind::Swap => "main",
                    PropKind::Nested => "shown",
                }
            )
        })
        .collect();
    let default = p
        .bound
        .first()
        .and_then(|b| variant::field_value(doc, *b, p.kind));
    let default = say(doc, &default);
    let line = match bound.as_slice() {
        [] => "Not bound to any layer".to_string(),
        [one] => format!("{default} · {one}"),
        [one, rest @ ..] => format!("{default} · {one} and {} more", rest.len()),
    };
    ui.label(egui::RichText::new(line).size(11.0).color(theme::text::DIM));
    // A swap property's filter (§15 D983 (5)): the prefix of a main's name the
    // picker offers, every main when empty. Edited here, on the main — **as a row
    // of its own like the name's** (§15 D1000, the maintainer's look): the funnel
    // where the kind's glyph is, the field to the card's edge, and a line under it
    // saying what it does, where it was a *Filter* label in a column no other row
    // of this card has.
    if p.kind == PropKind::Swap {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
            ui.label(
                egui::RichText::new(icon::FUNNEL)
                    .font(theme::icon_font(14.0))
                    .color(theme::text::MUTED),
            );
            let w = ui.available_width();
            let id = ui.id().with(("prop-filter", owner, p.id));
            if let Some(f) = filter_field(ui, id, &p.filter, egui::vec2(w, ui::CONTROL_H)) {
                edits.push(defer(move |cx| {
                    Ok(variant::edit_property(cx.doc, owner, item, |q| {
                        Some(Property {
                            filter: f.clone(),
                            ..q.clone()
                        })
                    }))
                }));
            }
        });
        ui.label(
            egui::RichText::new("Filter displayed list by name")
                .size(11.0)
                .color(theme::text::DIM),
        );
        preferred_rows(ui, doc, owner, p, edits);
    }
}

/// A swap property's preferred mains, under its filter: one row each — the star,
/// the main's name, a × that drops it — then a picker that adds one. The swap
/// picker lists them first, in this order (`swap::preferred`).
fn preferred_rows(
    ui: &mut egui::Ui,
    doc: &ondin_core::Document,
    owner: NodeId,
    p: &Keyed<Property>,
    edits: &mut Vec<Deferred>,
) {
    let item = p.id;
    let write = |edits: &mut Vec<Deferred>, preferred: Vec<NodeId>| {
        edits.push(defer(move |cx| {
            Ok(variant::edit_property(cx.doc, owner, item, |q| {
                Some(Property {
                    preferred: preferred.clone(),
                    ..q.clone()
                })
            }))
        }));
    };
    for (i, m) in p.preferred.iter().enumerate() {
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
            ui.label(
                egui::RichText::new(icon::STAR)
                    .font(theme::icon_font(14.0))
                    .color(theme::text::MUTED),
            );
            // A main since deleted keeps its place, named as gone.
            let (name, ink) = match doc.get(*m).filter(|n| n.component()) {
                Some(n) => (n.name().to_string(), theme::text::STRONG),
                None => ("Deleted main".to_string(), theme::text::DIM),
            };
            let w = ui.available_width() - ui::CONTROL_H - ui::CARD_COL_GAP;
            ui.add_sized(
                egui::vec2(w, ui::CONTROL_H),
                egui::Label::new(egui::RichText::new(name).size(12.0).color(ink)).truncate(),
            );
            if ui::field_button(ui, icon::X, ui::CONTROL_H, 12.0, ui::FieldButton::Off)
                .on_hover_text("No longer preferred")
                .clicked()
            {
                let mut next = p.preferred.clone();
                next.remove(i);
                write(edits, next);
            }
        });
    }
    let Some(slot) = p.bound.first().copied() else {
        return;
    };
    let options = |search: &str| -> Vec<SwapOption> {
        swap::options(doc, slot, "", search)
            .into_iter()
            .filter(|m| !p.preferred.contains(m))
            .map(|m| {
                let name = doc.get(m).map(|n| n.name().to_string()).unwrap_or_default();
                (SwapTarget::Here(m), name, None)
            })
            .collect()
    };
    let w = ui.available_width();
    let (pick, _) = swap_picker(
        ui,
        ("prop-preferred", owner, p.id),
        w,
        "Add a preferred main…",
        None,
        doc,
        options,
        None,
    );
    if let Some(SwapTarget::Here(m)) = pick {
        let mut next = p.preferred.clone();
        next.push(m);
        write(edits, next);
    }
}

/// A hairline across the card between two groups of its rows (§15 D1000) — the
/// card's own row gap above and below it, so a rule adds one gap and a line.
fn card_rule(ui: &mut egui::Ui) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), egui::Sense::empty());
    let ppp = ui.ctx().pixels_per_point();
    let y = ui::hairline_in_column(rect.center().y, ppp, 1.0);
    ui.painter().line_segment(
        [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
        egui::Stroke::new(1.0, theme::color::text_a(26)),
    );
}

/// A new swap property on `owner` bound to the nested instance `slot`, named for
/// it and filtered by the main it shows — the binding line's *New property…*
/// for an *Instance* (§15 D983 (5)), from the Properties card's `+` menu (§15
/// D1000, D1005).
fn new_swap_property(
    doc: &ondin_core::Document,
    ids: &mut ondin_core::IdSource,
    owner: NodeId,
    slot: NodeId,
) -> Option<Transaction> {
    let n = doc.get(slot)?;
    let filter = n
        .link()
        .and_then(|m| doc.get(m))
        .map(|m| swap::suggested_filter(m.name()))
        .unwrap_or_default();
    let mut props = doc.get(owner)?.props().to_vec();
    let name = fresh_name(&props, n.name());
    props.push(Keyed::new(
        ids.mint_item(),
        Property {
            name,
            kind: PropKind::Swap,
            bound: vec![slot],
            filter,
            preferred: Vec::new(),
        },
    ));
    Some(Transaction(vec![ondin_core::Operation::SetProperties {
        id: owner,
        props,
    }]))
}

/// [`committed_field`] for a swap property's filter, trimmed like a name but
/// which may be cleared — an empty filter offers every main — and is hinted with
/// what it does when it is.
fn filter_field(
    ui: &mut egui::Ui,
    id: egui::Id,
    current: &str,
    size: egui::Vec2,
) -> Option<String> {
    // Trimmed like a name, but empty allowed: an empty filter offers every main.
    let rules = Rules {
        hint: "Every main",
        name: false,
        mixed: false,
    };
    let (resp, typed) = committed_field(ui, id, current, size, 12.0, rules);
    resp.on_hover_text("The picker offers the mains whose name starts with this");
    typed.map(|t| t.trim().to_string()).filter(|t| t != current)
}

/// A property's value as a row or a tooltip says it — a swap's by its main's name.
fn say(doc: &ondin_core::Document, v: &Option<PropValue>) -> String {
    match v {
        Some(PropValue::Boolean(true)) => "On".into(),
        Some(PropValue::Boolean(false)) => "Off".into(),
        Some(PropValue::Text(t)) => t.clone(),
        Some(PropValue::Swap(m)) => doc
            .get(*m)
            .map(|n| n.name().to_string())
            .unwrap_or_default(),
        None => String::new(),
    }
}

/// The main's own value of `p` — its default — read off the bound layer inside
/// `main`.
fn main_default(doc: &ondin_core::Document, main: NodeId, p: &Property) -> Option<PropValue> {
    let inside = ondin_core::build::subtree_nodes(doc, &[main]);
    p.bound
        .iter()
        .find(|b| inside.contains(b))
        .and_then(|b| variant::field_value(doc, *b, p.kind))
}

/// `base`, or `base 2`, `base 3`… — the first no property has.
fn fresh_name(props: &[Keyed<Property>], base: &str) -> String {
    let taken = |n: &str| props.iter().any(|p| p.name == n);
    if !taken(base) {
        return base.to_string();
    }
    (2..)
        .map(|k| format!("{base} {k}"))
        .find(|n| !taken(n))
        .expect("an unbounded range")
}

fn plural(n: usize, what: &str) -> String {
    match n {
        1 => format!("1 {what}"),
        n => format!("{n} {what}s"),
    }
}

/// A property row's chip over `roots` (§15 D1060; D1070's amendment for a Text
/// property), the *Mixed* rule as the fields keep it (§15 D1045): **every layer
/// the property views bound**, by `var_of`, or none — a binding on some and not
/// others draws D130's plain row. One variable, its name and the chip's tooltip
/// — *name · value · Mode*, or *name · Mixed* where modes give the layers
/// different values; several, *Mixed* with their names listed.
fn bound_chip(
    doc: &ondin_core::Document,
    roots: &[NodeId],
    p: &Property,
    mixed: bool,
    var_of: impl Fn(&ondin_core::Node) -> Option<ondin_core::VarId>,
) -> Option<(String, String)> {
    let cps: Vec<NodeId> = roots
        .iter()
        .flat_map(|r| variant::counterparts(doc, *r, p))
        .collect();
    let mut vars = Vec::new();
    for c in &cps {
        let v = var_of(doc.get(*c)?)?;
        if !vars.contains(&v) {
            vars.push(v);
        }
    }
    let table = doc.variables();
    let name = |v| table.get(v).map(|x| x.name.clone()).unwrap_or_default();
    Some(match vars.as_slice() {
        [] => return None,
        [var] if mixed => (name(*var), format!("{} · {}", name(*var), ui::MIXED_WORD)),
        [var] => {
            let at = cps[0];
            let mode = doc.mode_at(at).and_then(|m| table.mode(m));
            let tip = super::binding::chip_tip(&name(*var), doc.resolve_var(at, *var), mode);
            (name(*var), tip)
        }
        many => (
            ui::MIXED_WORD.to_owned(),
            many.iter().map(|v| name(*v)).collect::<Vec<_>>().join("\n"),
        ),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::IdSource;

    /// **A binding dropdown's decision**: picking a property of the kind moves
    /// the layer to it — off the other, never on both — *None* takes it off,
    /// and *New property…* makes one named for the layer, fresh among the
    /// others, a swap property with the suggested filter.
    ///
    /// Flips, run: never unbinding (`retain(|_| true)`) fails "off the first";
    /// the new property's name not made fresh fails "made fresh".
    #[test]
    fn a_binding_moves_the_layer_and_a_new_property_is_named_for_it() {
        let mut ids = IdSource::new(0xBB);
        let (x, other) = (ids.mint(), ids.mint());
        let (a, b, t) = (ids.mint_item(), ids.mint_item(), ids.mint_item());
        let prop = |name: &str, kind, bound: Vec<NodeId>| Property {
            name: name.into(),
            kind,
            bound,
            filter: String::new(),
            preferred: Vec::new(),
        };
        let props = vec![
            Keyed::new(a, prop("Show icon", PropKind::Boolean, vec![x])),
            Keyed::new(b, prop("Show Label", PropKind::Boolean, vec![other])),
            Keyed::new(t, prop("Text", PropKind::Text, vec![x])),
        ];
        let bound_to =
            |ps: &[Keyed<Property>], id| ps.iter().find(|p| p.id == id).unwrap().bound.clone();
        let next = rebind(&props, PropKind::Boolean, x, Bind::To(b), "Label", "");
        assert_eq!(bound_to(&next, a), Vec::<NodeId>::new(), "off the first");
        assert_eq!(bound_to(&next, b), vec![other, x], "onto the pick");
        assert_eq!(bound_to(&next, t), vec![x], "another kind untouched");

        let next = rebind(&props, PropKind::Boolean, x, Bind::None, "Label", "");
        assert!(bound_to(&next, a).is_empty() && bound_to(&next, b) == vec![other]);

        let fresh = ids.mint_item();
        let next = rebind(&props, PropKind::Boolean, x, Bind::New(fresh), "Label", "");
        let made = next.iter().find(|p| p.id == fresh).unwrap();
        assert_eq!(made.name, "Show Label 2", "made fresh");
        assert_eq!(made.bound, vec![x]);
        assert!(bound_to(&next, a).is_empty(), "and off the old one");

        let swap = ids.mint_item();
        let next = rebind(&props, PropKind::Swap, x, Bind::New(swap), "Icon", "Icons");
        let made = next.iter().find(|p| p.id == swap).unwrap();
        assert_eq!(
            (made.name.as_str(), made.filter.as_str()),
            ("Icon", "Icons")
        );
    }

    /// **The text fields' rules** (§15 D1003 (10)): a name trims and is never
    /// empty; content keeps its spaces and may be emptied; over a mixed
    /// selection the field starts empty and only typing commits. Flip, run: content trimmed (`name: true`) fails "spaces kept".
    #[test]
    fn a_field_commits_by_its_rules() {
        let content = Rules {
            hint: "",
            name: false,
            mixed: false,
        };
        assert_eq!(
            Rules::NAME.commits("  Large ", "Small"),
            Some("Large".into())
        );
        assert_eq!(
            Rules::NAME.commits("   ", "Small"),
            None,
            "a name is never empty"
        );
        assert_eq!(Rules::NAME.commits(" Small ", "Small"), None, "unchanged");
        assert_eq!(
            content.commits("Go  ", "Go"),
            Some("Go  ".into()),
            "spaces kept"
        );
        assert_eq!(content.commits("", "Go"), Some(String::new()), "emptied");
        assert_eq!(content.commits("Go", "Go"), None);
        let mixed = Rules {
            mixed: true,
            ..content
        };
        assert_eq!(mixed.commits("", ""), None, "untouched");
        assert_eq!(mixed.commits("OK", "Go"), Some("OK".into()));
    }

    /// **The swap picker groups as 4F has it** (§15 D1014): the document's own
    /// mains first, under *This document* only when a kit follows, then one group
    /// per kit alphabetically, each keeping the name order it was given. Without a
    /// kit the list is the flat one it always was, with no heading at all. A swap
    /// property's *Preferred* group leads.
    ///
    /// flip: `swap_groups` grouping `PREFERRED` as a kit → fails at "preferred
    /// first" (kits sort after the document's group).
    #[test]
    fn the_swap_picker_lists_own_mains_first_then_each_kit() {
        let mut ids = ondin_core::IdSource::new(0xBE);
        let [a, b, c, d, e, f] = [(); 6].map(|_| ids.mint());
        let kit = |id: &str, n: &str| {
            Some(KitHeading {
                kit: id.into(),
                name: n.into(),
                status: "v3".into(),
            })
        };
        // Two kits the library lost, both falling back to *Kit*: two groups,
        // not one — the grouping is by id (`arch-scribe`'s reading).
        let here = SwapTarget::Here;
        let found = vec![
            (here(a), "Icon / Arrow".to_string(), kit("k1", "Atlas")),
            (here(b), "Icon / Logo".to_string(), None),
            (here(c), "Icon / Quote".to_string(), kit("k2", "Marketing")),
            (here(d), "Icon / Check".to_string(), kit("k1", "Atlas")),
            (here(e), "Icon / Bell".to_string(), kit("k3", "Kit")),
            (here(f), "Icon / Bolt".to_string(), kit("k4", "Kit")),
        ];
        let id_of = |o: &SwapOption| match o.0 {
            SwapTarget::Here(id) => id,
            SwapTarget::Kit { main, .. } => main,
        };
        let groups = swap_groups(found);
        let shape: Vec<(Option<String>, Vec<NodeId>)> = groups
            .iter()
            .map(|(h, g)| {
                (
                    h.as_ref().map(|h| h.name.clone()),
                    g.iter().map(id_of).collect(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                (Some("This document".into()), vec![b]),
                (Some("Atlas".into()), vec![a, d]),
                (Some("Kit".into()), vec![e]),
                (Some("Kit".into()), vec![f]),
                (Some("Marketing".into()), vec![c]),
            ]
        );
        let alone = swap_groups(vec![(here(b), "Icon / Logo".into(), None)]);
        assert_eq!(alone.len(), 1);
        assert!(alone[0].0.is_none(), "no heading without a kit");

        // *Preferred* leads in the order given, and names the document's group.
        let star = kit(PREFERRED, "Preferred");
        let preferred = swap_groups(vec![
            (here(c), "Icon / Quote".into(), star.clone()),
            (here(a), "Icon / Arrow".into(), star),
            (here(b), "Icon / Logo".into(), None),
        ]);
        let shape: Vec<(Option<String>, Vec<NodeId>)> = preferred
            .iter()
            .map(|(h, g)| {
                (
                    h.as_ref().map(|h| h.name.clone()),
                    g.iter().map(id_of).collect(),
                )
            })
            .collect();
        assert_eq!(
            shape,
            vec![
                (Some("Preferred".into()), vec![c, a]),
                (Some("This document".into()), vec![b]),
            ],
            "preferred first and unsorted; the rest under a heading"
        );
    }

    /// **A swap to a kit main the document has never used** (§15 D1019), both
    /// doors: the nested copy's own *Swap* row and a swap property on its owner.
    /// Each pick vendors the kit's *Badge* and swaps to it in **one** undo step —
    /// the vault opens with the swap and closes with its undo — and the property's
    /// pick reaches the copy in an instance of the owner.
    ///
    /// Flip, run: `swap_to_kit` building the swap against `self.session.doc`
    /// rather than the vendored scratch fails *"the copy shows the kit's Badge"*:
    /// the main is not in the document yet, so `swap::swap` refuses and the verb
    /// says it cannot swap.
    #[test]
    fn a_swap_to_an_unused_kit_main_vendors_it_in_one_step() {
        use crate::kits::picker::tests::{app_on, file_kit, temp_root};
        let ctx = egui::Context::default();
        let dir = temp_root("swap-vendor");
        let (kit_id, _, badge) = file_kit(&dir);
        let mut app = app_on(&ctx, dir);
        let Slotted {
            glyph,
            row,
            slot,
            row_copy,
            nested,
        } = slotted(&mut app);

        let k = crate::kits::index::kits_in(&app.library, app.session.doc.meta().id.as_deref())
            .into_iter()
            .find(|k| k.id == kit_id)
            .expect("the kit is listed");
        app.kit_shelf.ask(&k);
        app.kit_shelf.settle();
        let published = app.kit_shelf.current(&k).expect("its publish loads");
        let shows_badge = |app: &OndinApp| {
            let doc = &app.session.doc;
            component::main_of(doc, nested).is_some_and(|m| {
                doc.in_vault(m)
                    && doc.get(m).and_then(|n| n.origin()).map(|o| o.node) == Some(badge)
            })
        };

        // The nested copy's own row.
        app.swap_to_kit(KitSwap {
            kit: k.clone(),
            published: published.clone(),
            main: badge,
            roots: vec![nested],
            prop: None,
        });
        assert!(shows_badge(&app), "the copy shows the kit's Badge");
        assert!(app.session.undo());
        assert!(
            app.session.doc.vault().is_none(),
            "one undo takes the vault"
        );
        assert_eq!(component::main_of(&app.session.doc, nested), Some(glyph));

        // A swap property on *Row*, bound to its slot, picked on the instance.
        let (tx, item) = variant::define(
            &app.session.doc,
            &mut app.session.ids,
            row,
            "Icon",
            PropKind::Swap,
            vec![slot],
        )
        .unwrap();
        assert!(app.session.commit(tx));
        let prop = app
            .session
            .doc
            .get(row)
            .unwrap()
            .props()
            .iter()
            .find(|p| p.id == item)
            .unwrap()
            .clone();
        app.swap_to_kit(KitSwap {
            kit: k,
            published,
            main: badge,
            roots: vec![row_copy],
            prop: Some(prop),
        });
        assert!(shows_badge(&app), "the property's pick reaches the copy");
        assert!(app.session.undo());
        assert!(app.session.doc.vault().is_none(), "one undo, again");
    }

    /// **A kit reached only through a swap reads as used** (§15 D1028 (4): *an
    /// instance on the page links or swaps to* it). The nested copy is swapped to
    /// the kit's *Badge*, which the document never used: its `link` still names
    /// the slot — *Glyph*, the document's own — and *Badge* is an empty frame, so
    /// no child's link reaches the vault either. Only `swap` names the vendored
    /// main, and since D1019 made a swap a door into a kit, `kits_on_the_page`'s
    /// swap arm is the one thing that keeps the picker from listing the kit under
    /// *In your library, not used here* while an instance shows its component.
    ///
    /// Flip, run: `kits_on_the_page` reading `n.link()` alone (the `n.swap()`
    /// dropped from its pair) fails *"used through a swap"*, `Some(false)`.
    #[test]
    fn a_kit_reached_only_through_a_swap_reads_as_used() {
        use crate::kits::picker::tests::{app_on, file_kit, kit_used, temp_root};
        let ctx = egui::Context::default();
        let dir = temp_root("swap-used");
        let (kit_id, _, badge) = file_kit(&dir);
        let mut app = app_on(&ctx, dir.clone());
        let Slotted { nested, .. } = slotted(&mut app);
        let k = crate::kits::index::kits_in(&app.library, app.session.doc.meta().id.as_deref())
            .into_iter()
            .find(|k| k.id == kit_id)
            .expect("the kit is listed");
        app.kit_shelf.ask(&k);
        app.kit_shelf.settle();
        let published = app.kit_shelf.current(&k).expect("its publish loads");
        assert_eq!(
            kit_used(&mut app, &ctx, &kit_id),
            Some(false),
            "the control: never used"
        );

        app.swap_to_kit(KitSwap {
            kit: k,
            published,
            main: badge,
            roots: vec![nested],
            prop: None,
        });
        let doc = &app.session.doc;
        let n = doc.get(nested).unwrap();
        assert!(
            n.link().is_some_and(|l| !doc.in_vault(l)),
            "the fixture: the link names the document's own slot"
        );
        assert!(
            n.swap().is_some_and(|s| doc.in_vault(s)),
            "the fixture: the swap names the vendored Badge"
        );
        assert!(n.children().is_empty(), "the fixture: no child to follow");
        assert_eq!(
            kit_used(&mut app, &ctx, &kit_id),
            Some(true),
            "used through a swap"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// One frame of the inspector, then the kit picker's pass — `OndinApp::ui`'s
    /// order, and the picker's pass is what lands the shelf's reads with the
    /// picker closed.
    fn drawn(
        app: &mut OndinApp,
        ctx: &egui::Context,
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Rect)> {
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1400.0, 2400.0),
            )),
            events,
            ..Default::default()
        };
        let out = ctx.run_ui(input, |ui| {
            ui.set_max_width(280.0);
            app.inspector_ui(ui);
            app.kit_picker_ui(ui.ctx());
        });
        let mut painted = Vec::new();
        fn walk(shape: &egui::Shape, out: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(t) => {
                    out.push((t.galley.text().to_string(), t.visual_bounding_rect()))
                }
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, out)),
                _ => {}
            }
        }
        for s in &out.shapes {
            walk(&s.shape, &mut painted);
        }
        painted
    }

    /// Press and release at `at`, the pointer moved there first; the painted
    /// text of the frame after.
    fn click_at(
        app: &mut OndinApp,
        ctx: &egui::Context,
        at: egui::Pos2,
    ) -> Vec<(String, egui::Rect)> {
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        drawn(app, ctx, vec![egui::Event::PointerMoved(at)]);
        drawn(app, ctx, vec![press(true)]);
        drawn(app, ctx, vec![press(false)]);
        drawn(app, ctx, vec![])
    }

    /// Type `text` into an open swap list's search field — found by its hint in
    /// `open`, the list's painted frame — and the frame after. The list shows
    /// what fits its scroll area, and the kit's group comes after the
    /// document's own, so a search is how its rows are reached, as a user's
    /// would be.
    fn search_for(
        app: &mut OndinApp,
        ctx: &egui::Context,
        open: &[(String, egui::Rect)],
        text: &str,
    ) -> Vec<(String, egui::Rect)> {
        let field = open
            .iter()
            .find(|(t, _)| t == "Search…")
            .unwrap_or_else(|| panic!("the list's search field: {open:?}"))
            .1
            .center();
        click_at(app, ctx, field);
        drawn(app, ctx, vec![egui::Event::Text(text.into())]);
        drawn(app, ctx, vec![]);
        drawn(app, ctx, vec![])
    }

    /// **Swap to a kit main through the Component card itself** (§15 D1019),
    /// not by calling `swap_list` and `swap_to_kit` underneath the rows. The
    /// inspector is drawn with the nested copy selected and the library's kit
    /// **never asked for** — no picker opened, no pill — and the *Swap* row's
    /// dropdown is clicked open: the open list asks the shelf, and once the read lands the
    /// kit's *Badge* is offered. Picked, the copy shows the vendored *Badge* in
    /// one undo step, and the kit is first in this document's last-use order.
    /// Then a swap property *Icon* on *Row*, picked on *Row*'s instance: its
    /// row's pick reaches the copy too.
    ///
    /// Flip, run: the `if swap_open` ask made dead (`false && swap_open`) fails
    /// *"offered once the open list has asked"* — nothing else asks in a fresh
    /// session, so the shelf has nothing to settle. Flip, run: the `kit_use`
    /// reorder after the commit deleted fails *"first in the last-use order"*,
    /// `None`. Flip, run: the property row's `Kit` arm writing `prop: None` —
    /// the nested row's swap, made on *Row*'s instance — fails *"the property's
    /// pick reaches the copy"*, the status reading *"That component cannot be
    /// swapped in here"*: the instance is no nested copy to take a swap.
    #[test]
    fn the_cards_swap_rows_offer_and_swap_to_an_unasked_kit() {
        use crate::kits::picker::tests::{app_on, file_kit, temp_root};
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let dir = temp_root("swap-drawn");
        let (kit_id, _, badge) = file_kit(&dir);
        let mut app = app_on(&ctx, dir.clone());
        let Slotted {
            glyph,
            row,
            slot,
            row_copy,
            nested,
        } = slotted(&mut app);
        let shows_badge = |app: &OndinApp| {
            let doc = &app.session.doc;
            component::main_of(doc, nested).is_some_and(|m| {
                doc.in_vault(m)
                    && doc.get(m).and_then(|n| n.origin()).map(|o| o.node) == Some(badge)
            })
        };
        // The dropdown beside a row's label: the shown main's name on its line.
        let combo = |painted: &[(String, egui::Rect)], label: &str| {
            let at = painted
                .iter()
                .find(|(t, _)| t == label)
                .unwrap_or_else(|| panic!("the {label} row: {painted:?}"))
                .1;
            painted
                .iter()
                .find(|(t, r)| {
                    t == "Glyph"
                        && (r.center().y - at.center().y).abs() < 4.0
                        && r.left() > at.right()
                })
                .map(|(_, r)| r.center())
                .unwrap_or_else(|| panic!("the {label} row's dropdown: {painted:?}"))
        };
        let offer = |painted: &[(String, egui::Rect)]| {
            painted
                .iter()
                .find(|(t, _)| t == "Badge")
                .map(|(_, r)| r.center())
        };

        app.session.selection.set_one(nested);
        let mut painted = Vec::new();
        for _ in 0..4 {
            painted = drawn(&mut app, &ctx, vec![]);
        }
        assert!(
            app.kit_shelf.load(&kit_id).is_none(),
            "the fixture: nothing has asked for the kit"
        );
        let at = combo(&painted, "Swap");
        let open = click_at(&mut app, &ctx, at);
        assert!(
            open.iter().any(|(t, _)| t == "Search…"),
            "the fixture: the list is open: {open:?}"
        );
        app.kit_shelf.settle();
        drawn(&mut app, &ctx, vec![]);
        let painted = search_for(&mut app, &ctx, &open, "Bad");
        let pick = offer(&painted)
            .unwrap_or_else(|| panic!("offered once the open list has asked: {painted:?}"));
        click_at(&mut app, &ctx, pick);
        assert!(
            shows_badge(&app),
            "the copy shows the kit's Badge: {}",
            app.session.status().text
        );
        let doc_id = app.session.doc.meta().id.clone().unwrap();
        assert_eq!(
            app.library.local.kit_use.get(&doc_id),
            Some(&vec![kit_id.clone()]),
            "first in the last-use order"
        );
        assert!(app.session.undo());
        assert!(app.session.doc.vault().is_none(), "one undo step");
        assert_eq!(component::main_of(&app.session.doc, nested), Some(glyph));

        // A swap property on *Row*, bound to its slot, picked on the instance.
        let (tx, _) = variant::define(
            &app.session.doc,
            &mut app.session.ids,
            row,
            "Icon",
            PropKind::Swap,
            vec![slot],
        )
        .unwrap();
        assert!(app.session.commit(tx));
        app.session.selection.set_one(row_copy);
        let mut painted = Vec::new();
        for _ in 0..4 {
            painted = drawn(&mut app, &ctx, vec![]);
        }
        let at = combo(&painted, "Icon");
        let open = click_at(&mut app, &ctx, at);
        let painted = search_for(&mut app, &ctx, &open, "Bad");
        let pick = offer(&painted).unwrap_or_else(|| panic!("the kit offered: {painted:?}"));
        click_at(&mut app, &ctx, pick);
        assert!(
            shows_badge(&app),
            "the property's pick reaches the copy: {}",
            app.session.status().text
        );
        assert!(app.session.undo());
        assert!(app.session.doc.vault().is_none(), "one undo step, again");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// **A swap to a variant the held set lacks is refused, with a status**
    /// (§15 D1019 names `NeedsUpdate`; `the_swap_list_offers_…` builds a waiting
    /// update and never picks under it). The page holds *Atlas*' set *Button* at v4; v5 adds a
    /// variant *Huge*. *Huge* is offered — the shelf reads v5 — and picked, the
    /// set would have to move to v5 first: nothing is committed, and the status
    /// says the update comes first.
    ///
    /// Flip, run: the `NeedsUpdate` arm's `session.fail` dropped (`drop(..)`)
    /// fails *"says the update comes first"*, the status empty.
    #[test]
    fn a_swap_to_a_variant_the_held_set_lacks_waits_for_the_update() {
        use crate::kits::picker::tests::{app_on, file_kit, temp_root};
        use ondin_core::variant::{VariantProp, VariantSet};
        let ctx = egui::Context::default();
        let dir = temp_root("swap-needs-update");
        let (kit_id, set, _) = file_kit(&dir);
        let path = dir.join("atlas.ondin");
        let mut app = app_on(&ctx, dir.clone());
        let Slotted { nested, .. } = slotted(&mut app);
        let v4 = ondin_core::io::load(&std::fs::read(&path).unwrap()).unwrap();
        let page = app.session.doc.first_page();
        let (tx, _) = ondin_core::kit::insert(
            &app.session.doc,
            &mut app.session.ids,
            &kit_id,
            4,
            v4.published().unwrap(),
            set,
            page,
        )
        .unwrap();
        assert!(app.session.commit(tx));

        // v5: a third variant, *Huge*.
        let mut kit = v4.clone();
        let huge = IdSource::new(0xAF).mint();
        kit.apply(&Transaction(vec![
            ondin_core::Operation::CreateNode {
                id: huge,
                parent: set,
                index: 2,
                kind: ondin_core::NodeKind::Artboard {
                    size: ondin_core::kurbo::Size::new(160.0, 30.0),
                },
                transform: None,
                name: Some("Huge".into()),
            },
            ondin_core::Operation::SetComponent {
                id: huge,
                component: true,
            },
            ondin_core::Operation::SetVariantSet {
                id: set,
                set: Some(VariantSet {
                    props: vec![VariantProp {
                        name: "Size".into(),
                        values: vec!["Small".into(), "Primary".into(), "Huge".into()],
                    }],
                }),
            },
            ondin_core::Operation::SetVariant {
                id: huge,
                values: vec!["Huge".into()],
            },
        ]))
        .unwrap();
        let mut meta = kit.meta().clone();
        meta.kit.as_mut().unwrap().version = 5;
        kit.set_meta(meta);
        kit.set_published(ondin_core::kit::freeze(&kit));
        crate::library::store::write_document(&path, &kit).unwrap();
        let f = std::fs::File::options().write(true).open(&path).unwrap();
        f.set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2))
            .unwrap();
        app.library.refresh();
        let k = crate::kits::index::kits_in(&app.library, app.session.doc.meta().id.as_deref())
            .into_iter()
            .find(|k| k.id == kit_id)
            .unwrap();
        assert_eq!(k.version, 5, "the fixture: the library reads v5");
        app.kit_shelf.ask(&k);
        app.kit_shelf.settle();
        let published = app.kit_shelf.current(&k).expect("v5 loads");
        let shelves = [(k.clone(), published.clone())];
        let offered = swap_list(
            &app.session.doc,
            std::slice::from_ref(&k),
            &shelves,
            nested,
            "",
            &[],
            "",
        );
        assert!(
            offered
                .iter()
                .any(|(t, _, _)| matches!(t, SwapTarget::Kit { main, .. } if *main == huge)),
            "the fixture: Huge is offered"
        );

        let before = app.session.doc.clone();
        app.swap_to_kit(KitSwap {
            kit: k,
            published,
            main: huge,
            roots: vec![nested],
            prop: None,
        });
        assert_eq!(app.session.doc, before, "nothing committed");
        assert_eq!(
            app.session.status().text,
            "This kit has an update to accept before that component can be swapped in",
            "says the update comes first"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// The swap fixture: *Row* nests an instance of *Glyph* (`slot`), and an
    /// instance of *Row* (`row_copy`) sits at the top, its copy of the slot
    /// (`nested`) the one that takes a swap.
    struct Slotted {
        glyph: NodeId,
        row: NodeId,
        slot: NodeId,
        row_copy: NodeId,
        nested: NodeId,
    }

    /// **A top-level instance changes main from the card**: *Row*'s instance gets a
    /// *Main* row (and no *Swap* row), its dropdown lists the frame mains, and the
    /// pick relinks it to *Glyph* in one undo step.
    /// flip: the *Main* row's guard as `swap::can_swap` → fails at "the Main row".
    #[test]
    fn a_top_level_instance_changes_main_from_its_card() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let Slotted {
            glyph,
            row,
            row_copy,
            ..
        } = slotted(&mut app);
        app.session.selection.set_one(row_copy);
        let mut painted = Vec::new();
        for _ in 0..4 {
            painted = drawn(&mut app, &ctx, vec![]);
        }
        let label = painted
            .iter()
            .find(|(t, _)| t == "Main")
            .unwrap_or_else(|| panic!("the Main row: {painted:?}"))
            .1;
        assert!(
            !painted.iter().any(|(t, _)| t == "Swap"),
            "a root does not swap"
        );
        let combo = painted
            .iter()
            .find(|(t, r)| {
                t == "Row"
                    && (r.center().y - label.center().y).abs() < 4.0
                    && r.left() > label.right()
            })
            .map(|(_, r)| r.center())
            .unwrap_or_else(|| panic!("the Main row's dropdown, reading Row: {painted:?}"));
        let open = click_at(&mut app, &ctx, combo);
        let offer = open
            .iter()
            .filter(|(t, r)| t == "Glyph" && r.top() > label.bottom())
            .map(|(_, r)| r.center())
            .next()
            .unwrap_or_else(|| panic!("Glyph offered: {open:?}"));
        let depth = app.session.history.undo_depth();
        click_at(&mut app, &ctx, offer);
        assert_eq!(app.session.doc.get(row_copy).unwrap().link(), Some(glyph));
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one undo step");
        assert!(app.session.undo());
        assert_eq!(app.session.doc.get(row_copy).unwrap().link(), Some(row));
    }

    fn slotted(app: &mut OndinApp) -> Slotted {
        use ondin_core::{Operation, build};
        let root = app.session.doc.first_page();
        let [glyph, row] = [(); 2].map(|_| app.session.ids.mint());
        let frame = |id, index, name: &str| Operation::CreateNode {
            id,
            parent: root,
            index,
            kind: ondin_core::NodeKind::Artboard {
                size: ondin_core::kurbo::Size::new(24.0, 24.0),
            },
            transform: None,
            name: Some(name.into()),
        };
        assert!(app.session.commit(Transaction(vec![
            frame(glyph, 0, "Glyph"),
            Operation::SetComponent {
                id: glyph,
                component: true
            },
            frame(row, 1, "Row"),
        ])));
        let copy_into = |app: &mut OndinApp, main: NodeId, parent: NodeId| {
            let nodes = app.session.doc.capture_subtree(main).unwrap();
            let (tx, made) = build::insert_subtrees(
                &app.session.doc,
                &mut app.session.ids,
                &[build::Placement {
                    nodes,
                    parent,
                    index: None,
                }],
                Default::default(),
            );
            assert!(app.session.commit(tx));
            made[0]
        };
        let slot = copy_into(app, glyph, row);
        assert!(
            app.session
                .commit(Transaction(vec![Operation::SetComponent {
                    id: row,
                    component: true
                }]))
        );
        let row_copy = copy_into(app, row, root);
        let nested = app.session.doc.get(row_copy).unwrap().children()[0];
        assert!(
            swap::can_swap(&app.session.doc, nested),
            "the fixture's slot"
        );
        Slotted {
            glyph,
            row,
            slot,
            row_copy,
            nested,
        }
    }

    /// **The swap list offers a kit's unused mains beside what the document
    /// holds, under one heading per kit** (§15 D1019): with nothing vendored,
    /// *Glyph* is the document's own and the kit's three frame mains are offered
    /// at the library's version. Once *Badge* is vendored at v4 and the library
    /// reads v5 — an update waiting — *Badge* is listed as the held copy, the
    /// other two still as offers, and **every row of the kit reads v4**, the
    /// version held.
    ///
    /// Flips, run: `swap_list` leaving out the shelves' offers fails *"offered
    /// before any use"*; the first cut's headings — an offer reading the
    /// library's version, nothing evening them out — fail *"one heading per
    /// kit"*, *Large* and *Small* at v5 beside *Badge* at v4.
    #[test]
    fn the_swap_list_offers_unused_kit_mains_under_one_heading() {
        use crate::kits::picker::tests::{app_on, file_kit, temp_root};
        let ctx = egui::Context::default();
        let dir = temp_root("swap-list");
        let (kit_id, _, badge) = file_kit(&dir);
        let mut app = app_on(&ctx, dir);
        let Slotted { glyph, nested, .. } = slotted(&mut app);
        let k = crate::kits::index::kits_in(&app.library, app.session.doc.meta().id.as_deref())
            .into_iter()
            .find(|k| k.id == kit_id)
            .unwrap();
        app.kit_shelf.ask(&k);
        app.kit_shelf.settle();
        let p = app.kit_shelf.current(&k).unwrap();
        let shape = |list: &[SwapOption]| -> Vec<(String, bool, Option<String>)> {
            list.iter()
                .map(|(t, name, h)| {
                    (
                        name.clone(),
                        matches!(t, SwapTarget::Kit { .. }),
                        h.as_ref().map(|h| h.status.clone()),
                    )
                })
                .collect()
        };

        let row = |name: &str, kit: bool, status: Option<&str>| {
            (name.to_string(), kit, status.map(str::to_string))
        };
        let shelves = [(k.clone(), p.clone())];
        let list = swap_list(
            &app.session.doc,
            std::slice::from_ref(&k),
            &shelves,
            nested,
            "",
            &[],
            "",
        );
        assert_eq!(
            shape(&list),
            vec![
                row("Badge", true, Some("v4")),
                row("Glyph", false, None),
                row("Large", true, Some("v4")),
                row("Small", true, Some("v4")),
            ],
            "offered before any use, never the Row the slot's instance shows"
        );
        assert!(matches!(list[1].0, SwapTarget::Here(m) if m == glyph));

        // *Badge* vendored at v4; the library now says v5.
        let root = app.session.doc.first_page();
        let (tx, _) = ondin_core::kit::insert(
            &app.session.doc,
            &mut app.session.ids,
            &kit_id,
            4,
            &p,
            badge,
            root,
        )
        .unwrap();
        assert!(app.session.commit(tx));
        let newer = crate::kits::index::KitEntry {
            version: 5,
            ..k.clone()
        };
        let shelves = [(newer.clone(), p.clone())];
        let list = swap_list(&app.session.doc, &[newer], &shelves, nested, "", &[], "");
        assert_eq!(
            shape(&list),
            vec![
                row("Badge", false, Some("v4")),
                row("Glyph", false, None),
                row("Large", true, Some("v4")),
                row("Small", true, Some("v4")),
            ],
            "one heading per kit"
        );
    }
}
