use std::rc::Rc;

use crate::app::{Ctx, Request};
use crate::screens::{BuildCx, Screen, button, frame, label};
use crate::ui::text::Arg;
use crate::ui::{Kind, Style, Ui};

#[derive(Default)]
pub struct Multiplayer;

impl Screen for Multiplayer {
    fn name(&self) -> &str {
        "core:multiplayer"
    }

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        let ticket = super::current_ticket();
        frame(ui, cx, "multiplayer.title", |ui| {
            if cx.in_game {
                ui.panel("panel", Style::default().pad(8.0).gap(4.0).stretch().w(340.0), |ui| {
                    let hosting = super::p2p::HOSTING.lock().is_some();
                    if hosting || ticket.is_some() {
                        label(ui, cx, "multiplayer.your_ticket", &[], "default");
                        ui.text_bound(
                            "dim",
                            Box::new(|_: &Ctx| {
                                let t = super::current_ticket().unwrap_or_else(|| "...".into());
                                let short = if t.len() > 52 { format!("{}...{}", &t[..28], &t[t.len() - 20..]) } else { t };
                                (String::new(), vec![Arg::Text(short)])
                            }),
                        );
                        button(ui, cx, "multiplayer.copy", &[], 120.0, move |_| {
                            if let Some(t) = super::current_ticket() {
                                crate::clipboard::set(&t);
                            }
                        });
                    } else {
                        button(ui, cx, "multiplayer.host", &[], 200.0, |c| c.requests.push(Request::Host));
                    }
                });
            }
            ui.panel("panel", Style::default().pad(8.0).gap(4.0).stretch().w(340.0), |ui| {
                label(ui, cx, "multiplayer.ticket", &[], "default");
                let input = ui.add(Kind::input("", 4096), Style::default().h(18.0).stretch());
                ui.set_id(input, "ticket");
                ui.button(
                    "button",
                    Style::default().min_w(120.0).min_h(20.0).pad_xy(6.0, 4.0),
                    Rc::new(|c: &mut Ctx| c.requests.push(Request::Join(String::new()))),
                    |ui| {
                        ui.text(cx.t("multiplayer.join", &[]), "button");
                    },
                );
            });
            button(ui, cx, "menu.back", &[], 200.0, |c| c.requests.push(Request::Close));
        });
    }
}
