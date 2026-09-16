//! Font discovery + on-demand loading for the app (§5.4a).
//!
//! Core (`ondin-core`) shapes text but never touches the filesystem or network;
//! it only knows the bundled Inter plus whatever bytes it's handed via
//! `ondin_core::text::register_fonts`. This service is the app-side supplier:
//!
//! - **System fonts** — enumerated via `fontdb` on a background thread.
//! - **Web fonts** — the Google Fonts library, delivered as plain files by
//!   Fontsource over the jsDelivr CDN (Google's own endpoints now serve
//!   subsetted/obfuscated data). The catalog is cached on disk and refreshed in
//!   the background; a family's files download on demand. **Switchable, and off
//!   means no request** — see [`FontService::set_web_fonts`], which is the whole
//!   of the difference between this and the two sources above it.
//!
//! All I/O happens here, never in core. Registration into core is per-thread, so
//! bytes are always registered on the UI thread in [`FontService::poll`].
//! Until a family arrives, text renders with the Inter fallback.
//!
//! ## What makes it feel instant
//!
//! Four decisions, each of which was a visible stall before it was made:
//!
//! 1. **Nothing blocks the UI thread.** Even a system font — a local file — is
//!    read on a worker; only `register_fonts` happens on the UI thread, because
//!    the shaping engine is thread-local. The `fontdb` scan of every installed
//!    font is a background thread too, so it is off the startup path.
//! 2. **One [`ureq::Agent`], shared.** `ureq`'s free functions (`ureq::get`) each
//!    build a *use-once* agent, so every file paid a fresh DNS lookup, TCP
//!    connect and TLS handshake. Four faces of one family meant four handshakes,
//!    serially. A long-lived agent keeps the connection alive and the pool warm.
//! 3. **The primary face is dispatched first and registered on its own.** Text
//!    swaps to the real typeface when the *first* file lands rather than the
//!    last, which is the difference between "instant" and "instant, for families
//!    with only one cut". It matters most where it is worst: a CJK family's
//!    default subset is megabytes per weight.
//! 4. **Previewing is prefetching.** The picker draws each row in the family it
//!    names (§9.2), which means the row's regular face — exactly the file
//!    applying the family needs — is already on disk and registered by the time
//!    the user clicks. Fontsource serves static files, so there is no
//!    name-sized subset to fetch instead; that turns out to be the feature.
//!
//! Because (4) registers font data for every family the user scrolls past, the
//! service also has to be able to *let go* — see [`FontService::trim_previews`].

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
/// **Bytes, not families, and the difference is three orders of magnitude.** This
/// was a count of 240, whose own comment gave the reason a count is the wrong
/// unit: a face costs tens of KB for a Latin subset and *megabytes* for a CJK one,
/// so 240 families of Noto CJK is a working set two or three orders of magnitude
/// larger than 240 Latin ones for the same number. What has to be bounded is the
/// memory, so the memory is what the bound counts. The in-memory twin of the disk
/// bound in §14.
///
/// 48 MB is chosen to leave the Latin case as it was — 240 families at the ~150 KB
/// a decoded Latin face runs to is about 36 MB, so the old working set still fits
/// and scrolling back up still finds the rows drawn — while a browse through CJK
/// families settles around twenty instead of 240.
const PREVIEW_BYTES: usize = 48 * 1024 * 1024;

/// Families kept whatever they weigh.
///
/// **A floor, not a second cap, and it is the anti-thrash guard.** A byte budget
/// alone will evict families that are *on screen* as soon as a few heavy ones fill
/// it — the picker then re-fetches them on the next frame, and re-evicts them on
/// the one after. Just above a screenful, which the list's own doc puts at about
/// twenty rows, so what is visible is never what is dropped.
///
/// **Kept small deliberately.** A generous floor is a second, weaker cap in
/// disguise: at 48 it would be the binding constraint for exactly the case
/// [`PREVIEW_BYTES`] exists for, since 48 CJK families is already ~115 MB and the
/// byte budget would never get to speak.
const PREVIEW_FLOOR: usize = 24;

/// How much downloaded font data stays on disk, in bytes.
///
/// **The disk twin of [`PREVIEW_BYTES`], and the same unit for the same reason**:
/// a Latin face is tens of KB where a CJK one is megabytes, so a count of files
/// bounds nothing. What is bounded is the megabytes, so megabytes are what it
/// counts.
///
/// 256 MB, chosen so the case that is not a problem is never touched: the whole
/// Google Latin catalog previewed end to end is about 85 MB (§15 D237, which took
/// the estimate from §14 before that section became a pointer), and a user who
/// has browsed it should not have to fetch it again. What the budget is *for* is
/// the browse through CJK families, where twenty rows can be a couple of hundred
/// megabytes on their own. Above this the oldest go.
///
/// **A pruned file costs one re-download and nothing else.** The cache is derived
/// data — never the source of truth for anything — which is what makes a bound
/// safe to apply without asking.
const FONT_CACHE_BYTES: u64 = 256 * 1024 * 1024;

/// The families warmed at startup so the picker is worth opening on a machine
/// with no network (§15 D352).
///
/// **An editorial list, and there is no alternative to one.** The Fontsource
/// endpoint carries eleven keys and not one of them is a rank, a count or a
/// trend, and the order is alphabetical rather than popular — checked against the
/// live endpoint, and recorded in full in §15 D330. So "popular" is a
/// judgement this app owns, it will go stale by itself, and its failure mode —
/// *my font is not listed* — is answered by the picker still listing all 1,976.
/// Nothing here is hidden from anyone; this only decides what is on disk early.
///
/// **Fifty, at the regular upright only.** That is `Want::Preview`'s single face
/// per family, so about 7 MB — a rounding error against [`FONT_CACHE_BYTES`]'s
/// 256 MB, and it cannot crowd [`PREVIEW_BYTES`] because the fifty are evictable
/// like any other preview.
///
/// ⚠️ **A misspelling here fails completely silently** — `faces_of` finds no
/// catalog entry, returns no specs, and the family is simply never warmed. Every
/// name below was checked against a real `catalog.json` (1,976 Google families,
/// the count §15 D330 records) rather than typed from memory, and
/// `every_prefetched_family_is_spelled_the_way_the_catalog_spells_it` re-checks
/// them against the **live** endpoint. ⚠️ That test is `#[ignore]`d, so it does
/// **not** run by default and nothing in the ordinary suite can tell you a name
/// here has rotted: `cargo test -p ondin-app -- --ignored` after editing this
/// list, and expect to need it, since Fontsource renames families (`Source Sans
/// Pro` became `Source Sans 3`).
/// **`Inter` is deliberately absent**: core registers it itself, so warming it
/// would be one download for a family that is never fetched.
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
/// fresh one. See the gate in [`FontService::new`] for why there is one at all.
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
/// **Separate from [`Want`], which it used to be derived from** (§15 D352).
/// `Want` says *how much of a family* to fetch and `Lane` says *how badly it is
/// wanted*, and the prefetch is the case that pulls them apart: it asks for
/// `Want::Preview`'s single face, exactly as a picker row does, and must be
/// served after every picker row rather than alongside them. Deriving one from
/// the other was correct for as long as there were only two of each.
/// **Declaration order is priority order, most urgent first**, and `Ord` is
/// derived so `min` is "the more urgent of the two" — which is how
/// [`FamilyState::lane`] is promoted and never downgraded. Reordering these
/// variants silently reverses that.
///
/// ⚠️ **The default is the *weakest* lane, and it has to be**, because it is the
/// identity for that `min`: a fresh [`FamilyState`] must not out-rank the request
/// that created it. This was `Preview` for its first few minutes, on the argument
/// that it should match [`Want`]'s default — and every prefetch then came out as
/// `Preview`, since `Preview.min(Prefetch)` is `Preview`. Caught by
/// `a_warmed_family_takes_the_prefetch_lane_and_the_switch_refuses_it` on its
/// first run, and by nothing else: the family is warmed either way, and the only
/// symptom on a real machine would be a picker stuttering while fifty speculative
/// downloads elbow into its bounded queue.
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

/// What one [`FontService::poll`] found. The halves are deliberately separate: a
/// longer family list is a repaint, but only *font data* invalidates the
/// shaped-text cache (§5.9), and invalidating it on the catalog's arrival
/// re-shaped every text node in the document for a change that added no fonts.
///
/// ⚠️ **No longer `Copy`** (§15 D591), because [`Self::registered`] carries the
/// families the data belongs to and the two booleans were not enough to decide
/// whether the document's cache should go.
#[derive(Clone, Default, Debug, PartialEq, Eq)]
pub struct FontPoll {
    /// The picker's list changed.
    pub families_changed: bool,
    /// Font data was registered with core, so **something** on screen may draw
    /// differently — a picker row's own preview strip, at least.
    pub fonts_registered: bool,
    /// Which families that data belongs to, lowercased (§15 D591,
    /// `[A4-L4-04]`).
    ///
    /// 🚨 **`fonts_registered` alone is too coarse for the document's cache.**
    /// The picker fetches preview faces continuously while it is scrolled — a
    /// backlog of 48 — and every frame on which any of them landed invalidated
    /// *every* text node in the document, ~23 ms on a 20-block document, for
    /// faces belonging to families the document does not name and that cannot
    /// change a single glyph of it. It recurred for as long as the scroll
    /// produced arrivals, which is exactly the interaction that has to be smooth
    /// and the reason the picker's row list was virtualized in the first place.
    ///
    /// **Half of this distinction already existed** — §5.4a and §15 D352 say a
    /// longer family *list* is a repaint and not a re-shape — and the next one
    /// along was missing: whether the arriving family is one the document
    /// references. A document naming a family that was unresolvable and has just
    /// landed still invalidates, which is the case the old code was right about.
    ///
    /// Lowercased because that is this module's own key form (`FontService::keys`)
    /// and a family name's case is not part of its identity here.
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

/// Why a face did not load — the distinction the variable-family fallback turns
/// on (§15 D585, `[S21-L1-04]`).
///
/// **§5.4a's rule is about the *decoder* and the code was applying it to the
/// network.** *"A family whose file the decoder cannot read falls back to the
/// static cuts automatically"*, and `FaceSpec::variable`'s own doc says the same —
/// but `FontMsg::Failed` carried no reason, so one 503 on
/// `roboto:vf@latest/…woff2` set `entry.cut = None` and Roboto was a *static*
/// family for the rest of the session: no `wght` or `wdth` rows in the Font tab,
/// the static cuts in place of the nine named instances, six files fetched where
/// one would have done. Nothing retried, nothing said so, and the disk catalog was
/// untouched — so a restart fixed it and the user had no way to know that.
///
/// **A transport failure leaves `cut` alone**, so the next `ensure` for that
/// family asks for the variable file again, which is what reopening the picker
/// ought to do.
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
/// **The third state a missing-font warning needed.** `text::is_family_available`
/// answers "not here yet" and "never coming" identically, so a warning built on it
/// alone accuses a font that is still downloading — which is why the warning existed
/// in the MCP snapshot and in no panel (§5.4a). This is the distinction, and the
/// boundary that matters is [`Self::Pending`] against [`Self::Missing`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FamilyStatus {
    /// Registered: core can shape and draw with it now.
    Ready,
    /// Not drawable **yet**, for one of three reasons that all mean *wait*: a face
    /// is in flight, a source that could still list the family has not landed, or
    /// nothing has asked for it. One word for the three because the UI has one
    /// thing to say about them — and because promoting any of them to a warning is
    /// exactly the accusation this enum exists to avoid.
    Pending,
    /// Not drawable **ever**: every source has landed and none lists this family,
    /// or every face of it that was tried came back unusable. This is the one that
    /// earns `theme::color::WARN`.
    Missing,
}

