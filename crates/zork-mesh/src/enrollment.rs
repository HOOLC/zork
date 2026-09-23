//! Bounded, encrypted invitation exchange before a device becomes a member.
//! This endpoint accepts enrollment only; business traffic uses the owned iroh runtime.
pub mod ticket;
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use iroh::{endpoint::presets, tls::CaTlsConfig, Endpoint, EndpointAddr, SecretKey};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{future::Future, path::Path, sync::Arc, time::Duration};
use zork_config::{membership::MeshDevice, MeshConfig};

const ALPN: &[u8] = b"zork/enrollment/1";
const MAX_BYTES: usize = 32 * 1024;
pub const INVITE_SECONDS: u64 = 15 * 60;
pub const PROTOCOL: u32 = 1;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invitation {
    #[serde(default)]
    pub channel: zork_config::channel::Channel,
    #[serde(default)]
    pub relay_quic_port: Option<u16>,
    #[serde(default)]
    pub quic_discovery_urls: Option<Vec<String>>,
    pub version: u32,
    pub id: String,
    pub secret: String,
    pub endpoint: EndpointAddr,
    pub device: MeshDevice,
    pub expires_at: u64,
    pub offline: bool,
    pub relay_urls: Option<Vec<String>>,
    pub discovery_url: Option<String>,
}

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}
pub fn secret() -> String {
    zork_config::random_token()
}

impl Invitation {
    pub fn encode(&self) -> Result<String> {
        Ok(format!(
            "zork-node1-{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(self)?)
        ))
    }
    pub fn decode(value: &str) -> Result<Self> {
        ensure!(value.len() <= MAX_BYTES, "invite_too_large");
        let raw = value
            .strip_prefix("zork-node1-")
            .context("unsupported_mesh_invite")?;
        let invite: Self = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(raw)?)?;
        ensure!(invite.version == PROTOCOL, "unsupported_mesh_protocol");
        ensure!(
            (invite.id.len() == 32 && invite.id.bytes().all(|b| b.is_ascii_hexdigit()))
                && invite.secret.len() == 64
                && invite.secret.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid_mesh_invite"
        );
        invite.device.validate()?;
        zork_config::services::ServicesConfig {
            relay_urls: invite.relay_urls.clone(),
            relay_quic_port: invite.relay_quic_port,
            discovery_url: invite.discovery_url.clone(),
            quic_discovery_urls: invite.quic_discovery_urls.clone(),
        }
        .validate()?;
        ensure!(
            invite.endpoint.addrs.len() <= 24,
            "too_many_invite_addresses"
        );
        for relay in invite.endpoint.relay_urls() {
            zork_config::services::validate_endpoint(relay.as_str())?;
        }
        Ok(invite)
    }
}

pub struct Enrollment {
    offline: bool,
    endpoint: Endpoint,
    http_route: crate::network::RelayHttpRoute,
    discovery: tokio::sync::Mutex<Option<crate::lan_discovery::Registration>>,
    local: std::sync::Mutex<Option<crate::local_discovery::Registration>>,
}

