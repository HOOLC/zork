//! Device identity and readiness, shared by navigation, selectors and settings.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::{loading, tooltip},
    design::ZORK_UI,
    resources::Text,
};
use gpui::{div, point, prelude::*, px, rgb, Hsla, PathBuilder};
pub use zork_client_types::device::DeviceStatus;

pub fn status_text(status: &DeviceStatus, locale: Option<&Text>) -> String {
    let (key, fallback) = match status {
        DeviceStatus::MeshNotStarted => ("device_mesh_not_started", "Mesh 未启动"),
        DeviceStatus::MeshPreparing => ("device_mesh_preparing", "Mesh 准备中"),
        DeviceStatus::MeshStopping => ("device_mesh_stopping", "Mesh 停止中"),
        DeviceStatus::MeshStopped => ("device_mesh_stopped", "Mesh 已停止"),
        DeviceStatus::MeshFailed(_) => ("device_mesh_failed", "Mesh 启动失败"),
        DeviceStatus::NotConnected => ("device_not_connected", "未连接"),
        DeviceStatus::Connecting => ("device_connecting", "连接中"),
        DeviceStatus::Direct => ("device_direct", "直连"),
        DeviceStatus::Relay => ("device_relay", "中继"),
        DeviceStatus::Connected => ("device_connected", "已连接"),
        DeviceStatus::Offline => ("device_offline", "离线"),
        DeviceStatus::Revoked => ("device_revoked", "访问已撤销"),
    };
    locale
        .map(|text| text.text(key))
        .filter(|text| text != key)
        .unwrap_or_else(|| fallback.into())
}

/// Compact symbol for text-only hosts that cannot render the full indicator.
pub fn status_symbol(status: &DeviceStatus) -> &'static str {
    match status {
        DeviceStatus::Direct | DeviceStatus::Connected => "●",
        DeviceStatus::Relay => "◉",
        DeviceStatus::MeshPreparing | DeviceStatus::MeshStopping | DeviceStatus::Connecting => "◌",
        DeviceStatus::MeshFailed(_) | DeviceStatus::Revoked => "×",
        DeviceStatus::MeshNotStarted
        | DeviceStatus::MeshStopped
        | DeviceStatus::NotConnected
        | DeviceStatus::Offline => "○",
    }
}

pub fn summary(name: &str, status: &DeviceStatus, _locale: Option<&Text>) -> String {
    format!("{name} {}", status_symbol(status))
}

/// Full wording remains available for accessibility and on-demand details.
pub fn accessible_summary(name: &str, status: &DeviceStatus, locale: Option<&Text>) -> String {
    format!("{name} · {}", status_text(status, locale))
}

/// Short monogram for a device: its first letter plus its first digit, if any.
pub fn monogram(name: &str) -> String {
    let first = name.chars().find(|c| c.is_alphanumeric());
    let digit = name.chars().skip(1).find(|c| c.is_ascii_digit());
    first
        .into_iter()
        .flat_map(|c| c.to_uppercase())
        .chain(digit)
        .collect()
}

/// The device's identity mark: a stable hue with the brand's folded corner.
/// It is one path with its top-right corner cut, so it sits on any surface,
/// including rows whose hover and selection change the fill behind it.
pub fn mark(name: &str, size: f32) -> impl IntoElement {
    let hue = crate::design::device_hue(name);
    div()
        .relative()
        .flex_shrink_0()
        .size(px(size))
        .child(
            gpui::canvas(
                |_, _, _| {},
                move |bounds, _, window, _| {
                    let (x, y) = (bounds.origin.x, bounds.origin.y);
                    let (w, h) = (bounds.size.width, bounds.size.height);
                    let (radius, handle) = crate::components::smooth::smooth_corner(
                        (size * 0.3).round(),
                        size / 2.,
                    );
                    let r = px(radius);
                    let k = px(radius * handle);
                    let f = px((size / 3.).round());
                    let mut body = PathBuilder::fill();
                    body.move_to(point(x + r, y));
                    body.line_to(point(x + w - f, y));
                    body.line_to(point(x + w, y + f));
                    body.line_to(point(x + w, y + h - r));
                    body.cubic_bezier_to(
                        point(x + w - r, y + h),
                        point(x + w, y + h - r + k),
                        point(x + w - r + k, y + h),
                    );
                    body.line_to(point(x + r, y + h));
                    body.cubic_bezier_to(
                        point(x, y + h - r),
                        point(x + r - k, y + h),
                        point(x, y + h - r + k),
                    );
                    body.line_to(point(x, y + r));
                    body.cubic_bezier_to(point(x + r, y), point(x, y + r - k), point(x + r - k, y));
                    body.close();
                    if let Ok(path) = body.build() {
                        window.paint_path(path, Hsla::from(rgb(hue)));
                    }
                    let mut flap = PathBuilder::fill();
                    flap.move_to(point(x + w - f, y));
                    flap.line_to(point(x + w, y + f));
                    flap.line_to(point(x + w - f, y + f));
                    flap.close();
                    if let Ok(path) = flap.build() {
                        window.paint_path(path, gpui::hsla(0., 0., 1., 0.45));
                    }
                },
            )
            .absolute()
            .inset_0(),
        )
        .child(
            div()
                .absolute()
                .inset_0()
                .flex()
                .items_center()
                .justify_center()
                .text_size(px((size * 0.56).round()))
                .line_height(px(size))
                .font_weight(gpui::FontWeight::SEMIBOLD)
                .text_color(rgb(0xFFFFFF))
                .child(monogram(name)),
        )
}

