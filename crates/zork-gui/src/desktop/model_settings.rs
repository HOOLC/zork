//! Cross-device presentation of the shared, core-owned profile catalogs.
use super::{profiles::ProfilesView, ui};
#[cfg(feature = "headless-bench")]
use crate::api::StationClient;
use crate::{
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
#[derive(Clone, Copy, PartialEq, Eq, Default)]
enum Grouping {
    #[default]
    Provider,
    Model,
}
#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct ModelRow {
    device_id: String,
    device_name: String,
    profile_id: String,
    profile_name: String,
    provider_id: String,
    provider_label: String,
    model: Option<(String, bool)>,
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
        let row = |model| ModelRow {
            device_id: device_id.to_owned(),
            device_name: device_name.to_owned(),
            profile_id: profile.profile_id.clone(),
            profile_name: profile.display_name().to_owned(),
            provider_id: profile.provider.clone(),
            provider_label: provider.to_owned(),
            model,
        };
        if profile.models.is_empty() {
            let group = if grouping == Grouping::Provider {
                provider
            } else {
                "待配置模型"
            };
            groups.entry(group.into()).or_default().push(row(None));
        }
        for model in &profile.models {
            let group = if grouping == Grouping::Provider {
                provider
            } else {
                &model.id
            };
            groups
                .entry(group.into())
                .or_default()
                .push(row(Some((model.id.clone(), model.enabled))));
        }
    }
}
pub struct ModelSettings {
    devices: Vec<Device>,
    grouping: Grouping,
    adding: bool,
    show_disabled: bool,
    modal: ui::ModalState,
    selected: Option<String>,
    onboarding_local: Option<String>,
}
impl ModelSettings {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            devices: vec![],
            grouping: Grouping::default(),
            adding: false,
            show_disabled: false,
            modal: ui::ModalState::new(cx),
            selected: None,
            onboarding_local: None,
        }
    }
    #[cfg(feature = "headless-bench")]
    pub fn onboarding_fixture(cx: &mut Context<Self>) -> Self {
        let mut fixture = zork_ui::stories::page_fixture();
        fixture["profile"] = serde_json::Value::Null;
        #[cfg(not(target_family = "wasm"))]
        let client = Arc::new(StationClient::fixture(
            fixture,
            serde_json::from_str(include_str!("../../tests/fixtures/provider_catalog.json"))
                .expect("provider fixture"),
        ));
        #[cfg(target_family = "wasm")]
        let client = Arc::new(StationClient::new("http://127.0.0.1:9", None));
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
        group_index: usize,
        key: String,
        rows: Vec<ModelRow>,
        grouping: Grouping,
        muted: bool,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let palette = ZORK_UI.palette;
        let provider = rows
            .first()
            .map(|row| row.provider_id.as_str())
            .unwrap_or("");
        let title = if grouping == Grouping::Provider {
            rows.first()
                .map(|row| row.provider_label.clone())
                .unwrap_or(key)
        } else {
            key
        };
        ui::section()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(if grouping == Grouping::Provider {
                        zork_ui::controls::provider_icon(provider, 22.).into_any_element()
                    } else {
                        ui::icon("icons/models.svg", 20.).into_any_element()
                    })
                    .child(
                        div()
                            .min_w_0()
                            .truncate()
                            .font_weight(gpui::FontWeight::SEMIBOLD)
                            .child(title),
                    )
                    .child(
                        div()
                            .text_size(px(11.))
                            .text_color(rgb(palette.muted))
                            .child(rows.len().to_string()),
                    ),
            )
            .children(rows.into_iter().enumerate().map(|(row_index, row)| {
                let ModelRow {
                    device_id,
                    device_name,
                    profile_id,
                    profile_name,
                    provider_id,
                    provider_label,
                    model,
                } = row;
                let model_id = model.as_ref().map(|(id, _)| id.clone());
                let title = if grouping == Grouping::Provider {
                    model_id.clone().unwrap_or_else(|| "配置模型".into())
                } else {
                    provider_label.clone()
                };
                let detail = (profile_name != provider_label).then_some(profile_name);
                let automation = format!(
                    "{}，{}，{}{}",
                    model_id.as_deref().unwrap_or("待配置模型"),
                    provider_label,
                    device_name,
                    if muted { "，未启用" } else { "" }
                );
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
                .py_2()
                .px_3()
                .justify_between()
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_3()
                        .min_w_0()
                        .when(grouping == Grouping::Model, |v| {
                            v.child(zork_ui::controls::provider_icon(&provider_id, 18.))
                        })
                        .child(
                            div()
                                .flex()
                                .flex_col()
                                .flex_1()
                                .min_w_0()
                                .items_start()
                                .gap_1()
                                .child(div().w_full().truncate().child(title))
                                .when_some(detail, |v, detail| {
                                    v.child(
                                        div()
                                            .w_full()
                                            .truncate()
                                            .text_size(px(11.))
                                            .text_color(rgb(palette.muted))
                                            .child(detail),
                                    )
                                }),
                        ),
                )
                .child(
                    div()
                        .flex_shrink_0()
                        .max_w(px(160.))
                        .truncate()
                        .text_size(px(11.))
                        .text_color(rgb(palette.muted))
                        .child(device_name),
                )
                .on_click(cx.listener(move |v, _, _, cx| {
                    v.selected = Some(device_id.clone());
                    if let Some(device) = v.devices.iter().find(|d| d.id == device_id) {
                        device.editor.update(cx, |editor, cx| {
                            editor.open_model(profile_id.clone(), model_id.clone(), cx)
                        });
                    }
                    cx.notify();
                }))
                .automation(AutomationRole::Button, automation)
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
        let palette = ZORK_UI.palette;
        let grouping_hint = match self.grouping {
            Grouping::Provider => "按供应商分组 · 点击切换为按模型分组",
            Grouping::Model => "按模型分组 · 点击切换为按供应商分组",
        };
        let grouping_icon = match self.grouping {
            Grouping::Provider => "icons/group-by-provider.svg",
            Grouping::Model => "icons/group-by-model.svg",
        };
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
        let visible_device_count = self
            .devices
            .iter()
            .filter(|device| {
                self.onboarding_local
                    .as_ref()
                    .is_none_or(|id| id == &device.id)
            })
            .count();
        for device in self.devices.iter().filter(|device| {
            self.onboarding_local
                .as_ref()
                .is_none_or(|id| id == &device.id)
        }) {
            let state = device.source.snapshot();
            connection_count += state.profiles.len();
            let name = zork_ui::device_name::summary(&device.name, &device.status, None);
            if state.loading {
                notices.push(format!("{name} · 正在加载模型…"));
            } else if let Some(error) = &state.error {
                notices.push(format!("{name} · {error}"));
            }
            let row_device = if matches!(
                device.status,
                zork_ui::device_name::DeviceStatus::Connected
                    | zork_ui::device_name::DeviceStatus::Direct
            ) {
                &device.name
            } else {
                &name
            };
            append_groups(&mut groups, self.grouping, &device.id, row_device, &state);
        }
        let mut active = Groups::new();
        let mut disabled = Groups::new();
        let mut unconfigured = Groups::new();
        for (key, rows) in groups {
            for row in rows {
                match row.model.as_ref() {
                    Some((_, true)) => active.entry(key.clone()).or_default().push(row),
                    Some((_, false)) => disabled.entry(key.clone()).or_default().push(row),
                    None => unconfigured
                        .entry(row.provider_id.clone())
                        .or_default()
                        .push(row),
                }
            }
        }
        let active_count: usize = active.values().map(Vec::len).sum();
        let disabled_count: usize = disabled.values().map(Vec::len).sum();
        let unconfigured_count: usize = unconfigured.values().map(Vec::len).sum();
        let has_notices = !notices.is_empty();
        let grouping = self.grouping;
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
                    .child(ui::page_title("模型设置"))
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
                            .map(|button| {
                                self.modal.source("model-device-dialog").bind(
                                    button,
                                    "添加连接",
                                    ui::ActionStyle {
                                        icon: Some("icons/plus.svg"),
                                        ..Default::default()
                                    },
                                )
                            })
                            .automation(AutomationRole::Button, "添加连接"),
                    ),
            )
            .child(
                ui::section()
                    .gap_2()
                    .when(connection_count == 0, |v| v.border_t_0().py_0())
                    .when(connection_count > 0, |v| {
                        v.child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .font_weight(gpui::FontWeight::SEMIBOLD)
                                        .child("模型连接"),
                                )
                                .child(
                                    div()
                                        .text_size(px(11.))
                                        .text_color(rgb(palette.muted))
                                        .child(connection_count.to_string()),
                                ),
                        )
                    })
                    .children(
                        self.devices
                            .iter()
                            .filter(|device| {
                                self.onboarding_local
                                    .as_ref()
                                    .is_none_or(|id| id == &device.id)
                            })
                            .map(|device| {
                                let has_connections = !device.source.snapshot().profiles.is_empty();
                                div()
                                    .when(visible_device_count > 1 && has_connections, |v| {
                                        v.child(
                                            div()
                                                .pt_2()
                                                .text_size(px(11.))
                                                .text_color(rgb(palette.muted))
                                                .child(format!(
                                                    "设备 · {}",
                                                    zork_ui::device_name::summary(
                                                        &device.name,
                                                        &device.status,
                                                        None,
                                                    )
                                                )),
                                        )
                                    })
                                    .child(device.editor.clone())
                            }),
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
                            format!("本机 · {active_count} 个已启用模型")
                        } else {
                            format!("{active_count} 个已启用模型")
                        },
                    ))
                    .when(self.onboarding_local.is_none(), |v| {
                        v.child(zork_ui::components::tooltip::hint(
                            ui::icon_button("model-grouping-toggle", true)
                                .child(ui::icon(grouping_icon, 16.))
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
            .children(notices.into_iter().map(ui::feedback))
            .when(active_count == 0 && !has_notices, |v| {
                v.child(
                    ui::section().child(ui::label("暂无启用的模型")).child(
                        div()
                            .text_color(rgb(palette.muted))
                            .child("添加连接或启用已有模型后，会显示在这里。"),
                    ),
                )
            })
            .children(active.into_iter().enumerate().map(|(index, (key, rows))| {
                self.render_group(index, key, rows, grouping, false, cx)
            }))
            .when(unconfigured_count > 0, |v| {
                v.child(
                    ui::section()
                        .gap_2()
                        .child(
                            div()
                                .font_weight(gpui::FontWeight::SEMIBOLD)
                                .child(format!("待配置连接 · {unconfigured_count}")),
                        )
                        .children(unconfigured.into_iter().enumerate().map(
                            |(index, (key, rows))| {
                                self.render_group(
                                    1000 + index,
                                    key,
                                    rows,
                                    Grouping::Provider,
                                    false,
                                    cx,
                                )
                            },
                        )),
                )
            })
            .when(disabled_count > 0, |v| {
                v.child(
                    ui::section()
                        .child(
                            div().flex().child(
                                ui::action_link(
                                    "model-disabled-toggle",
                                    if self.show_disabled {
                                        format!("收起未启用模型 · {disabled_count}")
                                    } else {
                                        format!("查看未启用模型 · {disabled_count}")
                                    },
                                    true,
                                )
                                .on_click(cx.listener(|v, _, _, cx| {
                                    v.show_disabled = !v.show_disabled;
                                    cx.notify();
                                }))
                                .automation(AutomationRole::Button, "切换未启用模型"),
                            ),
                        )
                        .when(self.show_disabled, |v| {
                            v.children(disabled.into_iter().enumerate().map(
                                |(index, (key, rows))| {
                                    self.render_group(2000 + index, key, rows, grouping, true, cx)
                                },
                            ))
                        }),
                )
            })
            .when(chooser_visible, |v| {
                v.child(ui::detail_modal(
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
                            .h_auto()
                            .py_3()
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
        assert_eq!(groups["custom"][0].device_name, "My device");
        groups.clear();
        append_groups(&mut groups, Grouping::Model, "node", "My device", &state);
        assert_eq!(groups["待配置模型"][0].profile_id, "new");
    }
}
