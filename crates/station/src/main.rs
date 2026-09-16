mod adb;
mod admin;
mod agent;
mod agent_configuration;
mod binshim;
mod browser;
mod business_cards;
mod channels;
mod config;
mod connections;
mod control_db;
mod control_github;
mod db;
mod delivery;
mod desktop_events;
mod enrollment;
mod files;
mod http;
mod im_entry;
mod inbound;
mod interaction_registry;
mod jobs;
mod mcp;
mod mesh;
mod message_log;
mod node;
mod node_access;
mod node_tools;
mod provider_login;
mod realtime;
mod shared_files;
mod shared_services;
mod slack;
mod slack_tools;
mod socket;
mod state;
mod status_projection;
mod timeline;
mod tool_stream;

use crate::{config::RuntimeConfig, db::GatewayDb, jobs::JobSupervisor, state::AppState};
use anyhow::Result;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tokio::sync::watch;
use tracing::info;
use zork_agent::{session::tools::ToolRegistry, AgentOptions, AgentRuntime};

fn main() -> Result<()> {
    zork_config::startup::mark("station.main");
    if std::env::args().nth(1).as_deref() == Some("--service-process") {
        let input = std::env::args()
            .nth(2)
            .ok_or_else(|| anyhow::anyhow!("missing service launch"))?;
        std::process::exit(shared_services::process::entry(&input)?);
    }
    let identity = zork_config::service::prepare_process_identity()?;
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run(identity))
}
async fn run(identity: zork_config::service::ProcessIdentity) -> Result<()> {
    zork_config::startup::mark("station.runtime_created");
    if std::env::args()
        .skip(1)
        .any(|arg| arg == "--help" || arg == "-h")
    {
        println!(
            "Usage: zork-station [--data DIR] [--listen HOST] [--agent-token TOKEN] [--no-streaming]"
        );
        return Ok(());
    }
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env().add_directive("info".parse()?),
        )
        .init();
    let shutdown = arm_shutdown_signal()?;
    let args = zork_config::parse_process_args()?;
    let mut config = RuntimeConfig::load()?;
    zork_config::startup::mark("station.config_loaded");
    for path in [
        &config.state_dir,
        &config.workspaces_root,
        &config.repos_root,
        &config.jobs_root,
        &config.log_dir,
    ] {
        std::fs::create_dir_all(path)?;
    }
    binshim::install(&mut config)?;
    zork_config::startup::mark("station.shims_installed");
    let file = zork_config::load_config(&config.data_root)?;
    let (db, prepared_agent) = std::thread::scope(|scope| -> Result<_> {
        let database = scope.spawn(|| -> Result<_> {
            zork_config::service::clear_signal_mask()?;
            GatewayDb::open_with_paths(
                &config.state_dir,
                &config.workspaces_root,
                &zork_config::files_root(&config.data_root),
                &config.data_root.join("chats"),
                &config.data_root.join("cache"),
            )
        });
        let preparation = scope.spawn(|| {
            zork_config::service::clear_signal_mask()?;
            AgentRuntime::prepare(&config.data_root)
        });
        // Launch Services stays on the main thread, overlapping only durable
        // storage IO. No session, tool or listener serves before all complete.
        let registered = identity.register();
        let db = database.join();
        let prepared = preparation.join();
        // Join both workers on every error path; their storage locks cannot
        // outlive startup or an unsuccessful process identity registration.
        registered?;
        let db = db.map_err(|_| anyhow::anyhow!("Station storage preparation interrupted"))??;
        let prepared = prepared
            .map_err(|_| anyhow::anyhow!("Agent storage preparation interrupted"))??;
        Ok((Arc::new(db), prepared))
    })?;
    zork_config::startup::mark("station.database_opened");
    let http_client = reqwest::Client::builder().no_proxy().build()?;
    let connections = Arc::new(
        connections::ConnectionManager::load(config.data_root.clone(), http_client).await?,
    );
    zork_config::startup::mark("station.connections_loaded");
    let control_db = Arc::new(control_db::ControlDb::open(&config.state_dir)?);
    zork_config::startup::mark("station.control_database_opened");
    control_db.attach_realtime(&db.realtime);
    // Bind before recovery starts: recovered tools may immediately call back into Gateway.
    let mut listeners = vec![
        (
            http::bind_listener(config.bind_addr).await?,
            ListenerKind::Runtime,
        ),
        (
            http::bind_listener(zork_config::parse_bind(&admin_bind(&config)?)?).await?,
            ListenerKind::Admin,
        ),
        (
            tokio::net::TcpListener::bind(zork_config::parse_bind(&config.agent_bind)?).await?,
            ListenerKind::Agent,
        ),
    ];
    if let Some(bind) = gateway_bind(&config).filter(|bind| *bind != config.bind_addr.to_string()) {
        listeners.push((
            http::bind_listener(zork_config::parse_bind(&bind)?).await?,
            ListenerKind::Gateway,
        ));
    }
    zork_config::startup::mark("station.listeners_bound");
    let tools = Arc::new(ToolRegistry::default());
    zork_agent_gateway_tools::register(&tools, config.broker_http_base_url.clone())?;
    let mesh = Arc::new(std::sync::OnceLock::new());
    let files = Arc::new(files::Files::new(
        config.data_root.clone(),
        db.clone(),
        mesh.clone(),
    ));
    let mut runtime = prepared_agent.start(AgentOptions {
        files: Some(files.clone()),
        configure_tools: Some({
            let base = config.broker_http_base_url.clone();
            Arc::new(move |registry| zork_agent_gateway_tools::extend_shell(registry, &base))
        }),
        skill_catalog: Some({
            let files = files.clone();
            Arc::new(move |session| files.catalog(session))
        }),
        service: zork_agent::session::service::ServiceOptions {
            runner: zork_agent::session::runner::RunnerOptions {
                configuration_source: Some({
                    let db = db.clone();
                    Arc::new(move |session| db.agent_configuration(session))
                }),
                selection_source: Some({
                    let db = db.clone();
                    Arc::new(move |session| db.selection_for_session(session))
                }),
                ..Default::default()
            },
            ..Default::default()
        },
        skill_sources: Some({
            let db = db.clone();
            let root = config.data_root.clone();
            Arc::new(move |session| {
                zork_config::load_config(&root)?
                    .skills
                    .sources(&root, &db.skill_paths_for_session(session)?)
            })
        }),
        data_root: config.data_root.clone(),
        fake_agent: args.fake_agent,
        no_streaming: args.no_streaming,
        context: file.context.clone(),
        tools,
        environment: {
            let mut environment = zork_agent_gateway_tools::environment(
                &config.data_root,
                &config.broker_http_base_url,
            )?;
            environment.insert(
                "SKILLS_ROOT".into(),
                zork_config::skill_bundles::skills_root(&config.data_root)
                    .to_string_lossy()
                    .into_owned(),
            );
            environment.insert(
                "REPOS_ROOT".into(),
                zork_config::files_root(&config.data_root)
                    .join("repos")
                    .to_string_lossy()
                    .into_owned(),
            );
            environment.insert(
                "SHARED_FILES_ROOT".into(),
                zork_config::shared_files_root(&config.data_root)
                    .to_string_lossy()
                    .into_owned(),
            );
            environment
        },
        ..Default::default()
    })?;
    zork_config::startup::mark("station.agent_started");
    let agent = runtime.agent().clone();
    let entries = im_entry::ImEntryGateway::new(db.clone(), connections.clone());
    let state = AppState {
        files,
        provider_auth: Arc::new(node::auth::Hub::new()?),
        login_cards: Default::default(),
        interaction_handlers: Arc::new(business_cards::lifecycle_handlers()?),
        node_tools: Arc::new(node_tools::Hub::open(&config.state_dir)?),
        mcp: Arc::new(mcp::Hub::open(&config.state_dir)?),
        browser: Arc::new(browser::Hub::open(&config.data_root)?),
        agent: agent.clone(),
        draining: Arc::new(AtomicBool::new(true)),
        config: config.clone(),
        db: db.clone(),
        connections,
        status_projection: status_projection::AgentStatusProjector::new(
            agent.clone(),
            entries.clone(),
        ),
        entries,
        jobs: Arc::new(JobSupervisor::new(db.clone(), config.clone(), agent)),
        mesh,
        admin: state::AdminPlane {
            db: control_db,
            admin_token: (!file.admin.token.trim().is_empty())
                .then(|| file.admin.token.trim().to_owned()),
            started_at: config.started_at.clone(),
            reload_sock: zork_config::zork_sock_path(&config.data_root),
        },
    };
    zork_config::startup::mark("station.hubs_created");
    let mut profile_changes = state.agent.profiles.subscribe();
    let profile_realtime = state.db.realtime.clone();
    let _profile_events = zork_notify::Task(tokio::spawn(async move {
        while profile_changes.changed().await.is_ok() {
            profile_realtime.notify(realtime::PROFILES);
        }
    }));
    let _config_watch = realtime::watch_config(config.data_root.clone(), db.realtime.clone())?;
    let channel_delivery = channels::start(state.clone())?;
    let user_interaction_delivery = interaction_registry::start(state.clone())?;
    let business_card_delivery = business_cards::start(state.clone())?;
    zork_config::startup::mark("station.delivery_started");
    let (stop_mesh, mesh_stopped) = watch::channel(false);
    let mut mesh_tasks = tokio::task::JoinSet::new();
    mesh_tasks.spawn(run_mesh(state.clone(), mesh_stopped));
    let result = serve(&state, &mut runtime, listeners, shutdown, &mut mesh_tasks).await;
    drop(channel_delivery);
    drop(user_interaction_delivery);
    drop(business_card_delivery);
    state.mcp.shutdown();
    state.node_tools.shutdown().await;
    let mesh_closing = std::time::Instant::now();
    state.draining.store(true, Ordering::Release);
    runtime.shutdown().await;
    state.status_projection.shutdown().await;
    zork_config::clear_ready_pid(&config.data_root, "zork-station");
    let _ = stop_mesh.send(true);
    while let Some(stopped) = mesh_tasks.join_next().await {
        stopped??;
    }
    info!(
        elapsed_ms = mesh_closing.elapsed().as_millis() as u64,
        "Mesh shutdown completed"
    );
    result
}

