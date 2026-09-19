//! A real loopback relay for isolated product tests. EOF stops all relay tasks.
use anyhow::{Context, Result};
use std::{io::Write, net::Ipv4Addr};
use tokio::io::AsyncReadExt;
#[tokio::main]
async fn main() -> Result<()> {
    let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    let mut config = iroh_relay::server::ServerConfig::default();
    config.relay = Some(iroh_relay::server::RelayConfig::new((
        Ipv4Addr::LOCALHOST,
        0,
    )));
    let server = iroh_relay::server::Server::spawn(config).await?;
    println!(
        "{}",
        serde_json::json!({"port":server.http_addr().context("relay listener")?.port()})
    );
    std::io::stdout().flush()?;
    let mut stop = [0u8; 1];
    let _ = tokio::io::stdin().read(&mut stop).await?;
    server.shutdown().await?;
    Ok(())
}
