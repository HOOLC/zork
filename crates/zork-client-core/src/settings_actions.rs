//! Business intents shared by platform adapters. HTTP is an implementation detail.
use crate::state::upgrade_status;
use crate::{
    model_edit::{ConnectionInput, ModelInput},
    Client,
};
use anyhow::Result;
use serde::Deserialize;
use serde_json::{json, Value};

#[derive(Deserialize)]
#[serde(tag = "action", rename_all = "snake_case", deny_unknown_fields)]
pub enum SettingsAction {
    NodeInfo,
    OpenModels,
    OpenProfile {
        profile: String,
    },
    CheckUpdate,
    Upgrade {
        version: String,
    },
    RenameDevice {
        name: String,
    },
    RenameProfile {
        profile: String,
        name: String,
    },
    SaveModel {
        profile: String,
        input: ModelInput,
    },
    RemoveModel {
        profile: String,
        model: Value,
    },
    EnableModel {
        profile: String,
        model: String,
        enabled: bool,
    },
    DiscoverModels {
        profile: String,
    },
    RefreshQuota {
        profile: String,
    },
    SaveConnection {
        input: ConnectionInput,
    },
    StartAuthorization {
        profile: String,
        provider: String,
        billing: String,
    },
    CompleteAuthorization {
        callback: String,
    },
    CancelAuthorization,
    AgentGrants {
        id: String,
        allowed: Vec<String>,
        expected: Vec<String>,
    },
    SaveAgent {
        input: AgentInput,
    },
    OpenAgent {
        id: String,
    },
    PrepareAgent {
        id: String,
    },
    StopConversation {
        session: String,
    },
}

pub use crate::agent_edit::AgentInput;

pub(crate) fn recover_operations(store: &crate::store::ClientStore) -> Result<()> {
    for node in store.nodes()? {
        if let Some(mut command) = store.get::<Value>(&node.id, "settings-command")? {
            if command["running"] == true {
                command["running"] = json!(false);
                command["error"] = json!("上次操作结果待确认，请刷新设置后重试");
                store.put(&node.id, "settings-command", &command)?;
            }
        }
        if let Some(mut operation) = store.get::<Value>(&node.id, "node-operation")? {
            if operation["running"] == true {
                // The remote operation may still run after this client exits.
                // A persisted monitor flag cannot represent a live local job.
                operation["running"] = json!(false);
                operation["completed"] = json!(false);
                operation["uncertain"] = json!(true);
                operation["message"] = json!("上次升级结果待确认，请刷新设备状态");
                store.put(&node.id, "node-operation", &operation)?;
            }
        }
    }
    Ok(())
}

