//! **Document tabs**: more than one open document in one window (design canvas,
//! *Document Tabs*). Each open document is a tab in the strip along the top of the
//! window, after the library's, which is always first.
//!
//! **One tab's editor state lives in `OndinApp`'s own fields** — `session` and the
//! app-side state that goes with one document — so every reader of `self.session`
//! reads the document in front. That tab is the *active* one ([`Tabs::active`]).
//! Every other loaded tab is parked: its state moved into a [`Parked`] by
//! [`OndinApp::park_active`] and moved back by `OndinApp::unpark`. A tab can also be
//! only a path ([`Body::Unloaded`]): restored at launch and not shown yet, being
//! read, or unreadable.
//!
//! Leaving a tab saves it as leaving the editor for the library does
//! (`OndinApp::go_to_dashboard`), so a tab behind is clean unless it cannot be
//! saved — never filed, or its save failed — and nothing is written behind the
//! user's back while it waits.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;

use ondin_core::NodeId;

use crate::app::{OndinApp, View};
use crate::session::EditorSession;

/// A tab's identity for the session; never reused, so a stale one finds nothing.
pub(crate) type TabId = u64;

/// The strip's state.
#[derive(Default)]
pub(crate) struct Tabs {
    /// The open documents, in strip order. The library's tab is not in it: it is
    /// always first and never moves.
    pub(crate) list: Vec<Tab>,
    /// The tab whose state is the app's own fields; `None` while the fields hold a
    /// scratch session no tab shows.
    pub(crate) active: Option<TabId>,
    /// The tab in front whenever the library is not ([`View::Editor`]): the active
    /// one, or one still being read or that could not be read, which the editor
    /// shows a placeholder for while the active state waits untouched.
    pub(crate) shown: Option<TabId>,
    next: TabId,
    /// Documents closed this session, last closed last, with the index each stood
    /// at: `Ctrl+Shift+T`'s stack.
    pub(crate) closed: Vec<(PathBuf, usize)>,
    /// The first tab drawn while they overflow the strip.
    pub(crate) scroll: usize,
    /// Whether the caret's list of every open document is showing.
    pub(crate) overflow_open: bool,
    /// A window opened while another Ondin runs keeps its tabs to itself: it
    /// neither brings back the last quit's tabs, which the other window may hold
    /// open, nor writes its own over that window's list.
    pub(crate) private: bool,
}

pub(crate) struct Tab {
    pub(crate) id: TabId,
    pub(crate) body: Body,
}

pub(crate) enum Body {
    /// Its state is the app's own fields.
    Active,
    /// Behind, its state moved aside.
    Parked(Box<Parked>),
    /// Only its file: not read yet, being read, or not readable (`failed`).
    Unloaded {
        path: PathBuf,
        failed: Option<String>,
    },
}

/// A tab's editor state while another is in front: the whole session — document,
/// history, selection, page, camera and save state — and what the app keeps beside
/// it for one document. The tool, the panes and the view switches are the window's
/// and stay where they are.
pub(crate) struct Parked {
    pub(crate) session: EditorSession,
    recovery_key: Option<String>,
    recovery_at: Option<u64>,
    recovery_last: Option<Instant>,
    last_autosave: Instant,
    collapsed: HashSet<NodeId>,
    kit_return: Option<(PathBuf, String)>,
}

impl Tabs {
    /// One tab, active and in front: the state the headless app starts in, its
    /// starter document open as the launch screen never leaves it.
    pub(crate) fn with_active() -> Self {
        let mut tabs = Self::default();
        let id = tabs.add(Body::Active);
        tabs.active = Some(id);
        tabs.shown = Some(id);
        tabs
    }

    /// Put a tab at the end of the strip.
    pub(crate) fn add(&mut self, body: Body) -> TabId {
        self.insert(self.list.len(), body)
    }

    /// Put a tab at `index`, or at the end when the strip is shorter.
    pub(crate) fn insert(&mut self, index: usize, body: Body) -> TabId {
        self.next += 1;
        let id = self.next;
        self.list
            .insert(index.min(self.list.len()), Tab { id, body });
        id
    }

    pub(crate) fn index(&self, id: TabId) -> Option<usize> {
        self.list.iter().position(|t| t.id == id)
    }

    pub(crate) fn get(&self, id: TabId) -> Option<&Tab> {
        self.list.iter().find(|t| t.id == id)
    }

    pub(crate) fn get_mut(&mut self, id: TabId) -> Option<&mut Tab> {
        self.list.iter_mut().find(|t| t.id == id)
    }

    /// Move tab `id` to `index`, the library's slot not counted.
    pub(crate) fn move_to(&mut self, id: TabId, index: usize) {
        let Some(from) = self.index(id) else {
            return;
        };
        let tab = self.list.remove(from);
        self.list.insert(index.min(self.list.len()), tab);
    }
}

/// What the strip and the close card say about one tab.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct TabInfo {
    pub id: TabId,
    pub name: String,
    /// The project's name, for the `Project / Document` tooltip; `None` unfiled.
    pub project: Option<String>,
    pub path: Option<PathBuf>,
    /// Whether the document is a shared kit, which wears the kit glyph.
    pub kit: bool,
    pub dirty: bool,
    pub loading: bool,
    pub failed: Option<String>,
}

impl TabInfo {
    /// The tooltip's `Project / Document`, or the name alone outside a project.
    pub(crate) fn full_path(&self) -> String {
        match &self.project {
            Some(project) => format!("{project} / {}", self.name),
            None => self.name.clone(),
        }
    }
}

impl OndinApp {
    /// The file tab `tab` holds, if it has one.
    pub(crate) fn tab_path<'a>(&'a self, tab: &'a Tab) -> Option<&'a Path> {
        match &tab.body {
            Body::Active => self.session.path.as_deref(),
            Body::Parked(p) => p.session.path.as_deref(),
            Body::Unloaded { path, .. } => Some(path),
        }
    }

    /// The tab holding `path`, so a document is never open twice: two sessions
    /// saving one file would overwrite each other.
    pub(crate) fn tab_of_path(&self, path: &Path) -> Option<TabId> {
        self.tabs
            .list
            .iter()
            .find(|t| self.tab_path(t) == Some(path))
            .map(|t| t.id)
    }

    /// The tab in front, or `None` while the library is.
    pub(crate) fn front_tab(&self) -> Option<TabId> {
        match self.view {
            View::Editor => self.tabs.shown,
            View::Dashboard => None,
        }
    }

    /// Every tab as the strip draws it, in order.
    pub(crate) fn tab_infos(&self) -> Vec<TabInfo> {
        self.tabs.list.iter().map(|t| self.tab_info(t)).collect()
    }

    fn tab_info(&self, tab: &Tab) -> TabInfo {
        let of_session = |s: &EditorSession| {
            let meta = s.doc.meta();
            TabInfo {
                id: tab.id,
                name: s.document_title(),
                project: self.project_name(meta.project.as_deref()),
                path: s.path.clone(),
                kit: meta.kit.as_ref().is_some_and(|k| k.is_shared()),
                dirty: s.is_dirty(),
                loading: false,
                failed: None,
            }
        };
        match &tab.body {
            Body::Active => of_session(&self.session),
            Body::Parked(p) => of_session(&p.session),
            Body::Unloaded { path, failed } => {
                let entry = self.library.entry_at(path);
                let stem = path
                    .file_stem()
                    .map(|s| s.to_string_lossy().into_owned())
                    .unwrap_or_default();
                TabInfo {
                    id: tab.id,
                    name: entry.map_or_else(
                        || crate::library::scan::display_name(&Default::default(), &stem),
                        |e| e.display_name(),
                    ),
                    project: self.project_name(entry.and_then(|e| e.meta.project.as_deref())),
                    path: Some(path.clone()),
                    kit: entry
                        .and_then(|e| e.meta.kit.as_ref())
                        .is_some_and(|k| k.is_shared()),
                    dirty: false,
                    loading: self.opening.as_ref().is_some_and(|o| o.tab == tab.id),
                    failed: failed.clone(),
                }
            }
        }
    }

    fn project_name(&self, id: Option<&str>) -> Option<String> {
        id.and_then(|id| self.library.projects.get(id))
            .map(|p| p.name.clone())
    }

