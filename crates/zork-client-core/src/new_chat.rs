//! Platform-independent model selection and new-Chat presentation.
use crate::api::{ProfileInfo, ProfileModel};
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

/// The pinned connection's own entry for `model`: `None` when the connection
/// is automatic or does not serve the model.
fn pinned<'a>(profiles: &'a [ProfileInfo], profile: &str, model: &str) -> Option<&'a ProfileModel> {
    if profile.is_empty() || profile == "auto" {
        return None;
    }
    profiles
        .iter()
        .find(|p| p.profile_id == profile)?
        .models
        .iter()
        .find(|m| m.id == model)
}

fn connection(profile: &ProfileInfo) -> ConnectionRef {
    ConnectionRef {
        profile: profile.profile_id.clone(),
        name: profile
            .name
            .clone()
            .filter(|n| !n.trim().is_empty())
            .unwrap_or_else(|| profile.profile_id.clone()),
        provider: profile.provider.clone(),
    }
}

/// Models are listed once each, whichever connections serve them; the
/// connection is optional. With `auto` the thinking values are those any
/// serving connection accepts; a pinned connection offers its own.
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
    let serving = |id: &str| -> Vec<&ProfileInfo> {
        profiles
            .iter()
            .filter(|p| p.models.iter().any(|m| m.id == id))
            .collect()
    };
    let levels: Vec<String> = match pinned(&profiles, profile, model) {
        Some(entry) => entry.thinking.clone(),
        None => choices["levels"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|v| v.as_str().map(str::to_owned))
            .collect(),
    };
    // The automatic pool for this model and thinking: the node picks one of
    // these per call, so clients name the pool rather than guess the account.
    let pool = crate::agent_edit::compatible_profiles(&profiles, Some(model), thinking)
        .into_iter()
        .map(|i| connection(&profiles[i]))
        .collect();
    Snapshot {
        text: text.into(),
        model: Choice {
            value: model.into(),
            options: choices["models"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| m["id"].as_str())
                .map(|id| {
                    let connections: Vec<_> = serving(id).into_iter().map(connection).collect();
                    // The model's own maker; a vendor connection vouches for ids only it knows.
                    let maker = std::iter::once(None)
                        .chain(connections.iter().map(|c| Some(c.provider.as_str())))
                        .find_map(|provider| crate::model_catalog::maker(id, provider))
                        .map(str::to_owned);
                    OptionItem {
                        connections,
                        maker,
                        ..option(id, id)
                    }
                })
                .collect(),
        },
        thinking: Choice {
            value: thinking.into(),
            options: levels
                .iter()
                .map(|id| option(id, &crate::thinking::value_label(id)))
                .collect(),
        },
        profile: Choice {
            value: if profile.is_empty() { "auto" } else { profile }.into(),
            options: std::iter::once(OptionItem {
                connections: pool,
                ..option("auto", "auto")
            })
            .chain(serving(model).into_iter().map(|p| {
                let c = connection(p);
                OptionItem {
                    provider: Some(c.provider.clone()).filter(|p| !p.is_empty()),
                    ..option(&c.profile, &c.name)
                }
            }))
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

/// Reconcile dependent choices only after an explicit selection intent. A
/// pinned connection stays while it serves the model and brings its own
/// thinking values; otherwise the connection returns to automatic.
pub fn choose(
    profiles: &[ProfileInfo],
    model: &str,
    thinking: &str,
    profile: &str,
) -> (String, String, String) {
    let profiles = selectable(profiles);
    if let Some(entry) = pinned(&profiles, profile, model) {
        return (
            model.into(),
            crate::agent_edit::thinking_after_choice(Some(entry), thinking),
            profile.into(),
        );
    }
    let choices = crate::agent_edit::choices(&profiles, "auto", model, thinking);
    (
        model.into(),
        choices["thinking"].as_str().unwrap_or_default().into(),
        "auto".into(),
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
        let mut profiles = profiles;
        if scenario.starts_with("picker") {
            // A router serving other makers' models: marks follow the model, not the connection.
            profiles.push(serde_json::from_value(serde_json::json!(
            {"profile_id":"opencode","name":"OpenCode Go","provider":"opencode-go","auth_configured":true,"models":[
                {"id":"deepseek-flash","thinking":["off","high"],"default_thinking":"high","limits":{"context_window_tokens":1048576,"max_output_tokens":131072}},
                {"id":"glm-5.1","thinking":["off","high"],"default_thinking":"high","limits":{"context_window_tokens":200000,"max_output_tokens":32768}},
                {"id":"muse-spark","thinking":["off"],"default_thinking":"off","limits":{"context_window_tokens":200000,"max_output_tokens":32768}}
            ]})).unwrap());
        }
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
    fn model_options_name_their_maker_apart_from_the_connection() {
        let snapshot = Fixture::new("picker").snapshot();
        let option = |id: &str| snapshot.model.options.iter().find(|o| o.value == id).unwrap();
        assert_eq!(option("deepseek-flash").maker.as_deref(), Some("deepseek"));
        assert_eq!(option("deepseek-flash").connections[0].provider, "opencode-go");
        assert_eq!(option("glm-5.1").maker.as_deref(), Some("zhipu"));
        assert_eq!(option("muse-spark").maker, None);
        // OpenAI's own connection vouches for its unlisted demo ids.
        assert_eq!(option("Demo model").maker.as_deref(), Some("openai"));
        let json = serde_json::to_value(option("muse-spark")).unwrap();
        assert!(json.get("maker").is_none());
        // Other scenarios keep their two connections.
        assert_eq!(Fixture::new("draft").profiles.len(), 2);
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
    fn values(choice: &Choice) -> Vec<&str> {
        choice.options.iter().map(|o| o.value.as_str()).collect()
    }
    #[test]
    fn a_model_served_by_two_connections_is_listed_once() {
        let snapshot = Fixture::new("picker").snapshot();
        assert_eq!(
            values(&snapshot.model),
            ["Demo model", "Demo fast", "deepseek-flash", "glm-5.1", "muse-spark"]
        );
        let demo = &snapshot.model.options[0];
        assert_eq!(
            demo.connections.iter().map(|c| c.profile.as_str()).collect::<Vec<_>>(),
            ["personal", "api"]
        );
    }
    #[test]
    fn connection_options_are_only_those_serving_the_model() {
        let fixture = Fixture::new("draft");
        let snapshot = present(&fixture.profiles, "", "Demo model", "high", "auto");
        assert_eq!(values(&snapshot.profile), ["auto", "personal", "api"]);
        let api = &snapshot.profile.options[2];
        assert_eq!((api.label.as_str(), api.provider.as_deref()), ("API", Some("openai")));
        assert_eq!(snapshot.profile.options[0].provider, None);
        let snapshot = present(&fixture.profiles, "", "Demo fast", "off", "auto");
        assert_eq!(values(&snapshot.profile), ["auto", "personal"]);
        let json = serde_json::to_value(&snapshot.model.options[0]).unwrap();
        assert!(json.get("provider").is_none());
    }
    #[test]
    fn automatic_connection_names_its_pool_and_merges_thinking() {
        let fixture = Fixture::new("draft");
        let (model, thinking, profile) = choose(&fixture.profiles, "Demo model", "high", "auto");
        assert_eq!((model.as_str(), thinking.as_str(), profile.as_str()), ("Demo model", "high", "auto"));
        let snapshot = present(&fixture.profiles, "go", &model, &thinking, &profile);
        assert!(snapshot.can_submit);
        // Every value some serving connection accepts.
        assert_eq!(values(&snapshot.thinking), ["low", "medium", "high"]);
        let pool = |thinking: &str| {
            present(&fixture.profiles, "", "Demo model", thinking, "auto").profile.options[0]
                .connections
                .iter()
                .map(|c| c.profile.clone())
                .collect::<Vec<_>>()
        };
        assert_eq!(pool("high"), ["personal", "api"]);
        // Only the personal account thinks at "low": the pool narrows to it.
        assert_eq!(pool("low"), ["personal"]);
    }
    #[test]
    fn pinning_a_connection_uses_its_own_thinking() {
        let mut fixture = Fixture::new("draft");
        fixture.apply(Action::Thinking { value: "low".into() });
        assert_eq!(fixture.selection, ("Demo model".into(), "low".into(), "auto".into()));
        // The API connection has no "low": pinning it keeps the pin and moves
        // thinking to that connection's default.
        fixture.apply(Action::Profile { value: "api".into() });
        assert_eq!(fixture.selection, ("Demo model".into(), "medium".into(), "api".into()));
        let snapshot = fixture.snapshot();
        assert_eq!(snapshot.profile.value, "api");
        assert_eq!(values(&snapshot.thinking), ["medium", "high"]);
        assert!(snapshot.error.is_none());
        // Back to automatic.
        fixture.apply(Action::Profile { value: "auto".into() });
        assert_eq!(fixture.selection.2, "auto");
        // A connection that does not serve the model cannot be pinned.
        fixture.apply(Action::Model { value: "Demo fast".into() });
        fixture.apply(Action::Profile { value: "api".into() });
        assert_eq!(fixture.selection.2, "auto");
    }
    #[test]
    fn changing_model_keeps_a_serving_pin_and_resets_otherwise() {
        let mut fixture = Fixture::new("picker");
        fixture.apply(Action::Profile { value: "personal".into() });
        fixture.apply(Action::Model { value: "Demo fast".into() });
        assert_eq!(fixture.selection, ("Demo fast".into(), "off".into(), "personal".into()));
        fixture.apply(Action::Model { value: "Demo model".into() });
        fixture.apply(Action::Profile { value: "api".into() });
        assert_eq!(fixture.selection.2, "api");
        // The API connection does not serve Demo fast.
        fixture.apply(Action::Model { value: "Demo fast".into() });
        assert_eq!(fixture.selection, ("Demo fast".into(), "off".into(), "auto".into()));
        fixture.apply(Action::Profile { value: "opencode".into() });
        assert_eq!(fixture.selection.2, "auto");
        fixture.apply(Action::Model { value: "glm-5.1".into() });
        fixture.apply(Action::Profile { value: "opencode".into() });
        assert_eq!(fixture.selection, ("glm-5.1".into(), "off".into(), "opencode".into()));
        fixture.apply(Action::Model { value: "Demo model".into() });
        assert_eq!(fixture.selection.2, "auto");
        assert!(fixture.snapshot().error.is_none());
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
