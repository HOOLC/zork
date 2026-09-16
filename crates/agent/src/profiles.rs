use std::path::PathBuf;
use std::sync::Arc;

use serde_json::Value;

use crate::session::ports::{ModelLimits, ProfileExecution, ProfileResolveError, ProfileResolver};
use crate::session::runner::RunnerOptions;
use crate::session::state::SessionState;
use crate::session::wire::SessionSelection;

type ProfileRefresh = Arc<tokio::sync::Mutex<Option<(u64, std::time::Instant)>>>;

#[derive(Clone)]
pub struct ProfileStore {
    data_root: PathBuf,
    fake: bool,
    no_streaming: bool,
    http: reqwest::Client,
    statuses: zork_profile::MemoryStore,
    changed: zork_notify::Notifier,
    pool: zork_profile::AccountPool,
    refreshes: Arc<std::sync::Mutex<std::collections::HashMap<String, ProfileRefresh>>>,
    revision: Arc<std::sync::atomic::AtomicU64>,
}

impl ProfileStore {
    pub fn open(data_root: PathBuf, fake: bool, no_streaming: bool) -> Self {
        let changed = zork_notify::Notifier::default();
        let publisher = changed.clone();
        Self {
            data_root,
            fake,
            no_streaming,
            http: reqwest::Client::new(),
            statuses: zork_profile::MemoryStore::observed(move || publisher.notify()),
            changed,
            pool: zork_profile::AccountPool::default(),
            refreshes: Arc::new(std::sync::Mutex::new(std::collections::HashMap::new())),
            revision: Arc::new(std::sync::atomic::AtomicU64::new(0)),
        }
    }

    pub fn subscribe(&self) -> zork_notify::Changes {
        self.changed.subscribe()
    }
    pub(crate) fn has_external_statuses(&self) -> anyhow::Result<bool> {
        Ok(!self.fake && self.list()?.iter().any(|p| p.auth_configured))
    }
    pub(crate) fn watch_profiles(
        &self,
        changed: zork_notify::Notifier,
    ) -> anyhow::Result<zork_notify::files::FileWatch> {
        let root = self.data_root.join("profiles");
        let selected = root.clone();
        Ok(zork_notify::files::watch_paths(
            vec![(root, false)],
            move |path| {
                path.parent() == Some(selected.as_path())
                    && path.extension().is_some_and(|ext| ext == "json")
            },
            move |_| changed.notify(),
        )?)
    }

    fn paths(&self) -> zork_profile::DataRootPaths {
        zork_profile::DataRootPaths {
            data_root: self.data_root.clone(),
        }
    }

    pub fn list(&self) -> anyhow::Result<Vec<zork_profile::ProfileView>> {
        zork_profile::list_profiles_with_status(&self.paths(), &self.statuses)
    }

    pub fn get(&self, profile_id: &str) -> anyhow::Result<Option<zork_profile::ProfileView>> {
        zork_profile::get_profile_with_status(&self.paths(), &self.statuses, profile_id)
    }

    pub fn put(&self, profile_id: &str, body: Value) -> anyhow::Result<zork_profile::ProfileView> {
        let profile = zork_profile::put_profile(&self.paths(), profile_id, body)?;
        self.pool.clear_failure(profile_id);
        zork_profile::ProfileStore::remove_probe(&self.statuses, profile_id)?;
        self.revision
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(profile)
    }

    pub fn delete(&self, profile_id: &str) -> anyhow::Result<()> {
        zork_profile::delete_profile(&self.paths(), profile_id)?;
        zork_profile::ProfileStore::remove_probe(&self.statuses, profile_id)
    }

    pub fn update_models(
        &self,
        profile_id: &str,
        models: Vec<zork_profile::ProfileModel>,
        expected: Option<Vec<zork_profile::ProfileModel>>,
    ) -> anyhow::Result<zork_profile::ProfileView> {
        zork_profile::update_models_checked(
            &self.paths(),
            profile_id,
            models,
            expected.as_deref(),
        )?;
        self.get(profile_id)?
            .ok_or_else(|| anyhow::anyhow!("profile not found"))
    }

    pub fn update_name(
        &self,
        profile_id: &str,
        name: &str,
    ) -> anyhow::Result<zork_profile::ProfileView> {
        zork_profile::update_name(&self.paths(), profile_id, name)?;
        self.get(profile_id)?
            .ok_or_else(|| anyhow::anyhow!("profile not found"))
    }