    /// The tabs with unsaved work, in strip order.
    pub(crate) fn dirty_tabs(&self) -> Vec<TabId> {
        self.tabs
            .list
            .iter()
            .filter(|t| match &t.body {
                Body::Active => self.session.is_dirty(),
                Body::Parked(p) => p.session.is_dirty(),
                Body::Unloaded { .. } => false,
            })
            .map(|t| t.id)
            .collect()
    }

    /// Whether closing the window would lose anything: a tab's unsaved work, or the
    /// scratch session's when no tab holds it.
    pub(crate) fn any_unsaved(&self) -> bool {
        !self.dirty_tabs().is_empty() || (self.tabs.active.is_none() && self.session.is_dirty())
    }

    /// The session of every loaded document, the one in front first: what the
    /// library's moves and trashes re-point.
    pub(crate) fn open_sessions(&mut self) -> Vec<&mut EditorSession> {
        let mut out = vec![&mut self.session];
        for tab in &mut self.tabs.list {
            if let Body::Parked(p) = &mut tab.body {
                out.push(&mut p.session);
            }
        }
        out
    }

    /// The file of every tab not read yet, and of every tab `Ctrl+Shift+T` may bring
    /// back, for the same re-pointing.
    pub(crate) fn unloaded_paths(&mut self) -> Vec<&mut PathBuf> {
        self.tabs
            .list
            .iter_mut()
            .filter_map(|t| match &mut t.body {
                Body::Unloaded { path, .. } => Some(path),
                _ => None,
            })
            .chain(self.tabs.closed.iter_mut().map(|(path, _)| path))
            .collect()
    }

    /// A library file has gone to the trash: an open document it backed keeps its
    /// tab with nowhere to save to, and a tab that only named it closes.
    pub(crate) fn open_file_trashed(&mut self, path: &Path) {
        for s in self.open_sessions() {
            if s.path.as_deref() == Some(path) {
                s.path = None;
            }
        }
        let gone: Vec<TabId> = self
            .tabs
            .list
            .iter()
            .filter(|t| matches!(&t.body, Body::Unloaded { path: p, .. } if p == path))
            .map(|t| t.id)
            .collect();
        for id in gone {
            self.discard_tab(id);
        }
    }

    // --- switching -----------------------------------------------------------

    /// Move the active tab's state aside, saved as leaving the editor saves it, and
    /// leave a scratch session in the fields. A no-op with no active tab.
    ///
    /// The text session is finished first, as every exit from the editor does: a
    /// save before it would miss what was typed.
    pub(crate) fn park_active(&mut self) {
        let Some(id) = self.tabs.active else {
            return;
        };
        self.finish_text_first();
        let ctx = self.egui_ctx.clone();
        self.cancel_gesture(&ctx);
        if self.session.is_dirty() && self.session.path.is_some() {
            self.save_file(false);
        }
        if self.session.is_dirty() {
            // No tick runs for a tab behind, so its snapshot is brought up to date
            // now; a never-filed document has nothing else.
            self.recovery.last = None;
            self.recovery_tick(&ctx);
        } else {
            self.drop_recovery();
        }
        // Every result lands while the fields are still the ones it is about.
        self.disk_settle();
        let parked = Parked {
            session: std::mem::take(&mut self.session),
            recovery_key: self.recovery.key.take(),
            recovery_at: self.recovery.at.take(),
            recovery_last: self.recovery.last.take(),
            last_autosave: self.last_autosave,
            collapsed: std::mem::take(&mut self.collapsed),
            kit_return: self.kit_return.take(),
        };
        if let Some(tab) = self.tabs.get_mut(id) {
            tab.body = Body::Parked(Box::new(parked));
        }
        self.tabs.active = None;
        self.leave_document();
    }

    /// Move parked tab `id`'s state into the fields. The caller parked the active
    /// tab first. Answers whether `id` was parked.
    fn unpark(&mut self, id: TabId) -> bool {
        let Some(tab) = self.tabs.get_mut(id) else {
            return false;
        };
        if !matches!(tab.body, Body::Parked(_)) {
            return false;
        }
        let Body::Parked(p) = std::mem::replace(&mut tab.body, Body::Active) else {
            return false;
        };
        let p = *p;
        self.session = p.session;
        self.leave_document();
        self.recovery.key = p.recovery_key;
        self.recovery.at = p.recovery_at;
        self.recovery.last = p.recovery_last;
        self.last_autosave = p.last_autosave;
        // After `leave_document`, which empties both.
        self.collapsed = p.collapsed;
        self.kit_return = p.kit_return;
        self.pages = crate::pages::PagesState::showing(self.session.page);
        self.tabs.active = Some(id);
        true
    }

    /// Drop the state that names the document leaving the fields: a gesture, an
    /// open popover, an edit buffered against its layers.
    fn leave_document(&mut self) {
        self.reset_transient_state();
        self.entered_group = None;
        self.points.clear();
        self.clones = None;
        self.renaming_layer = None;
        self.inspector_hold = None;
        self.layers_synced_selection.clear();
        self.layers_range_anchor = None;
        self.pending_picker = None;
        self.stroke_menu = None;
        self.effect_menu = None;
        self.type_menu = None;
        self.export_row_open = None;
        self.export_menu = false;
        self.offset_card = None;
        self.open_menu = crate::app::TopMenu::None;
        self.context_menu = None;
        self.palette = None;
    }

    /// Bring tab `id` to the front, its state into the fields; a tab that is only a
    /// path starts being read, its tab pulsing meanwhile.
    pub(crate) fn activate_tab(&mut self, id: TabId) {
        let Some(tab) = self.tabs.get(id) else {
            return;
        };
        match &tab.body {
            Body::Active => {}
            Body::Parked(_) => {
                self.park_active();
                self.unpark(id);
            }
            // A failed read waits for *Try again*.
            Body::Unloaded { failed, .. } => {
                let failed = failed.is_some();
                self.tabs.shown = Some(id);
                self.view = View::Editor;
                if !failed {
                    self.begin_load(id);
                }
                return;
            }
        }
        if self.view == View::Dashboard {
            self.leave_library();
        }
        self.tabs.shown = Some(id);
        self.view = View::Editor;
    }

    /// The library tab: the library in front, the active tab's state kept where it
    /// is (`go_to_dashboard` saves it).
    pub(crate) fn show_library(&mut self) {
        if self.view != View::Dashboard {
            self.go_to_dashboard();
        }
    }

    /// What the library's own screen leaves behind when a tab comes forward.
    pub(crate) fn leave_library(&mut self) {
        self.rename_entry = None;
        self.dash.search = None;
        self.dash.menu_for = None;
    }

    /// Bring forward the tab `step` places from the one in front, wrapping, the
    /// library's tab counted: `Ctrl+Tab` and `Ctrl+Shift+Tab`.
    pub(crate) fn step_tab(&mut self, step: isize) {
        let count = self.tabs.list.len() as isize + 1;
        let at = match self.front_tab().and_then(|id| self.tabs.index(id)) {
            Some(i) => i as isize + 1,
            None => 0,
        };
        let to = (at + step).rem_euclid(count) as usize;
        self.show_slot(to);
    }

    /// `Alt+1`…`Alt+8`: that slot, the library's tab counted as the first; `Alt+9`:
    /// the last tab. A slot past the end does nothing.
    pub(crate) fn jump_to_slot(&mut self, n: u8) {
        let slot = if n == 9 {
            self.tabs.list.len()
        } else {
            usize::from(n.saturating_sub(1))
        };
        if slot <= self.tabs.list.len() {
            self.show_slot(slot);
        }
    }

    /// Slot 0 is the library; slot `i` is `tabs.list[i - 1]`.
    fn show_slot(&mut self, slot: usize) {
        match slot.checked_sub(1).and_then(|i| self.tabs.list.get(i)) {
            Some(tab) => self.activate_tab(tab.id),
            None => self.show_library(),
        }
    }

    // --- opening ---------------------------------------------------------------

    /// Read tab `id`'s file off the frame, the tab pulsing until
    /// [`OndinApp::poll_open`] lands it. Only a tab that is only a path.
    pub(crate) fn begin_load(&mut self, id: TabId) {
        let Some(Body::Unloaded { path, .. }) = self.tabs.get(id).map(|t| &t.body) else {
            return;
        };
        let path = path.clone();
        self.read_into_tab(id, &path, false);
    }

