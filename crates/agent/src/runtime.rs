//! Embeddable Agent assembly. The host owns Tokio, logging, signals and transport.
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use crate::{
    application::Agent,
    provider::{AgentModelPort, FakeProvider, ProviderRouter},
    session::{
        ports::{
            ModelExecutor, SystemClock, SystemFileSystem, SystemIdGenerator, SystemProcessSpawner,
        },
        query::FileSessionQuery,
        service::{ServiceDependencies, ServiceOptions, SessionService},
        tools::{register_builtin_tools, BuiltinToolDependencies, ToolRegistry},
        StreamStore,
    },
    ProfileStore,
};

/// Host-supplied configuration; no CLI parsing, listeners or global environment mutation.
pub struct AgentOptions {
    /// Optional host resolver; receives a runtime session ID.
    pub skill_sources: Option<crate::skills::SkillSources>,
    pub skill_catalog: Option<crate::skills::SkillCatalogSource>,
    pub files: Option<Arc<dyn crate::session::ports::FileSystem>>,
    pub data_root: PathBuf,
    pub fake_agent: bool,
    pub no_streaming: bool,
    pub context: zork_config::ContextConfig,
    pub environment: BTreeMap<String, String>,
    /// Register host capabilities here before starting. Builtins are added at startup.
    pub tools: Arc<ToolRegistry>,
    pub configure_tools:
        Option<Arc<dyn Fn(&Arc<ToolRegistry>) -> anyhow::Result<()> + Send + Sync>>,
    pub service: ServiceOptions,
    /// None disables periodic status refresh. The first refresh runs immediately.
    pub profile_refresh_interval: Option<Duration>,
}

impl Default for AgentOptions {
    fn default() -> Self {
        Self {
            data_root: PathBuf::new(),
            skill_sources: None,
            skill_catalog: None,
            files: None,
            fake_agent: false,
            no_streaming: false,
            context: Default::default(),
            environment: BTreeMap::new(),
            tools: Arc::new(ToolRegistry::default()),
            configure_tools: None,
            service: ServiceOptions::default(),
            profile_refresh_interval: Some(Duration::from_secs(60)),
        }
    }
}

/// Owns all Agent background services. Call `shutdown().await` before stopping Tokio.
/// Drop only schedules best-effort cleanup; it cannot provide a graceful shutdown guarantee.
/// Agent handles and event streams must also be dropped to release the data-directory lock.
pub struct AgentRuntime {
    agent: Agent,
    refresh: Option<tokio::task::JoinHandle<()>>,
    profile_watch: Option<zork_notify::files::FileWatch>,
    stopped: bool,
}

/// Storage prepared on a host worker, before listeners and tool capabilities
/// are attached. Owns the writer lock; dropping it releases the preparation.
pub struct PreparedAgent {
    root: PathBuf,
    store: Arc<StreamStore>,
}

impl PreparedAgent {
    /// Activate the prepared data root inside the host's Tokio runtime.
    pub fn start(self, options: AgentOptions) -> anyhow::Result<AgentRuntime> {
        validate_options(&options)?;
        anyhow::ensure!(
            self.root == options.data_root,
            "prepared Agent data_root mismatch"
        );
        AgentRuntime::start_with_store(options, self.store)
    }
}

fn validate_options(options: &AgentOptions) -> anyhow::Result<()> {
    anyhow::ensure!(
        !options.data_root.as_os_str().is_empty(),
        "data_root is required"
    );
    anyhow::ensure!(
        options.profile_refresh_interval != Some(Duration::ZERO),
        "profile refresh interval must be positive"
    );
    tokio::runtime::Handle::try_current()?;
    Ok(())
}

impl AgentRuntime {
    /// Prepare durable storage and bundled Skills without starting sessions or
    /// requiring Tokio. The host can overlap this with its own database opening.
    pub fn prepare(data_root: &Path) -> anyhow::Result<PreparedAgent> {
        anyhow::ensure!(!data_root.as_os_str().is_empty(), "data_root is required");
        let store = Arc::new(StreamStore::open(data_root)?);
        zork_config::startup::mark("agent.store_opened");
        crate::skills::management::provision_bundled(data_root)?;
        zork_config::startup::mark("agent.skills_provisioned");
        Ok(PreparedAgent {
            root: data_root.to_owned(),
            store,
        })
    }

    /// Must be called inside a Tokio runtime. Opening storage is synchronous.
    pub fn start(options: AgentOptions) -> anyhow::Result<Self> {
        validate_options(&options)?;
        Self::prepare(&options.data_root)?.start(options)
    }