/// The facts [`FontService::family_status`] weighs.
///
/// **A struct rather than five positional `bool`s**, which is a swap waiting to
/// happen, and split out from the service so [`status_of`] is a truth table a test
/// can walk rather than a chain of `if`s inside a method that needs threads, a disk
/// cache and a network to exist.
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
    // **`!sources_settled` outranks everything below it.** A family the catalog has
    // not been read for is not missing, it is unknown, and the difference is the
    // whole point of this function: D175 is the record of a document opened before
    // the catalog landed, where every answer about its fonts was wrong for a while.
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
    /// Summed as each face arrives, because that is the only moment the size is
    /// known: `register_fonts` takes the `Vec<u8>` by value and core does not
    /// report what it kept. So this is the *decoded file* size rather than
    /// parley's own footprint, which is the honest thing to measure anyway — a
    /// woff2 is decompressed before it gets here, and the blob is what is held.
    bytes: usize,
    /// The most that has been asked of this family. `Preview` is the evictable
    /// state; `Full` never goes back.
    wanted: Want,
    /// The most urgent lane this family has been asked down, so the variable-file
    /// retry in [`FontService::poll`] goes back down the same one (§15 D352).
    ///
    /// **Stored rather than derived from [`wanted`](Self::wanted), which is what
    /// the retry used to do.** The two agreed while there were two lanes and two
    /// `Want`s; a prefetch is `Want::Preview` on the *third* lane, so deriving
    /// would send its retry to the picker's bounded LIFO — promoting it ahead of
    /// the prefetches behind it and letting it evict a row the user is looking at,
    /// which is the one thing the third lane exists to prevent.
    ///
    /// Never downgrades, [`wanted`](Self::wanted)'s rule: a family that was warmed
    /// and is then picked is urgent from then on.
    lane: Lane,
    /// The poll on which a picker row last drew this family — the LRU stamp
    /// [`FontService::trim_previews`] evicts by.
    touched: u64,
    /// Faces of this family that came back unusable — a download that failed, or
    /// bytes core declined to register (§15 D227).
    ///
    /// **The fact `poll` used to throw away.** `FontMsg::Failed` reached it and was
    /// read only for the variable-file retry; the *count* is what lets
    /// [`FamilyStatus::Missing`] be said about a family the catalog lists and cannot
    /// actually deliver, which is otherwise indistinguishable from one nobody has
    /// asked for.
    ///
    /// A count rather than a flag, because a family dispatches several faces and
    /// "one of them failed" is not the same claim as "all of them did" — it is only
    /// a verdict alongside `faces` being empty.
    failed: usize,
}

/// The picker's memoized filter result: which family indices matched, for what
/// needle, at what revision of the family list.
///
/// Memoized because the list is drawn every frame and the needle changes only on
/// a keystroke — and because the alternative was lowercasing ~1900 family names
/// on each of those frames.
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
    /// `families`, lowercased and parallel to it. The picker filters ~1900 names
    /// against every keystroke; lowercasing them on each pass was measurable.
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
    /// The cancel flag for the catalog thread that is *currently* the live one
    /// (§15 D720, `[S21-L5-06]`).
    ///
    /// **`set_web_fonts(false)` has to be able to stop a fetch that is already
    /// out**, or the switch means "no *new* requests" rather than what §15 D330
    /// promises: *"off means no request leaves the machine."* Without this, a user
    /// who turned the switch off while `spawn_catalog` was inside `fetch_catalog`
    /// still had two calls go to `api.fontsource.org`, still had `catalog.json`
    /// written to their disk, and still had the entries merged into `catalog` —
    /// all of it after they had said no.
    ///
    /// 🚨 **One flag per spawn, not one flag on the service, and the difference is
    /// a real race.** A shared flag would be set `false` by the off and `true`
    /// again by the next on — and a thread blocked inside `fetch_catalog` across
    /// both would come back, find it `true`, and write the response the user had
    /// cancelled. Each thread instead holds a flag that only ever goes `false`, so
    /// a cancelled fetch stays cancelled whatever the switch does afterwards.
    ///
    /// ⚠️ **It does not interrupt a `ureq` call in flight**; nothing here can.
    /// What it guarantees is that the *result* is dropped rather than written or
    /// merged, and that the second of the two calls is never made. The socket
    /// already open is the residual, and it is bounded by one request.
    catalog_live: Arc<std::sync::atomic::AtomicBool>,
    /// Whether the catalog thread has finished, however it finished (§15 D227).
    ///
    /// **Read off the channel closing rather than from a message of its own.** That
    /// thread sends up to two batches — the disk copy, then a refreshed fetch — and
    /// then returns, on all three paths: a fresh disk copy, a successful fetch, and a
    /// fetch that failed. Dropping the sender is therefore the one signal that covers
    /// all of them, and `try_recv` reports it as `Disconnected` for free. A "done"
    /// message would have to be sent from three places and would be missing from
    /// whichever one somebody forgot.
    ///
    /// Needed because *nothing* can be concluded about an unresolvable family until
    /// the sources have stopped arriving — [`status_of`].
    catalog_done: bool,
    /// Whether [`Self::prefetch_popular`] has already run this session
    /// (§15 D352).
    prefetched: bool,
    msg_rx: Receiver<FontMsg>,
    /// Whether the web library is offered at all (`prefs::Prefs::web_fonts`,
    /// §15 D330).
    ///
    /// **A filter over the catalog rather than a second catalog.** Off does not
    /// forget what has been fetched — [`Self::rebuild_families`] leaves the web
    /// names out of the picker's list and [`Self::faces_of`] refuses to dispatch a
    /// web face — so turning it back on inside a session is a rebuild rather than a
    /// re-fetch. What off *does* prevent is the network: at startup the catalog
    /// thread is not spawned at all, and no face can be queued because nothing asks
    /// for one.
    web_fonts: bool,
    /// The agent and the catalog path, kept so [`Self::set_web_fonts`] can start the
    /// catalog thread mid-session — the one thing about this service that is decided
    /// after construction.
    agent: ureq::Agent,
    catalog_path: PathBuf,
    /// Total bytes in [`Self::cache_dir`] as of the last [`Self::measure_cache`],
    /// or `None` if nothing has asked yet.
    ///
    /// **`None` and `Some(0)` are different answers and the UI says so.** An empty
    /// cache is a fact worth showing; a scan that has not finished is not, and a row
    /// that renders "0 MB" for both would read as "clear" while the directory held
    /// two hundred megabytes.
    cache_bytes: Option<u64>,
    /// A scan in flight. Its own channel rather than a message on `msg_rx`, so the
    /// answer cannot be mistaken for a face.
    size_rx: Option<Receiver<u64>>,
}

