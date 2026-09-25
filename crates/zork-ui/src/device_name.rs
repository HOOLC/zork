//! Device identity and readiness, shared by navigation, selectors and settings.
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    components::{loading, tooltip},
    design::ZORK_UI,
    resources::Text,
};
use gpui::{div, point, prelude::*, px, rgb, Hsla, PathBuilder};
pub use zork_client_types::device::{DeviceName, DeviceStatus};

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

/// Text-only surfaces (menus, notices, row meta) name the device with the same
/// short status wording as the device-name component, never a bare glyph
/// that would take on the surrounding text colour.
pub fn summary(name: &str, status: &DeviceStatus, locale: Option<&Text>) -> String {
    format!("{name} · {}", status_text(status, locale))
}

/// Full wording remains available for accessibility and on-demand details,
/// including the machine name when it differs from the display name.
pub fn accessible_summary(
    name: impl Into<DeviceName>,
    status: &DeviceStatus,
    locale: Option<&Text>,
) -> String {
    format!("{} · {}", name.into().accessible(), status_text(status, locale))
}

/// Display names longer than this may be cut off in narrow rows, so their
/// hint repeats them in full.
const HINT_REPEATS_DISPLAY: usize = 10;

/// The hover hint of a device name: the machine name it registered with,
/// and the display name itself when it may be truncated.
pub fn name_hint(name: &DeviceName) -> Option<String> {
    let machine = name.machine.as_ref()?;
    Some(if name.display.chars().count() > HINT_REPEATS_DISPLAY {
        format!("{}\n机器名称：{machine}", name.display)
    } else {
        format!("机器名称：{machine}")
    })
}

/// The display name as text, hinting its machine name on hover.
pub fn name_text(id: &gpui::ElementId, name: &DeviceName) -> gpui::AnyElement {
    let text = div()
        .id(format!("device-name-{id}"))
        .min_w_0()
        .text_ellipsis()
        .child(name.display.clone());
    match name_hint(name) {
        Some(hint) => tooltip::hint(
            text.automation(AutomationRole::Status, name.accessible()),
            format!("device-name-{id}"),
            hint,
        )
        .into_any_element(),
        None => text.into_any_element(),
    }
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

thread_local! {
    /// Colour keys of the host's devices by the names surfaces print, for
    /// places that only carry a device name (message rows, activity, archives).
    static COLOR_KEYS: std::cell::RefCell<std::collections::HashMap<String, String>> =
        Default::default();
}

/// The host publishes each device's names with its core colour key, so every
/// mark of the same device has the same hue.
pub fn set_color_keys(keys: impl IntoIterator<Item = (String, String)>) {
    COLOR_KEYS.with(|map| *map.borrow_mut() = keys.into_iter().collect());
}

/// The colour key for a device known only by name; the name itself when the
/// host has not published one (stories, devices outside the directory).
pub fn color_key(name: &str) -> String {
    COLOR_KEYS
        .with(|map| map.borrow().get(name).cloned())
        .unwrap_or_else(|| name.to_owned())
}

/// The device's identity mark: a stable hue with the brand's folded corner.
/// It is one path with its top-right corner cut, so it sits on any surface,
/// including rows whose hover and selection change the fill behind it.
pub fn mark(name: &str, size: f32) -> impl IntoElement {
    mark_keyed(name, &color_key(name), size)
}

/// A mark whose colour follows an explicit stable key.
pub fn mark_keyed(name: &str, key: &str, size: f32) -> impl IntoElement {
    let hue = crate::design::device_hue(key);
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
    name: impl Into<DeviceName>,
    status: &DeviceStatus,
    locale: Option<&Text>,
) -> impl IntoElement {
    let id = id.into();
    let name: DeviceName = name.into();
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
    let text = name_text(&id, &name);
    div()
        .id(id)
        .min_w_0()
        .flex()
        .items_center()
        .gap_2()
        .child(mark_keyed(
            &name.display,
            &name.color.clone().unwrap_or_else(|| color_key(&name.display)),
            18.,
        ))
        .child(text)
        .child(badge)
        .automation(
            AutomationRole::Status,
            format!("{} · {detail}", name.accessible()),
        )
}
