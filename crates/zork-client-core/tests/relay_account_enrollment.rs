//! Invitation bootstrap and approval must work without an account.
use anyhow::{ensure, Context, Result};
use serde_json::{json, Value};
use std::time::Duration;
use zork_client_core::{subscriptions::Key, Client, Command};

async fn command(client: &mut Client, value: Value) -> Result<Value> {
    // Match the production JNI boundary: keep the command future off the caller stack.
    Box::pin(client.execute(serde_json::from_value::<Command>(value)?)).await
}
async fn admin(method: reqwest::Method, path: &str, body: Option<Value>) -> Result<Value> {
    let mut request = reqwest::Client::new()
        .request(
            method,
            format!("{}{}", std::env::var("ZORK_ENROLLMENT_URL")?, path),
        )
        .bearer_auth(std::env::var("ZORK_ENROLLMENT_TOKEN")?);
    if let Some(body) = body {
        request = request.json(&body);
    }
    let response = request.send().await?;
    ensure!(
        response.status().is_success(),
        "isolated Station request failed"
    );
    Ok(response.json().await?)
}
async fn wait(client: &Client, phase: &str) -> Result<Value> {
    let mut wire = client.local().observe(Key::Invitation)?;
    tokio::time::timeout(Duration::from_secs(30), async {
        loop {
            if let Some(frame) = wire.prepare()? {
                let value = frame["snapshot"].clone();
                wire.finish(frame["batch"].as_u64().unwrap(), true);
                ensure!(
                    value["invitation"]["status"] != "login_required",
                    "invitation requested Google login"
                );
                if value["invitation"]["status"] == phase
                    || (phase == "joined" && value["joined_peer"].is_string())
                {
                    return Ok(value);
                }
            }
            wire.signals().changed().await?;
        }
    })
    .await
    .context("invitation did not advance")?
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "isolated Station supplied by the account delivery harness"]
async fn short_invitation_joins_and_recovers_without_google() -> Result<()> {
    Box::pin(run()).await
}
async fn run() -> Result<()> {
    let offline = std::env::var("ZORK_ENROLLMENT_PUBLIC").as_deref() != Ok("1");
    let root = tempfile::tempdir()?;
    let mut client = Client::open(root.path())?;
    let invite = admin(reqwest::Method::POST, "/v1/node/mesh/client-invites", None).await?;
    let ticket = invite["invitation"].as_str().context("short ticket")?;
    ensure!(
        zork_mesh::enrollment::ticket::Ticket::decode(ticket)?
            .network_config()?
            .offline
            == offline,
        "fixture network mode mismatch"
    );
    command(
        &mut client,
        json!({"op":"begin_invitation","ticket":ticket,"name":"Account regression phone"}),
    )
    .await?;
    let waiting = wait(&client, "awaiting_approval").await?;
    ensure!(
        waiting["nodes"]
            .as_array()
            .is_some_and(|nodes| nodes.is_empty()),
        "unapproved invitation granted access"
    );
    let identity = waiting["identity"].clone();
    let claims = admin(reqwest::Method::GET, "/v1/node/mesh/invites", None).await?;
    let claim = claims["items"]
        .as_array()
        .context("claims")?
        .iter()
        .find(|claim| claim["id"] == invite["id"])
        .context("claim")?;
    admin(
        reqwest::Method::POST,
        &format!(
            "/v1/node/mesh/invites/{}/approve",
            invite["id"].as_str().unwrap()
        ),
        Some(json!({"origin":identity,"claim_id":claim["claim_id"]})),
    )
    .await?;
    let joined = wait(&client, "joined").await?;
    let peer = joined["joined_peer"]
        .as_str()
        .context("joined peer")?
        .to_owned();
    ensure!(
        joined["network"]["direct_only"] == offline,
        "network choice was lost"
    );
    ensure!(
        zork_config::relay_account::load(root.path())?.is_none(),
        "Mesh join issued an account credential"
    );
    command(
        &mut client,
        json!({"op":"read","peer":peer,"path":"/v1/node/agents"}),
    )
    .await?;
    // A relay connection does not grant access to an unrelated device.
    let outsider_root = tempfile::tempdir()?;
    let config = zork_config::MeshConfig {
        enabled: true,
        offline,
        ..Default::default()
    };
    let (mut outsider, _) =
        zork_client_core::transport::start(outsider_root.path(), &config).await?;
    let outsider_node = outsider.node();
    outsider_node.trust(&peer, "expected Station", None).await?;
    let denied = outsider_node
        .exchange(&peer, &json!({"v":1,"request":{"kind":"client","method":"GET","path":"/v1/node/agents","body":null}}))
        .await;
    outsider.shutdown().await?;
    ensure!(
        denied?["error"] == "mesh_peer_not_paired",
        "relay admission granted an unpaired device business access"
    );
    println!("PASS: native Station rejects an unpaired device independently of relay admission");
    client.pause().await?;
    drop(client);
    let mut client = Client::open(root.path())?;
    let resumed = command(&mut client, json!({"op":"resume"})).await?;
    ensure!(
        resumed["identity"] == identity && resumed["network"]["direct_only"] == offline,
        "identity and network choice did not recover"
    );
    command(
        &mut client,
        json!({"op":"read","peer":peer,"path":"/v1/node/agents"}),
    )
    .await?;
    client.pause().await?;
    println!("PASS: real short invite, approval, Mesh read and restart recovery without Google credentials");
    Ok(())
}
