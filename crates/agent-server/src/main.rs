use std::{
    future::{Future, IntoFuture},
    io::Write,
    sync::Arc,
    time::Duration,
};

use axum::{routing::get, Json, Router};
use serde_json::json;
use zork_agent::{session::tools::ToolRegistry, AgentOptions, AgentRuntime};
use zork_agent_http as http;

const ENDPOINT_DRAIN_TIMEOUT: Duration = Duration::from_secs(1);

fn main() -> Result<(), Box<dyn std::error::Error>> {
    if std::env::args()
        .skip(1)
        .any(|arg| arg == "--help" || arg == "-h")
    {
        println!("Usage: zork-agent [--data DIR] [--agent-token TOKEN] [--no-streaming]");
        return Ok(());
    }
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?),
        )
        .init();

    let args = zork_config::parse_process_args()?;
    let mut file = zork_config::ensure_layout(&args.data_root)?;
    if let Some(host) = &args.listen_host {
        zork_config::apply_listen(&mut file, host);
    }
    let listen = if file.bind.agent.trim().is_empty() {
        "127.0.0.1:3010".to_owned()
    } else {
        file.bind.agent.clone()
    };
    let listen_addr = listen
        .parse()
        .map_err(|_| format!("invalid agent listen address {listen}"))?;
    let agent_token = args.agent_token.clone();

    let tokio_runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    tokio_runtime.block_on(run(
        listen_addr,
        file,
        args.data_root,
        args.fake_agent,
        args.no_streaming,
        agent_token,
    ))
}

async fn run(
    listen_addr: std::net::SocketAddr,
    file: zork_config::FileConfig,
    data_root: std::path::PathBuf,
    fake_agent: bool,
    no_streaming: bool,
    agent_token: Option<String>,
) -> Result<(), Box<dyn std::error::Error>> {
    let listener = tokio::net::TcpListener::bind(listen_addr).await?;
    let shutdown_signal = arm_shutdown_signal()?;
    let tools = Arc::new(ToolRegistry::default());
    zork_agent_station_tools::register(&tools, zork_config::loopback_base_url(&file.bind.runtime))?;
    let mut runtime = AgentRuntime::start(AgentOptions {
        data_root: data_root.clone(),
        fake_agent,
        no_streaming,
        context: file.context.clone(),
        environment: zork_agent_station_tools::environment(
            &data_root,
            &zork_config::loopback_base_url(&file.bind.runtime),
        )?,
        tools,
        ..AgentOptions::default()
    })?;
    let result = serve(
        listener,
        &data_root,
        agent_token,
        &mut runtime,
        shutdown_signal,
    )
    .await;
    runtime.shutdown().await;
    zork_config::clear_ready_pid(&data_root, "zork-agent");
    result
}

async fn serve(
    listener: tokio::net::TcpListener,
    data_root: &std::path::Path,
    agent_token: Option<String>,
    runtime: &mut AgentRuntime,
    shutdown_signal: impl Future<Output = ()>,
) -> Result<(), Box<dyn std::error::Error>> {
    let state = http::AppState {
        agent: runtime.agent().clone(),
        token: agent_token,
    };
    let router = Router::new()
        .route("/readyz", get(readyz))
        .merge(http::router(state));

    println!("zork-agent ready http://{}", listener.local_addr()?);
    std::io::stdout().flush()?;
    zork_config::write_ready_pid(&data_root, "zork-agent")?;

    tokio::pin!(shutdown_signal);
    let (shutdown, shutdown_requested) = tokio::sync::oneshot::channel::<()>();
    let serving = axum::serve(listener, router)
        .with_graceful_shutdown(async {
            let _ = shutdown_requested.await;
        })
        .into_future();
    tokio::pin!(serving);
    let result = tokio::select! {
        result = &mut serving => result,
        () = &mut shutdown_signal => {
            let _ = shutdown.send(());
            runtime.shutdown().await;
            match tokio::time::timeout(ENDPOINT_DRAIN_TIMEOUT, &mut serving).await {
                Ok(result) => result,
                Err(_) => Ok(()),
            }
        }
    };
    result?;
    Ok(())
}

async fn readyz() -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "pid": std::process::id(),
        "service": "zork-agent",
    }))
}

#[cfg(unix)]
fn arm_shutdown_signal() -> Result<impl Future<Output = ()>, std::io::Error> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    Ok(async move {
        tokio::select! {
            _ = terminate.recv() => {}
            _ = interrupt.recv() => {}
        }
    })
}

#[cfg(not(unix))]
fn arm_shutdown_signal() -> Result<impl Future<Output = ()>, std::io::Error> {
    Ok(async {
        let _ = tokio::signal::ctrl_c().await;
    })
}
