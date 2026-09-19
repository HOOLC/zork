//! Station enrollment intents and progress; shared by command-line/platform hosts.
use crate::api::StationClient;
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use zork_config::membership::MeshVersion;

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct JoinRequest {
    pub invitation: String,
    pub name: Option<String>,
    #[serde(default)]
    pub switch_from: Option<MeshVersion>,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct JoinProgress {
    pub id: String,
    pub phase: String,
    pub label: String,
    pub attempt: u64,
    pub retry_in_seconds: Option<u64>,
    pub finished: bool,
    pub result: Option<Value>,
    pub error: Option<String>,
}
pub async fn join(
    client: &StationClient,
    request: &JoinRequest,
    mut progress: impl FnMut(&JoinProgress),
) -> Result<Value> {
    let initial = client
        .node_request(
            http::Method::POST,
            "/v1/node/mesh/join".into(),
            Some(json!(request)),
        )
        .await?;
    let mut current: JoinProgress = serde_json::from_value(initial)?;
    ensure!(
        ulid::Ulid::from_string(&current.id).is_ok(),
        "invalid_join_operation"
    );
    let path = format!("/v1/node/mesh/join/{}", current.id);
    let mut retry = zork_mesh::retry::DiscoveryBackoff::default();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(20 * 60);
    loop {
        progress(&current);
        if current.finished {
            if let Some(error) = current.error {
                anyhow::bail!("{}", failure_label(&error));
            }
            return current.result.context("join_result_missing");
        }
        ensure!(
            tokio::time::Instant::now() < deadline,
            "接入仍在后台进行，请用 mesh status 查看进度"
        );
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        match client
            .node_request(http::Method::GET, path.clone(), None)
            .await
        {
            Ok(value) => {
                current = serde_json::from_value(value)?;
                retry.reset();
            }
            Err(error)
                if error.status().is_none() || matches!(error.status(), Some(502 | 503 | 504)) =>
            {
                let delay = retry.next_delay();
                current.label = "正在恢复设备连接，接入进度已保存".into();
                current.retry_in_seconds = Some(delay.as_secs());
                progress(&current);
                tokio::time::sleep(delay).await;
            }
            Err(error) => return Err(error.into()),
        }
    }
}
pub async fn leave(client: &StationClient, expected: &MeshVersion) -> Result<Value> {
    Ok(client
        .node_request(
            http::Method::POST,
            "/v1/node/mesh/leave".into(),
            Some(json!({"expected":expected})),
        )
        .await?)
}

pub fn failure_label(reason: &str) -> String {
    let message = if reason.contains("another_device") || reason.contains("already_used") {
        "邀请已被其它设备使用，请重新生成邀请"
    } else if reason.contains("expired") || reason.contains("revoked_invite") {
        "邀请已失效，请在成员设备重新生成"
    } else if reason == "already_in_another_mesh" {
        "当前已加入其它 Mesh，请使用切换并确认"
    } else if reason.contains("membership_changed") {
        "Mesh 成员已变化，请刷新后重新确认切换"
    } else if reason.contains("device_removed") {
        "设备已被移出 Mesh，需要新的邀请"
    } else if reason.ends_with("_limit") {
        "Mesh 设备数已达上限"
    } else if reason == "cannot_join_this_device_to_itself" {
        "不能将设备加入自己"
    } else if reason.starts_with("invalid_") || reason.starts_with("unsupported_") {
        "邀请无效或版本不受支持，请重新生成"
    } else {
        return format!("接入未完成：{reason}");
    };
    message.into()
}
