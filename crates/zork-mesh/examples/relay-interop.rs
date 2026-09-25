//! Interop check for a relay server with real iroh endpoints and no IP transports.
//!
//! `relay-interop vector` prints the BLAKE3 derive_key output iroh signs for a fixed
//! 16-byte challenge (0..16), a public test vector for other relay implementations.
//! `relay-interop check RELAY_URL` binds fresh in-memory identities with direct UDP
//! disabled, so every byte must pass through the relay, and verifies:
//! 1. both endpoints authenticate to the relay (home relay online);
//! 2. a QUIC connection with bulk and many small round trips;
//! 3. a second connection for an already connected EndpointId takes over (newest
//!    wins), and closing it hands delivery back to the older connection.
//!
//! `relay-interop cert DIR` writes a throwaway self-signed certificate for 127.0.0.1;
//! with RELAY_INTEROP_CERT=DIR/cert.der the check trusts it, so a local https relay
//! (`wrangler dev --local-protocol https`) sees what production sees behind TLS:
//! clients add the unverifiable `x-iroh-relay-client-auth-v1` header.
//!
//! Endpoint keys are generated in memory and never printed.
use anyhow::{ensure, Context, Result};
use iroh::{
    endpoint::presets, tls::CaTlsConfig, Endpoint, EndpointAddr, RelayMode, RelayUrl, SecretKey,
    Watcher,
};
use std::time::Duration;

const ALPN: &[u8] = b"zork/relay-interop/1";
const T: Duration = Duration::from_secs(30);

/// Trusts the throwaway certificate from `relay-interop cert DIR` when
/// RELAY_INTEROP_CERT names its DER file (an https relay served by `wrangler dev`).
fn tls() -> Result<Option<CaTlsConfig>> {
    let Some(path) = std::env::var_os("RELAY_INTEROP_CERT") else {
        return Ok(None);
    };
    let der = std::fs::read(path)?;
    Ok(Some(CaTlsConfig::custom_roots([
        rustls::pki_types::CertificateDer::from(der),
    ])))
}

async fn endpoint(key: SecretKey, relay: &RelayUrl) -> Result<Endpoint> {
    let mut builder = Endpoint::builder(presets::N0);
    if let Some(tls) = tls()? {
        builder = builder.ca_tls_config(tls);
    }
    let endpoint = builder
        .secret_key(key)
        .alpns(vec![ALPN.to_vec()])
        .clear_ip_transports()
        .clear_address_lookup()
        .relay_mode(RelayMode::Custom([relay.clone()].into_iter().collect()))
        .bind()
        .await?;
    if tokio::time::timeout(T, endpoint.online()).await.is_err() {
        eprintln!("net report: {:?}", endpoint.net_report().get());
        eprintln!("home relay: {:?}", endpoint.home_relay_status().get());
        anyhow::bail!("relay handshake / home relay did not come online");
    }
    Ok(endpoint)
}

/// Echo server: every bi-stream gets back the BLAKE3 digest of what was sent.
fn serve(endpoint: Endpoint, label: &'static str) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        while let Some(incoming) = endpoint.accept().await {
            tokio::spawn(async move {
                let Ok(conn) = incoming.await else { return };
                while let Ok((mut send, mut recv)) = conn.accept_bi().await {
                    let Ok(bytes) = recv.read_to_end(8 * 1024 * 1024).await else {
                        return;
                    };
                    let mut reply = label.as_bytes().to_vec();
                    reply.extend_from_slice(blake3::hash(&bytes).as_bytes());
                    if send.write_all(&reply).await.is_err() || send.finish().is_err() {
                        return;
                    }
                }
            });
        }
    })
}

async fn round_trip(conn: &iroh::endpoint::Connection, bytes: &[u8]) -> Result<String> {
    let (mut send, mut recv) = conn.open_bi().await?;
    send.write_all(bytes).await?;
    send.finish()?;
    let reply = recv.read_to_end(64).await?;
    ensure!(reply.len() > 32, "short reply");
    let (label, digest) = reply.split_at(reply.len() - 32);
    ensure!(
        digest == blake3::hash(bytes).as_bytes(),
        "payload digest mismatch"
    );
    Ok(String::from_utf8_lossy(label).into_owned())
}