    /// Read tab `id`'s file now, on this frame. A failure leaves the tab saying why.
    pub(crate) fn load_now(&mut self, id: TabId) -> bool {
        let Some(Body::Unloaded { path, .. }) = self.tabs.get(id).map(|t| &t.body) else {
            return false;
        };
        let path = path.clone();
        if self.opening.as_ref().is_some_and(|o| o.tab == id) {
            self.opening = None;
        }
        match crate::app::read_opened(&path, &self.library.root, self.prefs.history_size > 0) {
            Ok(opened) => {
                self.adopt_opened_with(&path, opened);
                true
            }
            Err(why) => {
                self.tab_failed(id, why);
                false
            }
        }
    }

    /// A tab's file could not be read: it stays, saying so, to try again or close.
    pub(crate) fn tab_failed(&mut self, id: TabId, why: String) {
        if let Some(Tab {
            body: Body::Unloaded { failed, .. },
            ..
        }) = self.tabs.get_mut(id)
        {
            *failed = Some(why.clone());
        }
        self.session.fail(why);
    }

    /// *Try again* on a tab that could not be read.
    pub(crate) fn retry_tab(&mut self, id: TabId) {
        if let Some(Tab {
            body: Body::Unloaded { failed, .. },
            ..
        }) = self.tabs.get_mut(id)
        {
            *failed = None;
        }
        self.begin_load(id);
    }

    /// The tab a document read for `path` goes into, its state made the fields':
    /// the tab already holding the file, else a new one at the end of the strip.
    /// The active tab is parked unless it is the one.
    pub(crate) fn tab_for_adopting(&mut self, path: Option<&Path>) -> TabId {
        let found = path.and_then(|p| self.tab_of_path(p));
        let id = match found {
            Some(id) if self.tabs.active == Some(id) => id,
            Some(id) => {
                self.park_active();
                if let Some(tab) = self.tabs.get_mut(id) {
                    // A parked state of the same file is replaced; its snapshot
                    // goes with it.
                    let old = std::mem::replace(&mut tab.body, Body::Active);
                    if let Body::Parked(p) = old
                        && let Some(key) = p.recovery_key
                    {
                        self.forget_snapshot(&key);
                    }
                }
                id
            }
            None => {
                self.park_active();
                self.tabs.add(Body::Active)
            }
        };
        if self.opening.as_ref().is_some_and(|o| o.tab == id) {
            self.opening = None;
        }
        self.tabs.active = Some(id);
        self.tabs.shown = Some(id);
        id
    }

    // --- closing ---------------------------------------------------------------

    /// Close tab `id`: the ×, a middle-click, `Ctrl+W`. A document in the library
    /// saves and closes without asking; only work that cannot be saved is asked
    /// about, on the *Unsaved changes* card.
    pub(crate) fn close_tab(&mut self, id: TabId) {
        if self.tabs.active == Some(id) {
            self.finish_text_first();
            if self.session.is_dirty() && self.session.path.is_some() {
                self.save_file(false);
            }
        }
        if self.dirty_tabs().contains(&id) {
            self.closing_tab = Some(id);
            return;
        }
        self.discard_tab(id);
    }

    /// Close tab `id` whatever it holds: its snapshot is forgotten, and the next tab
    /// to the right comes forward, else the one to the left, else the library.
    pub(crate) fn discard_tab(&mut self, id: TabId) {
        let Some(index) = self.tabs.index(id) else {
            return;
        };
        let was_front = self.front_tab() == Some(id);
        // Read before the fields are emptied: the session's own path, wherever a
        // rename or move left it.
        let path = self
            .tabs
            .get(id)
            .and_then(|t| self.tab_path(t))
            .map(Path::to_path_buf);
        if self.tabs.active == Some(id) {
            self.finish_text_first();
            let ctx = self.egui_ctx.clone();
            self.cancel_gesture(&ctx);
            self.drop_recovery();
            self.disk_settle();
            self.session = EditorSession::new();
            self.tabs.active = None;
            self.leave_document();
        }
        if self.opening.as_ref().is_some_and(|o| o.tab == id) {
            self.opening = None;
        }
        let tab = self.tabs.list.remove(index);
        if let Body::Parked(p) = &tab.body
            && let Some(key) = &p.recovery_key
        {
            self.forget_snapshot(key);
        }
        if let Some(path) = path {
            self.tabs.closed.push((path, index));
        }
        if self.tabs.shown == Some(id) {
            self.tabs.shown = None;
        }
        if was_front {
            let next = self
                .tabs
                .list
                .get(index)
                .or_else(|| index.checked_sub(1).and_then(|i| self.tabs.list.get(i)))
                .map(|t| t.id);
            match next {
                Some(next) => self.activate_tab(next),
                None => self.show_library(),
            }
        }
    }

    /// Throw away a parked tab's unsaved work as the window closes over it: its
    /// snapshot forgotten and its session counted clean. The active tab's is the
    /// caller's, through `drop_recovery`.
    pub(crate) fn forget_tab_work(&mut self, id: TabId) {
        let Some(Tab {
            body: Body::Parked(p),
            ..
        }) = self.tabs.get_mut(id)
        else {
            return;
        };
        let key = p.recovery_key.take();
        let path = p.session.path.clone();
        p.session
            .mark_saved(path.unwrap_or_else(|| PathBuf::from("Untitled.ondin")));
        if let Some(key) = key {
            self.forget_snapshot(&key);
        }
    }

    /// `Ctrl+Shift+T`: the tab closed last, back in its place in the strip.
    pub(crate) fn reopen_closed_tab(&mut self) {
        while let Some((path, index)) = self.tabs.closed.pop() {
            if let Some(id) = self.tab_of_path(&path) {
                self.activate_tab(id);
                return;
            }
            if !path.exists() {
                continue;
            }
            let id = self
                .tabs
                .insert(index, Body::Unloaded { path, failed: None });
            self.activate_tab(id);
            return;
        }
    }

    // --- next launch -------------------------------------------------------------

    /// Bring back the tabs open at the last quit, each only a path until shown, and
    /// say how many named a file that is gone.
    pub(crate) fn restore_tabs(&mut self) {
        let mut paths = self.prefs.open_tabs.clone();
        // From before tabs, the one document `reopen_last` named.
        if paths.is_empty()
            && self.prefs.reopen_last
            && let Some(last) = self.prefs.last_document.clone()
        {
            self.prefs.front_tab = Some(0);
            paths.push(last);
        }
        let mut gone = 0;
        for path in paths {
            if !path.exists() {
                gone += 1;
                continue;
            }
            if self.tab_of_path(&path).is_none() {
                self.tabs.add(Body::Unloaded { path, failed: None });
            }
        }
        if gone > 0 {
            self.session.info(if gone == 1 {
                "A document open last time is no longer on disk; its tab was closed".to_string()
            } else {
                format!(
                    "{gone} documents open last time are no longer on disk; their tabs were closed"
                )
            });
        }
    }

    /// The tab that was in front at the last quit, if it was a document's.
    pub(crate) fn restored_front(&self) -> Option<TabId> {
        let path = self
            .prefs
            .open_tabs
            .get(self.prefs.front_tab?)
            .cloned()
            .or_else(|| self.prefs.last_document.clone())?;
        self.tab_of_path(&path)
    }

    /// Write the open tabs and the one in front to the preferences when either has
    /// changed, so the next launch brings them back. Once a frame, like the view
    /// switches: every door that changes a tab would otherwise have to.
    pub(crate) fn remember_tabs(&mut self) {
        if self.tabs.private {
            return;
        }
        let front = self.front_tab();
        let mut paths = Vec::with_capacity(self.tabs.list.len());
        let mut front_at = None;
        for tab in &self.tabs.list {
            if let Some(path) = self.tab_path(tab) {
                if front == Some(tab.id) {
                    front_at = Some(paths.len());
                }
                paths.push(path.to_path_buf());
            }
        }
        if paths != self.prefs.open_tabs || front_at != self.prefs.front_tab {
            self.prefs.open_tabs = paths;
            self.prefs.front_tab = front_at;
            self.prefs.save();
        }
    }
}

// --- the strip ---------------------------------------------------------------------

