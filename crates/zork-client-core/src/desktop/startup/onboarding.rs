//! First-use policy is owned by the app runtime, not a window or login callback.
use super::*;
use crate::{relay_account::controller::Snapshot as Account, state::Profiles};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Onboarding {
    Login,
    Preparing,
    Models,
    Ready,
}

const KEY: &str = "onboarding-complete";

fn required(directory: &Directory) -> Result<bool> {
    if let Some(complete) = directory.store.get::<bool>("client", KEY)? {
        return Ok(!complete);
    }
    let saved = directory.snapshot();
    // Existing installations retain their workspace, including offline use.
    let complete = saved.local_enabled || !saved.nodes.is_empty();
    directory.store.put("client", KEY, &complete)?;
    Ok(!complete)
}

impl Recovery {
    pub(super) fn observe_account(self: &Arc<Self>) -> Result<()> {
        let mut updates = self.directory.account.subscribe();
        self.account_changed(&updates.snapshot());
        let weak = Arc::downgrade(self);
        let stop = self.onboarding_stop.clone();
        std::thread::Builder::new()
            .name("zork-onboarding".into())
            .spawn(move || {
                let runtime = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build();
                let Ok(runtime) = runtime else {
                    if let Some(owner) = weak.upgrade() {
                        let mut state = owner.state.lock().unwrap();
                        state.onboarding_error =
                            Some("无法启动首次使用流程，请重新打开应用".into());
                        owner.updates.publish(state.clone());
                    }
                    return;
                };
                runtime.block_on(async move {
                    loop {
                        tokio::select! {
                            _ = stop.notified() => break,
                            changed = updates.changed() => {
                                let Some(account) = changed else { break; };
                                let Some(owner) = weak.upgrade() else { break; };
                                owner.account_changed(&account);
                                if owner.updates.read().onboarding.is_none() { break; }
                            }
                        }
                    }
                });
            })?;
        Ok(())
    }

    fn account_changed(self: &Arc<Self>, account: &Account) {
        let mut state = self.state.lock().unwrap();
        if state.onboarding.is_none() || self.stopping.load(Ordering::Acquire) {
            return;
        }
        let next = if account.authenticated {
            if state.onboarding == Some(Onboarding::Login) {
                Some(Onboarding::Preparing)
            } else {
                state.onboarding
            }
        } else {
            Some(Onboarding::Login)
        };
        if state.onboarding != next {
            state.onboarding = next;
            self.updates.publish(state.clone());
        }
        let start = account.authenticated && state.local == Phase::NotRequired;
        drop(state);
        if start {
            self.launch(Step::Local);
        }
    }

    fn models_changed(&self, id: &str, binding: u64, profiles: &crate::state::ProfileData) {
        let mut state = self.state.lock().unwrap();
        if self.stopping.load(Ordering::Acquire)
            || state.onboarding.is_none()
            || !self.directory.account.snapshot().authenticated
            || state.local != Phase::Ready
            || state.selected.as_deref() != Some(id)
            || !self
                .directory
                .connection(id)
                .is_ok_and(|(current, _)| current == binding)
        {
            return;
        }
        if !profiles.loaded && profiles.error.is_none() {
            return;
        }
        let usable = profiles.loaded
            && !crate::new_chat::present(&profiles.profiles, "", "", "", "auto").needs_model;
        let next = if usable {
            Onboarding::Ready
        } else {
            Onboarding::Models
        };
        // A previously configured model skips setup entirely. New connections
        // retain the setup screen until the user elects to start chatting.
        if usable && state.onboarding == Some(Onboarding::Preparing) {
            match self.directory.store.put("client", KEY, &true) {
                Ok(()) => {
                    state.onboarding = None;
                    state.onboarding_error = None;
                    self.onboarding_stop.notify_one();
                }
                Err(error) => {
                    state.onboarding = Some(Onboarding::Ready);
                    state.onboarding_error = Some(error.to_string());
                }
            }
            self.updates.publish(state.clone());
        } else if state.onboarding != Some(next) || state.onboarding_error != profiles.error {
            state.onboarding = Some(next);
            state.onboarding_error = profiles.error.clone();
            self.updates.publish(state.clone());
        }
    }
}

impl Startup {
    /// Bind the same model controller used by the workspace. Results from an old
    /// device connection cannot advance the first-use flow.
    pub fn observe_onboarding_models(&self, id: &str, profiles: Arc<Profiles>) -> Result<()> {
        if self.recovery.updates.read().onboarding.is_none() {
            return Ok(());
        }
        let (binding, client) = self.directory.connection(id)?;
        let mut task = self.recovery.onboarding_models.lock().unwrap();
        if task
            .as_ref()
            .is_some_and(|(old_id, old_binding, _, _)| old_id == id && *old_binding == binding)
        {
            return Ok(());
        }
        if let Some((_, _, _, old)) = task.take() {
            old.abort();
        }
        let mut model_updates = profiles.subscribe_state();
        let mut startup_updates = self.subscribe();
        let weak = Arc::downgrade(&self.recovery);
        let node = id.to_owned();
        let handle = client.spawn(async move {
            loop {
                let Some(owner) = weak.upgrade() else {
                    return;
                };
                owner.models_changed(&node, binding, &model_updates.snapshot());
                if owner.updates.read().onboarding.is_none() {
                    return;
                }
                drop(owner);
                tokio::select! {
                    update = model_updates.changed() => { if update.is_none() { return; } },
                    update = startup_updates.changed() => { if update.is_none() { return; } },
                }
            }
        });
        *task = Some((id.to_owned(), binding, profiles, handle));
        Ok(())
    }

