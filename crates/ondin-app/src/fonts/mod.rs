//! Font discovery + on-demand loading for the app (§5.4a).
//!
//! Core (`ondin-core`) shapes text but never touches the filesystem or network;
//! it only knows the bundled Inter plus whatever bytes it's handed via
//! `ondin_core::text::register_fonts`. This service is the app-side supplier:
//!
//! - **System fonts** — enumerated via `fontdb` on a background thread.
//! - **Web fonts** — the Google Fonts library, delivered as plain files by
//!   Fontsource over the jsDelivr CDN. The catalog is cached on disk and
//!   refreshed in the background; a family's files download on demand.
//!   **Switchable, and off means no request** — see [`FontService::set_web_fonts`].
//!
//! All I/O happens here, never in core. Registration into core is per-thread, so
//! bytes are registered on the UI thread in [`FontService::poll`]. Until a family
//! arrives, text renders with the Inter fallback.
//!
//! ## What makes it feel instant
//!
//! 1. **Nothing blocks the UI thread.** Even a system font is read on a worker;
//!    only `register_fonts` runs on the UI thread, because the shaping engine is
//!    thread-local. The `fontdb` scan is a background thread too.
//! 2. **One [`ureq::Agent`], shared.** `ureq`'s free functions build a use-once
//!    agent, so every file would pay a fresh DNS lookup, connect and TLS handshake.
//! 3. **The primary face is dispatched first and registered on its own**, so text
//!    swaps to the real typeface when the first file lands, not the last.
//! 4. **Previewing is prefetching.** The picker draws each row in its own family
//!    (§9.2), so the regular face applying the family needs is already registered
//!    by the time the user clicks.
//!
//! Because (4) registers font data for every family the user scrolls past, the
//! service must also *let go* — see [`FontService::trim_previews`].

use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, channel};
use std::time::SystemTime;

mod preview;
mod queue;

pub use preview::FontPreviews;
use queue::{Job, Queue};

const FONTSOURCE_LIST: &str = "https://api.fontsource.org/v1/fonts";
const FONTSOURCE_VARIABLE: &str = "https://api.fontsource.org/v1/variable";
const FONTSOURCE_CDN: &str = "https://cdn.jsdelivr.net/fontsource/fonts";

/// How many faces may be in flight at once. Six is enough to keep a screenful of
/// preview rows filling in without turning a scroll into a thundering herd.
const WORKERS: usize = 6;

/// Most files one family will fetch. A guard on the pathological case (a static
/// family offering nine weights in two styles), not a considered budget.
const MAX_FACES: usize = 24;

/// How much preview-only font data stays registered, in bytes.
///
/// Bytes rather than families: a face is tens of KB for a Latin subset and
/// megabytes for a CJK one, so a count bounds nothing. 48 MB keeps a few hundred
/// Latin families registered while a CJK browse settles around twenty.
const PREVIEW_BYTES: usize = 48 * 1024 * 1024;

/// Families kept whatever they weigh.
///
/// A floor, not a second cap: the anti-thrash guard. A byte budget alone evicts
/// on-screen families once a few heavy ones fill it, and the picker would re-fetch
/// them every frame. Just above a screenful (about twenty rows). Kept small
/// because a generous floor would override [`PREVIEW_BYTES`] for CJK families.
const PREVIEW_FLOOR: usize = 24;

/// How much downloaded font data stays on disk, in bytes.
///
/// The disk twin of [`PREVIEW_BYTES`], counted in bytes for the same reason.
///
/// 256 MB leaves the whole Google Latin catalog (about 85 MB) untouched; it exists
/// for CJK browsing. Above it the oldest files go. A pruned file costs one
/// re-download: the cache is derived data.
const FONT_CACHE_BYTES: u64 = 256 * 1024 * 1024;

/// The families warmed at startup so the picker is worth opening on a machine
/// with no network (§15 D352).
///
/// An editorial list: the Fontsource endpoint carries no rank or popularity, and
/// its order is alphabetical. Only the regular upright of each is fetched (about
/// 7 MB), evictable like any other preview. `Inter` is absent because core
/// registers it itself.
///
/// A misspelling fails silently (`faces_of` finds no entry and the family is never
/// warmed). The `#[ignore]`d test
/// `every_prefetched_family_is_spelled_the_way_the_catalog_spells_it` checks the
/// names against the live endpoint; run `cargo test -p ondin-app -- --ignored`
/// after editing this list, since Fontsource renames families.
const POPULAR: &[&str] = &[
    // Sans workhorses — the ones a body-text pick lands on.
    "Roboto",
    "Open Sans",
    "Noto Sans",
    "Lato",
    "Montserrat",
    "Poppins",
    "Source Sans 3",
    "Raleway",
    "Nunito",
    "Nunito Sans",
    "Ubuntu",
    "Rubik",
    "Work Sans",
    "Mulish",
    "PT Sans",
    "Karla",
    "Manrope",
    "DM Sans",
    "Barlow",
    "Fira Sans",
    "IBM Plex Sans",
    "Figtree",
    // Newer UI sans, heavily used in product design.
    "Space Grotesk",
    "Outfit",
    "Plus Jakarta Sans",
    "Sora",
    "Urbanist",
    "Public Sans",
    // Display and condensed — headlines rather than paragraphs.
    "Oswald",
    "Anton",
    "Bebas Neue",
    "Archivo",
    "Josefin Sans",
    // Serifs.
    "Merriweather",
    "Playfair Display",
    "Lora",
    "PT Serif",
    "Libre Baskerville",
    "EB Garamond",
    "Cormorant Garamond",
    "Crimson Text",
    "Bitter",
    "Roboto Slab",
    // Monospace.
    "JetBrains Mono",
    "Fira Code",
    "Roboto Mono",
    "Source Code Pro",
    "IBM Plex Mono",
    // Rounded and soft.
    "Quicksand",
    "Cabin",
];

/// How long a written `catalog.json` is trusted before the network is asked for a
/// fresh one. The gate is in [`spawn_catalog`]; Google adds families monthly.
const CATALOG_MAX_AGE: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 60 * 60);

/// How much of a family to load.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
enum Want {
    /// Just the regular upright — enough to draw a picker row. Evictable.
    #[default]
    Preview,
    /// Every cut, because a document or a user's pick depends on it.
    Full,
}

/// Which of [`queue::Queue`]'s three lanes a request goes down.
///
/// Separate from [`Want`] (§15 D352): `Want` says how much of a family to fetch,
/// `Lane` how badly it is wanted. The prefetch asks for `Want::Preview`'s face yet
/// must be served after every picker row.
///
/// **Declaration order is priority order, most urgent first**, and `Ord` is derived
/// so `min` is the more urgent of two — how [`FamilyState::lane`] is promoted and
/// never downgraded. Reordering the variants reverses that.
///
/// **The default is the weakest lane**, being the identity for that `min`: a fresh
/// [`FamilyState`] must not out-rank the request that created it, or every
/// prefetch would come out as `Preview`.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Debug)]
enum Lane {
    /// Something is waiting: a document's font, or a family the user picked.
    Urgent,
    /// A picker row on screen, which may scroll away before a worker frees up.
    Preview,
    /// Nothing has asked yet — the popular set, warmed at startup.
    #[default]
    Prefetch,
}

/// What one [`FontService::poll`] found. A longer family list is a repaint, but
/// only font data invalidates the shaped-text cache (§5.9). Not `Copy`, because
/// [`Self::registered`] carries the families.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct FontPoll {
    /// The picker's list changed.
    pub families_changed: bool,
    /// Font data was registered with core, so **something** on screen may draw
    /// differently — a picker row's own preview strip, at least.
    pub fonts_registered: bool,
    /// Which families that data belongs to, lowercased (this module's key form).
    ///
    /// `fonts_registered` alone is too coarse for the document's cache: preview
    /// faces land continuously while the picker scrolls, and each arrival would
    /// re-shape every text node for families the document does not name.
    pub registered: Vec<String>,
}

impl FontPoll {
    pub fn any(&self) -> bool {
        self.families_changed || self.fonts_registered
    }

    /// Whether any family in [`Self::registered`] is one of `wanted` — the
    /// question `Resolved`'s text cache should be invalidated on.
    ///
    /// `wanted` is expected lowercased, as [`Self::registered`] is.
    pub fn touches(&self, wanted: &std::collections::HashSet<String>) -> bool {
        self.registered.iter().any(|f| wanted.contains(f))
    }
}

/// One web-font family from the Fontsource catalog.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct CatalogEntry {
    id: String,
    family: String,
    weights: Vec<u16>,
    styles: Vec<String>,
    subset: String,
    /// The axis-set name of the family's variable file, when it ships one —
    /// `wght`, `standard` or `full`; see [`variable_cut`]. `None` means the
    /// static cuts are all there is.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    cut: Option<String>,
    /// Whether the variable file has an italic companion (an `ital` axis). A
    /// family that slants through `slnt` does not: the one file covers both.
    #[serde(default)]
    vf_italic: bool,
}

/// Where one face's bytes come from.
#[derive(Clone)]
enum FaceSource {
    /// A file already on this machine — a system font.
    File(PathBuf),
    /// A Fontsource file, cached at `cache` once it lands. Stored decoded, so a
    /// woff2 is decompressed once and never again.
    Web { url: String, cache: PathBuf },
}

/// One font file to load.
#[derive(Clone)]
struct FaceSpec {
    /// Identity, for de-duplication: registering the same blob twice adds a
    /// second identical face to the family, which then shows up as a duplicate
    /// row in the variant list.
    key: String,
    source: FaceSource,
    /// Whether this is a variable file. The one failure the service reacts to:
    /// if the variable file cannot be decoded, the static cuts are tried.
    variable: bool,
}

/// A face's bytes, or the news that they could not be had.
enum FontMsg {
    Face {
        family: String,
        key: String,
        bytes: Vec<u8>,
    },
    Failed {
        family: String,
        key: String,
        variable: bool,
        reason: FaceFailure,
    },
}

/// Why a face did not load — the distinction the variable-family fallback turns on.
///
/// §5.4a's static-cut fallback is about the decoder, not the network. A transport
/// failure leaves `cut` alone, so the next `ensure` asks for the variable file
/// again instead of demoting the family to static for the session.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum FaceFailure {
    /// The bytes never arrived: a download that failed, a cache file that could
    /// not be read. Says nothing about the face and must not be treated as a
    /// verdict on it.
    Fetch,
    /// The bytes arrived and are not a font this build can read — a woff2 the
    /// decoder refused or panicked on, or something that is not sfnt at all.
    /// **This** is §5.4a's case.
    Decode,
}

/// The result of the background `fontdb` scan.
struct SystemScan {
    db: fontdb::Database,
    families: BTreeSet<String>,
}

/// What the app can honestly say about one family (§15 D227).
///
/// `text::is_family_available` answers "not here yet" and "never coming"
/// identically, so a warning built on it would accuse a font still downloading.
/// The boundary that matters is [`Self::Pending`] against [`Self::Missing`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FamilyStatus {
    /// Registered: core can shape and draw with it now.
    Ready,
    /// Not drawable **yet**: a face is in flight, a source that could still list
    /// the family has not landed, or nothing has asked for it.
    Pending,
    /// Not drawable **ever**: every source has landed and none lists this family,
    /// or every face tried came back unusable. Earns `theme::color::WARN`.
    Missing,
}

/// The facts [`FontService::family_status`] weighs.
///
/// A struct rather than five positional `bool`s, so [`status_of`] is a truth table
/// a test can walk without threads, a disk cache or a network.
#[derive(Clone, Copy, Debug)]
struct FamilyFacts {
    /// `text::is_family_available` — core has faces for it.
    drawable: bool,
    /// A face has been dispatched and not reported back.
    in_flight: bool,
    /// Both things a family can be resolved against have finished arriving: the
    /// system scan and the web catalog. Until then, *nothing* about a family that
    /// is not already drawable is a settled fact.
    sources_settled: bool,
    /// Some source offers this family, so asking would fetch something.
    listed: bool,
    /// At least one face was tried and came back unusable, and none registered.
    tried_and_failed: bool,
}

