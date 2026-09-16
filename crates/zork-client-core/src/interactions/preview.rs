//! Deterministic, offline adapter for native and Web component verification.
//! It uses the production input contract and card projection, with no services.
use super::*;
use serde_json::json;

pub struct Preview {
    request: Request,
    resolution: Option<Resolution>,
    errors: BTreeMap<String, String>,
    login: Option<LoginView>,
    login_callback: bool,
}

impl Preview {
    pub fn new(state: &str) -> Self {
        let input = state.starts_with("input") || state.starts_with("prefilled");
        let mut request = if ["approval", "approved", "declined", "cancelled"]
            .iter()
            .any(|prefix| state.starts_with(prefix))
        {
            Request::Approval {
                title: "批准发布预览版本".into(),
                description: "将本次已审核的变更发布到预览环境，供团队验收。".into(),
            }
        } else if state.starts_with("login") {
            Request::OAuth {
                title: if state.starts_with("login-callback") {
                    "Anthropic"
                } else {
                    "OpenAI"
                }
                .into(),
            }
        } else if input {
            serde_json::from_value(json!({"action":"input","title":"安排执行任务","fields":[
                {"id":"title","label":"任务名称","required":true},
                {"id":"target","label":"执行环境","kind":"choice","required":true,"default":"test",
                    "options":[{"value":"test","label":"测试环境"},{"value":"preview","label":"预览环境"}]},
                {"id":"notes","label":"补充说明","kind":"multiline","default":"保留现有工作。"}
            ]})).unwrap()
        } else {
            Request::AgentConfiguration {
                creating: !state.starts_with("update"),
                name: "实现队员".into(),
                prominent: if state.starts_with("update") {
                    vec!["/name".into()]
                } else {
                    vec!["/name".into(), "/instructions".into(), "/selection".into()]
                },
                fields: vec![
                    Field {
                        id: "/name".into(),
                        label: "interaction_name".into(),
                        required: true,
                        default: "实现队员".into(),
                        ..Default::default()
                    },
                    Field {
                        id: "/instructions".into(),
                        label: "interaction_instructions".into(),
                        kind: FieldKind::Multiline,
                        default: "完成交互卡片，并验证请求与结果的关联。".into(),
                        ..Default::default()
                    },
                    Field {
                        id: "/selection".into(),
                        label: "interaction_model".into(),
                        kind: FieldKind::Choice,
                        required: true,
                        default: "preview-model".into(),
                        options: vec![Choice {
                            value: "preview-model".into(),
                            label: "示例模型".into(),
                        }],
                    },
                    Field {
                        id: "/allowed_leaders".into(),
                        label: "interaction_grants".into(),
                        kind: FieldKind::MultiChoice,
                        default: "[\"requester\"]".into(),
                        options: vec![Choice {
                            value: "requester".into(),
                            label: "本次请求的发起者".into(),
                        }],
                        ..Default::default()
                    },
                ],
            }
        };
        if state.starts_with("prefilled") {
            if let Request::Input { fields, .. } = &mut request {
                fields[0].default = "整理本周项目进展".into();
                fields[2].default = "请保留现有工作，完成后给出变更摘要。".into();
            }
        }
        if state.starts_with("long") {
            if let Request::AgentConfiguration { fields, .. } = &mut request {
                fields[1].default =
                    "保留原始请求，结果更新当前卡片。每一行都应保持可读。\n".repeat(160);
            }
        }
        let mut preview = Self {
            request,
            resolution: None,
            errors: BTreeMap::new(),
            login: None,
            login_callback: state.starts_with("login-callback"),
        };
        if state.starts_with("completed") || state.starts_with("long") {
            preview.activate("submit", BTreeMap::new());
        }
        if state.starts_with("approved") {
            preview.activate("submit", BTreeMap::new());
        } else if state.starts_with("declined") {
            preview.activate("decline", BTreeMap::new());
        } else if state.starts_with("cancelled") {
            preview.finish(Outcome::Cancelled, json!({"reason":"invocation_cancelled"}));
        } else if state.starts_with("login-device") || state.starts_with("login-callback") {
            preview.prepare_login();
        } else if state.starts_with("login-completed") {
            preview.finish(
                Outcome::Completed,
                json!({"profile_id":"开发连接","connected":true}),
            );
        }
        preview
    }

    pub fn card(&self) -> Card {
        let mut projected = card(
            "preview",
            self.handler(),
            &self.request,
            self.resolution.as_ref(),
            None,
            &self.errors,
        );
        if let Some(login) = &self.login {
            login.apply(&mut projected);
        }
        projected
    }

    pub fn activate(&mut self, action: &str, values: BTreeMap<String, String>) {
        if self
            .resolution
            .as_ref()
            .is_some_and(|r| r.outcome.terminal())
        {
            return;
        }
        let Ok(command) = Command::from_action("preview", action, values)
            .and_then(|command| command.resolve(self.handler()))
        else {
            return;
        };
        let (outcome, values) = match command {
            Command::ContinueLogin { .. } => {
                self.prepare_login();
                return;
            }
            Command::CancelLogin { .. } => (Outcome::Cancelled, BTreeMap::new()),
            Command::LoginCallback { callback, .. } if !callback.trim().is_empty() => {
                // Offline demonstration only. A real login is completed by the
                // node's Provider adapter, never by a card or pasted text.
                self.login = None;
                self.finish(
                    Outcome::Completed,
                    json!({"profile_id":"开发连接","connected":true}),
                );
                return;
            }
            Command::Submit { values, .. } => match self.request.validate_values(&values) {
                Ok(values) => (Outcome::Completed, values),
                Err(errors) => {
                    self.errors = errors;
                    return;
                }
            },
            Command::Decline { .. } => (Outcome::Declined, BTreeMap::new()),
            _ => return,
        };
        self.errors.clear();
        self.login = None;
        self.finish(outcome, json!({"values":values}));
    }

    fn handler(&self) -> &'static str {
        if matches!(self.request, Request::OAuth { .. }) {
            PROVIDER_LOGIN
        } else {
            AGENT_CONFIGURATION
        }
    }
    fn prepare_login(&mut self) {
        let callback = self.login_callback;
        self.login = Some(LoginView {
            open_url: Some("https://example.invalid/sign-in-preview".into()),
            user_code: (!callback).then(|| "DEMO-8341".into()),
            callback,
            ..Default::default()
        });
        self.finish(Outcome::Pending, json!({"profile_id":"开发连接"}));
    }

    fn finish(&mut self, outcome: Outcome, output: serde_json::Value) {
        self.resolution = Some(Resolution {
            request_message_id: "preview".into(),
            response_id: "preview-response".into(),
            revision: self.resolution.as_ref().map_or(1, |r| r.revision + 1),
            outcome,
            actor: "offline-fixture".into(),
            output,
        });
    }
}
