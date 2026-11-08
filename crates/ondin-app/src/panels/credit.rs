//! Where an inserted asset's credit shows: the inspector's *Credit* card, on a
//! layer that carries one, and *Credits…*, the document's list.
//!
//! A credit is read, never edited: it came with the asset (`ondin_core::credit`).

use crate::app::OndinApp;
use crate::theme::{self, color, icon};
use crate::ui;
use ondin_core::credit::{Listed, credits_in};
use ondin_core::{Credit, CreditKind, NodeId};

/// The Credits card's width.
const W: f32 = 440.0;
/// A list row's height: a menu row's.
const ROW_H: f32 = ui::MENU_ROW_H;
/// The card's words when every credit is one its licence does not ask to show.
const NOTHING_OWED: &str = "Nothing here asks to be credited";

/// `2026-11-08` as *8 Nov 2026*; anything else as it is.
fn day(iso: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let mut parts = iso.splitn(3, '-').map(str::parse::<u32>);
    match (parts.next(), parts.next(), parts.next()) {
        (Some(Ok(y)), Some(Ok(m)), Some(Ok(d))) if (1..=12).contains(&m) => {
            format!("{d} {} {y}", MONTHS[m as usize - 1])
        }
        _ => iso.to_string(),
    }
}

/// The credit's sentence, its author's and provider's names as links where they
/// have an address. Answers the address clicked.
fn sentence(ui: &mut egui::Ui, c: &Credit) -> Option<String> {
    let mut clicked = None;
    // The two names in the order they appear, each with its address.
    let mut marks: Vec<(usize, &str, &str)> = [
        (c.author.as_str(), c.author_url.as_str()),
        (c.provider.as_str(), c.provider_url.as_str()),
    ]
    .into_iter()
    .filter(|(name, url)| !name.is_empty() && !url.is_empty())
    .filter_map(|(name, url)| c.text.find(name).map(|at| (at, name, url)))
    .collect();
    marks.sort_by_key(|m| m.0);
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        let plain = |ui: &mut egui::Ui, t: &str| {
            if !t.is_empty() {
                ui.label(
                    egui::RichText::new(t)
                        .size(ui::CONTROL_PT)
                        .color(color::text()),
                );
            }
        };
        let mut at = 0;
        for (start, name, url) in marks {
            if start < at {
                continue;
            }
            plain(ui, &c.text[at..start]);
            let link = ui
                .add(egui::Link::new(
                    egui::RichText::new(name)
                        .size(ui::CONTROL_PT)
                        .color(color::accent_300()),
                ))
                .on_hover_text(ondin_core::link_host(url));
            if link.clicked() {
                clicked = Some(url.to_string());
            }
            at = start + name.len();
        }
        plain(ui, &c.text[at..]);
    });
    clicked
}

impl OndinApp {
    /// The *Credit* card, on a layer that carries one: the sentence with its
    /// links, the licence and the day it came in, and a `⋯` with *Copy credit*
    /// and *Open photo page*.
    pub(super) fn inspector_credit(&mut self, ui: &mut egui::Ui, id: NodeId) {
        let Some(credit) = self.session.doc.get(id).and_then(|n| n.credit()).cloned() else {
            return;
        };
        let mut open = None;
        let action = Some(ui::HeadAction::new(icon::DOTS_THREE, "More"));
        let clicks = self.panel_head(ui, "Credit", None, action, None, |_app, ui| {
            ui.spacing_mut().item_spacing.y = 4.0;
            open = sentence(ui, &credit);
            let mut under = credit.licence.clone();
            if !credit.inserted.is_empty() {
                under.push_str(&format!(" · inserted {}", day(&credit.inserted)));
            }
            ui.label(
                egui::RichText::new(under)
                    .size(ui::CAPTION_PT)
                    .color(theme::text::faint()),
            );
        });
        if let Some(anchor) = clicks.action_rect {
            egui::Popup::new(
                ui.id().with("credit-more"),
                ui.ctx().clone(),
                anchor,
                ui.layer_id(),
            )
            .kind(egui::PopupKind::Menu)
            .layout(egui::Layout::top_down_justified(egui::Align::Min))
            .style(egui::containers::menu::menu_style)
            .open_memory(clicks.acted.then_some(egui::SetOpenCommand::Toggle))
            .close_behavior(egui::PopupCloseBehavior::CloseOnClick)
            .align(egui::RectAlign::BOTTOM_END)
            .gap(ui::MENU_GAP)
            .show(|ui| {
                ui::popup_rows(ui);
                ui.set_width(180.0);
                if ui::menu_item(ui, icon::COPY, "Copy credit", ROW_H).clicked() {
                    ui.ctx().copy_text(credit.plain());
                    self.session.info("Copied the credit");
                }
                let page = match credit.kind {
                    CreditKind::Photo => "Open photo page",
                    CreditKind::Icon => "Open the set's page",
                };
                let url = match credit.kind {
                    CreditKind::Photo => &credit.source_url,
                    CreditKind::Icon => &credit.provider_url,
                };
                let row = ui::MenuRow::new(icon::ARROW_UP_RIGHT, page).enabled(!url.is_empty());
                if ui::menu_row(ui, row, ROW_H).clicked() && !url.is_empty() {
                    open = Some(url.clone());
                }
            });
        }
        if let Some(url) = open {
            super::open_doc_link(ui.ctx(), &url);
        }
    }

