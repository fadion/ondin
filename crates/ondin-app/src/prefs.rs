//! Preferences that outlive a session — the settings that belong to the
//! *person* rather than to the document.
//!
//! Deliberately not in the `.ondin` file: an export preset is a working habit
//! and a last-used folder is a machine path, meaningless or a small leak for
//! whoever the file is shared with. Everything the artwork needs to export
//! itself is on the layers (§7).
//!
//! Two doors write the one file: the export panel's overflow popover for the
//! export habits, and [`crate::settings`] for rows no panel is a door onto. A
//! setting belongs beside the thing it changes when there is one.
//!
//! Best-effort at both ends: a failed read is `Default`, a failed write is
//! dropped, and nothing waits on either. The file lives under
//! `dirs::config_dir()/ondin`.

use crate::input::NudgeStep;
use ondin_core::ExportSpec;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// A named set of export specs, applied to a layer in one click. Copy-pasting
/// settings between layers stops being enough once the same set is needed in a
/// different file.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ExportPreset {
    pub name: String,
    pub specs: Vec<ExportSpec>,
}

/// The View and Snap menus' switches as last set, so a launch opens on them.
/// Focus mode is not here: it is a mode a launch should never open in.
///
/// Container-level `#[serde(default)]`, as on [`Prefs`]: a missing key takes
/// `Default`'s value.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ViewPrefs {
    pub rulers: bool,
    pub guides: bool,
    pub lock_guides: bool,
    pub grid: bool,
    pub layout_grids: bool,
    pub layers: bool,
    pub history: bool,
    pub minimap: bool,
    pub toolbar: bool,
    pub status: bool,
    pub snap_shapes: bool,
    pub snap_guides: bool,
    pub snap_grid: bool,
    pub snap_baselines: bool,
}

impl Default for ViewPrefs {
    /// Every switch on but the guide lock, History and the minimap.
    fn default() -> Self {
        Self {
            rulers: true,
            guides: true,
            lock_guides: false,
            grid: true,
            layout_grids: true,
            layers: true,
            history: false,
            minimap: false,
            toolbar: true,
            status: true,
            snap_shapes: true,
            snap_guides: true,
            snap_grid: true,
            snap_baselines: true,
        }
    }
}

/// The contrast check's level menu, as last set (`crate::contrast`). The mode
/// itself is view state and never stored.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ContrastPrefs {
    /// `aa`, `aaa` or `apca`; anything else reads as AA.
    pub level: String,
    /// Whether passing layers get their quiet chip.
    pub show_passing: bool,
    /// Whether small shapes are measured too, at 3 : 1.
    pub non_text: bool,
}

impl Default for ContrastPrefs {
    /// AA, passing chips shown, text only.
    fn default() -> Self {
        Self {
            level: "aa".into(),
            show_passing: true,
            non_text: false,
        }
    }
}

/// What the batch rename box last renamed with (`crate::batch_rename`), for the
/// next time it opens.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BatchRenamePrefs {
    pub find: String,
    /// *Rename to*, its tokens spelled `[Name]`, `[Number]`, `[Group k]`; empty
    /// reads as one Name token.
    pub template: String,
    pub from: i64,
    pub step: i64,
    /// Zero-pad to at least this many digits, 0 to 5. Read from `digits`, its
    /// name when it was 1 to 3, which means the same.
    #[serde(alias = "digits")]
    pub pad: u8,
    /// `layers`, `reversed`, `left-to-right` or `top-to-bottom`.
    pub order: String,
}

impl Default for BatchRenamePrefs {
    /// No find, the name kept, numbered 1, 2, 3 in layers order.
    fn default() -> Self {
        Self {
            find: String::new(),
            template: String::new(),
            from: 1,
            step: 1,
            pad: 0,
            order: "layers".into(),
        }
    }
}

