//! The tool rail's grouped slots: one button standing for several tools, showing the
//! armed member or else the one last picked, with a corner mark saying it holds
//! more. A held press or a right-click opens the members in a row beside it.
//!
//! The pointer's two members are Select with the marquee's two shapes, since the
//! lasso is the marquee's shape for Select and the Node tool alike (`crate::lasso`),
//! not a tool; its switch is the pointer slot's memory.

use crate::app::OndinApp;
use crate::input::ViewSwitch;
use crate::theme::{self, icon};
use crate::tools::Tool;
use crate::ui;
use eframe::egui;

/// How long a press on a grouped slot is held before its members open.
pub(crate) const HOLD_S: f64 = 0.45;

/// The members' row: its buttons, the room between them, and the card's padding.
const BUTTON: f32 = 30.0;
const GAP: f32 = 2.0;
const PAD: f32 = 4.0;

/// A rail slot holding more than one tool.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Group {
    Select,
    Frame,
    Pen,
}

/// One tool a grouped slot can stand for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) enum Member {
    Pointer,
    Lasso,
    Frame,
    Slice,
    Pen,
    Pencil,
}

impl Group {
    /// The members in the row's order; the first is what the slot shows until
    /// another is picked.
    pub(crate) fn members(self) -> [Member; 2] {
        match self {
            Self::Select => [Member::Pointer, Member::Lasso],
            Self::Frame => [Member::Frame, Member::Slice],
            Self::Pen => [Member::Pen, Member::Pencil],
        }
    }
}

impl Member {
    fn glyph(self) -> &'static str {
        match self {
            Self::Pointer => icon::CURSOR,
            Self::Lasso => icon::LASSO,
            Self::Frame => icon::FRAME,
            Self::Slice => icon::KNIFE,
            Self::Pen => icon::PEN_NIB,
            Self::Pencil => icon::PENCIL_SIMPLE,
        }
    }

    pub(crate) fn tip(self) -> &'static str {
        match self {
            Self::Pointer => "Select (V)",
            Self::Lasso => "Lasso select (Q)",
            Self::Frame => "Frame (F)",
            Self::Slice => "Slice — drag out a region to export (Shift+F)",
            Self::Pen => "Pen (P)",
            Self::Pencil => "Pencil (Shift+P)",
        }
    }
}

/// The members' row while it is open.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Flyout {
    pub(crate) group: Group,
    /// The slot it opened from, which it sits beside.
    at: egui::Rect,
    /// Opened by a press still held: letting go over a member picks it.
    held: bool,
}

/// The grouped slots' state. The pointer slot remembers through the lasso switch.
#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct Rail {
    /// The frame slot shows the slice when neither is armed.
    pub(crate) slice: bool,
    /// The pen slot shows the pencil when neither is armed.
    pub(crate) pencil: bool,
    pub(crate) flyout: Option<Flyout>,
}

impl OndinApp {
    /// The member a slot shows: the armed tool when it is one, else the last picked.
    pub(crate) fn rail_shown(&self, group: Group) -> Member {
        match group {
            Group::Select if self.lasso => Member::Lasso,
            Group::Select => Member::Pointer,
            Group::Frame => match self.tool {
                Tool::Frame => Member::Frame,
                Tool::Slice => Member::Slice,
                _ if self.rail.slice => Member::Slice,
                _ => Member::Frame,
            },
            // The pen armed inside a node edit is the pen, whatever was last picked.
            Group::Pen if self.pen_bias || self.tool == Tool::Pen => Member::Pen,
            Group::Pen if self.tool == Tool::Pencil || self.rail.pencil => Member::Pencil,
            Group::Pen => Member::Pen,
        }
    }

    /// Whether the canvas is armed with `member`.
    fn rail_armed(&self, member: Member) -> bool {
        match member {
            Member::Pointer => self.tool == Tool::Select && !self.lasso,
            Member::Lasso => self.tool == Tool::Select && self.lasso,
            Member::Frame => self.tool == Tool::Frame,
            Member::Slice => self.tool == Tool::Slice,
            Member::Pen => self.tool == Tool::Pen || self.pen_bias,
            Member::Pencil => self.tool == Tool::Pencil,
        }
    }

