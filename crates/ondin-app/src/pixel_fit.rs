//! *Pixel-fit* on the canvas: the selection's layers, or the edited path's points,
//! fitted by `ondin_core::pixel_fit` in one undo step, and the status line saying
//! what moved and what was skipped.

use crate::app::OndinApp;
use crate::theme::icon;
use ondin_core::Transaction;
use ondin_core::pixel_fit::Fitted;

/// The status line for a fit: what moved, then what was skipped and why.
pub(crate) fn message(fitted: &Fitted) -> String {
    let mut skipped = Vec::new();
    if fitted.text > 0 {
        skipped.push(crate::ui::count(fitted.text, "text layer"));
    }
    if fitted.turned > 0 {
        skipped.push(crate::ui::count(fitted.turned, "rotated layer"));
    }
    if fitted.locked > 0 {
        skipped.push(crate::ui::count(fitted.locked, "locked layer"));
    }
    let skipped = match skipped.split_last() {
        Some((last, rest)) if !rest.is_empty() => format!("{} and {last}", rest.join(", ")),
        _ => skipped.join(""),
    };
    match (fitted.moved, skipped.is_empty()) {
        (0, true) => "Already on the pixel grid".to_owned(),
        (0, false) => format!("Pixel-fit skipped {skipped}"),
        (n, true) => format!("Moved {} onto the pixel grid", crate::ui::count(n, "point")),
        (n, false) => format!(
            "Moved {} onto the pixel grid; skipped {skipped}",
            crate::ui::count(n, "point")
        ),
    }
}

impl OndinApp {
    /// *Pixel-fit* on the selection: every path, shape and line in it, through
    /// groups and frames, in one undo step.
    pub(crate) fn pixel_fit(&mut self) {
        let (tx, fitted) = ondin_core::pixel_fit::fit(
            &self.session.doc,
            &self.session.resolved,
            self.session.selection.ids(),
        );
        self.commit_fit(tx, &fitted);
    }

    /// *Pixel-fit* from the path bar: the edited path's selected points, or the
    /// whole path with none selected.
    pub(crate) fn pixel_fit_points(&mut self) {
        let Some(id) = self.edited_path() else { return };
        let points = self.subject_points(id);
        let only = (!points.is_empty()).then_some(points);
        let (tx, fitted) = ondin_core::pixel_fit::fit_points(
            &self.session.doc,
            &self.session.resolved,
            id,
            only.as_deref(),
        );
        self.commit_fit(tx, &fitted);
    }

