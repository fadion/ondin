//! The canvas tools (§9.4): [`Tool`], the tool rail's set, what a plain click with
//! one makes, and the counts `↑`/`↓` step while a shape is drawn. Each gesture
//! produces one `Transaction` committed on release (§9.3); the canvas supplies the
//! egui events.
//!
//! The arithmetic the gestures run is headless and lives in `ondin_core::edit`;
//! it is re-exported here, so the canvas calls it as `tools::…`.

pub use ondin_core::edit::{
    ARC_DEFAULT_STEPS, ARC_MAX_STEPS, MAX_SKEW, POLYGON_DEFAULT_SIDES, Resize,
    SPIRAL_DEFAULT_HALF_TURNS, SPIRAL_MAX_HALF_TURNS, STAR_DEFAULT_POINTS, Scaling, SelectionBox,
    SideDrag, align_points, arc_sweep, bend_segment, create_arc, create_artboard, create_ellipse,
    create_image, create_image_in_box, create_line, create_paths, create_polygon, create_rect,
    create_slice, create_spiral, create_star, create_text, crop_resized, croppable_fill,
    distribute_points, gesture_text_sizing, insert_point, join_ends, join_runs, joinable_ends,
    line_ends_world, map_subpaths, move_line_end, move_points, paired_size, path_subpaths,
    pen_anchors, pen_close_snap, pen_outline, pen_path, point_spacings, points_box,
    proportions_locked, pull_handle, redundant_points, remove_points, remove_segments,
    resizable_size, resize_box_to, resize_layer, resize_points, resize_selection,
    resized_text_sizing, reverse_subpaths, rewrite_path, rewrite_points, rotate_node,
    rotate_points, rotate_selection, segment_cubic, segment_ends, segment_is_straight,
    segment_point, set_smoothness, side_shear, skew_selection, skew_to_handle, smooth_points,
    solid_stroke, space_points, split_at_points, split_segments, splittable, step_sides,
    subpaths_touched, tangent_snap, transform_node, turn_about,
};

/// The active canvas tool. `Select` manipulates existing nodes; the others draw
/// something new (§9.4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Tool {
    Select,
    /// Drag anywhere to pan, without having to find empty canvas or hold space.
    Hand,
    /// Select's gestures, with the handle drag scaling the layer *photographically*
    /// instead of editing its geometry — see [`Scaling`].
    Scale,
    /// Drag across the faces the selection's closed shapes cut each other into (and
    /// its open paths and lines cut further) to merge them, Alt-drag to remove them
    /// (`crate::shape_builder`). It reshapes what is drawn rather than drawing, so it
    /// sits with Select, Hand and Scale, and stays armed after each edit.
    ShapeBuilder,
    /// Drag a line through closed shapes to cut them into the pieces it parts
    /// (`crate::knife`): the selection, or every unlocked shape under the line
    /// when nothing is selected. Shares the shape builder's rail slot, since both
    /// reshape what is drawn, and stays armed after each cut.
    Knife,
    Frame,
    /// Cuts a slice: a box dragged out as a rect's is, on whole pixels always, that
    /// exports whatever lies under it. Stays armed after a cut, so several slices
    /// can be cut in a row.
    Slice,
    Rect,
    Ellipse,
    Polygon,
    Star,
    Line,
    /// Draws an open spiral path out from the press: the drag is the outer radius
    /// and the angle it ends at, `↑`/`↓` add or take half a turn.
    Spiral,
    /// Draws an open circular arc from the press to the release: `↑`/`↓` change
    /// its sweep in [`ondin_core::edit::ARC_STEP_DEG`] steps.
    Arc,
    Pen,
    /// Draws a path freehand: the drag's trail is fitted to curves on release
    /// (`crate::pencil`). Stays armed after a stroke, since freehand comes in runs.
    Pencil,
    /// Edits the *points* of a path rather than the path as a layer — anchors,
    /// their handles, add and remove.
    ///
    /// A tool, not a mode: it has a letter and canvas chrome. The pen is
    /// reachable from inside it (`P` arms `app::pen_bias`), which reroutes no key.
    Node,
    Text,
    /// Picks a colour off the rendered canvas onto the selection
    /// (`crate::eyedropper`). One shot: a pick hands back to the tool it was
    /// armed from (`OndinApp::dropper_return`), unless Shift held it.
    Eyedropper,
    /// Places images the cursor is loaded with, one per click, and disarms itself when it
    /// runs out. Arming it with nothing loaded opens the file dialog, so the rail button
    /// and `Ctrl+Shift+K` are the same action.
    Image,
    /// Image editing: the layer's box becomes a window onto its picture. Dragging a
    /// handle crops, dragging the picture pans it, and a card holds the rest.
    ///
    /// No rail button or letter: it cannot be entered without a picture. It stays a `Tool`
    /// because input routing, the cursor, `chrome_hidden`, the `Escape` ladder and `Enter`
    /// all key off `self.tool`; the subject stays derived (`canvas::edited_image`).
    ImageEdit,
}

