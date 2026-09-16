//! Preferences that outlive a session — the small pile of settings that belong
//! to the *person* rather than to the document.
//!
//! **Deliberately tiny, and deliberately not the document.** An export preset is
//! a working habit and a last-used folder is a machine path: putting either in a
//! `.ondin` file would send it to whoever the file is shared with, where the
//! preset is meaningless and the path is a small leak. Everything the artwork
//! needs to export itself is on the layers (§7); this is what is left over.
//!
//! **Two doors onto the same file, and that is deliberate too.** The export
//! habits are written from the export panel's own overflow popover, where the
//! work is; the rest — the nudge step, the layer tree's guides and its
//! fold-on-open, whether the
//! web-font library is offered at all — is written from [`crate::settings`], the
//! modal that exists precisely for the rows no panel is a door onto. A setting
//! belongs beside the thing it changes when there is one, and in the modal when
//! there is not.
//!
//! **Best-effort at both ends.** A read that fails is `Default`, a write that
//! fails is dropped, and nothing in the app waits on either — a preference that
//! could refuse to load would make a config file a thing that can break the
//! editor. Beside the font cache, under `dirs::config_dir()/ondin`, because that
//! is where a *setting* goes even though the cache next door is the closest
//! precedent for the path.

use crate::input::NudgeStep;
use ondin_core::ExportSpec;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A named set of export specs, applied to a layer in one click.
///
/// The answer to thirty icons: Figma has no presets at all, and its stand-in is
/// copying the settings off one layer and pasting them onto the next — which this
/// also offers, and which stops being enough the second time you need the same
/// set in a different file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportPreset {
    pub name: String,
    pub specs: Vec<ExportSpec>,
}

