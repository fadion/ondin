//! The floating assets browser: open-source icons and stock photos, beside the
//! toolbar in the insert picker's frame.
//!
//! One of three panels in that place — the insert picker, the styles browser and
//! this — and opening one closes the others. Two tabs on a segmented control,
//! then a search box. A click places at the view's centre and selects; a drag
//! drops at the pointer. With shapes or frames selected, a photo's click fills
//! them instead, and Alt-click places. Every insert is one undo step and carries
//! its credit (`ondin_core::credit`).

use super::photos::{Orientation, Page, Photo, PhotoError, PhotoSize, SERVICES, ServiceSpec};
use super::sets::{self, IconSet, SETS, SetSpec};
use super::{PhotoIntent, SetState};
use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use crate::ui;
use ondin_core::kurbo::Point;
use ondin_core::{NodeId, NodeKind, Operation, Transaction};
use std::sync::Arc;

/// The panel: the insert picker's width, padding and height.
const W: f32 = 340.0;
const PAD: f32 = ui::RAIL_POPUP_PAD;
/// Between the panel's rows of controls.
const ROW_GAP: f32 = 8.0;
/// The icon grid: six to a row, 44 tall, 6 apart.
const COLS: usize = 6;
const TILE_H: f32 = 44.0;
const TILE_GAP: f32 = 6.0;
/// An icon's drawn size in its tile.
const ICON_PT: f32 = 22.0;
/// How many icons show before scrolling to the end asks for more.
const PAGE: usize = 120;
/// The photo columns' gap.
const PHOTO_GAP: f32 = 6.0;
/// A hovered photo's photographer: in from the tile's left and up from its foot
/// to the baseline.
const AUTHOR_INSET: f32 = 8.0;
/// The fill strip's height.
const STRIP_H: f32 = 24.0;
/// The foot caption's row, with the hairline above it.
const FOOT_H: f32 = 22.0;
/// The variant and orientation dropdowns and the size field. A dropdown grows to
/// fit its words, and the panel with it: the orientation's holds *Any
/// orientation* whole, and the variant's cuts a long one (*Outlined rounded*).
const VARIANT_W: f32 = 92.0;
const ORIENT_W: f32 = 124.0;
const SIZE_W: f32 = 58.0;
/// How long typing rests before a photo search goes out.
const TYPE_PAUSE: f64 = 0.4;
/// The search field's id, read to know whether it holds the keyboard.
const SEARCH_ID: &str = "assets-search";
/// The air above an empty state: the styles and variables browser's.
const EMPTY_AIR: f32 = 40.0;
/// What an icon's `currentColor` becomes: the ink a new text layer draws in,
/// which has no fill of its own and reads as black.
const ICON_INK: &str = "#000000";

/// One icon of the results: its set and where it sits there.
#[derive(Clone)]
struct Hit {
    spec: &'static SetSpec,
    set: Arc<IconSet>,
    index: usize,
}

impl Hit {
    fn icon(&self) -> &sets::Icon {
        &self.set.icons[self.index]
    }

    fn key(&self) -> String {
        let i = self.icon();
        format!("{}/{}/{}", self.spec.key, i.name, i.variant)
    }
}

/// What the results were worked out for.
#[derive(Clone, PartialEq)]
struct HitsKey {
    query: String,
    sets: Vec<&'static str>,
    variant: Option<usize>,
}

/// What is under the pointer or carried.
#[derive(Clone)]
enum Item {
    Icon(Hit),
    Photo(Box<Photo>),
}

impl Item {
    fn key(&self) -> String {
        match self {
            Item::Icon(h) => h.key(),
            Item::Photo(p) => format!("{}/{}", p.service, p.id),
        }
    }
}

/// What a frame of the panel asks for.
enum Act {
    Tab(bool),
    Settings,
    RetrySets,
    RetryPhotos,
    Place(Item, Option<egui::Pos2>),
    Fill(Box<Photo>),
}

/// What a frame of the panel reports, and the providers its tiles draw from.
struct Out<'a> {
    act: Option<Act>,
    /// The tile under the pointer and its rect: the hover card's subject.
    hovered: Option<(Item, egui::Rect)>,
    /// The tile being carried.
    dragging: Option<Item>,
    /// A right-click took the drag away: until the button is up its release
    /// places nothing.
    cancelled: bool,
    /// A photo is being fetched for an insert.
    fetching: bool,
    assets: &'a mut super::Assets,
    /// The panel's choices as this frame left them, saved when they changed.
    prefs: crate::prefs::AssetPrefs,
    /// A photo search to send after the frame.
    search: Option<(String, &'static ServiceSpec, Orientation)>,
}

/// The panel's own state while it is open.
#[derive(Default)]
pub(crate) struct Panel {
    query: String,
    /// Whether the search field takes the keyboard on the next frame.
    focus: bool,
    /// How many icons the grid shows.
    shown: usize,
    hits: Option<(HitsKey, Vec<Hit>)>,
    /// The photo search on show: query, service, orientation.
    searched: Option<(String, &'static str, Orientation)>,
    /// When the query last changed (`ctx` time), for the pause before a search.
    typed_at: Option<f64>,
    orientation: Orientation,
    /// The last page of photos asked for.
    page: u32,
    resting: ui::Resting<String>,
    rect: Option<egui::Rect>,
    dragging: Option<Item>,
}

impl Panel {
    /// Whether a tile is being carried out of the panel.
    pub(crate) fn dragging(&self) -> bool {
        self.dragging.is_some()
    }

    /// Drop the carried tile, as a right-click does to every drag.
    pub(crate) fn cancel_drag(&mut self) {
        self.dragging = None;
    }
}

/// The sets switched on, in Settings' order.
fn sets_on(on: &[String]) -> Vec<&'static SetSpec> {
    SETS.iter()
        .filter(|s| on.iter().any(|k| k == s.key))
        .collect()
}

/// The services switched on, in Settings' order.
fn services_on(on: &[String]) -> Vec<&'static ServiceSpec> {
    SERVICES
        .iter()
        .filter(|s| on.iter().any(|k| k == s.key))
        .collect()
}

