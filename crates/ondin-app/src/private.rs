//! Private mode: for sharing a screen without showing what things are called.
//!
//! While it is on, every name the app draws — a layer's, a frame's, a
//! component's, a page's, a document's, a project's and a version's — is drawn masked, as its
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

/// What *Combine as variants* says while it declines: each main's name would
/// become a variant value, which the variants card draws.
pub(crate) const DECLINE_COMBINE: &str =
    "Leave private mode to combine: variants take their mains' names";

/// A failed write as the status line says it: without the file's path while
/// names are masked, since a document's path holds its name and its project's.
pub(crate) fn io_error(e: &std::io::Error) -> String {
    if on() {
        crate::atomic::without_path(e)
    } else {
        e.to_string()
    }
}

/// The *Private mode* pill a top bar shows while names are masked, latched
/// ([`FieldButton::On`]) so whoever is sharing the screen can see it is on; `true`
/// when it was clicked, which turns private mode off. Draws nothing while it is
/// off. Worded as the mode, with a browser's incognito glyph: a bare "Private"
/// read as a document not shared with a team, which this personal app has no
/// notion of.
///
/// [`FieldButton::On`]: crate::ui::FieldButton::On
pub(crate) fn pill(ui: &mut egui::Ui) -> bool {
    use crate::theme::{self, color, icon};
    use crate::ui::{self, FieldButton};
    if !on() {
        return false;
    }
    let scale = ui::Scale::Toolbar;
    let glyph_pt = scale.glyph_pt();
    let font = egui::FontId::proportional(scale.text_pt());
    let text_w = ui
        .ctx()
        .fonts_mut(|f| f.layout_no_wrap("Private mode".into(), font.clone(), color::text()))
        .size()
        .x;
    let w = 9.0 + glyph_pt + 6.0 + text_w + 9.0;
    let (rect, resp, fg) = ui::button_face(ui, egui::vec2(w, ui::TOOLBAR_H), FieldButton::On);
    let p = ui.painter();
    let y = rect.center().y;
    p.text(
        egui::pos2(rect.left() + 9.0, y),
        egui::Align2::LEFT_CENTER,
        icon::INCOGNITO,
        theme::icon_font(glyph_pt),
        fg,
    );
    p.text(
        egui::pos2(rect.left() + 9.0 + glyph_pt + 6.0, y),
        egui::Align2::LEFT_CENTER,
        "Private mode",
        font,
        fg,
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

/// A version's name as drawn, given its place (from 0) in the version history: the
/// name, or "Version" and that place.
pub(crate) fn version(name: &str, place: usize) -> Cow<'_, str> {
    if !on() {
        return Cow::Borrowed(name);
    }
    Cow::Owned(format!("Version {}", place + 1))
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
    /// file all carry [`SECRET`], private mode on, a layer selected. The main
    /// holds a nested instance of another main, bound to a swap property whose
    /// filter is that main's name; a set holds two variant mains. Returns it and
    /// the ids of a frame, a rect in it, an instance, the main and the set.
    fn fixture(ctx: &egui::Context) -> (OndinApp, [NodeId; 5]) {
        use ondin_core::variant::{PropKind, Property, VariantProp, VariantSet};
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
        let (icons, slot, set, small, large) =
            (ids.mint(), ids.mint(), ids.mint(), ids.mint(), ids.mint());
        let swap = ondin_core::Keyed::new(
            ids.mint_item(),
            Property {
                name: "Icon".into(),
                kind: PropKind::Swap,
                bound: vec![slot],
                filter: format!("{SECRET} Icons"),
                preferred: Vec::new(),
            },
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
                node(icons, page, 2, frame_kind(), "Icons / Star"),
                Operation::SetComponent {
                    id: icons,
                    component: true,
                },
                node(slot, main, 0, frame_kind(), "icon"),
                Operation::SetLink {
                    id: slot,
                    link: Some(icons),
                },
                Operation::SetProperties {
                    id: main,
                    props: vec![swap],
                },
                node(copy, page, 3, frame_kind(), "button"),
                Operation::SetLink {
                    id: copy,
                    link: Some(main),
                },
                node(set, page, 4, frame_kind(), "set"),
                node(small, set, 0, frame_kind(), "small"),
                node(large, set, 1, frame_kind(), "large"),
                Operation::SetComponent {
                    id: small,
                    component: true,
                },
                Operation::SetComponent {
                    id: large,
                    component: true,
                },
                Operation::SetVariantSet {
                    id: set,
                    set: Some(VariantSet {
                        props: vec![VariantProp::new(
                            "Size",
                            vec!["Small".into(), "Large".into()],
                        )],
                        ..Default::default()
                    }),
                },
                Operation::SetVariant {
                    id: small,
                    values: vec!["Small".into()],
                },
                Operation::SetVariant {
                    id: large,
                    values: vec!["Large".into()],
                },
                Operation::AddPage {
                    id: page2,
                    index: 1,
                    name: format!("{SECRET} page two"),
                    ground: None,
                    draft: false,
                    modes: Vec::new(),
                },
            ]))
            .expect("the fixture commits");
        app.session.path = Some(std::env::temp_dir().join(format!("{SECRET} doc.ondin")));
        app.session.selection.set_one(rect);
        app.prefs.private = true;
        (app, [frame, rect, copy, main, set])
    }

    /// Every string one whole frame of `app` draws, `events` delivered to it. A
    /// window tall enough for the whole inspector: a card below the fold is
    /// culled, and would pass unread.
    fn drawn(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) -> Vec<String> {
        drawn_at(ctx, app, events)
            .into_iter()
            .map(|(t, _)| t)
            .collect()
    }

    /// `drawn`, each string with the rect it was drawn in.
    fn drawn_at(
        ctx: &egui::Context,
        app: &mut OndinApp,
        events: Vec<egui::Event>,
    ) -> Vec<(String, egui::Rect)> {
        let mut frame = eframe::Frame::_new_kittest();
        let input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::pos2(0.0, 0.0),
                egui::vec2(1320.0, 2400.0),
            )),
            events,
            ..Default::default()
        };
        let out = ctx.run_ui(input, |ui| eframe::App::ui(app, ui, &mut frame));
        let mut texts = Vec::new();
        fn walk(shape: &egui::Shape, texts: &mut Vec<(String, egui::Rect)>) {
            match shape {
                egui::Shape::Text(t) => {
                    texts.push((t.galley.text().to_string(), t.visual_bounding_rect()))
                }
                egui::Shape::Vec(v) => v.iter().for_each(|s| walk(s, texts)),
                _ => {}
            }
        }
        for s in &out.shapes {
            walk(&s.shape, &mut texts);
        }
        for v in out.viewport_output.values() {
            for c in &v.commands {
                texts.push((format!("{c:?}"), egui::Rect::NOTHING));
            }
        }
        texts
    }

    /// Press and release at `at`, the pointer moved there first.
    fn click(ctx: &egui::Context, app: &mut OndinApp, at: egui::Pos2) {
        let press = |pressed| egui::Event::PointerButton {
            pos: at,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: Default::default(),
        };
        drawn(ctx, app, vec![egui::Event::PointerMoved(at)]);
        drawn(ctx, app, vec![press(true)]);
        drawn(ctx, app, vec![press(false)]);
    }

    /// Several frames of `app`, so what opens on one frame is drawn on the next;
    /// the strings holding the sentinel, labelled with `surface`.
    fn leaks(ctx: &egui::Context, app: &mut OndinApp, surface: &str) -> Vec<String> {
        leaks_of(ctx, app, surface, &[SECRET])
    }

    /// [`leaks`] for a fixture whose names are `marks`, not [`SECRET`].
    fn leaks_of(
        ctx: &egui::Context,
        app: &mut OndinApp,
        surface: &str,
        marks: &[&str],
    ) -> Vec<String> {
        let mut out = Vec::new();
        for _ in 0..4 {
            out = drawn(ctx, app, Vec::new());
        }
        out.into_iter()
            .filter(|t| marks.iter().any(|m| t.contains(m)))
            .map(|t| format!("{surface}: {t}"))
            .collect()
    }

    /// One surface's share of the guard. With private mode off, opening it must
    /// draw a string holding [`SECRET`] the app did not draw without it — so it really is a
    /// surface that names something, and an empty answer below means something;
    /// with it on, the strings holding it are the answer. `close` puts the app
    /// back.
    fn surface(
        ctx: &egui::Context,
        app: &mut OndinApp,
        label: &str,
        open: impl Fn(&mut OndinApp, &egui::Context),
        close: impl Fn(&mut OndinApp),
    ) -> Vec<String> {
        surface_of(ctx, app, label, &[SECRET], open, close)
    }

    /// [`surface`] for a fixture whose names are `marks`.
    fn surface_of(
        ctx: &egui::Context,
        app: &mut OndinApp,
        label: &str,
        marks: &[&str],
        open: impl Fn(&mut OndinApp, &egui::Context),
        close: impl Fn(&mut OndinApp),
    ) -> Vec<String> {
        app.prefs.private = false;
        let without = leaks_of(ctx, app, label, marks);
        open(app, ctx);
        let with = leaks_of(ctx, app, label, marks);
        // Strings counted with repeats, so a second copy of a name is new too.
        let mut left = without.clone();
        let new = with
            .iter()
            .filter(|t| match left.iter().position(|w| w == *t) {
                Some(i) => {
                    left.remove(i);
                    false
                }
                None => true,
            })
            .count();
        assert!(
            new > 0,
            "the {label} draws a name of its own with private mode off:\n{with:#?}\nagainst\n{without:#?}"
        );
        close(app);
        app.prefs.private = true;
        open(app, ctx);
        let found = leaks_of(ctx, app, label, marks);
        close(app);
        found
    }

    /// With private mode on, no drawn string holds a layer's, page's,
    /// component's or document's name, whichever surface is open.
    ///
    /// flip: `set` ignoring its argument (names never masked) → fails at the
    /// editor's own frame; `property_row` drawing the filter field in private
    /// mode → "main selected"; `preferred_rows` naming a main by `.name()` →
    /// "swap picker open"; `paint_mark` tagging a mark with `row.name` → "compare".
    #[test]
    fn no_surface_draws_a_name_in_private_mode() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, [_, rect, copy, main, set]) = fixture(&ctx);
        // Control: the fixture's names are drawn when private mode is off.
        app.prefs.private = false;
        assert!(
            !leaks(&ctx, &mut app, "off").is_empty(),
            "the fixture draws its names when private mode is off"
        );
        app.prefs.private = true;

        let mut found = leaks(&ctx, &mut app, "editor");
        let c = &ctx;
        found.extend(surface(
            c,
            &mut app,
            "instance selected",
            |app, _| app.session.selection.set_one(copy),
            |app| app.session.selection.set_one(rect),
        ));
        // The main's card lists its swap property, whose filter is a main's name.
        found.extend(surface(
            c,
            &mut app,
            "main selected",
            |app, _| app.session.selection.set_one(main),
            |app| app.session.selection.set_one(rect),
        ));
        found.extend(surface(
            c,
            &mut app,
            "set selected",
            |app, _| app.session.selection.set_one(set),
            |app| app.session.selection.set_one(rect),
        ));
        // The swap property's picker, clicked open, lists mains by name.
        found.extend(surface(
            c,
            &mut app,
            "swap picker open",
            |app, ctx| {
                app.session.selection.set_one(main);
                drawn(ctx, app, Vec::new());
                let at = drawn_at(ctx, app, Vec::new())
                    .into_iter()
                    .find(|(t, _)| t == "Add a preferred main…")
                    .expect("the swap property's picker")
                    .1
                    .center();
                click(ctx, app, at);
            },
            |app| {
                egui::Popup::close_all(c);
                app.session.selection.set_one(rect);
            },
        ));
        found.extend(surface(
            c,
            &mut app,
            "palette",
            |app, ctx| app.toggle_palette(ctx),
            |app| app.palette = None,
        ));
        // Find and replace lists each match under its page and beside its layer.
        found.extend(surface(
            c,
            &mut app,
            "find and replace",
            |app, _| {
                app.open_find(true);
                app.find.find = "Hello".into();
                app.find.scope = crate::find::In::All;
            },
            |app| app.close_find(),
        ));
        // A compare names each changed layer in its card and in its mark's tag.
        let version = std::env::temp_dir().join(format!(
            "ondin-private-compare-{}.ondin",
            std::process::id()
        ));
        found.extend(surface(
            c,
            &mut app,
            "compare",
            |app, _| {
                let doc = ondin_core::io::save(&app.session.doc).unwrap();
                std::fs::write(&version, doc).unwrap();
                let locked = app.session.doc.get(rect).unwrap().locked();
                assert!(app.session.commit(ondin_core::Transaction(vec![
                    ondin_core::Operation::SetLocked {
                        id: rect,
                        locked: !locked,
                    }
                ])));
                let pick = crate::compare::Pick {
                    path: version.clone(),
                    title: "Before".into(),
                    when: None,
                };
                app.compare_with(pick, None);
                assert!(app.session.compare.is_some(), "fixture: comparing");
            },
            |app| app.leave_compare(),
        ));
        let _ = std::fs::remove_file(&version);
        found.extend(surface(
            c,
            &mut app,
            "palette, typed",
            |app, ctx| {
                app.toggle_palette(ctx);
                if let Some(p) = app.palette.as_mut() {
                    p.query = SECRET[..4].to_string();
                }
            },
            |app| app.palette = None,
        ));
        // A layer's menu names pages in its *Move to page* list, held open as
        // a hover holds it.
        found.extend(surface(
            c,
            &mut app,
            "context menu, move to page",
            |app, ctx| {
                let target = crate::menu::Target::Layer {
                    id: rect,
                    door: crate::menu::Door::Canvas,
                };
                app.open_context_menu(ctx, target, None);
                let menu = app.context_menu.as_mut().expect("the menu is open");
                menu.flyout = Some(crate::menu::Flyout {
                    parent: crate::menu::Item::MoveToPage,
                    row: egui::Rect::NOTHING,
                    highlight: None,
                });
            },
            |app| app.context_menu = None,
        ));
        // The variables browser, pressed to *In selection*, says whose variables
        // it lists. It stands beside the canvas, so one is laid out.
        let canvas = app.canvas_rect;
        app.canvas_rect =
            egui::Rect::from_min_size(egui::pos2(300.0, 60.0), egui::vec2(700.0, 600.0));
        found.extend(surface(
            c,
            &mut app,
            "variables browser",
            |app, _| {
                app.toggle_var_browser();
                if let Some(b) = app.var_browser.as_mut() {
                    b.in_selection = true;
                }
            },
            |app| app.var_browser = None,
        ));
        app.canvas_rect = canvas;
        // The export card's button names the layer it exports.
        found.extend(surface(
            c,
            &mut app,
            "export card",
            |app, _| {
                let png = ondin_core::ExportSpec::new(
                    ondin_core::ExportFormat::Png,
                    ondin_core::ExportScale::Times(1.0),
                );
                app.session
                    .try_commit(Transaction(vec![Operation::SetExports {
                        id: rect,
                        exports: ondin_core::keyed_by_position([png]),
                    }]))
                    .expect("an export on the rect");
                // Folded while the layer exported nothing, the card stays so
                // until the selection changes.
                app.collapsed_panels.remove("Export");
            },
            |app| {
                app.session
                    .try_commit(Transaction(vec![Operation::SetExports {
                        id: rect,
                        exports: Vec::new(),
                    }]))
                    .expect("no export on the rect");
            },
        ));
        // The contrast check's bar names its scope.
        found.extend(surface(
            c,
            &mut app,
            "contrast check",
            |app, _| app.toggle_contrast(),
            |app| app.contrast = None,
        ));
        // So does the colour-vision simulation's.
        found.extend(surface(
            c,
            &mut app,
            "colour-vision simulation",
            |app, _| app.toggle_vision(),
            |app| app.vision = None,
        ));
        found.extend(surface(
            c,
            &mut app,
            "version history",
            |app, _| app.versions_open = true,
            |app| app.versions_open = false,
        ));
        found.extend(surface(
            c,
            &mut app,
            "recovery card",
            |app, _| {
                app.recovery
                    .pending
                    .push(crate::library::recovery::Pending {
                        key: "k".into(),
                        path: std::env::temp_dir().join("ondin-private-no-snapshot"),
                        name: format!("{SECRET} doc"),
                        target: None,
                        written: None,
                    })
            },
            |app| app.recovery.pending.clear(),
        ));
        found.extend(surface(
            c,
            &mut app,
            "close confirmation",
            |app, _| {
                app.session.mark_unsaved();
                app.confirming_close = true;
            },
            |app| app.confirming_close = false,
        ));
        // Present mode's counter names the frame on show.
        found.extend(surface(
            c,
            &mut app,
            "present mode",
            |app, ctx| app.start_presenting(ctx),
            |app| app.presenting = None,
        ));

        app.view = View::Dashboard;
        found.extend(leaks(&ctx, &mut app, "library"));

        assert!(
            found.is_empty(),
            "names drawn in private mode:\n{}",
            found.join("\n")
        );
    }

    /// *Combine as variants* declines in private mode, since each main's name
    /// would become a variant value the variants card draws; off, it combines.
    /// flip: `combine_as_variants` without its `private::on()` check → fails at
    /// "declined".
    #[test]
    fn combine_as_variants_declines_in_private_mode() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let (mut app, [_, _, _, main, _]) = fixture(&ctx);
        let page = app.session.page;
        let icons = app.session.doc.get(page).unwrap().children()[2];
        app.session.selection.set(vec![main, icons]);
        super::set(true);
        app.combine_as_variants();
        super::set(false);
        assert_eq!(
            app.session.doc.get(main).and_then(|n| n.parent()),
            Some(page),
            "declined"
        );
        assert_eq!(app.session.status().text, super::DECLINE_COMBINE);
        app.combine_as_variants();
        assert_ne!(
            app.session.doc.get(main).and_then(|n| n.parent()),
            Some(page),
            "control: combined with private mode off"
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
        let pill = |texts: &[String]| texts.iter().any(|t| t == "Private mode");
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

    /// No kit surface names the kit, its components or the document using them:
    /// an instance's card, the insert picker, *Kits used*, *Make local*, the
    /// update pill's popover, and on the kit itself *Publish*. The picker's
    /// fixture kit is *Atlas*, holding
    /// *Badge* and a set in a frame *Forms*, used by *Landing*.
    ///
    /// flip: `kits_in` naming a kit by `display_name` → fails at the instance's
    /// card, the picker, *Kits used* and *Make local*.
    #[test]
    fn no_kit_surface_draws_a_name_in_private_mode() {
        use crate::kits::index::{Load, kits_in};
        use crate::kits::picker::tests::{app_on, file_kit, temp_root};
        const MARKS: [&str; 4] = ["Atlas", "Badge", "Forms", "Landing"];
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let root = temp_root("private");
        let (kit, _, badge) = file_kit(&root);
        let mut app = app_on(&ctx, root.clone());
        let k = kits_in(&app.library, app.session.doc.meta().id.as_deref())
            .into_iter()
            .find(|k| k.id == kit)
            .expect("the kit is listed");
        app.kit_shelf.ask(&k);
        app.kit_shelf.settle();
        let Some(Load::Ready(Some(p))) = app.kit_shelf.load(&kit) else {
            panic!("the kit's published state loads");
        };
        let page = app.session.doc.first_page();
        let (tx, inst) = ondin_core::kit::insert(
            &app.session.doc,
            &mut app.session.ids,
            &kit,
            4,
            p,
            badge,
            page,
        )
        .expect("an instance of Badge");
        assert!(app.session.commit(tx));
        let main = ondin_core::component::main_of(&app.session.doc, inst).expect("its main");
        let top = app.kit_line(main).expect("a kit main").top;

        let c = &ctx;
        let mut found = Vec::new();
        let mut one = |app: &mut OndinApp,
                       label,
                       open: &dyn Fn(&mut OndinApp, &egui::Context),
                       close: &dyn Fn(&mut OndinApp)| {
            found.extend(surface_of(c, app, label, &MARKS, open, close));
        };
        one(
            &mut app,
            "kit instance selected",
            &|app, _| app.session.selection.set_one(inst),
            &|app| app.session.selection.clear(),
        );
        one(
            &mut app,
            "insert picker",
            &|app, _| app.toggle_kit_picker(),
            &|app| app.kit_picker = None,
        );
        one(
            &mut app,
            "kits used",
            &|app, _| app.kits_used_open = true,
            &|app| app.kits_used_open = false,
        );
        one(
            &mut app,
            "make local",
            &|app, _| app.making_local = Some(top),
            &|app| app.making_local = None,
        );
        // The kit publishes again: the update pill's popover names the kit. Its
        // components are a count until a row is expanded, which this leaves shut.
        let kit_path = root.join("atlas.ondin");
        let mut published = ondin_core::io::load(&std::fs::read(&kit_path).unwrap()).unwrap();
        published
            .apply(&Transaction(vec![Operation::SetOpacity {
                id: badge,
                opacity: 0.5,
            }]))
            .unwrap();
        ondin_core::kit::publish(&mut published, None, 2).unwrap();
        crate::library::store::write_document(&kit_path, &published).unwrap();
        // A modification time on, so the shelf reads the file again.
        std::fs::File::options()
            .write(true)
            .open(&kit_path)
            .unwrap()
            .set_modified(std::time::SystemTime::now() + std::time::Duration::from_secs(2))
            .unwrap();
        app.library.refresh();
        one(
            &mut app,
            "kit update popover",
            &|app, _| {
                app.kit_shelf.settle();
                app.open_menu = crate::app::TopMenu::KitUpdates;
            },
            &|app| app.open_menu = crate::app::TopMenu::None,
        );
        // On the kit itself, with a change to publish.
        app.open_path(&root.join("atlas.ondin"));
        app.session
            .try_commit(Transaction(vec![Operation::SetOpacity {
                id: badge,
                opacity: 0.25,
            }]))
            .expect("a change to publish");
        one(&mut app, "publish", &|app, _| app.open_publish(), &|app| {
            app.publishing = None
        });
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            found.is_empty(),
            "names drawn in private mode:\n{}",
            found.join("\n")
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
        // The document's own cards name it.
        let c = &ctx;
        let file = path.clone();
        found.extend(surface(
            c,
            &mut app,
            "delete card",
            |app, _| app.dash.deleting = Some(file.clone()),
            |app| app.dash.deleting = None,
        ));
        found.extend(surface(
            c,
            &mut app,
            "move card",
            |app, _| app.dash.moving = Some(file.clone()),
            |app| app.dash.moving = None,
        ));
        found.extend(surface(
            c,
            &mut app,
            "missing-file card",
            |app, _| app.dash.missing = Some(file.clone()),
            |app| app.dash.missing = None,
        ));

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
