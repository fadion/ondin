//! The assistant's panel: a `ui::rail_popup` at the rail's foot, as tall as the
//! room above its button less [`TOP_AIR`], with a drag edge on its right. Head, thread,
//! composer. Drawn into a list of acts that run once the drawing is done.

use super::markdown::{self, Act as MdAct};
use super::thread::{Chip, Msg, Note, Part, ToolRow, ToolState};
use super::{Level, MAX_W, MIN_W, TOOLS_SHOWN, Why, ai};
use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use crate::ui;

/// The gap between messages in the thread.
const MSG_GAP: f32 = 14.0;
/// A tool row's height.
const TOOL_H: f32 = 22.0;
/// The head's height: its dropdown's.
const HEAD_H: f32 = ui::CONTROL_H;
/// The least the panel shrinks to: the head, a line of thread and the composer.
const FLOOR_H: f32 = 2.0 * ui::RAIL_POPUP_PAD + HEAD_H + 10.0 + 40.0 + 10.0 + 96.0;
/// A question's bubble at most this share of the thread's width.
const BUBBLE_SHARE: f32 = 0.82;
/// The drag edge's grab width, over the panel's right edge.
const EDGE_GRAB: f32 = 6.0;
/// The send button.
const SEND_W: f32 = 26.0;
/// Clear air between the panel's top and the canvas's: room to see what is
/// being asked about.
const TOP_AIR: f32 = 50.0;
/// Above an empty thread's empty state: its rail siblings' air.
const EMPTY_AIR: f32 = 40.0;

/// What the panel asked for this frame.
enum Act {
    Send(String, Vec<Chip>),
    /// An option of the last reply, sent as the next question.
    Choose(String),
    Stop,
    NewChat,
    Level(Level),
    Select(String),
    Hover(String),
    RemoveChip(usize),
    AskAgain(usize),
    Regenerate,
    OpenSettings,
    Close,
    Width(f32),
    WidthDone,
    Apply(u64),
    Undo(u64),
    Retry(u64, String),
}

impl OndinApp {
    /// The rail button's tooltip: what the marks on it mean.
    pub(crate) fn assistant_tip(&self) -> String {
        if self.prefs.private {
            return "Not available in private mode".into();
        }
        let base = "Assistant (Ctrl+Shift+A)";
        if self.assistant.running() && !self.assistant.open {
            return format!("{base} · working");
        }
        if self.assistant.unseen {
            return "Assistant · reply waiting".into();
        }
        base.into()
    }

    /// The rail button's marks: a ring while a turn runs out of sight, a dot
    /// while a reply waits unseen.
    pub(crate) fn paint_assistant_marks(&self, ui: &egui::Ui, button: egui::Rect) {
        let at = egui::pos2(button.right() - 6.5, button.top() + 6.5);
        let p = ui.painter();
        if self.assistant.running() && !self.assistant.open {
            p.circle_filled(at, 4.5, color::card());
            p.circle_stroke(at, 2.75, egui::Stroke::new(1.5, color::accent_300()));
        } else if self.assistant.unseen {
            p.circle_filled(at, 5.5, color::card());
            p.circle_filled(at, 3.5, color::accent_300());
        }
    }

    /// Draw the panel when it is open, and do what it asked.
    pub(crate) fn assistant_ui(&mut self, ctx: &egui::Context) {
        if !self.assistant.open {
            self.assistant.hovered = None;
            return;
        }
        let anchor = self.rail_popup_anchor(self.assistant_button);
        // Before the first frame has laid the canvas out.
        if !anchor.is_finite() {
            return;
        }
        let height = (anchor.y - self.canvas_rect.top() - TOP_AIR).max(FLOOR_H);
        let width = self.prefs.assistant.width.clamp(MIN_W, MAX_W);
        let blocked = self.assistant_blocked();
        let level = self.assistant_level();
        let harness = self.prefs.assistant.harness();
        let seal = self
            .assistant
            .found()
            .map(|f| (f.seal, f.seal_sentence.clone()));
        let now = ctx.input(|i| i.time);
        let mut acts: Vec<Act> = Vec::new();

        let resp = ui::rail_popup(ctx, "assistant-panel", anchor, height, |ui| {
            let inner = width - 2.0 * ui::RAIL_POPUP_PAD;
            ui.set_width(inner);
            ui.set_height(height - 2.0 * ui::RAIL_POPUP_PAD - 2.0);
            ui.spacing_mut().item_spacing.y = 10.0;
            self.assistant_head(ui, harness, seal.as_ref(), level, &mut acts);
            ui::card_rule(ui);
            // Last frame's composer with the gap above it, so the thread leaves
            // it exactly its room and the card's foot keeps the card's inset.
            let composer_id = ui.id().with("composer-h");
            let composer_h: f32 = ui.data(|d| d.get_temp(composer_id)).unwrap_or(106.0);
            let thread_h = (ui.available_height() - composer_h).max(40.0);
            // The thread's last room, content height and whether it was held
            // at its end. egui keeps a scroll at its end only after painting,
            // so when the room changes under a thread held there, its offset
            // is set here or the messages lag the composer by a frame.
            let fit_id = ui.id().with("thread-fit");
            let fit: Option<(f32, f32, bool)> = ui.data(|d| d.get_temp(fit_id));
            let mut scroll = egui::ScrollArea::vertical()
                .id_salt("assistant-thread")
                .auto_shrink([false, false])
                .stick_to_bottom(true)
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden);
            if let Some((room, content, true)) = fit
                && (room - thread_h).abs() > 0.5
            {
                scroll = scroll.vertical_scroll_offset((content - thread_h).max(0.0));
            }
            let out = ui
                .allocate_ui(egui::vec2(inner, thread_h), |ui| {
                    ui.set_min_height(thread_h);
                    scroll.show(ui, |ui| {
                        ui.set_width(inner);
                        self.assistant_thread(ui, now, &mut acts);
                    })
                })
                .inner;
            let room = out.inner_rect.height();
            let at_end = out.state.offset.y >= out.content_size.y - room - 0.5;
            ui.data_mut(|d| d.insert_temp(fit_id, (room, out.content_size.y, at_end)));
            let top = ui.cursor().top();
            self.assistant_composer(ui, blocked.as_ref(), &mut acts);
            let used = ui.cursor().top() - top;
            // A composer that changed height (a chip, a line) left the thread
            // the wrong room: lay the pass out again before it is shown, or the
            // thread and the composer's foot jump for a frame.
            if (used - composer_h).abs() > 0.5 {
                ui.ctx().request_discard("assistant composer height");
            }
            ui.data_mut(|d| d.insert_temp(composer_id, used));
        });