/// Everything the Settings modal writes, plus the export habits the export
/// panel's popover writes.
///
/// `#[serde(default)]` sits on the container, not on each field: a field-level
/// default fills a missing key with that type's `Default` (`false`, `0.0`),
/// whereas several settings here default otherwise (web fonts and tree guides
/// on, a nonzero nudge). The container form fills from `Prefs::default()`, the
/// one place the defaults are stated.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct Prefs {
    pub export_presets: Vec<ExportPreset>,
    /// Where the last export went, so *Export all* can repeat with no dialog.
    ///
    /// One folder, not a map per document path, which grows without bound and
    /// goes stale when a file moves. When empty or gone, the document's own
    /// folder is the fallback.
    pub export_dir: Option<PathBuf>,
    /// Read `/` in a layer's name as a folder separator (`ondin_export::PlanOptions`).
    pub export_folders_from_names: bool,
    /// Re-run every layer's export settings each time the document is saved.
    ///
    /// Off by default: it writes to disk as a side effect of a save, and the
    /// status line says what it did each time. "A save" means any save, not only
    /// Ctrl+S: `OndinApp::save_file` runs it at the end, so an autosave, leaving
    /// for the dashboard and an open all re-export. It runs off the autosave's
    /// result; a superseded write does not count as a save.
    pub export_on_save: bool,
    /// How far an arrow key moves the selection, and how far with Shift held.
    pub nudge: NudgeStep,
    /// Draw the dashed lines that join a layer row to the container it is in
    /// (`panels::layers`'s `tree_guide_segments`).
    ///
    /// On: the indent alone does not say which of the rows above a row is inside.
    /// Off saves a hairline per level of ink for a shallow document.
    pub tree_guides: bool,
    /// Fold every container in the layer tree when a document is opened.
    ///
    /// Off. Whether a tree reads better folded depends on the person's habits,
    /// not on the file, hence a preference.
    pub collapse_groups_on_open: bool,
    /// Offer the web-font library at all — the catalog in the picker, and the faces
    /// behind it (`fonts::FontService`).
    ///
    /// On, and off is a real state, not a degraded one: the machine's installed
    /// fonts plus the bundled Inter, with no catalog fetch and no face download,
    /// for offline, metered and locked-down machines. Neither state hides
    /// anything the other shows.
    pub web_fonts: bool,
    /// The family a new text layer starts in (`fonts::FontService::new_text_parts`).
    ///
    /// A family name rather than a font: one that is no longer installed or
    /// downloadable degrades to Inter at creation, and the preference stays, for a
    /// font that comes back.
    pub default_font: String,
    /// How wide the layers panel is, in points (`panels::layers::MIN_W ..= MAX_W`).
    ///
    /// A preference: how much room a tree needs is a fact about the monitor and
    /// the eyes, not the artwork.
    ///
    /// Written on the frame the drag ends, never during it, since `save` rewrites
    /// the whole file. The gate is [`crate::app::OndinApp::remember_layers_width`],
    /// which also filters out a width changed only by the window getting too
    /// narrow to hold it.
    pub layers_width: f32,
    /// Whether the layers pane's Pages section is folded to its header
    /// (`crate::pages`). A preference for `layers_width`'s reason.
    pub pages_folded: bool,
    /// Whether the layers pane's Layers section is folded to its header, hiding
    /// every layer name.
    pub layers_folded: bool,
    /// How tall the Pages section's list was dragged to, in points; `None` fits
    /// up to six rows.
    pub pages_height: Option<f32>,
    /// Whether the layers pane's History section is folded to its header
    /// (`crate::panels::history`).
    pub history_folded: bool,
    /// How tall the History section is, header included, in points, as its
    /// divider was dragged; `None` for the default.
    pub history_height: Option<f32>,
    /// Where the library lives, or `None` for `~/.ondin` (`crate::library::root`).
    ///
    /// The one preference whose loss is not cosmetic: lose it and the app opens
    /// on an empty library, because nothing else records where the documents are.
    /// So the Settings modal's *Base folder* row shows the resolved path, and
    /// [`Prefs::unreadable`] stops the fallback `Default` from being saved over the
    /// stored value.
    ///
    /// `None` rather than an eagerly resolved path, so the default follows the
    /// home directory.
    pub base_folder: Option<PathBuf>,
    /// Whether [`Prefs::save`] is a no-op — set only by `OndinApp::headless`.
    ///
    /// Written with plain backticks, not a link: `headless` is `#[cfg(test)]`,
    /// which `cargo doc` does not build.
    ///
    /// A flag on the value rather than a guard at each call site, so a test's app
    /// cannot write its temp folder into the developer's real preferences file
    /// through any of the `prefs.save()` calls. `headless` is the one place that
    /// decides what a test's app does differently.
    ///
    /// `#[serde(skip)]`: never written, always `false` when loaded, so a
    /// preferences file cannot make a real app stop saving.
    #[serde(skip)]
    pub ephemeral: bool,
    /// Set when `prefs.json` exists but could not be read.
    ///
    /// A latch on writing: an unparseable file makes [`Prefs::load`] answer
    /// `Default`, and the first `prefs.save()` of the session would then write
    /// those defaults over the file, losing the stored [`Self::base_folder`] for
    /// good. The same latch guards the library's projects record
    /// (`library::state::Library::may_write_projects`).
    ///
    /// `#[serde(skip)]`, for [`Self::ephemeral`]'s reason: it is a fact about
    /// this read, never a stored one.
    #[serde(skip)]
    pub unreadable: bool,
    /// Seconds between autosaves, or `0` for off.
    ///
    /// Thirty: an autosave is cheap and silent, with no dialog, status line or
    /// undo step. It is not a version — `Ctrl+S` pins those
    /// (`crate::library::store::pin_version`), so a short interval cannot flood
    /// the history.
    pub autosave_secs: u64,
    /// How long a deleted document stays in the trash, in days.
    ///
    /// `0` means purge on delete, a real choice: the trash is the library's only
    /// undo.
    pub trash_keep_days: u64,
    /// How many undo steps a document keeps once it is closed
    /// (`crate::library::history`), or `0` for none. The open session's history is
    /// never cut.
    ///
    /// A hundred: enough to come back to an afternoon's work, and a file of a size a
    /// synced library does not notice.
    pub history_size: u64,
    /// Whether `Ctrl+S` pins a version at all.
    ///
    /// On. Off is for a library in a synced folder where the extra copies are not
    /// wanted; it changes only whether a copy is kept beside the save.
    pub version_history: bool,
    /// Which of the dashboard's sidebar entries it opens on.
    ///
    /// A plain `String` matching a nav id rather than an enum, so an unrecognised
    /// value degrades to *Recent* instead of failing to deserialize the whole file
    /// and losing [`Self::base_folder`].
    pub dashboard_page: String,
    /// Grid or list, as the dashboard's view toggle wrote it.
    pub dashboard_list_view: bool,
    /// Which sort the dashboard opens on — one of `library::Sort`'s labels, with
    /// the same degrade-to-default rule as [`Self::dashboard_page`].
    pub dashboard_sort: String,
    /// Put the tab that was in front at the last quit in front again on launch,
    /// instead of the library. The tabs come back either way ([`Self::open_tabs`]).
    pub reopen_last: bool,
    /// The documents open in tabs at the last quit, in strip order; a document
    /// never filed has no path and is not among them.
    pub open_tabs: Vec<PathBuf>,
    /// Which of [`Self::open_tabs`] was in front, or `None` for the library.
    pub front_tab: Option<usize>,
    /// How many of [`Self::open_tabs`], from the first, were pinned: pinned tabs
    /// always lead the strip.
    pub pinned_tabs: usize,
    /// The document opened last. Read at launch only when [`Self::open_tabs`] is
    /// empty, as a preferences file from before tabs has it.
    ///
    /// A path rather than a document id, since resolving an id means scanning the
    /// library and this is read before the first scan. A stale path opens the
    /// dashboard.
    pub last_document: Option<PathBuf>,
    /// The command palette's last rows run, most recent first, as
    /// `crate::palette::verb_key` names them: by verb, so a renamed label
    /// (*Hide* / *Show*) is still the same entry. A habit, hence a preference.
    pub palette_recent: Vec<String>,
    /// The View and Snap switches, written as they change
    /// (`OndinApp::remember_view_switches`).
    pub view: ViewPrefs,
    /// The colour picker's value-row format, as `panels::picker::Format::key`
    /// names it, with [`Self::dashboard_page`]'s degrade-to-default rule (to Hex).
    pub colour_format: String,
    /// The colours last committed from a picker, a colour field or the
    /// eyedropper, most recent first, as sRGB 0..1 without alpha: a swatch sets
    /// the colour and the paint keeps its own opacity. The person's, not the
    /// document's, so they carry from one file to the next.
    pub recent_colours: Vec<[f32; 3]>,
    /// The contrast check's level and switches.
    pub contrast: ContrastPrefs,
    /// The batch rename box's last Find, template and numbering.
    pub batch_rename: BatchRenamePrefs,
}

