//! A private loopback ingress accepting exactly one authenticated peer and one
//! bounded request per connection. The eBPF bridge supplies the prelude before
//! forwarding any caller bytes. This is not a general Gateway HTTP listener.
use std::{future::Future, net::SocketAddr, sync::Arc, time::Duration};

use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
    sync::Semaphore,
};

use crate::MAX_FRAME;

pub const SOURCE: &str = include_str!("../socket/mesh.c");

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Peer {
    pub origin: String,
    pub device_key: String,
}

// No Debug or Serialize: this credential is local and must never be exported.
pub struct BridgeAuth {
    token: String,
}

impl BridgeAuth {
    pub fn new(token: String) -> Result<Self> {
        ensure!(
            token.len() == 64 && token.bytes().all(|b| b.is_ascii_hexdigit()),
            "invalid bridge credential"
        );
        Ok(Self { token })
    }
}

pub fn source_for_port(port: u16) -> Result<String> {
    ensure!(port != 0, "bridge needs a bound port");
    Ok(SOURCE.replace("ZORK_MESH_PORT", &port.to_string()))
}

async fn line<R: AsyncRead + Unpin>(stream: &mut R, max: usize) -> Result<String> {
    let mut bytes = Vec::new();
    for _ in 0..=max {
        let byte = stream.read_u8().await?;
        if byte == b'\n' {
            return String::from_utf8(bytes).context("invalid bridge prelude");
        }
        bytes.push(byte);
    }
    anyhow::bail!("bridge prelude too long")
}

fn equal_secret(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    a.iter().zip(b).fold(0_u8, |diff, (a, b)| diff | (a ^ b)) == 0
}

pub fn key_origin(hex: &str) -> Result<String> {
    ensure!(
        hex.len() == 64 && hex.bytes().all(|b| b.is_ascii_hexdigit()),
        "invalid device key"
    );
    const ALPHABET: &[u8] = b"ybndrfg8ejkmcpqxot1uwisza345h769";
    let mut encoded = String::from("key:");
    let mut acc: u32 = 0;
    let mut bits = 0;
    for index in (0..64).step_by(2) {
        let byte = u8::from_str_radix(&hex[index..index + 2], 16)?;
        acc = (acc << 8) | byte as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            encoded.push(ALPHABET[((acc >> bits) & 31) as usize] as char);
        }
        acc &= (1 << bits) - 1;
    }
    if bits > 0 {
        encoded.push(ALPHABET[((acc << (5 - bits)) & 31) as usize] as char);
    }
    Ok(encoded)
}

async fn authenticate<R: AsyncRead + Unpin>(stream: &mut R, auth: &BridgeAuth) -> Result<Peer> {
    ensure!(
        line(stream, 9).await? == "ZORKMESH1",
        "invalid bridge protocol"
    );
    let token = line(stream, 64).await?;
    ensure!(
        equal_secret(token.as_bytes(), auth.token.as_bytes()),
        "bridge authentication failed"
    );
    let origin = line(stream, 256).await?;
    let device_key = line(stream, 64).await?;
    // Personal mesh initially uses self-certifying key origins. Domain enrollment
    // and named-origin rotation require a separate product membership policy.
    ensure!(
        key_origin(&device_key)? == origin,
        "bridge identity mismatch"
    );
    Ok(Peer { origin, device_key })
}

pub async fn read_frame<R: AsyncRead + Unpin>(stream: &mut R) -> Result<Value> {
    let size = stream.read_u32().await? as usize;
    ensure!(size > 0 && size <= MAX_FRAME, "invalid mesh frame size");
    let mut bytes = vec![0; size];
    stream.read_exact(&mut bytes).await?;
    Ok(serde_json::from_slice(&bytes)?)
}

pub async fn write_frame<W: AsyncWrite + Unpin>(stream: &mut W, value: &Value) -> Result<()> {
    let bytes = serde_json::to_vec(value)?;
    ensure!(bytes.len() <= MAX_FRAME, "mesh response too large");
    stream.write_u32(bytes.len() as u32).await?;
    stream.write_all(&bytes).await?;
    stream.shutdown().await?;
    Ok(())
}