/// The strip's height. On Windows and Linux it is the window's title bar.
pub(crate) const STRIP_H: f32 = 36.0;
/// A tab's height, standing on the strip's foot so the front one joins the top bar.
const TAB_H: f32 = 29.0;
/// How wide a tab grows, and how far the tabs shrink together before the strip
/// stops shrinking them and the caret lists the rest.
const TAB_MAX_W: f32 = 200.0;
const TAB_MIN_W: f32 = 96.0;
/// The library's tab: a mark, not a document.
const HOME_W: f32 = 40.0;
const GAP: f32 = 4.0;
/// Before the library's tab.
const LEAD: f32 = 8.0;
/// The `+` and the caret, small tabs of their own.
const BUTTON: f32 = 30.0;
/// Empty strip kept beside the caption buttons, so the window can always be
/// dragged however many tabs are open.
const DRAG_ROOM: f32 = 56.0;
/// The close box and the dot it shares a place with.
const END_BOX: f32 = 18.0;
const OVERFLOW_W: f32 = 220.0;

/// The strip's ground, a step below the top bar's; a tab behind keeps a ground of
/// its own a step above the strip's, and hover lifts it one more. The front tab
/// takes the top bar's, `color::TOPBAR`.
const STRIP: egui::Color32 = egui::Color32::from_rgb(14, 14, 15);
const BEHIND: egui::Color32 = egui::Color32::from_rgb(20, 20, 21);
const LIFTED: egui::Color32 = egui::Color32::from_rgb(25, 25, 26);

/// What a key on either screen asks of the tabs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TabKey {
    /// `Ctrl+Tab` (1) and `Ctrl+Shift+Tab` (-1).
    Step(isize),
    /// `Alt+1`…`Alt+9`.
    Slot(u8),
    /// `Ctrl+Shift+T`.
    Reopen,
}

/// The tab keys pressed this frame. `Ctrl+1`, `Ctrl+2` and `Ctrl+PageUp/PageDown`
/// stay the zoom and page keys, so jumps go on `Alt` and stepping on `Ctrl+Tab`.
pub(crate) fn read_keys(ctx: &egui::Context) -> Option<TabKey> {
    ctx.input(|i| {
        let m = i.modifiers;
        if m.command && !m.alt && i.key_pressed(egui::Key::Tab) {
            return Some(TabKey::Step(if m.shift { -1 } else { 1 }));
        }
        if m.command && m.shift && !m.alt && i.key_pressed(egui::Key::T) {
            return Some(TabKey::Reopen);
        }
        if m.alt && !m.command && !m.shift {
            let digits = [
                egui::Key::Num1,
                egui::Key::Num2,
                egui::Key::Num3,
                egui::Key::Num4,
                egui::Key::Num5,
                egui::Key::Num6,
                egui::Key::Num7,
                egui::Key::Num8,
                egui::Key::Num9,
            ];
            for (n, key) in (1u8..).zip(digits) {
                if i.key_pressed(key) {
                    return Some(TabKey::Slot(n));
                }
            }
        }
        None
    })
}

/// Where the strip lays its tabs: the width each gets, and how many show.
#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct StripLayout {
    pub width: f32,
    /// How many tabs show; fewer than there are when they overflow.
    pub visible: usize,
    pub overflow: bool,
}

/// Lay `count` tabs into `room` points: the library's tab and the `+` already taken
/// out, the caret's room taken here when they overflow.
pub(crate) fn strip_layout(count: usize, room: f32) -> StripLayout {
    if count == 0 {
        return StripLayout {
            width: TAB_MAX_W,
            visible: 0,
            overflow: false,
        };
    }
    let fits = |n: usize, room: f32| (room - GAP * (n as f32 - 1.0)) / n as f32;
    let width = fits(count, room);
    if width >= TAB_MIN_W {
        return StripLayout {
            width: width.min(TAB_MAX_W),
            visible: count,
            overflow: false,
        };
    }
    let room = room - BUTTON - GAP;
    let visible = (((room + GAP) / (TAB_MIN_W + GAP)).floor().max(0.0) as usize).clamp(1, count);
    StripLayout {
        width: fits(visible, room).clamp(TAB_MIN_W, TAB_MAX_W),
        visible,
        overflow: true,
    }
}

/// The first tab drawn so the front one, at `front`, shows: the strip scrolls only
/// as far as it must.
pub(crate) fn scroll_to_show(
    scroll: usize,
    front: Option<usize>,
    visible: usize,
    count: usize,
) -> usize {
    let most = count.saturating_sub(visible);
    let scroll = scroll.min(most);
    match front {
        Some(f) if f < scroll => f,
        Some(f) if f >= scroll + visible => (f + 1).saturating_sub(visible).min(most),
        _ => scroll,
    }
}

impl OndinApp {
    /// **The strip**: the library's tab, a tab per open document, `+`, the caret
    /// when they overflow, and — on the hosts that draw their own — the window's
    /// caption buttons and the drag strip, since it is the title bar. On both
    /// screens. `act` false draws it inert for a frame, as the chrome's `act`.
    ///
    /// Its widgets sense the pointer and never the keyboard focus, as the system's
    /// title bar is not in the Tab order; the tab keys are `read_keys`'.
    pub(crate) fn tab_strip(&mut self, ui: &mut egui::Ui, act: bool) {
        // No separator: egui's default line across the foot would cut the front tab
        // off from the top bar it joins.
        egui::Panel::top("tab-strip")
            .exact_size(STRIP_H)
            .resizable(false)
            .show_separator_line(false)
            .frame(egui::Frame::NONE.fill(STRIP))
            .show(ui, |ui| self.strip_body(ui, act));
    }

    fn strip_body(&mut self, ui: &mut egui::Ui, act: bool) {
        let chrome = crate::chrome::Chrome::current();
        let strip = ui.max_rect();
        // Laid first, under everything that answers a press.
        crate::chrome::drag_strip(ui, strip, act);
        ui.scope_builder(
            egui::UiBuilder::new()
                .max_rect(strip)
                .layout(egui::Layout::right_to_left(egui::Align::Center)),
            |ui| crate::chrome::caption_buttons(ui, chrome, act),
        );
        let controls = if chrome.draws_own_controls() {
            3.0 * crate::chrome::CONTROL_W
        } else {
            0.0
        };

        let infos = self.tab_infos();
        let front = self.front_tab();
        let foot = strip.bottom();
        let mut x = strip.left() + chrome.leading_inset() + LEAD;
        let home =
            egui::Rect::from_min_size(egui::pos2(x, foot - TAB_H), egui::vec2(HOME_W, TAB_H));
        self.home_tab(ui, home, act);
        x += HOME_W + GAP;

        let room = strip.right() - controls - DRAG_ROOM - x - (BUTTON + GAP);
        let layout = strip_layout(infos.len(), room);
        let front_at = front.and_then(|id| infos.iter().position(|t| t.id == id));
        self.tabs.scroll = scroll_to_show(self.tabs.scroll, front_at, layout.visible, infos.len());
        let first = self.tabs.scroll;
        let run_left = x;
        for info in infos.iter().skip(first).take(layout.visible) {
            let rect = egui::Rect::from_min_size(
                egui::pos2(x, foot - TAB_H),
                egui::vec2(layout.width, TAB_H),
            );
            self.doc_tab(ui, rect, info, front == Some(info.id), act);
            x += layout.width + GAP;
        }
        // A tab dragged along the strip takes the slot under the pointer.
        if act && let Some((id, px)) = self.dragged_tab(ui) {
            let slot = ((px - run_left) / (layout.width + GAP)).floor().max(0.0) as usize;
            let to = (first + slot).min(infos.len().saturating_sub(1));
            if self.tabs.index(id) != Some(to) {
                self.tabs.move_to(id, to);
            }
        }

        let plus =
            egui::Rect::from_min_size(egui::pos2(x, foot - TAB_H), egui::vec2(BUTTON, TAB_H));
        if strip_button(ui, plus, crate::theme::icon::PLUS, false, "tab-new")
            .on_hover_text("New file (Ctrl+N)")
            .clicked()
            && act
        {
            self.new_tab_document();
        }
        if layout.overflow {
            let caret = plus.translate(egui::vec2(BUTTON + GAP, 0.0));
            let resp = strip_button(
                ui,
                caret,
                crate::theme::icon::CARET_DOWN,
                self.tabs.overflow_open,
                "tab-overflow",
            )
            .on_hover_text("Every open document");
            if resp.clicked() && act {
                self.tabs.overflow_open = !self.tabs.overflow_open;
            }
            if self.tabs.overflow_open {
                self.overflow_list(ui.ctx(), &resp, &infos, front);
            }
        } else {
            self.tabs.overflow_open = false;
        }
    }