    pub fn finish_onboarding(&self) -> Result<()> {
        let mut state = self.recovery.state.lock().unwrap();
        anyhow::ensure!(
            !self.recovery.stopping.load(Ordering::Acquire),
            "客户端正在退出"
        );
        anyhow::ensure!(
            state.onboarding == Some(Onboarding::Ready),
            "模型尚未准备好"
        );
        anyhow::ensure!(self.directory.account.snapshot().authenticated, "请先登录");
        anyhow::ensure!(state.local == Phase::Ready, "工作空间尚未准备好");
        let models = self.recovery.onboarding_models.lock().unwrap();
        let (id, binding, profiles, _) = models.as_ref().context("模型尚未准备好")?;
        anyhow::ensure!(
            state.selected.as_deref() == Some(id) && self.directory.connection(id)?.0 == *binding,
            "设备连接已变化"
        );
        anyhow::ensure!(
            !crate::new_chat::present(&profiles.snapshot().profiles, "", "", "", "auto")
                .needs_model,
            "模型尚未准备好"
        );
        self.directory.store.put("client", KEY, &true)?;
        state.onboarding = None;
        state.onboarding_error = None;
        self.recovery.updates.publish(state.clone());
        self.recovery.onboarding_stop.notify_one();
        Ok(())
    }
}