        // The drag edge on the panel's right.
        let rect = resp.rect;
        let edge = egui::Rect::from_min_max(
            egui::pos2(rect.right() - EDGE_GRAB / 2.0, rect.top() + 12.0),
            egui::pos2(rect.right() + EDGE_GRAB / 2.0, rect.bottom() - 12.0),
        );
        // Lit as the layers panel's edge is: a hairline, after the same dwell,
        // so a pointer merely crossing it does not light it.
        let (pos, pressed, down) = ctx.input(|i| {
            (
                i.pointer.hover_pos(),
                i.pointer.button_pressed(egui::PointerButton::Primary),
                i.pointer.button_down(egui::PointerButton::Primary),
            )
        });
        let in_band = pos.is_some_and(|p| edge.contains(p));
        self.assistant.edge = self.assistant.edge.advance(now, in_band, pressed, down);
        if let Some(due) = self.assistant.edge.due() {
            ctx.request_repaint_after(std::time::Duration::from_secs_f64((due - now).max(0.0)));
        }
        let lit = self.assistant.edge.lit();
        let edge_resp = egui::Area::new(egui::Id::new("assistant-edge"))
            .order(egui::Order::Foreground)
            .fixed_pos(edge.min)
            .show(ctx, |ui| {
                let (r, resp) = ui.allocate_exact_size(edge.size(), egui::Sense::drag());
                if lit || resp.dragged() {
                    let ink = match resp.dragged() {
                        true => color::accent(),
                        false => color::accent_700(),
                    };
                    // Over the card's own border, which is inside its rect.
                    let ppp = ui.ctx().pixels_per_point();
                    let x = ui::hairline_in_column(r.center().x - 0.5, ppp, 1.0);
                    ui.painter()
                        .vline(x, r.y_range(), egui::Stroke::new(1.0, ink));
                }
                resp.on_hover_cursor(egui::CursorIcon::ResizeHorizontal)
            })
            .inner;
        if edge_resp.dragged() {
            acts.push(Act::Width(
                (width + edge_resp.drag_delta().x).clamp(MIN_W, MAX_W),
            ));
        }
        if edge_resp.drag_stopped() {
            acts.push(Act::WidthDone);
        }
        if self.assistant.running() {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
        }
        self.assistant.hovered = None;
        for act in acts {
            self.assistant_act(act);
        }
    }

    /// Run one of the panel's acts.
    fn assistant_act(&mut self, act: Act) {
        match act {
            Act::Send(text, chips) => {
                if self.assistant_send(text, chips) {
                    self.assistant.draft.clear();
                    self.assistant.chips.clear();
                }
            }
            Act::Choose(text) => {
                // An option answers with the chips its question carried.
                let chips = self
                    .assistant
                    .msgs
                    .iter()
                    .rev()
                    .find_map(|m| match m {
                        Msg::Person { chips, .. } => Some(chips.clone()),
                        _ => None,
                    })
                    .unwrap_or_default();
                self.assistant_send(text, chips);
            }
            Act::Stop => self.assistant_stop(),
            Act::NewChat => self.assistant_new_chat(),
            Act::Level(l) => self.assistant_set_level(l),
            Act::Select(id) => self.assistant_select(&id),
            Act::Hover(id) => self.assistant.hovered = Some(id),
            Act::RemoveChip(i) => {
                if i < self.assistant.chips.len() {
                    self.assistant.chips.remove(i);
                }
            }
            Act::AskAgain(q) => self.assistant_ask_again(q),
            Act::Regenerate => self.assistant_regenerate(),
            Act::OpenSettings => {
                self.assistant.open = false;
                let ctx = self.egui_ctx.clone();
                self.toggle_settings(&ctx, Some(crate::settings::SettingsTab::Assistant));
            }
            Act::Close => self.assistant.open = false,
            Act::Width(w) => self.prefs.assistant.width = w,
            Act::WidthDone => self.prefs.save(),
            Act::Apply(id) => self.assistant_apply(id),
            Act::Undo(id) => self.assistant_undo(id),
            Act::Retry(id, name) => {
                // The layers it writes that are still there go with the question.
                let writes: Vec<String> = self
                    .mcp
                    .as_ref()
                    .and_then(|h| h.proposal(id))
                    .map(|p| p.writes.iter().map(|w| w.to_wire()).collect())
                    .unwrap_or_default();
                let chips = self.selection_chips_of(&writes);
                self.assistant_send(format!("{name} changed. Propose again."), chips);
            }
        }
    }

    /// The head: the harness and its seal, then Access and *New chat*.
    fn assistant_head(
        &self,
        ui: &mut egui::Ui,
        harness: Option<ai::Harness>,
        seal: Option<&(ai::Seal, String)>,
        level: Level,
        acts: &mut Vec<Act>,
    ) {
        ui.allocate_ui_with_layout(
            egui::vec2(ui.available_width(), HEAD_H),
            egui::Layout::left_to_right(egui::Align::Center),
            |ui| {
                ui.spacing_mut().item_spacing.x = 8.0;
                let name = harness.map_or("Assistant", |h| h.label());
                ui.label(
                    egui::RichText::new(name)
                        .size(ui::CONTROL_PT)
                        .color(color::text()),
                );
                if let Some((grade, sentence)) = seal {
                    let word = match grade {
                        ai::Seal::Sealed => "Sealed",
                        ai::Seal::Restricted => "Restricted",
                        ai::Seal::Unknown => "Not sealed",
                    };
                    ui.label(ui::glyph_and_text_tinted(
                        icon::LOCK_SIMPLE,
                        word,
                        theme::text::faint(),
                    ))
                    .on_hover_text(sentence.as_str());
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui::icon_button(ui, icon::PLUS, 24.0, 15.0, false, true)
                        .on_hover_text("New chat")
                        .clicked()
                    {
                        acts.push(Act::NewChat);
                    }
                    ui.spacing_mut().interact_size.y = ui::CONTROL_H;
                    ui.spacing_mut().button_padding.y = 0.0;
                    let mut picked = level;
                    egui::ComboBox::from_id_salt("assistant-access")
                        .icon(ui::combo_chevron)
                        .selected_text(ui::combo_text(level.label()))
                        .show_ui(ui, |ui| {
                            ui::menu_rows(ui);
                            for l in Level::ALL {
                                ui.selectable_value(&mut picked, l, l.label())
                                    .on_hover_text(level_tip(l));
                            }
                        })
                        .response
                        .on_hover_text("What the agent may do in this chat");
                    if picked != level {
                        acts.push(Act::Level(picked));
                    }
                });
            },
        );
    }

    /// Every message, the live turn's stage under its parts.
    fn assistant_thread(&self, ui: &mut egui::Ui, now: f64, acts: &mut Vec<Act>) {
        ui.spacing_mut().item_spacing.y = MSG_GAP;
        let msgs = &self.assistant.msgs;
        if msgs.is_empty() {
            ui.add_space(EMPTY_AIR);
            ui::empty_state(
                ui,
                Some(icon::SPARKLE),
                "Ask about the design in front of you",
                "Attach layers with the button over them or Ask the assistant on their menu",
                |_| {},
            );
            return;
        }
        let live = self.assistant.live_turn();
        let last_agent = msgs.iter().rposition(|m| matches!(m, Msg::Agent { .. }));
        let last_person = msgs.iter().rposition(|m| matches!(m, Msg::Person { .. }));
        for (i, m) in msgs.iter().enumerate() {
            match m {
                Msg::Person { text, chips } => self.person_bubble(ui, text, chips, acts),
                Msg::Agent {
                    speaker,
                    parts,
                    stats,
                    ..
                } => {
                    let is_live = live.is_some_and(|(msg, ..)| msg == i);
                    let last = Some(i) == last_agent && last_person < Some(i);
                    ui.vertical(|ui| {
                        ui.spacing_mut().item_spacing.y = 7.0;
                        speaker_row(ui, speaker);
                        self.agent_parts(ui, i, parts, last && !is_live, acts);
                        if let Some((_, sent, alive, h)) = live.filter(|_| is_live) {
                            let waiting = !alive
                                || match parts.last() {
                                    None => true,
                                    Some(Part::Tool(t)) => t.state != ToolState::Running,
                                    Some(Part::Text(_)) => false,
                                };
                            if waiting {
                                let words = match alive {
                                    false => format!("Starting {}…", h.label()),
                                    true => "Thinking…".into(),
                                };
                                stage_row(ui, &words, now - sent, now);
                            }
                        }
                        if let Some(s) = stats {
                            let prose = m.prose();
                            turn_foot(ui, s, &prose, last, acts);
                        }
                    });
                }
                Msg::Note(Note::Said(t)) => {
                    ui.label(
                        egui::RichText::new(t)
                            .size(ui::SMALL_PT)
                            .color(theme::text::faint()),
                    );
                }
                Msg::Note(Note::Failed {
                    text,
                    question,
                    details,
                }) => self.failure_note(ui, i, text, *question, details, acts),
            }
        }
    }

    /// A question: right-aligned and shrink-wrapped, its chips above its words.
    fn person_bubble(&self, ui: &mut egui::Ui, text: &str, chips: &[Chip], acts: &mut Vec<Act>) {
        let max = ui.available_width() * BUBBLE_SHARE;
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Min), |ui| {
            egui::Frame::NONE
                .fill(color::field())
                .stroke(egui::Stroke::new(1.0, color::field_border()))
                .corner_radius(egui::CornerRadius::same(8))
                .inner_margin(egui::Margin::symmetric(10, 7))
                .show(ui, |ui| {
                    ui.set_max_width(max - 20.0);
                    ui.with_layout(egui::Layout::top_down(egui::Align::Min), |ui| {
                        ui.spacing_mut().item_spacing.y = 6.0;
                        if !chips.is_empty() {
                            self.chip_row(ui, chips, false, acts);
                        }
                        ui.add(
                            egui::Label::new(
                                egui::RichText::new(text)
                                    .size(ui::CONTROL_PT)
                                    .color(color::text()),
                            )
                            .wrap(),
                        );
                    });
                });
        });
    }

    /// Chips in a wrapping row; `removable` above the composer.
    fn chip_row(&self, ui: &mut egui::Ui, chips: &[Chip], removable: bool, acts: &mut Vec<Act>) {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            for (n, c) in chips.iter().enumerate() {
                let gone = self.chip_gone(c);
                let name = self.layer_name(&c.id).unwrap_or_else(|| c.name.clone());
                let out = ui::layer_chip(
                    ui,
                    ui::LayerChip {
                        name: &name,
                        number: removable.then_some(n + 1),
                        gone,
                        removable,
                        lit: false,
                    },
                );
                if out.removed {
                    acts.push(Act::RemoveChip(n));
                } else if !gone {
                    if out.hovered {
                        acts.push(Act::Hover(c.id.clone()));
                    }
                    if out.resp.clicked() {
                        acts.push(Act::Select(c.id.clone()));
                    }
                }
            }
        });
    }

    /// An agent turn's parts in order: tool rows (after the third folded into
    /// *+N calls*), prose, and a card for each proposal.
    fn agent_parts(
        &self,
        ui: &mut egui::Ui,
        msg: usize,
        parts: &[Part],
        options_live: bool,
        acts: &mut Vec<Act>,
    ) {
        let total = parts.iter().filter(|p| matches!(p, Part::Tool(_))).count();
        let fold_id = egui::Id::new(("assistant-unfold", self.assistant.thread_epoch, msg));
        let unfolded = ui.data(|d| d.get_temp::<bool>(fold_id).unwrap_or(false));
        let mut seen = 0;
        let layer = |id: &str| self.layer_name(id);
        let look = markdown::Look {
            layer: &layer,
            options_live,
            ink: theme::text::strong(),
        };
        let mut proposals: Vec<u64> = Vec::new();
        for p in parts {
            match p {
                Part::Tool(t) => {
                    seen += 1;
                    if let Some(id) = t.proposal {
                        proposals.push(id);
                    }
                    if seen > TOOLS_SHOWN && !unfolded {
                        if seen == TOOLS_SHOWN + 1 {
                            let more = total - TOOLS_SHOWN;
                            if fold_row(ui, more).clicked() {
                                ui.data_mut(|d| d.insert_temp(fold_id, true));
                            }
                        }
                        continue;
                    }
                    tool_row(ui, t);
                }
                Part::Text(text) => {
                    let blocks = markdown::parse(text);
                    let mut md: Vec<MdAct> = Vec::new();
                    ui.vertical(|ui| markdown::show(ui, &blocks, &look, &mut md));
                    for a in md {
                        acts.push(match a {
                            MdAct::Select(id) => Act::Select(id),
                            MdAct::Hover(id) => Act::Hover(id),
                            MdAct::Choose(line) => Act::Choose(line),
                        });
                    }
                }
            }
        }
        for id in proposals {
            self.proposal_card(ui, id, acts);
        }
    }

    /// The note a harness leaves when it ends before answering.
    fn failure_note(
        &self,
        ui: &mut egui::Ui,
        msg: usize,
        text: &str,
        question: usize,
        details: &str,
        acts: &mut Vec<Act>,
    ) {
        let details_id = egui::Id::new(("assistant-details", self.assistant.thread_epoch, msg));
        egui::Frame::NONE
            .fill(color::danger_900())
            .stroke(egui::Stroke::new(1.0, color::danger_800()))
            .corner_radius(egui::CornerRadius::same(5))
            .inner_margin(egui::Margin::symmetric(10, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 6.0;
                ui.horizontal_top(|ui| {
                    ui.label(
                        egui::RichText::new(icon::WARNING)
                            .font(theme::icon_font(ui::SMALL_PT + 1.5))
                            .color(color::error()),
                    );
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(text)
                                .size(ui::SMALL_PT)
                                .color(theme::text::strong()),
                        )
                        .wrap(),
                    );
                });
                ui.horizontal(|ui| {
                    ui.add_space(20.0);
                    ui.spacing_mut().item_spacing.x = 12.0;
                    if !self.assistant.running() && ui::word_link(ui, "Ask again", false).clicked()
                    {
                        acts.push(Act::AskAgain(question));
                    }
                    if !details.trim().is_empty() {
                        let id = details_id;
                        let open = ui.data(|d| d.get_temp::<bool>(id).unwrap_or(false));
                        let word = if open { "Hide details" } else { "Show details" };
                        if ui::word_link(ui, word, true).clicked() {
                            ui.data_mut(|d| d.insert_temp(id, !open));
                        }
                    }
                });
                if ui.data(|d| d.get_temp::<bool>(details_id).unwrap_or(false)) {
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(details.trim())
                                .monospace()
                                .size(ui::CAPTION_PT)
                                .color(theme::text::muted()),
                        )
                        .wrap(),
                    );
                }
            });
    }

    /// The composer, or the reason it cannot send. Off wins over every other.
    fn assistant_composer(
        &mut self,
        ui: &mut egui::Ui,
        blocked: Option<&Why>,
        acts: &mut Vec<Act>,
    ) {
        if let Some(why) = blocked.filter(|w| !matches!(w, Why::Looking(_))) {
            egui::Frame::NONE
                .stroke(egui::Stroke::new(1.0, color::field_border()))
                .corner_radius(egui::CornerRadius::same(6))
                .inner_margin(egui::Margin::same(10))
                .show(ui, |ui| {
                    ui.set_width(ui.available_width());
                    ui.spacing_mut().item_spacing.y = 6.0;
                    ui.add(
                        egui::Label::new(
                            egui::RichText::new(why.sentence())
                                .size(ui::SMALL_PT)
                                .color(theme::text::dim()),
                        )
                        .wrap(),
                    );
                    let fix = match why {
                        Why::Off => Some("Open settings"),
                        Why::NotFound(..) => Some("Set the path"),
                        _ => None,
                    };
                    if let Some(word) = fix
                        && ui::word_link(ui, word, false).clicked()
                    {
                        acts.push(Act::OpenSettings);
                    }
                });
            return;
        }
        let running = self.assistant.running();
        let stopping = self.assistant_stopping();
        let edit_id = egui::Id::new("assistant-composer");
        let chips = self.assistant.chips.clone();
        let mut draft = std::mem::take(&mut self.assistant.draft);
        let mut send = false;
        // The text has no inset of its own (`TextArea::bare`), so the chips,
        // the text and the hint share this one left edge.
        let framed = ui::field_frame()
            .inner_margin(egui::Margin::symmetric(ui::FIELD_PAD_X as i8, 8))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 8.0;
                if !chips.is_empty() {
                    self.chip_row(ui, &chips, true, acts);
                }
                let hint = match self.assistant.msgs.is_empty() {
                    true => "Ask about this design",
                    false => "Ask a follow-up",
                };
                let mut area = ui::TextArea::new(hint, 2);
                area.id = Some(edit_id);
                area.enter_sends = true;
                area.bare = true;
                let resp = ui::text_area(ui, &mut draft, area);
                if self.assistant.focus {
                    resp.request_focus();
                    self.assistant.focus = false;
                }
                if resp.has_focus() {
                    let (enter, up, down) = ui.input(|i| {
                        (
                            i.key_pressed(egui::Key::Enter) && !i.modifiers.shift,
                            i.modifiers.command && i.key_pressed(egui::Key::ArrowUp),
                            i.modifiers.command && i.key_pressed(egui::Key::ArrowDown),
                        )
                    });
                    if enter && !running {
                        send = true;
                    }
                    if up || down {
                        let qs: Vec<String> = self
                            .assistant
                            .questions()
                            .iter()
                            .map(|s| (*s).to_owned())
                            .collect();
                        if !qs.is_empty() {
                            let at = recall_step(self.assistant.recall, up, qs.len());
                            self.assistant.recall = at;
                            draft = at.and_then(|i| qs.get(i).cloned()).unwrap_or_default();
                        }
                    }
                }
                // The hint sits on the row's foot, level with the send button's
                // bottom, so the space under it matches the space beside it.
                let foot = egui::vec2(ui.available_width(), SEND_W);
                ui.allocate_ui_with_layout(
                    foot,
                    egui::Layout::left_to_right(egui::Align::Max),
                    |ui| {
                        ui.label(
                            egui::RichText::new("Enter sends · Shift+Enter breaks the line")
                                .size(ui::CAPTION_PT)
                                .color(theme::text::faint()),
                        );
                        ui.with_layout(egui::Layout::right_to_left(egui::Align::Max), |ui| {
                            let (glyph, tip, state) = match running {
                                true if stopping => {
                                    (icon::STOP, "Stopping…", ui::FieldButton::Disabled)
                                }
                                true => (icon::STOP, "Stop", ui::FieldButton::On),
                                false if draft.trim().is_empty() => {
                                    (icon::ARROW_UP, "Send (Enter)", ui::FieldButton::Disabled)
                                }
                                false => (icon::ARROW_UP, "Send (Enter)", ui::FieldButton::On),
                            };
                            let b = ui::field_button(ui, glyph, SEND_W, state).on_hover_text(tip);
                            if b.clicked() {
                                match running {
                                    true if stopping => {}
                                    true => acts.push(Act::Stop),
                                    false => send = true,
                                }
                            }
                        });
                    },
                );
            });
        let _ = framed;
        if send && !draft.trim().is_empty() {
            acts.push(Act::Send(draft.clone(), chips));
        }
        self.assistant.draft = draft;
        // `Escape` in the composer closes the panel, as it closes its siblings.
        if ui.memory(|m| m.has_focus(edit_id)) && ui.input(|i| i.key_pressed(egui::Key::Escape)) {
            acts.push(Act::Close);
        }
    }

    /// A proposal's card: what it changes, and *Apply* — or why it no longer
    /// applies, or *Undo* once applied.
    fn proposal_card(&self, ui: &mut egui::Ui, id: u64, acts: &mut Vec<Act>) {
        let Some(p) = self.mcp.as_ref().and_then(|h| h.proposal(id)) else {
            ui.label(
                egui::RichText::new("This proposal is no longer held; ask again to see it.")
                    .size(ui::SMALL_PT)
                    .color(theme::text::faint()),
            );
            return;
        };
        let here = self.session.path.as_deref() == Some(p.document.as_path());
        let applied = self.assistant.applied.get(&id).copied();
        let stale = match (here, applied) {
            (true, None) => crate::mcp::propose::stale(&self.session, p),
            _ => None,
        };
        egui::Frame::NONE
            .fill(color::panel())
            .stroke(egui::Stroke::new(1.0, color::card_border()))
            .corner_radius(egui::CornerRadius::same(6))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.spacing_mut().item_spacing.y = 0.0;
                // Head: the sparkle, the title (the undo step's name), the count.
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(10, 8))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 8.0;
                            ui.label(
                                egui::RichText::new(icon::SPARKLE)
                                    .font(theme::icon_font(ui::CONTROL_PT + 1.0))
                                    .color(color::accent_300()),
                            );
                            ui.label(
                                egui::RichText::new(&p.title)
                                    .size(ui::CONTROL_PT)
                                    .color(color::text()),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    ui.label(
                                        egui::RichText::new(ui::count(p.changes.len(), "change"))
                                            .size(ui::SMALL_PT - 0.5)
                                            .color(theme::text::faint()),
                                    );
                                },
                            );
                        });
                    });
                ui::card_rule(ui);
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.spacing_mut().item_spacing.y = 4.0;
                        let (glyph, ink) = match applied {
                            Some(_) => (icon::CHECK, color::saved()),
                            None => (icon::CARET_RIGHT, theme::text::dim()),
                        };
                        for c in &p.changes {
                            ui.horizontal_top(|ui| {
                                ui.spacing_mut().item_spacing.x = 8.0;
                                ui.label(
                                    egui::RichText::new(glyph)
                                        .font(theme::icon_font(ui::SMALL_PT))
                                        .color(ink),
                                );
                                ui.add(
                                    egui::Label::new(
                                        egui::RichText::new(c)
                                            .size(ui::SMALL_PT)
                                            .color(theme::text::strong()),
                                    )
                                    .wrap(),
                                );
                            });
                        }
                    });
                ui::card_rule(ui);
                egui::Frame::NONE
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 10.0;
                            let faint = |t: &str| {
                                egui::RichText::new(t)
                                    .size(ui::SMALL_PT - 0.5)
                                    .color(theme::text::faint())
                            };
                            match (applied, &stale, here) {
                                (Some(a), _, _) => {
                                    ui.label(faint("Applied as one step"));
                                    let last =
                                        here && crate::mcp::propose::is_last(&self.session, &a);
                                    if last {
                                        ui.with_layout(
                                            egui::Layout::right_to_left(egui::Align::Center),
                                            |ui| {
                                                if ui::label_button(
                                                    ui,
                                                    "Undo",
                                                    egui::vec2(64.0, ui::CONTROL_H - 4.0),
                                                    ui::FieldButton::Off,
                                                )
                                                .clicked()
                                                {
                                                    acts.push(Act::Undo(id));
                                                }
                                            },
                                        );
                                    }
                                }
                                (None, _, false) => {
                                    ui.label(faint("Made in another document"));
                                }
                                (None, Some(name), true) => {
                                    ui.label(
                                        egui::RichText::new(icon::WARNING)
                                            .font(theme::icon_font(ui::SMALL_PT))
                                            .color(color::warn()),
                                    );
                                    ui.add(
                                        egui::Label::new(
                                            egui::RichText::new(format!(
                                                "{name} changed after this was proposed"
                                            ))
                                            .size(ui::SMALL_PT - 0.5)
                                            .color(color::warn()),
                                        )
                                        .truncate(),
                                    );
                                    if !self.assistant.running()
                                        && ui::word_link(ui, "Ask again", false).clicked()
                                    {
                                        acts.push(Act::Retry(id, name.clone()));
                                    }
                                }
                                (None, None, true) => {
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            if ui::label_button(
                                                ui,
                                                "Apply",
                                                egui::vec2(72.0, ui::CONTROL_H - 4.0),
                                                ui::FieldButton::On,
                                            )
                                            .on_hover_text("Make these changes as one undo step")
                                            .clicked()
                                            {
                                                acts.push(Act::Apply(id));
                                            }
                                        },
                                    );
                                }
                            }
                        });
                    });
            });
    }

    /// Apply a proposal from its card, as one undo step marked as the
    /// assistant's.
    fn assistant_apply(&mut self, id: u64) {
        let Some(p) = self.mcp.as_ref().and_then(|h| h.proposal(id)).cloned() else {
            return;
        };
        match crate::mcp::propose::apply(&mut self.session, &p) {
            Ok(a) => {
                self.assistant.applied.insert(id, a);
            }
            Err(why) => self.session.info(why),
        }
    }

    /// Undo an applied proposal while its step is the last.
    fn assistant_undo(&mut self, id: u64) {
        let Some(a) = self.assistant.applied.get(&id).copied() else {
            return;
        };
        if crate::mcp::propose::is_last(&self.session, &a) {
            self.undo();
            self.assistant.applied.remove(&id);
        }
    }

    /// Chips for layers by wire id.
    fn selection_chips_of(&self, ids: &[String]) -> Vec<Chip> {
        let doc = &self.session.doc;
        ids.iter()
            .filter_map(|w| {
                let id = ondin_core::NodeId::from_wire(w)?;
                let n = doc.get(id)?;
                let r = self.session.resolved.world_bounds(id);
                Some(Chip {
                    id: w.clone(),
                    name: n.name().to_owned(),
                    kind: ondin_export::snapshot::kind_name(n.kind()).to_owned(),
                    w: r.map_or(0.0, |r| r.width()),
                    h: r.map_or(0.0, |r| r.height()),
                })
            })
            .collect()
    }
}