    /// The library's tab: the brand mark on a tab's ground, so the mark is seen
    /// centred in its tab rather than against the taller strip.
    fn home_tab(&mut self, ui: &mut egui::Ui, rect: egui::Rect, act: bool) {
        let resp = ui.interact(rect, egui::Id::new("tab-library"), egui::Sense::CLICK);
        let ground = if self.view == View::Dashboard {
            crate::theme::color::TOPBAR
        } else if resp.hovered() {
            LIFTED
        } else {
            BEHIND
        };
        ui.painter().rect_filled(rect, tab_corners(), ground);
        let mark = egui::Rect::from_center_size(rect.center(), egui::vec2(18.0, 18.0));
        crate::ui::logo(ui, mark, false);
        if resp.on_hover_text("Library (Ctrl+O)").clicked() && act {
            self.show_library();
        }
    }

    /// One document's tab: the kit glyph or the warning, the name, and at its end
    /// the × on the front tab and the hovered one, else the unsaved dot.
    fn doc_tab(
        &mut self,
        ui: &mut egui::Ui,
        rect: egui::Rect,
        info: &TabInfo,
        front: bool,
        act: bool,
    ) {
        use crate::theme::{color, icon};
        let id = tab_widget_id(info.id);
        let resp = ui.interact(rect, id, egui::Sense::CLICK.union(egui::Sense::DRAG));
        let close = egui::Rect::from_center_size(
            egui::pos2(rect.right() - 6.0 - END_BOX / 2.0, rect.center().y),
            egui::vec2(END_BOX, END_BOX),
        );
        let close_resp = ui.interact(close, id.with("close"), egui::Sense::CLICK);
        let hovered = resp.hovered() || close_resp.hovered();
        let ground = if front {
            color::TOPBAR
        } else if hovered {
            LIFTED
        } else {
            BEHIND
        };
        let p = ui.painter().clone();
        p.rect_filled(rect, tab_corners(), ground);
        if info.failed.is_some() {
            p.rect_stroke(
                rect,
                tab_corners(),
                egui::Stroke::new(1.0, color::DANGER.gamma_multiply(0.35)),
                egui::StrokeKind::Inside,
            );
        }
        let ink = if info.failed.is_some() {
            color::DANGER_200
        } else if front || hovered {
            color::TEXT
        } else {
            crate::theme::text::MUTED
        };

        let mut left = rect.left() + 10.0;
        let glyph = if info.failed.is_some() {
            Some((icon::WARNING, color::DANGER))
        } else if info.kit {
            Some((icon::PACKAGE, color::ACCENT_300))
        } else {
            None
        };
        if let Some((glyph, tint)) = glyph {
            p.text(
                egui::pos2(left, rect.center().y),
                egui::Align2::LEFT_CENTER,
                glyph,
                crate::theme::icon_font(13.0),
                tint,
            );
            left += 13.0 + 7.0;
        }
        let alpha = match &self.opening {
            Some(o) if info.loading => crate::ui::pulse_alpha(o.at.elapsed().as_secs_f32()),
            _ => 1.0,
        };
        let room = (close.left() - 7.0 - left).max(0.0);
        let shown = ink.gamma_multiply(alpha);
        let galley = ui.ctx().fonts_mut(|f| {
            let mut job = egui::text::LayoutJob::simple_singleline(
                info.name.clone(),
                egui::FontId::proportional(12.0),
                shown,
            );
            job.wrap = egui::text::TextWrapping::truncate_at_width(room);
            f.layout_job(job)
        });
        p.galley(
            egui::pos2(left, rect.center().y - galley.size().y / 2.0),
            galley,
            shown,
        );

        let show_x = front || hovered;
        if show_x {
            if close_resp.hovered() {
                p.rect_filled(close, egui::CornerRadius::same(4), color::text_a(26));
            }
            p.text(
                close.center(),
                egui::Align2::CENTER_CENTER,
                icon::X,
                crate::theme::icon_font(12.0),
                crate::theme::text::MUTED,
            );
        } else if info.dirty {
            p.circle_filled(close.center(), 3.5, color::TEXT);
        }

        if !act {
            return;
        }
        if show_x && close_resp.clicked() {
            self.close_tab(info.id);
            return;
        }
        if resp.clicked_by(egui::PointerButton::Middle) {
            self.close_tab(info.id);
            return;
        }
        let resp = resp.on_hover_text(info.full_path());
        if (resp.clicked() || resp.drag_started()) && !front {
            self.activate_tab(info.id);
        }
    }

    /// The tab being dragged along the strip, and the pointer's x.
    fn dragged_tab(&self, ui: &egui::Ui) -> Option<(TabId, f32)> {
        let dragged = ui.ctx().dragged_id()?;
        let id = self
            .tabs
            .list
            .iter()
            .find(|t| tab_widget_id(t.id) == dragged)?
            .id;
        let x = ui.ctx().pointer_interact_pos()?.x;
        Some((id, x))
    }

    /// `+`: a new document in the project in front of you, as `Ctrl+N` makes one,
    /// in a new tab.
    fn new_tab_document(&mut self) {
        let project = match self.view {
            View::Editor => self.open_document_project(),
            View::Dashboard => self.nav_project(),
        };
        self.new_library_document(project);
    }

