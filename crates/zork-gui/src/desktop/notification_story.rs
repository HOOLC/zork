//! Mock parameters and events for the production notification settings component.
use gpui::{prelude::*, Context, FocusHandle, Window};
use zork_ui::settings::{NotificationAction, NotificationData};

pub struct NotificationStory {
    data: NotificationData,
    focus: [FocusHandle; 4],
}
impl NotificationStory {
    pub fn new(state: &str, cx: &mut Context<Self>) -> Self {
        Self {
            data: NotificationData {
                enabled: state != "disabled", preview: true, sound: true,
                current_muted: Some(state == "muted"),
                permission_label: crate::i18n::Locale::ZhCn.text(if state == "denied" {
                    "notification_permission_denied"
                } else { "notification_permission_allowed" }).into(),
                busy: state == "busy", system_settings: true,
                error: (state == "error").then(|| "通知服务暂时不可用。".into()),
            },
            focus: std::array::from_fn(|_| cx.focus_handle()),
        }
    }
}
impl Render for NotificationStory {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        zork_ui::settings::notifications(self.data.clone(), &self.focus,
            |key| crate::i18n::Locale::ZhCn.text(key).into(), cx,
            |view, action, cx| {
                match action {
                    NotificationAction::Enabled(value) => view.data.enabled = value,
                    NotificationAction::Preview(value) => view.data.preview = value,
                    NotificationAction::Sound(value) => view.data.sound = value,
                    NotificationAction::Mute(value) => view.data.current_muted = Some(value),
                    NotificationAction::RefreshPermission => view.data.permission_label =
                        crate::i18n::Locale::ZhCn.text("notification_permission_allowed").into(),
                    NotificationAction::Test | NotificationAction::SystemSettings => view.data.error = None,
                }
                cx.notify();
            })
    }
}