/// What each level lets the agent do, for its menu row and Settings' cell.
pub(crate) fn level_tip(l: Level) -> &'static str {
    match l {
        Level::Look => "Read and export the document",
        Level::Propose => "Read, and propose edits you apply",
        Level::Edit => "Read and edit the document; each edit is an undo step",
    }
}

/// The speaker's name over an agent's turn, in bold, with the sparkle.
fn speaker_row(ui: &mut egui::Ui, speaker: &str) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 6.0;
        // The glyph's ink sits high in its box beside the name.
        nudged_label(
            ui,
            egui::RichText::new(icon::SPARKLE)
                .font(theme::icon_font(ui::SMALL_PT + 0.5))
                .color(color::accent_300()),
            1.0,
        );
        ui.label(ui::bold(
            egui::RichText::new(speaker)
                .size(ui::SMALL_PT - 0.5)
                .color(theme::text::muted()),
        ));
    });
}

/// A label in a centred row, drawn `dy` lower than the row would put it.
fn nudged_label(ui: &mut egui::Ui, text: egui::RichText, dy: f32) {
    let galley = egui::WidgetText::from(text).into_galley(
        ui,
        Some(egui::TextWrapMode::Extend),
        f32::INFINITY,
        egui::TextStyle::Body,
    );
    // Twice the nudge taller, so centring the box moves the text by `dy`.
    let size = galley.size() + egui::vec2(0.0, 2.0 * dy);
    let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
    ui.painter().galley(
        rect.min + egui::vec2(0.0, 2.0 * dy),
        galley,
        theme::text::faint(),
    );
}

