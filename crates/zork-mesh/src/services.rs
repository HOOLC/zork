//! Browser-facing loopback listeners over authenticated iroh streams.
use crate::node::MeshNode;
use anyhow::{ensure, Context, Result};
use std::time::Duration;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::{JoinHandle, JoinSet},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceLink {
    pub origin: String,
    pub id: String,
    suffix: String,
}
impl ServiceLink {
    pub fn parse(value: &str) -> Result<Self> {
        ensure!(value.len() <= 8192, "service_link_too_long");
        let url = url::Url::parse(value)?;
        ensure!(
            url.scheme() == "zork"
                && url.host_str() == Some("service")
                && url.port().is_none()
                && url.username().is_empty()
                && url.password().is_none(),
            "invalid_service_link"
        );
        let mut parts = url.path().trim_start_matches('/').splitn(3, '/');
        let key = parts.next().context("missing_service_node")?;
        let node = iroh::EndpointId::from_z32(key)?;
        ensure!(node.to_z32() == key, "invalid_service_node");
        let id = parts.next().context("missing_service_id")?;
        ensure!(
            ulid::Ulid::from_string(id).is_ok_and(|value| value.to_string() == id)
                || (id.len() == 32
                    && id
                        .bytes()
                        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))),
            "invalid_service_id"
        );
        let mut suffix = format!("/{}", parts.next().unwrap_or_default());
        if let Some(query) = url.query() {
            suffix.push('?');
            suffix.push_str(query);
        }
        if let Some(fragment) = url.fragment() {
            suffix.push('#');
            suffix.push_str(fragment);
        }
        Ok(Self {
            origin: format!("key:{key}"),
            id: id.into(),
            suffix,
        })
    }
}

