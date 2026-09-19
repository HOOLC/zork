use super::*;

#[derive(Clone)]
pub struct NotificationData {
    pub enabled: bool,
    pub preview: bool,
    pub sound: bool,
    pub current_muted: Option<bool>,
    pub permission_label: String,
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
pub fn notifications<V: 'static>(
    data: NotificationData,
    focus: &[FocusHandle; 4],
    text: impl Fn(&str) -> String,
    cx: &Context<V>,
    action: impl Fn(&mut V, NotificationAction, &mut Context<V>) + 'static,
) -> Div {
    let action = Rc::new(action);
    let p = ZORK_UI.palette;
    let mut content = div().flex().flex_col().gap_3();
    for (index, (id, title, detail, selected)) in [
        (
            "notifications-toggle",
            "notification_enabled",
            "notification_enabled_detail",
            data.enabled,
        ),
        (
            "notifications-preview",
            "notification_preview",
            "notification_preview_detail",
            data.preview,
        ),
        (
            "notifications-sound",
            "notification_sound",
            "notification_sound_detail",
            data.sound,
        ),
    ]
    .into_iter()
    .enumerate()
    {
        let change = action.clone();
        content = content.child(row(
            text(title),
            text(detail),
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
            ),
        ));
    }
    if let Some(muted) = data.current_muted {
        let change = action.clone();
        content = content.child(row(
            text("notification_mute"),
            text("notification_mute_detail"),
            ui::switch(
                "notification-mute-current",
                text("notification_mute"),
                muted,
                true,
                &focus[3],
                cx,
                move |view, on, cx| change(view, NotificationAction::Mute(on), cx),
            ),
        ));
    }
    let refresh = action.clone();
    let test = action.clone();
    let enabled = data.enabled && !data.busy;
    content
        .child(
            row(
                text("notification_system_permission"),
                data.permission_label.clone(),
                ui::button(
                    "notifications-permission-refresh",
                    text("notification_check_permission"),
                    false,
                    true,
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    refresh(view, NotificationAction::RefreshPermission, cx)
                }))
                .automation(
                    AutomationRole::Button,
                    text("notification_check_permission"),
                ),
            )
            .id("notifications-permission-status")
            .automation(AutomationRole::Status, data.permission_label),
        )
        .child(
            ui::button(
                "notifications-test",
                text("notification_test"),
                false,
                enabled,
            )
            .on_click(cx.listener(move |view, _, _, cx| {
                if enabled {
                    test(view, NotificationAction::Test, cx);
                }
            }))
            .automation_enabled(
                enabled,
                AutomationRole::Button,
                text("notification_test"),
            ),
        )
        .when(data.system_settings, |content| {
            content.child(
                ui::button(
                    "notifications-system-settings",
                    text("notification_system_settings"),
                    false,
                    true,
                )
                .on_click(cx.listener(move |view, _, _, cx| {
                    action(view, NotificationAction::SystemSettings, cx)
                }))
                .automation(AutomationRole::Button, text("notification_system_settings")),
            )
        })
        .child(
            div()
                .text_size(px(12.))
                .text_color(rgb(p.muted))
                .child(text("notification_permission_detail")),
        )
        .when_some(data.error, |content, error| {
            content.child(div().text_color(rgb(p.danger)).child(error))
        })
}