/// Restoring peer state must precede Mesh publication, but local Agent and
/// desktop APIs do not depend on it. Keep that work owned and monitored.
async fn run_mesh(state: AppState, mut stopped: watch::Receiver<bool>) -> Result<()> {
    let prepared = mesh::MeshService::prepare(&state.config.data_root).await?;
    let Some(prepared) = prepared else {
        let _ = stopped.wait_for(|stop| *stop).await;
        return Ok(());
    };
    let service = prepared.service.clone();
    if *stopped.borrow() {
        return service.shutdown().await;
    }
    state.db.sync_bind_mesh_owner(service.origin())?;
    state
        .mesh
        .set(service.clone())
        .map_err(|_| anyhow::anyhow!("Mesh already initialized"))?;
    mesh::start(prepared, state.clone());
    state
        .db
        .realtime
        .notify(crate::realtime::MESH | crate::realtime::WORK);
    zork_config::write_ready_pid(&state.config.data_root, "zork-mesh")?;
    let result = tokio::select! {
        _ = stopped.wait_for(|stop| *stop) => Ok(()),
        result = service.wait() => Err(result.err().unwrap_or_else(|| anyhow::anyhow!("Gateway Synch tasks stopped"))),
    };
    zork_config::clear_ready_pid(&state.config.data_root, "zork-mesh");
    service.shutdown().await?;
    result
}

