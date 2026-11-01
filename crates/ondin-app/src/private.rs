//! Private mode: for sharing a screen without showing what things are called.
//!
//! While it is on, every name the app draws — a layer's, a frame's, a
//! component's, a page's, a document's and a project's — is drawn masked, as its
//! kind and its place ("Frame 2", "Page 3", "Document 1"). Text, pictures,
//! colours, and style and variable names stay: they are the work being shown.
//! Masked rather than removed, so whatever a name labels still selects, opens or
//! navigates as before.
//!
//! Every drawn name goes through this module. The switch is the `private`
//! preference, copied here once a frame by `OndinApp::ui`, so a view drawing a
//! name needs no handle on the app.

use ondin_core::{Document, NodeId};
use std::borrow::Cow;
use std::cell::Cell;

thread_local! {
    /// Whether names are masked this frame. Per thread, so tests running side by
    /// side each see their own app's switch; the app draws on one thread.
    static ON: Cell<bool> = const { Cell::new(false) };
}

/// Whether names are masked.
pub(crate) fn on() -> bool {
    ON.with(Cell::get)
}

/// Mask names from now on, or stop.
pub(crate) fn set(on: bool) {
    ON.with(|c| c.set(on));
}

/// What a door that needs the real name says while it declines.
pub(crate) const DECLINE_RENAME: &str = "Leave private mode to rename";

/// What a folded list of names says when asked to open.
pub(crate) const DECLINE_LIST: &str = "Leave private mode to see the names";

/// What a search over names says while it declines.
pub(crate) const DECLINE_SEARCH: &str = "Leave private mode to search names";

/// The accent *Private* pill a top bar shows while names are masked, so whoever
/// is sharing the screen can see it is on; `true` when it was clicked, which
/// turns private mode off. Draws nothing while it is off.
pub(crate) fn pill(ui: &mut egui::Ui) -> bool {
    use crate::theme::{self, color, icon};
    if !on() {
        return false;
    }
    let galley = ui.ctx().fonts_mut(|f| {
        f.layout_no_wrap(
            "Private".into(),
            egui::FontId::proportional(11.5),
            color::accent_100(),
        )
    });
    let w = 9.0 + 13.0 + 6.0 + galley.size().x + 9.0;
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, 24.0), egui::Sense::click());
    let p = ui.painter();
    let ground = color::accent_a(if resp.hovered() { 96 } else { 64 });
    p.rect_filled(rect, egui::CornerRadius::same(6), ground);
    let y = rect.center().y;
    p.text(
        egui::pos2(rect.left() + 9.0, y),
        egui::Align2::LEFT_CENTER,
        icon::EYE_SLASH,
        theme::icon_font(13.0),
        color::accent_100(),
    );
    p.galley(
        egui::pos2(rect.left() + 28.0, y - galley.size().y / 2.0),
        galley,
        color::accent_100(),
    );
    resp.on_hover_text("Names are hidden · click to leave private mode (Ctrl+Shift+N)")
        .clicked()
}

/// `id`'s name as drawn: the name itself, or masked as its kind's word and its
/// place among its siblings of that word — "Frame 2", "Rectangle 1". A page
/// root is [`page`]'s answer.
pub(crate) fn layer(doc: &Document, id: NodeId) -> Cow<'_, str> {
    let Some(node) = doc.get(id) else {
        return Cow::Borrowed("");
    };
    if !on() {
        return Cow::Borrowed(node.name());
    }
    if doc.is_page(id) {
        return page(doc, id);
    }
    let kind = word(doc, id);
    let place = node.parent().and_then(|p| doc.get(p)).map_or(1, |p| {
        p.children()
            .iter()
            .take_while(|c| **c != id)
            .filter(|c| word(doc, **c) == kind)
            .count()
            + 1
    });
    Cow::Owned(format!("{kind} {place}"))
}

/// The word a masked layer is called by: what it is, a main component or a
/// linked copy of one before its shape.
fn word(doc: &Document, id: NodeId) -> &'static str {
    match doc.get(id) {
        Some(n) if n.component() => "Component",
        Some(n) if n.link().is_some() => "Instance",
        Some(n) => n.kind().default_name(),
        None => "Layer",
    }
}

