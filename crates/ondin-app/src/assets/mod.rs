//! The assets browser's providers: open-source icon sets and stock-photo services.
//!
//! Nothing here touches the network until a set or a service is switched on in
//! Settings. An icon set is downloaded once, whole, the first time it is needed,
//! and kept in the machine's cache folder beside the web fonts, so it works
//! offline afterwards. Photos are searched online each time, and their thumbnails
//! are kept for the session only. Every request runs on [`WORKERS`] threads; the
//! frame loop reads what landed in [`Assets::pass`].

pub(crate) mod panel;
pub(crate) mod photos;
pub(crate) mod sets;

use crate::kits::index::Pixels;
use photos::{Page, Photo, PhotoError, PhotoSize, ServiceSpec};
use sets::{IconSet, SetSpec};
use std::collections::HashMap;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};

/// Where an Unsplash download is reported: the one address its key goes to
/// besides the search.
const UNSPLASH_API: &str = "https://api.unsplash.com/";
/// How many requests run at once.
const WORKERS: usize = 4;
/// The most a set's package or a photo may be, in bytes.
const MAX_BYTES: u64 = 64 * 1024 * 1024;
/// The widest a thumbnail is kept, in pixels: two columns of a 340-point panel
/// at twice the scale.
const THUMB_MAX_W: u32 = 320;

/// Where an icon set stands.
pub(crate) enum SetState {
    /// Being read from the cache or downloaded: bytes so far and, when the
    /// server says, of how many.
    Loading {
        done: u64,
        total: Option<u64>,
    },
    Ready(Arc<IconSet>),
    /// Neither in the cache nor reachable, or not readable; tried again on ask.
    Failed,
}

/// A tile picture: asked for, drawn, or not drawable.
enum Tile {
    Pending,
    Ready(egui::TextureHandle),
    Failed,
}

/// What a worker hands back.
enum Done {
    Progress {
        set: &'static str,
        done: u64,
        total: Option<u64>,
    },
    Set {
        set: &'static str,
        read: Result<Arc<IconSet>, String>,
    },
    IconTile {
        key: String,
        pixels: Option<Pixels>,
    },
    Thumb {
        url: String,
        pixels: Option<Pixels>,
    },
    Photos {
        token: u64,
        page: Result<Page, PhotoError>,
    },
    Fetched {
        token: u64,
        reads: Vec<Fetched>,
    },
    CacheSize(u64),
}

type Work = Box<dyn FnOnce() -> Done + Send>;

/// The frame's context as the workers reach it ([`Assets::pass`] fills it).
type Waker = Arc<Mutex<Option<egui::Context>>>;

/// Ask for a frame, once there is a context to ask.
fn wake(ctx: &Waker) {
    if let Some(c) = ctx.lock().ok().and_then(|g| g.clone()) {
        c.request_repaint();
    }
}

/// One photo fetched for an insert: the picture and its pixels, or why not.
pub(crate) type Fetched = Result<(crate::app::LoadedImage, ondin_core::peniko::ImageData), String>;

/// What a fetched photo is for.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum PhotoIntent {
    /// A new layer, centred on `at` (world), or mid-view.
    Place {
        at: Option<ondin_core::kurbo::Point>,
    },
    /// The image fill of these layers, the selection when it was clicked.
    Fill { targets: Vec<ondin_core::NodeId> },
}

/// Photos being fetched for one insert, and what to do with them once they land.
pub(crate) struct Fetch {
    pub(crate) token: u64,
    pub(crate) photos: Vec<Photo>,
    pub(crate) intent: PhotoIntent,
}