impl Tool {
    /// Whether this tool draws by click-dragging a box (as opposed to the
    /// click-driven pen, and Select's own gesture machinery).
    ///
    /// Text is also a click tool: a drag that returns to its start has no box under
    /// `MIN_CREATE_PX`, so the text release plants a caret (`canvas::text_gesture`); a
    /// shape abandons it.
    pub fn drags_a_box(self) -> bool {
        matches!(
            self,
            Tool::Frame
                | Tool::Slice
                | Tool::Rect
                | Tool::Ellipse
                | Tool::Polygon
                | Tool::Star
                | Tool::Line
                | Tool::Spiral
                | Tool::Arc
                | Tool::Text
                // A drag draws the box the image fills; a click places it fitted.
                | Tool::Image
        )
    }

    /// What a plain click with this tool makes, or `None` for a tool whose click
    /// means something else — or nothing.
    ///
    /// Exhaustive with no wildcard, so a new tool is a build error here. Frame and the
    /// line tools answer `None` deliberately: a default frame guesses at a layout, and
    /// a default line, spiral or arc would invent a direction.
    pub fn click_makes(self) -> Option<ClickCreate> {
        match self {
            Tool::Rect | Tool::Ellipse | Tool::Polygon | Tool::Star => {
                Some(ClickCreate::DefaultBox)
            }
            Tool::Image => Some(ClickCreate::FittedImage),
            Tool::Text => Some(ClickCreate::TextNode),
            // A slice's click picks the slice under it (`OndinApp::slice_at`).
            Tool::Frame | Tool::Line | Tool::Spiral | Tool::Arc | Tool::Slice => None,
            Tool::Select
            | Tool::Scale
            | Tool::ShapeBuilder
            | Tool::Knife
            | Tool::Pen
            | Tool::Pencil
            | Tool::Node
            | Tool::ImageEdit
            | Tool::Hand
            | Tool::Eyedropper => None,
        }
    }

    /// Whether this tool picks, moves and transforms existing layers — Select and
    /// Scale, which differ only in what a handle drag *means*.
    ///
    /// The cursor matches `Tool::Select | Tool::Scale` itself, since the two have
    /// different rest states (`canvas::rest_cursor`).
    pub fn selects(self) -> bool {
        matches!(self, Tool::Select | Tool::Scale)
    }

    /// What a handle drag with this tool does to the layer.
    pub fn scaling(self) -> Scaling {
        match self {
            Tool::Scale => Scaling::Photographic,
            _ => Scaling::Geometry,
        }
    }
}

/// What a plain click with a creation tool makes ([`Tool::click_makes`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ClickCreate {
    /// A [`CLICK_CREATE_SIZE`] square carrying the tool's own defaults.
    DefaultBox,
    /// The loaded picture at its intrinsic size, shrunk only if it exceeds the
    /// frame it lands in ([`ondin_core::edit::fitted_image_size`]).
    FittedImage,
    /// A text node that sizes itself to what is typed — or, over existing text, the
    /// caret placed in it.
    TextNode,
}

/// The square a shape tool's *click* gets, in world units so a click makes the same
/// shape at every zoom ([`ClickCreate::DefaultBox`]).
pub const CLICK_CREATE_SIZE: f64 = 100.0;

/// The count `↑`/`↓` step while `shape` is drawn: sides or points, a spiral's half
/// turns, an arc's [`ondin_core::edit::ARC_STEP_DEG`] steps.
pub fn drawn_count(shape: Tool) -> u32 {
    match shape {
        Tool::Star => STAR_DEFAULT_POINTS,
        Tool::Spiral => SPIRAL_DEFAULT_HALF_TURNS,
        Tool::Arc => ARC_DEFAULT_STEPS,
        _ => POLYGON_DEFAULT_SIDES,
    }
}

/// One press of `↑` or `↓` on the [`drawn_count`] of `shape`, held to its range.
pub fn step_drawn_count(shape: Tool, count: u32, up: bool, down: bool) -> u32 {
    let range = match shape {
        Tool::Spiral => 1..=SPIRAL_MAX_HALF_TURNS,
        Tool::Arc => 0..=ARC_MAX_STEPS,
        _ => return step_sides(count, up, down),
    };
    let delta = i64::from(up) - i64::from(down);
    (i64::from(count) + delta).clamp(i64::from(*range.start()), i64::from(*range.end())) as u32
}