impl FontService {
    /// `ctx` is cloned into every background thread, which calls
    /// `request_repaint` after each send.
    ///
    /// **Without it an arriving face does not wake the UI thread.** eframe here
    /// is reactive: the workers only ever `send`, and the one `request_repaint`
    /// after [`Self::poll`] is inside a frame that had to happen for some other
    /// reason — so the *first* arrival after the UI went quiet was stranded, and
    /// the rows in a scrolled-and-released family list kept the fallback font
    /// until the mouse moved. It survived hand-testing because pointer motion
    /// generates frames continuously, and once any frame picks up one message
    /// [`FontPoll::any`] self-sustains the chain; sitting still reading the list
    /// is the case that had no tick at all.
    ///
    /// `web_fonts` is `prefs::Prefs::web_fonts`, and with it `false` **this makes no
    /// network call at all** — the catalog thread is never spawned, so the receiver
    /// is disconnected from the first `poll` and the service reports *no catalog,
    /// finally* rather than *not yet*. See [`Self::set_web_fonts`] for the other
    /// half — the switch moving inside a session.
    pub fn new(ctx: &egui::Context, web_fonts: bool) -> Self {
        let cache_dir = dirs::cache_dir()
            .unwrap_or_else(std::env::temp_dir)
            .join("ondin");
        let fonts_dir = cache_dir.join("fonts");
        let _ = std::fs::create_dir_all(&fonts_dir);
        let catalog_path = cache_dir.join("catalog.json");

        // Bound the disk cache before anything asks it for a face. Its own thread
        // because it is a directory scan and the app has nothing to do with the
        // answer — and because it must not be the thing that delays the picker's
        // first frame. See [`trim_font_cache`] for why once, at startup, is where
        // this belongs.
        let sweep_dir = fonts_dir.clone();
        std::thread::spawn(move || trim_font_cache(&sweep_dir, FONT_CACHE_BYTES));

        // One agent for every request this process makes. See the module docs:
        // the free `ureq::get` builds a use-once agent, which is a fresh TLS
        // handshake per font file. The deadlines on it are `agent()`'s.
        let agent = agent();

        // Enumerate system fonts off the startup path. `load_system_fonts`
        // parses the name table of every installed file, and the app has nothing
        // to do with the answer until a picker is opened.
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

        // The catalog — see [`spawn_catalog`]. **Skipped entirely when web fonts
        // are off**, and skipped by never starting the thread rather than by
        // discarding what it sends: the whole point of the switch is that no
        // request leaves the machine. `channel()`'s sender is dropped on the spot,
        // which is the same "that source has stopped arriving" signal a finished
        // thread gives (see `catalog_done`), so nothing downstream needs a second
        // spelling of "there will be no catalog".
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
                    // Including a `Failed`: it clears `outstanding`, which is
                    // what the field's "loading" state reads.
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
    /// `OndinApp::headless` (§15 D303).
    ///
    /// **`new` spawns five threads** — the cache sweep, the system-font scan, the
    /// catalog fetch (which makes a *network* call) and `WORKERS` face loaders —
    /// and creates a cache directory on the way past. All of that is right for an
    /// app and wrong for a test, which would otherwise pay a font enumeration and
    /// an HTTP request per case and be at the mercy of both.
    ///
    /// **The receivers are live channels whose senders are already dropped**, which
    /// is what makes this inert rather than half-built: `poll` reads them with
    /// `try_recv` and gets `Disconnected`, which is exactly the "that source has
    /// stopped arriving" signal `catalog_done` is documented to read off a closed
    /// channel. So a headless service reports *no system fonts and no catalog,
    /// finally* rather than *not yet*, and `status_of` can reach its conclusions
    /// instead of waiting for ever.
    ///
    /// Inter is present, because core registers it itself and every text node
    /// starts in it — a service that claimed otherwise would be lying about the one
    /// family that is always there.
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
            // Inert: no thread was started, so there is nothing to cancel, and
            // `false` is the honest reading of "no live catalog fetch".
            catalog_live: Arc::new(std::sync::atomic::AtomicBool::new(false)),
            catalog_done: false,
            // An inert service has no workers and no network, so there is nothing
            // to warm; saying it is already done keeps `poll` from trying.
            prefetched: true,
            msg_rx,
            // **`true`, and it costs nothing**: there is no catalog and no thread to
            // fetch one, so the flag only decides whether web *names* would be listed
            // if any existed. `false` here would make a headless service a
            // web-fonts-off service, which is a second thing to remember about it and
            // the wrong default for `set_web_fonts`'s own tests to start from.
            web_fonts: true,
            // Constructing an agent opens no connection. The same one the live
            // service builds, deadlines included — an inert service that could
            // be made to fetch would otherwise fetch without them.
            agent: agent(),
            catalog_path: std::env::temp_dir().join("ondin-headless-never-read.json"),
            cache_bytes: None,
            size_rx: None,
        }
    }

    /// The attributes new text nodes start with. Inter is bundled in core, so it
    /// is the one family guaranteed to be present on every machine and offline.
    ///
    /// Everything not named here is the type's own default, which is what keeps a
    /// freshly created node out of the save file's optional keys: a new text node
    /// writes a family, a size, a weight, a line height **and a box trim**, and
    /// nothing else.
    ///
    /// **The trim is named here rather than made the type's default, and the
    /// distinction is the whole of D78's compatibility guarantee.** `BoxTrim`
    /// serializes with `skip_serializing_if`, so every text node written before this
    /// existed omits the field — moving `BoxTrim`'s own `#[default]` would therefore
    /// re-measure every text node in every saved file, tighter, changing its W/H
    /// fields, what align and distribute do with it, and its export viewBox. So
    /// `None` goes on meaning what an absent field has always meant, and this is
    /// where "trimmed by default" actually lives: the app *authors* a trim, the
    /// format does not assume one. The cost is the honest one — text in a file made
    /// before this gets the tight box only when someone toggles it in the Block tab.
    ///
    /// **No receiver, because it reads no service state.** It was a method taking
    /// `&self` and using none of it, which is the spelling that made the app's single
    /// most consequential authoring decision reachable only by standing up a
    /// `FontService` — threads, a cache directory and a system-font enumeration — so
    /// nothing tested it. It stays *on* `FontService` rather than becoming a free
    /// function because that is still whose business it is, and because if it ever
    /// does need session state — the last family the user picked is the obvious
    /// candidate — it takes `&self` again with no call site moving.
    pub fn default_text_parts() -> ondin_core::text::TextParts {
        ondin_core::text::TextParts {
            style: ondin_core::TextStyle {
                font_family: "Inter".into(),
                font_size: 24.0,
                // A stated 120% rather than Auto: a designer's default is a round
                // number they can reason about, where the font's own leading is a
                // different value in every family.
                line_height: Some(ondin_core::Length::Em(1.2)),
                ..ondin_core::TextStyle::default()
            },
            // **Cap height to baseline — the box a designer sees.** A text node's
            // box is what every gesture and every measurement reads, and the full
            // line box is padded by leading nobody drew: aligning two labels by it
            // lines up their line boxes rather than their letters, and measuring to
            // it quotes a distance to empty space. `AscentToDescent` was the other
            // candidate and contains all the ink, which is its own argument — but it
            // still includes the parts of the em nobody is looking at, and cap-to-
            // baseline is what Figma's Vertical trim and Graphite's tight box both
            // mean. Descenders fall outside the box as a result, which is why the
            // untrimmed box is *drawn* (`TextLayout::untrimmed_bounds`) rather than
            // simply discarded.
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
    /// **The picker is virtualized on this count** (§9.2): it draws the rows in
    /// view and no others. Building a widget per family instead cost about a
    /// second every time the list was opened, ~1900 rows of layout, id hashing
    /// and glyph rasterization for the twenty a scroll area then showed.
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
    /// **Fired when the catalog has finished arriving, not from `new`.** A web
    /// family's [`FaceSpec`] is built out of its `CatalogEntry`, so before the
    /// catalog lands `faces_of` answers nothing at all and this would be fifty
    /// no-ops. `catalog_done` is the existing signal for "that source has stopped
    /// arriving" and it is already correct on every path — including the one where
    /// the switch is off, where it is true from the start with an empty catalog, so
    /// the `web_fonts` guard is what stops this making requests rather than the
    /// timing.
    ///
    /// **Only families the catalog answers for.** `faces_of` resolves a *system*
    /// family to its installed file first (§5.4a's order), so a machine with
    /// Roboto installed gets no download and no registration from this — there is
    /// nothing to warm, the file is already local. Filtering on `FaceSource::Web`
    /// is what keeps this from spending the preview budget on faces that were never
    /// going to be fetched.
    ///
    /// ⚠️ **Turning the switch off does not cancel a warm already queued.** The
    /// jobs are in the queue by then and `set_web_fonts` does not drain it, so up
    /// to fifty requests can still go out after the user has said no. It is
    /// bounded and brief, and it is called out rather than fixed because the fix —
    /// draining a lane from the service — wants the queue to grow an operation
    /// that nothing else needs.
    ///
    /// 🚨 **This said *"that is **the one hole**"* and it was not** (§15 D720,
    /// `[S21-L5-06]`). The catalog thread was a second, and the more visible one:
    /// it talks to `api.fontsource.org` rather than to the CDN, and it *wrote to
    /// the user's disk* on the way back. It is closed now (`catalog_live`), which
    /// leaves this one — so the sentence is true again by arithmetic and would go
    /// wrong the same way the next time somebody adds a caller.
    ///
    /// **So the claim is stated as a rule rather than as a count.** The hole is
    /// *requests already queued or already in flight when the switch is turned
    /// off*; this warm is the queued case and it is the only one left. Anything
    /// new that reaches the network must either be startable only while the
    /// switch is on, or hold a cancel flag the way `spawn_catalog` does — and
    /// `grep -n 'agent' crates/ondin-app/src/fonts/mod.rs` is the enumeration,
    /// rather than a number in this paragraph.
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
    /// **Takes the list rather than reading [`POPULAR`], so the skip can be
    /// tested.** The rule needs a fixture with a *real* installed face to exercise
    /// at all — an empty `fontdb` makes `faces_of` answer nothing and the filter
    /// then looks like it works whether it is there or not, which is exactly what
    /// the first version of `the_warm_runs_once_and_skips_a_family_the_machine_already_has`
    /// proved and its flip-check exposed.
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
    /// **Strictly more than [`Self::is_loading`], which it does not replace.** That
    /// one answers "is a download in flight", which is what the family field's
    /// ellipsis is about and is cheap; this one is the *verdict*, and it consults the
    /// family list and whether the sources have stopped arriving.
    pub fn family_status(&self, family: &str) -> FamilyStatus {
        let state = self.state.get(family);
        status_of(FamilyFacts {
            drawable: ondin_core::text::is_family_available(family),
            in_flight: state.is_some_and(|s| s.outstanding > 0),
            // The scan's receiver is taken when it lands, so `None` *is* "landed".
            sources_settled: self.system_rx.is_none() && self.catalog_done,
            // Binary search: `families` is the sorted, de-duplicated list the picker
            // draws, so "some source offers this" is already computed and this is the
            // one question that must not cost a `fontdb` query per frame.
            listed: self.families.binary_search(&family.to_string()).is_ok(),
            // Only a verdict alongside nothing having registered: a family whose
            // italic failed and whose upright arrived is not missing.
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
    /// **On is a rebuild, not a re-fetch — unless there is nothing to rebuild
    /// from.** Turning it off leaves `catalog` where it is, so turning it back on
    /// inside one session costs a `rebuild_families` and no traffic. The fetch is
    /// started only when the catalog is genuinely empty, which is the case that
    /// matters: a session that *launched* with web fonts off never spawned the
    /// thread, so without this the switch would appear to do nothing until the app
    /// was restarted.
    ///
    /// ⚠️ **The new receiver has to come with `catalog_done = false`.** That flag
    /// means "the catalog source has stopped arriving", and it is `true` for a
    /// session launched with web fonts off — leaving it set while a fresh thread was
    /// running would let [`status_of`] call a family missing that is about to be
    /// listed, which is the one conclusion §15 D227 exists to prevent.
    pub fn set_web_fonts(&mut self, on: bool, ctx: &egui::Context) {
        if self.web_fonts == on {
            return;
        }
        self.web_fonts = on;
        // **Off cancels the catalog thread that is out** (§15 D720,
        // `[S21-L5-06]`). This used to set the flag and rebuild the family list
        // and nothing else, so a fetch already in flight went on to call
        // `api.fontsource.org` twice, write `catalog.json` and merge its entries —
        // after the user had said no. See `catalog_live`.
        //
        // 🚨 **A flag only ever goes `false`, and `store(on)` is the version of
        // this that looks right and is the bug.** Writing `on` back into the
        // *existing* flag un-cancels the thread still holding it — the one blocked
        // inside `fetch_catalog` across both clicks, which then returns and writes
        // the response the user cancelled. Off cancels; on either starts a thread
        // with a **new** flag or starts nothing at all.
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
    /// **Off the UI thread even though it is only a `read_dir`.** A cache at its
    /// 256 MB budget is a few thousand files, and `metadata()` per entry on Windows
    /// is a syscall per entry; the settings modal that asks this opens on a click,
    /// which is exactly the frame that must not stall. The scan is cheap enough that
    /// the answer usually lands on the next frame anyway — which is why the row it
    /// feeds says "counting" rather than showing a spinner.
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
    /// **Nothing is un-registered, and that is the point.** The disk cache is
    /// derived data — a face already registered with the shaping engine keeps
    /// working, and the document on screen does not so much as repaint. What the
    /// user has actually reclaimed is disk, which is what they asked for; the cost
    /// is one re-download apiece the next time a family is wanted and its bytes are
    /// no longer in memory.
    ///
    /// **Files in flight are not a hazard.** A worker mid-download writes its file
    /// back after this runs, which leaves the cache holding exactly the faces the
    /// session is using — the same state a sweep would reach a moment later.
    ///
    /// Synchronous, unlike [`Self::measure_cache`], because it is the answer to a
    /// button and the user has to be told what happened: a *deletion* reported on a
    /// later frame is a button that appears not to have worked. Bounded by the same
    /// budget the sweep enforces, so the worst case is a few thousand
    /// `remove_file`s.
    pub fn clear_cache(&mut self) -> u64 {
        let freed = dir_bytes(&self.cache_dir);
        if let Ok(entries) = std::fs::read_dir(&self.cache_dir) {
            for entry in entries.flatten() {
                if entry.metadata().is_ok_and(|m| m.is_file()) {
                    let _ = std::fs::remove_file(entry.path());
                }
            }
        }
        // Not `None`: the count is *known* to be zero, and a row that went back to
        // "counting" would read as the click having restarted a scan.
        self.cache_bytes = Some(0);
        freed
    }

    /// Give back a claim for a face that will never report (§15 D354).
    ///
    /// **Not a failure, and the difference decides what the picker says.** The
    /// `Failed` arm of [`Self::poll`] does this *and* bumps `FamilyState::failed`,
    /// which is what lets [`status_of`] call a family `Missing`. A job the backlog
    /// pushed out was never attempted, so counting it as a failure would put
    /// §5.4a's warning glyph on a family that is perfectly fine and merely
    /// un-fetched — the exact wrong conclusion §15 D227 exists to prevent.
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
            // Never goes back: a family the user picked is not a preview any
            // more, however it first came to be registered.
            state.wanted = Want::Full;
        }
        // The same rule one field over — `min` is the more urgent, `Lane`'s
        // variants being declared most-urgent-first.
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
                // **What the backlog bound pushes out has to be released here.**
                // A dropped job never reports, so its claim and its `outstanding`
                // would otherwise stand for the session — see `push_preview`.
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
        // **The switch is enforced here rather than only in the family list.** A
        // name can reach `ensure` from a document that was saved on a machine with
        // web fonts on, which never went through the picker — so filtering the
        // *list* would leave the one path that actually makes a request open.
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

        // Up to two: the disk copy, then the refreshed one. **`Disconnected` is the
        // catalog thread having finished**, on every path it can finish by, and it is
        // the only thing that lets a family be called missing rather than late
        // (§15 D227).
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

        // **After the catalog loop, so the entries the specs are built from are
        // already in** (§15 D352). `catalog_done` is set by that loop's
        // `Disconnected` arm, and it is also `true` from the start of a session
        // launched with the switch off — `prefetch_popular` is what refuses in that
        // case, not this condition.
        if self.catalog_done && !self.prefetched {
            self.prefetch_popular();
        }

        while let Ok(msg) = self.msg_rx.try_recv() {
            match msg {
                FontMsg::Face { family, key, bytes } => {
                    // Before the move: `register_fonts` consumes the blob, and this
                    // is the only place its size can be read (`FamilyState::bytes`).
                    let size = bytes.len();
                    let registered = !ondin_core::text::register_fonts(bytes).is_empty();
                    let lower = family.to_lowercase();
                    let state = self.state.entry(family).or_default();
                    state.outstanding = state.outstanding.saturating_sub(1);
                    if registered {
                        state.faces.push(key);
                        state.bytes += size;
                        out.fonts_registered = true;
                        // Which family, so the caller can ask whether the document
                        // cares (§15 D591). Duplicates are possible — a family
                        // arrives as several faces — and are left alone: the
                        // consumer is a membership test.
                        out.registered.push(lower);
                    } else {
                        // Not a font we can use. Un-claim it so a later attempt
                        // is not silently skipped — and count it, because bytes
                        // core declines are a failure exactly as a download that
                        // never arrived is (§15 D227).
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
                        // The variable file is the *preferred* form, not the only
                        // one: a family whose file this build's decoder cannot
                        // read still has its static cuts on the CDN.
                        //
                        // ⚠️ **On `Decode` only** (§15 D585, `[S21-L1-04]`). This
                        // downgrade is unconditional and unrecoverable —
                        // `entry.cut` is never restored anywhere, `fetch_catalog`
                        // being its only other writer and running at most twice a
                        // session — so applying it to a 503 cost the family its
                        // axes and its named instances until the app was
                        // restarted, with nothing on screen saying so. A `Fetch`
                        // failure leaves `cut` alone and the `ensure` below asks
                        // for the variable file again.
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

        // The cache scan. **Not on `out`**: `FontPoll` is what the caller acts on —
        // families changed, fonts registered — and a byte count changes neither. The
        // thread asked for its own repaint, which is all this needs.
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
    /// **The price of drawing every row in its own face is that browsing
    /// registers font data**, and font data is not small. Without this, scrolling
    /// the catalog is an unbounded leak; with it, the picker holds a bounded
    /// working set and re-fetches from the disk cache if the user scrolls back
    /// far enough. A family is exempt once anything asked for it in full — a
    /// document's font, or one the user picked — so eviction can never pull a
    /// face out from under the canvas.
    ///
    /// Oldest-drawn first, by the [`FamilyState::touched`] stamp a picker row
    /// sets, and it stops as soon as it is back under the budget or down to
    /// [`PREVIEW_FLOOR`] families — whichever comes first.
    fn trim_previews(&mut self) {
        let evictable = |s: &FamilyState| {
            s.wanted == Want::Preview && s.outstanding == 0 && !s.faces.is_empty()
        };
        // Measured before anything is collected: this runs every frame, and
        // building the candidate list unconditionally would allocate a few
        // hundred strings per frame to discover there was nothing to do.
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
            // Only the keys this family alone owns: one file can carry several
            // families (a TrueType collection), and un-claiming a shared file
            // would let it be registered a second time.
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
    /// fonts off.
    ///
    /// **The bundled Inter and the machine's own fonts are what "off" *is*.** It is
    /// not a degraded list: every family the machine has installed is still there,
    /// and nothing the on state shows is hidden by any other means.
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
/// **Split out from [`FontService::trim_previews`] because it is the whole of the
/// rule and none of the un-registering** — the same division `tools::resized_box`
/// makes — and because the alternative is a policy that can only be checked by
/// building a `FontService`, which scans the system's fonts and asks the network
/// for a catalog.
fn families_over_budget(
    mut candidates: Vec<(u64, usize, String)>,
    mut total: usize,
) -> Vec<String> {
    // `touched` leads, so the sort is the LRU order; the two fields after it only
    // make the result deterministic when two families were drawn on the same poll.
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
/// **The network half is gated on the disk copy's age.** It used to run
/// unconditionally, so every launch made two API calls even when `catalog.json` had
/// been written minutes earlier. A stale catalog costs the user a family that was
/// added since; the Google library gains those monthly, so a week is generous and
/// still means the fetch happens roughly never.
///
/// **A function rather than a block inside [`FontService::new`], because the
/// decision to have a catalog is no longer only made at startup.** Turning web
/// fonts on mid-session has to be able to start this, and the sender must not
/// outlive the thread — `catalog_done` is read off the channel *disconnecting* — so
/// there is nowhere for the sender to be kept and the receiver is the only thing
/// that can come back out.
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
        // Freshness is asked of the copy we could actually *read*: a file that is
        // recent but corrupt or empty has a good mtime and no catalog in it, and
        // skipping the fetch on that would leave the picker with system fonts alone
        // until the file was deleted by hand.
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
        // **The switch, asked immediately before the network** (§15 D720,
        // `[S21-L5-06]`). The disk copy above is local and is sent either way —
        // reading a file the app already wrote is not a request leaving the
        // machine — but everything past this line is, and §15 D330's promise is
        // that turning the switch off stops it.
        let live = || live.load(std::sync::atomic::Ordering::Relaxed);
        if !live() {
            return;
        }
        if let Some(entries) = fetch_catalog(&agent) {
            // **Asked again on the way back, which is the half that matters.**
            // `fetch_catalog` blocks, and the whole window this closes is the one
            // where the user turns the switch off *while it is out*. Coming back
            // to write `catalog.json` and merge the entries then would be the
            // request having left the machine and left something behind.
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
/// **The same walk [`trim_font_cache`] makes, and deliberately the same
/// exclusions**: subdirectories and unreadable entries are skipped rather than
/// counted, because the cache is a flat directory of face files and a number that
/// counted anything else would not be the number the budget is measured against.
/// A missing directory is 0 — nothing has been fetched yet, which is a real answer
/// and not an error.
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
/// `budget` is a parameter rather than [`FONT_CACHE_BYTES`] read directly, for the
/// reason its twin does not need one: this rule's caller reads a real directory,
/// and a test of the sweep that had to reach 256 MB before anything happened would
/// have to write 256 MB.
///
/// **Split out from [`trim_font_cache`] for [`families_over_budget`]'s reason**:
/// it is the whole of the rule and none of the deleting, and the alternative is a
/// policy that can only be checked by filling a real directory.
///
/// **No floor, unlike its in-memory twin, and the difference is worth knowing.**
/// [`PREVIEW_FLOOR`] exists because evicting what the picker is *drawing this
/// frame* makes it re-fetch and re-evict forever; this runs once at startup, with
/// nothing on screen and nothing in flight, so there is nothing to thrash against.
/// A floor here would only be a second, weaker cap — exactly what that constant's
/// own doc warns about.
fn files_over_budget(
    mut candidates: Vec<(SystemTime, u64, PathBuf)>,
    mut total: u64,
    budget: u64,
) -> Vec<PathBuf> {
    // `used` leads, so the sort is the LRU order; the two fields after it only
    // make the result deterministic between files stamped the same moment.
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
/// **Run once, off the startup path**, because a directory scan is I/O and the app
/// has nothing to do with the answer. That is also what makes the missing floor
/// safe (see [`files_over_budget`]) — at this moment nothing has been asked for,
/// so nothing can be pulled out from under a picker row.
///
/// **What "least recently used" rests on is the mtime, and [`load_face`] is what
/// makes that true**: a cache *hit* touches the file, so the stamp means "last
/// wanted" rather than "first downloaded". Without that a family opened in every
/// document would be evicted ahead of one glanced at once, purely for having been
/// fetched earlier.
///
/// Subdirectories and unreadable entries are skipped rather than counted: the
/// cache is a flat directory of face files, and a thing that is not one of those
/// is not this function's to delete.
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
        // An mtime the platform will not give up sorts as the epoch, i.e. goes
        // first — the same way `is_fresh` resolves an unknown stamp towards doing
        // the work rather than towards skipping it.
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
    // A variable file is one download for every weight the family has — and it
    // is what makes the axis rows and the variant list honest, since both are
    // read from the face's own `fvar` (§9.2 Font tab). Two files at most.
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

/// Read one face's bytes: from the disk cache or the local file if it is there,
/// from the CDN if it is not. Decodes woff2 and caches the result decoded.
/// [`load_face`] with an unwind guard, which is what a worker actually calls
/// (§15 D450).
///
/// **A panic in a decoder used to cost a worker for the session, not a face.**
/// The loop is `while let Some(job) = queue.pop()`, so an unwind out of
/// `load_face` leaves the `while` entirely: the thread is gone, and — worse — the
/// job's `FontMsg::Failed` is never sent, so `claimed` keeps the key and
/// `outstanding` never clears. That family reads as **loading for ever** and can
/// never be asked for again, and `WORKERS` is 6, so six such files end the
/// session's font loading with nothing on screen saying so.
///
/// **Measured, and 53 bytes is the whole hostile file**: a `wOF2` header
/// declaring one table with `origLength = 1,000,000` and a one-byte empty brotli
/// stream panics `woff2-patched-0.4.0`'s `push_simple_table_record` with *"range
/// end index 1000000 out of range for slice of length 0"* — the table's source
/// range is built from UIntBase128 values read out of the file and never compared
/// to the decompressed length. `load_face`'s `.ok()?` is written as though this
/// could not happen; `.ok()` turns an `Err` into `None` and does nothing about an
/// unwind.
///
/// **A caught unwind is exactly an `Err`**, which is the shape and the argument
/// `boolean::evaluate` already carries (§15 D239): the guard sits at the seam
/// every call goes through rather than at the one decoder known to panic today,
/// so `fontdb`, `std::fs::read` and the next decoder are covered without anyone
/// remembering to. `AssertUnwindSafe` because nothing observable crosses the
/// boundary — the agent and the spec are borrowed read-only and every scrap of
/// state a decoder builds is local to the call and dropped by the unwind.
///
/// ⚠️ **This is inert under `panic = "abort"`**, like every `catch_unwind`. That
/// setting is now a build failure rather than a hope (§15 D445), which is the
/// only reason this guard can be relied on.
///
/// ⚠️ **An unwind is [`FaceFailure::Decode`]**, not `Fetch` (§15 D585). It is
/// never a transport failure — the bytes are in hand by the time a decoder can
/// panic — and calling it `Decode` keeps the fallback reachable for the case the
/// guard was written for: a woff2 this build cannot read still has static cuts on
/// the CDN.
fn load_one_face(agent: &ureq::Agent, spec: &FaceSpec) -> Result<Vec<u8>, FaceFailure> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| load_face(agent, spec)))
        .unwrap_or(Err(FaceFailure::Decode))
}

fn load_face(agent: &ureq::Agent, spec: &FaceSpec) -> Result<Vec<u8>, FaceFailure> {
    match &spec.source {
        // A local file that will not open is `Fetch` — the bytes never arrived,
        // and nothing has looked at a face. One that opens and is not sfnt is
        // `Decode`, which is the same verdict the web arm reaches through
        // `sfnt_from`.
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
/// **The woff2 decoder panics on malformed input and `.ok()?` cannot see it.**
/// `.ok()` turns an `Err` into `None`; an unwind goes straight past it. Measured:
/// a **53-byte** file — a `wOF2` header declaring one table with
/// `origLength = 1,000,000`, then a one-byte empty brotli stream — panics
/// `woff2-patched-0.4.0`'s `push_simple_table_record` with *"range end index
/// 1000000 out of range for slice of length 0"*, because the table's source range
/// is built from UIntBase128 values read out of the file and never compared to
/// the decompressed length. A second shape, three table entries of `0x7FFFFFFF`,
/// does the same. The dependency's `is_valid_header` checks the signature and its
/// body is otherwise literally `// TODO: Add other checks`.
///
/// **Extracted from `load_face` so the guard sits where the panic is and can be
/// driven by a test.** The decode used to be inline on the `Web` arm, behind
/// `download`, so nothing headless could reach it — which is why the panic
/// survived a suite this size. `sfnt_from` takes bytes and answers bytes.
///
/// ⚠️ **[`load_one_face`] guards the whole job as well, and that is not
/// redundant.** This one is the decoder known to panic today; that one covers
/// `fontdb`, `std::fs::read` and the next decoder without anyone remembering to.
/// Both are inert under `panic = "abort"`, which is a build failure now (§15
/// D445).
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
/// **Opened for writing, which is what the stamp costs.** `File::set_modified`
/// needs a handle with write access on Windows, so a read-only open would fail
/// silently and leave every file wearing its download time. Nothing is written
/// through it — `write(true)` alone does not truncate.
///
/// Every failure is ignored on purpose: a file that cannot be touched keeps an
/// older stamp and is evicted a little early, which costs one re-download. That is
/// the whole downside, and it is not worth a branch anywhere else.
fn touch(path: &Path) {
    let _ = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(SystemTime::now()));
}

/// The one agent every request in this process goes through, with deadlines on
/// it.
///
/// ⚠️ **`ureq::Agent::new_with_defaults()` has no timeout of any kind**, which is
/// what this used to be (§15 D441). Read from `ureq-3.3.0`'s `impl Default for Timeouts`:
/// `global`, `per_call`, `resolve`, `connect`, `send_request`, `send_body`,
/// `recv_response` and `recv_body` are **all `None`**, and only `await_100` has a
/// value. So a server or middlebox that accepts the connection and then stops
/// sending — a captive portal, a hung proxy, an idle-but-open socket — blocked
/// `read_to_vec()` for ever. The worker never returned from `load_face`, never
/// popped another job, and was gone for the session; `WORKERS` is 6 and
/// `prefetch_popular` dispatches **fifty** jobs at the first poll after the
/// catalog lands, so six concurrent stalls at launch are one bad network away.
/// From then on no face loads at all, the document's own fonts on the urgent lane
/// included, every family sits at `Pending` so §5.4a's missing-font warning never
/// fires, and the picker's loading dots pulse for ever. The `.limit()` on the
/// body bounds **bytes**, which a stall never reaches.
///
/// **A global deadline as well as the two component ones**, because they answer
/// different questions: `timeout_connect` covers a blackholed SYN, `timeout_recv_body`
/// covers a body that stops mid-stream, and `timeout_global` covers the shape
/// neither does — a peer dribbling a byte at a time, which resets a per-read
/// deadline for ever. It is end-to-end from DNS lookup onward.
///
/// **Generous rather than tight.** These are not latency budgets: a CJK family's
/// default subset is megabytes per weight, and a slow connection downloading a
/// real file must not be cut off. The number that matters is that they are
/// *finite* — the failure being fixed is unbounded, not slow.
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
    /// Raw item from the variable-fonts endpoint. Only the axis *names* are
    /// taken: the ranges are read out of the face's own `fvar` later, which is
    /// the rule the Font tab already follows.
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
            // ⚠️ **Both fields, because both end up in the cache filename.** The
            // key is `{id}-{subset}-{weight}-{style}.ttf`; `weight` is a `u16`
            // and `style` comes from a fixed pair, so these two are the whole
            // untrusted surface. See [`is_catalog_token`].
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
/// Fontsource names the file after the axis *set* it carries, and which sets
/// exist depends on the family. Probed against the CDN rather than guessed:
///
/// - `wght` — exists exactly when the family has a weight axis (`42dot Sans`,
///   which has only that one, has `wght` and no `standard`).
/// - `standard` — every axis except `ital`, and only published when there is
///   something besides weight to carry (Roboto's `wght`+`wdth`). Preferred over
///   `wght` for those, so the panel's axis rows get the axes the family has.
/// - `full` — the fallback for a family with no weight axis at all (`Agu
///   Display`, which varies on `MORF`), where neither of the others exists.
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
/// **Every uncertain answer is "not fresh".** A missing file, an mtime the
/// platform will not give up, and a stamp in the future all fall through to a
/// fetch — the cost of one unnecessary refresh is two requests, where the cost of
/// wrongly skipping one is a catalog that never updates again.
fn is_fresh(path: &Path, max_age: std::time::Duration) -> bool {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .is_ok_and(|t| t.elapsed().is_ok_and(|age| age < max_age))
}

fn read_catalog(path: &Path) -> Option<Vec<CatalogEntry>> {
    let bytes = std::fs::read(path).ok()?;
    let entries: Vec<CatalogEntry> = serde_json::from_slice(&bytes).ok()?;
    // ⚠️ **Filtered here as well as at `fetch_catalog`, because the catalog is
    // *persisted*.** Without this, one hostile response keeps working on every
    // later launch — after the server has been fixed — which is the half of the
    // problem that outlives the network. See [`is_catalog_token`].
    let entries: Vec<CatalogEntry> = entries
        .into_iter()
        .filter(|e| is_catalog_token(&e.id) && is_catalog_token(&e.subset))
        .collect();
    (!entries.is_empty()).then_some(entries)
}

/// Whether a value out of the Fontsource catalog is safe to put in a **filename**.
///
/// ⚠️ **The id is used twice — as a URL segment and as a local cache path — and
/// only the second is dangerous.** `static_face` builds
/// `format!("{id}-{subset}-{weight}-{style}.ttf")` and then `dir.join(&key)`, so
/// an id of `"../../../../AppData/Roaming/…"` writes font bytes outside
/// `cache_dir()/ondin/fonts`, and an absolute one discards the base entirely.
/// The bytes must pass [`is_sfnt`], so the primitive is *write a valid font file
/// to an arbitrary path* — bounded, and not nothing. It also poisons the
/// legitimate cache: `"../fonts/inter"` collides two families onto one file, and
/// `trim_font_cache` sweeps by directory listing, so an escaped file is never
/// evicted.
///
/// **Rejected at the parse rather than sanitised at the join**, because the same
/// string is used in two places and fixing one of them is how the other gets
/// forgotten. `[a-z0-9-]` is the shape Fontsource actually publishes, so a
/// rejected entry means the response was not a Fontsource catalog.
///
/// This needs the API itself to be hostile or compromised — `ureq` runs with TLS
/// verification on and the download is capped — which is why it is a filter and
/// not an error the user is shown.
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

    /// **A malformed woff2 is a failed face, not a dead worker.**
    ///
    /// The fixture is the one `[S21-L5-01]` measured: a `wOF2` header declaring
    /// one table whose `origLength` is 1,000,000, followed by a one-byte empty
    /// brotli stream. `woff2-patched-0.4.0` builds the table's source range from
    /// UIntBase128 values read out of the file, never compares them to the
    /// decompressed length, and panics *"range end index 1000000 out of range for
    /// slice of length 0"*.
    ///
    /// **What the panic cost is the finding, not the panic.** The worker loop is
    /// `while let Some(job) = queue.pop()`, so the unwind leaves the loop: the
    /// thread is gone for the session, the job's `Failed` is never sent, `claimed`
    /// keeps the key and the family reads as *loading* for ever. Six such files
    /// and the session loads no fonts at all, silently.
    ///
    /// ⚠️ **This asserts through `sfnt_from`, and the reason is the finding
    /// underneath the finding.** The first version of this test drove `load_face`
    /// with a `FaceSource::File` — and passed **with the guard removed**, because
    /// the `File` arm never decodes woff2: the decode is on the `Web` arm, behind
    /// `download`. `load_one_face` is unreachable headlessly for the same reason.
    /// So the decode was extracted to a function that takes bytes and answers
    /// bytes, which is the only seam a test can reach — and **that is why a panic
    /// on a 53-byte input survived a suite this size.** A guard behind a network
    /// call is a guard no test covers.
    ///
    /// ⚠️ **Flipped** by removing the `catch_unwind` inside `sfnt_from`: the test
    /// binary dies with *"range end index 1000000 out of range for slice of length
    /// 0"*, the reported panic verbatim, and the assertion is never reached —
    /// which is the *shape* of the production failure, a thread that stops rather
    /// than a value that is wrong. `#[should_panic]` would have been the wrong
    /// tool: it asserts a panic happens, where the whole point is containment.
    #[test]
    fn a_malformed_woff2_is_a_failed_face_and_not_a_dead_worker() {
        // The fixture `[S21-L5-01]` measured, byte for byte: a 48-byte wOF2
        // header, one table entry whose origLength is 1,000,000 as UIntBase128,
        // and a one-byte empty brotli stream. 53 bytes is the whole hostile file.
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

        // The control: an sfnt-framed file is not touched by the woff2 path and
        // still comes through, so this is about containment rather than about
        // `sfnt_from` refusing everything.
        let mut sfnt = vec![0x00, 0x01, 0x00, 0x00];
        sfnt.extend_from_slice(&[0u8; 60]);
        assert!(sfnt_from(sfnt).is_some(), "an ordinary sfnt still loads");
    }

    /// **New text is trimmed by default, and its line box survives to be drawn.**
    ///
    /// Asserted through the geometry and not only by reading the field back, because
    /// the field being right is the easy half: a `trim` that reached the node and
    /// then failed to reach `box_of` would pass a field check and produce exactly the
    /// old behaviour. So this lays the defaults out for real and asks the two
    /// questions the feature is: is the box tighter than the line box, and is the
    /// line box still reported so the canvas has a second outline to draw.
    ///
    /// It is also the test that fails if anybody "tidies" the trim out of
    /// `FontService::default_text_parts` on the grounds that it duplicates
    /// `BoxTrim`'s own `#[default]`. It does not: `None` is what an absent field in a
    /// saved file means and must go on meaning, and this is the app authoring a value
    /// instead (§15 D78 and the comment there).
    #[test]
    fn a_new_text_node_gets_the_tight_box_and_keeps_its_line_box() {
        let mut parts = FontService::default_text_parts();
        assert_eq!(
            parts.block.trim,
            ondin_core::BoxTrim::CapToBaseline,
            "new text is no longer trimmed by default"
        );
        // A click plants `Auto`, which is the sizing trim actually tightens — a
        // dragged `Fixed` box is the one the user typed and trim does not
        // reinterpret it.
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
        // Descenders fall outside a cap-to-baseline box, which is the trade the
        // dashed line box exists to make visible rather than a defect. "y" has one.
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

    /// **The case the count could not tell apart from the Latin one.** Browsing 60
    /// CJK families at 2 MB a face is 120 MB; a cap of 240 families let that reach
    /// half a gigabyte before it evicted anything, because the unit was wrong.
    #[test]
    fn a_browse_through_heavy_families_is_bounded_by_bytes_not_by_count() {
        let (rows, total) = browsed(60, 2 * MB);
        let dropped = families_over_budget(rows, total);
        assert_eq!(dropped.len(), 36, "60 at 2 MB must come back under 48 MB");
        // Oldest-drawn first, which is the LRU stamp a picker row sets — so what
        // is on screen is the last thing to go.
        assert_eq!(dropped.first().map(String::as_str), Some("Family 000"));
        assert_eq!(dropped.last().map(String::as_str), Some("Family 035"));
        assert_eq!((60 - dropped.len()) * 2 * MB, PREVIEW_BYTES);
    }

    /// And the Latin case is left alone: the old working set of 240 families still
    /// fits, several times over, which is what the byte budget was chosen against.
    #[test]
    fn a_browse_through_latin_families_evicts_nothing_where_the_count_evicted() {
        let (rows, total) = browsed(300, 150 * 1024);
        assert!(total < PREVIEW_BYTES, "300 Latin faces is {total} bytes");
        assert!(families_over_budget(rows, total).is_empty());
    }

    /// **The floor wins over the budget, and it has to.** A family whose face is
    /// 20 MB puts the budget below a screenful, and evicting what is on screen
    /// means fetching it again next frame and evicting it again on the one after.
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

    /// ⚠️ **Web fonts off has to close *both* doors, and the second one is the
    /// one that is easy to miss.**
    ///
    /// Leaving the catalog out of the picker's list is the visible half. The other
    /// half is `FontService::faces_of`: a family name reaches `ensure` from a
    /// *document* — one saved on a machine where the switch was on — which never
    /// goes near the picker, so a version that filtered only the list would still
    /// queue a download the moment such a file was opened. That is the case this
    /// asserts, and it is the whole point of the switch: off means no request.
    ///
    /// Flip-checked twice, on the two arms separately. Dropping the `web_fonts`
    /// guard from `rebuild_families` leaves *Roboto* listed with the switch off;
    /// dropping it from `faces_of` leaves the face list one long with the switch
    /// off — and each flip passes the *other* assertion, which is why they are both
    /// here.
    ///
    /// `set_web_fonts(true, …)` starts no thread here, because the catalog is not
    /// empty — so this test makes no network call in either direction.
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

    /// **Off cancels a catalog fetch that is already out** (§15 D720,
    /// `[S21-L5-06]`).
    ///
    /// `set_web_fonts(false, …)` used to set the flag and rebuild the family list
    /// and nothing else. A `spawn_catalog` thread inside `fetch_catalog` at that
    /// moment went on to make **two** calls to `api.fontsource.org`, write
    /// `catalog.json` to the user's disk and merge the entries — all after the
    /// user had said no, against §15 D330's *"off means no request leaves the
    /// machine."*
    ///
    /// **The cancel flag is the seam this asserts on**, and it is why the test is
    /// writable at all: the finding rated it *"hard to write honestly without a
    /// network seam"* and proposed saying so rather than asserting on a thread's
    /// timing. The flag **is** the seam. What is asserted is the thing
    /// `spawn_catalog` actually reads immediately before the network and again on
    /// the way back — not that a socket was or was not opened, which no test here
    /// can see.
    ///
    /// 🚨 **The last assertion is the race, and it is the reason the flag is
    /// per-spawn rather than one field on the service.** A shared flag would be
    /// set `false` by the off and `true` again by the next on — and a thread
    /// blocked inside `fetch_catalog` across both would come back, read `true`,
    /// and write the very response the user cancelled. Holding the *old* handle
    /// and checking it stays `false` is the only way to state that, because the
    /// service's own field has legitimately moved on by then.
    ///
    /// ⚠️ **No thread is started anywhere in this test**: `inert()` has none, and
    /// `set_web_fonts(true, …)` spawns only when the catalog is empty, which it is
    /// not here. So this makes no network call in either direction — the same
    /// property the test above states about itself.
    ///
    /// 🚨 **This assertion was red against the first version of the fix, which is
    /// the whole argument for writing it.** That version cancelled correctly and
    /// then said `self.catalog_live.store(on, …)` — one line, obviously right,
    /// and it writes `true` back into the flag the *cancelled* thread is still
    /// holding. The fix had reintroduced the race its own doc comment described,
    /// and the first two assertions were green. **A guard that is correct in the
    /// direction you were thinking about is not a guard**; what caught it was
    /// asserting on the handle the caller has let go of rather than on the field.
    ///
    /// ⚠️ **Flip-check, run: `if !on { … }` back to `store(on, …)`.** Red at the
    /// third assertion and green at the other two — which is the same shape, and
    /// says this test is not a restatement of the one above it.
    #[test]
    fn turning_web_fonts_off_cancels_a_catalog_fetch_that_is_already_out() {
        use std::sync::atomic::Ordering::Relaxed;

        let ctx = egui::Context::default();
        let mut service = FontService::inert();
        // Stand in for a live thread's handle. `inert()` starts none, so this is
        // the flag such a thread *would* be holding.
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

        // Back on. The catalog is not empty, so no new thread is spawned and the
        // service keeps this same handle — which must stay cancelled.
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

    /// The list is virtualized on a *window* into the matches — this is the
    /// lookup that makes it possible, and it has to survive a window that runs
    /// off the end (the last page of a list, or a needle that just shortened it).
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

    /// The memo has two inputs, and forgetting the second one is the bug worth
    /// pinning: the family list grows *after* the picker has already been opened
    /// and filtered — the catalog lands, or the system scan finishes — and stale
    /// indices then point at the wrong names.
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

    /// **A family is never called missing before the sources have stopped arriving**
    /// (§15 D227).
    ///
    /// This is the whole reason the third state took a state to build rather than a
    /// colour: `is_family_available` answers "still downloading" and "nobody has this
    /// font" identically, so §5.4a carried the UI warning as a *gap* rather than a
    /// promise, on the grounds that a warning built on it would accuse a font that
    /// was on its way. The boundary this asserts is `Pending`/`Missing`, and the
    /// unsettled row is the one that matters — it is also the state a **cold start**
    /// spends its first second in, so getting it wrong would flash a warning over
    /// every font in a document that then loaded perfectly (D175 is the record of that
    /// second being real).
    ///
    /// **Flipped against dropping `!sources_settled`** — the term is easy to read as
    /// belt-and-braces, since the other three facts look sufficient — and against
    /// dropping the `listed` arm, which is what makes an unknown family missing at all.
    /// The first fails the two unsettled rows; the second fails the last one.
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

        // Drawable wins over everything, including a past failure: a family whose
        // italic never arrived and whose upright did is not missing.
        assert_eq!(status_of(facts(true, false, true, true, true)), Ready);
        // In flight.
        assert_eq!(status_of(facts(false, true, true, true, false)), Pending);

        // **Unsettled, and otherwise looking exactly like a missing family.** Both
        // rows are what a cold start looks like: nothing registered, nothing in
        // flight, and no source has said whether it has the font.
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

    /// **The disk budget drops the least recently used, and stops as soon as it is
    /// back under.** Both halves have a plausible wrong version that the fixture is
    /// built to separate: the sizes and the stamps deliberately disagree, so a rule
    /// that sorted on bytes would delete the 40 MB file and leave the 200 MB one,
    /// and a rule that read the order the other way would delete the newest.
    ///
    /// The third case is the one worth having: over budget does **not** mean empty
    /// the cache. Two 200 MB files are 144 over, and dropping one answers it.
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

    /// The sweep against a real directory, which is what the rule above cannot
    /// cover: reading the sizes, skipping what is not a face file, and actually
    /// deleting.
    ///
    /// **The subdirectory arm is insurance and not a live catch, which is worth
    /// saying rather than implying.** Deleting the `is_file` guard leaves it green:
    /// a directory's `metadata().len()` is zero, so it changes no total, and
    /// `remove_file` refuses one anyway. It is here because the sweep deletes
    /// things, and "only files, and only ones it counted" should be written down
    /// somewhere that fails if the loop is ever rewritten to recurse.
    #[test]
    fn the_sweep_deletes_the_stalest_faces_off_the_disk() {
        // 🚨 **Process-unique, and this one is the worst of the five that were
        // not**: it opens by *deleting the directory*, so a second `cargo test` on
        // the same machine has its fixture removed from under it mid-run. The house
        // pattern is `ondin-<what>-<pid>`.
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

        // 300 bytes of faces against a 250-byte budget: one has to go, and the
        // directory is not part of the sum.
        trim_font_cache(&dir, 250);
        assert!(!stale.exists(), "the stalest face is the one that goes");
        assert!(middling.exists() && recent.exists(), "and only that one");
        assert!(dir.join("sub").is_dir(), "a directory is not a face file");

        // And a budget the directory is already under changes nothing.
        trim_font_cache(&dir, 250);
        assert!(middling.exists() && recent.exists());
    }

    /// **A cache hit stamps the file**, which is the whole of what makes
    /// `trim_font_cache`'s order "least recently used" rather than "first
    /// downloaded" — without it a family opened in every document is evicted ahead
    /// of one glanced at once, for having been fetched earlier.
    ///
    /// The arm that can actually fail is the write access, and it was checked by
    /// putting the bug in: `File::set_modified` needs a writable handle on Windows,
    /// so `File::open` — the obvious spelling for a function that writes nothing —
    /// leaves the stamp exactly where it was and returns an error this function
    /// discards. Every file would keep its download time and no test of the *rule*
    /// would notice.
    #[test]
    fn touching_a_cached_face_moves_its_stamp_forward() {
        // Process-unique, per the house pattern — see the sweep test above.
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

    /// The catalog gate. The absent-file case is the one that matters: read the
    /// comparison the other way round and first run never fetches, leaving the
    /// picker with system fonts and Inter forever.
    #[test]
    fn a_just_written_catalog_is_fresh_and_a_missing_one_never_is() {
        // Process-unique, per the house pattern — and this one writes a *stamp* it
        // then reads back, so a second run's write is exactly what would falsify it.
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
    /// The unit tests above cover the pieces; this is the only thing that proves
    /// they are wired to each other, and it is where a mistake in the threading
    /// would show up. It also demonstrates the split `poll` reports: the catalog
    /// arriving is `families_changed` **only**, and re-shaping the document on it
    /// was the wasted work that split was made for.
    // Network-gated: run with `cargo test -p ondin-app -- --ignored`.
    #[test]
    #[ignore = "requires network (Fontsource/jsDelivr)"]
    fn the_service_registers_a_web_family_end_to_end() {
        // A headless context: the repaints the threads ask for have nowhere to
        // go, and this test drives `poll` in a loop itself.
        // `true`: this test *is* the web path, so the switch that would skip the
        // catalog thread has to be on for it to have anything to prove.
        let mut service = FontService::new(&egui::Context::default(), true);

        // Wait for the catalog. Polling in a loop is what the app does per frame.
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

        for _ in 0..600 {
            if service.poll().fonts_registered {
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
        // Roboto's variable file carries the whole weight axis, so one download
        // is nine weights (§9.2 Font tab reads them off `fvar`).
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

        // Process-unique, per the house pattern. `#[ignore]`d on the network, so it
        // is not part of the collision this batch is about — swept with the other
        // four so the file has one answer rather than two.
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

    /// **The list is fifty distinct names and does not include the bundled one.**
    ///
    /// Cheap, and it catches the two edits that are easy to make by hand: pasting
    /// a name twice, and adding `Inter` — which core registers itself, so warming
    /// it would be one download for a family that is never fetched.
    ///
    /// It deliberately says nothing about whether the names are *real*; that
    /// cannot be known without the catalog, and
    /// `every_prefetched_family_is_spelled_the_way_the_catalog_spells_it` is the
    /// test that asks.
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

    /// **Every name is spelled the way the catalog spells it.**
    ///
    /// ⚠️ **This is the only check that can exist for the thing most likely to be
    /// wrong.** A misspelling does not fail, warn, or degrade — `faces_of` finds
    /// no entry, returns no specs, and that family is quietly never warmed. There
    /// is no local source of truth to compare against: the catalog is the API's,
    /// and the app's own copy is a cache that may not exist. So this asks the live
    /// endpoint and is `#[ignore]`d beside the other network test.
    ///
    /// Every name was checked this way when the list was written. **Re-run it
    /// after editing `POPULAR`** — Fontsource does rename families (`Source Sans
    /// Pro` became `Source Sans 3`), so a name correct today can rot without
    /// anything here changing.
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

    /// **A preview the backlog pushed out is released, not left claimed for the
    /// session** (§15 D354).
    ///
    /// ⚠️ **Found while wiring the picker's loading dot, and it is older than
    /// both.** The bound has dropped jobs from `preview`'s oldest end since it was
    /// written, deliberately — but a dropped job never reports, so its key stayed
    /// in `claimed` and its family's `outstanding` stayed at 1 **forever**. Two
    /// consequences, and the second is the bad one: the family reads as
    /// permanently loading, and because `ensure` filters on `claimed`, its face
    /// can never be requested again — so one flung scrollbar left those rows
    /// unpreviewable for the rest of the session, with no way back short of a
    /// restart. Invisible until something drew the loading state.
    ///
    /// ⚠️ **Released rather than failed.** `FamilyState::failed` is what
    /// `status_of` reads to call a family `Missing`, and a job that was never
    /// attempted is not a family that is missing — counting it would put §5.4a's
    /// warning on a font that is perfectly fine (§15 D227). Asserted here, because
    /// the two spellings are one line apart in `poll` and the wrong one is a
    /// plausible edit.
    ///
    /// ⚠️ Flipped by discarding `push_preview`'s return value: the first assertion
    /// fails with the family still claimed and still loading.
    #[test]
    fn a_preview_dropped_by_the_backlog_can_be_asked_for_again() {
        let mut service = FontService::inert();
        service.web_fonts = true;
        // One more family than the lane will hold, so exactly the oldest is
        // pushed out — `queue::PREVIEW_BACKLOG` is the bound doing it.
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
        // And the claim came back, which is what lets it be asked for again — the
        // half that makes the row recoverable rather than merely honest.
        service.ensure_preview(evicted);
        assert!(
            service.is_loading(evicted),
            "`{evicted}` could not be re-requested, so its row can never draw in \
             its own face again"
        );

        // The newest is still queued and untouched, or the bound dropped the
        // wrong end and this test is about nothing.
        let newest = names.last().expect("a fixture");
        assert!(
            service.is_loading(newest),
            "the fixture: the newest is in flight"
        );
    }

    /// **A warmed family goes down the prefetch lane, and the switch is what
    /// refuses.**
    ///
    /// The lane is asserted through `FamilyState::lane` rather than by reading
    /// the queue, which is `queue`'s own private business — and that field is the
    /// thing worth pinning anyway, since it is what the variable-file retry reads.
    ///
    /// ⚠️ Flipped by passing `Lane::Preview` in `prefetch_popular`: the lane
    /// assertion fails, and nothing else does — the family is still warmed, still
    /// registered, still evictable. That is the whole hazard of this change in one
    /// line: **the wrong lane is invisible in every observable except this one**,
    /// and on a real run it shows up only as a picker that stutters while fifty
    /// speculative downloads elbow into its bounded queue.
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

        // The switch refuses, and refuses in `prefetch_popular` rather than by
        // the catalog being empty — D330's rule that off means no request leaves
        // the machine, which the list alone would not enforce.
        let mut off = FontService::inert();
        off.web_fonts = false;
        off.catalog.insert(first.to_string(), entry(first));
        off.rebuild_families();
        off.prefetch_popular();
        // Not `state.is_empty()`: core registers Inter itself, so a fresh service
        // already holds one entry. Asking about the *popular* names is the
        // question, and the looser spelling passed for the wrong reason on the
        // switch-on case before this was noticed.
        assert!(
            POPULAR.iter().all(|f| !off.state.contains_key(*f)),
            "web fonts are off and the popular set was queued anyway"
        );
    }

    /// **It runs once, and a family the machine already has is not warmed.**
    ///
    /// Two independent reasons a name in `POPULAR` produces no work, and both
    /// matter: running twice would double-queue fifty faces on every catalog
    /// refresh, and warming a family that is *installed* spends the preview budget
    /// on bytes that were never going to be downloaded — `faces_of` resolves a
    /// system family to its local file first (§5.4a's order), so there is nothing
    /// to fetch.
    ///
    /// ⚠️ **Both halves of this were vacuous in their first draft, and both were
    /// caught by their own flip-check rather than by review.**
    ///
    /// The once-only half asserted `service.prefetched` after the call — which
    /// `FontService::inert` already sets to `true`, so it was testing the
    /// constructor. It now starts the fixture at `false` and asserts the
    /// *transition*, which is what removing the assignment breaks. (The companion
    /// "a second run queues nothing" assertion is honest but weak on its own:
    /// `claimed` stops the same face being re-queued regardless, so it passes
    /// under the flip. The flag matters after an eviction, when `claimed` no
    /// longer holds the key — kept, and labelled as the weaker of the two.)
    ///
    /// The skip half used an **empty `fontdb`**, so `faces_of` answered nothing
    /// and the `FaceSource::Web` filter was never reached — deleting the filter
    /// left the test green. A real face has to be installed for the branch to
    /// exist at all, so this loads one off disk and takes the family name from
    /// the database rather than assuming it.
    ///
    /// ⚠️ Flipped, after the rewrite: removing `self.prefetched = true` fails on
    /// the transition assertion; dropping the `FaceSource::Web` filter fails on
    /// the skip.
    #[test]
    fn the_warm_runs_once_and_skips_a_family_the_machine_already_has() {
        let mut service = FontService::inert();
        service.web_fonts = true;
        // `inert` starts this `true` — there is nothing for an inert service to
        // warm — so the fixture has to put it back or the assertion below is about
        // the constructor.
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

        // Installed rather than downloadable. A **real** face, because the branch
        // only exists when `faces_of` has system specs to return.
        // ⚠️ `load_font_file`, not `load_font_data`. The latter registers the face
        // as a `Source::Binary`, and `system_faces` skips anything that is not a
        // `Source::File` — so the fixture built that way has a family in `system`,
        // a face in the database, and still no specs, which is the shape that made
        // the first version of this test vacuous.
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
        // **File** spec. Without this the filter is never reached and the skip
        // below passes for the wrong reason — which is what it did.
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
    /// **persisted** catalog is filtered on the way back in as well.
    ///
    /// ⚠️ **The re-read is the half that would be missed**, and it is the half
    /// that outlives the incident: `write_catalog` stores the response, so one
    /// bad `id` keeps building a cache path on every later launch, long after
    /// the server has been fixed. Filtering only at `fetch_catalog` would look
    /// like a fix and be one for a single session.
    ///
    /// **The assertion is the cache path, not the field.** The id is used in two
    /// places — a URL segment and a local filename — and only the filename is
    /// dangerous, so what has to be true is that nothing joinable survives:
    /// `static_face` builds `{id}-{subset}-{weight}-{style}.ttf` and then
    /// `dir.join(&key)`.
    ///
    /// Flip-check, run: dropping the filter from `read_catalog` fails on the
    /// length assertion at `"../../evil"` — and note the *fetch* side's filter
    /// keeps that green, which is exactly why both are here.
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

        // And a catalog that is *entirely* bad reads as no catalog rather than as
        // an empty one, which is what makes the next launch fetch again.
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

    /// **`[S21-L5-03]`'s loss: every deadline was `None`.**
    ///
    /// `ureq::Agent::new_with_defaults()` sets no timeout of any kind — read
    /// from `ureq-3.3.0`'s `impl Default for Timeouts`, where every field but
    /// `await_100` is `None` — so a peer that accepted the connection and then
    /// stopped sending held a worker for the life of the process. Six of those
    /// is the whole pool, and `prefetch_popular` dispatches fifty jobs at once.
    ///
    /// ⚠️ **The assertion is that they are *finite*, not what they are.** These
    /// are not latency budgets: a CJK subset is megabytes per weight and a slow
    /// connection must not be cut off, so pinning the numbers would make this a
    /// test about taste that fails whenever somebody tunes one. What must never
    /// come back is `None`.
    ///
    /// ⚠️ **And the inert agent is asserted too.** `FontService::inert` built its
    /// own with `new_with_defaults`, which is the shape a second construction
    /// always takes — a guard added at one and missing at the other.
    ///
    /// Flip-check, run: `ureq::Agent::new_with_defaults()` in `agent()` fails at
    /// *"connect"* with `None`, naming which deadline is missing rather than
    /// just that one is.
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
    /// ⚠️ **`inert()` drops its own sender**, deliberately — a headless service
    /// must read `Disconnected` rather than *not yet* — so the receiver is
    /// replaced here rather than reaching for a constructor that keeps one.
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

    /// **A variable file that failed to *download* is still a variable family**
    /// (§15 D585, `[S21-L1-04]`).
    ///
    /// §5.4a's rule — *"a family whose file the decoder cannot read falls back to
    /// the static cuts automatically"* — is about the decoder, and `FontMsg::Failed`
    /// carried no reason, so one 503 on `roboto:vf@latest/…woff2` took Roboto's
    /// axes and its nine named instances away for the rest of the session. The
    /// downgrade is unrecoverable in memory (`entry.cut` has one other writer,
    /// `fetch_catalog`, which runs at most twice a session) and the disk catalog is
    /// untouched, so a restart was the fix and nothing said so.
    ///
    /// ⚠️ **The `Decode` control is in the same test on purpose.** Asserting only
    /// that `Fetch` leaves `cut` alone is satisfied by a build that never downgrades
    /// at all — which would be a different bug, and a worse one, since a family
    /// whose variable file this build genuinely cannot read would then never fall
    /// back and would simply not draw.
    ///
    /// **Flip run**, the `reason == FaceFailure::Decode` term removed: fails on
    /// *"a download failure took the family's variable file away"*, `None` against
    /// `Some("standard")`, with the `Decode` half still green.
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
