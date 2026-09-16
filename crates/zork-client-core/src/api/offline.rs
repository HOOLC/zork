//! In-memory Gateway responses shared by native and portable development hosts.
use super::ProfileInfo;
use serde_json::{json, Value};
use std::sync::Mutex;
pub struct Fixture {
    data: Mutex<Value>,
    providers: Value,
}
impl Fixture {
    pub fn new(fixture: Value, providers: Value) -> Self {
        let id = fixture["profile"]["profile_id"]
            .as_str()
            .expect("fixture profile ID")
            .to_owned();
        Self {
            data: Mutex::new(
                json!({"profiles":{id:fixture["profile"]},"agents":fixture["agents"],"authorizations":{}}),
            ),
            providers,
        }
    }
    pub fn list_profiles(&self) -> anyhow::Result<Vec<ProfileInfo>> {
        let d = self.data.lock().unwrap();
        Ok(d["profiles"]
            .as_object()
            .unwrap()
            .values()
            .map(|v| serde_json::from_value(v.clone()).unwrap())
            .collect())
    }
    pub fn node_request(
        &self,
        method: http::Method,
        path: String,
        body: Option<Value>,
    ) -> anyhow::Result<Value> {
        let mut d = self.data.lock().unwrap();
        let body = body.unwrap_or(Value::Null);
        match path.as_str() {
            "/v1/node/providers" => return Ok(self.providers.clone()),
            "/v1/node/mesh" => return Ok(json!({"origin":"storybook","enabled":false})),
            "/v1/node/agents" => {
                if method == http::Method::POST {
                    d["agents"].as_array_mut().unwrap().push(body.clone());
                    return Ok(body);
                }
                return Ok(json!({"items":d["agents"]}));
            }
            _ => {}
        }
        if path == "/v1/node/auth" && method == http::Method::POST {
            let id = format!("fixture-auth-{}", ulid::Ulid::new());
            d["authorizations"][&id] = json!({"profile_id":body["profile_id"],"provider":body["provider"],"billing":body["billing"]});
            return Ok(json!({"id":id,"flow":"browser_callback","verification_url":"about:blank"}));
        }
        if let Some(id) = path.strip_prefix("/v1/node/auth/") {
            if method == http::Method::DELETE {
                d["authorizations"].as_object_mut().unwrap().remove(id);
                return Ok(Value::Null);
            }
            let attempt = d["authorizations"][id].clone();
            anyhow::ensure!(attempt.is_object(), "authorization not found");
            if body["callback"].as_str().is_none_or(str::is_empty) {
                return Ok(json!({"status":"pending","retry_after_seconds":1}));
            }
            let profile_id = attempt["profile_id"].as_str().unwrap();
            let models = d["profiles"][profile_id]
                .get("models")
                .cloned()
                .unwrap_or_else(|| json!([]));
            d["profiles"][profile_id] = json!({"profile_id":profile_id,"provider":attempt["provider"],"billing":attempt["billing"],"verified":true,"models":models});
            d["authorizations"].as_object_mut().unwrap().remove(id);
            return Ok(json!({"status":"completed"}));
        }
        if let Some(tail) = path.strip_prefix("/v1/node/profiles/") {
            let (id, action) = tail.split_once('/').unwrap_or((tail, ""));
            if action.is_empty() && method == http::Method::PUT {
                d["profiles"][id] = json!({"profile_id":id,"provider":body["provider"],"billing":body["billing"],"base_url":body["base_url"],"verified":true,"models":body.get("models").cloned().unwrap_or_else(||json!([]))});
            }
            if action.is_empty() && method == http::Method::DELETE {
                d["profiles"].as_object_mut().unwrap().remove(id);
                return Ok(Value::Null);
            }
            anyhow::ensure!(d["profiles"][id].is_object(), "profile not found");
            if action == "name" && method == http::Method::PUT {
                d["profiles"][id]["name"] = body["name"].clone();
            }
            if action == "models/refresh" && method == http::Method::POST {
                let models = d["profiles"][id]["models"].as_array_mut().unwrap();
                let mut added = 0;
                if !models.iter().any(|m| m["id"] == "updated-model") {
                    let mut model=models.first().cloned().unwrap_or_else(||json!({"api":"openai-completions","thinking":["off"],"default_thinking":"off","capabilities":{"input":["text"]},"limits":{"context_window_tokens":128000,"max_output_tokens":8192}}));
                    model["id"] = json!("updated-model");
                    model["default"] = json!(false);
                    model["enabled"] = json!(true);
                    models.push(model);
                    added = 1;
                }
                return Ok(
                    json!({"profile":d["profiles"][id],"added":added,"configured":added,"truncated":false}),
                );
            }
            if action == "models/enabled" && method == http::Method::PUT {
                let model = d["profiles"][id]["models"]
                    .as_array_mut()
                    .unwrap()
                    .iter_mut()
                    .find(|m| m["id"] == body["model_id"])
                    .ok_or_else(|| anyhow::anyhow!("model not found"))?;
                anyhow::ensure!(
                    body["enabled"] != true || model["limits"].is_object(),
                    "model limits are missing"
                );
                model["enabled"] = body["enabled"].clone();
            }
            if action == "models" && method == http::Method::PUT {
                anyhow::ensure!(
                    body.get("expected_models")
                        .is_none_or(|expected| *expected == d["profiles"][id]["models"]),
                    "model_configuration_changed"
                );
                d["profiles"][id]["models"] = body["models"].clone();
            }
            if action == "refresh" && method == http::Method::POST {
                d["profiles"][id]["checkedAt"] = json!(chrono::Utc::now().to_rfc3339());
                // Deterministic usage changes let the offline story verify refresh rendering.
                if let Some(used) =
                    d["profiles"][id]["rateLimits"]["rateLimits"]["primary"]["usedPercent"].as_f64()
                {
                    d["profiles"][id]["rateLimits"]["rateLimits"]["primary"]["usedPercent"] =
                        json!((used + 1.).min(100.));
                }
            }
            if action == "discovered-models" {
                return Ok(
                    json!({"supported":true,"items":[{"id":"fixture-model"}],"truncated":false}),
                );
            }
            return Ok(d["profiles"][id].clone());
        }
        if let Some(tail) = path.strip_prefix("/v1/node/agents/") {
            let (id, action) = tail.split_once('/').unwrap_or((tail, ""));
            if let Some(agent) = d["agents"]
                .as_array_mut()
                .unwrap()
                .iter_mut()
                .find(|a| a["id"] == id)
            {
                if action == "model" {
                    agent["profile_id"] = body["profile_id"].clone();
                    agent["model"] = body["model"].clone();
                    agent["thinking"] = body["thinking"].clone();
                }
                if action == "avatar" {
                    agent["avatar"] = body["avatar"].clone();
                }
                if action == "grants" {
                    agent["allowed_leaders"] = body["allowed_leaders"].clone();
                }
                return Ok(agent.clone());
            }
        }
        anyhow::bail!("此操作不在组件展台数据适配器范围内")
    }
}