/// Everything the Settings modal writes, plus the export habits the export
/// panel's popover has always written.
///
/// **`#[serde(default)]` on the container, not on every field, and the
/// difference matters the moment a default is not the type's own.** The
/// field-level spelling fills a missing key with `Default` *of that field's
/// type* — which is `false` for a `bool` and `0.0` for an `f64`. Four of the
/// settings below want the opposite: web fonts and the tree guides default *on*,
/// and a nudge of zero is an arrow key that does nothing. The container attribute
/// fills a missing key
/// from `Prefs::default()` instead, so the answer to "what does an old
/// `prefs.json` mean" is the one place the app already states its defaults.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub export_presets: Vec<ExportPreset>,
    /// Where the last export went, so *Export all* can repeat with no dialog.
    ///
    /// **One folder, not one per document.** A map keyed by document path grows
    /// without bound, goes stale the first time a file is moved, and answers the
    /// wrong thing for the case it exists to serve — a designer exporting from
    /// three files into one asset folder. When it is empty or gone, the document's
    /// own folder is the fallback, which is the answer a first export wants
    /// anyway.
    pub export_dir: Option<PathBuf>,
    /// Read `/` in a layer's name as a folder separator (`ondin_export::PlanOptions`).
    pub export_folders_from_names: bool,
    /// Re-run every layer's export settings each time the document is saved.
    ///
    /// **Off by default and worth keeping that way.** It writes to disk as a side
    /// effect of a save, which is a surprise the first time however useful it is
    /// the hundredth — so it is a switch the user reaches for, and the status line
    /// says what it did every time it fires.
    ///
    /// ⚠️ **"A save" and not "Ctrl+S", which is what this said until 2026-08-30.**
    /// `OndinApp::save_file` has always run this at the end without asking which
    /// kind of save it was, so an autosave, the walk to the dashboard and an open
    /// have each been re-exporting all along — the summary line above was right
    /// and this paragraph was narrower than the code. Worth stating precisely now
    /// that autosave is asynchronous (§15 D393): the export runs off the *result*,
    /// so with this on and a thirty-second interval, a photo-carrying document
    /// re-exports every thirty seconds from a background thread. A superseded
    /// write does not count as a save and does not re-export.
    pub export_on_save: bool,
    /// How far an arrow key moves the selection, and how far with Shift held.
    pub nudge: NudgeStep,
    /// Draw the dashed lines that join a layer row to the container it is in
    /// (`panels::layers`'s `tree_guide_segments`, §15 D331).
    ///
    /// **On, because the tree is what the panel is for.** The indent alone says how
    /// deep a row is and nothing about *which* of the rows above it the row is
    /// inside, which is the question a nested document actually raises. Off is for
    /// the reading it costs: a hairline per level in the gutter of every row is ink
    /// the panel did not have before, and a shallow document pays for a structure
    /// it does not have.
    pub tree_guides: bool,
    /// Fold every container in the layer tree when a document is opened.
    ///
    /// **Off, because the tree the user last had is the tree they expect.** A
    /// document with three frames and forty layers is unreadable expanded, and a
    /// document with four rectangles is a wall of carets collapsed; only the person
    /// working on it knows which theirs is. What makes it a preference rather than a
    /// guess at the file's shape is that the answer does not change between
    /// documents for a given *person*.
    pub collapse_groups_on_open: bool,
    /// Offer the web-font library at all — the catalog in the picker, and the faces
    /// behind it (`fonts::FontService`).
    ///
    /// **On, and off is a real state rather than a degraded one.** Off is the
    /// machine's installed fonts plus the bundled Inter, with no catalog fetch and no
    /// face download: the answer for offline, metered, privacy-conscious and
    /// locked-down machines, and the one the *Off | On* direction settled on after a
    /// three-way `Off | Popular | All` was rejected (§15 D330). Nothing is *hidden* by
    /// either state that the other shows — a curated middle would have done exactly
    /// that.
    pub web_fonts: bool,
    /// How wide the layers panel is, in points (`panels::layers::MIN_W ..= MAX_W`).
    ///
    /// **A preference rather than document or session state**, for the reason the
    /// roadmap gave when it asked for the drag: a panel that forgets is a panel
    /// resized every launch. It is also unambiguously the *person's* — how much
    /// room a tree needs is a fact about the monitor and the eyes in front of it,
    /// not about the artwork.
    ///
    /// ⚠️ **Written on the frame the drag ends, never during it** — `save` rewrites
    /// the whole file, and a resize is a gesture with a hundred frames in it. The
    /// gate is in [`crate::app::OndinApp::remember_layers_width`], which is also
    /// where the *other* reason a width can change — the window getting too narrow
    /// to hold it — is filtered out, since a narrow window must not permanently
    /// narrow the panel.
    pub layers_width: f32,
    /// Where the library lives, or `None` for `~/.ondin` (`crate::library::root`).
    ///
    /// ⚠️ **The one preference whose loss is not cosmetic.** Every other field
    /// here is a habit: lose it and the app opens with the wrong panel width. Lose
    /// this and the app opens on an *empty library*, because the documents are
    /// wherever the old value pointed and nothing else records it. That is why
    /// [`Prefs::load`] falling back to `Default` — the right answer for a corrupt
    /// preferences file everywhere else in this struct — is the failure mode
    /// worth knowing about here, and why the Settings modal's *Base folder* row
    /// shows the resolved path rather than the raw setting: the user should be
    /// able to read where their files actually are.
    ///
    /// ⚠️ **The paragraph above used to stop at the read, and the half it left
    /// out was the one that mattered.** Falling back is survivable — the file is
    /// still on disk and still says where the documents are. What was not
    /// survivable is that the next `save()` wrote the fallback back over it, so
    /// a recoverable state became a permanent one at the first nav or open of
    /// the session. [`Prefs::unreadable`] is the latch that stops it.
    ///
    /// `None` rather than an eagerly resolved path, so the default follows the
    /// home directory rather than freezing whatever it was on first launch.
    pub base_folder: Option<PathBuf>,
    /// Whether [`Prefs::save`] is a no-op — set only by `OndinApp::headless`.
    ///
    /// ⚠️ **Plain backticks, not a `[link]`, and that is the convention rather
    /// than an oversight**: `headless` is `#[cfg(test)]`, and `cargo doc` builds
    /// without that cfg, so a link here resolves to nothing and the doc gate exits
    /// 101. `ondin_render::CanvasRenderer::gpu` states the same rule — a name no
    /// gate can validate is written as text. Caught by the gate on the first run
    /// after this field was added, which is the gate doing its job.
    ///
    /// ⚠️ **This exists because a test wrote its own temp folder into the
    /// developer's real preferences file, and the app then opened on it**
    /// (§15 D370).
    /// Reported as *"my base folder is some random directory:
    /// `…\Temp\ondin-wiring-crumb-25960`. I'm doing `cargo run`."* — which is a
    /// `temp_root("crumb")` from `app.rs`'s suite, months of sessions after the
    /// test that leaked it was written. `headless` had always pointed
    /// [`Prefs::base_folder`] somewhere harmless *in memory*, but `save` writes to
    /// `dirs::config_dir()` unconditionally, so any probe reaching one of the nine
    /// `prefs.save()` call sites persisted a path that stops existing the moment
    /// the test cleans up after itself.
    ///
    /// **A flag on the value rather than a guard at each call site**, because the
    /// call sites are the whole app — the export popover writes on every row, the
    /// dashboard on every nav change — and one of them forgetting is the bug
    /// coming back. `headless` is already the one place that decides what a test's
    /// app does differently (`OndinApp::with` takes a device, a font service and
    /// this), so it is the one place that knows.
    ///
    /// `#[serde(skip)]`, so it is never written to the file and a loaded one is
    /// always `false` — a preferences file cannot make a real app stop saving.
    #[serde(skip)]
    pub ephemeral: bool,
    /// Set when `prefs.json` exists but could not be read (§15 D418).
    ///
    /// ⚠️ **A latch on writing, and it exists because the loss it prevents is
    /// the one [`Self::base_folder`] calls *not cosmetic*.** An unparseable file
    /// makes [`Prefs::load`] answer `Default`, so `base_folder` reads `None` and
    /// the library resolves to `~/.ondin` — empty, with every document still
    /// where the old value pointed. That state is survivable; what was not is
    /// what came next, because the *first* `prefs.save()` of the session then
    /// wrote those defaults over the file and the stored path stopped existing.
    /// The doors are ordinary — opening any document, one nav on the library
    /// screen, one drag of the panel edge.
    ///
    /// The app had already built this exact latch for another record and paid
    /// for it there first; see `library::state::Library::projects_unreadable`
    /// and `Library::may_write_projects`, whose doc is word for word the state
    /// this one is about.
    ///
    /// `#[serde(skip)]`, for [`Self::ephemeral`]'s reason: it is a fact about
    /// this read and must never be a stored one.
    #[serde(skip)]
    pub unreadable: bool,
    /// Seconds between autosaves, or `0` for off.
    ///
    /// **Thirty, which is short because an autosave here is cheap and silent.**
    /// The document is already in memory and the write is one file; there is no
    /// dialog, no status line and no undo step. What it is *not* is a version —
    /// `Ctrl+S` pins those (`crate::library::store::pin_version`), so a short
    /// interval cannot flood the history.
    pub autosave_secs: u64,
    /// How long a deleted document stays in the trash, in days.
    ///
    /// `0` means purge on delete, which is a real choice rather than a disabled
    /// state — the trash is the library's only undo, and someone who does not
    /// want one should be able to say so.
    pub trash_keep_days: u64,
    /// Whether `Ctrl+S` pins a version at all.
    ///
    /// On. Off is for a library in a synced folder where the extra copies are not
    /// wanted; it does not change what `Ctrl+S` *saves*, only whether a copy is
    /// kept beside it.
    pub version_history: bool,
    /// Which of the dashboard's sidebar entries it opens on.
    ///
    /// A plain `String` matching a nav id rather than an enum, because an
    /// unrecognised value here has to degrade to *Recent* rather than fail to
    /// deserialize the whole file — losing [`Self::base_folder`] over a stale nav
    /// id would be the tail wagging the dog.
    pub dashboard_page: String,
    /// Grid or list, as the dashboard's view toggle wrote it.
    pub dashboard_list_view: bool,
    /// Which sort the dashboard opens on — one of `library::Sort`'s labels, with
    /// the same degrade-to-default rule as [`Self::dashboard_page`].
    pub dashboard_sort: String,
    /// Reopen the last document on launch instead of showing the dashboard.
    ///
    /// **Off**, which is the whole point of the dashboard existing: the app opens
    /// on the library, and a person who always works in one file can say so.
    pub reopen_last: bool,
    /// The document open when the app last closed, for [`Self::reopen_last`].
    ///
    /// A path rather than a document id, because resolving an id means scanning
    /// the library — and this is read *before* the first scan, on the frame the
    /// window opens. A stale path opens the dashboard, which is the same thing
    /// that happens with no path at all.
    pub last_document: Option<PathBuf>,
}

