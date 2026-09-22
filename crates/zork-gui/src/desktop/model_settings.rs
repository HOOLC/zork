//! Cross-device presentation of the shared, core-owned profile catalogs.
use super::{profiles::ProfilesView, ui};
use crate::{
    automation::{AutomationElementExt, AutomationRole},
    design::ZORK_UI,
};
use gpui::{div, prelude::*, rgb, Context, Entity, Task, Window};
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
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Grouping {
    #[default]
    Provider,
    Model,
}
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ModelRow {
    device_id: String,
    profile_id: String,
    model: Option<(String, bool)>,
    origin: String,
}
type Groups = BTreeMap<String, Vec<ModelRow>>;
fn append_groups(
    groups: &mut Groups,
    grouping: Grouping,
    device_id: &str,
    device_name: &str,
    state: &crate::api::ProfileData,
) {
    for profile in state.profiles.iter() {
        let provider = state
            .providers
            .iter()
            .find(|p| p["id"] == profile.provider)
            .and_then(|p| p["label"].as_str())
            .unwrap_or(&profile.provider);
        let origin = format!(
            "{} · {} · {}",
            provider,
            profile.display_name(),
            device_name
        );
        if profile.models.is_empty() {
            let group = if grouping == Grouping::Provider {
                provider
            } else {
                "待配置模型"
            };
            groups.entry(group.into()).or_default().push(ModelRow {
                device_id: device_id.to_owned(),
                profile_id: profile.profile_id.clone(),
                model: None,
                origin: origin.clone(),
            });
        }
        for model in &profile.models {
            let group = if grouping == Grouping::Provider {
                provider
            } else {
                &model.id
            };
            groups.entry(group.into()).or_default().push(ModelRow {
                device_id: device_id.to_owned(),
                profile_id: profile.profile_id.clone(),
                model: Some((model.id.clone(), model.enabled)),
                origin: origin.clone(),
            });
        }
    }
}
pub struct ModelSettings {
    devices: Vec<Device>,
    grouping: Grouping,
    adding: bool,
    selected: Option<String>,
    onboarding_local: Option<String>,
}
impl ModelSettings {
    pub fn new(_: &mut Context<Self>) -> Self {
        Self {
            devices: vec![],
            grouping: Grouping::default(),
            adding: false,
            selected: None,
            onboarding_local: None,
        }
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
    }
    pub fn set_visible(&mut self, visible: bool, cx: &mut Context<Self>) {
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
                let mut view = ProfilesView::editor(source.clone(), name.clone(), cx);
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
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = ZORK_UI.palette;
        let grouping_hint = match self.grouping {
            Grouping::Provider => "当前按供应商分组，点击切换为按模型分组",
            Grouping::Model => "当前按模型分组，点击切换为按供应商分组",
        };
        // This is a read-only rendering projection, never another editable catalog.
        let mut groups = Groups::new();
        let mut notices = Vec::new();
        for device in self.devices.iter().filter(|device| {
            self.onboarding_local
                .as_ref()
                .is_none_or(|id| id == &device.id)
        }) {
            let state = device.source.snapshot();
            let name = zork_ui::device_name::summary(&device.name, &device.status, None);
            if state.loading {
                notices.push(format!("{name} · 正在加载模型…"));
            } else if let Some(error) = &state.error {
                notices.push(format!("{name} · {error}"));
            }
            append_groups(&mut groups, self.grouping, &device.id, &name, &state);
        }

        let empty = groups.is_empty() && notices.is_empty();
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(ui::page_title("模型设置"))
                    .child(
                        ui::page_action("models-add", "添加连接")
                            .on_click(cx.listener(|v, _, _, cx| {
                                if v.onboarding_local.is_some() {
                                    v.add_local_connection(cx);
                                } else {
                                    v.adding = !v.adding;
                                }
                                cx.notify();
                            }))
                            .automation(AutomationRole::Button, "添加连接"),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .gap_4()
                    .child(div().text_color(rgb(palette.muted)).child(
                        if self.onboarding_local.is_some() {
                            "本机的模型与连接"
                        } else {
                            "所有设备的模型与连接"
                        },
                    ))
                    .when(self.onboarding_local.is_none(), |v| {
                        v.child(zork_ui::components::tooltip::hint(
                            ui::icon_button("model-grouping-toggle", true)
                                .child(ui::icon("icons/grouping.svg", 16.))
                                .aria_label(grouping_hint)
                                .on_click(cx.listener(|v, _, _, cx| {
                                    v.grouping = match v.grouping {
                                        Grouping::Provider => Grouping::Model,
                                        Grouping::Model => Grouping::Provider,
                                    };
                                    cx.notify();
                                }))
                                .automation(AutomationRole::Button, grouping_hint),
                            "model-grouping-toggle",
                            grouping_hint,
                        ))
                    }),
            )
            .when(self.adding && self.onboarding_local.is_none(), |v| {
                v.child(
                    ui::section()
                        .child(ui::label("选择保存连接的设备"))
                        .when(self.devices.is_empty(), |v| v.child("请先连接设备"))
                        .children(self.devices.iter().map(|device| {
                            let id = device.id.clone();
                            ui::button(format!("model-add-device-{id}"), "", false, true)
                                .child(zork_ui::device_name::label(
                                    format!("model-add-device-name-{id}"),
                                    device.name.clone(),
                                    &device.status,
                                    None,
                                ))
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    v.selected = Some(id.clone());
                                    v.adding = false;
                                    if let Some(device) = v.devices.iter().find(|d| d.id == id) {
                                        device
                                            .editor
                                            .update(cx, |editor, cx| editor.add_connection(cx));
                                    }
                                    cx.notify();
                                }))
                                .automation(
                                    AutomationRole::Button,
                                    format!(
                                        "添加连接到 {}",
                                        zork_ui::device_name::summary(
                                            &device.name,
                                            &device.status,
                                            None
                                        )
                                    ),
                                )
                        })),
                )
            })
            .children(notices.into_iter().map(ui::feedback))
            .when(empty, |v| v.child("暂无模型，添加连接后即可配置。"))
            .children(
                groups
                    .into_iter()
                    .enumerate()
                    .map(|(group_index, (label, rows))| {
                        ui::section().child(ui::label(label)).children(
                            rows.into_iter().enumerate().map(|(row_index, row)| {
                                let ModelRow {
                                    device_id,
                                    profile_id,
                                    model,
                                    origin,
                                } = row;
                                let label = model
                                    .as_ref()
                                    .map(|(id, _)| id.as_str())
                                    .unwrap_or("待配置模型");
                                let disabled = model.as_ref().is_some_and(|(_, enabled)| !enabled);
                                let automation = format!("{} · {}", label, origin);
                                ui::quiet_button(
                                    format!("model-entry-{group_index}-{row_index}"),
                                    "",
                                    true,
                                    ui::IconButtonSize::Standard,
                                )
                                .radius(ui::FIELD_RADIUS)
                                .font_weight(gpui::FontWeight::NORMAL)
                                .w_full()
                                .h_auto()
                                .py_3()
                                .justify_start()
                                .child(
                                    div()
                                        .flex()
                                        .flex_col()
                                        .items_start()
                                        .gap_1()
                                        .child(format!(
                                            "{}{}",
                                            label,
                                            if disabled { " · 未启用" } else { "" }
                                        ))
                                        .child(div().text_color(rgb(palette.muted)).child(origin)),
                                )
                                .on_click(cx.listener(move |v, _, _, cx| {
                                    v.selected = Some(device_id.clone());
                                    if let Some(device) =
                                        v.devices.iter().find(|d| d.id == device_id)
                                    {
                                        device.editor.update(cx, |editor, cx| {
                                            editor.open_model(
                                                profile_id.clone(),
                                                model.as_ref().map(|(id, _)| id.clone()),
                                                cx,
                                            )
                                        });
                                    }
                                    cx.notify();
                                }))
                                .automation(AutomationRole::Button, automation)
                            }),
                        )
                    }),
            )
            .when_some(
                self.selected
                    .as_ref()
                    .and_then(|id| self.devices.iter().find(|d| &d.id == id))
                    .map(|d| d.editor.clone()),
                |v, editor| v.child(editor),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn grouping_preserves_same_model_and_connection_ids_on_distinct_devices() {
        let state = crate::api::ProfileData {
            profiles: Arc::new(vec![serde_json::from_value(json!({
                "profile_id": "primary", "provider": "openai", "models": [
                    {"id": "shared-model", "enabled": false}, {"id": "other-model"}
                ]
            }))
            .unwrap()]),
            ..Default::default()
        };
        let mut providers = Groups::new();
        let mut models = Groups::new();
        for id in ["desktop", "laptop"] {
            append_groups(&mut providers, Grouping::Provider, id, id, &state);
            append_groups(&mut models, Grouping::Model, id, id, &state);
        }
        assert_eq!(providers["openai"].len(), 4);
        assert_eq!(models["shared-model"].len(), 2);
        assert_eq!(models["shared-model"][0].device_id, "desktop");
        assert_eq!(models["shared-model"][1].device_id, "laptop");
        assert!(!models["shared-model"][0].model.as_ref().unwrap().1);
        assert!(models["other-model"][0].model.as_ref().unwrap().1);
        let mut before = providers.into_values().flatten().collect::<Vec<_>>();
        let mut after = models.into_values().flatten().collect::<Vec<_>>();
        before.sort();
        after.sort();
        assert_eq!(before, after);
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
        append_groups(&mut groups, Grouping::Provider, "node", "My device", &state);
        assert_eq!(groups["custom"][0].profile_id, "new");
        assert!(groups["custom"][0].origin.contains("My device"));
        groups.clear();
        append_groups(&mut groups, Grouping::Model, "node", "My device", &state);
        assert_eq!(groups["待配置模型"][0].profile_id, "new");
    }
}