    fn start_with_store(mut options: AgentOptions, store: Arc<StreamStore>) -> anyhow::Result<Self> {
        let root = options.data_root.clone();
        let sources = options.skill_sources.unwrap_or_else(|| {
            Arc::new(move |_| {
                let config = if zork_config::config_path(&root).try_exists()? {
                    zork_config::load_config(&root)?.skills
                } else {
                    zork_config::SkillsConfig::default()
                };
                config.sources(&root, &[])
            })
        });
        for name in [
            "skill.list",
            "skill.sources",
            "skill.validate",
            "skill.write",
            "skill.archive",
            "skill.bundle",
        ] {
            options
                .tools
                .register_retired(name, Arc::new(crate::session::tools::NoToolState));
        }
        let catalog = options
            .skill_catalog
            .unwrap_or_else(|| crate::skills::local_catalog(sources.clone()));
        options.service.runner.skill_sources = Some(sources);
        options.service.runner.skill_catalog = Some(catalog);
        let query = Arc::new(FileSessionQuery::open(&options.data_root));
        let provider: Arc<dyn ModelExecutor> = if options.fake_agent {
            Arc::new(FakeProvider)
        } else {
            Arc::new(ProviderRouter::new())
        };
        let profiles = Arc::new(ProfileStore::open(
            options.data_root,
            options.fake_agent,
            options.no_streaming,
        ));
        zork_config::startup::mark("agent.profiles_opened");
        let model = Arc::new(AgentModelPort::new(provider, profiles.clone()));
        let clock = Arc::new(SystemClock);
        register_builtin_tools(
            &options.tools,
            BuiltinToolDependencies {
                environment: options.environment,
                query: query.clone(),
                clock: clock.clone(),
                files: options.files.unwrap_or_else(|| Arc::new(SystemFileSystem)),
                processes: Arc::new(SystemProcessSpawner),
            },
        )?;
        if let Some(configure) = options.configure_tools {
            configure(&options.tools)?;
        }
        zork_config::startup::mark("agent.tools_registered");
        let profile_options = ProfileStore::runner_options(&profiles);
        options.service.runner.input_budget = profile_options.input_budget;
        options.service.runner.max_output_tokens = profile_options.max_output_tokens;
        options.service.runner.context = options.context;
        let service = SessionService::start(
            ServiceDependencies {
                store,
                query,
                model,
                tools: options.tools,
                clock,
                ids: Arc::new(SystemIdGenerator),
            },
            options.service,
        );
        zork_config::startup::mark("agent.sessions_started");
        let configuration = zork_notify::Notifier::default();
        let profile_watch = options
            .profile_refresh_interval
            .filter(|_| !options.fake_agent)
            .map(|_| profiles.watch_profiles(configuration.clone()))
            .transpose()?;
        let mut configured = configuration.subscribe();
        zork_config::startup::mark("agent.profile_watch_ready");
        let refresh = options.profile_refresh_interval.filter(|_| !options.fake_agent).map(|interval| {
            let profiles = profiles.clone();
            tokio::spawn(async move {
                let mut retry = zork_notify::retry::Retry::default();
                loop {
                    configured.checkpoint();
                    let (external, mut failed) = match profiles.has_external_statuses() {
                        Ok(value) => (value, false),
                        Err(error) => { tracing::error!(%error, "profile configuration read failed"); (false, true) },
                    };
                    if external {
                        if let Err(error) = profiles.refresh_all_statuses().await {
                            failed = true;
                            tracing::error!(%error, "external profile status collection failed");
                        }
                    }
                    // Only the collector samples external query-only providers.
                    // Internal readers subscribe to committed results. With no
                    // configured accounts even this collector has no clock.
                    if !failed { retry.reset(); }
                    tokio::select! {
                        _ = retry.wait(), if failed => {},
                        changed = configured.changed() => if changed.is_err() { return; },
                        _ = tokio::time::sleep(interval), if external && !failed => {},
                    }
                }
            })
        });
        Ok(Self {
            agent: Agent { service, profiles },
            refresh,
            profile_watch,
            stopped: false,
        })
    }

    pub fn agent(&self) -> &Agent {
        &self.agent
    }

    /// Idempotent. Stops refresh, cancels running work, and joins core background tasks.
    pub async fn shutdown(&mut self) {
        if self.stopped {
            return;
        }
        if let Some(task) = self.refresh.take() {
            task.abort();
            let _ = task.await;
        }
        self.profile_watch.take();
        self.agent.service.shutdown().await;
        self.stopped = true;
    }
}

impl Drop for AgentRuntime {
    fn drop(&mut self) {
        if let Some(task) = self.refresh.take() {
            task.abort();
        }
        if !self.stopped {
            if let Ok(runtime) = tokio::runtime::Handle::try_current() {
                let service = self.agent.service.clone();
                runtime.spawn(async move {
                    service.shutdown().await;
                });
            }
        }
    }
}