/// Drop closes the listener and every accepted stream; no detached forwards.
pub struct LocalService {
    pub url: String,
    task: JoinHandle<()>,
}
impl Drop for LocalService {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl LocalService {
    pub async fn open(node: MeshNode, link: &ServiceLink) -> Result<Self> {
        // Fail before opening a browser if the service is unavailable or denied.
        let initial = node.connect_service(&link.origin, &link.id).await?;
        let listener = TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        // Different services need different hosts: cookies do not isolate by port.
        let authority = format!(
            "s{}.localhost:{port}",
            &blake3::hash(format!("{}:{}", link.origin, link.id).as_bytes()).to_hex()[..32]
        );
        let url = format!("http://{authority}{}", link.suffix);
        let link = link.clone();
        let task = tokio::spawn(async move {
            let mut connections = JoinSet::new();
            let mut initial = Some(initial);
            loop {
                tokio::select! {
                    biased;
                    _ = connections.join_next(), if !connections.is_empty() => {},
                    result = listener.accept() => {
                        let Ok((mut socket, _)) = result else { break };
                        if connections.len() >= 32 { continue; }
                        let node = node.clone();
                        let link = link.clone();
                        let authority = authority.clone();
                        let primed = initial.take();
                        connections.spawn(async move {
                            let result = async {
                                let prefix = tokio::time::timeout(Duration::from_secs(10), browser_header(&mut socket, &authority)).await??;
                                let upstream = match primed { Some(stream) => stream, None => node.connect_service(&link.origin, &link.id).await? };
                                Ok::<_, anyhow::Error>((prefix, upstream))
                            }.await;
                            match result {
                                Ok((prefix, upstream)) => { let _ = upstream.forward(&mut socket, &prefix).await; }
                                Err(_) => {
                                    let _ = socket.write_all(b"HTTP/1.1 502 Bad Station\r\nContent-Type: text/plain; charset=utf-8\r\nConnection: close\r\n\r\nService unavailable. Reopen the shared link or ask the agent to share it again.").await;
                                }
                            }
                        });
                    }
                }
            }
        });
        Ok(Self { url, task })
    }
}

/// Keep arbitrary hosts and cross-site browser requests out of the local bridge.
/// Inspect only the initial HTTP header; the remainder, including upgrades, is raw.
async fn browser_header(socket: &mut TcpStream, authority: &str) -> Result<Vec<u8>> {
    let mut bytes = Vec::with_capacity(4096);
    loop {
        let previous = bytes.len();
        let mut chunk = [0; 4096];
        let count = socket.read(&mut chunk).await?;
        ensure!(count > 0, "incomplete_http_header");
        bytes.extend_from_slice(&chunk[..count]);
        let start = previous.saturating_sub(3);
        if let Some(end) = bytes[start..]
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
        {
            let end = start + end + 4;
            ensure!(end <= 32768, "service_http_header_too_large");
            validate_header(&bytes[..end], authority)?;
            // Preserve any body or upgrade bytes received with the header.
            return Ok(bytes);
        }
        ensure!(bytes.len() < 32768, "service_http_header_too_large");
    }
}

fn validate_header(bytes: &[u8], authority: &str) -> Result<()> {
    let header = std::str::from_utf8(bytes)?;
    let mut host = None;
    for line in header.split("\r\n").skip(1).filter(|s| !s.is_empty()) {
        let (key, value) = line.split_once(':').context("invalid_http_header")?;
        let value = value.trim();
        if key.eq_ignore_ascii_case("host") {
            ensure!(host.replace(value).is_none(), "duplicate_host");
        }
        if key.eq_ignore_ascii_case("origin") {
            ensure!(
                value == format!("http://{authority}"),
                "cross_origin_service_request"
            );
        }
        if key.eq_ignore_ascii_case("sec-fetch-site") {
            ensure!(
                matches!(value, "same-origin" | "none"),
                "cross_site_service_request"
            );
        }
    }
    ensure!(host == Some(authority), "unexpected_service_host");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn links_preserve_paths_and_reject_noncanonical_targets() {
        let key = crate::control::key_origin(&"00".repeat(32)).unwrap();
        let key = key.trim_start_matches("key:");
        let url = format!("zork://service/{key}/{}/a?q=1#part", "a".repeat(32));
        let link = ServiceLink::parse(&url).unwrap();
        assert_eq!(link.suffix, "/a?q=1#part");
        let id = ulid::Ulid::new().to_string();
        let new_url = url.replace(&"a".repeat(32), &id);
        let new_link = ServiceLink::parse(&new_url).unwrap();
        assert_eq!(new_link.id, id);
        assert_eq!(new_link.suffix, link.suffix);
        for invalid in [
            "8ZZZZZZZZZZZZZZZZZZZZZZZZZ",
            "../admin",
            "01INVALID00000000000000000",
        ] {
            assert!(ServiceLink::parse(&url.replace(&"a".repeat(32), invalid)).is_err());
        }
        for bad in [
            url.replace("zork:", "https:"),
            url.replace("service/", "user@service/"),
            url.replace(&"a".repeat(32), "../admin"),
            url.replace("service/", "service:80/"),
        ] {
            assert!(ServiceLink::parse(&bad).is_err(), "{bad}");
        }
    }
    #[test]
    fn browser_bridge_rejects_cross_site_and_rebound_hosts() {
        let host = "s123.localhost:9876";
        assert!(validate_header(format!("GET / HTTP/1.1\r\nHost: {host}\r\nOrigin: http://{host}\r\nSec-Fetch-Site: same-origin\r\n\r\n").as_bytes(), host).is_ok());
        for headers in [
            "Host: evil.example",
            "Host: s123.localhost:9876\r\nOrigin: https://evil.example",
            "Host: s123.localhost:9876\r\nSec-Fetch-Site: cross-site",
            "Host: s123.localhost:9876\r\nHost: evil.example",
        ] {
            assert!(validate_header(
                format!("GET / HTTP/1.1\r\n{headers}\r\n\r\n").as_bytes(),
                host
            )
            .is_err());
        }
    }
}