/// The providers' state, shared by the panel and Settings.
pub(crate) struct Assets {
    jobs: Option<mpsc::Sender<Work>>,
    done: mpsc::Receiver<Done>,
    done_tx: mpsc::Sender<Done>,
    /// What a worker wakes the frame through: filled by the first `pass`, read
    /// at each wake, so a worker started before any frame still wakes one.
    ctx: Waker,
    pub(crate) sets: HashMap<&'static str, SetState>,
    tiles: HashMap<String, Tile>,
    thumbs: HashMap<String, Tile>,
    /// The latest photo search's token: an answer to an older one is dropped.
    token: u64,
    /// The latest search's answer, `None` while it runs.
    pub(crate) photos: Option<Result<Page, PhotoError>>,
    /// A later page is asked for and has not landed: the shown page stays, and no
    /// other is asked meanwhile.
    more_asked: bool,
    /// The photos being fetched for an insert; a second click replaces them.
    pub(crate) fetch: Option<Fetch>,
    fetch_seq: u64,
    /// Fetched photos waiting for the app to place them, one read per photo.
    pub(crate) landed: Option<(Fetch, Vec<Fetched>)>,
    cache_bytes: Option<u64>,
    /// The panel's choices changed and are not yet written to the preferences.
    pub(crate) prefs_dirty: bool,
    /// Off the network, for tests: nothing is spawned.
    offline: bool,
}

impl Default for Assets {
    fn default() -> Self {
        let (done_tx, done) = mpsc::channel();
        Self {
            jobs: None,
            done,
            done_tx,
            ctx: Waker::default(),
            sets: HashMap::new(),
            tiles: HashMap::new(),
            thumbs: HashMap::new(),
            token: 0,
            photos: None,
            more_asked: false,
            fetch: None,
            fetch_seq: 0,
            landed: None,
            cache_bytes: None,
            prefs_dirty: false,
            offline: cfg!(test),
        }
    }
}

/// The folder the icon sets are kept in.
pub(crate) fn cache_dir() -> PathBuf {
    crate::machine_dir::root_or_temp().join("assets")
}

/// The one agent every request goes through, with the font service's deadlines
/// (`fonts::agent`): `ureq`'s own default has none, so a stalled server would
/// hold a worker for the session.
fn agent() -> ureq::Agent {
    use std::time::Duration;
    ureq::config::Config::builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(60)))
        .timeout_global(Some(Duration::from_secs(120)))
        .http_status_as_error(false)
        .build()
        .new_agent()
}

/// `url`'s body, at most [`MAX_BYTES`], `progress` called as it arrives with the
/// bytes so far and the length the server declared. A status of 400 or more is
/// `Err(Some(status))`; the network is `Err(None)`.
fn get(
    agent: &ureq::Agent,
    url: &str,
    auth: Option<&str>,
    mut progress: impl FnMut(u64, Option<u64>),
) -> Result<Vec<u8>, Option<u16>> {
    let mut req = agent.get(url);
    if let Some(a) = auth {
        req = req.header("Authorization", a);
    }
    if url.starts_with("https://api.unsplash.com") {
        req = req.header("Accept-Version", "v1");
    }
    let mut resp = req.call().map_err(|_| None)?;
    let status = resp.status().as_u16();
    if status >= 400 {
        return Err(Some(status));
    }
    let total = resp
        .headers()
        .get("content-length")
        .and_then(|v| v.to_str().ok()?.parse::<u64>().ok());
    let mut reader = resp.body_mut().with_config().limit(MAX_BYTES).reader();
    let mut out = Vec::new();
    let mut buf = [0u8; 64 * 1024];
    let mut last = 0;
    loop {
        let n = reader.read(&mut buf).map_err(|_| None)?;
        if n == 0 {
            break;
        }
        out.extend_from_slice(&buf[..n]);
        let so_far = out.len() as u64;
        // Every quarter megabyte: a message per read would flood the frame.
        if so_far - last >= 256 * 1024 {
            last = so_far;
            progress(so_far, total);
        }
    }
    Ok(out)
}