enum ListenerKind {
    Runtime,
    Gateway,
    Admin,
    Agent,
}

async fn serve(
    state: &AppState,
    runtime: &mut AgentRuntime,
    listeners: Vec<(tokio::net::TcpListener, ListenerKind)>,
    shutdown: impl std::future::Future<Output = ()>,
    mesh_tasks: &mut tokio::task::JoinSet<Result<()>>,
) -> Result<()> {
    let (stop_http, http_stopped) = watch::channel(false);
    let mut servers = tokio::task::JoinSet::new();
    for (listener, kind) in listeners {
        let router = match kind {
            ListenerKind::Runtime => http::router(state.clone()),
            ListenerKind::Gateway => http::gateway_router(state.clone()),
            ListenerKind::Admin => admin::router(state.clone()),
            ListenerKind::Agent => axum::Router::new()
                .route(
                    "/readyz",
                    axum::routing::get(agent_readyz).with_state(state.draining.clone()),
                )
                .merge(zork_agent_http::router(zork_agent_http::AppState {
                    agent: state.agent.clone(),
                    token: state.config.agent_token.clone(),
                })),
        };
        zork_config::startup::mark(match kind {
            ListenerKind::Runtime => "station.runtime_router_built",
            ListenerKind::Gateway => "station.gateway_router_built",
            ListenerKind::Admin => "station.admin_router_built",
            ListenerKind::Agent => "station.agent_router_built",
        });
        let router = router.layer(axum::middleware::from_fn_with_state(
            http_stopped.clone(),
            http::close_event_streams_on_shutdown,
        ));
        let mut stopped = http_stopped.clone();
        servers.spawn(async move {
            axum::serve(listener, router)
                .with_graceful_shutdown(async move {
                    let _ = stopped.wait_for(|stop| *stop).await;
                })
                .await
        });
    }
    // Listeners remain alive while Agent drains, including tool callbacks and final status delivery.
    state.jobs.restore().await?;
    zork_config::startup::mark("station.jobs_restored");
    for session in state.db.list_sessions()? {
        if let Some(id) = session.id.as_deref() {
            state
                .status_projection
                .ensure(
                    &session.key,
                    id,
                    &session.connection_id,
                    &session.channel_id,
                    &session.root_thread_ts,
                )
                .await;
        }
    }
    let (stop_connections, connections_stopped) = watch::channel(false);
    zork_config::startup::mark("station.projections_restored");
    let socket_state = state.clone();
    let mut socket_task = tokio::spawn(socket::run_connections(socket_state, connections_stopped));
    zork_config::write_ready_pid(&state.config.data_root, "zork-station")?;
    state.draining.store(false, Ordering::Release);
    zork_config::startup::mark("station.ready");
    let mcp_state = state.clone();
    let mcp_maintenance = tokio::spawn(async move {
        loop {
            tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            mcp_state.mcp.reap_idle();
            if let Err(error) = mcp_state.db.sync_maintain() {
                tracing::warn!(%error, "sync retention maintenance failed");
            }
        }
    });
    info!(runtime = %state.config.bind_addr, agent = %state.config.agent_bind, "station ready with embedded Agent");
    let result = tokio::select! {
        () = shutdown => Ok(()),
        result = servers.join_next() => Err(anyhow::anyhow!("Station HTTP listener stopped: {result:?}")),
        _ = &mut socket_task => Err(anyhow::anyhow!("Station connections stopped")),
        result = mesh_tasks.join_next() => {
            Err(anyhow::anyhow!("Station Mesh task stopped: {result:?}"))
        }
    };
    state.draining.store(true, Ordering::Release);
    mcp_maintenance.abort();
    state.mcp.shutdown();
    state.node_tools.shutdown().await;
    let draining_started = std::time::Instant::now();
    zork_config::clear_ready_pid(&state.config.data_root, "zork-station");
    let _ = stop_connections.send(true);
    runtime.shutdown().await;
    info!(
        elapsed_ms = draining_started.elapsed().as_millis() as u64,
        "Agent shutdown completed"
    );
    state.status_projection.shutdown().await;
    let _ = stop_http.send(true);
    if tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while servers.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        tracing::warn!("HTTP shutdown grace expired");
        servers.abort_all();
        while servers.join_next().await.is_some() {}
    }
    if !socket_task.is_finished() {
        socket_task.abort();
        let _ = socket_task.await;
    }
    info!(
        elapsed_ms = draining_started.elapsed().as_millis() as u64,
        "HTTP shutdown completed"
    );
    result
}