/// The rule behind [`FontService::family_status`]. See [`FamilyStatus`].
fn status_of(f: FamilyFacts) -> FamilyStatus {
    if f.drawable {
        return FamilyStatus::Ready;
    }
    // `!sources_settled` outranks everything below it: a family the catalog has not
    // been read for is unknown, not missing.
    if f.in_flight || !f.sources_settled {
        return FamilyStatus::Pending;
    }
    if f.tried_and_failed || !f.listed {
        return FamilyStatus::Missing;
    }
    // Listed, settled, nothing tried: nobody has asked yet. Not a warning — asking
    // would fetch it — and not drawable either, so it is not `Ready`.
    FamilyStatus::Pending
}

/// What the service has done about one family.
#[derive(Default)]
struct FamilyState {
    /// Files registered with core for this family, by [`FaceSpec::key`].
    faces: Vec<String>,
    /// Faces dispatched to the pool and not yet reported back.
    outstanding: usize,
    /// Bytes of font data registered for this family — what
    /// [`FontService::trim_previews`] budgets.
    ///
    /// Summed as each face arrives (core does not report what it kept), so this
    /// is the decoded file size.
    bytes: usize,
    /// The most that has been asked of this family. `Preview` is the evictable
    /// state; `Full` never goes back.
    wanted: Want,
    /// The most urgent lane this family has been asked down, so the variable-file
    /// retry in [`FontService::poll`] goes back down the same one (§15 D352).
    ///
    /// Stored rather than derived from [`wanted`](Self::wanted): a prefetch is
    /// `Want::Preview` on the third lane, and deriving would send its retry to the
    /// picker's bounded LIFO, where it could evict a visible row. Never downgrades.
    lane: Lane,
    /// The poll on which a picker row last drew this family — the LRU stamp
    /// [`FontService::trim_previews`] evicts by.
    touched: u64,
    /// Faces of this family that came back unusable — a download that failed, or
    /// bytes core declined to register.
    ///
    /// Lets [`FamilyStatus::Missing`] be said of a family the catalog lists but
    /// cannot deliver. A count rather than a flag: one failure is a verdict only
    /// alongside `faces` being empty.
    failed: usize,
}

/// The picker's memoized filter result: which family indices matched, for what
/// needle, at what revision of the family list.
///
/// Memoized because the list is drawn every frame and the needle changes only on
/// a keystroke.
#[derive(Default)]
struct Filter {
    needle: String,
    rows: Vec<u32>,
    /// The `families` revision `rows` was computed from — a family list that has
    /// grown (the catalog landing, the system scan finishing) invalidates the
    /// indices as surely as a new needle does.
    revision: u64,
    valid: bool,
}

impl Filter {
    /// The indices of `keys` matching `needle`, recomputing only when something
    /// it depends on has changed.
    fn sync(&mut self, needle: &str, keys: &[String], revision: u64) -> &[u32] {
        if self.valid && self.revision == revision && self.needle == needle {
            return &self.rows;
        }
        let lower = needle.to_lowercase();
        self.rows.clear();
        for (i, key) in keys.iter().enumerate() {
            if lower.is_empty() || key.contains(&lower) {
                self.rows.push(i as u32);
            }
        }
        self.needle = needle.to_string();
        self.revision = revision;
        self.valid = true;
        &self.rows
    }
}

/// Supplies font family names and loads their bytes into core on demand.
pub struct FontService {
    /// System font database — `None` until the background scan lands.
    db: Option<fontdb::Database>,
    system: BTreeSet<String>,
    /// Web-font catalog by family name (empty until the catalog lands).
    catalog: HashMap<String, CatalogEntry>,
    /// Sorted, de-duplicated family names for the picker (Inter + system + web).
    families: Vec<String>,
    /// `families`, lowercased and parallel to it, so a keystroke does not
    /// re-lowercase every name.
    keys: Vec<String>,
    /// Bumped whenever `families` changes, so [`Filter`] knows to recompute.
    revision: u64,
    filter: Filter,
    state: HashMap<String, FamilyState>,
    /// Every face key dispatched *or* registered, so no file is fetched twice —
    /// including when a preview load is upgraded to a full one mid-flight.
    claimed: HashSet<String>,
    /// Preview-only families that have been un-registered and whose cached
    /// preview strips the picker should therefore drop.
    evicted: Vec<String>,
    /// Poll counter, the clock behind `FamilyState::touched`.
    clock: u64,
    cache_dir: PathBuf,
    queue: Arc<Queue>,
    system_rx: Option<Receiver<SystemScan>>,
    catalog_rx: Receiver<Vec<CatalogEntry>>,
    /// The cancel flag of the catalog thread that is currently live (§15 D330:
    /// off means no request leaves the machine).
    ///
    /// One flag per spawn, not one on the service: a shared flag set `false` by an
    /// off and `true` by the next on would let a thread blocked in `fetch_catalog`
    /// across both write the response the user had cancelled. It cannot interrupt a
    /// `ureq` call in flight; it makes the result be dropped, not written or merged.
    catalog_live: Arc<std::sync::atomic::AtomicBool>,
    /// Whether the catalog thread has finished, however it finished.
    ///
    /// Read off the channel closing: the thread returns on three paths (fresh disk
    /// copy, successful fetch, failed fetch), and dropping the sender covers all of
    /// them where a "done" message would be missed on one. Nothing can be concluded
    /// about an unresolvable family until the sources stop arriving — [`status_of`].
    catalog_done: bool,
    /// Whether [`Self::prefetch_popular`] has already run this session.
    prefetched: bool,
    msg_rx: Receiver<FontMsg>,
    /// Whether the web library is offered at all (`prefs::Prefs::web_fonts`,
    /// §15 D330).
    ///
    /// A filter over the catalog, not a second catalog: off does not forget what
    /// was fetched, so turning it back on is a rebuild, not a re-fetch.
    web_fonts: bool,
    /// The agent and the catalog path, kept so [`Self::set_web_fonts`] can start the
    /// catalog thread mid-session.
    agent: ureq::Agent,
    catalog_path: PathBuf,
    /// Total bytes in [`Self::cache_dir`] as of the last [`Self::measure_cache`].
    /// `None` (nothing has measured) and `Some(0)` (an empty cache) are different
    /// answers and the UI says so.
    cache_bytes: Option<u64>,
    /// A scan in flight. Its own channel rather than a message on `msg_rx`, so the
    /// answer cannot be mistaken for a face.
    size_rx: Option<Receiver<u64>>,
}

impl FontService {
    /// `ctx` is cloned into every background thread, which calls
    /// `request_repaint` after each send.
    ///
    /// Without it an arriving face does not wake the reactive UI thread: the first
    /// arrival after the UI went quiet would be stranded.
    ///
    /// `web_fonts` is `prefs::Prefs::web_fonts`; with it `false` this makes no
    /// network call at all — the catalog thread is never spawned, so the service
    /// reports *no catalog, finally* rather than *not yet*. See
    /// [`Self::set_web_fonts`] for the switch moving inside a session.
    pub fn new(ctx: &egui::Context, web_fonts: bool) -> Self {
        let cache_dir = crate::machine_dir::root_or_temp();
        let fonts_dir = cache_dir.join("fonts");
        let _ = std::fs::create_dir_all(&fonts_dir);
        let catalog_path = cache_dir.join("catalog.json");

        // Bound the disk cache before anything asks it for a face, on its own thread
        // so the directory scan cannot delay the first frame. See [`trim_font_cache`].
        let sweep_dir = fonts_dir.clone();
        std::thread::spawn(move || trim_font_cache(&sweep_dir, FONT_CACHE_BYTES));

        // One agent for every request this process makes (see the module docs).
        let agent = agent();

        // Enumerate system fonts off the startup path; nothing needs the answer
        // until a picker is opened.
        let (system_tx, system_rx) = channel();
        let system_ctx = ctx.clone();
        std::thread::spawn(move || {
            let mut db = fontdb::Database::new();
            db.load_system_fonts();
            let mut families = BTreeSet::new();
            for face in db.faces() {
                for (name, _lang) in &face.families {
                    families.insert(name.clone());
                }
            }
            if system_tx.send(SystemScan { db, families }).is_ok() {
                system_ctx.request_repaint();
            }
        });

        // The catalog — see [`spawn_catalog`]. With web fonts off the thread is never
        // started; the dropped sender is the same "stopped arriving" signal a
        // finished thread gives (see `catalog_done`).
        let catalog_live = Arc::new(std::sync::atomic::AtomicBool::new(web_fonts));
        let catalog_rx = if web_fonts {
            spawn_catalog(ctx, &agent, catalog_path.clone(), Arc::clone(&catalog_live))
        } else {
            channel().1
        };

        let (msg_tx, msg_rx) = channel();
        let queue = Arc::new(Queue::default());
        for _ in 0..WORKERS {
            let queue = Arc::clone(&queue);
            let tx = msg_tx.clone();
            let agent = agent.clone();
            let ctx = ctx.clone();
            std::thread::spawn(move || {
                while let Some(job) = queue.pop() {
                    let msg = match load_one_face(&agent, &job.face) {
                        Ok(bytes) => FontMsg::Face {
                            family: job.family,
                            key: job.face.key,
                            bytes,
                        },
                        Err(reason) => FontMsg::Failed {
                            family: job.family,
                            key: job.face.key,
                            variable: job.face.variable,
                            reason,
                        },
                    };
                    if tx.send(msg).is_err() {
                        return; // the app is gone
                    }
                    // Including a `Failed`: it clears `outstanding`.
                    ctx.request_repaint();
                }
            });
        }

        let mut state = HashMap::new();
        // Core registers Inter itself, so it is loaded before anything asks.
        state.insert(
            "Inter".to_string(),
            FamilyState {
                wanted: Want::Full,
                ..FamilyState::default()
            },
        );

        Self {
            db: None,
            system: BTreeSet::new(),
            catalog: HashMap::new(),
            families: vec!["Inter".to_string()],
            keys: vec!["inter".to_string()],
            revision: 0,
            filter: Filter::default(),
            state,
            claimed: HashSet::new(),
            evicted: Vec::new(),
            clock: 0,
            cache_dir: fonts_dir,
            queue,
            system_rx: Some(system_rx),
            catalog_rx,
            catalog_live,
            catalog_done: false,
            prefetched: false,
            msg_rx,
            web_fonts,
            agent,
            catalog_path,
            cache_bytes: None,
            size_rx: None,
        }
    }

    /// A service that starts no threads, reads no disk and fetches nothing, for
    /// `OndinApp::headless`.
    ///
    /// The receivers are channels whose senders are already dropped, so `poll` sees
    /// `Disconnected` — the "stopped arriving" signal — and `status_of` can reach
    /// conclusions instead of waiting. Inter is present, as in the live service.
    #[cfg(test)]
    pub(crate) fn inert() -> Self {
        let (_, system_rx) = channel();
        let (_, catalog_rx) = channel();
        let (_, msg_rx) = channel();
        let mut state = HashMap::new();
        state.insert(
            "Inter".to_string(),
            FamilyState {
                wanted: Want::Full,
                ..FamilyState::default()
            },
        );
        Self {
            db: None,
            system: BTreeSet::new(),
            catalog: HashMap::new(),
            families: vec!["Inter".to_string()],
            keys: vec!["inter".to_string()],
            revision: 0,
            filter: Filter::default(),
            state,
            claimed: HashSet::new(),
            evicted: Vec::new(),
            clock: 0,
            cache_dir: std::env::temp_dir().join("ondin-headless-never-written"),
            queue: Arc::new(Queue::default()),
            system_rx: Some(system_rx),
            catalog_rx,
            // No thread was started, so there is nothing to cancel.
            catalog_live: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            catalog_done: false,
            // Nothing to warm; keeps `poll` from trying.
            prefetched: true,
            msg_rx,
            // `true` is the right default for `set_web_fonts`'s own tests; with no
            // catalog it only decides whether web names would be listed.
            web_fonts: true,
            // Constructing an agent opens no connection; deadlines included.
            agent: agent(),
            catalog_path: std::env::temp_dir().join("ondin-headless-never-read.json"),
            cache_bytes: None,
            size_rx: None,
        }
    }

