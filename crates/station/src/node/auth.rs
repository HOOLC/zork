//! Shared provider authorization service for settings and interaction cards.
//! Challenge material stays in this service and authenticated client responses.
use super::*;
use serde::Serialize;

pub(crate) struct Hub {
    attempts: Mutex<HashMap<String, Arc<Mutex<Attempt>>>>,
    pub(super) http: reqwest::Client,
}
impl Hub {
    pub(crate) fn new() -> anyhow::Result<Self> {
        Ok(Self {
            attempts: Default::default(),
            http: reqwest::Client::builder()
                .timeout(Duration::from_secs(30))
                .build()?,
        })
    }
}

async fn find(state: &AppState, id: &str) -> Result<Arc<Mutex<Attempt>>, Failure> {
    state
        .provider_auth
        .attempts
        .lock()
        .await
        .get(id)
        .cloned()
        .ok_or(failure(
            StatusCode::GONE,
            "Sign-in expired or was replaced; start again",
        ))
}

#[derive(Debug)]
pub(crate) struct Failure {
    pub status: StatusCode,
    pub message: &'static str,
    pub effects_unconfirmed: bool,
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.message)
    }
}
impl std::error::Error for Failure {}
fn failure(status: StatusCode, message: &'static str) -> Failure {
    Failure {
        status,
        message,
        effects_unconfirmed: false,
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Begin {
    pub profile_id: String,
    pub provider: String,
    pub billing: String,
}
#[derive(Clone, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub(crate) struct Poll {
    pub callback: Option<String>,
}

fn describe(id: &str, attempt: &Attempt) -> Value {
    let browser = matches!(
        attempt.pending.provider.as_str(),
        "anthropic" | "openrouter"
    );
    json!({"id":id,"profile_id":attempt.profile_id,"verification_url":attempt.pending.verification_url,
        "user_code":if browser{""}else{&attempt.pending.user_code},"interval_seconds":attempt.pending.interval_seconds,
        "expires_at":attempt.pending.expires_at,"flow":if browser{"browser_callback"}else{"device_code"}})
}

pub(crate) async fn begin(
    state: &AppState,
    body: Begin,
    key: Option<&str>,
) -> Result<Value, Failure> {
    if !valid_id(&body.profile_id) {
        return Err(failure(StatusCode::BAD_REQUEST, "Invalid profile ID"));
    }
    let provider = zork_profile::providers::get(&body.provider)
        .map_err(|_| failure(StatusCode::BAD_REQUEST, "Unknown provider"))?;
    if !provider.supports_device_code(&body.billing) {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "This provider and billing mode require an API key",
        ));
    }
    let _profile_guard = state
        .entries
        .lock_local_task(&format!("provider-auth:{}", body.profile_id))
        .await;
    let id = key
        .map(str::to_owned)
        .unwrap_or_else(|| ulid::Ulid::new().to_string());
    if let Ok(saved) = find(state, &id).await {
        let attempt = saved.lock().await;
        return Ok(describe(&id, &attempt));
    }
    let pending = provider
        .start_device_code(&state.provider_auth.http)
        .await
        .map_err(|_| {
            failure(
                StatusCode::BAD_GATEWAY,
                "Provider sign-in could not start; retry later",
            )
        })?;
    if pending.billing != body.billing {
        return Err(failure(
            StatusCode::BAD_REQUEST,
            "Provider returned an unexpected billing mode",
        ));
    }
    let ttl = chrono::DateTime::parse_from_rfc3339(&pending.expires_at)
        .ok()
        .and_then(|at| {
            (at.with_timezone(&chrono::Utc) - chrono::Utc::now())
                .to_std()
                .ok()
        })
        .ok_or(failure(
            StatusCode::BAD_GATEWAY,
            "Provider returned an expired sign-in",
        ))?;
    // One credential writer per profile. No global limit on unrelated attempts.
    let saved: Vec<_> = state
        .provider_auth
        .attempts
        .lock()
        .await
        .iter()
        .map(|(id, a)| (id.clone(), a.clone()))
        .collect();
    let mut obsolete = Vec::new();
    for (key, saved) in saved {
        if let Ok(a) = saved.try_lock() {
            if a.profile_id == body.profile_id || a.expires <= Instant::now() {
                obsolete.push(key);
            }
        }
    }
    let mut attempts = state.provider_auth.attempts.lock().await;
    for key in obsolete {
        attempts.remove(&key);
    }
    let attempt = Attempt {
        profile_id: body.profile_id,
        pending,
        expires: Instant::now() + ttl,
        next_poll: Instant::now(),
        completed: false,
    };
    let value = describe(&id, &attempt);
    attempts.insert(id, Arc::new(Mutex::new(attempt)));
    Ok(value)
}

