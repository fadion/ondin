//! The installed font families `serve` lists and draws with. The window has its
//! font service for this; `serve` reads the system's fonts once, on first need,
//! and registers a family's faces with the shaping engine when a document names it.

use ondin_core::Document;
use std::collections::HashSet;
use std::path::PathBuf;

pub(crate) struct SystemFonts {
    db: fontdb::Database,
    families: Vec<String>,
}

impl SystemFonts {
    /// Read every installed face. Slow (a directory walk and a header read per
    /// file), so `serve` calls it once.
    pub(crate) fn load() -> Self {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let families = crate::fonts::listed_families(&db).into_iter().collect();
        Self { db, families }
    }

    /// Every installed family, sorted.
    pub(crate) fn families(&self) -> &[String] {
        &self.families
    }

    /// Register the installed faces of each family `doc` names that the engine
    /// on this thread lacks; whether any registered, so the caller lays text out again.
    pub(crate) fn register_for(&self, doc: &Document) -> bool {
        let mut read: HashSet<PathBuf> = HashSet::new();
        let mut any = false;
        for (family, _) in ondin_core::families::families_in_use(doc) {
            if ondin_core::text::is_family_available(&family) {
                continue;
            }
            for face in self.db.faces() {
                if !face
                    .families
                    .iter()
                    .any(|(n, _)| n.eq_ignore_ascii_case(&family))
                {
                    continue;
                }
                let path = match &face.source {
                    fontdb::Source::File(p) | fontdb::Source::SharedFile(p, _) => p.clone(),
                    // `load_system_fonts` reads files only.
                    fontdb::Source::Binary(_) => continue,
                };
                // A collection file holds several faces; it registers whole once.
                if !read.insert(path.clone()) {
                    continue;
                }
                if let Ok(bytes) = std::fs::read(&path) {
                    any |= !ondin_core::text::register_fonts(bytes).is_empty();
                }
            }
        }
        any
    }
}