/// The icons of `sets` answering `query` in `variant` (each set's default under
/// *All sets*), best first: by rank, then name, then set order.
fn find_icons(
    query: &str,
    sets: &[(&'static SetSpec, Arc<IconSet>)],
    variant: Option<usize>,
) -> Vec<Hit> {
    let query = query.trim().to_lowercase();
    let words: Vec<&str> = query.split([' ', '-']).filter(|w| !w.is_empty()).collect();
    let mut ranked: Vec<(u8, usize, Hit)> = Vec::new();
    for (order, (spec, set)) in sets.iter().enumerate() {
        let v = variant.unwrap_or(0);
        for (index, icon) in set.icons.iter().enumerate() {
            if icon.variant != v {
                continue;
            }
            let rank = match words.is_empty() {
                true => Some(0),
                false => sets::rank(icon, &query, &words),
            };
            if let Some(r) = rank {
                ranked.push((
                    r,
                    order,
                    Hit {
                        spec,
                        set: Arc::clone(set),
                        index,
                    },
                ));
            }
        }
    }
    ranked.sort_by(|a, b| {
        a.0.cmp(&b.0)
            .then_with(|| a.2.icon().name.cmp(&b.2.icon().name))
            .then(a.1.cmp(&b.1))
    });
    ranked.into_iter().map(|(_, _, h)| h).collect()
}

/// *Phosphor, Lucide or Font Awesome Free*.
fn or_list(names: &[&str]) -> String {
    match names {
        [] => String::new(),
        [one] => (*one).to_string(),
        [rest @ .., last] => format!("{} or {last}", rest.join(", ")),
    }
}

/// A count with thousands marked, *2,184*.
fn grouped(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

/// *1 icon*, *2,184 photos*.
fn counted(n: u64, what: &str) -> String {
    match n {
        1 => format!("1 {what}"),
        n => format!("{} {what}s", grouped(n)),
    }
}

/// Today as the credit records it, `YYYY-MM-DD` (UTC).
pub(crate) fn today() -> String {
    let (y, m, d) = crate::library::clock::civil_from_unix(crate::library::clock::now());
    format!("{y:04}-{m:02}-{d:02}")
}

/// An icon's credit: the set's licence and author, and for a set whose licence
/// asks for it (CC BY), the attribution the Credits card lists.
pub(crate) fn icon_credit(spec: &SetSpec, today: &str) -> ondin_core::Credit {
    let author = match spec.key {
        "font-awesome" => "Font Awesome".to_string(),
        _ => spec.author.to_string(),
    };
    ondin_core::Credit {
        kind: ondin_core::CreditKind::Icon,
        text: format!("Icons by {author}, {}", spec.licence),
        author,
        author_url: String::new(),
        provider: spec.name.to_string(),
        provider_url: spec.home.to_string(),
        source_url: String::new(),
        licence: spec.licence.to_string(),
        licence_url: spec.licence_url.to_string(),
        required: spec.credit_required,
        inserted: today.to_string(),
    }
}

/// What the error says, as heading and line.
fn photo_error_words(err: &PhotoError, service: &str) -> (String, String) {
    match err {
        PhotoError::NoKey => (
            format!("{service} needs a key"),
            "Add one in Settings, or switch on Openverse, which needs none.".into(),
        ),
        PhotoError::Refused => (
            format!("{service} refused the key"),
            "Check it in Settings.".into(),
        ),
        PhotoError::Limited => (
            format!("{service}'s hourly limit is reached"),
            "Try again in an hour.".into(),
        ),
        PhotoError::Unreachable => (
            format!("Can't reach {service}"),
            "Check the connection. Icon sets already downloaded still work.".into(),
        ),
    }
}

/// Whether a layer can take a photo as its fill: a closed shape or a frame,
/// unlocked, and not part of an inserted icon, on itself or an ancestor — a layer
/// holds one credit, and the photo's would take the place of the icon's (a CC BY
/// set's attribution) once the icon is ungrouped.
fn fillable(doc: &ondin_core::Document, id: NodeId) -> bool {
    let in_icon =
        std::iter::successors(doc.get(id), |n| n.parent().and_then(|p| doc.get(p))).any(|n| {
            n.credit()
                .is_some_and(|c| c.kind == ondin_core::CreditKind::Icon)
        });
    doc.get(id).is_some_and(|n| {
        n.kind().takes_paint()
            && !matches!(
                n.kind(),
                NodeKind::Text { .. }
                    | NodeKind::Line { .. }
                    | NodeKind::Spiral { .. }
                    | NodeKind::Arc { .. }
            )
    }) && !in_icon
        && !ondin_core::is_effectively_locked(doc, id)
}

impl OndinApp {
    /// Open the panel with its search taking the keyboard, or close it. Closes the
    /// insert picker and the styles browser, which share its place.
    pub(crate) fn toggle_assets(&mut self) {
        self.asset_panel = match self.asset_panel {
            Some(_) => None,
            None => {
                self.kit_picker = None;
                self.var_browser = None;
                // Whether a keyed service has its key, for the panel's empty state.
                let ctx = self.egui_ctx.clone();
                let on = self.prefs.assets.photo_services.clone();
                self.read_asset_keys(&ctx, &on);
                Some(Panel {
                    focus: true,
                    shown: PAGE,
                    ..Panel::default()
                })
            }
        };
    }

    /// The layers a photo's click fills: the selection, when every layer in it
    /// can take one.
    fn photo_targets(&self) -> Vec<NodeId> {
        let ids = self.session.selection.ids();
        match !ids.is_empty() && ids.iter().all(|id| fillable(&self.session.doc, *id)) {
            true => ids.to_vec(),
            false => Vec::new(),
        }
    }

    /// The providers' frame work, whether or not the panel shows (focus mode
    /// hides it): what the workers handed back, a fetched photo's landing, the
    /// keychain's answer, and the panel's choices written once no button is down
    /// (a scrubbed size field changes them every frame).
    pub(crate) fn assets_pass(&mut self, ctx: &egui::Context) {
        self.assets.pass(ctx);
        self.land_asset_photos();
        self.poll_asset_keys();
        if self.assets.prefs_dirty && !ctx.input(|i| i.pointer.any_down()) {
            self.assets.prefs_dirty = false;
            self.prefs.save();
        }
    }

    /// Draw the panel, when it is open, and do what it asked.
    pub(crate) fn assets_ui(&mut self, ctx: &egui::Context) {
        if self.asset_panel.is_none() {
            return;
        }
        let prefs = self.prefs.assets.clone();
        let on_sets = sets_on(&prefs.icon_sets);
        let on_services = services_on(&prefs.photo_services);
        let photos_tab = prefs.photos_tab;
        let targets = self.photo_targets();
        let fill_names: Vec<String> = targets
            .iter()
            .map(|id| crate::private::layer(&self.session.doc, *id).into_owned())
            .collect();
        let keys = self.asset_keys.clone();
        let now = ctx.input(|i| i.time);

        let anchor = self.rail_popup_anchor(self.asset_button);
        let height = crate::kits::picker::panel_height(anchor.y, self.canvas_rect.top());

        // The chosen set, its variant and the size.
        let set = on_sets.iter().copied().find(|s| s.key == prefs.set);
        let shown_sets: Vec<&'static SetSpec> = match set {
            Some(s) => vec![s],
            None => on_sets.clone(),
        };
        let variant = set.map(|s| {
            prefs
                .variants
                .get(s.key)
                .and_then(|label| s.variants.iter().position(|v| v.label == label))
                .unwrap_or(0)
        });
        let mut ready: Vec<(&'static SetSpec, Arc<IconSet>)> = Vec::new();
        let mut loading: Vec<(&'static SetSpec, u64, Option<u64>)> = Vec::new();
        let mut failed: Vec<&'static SetSpec> = Vec::new();
        if !photos_tab {
            for spec in &shown_sets {
                match self.assets.set(spec) {
                    SetState::Ready(s) => ready.push((spec, Arc::clone(s))),
                    SetState::Loading { done, total } => loading.push((spec, *done, *total)),
                    SetState::Failed => failed.push(spec),
                }
            }
        }
        let grid = match set.and_then(|s| ready.iter().find(|(r, _)| r.key == s.key)) {
            Some((_, s)) => s.grid_of(variant.unwrap_or(0)),
            None => 24.0,
        };
        let size = prefs.icon_size.unwrap_or(grid);

        let service = on_services
            .iter()
            .copied()
            .find(|s| s.key == prefs.service)
            .or_else(|| on_services.first().copied());
        let key_missing = service.is_some_and(|s| {
            s.key_label.is_some() && keys.as_ref().is_some_and(|k| !k.contains_key(s.key))
        });

        let mut out = Out {
            act: None,
            hovered: None,
            dragging: None,
            cancelled: self.gesture_cancelled,
            fetching: self.assets.fetch.is_some(),
            assets: &mut self.assets,
            prefs: prefs.clone(),
            search: None,
        };

        let panel = self.asset_panel.as_mut().expect("checked above");
        // The icon results, worked out again only when what they answer changed.
        let hits_key = HitsKey {
            query: panel.query.trim().to_lowercase(),
            sets: ready.iter().map(|(s, _)| s.key).collect(),
            variant,
        };
        if panel.hits.as_ref().is_none_or(|(k, _)| *k != hits_key) && !photos_tab {
            let hits = find_icons(&hits_key.query, &ready, variant);
            if panel
                .hits
                .as_ref()
                .is_some_and(|(k, _)| k.query != hits_key.query)
            {
                panel.shown = PAGE;
            }
            panel.hits = Some((hits_key, hits));
        }
        // A photo search goes out once typing rests, or at once on Enter.
        let enter = ctx.memory(|m| m.has_focus(egui::Id::new(SEARCH_ID)))
            && ctx.input(|i| i.key_pressed(egui::Key::Enter));
        if photos_tab && let Some(svc) = service {
            let want = (panel.query.trim().to_string(), svc.key, panel.orientation);
            let rested = panel.typed_at.is_none_or(|t| now - t >= TYPE_PAUSE);
            if !want.0.is_empty() && panel.searched.as_ref() != Some(&want) && (rested || enter) {
                panel.searched = Some(want.clone());
                panel.page = 1;
                out.search = Some((want.0, svc, want.2));
            } else if let Some(t) = panel.typed_at.filter(|_| !rested) {
                ctx.request_repaint_after(std::time::Duration::from_secs_f64(
                    TYPE_PAUSE - (now - t),
                ));
            }
        }

        let area = ui::rail_popup(ctx, "assets-panel", anchor, height, |ui| {
            let inner = W - 2.0 * PAD;
            ui.set_width(inner);
            ui.set_height(height - 2.0 * PAD - 2.0);
            ui.spacing_mut().item_spacing.y = 0.0;
            let tab = usize::from(photos_tab);
            let picked = ui::segmented(ui, inner, ui::SEGMENT_CELL_H, 2, tab, |p, i, r, on| {
                ui::segment_label(p, r, ["Icons", "Photos"][i], on);
            });
            if let Some(i) = picked.filter(|i| *i != tab) {
                out.act = Some(Act::Tab(i == 1));
            }
            ui.add_space(ROW_GAP);
            let bottom = ui.max_rect().bottom();
            match photos_tab {
                false => {
                    let view = IconsView {
                        on: &on_sets,
                        set,
                        variant,
                        size,
                        ready: &ready,
                        loading: &loading,
                        failed: &failed,
                    };
                    icons_body(ui, panel, &view, bottom, now, &mut out);
                }
                true => {
                    let view = PhotosView {
                        on: &on_services,
                        service,
                        key_missing,
                        fill_names: &fill_names,
                    };
                    photos_body(ui, panel, &view, bottom, now, &mut out);
                }
            }
        });
        let rect = area.rect;
        let Out {
            act,
            hovered,
            dragging,
            prefs: new_prefs,
            search,
            ..
        } = out;

        // Searches, after the frame: a later page joins the shown one.
        if let Some((q, svc, o)) = search {
            let page = self.asset_panel.as_ref().map_or(1, |p| p.page.max(1));
            self.assets
                .search_photos(svc, &q, page, o, self.asset_keys.as_ref());
        }
        if new_prefs != self.prefs.assets {
            self.prefs.assets = new_prefs;
            self.assets.prefs_dirty = true;
        }

        // The carried tile follows the pointer; over the canvas the drop adds.
        let canvas = self.canvas_rect;
        if let Some(p) = self.asset_panel.as_mut() {
            p.rect = Some(rect);
            p.dragging = dragging.clone();
            let resting = hovered.as_ref().filter(|_| dragging.is_none());
            if p.resting.rested(ctx, resting.map(|(item, _)| item.key()))
                && let Some((item, at)) = resting
            {
                let size_px = PhotoSize::from_key(&self.prefs.assets.photo_size);
                hover_card(ctx, item, rect, *at, size, size_px, !targets.is_empty());
            }
        }
        if let Some(item) = &dragging
            && let Some(at) = ctx.pointer_latest_pos()
        {
            let over =
                crate::kits::picker::dropped_on_canvas(at, canvas, rect, ctx.layer_id_at(at));
            if over {
                let add = crate::cursor::Bitmap::Arrow(Some(crate::cursor::Badge::Plus));
                ctx.set_cursor_image(self.cursors.image(ctx, add));
            }
            self.draw_asset_ghost(ctx, item, at);
        }

        match act {
            None => {}
            Some(Act::Tab(photos)) => {
                self.prefs.assets.photos_tab = photos;
                self.assets.prefs_dirty = true;
                if let Some(p) = self.asset_panel.as_mut() {
                    p.focus = true;
                }
            }
            Some(Act::Settings) => {
                self.asset_panel = None;
                self.toggle_settings(ctx);
            }
            Some(Act::RetrySets) => {
                for s in &on_sets {
                    self.assets.retry_set(s.key);
                }
            }
            Some(Act::RetryPhotos) => {
                if let Some(p) = self.asset_panel.as_mut() {
                    p.searched = None;
                    p.typed_at = None;
                }
            }
            Some(Act::Place(item, at)) => {
                let world = match at {
                    Some(at) => {
                        if !crate::kits::picker::dropped_on_canvas(
                            at,
                            canvas,
                            rect,
                            ctx.layer_id_at(at),
                        ) {
                            return;
                        }
                        Some(self.to_world(at, canvas, ctx.pixels_per_point()))
                    }
                    None => None,
                };
                match item {
                    Item::Icon(hit) => self.insert_icon(&hit, size, world),
                    Item::Photo(photo) => {
                        self.fetch_asset_photos(vec![*photo], PhotoIntent::Place { at: world })
                    }
                }
            }
            Some(Act::Fill(photo)) => {
                // Several layers take a different photo each, the clicked one first,
                // then the ones after it in grid order.
                let mut photos = vec![(*photo).clone()];
                if let Some(Ok(page)) = &self.assets.photos {
                    let from = page
                        .photos
                        .iter()
                        .position(|p| p.id == photo.id)
                        .unwrap_or(0);
                    photos.extend(
                        page.photos
                            .iter()
                            .skip(from + 1)
                            .chain(page.photos.iter().take(from))
                            .cloned(),
                    );
                }
                photos.truncate(targets.len());
                let targets = targets[..photos.len()].to_vec();
                self.fetch_asset_photos(photos, PhotoIntent::Fill { targets });
            }
        }
    }

    /// Ask for `photos` at Settings' size, saying so meanwhile.
    fn fetch_asset_photos(&mut self, photos: Vec<Photo>, intent: PhotoIntent) {
        let size = PhotoSize::from_key(&self.prefs.assets.photo_size);
        self.session.info(match photos.len() {
            1 => "Downloading the photo…".to_string(),
            n => format!("Downloading {n} photos…"),
        });
        self.assets.fetch_photos(photos, size, intent);
    }

    /// The carried tile under the pointer, faded, on top of everything.
    fn draw_asset_ghost(&mut self, ctx: &egui::Context, item: &Item, at: egui::Pos2) {
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Tooltip,
            egui::Id::new("assets-ghost"),
        ));
        let (tex, tint) = match item {
            Item::Icon(h) => {
                let px = (ICON_PT * ctx.pixels_per_point()).round();
                let key = format!("{}@{px}", h.key());
                let icon = h.icon().clone();
                (
                    self.assets
                        .icon_tile(key, || icon.svg(f64::from(px), "#ffffff"))
                        .cloned(),
                    theme::text::strong(),
                )
            }
            Item::Photo(p) => (self.assets.thumb(&p.thumb).cloned(), egui::Color32::WHITE),
        };
        if let Some(t) = tex {
            let size = t.size_vec2() / ctx.pixels_per_point();
            let size = match item {
                Item::Photo(_) => size * (120.0 / size.x.max(1.0)),
                Item::Icon(_) => size,
            };
            let r = egui::Rect::from_center_size(at, size);
            painter.image(t.id(), r, crate::thumbs::FULL_UV, tint.gamma_multiply(0.7));
        }
    }

    /// Place `hit` as a group of vectors named after it, `size` points across,
    /// centred on `at` (world) or mid-view, and select it: *Insert icon*.
    fn insert_icon(&mut self, hit: &Hit, size: f64, at: Option<Point>) {
        let icon = hit.icon();
        let svg = icon.svg(size, ICON_INK);
        let mut ids = ondin_core::IdSource::new(crate::session::random_actor());
        let parent = ids.mint();
        let mut images = crate::canvas::Decoded::default();
        let Ok(mut out) =
            crate::canvas::read_svg(&svg, &mut ids, parent, Some(&icon.name), &mut images)
        else {
            self.session
                .fail(format!("{} could not be read", icon.name));
            return;
        };
        out.tx.0.push(Operation::SetCredit {
            id: out.root,
            credit: Some(Box::new(icon_credit(hit.spec, &today()))),
        });
        self.session.name_step("Insert icon", icon::ASSETS);
        let at = at.unwrap_or_else(|| self.viewport_centre());
        match self.place_svg(out, Some(at)) {
            crate::canvas::SvgImport::Placed(_) => self
                .session
                .info(format!("Placed {} · Ctrl+Z to undo", icon.name)),
            _ => self
                .session
                .fail(format!("{} could not be placed", icon.name)),
        }
    }

    /// Land the photos fetched for an insert: a new layer (*Insert photo*), or the
    /// fill of each target (*Fill with photo*), each with its credit, as one step.
    pub(crate) fn land_asset_photos(&mut self) {
        if self.gesture_live() || self.modal_is_up() {
            return;
        }
        let Some((fetch, reads)) = self.assets.landed.take() else {
            return;
        };
        let today = today();
        // Each read keeps its slot, so a Fill's target is the one its photo was
        // chosen for; a failed download leaves its slot empty and the rest land.
        let mut loaded = Vec::new();
        let mut failed = None;
        for read in reads {
            match read {
                Ok((image, pixels)) => {
                    self.canvas.images_mut().adopt(&image.id, pixels);
                    loaded.push(Some(image));
                }
                Err(why) => {
                    failed.get_or_insert(why);
                    loaded.push(None);
                }
            }
        }
        let missed = loaded.iter().filter(|l| l.is_none()).count();
        if missed == loaded.len() {
            if let Some(why) = failed {
                self.session.fail(why);
            }
            return;
        }
        let keys = self.asset_keys.clone();
        match fetch.intent {
            PhotoIntent::Place { at } => {
                let (Some(photo), Some(Some(image))) = (fetch.photos.first(), loaded.first())
                else {
                    return;
                };
                let world = at.unwrap_or_else(|| self.viewport_centre());
                let (mut tx, node) = self.image_tx(None, image, world);
                tx.0.push(Operation::SetCredit {
                    id: node,
                    credit: Some(Box::new(photo.credit(&today))),
                });
                self.session.name_step("Insert photo", icon::IMAGE);
                if self.commit_edit(tx) {
                    self.assets.report_download(photo, keys.as_ref());
                    self.session.selection.set_one(node);
                    self.session.info(format!(
                        "Placed {} · {} · Ctrl+Z to undo",
                        image.label(),
                        photo.credit(&today).text
                    ));
                }
            }
            PhotoIntent::Fill { targets } => {
                let mut ops = Vec::new();
                let mut advanced = self.session.doc.clone();
                let mut filled = Vec::new();
                let mut used = Vec::new();
                for ((photo, image), id) in fetch.photos.iter().zip(&loaded).zip(&targets) {
                    let Some(image) = image else {
                        continue;
                    };
                    // Locked or otherwise changed while the photo downloaded.
                    if !fillable(&advanced, *id) {
                        continue;
                    }
                    let Some(tx) = fill_tx(&advanced, *id, image) else {
                        continue;
                    };
                    let mut tx = tx;
                    tx.0.push(Operation::SetCredit {
                        id: *id,
                        credit: Some(Box::new(photo.credit(&today))),
                    });
                    if advanced.apply(&tx).is_ok() {
                        ops.extend(tx.0);
                        filled.push(*id);
                        used.push(photo);
                    }
                }
                if filled.is_empty() {
                    self.session
                        .fail("The selection changed before the photo arrived");
                    return;
                }
                self.session
                    .name_step("Fill with photo", icon::PAINT_BUCKET);
                if self.commit_edit(Transaction(ops)) {
                    for photo in &used {
                        self.assets.report_download(photo, keys.as_ref());
                    }
                    let what = match filled.as_slice() {
                        [one] => crate::private::layer(&self.session.doc, *one).into_owned(),
                        many => ui::count(many.len(), "layer"),
                    };
                    let said = match (used.as_slice(), missed) {
                        // The credit of the photo written, not of the first fetched:
                        // a skipped target's photo is not used.
                        ([photo], 0) => format!(
                            "Filled {what} · {} · Ctrl+Z to undo",
                            photo.credit(&today).text
                        ),
                        (_, 0) => format!("Filled {what} · Ctrl+Z to undo"),
                        (_, n) => format!(
                            "Filled {what} · {} could not be downloaded · Ctrl+Z to undo",
                            ui::count(n, "photo")
                        ),
                    };
                    self.session.info(said);
                }
            }
        }
    }
}

/// The transaction giving `id` `image` as its fill, framed to fill the layer:
/// its picture fill replaced, else its top fill, else a new fill.
fn fill_tx(
    doc: &ondin_core::Document,
    id: NodeId,
    image: &crate::app::LoadedImage,
) -> Option<Transaction> {
    let paint = doc.get(id)?.paint();
    match crate::tools::croppable_fill(paint).or_else(|| paint.fills.len().checked_sub(1)) {
        Some(index) => ondin_core::build::replace_image_fill(
            doc,
            id,
            index,
            &image.id,
            Some(image.entry.clone()),
        ),
        None => {
            let mut ops = Vec::new();
            if !doc.has_image(&image.id) {
                ops.push(Operation::AddImage {
                    id: image.id.clone(),
                    entry: image.entry.clone(),
                });
            }
            ops.push(Operation::SetFills {
                id,
                fills: ondin_core::keyed_by_position([ondin_core::Fill {
                    brush: ondin_core::image_brush(image.id.clone()).into(),
                    visible: true,
                }]),
            });
            Some(Transaction(ops))
        }
    }
}

/// The search box, taking the keyboard once when the panel opens or a tab turns.
fn search_field(ui: &mut egui::Ui, panel: &mut Panel, hint: &str, now: f64) {
    let edit = ui::search_box(
        ui,
        egui::vec2(ui.available_width(), ui::CONTROL_H),
        &mut panel.query,
        hint,
        egui::Id::new(SEARCH_ID),
    );
    if panel.focus {
        panel.focus = false;
        edit.request_focus();
    }
    if edit.changed() {
        panel.typed_at = Some(now);
    }
}

/// The set and variant dropdowns and the size field.
fn icon_controls(
    ui: &mut egui::Ui,
    on: &[&'static SetSpec],
    set: Option<&'static SetSpec>,
    variant: Option<usize>,
    size: f64,
    prefs: &mut crate::prefs::AssetPrefs,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
        ui.spacing_mut().interact_size.y = ui::CONTROL_H;
        ui.spacing_mut().button_padding.y = 0.0;
        let set_w = ui.available_width() - VARIANT_W - SIZE_W - 2.0 * ui::CARD_COL_GAP;
        let mut picked = set.map(|s| s.key);
        egui::ComboBox::from_id_salt("assets-set")
            .icon(ui::combo_chevron)
            .width(set_w)
            .selected_text(ui::combo_text(set.map_or("All sets", |s| s.name)))
            .show_ui(ui, |ui| {
                ui::menu_rows(ui);
                ui.selectable_value(&mut picked, None, "All sets");
                for s in on {
                    ui.selectable_value(&mut picked, Some(s.key), s.name);
                }
            });
        if picked != set.map(|s| s.key) {
            prefs.set = picked.unwrap_or_default().to_string();
        }
        match (set, variant) {
            (Some(s), Some(v)) if s.variants.len() > 1 => {
                let mut chosen = v;
                // In a child no wider than the dropdown, which its truncation reads.
                ui.allocate_ui(egui::vec2(VARIANT_W, ui::CONTROL_H), |ui| {
                    egui::ComboBox::from_id_salt("assets-variant")
                        .icon(ui::combo_chevron)
                        .width(VARIANT_W)
                        .truncate()
                        .selected_text(ui::combo_text(s.variants[v].label))
                        .show_ui(ui, |ui| {
                            ui::menu_rows(ui);
                            for (i, var) in s.variants.iter().enumerate() {
                                ui.selectable_value(&mut chosen, i, var.label);
                            }
                        });
                });
                if chosen != v {
                    prefs
                        .variants
                        .insert(s.key.to_string(), s.variants[chosen].label.to_string());
                }
            }
            (set, _) => {
                let label = set.map_or("Variant", |s| s.variants[0].label);
                let tip = match set {
                    Some(s) => format!("{} has one style", s.name),
                    None => "Pick one set to choose its variant".to_string(),
                };
                ui.add_enabled_ui(false, |ui| {
                    egui::ComboBox::from_id_salt("assets-variant-off")
                        .icon(ui::combo_chevron)
                        .width(VARIANT_W)
                        .truncate()
                        .selected_text(ui::combo_text(label))
                        .show_ui(ui, |_| {});
                })
                .response
                .on_disabled_hover_text(tip);
            }
        }
        let mut v = size;
        let (resp, _) = ui::value_field_suffixed(
            ui,
            egui::vec2(SIZE_W, ui::CONTROL_H),
            ui::Prefix::Text(""),
            Some(ui::Suffix {
                text: "px",
                clickable: false,
                tooltip: "",
            }),
            &mut v,
            ui::Scrub::whole(0.25).range(4.0..=1024.0),
            |d| d.custom_formatter(ui::number(0)),
        );
        resp.on_hover_text("The size an icon is placed at; its set's own grid at first");
        if (v - size).abs() > f64::EPSILON {
            prefs.icon_size = Some(v.round().clamp(4.0, 1024.0));
        }
    });
}

/// The service and orientation dropdowns.
fn photo_controls(
    ui: &mut egui::Ui,
    on: &[&'static ServiceSpec],
    service: &'static ServiceSpec,
    orientation: &mut Orientation,
    prefs: &mut crate::prefs::AssetPrefs,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = ui::CARD_COL_GAP;
        ui.spacing_mut().interact_size.y = ui::CONTROL_H;
        ui.spacing_mut().button_padding.y = 0.0;
        let svc_w = ui.available_width() - ORIENT_W - ui::CARD_COL_GAP;
        let mut picked = service.key;
        egui::ComboBox::from_id_salt("assets-service")
            .icon(ui::combo_chevron)
            .width(svc_w)
            .selected_text(ui::combo_text(service.name))
            .show_ui(ui, |ui| {
                ui::menu_rows(ui);
                for s in on {
                    ui.selectable_value(&mut picked, s.key, s.name);
                }
            });
        if picked != service.key {
            prefs.service = picked.to_string();
        }
        egui::ComboBox::from_id_salt("assets-orientation")
            .icon(ui::combo_chevron)
            .width(ORIENT_W)
            .selected_text(ui::combo_text(orientation.label()))
            .show_ui(ui, |ui| {
                ui::menu_rows(ui);
                for o in Orientation::ALL {
                    ui.selectable_value(orientation, o, o.label());
                }
            });
    });
}