async fn connect_and_check(
    a: &Endpoint,
    to: EndpointAddr,
    bulk: usize,
    small: usize,
) -> Result<String> {
    let conn = tokio::time::timeout(T, a.connect(to, ALPN))
        .await
        .context("connect timed out")??;
    let payload: Vec<u8> = (0..bulk).map(|i| (i % 251) as u8).collect();
    let label = tokio::time::timeout(T * 2, round_trip(&conn, &payload))
        .await
        .context("bulk round trip timed out")??;
    for i in 0..small {
        let got = tokio::time::timeout(T, round_trip(&conn, format!("ping {i}").as_bytes()))
            .await
            .context("small round trip timed out")??;
        ensure!(got == label, "reply switched servers mid-connection");
    }
    conn.close(0u32.into(), b"done");
    Ok(label)
}

async fn check(relay: RelayUrl) -> Result<()> {
    let a = endpoint(SecretKey::generate(), &relay).await?;
    let b_key = SecretKey::generate();
    let b = endpoint(b_key.clone(), &relay).await?;
    println!(
        "PASS: two endpoints authenticated to the relay (signed challenge), direct IP disabled"
    );
    let b_server = serve(b.clone(), "b1");
    // Only the relay URL is given: no IP addresses exist for either side.
    let to_b = EndpointAddr::new(b.id()).with_relay_url(relay.clone());
    let label = connect_and_check(&a, to_b.clone(), 1024 * 1024, 64).await?;
    ensure!(label == "b1");
    println!("PASS: QUIC over relay: 1 MiB bulk + 64 small round trips verified");

    // Same identity connects again: the relay must route to the newest connection.
    let b2 = endpoint(b_key, &relay).await?;
    let b2_server = serve(b2.clone(), "b2");
    tokio::time::sleep(Duration::from_millis(500)).await;
    let label = connect_and_check(&a, to_b.clone(), 64 * 1024, 8).await?;
    ensure!(
        label == "b2",
        "duplicate EndpointId: expected the newest connection, got {label}"
    );
    println!("PASS: duplicate EndpointId: newest connection receives traffic");

    // Closing the newest hands delivery back to the older, still-open connection.
    b2.close().await;
    b2_server.abort();
    tokio::time::sleep(Duration::from_secs(1)).await;
    let label = connect_and_check(&a, to_b, 64 * 1024, 8).await?;
    ensure!(
        label == "b1",
        "after the newest closed, expected the older connection, got {label}"
    );
    println!("PASS: duplicate EndpointId: older connection is promoted when the newest closes");

    b_server.abort();
    a.close().await;
    b.close().await;
    println!("PASS: relay interop");
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("vector") => {
            let challenge: Vec<u8> = (0u8..16).collect();
            let message =
                blake3::derive_key("iroh-relay handshake v1 challenge signature", &challenge);
            println!(
                "{}",
                message
                    .iter()
                    .map(|b| format!("{b:02x}"))
                    .collect::<String>()
            );
            Ok(())
        }
        Some("cert") => {
            // A throwaway self-signed certificate for 127.0.0.1 (local TLS only).
            let dir = std::path::Path::new(args.get(2).context("missing directory")?);
            let cert =
                rcgen::generate_simple_self_signed(vec!["127.0.0.1".into(), "localhost".into()])?;
            std::fs::write(dir.join("cert.pem"), cert.cert.pem())?;
            std::fs::write(dir.join("cert.der"), cert.cert.der())?;
            std::fs::write(dir.join("key.pem"), cert.signing_key.serialize_pem())?;
            Ok(())
        }
        Some("check") => check(args.get(2).context("missing relay URL")?.parse()?).await,
        _ => anyhow::bail!("usage: relay-interop vector | cert DIR | check RELAY_URL"),
    }
}
