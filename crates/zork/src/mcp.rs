//! Node-local MCP administration. Secrets are resolved by the serving Station.
use anyhow::{ensure, Context, Result};
use reqwest::Method;
use serde_json::{json, Value};
use std::time::Duration;

pub async fn run(mut argv: Vec<String>) -> Result<()> {
    let op = if argv.first().is_some_and(|v| !v.starts_with('-')) {
        argv.remove(0)
    } else {
        "list".into()
    };
    let count = match op.as_str() {
        "list" => 0,
        "add" | "get" | "probe" | "remove" | "enable" | "disable" => 1,
        "update" => 2,
        _ => anyhow::bail!("Unknown MCP operation {op}"),
    };
    ensure!(
        argv.len() >= count,
        "Missing MCP operation arguments; use zork --help"
    );
    let positional = argv.drain(..count).collect::<Vec<_>>();
    let args = zork_config::parse_process_args_from(argv)?;
    let config = zork_config::load_config(&args.data_root)?;
    let base = zork_config::loopback_base_url(&config.bind.control);
    let http = reqwest::Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(20))
        .build()?;
    let request = |method: Method, path: String, body: Option<Value>| {
        let mut r = http
            .request(method, format!("{base}/admin/api/mcp{path}"))
            .bearer_auth(&config.admin.token);
        if let Some(body) = body {
            r = r.json(&body);
        }
        async move {
            let response = r
                .send()
                .await
                .context("Cannot reach the configured Station MCP administration API")?;
            let status = response.status();
            let value: Value = response.json().await?;
            ensure!(
                status.is_success(),
                "{}",
                value["error"]
                    .as_str()
                    .unwrap_or("MCP administration failed")
            );
            Ok::<Value, anyhow::Error>(value)
        }
    };
    let result = if op == "list" {
        request(Method::GET, "".into(), None).await?
    } else if op == "add" {
        let input: Value = serde_json::from_slice(&std::fs::read(&positional[0])?)?;
        request(Method::POST, "".into(), Some(input)).await?
    } else {
        let id = &positional[0];
        ensure!(
            id.len() == 26
                && id
                    .bytes()
                    .all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()),
            "Expected a complete MCP service ULID"
        );
        let path = format!("/{id}");
        match op.as_str() {
            "get" => request(Method::GET, path, None).await?,
            "probe" => request(Method::POST, format!("{path}/probe"), Some(json!({}))).await?,
            _ => {
                let mut current = request(Method::GET, path.clone(), None).await?;
                let revision = current["revision"]
                    .as_str()
                    .context("Missing configuration revision")?
                    .to_owned();
                if op == "remove" {
                    request(
                        Method::DELETE,
                        path,
                        Some(json!({"expected_revision":revision})),
                    )
                    .await?
                } else {
                    if op == "update" {
                        current = serde_json::from_slice(&std::fs::read(&positional[1])?)?;
                    } else {
                        let object = current
                            .as_object_mut()
                            .context("Invalid MCP configuration")?;
                        object.remove("id");
                        object.remove("revision");
                        object.insert("enabled".into(), json!(op == "enable"));
                    }
                    request(
                        Method::PUT,
                        path,
                        Some(json!({"expected_revision":revision,"config":current})),
                    )
                    .await?
                }
            }
        }
    };
    println!("{}", serde_json::to_string_pretty(&result)?);
    Ok(())
}
