//! Bounded, encrypted invitation exchange before Synch knows a device key.
//! This endpoint accepts enrollment only; all normal traffic stays on Synch.
pub mod ticket;
use anyhow::{ensure, Context, Result};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use iroh::{
    address_lookup::{PkarrPublisher, PkarrResolver},
    endpoint::presets,
    tls::CaTlsConfig,
    Endpoint, EndpointAddr, RelayMode, RelayUrl, SecretKey,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::{future::Future, path::Path, sync::Arc, time::Duration};
use zork_config::{membership::MeshDevice, MeshConfig};

const ALPN: &[u8] = b"zork/enrollment/1";
const MAX_BYTES: usize = 32 * 1024;
pub const INVITE_SECONDS: u64 = 15 * 60;
pub const PROTOCOL: u32 = 1;

#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InviteKind {
    #[default]
    Station,
    Client,
}
impl InviteKind {
    pub fn is_station(&self) -> bool {
        *self == Self::Station
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Invitation {
    #[serde(default, skip_serializing_if = "InviteKind::is_station")]
    pub kind: InviteKind,
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

// Tuple encoding omits repeated JSON field names. Compression retains all
// identity pins and network settings while avoiding a dense camera target.
#[derive(Serialize, Deserialize)]
struct CompactTicket(
    String,
    String,
    EndpointAddr,
    MeshDevice,
    u64,
    bool,
    Option<Vec<String>>,
    Option<String>,
);

impl Invitation {
    pub fn encode(&self) -> Result<String> {
        let (prefix, bytes) = if self.kind == InviteKind::Client {
            use std::io::Write;
            let compact = CompactTicket(
                self.id.clone(),
                self.secret.clone(),
                self.endpoint.clone(),
                self.device.clone(),
                self.expires_at,
                self.offline,
                self.relay_urls.clone(),
                self.discovery_url.clone(),
            );
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
            encoder.write_all(&serde_json::to_vec(&compact)?)?;
            ("zork-client1-", encoder.finish()?)
        } else {
            use std::io::Write;
            let mut encoder =
                flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
            let compact = CompactTicket(
                self.id.clone(),
                self.secret.clone(),
                self.endpoint.clone(),
                self.device.clone(),
                self.expires_at,
                self.offline,
                self.relay_urls.clone(),
                self.discovery_url.clone(),
            );
            // Distinct envelope from the phone tuple: changing the prefix alone
            // can never change an invitation's role.
            encoder.write_all(&serde_json::to_vec(&(PROTOCOL, compact))?)?;
            ("zork-mesh2-", encoder.finish()?)
        };
        Ok(format!("{prefix}{}", URL_SAFE_NO_PAD.encode(bytes)))
    }
    pub fn decode(value: &str) -> Result<Self> {
        ensure!(value.len() <= MAX_BYTES, "invite_too_large");
        let invite: Self = if let Some(raw) = value
            .strip_prefix("zork-client1-")
            .or_else(|| value.strip_prefix("zork-mesh2-"))
        {
            use std::io::Read;
            let bytes = URL_SAFE_NO_PAD.decode(raw)?;
            let mut decoder = flate2::read::ZlibDecoder::new(bytes.as_slice());
            let mut decoded = Vec::new();
            (&mut decoder)
                .take(MAX_BYTES as u64 + 1)
                .read_to_end(&mut decoded)?;
            ensure!(
                decoded.len() <= MAX_BYTES && decoder.total_in() == bytes.len() as u64,
                "invalid_compressed_invite"
            );
            if value.starts_with("zork-client1-") {
                let CompactTicket(
                    id,
                    secret,
                    endpoint,
                    device,
                    expires_at,
                    offline,
                    relay_urls,
                    discovery_url,
                ) = serde_json::from_slice(&decoded)?;
                Self {
                    kind: InviteKind::Client,
                    version: PROTOCOL,
                    id,
                    secret,
                    endpoint,
                    device,
                    expires_at,
                    offline,
                    relay_urls,
                    discovery_url,
                }
            } else {
                let (
                    version,
                    CompactTicket(
                        id,
                        secret,
                        endpoint,
                        device,
                        expires_at,
                        offline,
                        relay_urls,
                        discovery_url,
                    ),
                ): (u32, CompactTicket) = serde_json::from_slice(&decoded)?;
                Self {
                    kind: InviteKind::Station,
                    version,
                    id,
                    secret,
                    endpoint,
                    device,
                    expires_at,
                    offline,
                    relay_urls,
                    discovery_url,
                }
            }
        } else {
            let raw = value
                .strip_prefix("zork-mesh1-")
                .context("unsupported_mesh_invite")?;
            let invite: Self = serde_json::from_slice(&URL_SAFE_NO_PAD.decode(raw)?)?;
            ensure!(invite.kind == InviteKind::Station, "invite_kind_mismatch");
            invite
        };
        ensure!(invite.version == PROTOCOL, "unsupported_mesh_protocol");
        ensure!(
            (ulid::Ulid::from_string(&invite.id).is_ok_and(|value| value.to_string() == invite.id)
                || (invite.id.len() == 32 && invite.id.bytes().all(|b| b.is_ascii_hexdigit())))
                && invite.secret.len() == 64
                && invite.secret.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid_mesh_invite"
        );
        invite.device.validate()?;
        zork_config::services::ServicesConfig {
            relay_urls: invite.relay_urls.clone(),
            discovery_url: invite.discovery_url.clone(),
            cue: None,
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
    endpoint: Endpoint,
    offline: bool,
    relay_urls: Vec<String>,
    relay_access: crate::relay_access::RelayAccess,
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
        let mut builder = Endpoint::builder(presets::N0)
            .secret_key(key)
            .ca_tls_config(CaTlsConfig::system())
            .alpns(vec![ALPN.to_vec()]);
        if config.offline {
            builder = builder
                .relay_mode(RelayMode::Disabled)
                .clear_address_lookup();
        } else {
            if let Some(relays) = &config.relay_urls {
                builder = builder.relay_mode(RelayMode::Custom(
                    relays
                        .iter()
                        .map(|s| s.parse::<RelayUrl>())
                        .collect::<std::result::Result<Vec<_>, _>>()?
                        .into_iter()
                        .collect(),
                ));
            }
            if let Some(discovery) = &config.discovery_url {
                let url: url::Url = discovery.parse()?;
                builder = builder
                    .clear_address_lookup()
                    .address_lookup(PkarrPublisher::builder(url.clone()))
                    .address_lookup(PkarrResolver::builder(url));
            }
        }
        if let Some(addr) = &config.bind {
            let mut addr: std::net::SocketAddr = addr.parse()?;
            addr.set_port(0);
            builder = builder.clear_ip_transports().bind_addr(addr)?;
        }
        Ok(Self {
            endpoint: builder.bind().await?,
            offline: config.offline,
            relay_urls: config.relay_urls.clone().unwrap_or_default(),
            relay_access: Default::default(),
        })
    }

    pub async fn set_relay_access(&self, origin: &str, token: Option<&str>) -> Result<()> {
        if self.offline {
            return Ok(());
        }
        self.relay_access
            .apply(&self.endpoint, &self.relay_urls, origin, token)
            .await
    }

    pub async fn address(&self) -> EndpointAddr {
        if !self.offline {
            let _ = tokio::time::timeout(Duration::from_secs(5), self.endpoint.online()).await;
        }
        self.endpoint.addr()
    }
    pub async fn close(&self) {
        self.endpoint.close().await;
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
        while let Some(incoming) = self.endpoint.accept().await {
            let Ok(slot) = slots.clone().try_acquire_owned() else {
                incoming.refuse();
                continue;
            };
            let handler = handler.clone();
            tokio::spawn(async move {
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
            kind: InviteKind::Station,
            version: 1,
            id: ulid::Ulid::new().to_string(),
            secret: secret(),
            endpoint: server.address().await,
            device: MeshDevice {
                origin: format!("key:{}", "y".repeat(52)),
                name: "fixture".into(),
                addr: None,
            },
            expires_at: now() + 60,
            offline: true,
            relay_urls: None,
            discovery_url: None,
        };
        let mut old_id = invitation.clone();
        old_id.id = "a".repeat(32);
        assert_eq!(
            Invitation::decode(&old_id.encode().unwrap()).unwrap().id,
            old_id.id
        );
        let encoded = invitation.encode().unwrap();
        assert_eq!(
            Invitation::decode(&encoded).unwrap().endpoint,
            invitation.endpoint
        );
        let legacy = format!(
            "zork-mesh1-{}",
            URL_SAFE_NO_PAD.encode(serde_json::to_vec(&invitation).unwrap())
        );
        assert_eq!(
            serde_json::to_value(Invitation::decode(&legacy).unwrap()).unwrap(),
            serde_json::to_value(&invitation).unwrap()
        );
        assert!(encoded.len() < legacy.len() * 3 / 4);
        assert!(Invitation::decode(&encoded.replace("zork-mesh2-", "zork-client1-")).is_err());
        let mut phone = invitation.clone();
        phone.kind = InviteKind::Client;
        let compact = phone.encode().unwrap();
        let legacy_len = "zork-client1-".len()
            + URL_SAFE_NO_PAD
                .encode(serde_json::to_vec(&phone).unwrap())
                .len();
        assert!(
            compact.len() < legacy_len * 3 / 4,
            "compact invitation should materially shrink QR input"
        );
        assert_eq!(
            serde_json::to_value(Invitation::decode(&compact).unwrap()).unwrap(),
            serde_json::to_value(&phone).unwrap()
        );
        assert!(Invitation::decode(&compact.replace("zork-client1-", "zork-mesh1-")).is_err());
        assert!(Invitation::decode(&compact.replace("zork-client1-", "zork-mesh2-")).is_err());
        let mut damaged = URL_SAFE_NO_PAD
            .decode(compact.strip_prefix("zork-client1-").unwrap())
            .unwrap();
        damaged.push(1);
        assert!(
            Invitation::decode(&format!("zork-client1-{}", URL_SAFE_NO_PAD.encode(damaged)))
                .is_err()
        );
        use std::io::Write;
        let mut bomb = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::best());
        bomb.write_all(&vec![b'x'; MAX_BYTES + 1]).unwrap();
        assert!(Invitation::decode(&format!(
            "zork-client1-{}",
            URL_SAFE_NO_PAD.encode(bomb.finish().unwrap())
        ))
        .is_err());
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
