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
use zork_client_core::model_connections::{
    account_title, connection_title, merge_accounts, AccountEntry, AccountSource, ConnectionTitle,
};

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
#[derive(Clone)]
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
fn sort_rows(rows: &mut [ConnectionRow]) {
    rows.sort_by(|a, b| {
        connection_title(&a.profile, &a.providers)
            .title
            .cmp(&connection_title(&b.profile, &b.providers).title)
            .then_with(|| a.device_name.cmp(&b.device_name))
            .then_with(|| a.device_id.cmp(&b.device_id))
            .then_with(|| a.profile.profile_id.cmp(&b.profile.profile_id))
    });
}
/// Rows of one provider grouped by account in shared core.
fn account_entries(rows: &[ConnectionRow]) -> Vec<AccountEntry> {
    let sources: Vec<_> = rows
        .iter()
        .map(|row| AccountSource {
            profile: &row.profile,
            quota_failed: row.quota_failed,
        })
        .collect();
    merge_accounts(&sources)
}
/// One title for every copy of an account.
fn entry_title(rows: &[ConnectionRow], entry: &AccountEntry) -> ConnectionTitle {
    account_title(
        entry
            .sources
            .iter()
            .map(|&index| (&rows[index].profile, rows[index].providers.as_slice())),
    )
}
/// A merged account is named by its identity, which is equal on every source.
fn account_id(rows: &[ConnectionRow], entry: &AccountEntry) -> String {
    let row = &rows[entry.sources[0]];
    format!(
        "{}|{}",
        row.profile.provider,
        row.profile.account_key.as_deref().unwrap_or_default()
    )
}
pub struct ModelSettings {
    devices: Vec<Device>,
    selected: Option<String>,
    onboarding_local: Option<String>,
    /// The merged account whose device sources are listed.
    open_account: Option<String>,
}
impl ModelSettings {
    pub fn new(cx: &mut Context<Self>) -> Self {
        Self {
            devices: vec![],
            selected: None,
            onboarding_local: None,
            open_account: None,
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
    fn add_on_device(&mut self, id: &str, provider: Option<usize>, cx: &mut Context<Self>) {
        self.selected = Some(id.to_owned());
        if let Some(device) = self.devices.iter().find(|device| device.id == id) {
            device.editor.update(cx, |editor, cx| match provider {
                Some(provider) => editor.add_connection_with(provider, cx),
                None => editor.add_connection(cx),
            });
        }
        cx.notify();
    }
    /// The device a new connection is saved on unless the user picks another.
    fn default_target(&self) -> Option<String> {
        self.selected
            .clone()
            .filter(|id| self.has_device(id))
            .or_else(|| self.devices.first().map(|d| d.id.clone()))
    }
    /// Every editor can move the add flow to any other device.
    fn share_targets(&self, cx: &mut Context<Self>) {
        let targets: Vec<_> = self
            .devices
            .iter()
            .map(|d| super::profiles::SaveTarget {
                id: d.id.clone(),
                name: d.name.clone(),
                status: d.status.clone(),
            })
            .collect();
        let owner = cx.entity().downgrade();
        let retarget: super::profiles::Retarget = std::rc::Rc::new(move |id, provider, app| {
            let _ = owner.update(app, |view, cx| view.add_on_device(&id, Some(provider), cx));
        });
        for device in &self.devices {
            let (targets, retarget) = (targets.clone(), retarget.clone());
            device
                .editor
                .update(cx, |editor, _| editor.set_targets(targets, retarget));
        }
    }
    /// One card per account: a single device's row opens its connection; an
    /// account saved on several devices opens the list of its device sources.
    fn render_group(
        &self,
        provider_id: String,
        mut rows: Vec<ConnectionRow>,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let palette = ZORK_UI.palette;
        sort_rows(&mut rows);
        let accounts = account_entries(&rows);
        let provider_label = rows
            .first()
            .map(|row| row.provider_label.clone())
            .unwrap_or_else(|| provider_id.clone());
        let account_count = accounts.len();
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
                            .child(format!("{account_count} 个账号")),
                    )
                    .automation(
                        AutomationRole::Status,
                        format!("{provider_label} · {account_count} 个账号"),
                    ),
            )
            .children(accounts.iter().map(|entry| {
                if entry.sources.len() == 1 {
                    return self.render_row(&rows[entry.sources[0]], cx);
                }
                let key = account_id(&rows, entry);
                let on_click = cx.listener(move |v, _, _, cx| {
                    v.open_account = Some(key.clone());
                    cx.notify();
                });
                self.render_merged(&rows, entry, on_click, cx)
            }))
            .into_any_element()
    }
    /// One device's connection; opening it edits that device's copy.
    fn render_row(&self, row: &ConnectionRow, cx: &mut Context<Self>) -> gpui::AnyElement {
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
        editor.read_with(cx, |view, _| {
            view.render_profile_row_with_click(
                &row.profile,
                &row.providers,
                row.quota_failed,
                &[(row.device_name.clone(), row.device_status.clone())],
                None,
                None,
                None,
                on_click,
            )
        })
    }
    /// The account's card: the first device's name, every device, the quota
    /// of the freshest successful sample and the union of models.
    fn render_merged(
        &self,
        rows: &[ConnectionRow],
        entry: &AccountEntry,
        on_click: impl Fn(&gpui::ClickEvent, &mut Window, &mut gpui::App) + 'static,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let primary = &rows[entry.primary];
        let Some(device) = self.devices.iter().find(|d| d.id == primary.device_id) else {
            return gpui::Empty.into_any_element();
        };
        let first = &rows[entry.sources[0]];
        let profile = primary.profile.clone();
        let title = entry_title(rows, entry);
        // Named by its first source, so it stays put when the quota source changes.
        let key = format!("account-{}-{}", first.device_id, first.profile.profile_id);
        let devices: Vec<_> = entry
            .sources
            .iter()
            .map(|&index| {
                (
                    rows[index].device_name.clone(),
                    rows[index].device_status.clone(),
                )
            })
            .collect();
        device.editor.read_with(cx, |view, _| {
            view.render_profile_row_with_click(
                &profile,
                &primary.providers,
                primary.quota_failed,
                &devices,
                Some(entry.models),
                Some(key),
                Some(title),
                on_click,
            )
        })
    }
    /// The device sources of one account, each managed on its own.
    fn render_account_page(
        &self,
        rows: Vec<ConnectionRow>,
        entry: AccountEntry,
        cx: &mut Context<Self>,
    ) -> gpui::AnyElement {
        let p = ZORK_UI.palette;
        let first = &rows[entry.sources[0]];
        let provider = first.profile.provider.clone();
        let title = entry_title(&rows, &entry);
        let name = title.title.clone();
        let devices = entry
            .sources
            .iter()
            .map(|&index| rows[index].device_name.as_str())
            .collect::<Vec<_>>()
            .join("、");
        let meta = match &title.name {
            Some(custom) => format!(
                "{custom} · {} · 同一账号保存在 {devices}",
                first.provider_label
            ),
            None => format!("{} · 同一账号保存在 {devices}", first.provider_label),
        };
        div()
            .w_full()
            .flex()
            .flex_col()
            .gap_4()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(px(12.))
                    .child(
                        ui::icon_button("model-account-back", true)
                            .child(ui::icon("icons/arrow-left.svg", 16.))
                            .on_click(cx.listener(|v, _, _, cx| {
                                v.open_account = None;
                                cx.notify();
                            }))
                            .automation(AutomationRole::Button, "返回模型连接"),
                    )
                    .child(
                        div()
                            .flex_shrink_0()
                            .child(zork_ui::controls::provider_icon(&provider, 28.)),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .id("model-account-name")
                                    .truncate()
                                    .text_size(px(20.))
                                    .font_weight(gpui::FontWeight::SEMIBOLD)
                                    .child(name.clone())
                                    .automation(AutomationRole::Status, name),
                            )
                            .child(
                                div()
                                    .id("model-account-meta")
                                    .truncate()
                                    .text_size(px(12.))
                                    .text_color(rgb(p.muted))
                                    .child(meta.clone())
                                    .automation(AutomationRole::Status, meta),
                            ),
                    ),
            )
            .child(
                ui::section().gap_2().children(
                    entry
                        .sources
                        .iter()
                        .map(|&index| self.render_row(&rows[index], cx)),
                ),
            )
            .children(self.devices.iter().map(|device| device.editor.clone()))
            .into_any_element()
    }
    /// The device whose sample represents the first merged account.
    #[cfg(feature = "headless-bench")]
    pub fn headless_card_source(&self, _cx: &gpui::App) -> Option<String> {
        let mut groups = Groups::new();
        for device in &self.devices {
            let state = device.source.snapshot();
            append_groups(
                &mut groups,
                &device.id,
                &device.name,
                &device.status,
                &state,
            );
        }
        groups.into_values().find_map(|mut rows| {
            sort_rows(&mut rows);
            let entry = account_entries(&rows)
                .into_iter()
                .find(|entry| entry.sources.len() > 1)?;
            Some(rows[entry.primary].device_id.clone())
        })
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
        self.share_targets(cx);
        cx.notify();
    }
}
impl Render for ModelSettings {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // An opened connection replaces the list with its own page.
        if let Some(device) = self
            .devices
            .iter()
            .find(|device| device.editor.read(cx).has_page())
        {
            return div()
                .w_full()
                .child(device.editor.clone())
                .into_any_element();
        }
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
        if let Some(open) = self.open_account.clone() {
            // The account page stays while two or more devices still hold it.
            let found = groups.values().cloned().find_map(|mut rows| {
                sort_rows(&mut rows);
                let entry = account_entries(&rows)
                    .into_iter()
                    .find(|entry| entry.sources.len() > 1 && account_id(&rows, entry) == open)?;
                Some((rows, entry))
            });
            match found {
                Some((rows, entry)) => return self.render_account_page(rows, entry, cx),
                None => self.open_account = None,
            }
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
                                } else if let Some(id) = v.default_target() {
                                    v.add_on_device(&id, None, cx);
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
            .into_any_element()
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
    fn same_account_rows_merge_and_rows_without_identity_do_not() {
        let state = |key: Option<&str>| crate::api::ProfileData {
            profiles: Arc::new(vec![serde_json::from_value(json!({
                "profile_id": "go", "provider": "opencode-go", "account_key": key, "models": []
            }))
            .unwrap()]),
            ..Default::default()
        };
        let mut groups = Groups::new();
        for id in ["studio", "mba"] {
            append_groups(
                &mut groups,
                id,
                id,
                &DeviceStatus::Connected,
                &state(Some("opencode-go:k:1")),
            );
        }
        append_groups(
            &mut groups,
            "mini",
            "mini",
            &DeviceStatus::Connected,
            &state(None),
        );
        let mut rows = groups.remove("opencode-go").unwrap();
        sort_rows(&mut rows);
        let entries = account_entries(&rows);
        assert_eq!(entries.len(), 2);
        let merged = entries.iter().find(|e| e.sources.len() == 2).unwrap();
        assert_eq!(account_id(&rows, merged), "opencode-go|opencode-go:k:1");
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