pub(super) fn initial(directory: &Directory) -> Result<Option<Onboarding>> {
    Ok(required(directory)?.then_some(Onboarding::Login))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{api::ProfileInfo, state::ProfileData, store::ClientStore};

    fn directory(root: &std::path::Path) -> Arc<Directory> {
        Directory::open(root).unwrap()
    }

    fn with_node(root: &std::path::Path) -> Arc<Directory> {
        let store = ClientStore::open(root).unwrap();
        store
            .save_node(&SavedNode {
                id: "local".into(),
                name: "Local".into(),
                url: "http://127.0.0.1:9".into(),
                token: None,
                local: true,
                group: None,
                mesh: None,
            })
            .unwrap();
        drop(store);
        directory(root)
    }

    fn models(auth: bool, enabled: bool) -> ProfileData {
        let profiles: Vec<ProfileInfo> = serde_json::from_value(serde_json::json!([{
            "profile_id":"test", "provider":"openai", "auth_configured":auth,
            "models":[{"id":"test-model","enabled":enabled,"thinking":["off"],
                "default_thinking":"off","limits":{"context_window_tokens":100000,"max_output_tokens":1000}}]
        }])).unwrap();
        ProfileData {
            profiles: Arc::new(profiles),
            loaded: true,
            ..Default::default()
        }
    }

    fn signed_in(directory: &Directory, root: &std::path::Path, authenticated: bool) {
        use zork_config::relay_account::{self as storage, AccountFile, RelaySession};
        let now = storage::now();
        let current = authenticated.then(|| RelaySession {
            origin: directory.account.snapshot().origin.clone(),
            token: "test-access".into(),
            refresh_token: "test-refresh".into(),
            subject: "test-user".into(),
            email: None,
            session_id: ulid::Ulid::new().to_string(),
            expires_at: now + 3600,
            session_expires_at: now + 3600,
            refresh_expires_at: now + 3600,
            pending_refresh: None,
        });
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        let _lock = loop {
            if let Some(lock) = storage::try_lock(root).unwrap() {
                break lock;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "account fixture lock was busy"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        storage::write(
            root,
            &AccountFile {
                current,
                ..Default::default()
            },
        )
        .unwrap();
        drop(_lock);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
        while directory.account.snapshot().authenticated != authenticated {
            assert!(
                std::time::Instant::now() < deadline,
                "account fixture did not converge"
            );
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    }

    #[test]
    fn new_install_resumes_incomplete_flow_and_existing_install_is_not_gated() {
        let fresh = tempfile::tempdir().unwrap();
        let root = fresh.path().join("client");
        let first = directory(&root);
        assert!(required(&first).unwrap());
        first.store.set_local_node_enabled(true).unwrap();
        drop(first);
        // Creating the local runtime halfway through is not completion.
        let resumed = directory(&root);
        assert!(required(&resumed).unwrap());
        resumed.store.put("client", KEY, &true).unwrap();
        assert!(!required(&resumed).unwrap());

        let existing = tempfile::tempdir().unwrap();
        assert!(!required(&with_node(&existing.path().join("client"))).unwrap());
        let enabled = tempfile::tempdir().unwrap();
        let root = enabled.path().join("client");
        ClientStore::open(&root)
            .unwrap()
            .set_local_node_enabled(true)
            .unwrap();
        assert!(!required(&directory(&root)).unwrap());
    }

    #[test]
    fn an_identity_without_valid_credentials_does_not_finish_login() {
        let dir = tempfile::tempdir().unwrap();
        let recovery = Recovery::new(
            directory(&dir.path().join("client")),
            State {
                onboarding: Some(Onboarding::Login),
                local: Phase::Preparing,
                ..Default::default()
            },
        );
        let mut account = Account {
            subject: Some("known-user".into()),
            ..Default::default()
        };
        recovery.account_changed(&account);
        assert_eq!(recovery.updates.read().onboarding, Some(Onboarding::Login));
        account.authenticated = true;
        recovery.account_changed(&account);
        assert_eq!(
            recovery.updates.read().onboarding,
            Some(Onboarding::Preparing)
        );
        account.authenticated = false;
        recovery.account_changed(&account);
        assert_eq!(recovery.updates.read().onboarding, Some(Onboarding::Login));
        recovery.stopping.store(true, Ordering::Release);
        account.authenticated = true;
        recovery.account_changed(&account);
        assert_eq!(recovery.updates.read().onboarding, Some(Onboarding::Login));
    }

    #[test]
    fn model_gate_uses_chat_availability_and_rejects_old_bindings() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("client");
        let source = with_node(&root);
        source.store.put("client", KEY, &false).unwrap();
        signed_in(&source, &root, true);
        let binding = source.connection("local").unwrap().0;
        let recovery = Recovery::new(
            source.clone(),
            State {
                selected: Some("local".into()),
                local: Phase::Ready,
                onboarding: Some(Onboarding::Preparing),
                ..Default::default()
            },
        );
        recovery.models_changed("local", binding, &ProfileData::default());
        assert_eq!(
            recovery.updates.read().onboarding,
            Some(Onboarding::Preparing)
        );
        recovery.models_changed("local", binding + 1, &models(true, true));
        assert!(required(&source).unwrap());
        recovery.models_changed("local", binding, &models(false, true));
        assert_eq!(recovery.updates.read().onboarding, Some(Onboarding::Models));
        recovery.models_changed("local", binding, &models(true, false));
        assert_eq!(recovery.updates.read().onboarding, Some(Onboarding::Models));
        recovery.models_changed("local", binding, &models(true, true));
        assert_eq!(recovery.updates.read().onboarding, Some(Onboarding::Ready));
        assert!(required(&source).unwrap());
        recovery.models_changed("local", binding, &models(true, false));
        assert_eq!(recovery.updates.read().onboarding, Some(Onboarding::Models));
    }

    #[test]
    fn existing_usable_model_skips_setup_but_signed_out_and_late_results_do_not() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("client");
        let source = with_node(&root);
        source.store.put("client", KEY, &false).unwrap();
        let binding = source.connection("local").unwrap().0;
        let recovery = Recovery::new(
            source.clone(),
            State {
                selected: Some("local".into()),
                local: Phase::Ready,
                onboarding: Some(Onboarding::Preparing),
                ..Default::default()
            },
        );
        signed_in(&source, &root, false);
        recovery.models_changed("local", binding, &models(true, true));
        assert!(required(&source).unwrap());
        signed_in(&source, &root, true);
        recovery.models_changed("local", binding, &models(true, true));
        assert!(recovery.updates.read().onboarding.is_none());
        assert!(!required(&source).unwrap());
        recovery.models_changed("local", binding, &models(false, false));
        assert!(recovery.updates.read().onboarding.is_none());
    }

    #[tokio::test]
    async fn completion_rechecks_current_models_and_persists_before_leaving() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("client");
        let source = with_node(&root);
        source.store.put("client", KEY, &false).unwrap();
        signed_in(&source, &root, true);
        let (binding, client) = source.connection("local").unwrap();
        let profiles = Profiles::new(client.clone());
        let recovery = Recovery::new(
            source.clone(),
            State {
                selected: Some("local".into()),
                local: Phase::Ready,
                onboarding: Some(Onboarding::Ready),
                ..Default::default()
            },
        );
        let task = client.spawn(async {});
        *recovery.onboarding_models.lock().unwrap() =
            Some(("local".into(), binding, profiles.clone(), task));
        let startup = Startup {
            directory: source.clone(),
            recovery,
        };
        profiles.seed(models(true, false));
        assert!(startup.finish_onboarding().is_err());
        assert!(required(&source).unwrap());
        profiles.seed(models(true, true));
        signed_in(&source, &root, false);
        assert!(startup.finish_onboarding().is_err());
        signed_in(&source, &root, true);
        startup.finish_onboarding().unwrap();
        assert!(!required(&source).unwrap());
        assert!(startup.subscribe().snapshot().onboarding.is_none());
    }
}