impl Client {
    pub(crate) async fn tracked_settings_action(
        &mut self,
        peer: String,
        action: SettingsAction,
        request_id: Option<String>,
    ) -> Result<Value> {
        let Some(id) = request_id else {
            return self.settings_action(peer, action).await;
        };
        self.peer(&peer)?;
        crate::model_edit::valid_id(&id)?;
        if let Some(previous) = self.store.get::<Value>(&peer, "settings-command")? {
            if previous["id"] == id {
                anyhow::ensure!(previous["running"] != true, "操作仍在进行");
                if let Some(error) = previous["error"].as_str() {
                    anyhow::bail!("{error}");
                }
                return Ok(previous["result"].clone());
            }
        }
        self.store
            .put(&peer, "settings-command", &json!({"id":id,"running":true}))?;
        let secret = match &action {
            SettingsAction::SaveConnection { input } => Some(input.key.clone()),
            SettingsAction::CompleteAuthorization { callback } => Some(callback.clone()),
            _ => None,
        };
        let result = self
            .settings_action(peer.clone(), action)
            .await
            .map_err(|error| {
                let Some(secret) = secret.filter(|s| !s.is_empty()) else {
                    return error;
                };
                let mut message = error.to_string().replace(&secret, "[已隐藏]");
                if let Ok(url) = reqwest::Url::parse(&secret) {
                    for (key, value) in url.query_pairs() {
                        if matches!(key.as_ref(), "code" | "token" | "access_token" | "id_token")
                            && !value.is_empty()
                        {
                            message = message.replace(value.as_ref(), "[已隐藏]");
                        }
                    }
                }
                anyhow::anyhow!(message)
            });
        self.store.put(
            &peer,
            "settings-command",
            &match &result {
                Ok(value) => json!({"id":id,"running":false,"completed":true,"result":value}),
                Err(error) => {
                    json!({"id":id,"running":false,"completed":false,"error":error.to_string()})
                }
            },
        )?;
        result
    }
    pub(crate) async fn settings_action(
        &mut self,
        peer: String,
        action: SettingsAction,
    ) -> Result<Value> {
        self.peer(&peer)?;
        let station = self.station(&peer)?;
        let device = crate::state::Device::open(
            station.clone(),
            Some((self.store.clone(), peer.clone())),
            true,
        );
        device.start();
        self.devices.insert(peer.clone(), device.clone());
        let profiles = device.profiles();
        match action {
            SettingsAction::OpenModels => {
                profiles.refresh_stale().await;
                Ok(json!({}))
            }
            SettingsAction::OpenProfile { profile } => {
                profiles.open_detail(&profile).await?;
                Ok(json!({}))
            }
            SettingsAction::NodeInfo => self.request(&peer, "GET", "/v1/node/info", None).await,
            SettingsAction::CheckUpdate => {
                let value = self.request(&peer, "GET", "/v1/node/update", None).await?;
                self.store.put(&peer, "node-update-check", &value)?;
                Ok(value)
            }
            SettingsAction::RenameDevice { name } => {
                let name = zork_config::membership::validate_device_name(&name)?;
                self.request(&peer, "PUT", "/v1/node/name", Some(json!({"name":name})))
                    .await
            }
            SettingsAction::RenameProfile { profile, name } => {
                profiles.rename(profile, name).await?;
                Ok(json!({}))
            }
            SettingsAction::SaveModel { profile, input } => {
                profiles.save_model(&profile, input).await?;
                Ok(json!({}))
            }
            SettingsAction::RemoveModel { profile, model } => {
                profiles.remove_model(&profile, model).await?;
                Ok(json!({}))
            }
            SettingsAction::EnableModel {
                profile,
                model,
                enabled,
            } => {
                profiles.set_model_enabled(&profile, model, enabled).await?;
                Ok(json!({}))
            }
            SettingsAction::DiscoverModels { profile } => profiles.discover_models(&profile).await,
            SettingsAction::RefreshQuota { profile } => {
                profiles.refresh_quota(profile.clone()).await;
                anyhow::ensure!(
                    !profiles.snapshot().failed.contains(&profile),
                    "额度刷新失败，已保留上次结果，请重试"
                );
                Ok(json!({}))
            }
            SettingsAction::SaveConnection { input } => {
                profiles.refresh().await;
                profiles.save_connection(input).await?;
                Ok(json!({}))
            }
            SettingsAction::StartAuthorization {
                profile,
                provider,
                billing,
            } => {
                profiles
                    .start_authorization(profile, provider, billing)
                    .await?;
                Ok(json!({}))
            }
            SettingsAction::CompleteAuthorization { callback } => {
                profiles.continue_authorization(callback);
                Ok(json!({}))
            }
            SettingsAction::CancelAuthorization => {
                profiles.cancel_authorization().await?;
                Ok(json!({}))
            }
            SettingsAction::AgentGrants {
                id,
                allowed,
                expected,
            } => {
                crate::model_edit::valid_id(&id)?;
                device
                    .agents()
                    .update_agent_grants(
                        &id,
                        crate::agent_edit::grant_references(allowed, ""),
                        json!(expected),
                    )
                    .await?;
                Ok(json!({}))
            }
            SettingsAction::SaveAgent { input } => device.agents().save(input).await,
            SettingsAction::OpenAgent { id } => {
                crate::model_edit::valid_id(&id)?;
                device.open_agent(&id).await
            }
            SettingsAction::PrepareAgent { id } => {
                crate::model_edit::valid_id(&id)?;
                if device.snapshot().online == Some(true) {
                    device.open_agent(&id).await
                } else {
                    Ok(json!({"cached":true}))
                }
            }
            SettingsAction::StopConversation { session } => {
                crate::valid_session(&session)?;
                device.conversation(&session).stop();
                Ok(json!({}))
            }
            SettingsAction::Upgrade { version } => {
                let current: Option<Value> = self.store.get(&peer, "node-operation")?;
                anyhow::ensure!(
                    !current.is_some_and(|v| v["running"] == true),
                    "设备升级仍在进行"
                );
                let id = ulid::Ulid::new().to_string();
                self.store.put(
                    &peer,
                    "node-operation",
                    &json!({"id":id,"version":version,"running":true,"message":"正在提交升级"}),
                )?;
                let store = self.store.clone();
                let job_id = id.clone();
                station.spawn(async move {
                    let outcome = device.upgrade(&version, |state| {
                            let status = upgrade_status(&state.info, &version).unwrap_or(&Value::Null);
                            let message = status["message"].as_str().filter(|s| !s.is_empty()).unwrap_or("等待设备恢复连接…");
                            store.put(&peer,"node-operation",&json!({"id":job_id,"version":version,"running":true,"message":message}))?;
                            Ok(())
                    }).await;
                    let _ = store.put(&peer,"node-operation",&json!({"id":job_id,"version":version,"running":false,"completed":outcome.is_ok(),"error":outcome.err().map(|e|e.to_string())}));
                });
                Ok(json!({"operation":id}))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    };
    fn save_node(client: &Client, url: String) {
        client.stations.lock().unwrap().insert(
            "node".into(),
            Arc::new(crate::api::StationClient::new(url.clone(), None)),
        );
        client
            .store
            .save_node(&crate::store::SavedNode {
                id: "node".into(),
                name: "fixture".into(),
                url,
                token: None,
                local: false,
                mesh: None,
                group: None,
            })
            .unwrap();
    }
    #[tokio::test]
    async fn checked_version_and_tracked_result_survive_reopening_without_repeating_the_request() {
        let calls = Arc::new(AtomicUsize::new(0));
        let count = calls.clone();
        let router = axum::Router::new().route(
            "/v1/node/update",
            axum::routing::get(move || {
                count.fetch_add(1, Ordering::SeqCst);
                async { axum::Json(json!({"latest_version":"2.0","update":{"supported":true}})) }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let root = tempfile::tempdir().unwrap();
        let mut client = Client::open(root.path()).unwrap();
        save_node(&client, url);
        let result = client
            .tracked_settings_action(
                "node".into(),
                SettingsAction::CheckUpdate,
                Some("request".into()),
            )
            .await
            .unwrap();
        assert_eq!(result["latest_version"], "2.0");
        assert_eq!(
            crate::settings::snapshot(&client.store, "node").unwrap()["update_check"]
                ["latest_version"],
            "2.0"
        );
        drop(client);
        let mut client = Client::open(root.path()).unwrap();
        let replay = client
            .tracked_settings_action(
                "node".into(),
                SettingsAction::CheckUpdate,
                Some("request".into()),
            )
            .await
            .unwrap();
        assert_eq!(replay, result);
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        server.abort();
    }
    #[tokio::test]
    async fn failed_quota_request_preserves_old_value_and_publishes_failure() {
        let router = axum::Router::new().route(
            "/v1/node/profiles/account/refresh",
            axum::routing::post(|| async { axum::http::StatusCode::BAD_GATEWAY }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let root = tempfile::tempdir().unwrap();
        let mut client = Client::open(root.path()).unwrap();
        save_node(&client, url);
        client.store.put("node","public-settings",&json!({"ready":true,"profiles":[{"profile_id":"account",
            "rateLimits":{"ok":true,"rateLimits":{"primary":{"usedPercent":28,"windowDurationMins":300}}}}]})).unwrap();
        let result = client
            .tracked_settings_action(
                "node".into(),
                SettingsAction::RefreshQuota {
                    profile: "account".into(),
                },
                Some("quota-request".into()),
            )
            .await;
        assert!(result.unwrap_err().to_string().contains("额度刷新失败"));
        let snapshot = crate::settings::snapshot(&client.store, "node").unwrap();
        assert_eq!(
            snapshot["profiles"][0]["quota"]["windows"][0]["remaining"],
            72.0
        );
        assert_eq!(snapshot["profile_failed"], json!(["account"]));
        assert_eq!(snapshot["command"]["completed"], false);
        server.abort();
    }
    #[test]
    fn upgrade_confirmation_belongs_to_the_requested_release() {
        let old = json!({"update":{"status":{"version":"1.0","phase":"complete"}}});
        assert!(upgrade_status(&old, "2.0").is_none());
        assert!(upgrade_status(&Value::Null, "2.0").is_none());
        let current = json!({"update":{"status":{"version":"2.0","phase":"complete"}}});
        assert_eq!(
            upgrade_status(&current, "2.0").unwrap()["phase"],
            "complete"
        );
    }
    #[test]
    fn restart_keeps_upgrade_outcome_uncertain_without_a_permanent_busy_flag() {
        let root = tempfile::tempdir().unwrap();
        let store = crate::store::ClientStore::open(root.path()).unwrap();
        store
            .save_node(&crate::store::SavedNode {
                id: "node".into(),
                name: "fixture".into(),
                url: String::new(),
                token: None,
                local: false,
                mesh: None,
                group: None,
            })
            .unwrap();
        store
            .put(
                "node",
                "node-operation",
                &json!({"id":"previous","version":"2.0","running":true}),
            )
            .unwrap();
        let _client = Client::open(root.path()).unwrap();
        let recovered: Value = store.get("node", "node-operation").unwrap().unwrap();
        assert_eq!(recovered["id"], "previous");
        assert_eq!(recovered["version"], "2.0");
        assert_eq!(recovered["running"], false);
        assert_eq!(recovered["completed"], false);
        assert_eq!(recovered["uncertain"], true);
    }
}
