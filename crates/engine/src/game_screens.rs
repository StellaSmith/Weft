//! In-game screens: container menus (built by the mod that registered the menu), death, chat.

use crate::app::Request;
use crate::client::modui;
use crate::content::content;
use crate::screens::{BuildCx, Screen, button, label};
use crate::ui::{Kind, Style, Ui};

pub struct MenuScreen {
    pub kind: u32,
}

impl Screen for MenuScreen {
    fn name(&self) -> &str {
        "core:menu"
    }

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        let c = content();
        ui.root_style(Style::default().stretch());
        ui.panel("overlay", Style::default().grow().center(), |ui| {
            if let Some(def) = c.game.menus.get(self.kind as usize) {
                if let Some(cb) = def.screen {
                    modui::build(ui, cx.lang, def.owner, cb, cx.client);
                }
            }
        });
    }
}

pub struct Death;

impl Screen for Death {
    fn name(&self) -> &str {
        "core:death"
    }

    fn build(&mut self, ui: &mut Ui, cx: &BuildCx) {
        ui.root_style(Style::default().stretch());
        ui.panel("death", Style::default().grow().center().gap(12.0), |ui| {
            let t = label(ui, cx, "death.title", &[], "title");
            if let Some(t) = ui.text_mut(t) {
                t.scale = 2.0;
                t.wrap = false;
            }
            button(ui, cx, "death.respawn", &[], 200.0, |c| c.requests.push(Request::Respawn));
            button(ui, cx, "menu.save_quit", &[], 200.0, |c| c.requests.push(Request::LeaveWorld));
        });
    }
}

pub struct Chat;

impl Screen for Chat {
    fn name(&self) -> &str {
        "core:chat"
    }

    fn build(&mut self, ui: &mut Ui, _cx: &BuildCx) {
        ui.root_style(Style::default().stretch());
        ui.container(Style::default().grow().pad(4.0).justify(crate::ui::Justify::End), |ui| {
            let input = ui.add(
                Kind::Input {
                    value: String::new(),
                    max: 256,
                    on_change: None,
                },
                Style::default().h(14.0).stretch(),
            );
            ui.set_id(input, "chat");
            ui.focused = Some(input);
        });
    }
}