/// Read a set from the cache, downloading each package that is not there.
fn load_set(
    agent: &ureq::Agent,
    dir: &Path,
    spec: &'static SetSpec,
    tx: &mpsc::Sender<Done>,
    ctx: &Waker,
) -> Result<Arc<IconSet>, String> {
    let mut jsons = Vec::new();
    let mut before = 0;
    for package in spec.packages {
        let path = dir.join(package.file_name());
        let bytes = match std::fs::read(&path) {
            Ok(b) => b,
            Err(_) => {
                let bytes = get(agent, &package.url(), None, |done, total| {
                    let _ = tx.send(Done::Progress {
                        set: spec.key,
                        done: before + done,
                        total: total.map(|t| before + t),
                    });
                    wake(ctx);
                })
                .map_err(|_| format!("Can't reach {}", spec.name))?;
                // Written beside and renamed, so a quit mid-write leaves no
                // half a package to be read as whole next time.
                let _ = std::fs::create_dir_all(dir);
                let tmp = path.with_extension("part");
                if std::fs::write(&tmp, &bytes).is_ok() {
                    let _ = std::fs::rename(&tmp, &path);
                }
                bytes
            }
        };
        before += bytes.len() as u64;
        jsons.push(bytes);
    }
    let read = sets::read_set(spec, &jsons);
    if read.is_err() {
        // A package that will not read is fetched again next time.
        for package in spec.packages {
            let _ = std::fs::remove_file(dir.join(package.file_name()));
        }
    }
    read.map(Arc::new)
}

/// A picture's bytes decoded and shrunk to at most [`THUMB_MAX_W`] wide.
fn thumb_pixels(bytes: &[u8]) -> Option<Pixels> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = match img.width() > THUMB_MAX_W {
        true => img.resize(THUMB_MAX_W, u32::MAX, image::imageops::FilterType::Triangle),
        false => img,
    };
    let rgba = img.to_rgba8();
    Some(Pixels {
        width: rgba.width(),
        height: rgba.height(),
        rgba: rgba.into_raw(),
    })
}

/// An icon's markup drawn in white at `px` pixels, for the panel to tint: read
/// through the SVG importer, so the tile is what an insert makes.
pub(crate) fn icon_pixels(svg: &str) -> Option<Pixels> {
    let mut doc = ondin_core::Document::new(ondin_core::IdSource::new(1).mint());
    let page = doc.first_page();
    let mut ids = ondin_core::IdSource::new(2);
    let out = ondin_core::svg_in::import(svg, &mut ids, page, 0, None).ok()?;
    doc.apply(&out.tx).ok()?;
    let res = ondin_core::Resolved::rebuild(&doc);
    let size = out.size?;
    crate::kits::index::raster_fit(
        &doc,
        &res,
        out.root,
        size.width,
        size.height,
        1.0,
        &ondin_render::ImageStore::new(),
    )
}

/// The size of everything under `dir`, in bytes.
fn dir_bytes(dir: &Path) -> u64 {
    std::fs::read_dir(dir)
        .map(|rd| {
            rd.flatten()
                .filter_map(|e| e.metadata().ok())
                .filter(|m| m.is_file())
                .map(|m| m.len())
                .sum()
        })
        .unwrap_or(0)
}

impl Assets {
    /// Hand `work` to the pool, starting it on the first call.
    fn run(&mut self, work: Work) {
        if self.offline {
            return;
        }
        let jobs = self.jobs.get_or_insert_with(|| {
            let (tx, rx) = mpsc::channel::<Work>();
            let rx = Arc::new(Mutex::new(rx));
            for i in 0..WORKERS {
                let rx = Arc::clone(&rx);
                let done = self.done_tx.clone();
                let ctx = Arc::clone(&self.ctx);
                let _ = std::thread::Builder::new()
                    .name(format!("ondin-assets-{i}"))
                    .spawn(move || {
                        loop {
                            let next = rx.lock().ok().and_then(|r| r.recv().ok());
                            let Some(work) = next else { break };
                            let _ = done.send(work());
                            wake(&ctx);
                        }
                    });
            }
            tx
        });
        let _ = jobs.send(work);
    }