    /// Arm `member`: its tool, and for the pointer's two the marquee's shape.
    pub(crate) fn rail_pick(&mut self, member: Member) {
        let tool = match member {
            Member::Pointer | Member::Lasso => {
                self.choose_tool(Tool::Select);
                let lasso = member == Member::Lasso;
                if self.lasso != lasso {
                    self.set_view_switch(ViewSwitch::Lasso, lasso);
                }
                return;
            }
            Member::Frame => Tool::Frame,
            Member::Slice => Tool::Slice,
            Member::Pen => Tool::Pen,
            Member::Pencil => Tool::Pencil,
        };
        self.choose_tool(tool);
    }

    /// Note `tool` as its slot's member, however it was armed: the rail, its
    /// letter or the palette.
    pub(crate) fn remember_rail_pick(&mut self, tool: Tool) {
        match tool {
            Tool::Frame => self.rail.slice = false,
            Tool::Slice => self.rail.slice = true,
            Tool::Pen => self.rail.pencil = false,
            Tool::Pencil => self.rail.pencil = true,
            _ => {}
        }
    }

    /// A grouped slot: a click arms what it shows, a press held [`HOLD_S`], a
    /// right-click or `Alt+↓` opens its members beside it.
    pub(crate) fn rail_group(&mut self, ui: &mut egui::Ui, group: Group) -> egui::Response {
        let shown = self.rail_shown(group);
        let mut resp = ui::icon_button(
            ui,
            shown.glyph(),
            BUTTON,
            20.0,
            self.rail_armed(shown),
            true,
        );
        paint_more_mark(ui, resp.rect);

        let (now, pressed_at) = ui.input(|i| (i.time, i.pointer.press_start_time()));
        let held = resp.is_pointer_button_down_on() && ui.input(|i| i.pointer.primary_down());
        let open = self.rail.flyout.is_some_and(|f| f.group == group);
        if held && !open {
            let since = pressed_at.map_or(0.0, |t| now - t);
            if since >= HOLD_S {
                self.rail.flyout = Some(Flyout {
                    group,
                    at: resp.rect,
                    held: true,
                });
            } else {
                ui.ctx()
                    .request_repaint_after(std::time::Duration::from_secs_f64(HOLD_S - since));
            }
        }
        // `Alt+↓` on the focused slot opens it from the keys, as a dropdown opens.
        if resp.secondary_clicked() || ui::list_key(ui, &resp) {
            self.rail.flyout = Some(Flyout {
                group,
                at: resp.rect,
                held: false,
            });
        } else if resp.clicked() {
            // The release ending a hold that opened the row is not a pick.
            let ends_hold = self.rail.flyout.is_some_and(|f| f.group == group && f.held);
            if !ends_hold {
                self.rail.flyout = None;
                self.rail_pick(shown);
            }
        }
        // A tooltip under the open row, or over a press about to open it, would fight it.
        if !held && !open {
            resp = resp.on_hover_text(shown.tip());
        }
        resp
    }