async fn agent_readyz(
    axum::extract::State(draining): axum::extract::State<Arc<AtomicBool>>,
) -> impl axum::response::IntoResponse {
    let ready = !draining.load(Ordering::Acquire);
    (
        if ready {
            axum::http::StatusCode::OK
        } else {
            axum::http::StatusCode::SERVICE_UNAVAILABLE
        },
        axum::Json(
            serde_json::json!({"ok":ready,"service":"zork-agent","pid":std::process::id(),"embedded":true}),
        ),
    )
}

/// Optional Slack-facing listener. Test roots can omit it and use only the
/// broker API listener.
fn gateway_bind(config: &RuntimeConfig) -> Option<String> {
    let file = zork_config::load_config(&config.data_root).ok()?;
    let bind = file.bind.gateway.trim().to_string();
    if bind.is_empty() {
        None
    } else {
        Some(bind)
    }
}

/// Management API listener, separate from the broker API listener.
fn admin_bind(config: &RuntimeConfig) -> Result<String> {
    let bind = zork_config::load_config(&config.data_root)
        .ok()
        .map(|file| file.bind.control.trim().to_string())
        .unwrap_or_default();
    if bind.is_empty() || bind == config.bind_addr.to_string() {
        Ok("127.0.0.1:3001".to_string())
    } else {
        Ok(bind)
    }
}

#[cfg(unix)]
fn arm_shutdown_signal() -> std::io::Result<impl std::future::Future<Output = ()>> {
    let mut terminate = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
    let mut interrupt = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::interrupt())?;
    Ok(async move {
        tokio::select! { _ = terminate.recv() => {}, _ = interrupt.recv() => {} }
    })
}
#[cfg(not(unix))]
fn arm_shutdown_signal() -> std::io::Result<impl std::future::Future<Output = ()>> {
    Ok(async {
        let _ = tokio::signal::ctrl_c().await;
    })
}