/// A page's name as drawn: the name, or "Page" and its place in the page order.
pub(crate) fn page(doc: &Document, page: NodeId) -> Cow<'_, str> {
    if !on() {
        return Cow::Borrowed(doc.get(page).map_or("", |n| n.name()));
    }
    let place = doc.page_index(page).map_or(1, |i| i + 1);
    Cow::Owned(format!("Page {place}"))
}

/// A document's name as drawn, given its place (from 0) among the documents it
/// is shown beside: the name, or "Document" and that place.
pub(crate) fn document(name: &str, place: usize) -> Cow<'_, str> {
    if !on() {
        return Cow::Borrowed(name);
    }
    Cow::Owned(format!("Document {}", place + 1))
}

/// A project's name as drawn, given its place (from 0) among the projects it is
/// shown beside: the name, or "Project" and that place.
pub(crate) fn project(name: &str, place: usize) -> Cow<'_, str> {
    if !on() {
        return Cow::Borrowed(name);
    }
    Cow::Owned(format!("Project {}", place + 1))
}

/// The guard: a document whose every name carries a sentinel, drawn whole with
/// each surface open, fails on any drawn string holding it.
#[cfg(test)]
mod leak {
    use crate::app::{OndinApp, View};
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{NodeId, NodeKind, Operation, Transaction};

    const SECRET: &str = "Zqxsecret";