    fn commit_fit(&mut self, tx: Transaction, fitted: &Fitted) {
        if !tx.0.is_empty() {
            self.session.name_step("Pixel-fit", icon::PIXEL_FIT);
            if !self.commit_edit(tx) {
                return;
            }
        }
        self.session.info(message(fitted));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use eframe::egui;
    use ondin_core::kurbo::{Affine, Point, Rect};
    use ondin_core::{NodeKind, Operation};

    /// Over a selection of a stroked rect, a path, a turned rect and a text layer,
    /// one *Pixel-fit* step puts the rect's X, Y, W and H on whole units (its 1-wide
    /// centred stroke changes nothing) and the path's corners too, and skips and
    /// names the other two.
    /// flip: `pixel_fit::fit_layer` without its `square_to_the_page` gate → fails at
    /// "the turned rect is left alone".
    #[test]
    fn pixel_fit_fits_the_selection_in_one_step() {
        use ondin_core::kurbo::{BezPath, Size};
        use ondin_core::{Brush, Stroke, keyed_by_position};
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let page = app.session.doc.first_page();
        let [rect, path, turned, text] = [(); 4].map(|_| app.session.ids.mint());
        let shape = |id, index, at: (f64, f64), rotate: f64| Operation::CreateNode {
            id,
            parent: page,
            index,
            kind: NodeKind::Rect {
                size: Size::new(300.5, 9.7),
                corner_radii: Default::default(),
                corner_smoothing: 0.0,
            },
            transform: Some(Affine::translate(at) * Affine::rotate(rotate)),
            name: None,
        };
        let mut line = BezPath::new();
        line.move_to((0.4, 0.4));
        line.line_to((6.6, 0.4));
        line.line_to((6.6, 5.2));
        let tx = Transaction(vec![
            shape(rect, 0, (663.0, 4.6), 0.0),
            Operation::SetStrokes {
                id: rect,
                strokes: keyed_by_position([Stroke {
                    brush: Brush::Solid(ondin_core::peniko::Color::BLACK).into(),
                    width: 1.0,
                    ..Default::default()
                }]),
            },
            Operation::CreateNode {
                id: path,
                parent: page,
                index: 1,
                kind: NodeKind::Path {
                    path: line,
                    corner_radii: Vec::new(),
                    corner_smoothing: 0.0,
                    mirror: ondin_core::symmetry::PathMirror::NONE,
                },
                transform: Some(Affine::translate((40.0, 40.0))),
                name: None,
            },
            shape(turned, 2, (80.3, 80.3), 0.3),
            Operation::CreateNode {
                id: text,
                parent: page,
                index: 3,
                kind: NodeKind::Text {
                    content: "Hi".into(),
                    style: Box::default(),
                    spans: Default::default(),
                    para_spans: Default::default(),
                    paragraph: Default::default(),
                    block: Default::default(),
                    sizing: ondin_core::TextSizing::Auto,
                    on_path: None,
                    on_path_flip: false,
                    on_path_offset: 0.0,
                },
                transform: Some(Affine::translate((0.3, 90.2))),
                name: None,
            },
        ]);
        assert!(app.session.commit(tx));
        let turned_before = app.session.resolved.world_transform(turned);
        app.session.selection.set(vec![rect, path, turned, text]);
        let depth = app.session.history.undo_depth();
        app.pixel_fit();

        let res = &app.session.resolved;
        let world = |id| {
            let local = ondin_core::local_box(&app.session.doc, res, id).expect("a box");
            res.world_transform(id)
                .expect("placed")
                .transform_rect_bbox(local)
        };
        assert_eq!(
            world(rect),
            Rect::new(663.0, 5.0, 964.0, 14.0),
            "X stays on 663, W 300.5 becomes 301, Y and H land whole"
        );
        let (_, subs) = app.edited_subpaths(path).expect("a path");
        let at: Vec<Point> = subs[0].anchors.iter().map(|a| a.at).collect();
        assert_eq!(
            at,
            [
                Point::new(40.0, 40.0),
                Point::new(47.0, 40.0),
                Point::new(47.0, 45.0)
            ]
        );
        assert_eq!(
            res.world_transform(turned),
            turned_before,
            "the turned rect is left alone"
        );
        assert_eq!(app.session.history.undo_depth(), depth + 1, "one step");
        assert_eq!(
            app.session.status().text,
            "Moved 7 points onto the pixel grid; skipped 1 text layer and 1 rotated layer"
        );
    }

    /// The status line names what moved and what was skipped.
    #[test]
    fn the_status_line_says_what_happened() {
        let say = |moved, text, turned| {
            message(&Fitted {
                moved,
                text,
                turned,
                locked: 0,
            })
        };
        assert_eq!(say(0, 0, 0), "Already on the pixel grid");
        assert_eq!(say(14, 0, 0), "Moved 14 points onto the pixel grid");
        assert_eq!(say(0, 2, 0), "Pixel-fit skipped 2 text layers");
        assert_eq!(say(0, 0, 1), "Pixel-fit skipped 1 rotated layer");
        assert_eq!(
            say(4, 1, 1),
            "Moved 4 points onto the pixel grid; skipped 1 text layer and 1 rotated layer"
        );
        let all = message(&Fitted {
            moved: 0,
            text: 1,
            turned: 2,
            locked: 1,
        });
        assert_eq!(
            all,
            "Pixel-fit skipped 1 text layer, 2 rotated layers and 1 locked layer"
        );
    }
}