impl Enrollment {
    pub async fn bind(root: &Path, config: &MeshConfig) -> Result<Self> {
        let dir = root.join("mesh");
        std::fs::create_dir_all(&dir)?;
        let path = dir.join("enrollment.key");
        let key = match std::fs::read(&path) {
            Ok(bytes) => SecretKey::from_bytes(
                &bytes
                    .try_into()
                    .map_err(|_| anyhow::anyhow!("invalid_enrollment_key"))?,
            ),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                let key = SecretKey::generate();
                use std::io::Write;
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                let mut file = options.open(&path)?;
                file.write_all(&key.to_bytes())?;
                file.sync_all()?;
                key
            }
            Err(e) => return Err(e.into()),
        };
        let mut options = crate::managed::network_options(config)?;
        if let Some(addr) = &mut options.bind_addr {
            addr.set_port(0);
        }
        let (builder, http_route) = crate::network::configure_endpoint(
            Endpoint::builder(presets::N0)
                .secret_key(key)
                .ca_tls_config(CaTlsConfig::system())
                .alpns(vec![ALPN.to_vec()]),
            &options,
        )
        .await?;
        let endpoint = builder.bind().await?;
        let discovery = if std::env::var_os("ZORK_MESH_LAN_DISCOVERY").is_none_or(|v| v != "0") {
            Some(crate::lan_discovery::install(
                &endpoint,
                "zork-enrollment-v1",
            )?)
        } else {
            None
        };
        let local = crate::local_discovery::install(&endpoint).await?;
        Ok(Self {
            offline: config.offline,
            endpoint,
            http_route,
            discovery: tokio::sync::Mutex::new(discovery),
            local: std::sync::Mutex::new(local),
        })
    }

    pub async fn bind_for_node(
        root: &Path,
        config: &MeshConfig,
        _node: &crate::node::MeshNode,
    ) -> Result<Self> {
        Self::bind(root, config).await
    }

    pub async fn address(&self) -> EndpointAddr {
        let current = self.endpoint.addr();
        if !self.offline
            && current.ip_addrs().next().is_none()
            && current.relay_urls().next().is_none()
        {
            let _ = tokio::time::timeout(Duration::from_secs(5), self.endpoint.online()).await;
        }
        self.endpoint.addr()
    }
    pub async fn close(&self) {
        if let Some(discovery) = self.discovery.lock().await.take() {
            discovery.shutdown().await;
        }
        self.endpoint.close().await;
        self.http_route.shutdown().await;
        self.local.lock().unwrap().take();
    }

    pub async fn exchange(&self, invitation: &Invitation, body: &Value) -> Result<Value> {
        self.exchange_endpoint(invitation.endpoint.clone(), body)
            .await
    }
    pub async fn exchange_endpoint(&self, endpoint: EndpointAddr, body: &Value) -> Result<Value> {
        let bytes = serde_json::to_vec(body)?;
        ensure!(bytes.len() <= MAX_BYTES, "enrollment_request_too_large");
        tokio::time::timeout(Duration::from_secs(45), async {
            let connection = self.endpoint.connect(endpoint, ALPN).await?;
            let (mut send, mut receive) = connection.open_bi().await?;
            send.write_all(&bytes).await?;
            send.finish()?;
            let data = receive.read_to_end(MAX_BYTES).await?;
            let reply: Value = serde_json::from_slice(&data)?;
            connection.close(0u32.into(), b"enrollment received");
            ensure!(
                reply["ok"] == true,
                "{}",
                reply["error"].as_str().unwrap_or("enrollment_failed")
            );
            Ok(reply["data"].clone())
        })
        .await
        .context("enrollment_connection_timeout")?
    }

    pub async fn serve<F, Fut>(&self, handler: F) -> Result<()>
    where
        F: Fn(String, Value) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<Value>> + Send + 'static,
    {
        let handler = Arc::new(handler);
        let slots = Arc::new(tokio::sync::Semaphore::new(8));
        let mut requests = tokio::task::JoinSet::new();
        loop {
            let incoming = tokio::select! {
                incoming = self.endpoint.accept() => incoming,
                _ = requests.join_next(), if !requests.is_empty() => continue,
            };
            let Some(incoming) = incoming else {
                break;
            };
            let Ok(slot) = slots.clone().try_acquire_owned() else {
                incoming.refuse();
                continue;
            };
            let handler = handler.clone();
            requests.spawn(async move {
                let _slot = slot;
                let _ = tokio::time::timeout(Duration::from_secs(50), async {
                    let connection = incoming.await?;
                    let (mut send, mut receive) = connection.accept_bi().await?;
                    let request = tokio::time::timeout(
                        Duration::from_secs(5),
                        receive.read_to_end(MAX_BYTES),
                    )
                    .await??;
                    let request: Value = serde_json::from_slice(&request)?;
                    let response = match handler(connection.remote_id().to_string(), request).await
                    {
                        Ok(value) => json!({"ok":true,"data":value}),
                        Err(error) => json!({"ok":false,"error":error.to_string()}),
                    };
                    let bytes = serde_json::to_vec(&response)?;
                    ensure!(bytes.len() <= MAX_BYTES, "enrollment_response_too_large");
                    send.write_all(&bytes).await?;
                    send.finish()?;
                    let _ = tokio::time::timeout(Duration::from_secs(3), connection.closed()).await;
                    Ok::<_, anyhow::Error>(())
                })
                .await;
            });
        }
        requests.shutdown().await;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn invitation_pins_server_and_uses_real_encrypted_transport() {
        let a = tempfile::tempdir().unwrap();
        let b = tempfile::tempdir().unwrap();
        let config = MeshConfig {
            offline: true,
            bind: Some("127.0.0.1:0".into()),
            ..Default::default()
        };
        let server = Arc::new(Enrollment::bind(a.path(), &config).await.unwrap());
        let client = Enrollment::bind(b.path(), &config).await.unwrap();
        let invitation = Invitation {
            channel: Default::default(),
            relay_quic_port: None,
            quic_discovery_urls: None,
            version: 1,
            id: "a".repeat(32),
            secret: secret(),
            endpoint: server.address().await,
            device: MeshDevice {
                routes: None,
                origin: format!("key:{}", "y".repeat(52)),
                name: "fixture".into(),
                addr: None,
            },
            expires_at: now() + 60,
            offline: true,
            relay_urls: None,
            discovery_url: None,
        };
        let encoded = invitation.encode().unwrap();
        assert_eq!(
            Invitation::decode(&encoded).unwrap().endpoint,
            invitation.endpoint
        );
        let server_task =
            tokio::spawn(async move { server.serve(|_, value| async move { Ok(value) }).await });
        assert_eq!(
            client
                .exchange(&invitation, &json!({"test":"encrypted"}))
                .await
                .unwrap(),
            json!({"test":"encrypted"})
        );
        server_task.abort();
    }
}