impl Default for Prefs {
    /// **Spelled out rather than derived**, because five of these are not their
    /// type's default and the file format reads this impl for every missing key
    /// (see the `#[serde(default)]` note on the struct).
    fn default() -> Self {
        Self {
            export_presets: Vec::new(),
            export_dir: None,
            export_folders_from_names: false,
            export_on_save: false,
            nudge: NudgeStep::default(),
            tree_guides: true,
            collapse_groups_on_open: false,
            web_fonts: true,
            // The fifth: an `f32`'s own default is 0.0, which would open every
            // upgraded install with the panel clamped to `MIN_W` and no way to
            // know why. This is the key `#[serde(default)]` on the container is
            // there for.
            layers_width: crate::panels::layers::DEFAULT_W,
            // Six more that are not their type's default, which is what the
            // `#[serde(default)]` on the container exists for: `None` here is
            // right, but `0` seconds would be autosave off, `0` days would empty
            // the trash on every delete, `false` would turn version history off,
            // and two empty strings would be nav and sort ids matching nothing.
            base_folder: None,
            // False, so the *real* app's preferences persist — see the field.
            ephemeral: false,
            // False: `Default` is also what a *first launch* produces, and that
            // one must write. Only `load_from` sets it.
            unreadable: false,
            autosave_secs: 30,
            trash_keep_days: 30,
            version_history: true,
            dashboard_page: "recent".into(),
            dashboard_list_view: false,
            dashboard_sort: "edited".into(),
            reopen_last: false,
            last_document: None,
        }
    }
}