    /// The attributes new text nodes start with. Inter is bundled in core, so it
    /// is the one family guaranteed to be present on every machine and offline.
    ///
    /// Everything not named here is the type's own default, which keeps a new node
    /// out of the save file's optional keys.
    ///
    /// The trim is named here rather than made `BoxTrim`'s `#[default]` (D78):
    /// `BoxTrim` skips serializing, so changing its default would re-measure every
    /// text node in every saved file. The app authors a trim; the format does not
    /// assume one.
    ///
    /// No receiver, because it reads no service state; that keeps it testable
    /// without standing up a `FontService`.
    pub fn default_text_parts() -> ondin_core::text::TextParts {
        ondin_core::text::TextParts {
            style: ondin_core::TextStyle {
                font_family: "Inter".into(),
                font_size: 24.0,
                // A stated 120% rather than Auto, whose leading differs per family.
                line_height: Some(ondin_core::Length::Em(1.2)),
                ..ondin_core::TextStyle::default()
            },
            // Cap height to baseline, the box a designer sees: the full line box is
            // padded by leading nobody drew. Descenders fall outside it, which is why
            // the untrimmed box is drawn (`TextLayout::untrimmed_bounds`).
            block: ondin_core::BlockStyle {
                trim: ondin_core::BoxTrim::CapToBaseline,
                ..Default::default()
            },
            ..ondin_core::text::TextParts::default()
        }
    }

    // --- the picker's list ------------------------------------------------

    /// How many families match `needle`, memoizing the match list for
    /// [`Self::matched_names`].
    ///
    /// The picker is virtualized on this count (§9.2): it draws the rows in view
    /// and no others.
    pub fn match_count(&mut self, needle: &str) -> usize {
        self.filter.sync(needle, &self.keys, self.revision).len()
    }

    /// The names of matches `rows`, cloned — a screenful at a time, so the
    /// caller can go on to mutate the service while drawing them.
    pub fn matched_names(&mut self, needle: &str, rows: std::ops::Range<usize>) -> Vec<String> {
        let matched = self.filter.sync(needle, &self.keys, self.revision);
        let end = rows.end.min(matched.len());
        matched[rows.start.min(end)..end]
            .iter()
            .map(|i| self.families[*i as usize].clone())
            .collect()
    }

    // --- loading -----------------------------------------------------------

    /// Ensure every cut of `family` is registered with core's shaping engine.
    /// No-op if already loaded, in flight, or unknown.
    pub fn ensure_loaded(&mut self, family: &str) {
        self.ensure(family, Want::Full, Lane::Urgent);
    }

    /// Warm [`POPULAR`] on the prefetch lane, once per session (§15 D352).
    ///
    /// Fired when the catalog has finished arriving, not from `new`: before it
    /// lands `faces_of` answers nothing. With the switch off `catalog_done` is true
    /// from the start, so the `web_fonts` guard is what stops requests.
    ///
    /// Turning the switch off does not drain a warm already queued, so up to fifty
    /// requests can still go out afterwards. The rule: requests already queued or in
    /// flight when the switch goes off are the only hole; anything new that reaches
    /// the network must be startable only while the switch is on, or hold a cancel
    /// flag as `spawn_catalog` does (`grep -n 'agent'` enumerates the callers).
    fn prefetch_popular(&mut self) {
        self.prefetched = true;
        if !self.web_fonts {
            return;
        }
        self.warm(POPULAR);
    }

    /// Queue the regular upright of each of `families` on the prefetch lane,
    /// skipping any this machine already has installed.
    ///
    /// Only families the catalog answers for are queued (`faces_of` resolves a
    /// system family to its installed file first), so no preview budget is spent on
    /// faces that were never going to be fetched. Takes the list rather than
    /// reading [`POPULAR`] so the skip can be tested with a real installed face.
    fn warm(&mut self, families: &[&str]) {
        for family in families {
            let web = self
                .faces_of(family, Want::Preview)
                .iter()
                .any(|s| matches!(s.source, FaceSource::Web { .. }));
            if web {
                self.ensure(family, Want::Preview, Lane::Prefetch);
            }
        }
    }

    /// Ensure enough of `family` is registered to draw its name in it — the
    /// regular upright, and nothing else.
    ///
    /// The face this fetches is the same one [`Self::ensure_loaded`] needs
    /// first, so a previewed family applies without a download.
    pub fn ensure_preview(&mut self, family: &str) {
        self.ensure(family, Want::Preview, Lane::Preview);
    }

    /// Whether a face of `family` is on its way. The family field says so, so a
    /// slow download reads as slow rather than as broken.
    pub fn is_loading(&self, family: &str) -> bool {
        self.state.get(family).is_some_and(|s| s.outstanding > 0)
    }

    /// Ready, pending or missing — the question a UI warning has to ask
    /// (§15 D227, §5.4a).
    ///
    /// More than [`Self::is_loading`], which only says a download is in flight:
    /// this is the verdict, consulting the family list and whether the sources
    /// have stopped arriving.
    pub fn family_status(&self, family: &str) -> FamilyStatus {
        let state = self.state.get(family);
        status_of(FamilyFacts {
            drawable: ondin_core::text::is_family_available(family),
            in_flight: state.is_some_and(|s| s.outstanding > 0),
            // The scan's receiver is taken when it lands, so `None` *is* "landed".
            sources_settled: self.system_rx.is_none() && self.catalog_done,
            // `families` is sorted, so this costs no `fontdb` query per frame.
            listed: self.families.binary_search(&family.to_string()).is_ok(),
            // A family whose italic failed and whose upright arrived is not missing.
            tried_and_failed: state.is_some_and(|s| s.failed > 0 && s.faces.is_empty()),
        })
    }

    /// Note that a picker row drew `family`, which is what keeps it from being
    /// evicted while it is on screen.
    pub fn touch(&mut self, family: &str) {
        if let Some(state) = self.state.get_mut(family) {
            state.touched = self.clock;
        }
    }

    /// Preview-only families un-registered since the last call, whose cached
    /// preview strips are now stale.
    pub fn take_evicted(&mut self) -> Vec<String> {
        std::mem::take(&mut self.evicted)
    }

    // --- the web-font switch and the disk cache ----------------------------

    /// Turn the web library on or off for the rest of the session.
    ///
    /// On is a rebuild, not a re-fetch, unless the catalog is empty (a session
    /// launched with the switch off never spawned the thread).
    ///
    /// The new receiver must come with `catalog_done = false`: the flag is `true`
    /// for a session launched with web fonts off, and leaving it set would let
    /// [`status_of`] call a family missing that is about to be listed.
    pub fn set_web_fonts(&mut self, on: bool, ctx: &egui::Context) {
        if self.web_fonts == on {
            return;
        }
        self.web_fonts = on;
        // Off cancels the catalog thread that is out (see `catalog_live`). A flag
        // only ever goes `false`: `store(on)` would un-cancel a thread still
        // blocked in `fetch_catalog`. On starts a thread with a new flag, or none.
        if !on {
            self.catalog_live
                .store(false, std::sync::atomic::Ordering::Relaxed);
        } else if self.catalog.is_empty() {
            self.catalog_live = Arc::new(std::sync::atomic::AtomicBool::new(true));
            self.catalog_rx = spawn_catalog(
                ctx,
                &self.agent,
                self.catalog_path.clone(),
                Arc::clone(&self.catalog_live),
            );
            self.catalog_done = false;
        }
        self.rebuild_families();
    }

    /// Bytes on disk as of the last [`Self::measure_cache`], or `None` while the
    /// answer is still being counted.
    pub fn cache_bytes(&self) -> Option<u64> {
        self.cache_bytes
    }

