//! Cross-device presentation of the shared, core-owned profile catalogs.
use super::{profiles::ProfilesView, ui};
#[cfg(feature = "headless-bench")]
use crate::api::StationClient;
use crate::{
    api::ProfileInfo,
    automation::{AutomationElementExt, AutomationRole},
    design::ZORK_UI,
};
use gpui::{div, prelude::*, px, rgb, Context, Entity, Task, Window};
use std::{collections::BTreeMap, sync::Arc};

type Source = (
    String,
    String,
    Arc<crate::api::Profiles>,
    zork_ui::device_name::DeviceStatus,
);
struct Device {
    id: String,
    name: String,
    status: zork_ui::device_name::DeviceStatus,
    source: Arc<crate::api::Profiles>,
    editor: Entity<ProfilesView>,
    _updates: Task<()>,
}
struct ConnectionRow {
    device_id: String,
    device_name: String,
    device_status: zork_ui::device_name::DeviceStatus,
    provider_label: String,
    profile: ProfileInfo,
    providers: Arc<Vec<serde_json::Value>>,
    quota_failed: bool,
}
type Groups = BTreeMap<String, Vec<ConnectionRow>>;
fn append_groups(
    groups: &mut Groups,
    device_id: &str,
    device_name: &str,
    device_status: &zork_ui::device_name::DeviceStatus,
    state: &crate::api::ProfileData,
) {
    for profile in state.profiles.iter() {
        let provider = state
            .providers
            .iter()
            .find(|p| p["id"] == profile.provider)
            .and_then(|p| p["label"].as_str())
            .unwrap_or(&profile.provider);
        groups
            .entry(profile.provider.clone())
            .or_default()
            .push(ConnectionRow {
                device_id: device_id.to_owned(),
                device_name: device_name.to_owned(),
                device_status: device_status.clone(),
                provider_label: provider.to_owned(),
                profile: profile.clone(),
                providers: state.providers.clone(),
                quota_failed: state.failed.contains(&profile.profile_id),
            });
    }
}
pub struct ModelSettings {
    devices: Vec<Device>,
    adding: bool,
    modal: ui::ModalState,
    selected: Option<String>,
    onboarding_local: Option<String>,
}
impl ModelSettings {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            devices: vec![],
            adding: false,
            modal: ui::ModalState::new(cx),
            selected: None,
            onboarding_local: None,
        }
    }
    #[cfg(feature = "headless-bench")]
    pub fn onboarding_fixture(cx: &mut Context<Self>) -> Self {
        let mut fixture = zork_ui::stories::page_fixture();
        fixture["profile"] = serde_json::Value::Null;
        let client = Arc::new(StationClient::fixture(
            fixture,
            serde_json::from_str(include_str!("../../tests/fixtures/provider_catalog.json"))
                .expect("provider fixture"),
        ));
        let mut view = Self::new(cx);
        view.set_sources(
            vec![(
                "mini1".into(),
                "mini1".into(),
                crate::api::Profiles::new(client),
                zork_ui::device_name::DeviceStatus::Direct,
            )],
            cx,
        );
        for device in &view.devices {
            device.editor.update(cx, |editor, cx| {
                editor.freeze_onboarding_catalog();
                cx.notify();
            });
        }
        view
    }
    pub fn has_device(&self, id: &str) -> bool {
        self.devices.iter().any(|device| device.id == id)
    }
    pub fn set_onboarding_local(&mut self, id: Option<String>, cx: &mut Context<Self>) {
        if self.onboarding_local == id {
            return;
        }
        if let Some(id) = &id {
            self.selected = Some(id.clone());
            self.adding = false;
        }
        self.onboarding_local = id;
        cx.notify();
    }
    pub fn begin_onboarding(&mut self, id: &str, cx: &mut Context<Self>) -> bool {
        if !self.has_device(id) {
            return false;
        }
        self.set_onboarding_local(Some(id.into()), cx);
        self.add_local_connection(cx);
        true
    }
    fn add_local_connection(&mut self, cx: &mut Context<Self>) {
        let Some(id) = &self.onboarding_local else {
            return;
        };
        self.selected = Some(id.clone());
        if let Some(device) = self.devices.iter().find(|device| &device.id == id) {
            device
                .editor
                .update(cx, |editor, cx| editor.add_connection(cx));
        }
        cx.notify();
    }
    fn add_on_device(&mut self, id: &str, cx: &mut Context<Self>) {
        self.selected = Some(id.to_owned());
        self.adding = false;
        if let Some(device) = self.devices.iter().find(|device| device.id == id) {
            device
                .editor
                .update(cx, |editor, cx| editor.add_connection(cx));
        }
        cx.notify();
    }
    fn render_group(
        &self,
        provider_id: String,
        mut rows: Vec<ConnectionRow>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let palette = ZORK_UI.palette;
        rows.sort_by(|a, b| {
            a.profile
                .display_name()
                .cmp(b.profile.display_name())
                .then_with(|| a.device_name.cmp(&b.device_name))
                .then_with(|| a.device_id.cmp(&b.device_id))
                .then_with(|| a.profile.profile_id.cmp(&b.profile.profile_id))
        });
        let provider_label = rows
            .first()
            .map(|row| row.provider_label.clone())
            .unwrap_or_else(|| provider_id.clone());
        let profile_count = rows.len();
        ui::section()
            .gap_2()
            .child(
                div()
                    .id(format!("model-provider-{provider_id}"))
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(zork_ui::controls::provider_icon(&provider_id, 22.))
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(provider_label.clone()),
                    )
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(rgb(palette.muted))
                            .child(format!("{profile_count} 个 Profile")),
                    )
                    .automation(
                        AutomationRole::Status,
                        format!("{provider_label} · {profile_count} 个 Profile"),
                    ),
            )
            .children(rows.into_iter().map(|row| {
                let Some(device) = self.devices.iter().find(|d| d.id == row.device_id) else {
                    return gpui::Empty.into_any_element();
                };
                let editor = device.editor.clone();
                let target = editor.clone();
                let device_id = row.device_id.clone();
                let profile_id = row.profile.profile_id.clone();
                let on_click = cx.listener(move |v, _, _, cx| {
                    v.selected = Some(device_id.clone());
                    target.update(cx, |editor, cx| {
                        editor.open_model(profile_id.clone(), None, cx)
                    });
                    cx.notify();
                });
                let card = editor.read_with(cx, |view, _| {
                    view.render_profile_row_with_click(
                        &row.profile,
                        &row.providers,
                        row.quota_failed,
                        Some(&row.device_name),
                        Some(&row.device_status),
                        on_click,
                    )
                });
                card
            }))
            .into_any_element()
    }
    #[cfg(feature = "headless-bench")]
    pub fn headless_selection(&self, cx: &gpui::App) -> serde_json::Value {
        self.selected.as_ref().and_then(|id| self.devices.iter().find(|d| &d.id == id))
            .map(|d| serde_json::json!({"device": d.id, "editor": d.editor.read(cx).headless_state(cx)}))
            .unwrap_or_default()
    }
    pub fn set_locale(&mut self, locale: crate::i18n::Locale, cx: &mut Context<Self>) {
        for device in &self.devices {
            device
                .editor
                .update(cx, |view, cx| view.set_locale(locale, cx));
        }
        cx.notify();
    }
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
        if !visible && self.adding {
            self.adding = false;
            cx.notify();
        }
        for device in &self.devices {
            let visible = visible
                && self
                    .onboarding_local
                    .as_ref()
                    .is_none_or(|id| id == &device.id);
            device
                .editor
                .update(cx, |view, cx| view.set_visible(visible, cx));
        }
    }
    pub fn set_sources(&mut self, sources: Vec<Source>, cx: &mut Context<Self>) {
        self.devices.retain(|device| {
            sources
                .iter()
                .any(|(id, _, source, _)| id == &device.id && Arc::ptr_eq(source, &device.source))
        });
        if self
            .selected
            .as_ref()
            .is_some_and(|id| !self.devices.iter().any(|d| &d.id == id))
        {
            self.selected = None;
        }
        for (id, name, source, status) in sources {
            if let Some(device) = self.devices.iter_mut().find(|d| d.id == id) {
                device.name = name.clone();
                device.status = status.clone();
                device.editor.update(cx, |v, cx| {
                    v.set_device_name(name);
                    v.set_device_status(status, cx);
                });
                continue;
            }
            let editor = cx.new(|cx| {
                let mut view = ProfilesView::editor(source.clone(), id.clone(), name.clone(), cx);
                view.set_device_status(status.clone(), cx);
                view
            });
            let mut updates = source.subscribe_state();
            let _updates = cx.spawn(async move |this, cx| {
                while updates.changed().await.is_some() {
                    if this.update(cx, |_, cx| cx.notify()).is_err() {
                        return;
                    }
                }
            });
            self.devices.push(Device {
                id,
                name,
                status,
                source,
                editor,
                _updates,
            });
        }
        cx.notify();
    }
}
impl Render for ModelSettings {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.modal
            .sync(self.adding.then_some("model-device-dialog"), window, cx);
        let chooser_visible = self
            .modal
            .retain("model-device-dialog", self.adding.then_some(()), cx)
            .is_some();
        // These groups are rebuilt from core snapshots for presentation only.
        let mut groups = Groups::new();
        let mut notices = Vec::new();
        let mut connection_count = 0;
        for device in self.devices.iter().filter(|device| {
            self.onboarding_local
                .as_ref()
                .is_none_or(|id| id == &device.id)
        }) {
            let state = device.source.snapshot();
            connection_count += state.profiles.len();
            let name = zork_ui::device_name::summary(&device.name, &device.status, None);
            if state.loading {
                notices.push((device.id.clone(), format!("{name} · 正在加载模型…"), false));
            } else if let Some(error) = &state.error {
                notices.push((
                    device.id.clone(),
                    format!("{name} · 读取失败：{error}"),
                    true,
                ));
            }
            append_groups(
                &mut groups,
                &device.id,
                &device.name,
                &device.status,
                &state,
            );
        }
        let has_notices = !notices.is_empty();
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(ui::page_title("模型连接"))
                    .child(
                        ui::page_action("models-add", "添加连接")
                            .on_click(cx.listener(|v, _, _, cx| {
                                if v.onboarding_local.is_some() {
                                    v.add_local_connection(cx);
                                } else if v.devices.len() == 1 {
                                    let id = v.devices[0].id.clone();
                                    v.add_on_device(&id, cx);
                                } else {
                                    v.adding = true;
                                }
                                cx.notify();
                            }))
                            .automation(AutomationRole::Button, "添加连接"),
                    ),
            )
            // A device that cannot be read keeps a retry beside its reason, so a
            // failed load never reads as a device without connections.
            .children(notices.into_iter().map(|(id, text, failed)| {
                if !failed {
                    return ui::status_notice(text, ui::NoticeKind::Loading);
                }
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(
                        div()
                            .flex_1()
                            .min_w(px(0.))
                            .child(ui::status_notice(text, ui::NoticeKind::Warning)),
                    )
                    .child(
                        ui::button(format!("models-retry-{id}"), "重试", false, true)
                            .on_click(cx.listener(move |v, _, _, cx| {
                                if let Some(device) = v.devices.iter().find(|d| d.id == id) {
                                    device.editor.update(cx, |view, cx| view.refresh(cx));
                                }
                            }))
                            .automation(AutomationRole::Button, "重试读取模型连接"),
                    )
            }))
            .when(connection_count == 0 && !has_notices, |v| {
                v.child(
                    ui::section().child(ui::label("暂无模型连接")).child(
                        div()
                            .text_color(rgb(ZORK_UI.palette.muted))
                            .child("添加订阅账号或 API 连接后，会显示在这里。"),
                    ),
                )
            })
            .children(
                groups
                    .into_iter()
                    .map(|(provider, rows)| self.render_group(provider, rows, cx)),
            )
            .children(self.devices.iter().filter_map(|device| {
                self.onboarding_local
                    .as_ref()
                    .is_none_or(|id| id == &device.id)
                    .then(|| device.editor.clone())
            }))
            .when(chooser_visible, |v| {
                v.child(ui::detail_modal_sized(
                    "model-device-dialog",
                    "选择保存连接的设备",
                    ui::section()
                        .border_t_0()
                        .py_0()
                        .when(self.devices.is_empty(), |v| v.child("请先连接设备"))
                        .children(self.devices.iter().map(|device| {
                            let id = device.id.clone();
                            ui::quiet_button(
                                format!("model-add-device-{id}"),
                                "",
                                true,
                                ui::IconButtonSize::Standard,
                            )
                            .w_full()
                            .h(px(40.))
                            .justify_start()
                            .child(zork_ui::device_name::label(
                                format!("model-add-device-name-{id}"),
                                device.name.clone(),
                                &device.status,
                                None,
                            ))
                            .on_click(cx.listener(move |v, _, _, cx| {
                                v.add_on_device(&id, cx);
                            }))
                            .automation(
                                AutomationRole::Button,
                                format!(
                                    "添加连接到 {}",
                                    zork_ui::device_name::accessible_summary(
                                        &device.name,
                                        &device.status,
                                        None
                                    )
                                ),
                            )
                        })),
                    None,
                    &self.modal,
                    ui::DIALOG_FORM_WIDTH,
                    window,
                    cx,
                    true,
                    |v, _, cx| {
                        v.adding = false;
                        cx.notify();
                    },
                ))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use zork_ui::device_name::DeviceStatus;

    #[test]
    fn provider_groups_keep_profiles_and_sources_distinct() {
        let state = crate::api::ProfileData {
            profiles: Arc::new(vec![
                serde_json::from_value(json!({
                    "profile_id": "primary", "provider": "openai", "models": [
                        {"id": "shared-model", "enabled": false}
                    ]
                }))
                .unwrap(),
                serde_json::from_value(json!({
                    "profile_id": "secondary", "provider": "openai", "models": []
                }))
                .unwrap(),
            ]),
            ..Default::default()
        };
        let mut providers = Groups::new();
        for id in ["desktop", "laptop"] {
            append_groups(&mut providers, id, id, &DeviceStatus::Connected, &state);
        }
        assert_eq!(providers.len(), 1);
        assert_eq!(providers["openai"].len(), 4);
        for id in ["desktop", "laptop"] {
            for profile in ["primary", "secondary"] {
                assert!(providers["openai"]
                    .iter()
                    .any(|row| row.device_id == id && row.profile.profile_id == profile));
            }
        }
        assert_eq!(
            providers["openai"]
                .iter()
                .filter(|row| row.profile.models.is_empty())
                .count(),
            2
        );
    }

    #[test]
    fn empty_connections_remain_reachable_and_provider_labels_have_fallbacks() {
        let state = crate::api::ProfileData {
            profiles: Arc::new(vec![serde_json::from_value(json!({
                "profile_id": "new", "provider": "custom", "models": []
            }))
            .unwrap()]),
            ..Default::default()
        };
        let mut groups = Groups::new();
        append_groups(
            &mut groups,
            "node",
            "My device",
            &DeviceStatus::Offline,
            &state,
        );
        assert_eq!(groups["custom"][0].profile.profile_id, "new");
        assert_eq!(groups["custom"][0].device_name, "My device");
        assert_eq!(groups["custom"][0].device_status, DeviceStatus::Offline);
        assert_eq!(groups["custom"][0].provider_label, "custom");
    }
}
