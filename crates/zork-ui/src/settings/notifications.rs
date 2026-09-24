use super::*;

#[derive(Clone)]
pub struct NotificationData {
    pub enabled: bool,
    pub preview: bool,
    pub sound: bool,
    pub current_muted: Option<bool>,
    pub permission_label: String,
    /// The system blocks notifications; only then does the page mention permission.
    pub permission_denied: bool,
    pub busy: bool,
    pub system_settings: bool,
    pub error: Option<String>,
}

#[derive(Clone, Copy)]
pub enum NotificationAction {
    Enabled(bool),
    Preview(bool),
    Sound(bool),
    Mute(bool),
    RefreshPermission,
    Test,
    SystemSettings,
}

/// Complete notification settings. Parameters and intent handling are the only
/// difference between a platform settings page and its Playground specimen.
/// Switches carry no descriptions; the page checks permission when it opens
/// and speaks up only when the system blocks delivery.
pub fn notifications<V: 'static>(
    data: NotificationData,
    focus: &[FocusHandle; 4],
    text: impl Fn(&str) -> String,
    cx: &Context<V>,
    action: impl Fn(&mut V, NotificationAction, &mut Context<V>) + 'static,
) -> Div {
    let action = Rc::new(action);
    let p = ZORK_UI.palette;
    let switch_row = |title: String, info: Option<(&'static str, String)>, control: gpui::AnyElement| {
        div()
            .w_full()
            .flex()
            .items_center()
            .gap_2()
            .min_h(px(40.))
            .child(div().text_size(px(14.)).child(title))
            .children(info.map(|(id, text)| crate::components::disclosure::info(id, text)))
            .child(div().flex_1())
            .child(control)
    };
    let mut content = div().flex().flex_col();
    if data.permission_denied {
        let open = action.clone();
        content = content.child(
            div()
                .id("notifications-permission-status")
                .mb_3()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    div()
                        .flex_1()
                        .child(ui::status_notice(
                            data.permission_label.clone(),
                            ui::NoticeKind::Warning,
                        )),
                )
                .when(data.system_settings, |v| {
                    v.child(
                        ui::button(
                            "notifications-system-settings",
                            text("notification_system_settings"),
                            false,
                            true,
                        )
                        .on_click(cx.listener(move |view, _, _, cx| {
                            open(view, NotificationAction::SystemSettings, cx)
                        }))
                        .automation(AutomationRole::Button, text("notification_system_settings")),
                    )
                })
                .automation(AutomationRole::Status, data.permission_label.clone()),
        );
    }
    for (index, (id, title, detail, selected)) in [
        ("notifications-toggle", "notification_enabled", None, data.enabled),
        (
            "notifications-preview",
            "notification_preview",
            Some(("notifications-preview-info", "notification_preview_detail")),
            data.preview,
        ),
        ("notifications-sound", "notification_sound", None, data.sound),
    ]
    .into_iter()
    .enumerate()
    {
        let change = action.clone();
        content = content.child(switch_row(
            text(title),
            detail.map(|(id, key)| (id, text(key))),
            ui::switch(
                id,
                text(title),
                selected,
                true,
                &focus[index],
                cx,
                move |view, on, cx| {
                    change(
                        view,
                        match index {
                            0 => NotificationAction::Enabled(on),
                            1 => NotificationAction::Preview(on),
                            _ => NotificationAction::Sound(on),
                        },
                        cx,
                    );
                },
            )
            .into_any_element(),
        ));
    }
    if let Some(muted) = data.current_muted {
        let change = action.clone();
        content = content.child(switch_row(
            text("notification_mute"),
            None,
            ui::switch(
                "notification-mute-current",
                text("notification_mute"),
                muted,
                true,
                &focus[3],
                cx,
                move |view, on, cx| change(view, NotificationAction::Mute(on), cx),
            )
            .into_any_element(),
        ));
    }
    let test = action.clone();
    let enabled = data.enabled && !data.busy;
    content
        .child(
            div().mt_3().flex().items_start().child(
                ui::quiet_button(
                    "notifications-test",
                    text("notification_test"),
                    enabled,
                    ui::IconButtonSize::Compact,
                )
                .ml(px(-12.))
                .on_click(cx.listener(move |view, _, _, cx| {
                    if enabled {
                        test(view, NotificationAction::Test, cx);
                    }
                }))
                .automation_enabled(enabled, AutomationRole::Button, text("notification_test")),
            ),
        )
        .when_some(data.error, |content, error| {
            content.child(div().mt_2().text_color(rgb(p.danger)).child(error))
        })
}
