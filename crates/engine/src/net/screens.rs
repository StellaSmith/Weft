use std::rc::Rc;

use crate::app::{Ctx, Request};
use crate::screens::{BuildCx, Screen, button, frame, label};
use crate::ui::text::{self, Arg};
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
                    match &ticket {
                        Some(t) => {
                            label(ui, cx, "multiplayer.your_ticket", &[], "default");
                            let short = if t.len() > 48 { format!("{}...{}", &t[..24], &t[t.len() - 16..]) } else { t.clone() };
                            ui.text(text::plain(&short), "dim");
                            let t = t.clone();
                            button(ui, cx, "multiplayer.copy", &[], 120.0, move |_| {
                                crate::clipboard::set(&t);
                            });
                        }
                        None => {
                            button(ui, cx, "multiplayer.host", &[], 200.0, |c| c.requests.push(Request::Host));
                        }
                    }
                });
            }
            ui.panel("panel", Style::default().pad(8.0).gap(4.0).stretch().w(340.0), |ui| {
                label(ui, cx, "multiplayer.ticket", &[], "default");
                let input = ui.add(Kind::Input { value: String::new(), max: 4096, on_change: None }, Style::default().h(18.0).stretch());
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
            let _ = Arg::Int(0);
        });
    }
}