/// *Click fills Hero image* · *Alt-click places*, in the accent wash.
fn fill_strip(ui: &mut egui::Ui, text: &str) {
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), STRIP_H),
        egui::Sense::hover(),
    );
    let p = ui.painter();
    p.rect_filled(
        rect,
        egui::CornerRadius::same(ui::LIST_ROW_RADIUS),
        color::accent_a(36),
    );
    p.text(
        rect.left_center() + egui::vec2(8.0, 0.0),
        egui::Align2::LEFT_CENTER,
        icon::PAINT_BUCKET,
        theme::icon_font(13.0),
        color::accent_100(),
    );
    p.text(
        rect.left_center() + egui::vec2(26.0, 0.0),
        egui::Align2::LEFT_CENTER,
        text,
        egui::FontId::proportional(ui::SMALL_PT),
        color::accent_100(),
    );
    p.text(
        rect.right_center() - egui::vec2(8.0, 0.0),
        egui::Align2::RIGHT_CENTER,
        crate::input::keys("Alt+click places"),
        egui::FontId::proportional(ui::CAPTION_PT),
        color::accent_100().gamma_multiply(0.6),
    );
}

/// *Downloading Lucide · 1.2 of 3.4 MB*.
fn progress_row(ui: &mut egui::Ui, name: &str, done: u64, total: Option<u64>) {
    let mb = |n: u64| format!("{:.1}", n as f64 / (1024.0 * 1024.0));
    let text = match total {
        Some(t) if t > 0 => format!("Downloading {name} · {} of {} MB", mb(done), mb(t)),
        _ if done > 0 => format!("Downloading {name} · {} MB", mb(done)),
        _ => format!("Downloading {name}…"),
    };
    ui.label(
        egui::RichText::new(text)
            .size(ui::CAPTION_PT)
            .color(theme::text::dim()),
    );
    ui.add_space(4.0);
}