    pub fn set_model_enabled(
        &self,
        profile_id: &str,
        model_id: &str,
        enabled: bool,
    ) -> anyhow::Result<(zork_profile::ProfileView, bool)> {
        let changed =
            zork_profile::set_model_enabled(&self.paths(), profile_id, model_id, enabled)?;
        Ok((
            self.get(profile_id)?
                .ok_or_else(|| anyhow::anyhow!("profile not found"))?,
            changed,
        ))
    }

    pub async fn refresh_models(
        &self,
        profile_id: &str,
    ) -> anyhow::Result<zork_profile::ModelUpdate> {
        let mut result = zork_profile::refresh_models(&self.paths(), profile_id).await?;
        result.profile = self
            .get(profile_id)?
            .ok_or_else(|| anyhow::anyhow!("profile not found"))?;
        Ok(result)
    }

    pub async fn discover_models(
        &self,
        profile_id: &str,
    ) -> anyhow::Result<zork_profile::ModelDiscovery> {
        zork_profile::discover_models(&self.paths(), profile_id).await
    }

    pub async fn refresh_all_statuses(&self) -> anyhow::Result<()> {
        if !self.fake {
            for profile in self.list()? {
                self.refresh_status(&profile.profile_id).await?;
            }
        }
        Ok(())
    }

    pub async fn refresh_status(&self, profile_id: &str) -> anyhow::Result<()> {
        if !self.fake {
            let refresh = self
                .refreshes
                .lock()
                .map_err(|_| anyhow::anyhow!("profile refresh lock poisoned"))?
                .entry(profile_id.to_owned())
                .or_insert_with(|| Arc::new(tokio::sync::Mutex::new(None)))
                .clone();
            let mut refreshed = refresh.lock().await;
            let revision = self.revision.load(std::sync::atomic::Ordering::SeqCst);
            // Coalesce UI and background refreshes, including failed probes.
            // A credential edit clears the snapshot and must bypass the cooldown.
            if self
                .get(profile_id)?
                .is_some_and(|p| p.checked_at.is_some())
                && refreshed.is_some_and(|(r, t)| r == revision && t.elapsed().as_secs() < 30)
            {
                return Ok(());
            }
            zork_profile::refresh_profile(&self.paths(), &self.statuses, &self.http, profile_id)
                .await?;
            // A save can occur while the provider is answering. Its next probe must
            // not reuse this result, even when the saved credentials are unchanged.
            *refreshed = Some((revision, std::time::Instant::now()));
        }
        Ok(())
    }

    pub fn acquire_account(
        &self,
        selection: &SessionSelection,
        preferred: Option<&str>,
        require_images: bool,
    ) -> Result<zork_profile::AccountLease, ProfileResolveError> {
        let mut profiles = self.list().map_err(|_| ProfileResolveError::Backend)?;
        if require_images {
            profiles.retain(|profile| {
                zork_profile::compatible_model(profile, &selection.model, &selection.thinking)
                    .is_some_and(|model| {
                        model
                            .capabilities
                            .input
                            .iter()
                            .any(|input| input == "image")
                    })
            });
        }
        self.pool
            .acquire(
                &profiles,
                &selection.model,
                &selection.thinking,
                preferred,
                unix_now(),
            )
            .ok_or(ProfileResolveError::AuthUnavailable)
    }

    pub fn reserve_explicit_account(&self, profile_id: &str) -> zork_profile::AccountLease {
        self.pool.reserve_explicit(profile_id)
    }

    pub fn runner_options(profiles: &Arc<Self>) -> RunnerOptions {
        let budget_profiles = profiles.clone();
        let input_budget = Arc::new(move |state: &SessionState| {
            let selection = state.selection.as_ref()?;
            let limits = budget_profiles.model_limits(selection).ok()?;
            Some(handoff_input_trigger(
                limits.context_window_tokens,
                u64::from(limits.max_output_tokens),
                limits.reserve_percent,
            ))
        });
        let output_profiles = profiles.clone();
        let max_output_tokens = Arc::new(move |state: &SessionState| {
            let selection = state.selection.as_ref()?;
            output_profiles
                .model_limits(selection)
                .ok()
                .map(|limits| limits.max_output_tokens)
        });
        RunnerOptions {
            input_budget,
            max_output_tokens,
            ..RunnerOptions::default()
        }
    }
}

