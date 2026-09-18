//! Run with the isolated Station from scripts/android/test_enrollment.py.
use serde_json::{json, Value};
use zork_client_core::{Client, Command};
use zork_mesh::enrollment::InviteKind;

async fn command(client: &mut Client, value: Value) -> anyhow::Result<Value> {
    client
        .execute(serde_json::from_value::<Command>(value)?)
        .await
}
async fn admin(method: reqwest::Method, path: &str, body: Option<Value>) -> anyhow::Result<Value> {
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
    let status = response.status();
    let value: Value = response.json().await?;
    anyhow::ensure!(status.is_success(), "{status}: {value}");
    Ok(value)
}
async fn create() -> Value {
    admin(reqwest::Method::POST, "/v1/node/mesh/client-invites", None)
        .await
        .unwrap()
}
async fn status(id: &Value) -> Value {
    admin(reqwest::Method::GET, "/v1/node/mesh/invites", None)
        .await
        .unwrap()["items"]
        .as_array()
        .unwrap()
        .iter()
        .find(|i| i["id"] == *id)
        .unwrap()
        .clone()
}
async fn approve(invite: &Value, claim: &Value) -> anyhow::Result<Value> {
    admin(
        reqwest::Method::POST,
        &format!(
            "/v1/node/mesh/invites/{}/approve",
            invite["id"].as_str().unwrap()
        ),
        Some(json!({"origin":claim["device"]["origin"],"claim_id":claim["claim_id"]})),
    )
    .await
}
async fn begin(client: &mut Client, invite: &Value) -> Value {
    command(
        client,
        json!({"op":"begin_invitation","ticket":invite["invitation"],"name":"Find N6 test"}),
    )
    .await
    .unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore = "requires isolated Station; use scripts/android/test_enrollment.py"]
