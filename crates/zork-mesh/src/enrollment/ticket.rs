//! Minimal bootstrap capability. Metadata is fetched only over pinned TLS.
use super::*;

#[derive(Clone)]
pub struct Ticket {
    pub kind: InviteKind,
    pub endpoint: EndpointAddr,
    token: [u8; 16],
    bootstrap: Bootstrap,
}
#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Bootstrap {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    offline: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    address: Option<std::net::SocketAddr>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    discovery_url: Option<String>,
}
fn address_rank(ip: std::net::IpAddr, config: &MeshConfig) -> u8 {
    if config
        .bind
        .as_ref()
        .and_then(|s| s.parse::<std::net::SocketAddr>().ok())
        .is_some_and(|a| !a.ip().is_unspecified() && a.ip() == ip)
    {
        return 0;
    }
    match ip {
        std::net::IpAddr::V4(ip) if ip.is_private() => 1,
        std::net::IpAddr::V6(ip) if ip.is_unique_local() => 2,
        ip if ip.is_loopback() => 5,
        std::net::IpAddr::V6(ip) if ip.is_unicast_link_local() => 6,
        std::net::IpAddr::V4(_) => 3,
        _ => 4,
    }
}
impl Ticket {
    pub fn new(kind: InviteKind, address: EndpointAddr, config: &MeshConfig) -> Result<Self> {
        let bytes = SecretKey::generate().to_bytes();
        let token = bytes[..16].try_into().unwrap();
        let direct = if config.offline {
            Some(
                address
                    .ip_addrs()
                    .filter(|a| !a.ip().is_unspecified())
                    .min_by_key(|a| address_rank(a.ip(), config))
                    .copied()
                    .context("offline_invitation_needs_address")?,
            )
        } else {
            None
        };
        let mut endpoint = EndpointAddr::new(address.id);
        if let Some(addr) = direct {
            endpoint = endpoint.with_ip_addr(addr);
        }
        Ok(Self {
            kind,
            endpoint,
            token,
            bootstrap: Bootstrap {
                offline: config.offline,
                address: direct,
                discovery_url: config.discovery_url.clone(),
            },
        })
    }
    pub fn is_short(value: &str) -> bool {
        value.starts_with("zj1_") || value.starts_with("zc1_")
    }
    pub fn id(&self) -> String {
        crate::content_root(&self.token)[..32].to_owned()
    }
    pub fn secret(&self) -> String {
        crate::content_root(&self.token)
    }
    pub fn encode(&self) -> Result<String> {
        let mut bytes = self.endpoint.id.as_bytes().to_vec();
        bytes.extend_from_slice(&self.token);
        if self.bootstrap.offline || self.bootstrap.discovery_url.is_some() {
            let flags = u8::from(self.bootstrap.offline)
                | self
                    .bootstrap
                    .address
                    .map(|a| if a.is_ipv4() { 2 } else { 4 })
                    .unwrap_or(0)
                | if self.bootstrap.discovery_url.is_some() {
                    8
                } else {
                    0
                };
            bytes.push(flags);
            if let Some(addr) = self.bootstrap.address {
                match addr.ip() {
                    std::net::IpAddr::V4(ip) => bytes.extend_from_slice(&ip.octets()),
                    std::net::IpAddr::V6(ip) => bytes.extend_from_slice(&ip.octets()),
                }
                bytes.extend_from_slice(&addr.port().to_be_bytes());
            }
            if let Some(url) = &self.bootstrap.discovery_url {
                ensure!(url.len() <= 2048, "bootstrap_url_too_long");
                bytes.extend_from_slice(&(url.len() as u16).to_be_bytes());
                bytes.extend_from_slice(url.as_bytes());
            }
        }
        Ok(format!(
            "{}{}",
            if self.kind == InviteKind::Client {
                "zc1_"
            } else {
                "zj1_"
            },
            URL_SAFE_NO_PAD.encode(bytes)
        ))
    }
    pub fn decode(value: &str) -> Result<Self> {
        ensure!(value.len() <= 4096, "invite_too_large");
        let (kind, raw) = if let Some(raw) = value.strip_prefix("zc1_") {
            (InviteKind::Client, raw)
        } else {
            (
                InviteKind::Station,
                value
                    .strip_prefix("zj1_")
                    .context("unsupported_mesh_invite")?,
            )
        };
        let bytes = URL_SAFE_NO_PAD.decode(raw)?;
        ensure!(bytes.len() >= 48, "invalid_short_invite");
        let id = iroh::EndpointId::from_bytes(bytes[..32].try_into().unwrap())?;
        let mut bootstrap = Bootstrap::default();
        if bytes.len() > 48 {
            let flags = bytes[48];
            ensure!(
                flags != 0 && flags & !15 == 0 && flags & 6 != 6,
                "invalid_bootstrap_flags"
            );
            bootstrap.offline = flags & 1 != 0;
            let mut cursor = 49;
            let address_len = if flags & 2 != 0 {
                4
            } else if flags & 4 != 0 {
                16
            } else {
                0
            };
            if address_len != 0 {
                ensure!(
                    bytes.len() >= cursor + address_len + 2,
                    "invalid_bootstrap_address"
                );
                let ip: std::net::IpAddr = if address_len == 4 {
                    std::net::Ipv4Addr::from(
                        <[u8; 4]>::try_from(&bytes[cursor..cursor + 4]).unwrap(),
                    )
                    .into()
                } else {
                    std::net::Ipv6Addr::from(
                        <[u8; 16]>::try_from(&bytes[cursor..cursor + 16]).unwrap(),
                    )
                    .into()
                };
                cursor += address_len;
                let port = u16::from_be_bytes(bytes[cursor..cursor + 2].try_into().unwrap());
                cursor += 2;
                bootstrap.address = Some(std::net::SocketAddr::new(ip, port));
            }
            if flags & 8 != 0 {
                ensure!(bytes.len() >= cursor + 2, "invalid_bootstrap_url");
                let len =
                    u16::from_be_bytes(bytes[cursor..cursor + 2].try_into().unwrap()) as usize;
                cursor += 2;
                ensure!(
                    len > 0 && len <= 2048 && bytes.len() >= cursor + len,
                    "invalid_bootstrap_url"
                );
                bootstrap.discovery_url =
                    Some(std::str::from_utf8(&bytes[cursor..cursor + len])?.to_owned());
                cursor += len;
            }
            ensure!(cursor == bytes.len(), "invalid_bootstrap_trailing_data");
        }
        let mut endpoint = EndpointAddr::new(id);
        if let Some(addr) = bootstrap.address {
            ensure!(
                addr.port() != 0 && !addr.ip().is_unspecified() && !addr.ip().is_multicast(),
                "invalid_bootstrap_address"
            );
            endpoint = endpoint.with_ip_addr(addr);
        }
        ensure!(
            !bootstrap.offline || bootstrap.address.is_some(),
            "offline_invitation_needs_address"
        );
        zork_config::services::ServicesConfig {
            discovery_url: bootstrap.discovery_url.clone(),
            ..Default::default()
        }
        .validate()?;
        Ok(Self {
            kind,
            endpoint,
            token: bytes[32..48].try_into().unwrap(),
            bootstrap,
        })
    }
    pub fn network_config(&self) -> MeshConfig {
        MeshConfig {
            offline: self.bootstrap.offline,
            discovery_url: self.bootstrap.discovery_url.clone(),
            bind: Some("0.0.0.0:0".into()),
            ..Default::default()
        }
    }
    pub async fn resolve(&self, root: &Path) -> Result<Invitation> {
        let config = self.network_config();
        let transport =
            tokio::time::timeout(Duration::from_secs(15), Enrollment::bind(root, &config))
                .await
                .context("bootstrap_bind_timeout")??;
        let result = self.resolve_using(&transport).await;
        let _ = tokio::time::timeout(Duration::from_secs(3), transport.close()).await;
        result
    }
    pub async fn resolve_using(&self, transport: &Enrollment) -> Result<Invitation> {
        let result = transport
            .exchange_endpoint(
                self.endpoint.clone(),
                &json!({"op":"resolve","id":self.id(),"secret":self.secret(),"kind":self.kind}),
            )
            .await;
        let value = result?;
        let invite: Invitation = serde_json::from_value(value["invitation"].clone())?;
        ensure!(
            invite.kind == self.kind
                && invite.id == self.id()
                && invite.secret == self.secret()
                && invite.endpoint.id == self.endpoint.id,
            "invite_metadata_mismatch"
        );
        // Reuse the full legacy validator, including URL and device checks.
        Invitation::decode(&invite.encode()?)
    }
}