    /// The open slot's members in a row beside it, the shown one lit. A click picks
    /// one; so does letting go over one at the end of the hold that opened it.
    /// Escape or a press anywhere but the row and its slot closes it.
    pub(crate) fn rail_flyout(&mut self, ctx: &egui::Context) {
        let Some(mut flyout) = self.rail.flyout else {
            return;
        };
        let members = flyout.group.members();
        let shown = self.rail_shown(flyout.group);
        let n = members.len() as f32;
        let edge = 2.0 * (PAD + ui::MENU_BORDER);
        let size = egui::vec2(n * BUTTON + (n - 1.0) * GAP + edge, BUTTON + edge);
        let pos = ui::flyout_place(ctx, flyout.at, size, PAD);
        let pointer = ctx.input(|i| i.pointer.interact_pos());
        // The member under a held drag, from the row's geometry: the press belongs
        // to the slot, so no button in the row reads as hovered.
        let first = pos + egui::Vec2::splat(PAD + ui::MENU_BORDER);
        let dragged_over = members.into_iter().enumerate().find_map(|(i, m)| {
            let at = egui::Rect::from_min_size(
                first + egui::vec2(i as f32 * (BUTTON + GAP), 0.0),
                egui::Vec2::splat(BUTTON),
            );
            (flyout.held && pointer.is_some_and(|p| at.contains(p))).then_some(m)
        });
        let mut picked = None;
        let area = egui::Area::new(egui::Id::new("rail-flyout"))
            .order(egui::Order::Foreground)
            .fixed_pos(pos)
            .show(ctx, |ui| {
                ui::hold_tab(ui);
                ui::menu_frame(PAD).show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = GAP;
                        for m in members {
                            let lit = dragged_over.map_or(m == shown, |d| d == m);
                            let resp = ui::icon_button(ui, m.glyph(), BUTTON, 20.0, lit, true)
                                .on_hover_text(m.tip());
                            if resp.clicked() {
                                picked = Some(m);
                            }
                        }
                    });
                });
            })
            .response;

        let (released, pressed, escape) = ctx.input(|i| {
            (
                i.pointer.primary_released(),
                i.pointer.any_pressed(),
                i.key_pressed(egui::Key::Escape),
            )
        });
        if flyout.held && released {
            picked = picked.or(dragged_over);
            flyout.held = false;
        }
        if let Some(m) = picked {
            self.rail.flyout = None;
            self.rail_pick(m);
            return;
        }
        let elsewhere =
            pressed && !area.contains_pointer() && !pointer.is_some_and(|p| flyout.at.contains(p));
        self.rail.flyout = (!escape && !elsewhere).then_some(flyout);
    }
}