pub async fn serve<F, Fut>(listener: TcpListener, auth: Arc<BridgeAuth>, handler: F) -> Result<()>
where
    F: Fn(Peer, Value) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Value>> + Send + 'static,
{
    let SocketAddr::V4(addr) = listener.local_addr()? else {
        anyhow::bail!("bridge requires IPv4 loopback");
    };
    ensure!(addr.ip().is_loopback(), "mesh ingress must bind loopback");
    let slots = Arc::new(Semaphore::new(32));
    loop {
        let (mut stream, _) = listener.accept().await?;
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let auth = auth.clone();
        let handler = handler.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let result = tokio::time::timeout(Duration::from_secs(25), async {
                let peer = authenticate(&mut stream, &auth).await?;
                let request = read_frame(&mut stream).await?;
                let response = handler(peer, request).await?;
                write_frame(&mut stream, &response).await
            })
            .await;
            if !matches!(result, Ok(Ok(()))) {
                // Do not log caller payloads or prelude errors carrying secrets.
                tracing::debug!("mesh ingress rejected or interrupted a request");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[tokio::test]
    async fn prelude_rejects_forged_identity_and_wrong_secret() {
        let auth = BridgeAuth::new("a".repeat(64)).unwrap();
        let hex = "00".repeat(32);
        let origin = key_origin(&hex).unwrap();
        for (token, name, success) in [
            ("a".repeat(64), origin, true),
            ("b".repeat(64), "key:forged".into(), false),
            ("a".repeat(64), "key:forged".into(), false),
        ] {
            let bytes = format!("ZORKMESH1\n{token}\n{name}\n{hex}\n");
            assert_eq!(
                authenticate(&mut bytes.as_bytes(), &auth).await.is_ok(),
                success
            );
        }
    }
    #[tokio::test]
    async fn frame_rejects_oversize_before_reading_body() {
        let bytes = ((MAX_FRAME + 1) as u32).to_be_bytes();
        assert!(read_frame(&mut bytes.as_slice()).await.is_err());
    }
}

pub enum Reply {
    Once(Value),
    Tunnel {
        upstream: tokio::net::TcpStream,
        cancelled: tokio::sync::watch::Receiver<bool>,
        /// Optional capacity/lifetime lease, held until forwarding finishes.
        guard: Option<Box<dyn Send>>,
    },
    Subscription(tokio::sync::mpsc::Receiver<Value>),
}

/// Subscription connections stay open until cancellation or transport failure.
/// Only authentication and initial request handling have a request deadline.
pub async fn serve_subscriptions<F, Fut>(
    listener: TcpListener,
    auth: Arc<BridgeAuth>,
    handler: F,
) -> Result<()>
where
    F: Fn(Peer, Value) -> Fut + Clone + Send + Sync + 'static,
    Fut: Future<Output = Result<Reply>> + Send + 'static,
{
    let SocketAddr::V4(addr) = listener.local_addr()? else {
        anyhow::bail!("bridge requires IPv4 loopback")
    };
    ensure!(addr.ip().is_loopback(), "mesh ingress must bind loopback");
    let slots = Arc::new(Semaphore::new(128));
    loop {
        let (mut stream, _) = listener.accept().await?;
        let Ok(permit) = slots.clone().try_acquire_owned() else {
            continue;
        };
        let auth = auth.clone();
        let handler = handler.clone();
        tokio::spawn(async move {
            let _permit = permit;
            let reply = tokio::time::timeout(Duration::from_secs(25), async {
                let peer = authenticate(&mut stream, &auth).await?;
                let request = read_frame(&mut stream).await?;
                handler(peer, request).await
            })
            .await;
            match reply {
                Ok(Ok(Reply::Tunnel {
                    mut upstream,
                    mut cancelled,
                    guard,
                })) => {
                    let _guard = guard;
                    let bytes = br#"{"v":1,"ok":true}"#;
                    if stream.write_u32(bytes.len() as u32).await.is_ok()
                        && stream.write_all(bytes).await.is_ok()
                        && !*cancelled.borrow()
                    {
                        tokio::select! {
                            _ = tokio::io::copy_bidirectional(&mut stream, &mut upstream) => {},
                            _ = cancelled.changed() => {},
                        }
                    }
                }
                Ok(Ok(Reply::Once(value))) => {
                    let _ = write_frame(&mut stream, &value).await;
                }
                Ok(Ok(Reply::Subscription(mut rx))) => {
                    let (mut read, mut write) = stream.into_split();
                    let mut closed = [0];
                    loop {
                        let frame = tokio::select! {
                            _=read.read(&mut closed)=>break,
                            frame=rx.recv()=>match frame{Some(v)=>v,None=>break},
                        };
                        let Ok(bytes) = serde_json::to_vec(&frame) else {
                            break;
                        };
                        if bytes.len() > MAX_FRAME {
                            break;
                        }
                        let sent = tokio::time::timeout(Duration::from_secs(25), async {
                            write.write_u32(bytes.len() as u32).await?;
                            write.write_all(&bytes).await
                        })
                        .await;
                        if !matches!(sent, Ok(Ok(()))) {
                            break;
                        }
                    }
                }
                _ => tracing::debug!("mesh ingress rejected or interrupted a request"),
            }
        });
    }
}