pub async fn resolve(root: &Path, value: &str, expected: InviteKind) -> Result<Invitation> {
    let invite = if Ticket::is_short(value) {
        let ticket = Ticket::decode(value)?;
        ensure!(ticket.kind == expected, "invite_kind_mismatch");
        ticket.resolve(root).await?
    } else {
        Invitation::decode(value)?
    };
    ensure!(invite.kind == expected, "invite_kind_mismatch");
    Ok(invite)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn short_ticket_is_68_chars_and_preserves_bootstrap() {
        for kind in [InviteKind::Client, InviteKind::Station] {
            let addr = EndpointAddr::new(SecretKey::generate().public());
            let ticket = Ticket::new(kind, addr, &MeshConfig::default()).unwrap();
            let value = ticket.encode().unwrap();
            assert_eq!(value.len(), 68);
            let decoded = Ticket::decode(&value).unwrap();
            assert_eq!(decoded.id(), ticket.id());
            assert_eq!(decoded.secret(), ticket.secret());
            assert_eq!(decoded.kind, kind);
            assert!(Ticket::decode(&value[..30]).is_err());
        }
        let config = MeshConfig {
            offline: true,
            ..Default::default()
        };
        let addr = EndpointAddr::new(SecretKey::generate().public())
            .with_ip_addr("127.0.0.1:2345".parse().unwrap());
        let ticket = Ticket::new(InviteKind::Station, addr, &config).unwrap();
        let multi = EndpointAddr::new(SecretKey::generate().public())
            .with_ip_addr("100.67.165.110:2345".parse().unwrap())
            .with_ip_addr("192.168.20.158:2345".parse().unwrap());
        assert_eq!(
            Ticket::new(InviteKind::Station, multi, &config)
                .unwrap()
                .bootstrap
                .address
                .unwrap()
                .ip()
                .to_string(),
            "192.168.20.158"
        );
        assert_eq!(ticket.encode().unwrap().len(), 78);
        assert_eq!(
            Ticket::decode(&ticket.encode().unwrap())
                .unwrap()
                .bootstrap
                .address,
            ticket.bootstrap.address
        );
        let mut corrupted = URL_SAFE_NO_PAD
            .decode(&ticket.encode().unwrap()[4..])
            .unwrap();
        corrupted.push(1);
        assert!(Ticket::decode(&format!("zj1_{}", URL_SAFE_NO_PAD.encode(corrupted))).is_err());
    }
}
