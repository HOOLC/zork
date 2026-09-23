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
    pub name: String,
    pub device: Option<DeviceIdentity>,
    pub billing: String,
    pub model_count: String,
    pub verified: bool,
    pub verification: String,
    pub quota: Option<Quota>,
}

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
    .radius(controls::FIELD_RADIUS)
    .font_weight(gpui::FontWeight::NORMAL)
    .justify_start()
    .w_full()
    .on_click(on_click)
    .min_h(px(80.))
    .px(px(12.))
    .py(px(8.))
    .rounded(px(controls::FIELD_RADIUS))
    .flex()
    .items_center()
    .gap_3()
    .child(
        div()
            .id(format!("profile-avatar-{}", card.key))
            .size(px(40.))
            .flex_shrink_0()
            .rounded(px(controls::FIELD_RADIUS))
            .bg(rgb(p.sidebar))
            .flex()
            .items_center()
            .justify_center()
            .child(provider_icon(&card.provider, 24.))
            .automation(AutomationRole::Status, card.provider.clone()),
    )
    .child(
        div()
            .flex_1()
            .min_w_0()
            .flex()
            .flex_col()
            .gap_0()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .text_size(px(13.))
                            .font_weight(gpui::FontWeight::MEDIUM)
                            .child(card.name.clone()),
                    )
                    .when_some(card.device.clone(), |v, device| {
                        v.child(
                            div()
                                .flex_shrink_0()
                                .max_w(px(150.))
                                .min_w_0()
                                .text_size(px(11.))
                                .text_color(rgb(p.muted))
                                .child(device_name::label(
                                    format!("profile-device-{}", card.key),
                                    device.name,
                                    &device.status,
                                    None,
                                )),
                        )
                    }),
            )
            .child(
                div()
                    .id(format!("profile-billing-{}", card.key))
                    .truncate()
                    .text_size(px(11.))
                    .line_height(px(16.))
                    .text_color(rgb(p.muted))
                    .child(card.billing.clone())
                    .automation(AutomationRole::Status, card.billing.clone()),
            )
            .when_some(
                card.quota
                    .as_ref()
                    .filter(|quota| !quota.summary.is_empty()),
                |v, quota| v.child(quota_summary(&card.key, quota)),
            ),
    )
    .child(
        div()
            .id(format!("profile-model-count-{}", card.key))
            .text_size(px(11.))
            .text_color(rgb(p.muted))
            .child(card.model_count.clone())
            .automation(AutomationRole::Status, card.model_count),
    )
    .child(
        div()
            .id(format!("profile-verification-{}", card.key))
            .px_2()
            .py_1()
            .rounded_full()
            .text_size(px(11.))
            .bg(gpui::rgba(
                ((if card.verified { p.success } else { p.warning }) << 8) | 0x12,
            ))
            .text_color(rgb(if card.verified { p.success } else { p.warning }))
            .child(card.verification.clone())
            .automation(AutomationRole::Status, card.verification),
    )
}

fn quota_summary(key: &str, quota: &Quota) -> gpui::AnyElement {
    let p = ZORK_UI.palette;
    div()
        .id(format!("profile-quota-summary-{key}"))
        .flex()
        .flex_wrap()
        .items_center()
        .gap(px(12.))
        .min_w_0()
        .mt(px(3.))
        .text_size(px(11.))
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
                        let color = if window.remaining == 0. {
                            p.danger
                        } else if window.remaining < 20. {
                            p.warning
                        } else {
                            p.success
                        };
                        let size = 24.;
                        let track = ring_path(size, 1.);
                        let progress = ring_path(size, window.remaining / 100.);
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
                            .gap(px(5.))
                            .child(
                                div()
                                    .text_size(px(10.))
                                    .font_weight(gpui::FontWeight::MEDIUM)
                                    .text_color(rgb(p.muted))
                                    .child(window.short_label.clone()),
                            )
                            .child(
                                div()
                                    .relative()
                                    .size(px(size))
                                    .flex_shrink_0()
                                    .flex()
                                    .items_center()
                                    .justify_center()
                                    .child(
                                        gpui::canvas(
                                            |_, _, _| {},
                                            move |bounds, _, window, _| {
                                                if let Some(track) = &track {
                                                    window.paint_path_at(
                                                        track,
                                                        bounds.origin,
                                                        gpui::rgba((p.muted << 8) | 0x30),
                                                    );
                                                }
                                                if let Some(progress) = &progress {
                                                    window.paint_path_at(
                                                        progress,
                                                        bounds.origin,
                                                        rgb(color),
                                                    );
                                                }
                                            },
                                        )
                                        .absolute()
                                        .inset_0(),
                                    )
                                    .child(
                                        div()
                                            .text_size(px(10.))
                                            .font_weight(gpui::FontWeight::SEMIBOLD)
                                            .text_color(rgb(color))
                                            .child(window.center_value.clone()),
                                    ),
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

fn ring_path(size: f32, fraction: f32) -> Option<gpui::Path<gpui::Pixels>> {
    const STROKE: f32 = 2.25;
    if !size.is_finite() || size <= STROKE || !fraction.is_finite() {
        return None;
    }
    let fraction = fraction.clamp(0., 1.);
    if fraction <= 0. {
        return None;
    }
    let steps = (fraction * 64.).ceil().max(2.) as usize;
    let center = size / 2.;
    let radius = (size - STROKE) / 2.;
    let mut path = gpui::PathBuilder::stroke(px(STROKE)).with_style(gpui::PathStyle::Stroke(
        gpui::StrokeOptions::default()
            .with_line_width(STROKE)
            .with_line_cap(lyon::path::LineCap::Round)
            .with_line_join(lyon::path::LineJoin::Round),
    ));
    for step in 0..=steps {
        let angle = -std::f32::consts::FRAC_PI_2
            - std::f32::consts::TAU * fraction * step as f32 / steps as f32;
        let point = gpui::point(
            px(center + radius * angle.cos()),
            px(center + radius * angle.sin()),
        );
        if step == 0 {
            path.move_to(point);
        } else {
            path.line_to(point);
        }
    }
    if fraction == 1. {
        path.close();
    }
    path.build().ok()
}