/// An empty state with one button, its press answering `then`.
fn empty(
    ui: &mut egui::Ui,
    glyph: &str,
    words: (&str, &str),
    button: &str,
    out: &mut Out<'_>,
    then: Act,
) {
    ui.add_space(EMPTY_AIR);
    let pressed = ui::empty_state(ui, Some(glyph), words.0, words.1, |ui| {
        ui.add_space(2.0);
        crate::settings::text_button(ui, button, ui::FieldButton::Off).clicked()
    });
    if pressed {
        out.act = Some(then);
    }
}

/// A one-line note where the grid would be, as the insert picker's.
fn note(ui: &mut egui::Ui, text: &str) {
    ui.add_space(6.0);
    ui::empty_note(ui, text);
}

/// What the Icons tab shows this frame.
struct IconsView<'a> {
    on: &'a [&'static SetSpec],
    set: Option<&'static SetSpec>,
    variant: Option<usize>,
    size: f64,
    ready: &'a [(&'static SetSpec, Arc<IconSet>)],
    loading: &'a [(&'static SetSpec, u64, Option<u64>)],
    failed: &'a [&'static SetSpec],
}

/// The Icons tab under the tabs: the search, the set, variant and size, any set
/// still downloading, then the grid or what stands in for it, and the foot.
fn icons_body(
    ui: &mut egui::Ui,
    panel: &mut Panel,
    view: &IconsView<'_>,
    bottom: f32,
    now: f64,
    out: &mut Out<'_>,
) {
    if view.on.is_empty() {
        let words = (
            "No icon sets are on",
            "Switch on the sets you want in Settings. Each downloads once and works offline.",
        );
        empty(ui, icon::SHAPES, words, "Open settings", out, Act::Settings);
        return;
    }
    search_field(ui, panel, "Search icons", now);
    ui.add_space(ROW_GAP);
    icon_controls(
        ui,
        view.on,
        view.set,
        view.variant,
        view.size,
        &mut out.prefs,
    );
    ui.add_space(ROW_GAP);
    let hits = panel
        .hits
        .as_ref()
        .map(|(_, h)| h.clone())
        .unwrap_or_default();
    for (spec, done, total) in view.loading {
        progress_row(ui, spec.name, *done, *total);
    }
    if !hits.is_empty() {
        let list_h = bottom - ui.cursor().top() - FOOT_H - ROW_GAP;
        icon_grid(ui, panel, &hits, list_h, out);
    } else if !view.failed.is_empty() && view.loading.is_empty() && view.ready.is_empty() {
        let names: Vec<&str> = view.failed.iter().map(|s| s.name).collect();
        let head = format!("Can't reach {}", or_list(&names));
        let words = (
            head.as_str(),
            "Check the connection. Sets already downloaded still work.",
        );
        empty(
            ui,
            icon::WIFI_SLASH,
            words,
            "Try again",
            out,
            Act::RetrySets,
        );
    } else if view.loading.is_empty() {
        note(ui, &format!("No icons match “{}”", panel.query.trim()));
    }
    let left = match view.set {
        Some(s) => format!("{} · {}", s.name, s.licence),
        None => format!("All sets · {}", ui::count(view.on.len(), "set")),
    };
    foot(ui, bottom, &left, &counted(hits.len() as u64, "icon"));
}