/// Status never relies on color alone: online is a filled dot, relay a half
/// dot, offline a ring, failure a cross; anything but the normal state also
/// carries its short wording.
fn indicator(id: &gpui::ElementId, status: &DeviceStatus) -> gpui::AnyElement {
    let p = ZORK_UI.palette;
    match status {
        DeviceStatus::MeshPreparing | DeviceStatus::MeshStopping | DeviceStatus::Connecting => {
            loading::indicator(format!("device-status-loading-{id:?}"), 12.)
                .without_delay()
                .into_any_element()
        }
        DeviceStatus::Direct | DeviceStatus::Connected => div()
            .size(px(8.))
            .rounded_full()
            .bg(rgb(p.success))
            .into_any_element(),
        // A ring with a center dot (◉): readable at 9 px, unlike a half fill.
        DeviceStatus::Relay => div()
            .size(px(9.))
            .rounded_full()
            .border(px(1.5))
            .border_color(rgb(p.warning))
            .flex()
            .items_center()
            .justify_center()
            .child(div().size(px(3.)).rounded_full().bg(rgb(p.warning)))
            .into_any_element(),
        DeviceStatus::MeshFailed(_) | DeviceStatus::Revoked => gpui::svg()
            .path("icons/x.svg")
            .size(px(10.))
            .text_color(rgb(p.danger))
            .into_any_element(),
        _ => div()
            .size(px(8.))
            .rounded_full()
            .border(px(1.5))
            .border_color(rgb(p.subtle))
            .into_any_element(),
    }
}

pub fn label(
    id: impl Into<gpui::ElementId>,
    name: impl Into<String>,
    status: &DeviceStatus,
    locale: Option<&Text>,
) -> impl IntoElement {
    let id = id.into();
    let name = name.into();
    let text = status_text(status, locale);
    let p = ZORK_UI.palette;
    let normal = matches!(status, DeviceStatus::Direct | DeviceStatus::Connected);
    let ink = match status {
        DeviceStatus::MeshFailed(_) | DeviceStatus::Revoked => p.danger,
        DeviceStatus::Relay => p.warning,
        _ => p.subtle,
    };
    let indicator = indicator(&id, status);
    let detail = match status {
        DeviceStatus::MeshFailed(error) => format!("{text}：{error}"),
        _ => text.clone(),
    };
    let badge = div()
        .id(format!("device-status-{id:?}"))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(5.))
        .child(indicator)
        .when(!normal, |v| {
            v.child(
                div()
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .text_color(rgb(ink))
                    .whitespace_nowrap()
                    .child(text.clone()),
            )
        })
        .automation(AutomationRole::Status, detail.clone());
    let badge = tooltip::hint(badge, format!("device-status-{id:?}"), detail.clone());
    div()
        .id(id)
        .min_w_0()
        .flex()
        .items_center()
        .gap_2()
        .child(mark(&name, 18.))
        .child(div().min_w_0().text_ellipsis().child(name.clone()))
        .child(badge)
        .automation(AutomationRole::Status, format!("{name} · {detail}"))
}