impl Default for Prefs {
    /// Spelled out rather than derived, because several fields are not their
    /// type's default and the file format reads this impl for every missing key.
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
            default_font: "Inter".into(),
            // An `f32`'s own default of 0.0 would clamp the panel to `MIN_W` on
            // every upgraded install.
            layers_width: crate::panels::layers::DEFAULT_W,
            pages_folded: false,
            layers_folded: false,
            pages_height: None,
            history_folded: false,
            history_height: None,
            // `0` seconds would be autosave off, `0` days would empty the trash
            // on every delete, `false` would turn version history off, and empty
            // strings would be nav and sort ids matching nothing.
            base_folder: None,
            ephemeral: false,
            // False: `Default` is also a first launch's value, which must write.
            // Only `load_from` sets it.
            unreadable: false,
            autosave_secs: 30,
            trash_keep_days: 30,
            history_size: 100,
            version_history: true,
            dashboard_page: "recent".into(),
            dashboard_list_view: false,
            dashboard_sort: "edited".into(),
            reopen_last: false,
            open_tabs: Vec::new(),
            front_tab: None,
            pinned_tabs: 0,
            last_document: None,
            palette_recent: Vec::new(),
            view: ViewPrefs::default(),
            colour_format: "hex".into(),
            recent_colours: Vec::new(),
            contrast: ContrastPrefs::default(),
            batch_rename: BatchRenamePrefs::default(),
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