    /// Count the disk cache, on a background thread.
    ///
    /// Off the UI thread: `metadata()` per entry on a few thousand files must not
    /// stall the frame the settings modal opens on.
    ///
    /// A second call while one is in flight is a no-op, so holding the modal open
    /// does not start a scan per frame.
    pub fn measure_cache(&mut self, ctx: &egui::Context) {
        if self.size_rx.is_some() {
            return;
        }
        let (tx, rx) = channel();
        let dir = self.cache_dir.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            if tx.send(dir_bytes(&dir)).is_ok() {
                ctx.request_repaint();
            }
        });
        self.size_rx = Some(rx);
    }

    /// Delete every cached face file, and report how many bytes went.
    ///
    /// Nothing is un-registered: the cache is derived data, so a registered face
    /// keeps working and the cost is one re-download later. A worker mid-download
    /// writes its file back afterwards, which is harmless.
    ///
    /// Synchronous, unlike [`Self::measure_cache`], because it answers a button and
    /// the result must show at once; bounded by the cache budget.
    pub fn clear_cache(&mut self) -> u64 {
        let freed = dir_bytes(&self.cache_dir);
        if let Ok(entries) = std::fs::read_dir(&self.cache_dir) {
            for entry in entries.flatten() {
                if entry.metadata().is_ok_and(|m| m.is_file()) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        // Not `None`: the count is known to be zero.
        self.cache_bytes = Some(0);
        freed
    }

    /// Give back a claim for a face that will never report (§15 D354).
    ///
    /// Not a failure: the `Failed` arm of [`Self::poll`] also bumps
    /// `FamilyState::failed`, which lets [`status_of`] call a family `Missing`. A
    /// job the backlog pushed out was never attempted.
    fn release(&mut self, family: &str, key: &str) {
        self.claimed.remove(key);
        if let Some(state) = self.state.get_mut(family) {
            state.outstanding = state.outstanding.saturating_sub(1);
        }
    }

    fn ensure(&mut self, family: &str, want: Want, lane: Lane) {
        let specs: Vec<FaceSpec> = self
            .faces_of(family, want)
            .into_iter()
            .filter(|s| !self.claimed.contains(&s.key))
            .collect();

        let clock = self.clock;
        let state = self.state.entry(family.to_string()).or_default();
        state.touched = clock;
        if want == Want::Full {
            // Never goes back: a picked family is not a preview any more.
            state.wanted = Want::Full;
        }
        // `min` is the more urgent, `Lane` being declared most-urgent-first.
        state.lane = state.lane.min(lane);
        if specs.is_empty() {
            return;
        }
        state.outstanding += specs.len();

        for face in specs {
            self.claimed.insert(face.key.clone());
            let job = Job {
                family: family.to_string(),
                face,
            };
            match lane {
                Lane::Urgent => self.queue.push_urgent(job),
                // What the backlog bound pushes out must be released here: a dropped
                // job never reports (see `push_preview`).
                Lane::Preview => {
                    if let Some(out) = self.queue.push_preview(job) {
                        self.release(&out.family, &out.face.key);
                    }
                }
                Lane::Prefetch => self.queue.push_prefetch(job),
            }
        }
    }

    /// The files that make up `family`, **primary face first**.
    fn faces_of(&self, family: &str, want: Want) -> Vec<FaceSpec> {
        // System fonts win over the catalog, matching §5.4a's resolution order:
        // an installed family is the one the machine already has.
        if let Some(db) = &self.db
            && self.system.contains(family)
        {
            return system_faces(db, family, want);
        }
        // The switch is enforced here, not only in the family list: a document saved
        // with web fonts on can name a family without going through the picker.
        if self.web_fonts
            && let Some(entry) = self.catalog.get(family)
        {
            return web_faces(entry, &self.cache_dir, want);
        }
        Vec::new()
    }

    /// Drain background results: merge the system scan and the catalog when they
    /// arrive, and register any font data that landed. Call once per frame.
    pub fn poll(&mut self) -> FontPoll {
        let mut out = FontPoll::default();
        self.clock += 1;

        if let Some(rx) = &self.system_rx
            && let Ok(scan) = rx.try_recv()
        {
            self.system = scan.families;
            self.db = Some(scan.db);
            self.system_rx = None;
            self.rebuild_families();
            out.families_changed = true;
        }

        // Up to two batches: the disk copy, then the refreshed one. `Disconnected` is
        // the catalog thread having finished, which lets a family be called missing
        // rather than late.
        loop {
            match self.catalog_rx.try_recv() {
                Ok(entries) => {
                    for entry in entries {
                        self.catalog.insert(entry.family.clone(), entry);
                    }
                    self.rebuild_families();
                    out.families_changed = true;
                }
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    self.catalog_done = true;
                    break;
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => break,
            }
        }

        // After the catalog loop, so the entries the specs are built from are in.
        // `prefetch_popular` is what refuses when the switch is off.
        if self.catalog_done && !self.prefetched {
            self.prefetch_popular();
        }

        while let Ok(msg) = self.msg_rx.try_recv() {
            match msg {
                FontMsg::Face { family, key, bytes } => {
                    // Before the move: `register_fonts` consumes the blob.
                    let size = bytes.len();
                    let registered = !ondin_core::text::register_fonts(bytes).is_empty();
                    let lower = family.to_lowercase();
                    let state = self.state.entry(family).or_default();
                    state.outstanding = state.outstanding.saturating_sub(1);
                    if registered {
                        state.faces.push(key);
                        state.bytes += size;
                        out.fonts_registered = true;
                        // Duplicates are left alone: the consumer is a membership test.
                        out.registered.push(lower);
                    } else {
                        // Not a usable font: un-claim it so a later attempt is not
                        // skipped, and count it as a failure.
                        state.failed += 1;
                        self.claimed.remove(&key);
                    }
                }
                FontMsg::Failed {
                    family,
                    key,
                    variable,
                    reason,
                } => {
                    let state = self.state.entry(family.clone()).or_default();
                    state.outstanding = state.outstanding.saturating_sub(1);
                    state.failed += 1;
                    let want = state.wanted;
                    let lane = state.lane;
                    self.claimed.remove(&key);
                    if variable {
                        // The variable file is the preferred form: a family whose file
                        // the decoder cannot read still has static cuts on the CDN.
                        // On `Decode` only: the downgrade is unrecoverable for the
                        // session (`entry.cut` is restored only by `fetch_catalog`),
                        // while a `Fetch` failure lets the `ensure` below retry.
                        if reason == FaceFailure::Decode
                            && let Some(entry) = self.catalog.get_mut(&family)
                        {
                            entry.cut = None;
                        }
                        self.ensure(&family, want, lane);
                    }
                }
            }
        }

        // The cache scan is not on `out`: a byte count changes neither the family
        // list nor the registered fonts, and the thread asked for its own repaint.
        if let Some(rx) = &self.size_rx
            && let Ok(bytes) = rx.try_recv()
        {
            self.cache_bytes = Some(bytes);
            self.size_rx = None;
        }

        self.trim_previews();
        out
    }

    /// Un-register the least recently drawn preview-only families, once they hold
    /// more than [`PREVIEW_BYTES`] between them.
    ///
    /// Drawing every row in its own face registers font data, so scrolling the
    /// catalog would otherwise leak without bound. A family is exempt once anything
    /// asked for it in full (a document's font, a user's pick), so eviction never
    /// pulls a face from under the canvas.
    ///
    /// Oldest-drawn first, by the [`FamilyState::touched`] stamp, stopping when back
    /// under the budget or down to [`PREVIEW_FLOOR`] families.
    fn trim_previews(&mut self) {
        let evictable = |s: &FamilyState| {
            s.wanted == Want::Preview && s.outstanding == 0 && !s.faces.is_empty()
        };
        // Measured before collecting: this runs every frame.
        let (mut held, mut total) = (0_usize, 0_usize);
        for state in self.state.values().filter(|s| evictable(s)) {
            held += 1;
            total += state.bytes;
        }
        if total <= PREVIEW_BYTES || held <= PREVIEW_FLOOR {
            return;
        }
        let candidates: Vec<(u64, usize, String)> = self
            .state
            .iter()
            .filter(|(_, s)| evictable(s))
            .map(|(name, s)| (s.touched, s.bytes, name.clone()))
            .collect();
        for family in families_over_budget(candidates, total) {
            // Only keys this family alone owns: a TrueType collection carries several
            // families, and un-claiming a shared file would register it twice.
            let Some(state) = self.state.remove(&family) else {
                continue;
            };
            for key in &state.faces {
                if !self.state.values().any(|s| s.faces.contains(key)) {
                    self.claimed.remove(key);
                }
            }
            ondin_core::text::unregister_family(&family);
            self.evicted.push(family);
        }
    }

    /// Rebuild the picker's list from the three sources — two of them, with web
    /// fonts off (bundled Inter and the machine's own fonts).
    fn rebuild_families(&mut self) {
        let mut names = BTreeSet::new();
        names.insert("Inter".to_string()); // bundled in core
        names.extend(self.system.iter().cloned());
        if self.web_fonts {
            names.extend(self.catalog.keys().cloned());
        }
        self.families = names.into_iter().collect();
        self.keys = self.families.iter().map(|n| n.to_lowercase()).collect();
        self.revision += 1;
    }
}

/// Which of `candidates` — `(touched, bytes, family)` — to un-register, to get
/// `total` bytes back under [`PREVIEW_BYTES`] without taking the count below
/// [`PREVIEW_FLOOR`]. Oldest-drawn first.
///
/// Split out from [`FontService::trim_previews`] so the policy can be tested
/// without building a `FontService`.
fn families_over_budget(
    mut candidates: Vec<(u64, usize, String)>,
    mut total: usize,
) -> Vec<String> {
    // `touched` leads, so the sort is LRU order; the rest only make ties
    // deterministic.
    candidates.sort_unstable();
    let mut held = candidates.len();
    let mut drop = Vec::new();
    for (_, bytes, family) in candidates {
        if total <= PREVIEW_BYTES || held <= PREVIEW_FLOOR {
            break;
        }
        total = total.saturating_sub(bytes);
        held -= 1;
        drop.push(family);
    }
    drop
}

/// Start the catalog thread and hand back the receiver it will send on: the disk
/// copy first and the network second, so the picker has the full library on the
/// frame it first opens — and offline, where the fetch never lands at all (§5.4a).
///
/// The network half is gated on the disk copy's age ([`CATALOG_MAX_AGE`]). A
/// function rather than a block in [`FontService::new`] because turning web fonts
/// on mid-session starts it too; the sender must not outlive the thread
/// (`catalog_done` reads the channel disconnecting), so only the receiver returns.
fn spawn_catalog(
    ctx: &egui::Context,
    agent: &ureq::Agent,
    path: PathBuf,
    live: Arc<std::sync::atomic::AtomicBool>,
) -> Receiver<Vec<CatalogEntry>> {
    let (tx, rx) = channel();
    let agent = agent.clone();
    let ctx = ctx.clone();
    std::thread::spawn(move || {
        // Freshness is asked of a copy we could actually read: a recent but corrupt
        // file has a good mtime and no catalog.
        let disk = read_catalog(&path);
        let fresh = disk.is_some() && is_fresh(&path, CATALOG_MAX_AGE);
        if let Some(entries) = disk
            && tx.send(entries).is_ok()
        {
            ctx.request_repaint();
        }
        if fresh {
            return;
        }
        // The switch, asked immediately before the network: the disk copy above is
        // local and sent either way, but everything past here is a request (§15 D330).
        let live = || live.load(std::sync::atomic::Ordering::Relaxed);
        if !live() {
            return;
        }
        if let Some(entries) = fetch_catalog(&agent) {
            // Asked again on the way back: `fetch_catalog` blocks, and the user may
            // have turned the switch off while it was out.
            if !live() {
                return;
            }
            write_catalog(&path, &entries);
            if tx.send(entries).is_ok() {
                ctx.request_repaint();
            }
        }
    });
    rx
}

/// Total bytes of the plain files directly in `dir`, and nothing else.
///
/// The same walk and exclusions as [`trim_font_cache`]: subdirectories and
/// unreadable entries are skipped, so the number is the one the budget measures. A
/// missing directory is 0.
fn dir_bytes(dir: &Path) -> u64 {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return 0;
    };
    entries
        .flatten()
        .filter_map(|e| e.metadata().ok())
        .filter(|m| m.is_file())
        .map(|m| m.len())
        .sum()
}

/// Which of `candidates` — `(used, bytes, path)` — to delete, to get `total` bytes
/// back under `budget`. Least recently used first.
///
/// `budget` is a parameter so a test need not write 256 MB. Split out from
/// [`trim_font_cache`] so the policy can be tested without a real directory.
///
/// No floor, unlike [`families_over_budget`]: this runs once at startup with
/// nothing on screen, so there is nothing to thrash against.
fn files_over_budget(
    mut candidates: Vec<(SystemTime, u64, PathBuf)>,
    mut total: u64,
    budget: u64,
) -> Vec<PathBuf> {
    // `used` leads, so the sort is LRU order; the rest only make ties
    // deterministic.
    candidates.sort_unstable();
    let mut drop = Vec::new();
    for (_, bytes, path) in candidates {
        if total <= budget {
            break;
        }
        total = total.saturating_sub(bytes);
        drop.push(path);
    }
    drop
}

/// Delete the least recently used cached faces until the directory is back under
/// [`FONT_CACHE_BYTES`].
///
/// Run once, off the startup path, which is what makes the missing floor safe (see
/// [`files_over_budget`]).
///
/// "Least recently used" rests on the mtime: [`load_face`] touches the file on a
/// cache hit, so the stamp means "last wanted". Subdirectories and unreadable
/// entries are skipped; they are not this function's to delete.
fn trim_font_cache(dir: &Path, budget: u64) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let (mut total, mut candidates) = (0_u64, Vec::new());
    for entry in entries.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        if !meta.is_file() {
            continue;
        }
        // An unknown mtime sorts as the epoch, i.e. goes first (as in `is_fresh`).
        let used = meta.modified().unwrap_or(SystemTime::UNIX_EPOCH);
        total += meta.len();
        candidates.push((used, meta.len(), entry.path()));
    }
    if total <= budget {
        return;
    }
    for path in files_over_budget(candidates, total, budget) {
        let _ = std::fs::remove_file(path);
    }
}

impl Drop for FontService {
    fn drop(&mut self) {
        self.queue.stop();
    }
}

// ---------------------------------------------------------------------------
// Face lists
// ---------------------------------------------------------------------------

/// The system files backing `family`, the regular upright first.
///
/// Non-file sources are skipped: `load_system_fonts` only ever produces file
/// sources, and a worker thread reads bytes by path rather than borrowing the
/// database.
fn system_faces(db: &fontdb::Database, family: &str, want: Want) -> Vec<FaceSpec> {
    let mut faces: Vec<(bool, PathBuf)> = Vec::new();
    let mut seen = HashSet::new();
    for face in db.faces() {
        if !face.families.iter().any(|(n, _)| n == family) {
            continue;
        }
        let fontdb::Source::File(path) = &face.source else {
            continue;
        };
        if !seen.insert(path.clone()) {
            continue;
        }
        let regular = face.weight.0 == 400 && face.style == fontdb::Style::Normal;
        faces.push((regular, path.clone()));
    }
    // The regular upright leads: it is what a preview draws and what the
    // document shapes with while the rest are still arriving.
    faces.sort_by_key(|(regular, _)| !*regular);
    if want == Want::Preview {
        faces.truncate(1);
    }
    faces.truncate(MAX_FACES);
    faces
        .into_iter()
        .map(|(_, path)| FaceSpec {
            key: path.display().to_string(),
            source: FaceSource::File(path),
            variable: false,
        })
        .collect()
}

/// The Fontsource files backing `entry`, the primary face first.
fn web_faces(entry: &CatalogEntry, dir: &Path, want: Want) -> Vec<FaceSpec> {
    // A variable file is one download for every weight, and the axis rows and
    // variant list are read from its own `fvar` (§9.2 Font tab). Two files at most.
    if let Some(cut) = &entry.cut {
        let mut out = vec![vf_face(entry, cut, "normal", dir)];
        if want == Want::Full && entry.vf_italic {
            out.push(vf_face(entry, cut, "italic", dir));
        }
        return out;
    }

    let mut weights = entry.weights.clone();
    weights.sort_unstable();
    let styles: Vec<&str> = ["normal", "italic"]
        .into_iter()
        .filter(|s| entry.styles.iter().any(|x| x == s))
        .collect();
    let styles = if styles.is_empty() {
        vec!["normal"]
    } else {
        styles
    };
    let primary_weight = nearest(&weights, 400);
    let primary_style = if styles.contains(&"normal") {
        "normal"
    } else {
        styles[0]
    };

    let mut out = vec![static_face(entry, primary_weight, primary_style, dir)];
    if want == Want::Preview {
        return out;
    }
    for weight in &weights {
        for style in &styles {
            if (*weight, *style) != (primary_weight, primary_style) {
                out.push(static_face(entry, *weight, style, dir));
            }
        }
    }
    out.truncate(MAX_FACES);
    out
}