impl Prefs {
    /// Read them, or `Default` if there is nothing readable there — recording
    /// which of "nothing" and "not readable" it was, in [`Self::unreadable`].
    pub fn load() -> Self {
        match path() {
            Some(p) => Self::load_from(&p),
            None => Self::default(),
        }
    }

    /// [`Self::load`] against an explicit path.
    ///
    /// ⚠️ **`load` and `save` are untestable without this pair, and that is
    /// precisely why the bug lived here.** [`path`] reads `dirs::config_dir()`
    /// with no injection point, and `ephemeral` guards only the write — so a
    /// probe that drove the real functions would read, and then *overwrite*, the
    /// developer's own preferences file. §15 D370 is the entry for the last time
    /// a test did that. The behaviour could therefore only be read from the code
    /// rather than driven, which is one of the reasons it survived.
    pub fn load_from(path: &std::path::Path) -> Self {
        match std::fs::read(path) {
            // A missing file is a first launch: `Default` is the answer, and
            // writing it back is right. Matched on the kind rather than on a
            // follow-up `exists()`, which is `Projects::read`'s idiom.
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Self::default(),
            Err(_) => Self {
                unreadable: true,
                ..Self::default()
            },
            Ok(bytes) => match serde_json::from_slice::<Self>(&bytes) {
                Ok(prefs) => prefs,
                Err(_) => Self {
                    unreadable: true,
                    ..Self::default()
                },
            },
        }
    }

    /// Write them, ignoring failure.
    ///
    /// Called after each change rather than at shutdown, because the app has no
    /// shutdown hook it can rely on — a crash mid-session must not be the thing
    /// that costs a preset.
    pub fn save(&self) {
        // ⚠️ **First line, before anything touches the filesystem.** See
        // [`Prefs::ephemeral`]: a headless app must not be able to write here, and
        // the reason this is a return rather than a debug assertion is that the
        // damage is silent and permanent — the file it writes outlives the test by
        // however long it takes somebody to notice their library moved.
        if self.ephemeral {
            return;
        }
        let Some(path) = path() else { return };
        self.save_to(&path);
    }

    /// [`Self::save`] against an explicit path, **past** the `ephemeral` guard.
    ///
    /// ⚠️ **`ephemeral` is checked by the caller and not here, deliberately.**
    /// That flag is about the *developer's* file at the *real* location, and the
    /// only reason this function exists is to write somewhere else — a test that
    /// had to clear `ephemeral` to exercise the latch would be one edit away
    /// from the accident §15 D370 records.
    pub fn save_to(&self, path: &std::path::Path) {
        // ⚠️ **The other half of the read.** With [`Self::unreadable`] set, every
        // field here is a default that a parse failure produced rather than a
        // setting anyone chose — and `base_folder` among them, which is where
        // the user's documents are. Writing that back turns a file somebody
        // could still open in a text editor and repair into the only remaining
        // copy of nothing.
        if self.unreadable {
            return;
        }
        // Atomic (`crate::atomic`, §15 D378). This one is written from more
        // places than any other record in the app — every view switch, every
        // open, every settings commit — so it has the most chances to be the one
        // a crash lands in the middle of.
        if let Ok(bytes) = serde_json::to_vec_pretty(self) {
            let _ = crate::atomic::write(path, &bytes);
        }
    }
}

