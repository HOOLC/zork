//! Local-data reset confirmation. Hosts provide read-only progress and handle
//! Confirmed; the shared component owns only disclosure and focus.
use crate::{
    components::widgets::primitives::dialog::AlertDialog, design::ZORK_UI, resources::Text,
};
use gpui::{prelude::*, *};

#[derive(Clone, Default, PartialEq)]
pub struct Data {
    pub busy: bool,
    pub error: Option<String>,
}
pub struct Confirmed;
pub struct DataSettings {
    data: Data,
    text: Text,
    open: bool,
    alert: AlertDialog,
}
impl EventEmitter<Confirmed> for DataSettings {}
impl DataSettings {
    pub fn new(data: Data, text: Text, cx: &mut Context<Self>) -> Self {
        let mut alert = AlertDialog::new(cx).destructive();
        alert.cancel_label(text.text("client_data_cancel"));
        alert.details(
            text.text("client_clear_data_details_label"),
            text.text("client_clear_data_details"),
        );
        Self {
            data,
            text,
            open: false,
            alert,
        }
    }
    pub fn configure(&mut self, data: Data, text: Text, cx: &mut Context<Self>) {
        let changed = self.data != data
            || self.text.text("client_clear_data") != text.text("client_clear_data");
        self.data = data;
        self.text = text;
        self.alert
            .cancel_label(self.text.text("client_data_cancel"));
        self.alert.details(
            self.text.text("client_clear_data_details_label"),
            self.text.text("client_clear_data_details"),
        );
        if changed {
            cx.notify();
        }
    }
    pub fn cancel(&mut self, cx: &mut Context<Self>) {
        if !self.data.busy && self.open {
            self.open = false;
            cx.notify();
        }
    }
    pub fn inspect(&self) -> serde_json::Value {
        serde_json::json!({"open":self.open,"busy":self.data.busy,"error":self.data.error,"dialog":self.alert.inspect()})
    }
}
impl Render for DataSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let text = self.text.clone();
        let button = self.alert.trigger(
            "clear-client-data",
            text.text("client_clear_data"),
            152.,
            window,
            cx,
            |view, _, cx| {
                if !view.data.busy {
                    view.open = true;
                    cx.notify();
                }
            },
        );
        let description = match &self.data.error {
            Some(error) => format!("{}\n\n{error}", text.text("client_clear_data_warning")),
            None => text.text("client_clear_data_warning").to_string(),
        };
        let dialog = self.alert.render(
            "clear-client-data-dialog",
            text.text("client_clear_data_title"),
            description,
            text.text(if self.data.busy {
                "client_clearing_data"
            } else {
                "client_clear_data_confirm"
            }),
            self.open,
            self.data.busy,
            window,
            cx,
            |view, _, cx| view.cancel(cx),
            |view, _, cx| {
                if view.open && !view.data.busy {
                    cx.emit(Confirmed);
                }
            },
        );
        div()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .text_size(px(13.))
                    .line_height(px(22.))
                    .text_color(rgb(ZORK_UI.palette.muted))
                    .child(text.text("client_clear_data_detail")),
            )
            .child(div().child(button))
            .when_some(
                self.data.error.clone().filter(|_| !self.open),
                |v, error| {
                    v.child(
                        div()
                            .text_size(px(13.))
                            .text_color(rgb(ZORK_UI.palette.danger))
                            .child(error),
                    )
                },
            )
            .when_some(dialog, |v, dialog| v.child(dialog))
    }
}
