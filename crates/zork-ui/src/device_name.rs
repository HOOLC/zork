//! Device identity and readiness, shared by navigation, selectors and settings.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::{loading, tooltip},
    design::ZORK_UI,
    resources::Text,
};
use gpui::{div, prelude::*, px, rgb};
pub use zork_client_types::device::DeviceStatus;

pub fn status_text(status: &DeviceStatus, locale: Option<&Text>) -> String {
    let (key, fallback) = match status {
        DeviceStatus::MeshNotStarted => ("device_mesh_not_started", "Mesh 未启动"),
        DeviceStatus::MeshPreparing => ("device_mesh_preparing", "Mesh 准备中"),
        DeviceStatus::MeshStopping => ("device_mesh_stopping", "Mesh 停止中"),
        DeviceStatus::MeshStopped => ("device_mesh_stopped", "Mesh 已停止"),
        DeviceStatus::MeshFailed(_) => ("device_mesh_failed", "Mesh 启动失败"),
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

/// Text-only hosts (menu details and tooltip rows) retain the same status wording.
pub fn summary(name: &str, status: &DeviceStatus, locale: Option<&Text>) -> String {
    format!("{name} · {}", status_text(status, locale))
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
    let ink = match status {
        DeviceStatus::Direct | DeviceStatus::Connected => p.success,
        DeviceStatus::MeshFailed(_) | DeviceStatus::Revoked => p.danger,
        DeviceStatus::Relay | DeviceStatus::MeshPreparing | DeviceStatus::Connecting => p.warning,
        _ => p.muted,
    };
    let busy = matches!(
        status,
        DeviceStatus::MeshPreparing | DeviceStatus::MeshStopping | DeviceStatus::Connecting
    );
    let indicator = if busy {
        loading::indicator("device-status-loading", 12.)
            .without_delay()
            .into_any_element()
    } else {
        div()
            .size(px(5.))
            .rounded_full()
            .bg(rgb(ink))
            .into_any_element()
    };
    let detail = match status {
        DeviceStatus::MeshFailed(error) => format!("{text}：{error}"),
        _ => text.clone(),
    };
    let badge = div()
        .id("device-status")
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap(px(5.))
        .text_size(px(11.))
        .font_weight(gpui::FontWeight::NORMAL)
        .text_color(rgb(ink))
        .child(indicator)
        .child(text)
        .automation(AutomationRole::Status, detail.clone());
    let badge = if matches!(status, DeviceStatus::MeshFailed(_)) {
        tooltip::hint(badge, format!("device-status-{id:?}"), detail.clone()).into_any_element()
    } else {
        badge.into_any_element()
    };
    div()
        .id(id)
        .min_w_0()
        .flex()
        .items_center()
        .gap_2()
        .child(div().min_w_0().text_ellipsis().child(name.clone()))
        .child(badge)
        .automation(AutomationRole::Status, format!("{name} · {detail}"))
}