/// What the Photos tab shows this frame.
struct PhotosView<'a> {
    on: &'a [&'static ServiceSpec],
    service: Option<&'static ServiceSpec>,
    key_missing: bool,
    fill_names: &'a [String],
}

/// The Photos tab under the tabs: the search, the service and orientation, the
/// fill strip when a click fills, then the photos or what stands in for them,
/// and the foot. The end of the grid in view asks for the next page.
fn photos_body(
    ui: &mut egui::Ui,
    panel: &mut Panel,
    view: &PhotosView<'_>,
    bottom: f32,
    now: f64,
    out: &mut Out<'_>,
) {
    let Some(svc) = view.service else {
        let words = (
            "No photo services are on",
            "Switch on Unsplash, Pexels or Openverse in Settings.",
        );
        empty(ui, icon::IMAGE, words, "Open settings", out, Act::Settings);
        return;
    };
    search_field(ui, panel, "Search photos", now);
    ui.add_space(ROW_GAP);
    photo_controls(ui, view.on, svc, &mut panel.orientation, &mut out.prefs);
    ui.add_space(ROW_GAP);
    if !view.fill_names.is_empty() {
        let what = match view.fill_names {
            [one] => one.clone(),
            many => ui::count(many.len(), "layer"),
        };
        fill_strip(ui, &format!("Click fills {what}"));
        ui.add_space(ROW_GAP);
    }
    let mut total = None;
    if view.key_missing {
        let (head, line) = photo_error_words(&PhotoError::NoKey, svc.name);
        empty(
            ui,
            icon::IMAGE,
            (&head, &line),
            "Open settings",
            out,
            Act::Settings,
        );
    } else if panel.searched.is_none() {
        note(ui, &format!("Search {} for photos", svc.name));
    } else {
        match out.assets.photos.clone() {
            None => note(ui, &format!("Searching {}…", svc.name)),
            Some(Err(e)) => {
                let (head, line) = photo_error_words(&e, svc.name);
                let (button, then) = match e {
                    PhotoError::NoKey | PhotoError::Refused => ("Open settings", Act::Settings),
                    _ => ("Try again", Act::RetryPhotos),
                };
                let glyph = match e {
                    PhotoError::Unreachable => icon::WIFI_SLASH,
                    _ => icon::IMAGE,
                };
                empty(ui, glyph, (&head, &line), button, out, then);
            }
            Some(Ok(page)) if page.photos.is_empty() => {
                note(
                    ui,
                    &format!("No photos match “{}” on {}", panel.query.trim(), svc.name),
                );
            }
            Some(Ok(page)) => {
                total = Some(page.total);
                let list_h = bottom - ui.cursor().top() - FOOT_H - ROW_GAP;
                let fills = !view.fill_names.is_empty();
                let end = photo_grid(ui, &page, list_h, fills, out);
                if end
                    && page.more
                    && !out.assets.searching()
                    && let Some((q, s, o)) = &panel.searched
                    && let Some(spec) = ServiceSpec::by_key(s)
                {
                    panel.page += 1;
                    out.search = Some((q.clone(), spec, *o));
                }
            }
        }
    }
    let licence = svc.licence.unwrap_or("licence varies by photo");
    let right = total.map(|t| counted(t, "photo")).unwrap_or_default();
    foot(ui, bottom, &format!("{} · {licence}", svc.name), &right);
}

/// The caption at the panel's foot, on a hairline: what is shown and its licence
/// on the left, the count on the right.
fn foot(ui: &mut egui::Ui, bottom: f32, left: &str, right: &str) {
    let rect = egui::Rect::from_min_max(
        egui::pos2(ui.max_rect().left(), bottom - FOOT_H),
        egui::pos2(ui.max_rect().right(), bottom),
    );
    let p = ui.painter();
    let ppp = ui.ctx().pixels_per_point();
    let y = ui::hairline_in_column(rect.top() + 0.5, ppp, 1.0);
    p.line_segment(
        [egui::pos2(rect.left(), y), egui::pos2(rect.right(), y)],
        egui::Stroke::new(1.0, color::divider()),
    );
    let mid = rect.center().y + 4.0;
    p.text(
        egui::pos2(rect.left(), mid),
        egui::Align2::LEFT_CENTER,
        left,
        egui::FontId::proportional(ui::CAPTION_PT),
        theme::text::faint(),
    );
    p.text(
        egui::pos2(rect.right(), mid),
        egui::Align2::RIGHT_CENTER,
        right,
        egui::FontId::proportional(ui::CAPTION_PT),
        theme::text::faint(),
    );
}

/// A tile's click, drag and hover, shared by icons and photos. A drag carries the
/// item and drops where it is let go, unless a right-click took it away.
fn take(ui: &egui::Ui, resp: &egui::Response, item: Item, fills: bool, out: &mut Out<'_>) {
    let alt = ui.input(|i| i.modifiers.alt);
    if resp.clicked() {
        out.act = Some(match (&item, fills && !alt) {
            (Item::Photo(p), true) => Act::Fill(p.clone()),
            _ => Act::Place(item.clone(), None),
        });
    }
    if resp.hovered() && !resp.dragged() {
        out.hovered = Some((item.clone(), resp.rect));
    }
    if resp.dragged() && !out.cancelled {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        out.dragging = Some(item.clone());
    }
    if resp.drag_stopped()
        && !out.cancelled
        && let Some(at) = ui.ctx().pointer_latest_pos()
    {
        out.act = Some(Act::Place(item, Some(at)));
    }
}