/// What the pill says of the [`drawn_count`]: `6 sides`, `5 points`, `3.5 turns`,
/// `90°`. Polygon and star counts start at three, so they are always plural.
pub fn drawn_count_label(shape: Tool, count: u32) -> Option<String> {
    match shape {
        Tool::Polygon => Some(format!("{count} sides")),
        Tool::Star => Some(format!("{count} points")),
        Tool::Spiral => {
            let half = if count % 2 == 1 { ".5" } else { "" };
            let unit = if count == 2 { "turn" } else { "turns" };
            Some(format!("{}{half} {unit}", count / 2))
        }
        Tool::Arc => Some(format!("{}°", arc_sweep(count))),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ondin_core::edit::ARC_SIDE_RUNGS;

    /// `drags_a_box` is a `matches!` list the compiler does not check against intent:
    /// Text is in it, and the selecting and editing tools are not.
    #[test]
    fn text_both_drags_a_box_and_stays_a_click_tool() {
        assert!(Tool::Text.drags_a_box());
        for tool in [
            Tool::Frame,
            Tool::Rect,
            Tool::Ellipse,
            Tool::Polygon,
            Tool::Star,
            Tool::Line,
            Tool::Spiral,
            Tool::Arc,
        ] {
            assert!(tool.drags_a_box(), "{tool:?}");
        }
        // This predicate also decides which tools clear the selection when armed;
        // `Node` and `ImageEdit` are entered from a selection, so a wrong answer would
        // empty the tool as it opened.
        for tool in [
            Tool::Pen,
            Tool::Node,
            Tool::ImageEdit,
            Tool::Select,
            Tool::Scale,
            Tool::ShapeBuilder,
            Tool::Knife,
            Tool::Hand,
        ] {
            assert!(!tool.drags_a_box(), "{tool:?}");
        }
    }

    /// The whole click table, tool by tool, and the containment that anything
    /// answering a click also drags a box. The `match` in `click_makes` makes a missing
    /// arm a build error; this makes a wrong arm a failure.
    #[test]
    fn every_tool_answers_a_click_with_exactly_what_it_should() {
        use ClickCreate::*;
        for (tool, want) in [
            (Tool::Rect, Some(DefaultBox)),
            (Tool::Ellipse, Some(DefaultBox)),
            (Tool::Polygon, Some(DefaultBox)),
            (Tool::Star, Some(DefaultBox)),
            (Tool::Image, Some(FittedImage)),
            (Tool::Text, Some(TextNode)),
            // Box tools deliberately without a click answer, then non-creation tools.
            (Tool::Frame, None),
            (Tool::Line, None),
            (Tool::Spiral, None),
            (Tool::Arc, None),
            (Tool::Select, None),
            (Tool::Scale, None),
            (Tool::ShapeBuilder, None),
            (Tool::Knife, None),
            (Tool::Pen, None),
            (Tool::Node, None),
            (Tool::ImageEdit, None),
            (Tool::Hand, None),
        ] {
            assert_eq!(tool.click_makes(), want, "{tool:?}");
            if want.is_some() {
                assert!(
                    tool.drags_a_box(),
                    "{tool:?} answers a click but cannot be dragged out, so it has \
                     only one way to be given a size"
                );
            }
        }
    }

    /// The keys walk each count through its range and stop at the ends.
    #[test]
    fn the_drawn_counts_stop_at_their_ends() {
        assert_eq!(step_drawn_count(Tool::Spiral, 1, false, true), 1);
        assert_eq!(step_drawn_count(Tool::Spiral, 7, true, false), 8);
        assert_eq!(
            step_drawn_count(Tool::Spiral, SPIRAL_MAX_HALF_TURNS, true, false),
            SPIRAL_MAX_HALF_TURNS
        );
        assert_eq!(step_drawn_count(Tool::Arc, 0, false, true), 0);
        assert_eq!(
            step_drawn_count(Tool::Arc, ARC_MAX_STEPS, true, false),
            ARC_MAX_STEPS
        );
        assert_eq!(
            step_drawn_count(Tool::Polygon, 3, false, true),
            step_sides(3, false, true)
        );
        assert_eq!(drawn_count(Tool::Spiral), 7);
        assert_eq!(drawn_count(Tool::Arc), ARC_DEFAULT_STEPS);
    }

    /// The pill says sides, points, turns in halves and the sweep in degrees; other
    /// tools say nothing.
    #[test]
    fn the_pill_names_turns_and_sweep() {
        assert_eq!(
            drawn_count_label(Tool::Spiral, 7).as_deref(),
            Some("3.5 turns")
        );
        assert_eq!(
            drawn_count_label(Tool::Spiral, 2).as_deref(),
            Some("1 turn")
        );
        assert_eq!(
            drawn_count_label(Tool::Spiral, 1).as_deref(),
            Some("0.5 turns")
        );
        assert_eq!(
            drawn_count_label(Tool::Arc, ARC_DEFAULT_STEPS).as_deref(),
            Some("90°")
        );
        assert_eq!(
            drawn_count_label(Tool::Arc, ARC_SIDE_RUNGS - 1).as_deref(),
            Some("-5°")
        );
        assert_eq!(
            drawn_count_label(Tool::Polygon, 5).as_deref(),
            Some("5 sides")
        );
        assert_eq!(
            drawn_count_label(Tool::Star, 3).as_deref(),
            Some("3 points")
        );
        assert_eq!(drawn_count_label(Tool::Rect, 5), None);
    }
}
