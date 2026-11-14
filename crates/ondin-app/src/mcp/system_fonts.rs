//! The installed font families `serve` lists and draws with. The window has its
//! font service for this; `serve` reads the system's fonts once, on first need,
//! and registers a family's faces with the shaping engine when a document names it.

use ondin_core::Document;
use std::collections::HashSet;
use std::path::PathBuf;

pub(crate) struct SystemFonts {
    db: fontdb::Database,
    families: Vec<String>,
    /// The families looked for already, lowercased, found or not: each commit
    /// asks again, and a file read and registered twice is memory kept twice.
    tried: HashSet<String>,
}

impl SystemFonts {
    /// Read every installed face. Slow (a directory walk and a header read per
    /// file), so `serve` calls it once.
    pub(crate) fn load() -> Self {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        let families = crate::fonts::listed_families(&db).into_iter().collect();
        Self {
            db,
            families,
            tried: HashSet::new(),
        }
    }

    /// Every installed family, sorted.
    pub(crate) fn families(&self) -> &[String] {
        &self.families
    }

    /// Register the installed faces of each family `doc` names that the engine
    /// on this thread lacks, each family looked for once; whether any
    /// registered, so the caller lays text out again. A family named by one of
    /// a face's other names (a localized one) registers under that name, as
    /// the window's font service does.
    pub(crate) fn register_for(&mut self, doc: &Document) -> bool {
        let mut any = false;
        for (family, _) in ondin_core::families::families_in_use(doc) {
            if ondin_core::text::is_family_available(&family)
                || !self.tried.insert(family.to_lowercase())
            {
                continue;
            }
            let mut read: HashSet<PathBuf> = HashSet::new();
            for face in self.db.faces() {
                let Some(at) = face
                    .families
                    .iter()
                    .position(|(n, _)| n.eq_ignore_ascii_case(&family))
                else {
                    continue;
                };
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
                    // The engine registers a face under its own (English or first)
                    // name; a document naming it otherwise gets it as an alias.
                    any |= match at {
                        0 => {
                            let names = ondin_core::text::register_fonts(bytes.clone());
                            names.iter().any(|n| n.eq_ignore_ascii_case(&family))
                                || ondin_core::text::register_fonts_as(bytes, &family)
                        }
                        _ => ondin_core::text::register_fonts_as(bytes, &family),
                    };
                }
            }
        }
        any
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::{NodeKind, Operation, TextStyle, Transaction};

    /// A document naming an installed face by its other (localized) name gets
    /// it under that name, and asking again reads no file.
    /// flip: `register_for` registering the matched face under its own name
    /// only → fails at "available by the alias".
    #[test]
    #[ignore = "reads the machine's installed fonts"]
    fn system_fonts_register_a_face_by_its_other_name_once() {
        let mut fonts = SystemFonts::load();
        let Some(alias) = fonts
            .db
            .faces()
            .filter(|f| f.families.len() > 1)
            .map(|f| f.families[1].0.clone())
            .find(|n| !ondin_core::text::is_family_available(n))
        else {
            eprintln!("no installed face has a second family name; nothing to check");
            return;
        };
        let mut ids = ondin_core::IdSource::new(0xF0);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let kind = NodeKind::Text {
            content: "Hi".into(),
            style: Box::new(TextStyle {
                font_family: alias.clone(),
                font_size: 24.0,
                ..TextStyle::default()
            }),
            spans: Default::default(),
            para_spans: Default::default(),
            paragraph: Default::default(),
            block: Default::default(),
            sizing: ondin_core::TextSizing::Auto,
            on_path: None,
            on_path_flip: false,
            on_path_offset: 0.0,
        };
        let id = ids.mint();
        let op = Operation::CreateNode {
            id,
            parent: root,
            index: 0,
            kind,
            transform: None,
            name: None,
        };
        doc.apply(&Transaction(vec![op])).expect("a text layer");
        assert!(fonts.register_for(&doc), "registered");
        assert!(
            ondin_core::text::is_family_available(&alias),
            "available by the alias {alias}"
        );
        assert!(!fonts.register_for(&doc), "asked once");
    }
}