async fn phone_requires_approval_and_survives_restart_without_node_privileges() {
    let dir = tempfile::tempdir().unwrap();
    let mut client = Client::open(dir.path()).unwrap();
    let invite = create().await;
    let ticket = zork_mesh::enrollment::ticket::resolve(
        &dir.path().join("test-resolve"),
        invite["invitation"].as_str().unwrap(),
        InviteKind::Client,
    )
    .await
    .unwrap();
    assert_eq!(ticket.kind, InviteKind::Client);
    assert_eq!(invite["scope"], "client");
    assert!(invite.get("command").is_none());
    begin(&mut client, &invite).await;
    let preclaim = status(&invite["id"]).await;
    assert!(
        approve(&invite, &preclaim).await.is_err(),
        "unverified claim must never be approved"
    );
    let waiting = command(&mut client, json!({"op":"poll_invitation"}))
        .await
        .unwrap();
    assert_eq!(waiting["invitation"]["status"], "awaiting_approval");
    assert!(waiting["nodes"].as_array().unwrap().is_empty());
    let identity = waiting["identity"].clone();
    let claim = status(&invite["id"]).await;
    assert_eq!(claim["status"], "awaiting_approval");
    assert_eq!(claim["device"]["origin"], identity);
    let config = admin(reqwest::Method::GET, "/v1/node/mesh", None)
        .await
        .unwrap();
    assert!(!config["config"]["peers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["origin"] == identity));
    let mut stale = claim.clone();
    stale["claim_id"] = json!("wrong");
    assert!(approve(&invite, &stale).await.is_err());
    // A second identity cannot steal the scanned invitation.
    let otherdir = tempfile::tempdir().unwrap();
    let mut other = Client::open(otherdir.path()).unwrap();
    begin(&mut other, &invite).await;
    assert!(command(&mut other, json!({"op":"poll_invitation"}))
        .await
        .is_err());
    other.pause().await.unwrap();
    // Process recreation keeps the same device and invitation, before approval.
    client.pause().await.unwrap();
    drop(client);
    let mut client = Client::open(dir.path()).unwrap();
    let resumed = command(&mut client, json!({"op":"resume"})).await.unwrap();
    assert_eq!(resumed["identity"], identity);
    approve(&invite, &claim).await.unwrap();
    let accepted = command(&mut client, json!({"op":"poll_invitation"}))
        .await
        .unwrap();
    assert!(accepted["invitation"].is_null());
    assert_eq!(accepted["joined_peer"], ticket.device.origin);
    let config = admin(reqwest::Method::GET, "/v1/node/mesh", None)
        .await
        .unwrap();
    let mesh = &config["config"];
    assert!(mesh["group"]["clients"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["origin"] == identity));
    assert!(!mesh["group"]["members"]
        .as_array()
        .unwrap()
        .iter()
        .any(|p| p["origin"] == identity));
    let grant = mesh["peers"]
        .as_array()
        .unwrap()
        .iter()
        .find(|p| p["origin"] == identity)
        .unwrap();
    assert_eq!(grant["client"], true);
    assert_eq!(grant["collaborate"], false);
    assert!(grant["execute"].as_array().unwrap().is_empty());
    command(
        &mut client,
        json!({"op":"read","peer":ticket.device.origin,"path":"/v1/node/agents"}),
    )
    .await
    .unwrap();
    client.pause().await.unwrap();
    drop(client);
    let mut client = Client::open(dir.path()).unwrap();
    command(&mut client, json!({"op":"resume"})).await.unwrap();
    command(
        &mut client,
        json!({"op":"read","peer":ticket.device.origin,"path":"/v1/node/agents"}),
    )
    .await
    .unwrap();
    // Same identity may recover the same receipt, without adding a duplicate grant.
    begin(&mut client, &invite).await;
    command(&mut client, json!({"op":"poll_invitation"}))
        .await
        .unwrap();
    admin(
        reqwest::Method::POST,
        "/v1/node/mesh/members/remove",
        Some(json!({"origin":identity})),
    )
    .await
    .unwrap();
    assert!(
        command(
            &mut client,
            json!({"op":"begin_invitation","ticket":invite["invitation"],"name":"Phone"})
        )
        .await
        .is_err(),
        "removed phone cannot reuse invitation"
    );
    command(&mut client, json!({"op":"cancel_invitation"}))
        .await
        .unwrap();
    client.pause().await.unwrap();
    // Explicit rejection and cancel leave no saved peers.
    let denied = create().await;
    let fresh = tempfile::tempdir().unwrap();
    let mut denied_client = Client::open(fresh.path()).unwrap();
    let mut legacy = denied.clone();
    let full = zork_mesh::enrollment::ticket::resolve(
        &fresh.path().join("legacy-resolve"),
        denied["invitation"].as_str().unwrap(),
        InviteKind::Client,
    )
    .await
    .unwrap();
    legacy["invitation"] = json!(full.encode().unwrap());
    begin(&mut denied_client, &legacy).await;
    command(&mut denied_client, json!({"op":"poll_invitation"}))
        .await
        .unwrap();
    admin(
        reqwest::Method::DELETE,
        &format!("/v1/node/mesh/invites/{}", denied["id"].as_str().unwrap()),
        None,
    )
    .await
    .unwrap();
    assert!(command(&mut denied_client, json!({"op":"poll_invitation"}))
        .await
        .is_err());
    let cancelled = command(&mut denied_client, json!({"op":"cancel_invitation"}))
        .await
        .unwrap();
    assert!(cancelled["invitation"].is_null());
    assert!(cancelled["nodes"].as_array().unwrap().is_empty());
    assert!(command(
        &mut denied_client,
        json!({"op":"begin_invitation","ticket":"https://example.com","name":"Phone"})
    )
    .await
    .is_err());
    let station_invite = admin(reqwest::Method::POST, "/v1/node/mesh/invites", None)
        .await
        .unwrap();
    assert!(command(
        &mut denied_client,
        json!({"op":"begin_invitation","ticket":station_invite["invitation"],"name":"Phone"})
    )
    .await
    .is_err());
    let mut expired = ticket.clone();
    expired.expires_at = 1;
    assert!(command(
        &mut denied_client,
        json!({"op":"begin_invitation","ticket":expired.encode().unwrap(),"name":"Phone"})
    )
    .await
    .is_err());
    denied_client.pause().await.unwrap();
}