    /// Whether any layer of the document carries a credit its licence asks to
    /// show: what *Credits…* is offered on, since the card lists only those.
    pub(crate) fn has_credits(&self) -> bool {
        !self.listed_credits().is_empty()
    }

    /// The credits the Credits card lists: every page's, those whose licence asks
    /// for them, photos first.
    fn listed_credits(&self) -> Vec<Listed> {
        let doc = &self.session.doc;
        let pages: Vec<NodeId> = doc.page_ids().collect();
        let mut listed: Vec<Listed> = credits_in(doc, &pages)
            .into_iter()
            .filter(|l| l.credit.required)
            .collect();
        listed.sort_by_key(|l| l.credit.kind != CreditKind::Photo);
        listed
    }

    /// The Credits card, while it is open: the photos one to a line and each icon
    /// set's one sentence, a click selecting the layers a line credits; *Copy all*
    /// puts the list on the clipboard.
    pub(crate) fn credits_ui(&mut self, ctx: &egui::Context) {
        if !self.credits_open {
            return;
        }
        let listed = self.listed_credits();
        let doc = &self.session.doc;
        let pages: Vec<NodeId> = doc.page_ids().collect();
        let unlisted = credits_in(doc, &pages).iter().any(|l| !l.credit.required);
        let canvas = self.canvas_rect;
        let mut close = false;
        let mut copy = false;
        let mut select: Option<Vec<NodeId>> = None;
        let hold = crate::settings::CardHold::Open;
        crate::settings::canvas_card(ctx, "credits", canvas, W, hold, |ui| {
            if crate::settings::card_head(ui, W, "Credits") {
                close = true;
            }
            egui::Frame::NONE
                .inner_margin(egui::Margin {
                    left: 8,
                    right: 8,
                    top: 0,
                    bottom: 10,
                })
                .show(ui, |ui| {
                    ui.set_width(W - 16.0);
                    if listed.is_empty() {
                        // At the eyebrows' and notes' inset, level with the head.
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.add_space(6.0);
                            ui::empty_note(ui, NOTHING_OWED);
                        });
                    }
                    for kind in [CreditKind::Photo, CreditKind::Icon] {
                        let rows: Vec<&Listed> =
                            listed.iter().filter(|l| l.credit.kind == kind).collect();
                        if rows.is_empty() {
                            continue;
                        }
                        let (label, glyph) = match kind {
                            CreditKind::Photo => ("Photos", icon::IMAGE),
                            CreditKind::Icon => ("Icons", icon::SHAPES),
                        };
                        let layers: usize = rows.iter().map(|l| l.layers.len()).sum();
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.add_space(6.0);
                            ui.label(ui::eyebrow(&format!("{label} · {layers}")));
                        });
                        ui.add_space(4.0);
                        for l in rows {
                            let right = match l.layers.as_slice() {
                                [one] => crate::private::layer(doc, *one).into_owned(),
                                many => ui::count(many.len(), "layer"),
                            };
                            let row = ui::MenuRow::new(glyph, &l.credit.text).note(&right);
                            if ui::menu_row(ui, row, ROW_H).clicked() {
                                select = Some(l.layers.clone());
                            }
                        }
                    }
                    if unlisted {
                        ui.add_space(8.0);
                        ui.horizontal(|ui| {
                            ui.add_space(6.0);
                            ui.add(
                                egui::Label::new(
                                    egui::RichText::new(
                                        "MIT, ISC and Apache icons and public-domain photos ask \
                                         for no credit on the design, so they are not listed.",
                                    )
                                    .size(ui::CAPTION_PT)
                                    .color(theme::text::faint()),
                                )
                                .wrap(),
                            );
                        });
                    }
                });
            ui::card_rule(ui);
            let copy_state = match listed.is_empty() {
                true => ui::FieldButton::Disabled,
                false => ui::FieldButton::Primary,
            };
            match crate::settings::card_foot(
                ui,
                W,
                if listed.is_empty() {
                    ""
                } else {
                    "Click a row to select its layers"
                },
                &[("Close", ui::FieldButton::Off), ("Copy all", copy_state)],
                listed.is_empty().then_some(NOTHING_OWED),
            ) {
                Some(0) => close = true,
                Some(_) if !listed.is_empty() => copy = true,
                _ => {}
            }
        });
        if copy {
            let text: Vec<String> = listed.iter().map(|l| l.credit.plain()).collect();
            ctx.copy_text(text.join("\n"));
            self.session
                .info(format!("Copied {}", ui::count(listed.len(), "credit")));
        }
        if let Some(ids) = select {
            self.select_credited(ids);
        }
        if close {
            self.credits_open = false;
        }
    }

    /// Select the layers a credit line names, going to their page when none is
    /// on this one.
    fn select_credited(&mut self, ids: Vec<NodeId>) {
        let doc = &self.session.doc;
        let here: Vec<NodeId> = ids
            .iter()
            .copied()
            .filter(|&id| doc.page_of(id) == Some(self.session.page))
            .collect();
        let ids = match here.is_empty() {
            false => here,
            true => {
                let Some(page) = ids.first().and_then(|&id| doc.page_of(id)) else {
                    return;
                };
                let there: Vec<NodeId> = ids
                    .into_iter()
                    .filter(|&id| doc.page_of(id) == Some(page))
                    .collect();
                self.go_to_page(page);
                there
            }
        };
        self.session.selection.set(ids);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// An inserted day reads as people write it, and a malformed one as it is.
    #[test]
    fn the_inserted_day_reads_as_a_date() {
        assert_eq!(day("2026-11-08"), "8 Nov 2026");
        assert_eq!(day("2026-13-08"), "2026-13-08");
        assert_eq!(day(""), "");
    }

    /// A credit line whose layer has a long name keeps half the row for its
    /// sentence, both cut with an ellipsis.
    /// flip: paint `menu_row`'s note uncut → fails at "the name is cut".
    #[test]
    fn a_long_layer_name_leaves_the_credit_its_half() {
        use ondin_core::{NodeKind, Operation, Transaction};
        let ctx = egui::Context::default();
        crate::theme::install(&ctx);
        let mut app = OndinApp::headless(&ctx);
        let id = app.session.ids.mint();
        let page = app.session.page;
        let sentence = "Photo by Somebody With A Very Long Name Indeed on Unsplash";
        let name = "A photo of a laptop on a wooden desk by the window, late at night";
        let credit = Credit {
            kind: CreditKind::Photo,
            text: sentence.into(),
            author: "x".into(),
            author_url: String::new(),
            provider: "Unsplash".into(),
            provider_url: String::new(),
            source_url: String::new(),
            licence: "Unsplash License".into(),
            licence_url: String::new(),
            required: true,
            inserted: String::new(),
        };
        app.session.commit(Transaction(vec![
            Operation::CreateNode {
                id,
                parent: page,
                index: 0,
                kind: NodeKind::Rect {
                    size: ondin_core::kurbo::Size::new(40.0, 30.0),
                    corner_radii: Default::default(),
                    corner_smoothing: 0.0,
                },
                transform: None,
                name: Some(name.into()),
            },
            Operation::SetCredit {
                id,
                credit: Some(Box::new(credit)),
            },
        ]));
        app.canvas_rect =
            egui::Rect::from_min_size(egui::pos2(0.0, 0.0), egui::vec2(1200.0, 800.0));
        app.credits_open = true;
        let mut texts = Vec::new();
        for _ in 0..2 {
            let out = ctx.run_ui(
                egui::RawInput {
                    screen_rect: Some(egui::Rect::from_min_size(
                        egui::Pos2::ZERO,
                        egui::vec2(1200.0, 800.0),
                    )),
                    ..Default::default()
                },
                |ui| app.credits_ui(ui.ctx()),
            );
            texts = out
                .shapes
                .into_iter()
                .filter_map(|s| match s.shape {
                    egui::epaint::Shape::Text(t) => Some(t.galley),
                    _ => None,
                })
                .collect();
        }
        let find = |text: &str| {
            texts
                .iter()
                .find(|g| g.text() == text)
                .unwrap_or_else(|| panic!("{text:?} is drawn"))
        };
        let (line, layer) = (find(sentence), find(name));
        assert!(layer.elided, "the name is cut");
        assert!(line.elided, "the fixture: the sentence is too long to fit");
        let ends = |g: &egui::Galley| g.rows[0].row.glyphs.last().map(|g| g.chr);
        assert_eq!(ends(line), Some('…'));
        assert_eq!(ends(layer), Some('…'));
        assert!(
            line.size().x >= 0.9 * layer.size().x,
            "the sentence keeps about the name's room: {} vs {}",
            line.size().x,
            layer.size().x
        );
    }
}