/// The corner mark on a grouped slot: a small triangle in its bottom right.
fn paint_more_mark(ui: &egui::Ui, slot: egui::Rect) {
    const SIDE: f32 = 4.0;
    let corner = slot.right_bottom() - egui::vec2(3.0, 3.0);
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            corner,
            corner - egui::vec2(0.0, SIDE),
            corner - egui::vec2(SIDE, 0.0),
        ],
        theme::text::dim(),
        egui::Stroke::NONE,
    ));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::input::Action;

    /// A headless app with one grouped slot drawn per frame, on a clock the test sets.
    struct Rig {
        ctx: egui::Context,
        app: OndinApp,
        group: Group,
        slot: egui::Rect,
    }

    impl Rig {
        fn new(group: Group) -> Self {
            let ctx = egui::Context::default();
            theme::install(&ctx);
            let app = OndinApp::headless(&ctx);
            let mut rig = Self {
                ctx,
                app,
                group,
                slot: egui::Rect::NOTHING,
            };
            rig.frame(0.0, Vec::new());
            rig.frame(0.0, Vec::new());
            rig
        }

        fn frame(&mut self, time: f64, events: Vec<egui::Event>) {
            let input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(800.0, 600.0),
                )),
                time: Some(time),
                events,
                ..Default::default()
            };
            let (app, group) = (&mut self.app, self.group);
            let mut slot = egui::Rect::NOTHING;
            let _ = self.ctx.run_ui(input, |ui| {
                slot = app.rail_group(ui, group).rect;
                app.rail_flyout(ui.ctx());
            });
            self.slot = slot;
        }

        /// The centre of the open row's `i`th member.
        fn member(&self, i: usize) -> egui::Pos2 {
            let card = self
                .ctx
                .memory(|m| m.area_rect(egui::Id::new("rail-flyout")))
                .expect("the row is open");
            card.min
                + egui::Vec2::splat(PAD + ui::MENU_BORDER + BUTTON / 2.0)
                + egui::vec2(i as f32 * (BUTTON + GAP), 0.0)
        }
    }

    fn button(pos: egui::Pos2, button: egui::PointerButton, pressed: bool) -> egui::Event {
        egui::Event::PointerButton {
            pos,
            button,
            pressed,
            modifiers: Default::default(),
        }
    }

    fn press(pos: egui::Pos2, pressed: bool) -> egui::Event {
        button(pos, egui::PointerButton::Primary, pressed)
    }

    /// A quick click arms what the slot shows and opens nothing.
    #[test]
    fn a_click_arms_the_slot_s_tool() {
        let mut rig = Rig::new(Group::Frame);
        let at = rig.slot.center();
        rig.frame(1.0, vec![egui::Event::PointerMoved(at), press(at, true)]);
        rig.frame(1.1, vec![press(at, false)]);
        assert_eq!(rig.app.tool, Tool::Frame);
        assert!(rig.app.rail.flyout.is_none(), "a click opens no row");
    }

    /// A press held `HOLD_S` opens the row; letting go on the slot leaves it open;
    /// letting go of a later hold over a member picks it, and the slot then shows it.
    ///
    /// flip: `rail_group` without its `ends_hold` guard → fails at "letting go on
    /// the slot leaves the row open".
    #[test]
    fn a_held_press_opens_the_row_and_a_release_over_a_member_picks_it() {
        let mut rig = Rig::new(Group::Select);
        rig.app.choose_tool(Tool::Rect);
        assert!(!rig.app.lasso, "the fixture starts on a box marquee");
        let at = rig.slot.center();
        rig.frame(1.0, vec![egui::Event::PointerMoved(at), press(at, true)]);
        rig.frame(1.0 + HOLD_S / 2.0, Vec::new());
        assert!(rig.app.rail.flyout.is_none(), "not before the hold");
        rig.frame(1.0 + HOLD_S + 0.01, Vec::new());
        assert!(rig.app.rail.flyout.is_some(), "the hold opens the row");
        rig.frame(1.6, vec![press(at, false)]);
        rig.frame(1.7, Vec::new());
        assert!(
            rig.app.rail.flyout.is_some(),
            "letting go on the slot leaves the row open"
        );
        assert_eq!(rig.app.tool, Tool::Rect, "and arms nothing");

        // Closed by a press elsewhere, then a second hold dragged onto the lasso.
        let away = egui::pos2(600.0, 500.0);
        rig.frame(
            2.0,
            vec![egui::Event::PointerMoved(away), press(away, true)],
        );
        rig.frame(2.1, vec![press(away, false)]);
        assert!(rig.app.rail.flyout.is_none(), "a press elsewhere closes it");
        rig.frame(3.0, vec![egui::Event::PointerMoved(at), press(at, true)]);
        rig.frame(3.0 + HOLD_S + 0.01, Vec::new());
        let lasso = rig.member(1);
        rig.frame(3.6, vec![egui::Event::PointerMoved(lasso)]);
        rig.frame(3.7, vec![press(lasso, false)]);
        assert_eq!(rig.app.tool, Tool::Select);
        assert!(rig.app.lasso, "the release picked the lasso");
        assert!(rig.app.rail.flyout.is_none());
        assert_eq!(rig.app.rail_shown(Group::Select), Member::Lasso);
    }

    /// A right-click opens the row at once; a click in it picks, and the slot keeps
    /// showing that member after another tool is armed, until its letter picks the
    /// first one back.
    #[test]
    fn a_right_click_opens_the_row_and_the_slot_remembers_its_pick() {
        let mut rig = Rig::new(Group::Frame);
        let at = rig.slot.center();
        let right = egui::PointerButton::Secondary;
        rig.frame(
            1.0,
            vec![egui::Event::PointerMoved(at), button(at, right, true)],
        );
        rig.frame(1.1, vec![button(at, right, false)]);
        assert!(rig.app.rail.flyout.is_some(), "a right-click opens the row");

        let slice = rig.member(1);
        rig.frame(1.2, vec![egui::Event::PointerMoved(slice)]);
        rig.frame(1.3, vec![press(slice, true)]);
        rig.frame(1.4, vec![press(slice, false)]);
        assert_eq!(rig.app.tool, Tool::Slice);
        assert!(rig.app.rail.flyout.is_none(), "a pick closes the row");

        rig.app.choose_tool(Tool::Rect);
        assert_eq!(
            rig.app.rail_shown(Group::Frame),
            Member::Slice,
            "remembered"
        );
        let ctx = rig.ctx.clone();
        rig.app.dispatch(&ctx, Action::ChooseTool(Tool::Frame));
        assert_eq!(
            rig.app.rail_shown(Group::Frame),
            Member::Frame,
            "`F` takes it back"
        );
    }

    /// `V` is the pointer with a box marquee; `Q` flips the marquee and arms Select
    /// from a drawing tool, but leaves the Node tool where it is.
    ///
    /// flip: the `ChooseTool(Tool::Select)` arm of `dispatch` removed → fails at
    /// "`V` turns the lasso off".
    #[test]
    fn v_is_the_box_pointer_and_q_arms_the_lasso() {
        let mut rig = Rig::new(Group::Select);
        let ctx = rig.ctx.clone();
        rig.app.choose_tool(Tool::Rect);
        rig.app
            .dispatch(&ctx, Action::ToggleView(ViewSwitch::Lasso));
        assert_eq!(rig.app.tool, Tool::Select, "`Q` arms Select");
        assert!(rig.app.lasso);

        rig.app.dispatch(&ctx, Action::ChooseTool(Tool::Select));
        assert!(!rig.app.lasso, "`V` turns the lasso off");

        rig.app.tool = Tool::Node;
        rig.app
            .dispatch(&ctx, Action::ToggleView(ViewSwitch::Lasso));
        assert_eq!(rig.app.tool, Tool::Node, "the node tool keeps its marquee");
        assert!(rig.app.lasso);
    }

    /// Escape closes the open row without arming anything.
    #[test]
    fn escape_closes_the_row() {
        let mut rig = Rig::new(Group::Pen);
        let at = rig.slot.center();
        let right = egui::PointerButton::Secondary;
        rig.frame(
            1.0,
            vec![egui::Event::PointerMoved(at), button(at, right, true)],
        );
        rig.frame(1.1, vec![button(at, right, false)]);
        assert!(rig.app.rail.flyout.is_some());
        let key = egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        rig.frame(1.2, vec![key]);
        assert!(rig.app.rail.flyout.is_none());
        assert_eq!(rig.app.tool, Tool::Select);
    }

    /// One frame of the whole app, keymap included.
    fn app_frame(ctx: &egui::Context, app: &mut OndinApp, events: Vec<egui::Event>) {
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

    fn open_row(group: Group) -> Option<Flyout> {
        Some(Flyout {
            group,
            at: egui::Rect::from_min_size(egui::pos2(8.0, 200.0), egui::vec2(32.0, 32.0)),
            held: false,
        })
    }

    /// Escape with a members row open closes the row and reaches nothing else:
    /// the selection and the armed tool stay.
    /// flip: removing the gate's `rail.flyout` arm in `ui` → fails at the selection
    /// assertion.
    #[test]
    fn escape_on_the_row_leaves_the_selection_and_the_tool() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let id = crate::palette::tests::add_rect(&mut app);
        app.session.selection.set_one(id);
        app_frame(&ctx, &mut app, Vec::new());
        assert_eq!(
            app.session.selection.ids(),
            [id],
            "the fixture keeps its selection"
        );
        let escape = || egui::Event::Key {
            key: egui::Key::Escape,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: Default::default(),
        };
        app.rail.flyout = open_row(Group::Pen);
        app_frame(&ctx, &mut app, vec![escape()]);
        assert!(app.rail.flyout.is_none());
        assert_eq!(app.session.selection.ids(), [id]);

        app.choose_tool(Tool::Rect);
        app.rail.flyout = open_row(Group::Frame);
        app_frame(&ctx, &mut app, vec![escape()]);
        assert!(app.rail.flyout.is_none());
        assert_eq!(app.tool, Tool::Rect);
    }

    /// A members row open when focus mode hides the rail is closed when it returns.
    /// flip: dropping the focus arm's `rail.flyout = None` in `ui` → fails at the
    /// last assertion.
    #[test]
    fn hiding_the_rail_closes_its_row() {
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        app_frame(&ctx, &mut app, Vec::new());
        app.rail.flyout = open_row(Group::Frame);
        app.focus = true;
        app_frame(&ctx, &mut app, Vec::new());
        app.focus = false;
        app_frame(&ctx, &mut app, Vec::new());
        assert!(app.rail.flyout.is_none());
    }
}