/// The icon grid, scrolled, its tiles drawn as they arrive; more load as the end
/// comes into view.
fn icon_grid(ui: &mut egui::Ui, panel: &mut Panel, hits: &[Hit], height: f32, out: &mut Out<'_>) {
    let ppp = ui.ctx().pixels_per_point();
    let px = (ICON_PT * ppp).round();
    let shown = panel.shown.min(hits.len());
    let mut near_end = false;
    egui::ScrollArea::vertical()
        .id_salt("assets-icons")
        .max_height(height)
        .min_scrolled_height(height)
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
            let w = ui.available_width();
            let tile_w = (w - TILE_GAP * (COLS as f32 - 1.0)) / COLS as f32;
            for (row, chunk) in hits[..shown].chunks(COLS).enumerate() {
                let (band, _) = ui.allocate_exact_size(egui::vec2(w, TILE_H), egui::Sense::hover());
                let visible = ui.is_rect_visible(band);
                for (i, hit) in chunk.iter().enumerate() {
                    let cell = egui::Rect::from_min_size(
                        egui::pos2(band.left() + i as f32 * (tile_w + TILE_GAP), band.top()),
                        egui::vec2(tile_w, TILE_H),
                    );
                    let resp = ui.interact(
                        cell,
                        egui::Id::new(("asset-icon", hit.key())),
                        egui::Sense::click_and_drag(),
                    );
                    take(ui, &resp, Item::Icon(hit.clone()), false, out);
                    if !visible {
                        continue;
                    }
                    ui::list_row_ground(
                        ui.painter(),
                        cell,
                        false,
                        resp.hovered() || resp.dragged(),
                    );
                    let key = format!("{}@{px}", hit.key());
                    let icon = hit.icon().clone();
                    let svg = || icon.svg(f64::from(px), "#ffffff");
                    if let Some(t) = out.assets.icon_tile(key, svg) {
                        let size = t.size_vec2() / ppp;
                        let r = egui::Rect::from_center_size(cell.center(), size);
                        ui.painter().image(
                            t.id(),
                            r,
                            crate::thumbs::FULL_UV,
                            theme::text::strong(),
                        );
                    }
                }
                ui.add_space(TILE_GAP);
                if (row + 4) * COLS >= shown && ui.is_rect_visible(band) {
                    near_end = true;
                }
            }
        });
    if near_end && shown < hits.len() {
        panel.shown += PAGE;
        ui.ctx().request_repaint();
    }
}

/// The photos in two columns, each tile at its photo's own aspect; answers
/// whether the end came into view, for the next page.
fn photo_grid(ui: &mut egui::Ui, page: &Page, height: f32, fills: bool, out: &mut Out<'_>) -> bool {
    let mut near_end = false;
    egui::ScrollArea::vertical()
        .id_salt("assets-photos")
        .max_height(height)
        .min_scrolled_height(height)
        .auto_shrink([false, false])
        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
        .show(ui, |ui| {
            let w = ui.available_width();
            let col_w = (w - PHOTO_GAP) / 2.0;
            let top = ui.cursor().top();
            let mut y = [top, top];
            for photo in &page.photos {
                let col = usize::from(y[1] < y[0]);
                let aspect = match photo.size {
                    Some((pw, ph)) => ph as f32 / pw as f32,
                    None => out
                        .assets
                        .thumb(&photo.thumb)
                        .map_or(0.75, |t| t.size_vec2().y / t.size_vec2().x.max(1.0)),
                };
                let h = (col_w * aspect).clamp(48.0, 360.0);
                let cell = egui::Rect::from_min_size(
                    egui::pos2(
                        ui.max_rect().left() + col as f32 * (col_w + PHOTO_GAP),
                        y[col],
                    ),
                    egui::vec2(col_w, h),
                );
                y[col] += h + PHOTO_GAP;
                let resp = ui.interact(
                    cell,
                    egui::Id::new(("asset-photo", photo.service, &photo.id)),
                    egui::Sense::click_and_drag(),
                );
                take(ui, &resp, Item::Photo(Box::new(photo.clone())), fills, out);
                if !ui.is_rect_visible(cell) {
                    continue;
                }
                let p = ui.painter();
                let radius = egui::CornerRadius::same(ui::LIST_ROW_RADIUS);
                match out.assets.thumb(&photo.thumb) {
                    Some(t) => {
                        p.add(egui::Shape::Rect(
                            egui::epaint::RectShape::filled(cell, radius, egui::Color32::WHITE)
                                .with_texture(t.id(), crate::thumbs::FULL_UV),
                        ));
                    }
                    None => {
                        p.rect_filled(cell, radius, color::text_a(18));
                    }
                }
                let lit = resp.hovered() || resp.dragged();
                if lit {
                    p.rect_stroke(
                        cell,
                        radius,
                        egui::Stroke::new(2.0, color::accent_400()),
                        egui::StrokeKind::Inside,
                    );
                    // The photographer over the tile's foot, on a fade to black.
                    let band = egui::Rect::from_min_max(
                        egui::pos2(cell.left(), cell.bottom() - 24.0),
                        cell.right_bottom(),
                    );
                    let mut mesh = egui::Mesh::default();
                    let clear = egui::Color32::from_black_alpha(0);
                    let dark = egui::Color32::from_black_alpha(160);
                    mesh.colored_vertex(band.left_top(), clear);
                    mesh.colored_vertex(band.right_top(), clear);
                    mesh.colored_vertex(band.right_bottom(), dark);
                    mesh.colored_vertex(band.left_bottom(), dark);
                    mesh.add_triangle(0, 1, 2);
                    mesh.add_triangle(0, 2, 3);
                    p.add(mesh);
                    // Its baseline as far above the tile's foot as the name is in
                    // from the left, so the two margins read equal.
                    let name = p.layout_no_wrap(
                        photo.author.clone(),
                        egui::FontId::proportional(ui::CAPTION_PT),
                        egui::Color32::WHITE,
                    );
                    let baseline = name
                        .rows
                        .first()
                        .and_then(|r| r.row.glyphs.first().map(|g| r.pos.y + g.pos.y))
                        .unwrap_or(name.size().y);
                    p.with_clip_rect(cell).galley(
                        egui::pos2(
                            cell.left() + AUTHOR_INSET,
                            cell.bottom() - AUTHOR_INSET - baseline,
                        ),
                        name,
                        egui::Color32::WHITE,
                    );
                }
                if out.fetching && lit {
                    ui.ctx().set_cursor_icon(egui::CursorIcon::Progress);
                }
            }
            let end = y[0].max(y[1]);
            ui.allocate_space(egui::vec2(w, (end - top).max(0.0)));
            let probe = egui::Rect::from_min_size(
                egui::pos2(ui.max_rect().left(), end - 200.0),
                egui::vec2(w, 200.0),
            );
            near_end = ui.is_rect_visible(probe);
        });
    near_end
}