    /// Take what the workers handed back. Called once a frame.
    pub(crate) fn pass(&mut self, ctx: &egui::Context) {
        if let Ok(mut slot) = self.ctx.lock()
            && slot.is_none()
        {
            *slot = Some(ctx.clone());
        }
        while let Ok(done) = self.done.try_recv() {
            match done {
                Done::Progress { set, done, total } => {
                    if let Some(SetState::Loading { .. }) = self.sets.get(set) {
                        self.sets.insert(set, SetState::Loading { done, total });
                    }
                }
                Done::Set { set, read } => {
                    self.sets.insert(
                        set,
                        match read {
                            Ok(s) => SetState::Ready(s),
                            Err(e) => {
                                log::info!(target: "ondin", "icon set {set}: {e}");
                                SetState::Failed
                            }
                        },
                    );
                    self.cache_bytes = None;
                }
                Done::IconTile { key, pixels } => {
                    let tile = match pixels {
                        Some(p) => Tile::Ready(p.texture(ctx, format!("asset-icon-{key}"))),
                        None => Tile::Failed,
                    };
                    self.tiles.insert(key, tile);
                }
                Done::Thumb { url, pixels } => {
                    let tile = match pixels {
                        Some(p) => Tile::Ready(p.texture(ctx, format!("asset-thumb-{url}"))),
                        None => Tile::Failed,
                    };
                    self.thumbs.insert(url, tile);
                }
                Done::Photos { token, page } => {
                    if token == self.token {
                        let later = std::mem::take(&mut self.more_asked);
                        self.photos = Some(match (self.photos.take(), page) {
                            // A later page joins the one shown, less any photo the
                            // provider's drift repeats; one that failed or came back
                            // empty ends the paging and leaves the grid as it was.
                            (Some(Ok(shown)), next) if later => Ok(joined(shown, next)),
                            (_, page) => page,
                        });
                    }
                }
                Done::Fetched { token, reads } => {
                    if let Some(f) = self.fetch.take_if(|f| f.token == token) {
                        self.landed = Some((f, reads));
                    }
                }
                Done::CacheSize(n) => self.cache_bytes = Some(n),
            }
        }
    }

    /// The set by key, asked for if it is not loaded or loading: read from the
    /// cache, or downloaded once.
    pub(crate) fn set(&mut self, spec: &'static SetSpec) -> &SetState {
        if !self.sets.contains_key(spec.key) {
            self.sets.insert(
                spec.key,
                SetState::Loading {
                    done: 0,
                    total: None,
                },
            );
            let tx = self.done_tx.clone();
            let ctx = Arc::clone(&self.ctx);
            self.run(Box::new(move || Done::Set {
                set: spec.key,
                read: load_set(&agent(), &cache_dir(), spec, &tx, &ctx),
            }));
        }
        &self.sets[spec.key]
    }

    /// Try a failed set again.
    pub(crate) fn retry_set(&mut self, key: &str) {
        if matches!(self.sets.get(key), Some(SetState::Failed)) {
            self.sets.remove(key);
        }
    }

    /// An icon's tile, drawn white for the panel to tint, asked for once.
    pub(crate) fn icon_tile(
        &mut self,
        key: String,
        svg: impl FnOnce() -> String,
    ) -> Option<&egui::TextureHandle> {
        if !self.tiles.contains_key(&key) {
            self.tiles.insert(key.clone(), Tile::Pending);
            let svg = svg();
            self.run(Box::new(move || Done::IconTile {
                pixels: icon_pixels(&svg),
                key,
            }));
            return None;
        }
        match self.tiles.get(&key) {
            Some(Tile::Ready(t)) => Some(t),
            _ => None,
        }
    }

    /// A photo's thumbnail, fetched once this session.
    pub(crate) fn thumb(&mut self, url: &str) -> Option<&egui::TextureHandle> {
        if !self.thumbs.contains_key(url) {
            self.thumbs.insert(url.to_string(), Tile::Pending);
            let url = url.to_string();
            self.run(Box::new(move || Done::Thumb {
                pixels: get(&agent(), &url, None, |_, _| {})
                    .ok()
                    .and_then(|b| thumb_pixels(&b)),
                url,
            }));
            return None;
        }
        match self.thumbs.get(url) {
            Some(Tile::Ready(t)) => Some(t),
            _ => None,
        }
    }