    /// The caret's list: every open document, the front one ticked, the unsaved
    /// ones dotted. Choosing one brings it forward, the strip scrolling to it.
    fn overflow_list(
        &mut self,
        ctx: &egui::Context,
        head: &egui::Response,
        infos: &[TabInfo],
        front: Option<TabId>,
    ) {
        use crate::theme::{color, icon};
        const PAD: f32 = 5.0;
        let anchor = egui::pos2(
            head.rect.right() - OVERFLOW_W,
            head.rect.bottom() + crate::ui::MENU_GAP,
        );
        let mut chosen = None;
        let menu = egui::Area::new(egui::Id::new("tab-overflow-list"))
            .order(egui::Order::Foreground)
            .fixed_pos(anchor)
            .show(ctx, |ui| {
                crate::ui::menu_frame(PAD).show(ui, |ui| {
                    ui.set_width(crate::ui::menu_inner_w(OVERFLOW_W, PAD));
                    ui.spacing_mut().item_spacing.y = 1.0;
                    for info in infos {
                        let (rect, resp) = ui.allocate_exact_size(
                            egui::vec2(ui.available_width(), crate::ui::MENU_ITEM_H),
                            egui::Sense::click(),
                        );
                        let on = front == Some(info.id);
                        let ink = if on || resp.hovered() {
                            color::TEXT
                        } else {
                            crate::theme::text::MUTED
                        };
                        let galley = ui.ctx().fonts_mut(|f| {
                            let mut job = egui::text::LayoutJob::simple_singleline(
                                info.name.clone(),
                                egui::FontId::proportional(11.5),
                                ink,
                            );
                            job.wrap =
                                egui::text::TextWrapping::truncate_at_width(rect.width() - 50.0);
                            f.layout_job(job)
                        });
                        let p = ui.painter();
                        if resp.hovered() {
                            p.rect_filled(rect, egui::CornerRadius::same(4), color::text_a(20));
                        }
                        if on {
                            p.text(
                                egui::pos2(rect.left() + 14.5, rect.center().y),
                                egui::Align2::CENTER_CENTER,
                                icon::CHECK,
                                crate::theme::icon_font(13.0),
                                color::ACCENT,
                            );
                        }
                        p.galley(
                            egui::pos2(rect.left() + 28.0, rect.center().y - galley.size().y / 2.0),
                            galley,
                            ink,
                        );
                        if info.dirty {
                            p.circle_filled(
                                egui::pos2(rect.right() - 11.0, rect.center().y),
                                3.0,
                                color::TEXT,
                            );
                        }
                        if resp.clicked() {
                            chosen = Some(info.id);
                        }
                    }
                });
            })
            .response;
        if let Some(id) = chosen {
            self.tabs.overflow_open = false;
            self.activate_tab(id);
            return;
        }
        let elsewhere = ctx.input(|i| i.pointer.any_click())
            && !menu.contains_pointer()
            && !head.contains_pointer();
        if elsewhere || ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            self.tabs.overflow_open = false;
        }
    }

    /// Act on a tab key read by [`read_keys`].
    pub(crate) fn tab_key(&mut self, key: TabKey) {
        match key {
            TabKey::Step(step) => self.step_tab(step),
            TabKey::Slot(n) => self.jump_to_slot(n),
            TabKey::Reopen => self.reopen_closed_tab(),
        }
    }

    /// The editor's place while the tab in front is being read, or could not be:
    /// its name, and for a failed read why, with *Try again* and *Close tab*.
    pub(crate) fn tab_placeholder(&mut self, ui: &mut egui::Ui) {
        use crate::theme::{color, text};
        let Some(id) = self.tabs.shown else {
            return;
        };
        let Some(info) = self.tabs.get(id).map(|t| self.tab_info(t)) else {
            return;
        };
        egui::Panel::top("tab-placeholder-bar")
            .exact_size(crate::app::TOP_BAR_H)
            .resizable(false)
            .frame(
                egui::Frame::NONE
                    .fill(color::TOPBAR)
                    .inner_margin(egui::Margin::symmetric(14, 0)),
            )
            .show(ui, |ui| {
                ui.horizontal_centered(|ui| {
                    if let Some(project) = &info.project {
                        ui.label(egui::RichText::new(project).size(13.0).color(text::MUTED));
                        ui.label(egui::RichText::new("/").size(13.0).color(text::FAINT));
                    }
                    ui.label(
                        egui::RichText::new(&info.name)
                            .size(13.0)
                            .color(color::TEXT),
                    );
                });
            });
        let mut retry = false;
        let mut close = false;
        egui::CentralPanel::default()
            .frame(egui::Frame::NONE.fill(color::BG))
            .show(ui, |ui| {
                ui.vertical_centered(|ui| {
                    ui.add_space((ui.available_height() / 2.0 - 40.0).max(0.0));
                    let Some(why) = &info.failed else {
                        ui.label(
                            egui::RichText::new(format!("Loading {}…", info.name))
                                .size(11.5)
                                .color(text::DIM),
                        );
                        return;
                    };
                    ui.label(
                        egui::RichText::new(format!("“{}” could not be read", info.name))
                            .size(13.0)
                            .color(color::TEXT),
                    );
                    ui.add_space(4.0);
                    ui.label(egui::RichText::new(why).size(11.5).color(text::DIM));
                    ui.add_space(14.0);
                    ui.horizontal(|ui| {
                        let w = 2.0 * 96.0 + 8.0;
                        ui.add_space((ui.available_width() - w).max(0.0) / 2.0);
                        ui.spacing_mut().item_spacing.x = 8.0;
                        retry = crate::settings::text_button(
                            ui,
                            "Try again",
                            crate::ui::FieldButton::On,
                        )
                        .clicked();
                        close = crate::settings::text_button(
                            ui,
                            "Close tab",
                            crate::ui::FieldButton::Off,
                        )
                        .clicked();
                    });
                });
            });
        let (close_key, library_key) = ui.ctx().input(|i| {
            let cmd = i.modifiers.command && !i.modifiers.alt && !i.modifiers.shift;
            (
                cmd && i.key_pressed(egui::Key::W),
                cmd && i.key_pressed(egui::Key::O),
            )
        });
        if let Some(key) = read_keys(ui.ctx()) {
            self.tab_key(key);
        } else if retry {
            self.retry_tab(id);
        } else if close || close_key {
            self.close_tab(id);
        } else if library_key {
            self.show_library();
        }
    }
}

/// A document tab's widget id, which a drag is recognised by.
fn tab_widget_id(id: TabId) -> egui::Id {
    egui::Id::new(("doc-tab", id))
}

/// A tab's corners: round on top, square where it stands on the bar below.
fn tab_corners() -> egui::CornerRadius {
    egui::CornerRadius {
        nw: 7,
        ne: 7,
        sw: 0,
        se: 0,
    }
}