pub(crate) fn unix_now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn handoff_input_trigger(
    context_window_tokens: u64,
    max_output_tokens: u64,
    reserve_percent: u64,
) -> u64 {
    const HANDOFF_STEP_OVERHEAD_TOKENS: u64 = 4_096;
    let output_ceiling = context_window_tokens.saturating_sub(max_output_tokens);
    let percent_reserve = context_window_tokens.saturating_mul(reserve_percent) / 100;
    context_window_tokens
        .saturating_sub(max_output_tokens.max(percent_reserve))
        .min(output_ceiling.saturating_sub(HANDOFF_STEP_OVERHEAD_TOKENS))
}

#[async_trait::async_trait]
impl ProfileResolver for ProfileStore {
    fn model_limits(
        &self,
        selection: &SessionSelection,
    ) -> Result<ModelLimits, ProfileResolveError> {
        if selection.is_auto() {
            // Stable conservative limits across every compatible account, even
            // temporarily exhausted ones. Quota changes cannot enlarge a budget.
            let profiles = self.list().map_err(|_| ProfileResolveError::Backend)?;
            let mut limits = profiles.iter().filter_map(|p| {
                zork_profile::compatible_model(p, &selection.model, &selection.thinking)?
                    .limits
                    .as_ref()
            });
            let first = limits.next().ok_or(ProfileResolveError::InvalidSelection)?;
            let mut output_ceiling = first
                .context_window_tokens
                .saturating_sub(u64::from(first.max_output_tokens));
            let mut result = ModelLimits {
                context_window_tokens: first.context_window_tokens,
                max_output_tokens: first.max_output_tokens,
                reserve_percent: first.reserve_percent,
            };
            for next in limits {
                output_ceiling = output_ceiling.min(
                    next.context_window_tokens
                        .saturating_sub(u64::from(next.max_output_tokens)),
                );
                result.context_window_tokens =
                    result.context_window_tokens.min(next.context_window_tokens);
                result.max_output_tokens = result.max_output_tokens.min(next.max_output_tokens);
                result.reserve_percent = result.reserve_percent.max(next.reserve_percent);
            }
            result.context_window_tokens = result
                .context_window_tokens
                .min(output_ceiling.saturating_add(u64::from(result.max_output_tokens)));
            return Ok(result);
        }
        let paths = self.paths();
        let document = zork_profile::read_profile(&paths, &selection.profile_id)
            .map_err(|_| ProfileResolveError::NotFound)?;
        let model = zork_profile::select_model(&document, &selection.model, &selection.thinking)
            .map_err(|_| ProfileResolveError::InvalidSelection)?;
        let limits = model
            .limits
            .as_ref()
            .ok_or(ProfileResolveError::InvalidSelection)?;
        Ok(ModelLimits {
            context_window_tokens: limits.context_window_tokens,
            max_output_tokens: limits.max_output_tokens,
            reserve_percent: limits.reserve_percent,
        })
    }