    /// [`Self::load`] against an explicit path, so tests need not read and
    /// overwrite the developer's real preferences file ([`path`] has no
    /// injection point).
    pub fn load_from(path: &std::path::Path) -> Self {
        match std::fs::read(path) {
            // A missing file is a first launch: `Default`, and writing it back
            // is right.
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

    /// Write them, ignoring failure. Called after each change rather than at
    /// shutdown, which the app cannot rely on.
    pub fn save(&self) {
        self.save_located(path);
    }

    /// [`Self::save`] with the real location supplied by `locate`, so a test can
    /// check the `ephemeral` guard against a temp path instead of the real file.
    fn save_located(&self, locate: impl FnOnce() -> Option<PathBuf>) {
        // First, before anything touches the filesystem; a return rather than a
        // debug assertion because the damage of a headless write is silent and
        // permanent. See [`Prefs::ephemeral`].
        if self.ephemeral {
            return;
        }
        let Some(path) = locate() else { return };
        self.save_to(&path);
    }

    /// [`Self::save`] against an explicit path, past the `ephemeral` guard: that
    /// flag protects the real location, and a test that had to clear it to
    /// exercise the latch would be one edit from overwriting the real file.
    pub fn save_to(&self, path: &std::path::Path) {
        // With [`Self::unreadable`] set every field is a default a parse failure
        // produced, `base_folder` included; writing it back would destroy a file
        // that could still be repaired by hand.
        if self.unreadable {
            return;
        }
        // Atomic (`crate::atomic`): this record is written from more places than
        // any other, so it has the most chances to meet a crash mid-write.
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

    /// A file from before the view switches were kept opens with all of them on, and
    /// one that names a single switch keeps the rest on.
    #[test]
    fn a_missing_view_switch_reads_as_on() {
        let old: Prefs = serde_json::from_str("{}").unwrap();
        assert_eq!(old.view, ViewPrefs::default());
        let one: Prefs = serde_json::from_str(r#"{"view":{"snap_grid":false}}"#).unwrap();
        assert!(!one.view.snap_grid);
        assert!(one.view.rulers && one.view.snap_shapes && !one.view.lock_guides);
    }

    /// The `ephemeral` flag defaults off and never survives serialization. It
    /// asserts on the flag and the JSON rather than calling `save()`, which would
    /// write the real preferences file if the guard were gone.
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
        assert!(headless.ephemeral);

        // It does not survive a round trip, so a file containing
        // `"ephemeral": true` cannot stop a real install from saving; dropping
        // `#[serde(skip)]` has no other visible effect.
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

    /// An ephemeral `save` writes nothing and never asks where the file lives;
    /// an ordinary one writes.
    /// flip: removing the `if self.ephemeral { return; }` from `save_located` →
    /// fails at "an ephemeral save never asks where the file lives".
    #[test]
    fn an_ephemeral_save_writes_nothing() {
        let dir = std::env::temp_dir().join(format!("ondin-prefs-eph-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("prefs.json");

        let asked = std::cell::Cell::new(false);
        let headless = Prefs {
            ephemeral: true,
            ..Default::default()
        };
        headless.save_located(|| {
            asked.set(true);
            Some(path.clone())
        });
        assert!(
            !asked.get(),
            "an ephemeral save never asks where the file lives"
        );
        assert!(!path.exists(), "and writes nothing");

        // Control: the same call without the flag writes, so the closure is the
        // path a real save takes.
        Prefs::default().save_located(|| Some(path.clone()));
        assert!(path.exists(), "an ordinary save writes the file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// A `prefs.json` that does not parse is not replaced by the defaults it
    /// produced (they would lose `base_folder`); a missing file is a first launch
    /// and still writes.
    /// flip: removing the `if self.unreadable { return; }` from `save_to` →
    /// fails at the bytes assertion.
    #[test]
    fn an_unreadable_prefs_file_is_not_overwritten_by_the_defaults_it_produced() {
        let dir = std::env::temp_dir().join(format!("ondin-prefs-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("prefs.json");
        // Half a file, with the one field that matters still repairable by hand.
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