    /// Search `service` for `query`, page `page`: the first page replaces what is
    /// shown, a later one joins it. The key is read from the keychain on the worker,
    /// and sent to its own service only.
    pub(crate) fn search_photos(
        &mut self,
        service: &'static ServiceSpec,
        query: &str,
        page: u32,
        orientation: photos::Orientation,
    ) {
        if page <= 1 {
            self.token += 1;
            self.photos = None;
        }
        self.more_asked = page > 1;
        let token = self.token;
        let query = query.to_string();
        self.run(Box::new(move || {
            let key = service
                .key_label
                .and_then(|_| crate::keychain::read_account(&service.account()));
            let page = photos::request(service, &query, page, orientation, key.as_deref())
                .and_then(|(url, auth)| {
                    let body = get(&agent(), &url, auth.as_deref(), |_, _| {})
                        .map_err(|e| e.map_or(PhotoError::Unreachable, photos::status_error))?;
                    photos::read_page(service, &body, page).ok_or(PhotoError::Unreachable)
                });
            Done::Photos { token, page }
        }));
    }

    /// Whether a search is in flight.
    pub(crate) fn searching(&self) -> bool {
        (self.photos.is_none() && self.token > 0) || self.more_asked
    }

    /// Fetch `photos` at `size` for one insert, then report each download to its
    /// service when it asks for that (Unsplash): at the insert, never on preview.
    /// A fetch asked for meanwhile replaces this one, whose answer is dropped.
    pub(crate) fn fetch_photos(
        &mut self,
        photos: Vec<Photo>,
        size: PhotoSize,
        intent: PhotoIntent,
    ) {
        self.fetch_seq += 1;
        let token = self.fetch_seq;
        let jobs: Vec<(String, Option<String>, Option<&'static ServiceSpec>, String)> = photos
            .iter()
            .map(|p| {
                let url = p.urls[match size {
                    PhotoSize::Small => 0,
                    PhotoSize::Regular => 1,
                    PhotoSize::Full => 2,
                }]
                .clone();
                (
                    url,
                    p.report_url.clone(),
                    ServiceSpec::by_key(p.service),
                    p.layer_name(),
                )
            })
            .collect();
        self.fetch = Some(Fetch {
            token,
            photos,
            intent,
        });
        self.run(Box::new(move || {
            let agent = agent();
            let reads = jobs
                .into_iter()
                .map(|(url, report, service, name)| {
                    let read = get(&agent, &url, None, |_, _| {})
                        .map_err(|_| "The photo could not be downloaded".to_string())
                        .and_then(|bytes| {
                            crate::app::read_picture(bytes, None, Some(name))
                                .map_err(|_| "The photo could not be read".to_string())
                        });
                    // The address comes from the API's answer: the key goes with
                    // it only to the service's own API, over https.
                    if read.is_ok()
                        && let (Some(report), Some(service)) = (report, service)
                        && service.key == "unsplash"
                        && report.starts_with(UNSPLASH_API)
                    {
                        let key = crate::keychain::read_account(&service.account());
                        let auth = key.map(|k| format!("Client-ID {}", k.trim()));
                        let _ = get(&agent, &report, auth.as_deref(), |_, _| {});
                    }
                    read
                })
                .collect();
            Done::Fetched { token, reads }
        }));
    }

    /// The cache's size, counted once after it was asked for or changed; `None`
    /// until the count lands.
    pub(crate) fn cache_bytes(&mut self) -> Option<u64> {
        if self.cache_bytes.is_none() && !self.offline {
            self.cache_bytes = Some(u64::MAX);
            self.run(Box::new(|| Done::CacheSize(dir_bytes(&cache_dir()))));
        }
        self.cache_bytes.filter(|n| *n != u64::MAX)
    }

    /// Empty the cache: every downloaded set goes, and is downloaded again when
    /// next searched. Returns how many bytes went.
    pub(crate) fn clear_cache(&mut self) -> u64 {
        let dir = cache_dir();
        let freed = dir_bytes(&dir);
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let _ = std::fs::remove_file(e.path());
            }
        }
        self.sets.clear();
        self.tiles.clear();
        self.cache_bytes = Some(0);
        freed
    }
}