fn vf_face(entry: &CatalogEntry, cut: &str, style: &str, dir: &Path) -> FaceSpec {
    // `.ttf`, not `.woff2`: the cache holds the decoded bytes so the decompress
    // happens once ever, not once per launch.
    let key = format!("{}-{}-{}-{}.vf.ttf", entry.id, entry.subset, cut, style);
    FaceSpec {
        source: FaceSource::Web {
            url: format!(
                "{FONTSOURCE_CDN}/{}:vf@latest/{}-{}-{}.woff2",
                entry.id, entry.subset, cut, style
            ),
            cache: dir.join(&key),
        },
        key,
        variable: true,
    }
}

fn static_face(entry: &CatalogEntry, weight: u16, style: &str, dir: &Path) -> FaceSpec {
    let key = format!("{}-{}-{}-{}.ttf", entry.id, entry.subset, weight, style);
    FaceSpec {
        source: FaceSource::Web {
            url: format!(
                "{FONTSOURCE_CDN}/{}@latest/{}-{}-{}.ttf",
                entry.id, entry.subset, weight, style
            ),
            cache: dir.join(&key),
        },
        key,
        variable: false,
    }
}

/// The offered weight closest to `target`, or `target` itself for a family that
/// lists none.
fn nearest(weights: &[u16], target: u16) -> u16 {
    weights
        .iter()
        .copied()
        .min_by_key(|w| w.abs_diff(target))
        .unwrap_or(target)
}

// ---------------------------------------------------------------------------
// I/O
// ---------------------------------------------------------------------------

/// [`load_face`] with an unwind guard, which is what a worker actually calls
/// (§15 D450).
///
/// A decoder panic would otherwise leave the worker's loop: the thread is lost
/// and the job's `FontMsg::Failed` is never sent, so the family reads as loading
/// for ever. A caught unwind is exactly an `Err`, and the guard sits at the seam
/// every call goes through (§15 D239), covering `fontdb`, `std::fs::read` and the
/// next decoder. Inert under `panic = "abort"`, which is a build failure (§15 D445).
///
/// An unwind is [`FaceFailure::Decode`], not `Fetch`: the bytes are in hand by the
/// time a decoder panics, and `Decode` keeps the static-cut fallback reachable.
fn load_one_face(agent: &ureq::Agent, spec: &FaceSpec) -> Result<Vec<u8>, FaceFailure> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| load_face(agent, spec)))
        .unwrap_or(Err(FaceFailure::Decode))
}

/// Read one face's bytes: from the disk cache or the local file if it is there,
/// from the CDN if it is not. Decodes woff2 and caches the result decoded.
fn load_face(agent: &ureq::Agent, spec: &FaceSpec) -> Result<Vec<u8>, FaceFailure> {
    match &spec.source {
        // A local file that will not open is `Fetch`; one that opens and is not
        // sfnt is `Decode`, as the web arm reaches through `sfnt_from`.
        FaceSource::File(path) => match std::fs::read(path) {
            Err(_) => Err(FaceFailure::Fetch),
            Ok(b) if is_sfnt(&b) => Ok(b),
            Ok(_) => Err(FaceFailure::Decode),
        },
        FaceSource::Web { url, cache } => {
            if let Ok(bytes) = std::fs::read(cache)
                && is_sfnt(&bytes)
            {
                touch(cache);
                return Ok(bytes);
            }
            let raw = download(agent, url).ok_or(FaceFailure::Fetch)?;
            let bytes = sfnt_from(raw).ok_or(FaceFailure::Decode)?;
            let _ = std::fs::write(cache, &bytes);
            Ok(bytes)
        }
    }
}

/// Downloaded bytes as sfnt — decompressing woff2 — or `None` if they are not a
/// font this app can read (§15 D450).
///
/// The woff2 decoder panics on malformed input (a 53-byte file declaring an
/// oversized table suffices; the dependency's header check is a `TODO`), and
/// `.ok()` turns an `Err` into `None` but cannot see an unwind. Extracted from
/// `load_face` so the guard sits where the panic is and a test can drive it.
///
/// [`load_one_face`] guards the whole job as well, and that is not redundant:
/// this is the decoder known to panic, that one covers everything else.
fn sfnt_from(raw: Vec<u8>) -> Option<Vec<u8>> {
    let bytes = if woff2::decode::is_woff2(&raw) {
        std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            woff2::convert_woff2_to_ttf(&mut &raw[..]).ok()
        }))
        .ok()
        .flatten()?
    } else {
        raw
    };
    is_sfnt(&bytes).then_some(bytes)
}

/// Stamp a cached face as used now, so [`trim_font_cache`]'s "oldest first" means
/// least recently *wanted* rather than first downloaded.
///
/// Opened for writing because `File::set_modified` needs write access on Windows;
/// `write(true)` alone does not truncate. Every failure is ignored: an untouched
/// file is evicted a little early, costing one re-download.
fn touch(path: &Path) {
    let _ = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(SystemTime::now()));
}

/// The one agent every request in this process goes through, with deadlines on
/// it.
///
/// `ureq::Agent::new_with_defaults()` has no timeouts at all (§15 D441), so a
/// stalled server would block a worker for the session, and with `WORKERS` at 6 a
/// bad network ends all font loading. The `.limit()` on the body bounds bytes,
/// which a stall never reaches.
///
/// The global deadline is not redundant: `timeout_connect` covers a blackholed SYN,
/// `timeout_recv_body` a body that stops, and `timeout_global` a peer dribbling a
/// byte at a time. The values are generous (a CJK subset is megabytes); what
/// matters is that they are finite.
fn agent() -> ureq::Agent {
    use std::time::Duration;
    ureq::config::Config::builder()
        .timeout_connect(Some(Duration::from_secs(15)))
        .timeout_recv_response(Some(Duration::from_secs(30)))
        .timeout_recv_body(Some(Duration::from_secs(60)))
        .timeout_global(Some(Duration::from_secs(120)))
        .build()
        .new_agent()
}

fn download(agent: &ureq::Agent, url: &str) -> Option<Vec<u8>> {
    agent
        .get(url)
        .call()
        .ok()?
        .body_mut()
        .with_config()
        .limit(64 * 1024 * 1024)
        .read_to_vec()
        .ok()
}

/// Fetch + parse the Fontsource catalog, keeping only Google Fonts families and
/// folding in each family's variable file, where it has one.
fn fetch_catalog(agent: &ureq::Agent) -> Option<Vec<CatalogEntry>> {
    /// Raw Fontsource list-API item.
    #[derive(serde::Deserialize)]
    struct Item {
        id: String,
        family: String,
        weights: Vec<u16>,
        styles: Vec<String>,
        #[serde(rename = "defSubset")]
        def_subset: String,
        #[serde(rename = "type")]
        kind: String,
    }
    /// Raw item from the variable-fonts endpoint. Only the axis names are taken;
    /// the ranges are read from the face's own `fvar`.
    #[derive(serde::Deserialize)]
    struct Variable {
        axes: HashMap<String, serde::de::IgnoredAny>,
    }

    let list: Vec<Item> = serde_json::from_slice(&fetch(agent, FONTSOURCE_LIST)?).ok()?;
    // A missing variable list is not a failed catalog: every family still has
    // its static cuts.
    let variable: HashMap<String, Variable> = fetch(agent, FONTSOURCE_VARIABLE)
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default();

    Some(
        list.into_iter()
            .filter(|i| i.kind == "google")
            // Both fields end up in the cache filename and are the whole untrusted
            // surface. See [`is_catalog_token`].
            .filter(|i| is_catalog_token(&i.id) && is_catalog_token(&i.def_subset))
            .map(|i| {
                let axes = variable.get(&i.id).map(|v| &v.axes);
                CatalogEntry {
                    cut: axes.and_then(|a| variable_cut(a.keys().map(String::as_str))),
                    vf_italic: axes.is_some_and(|a| a.contains_key("ital")),
                    id: i.id,
                    family: i.family,
                    weights: i.weights,
                    styles: i.styles,
                    subset: i.def_subset,
                }
            })
            .collect(),
    )
}

/// Which variable file a family with these axes publishes.
///
/// Fontsource names the file after the axis set it carries (probed against the CDN):
///
/// - `wght` — exists exactly when the family has a weight axis.
/// - `standard` — every axis except `ital`, published only when there is
///   something besides weight (Roboto's `wght`+`wdth`). Preferred over `wght`.
/// - `full` — the fallback for a family with no weight axis at all.
fn variable_cut<'a>(axes: impl Iterator<Item = &'a str>) -> Option<String> {
    let axes: Vec<&str> = axes.collect();
    if axes.is_empty() {
        return None;
    }
    let has_weight = axes.contains(&"wght");
    let others = axes.iter().any(|a| *a != "wght" && *a != "ital");
    Some(
        match (has_weight, others) {
            (false, _) => "full",
            (true, true) => "standard",
            (true, false) => "wght",
        }
        .to_string(),
    )
}

fn fetch(agent: &ureq::Agent, url: &str) -> Option<Vec<u8>> {
    download(agent, url)
}

/// Whether `path` was written less than `max_age` ago.
///
/// Every uncertain answer (missing file, unreadable mtime, a stamp in the future)
/// is "not fresh": an unneeded refresh costs two requests, a wrongly skipped one
/// a catalog that never updates.
fn is_fresh(path: &Path, max_age: std::time::Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .is_ok_and(|t| t.elapsed().is_ok_and(|age| age < max_age))
}

fn read_catalog(path: &Path) -> Option<Vec<CatalogEntry>> {
    let bytes = std::fs::read(path).ok()?;
    let entries: Vec<CatalogEntry> = serde_json::from_slice(&bytes).ok()?;
    // Filtered here as well as at `fetch_catalog`, because the catalog is persisted:
    // one hostile response would otherwise keep working on every later launch.
    let entries: Vec<CatalogEntry> = entries
        .into_iter()
        .filter(|e| is_catalog_token(&e.id) && is_catalog_token(&e.subset))
        .collect();
    (!entries.is_empty()).then_some(entries)
}

/// Whether a value out of the Fontsource catalog is safe to put in a **filename**.
///
/// The id is used as a URL segment and as a local cache path (`dir.join(&key)`),
/// so an id like `"../../AppData/…"` would write a font file outside the cache
/// directory, and an absolute one discards the base. Rejected at the parse rather
/// than sanitised at the join; `[a-z0-9-]` is the shape Fontsource publishes. It
/// needs a hostile API (TLS is verified), so it is a silent filter, not an error.
fn is_catalog_token(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
}

fn write_catalog(path: &Path, entries: &[CatalogEntry]) {
    if let Ok(bytes) = serde_json::to_vec(entries) {
        let _ = std::fs::write(path, bytes);
    }
}