    /// The app in the editor on a document whose layers, pages, components and
    /// file all carry [`SECRET`], private mode on, a layer selected. Returns it
    /// and the ids of a frame, a rect in it and an instance.
    fn fixture(ctx: &egui::Context) -> (OndinApp, [NodeId; 3]) {
        let mut app = OndinApp::headless(ctx);
        app.view = View::Editor;
        app.show_minimap = true;
        let page = app.session.page;
        let ids = &mut app.session.ids;
        let (frame, rect, text, main, copy, page2) = (
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
            ids.mint(),
        );
        let node = |id, parent, index, kind, name: &str| Operation::CreateNode {
            id,
            parent,
            index,
            kind,
            transform: Some(Affine::translate((40.0 * index as f64, 40.0))),
            name: Some(format!("{SECRET} {name}")),
        };
        let frame_kind = || NodeKind::Artboard {
            size: Size::new(30.0, 20.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        };
        let rect_kind = || NodeKind::Rect {
            size: Size::new(30.0, 20.0),
            corner_radii: Default::default(),
            corner_smoothing: 0.0,
        };
        let text_kind = NodeKind::Text {
            content: "Hello".into(),
            style: Box::new(ondin_core::TextStyle {
                font_family: "Inter".into(),
                font_size: 12.0,
                ..Default::default()
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
        app.session
            .try_commit(Transaction(vec![
                Operation::SetName {
                    id: page,
                    name: format!("{SECRET} page"),
                },
                node(frame, page, 0, frame_kind(), "frame"),
                node(rect, frame, 0, rect_kind(), "rect"),
                node(text, frame, 1, text_kind, "text"),
                node(main, page, 1, frame_kind(), "button"),
                Operation::SetComponent {
                    id: main,
                    component: true,
                },
                node(copy, page, 2, frame_kind(), "button"),
                Operation::SetLink {
                    id: copy,
                    link: Some(main),
                },
                Operation::AddPage {
                    id: page2,
                    index: 1,
                    name: format!("{SECRET} page two"),
                    ground: None,
                    draft: false,
                    mode: None,
                },
            ]))
            .expect("the fixture commits");
        app.session.path = Some(std::env::temp_dir().join(format!("{SECRET} doc.ondin")));
        app.session.selection.set_one(rect);
        app.prefs.private = true;
        (app, [frame, rect, copy])
    }

    /// Every string one whole frame of `app` draws, `events` delivered to it.
    fn drawn(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) -> Vec<String> {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1320.0, 820.0),
            )),
            events,
            ..Default::default()
        };
        let out = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
        let mut texts = Vec::new();
        fn walk(shape: &egui::Shape, texts: &mut Vec<String>) {
            match shape {
                egui::Shape::Text(t) => texts.push(t.galley.text().to_string()),
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, texts)),
                _ => {}
            }
        }
        for s in &out.shapes {
            walk(&s.shape, &mut texts);
        }
        for v in out.viewport_output.values() {
            for c in &v.commands {
                texts.push(format!("{c:?}"));
            }
        }
        texts
    }

    /// Several frames of `app`, so what opens on one frame is drawn on the next;
    /// the strings holding the sentinel, labelled with `surface`.
    fn leaks(ctx: &egui::Context, app: &mut OndinApp, surface: &str) -> Vec<String> {
        let mut out = Vec::new();
        for _ in 0..4 {
            out = drawn(ctx, app, Vec::new());
        }
        out.into_iter()
            .filter(|t| t.contains(SECRET))
            .map(|t| format!("{surface}: {t}"))
            .collect()
    }

    /// With private mode on, no drawn string holds a layer's, page's,
    /// component's or document's name, whichever surface is open.
    ///
    /// flip: `set` ignoring its argument (names never masked) → fails at the
    /// editor's own frame.
    #[test]
    fn no_surface_draws_a_name_in_private_mode() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, [_, rect, copy]) = fixture(&ctx);
        // Control: the fixture's names are drawn when private mode is off.
        app.prefs.private = false;
        assert!(
            !leaks(&ctx, &mut app, "off").is_empty(),
            "the fixture draws its names when private mode is off"
        );
        app.prefs.private = true;

        let mut found = leaks(&ctx, &mut app, "editor");
        app.session.selection.set_one(copy);
        found.extend(leaks(&ctx, &mut app, "instance selected"));
        app.session.selection.set_one(rect);

        app.toggle_palette(&ctx);
        found.extend(leaks(&ctx, &mut app, "palette"));
        if let Some(p) = app.palette.as_mut() {
            p.query = SECRET[..4].to_string();
        }
        found.extend(leaks(&ctx, &mut app, "palette, typed"));
        app.palette = None;

        app.open_context_menu(
            &ctx,
            crate::menu::Target::Layer {
                id: rect,
                door: crate::menu::Door::Canvas,
            },
            None,
        );
        found.extend(leaks(&ctx, &mut app, "context menu"));
        app.context_menu = None;

        app.view = View::Dashboard;
        found.extend(leaks(&ctx, &mut app, "library"));

        assert!(
            found.is_empty(),
            "names drawn in private mode:\n{}",
            found.join("\n")
        );
    }

    /// `Ctrl+Shift+N` turns private mode on as a saved preference, the top bar
    /// shows the *Private* pill, and the panes fold over the fold preferences
    /// without changing them; the chord again turns it off and the pill goes.
    ///
    /// flip: `set_private` writing `layers_folded` → fails at "the fold
    /// preference is left as it was".
    #[test]
    fn the_chord_turns_it_on_and_the_pill_says_so() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, _) = fixture(&ctx);
        app.prefs.private = false;
        let chord = || {
            let modifiers = egui::Modifiers {
                ctrl: true,
                shift: true,
                command: true,
                ..Default::default()
            };
            vec![egui::Event::Key {
                key: egui::Key::N,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers,
            }]
        };
        let pill = |texts: &[String]| texts.iter().any(|t| t == "Private");
        let frame_with = |app: &mut OndinApp, events: Vec<egui::Event>| {
            let mut frame = eframe::Frame::_new_kittest();
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::pos2(0.0, 0.0),
                    egui::vec2(1320.0, 820.0),
                )),
                modifiers: egui::Modifiers {
                    ctrl: true,
                    shift: true,
                    command: true,
                    ..Default::default()
                },
                events,
                ..Default::default()
            };
            let _ = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
        };
        assert!(
            !pill(&drawn(&ctx, &mut app, Vec::new())),
            "no pill while off"
        );
        frame_with(&mut app, chord());
        assert!(app.prefs.private, "the chord turns it on");
        assert!(
            pill(&drawn(&ctx, &mut app, Vec::new())),
            "and the pill shows it"
        );
        assert!(
            !app.prefs.layers_folded && !app.prefs.pages_folded,
            "the fold preference is left as it was"
        );
        frame_with(&mut app, chord());
        assert!(!app.prefs.private, "the chord again turns it off");
        assert!(
            !pill(&drawn(&ctx, &mut app, Vec::new())),
            "and the pill goes"
        );
    }

    /// The library masks its projects' and documents' names, on *Recent*, in the
    /// project and in its list view, and the editor's crumb and tab do too.
    #[test]
    fn the_library_draws_no_name_in_private_mode() {
        use crate::library::{project, state::Library, store};
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = std::env::temp_dir().join(format!("ondin-private-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        let mut app = OndinApp::headless(&ctx);
        app.prefs.base_folder = Some(root.clone());
        app.library = Library::open(root.clone());
        app.library.projects.projects.push(project::Project {
            id: "p-1".into(),
            name: format!("{SECRET} project"),
            color: project::PROJECT_COLORS[0].into(),
            folder: Some("secret".into()),
            created: 0,
            archived: false,
        });
        let filed = app.library.projects.get("p-1").cloned();
        let mut doc = ondin_core::Document::new(app.session.ids.mint());
        let path = store::file_document(&root, filed.as_ref(), &format!("{SECRET} doc"), &mut doc)
            .unwrap();
        app.library.refresh();
        app.prefs.private = true;
        app.view = View::Dashboard;

        let mut found = leaks(&ctx, &mut app, "recent");
        app.dash.nav = crate::panels::dashboard::Nav::Project("p-1".into());
        found.extend(leaks(&ctx, &mut app, "project"));
        app.dash.list_view = !app.dash.list_view;
        found.extend(leaks(&ctx, &mut app, "project, other view"));

        app.session.adopt_document(doc, Some(path));
        app.view = View::Editor;
        found.extend(leaks(&ctx, &mut app, "editor"));

        // Control: the same frames name them with private mode off.
        app.prefs.private = false;
        assert!(
            !leaks(&ctx, &mut app, "off").is_empty(),
            "the fixture is named"
        );
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            found.is_empty(),
            "names drawn in private mode:\n{}",
            found.join("\n")
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::kurbo::{Affine, Size};
    use ondin_core::{IdSource, NodeKind, Operation, Transaction};

    /// Masked, a layer is its kind and its place among siblings of that kind, a
    /// page its place in the page order; off, both are their names.
    #[test]
    fn a_masked_name_is_its_kind_and_its_place() {
        let mut ids = IdSource::new(1);
        let root = ids.mint();
        let mut doc = Document::new(root);
        let frame = |id, index| Operation::CreateNode {
            id,
            parent: root,
            index,
            kind: NodeKind::Artboard {
                size: Size::new(10.0, 10.0),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::IDENTITY),
            name: Some("Acme checkout".into()),
        };
        let (a, b, r) = (ids.mint(), ids.mint(), ids.mint());
        doc.apply(&Transaction(vec![
            frame(a, 0),
            Operation::CreateNode {
                id: r,
                parent: root,
                index: 1,
                kind: NodeKind::Rect {
                    size: Size::new(4.0, 4.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: Some(Affine::IDENTITY),
                name: Some("Acme logo".into()),
            },
            frame(b, 2),
        ]))
        .unwrap();
        set(false);
        assert_eq!(layer(&doc, b), "Acme checkout");
        set(true);
        assert_eq!(layer(&doc, a), "Frame 1");
        assert_eq!(layer(&doc, r), "Rectangle 1");
        assert_eq!(
            layer(&doc, b),
            "Frame 2",
            "the rectangle between does not count"
        );
        assert_eq!(page(&doc, doc.first_page()), "Page 1");
        assert_eq!(document("Acme", 2), "Document 3");
        set(false);
    }
}