/// `shown` with a later page's answer joined on: its new photos appended (one
/// already shown is not shown twice, which would also give two tiles one id),
/// paging on while it brought something; an error or an empty page stops it.
fn joined(mut shown: Page, next: Result<Page, PhotoError>) -> Page {
    match next {
        Ok(next) => {
            let fresh: Vec<Photo> = next
                .photos
                .into_iter()
                .filter(|p| shown.photos.iter().all(|s| s.id != p.id))
                .collect();
            shown.more = next.more && !fresh.is_empty();
            shown.photos.extend(fresh);
        }
        Err(_) => shown.more = false,
    }
    shown
}

#[cfg(test)]
mod tests {
    use super::*;

    /// While a later page is in flight the search reads as busy, so the grid's
    /// end in view asks for one page, not one a frame; its landing joins the
    /// shown page and frees the next ask.
    /// flip: drop the `more_asked` term in `Assets::searching` → fails at the
    /// first busy assertion.
    #[test]
    fn one_later_page_is_asked_at_a_time() {
        let ctx = egui::Context::default();
        let mut a = Assets::default();
        let svc = ServiceSpec::by_key("openverse").unwrap();
        let photo = |id: &str| Photo {
            service: "openverse",
            id: id.into(),
            size: None,
            thumb: String::new(),
            urls: Default::default(),
            title: String::new(),
            author: String::new(),
            author_url: String::new(),
            page_url: String::new(),
            licence: String::new(),
            licence_url: String::new(),
            report_url: None,
        };
        a.search_photos(svc, "coffee", 1, photos::Orientation::Any);
        let first = Page {
            photos: vec![photo("a")],
            total: 2,
            more: true,
        };
        let _ = a.done_tx.send(Done::Photos {
            token: a.token,
            page: Ok(first),
        });
        a.pass(&ctx);
        assert!(!a.searching(), "the fixture: the first page landed");
        a.search_photos(svc, "coffee", 2, photos::Orientation::Any);
        assert!(a.searching(), "a later page in flight is a search");
        // The provider's drift repeats "a": it is not shown twice.
        let next = Page {
            photos: vec![photo("a"), photo("b")],
            total: 3,
            more: true,
        };
        let _ = a.done_tx.send(Done::Photos {
            token: a.token,
            page: Ok(next),
        });
        a.pass(&ctx);
        assert!(!a.searching());
        let Some(Ok(shown)) = &a.photos else {
            panic!("a page is shown");
        };
        let ids: Vec<&str> = shown.photos.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(ids, ["a", "b"], "the later page joined, its repeat dropped");
        assert!(shown.more);

        // A later page refused (an hourly limit) keeps the grid and stops paging.
        a.search_photos(svc, "coffee", 3, photos::Orientation::Any);
        let _ = a.done_tx.send(Done::Photos {
            token: a.token,
            page: Err(PhotoError::Limited),
        });
        a.pass(&ctx);
        let Some(Ok(shown)) = &a.photos else {
            panic!("the grid survives a failed later page");
        };
        assert_eq!(shown.photos.len(), 2);
        assert!(!shown.more);
    }

    /// A thumbnail wider than the panel needs is shrunk, keeping its aspect.
    #[test]
    fn a_wide_thumbnail_is_shrunk_to_the_panel() {
        let png = crate::app::rgba_to_png(&vec![200u8; 800 * 400 * 4], (800, 400)).unwrap();
        let p = thumb_pixels(&png).unwrap();
        assert_eq!((p.width, p.height), (THUMB_MAX_W, THUMB_MAX_W / 2));
    }

    /// An icon draws through the SVG importer at its own size.
    #[test]
    fn an_icon_tile_is_drawn_at_its_size() {
        let svg = r##"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24" viewBox="0 0 24 24" color="#ffffff"><rect x="2" y="2" width="20" height="20" fill="currentColor"/></svg>"##;
        let p = icon_pixels(svg).unwrap();
        assert!((19..=22).contains(&p.width), "{}", p.width);
        assert!(p.rgba.chunks(4).any(|px| px == [255, 255, 255, 255]));
    }
}
