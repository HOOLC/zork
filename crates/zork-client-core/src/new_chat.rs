//! Platform-independent model selection and new-Chat presentation.
use crate::api::ProfileInfo;
use zork_client_types::device::DeviceStatus;
pub use zork_client_types::new_chat::{Action, Choice};
use zork_client_types::new_chat::{ConnectionRef, OptionItem, Snapshot};

pub fn selectable(profiles: &[ProfileInfo]) -> Vec<ProfileInfo> {
    profiles
        .iter()
        .filter(|p| {
            p.extra
                .get("auth_configured")
                .and_then(serde_json::Value::as_bool)
                == Some(true)
        })
        .cloned()
        .map(|mut profile| {
            profile.models.retain(|m| {
                m.enabled
                    && m.extra
                        .get("limits")
                        .is_some_and(serde_json::Value::is_object)
            });
            profile
        })
        .collect()
}

pub fn present(
    profiles: &[ProfileInfo],
    text: &str,
    model: &str,
    thinking: &str,
    profile: &str,
) -> Snapshot {
    let profiles = selectable(profiles);
    let choices = crate::agent_edit::choices(&profiles, profile, model, thinking);
    let valid = crate::agent_edit::validate_selection(&profiles, profile, model, thinking).is_ok();
    let too_large = text.len() > zork_client_types::chat::MAX_MESSAGE_TEXT_BYTES;
    let option = |value: &str, label: &str| OptionItem {
        value: value.into(),
        label: label.into(),
        ..Default::default()
    };
    // Which connections offer each model, so pickers can group by connection.
    let connections = |id: &str| -> Vec<ConnectionRef> {
        profiles
            .iter()
            .filter(|p| p.models.iter().any(|m| m.id == id))
            .map(|p| ConnectionRef {
                profile: p.profile_id.clone(),
                name: p.name.clone().filter(|n| !n.trim().is_empty()).unwrap_or_else(|| p.profile_id.clone()),
                provider: p.provider.clone(),
            })
            .collect()
    };
    Snapshot {
        text: text.into(),
        model: Choice {
            value: model.into(),
            options: choices["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| m["id"].as_str())
                .map(|id| OptionItem {
                    connections: connections(id),
                    ..option(id, id)
                })
                .collect(),
        },
        thinking: Choice {
            value: thinking.into(),
            options: choices["levels"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|v| v.as_str())
                .map(|id| option(id, &crate::thinking::value_label(id)))
                .collect(),
        },
        profile: Choice {
            value: if profile.is_empty() { "auto" } else { profile }.into(),
            options: choices["profiles"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|p| {
                    let id = p["profile_id"].as_str()?;
                    Some(option(
                        id,
                        if id == "auto" {
                            "auto"
                        } else {
                            p["name"].as_str().unwrap_or(id)
                        },
                    ))
                })
                .collect(),
        },
        editable: true,
        can_submit: !text.trim().is_empty() && valid && !too_large,
        error: if too_large {
            Some("首条消息不能超过 32 KiB".into())
        } else if !model.is_empty() && !valid {
            Some("所选模型、思考深度或连接不可用，请重新选择".into())
        } else {
            None
        },
        needs_model: crate::agent_edit::profile_options(&profiles)[0]
            .models
            .is_empty(),
        ..Default::default()
    }
}

/// Reconcile dependent choices only after an explicit selection intent.
pub fn choose(
    profiles: &[ProfileInfo],
    model: &str,
    thinking: &str,
    profile: &str,
) -> (String, String, String) {
    let choices = crate::agent_edit::choices(&selectable(profiles), profile, model, thinking);
    (
        model.into(),
        choices["thinking"].as_str().unwrap_or_default().into(),
        choices["profile"].as_str().unwrap_or("auto").into(),
    )
}

/// Offline gallery adapter. The same selection rules feed every platform.
pub struct Fixture {
    profiles: Vec<ProfileInfo>,
    text: String,
    selection: (String, String, String),
    scenario: String,
    device: String,
    local_only: bool,
}
impl Fixture {
    pub fn new(scenario: &str) -> Self {
        let profiles = if scenario == "no-models" {
            vec![]
        } else {
            serde_json::from_value(serde_json::json!([
            {"profile_id":"personal","name":"个人账号","provider":"openai","auth_configured":true,"models":[
                {"id":"Demo model","thinking":["low","medium","high"],"default_thinking":"high","limits":{"context_window_tokens":128000,"max_output_tokens":8192}},
                {"id":"Demo fast","thinking":["off","low"],"default_thinking":"off","limits":{"context_window_tokens":64000,"max_output_tokens":4096}}
            ]},
            {"profile_id":"api","name":"API","provider":"openai","auth_configured":true,"models":[
                {"id":"Demo model","thinking":["medium","high"],"default_thinking":"medium","limits":{"context_window_tokens":128000,"max_output_tokens":8192}}
            ]}
        ])).unwrap()
        };
        Self {
            profiles,
            text: if matches!(scenario, "creating" | "retry" | "error") {
                "帮我整理这个项目的下一步计划。".into()
            } else {
                String::new()
            },
            selection: ("Demo model".into(), "high".into(), "auto".into()),
            scenario: scenario.into(),
            device: "local".into(),
            local_only: scenario == "first-chat",
        }
    }
    pub fn snapshot(&self) -> Snapshot {
        let mut view = present(
            &self.profiles,
            &self.text,
            &self.selection.0,
            &self.selection.1,
            &self.selection.2,
        );
        view.device = Choice {
            value: self.device.clone(),
            options: [("local", "本机"), ("remote", "远程设备")]
                .into_iter()
                .filter(|(value, _)| !self.local_only || *value == "local")
                .map(|(value, label)| OptionItem {
                    value: value.into(),
                    label: label.into(),
                    status: Some(if value == "local" {
                        DeviceStatus::Direct
                    } else {
                        DeviceStatus::MeshPreparing
                    }),
                    ..Default::default()
                })
                .collect(),
        };
        view.busy = self.scenario == "creating";
        view.uncertain = self.scenario == "retry";
        view.editable = !view.busy && !view.uncertain;
        view.can_submit = !view.busy && (view.can_submit || view.uncertain);
        if self.scenario == "error" {
            view.error = Some("设备暂时不可用，输入已保留。".into());
        }
        view
    }
    pub fn select_device(&mut self, id: &str) {
        let snapshot = self.snapshot();
        if snapshot.editable
            && snapshot
                .device
                .options
                .iter()
                .any(|option| option.value == id)
        {
            self.device = id.into();
        }
    }
    pub fn apply(&mut self, action: zork_client_types::new_chat::Action) {
        use zork_client_types::new_chat::Action;
        match action {
            Action::Begin => {}
            Action::Edit { text } if self.snapshot().editable => self.text = text,
            Action::Model { value } if self.snapshot().editable => {
                self.selection =
                    choose(&self.profiles, &value, &self.selection.1, &self.selection.2)
            }
            Action::Thinking { value } if self.snapshot().editable => {
                self.selection =
                    choose(&self.profiles, &self.selection.0, &value, &self.selection.2)
            }
            Action::Profile { value } if self.snapshot().editable => {
                self.selection =
                    choose(&self.profiles, &self.selection.0, &self.selection.1, &value)
            }
            Action::Select { profile, model } if self.snapshot().editable => {
                self.selection = choose(&self.profiles, &model, &self.selection.1, &profile)
            }
            Action::Submit { text } if self.snapshot().can_submit => {
                self.text = text;
                self.scenario = "creating".into();
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn model_options_name_the_connections_that_offer_them() {
        let fixture = Fixture::new("draft");
        let snapshot = present(&fixture.profiles, "", "", "", "auto");
        let connections = |id: &str| {
            snapshot
                .model
                .options
                .iter()
                .find(|o| o.value == id)
                .unwrap()
                .connections
                .iter()
                .map(|c| (c.profile.as_str(), c.name.as_str()))
                .collect::<Vec<_>>()
        };
        assert_eq!(connections("Demo model"), [("personal", "个人账号"), ("api", "API")]);
        assert_eq!(connections("Demo fast"), [("personal", "个人账号")]);
        // Other choices stay unchanged on the wire.
        let json = serde_json::to_value(&snapshot.thinking).unwrap();
        assert!(json["options"][0].get("connections").is_none());
        // Older payloads without the new fields still parse.
        let old: OptionItem = serde_json::from_value(serde_json::json!({"value":"m","label":"m"})).unwrap();
        assert!(old.connections.is_empty() && old.device.is_none());
    }
    #[test]
    fn model_depth_and_optional_profile_choices_respect_actual_capabilities() {
        let fixture = Fixture::new("draft");
        let snapshot = present(&fixture.profiles, "work", "Demo fast", "off", "auto");
        assert!(snapshot.can_submit);
        assert_eq!(
            snapshot
                .thinking
                .options
                .iter()
                .map(|o| o.value.as_str())
                .collect::<Vec<_>>(),
            ["off", "low"]
        );
        assert_eq!(
            snapshot
                .profile
                .options
                .iter()
                .map(|o| o.value.as_str())
                .collect::<Vec<_>>(),
            ["auto", "personal"]
        );
        assert!(!present(&fixture.profiles, "work", "Demo fast", "high", "auto").can_submit);
        assert!(!present(&fixture.profiles, "work", "Demo fast", "off", "api").can_submit);
        assert!(!present(&fixture.profiles, " ", "Demo model", "high", "auto").can_submit);
        assert!(
            !present(
                &fixture.profiles,
                &"x".repeat(32769),
                "Demo model",
                "high",
                "auto"
            )
            .can_submit
        );
    }
    #[test]
    fn unavailable_accounts_and_unconfigured_limits_are_not_selectable() {
        let mut fixture = Fixture::new("draft");
        fixture.profiles[0]
            .extra
            .insert("auth_configured".into(), false.into());
        fixture.profiles[1].models[0].extra.remove("limits");
        let snapshot = fixture.snapshot();
        assert!(snapshot.model.options.is_empty());
        assert!(snapshot.needs_model);
    }
}
