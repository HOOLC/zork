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
    /// Explicit confirmation to abandon an unfinished join that used another
    /// invitation. Omitted on the wire when false, so older Stations accept it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub replace: bool,
}
impl JoinRequest {
    /// Same invitation and choices; `replace` is a confirmation, not a target.
    pub fn same_target(&self, other: &Self) -> bool {
        self.invitation == other.invitation
            && self.name == other.name
            && self.switch_from == other.switch_from
    }
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
/// Stop the Station's unfinished join (the current one when `id` is None).
pub async fn cancel(client: &StationClient, id: Option<&str>) -> Result<JoinProgress> {
    let id = match id {
        Some(id) => id.to_owned(),
        None => {
            current(client)
                .await?
                .filter(|progress| !progress.finished)
                .context("没有正在进行的接入")?
                .id
        }
    };
    ensure!(
        ulid::Ulid::from_string(&id).is_ok(),
        "invalid_join_operation"
    );
    let value = client
        .node_request(
            http::Method::DELETE,
            format!("/v1/node/mesh/join/{id}"),
            None,
        )
        .await
        .map_err(|error| anyhow::anyhow!(failure_label(&error.to_string())))?;
    Ok(serde_json::from_value(value)?)
}
/// The Station's last join operation, finished or not.
pub async fn current(client: &StationClient) -> Result<Option<JoinProgress>> {
    let value = client
        .node_request(http::Method::GET, "/v1/node/mesh".into(), None)
        .await?;
    Ok(match value.get("join").filter(|join| !join.is_null()) {
        Some(join) => Some(serde_json::from_value(join.clone())?),
        None => None,
    })
}
/// Remove a member from the Mesh directory. On a member this is forwarded to
/// the managing device; the removed device loses access immediately.
pub async fn remove_member(client: &StationClient, origin: &str) -> Result<Value> {
    Ok(client
        .node_request(
            http::Method::POST,
            "/v1/node/mesh/members/remove".into(),
            Some(json!({"origin":origin})),
        )
        .await?)
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
    let message = if reason.contains("join_cancelled") {
        "接入已取消"
    } else if reason.contains("another_join_is_in_progress") {
        "本机已有未完成的接入；可以取消它，或确认改用新的邀请"
    } else if reason.contains("join_confirming_try_again") {
        "接入正在确认成员关系，请几秒后再试"
    } else if reason.contains("join_operation_not_found") {
        "没有找到这个接入操作"
    } else if reason.contains("expired_while_unreachable") {
        "邀请设备在邀请有效期内一直无法连接，接入已停止；请重新生成邀请"
    } else if reason.contains("mesh_disabled") {
        "对方设备未启用 Mesh，请在对方启用后重新生成邀请"
    } else if reason.contains("upgrade_required")
        || reason.contains("版本过旧")
        || reason.contains("版本较新")
    {
        return reason.to_owned();
    } else if reason.contains("another_device") || reason.contains("already_used") {
        "邀请已被其它设备使用，请重新生成邀请"
    } else if reason.contains("expired")
        || reason.contains("revoked_invite")
        || reason.contains("invite_not_found")
        || reason.contains("invite_unknown")
    {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replace_is_a_confirmation_not_part_of_the_target_and_is_omitted_when_false() {
        let request = JoinRequest {
            invitation: "zj1_a".into(),
            name: None,
            switch_from: None,
            replace: false,
        };
        let wire = serde_json::to_value(&request).unwrap();
        assert!(
            wire.get("replace").is_none(),
            "older Stations reject unknown fields"
        );
        let confirmed = JoinRequest {
            replace: true,
            ..request.clone()
        };
        assert!(request.same_target(&confirmed));
        assert!(!request.same_target(&JoinRequest {
            invitation: "zj1_b".into(),
            ..request.clone()
        }));
    }

    #[test]
    fn permanent_failures_have_definitive_labels() {
        for reason in [
            "invite_expired",
            "invite_expired_or_unknown",
            "invite_expired_or_restart",
            "invite_not_found",
            "invalid_or_revoked_invite",
        ] {
            assert_eq!(
                failure_label(reason),
                "邀请已失效，请在成员设备重新生成",
                "{reason}"
            );
        }
        assert_eq!(failure_label("join_cancelled"), "接入已取消");
        assert!(
            failure_label("invite_expired_while_unreachable: enrollment_connection_timeout")
                .contains("一直无法连接")
        );
        assert!(failure_label("invite_already_used").contains("其它设备使用"));
    }
}