#[cfg(all(test, not(target_family = "wasm")))]
mod tests {
    use super::*;
    use std::sync::Arc;
    #[tokio::test]
    async fn native_fixture_runs_the_real_profile_and_agent_controllers() {
        let client = Arc::new(super::super::GatewayClient::fixture(
            json!({"profile":{"profile_id":"existing","provider":"demo","models":[]},"agents":[]}),
            json!({"providers":[{"id":"demo","billing":[{"id":"key","base_url":"https://example.invalid"}]}]}),
        ));
        let profiles = crate::state::Profiles::new(client.clone());
        profiles.refresh().await;
        profiles
            .save_connection(crate::model_edit::ConnectionInput {
                id: "new-connection".into(),
                provider: "demo".into(),
                billing: "key".into(),
                base_url: String::new(),
                key: "fixture-secret-not-retained".into(),
            })
            .await
            .unwrap();
        assert!(profiles
            .snapshot()
            .profiles
            .iter()
            .any(|profile| profile.profile_id == "new-connection"));
        profiles
            .rename("new-connection".into(), "演示连接".into())
            .await
            .unwrap();
        assert_eq!(
            profiles
                .snapshot()
                .profiles
                .iter()
                .find(|profile| profile.profile_id == "new-connection")
                .unwrap()
                .name
                .as_deref(),
            Some("演示连接")
        );
        profiles
            .start_authorization("signed-in".into(), "demo".into(), "key".into())
            .await
            .unwrap();
        assert!(profiles.snapshot().authorization.is_some());
        profiles
            .finish_authorization("fixture-callback-not-retained".into())
            .await
            .unwrap();
        assert!(profiles.snapshot().authorization_complete);
        assert!(profiles
            .snapshot()
            .profiles
            .iter()
            .any(|profile| profile.profile_id == "signed-in" && profile.verified));
        let agents = crate::state::Agents::new(client.clone(), profiles);
        agents
            .create_agent(json!({"id":"helper","name":"演示队员","avatar":"fox"}))
            .await
            .unwrap();
        assert!(agents
            .snapshot()
            .agents
            .iter()
            .any(|agent| agent["id"] == "helper"));
        let stored = client
            .fixture
            .as_ref()
            .unwrap()
            .data
            .lock()
            .unwrap()
            .to_string();
        assert!(!stored.contains("fixture-secret-not-retained"));
        assert!(!stored.contains("fixture-callback-not-retained"));
    }
}