pub(crate) async fn poll(state: &AppState, id: &str, body: Poll) -> Result<Value, Failure> {
    let saved = find(state, id).await?;
    let profile = saved.lock().await.profile_id.clone();
    let _profile_guard = state
        .entries
        .lock_local_task(&format!("provider-auth:{profile}"))
        .await;
    let current = find(state, id).await?;
    if !Arc::ptr_eq(&current, &saved) {
        return Err(failure(StatusCode::GONE, "Sign-in was replaced"));
    }
    let mut attempt = saved.lock().await;
    if attempt.completed {
        return Ok(json!({"status":"completed","profile_id":attempt.profile_id}));
    }
    if attempt.expires <= Instant::now() {
        state.provider_auth.attempts.lock().await.remove(id);
        return Err(failure(StatusCode::GONE, "Sign-in expired; start again"));
    }
    if let Some(callback) = body.callback {
        if !matches!(
            attempt.pending.provider.as_str(),
            "anthropic" | "openrouter"
        ) || callback.len() > 8192
        {
            return Err(failure(
                StatusCode::BAD_REQUEST,
                "Unexpected browser callback",
            ));
        }
        attempt.pending.user_code = callback;
    }
    if attempt.next_poll > Instant::now() {
        return Ok(
            json!({"status":"pending","retry_after_seconds":attempt.next_poll.saturating_duration_since(Instant::now()).as_secs().max(1)}),
        );
    }
    let provider =
        zork_profile::providers::get(&attempt.pending.provider).expect("validated provider");
    match provider
        .poll_device_code(&state.provider_auth.http, &attempt.pending)
        .await
    {
        Ok(DeviceCodePoll::Pending {
            retry_after_seconds,
        }) => {
            let retry = retry_after_seconds.max(1);
            attempt.next_poll = Instant::now() + Duration::from_secs(retry);
            Ok(json!({"status":"pending","retry_after_seconds":retry}))
        }
        Ok(DeviceCodePoll::Completed { auth }) => {
            let template = provider
                .template(&attempt.pending.billing)
                .map_err(|_| failure(StatusCode::BAD_REQUEST, "Provider template unavailable"))?;
            let mut document = template.document(auth);
            document["models"] =
                match crate::agent::get_profile(&state.agent, &attempt.profile_id).await {
                    Ok(existing)
                        if existing["provider"] == attempt.pending.provider
                            && existing["billing"] == attempt.pending.billing =>
                    {
                        existing["models"].clone()
                    }
                    _ => json!([]),
                };
            provider.decorate_document(&mut document);
            crate::agent::put_profile(&state.agent, &attempt.profile_id, &document)
                .await
                .map_err(|_| Failure {
                    effects_unconfirmed: true,
                    ..failure(
                        StatusCode::BAD_GATEWAY,
                        "Could not confirm the authorized profile was saved",
                    )
                })?;
            attempt.completed = true;
            attempt.pending = DeviceCode::default();
            Ok(json!({"status":"completed","profile_id":attempt.profile_id}))
        }
        Err(_) => {
            state.provider_auth.attempts.lock().await.remove(id);
            Err(failure(
                StatusCode::BAD_GATEWAY,
                "Authorization was rejected or expired; start again",
            ))
        }
    }
}

pub(crate) async fn cancel(state: &AppState, id: &str) {
    if let Ok(saved) = find(state, id).await {
        let profile = saved.lock().await.profile_id.clone();
        let _guard = state
            .entries
            .lock_local_task(&format!("provider-auth:{profile}"))
            .await;
        state.provider_auth.attempts.lock().await.remove(id);
    }
}
