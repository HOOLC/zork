//! HTTPS uses the system TCP route (including a configured proxy or TUN).
//! UDP address discovery uses its own resolver on the endpoint's real socket.
use std::{net::{Ipv4Addr, SocketAddr}, sync::{Arc, Mutex}, time::Duration};
use tokio::{io::{AsyncReadExt, AsyncWriteExt}, net::{TcpListener, TcpStream}, task::JoinSet};
use url::Url;

/// Owns any local proxy needed by this endpoint. Explicit proxies are external.
#[derive(Clone)]
pub struct RelayHttpRoute {
    url: Option<Url>,
    owner: Option<Arc<ProxyOwner>>,
}
impl std::fmt::Debug for RelayHttpRoute {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_struct("RelayHttpRoute").finish_non_exhaustive() }
}
impl RelayHttpRoute {
    pub(crate) fn disabled() -> Self { Self { url: None, owner: None } }
    pub(crate) fn url(&self) -> Option<Url> { self.url.clone() }
    /// Closes only proxy IO owned by this endpoint.
    pub async fn shutdown(&self) {
        let task = self.owner.as_ref().and_then(|owner| owner.task.lock().unwrap().take());
        if let Some(task) = task { task.abort(); let _ = task.await; }
    }
}
struct ProxyOwner { task: Mutex<Option<tokio::task::JoinHandle<()>>> }
impl Drop for ProxyOwner {
    fn drop(&mut self) { if let Some(task) = self.task.get_mut().unwrap().take() { task.abort(); } }
}

pub(crate) async fn relay_https_proxy() -> std::io::Result<RelayHttpRoute> {
    for key in ["HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy"] {
        if let Ok(value) = std::env::var(key) {
            if let Ok(url) = value.parse::<Url>() {
                if matches!(url.scheme(), "http" | "https") { return Ok(RelayHttpRoute { url: Some(url), owner: None }); }
            }
        }
    }
    start_system_connect_proxy().await
}

async fn start_system_connect_proxy() -> std::io::Result<RelayHttpRoute> {
    let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
    let url = Url::parse(&format!("http://{}", listener.local_addr()?)).map_err(std::io::Error::other)?;
    let task = tokio::spawn(async move {
        let mut sessions = JoinSet::new();
        let mut failures = 0u64;
        loop {
            let accepted = tokio::select! {
                result = listener.accept() => result,
                _ = sessions.join_next(), if !sessions.is_empty() => continue,
            };
            match accepted {
                Ok((stream, _)) if sessions.len() < 128 => { failures = 0; sessions.spawn(async move {
                    if let Err(error) = handle_connect(stream).await { tracing::debug!(%error, "relay HTTPS session ended"); }
                }); }
                Ok(_) => {},
                Err(error) => {
                    let delay = 1u64.saturating_add((failures / 5).saturating_mul(5));
                    failures = failures.saturating_add(1);
                    tracing::warn!(%error, retry_in_secs = delay, "relay HTTPS accept retry");
                    tokio::time::sleep(Duration::from_secs(delay)).await;
                }
            }
        }
    });
    Ok(RelayHttpRoute { url: Some(url), owner: Some(Arc::new(ProxyOwner { task: Mutex::new(Some(task)) })) })
}

async fn handle_connect(mut inbound: TcpStream) -> std::io::Result<()> {
    inbound.set_nodelay(true)?;
    let header = tokio::time::timeout(Duration::from_secs(5), async {
        let mut buf = Vec::with_capacity(256);
        while !buf.ends_with(b"\r\n\r\n") {
            let byte = inbound.read_u8().await?;
            buf.push(byte);
            if buf.len() > 8192 { return Err(std::io::Error::other("proxy headers too large")); }
        }
        String::from_utf8(buf).map_err(std::io::Error::other)
    }).await.map_err(std::io::Error::other)??;
    let mut lines = header.lines();
    let mut first = lines.next().unwrap_or_default().split_whitespace();
    let method = first.next().unwrap_or_default();
    let target = first.next().unwrap_or_default();
    let tunnel = method == "CONNECT";
    let (host, port, path) = if tunnel {
        let (host, port) = parse_connect_target(target).ok_or_else(|| std::io::Error::other("invalid CONNECT target"))?;
        (host, port, None)
    } else {
        // HTTP probes use absolute-form requests. All product payloads use TLS.
        if !matches!(method, "GET" | "HEAD") { return Err(std::io::Error::other("unsupported proxy request")); }
        let url = Url::parse(target).map_err(std::io::Error::other)?;
        if url.scheme() != "http" || !url.username().is_empty() || url.password().is_some() { return Err(std::io::Error::other("invalid HTTP probe")); }
        let host = url.host_str().ok_or_else(|| std::io::Error::other("proxy target missing host"))?.to_owned();
        let path = match url.query() { Some(query) => format!("{}?{query}", url.path()), None => url.path().to_owned() };
        (host, url.port_or_known_default().unwrap_or(80), Some(path))
    };
    let mut outbound = tokio::time::timeout(Duration::from_secs(15), TcpStream::connect((host.as_str(), port)))
        .await.map_err(std::io::Error::other)??;
    outbound.set_nodelay(true)?;
    if tunnel { inbound.write_all(b"HTTP/1.1 200 Connection Established\r\n\r\n").await?; }
    else {
        let mut request = format!("{method} {} HTTP/1.1\r\n", path.unwrap());
        for line in lines.filter(|line| !line.is_empty() && !line.to_ascii_lowercase().starts_with("proxy-")) {
            request.push_str(line); request.push_str("\r\n");
        }
        request.push_str("\r\n");
        outbound.write_all(request.as_bytes()).await?;
    }
    tokio::io::copy_bidirectional(&mut inbound, &mut outbound).await?;
    Ok(())
}
fn parse_connect_target(target: &str) -> Option<(String, u16)> {
    if let Ok(addr) = target.parse::<SocketAddr>() { return Some((addr.ip().to_string(), addr.port())); }
    let (host, port) = target.rsplit_once(':')?;
    let host = host.trim_matches(['[', ']']);
    if host.is_empty() || host.chars().any(char::is_whitespace) { return None; }
    Some((host.into(), port.parse().ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn proxy_closes_listener_and_live_tunnels_with_its_endpoint() {
        let echo = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let proxy = start_system_connect_proxy().await.unwrap();
        let proxy_addr = (Ipv4Addr::LOCALHOST, proxy.url().unwrap().port().unwrap());
        let mut client = TcpStream::connect(proxy_addr).await.unwrap();
        client.write_all(format!("CONNECT {} HTTP/1.1\r\n\r\n", echo.local_addr().unwrap()).as_bytes()).await.unwrap();
        let (mut server, _) = echo.accept().await.unwrap();
        let mut headers = [0; 39];
        client.read_exact(&mut headers).await.unwrap();
        assert_eq!(&headers, b"HTTP/1.1 200 Connection Established\r\n\r\n");
        client.write_all(b"ping").await.unwrap();
        let mut bytes = [0; 4]; server.read_exact(&mut bytes).await.unwrap(); assert_eq!(&bytes,b"ping");
        proxy.shutdown().await;
        assert!(TcpStream::connect(proxy_addr).await.is_err());
        assert_eq!(tokio::time::timeout(Duration::from_secs(2), server.read(&mut bytes)).await.unwrap().unwrap(),0);
    }
}