/// The waiting line: the spinner, the stage and the seconds since the send.
fn stage_row(ui: &mut egui::Ui, words: &str, secs: f64, now: f64) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 7.0;
        let (r, _) =
            ui.allocate_exact_size(egui::vec2(ui::CONTROL_GLYPH_PT, 16.0), egui::Sense::hover());
        ui::paint_spinner(
            ui.painter(),
            r.center(),
            ui::CONTROL_GLYPH_PT - 2.0,
            color::accent_300(),
            now,
        );
        ui.label(
            egui::RichText::new(words)
                .size(ui::CONTROL_PT)
                .color(theme::text::dim()),
        );
        // Smaller than the stage's words, so centring sets it high of their line.
        nudged_label(
            ui,
            egui::RichText::new(format!("{} s", secs.max(0.0) as u64))
                .size(ui::SMALL_PT)
                .color(theme::text::faint()),
            1.0,
        );
    });
}

/// A tool call: its name in mono, its argument, a status dot and its result.
fn tool_row(ui: &mut egui::Ui, t: &ToolRow) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, TOOL_H), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 4.0, color::field());
    let y = rect.center().y;
    let dot = egui::pos2(rect.left() + 8.0 + 3.0, y);
    match t.state {
        ToolState::Running => {
            p.circle_stroke(dot, 2.5, egui::Stroke::new(1.5, color::accent_300()));
        }
        ToolState::Done => {
            p.circle_filled(dot, 3.0, color::saved());
        }
        ToolState::Failed => {
            p.circle_filled(dot, 3.0, color::error());
        }
    }
    let name_ink = match t.state {
        ToolState::Failed => color::error(),
        _ => theme::text::strong(),
    };
    let name = p.layout_no_wrap(
        t.name.clone(),
        egui::FontId::monospace(ui::SMALL_PT - 0.5),
        name_ink,
    );
    let name_x = rect.left() + 8.0 + 6.0 + 8.0;
    let name_w = name.size().x;
    p.galley(egui::pos2(name_x, y - name.size().y / 2.0), name, name_ink);
    let result_ink = match t.state {
        ToolState::Failed => color::error().gamma_multiply(0.75),
        _ => theme::text::faint(),
    };
    let result = p.layout_no_wrap(
        t.result.clone(),
        egui::FontId::proportional(ui::CAPTION_PT),
        result_ink,
    );
    let result_x = rect.right() - 8.0 - result.size().x;
    let result_w = result.size().x;
    p.galley(
        egui::pos2(result_x, y - result.size().y / 2.0),
        result,
        result_ink,
    );
    let arg_x = name_x + name_w + 8.0;
    let room = result_x - 8.0 - arg_x;
    if room > 12.0 && !t.arg.is_empty() {
        let job = egui::text::LayoutJob {
            wrap: egui::text::TextWrapping::truncate_at_width(room),
            ..egui::text::LayoutJob::simple_singleline(
                t.arg.clone(),
                egui::FontId::proportional(ui::SMALL_PT - 0.5),
                theme::text::faint(),
            )
        };
        let arg = ui.fonts_mut(|f| f.layout_job(job));
        ui.painter().galley(
            egui::pos2(arg_x, y - arg.size().y / 2.0),
            arg,
            theme::text::faint(),
        );
    }
    let _ = result_w;
}

