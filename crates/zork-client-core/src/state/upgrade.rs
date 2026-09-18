//! One upgrade monitor for all frontends, driven by the shared device feed.
use super::{Device, DeviceData, Domains};
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::{sync::Arc, time::Duration};

pub(crate) fn upgrade_status<'a>(info: &'a Value, version: &str) -> Option<&'a Value> {
    let status = &info["update"]["status"];
    (status["version"] == version).then_some(status)
}

impl Device {
    /// Submit once, then observe. Dropping a monitor never repeats or cancels the
    /// remote installation. Its durable receipt survives the Station restart.
    pub async fn upgrade(
        self: &Arc<Self>,
        version: &str,
        mut progress: impl FnMut(&DeviceData) -> Result<()>,
    ) -> Result<()> {
        ensure!(
            zork_config::update::valid_version(version),
            "无效的升级版本"
        );
        let _operation = self.upgrade_gate.try_lock().context("设备升级仍在进行")?;
        let mut stopped = self.operation_stopped.subscribe();
        let mut updates = self.subscribe_domains(Domains::CONNECTION);
        let initial = updates.snapshot();
        let previous = upgrade_status(&initial.state.info, version).cloned();
        self.start();
        let accepted = self
            .client
            .node_request(
                http::Method::POST,
                "/v1/node/update".into(),
                Some(json!({"version":version})),
            )
            .await?;
        let operation = accepted["update"]["status"]["operation_id"].as_str();
        let mut witnessed_current = false;
        let completion = tokio::time::timeout(Duration::from_secs(360), async {
            loop {
                let update = updates.snapshot();
                ensure!(!update.state.revoked, "设备访问权限已撤销");
                progress(&update.state)?;
                if let Some(status) = upgrade_status(&update.state.info, version) {
                    let current = match operation {
                        Some(id) => status["operation_id"] == id,
                        // Older nodes have no operation identity. Never accept
                        // a cached terminal result from a previous installation.
                        None => witnessed_current || previous.as_ref() != Some(status),
                    };
                    if current {
                        witnessed_current = true;
                        match status["phase"].as_str() {
                            Some("complete") => return Ok(()),
                            Some("failed") => anyhow::bail!(
                                "{}",
                                status["message"].as_str().unwrap_or("升级失败")
                            ),
                            _ => {}
                        }
                    }
                }
                updates.changed().await.context("设备连接已关闭")?;
            }
        });
        tokio::select! {
            _ = stopped.changed() => anyhow::bail!("设备连接已暂停，升级结果待确认"),
            result = completion => result.context("暂时无法确认升级结果，请恢复连接后刷新设备状态")?,
        }
    }
}
