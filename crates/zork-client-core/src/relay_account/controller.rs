//! The profile owns this controller; closing an account view only drops an observer.
use super::*;
use std::sync::Arc;
use tokio::sync::mpsc;
pub use zork_client_types::account::{Phase, Snapshot};
use zork_observe::{ValueSource, ValueSubscription};

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Action {
    Login,
    Cancel,
    Logout,
}
pub struct Controller {
    pub(crate) source: Arc<ValueSource<Snapshot>>,
    commands: mpsc::Sender<Action>,
}
impl Controller {
    #[cfg(test)]
    pub(crate) fn fixture(snapshot: Snapshot) -> Arc<Self> {
        let (commands, _receiver) = mpsc::channel(16);
        Arc::new(Self {
            source: Arc::new(ValueSource::new(snapshot)),
            commands,
        })
    }

    pub fn open(root: &Path) -> Result<Arc<Self>> {
        Self::new(Account::configured(root)?)
    }
    pub fn new(account: Account) -> Result<Arc<Self>> {
        let watch = zork_notify::files::Source::new([storage::path(account.data_root())])?;
        let changes = watch.subscribe();
        let mut initial = Snapshot {
            origin: account.origin().to_owned(),
            ..Default::default()
        };
        update_identity(&account, &mut initial);
        let source = Arc::new(ValueSource::new(initial));
        let (commands, receiver) = mpsc::channel(16);
        let controller = Arc::new(Self {
            source: source.clone(),
            commands,
        });
        std::thread::Builder::new()
            .name("zork-account".into())
            .spawn(move || {
                let runtime = match tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                {
                    Ok(runtime) => runtime,
                    Err(_) => {
                        let mut value = (*source.read()).clone();
                        value.error = Some("无法启动账号服务".into());
                        source.publish(value);
                        return;
                    }
                };
                runtime.block_on(run(account, source, receiver, watch, changes));
            })?;
        Ok(controller)
    }
    pub fn submit(&self, action: Action) -> Result<()> {
        self.commands
            .try_send(action)
            .map_err(|_| anyhow::anyhow!("账号操作繁忙，请稍后重试"))
    }
    pub fn snapshot(&self) -> Arc<Snapshot> {
        self.source.read()
    }
    pub fn subscribe(&self) -> ValueSubscription<Snapshot> {
        self.source.subscribe()
    }
}
fn update_identity(account: &Account, state: &mut Snapshot) {
    match account.local_identity() {
        Ok((subject, email, authenticated, pending)) => {
            state.subject = subject;
            state.email = email;
            state.authenticated = authenticated;
            state.pending_revocations = pending;
        }
        Err(_) => {
            state.subject = None;
            state.email = None;
            state.authenticated = false;
            state.error = Some("无法读取本机账号状态".into());
        }
    }
}
async fn run(
    account: Account,
    source: Arc<ValueSource<Snapshot>>,
    mut commands: mpsc::Receiver<Action>,
    _watch: zork_notify::files::Source,
    mut changes: zork_notify::Changes,
) {
    let wake = Arc::new(tokio::sync::Notify::new());
    let _maintenance = account.maintain({
        let wake = wake.clone();
        move |_| {
            let wake = wake.clone();
            async move {
                wake.notify_one();
                Ok(())
            }
        }
    });
    let (urls, mut ready) = mpsc::channel(16);
    let mut jobs = tokio::task::JoinSet::new();
    let mut generation = 0u64;
    let mut login_attempt: Option<String> = None;
    let mut state = (*source.read()).clone();
    loop {
        tokio::select! {
            command = commands.recv() => {
                let Some(command) = command else { break; };
                match command {
                    Action::Login if !state.busy() => {
                        if jobs.len() >= 4 { state.error = Some("正在清理之前的登录请求，请稍后重试".into()); source.publish(state.clone()); continue; }
                        generation += 1;
                        state.phase = Phase::Starting; state.login_url = None; state.error = None;
                        source.publish(state.clone());
                        let attempt = ulid::Ulid::new().to_string();
                        if let Err(error) = account.reserve_login(&attempt).await {
                            state.phase = Phase::Idle; state.error = Some(error.to_string());
                            source.publish(state.clone()); continue;
                        }
                        login_attempt = Some(attempt.clone());
                        let (account, urls, owner) = (account.clone(), urls.clone(), generation);
                        jobs.spawn(async move {
                            let result = async {
                                let login = account.start_device_login(&zork_config::device_name(), attempt).await?;
                                let _ = urls.send((owner, login.url().to_owned())).await;
                                login.finish().await?;
                                Ok::<_, anyhow::Error>(())
                            }.await;
                            (owner, result)
                        });
                    }
                    Action::Cancel if state.phase != Phase::SigningOut => {
                        generation += 1;
                        state.phase = Phase::Idle; state.login_url = None; state.error = None;
                        source.publish(state.clone());
                        if let Some(attempt) = login_attempt.take() {
                            if let Err(error) = account.cancel_login(&attempt).await { state.error = Some(error.to_string()); }
                        }
                        // Keep the request alive to consume/revoke any late credentials.
                    }
                    Action::Logout if !state.busy() => {
                        generation += 1;
                        state.phase = Phase::SigningOut; state.login_url = None; state.error = None;
                        let (account, owner) = (account.clone(), generation);
                        jobs.spawn(async move { (owner, account.logout(false).await.map(|_| ())) });
                    }
                    _ => {}
                }
            }
            Some((owner, url)) = ready.recv() => {
                if owner == generation { state.phase = Phase::Waiting; state.login_url = Some(url); }
            }
            Some(result) = jobs.join_next(), if !jobs.is_empty() => {
                if let Ok((owner, result)) = result {
                    if owner == generation { login_attempt = None; state.phase = Phase::Idle; state.login_url = None; state.error = result.err().map(|error| error.to_string()); }
                }
            }
            change = changes.changed() => { if change.is_err() { break; } }
            _ = wake.notified() => {}
        }
        update_identity(&account, &mut state);
        source.publish(state.clone());
    }
    if let Some(attempt) = login_attempt {
        let _ = account.cancel_login(&attempt).await;
    }
    // Do not drop an in-flight credential response before its owner can revoke it.
    let _ = tokio::time::timeout(Duration::from_secs(35), async {
        while jobs.join_next().await.is_some() {}
    })
    .await;
}