/// *+N calls*, the folded rows' stand-in; a click opens them.
fn fold_row(ui: &mut egui::Ui, more: usize) -> egui::Response {
    let w = ui.available_width();
    let (rect, resp) = ui.allocate_exact_size(egui::vec2(w, TOOL_H), egui::Sense::click());
    let p = ui.painter();
    p.rect_filled(
        rect,
        4.0,
        ui::list_row_fill(false, resp.hovered()).unwrap_or(color::field()),
    );
    p.text(
        egui::pos2(rect.left() + 8.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        format!("+{}", ui::count(more, "call")),
        egui::FontId::proportional(ui::CAPTION_PT),
        theme::text::dim(),
    );
    resp.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// Under a finished turn: its time and tokens, *Copy*, and on the last turn
/// *Regenerate*.
fn turn_foot(
    ui: &mut egui::Ui,
    s: &super::thread::Stats,
    prose: &str,
    last: bool,
    acts: &mut Vec<Act>,
) {
    ui.horizontal(|ui| {
        ui.spacing_mut().item_spacing.x = 10.0;
        let mut words = format!("{} s", s.seconds.round() as u64);
        if let Some(t) = s.tokens {
            words.push_str(&format!(" · {} tokens", tokens(t)));
        }
        ui.label(
            egui::RichText::new(words)
                .size(ui::CAPTION_PT)
                .color(theme::text::faint()),
        );
        if ui::icon_button(ui, icon::COPY, 18.0, 12.0, false, true)
            .on_hover_text("Copy")
            .clicked()
        {
            ui.ctx().copy_text(prose.to_owned());
        }
        if last
            && ui::icon_button(ui, icon::ARROW_CLOCKWISE, 18.0, 12.0, false, true)
                .on_hover_text("Regenerate")
                .clicked()
        {
            acts.push(Act::Regenerate);
        }
    });
}

/// A token count as the foot shows it: `850`, `3.1k`.
fn tokens(n: u64) -> String {
    match n {
        0..1000 => n.to_string(),
        _ => format!("{:.1}k", n as f64 / 1000.0),
    }
}

/// The question `Ctrl+↑` (`up`) or `Ctrl+↓` steps to among `len` questions,
/// from `recall`; one past this thread's end, left from another, starts over.
fn recall_step(recall: Option<usize>, up: bool, len: usize) -> Option<usize> {
    match (recall.filter(|&i| i < len), up) {
        _ if len == 0 => None,
        (None, true) => Some(len - 1),
        (None, false) => None,
        (Some(i), true) => Some(i.saturating_sub(1)),
        (Some(i), false) if i + 1 < len => Some(i + 1),
        (Some(_), false) => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn token_counts_read_short() {
        assert_eq!(tokens(850), "850");
        assert_eq!(tokens(3100), "3.1k");
    }

    /// A recall left past the end by a longer thread starts at the newest.
    /// flip: `recall_step` without its `filter` → fails at `Some(6)`.
    #[test]
    fn recall_from_a_longer_thread_stays_in_range() {
        assert_eq!(recall_step(Some(7), true, 2), Some(1));
        assert_eq!(recall_step(Some(7), false, 2), None);
        assert_eq!(recall_step(None, true, 2), Some(1));
        assert_eq!(recall_step(Some(1), true, 2), Some(0));
        assert_eq!(recall_step(Some(0), false, 2), Some(1));
        assert_eq!(recall_step(None, true, 0), None);
    }
}