/// The hover card beside the panel, once the pointer has rested.
#[allow(clippy::too_many_arguments)]
fn hover_card(
    ctx: &egui::Context,
    item: &Item,
    panel: egui::Rect,
    row: egui::Rect,
    icon_size: f64,
    photo_size: PhotoSize,
    fills: bool,
) {
    ui::rail_hover_card(ctx, egui::Id::new("assets-card"), panel, row, |ui| {
        ui.spacing_mut().item_spacing.y = 5.0;
        let line = |ui: &mut egui::Ui, text: &str| {
            ui.add(
                egui::Label::new(
                    egui::RichText::new(text)
                        .size(ui::SMALL_PT)
                        .color(theme::text::dim()),
                )
                .wrap(),
            );
        };
        match item {
            Item::Icon(h) => {
                let icon = h.icon();
                ui.label(
                    egui::RichText::new(&icon.name)
                        .size(ui::CONTROL_PT)
                        .color(theme::text::strong()),
                );
                line(
                    ui,
                    &format!(
                        "{} · {} · {icon_size:.0} px",
                        h.spec.name, h.spec.variants[icon.variant].label
                    ),
                );
                if !icon.aliases.is_empty() {
                    line(ui, &format!("Also: {}", icon.aliases.join(", ")));
                }
                line(ui, &format!("Licence: {}", h.spec.licence));
                line(ui, "Click to place · drag to drop");
            }
            Item::Photo(p) => {
                if !p.title.is_empty() {
                    ui.label(
                        egui::RichText::new(&p.title)
                            .size(ui::CONTROL_PT)
                            .color(theme::text::strong()),
                    );
                }
                line(ui, &p.credit("").text);
                let at = match (photo_size, p.service) {
                    (_, "openverse") | (PhotoSize::Full, _) => "full size".to_string(),
                    (PhotoSize::Small, _) => "400 px wide".to_string(),
                    (PhotoSize::Regular, _) => "1080 px wide".to_string(),
                };
                line(ui, &format!("Comes in at {at} · {}", p.licence));
                line(
                    ui,
                    match fills {
                        true => "Click to fill · Alt-click to place · drag to drop",
                        false => "Click to place · drag to drop",
                    },
                );
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set_with(names: &[&str]) -> Arc<IconSet> {
        Arc::new(IconSet {
            icons: names
                .iter()
                .map(|n| sets::Icon {
                    name: n.to_string(),
                    variant: 0,
                    body: "<path d='M0 0h1v1z'/>".into(),
                    view: [0.0, 0.0, 24.0, 24.0],
                    aliases: Vec::new(),
                })
                .collect(),
            grid: 24.0,
            variant_grids: Vec::new(),
        })
    }

    /// Results rank across sets, then fall back to name order, and an empty
    /// query browses everything.
    #[test]
    fn icons_rank_across_sets() {
        let a = SetSpec::by_key("lucide").unwrap();
        let b = SetSpec::by_key("tabler").unwrap();
        let sets = vec![
            (a, set_with(&["arrow-right", "sparrow"])),
            (b, set_with(&["arrow", "car"])),
        ];
        let names = |q: &str| -> Vec<String> {
            find_icons(q, &sets, None)
                .iter()
                .map(|h| format!("{}:{}", h.spec.key, h.icon().name))
                .collect()
        };
        assert_eq!(
            names("arrow"),
            ["tabler:arrow", "lucide:arrow-right", "lucide:sparrow"]
        );
        assert_eq!(names("").len(), 4);
        assert!(names("zebra").is_empty());
    }

    /// The words that join a list and mark thousands.
    #[test]
    fn lists_and_counts_read_as_prose() {
        assert_eq!(or_list(&["A"]), "A");
        assert_eq!(or_list(&["A", "B", "C"]), "A, B or C");
        assert_eq!(grouped(2184), "2,184");
        assert_eq!(grouped(999), "999");
        assert_eq!(grouped(1_234_567), "1,234,567");
        assert_eq!(counted(1, "icon"), "1 icon");
        assert_eq!(counted(2184, "photo"), "2,184 photos");
    }

    /// The last undo step's name.
    fn last_step(app: &OndinApp) -> String {
        let step = app.session.history.done().last().expect("a step");
        step.note.label.clone()
    }

    /// A 40 × 30 rectangle on the page, solid-filled; its id.
    fn rect(app: &mut OndinApp) -> NodeId {
        let id = app.session.ids.mint();
        let page = app.session.page;
        app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id,
                parent: page,
                index: 0,
                kind: NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(40.0, 30.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some("Hero image".into()),
            },
            Operation::SetFills {
                id,
                fills: ondin_core::keyed_by_position([ondin_core::Fill {
                    brush: ondin_core::Brush::Solid(ondin_core::peniko::Color::BLACK).into(),
                    visible: true,
                }]),
            },
        ]));
        id
    }

    /// A photo whose credit is Unsplash's.
    fn photo() -> Photo {
        Photo {
            service: "unsplash",
            id: "x1".into(),
            size: Some((8, 6)),
            thumb: "https://i/s".into(),
            urls: Default::default(),
            title: "a coffee cup".into(),
            author: "Ana Ruiz".into(),
            author_url: "https://unsplash.com/@ana".into(),
            page_url: "https://unsplash.com/photos/x1".into(),
            licence: "Unsplash License".into(),
            licence_url: "https://unsplash.com/license".into(),
            report_url: None,
        }
    }

    /// Hand the app `photo` as if its fetch had landed, for `intent`.
    fn land(app: &mut OndinApp, photos: Vec<Photo>, intent: PhotoIntent) {
        let png = crate::app::rgba_to_png(&[200u8; 8 * 6 * 4], (8, 6)).unwrap();
        let reads = photos
            .iter()
            .map(|_| {
                crate::app::read_picture(png.clone(), None, Some("a coffee cup".into()))
                    .map_err(|_| String::new())
            })
            .collect();
        app.assets.landed = Some((
            super::super::Fetch {
                token: 1,
                photos,
                intent,
            },
            reads,
        ));
        app.land_asset_photos();
    }

    /// An icon inserts as a group named after it, at the size asked, credited, as
    /// one *Insert icon* step, and is selected.
    #[test]
    fn an_icon_inserts_as_a_credited_group() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let spec = SetSpec::by_key("font-awesome").unwrap();
        let set = Arc::new(IconSet {
            icons: vec![sets::Icon {
                name: "bell".into(),
                variant: 0,
                body: r#"<path fill="currentColor" d="M0 0h32v32H0z"/>"#.into(),
                view: [0.0, 0.0, 32.0, 32.0],
                aliases: Vec::new(),
            }],
            grid: 32.0,
            variant_grids: Vec::new(),
        });
        let hit = Hit {
            spec,
            set,
            index: 0,
        };
        app.insert_icon(&hit, 24.0, None);
        let [id] = *app.session.selection.ids() else {
            panic!("one layer selected, got {:?}", app.session.selection.ids());
        };
        let node = app.session.doc.get(id).unwrap();
        assert_eq!(node.name(), "bell");
        assert!(matches!(node.kind(), NodeKind::Group));
        let c = node.credit().expect("credited");
        assert_eq!(c.text, "Icons by Font Awesome, CC BY 4.0");
        assert!(c.required);
        assert_eq!(last_step(&app), "Insert icon");
        let w = app.session.resolved.world_bounds(id).unwrap().width();
        assert!((w - 24.0).abs() < 0.5, "24 across, got {w}");
        assert!(app.has_credits(), "a CC BY icon is listed");
    }

    /// A clicked photo with a shape selected fills it, credit and all, as one
    /// *Fill with photo* step; undone, the credit goes with the fill.
    #[test]
    fn a_photo_fills_the_selected_shape_with_its_credit() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let target = rect(&mut app);
        app.session.selection.set_one(target);
        assert_eq!(app.photo_targets(), vec![target], "the fixture: fillable");
        land(
            &mut app,
            vec![photo()],
            PhotoIntent::Fill {
                targets: vec![target],
            },
        );
        let node = app.session.doc.get(target).unwrap();
        assert!(matches!(
            node.paint().fills[0].brush.literal(),
            Some(ondin_core::Brush::Image(_))
        ));
        assert_eq!(
            node.credit().map(|c| c.text.as_str()),
            Some("Photo by Ana Ruiz on Unsplash")
        );
        assert_eq!(last_step(&app), "Fill with photo");
        assert!(app.has_credits());
        app.undo();
        assert_eq!(app.session.doc.get(target).unwrap().credit(), None);
    }

    /// A placed photo is a new layer named for what it shows, credited,
    /// *Insert photo*.
    #[test]
    fn a_placed_photo_is_a_credited_layer() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        land(&mut app, vec![photo()], PhotoIntent::Place { at: None });
        let [id] = *app.session.selection.ids() else {
            panic!("one layer selected");
        };
        let node = app.session.doc.get(id).unwrap();
        assert_eq!(node.name(), "a coffee cup");
        assert!(node.credit().is_some());
        assert_eq!(last_step(&app), "Insert photo");
    }

    /// Text, a line and a locked shape cannot take a photo: no fill strip, and
    /// a click places instead.
    #[test]
    fn only_unlocked_shapes_and_frames_take_a_photo() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let target = rect(&mut app);
        app.session.selection.set_one(target);
        app.session.commit(Transaction(vec![Operation::SetLocked {
            id: target,
            locked: true,
        }]));
        assert!(app.photo_targets().is_empty());
    }

    /// A part of an inserted icon is no photo target: its one credit is the icon's.
    /// flip: drop the credit test in `fillable` → fails here.
    #[test]
    fn an_icons_part_takes_no_photo() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let target = rect(&mut app);
        app.session.commit(Transaction(vec![Operation::SetCredit {
            id: target,
            credit: Some(Box::new(icon_credit(
                SetSpec::by_key("font-awesome").unwrap(),
                "2026-11-08",
            ))),
        }]));
        app.session.selection.set_one(target);
        assert!(app.photo_targets().is_empty());
    }

    /// A photo asked for in one document is not placed in the next.
    #[test]
    fn a_fetch_does_not_follow_a_document_switch() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.assets.fetch = Some(super::super::Fetch {
            token: 1,
            photos: vec![photo()],
            intent: PhotoIntent::Place { at: None },
        });
        app.reset_transient_state();
        assert!(app.assets.fetch.is_none() && app.assets.landed.is_none());
    }

    /// A path inside an inserted icon's group is no photo target either: filled,
    /// then ungrouped, it would keep the photo's credit and drop the icon's.
    /// flip: `fillable` testing the layer's own credit only → fails here.
    #[test]
    fn a_path_inside_an_icon_takes_no_photo() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let set = Arc::new(IconSet {
            icons: vec![sets::Icon {
                name: "bell".into(),
                variant: 0,
                body: r#"<path fill="currentColor" d="M0 0h32v32H0z"/>"#.into(),
                view: [0.0, 0.0, 32.0, 32.0],
                aliases: Vec::new(),
            }],
            grid: 32.0,
            variant_grids: Vec::new(),
        });
        let hit = Hit {
            spec: SetSpec::by_key("font-awesome").unwrap(),
            set,
            index: 0,
        };
        app.insert_icon(&hit, 24.0, None);
        let group = app.session.selection.ids()[0];
        let part = app.session.doc.get(group).unwrap().children()[0];
        app.session.selection.set_one(part);
        assert!(app.photo_targets().is_empty());
    }

    /// One failed download among three fills the other two targets and says how
    /// many photos were missed; a target locked while its photo downloaded is
    /// left alone.
    /// flip: `land_asset_photos` without its `fillable` re-check → fails at "the
    /// locked target keeps its fill".
    #[test]
    fn a_failed_or_locked_slot_leaves_the_rest_to_land() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let [a, b, c, d] = [0; 4].map(|_| rect(&mut app));
        app.session.commit(Transaction(vec![Operation::SetLocked {
            id: d,
            locked: true,
        }]));
        let png = crate::app::rgba_to_png(&[200u8; 8 * 6 * 4], (8, 6)).unwrap();
        let ok = || {
            crate::app::read_picture(png.clone(), None, Some("a coffee cup".into()))
                .map_err(|_| String::new())
        };
        app.assets.landed = Some((
            super::super::Fetch {
                token: 1,
                photos: vec![photo(); 4],
                intent: PhotoIntent::Fill {
                    targets: vec![a, b, c, d],
                },
            },
            vec![
                ok(),
                Err("The photo could not be downloaded".into()),
                ok(),
                ok(),
            ],
        ));
        app.land_asset_photos();
        let imaged = |id| {
            matches!(
                app.session.doc.get(id).unwrap().paint().fills[0]
                    .brush
                    .literal(),
                Some(ondin_core::Brush::Image(_))
            )
        };
        assert!(imaged(a) && imaged(c), "the others land");
        assert!(!imaged(b), "the failed slot keeps its fill");
        assert!(!imaged(d), "the locked target keeps its fill");
        let said = &app.session.status().text;
        assert!(said.contains("1 photo could not be downloaded"), "{said}");
    }

    /// A Fill whose first target was locked while the photos downloaded names
    /// the photographer of the photo written, and reports to Unsplash only that
    /// photo, after the commit; a report address off Unsplash's API is never
    /// called, and nothing is reported when nothing is filled.
    /// flip: the status taking the first loaded photo's credit → fails at "the
    /// used photo's author"; `report_download` called for every fetched photo →
    /// fails at "only the photo used"; its `UNSPLASH_API` guard dropped → fails at
    /// "never off the API".
    #[test]
    fn a_skipped_target_neither_names_nor_reports_its_photo() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        let [a, b] = [0; 2].map(|_| rect(&mut app));
        app.session.commit(Transaction(vec![Operation::SetLocked {
            id: a,
            locked: true,
        }]));
        let by = |author: &str, report: &str| Photo {
            author: author.into(),
            report_url: Some(report.into()),
            ..photo()
        };
        let p1 = by("Ana Ruiz", "https://api.unsplash.com/photos/p1/download");
        let p2 = by("Bo Lin", "https://api.unsplash.com/photos/p2/download");
        land(
            &mut app,
            vec![p1, p2],
            PhotoIntent::Fill {
                targets: vec![a, b],
            },
        );
        let said = &app.session.status().text;
        assert!(
            said.contains("Bo Lin") && !said.contains("Ana Ruiz"),
            "the used photo's author: {said}"
        );
        assert_eq!(
            app.assets.reported,
            ["https://api.unsplash.com/photos/p2/download"],
            "only the photo used"
        );

        app.assets.reported.clear();
        let elsewhere = by("Cy", "https://example.com/photos/p3/download");
        land(
            &mut app,
            vec![elsewhere],
            PhotoIntent::Fill { targets: vec![b] },
        );
        assert!(app.assets.reported.is_empty(), "never off the API");

        let p4 = by("Di", "https://api.unsplash.com/photos/p4/download");
        land(&mut app, vec![p4], PhotoIntent::Fill { targets: vec![a] });
        assert!(
            app.assets.reported.is_empty(),
            "nothing filled, nothing reported"
        );
    }

    /// The panel shares its place: opening it closes the insert picker and the
    /// styles browser, opening either closes it, and Escape closes it.
    #[test]
    fn the_panel_shares_the_pickers_place() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.toggle_kit_picker();
        app.toggle_assets();
        assert!(app.asset_panel.is_some() && app.kit_picker.is_none());
        app.toggle_var_browser();
        assert!(app.asset_panel.is_none() && app.var_browser.is_some());
        app.toggle_assets();
        assert!(app.var_browser.is_none());
        app.escape(&ctx);
        assert!(app.asset_panel.is_none(), "the ladder closes it");
    }

    /// With no set switched on, the Icons tab says how to switch one on, and
    /// nothing is asked of the network.
    #[test]
    fn with_nothing_on_the_panel_points_to_settings() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        // No canvas is drawn here: the panel hangs off its corner.
        app.canvas_rect =
            egui::Rect::from_min_size(egui::pos2(46.0, 40.0), egui::vec2(1000.0, 700.0));
        app.toggle_assets();
        let mut texts = Vec::new();
        for _ in 0..3 {
            let out = ctx.run_ui(Default::default(), |ui| app.assets_ui(ui.ctx()));
            texts = out
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
        }
        assert!(
            texts.iter().any(|t| t == "No icon sets are on"),
            "{texts:?}"
        );
        assert!(texts.iter().any(|t| t == "Open settings"), "{texts:?}");
        assert!(app.assets.sets.is_empty(), "no set was asked for");
    }

    /// The panel's rect over `frames` frames.
    fn panel_rect(app: &mut OndinApp, ctx: &egui::Context) -> (egui::Rect, Vec<String>) {
        let mut texts = Vec::new();
        for _ in 0..3 {
            let out = ctx.run_ui(Default::default(), |ui| app.assets_ui(ui.ctx()));
            texts = out
                .shapes
                .iter()
                .filter_map(|s| match &s.shape {
                    egui::Shape::Text(t) => Some(t.galley.text().to_owned()),
                    _ => None,
                })
                .collect();
        }
        let rect = app
            .asset_panel
            .as_ref()
            .and_then(|p| p.rect)
            .expect("drawn");
        (rect, texts)
    }

    /// The panel keeps one size whatever it holds — an empty tab, a grid, the
    /// other tab, with a service's controls or without — as the insert picker
    /// does: an egui popup that shrinks never grows back. With a set loaded, its
    /// grid and foot show.
    /// flip: `ORIENT_W` back at 118 → fails at "a service's controls fit".
    #[test]
    fn the_panel_keeps_one_size_whatever_it_holds() {
        let ctx = egui::Context::default();
        let mut app = OndinApp::headless(&ctx);
        app.canvas_rect =
            egui::Rect::from_min_size(egui::pos2(46.0, 40.0), egui::vec2(1000.0, 700.0));
        app.toggle_assets();
        let (empty, _) = panel_rect(&mut app, &ctx);

        app.prefs.assets.icon_sets = vec!["phosphor".into()];
        app.prefs.assets.set = "phosphor".into();
        let names: Vec<String> = (0..40).map(|i| format!("icon-{i:02}")).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        app.assets
            .sets
            .insert("phosphor", SetState::Ready(set_with(&names)));
        let (grid, texts) = panel_rect(&mut app, &ctx);
        assert!(texts.iter().any(|t| t == "Phosphor · MIT"), "{texts:?}");
        assert!(texts.iter().any(|t| t == "40 icons"), "{texts:?}");

        // The longest variant label there is, cut rather than widening the panel.
        app.prefs.assets.icon_sets = vec!["material-symbols".into()];
        app.prefs.assets.set = "material-symbols".into();
        app.prefs
            .assets
            .variants
            .insert("material-symbols".into(), "Outlined rounded".into());
        app.assets
            .sets
            .insert("material-symbols", SetState::Ready(set_with(&names)));
        let (long_variant, _) = panel_rect(&mut app, &ctx);
        assert_eq!(long_variant.size(), grid.size(), "a long variant is cut");

        app.prefs.assets.photos_tab = true;
        let (photos, _) = panel_rect(&mut app, &ctx);
        app.prefs.assets.photo_services = vec!["openverse".into()];
        let (searchable, texts) = panel_rect(&mut app, &ctx);
        assert!(
            texts.iter().any(|t| t == "Search Openverse for photos"),
            "the fixture: the service's controls show: {texts:?}"
        );
        assert_eq!(empty.size(), grid.size());
        assert_eq!(grid.size(), photos.size());
        assert_eq!(photos.size(), searchable.size(), "a service's controls fit");
        // The frame's hairline each side, as the insert picker's.
        assert_eq!(grid.width(), W + 2.0);
    }

    /// A CC BY set's icon owes its credit; an MIT one keeps it as provenance.
    #[test]
    fn only_a_cc_by_icon_owes_its_credit() {
        let fa = icon_credit(SetSpec::by_key("font-awesome").unwrap(), "2026-11-08");
        assert_eq!(fa.text, "Icons by Font Awesome, CC BY 4.0");
        assert!(fa.required);
        assert!(fa.is_well_formed());
        let ph = icon_credit(SetSpec::by_key("phosphor").unwrap(), "2026-11-08");
        assert!(!ph.required);
    }
}
