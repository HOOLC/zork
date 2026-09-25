//! Shared presentation of a model connection. Callers provide display values and intent.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::tooltip,
    controls::{self, provider_icon},
    design::ZORK_UI,
    device_name::{self, DeviceStatus},
};
use gpui::{div, prelude::*, px, rgb, App, ClickEvent, Window};

#[derive(Clone, Debug)]
pub struct DeviceIdentity {
    pub name: String,
    pub status: DeviceStatus,
}

#[derive(Clone, Debug)]
pub struct QuotaWindow {
    pub label: String,
    pub short_label: String,
    pub remaining: f32,
    pub center_value: String,
    pub value: String,
    pub reset: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct Quota {
    pub summary: String,
    pub failed: bool,
    pub windows: Vec<QuotaWindow>,
    pub balance: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ProfileCard {
    pub key: String,
    pub provider: String,
    /// The account: its email or login, or the access plus its key tail.
    pub name: String,
    /// A Profile name the user set, shown after the account, muted.
    pub custom_name: Option<String>,
    /// Where the connection lives; one account saved on several devices lists each.
    pub devices: Vec<DeviceIdentity>,
    pub billing: String,
    pub model_count: String,
    pub verified: bool,
    pub verification: String,
    pub quota: Option<Quota>,
}

/// One quiet line per connection: account and device first; quota as thin bars;
/// status and billing only matter when something needs fixing.
pub fn render(
    card: ProfileCard,
    on_click: impl Fn(&ClickEvent, &mut Window, &mut App) + 'static,
) -> controls::Action {
    let p = ZORK_UI.palette;
    controls::quiet_button(
        format!("profile-detail-{}", card.key),
        "",
        true,
        controls::IconButtonSize::Standard,
    )
    .radius(22.)
    .font_weight(gpui::FontWeight::NORMAL)
    .justify_start()
    .w_full()
    .on_click(on_click)
    .h(px(44.))
    .pl(px(14.))
    .pr(px(12.))
    .flex()
    .items_center()
    .gap(px(10.))
    .child(
        div()
            .id(format!("profile-provider-mark-{}", card.key))
            .flex_shrink_0()
            .child(provider_icon(&card.provider, 18.))
            .automation(AutomationRole::Status, card.provider.clone()),
    )
    .child({
        // An API key's tail identifies the account: only the access before it ellipsizes.
        let (head, tail) = match card.name.rfind(" · ···") {
            Some(at) if at > 0 => (card.name[..at].to_owned(), Some(card.name[at..].to_owned())),
            _ => (card.name.clone(), None),
        };
        div()
            .id(format!("profile-name-{}", card.key))
            // The account stays legible however narrow the row gets.
            .min_w(px(40.))
            .flex()
            .items_center()
            .text_size(px(13.))
            .font_weight(gpui::FontWeight::MEDIUM)
            .child(div().min_w_0().truncate().child(head))
            .when_some(tail, |v, tail| v.child(div().flex_shrink_0().child(tail)))
            .automation(AutomationRole::Status, format!("{} · {}", card.name, card.billing))
    })
    .when_some(card.custom_name.clone(), |v, custom| {
        v.child(
            div()
                .id(format!("profile-custom-name-{}", card.key))
                // Secondary: gives up width well before the account does.
                .min_w(px(28.))
                .flex_shrink(8.)
                .max_w(px(160.))
                .truncate()
                .text_size(px(12.))
                .text_color(rgb(p.muted))
                .child(custom.clone())
                .automation(AutomationRole::Status, custom),
        )
    })
    .when(card.devices.len() == 1, |v| {
        let device = card.devices[0].clone();
        v.child(
            div()
                .flex_shrink_0()
                .max_w(px(150.))
                .min_w_0()
                .text_size(px(12.))
                .text_color(rgb(p.muted))
                .child(device_name::label(
                    format!("profile-device-{}", card.key),
                    device.name,
                    &device.status,
                    None,
                )),
        )
    })
    // One account on several devices: "A、B"; each device's status is on its own row.
    .when(card.devices.len() > 1, |v| {
        let names = card
            .devices
            .iter()
            .map(|d| d.name.as_str())
            .collect::<Vec<_>>()
            .join("、");
        v.child(
            div()
                .id(format!("profile-devices-{}", card.key))
                .flex_shrink_0()
                .max_w(px(180.))
                .truncate()
                .text_size(px(12.))
                .text_color(rgb(p.muted))
                .child(names.clone())
                .automation(AutomationRole::Status, names),
        )
    })
    .child(div().flex_1())
    .when_some(
        card.quota
            .as_ref()
            .filter(|quota| !quota.summary.is_empty()),
        |v, quota| v.child(quota_summary(&card.key, quota)),
    )
    .child(
        div()
            .id(format!("profile-model-count-{}", card.key))
            .flex_shrink_0()
            .w(px(72.))
            .text_size(px(12.))
            .text_color(rgb(p.subtle))
            .child(card.model_count.clone())
            .automation(AutomationRole::Status, card.model_count),
    )
    .when(!card.verified, |v| {
        v.child(
            div()
                .id(format!("profile-verification-{}", card.key))
                .flex_shrink_0()
                .h(px(22.))
                .px(px(9.))
                .flex()
                .items_center()
                .rounded_full()
                .text_size(px(12.))
                .font_weight(gpui::FontWeight::MEDIUM)
                .bg(gpui::rgba((p.warning << 8) | 0x1f))
                .text_color(rgb(p.warning))
                .child(card.verification.clone())
                .automation(AutomationRole::Status, card.verification),
        )
    })
    .child(
        controls::icon("icons/chevron-down.svg", 14.)
            .text_color(rgb(p.subtle))
            .with_transformation(gpui::Transformation::rotate(gpui::radians(
                -std::f32::consts::FRAC_PI_2,
            ))),
    )
}

fn quota_summary(key: &str, quota: &Quota) -> gpui::AnyElement {
    let p = ZORK_UI.palette;
    div()
        .id(format!("profile-quota-summary-{key}"))
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(10.))
        .min_w_0()
        .flex_shrink_0()
        .text_size(px(12.))
        .when(quota.failed, |v| {
            v.text_color(rgb(p.warning)).child(quota.summary.clone())
        })
        .when(!quota.failed, |v| {
            v.children(
                quota
                    .windows
                    .iter()
                    .take(2)
                    .enumerate()
                    .map(|(index, window)| {
                        // Ink by default; colour only when the window runs low.
                        let color = if window.remaining < 10. {
                            p.danger
                        } else if window.remaining < 30. {
                            p.warning
                        } else {
                            p.text
                        };
                        let label = format!("{} · {}", window.label, window.value);
                        let hint = window
                            .reset
                            .as_ref()
                            .map(|reset| format!("{label} · {reset}"))
                            .unwrap_or_else(|| label.clone());
                        let indicator = div()
                            .id(format!("profile-quota-window-{key}-{index}"))
                            .flex()
                            .flex_shrink_0()
                            .items_center()
                            .gap(px(6.))
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(p.subtle))
                                    .child(window.short_label.clone()),
                            )
                            .child(
                                div()
                                    .w(px(56.))
                                    .h(px(5.))
                                    .rounded_full()
                                    .bg(rgb(p.prompt))
                                    .overflow_hidden()
                                    .child(
                                        div()
                                            .h_full()
                                            .rounded_full()
                                            .w(px(56. * (window.remaining / 100.).clamp(0., 1.)))
                                            .bg(rgb(color)),
                                    ),
                            )
                            .child(
                                div()
                                    .text_size(px(12.))
                                    .text_color(rgb(if color == p.text { p.muted } else { color }))
                                    .child(window.center_value.clone()),
                            )
                            .automation(AutomationRole::Status, label);
                        tooltip::hint(
                            indicator,
                            format!("profile-quota-reset-{key}-{index}"),
                            hint,
                        )
                    }),
            )
            .when_some(quota.balance.clone(), |v, balance| {
                v.child(
                    div()
                        .flex_shrink_0()
                        .font_weight(gpui::FontWeight::MEDIUM)
                        .text_color(rgb(p.text))
                        .child(balance),
                )
            })
        })
        .automation(AutomationRole::Status, quota.summary.clone())
        .into_any_element()
}