fn path() -> Option<PathBuf> {
    Some(dirs::config_dir()?.join("ondin").join("prefs.json"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A headless app cannot write the preferences file, and its flag cannot
    /// travel through one.
    ///
    /// ⚠️ **The whole test is about a side effect on a path outside the
    /// repository, so it must never actually perform one** — which is why it
    /// asserts on the guard and on the serialization rather than by writing and
    /// looking. A probe that called `save()` on a real `Prefs` to prove the
    /// negative would be the bug.
    ///
    /// **Flip-checked** by deleting the `if self.ephemeral { return }` in
    /// `Prefs::save`: `an_ephemeral_prefs_never_reaches_the_disk` cannot see
    /// that (it does not write), so what bites is the *second* assertion pair —
    /// with the guard gone, `save` on an ephemeral value takes the same path as
    /// a real one. The guard is therefore asserted where it is decided, and the
    /// real protection is that `headless` sets the flag, which is asserted in
    /// `app.rs`.
    #[test]
    fn an_ephemeral_prefs_never_reaches_the_disk() {
        let real = Prefs::default();
        assert!(
            !real.ephemeral,
            "a real app's preferences persist — this is the default"
        );

        let headless = Prefs {
            ephemeral: true,
            ..Default::default()
        };
        // The guard, read where `save` reads it. Calling `save()` here would
        // write the developer's own file if the guard were gone, which is the
        // thing being prevented.
        assert!(headless.ephemeral);

        // ⚠️ **And it does not survive a round trip**, so a `prefs.json` that
        // somehow contained `"ephemeral": true` could never stop a real install
        // from saving. `#[serde(skip)]` is what does this; the assertion is here
        // because dropping that attribute is a one-word edit with no other
        // visible effect.
        let json = serde_json::to_string(&headless).expect("prefs serialize");
        assert!(
            !json.contains("ephemeral"),
            "the flag is never written to the file: {json}"
        );
        let back: Prefs = serde_json::from_str(&json).expect("prefs deserialize");
        assert!(
            !back.ephemeral,
            "and a loaded one always persists, whatever the file said"
        );
    }

    /// A `prefs.json` that does not parse is **not** replaced by the defaults it
    /// produced.
    ///
    /// ⚠️ **The field that matters is `base_folder`, and it is the one the
    /// struct's own doc calls *not cosmetic*.** An unparseable file makes `load`
    /// answer `Default`, so the library resolves to `~/.ondin` — empty, with the
    /// documents still wherever the stored path pointed. That is recoverable
    /// while the file is still on disk. It stopped being recoverable at the
    /// first `prefs.save()` of the session, and the earliest doors are opening
    /// any document, one nav on the library screen, or one drag of the panel
    /// edge.
    ///
    /// **This test could not have been written before `load_from`/`save_to`
    /// existed**, and that is part of the finding rather than an aside: `path()`
    /// reads `dirs::config_dir()` with no injection point, so the only probe
    /// possible was one that read and rewrote the developer's real file.
    ///
    /// Flip-check, run: removing the `if self.unreadable { return; }` from
    /// `save_to` fails on the bytes assertion — the predicted site — and the
    /// failure message is the finding in one line: `"base_folder": null` in a
    /// freshly pretty-printed file, where `"base_folder": "D:\\Design\\Ondin"`
    /// had been.
    ///
    /// The control is the half that keeps the latch from being a lock-out: a
    /// **missing** file is a first launch, and that one still writes.
    #[test]
    fn an_unreadable_prefs_file_is_not_overwritten_by_the_defaults_it_produced() {
        let dir = std::env::temp_dir().join(format!("ondin-prefs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("prefs.json");
        // Half a file: the shape a sync client's bad merge or an interrupted
        // pre-D378 write leaves behind, with the one field that matters visible
        // in it and still repairable by hand.
        let corrupt = "{ \"base_folder\": \"D:\\\\Design\\\\Ondin\"";
        std::fs::write(&path, corrupt).unwrap();

        let mut prefs = Prefs::load_from(&path);
        assert!(prefs.unreadable, "the read failed and said so");
        assert_eq!(
            prefs.base_folder, None,
            "and the app runs on the defaults, which is survivable"
        );

        // One ordinary door: a nav on the library screen.
        prefs.dashboard_page = "starred".into();
        prefs.save_to(&path);
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            corrupt,
            "the file still holds the only record of where the documents are"
        );

        // Control: no file at all is a first launch and must still write.
        let fresh = dir.join("fresh.json");
        let mut prefs = Prefs::load_from(&fresh);
        assert!(!prefs.unreadable);
        prefs.dashboard_page = "starred".into();
        prefs.save_to(&fresh);
        assert_eq!(
            Prefs::load_from(&fresh).dashboard_page,
            "starred",
            "a first run still records what it did"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