/// Cheap sanity check that bytes begin with a known sfnt signature, so a stray
/// error page never reaches the shaper.
fn is_sfnt(bytes: &[u8]) -> bool {
    matches!(
        bytes.get(0..4),
        Some(&[0x00, 0x01, 0x00, 0x00]) // TrueType
            | Some(b"OTTO")             // CFF/OpenType
            | Some(b"true")
            | Some(b"ttcf") // collection
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A malformed woff2 (one table declaring `origLength` 1,000,000) is a failed
    /// face, not a dead worker. Asserts through `sfnt_from`, the only seam a test
    /// can reach (the decode is behind `download`).
    /// flip: remove the `catch_unwind` in `sfnt_from` → the test binary dies on the
    /// decoder's panic before the first assertion.
    #[test]
    fn a_malformed_woff2_is_a_failed_face_and_not_a_dead_worker() {
        // A 48-byte wOF2 header, one table entry whose origLength is 1,000,000 as
        // UIntBase128, and a one-byte empty brotli stream.
        let mut f = Vec::new();
        f.extend_from_slice(b"wOF2");
        f.extend_from_slice(&0x0001_0000u32.to_be_bytes()); // flavor
        f.extend_from_slice(&53u32.to_be_bytes()); // length
        f.extend_from_slice(&1u16.to_be_bytes()); // numTables
        f.extend_from_slice(&0u16.to_be_bytes()); // reserved
        f.extend_from_slice(&1_000_000u32.to_be_bytes()); // totalSfntSize
        f.extend_from_slice(&1u32.to_be_bytes()); // totalCompressedSize
        f.extend_from_slice(&0u16.to_be_bytes()); // majorVersion
        f.extend_from_slice(&0u16.to_be_bytes()); // minorVersion
        for _ in 0..5 {
            f.extend_from_slice(&0u32.to_be_bytes()); // meta/priv offsets+lengths
        }
        f.push(0x00); // table entry: flags
        f.extend_from_slice(&[0xBD, 0x84, 0x40]); // origLength = 1,000,000
        f.push(0x06); // an empty brotli stream
        assert_eq!(f.len(), 53, "the fixture is the file that was measured");
        assert!(
            woff2::decode::is_woff2(&f),
            "and it reaches the decoder rather than being dispatched away"
        );

        assert_eq!(
            sfnt_from(f),
            None,
            "a malformed woff2 is a face that failed to load"
        );

        // The control: an sfnt-framed file still comes through.
        let mut sfnt = vec![0x00, 0x01, 0x00, 0x00];
        sfnt.extend_from_slice(&[0u8; 60]);
        assert!(sfnt_from(sfnt).is_some(), "an ordinary sfnt still loads");
    }

    /// New text is trimmed by default and its line box survives to be drawn.
    /// Asserted through laid-out geometry, not only the field, and it fails if the
    /// trim is "tidied" out of `default_text_parts` as a duplicate of `BoxTrim`'s
    /// `#[default]` (§15 D78).
    #[test]
    fn a_new_text_node_gets_the_tight_box_and_keeps_its_line_box() {
        let mut parts = FontService::default_text_parts();
        assert_eq!(
            parts.block.trim,
            ondin_core::BoxTrim::CapToBaseline,
            "new text is no longer trimmed by default"
        );
        // A click plants `Auto`, the sizing trim tightens; a dragged `Fixed` box is
        // not reinterpreted.
        parts.sizing = ondin_core::TextSizing::Auto;
        let tl = ondin_core::text::layout(parts.as_ref("Hxy"));
        assert!(
            tl.bounds().height() < tl.untrimmed_bounds().height(),
            "the box did not tighten: {:?} vs line box {:?}",
            tl.bounds(),
            tl.untrimmed_bounds()
        );
        assert!(
            tl.origin.y > 0.0,
            "the box should start below the local origin, at {}",
            tl.origin.y
        );
        // Descenders ("y" has one) fall outside a cap-to-baseline box.
        let lowest = tl.runs[0]
            .glyphs
            .iter()
            .map(|g| f64::from(g.y))
            .fold(f64::MIN, f64::max);
        assert!(
            (tl.bounds().y1 - lowest).abs() < 0.5,
            "the box should end on the baseline {lowest}, not at {}",
            tl.bounds().y1
        );
    }

    /// `n` families, each `bytes` big, drawn in order — family 0 the oldest.
    fn browsed(n: usize, bytes: usize) -> (Vec<(u64, usize, String)>, usize) {
        let rows: Vec<(u64, usize, String)> = (0..n)
            .map(|i| (i as u64, bytes, format!("Family {i:03}")))
            .collect();
        (rows, n * bytes)
    }

    const MB: usize = 1024 * 1024;

    /// 60 CJK families at 2 MB a face are bounded by bytes, not by a family count.
    #[test]
    fn a_browse_through_heavy_families_is_bounded_by_bytes_not_by_count() {
        let (rows, total) = browsed(60, 2 * MB);
        let dropped = families_over_budget(rows, total);
        assert_eq!(dropped.len(), 36, "60 at 2 MB must come back under 48 MB");
        // Oldest-drawn first, so what is on screen is the last thing to go.
        assert_eq!(dropped.first().map(String::as_str), Some("Family 000"));
        assert_eq!(dropped.last().map(String::as_str), Some("Family 035"));
        assert_eq!((60 - dropped.len()) * 2 * MB, PREVIEW_BYTES);
    }

    /// The Latin case is left alone: 300 small families still fit the byte budget.
    #[test]
    fn a_browse_through_latin_families_evicts_nothing_where_the_count_evicted() {
        let (rows, total) = browsed(300, 150 * 1024);
        assert!(total < PREVIEW_BYTES, "300 Latin faces is {total} bytes");
        assert!(families_over_budget(rows, total).is_empty());
    }

    /// The floor wins over the budget, so heavy families cannot evict a screenful.
    #[test]
    fn the_floor_stops_eviction_before_it_reaches_the_rows_on_screen() {
        let (rows, total) = browsed(30, 20 * MB);
        let dropped = families_over_budget(rows, total);
        assert_eq!(dropped.len(), 30 - PREVIEW_FLOOR, "held down to the floor");
        // Under the floor to begin with, nothing goes however heavy it is.
        let (rows, total) = browsed(10, 20 * MB);
        assert!(families_over_budget(rows, total).is_empty());
    }

    #[test]
    fn a_weight_only_family_takes_the_wght_cut() {
        assert_eq!(
            variable_cut(["wght"].into_iter()).as_deref(),
            Some("wght"),
            "42dot Sans publishes latin-wght-normal and no latin-standard-normal"
        );
        assert_eq!(
            variable_cut(["wght", "ital"].into_iter()).as_deref(),
            Some("wght"),
            "ital is a second *file*, not a reason to take the standard cut"
        );
    }

    /// Roboto: `wght`+`wdth`+`ital`. Taking `wght` would download a file with no
    /// width axis in it, and the panel would then offer no width row for a family
    /// that has one.
    #[test]
    fn a_family_with_more_than_weight_takes_the_standard_cut() {
        assert_eq!(
            variable_cut(["wght", "wdth", "ital"].into_iter()).as_deref(),
            Some("standard")
        );
    }

    /// Agu Display varies on `MORF` alone: neither `wght` nor `standard` exists
    /// on the CDN for it, only `full`.
    #[test]
    fn a_family_with_no_weight_axis_takes_the_full_cut() {
        assert_eq!(variable_cut(["MORF"].into_iter()).as_deref(), Some("full"));
        assert_eq!(variable_cut([].into_iter()), None);
    }

    /// Web fonts off closes both doors: the picker's list and `faces_of`, which a
    /// document-named family reaches without the picker.
    /// flip: drop the `web_fonts` guard from `rebuild_families` → fails at the
    /// "off takes the web names out" assertion; drop it from `faces_of` → fails at
    /// the "refuses to fetch" assertion. No thread starts (the catalog is not empty).
    #[test]
    fn web_fonts_off_hides_the_catalog_and_refuses_to_fetch_from_it() {
        let ctx = egui::Context::default();
        let mut service = FontService::inert();
        service.catalog.insert(
            "Roboto".into(),
            CatalogEntry {
                id: "roboto".into(),
                family: "Roboto".into(),
                weights: vec![400],
                styles: vec!["normal".into()],
                subset: "latin".into(),
                cut: None,
                vf_italic: false,
            },
        );
        service.rebuild_families();

        // The fixture: with the switch on, the family is both listed and fetchable.
        assert!(
            service.families.contains(&"Roboto".to_string()),
            "the fixture: a catalog entry is a listed family, or the case below is \
             about nothing"
        );
        assert_eq!(
            service.faces_of("Roboto", Want::Preview).len(),
            1,
            "the fixture: and its regular face is what a preview asks for"
        );
        let listed = service.families.len();

        service.set_web_fonts(false, &ctx);
        assert!(
            !service.families.contains(&"Roboto".to_string()),
            "off takes the web names out of the picker's list"
        );
        assert_eq!(
            service.families.len(),
            listed - 1,
            "and takes out exactly them — Inter is still there"
        );
        assert!(
            service.faces_of("Roboto", Want::Preview).is_empty(),
            "off refuses to fetch a web face even for a family named by a document"
        );

        // And back on is a rebuild rather than a re-fetch: the entry never left.
        service.set_web_fonts(true, &ctx);
        assert!(
            service.families.contains(&"Roboto".to_string()),
            "on inside one session costs no traffic — the catalog was kept"
        );
        assert_eq!(service.faces_of("Roboto", Want::Preview).len(), 1);
    }

    /// Off cancels a catalog fetch that is already out (§15 D330), asserted on the
    /// cancel flag `spawn_catalog` reads before the network and on the way back.
    /// The last assertion holds the old handle: the service's own field has moved
    /// on. No thread starts (`inert()` has none; the catalog is not empty).
    /// flip: `set_web_fonts` stores `on` into the existing flag instead of
    /// cancelling → fails at the last assertion only.
    #[test]
    fn turning_web_fonts_off_cancels_a_catalog_fetch_that_is_already_out() {
        use std::sync::atomic::Ordering::Relaxed;

        let ctx = egui::Context::default();
        let mut service = FontService::inert();
        // Stands in for a live thread's handle; `inert()` starts none.
        let in_flight = Arc::new(std::sync::atomic::AtomicBool::new(true));
        service.catalog_live = Arc::clone(&in_flight);
        service.web_fonts = true;
        service.catalog.insert(
            "Roboto".into(),
            CatalogEntry {
                id: "roboto".into(),
                family: "Roboto".into(),
                weights: vec![400],
                styles: vec!["normal".into()],
                subset: "latin".into(),
                cut: None,
                vf_italic: false,
            },
        );
        assert!(
            in_flight.load(Relaxed),
            "the fixture is a fetch in flight, or this test is about nothing"
        );

        service.set_web_fonts(false, &ctx);
        assert!(
            !in_flight.load(Relaxed),
            "the switch going off has to reach the thread — otherwise two calls to \
             api.fontsource.org and a write to the user's disk still happen after \
             they said no"
        );

        // Back on: the catalog is not empty, so the service keeps this same handle.
        service.set_web_fonts(true, &ctx);
        assert!(
            !in_flight.load(Relaxed),
            "a cancelled fetch stays cancelled: the thread holding this flag was \
             told no, and a later yes is a different question"
        );
    }

    /// The primary face is the one the canvas shapes with while the rest are
    /// still in flight, so it has to be first in the list and alone in a preview.
    #[test]
    fn the_primary_web_face_leads_and_is_all_a_preview_asks_for() {
        let entry = CatalogEntry {
            id: "roboto".into(),
            family: "Roboto".into(),
            weights: vec![100, 400, 700],
            styles: vec!["normal".into(), "italic".into()],
            subset: "latin".into(),
            cut: None,
            vf_italic: false,
        };
        let dir = Path::new("/cache");
        let preview = web_faces(&entry, dir, Want::Preview);
        assert_eq!(preview.len(), 1);
        assert_eq!(preview[0].key, "roboto-latin-400-normal.ttf");

        let full = web_faces(&entry, dir, Want::Full);
        assert_eq!(full[0].key, preview[0].key, "the same file leads both");
        assert_eq!(full.len(), 6, "3 weights x 2 styles");
    }

    /// The whole point of the variable path: one file instead of six, and the
    /// same one file for a preview as for the full family.
    #[test]
    fn a_variable_family_fetches_one_file_for_every_weight() {
        let entry = CatalogEntry {
            id: "roboto".into(),
            family: "Roboto".into(),
            weights: vec![100, 200, 300, 400, 500, 600, 700, 800, 900],
            styles: vec!["normal".into(), "italic".into()],
            subset: "latin".into(),
            cut: Some("standard".into()),
            vf_italic: true,
        };
        let dir = Path::new("/cache");
        let full = web_faces(&entry, dir, Want::Full);
        assert_eq!(full.len(), 2, "upright + italic, not 18 static cuts");
        assert_eq!(full[0].key, "roboto-latin-standard-normal.vf.ttf");
        assert!(matches!(
            &full[0].source,
            FaceSource::Web { url, .. }
                if url == "https://cdn.jsdelivr.net/fontsource/fonts/roboto:vf@latest/latin-standard-normal.woff2"
        ));
        assert_eq!(web_faces(&entry, dir, Want::Preview).len(), 1);
    }

    fn keys(names: &[&str]) -> Vec<String> {
        names.iter().map(|n| n.to_lowercase()).collect()
    }

    /// The virtualized window must survive running off the end of the matches.
    #[test]
    fn the_match_window_is_clamped_to_what_matched() {
        let keys = keys(&["Inter", "Roboto", "Roboto Mono", "Rubik"]);
        let mut filter = Filter::default();
        assert_eq!(filter.sync("", &keys, 0), &[0, 1, 2, 3]);
        assert_eq!(filter.sync("robo", &keys, 0), &[1, 2]);
        assert_eq!(filter.sync("ROBO", &keys, 0), &[1, 2], "case-insensitive");

        // The window the scroll area asks for, one page past the end.
        let matched = filter.sync("robo", &keys, 0);
        let end = 40usize.min(matched.len());
        assert_eq!(&matched[30usize.min(end)..end], &[] as &[u32]);
    }

    /// The memo's second input, the list revision, is what keeps indices valid when
    /// the catalog lands or the system scan finishes after the picker opened.
    #[test]
    fn a_longer_family_list_invalidates_the_memo_even_with_the_same_needle() {
        let short = keys(&["Inter", "Roboto"]);
        let long = keys(&["Alegreya", "Inter", "Roboto"]);
        let mut filter = Filter::default();
        assert_eq!(filter.sync("inter", &short, 0), &[0]);
        assert_eq!(
            filter.sync("inter", &long, 1),
            &[1],
            "Inter moved to index 1 when the catalog landed"
        );
    }

    /// A family is never called missing before the sources have stopped arriving
    /// (§15 D227); the unsettled rows are what a cold start looks like.
    /// flip: drop `!sources_settled` from `status_of` → fails at the two unsettled
    /// rows; drop the `listed` arm → fails at the "no source offers it" row.
    #[test]
    fn a_family_is_pending_until_its_sources_have_stopped_arriving() {
        let facts = |drawable, in_flight, sources_settled, listed, tried_and_failed| FamilyFacts {
            drawable,
            in_flight,
            sources_settled,
            listed,
            tried_and_failed,
        };
        use FamilyStatus::{Missing, Pending, Ready};

        // Drawable wins over everything, including a past failure.
        assert_eq!(status_of(facts(true, false, true, true, true)), Ready);
        // In flight.
        assert_eq!(status_of(facts(false, true, true, true, false)), Pending);

        // Unsettled, and otherwise looking exactly like a missing family.
        assert_eq!(
            status_of(facts(false, false, false, false, false)),
            Pending,
            "an unlisted family before the catalog has been read is unknown, not missing"
        );
        assert_eq!(
            status_of(facts(false, false, false, false, true)),
            Pending,
            "and a failed attempt before that does not settle it either — the catalog \
             may yet list a family the system scan did not"
        );

        // Settled: now the two verdicts are real.
        assert_eq!(
            status_of(facts(false, false, true, false, false)),
            Missing,
            "no source offers it and none is coming"
        );
        assert_eq!(
            status_of(facts(false, false, true, true, true)),
            Missing,
            "listed, and everything tried came back unusable"
        );
        assert_eq!(
            status_of(facts(false, false, true, true, false)),
            Pending,
            "listed and never asked for is not a warning — asking would fetch it"
        );
    }

    /// The disk budget drops the least recently used and stops as soon as it is
    /// back under. Sizes and stamps deliberately disagree, so sorting on bytes or
    /// reading the order the other way both fail; the third case pins that being
    /// over budget does not mean emptying the cache.
    #[test]
    fn the_font_cache_drops_its_stalest_files_and_stops_at_the_budget() {
        const MB: u64 = 1024 * 1024;
        let at = |secs| SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(secs);
        let file = |name: &str, mb: u64, secs| (at(secs), mb * MB, PathBuf::from(name));

        let stale = file("stale", 200, 100);
        let older = file("older", 40, 200);
        let fresh = file("fresh", 40, 300);
        assert_eq!(
            files_over_budget(
                vec![fresh.clone(), stale.clone(), older.clone()],
                280 * MB,
                FONT_CACHE_BYTES
            ),
            vec![PathBuf::from("stale")],
            "the stalest file is 200 MB and answers the whole overage on its own"
        );

        assert!(
            files_over_budget(vec![older, fresh], 80 * MB, FONT_CACHE_BYTES).is_empty(),
            "under the budget nothing is touched"
        );

        let huge = file("huge", 200, 400);
        assert_eq!(
            files_over_budget(vec![huge, stale], 400 * MB, FONT_CACHE_BYTES),
            vec![PathBuf::from("stale")],
            "400 MB is 144 over, and one file answers it — being over the budget is \
             not a reason to empty the cache"
        );
    }

    /// The sweep against a real directory: reading sizes, skipping what is not a
    /// face file, and deleting. The subdirectory arm is insurance, not a live catch:
    /// deleting the `is_file` guard leaves it green (a directory has zero length and
    /// `remove_file` refuses it).
    #[test]
    fn the_sweep_deletes_the_stalest_faces_off_the_disk() {
        // Process-unique (`ondin-<what>-<pid>`): it opens by deleting the directory.
        let dir = std::env::temp_dir().join(format!("ondin-font-sweep-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::create_dir_all(dir.join("sub")).unwrap();

        let now = SystemTime::now();
        let face = |name: &str, ago: u64| {
            let path = dir.join(name);
            std::fs::write(&path, vec![0u8; 100]).unwrap();
            std::fs::OpenOptions::new()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(now - std::time::Duration::from_secs(ago))
                .unwrap();
            path
        };
        let (stale, middling, recent) =
            (face("a.ttf", 300), face("b.ttf", 200), face("c.ttf", 100));

        // 300 bytes against a 250-byte budget: one goes; the directory is not summed.
        trim_font_cache(&dir, 250);
        assert!(!stale.exists(), "the stalest face is the one that goes");
        assert!(middling.exists() && recent.exists(), "and only that one");
        assert!(dir.join("sub").is_dir(), "a directory is not a face file");

        // And a budget the directory is already under changes nothing.
        trim_font_cache(&dir, 250);
        assert!(middling.exists() && recent.exists());
    }

    /// A cache hit stamps the file, which makes `trim_font_cache`'s order "least
    /// recently used". flip: open with `File::open` in `touch` (no write access, so
    /// `set_modified` fails on Windows) → fails at "the stamp did not move".
    #[test]
    fn touching_a_cached_face_moves_its_stamp_forward() {
        // Process-unique, like the sweep test.
        let dir = std::env::temp_dir().join(format!("ondin-font-touch-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("face.ttf");
        std::fs::write(&path, b"x").unwrap();
        let long_ago = SystemTime::now() - std::time::Duration::from_secs(60 * 60 * 24 * 30);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&path)
            .unwrap()
            .set_modified(long_ago)
            .unwrap();

        touch(&path);

        let after = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert!(after > long_ago, "the stamp did not move");
        assert!(
            after.elapsed().unwrap() < std::time::Duration::from_secs(60),
            "the stamp moved, but not to now"
        );
        assert_eq!(
            std::fs::read(&path).unwrap(),
            b"x",
            "and nothing was written"
        );
    }

    /// The catalog gate: the absent-file case matters, or a first run never fetches.
    #[test]
    fn a_just_written_catalog_is_fresh_and_a_missing_one_never_is() {
        // Process-unique: it reads back a stamp a second run would overwrite.
        let dir = std::env::temp_dir().join(format!("ondin-catalog-age-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("catalog.json");
        std::fs::write(&path, b"[]").unwrap();

        assert!(is_fresh(&path, CATALOG_MAX_AGE), "written moments ago");
        assert!(
            !is_fresh(&path, std::time::Duration::ZERO),
            "a zero budget expires everything — the every-launch behaviour this replaced"
        );
        assert!(
            !is_fresh(&dir.join("no-such-file.json"), CATALOG_MAX_AGE),
            "nothing to trust, so fetch"
        );
    }

    #[test]
    fn the_nearest_offered_weight_stands_in_for_a_missing_400() {
        assert_eq!(nearest(&[100, 300, 500, 900], 400), 300);
        assert_eq!(nearest(&[], 400), 400);
        assert_eq!(nearest(&[700], 400), 700);
    }

    /// The whole pipeline, once: catalog → `ensure_loaded` → queue → worker →
    /// download → woff2 decode → `poll` → registered with core.
    ///
    /// Proves the pieces are wired together, and that the catalog arriving is
    /// `families_changed` only.
    // Network-gated: run with `cargo test -p ondin-app -- --ignored`.
    #[test]
    #[ignore = "requires network (Fontsource/jsDelivr)"]
    fn the_service_registers_a_web_family_end_to_end() {
        // A headless context; this test drives `poll` itself. `true`: this is the
        // web path.
        let mut service = FontService::new(&egui::Context::default(), true);
        // No popular-family prefetch: its faces set `fonts_registered` too, so the
        // poll carrying the catalog could carry a registration that is not its own.
        service.prefetched = true;

        // Wait for the catalog.
        let mut catalog_poll = None;
        for _ in 0..600 {
            let poll = service.poll();
            if poll.families_changed && !service.catalog.is_empty() {
                catalog_poll = Some(poll);
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let poll = catalog_poll.expect("the catalog landed within 12s");
        assert!(
            !poll.fonts_registered,
            "the catalog is metadata: it must not invalidate a single text layout"
        );
        assert!(service.families.len() > 500, "the picker has the library");

        let family = "Roboto";
        assert!(
            service.catalog.contains_key(family),
            "Roboto is in the Google catalog"
        );
        service.ensure_loaded(family);
        assert!(service.is_loading(family), "the field can say so");

        // Wait on Roboto itself: any face registering sets `fonts_registered`.
        for _ in 0..600 {
            service.poll();
            if ondin_core::text::is_family_available(family) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        assert!(
            ondin_core::text::is_family_available(family),
            "core can shape with it now"
        );
        assert!(
            ondin_core::text::family_preview(family, family, 15.0).is_some(),
            "and the picker can draw its row in it"
        );
        // Roboto's variable file carries the whole weight axis (§9.2 Font tab).
        let axes = ondin_core::text::family_axes(family);
        assert!(
            axes.iter().any(|a| a.tag.to_string() == "wght"),
            "the variable cut arrived, not a single static weight: {:?}",
            axes.iter().map(|a| a.tag.to_string()).collect::<Vec<_>>()
        );
    }

    // Network-gated: run with `cargo test -p ondin-app -- --ignored`.
    #[test]
    #[ignore = "requires network (Fontsource/jsDelivr)"]
    fn catalog_and_download_yield_a_shapeable_font() {
        let agent = ureq::Agent::new_with_defaults();
        let catalog = fetch_catalog(&agent).expect("fetch catalog");
        assert!(catalog.len() > 500, "expected a large catalog");
        let roboto = catalog
            .iter()
            .find(|e| e.family == "Roboto")
            .expect("Roboto in catalog");
        assert_eq!(
            roboto.cut.as_deref(),
            Some("standard"),
            "Roboto is variable on wght+wdth"
        );

        // Process-unique.
        let dir = std::env::temp_dir().join(format!("ondin-font-web-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let faces = web_faces(roboto, &dir, Want::Full);
        let bytes = load_face(&agent, &faces[0]).expect("download the primary face");
        assert!(is_sfnt(&bytes), "woff2 came back decoded to sfnt");

        // Core can register and shape with it.
        let names = ondin_core::text::register_fonts(bytes);
        assert!(
            names.iter().any(|n| n.contains("Roboto")),
            "registered family names include Roboto, got {names:?}"
        );
    }

    // --- the popular prefetch (§15 D352) -----------------------------------

    /// A catalog entry for `family`, static, regular only.
    fn entry(family: &str) -> CatalogEntry {
        CatalogEntry {
            id: family.to_lowercase().replace(' ', "-"),
            family: family.to_string(),
            weights: vec![400],
            styles: vec!["normal".into()],
            subset: "latin".into(),
            cut: None,
            vf_italic: false,
        }
    }

    /// The list is fifty distinct names without the bundled `Inter`. Says nothing
    /// about whether names are real; the `#[ignore]`d spelling test asks that.
    #[test]
    fn the_popular_list_is_fifty_distinct_families_and_none_of_them_is_inter() {
        let unique: std::collections::BTreeSet<_> = POPULAR.iter().collect();
        assert_eq!(
            unique.len(),
            POPULAR.len(),
            "a family is listed twice; the duplicate warms nothing extra and \
             misleads the next reader about the size of the set"
        );
        assert_eq!(POPULAR.len(), 50, "the set is fifty by design");
        assert!(
            !POPULAR.contains(&"Inter"),
            "Inter is bundled in core and never fetched, so warming it is one \
             wasted download"
        );
        assert!(
            POPULAR
                .iter()
                .all(|f| !f.trim().is_empty() && f.trim() == *f),
            "a name with stray whitespace matches no catalog entry and fails silently"
        );
    }

    /// Every name is spelled the way the live catalog spells it; a misspelling
    /// otherwise fails silently. Re-run after editing `POPULAR`, since Fontsource
    /// renames families.
    // Network-gated: run with `cargo test -p ondin-app -- --ignored`.
    #[test]
    #[ignore = "requires network (Fontsource)"]
    fn every_prefetched_family_is_spelled_the_way_the_catalog_spells_it() {
        let agent = ureq::Agent::new_with_defaults();
        let catalog = fetch_catalog(&agent).expect("the Fontsource catalog");
        let known: std::collections::BTreeSet<&str> =
            catalog.iter().map(|e| e.family.as_str()).collect();
        let missing: Vec<&&str> = POPULAR.iter().filter(|f| !known.contains(**f)).collect();
        assert!(
            missing.is_empty(),
            "{} of {} popular families are not in the catalog under these names, \
             so they are silently never warmed: {missing:?}",
            missing.len(),
            POPULAR.len()
        );
    }

    /// A preview the backlog pushed out is released, not left claimed (§15 D354),
    /// and not counted as `failed`, which would put the missing-font warning on a
    /// fine font.
    /// flip: discard `push_preview`'s return value in `ensure` → fails at the first
    /// assertion, the family still loading.
    #[test]
    fn a_preview_dropped_by_the_backlog_can_be_asked_for_again() {
        let mut service = FontService::inert();
        service.web_fonts = true;
        // One more family than the lane holds, so exactly the oldest is pushed out
        // (`queue::PREVIEW_BACKLOG`).
        let names: Vec<String> = (0..super::queue::PREVIEW_BACKLOG + 1)
            .map(|i| format!("Fam {i}"))
            .collect();
        for n in &names {
            service.catalog.insert(n.clone(), entry(n));
        }
        service.rebuild_families();
        for n in &names {
            service.ensure_preview(n);
        }

        let evicted = &names[0];
        assert!(
            !service.is_loading(evicted),
            "`{evicted}` was pushed out of the backlog and still reads as loading, \
             which it will now do for the whole session"
        );
        assert_eq!(
            service.state[evicted].failed, 0,
            "a job that was never attempted must not count as a failure, or the \
             family wears the missing-font warning"
        );
        // And the claim came back, so it can be asked for again.
        service.ensure_preview(evicted);
        assert!(
            service.is_loading(evicted),
            "`{evicted}` could not be re-requested, so its row can never draw in \
             its own face again"
        );

        // The newest is still queued, or the bound dropped the wrong end.
        let newest = names.last().expect("a fixture");
        assert!(
            service.is_loading(newest),
            "the fixture: the newest is in flight"
        );
    }

    /// A warmed family goes down the prefetch lane, and the switch refuses.
    /// Asserted through `FamilyState::lane`, which the variable-file retry reads.
    /// flip: pass `Lane::Preview` in `prefetch_popular` → fails at the lane
    /// assertion only; the wrong lane is invisible in every other observable.
    #[test]
    fn a_warmed_family_takes_the_prefetch_lane_and_the_switch_refuses_it() {
        let mut service = FontService::inert();
        service.web_fonts = true;
        let first = POPULAR[0];
        service.catalog.insert(first.to_string(), entry(first));
        service.rebuild_families();
        assert_eq!(
            service.faces_of(first, Want::Preview).len(),
            1,
            "the fixture: `{first}` has a web face to warm, or this tests nothing"
        );

        service.prefetch_popular();
        let state = service.state.get(first).expect("the family was warmed");
        assert_eq!(state.lane, Lane::Prefetch, "warmed down the wrong lane");
        assert_eq!(state.wanted, Want::Preview, "and it stays evictable");
        assert_eq!(state.outstanding, 1, "the regular upright, and only that");

        // The switch refuses in `prefetch_popular` itself, not by the catalog being
        // empty (D330).
        let mut off = FontService::inert();
        off.web_fonts = false;
        off.catalog.insert(first.to_string(), entry(first));
        off.rebuild_families();
        off.prefetch_popular();
        // Not `state.is_empty()`: a fresh service already holds an `Inter` entry.
        assert!(
            POPULAR.iter().all(|f| !off.state.contains_key(*f)),
            "web fonts are off and the popular set was queued anyway"
        );
    }

    /// The warm runs once, and a family the machine already has is not warmed.
    /// flip: remove `self.prefetched = true` in `prefetch_popular` → fails at the
    /// transition assertion (the "second run" one is weaker: `claimed` also stops
    /// a re-queue); drop the `FaceSource::Web` filter in `warm` → fails at the skip.
    #[test]
    fn the_warm_runs_once_and_skips_a_family_the_machine_already_has() {
        let mut service = FontService::inert();
        service.web_fonts = true;
        // `inert` starts this `true`; put it back or the assertion tests the
        // constructor.
        service.prefetched = false;
        let first = POPULAR[0];
        service.catalog.insert(first.to_string(), entry(first));
        service.rebuild_families();

        service.prefetch_popular();
        assert!(
            service.prefetched,
            "the flag did not flip, so `poll` would warm the popular set again on \
             every frame after the catalog lands"
        );
        let before = service.state[first].outstanding;
        service.prefetch_popular();
        assert_eq!(
            service.state[first].outstanding, before,
            "a second run queued the same family again"
        );

        // Installed rather than downloadable: a real face, since the branch only
        // exists when `faces_of` has system specs. `load_font_file`, not
        // `load_font_data`: `system_faces` skips anything but a `Source::File`.
        let mut db = fontdb::Database::new();
        db.load_font_file(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/fonts/Phosphor.ttf"
        ))
        .expect("the bundled icon font is in the tree");
        let name = db
            .faces()
            .next()
            .expect("one face loaded")
            .families
            .first()
            .expect("it names a family")
            .0
            .clone();

        let mut installed = FontService::inert();
        installed.web_fonts = true;
        installed.prefetched = false;
        installed.catalog.insert(name.clone(), entry(&name));
        installed.system = [name.clone()].into_iter().collect();
        installed.db = Some(db);
        installed.rebuild_families();
        // The fixture, asserted: `faces_of` takes the system branch and answers a
        // File spec, or the skip below passes for the wrong reason.
        let specs = installed.faces_of(&name, Want::Preview);
        assert!(
            matches!(specs.first().map(|s| &s.source), Some(FaceSource::File(_))),
            "the fixture: `{name}` must resolve to an installed file, got {:?}",
            specs.len()
        );

        installed.warm(&[&name]);
        assert!(
            !installed.state.contains_key(&name),
            "`{name}` is installed on this machine, so warming it registers a face \
             the picker was never going to ask the network for"
        );
    }

    /// A catalog entry whose `id` or `subset` is a path is dropped, and the
    /// persisted catalog is filtered on the way back in too.
    /// flip: drop the filter from `read_catalog` → fails at the length assertion on
    /// `"../../evil"`.
    #[test]
    fn a_catalog_entry_whose_id_is_a_path_is_dropped() {
        assert!(is_catalog_token("noto-sans-jp"));
        assert!(is_catalog_token("abril-fatface"));
        for bad in [
            "../../evil",
            "..",
            "sub/dir",
            r"sub\dir",
            r"C:\Users\Public\x",
            "/etc/x",
            "Inter",
            "inter_tight",
            "",
        ] {
            assert!(!is_catalog_token(bad), "{bad:?}");
        }

        let dir = std::env::temp_dir().join(format!("ondin-catalog-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("catalog.json");

        let mut escaped = entry("Inter");
        escaped.id = "../../evil".into();
        let mut escaped_subset = entry("Roboto");
        escaped_subset.subset = "../latin".into();
        let good = entry("Noto Sans JP");
        write_catalog(&path, &[escaped, escaped_subset, good.clone()]);

        let read = read_catalog(&path).expect("a catalog with one good entry still loads");
        assert_eq!(
            read.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            vec![good.id.as_str()],
            "only the entry that cannot become a path survives the re-read"
        );

        // An entirely bad catalog reads as none, so the next launch fetches again.
        write_catalog(
            &path,
            &[{
                let mut e = entry("Inter");
                e.id = "../../evil".into();
                e
            }],
        );
        assert!(read_catalog(&path).is_none());
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every request deadline is finite (not what it is: the numbers are tunable),
    /// on the shared agent and the inert one alike.
    /// flip: `ureq::Agent::new_with_defaults()` in `agent()` → fails at "connect".
    #[test]
    fn every_request_deadline_is_finite() {
        let inert = FontService::inert();
        for (what, a) in [("shared", &agent()), ("inert", &inert.agent)] {
            let t = a.config().timeouts();
            for (name, v) in [
                ("connect", t.connect),
                ("recv_response", t.recv_response),
                ("recv_body", t.recv_body),
                ("global", t.global),
            ] {
                assert!(v.is_some(), "{what} agent has no {name} deadline: {v:?}");
            }
        }
    }

    /// A service holding one variable family, with a channel the test owns.
    ///
    /// `inert()` drops its own sender, so the receiver is replaced here.
    fn service_with_a_variable_family() -> (FontService, std::sync::mpsc::Sender<FontMsg>) {
        let (tx, rx) = channel();
        let mut svc = FontService::inert();
        svc.msg_rx = rx;
        svc.catalog.insert(
            "Roboto".to_string(),
            CatalogEntry {
                id: "roboto".to_string(),
                family: "Roboto".to_string(),
                weights: vec![400, 700],
                styles: vec!["normal".to_string()],
                subset: "latin".to_string(),
                cut: Some("standard".to_string()),
                vf_italic: false,
            },
        );
        (svc, tx)
    }

    fn a_failed_variable_face(reason: FaceFailure) -> FontMsg {
        FontMsg::Failed {
            family: "Roboto".to_string(),
            key: "roboto:vf".to_string(),
            variable: true,
            reason,
        }
    }

    /// A variable file that failed to download is still a variable family; the
    /// `Decode` control guards against a build that never downgrades.
    /// flip: remove the `reason == FaceFailure::Decode` term in `poll` → fails at
    /// "a download failure took the family's variable file away", `Decode` half green.
    #[test]
    fn a_variable_file_that_failed_to_download_is_still_a_variable_family() {
        let (mut svc, tx) = service_with_a_variable_family();
        let cut = |s: &FontService| s.catalog["Roboto"].cut.clone();
        assert_eq!(
            cut(&svc),
            Some("standard".to_string()),
            "the fixture: the family starts with a variable file"
        );

        tx.send(a_failed_variable_face(FaceFailure::Fetch))
            .expect("the test owns the channel");
        svc.poll();
        assert_eq!(
            cut(&svc),
            Some("standard".to_string()),
            "a download failure took the family's variable file away"
        );

        // The control: the case §5.4a is actually about still falls back.
        let (mut svc, tx) = service_with_a_variable_family();
        tx.send(a_failed_variable_face(FaceFailure::Decode))
            .expect("the test owns the channel");
        svc.poll();
        assert_eq!(
            cut(&svc),
            None,
            "the control: a file the decoder cannot read must still fall back to \
             the static cuts"
        );
    }
}