/// The strip's `+` and caret: a glyph on a small tab of its own, lifted on hover
/// and while what it opens is open.
fn strip_button(
    ui: &mut egui::Ui,
    rect: egui::Rect,
    glyph: &str,
    open: bool,
    id: &str,
) -> egui::Response {
    use crate::theme::color;
    let resp = ui.interact(rect, egui::Id::new(id), egui::Sense::CLICK);
    let lit = open || resp.hovered();
    ui.painter()
        .rect_filled(rect, tab_corners(), if lit { LIFTED } else { BEHIND });
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        glyph,
        crate::theme::icon_font(14.0),
        if lit {
            color::TEXT
        } else {
            crate::theme::text::DIM
        },
    );
    resp
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::{NodeKind, Operation, Transaction};

    fn temp_root(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("ondin-tabs-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    /// A headless app over an empty library at `root`, its starter document in the
    /// one tab it starts with.
    fn app_at(ctx: &egui::Context, root: &Path) -> OndinApp {
        let mut app = OndinApp::headless(ctx);
        app.prefs.base_folder = Some(root.to_path_buf());
        app.library = crate::library::state::Library::open(root.to_path_buf());
        app
    }

    /// A document saved at `root/<name>.ondin`.
    fn file(root: &Path, name: &str) -> PathBuf {
        let mut ids = ondin_core::IdSource::new(7);
        let doc = ondin_core::Document::new(ids.mint());
        let path = root.join(format!("{name}.ondin"));
        crate::library::store::write_document(&path, &doc).unwrap();
        path
    }

    /// One undoable edit to the document in front: a rectangle on its page.
    fn edit(app: &mut OndinApp) {
        let s = &mut app.session;
        let id = s.ids.mint();
        let parent = s.page;
        let index = s.doc.get(parent).map_or(0, |n| n.children().len());
        assert!(s.commit(Transaction(vec![Operation::CreateNode {
            id,
            parent,
            index,
            kind: NodeKind::Rect {
                size: ondin_core::kurbo::Size::new(10.0, 10.0),
                corner_radii: ondin_core::kurbo::RoundedRectRadii::default(),
                corner_smoothing: 0.0,
            },
            transform: None,
            name: None,
        }])));
    }

    fn front_path(app: &OndinApp) -> Option<PathBuf> {
        let id = app.front_tab()?;
        let tab = app.tabs.get(id)?;
        app.tab_path(tab).map(Path::to_path_buf)
    }

    /// Switching back to a tab finds its session where it was left — camera, undo
    /// history — rather than the file read again.
    /// flip: `unpark` dropping the parked session → fails at the path, a's no longer.
    #[test]
    fn a_tab_brought_back_keeps_its_camera_and_history() {
        let ctx = egui::Context::default();
        let root = temp_root("keeps");
        let mut app = app_at(&ctx, &root);
        let (a, b) = (file(&root, "a"), file(&root, "b"));
        app.open_path(&a);
        edit(&mut app);
        app.session.camera.zoom = 2.5;
        app.open_path(&b);
        assert_eq!(
            app.session.path.as_deref(),
            Some(b.as_path()),
            "fixture: b in front"
        );
        assert_eq!(
            app.tabs.list.len(),
            3,
            "the starter, a and b each have a tab"
        );

        let a_tab = app.tab_of_path(&a).expect("a keeps its tab");
        app.activate_tab(a_tab);
        assert_eq!(app.session.path.as_deref(), Some(a.as_path()));
        assert_eq!(app.session.camera.zoom, 2.5, "its camera");
        assert!(app.session.history.can_undo(), "its history");
        assert!(
            matches!(
                app.tabs.get(app.tab_of_path(&b).unwrap()).unwrap().body,
                Body::Parked(_)
            ),
            "b is parked behind it"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Opening a document that already has a tab brings that tab forward; the
    /// document is never open twice.
    /// flip: `open_path` skipping its `tab_of_path` lookup → fails at "the same
    /// session, not a reload"; `tab_for_adopting` still finds the tab, so the count
    /// holds and the parked state is read over.
    #[test]
    fn a_document_already_open_is_brought_forward_not_opened_again() {
        let ctx = egui::Context::default();
        let root = temp_root("twice");
        let mut app = app_at(&ctx, &root);
        let (a, b) = (file(&root, "a"), file(&root, "b"));
        app.open_path(&a);
        app.session.camera.zoom = 3.0;
        app.open_path(&b);
        app.open_path(&a);
        assert_eq!(app.tabs.list.len(), 3, "no second tab for a");
        assert_eq!(front_path(&app).as_deref(), Some(a.as_path()));
        assert_eq!(
            app.session.camera.zoom, 3.0,
            "the same session, not a reload"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Leaving a tab saves its document, so a tab behind is clean and its file
    /// holds the edit.
    /// flip: `park_active` without its `save_file` → fails at "clean behind".
    #[test]
    fn leaving_a_tab_saves_it() {
        let ctx = egui::Context::default();
        let root = temp_root("leave-saves");
        let mut app = app_at(&ctx, &root);
        let (a, b) = (file(&root, "a"), file(&root, "b"));
        app.open_path(&a);
        edit(&mut app);
        assert!(app.session.is_dirty(), "fixture: an unsaved edit");
        app.open_path(&b);
        assert!(app.dirty_tabs().is_empty(), "clean behind");
        let on_disk = ondin_core::io::load(&std::fs::read(&a).unwrap()).unwrap();
        assert_eq!(
            on_disk.get(on_disk.first_page()).unwrap().children().len(),
            1,
            "the rectangle reached the file"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Closing the tab in front brings the next one to the right forward, else the
    /// one to the left, else the library.
    /// flip: `discard_tab` trying the left neighbour first → fails at "the right one".
    #[test]
    fn closing_the_front_tab_brings_its_neighbour_forward() {
        let ctx = egui::Context::default();
        let root = temp_root("close-order");
        let mut app = app_at(&ctx, &root);
        let (a, b, c) = (file(&root, "a"), file(&root, "b"), file(&root, "c"));
        let starter = app.tabs.active.unwrap();
        app.open_path(&a);
        app.open_path(&b);
        app.open_path(&c);
        app.activate_tab(app.tab_of_path(&b).unwrap());

        app.close_tab(app.front_tab().unwrap());
        assert_eq!(
            front_path(&app).as_deref(),
            Some(c.as_path()),
            "the right one"
        );
        app.close_tab(app.front_tab().unwrap());
        assert_eq!(
            front_path(&app).as_deref(),
            Some(a.as_path()),
            "then the left"
        );
        app.close_tab(app.front_tab().unwrap());
        assert_eq!(app.front_tab(), Some(starter));
        app.close_tab(starter);
        assert_eq!(app.view, View::Dashboard, "then the library");
        assert!(app.tabs.list.is_empty());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A tab whose work cannot be saved asks before it closes; one in the library
    /// closes without asking.
    /// flip: `close_tab` discarding without the `dirty_tabs` check → fails at "asks".
    #[test]
    fn only_work_that_cannot_be_saved_is_asked_about() {
        let ctx = egui::Context::default();
        let root = temp_root("close-asks");
        let mut app = app_at(&ctx, &root);
        let starter = app.tabs.active.unwrap();
        edit(&mut app);
        app.close_tab(starter);
        assert_eq!(app.closing_tab, Some(starter), "asks");
        assert!(app.tabs.get(starter).is_some(), "and waits");

        app.closing_tab = None;
        let a = file(&root, "a");
        app.open_path(&a);
        edit(&mut app);
        let a_tab = app.front_tab().unwrap();
        app.close_tab(a_tab);
        assert_eq!(app.closing_tab, None, "saved instead of asked");
        assert!(app.tabs.get(a_tab).is_none());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The window's close counts a tab behind with unsaved work, not only the one in
    /// front.
    /// flip: `dirty_tabs` reading only the active tab → fails at the list.
    #[test]
    fn unsaved_work_behind_is_counted() {
        let ctx = egui::Context::default();
        let root = temp_root("dirty-behind");
        let mut app = app_at(&ctx, &root);
        let starter = app.tabs.active.unwrap();
        edit(&mut app);
        app.open_path(&file(&root, "a"));
        assert!(!app.session.is_dirty(), "fixture: the front tab is clean");
        assert_eq!(app.dirty_tabs(), vec![starter]);
        assert!(app.any_unsaved());
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The tabs open at quit come back in their order with the same one in front; a
    /// file gone from disk drops its tab and says so.
    /// flip: `restore_tabs` without its `exists` check → fails at "two tabs".
    #[test]
    fn tabs_come_back_at_the_next_launch() {
        let ctx = egui::Context::default();
        let root = temp_root("restore");
        let mut app = app_at(&ctx, &root);
        let (a, b, c) = (file(&root, "a"), file(&root, "b"), file(&root, "c"));
        let starter = app.tabs.active.unwrap();
        app.discard_tab(starter);
        for path in [&a, &b, &c] {
            app.open_path(path);
        }
        app.activate_tab(app.tab_of_path(&b).unwrap());
        app.remember_tabs();
        assert_eq!(app.prefs.open_tabs, vec![a.clone(), b.clone(), c.clone()]);
        assert_eq!(app.prefs.front_tab, Some(1));

        std::fs::remove_file(&a).unwrap();
        let mut next = app_at(&ctx, &root);
        next.tabs = Tabs::default();
        next.prefs.open_tabs = app.prefs.open_tabs.clone();
        next.prefs.front_tab = app.prefs.front_tab;
        next.restore_tabs();
        assert_eq!(next.tabs.list.len(), 2, "two tabs");
        assert!(
            next.tabs
                .list
                .iter()
                .all(|t| matches!(t.body, Body::Unloaded { .. })),
            "each only a path until shown"
        );
        assert_eq!(next.restored_front(), next.tab_of_path(&b));
        assert!(
            next.session.status().text.contains("no longer on disk"),
            "and the gone one is reported: {:?}",
            next.session.status().text
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `Ctrl+Tab` steps through the strip and wraps through the library; `Alt+n`
    /// counts the library as 1 and `Alt+9` is the last tab.
    /// flip: `step_tab` not counting the library's slot → fails at "the library".
    #[test]
    fn the_tab_keys_walk_the_strip_with_the_library_first() {
        let ctx = egui::Context::default();
        let root = temp_root("keys");
        let mut app = app_at(&ctx, &root);
        let starter = app.tabs.active.unwrap();
        let (a, b) = (file(&root, "a"), file(&root, "b"));
        app.open_path(&a);
        app.open_path(&b);
        app.activate_tab(app.tab_of_path(&a).unwrap());

        app.step_tab(1);
        assert_eq!(front_path(&app).as_deref(), Some(b.as_path()));
        app.step_tab(1);
        assert_eq!(app.view, View::Dashboard, "the library");
        app.step_tab(1);
        assert_eq!(app.front_tab(), Some(starter), "wrapping round");
        app.step_tab(-1);
        assert_eq!(app.view, View::Dashboard);

        app.jump_to_slot(9);
        assert_eq!(
            front_path(&app).as_deref(),
            Some(b.as_path()),
            "Alt+9 is the last"
        );
        app.jump_to_slot(1);
        assert_eq!(app.view, View::Dashboard, "Alt+1 is the library");
        app.jump_to_slot(3);
        assert_eq!(front_path(&app).as_deref(), Some(a.as_path()));
        app.jump_to_slot(6);
        assert_eq!(
            front_path(&app).as_deref(),
            Some(a.as_path()),
            "past the end: nothing"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    /// `Ctrl+Shift+T` brings the tab closed last back in its place.
    /// flip: `reopen_closed_tab` adding at the end → fails at "in its place".
    #[test]
    fn a_closed_tab_comes_back_in_its_place() {
        let ctx = egui::Context::default();
        let root = temp_root("reopen");
        let mut app = app_at(&ctx, &root);
        let (a, b) = (file(&root, "a"), file(&root, "b"));
        app.open_path(&a);
        app.open_path(&b);
        let a_tab = app.tab_of_path(&a).unwrap();
        app.close_tab(a_tab);
        assert!(app.tab_of_path(&a).is_none(), "fixture: closed");

        app.reopen_closed_tab();
        app.settle_open();
        let back = app.tab_of_path(&a).expect("a is open again");
        assert_eq!(app.tabs.index(back), Some(1), "in its place");
        assert_eq!(app.tabs.active, Some(back), "and in front, loaded");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The palette's tab rows reach the tabs through `dispatch`, as their chords do.
    /// flip: `Item::PreviousTab` mapping to `Step(1)` → fails at "back to a".
    #[test]
    fn the_palettes_tab_rows_step_and_reopen() {
        use crate::menu::Item;
        let ctx = egui::Context::default();
        let root = temp_root("palette-rows");
        let mut app = app_at(&ctx, &root);
        let (a, b) = (file(&root, "a"), file(&root, "b"));
        app.open_path(&a);
        app.open_path(&b);
        app.activate_tab(app.tab_of_path(&a).unwrap());
        let run = |app: &mut OndinApp, item: Item| {
            app.dispatch(&ctx, item.action().expect("an app verb"));
            app.settle_open();
        };

        run(&mut app, Item::NextTab);
        assert_eq!(front_path(&app).as_deref(), Some(b.as_path()), "on to b");
        run(&mut app, Item::PreviousTab);
        assert_eq!(front_path(&app).as_deref(), Some(a.as_path()), "back to a");
        app.close_tab(app.tab_of_path(&b).unwrap());
        run(&mut app, Item::ReopenTab);
        assert!(app.tab_of_path(&b).is_some(), "b is open again");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A document opened from the library gets its tab at once, in front and
    /// pulsing, and lands in it; one that cannot be read takes the tab away again.
    /// flip: `land_open` keeping a failed fresh tab → fails at "the tab goes".
    #[test]
    fn an_open_from_the_library_lands_in_a_tab_made_for_it() {
        let ctx = egui::Context::default();
        let root = temp_root("begin-open");
        let mut app = app_at(&ctx, &root);
        app.view = View::Dashboard;
        let a = file(&root, "a");
        app.begin_open(&a);
        let tab = app.tab_of_path(&a).expect("a tab at once");
        assert_eq!(app.front_tab(), Some(tab), "in front");
        assert!(
            app.tab_infos().iter().any(|t| t.id == tab && t.loading),
            "pulsing"
        );
        app.settle_open();
        assert_eq!(app.tabs.active, Some(tab));
        assert_eq!(app.session.path.as_deref(), Some(a.as_path()));

        app.show_library();
        let broken = root.join("broken.ondin");
        std::fs::write(&broken, b"not a document").unwrap();
        let before = app.tabs.list.len();
        app.begin_open(&broken);
        app.settle_open();
        assert_eq!(app.tabs.list.len(), before, "the tab goes");
        assert_eq!(app.view, View::Dashboard, "back where the click came from");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// A library file trashed while open leaves its tab with nowhere to save to; a
    /// tab that only named it closes.
    /// flip: `open_file_trashed` skipping parked sessions → fails at "behind".
    #[test]
    fn a_trashed_file_unfiles_its_tab() {
        let ctx = egui::Context::default();
        let root = temp_root("trashed");
        let mut app = app_at(&ctx, &root);
        let (a, b, c) = (file(&root, "a"), file(&root, "b"), file(&root, "c"));
        app.open_path(&a);
        app.open_path(&b);
        let c_tab = app.tabs.add(Body::Unloaded {
            path: c.clone(),
            failed: None,
        });
        app.open_file_trashed(&a);
        app.open_file_trashed(&c);
        assert!(app.tab_of_path(&a).is_none(), "behind");
        assert!(app.tabs.get(c_tab).is_none(), "the unread one closes");
        assert_eq!(app.tabs.list.len(), 3, "a's tab stays, unfiled");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// The tabs share the room down to their least width; past it the strip shows
    /// what fits and the caret lists the rest.
    #[test]
    fn the_strip_shrinks_tabs_then_overflows() {
        let wide = strip_layout(3, 1000.0);
        assert_eq!(
            (wide.width, wide.visible, wide.overflow),
            (TAB_MAX_W, 3, false)
        );
        let shared = strip_layout(5, 500.0);
        assert!(!shared.overflow && shared.width < TAB_MAX_W && shared.width >= TAB_MIN_W);
        let many = strip_layout(9, 500.0);
        assert!(many.overflow && many.visible < 9 && many.width >= TAB_MIN_W);
        let shown = many.visible as f32 * many.width + (many.visible as f32 - 1.0) * GAP;
        assert!(
            shown + BUTTON + GAP <= 500.0 + 0.01,
            "the caret fits beside them"
        );

        assert_eq!(
            scroll_to_show(0, Some(8), 4, 9),
            5,
            "scrolled just far enough"
        );
        assert_eq!(scroll_to_show(5, Some(2), 4, 9), 2);
        assert_eq!(scroll_to_show(3, Some(4), 4, 9), 3, "already showing");
    }

    /// One whole app frame in a 1320 × 820 window, `events` delivered.
    fn frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1320.0, 820.0),
            )),
            events,
            ..Default::default()
        };
        let _ = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
    }

    /// A click with `button` at `at`: the pointer moved there first, then pressed
    /// and released on frames of their own, as egui reads interaction a frame late.
    fn click(ctx: &egui::Context, app: &mut OndinApp, at: egui::Pos2, button: egui::PointerButton) {
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button,
            pressed,
            modifiers: Default::default(),
        };
        frame(ctx, app, vec![egui::Event::PointerMoved(at)]);
        frame(ctx, app, vec![press(true)]);
        frame(ctx, app, vec![press(false)]);
        frame(ctx, app, vec![]);
    }

    /// The centre of the `i`th document tab, and of its close box, in the
    /// 1320-wide window, where the tabs have their full width.
    fn tab_points(i: usize) -> (egui::Pos2, egui::Pos2) {
        let left = LEAD + HOME_W + GAP + i as f32 * (TAB_MAX_W + GAP);
        let y = STRIP_H - TAB_H / 2.0;
        (
            egui::pos2(left + TAB_MAX_W / 2.0, y),
            egui::pos2(left + TAB_MAX_W - 6.0 - END_BOX / 2.0, y),
        )
    }

    /// The strip drives the tabs: a click brings a tab forward, the front tab's ×
    /// closes it, a middle-click closes a tab behind, and the library's tab shows
    /// the library.
    /// flip: `doc_tab`'s close-box arm removed → fails at "the front tab's × closes it".
    #[test]
    fn the_strip_brings_forward_and_closes_tabs() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("strip-clicks");
        let mut app = app_at(&ctx, &root);
        let (a, b) = (file(&root, "a"), file(&root, "b"));
        app.open_path(&a);
        app.open_path(&b);
        for _ in 0..3 {
            frame(&ctx, &mut app, vec![]);
        }
        let primary = egui::PointerButton::Primary;
        assert_eq!(
            front_path(&app).as_deref(),
            Some(b.as_path()),
            "fixture: b in front"
        );

        let (a_body, a_close) = tab_points(1);
        click(&ctx, &mut app, a_body, primary);
        assert_eq!(
            front_path(&app).as_deref(),
            Some(a.as_path()),
            "a tab behind comes forward"
        );

        click(&ctx, &mut app, a_close, primary);
        assert!(app.tab_of_path(&a).is_none(), "the front tab's × closes it");
        assert_eq!(front_path(&app).as_deref(), Some(b.as_path()));

        let (starter, _) = tab_points(0);
        click(&ctx, &mut app, starter, egui::PointerButton::Middle);
        assert_eq!(app.tabs.list.len(), 1, "a middle-click closes a tab behind");

        click(
            &ctx,
            &mut app,
            egui::pos2(LEAD + HOME_W / 2.0, STRIP_H - TAB_H / 2.0),
            primary,
        );
        assert_eq!(app.view, View::Dashboard, "the library's tab");
        let _ = std::fs::remove_dir_all(&root);
    }

    /// Nothing is drawn across the strip's foot, so the front tab runs into the top
    /// bar without a seam.
    /// flip: `tab_strip` keeping egui's separator line → fails at "no seam".
    #[test]
    fn the_front_tab_meets_the_top_bar_without_a_seam() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1320.0, 820.0),
            )),
            ..Default::default()
        };
        let mut out = None;
        for _ in 0..3 {
            out = Some(ctx.run_ui(input.clone(), |ui| {
                eframe::App::ui(&mut app, ui, &mut frame)
            }));
        }
        assert!(app.front_tab().is_some(), "fixture: a tab in front");
        let seams: Vec<_> = out
            .unwrap()
            .shapes
            .iter()
            .filter_map(|cs| match &cs.shape {
                // egui still emits a hidden separator, with `Stroke::NONE`.
                egui::Shape::LineSegment { points, stroke }
                    if !stroke.is_empty()
                        && (points[0].y - STRIP_H).abs() <= 1.0
                        && (points[1].y - STRIP_H).abs() <= 1.0
                        && (points[1].x - points[0].x).abs() > 200.0 =>
                {
                    Some(*points)
                }
                _ => None,
            })
            .collect();
        assert!(seams.is_empty(), "no seam: {seams:?}");
    }
}