    async fn resolve(
        &self,
        selection: &SessionSelection,
    ) -> Result<ProfileExecution, ProfileResolveError> {
        let limits = self.model_limits(selection)?;
        if self.fake {
            return Ok(ProfileExecution::new(
                selection.profile_id.clone(),
                "openai".to_owned(),
                selection.model.clone(),
                "openai-completions".to_owned(),
                !self.no_streaming,
                false,
                None,
                "http://127.0.0.1:9/v1".to_owned(),
                Default::default(),
                selection.thinking.clone(),
                limits,
                "fake".to_owned(),
            ));
        }
        let execution = zork_profile::load_selected(
            &self.paths(),
            &self.http,
            &selection.profile_id,
            &selection.model,
            &selection.thinking,
        )
        .await
        .map_err(|_| ProfileResolveError::AuthUnavailable)?;
        Ok(ProfileExecution::new(
            execution.profile_id,
            execution.provider.clone(),
            execution.model,
            execution.api,
            execution.streaming && !self.no_streaming,
            execution.parallel_tool_calls,
            execution.service_tier,
            execution.base_url,
            execution.headers,
            execution.thinking,
            ModelLimits {
                context_window_tokens: execution.limits.context_window_tokens,
                max_output_tokens: execution.limits.max_output_tokens,
                reserve_percent: execution.limits.reserve_percent,
            },
            execution.bearer,
        )
        .with_image_input(execution.image_input)
        .with_single_system_message(execution.single_system_message))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn model_catalog_updates_persist_and_failures_preserve_configuration() {
        use std::sync::atomic::{AtomicBool, Ordering};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}/v1", listener.local_addr().unwrap());
        let requests = Arc::new(std::sync::Mutex::new(Vec::new()));
        let failed = Arc::new(AtomicBool::new(false));
        let server = tokio::spawn({
            let requests = requests.clone();
            let failed = failed.clone();
            async move {
                loop {
                    let (mut socket, _) = listener.accept().await.unwrap();
                    let mut bytes = Vec::new();
                    let mut chunk = [0u8; 1024];
                    while !bytes.windows(4).any(|s| s == b"\r\n\r\n") {
                        let n = socket.read(&mut chunk).await.unwrap();
                        if n == 0 {
                            break;
                        }
                        bytes.extend_from_slice(&chunk[..n]);
                    }
                    let request = String::from_utf8_lossy(&bytes).to_ascii_lowercase();
                    requests.lock().unwrap().push(request.clone());
                    let body = if request.contains("client_version") {
                        json!({"models":[{"slug":"gpt-5.6-luna","visibility":"list","context_window":128000}]})
                    } else {
                        json!({"data":[{"id":"ready","contextWindow":32000,"maxCompletionTokens":4096},{"id":"pending"}]})
                    };
                    let body = body.to_string();
                    let status = if failed.load(Ordering::SeqCst) {
                        "503 Service Unavailable"
                    } else {
                        "200 OK"
                    };
                    let response = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                        body.len()
                    );
                    socket.write_all(response.as_bytes()).await.unwrap();
                }
            }
        });
        let root = tempfile::tempdir().unwrap();
        let profiles = ProfileStore::open(root.path().to_owned(), true, false);
        for (provider, billing) in [
            ("xai", "subscription"),
            ("openai", "subscription"),
            ("anthropic", "subscription"),
            ("openai-compatible", "usage"),
        ] {
            profiles.put(provider,json!({"provider":provider,"billing":billing,"base_url":base,"auth":{"access":"private-access","key":"private-access","accountId":"private-account"},"models":[]})).unwrap();
            let result = profiles.refresh_models(provider).await.unwrap();
            assert!(result.added > 0);
            assert!(!serde_json::to_string(&result)
                .unwrap()
                .contains("private-access"));
        }
        let requests = requests.lock().unwrap().clone();
        assert!(requests[0].contains("get /v1/models "));
        assert!(requests[0].contains("x-xai-token-auth: xai-grok-cli"));
        assert!(requests[1].contains("client_version="));
        assert!(requests[1].contains("chatgpt-account-id: private-account"));
        assert!(requests[2].contains("authorization: bearer private-access"));
        assert!(requests[2].contains("anthropic-beta:"));
        assert!(requests[3].contains("authorization: bearer private-access"));
        let result = profiles.refresh_models("openai-compatible").await.unwrap();
        assert_eq!(result.added, 0);
        let (disabled, changed) = profiles
            .set_model_enabled("openai-compatible", "ready", false)
            .unwrap();
        assert!(changed);
        assert!(
            !disabled
                .models
                .iter()
                .find(|model| model.id == "ready")
                .unwrap()
                .enabled
        );
        assert!(profiles
            .set_model_enabled("openai-compatible", "pending", true)
            .is_err());
        profiles.refresh_models("openai-compatible").await.unwrap();
        let stored = zork_profile::read_profile(&profiles.paths(), "openai-compatible").unwrap();
        assert!(
            !stored
                .models
                .iter()
                .find(|model| model.id == "ready")
                .unwrap()
                .enabled
        );
        failed.store(true, Ordering::SeqCst);
        assert!(profiles.refresh_models("openai-compatible").await.is_err());
        assert_eq!(
            zork_profile::read_profile(&profiles.paths(), "openai-compatible").unwrap(),
            stored
        );
        server.abort();
    }

    #[tokio::test]
    async fn refresh_coalesces_and_credential_edits_invalidate_the_cooldown() {
        use zork_profile::ProfileStore as _;
        let root = tempfile::tempdir().unwrap();
        let profiles = ProfileStore::open(root.path().to_owned(), false, false);
        let document = json!({"provider":"openai-compatible","billing":"usage","base_url":"https://example.invalid/v1","models":[],"auth":{"type":"api_key","key":"test-key"}});
        profiles.put("fixture", document.clone()).unwrap();
        profiles.refresh_status("fixture").await.unwrap();
        assert!(profiles
            .get("fixture")
            .unwrap()
            .unwrap()
            .checked_at
            .is_some());
        let cached = json!({"ok":false,"error":"cached_failure"});
        profiles
            .statuses
            .upsert_probe("fixture", &json!({"ok":true}), &cached)
            .unwrap();
        let (first, second) = tokio::join!(
            profiles.refresh_status("fixture"),
            profiles.refresh_status("fixture")
        );
        first.unwrap();
        second.unwrap();
        assert_eq!(
            profiles.get("fixture").unwrap().unwrap().rate_limits,
            cached
        );
        let checked = profiles.get("fixture").unwrap().unwrap().checked_at;
        let renamed = profiles.update_name("fixture", "My connection").unwrap();
        assert_eq!(renamed.name.as_deref(), Some("My connection"));
        assert_eq!(renamed.rate_limits, cached);
        assert_eq!(renamed.checked_at, checked);
        profiles.put("fixture", document).unwrap();
        profiles.refresh_status("fixture").await.unwrap();
        assert_eq!(
            profiles.get("fixture").unwrap().unwrap().rate_limits,
            json!({"ok":true,"reported":false})
        );
        let revision = profiles.revision.load(std::sync::atomic::Ordering::SeqCst);
        let document = json!({"provider":"openai-compatible","billing":"usage","base_url":"https://changed.invalid/v1","models":[],"auth":{"type":"api_key","key":"test-key"}});
        profiles.put("fixture", document).unwrap();
        // Simulate a pre-save probe completing after the save cleared its snapshot.
        profiles
            .statuses
            .upsert_probe("fixture", &json!({"ok":true}), &cached)
            .unwrap();
        let refresh = profiles.refreshes.lock().unwrap()["fixture"].clone();
        *refresh.lock().await = Some((revision, std::time::Instant::now()));
        profiles.refresh_status("fixture").await.unwrap();
        assert_eq!(
            profiles.get("fixture").unwrap().unwrap().rate_limits,
            json!({"ok":true,"reported":false})
        );
        assert!(profiles.refresh_status("missing").await.is_err());
    }

    #[tokio::test]
    // Contract: docs/design/agent-runtime.md [PROVIDER-01, PROVIDER-03]
    async fn process_override_disables_streaming_for_every_resolved_profile() {
        let root = tempfile::tempdir().unwrap();
        let profile = ProfileStore::open(root.path().to_owned(), true, false);
        profile
            .put(
                "fixture",
                json!({
                    "provider": "openai-compatible",
                    "billing": "usage",
                    "base_url": "https://example.invalid/v1",
                    "models": [{
                        "id": "model",
                        "api": "openai-completions",
                        "thinking": ["high"],
                        "default_thinking": "high",
                        "capabilities": {"input": ["text"]},
                        "limits": {"context_window_tokens": 100_000, "max_output_tokens": 10_000},
                        "default": true
                    }]
                }),
            )
            .unwrap();
        let selection = SessionSelection {
            profile_id: "fixture".to_owned(),
            model: "model".to_owned(),
            thinking: "high".to_owned(),
        };

        let enabled = profile.resolve(&selection).await.unwrap();
        assert!(enabled.streaming());

        let disabled_profile = ProfileStore::open(root.path().to_owned(), true, true);
        let disabled = disabled_profile.resolve(&selection).await.unwrap();
        assert!(!disabled.streaming());
    }

    #[tokio::test]
    // Contract: docs/design/agent-runtime.md [PROVIDER-03]
    async fn execution_rejects_a_model_without_limits_before_provider_resolution() {
        let root = tempfile::tempdir().unwrap();
        let profiles = ProfileStore::open(root.path().to_owned(), true, false);
        profiles
            .put(
                "fixture",
                json!({
                    "provider": "openai-compatible",
                    "billing": "usage",
                    "base_url": "https://example.invalid/v1",
                    "models": [{
                        "id": "model",
                        "api": "openai-completions",
                        "thinking": ["high"],
                        "default_thinking": "high",
                        "capabilities": {"input": ["text"]},
                        "default": true
                    }]
                }),
            )
            .unwrap();

        assert!(matches!(
            profiles
                .resolve(&SessionSelection {
                    profile_id: "fixture".into(),
                    model: "model".into(),
                    thinking: "high".into(),
                })
                .await,
            Err(ProfileResolveError::InvalidSelection)
        ));
    }
}
