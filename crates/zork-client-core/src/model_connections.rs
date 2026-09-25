//! Model connections across every saved device, for clients that manage them
//! globally. Each device carries its own load state: a device that could not
//! be read keeps its cached connections and reports why, never "none".
//!
//! The same provider account saved on several devices is one connection: rows
//! with equal `(provider, account_key)` merge, and each device stays a source
//! that is still managed on its own. Rows without an identity never merge.
//!
//! A connection is titled by its account (see [`connection_title`]); a Profile
//! name only shows when the user set one.
use crate::{api::ProfileInfo, settings, store::ClientStore};
use anyhow::Result;
use serde_json::{json, Value};

/// One device's copy of a connection.
pub struct AccountSource<'a> {
    pub profile: &'a ProfileInfo,
    /// The device's latest quota refresh for this profile failed.
    pub quota_failed: bool,
}

/// One account as shown in the list.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AccountEntry {
    /// Indices into the input, in input order; the first names the entry.
    pub sources: Vec<usize>,
    /// The source whose quota and status represent the account: the freshest
    /// successful sample, else the freshest sample.
    pub primary: usize,
    /// Distinct model ids across every source.
    pub models: usize,
}

/// Groups device rows by account, keeping input order. A failed sample never
/// represents an account that has a successful one, however recent.
pub fn merge_accounts(sources: &[AccountSource<'_>]) -> Vec<AccountEntry> {
    let mut entries: Vec<AccountEntry> = vec![];
    let mut keys: std::collections::HashMap<(&str, &str), usize> = Default::default();
    for (index, source) in sources.iter().enumerate() {
        let key = source
            .profile
            .account_key
            .as_deref()
            .filter(|key| !key.is_empty())
            .map(|key| (source.profile.provider.as_str(), key));
        match key.and_then(|key| keys.get(&key).copied()) {
            Some(entry) => entries[entry].sources.push(index),
            None => {
                if let Some(key) = key {
                    keys.insert(key, entries.len());
                }
                entries.push(AccountEntry {
                    sources: vec![index],
                    primary: index,
                    models: 0,
                });
            }
        }
    }
    for entry in &mut entries {
        entry.primary = entry
            .sources
            .iter()
            .copied()
            // `max_by_key` keeps the last maximum; reverse so ties keep the first.
            .rev()
            .max_by_key(|&index| sample(&sources[index]))
            .unwrap_or(entry.primary);
        entry.models = entry
            .sources
            .iter()
            .flat_map(|&index| sources[index].profile.models.iter().map(|m| m.id.as_str()))
            .collect::<std::collections::HashSet<_>>()
            .len();
    }
    entries
}

/// How a connection is titled on every client.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize)]
pub struct ConnectionTitle {
    /// The account label; for an API key the access plus its tail
    /// ("OpenCode Go 订阅 · ···a1b2"); else the access label. Never empty.
    pub title: String,
    /// A name the user set explicitly, shown secondary and muted. Generated
    /// names (equal to the provider, access or id defaults) are never shown.
    pub name: Option<String>,
}

/// Prefix of an API key's label; the rest is its last four characters.
const KEY_TAIL: &str = "···";

/// The title of one device's connection. `providers` is that device's catalog;
/// without it the title falls back to the provider id.
pub fn connection_title(profile: &ProfileInfo, providers: &[Value]) -> ConnectionTitle {
    let text = |value: &Value| {
        value
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
    };
    let provider = providers
        .iter()
        .find(|p| p["id"] == profile.provider.as_str());
    let provider_label = provider.and_then(|p| text(&p["label"]));
    let billings: Vec<&Value> = provider
        .and_then(|p| p["billing"].as_array())
        .map(|items| items.iter().collect())
        .unwrap_or_default();
    let billing_label = billings
        .iter()
        .find(|b| b["id"].as_str() == profile.billing.as_deref())
        .and_then(|b| text(&b["label"]));
    let access = billing_label
        .clone()
        .or_else(|| provider_label.clone())
        .or_else(|| Some(profile.provider.trim().to_owned()).filter(|s| !s.is_empty()))
        .or_else(|| Some(profile.profile_id.trim().to_owned()).filter(|s| !s.is_empty()))
        .unwrap_or_else(|| "模型连接".to_owned());
    let label = profile
        .account_label
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty() && *s != KEY_TAIL);
    let title = match label {
        Some(tail) if tail.starts_with(KEY_TAIL) => format!("{access} · {tail}"),
        Some(label) => label.to_owned(),
        None => access.clone(),
    };
    // Every name a client or an older version generated for this connection.
    let mut defaults = vec![
        profile.profile_id.clone(),
        generated_id_base(&profile.profile_id).to_owned(),
        profile.provider.clone(),
        access,
        title.clone(),
    ];
    defaults.extend(label.map(str::to_owned));
    defaults.extend(provider_label.clone());
    for billing in &billings {
        if let Some(label) = text(&billing["label"]) {
            if let Some(provider) = &provider_label {
                defaults.push(format!("{provider} {label}"));
            }
            defaults.push(label);
        }
    }
    let defaults: Vec<String> = defaults.iter().map(|d| name_key(d)).collect();
    let name = profile
        .name
        .as_deref()
        .map(str::trim)
        .filter(|name| !name.is_empty() && !defaults.contains(&name_key(name)))
        .map(str::to_owned);
    ConnectionTitle { title, name }
}

/// The title of one account saved on several devices: the first source that
/// knows the account names it, so every copy reads the same.
pub fn account_title<'a>(
    sources: impl IntoIterator<Item = (&'a ProfileInfo, &'a [Value])>,
) -> ConnectionTitle {
    let titles: Vec<_> = sources
        .into_iter()
        .map(|(profile, providers)| {
            let known = profile
                .account_label
                .as_deref()
                .is_some_and(|l| !l.trim().is_empty());
            (known, connection_title(profile, providers))
        })
        .collect();
    let title = titles
        .iter()
        .find(|(known, _)| *known)
        .or(titles.first())
        .map(|(_, t)| t.title.clone())
        .unwrap_or_else(|| "模型连接".to_owned());
    let name = titles.iter().find_map(|(_, t)| t.name.clone());
    ConnectionTitle { title, name }
}

/// `opencode-go-2` came from `opencode-go`: clients number repeated ids.
fn generated_id_base(id: &str) -> &str {
    match id.rsplit_once('-') {
        Some((base, n)) if !base.is_empty() && n.parse::<u32>().is_ok() => base,
        _ => id,
    }
}

/// Names compare without case, spaces or punctuation: "OpenCode-Go" is the
/// generated "opencode-go".
fn name_key(name: &str) -> String {
    name.chars()
        .filter(|c| c.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

/// Adds core's `title` and `custom_name` to each profile of one device.
pub(crate) fn present_titles(profiles: &mut [Value], providers: &[Value]) {
    for profile in profiles {
        let Ok(parsed) = serde_json::from_value::<ProfileInfo>(profile.clone()) else {
            continue;
        };
        let title = connection_title(&parsed, providers);
        profile["title"] = json!(title.title);
        profile["custom_name"] = json!(title.name);
    }
}

/// Orders samples: any success above any failure, then by sampling time.
fn sample(source: &AccountSource<'_>) -> (bool, i64) {
    let profile = source.profile;
    let at = profile
        .checked_at
        .as_deref()
        .and_then(|at| chrono::DateTime::parse_from_rfc3339(at).ok())
        .map(|at| at.timestamp_millis());
    let quota = profile.quota();
    let reported = !matches!(profile.rate_limits["error"].as_str(), Some("not_probed"))
        && !profile.rate_limits.is_null();
    let success = at.is_some() && reported && !source.quota_failed && !quota.failed;
    (success, at.unwrap_or(i64::MIN))
}

/// The JSON form of [`merge_accounts`] over every device's projection.
fn accounts(devices: &[Value]) -> Value {
    let mut rows = vec![];
    for device in devices {
        for profile in device["profiles"].as_array().into_iter().flatten() {
            let parsed = serde_json::from_value::<ProfileInfo>(profile.clone()).ok();
            rows.push((device, profile, parsed));
        }
    }
    // A profile core cannot parse stays its own entry.
    let placeholder = ProfileInfo {
        profile_id: String::new(),
        name: None,
        provider: String::new(),
        billing: None,
        verified: false,
        account_key: None,
        account_label: None,
        account: Value::Null,
        rate_limits: Value::Null,
        checked_at: None,
        models: vec![],
        extra: Default::default(),
    };
    let sources: Vec<_> = rows
        .iter()
        .map(|(_, _, parsed)| AccountSource {
            profile: parsed.as_ref().unwrap_or(&placeholder),
            quota_failed: false,
        })
        .collect();
    let entries = merge_accounts(&sources);
    Value::Array(
        entries
            .into_iter()
            .map(|entry| {
                let (device, profile, _) = rows[entry.primary];
                let title = account_title(entry.sources.iter().map(|&index| {
                    let (device, _, _) = rows[index];
                    (
                        sources[index].profile,
                        device["providers"].as_array().map(Vec::as_slice).unwrap_or(&[]),
                    )
                }));
                let id = match sources[entry.primary].profile.account_key.as_deref() {
                    Some(key) if !key.is_empty() => format!("account:{key}"),
                    _ => format!("profile:{}/{}", device["peer"].as_str().unwrap_or_default(),
                        profile["profile_id"].as_str().unwrap_or_default()),
                };
                json!({
                    "id": id,
                    "provider": profile["provider"],
                    "name": title.title,
                    "title": title.title,
                    "custom_name": title.name,
                    "peer": device["peer"],
                    "profile": profile,
                    "models": entry.models,
                    "sources": entry.sources.iter().map(|&index| {
                        let (device, profile, _) = rows[index];
                        json!({"peer": device["peer"], "device": device["name"], "profile": profile})
                    }).collect::<Vec<_>>(),
                })
            })
            .collect(),
    )
}

pub(crate) const LOADED_AT: &str = "settings-loaded-at";

/// One device's projection. `refresh_error` is the failure of the read that
/// produced `snapshot`, if any; a cache-only read passes `None`.
pub(crate) fn device(
    peer: &str,
    name: &str,
    snapshot: &Value,
    refresh_error: Option<&str>,
    loaded_at_ms: Option<u64>,
) -> Value {
    let text = |value: &Value| value.as_str().filter(|s| !s.is_empty()).map(str::to_owned);
    if snapshot["revoked"] == true {
        return json!({"peer":peer,"name":name,"state":"revoked","error":"设备访问权限已撤销",
            "cached":false,"loaded_at_ms":null,"profiles":[],"providers":[]});
    }
    let ready = snapshot["ready"] == true;
    let error = refresh_error
        .map(str::to_owned)
        .or_else(|| text(&snapshot["profiles_error"]));
    let state = match (&error, ready) {
        (Some(_), _) => "failed",
        (None, false) => "loading",
        (None, true) if snapshot["profiles_ready"] == false => "loading",
        (None, true) => "ready",
    };
    let list = |key: &str| match &snapshot[key] {
        Value::Array(items) if ready => Value::Array(items.clone()),
        _ => json!([]),
    };
    json!({"peer":peer,"name":name,"state":state,"error":error,
        "cached":ready && error.is_some(),
        "loaded_at_ms":loaded_at_ms,
        "profiles":list("profiles"),
        "providers":list("providers")})
}

fn loaded_at(store: &ClientStore, peer: &str) -> Result<Option<u64>> {
    store.get(peer, LOADED_AT)
}

/// Every saved device's committed connections, without network I/O.
pub(crate) fn cached(store: &ClientStore) -> Result<Value> {
    let mut devices = vec![];
    for node in store.nodes()? {
        let snapshot = settings::cached(store, &node.id)?;
        devices.push(device(
            &node.id,
            &node.name,
            &snapshot,
            None,
            loaded_at(store, &node.id)?,
        ));
    }
    Ok(json!({"accounts":accounts(&devices),"devices":devices}))
}

/// Reads every device concurrently. One device's failure only marks that
/// device; the others still complete.
pub(crate) async fn refresh(
    store: std::sync::Arc<ClientStore>,
    station: impl Fn(&str) -> Result<std::sync::Arc<crate::api::StationClient>>,
) -> Result<Value> {
    let nodes = store.nodes()?;
    let reads = nodes.iter().map(|node| {
        let client = station(&node.id);
        let store = store.clone();
        async move {
            let refreshed = match client {
                Ok(client) => settings::refresh(client, store.clone(), &node.id).await,
                Err(error) => Err(error),
            };
            let (snapshot, error) = match refreshed {
                // A refresh reports its own failure while keeping the cache.
                Ok(snapshot) if snapshot["cached"] == false => (snapshot, None),
                Ok(snapshot) => {
                    let error = snapshot["error"]
                        .as_str()
                        .unwrap_or("设备暂时无法连接")
                        .to_owned();
                    (snapshot, Some(error))
                }
                Err(error) => (settings::cached(&store, &node.id)?, Some(error.to_string())),
            };
            Ok::<_, anyhow::Error>(device(
                &node.id,
                &node.name,
                &snapshot,
                error.as_deref(),
                loaded_at(&store, &node.id)?,
            ))
        }
    });
    let devices = futures_util::future::join_all(reads)
        .await
        .into_iter()
        .collect::<Result<Vec<_>>>()?;
    Ok(json!({"accounts":accounts(&devices),"devices":devices}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::SavedNode;

    fn node(id: &str, name: &str) -> SavedNode {
        SavedNode {
            machine_name: None,
            color_key: None,
            id: id.into(),
            name: name.into(),
            url: String::new(),
            token: None,
            local: false,
            mesh: None,
            group: None,
        }
    }

    fn profile(
        provider: &str,
        key: Option<&str>,
        at: Option<&str>,
        ok: bool,
        models: &[&str],
    ) -> ProfileInfo {
        serde_json::from_value(json!({
            "profile_id": "p", "provider": provider, "account_key": key, "checkedAt": at,
            "rateLimits": if ok { json!({"ok":true,"rateLimits":{"primary":{"usedPercent":10}}}) }
                else { json!({"ok":false,"error":"usage_query_failed"}) },
            "models": models.iter().map(|id| json!({"id": id})).collect::<Vec<_>>(),
        }))
        .unwrap()
    }
    fn merge(profiles: &[ProfileInfo]) -> Vec<AccountEntry> {
        let sources: Vec<_> = profiles
            .iter()
            .map(|profile| AccountSource {
                profile,
                quota_failed: false,
            })
            .collect();
        merge_accounts(&sources)
    }

    #[test]
    fn same_account_on_two_devices_is_one_entry_with_union_models() {
        let at = Some("2026-09-26T00:00:00Z");
        let entries = merge(&[
            profile(
                "opencode-go",
                Some("opencode-go:k:1"),
                at,
                true,
                &["a", "b"],
            ),
            profile("openai", Some("openai:a:9"), at, true, &[]),
            profile(
                "opencode-go",
                Some("opencode-go:k:1"),
                at,
                true,
                &["b", "c"],
            ),
        ]);
        assert_eq!(entries.len(), 2);
        assert_eq!(entries[0].sources, vec![0, 2]);
        assert_eq!(entries[0].models, 3);
        assert_eq!(entries[0].primary, 0, "ties keep the first device");
        assert_eq!(entries[1].sources, vec![1]);
    }

    #[test]
    fn rows_without_an_identity_or_with_another_provider_never_merge() {
        let at = Some("2026-09-26T00:00:00Z");
        let entries = merge(&[
            profile("opencode-go", None, at, true, &[]),
            profile("opencode-go", None, at, true, &[]),
            profile("opencode-go", Some(""), at, true, &[]),
            profile("deepseek", Some("same"), at, true, &[]),
            profile("kimi-coding", Some("same"), at, true, &[]),
        ]);
        assert_eq!(entries.len(), 5);
    }

    #[test]
    fn freshest_success_represents_the_account_and_failures_never_override_it() {
        let key = Some("xai:a:1");
        let old = profile("xai", key, Some("2026-09-26T00:00:00Z"), true, &[]);
        let new = profile("xai", key, Some("2026-09-26T01:00:00Z"), true, &[]);
        let failed_newest = profile("xai", key, Some("2026-09-26T02:00:00Z"), false, &[]);
        let never = profile("xai", key, None, true, &[]);
        assert_eq!(merge(&[old.clone(), new.clone()])[0].primary, 1);
        assert_eq!(merge(&[new.clone(), old.clone()])[0].primary, 0);
        assert_eq!(
            merge(&[failed_newest.clone(), old.clone(), never.clone()])[0].primary,
            1
        );
        // A device whose own refresh failed does not stand for the account either.
        let sources = [
            AccountSource {
                profile: &new,
                quota_failed: true,
            },
            AccountSource {
                profile: &old,
                quota_failed: false,
            },
        ];
        assert_eq!(merge_accounts(&sources)[0].primary, 1);
        // With no success at all, the freshest failure is shown.
        let older_failure = profile("xai", key, Some("2026-09-25T00:00:00Z"), false, &[]);
        assert_eq!(merge(&[older_failure, failed_newest])[0].primary, 1);
    }

    #[test]
    fn json_accounts_carry_every_device_source() {
        let devices = [
            device(
                "a",
                "A",
                &json!({"ready":true,"profiles_ready":true,"providers":[],
                "profiles":[{"profile_id":"go","name":"OpenCode-Go","provider":"opencode-go","account_key":"opencode-go:k:1",
                    "checkedAt":"2026-09-26T00:00:00Z","rateLimits":{"ok":true}}]}),
                None,
                None,
            ),
            device(
                "b",
                "B",
                &json!({"ready":true,"profiles_ready":true,"providers":[],
                "profiles":[{"profile_id":"go2","provider":"opencode-go","account_key":"opencode-go:k:1",
                    "checkedAt":"2026-09-26T01:00:00Z","rateLimits":{"ok":true}},
                    {"profile_id":"solo","provider":"opencode-go"}]}),
                None,
                None,
            ),
        ];
        let accounts = accounts(&devices);
        let accounts = accounts.as_array().unwrap();
        assert_eq!(accounts.len(), 2);
        // "OpenCode-Go" is the generated default, so the account titles the card.
        assert_eq!(accounts[0]["name"], "opencode-go");
        assert_eq!(accounts[0]["title"], "opencode-go");
        assert_eq!(accounts[0]["custom_name"], Value::Null);
        assert_eq!(accounts[0]["peer"], "b");
        assert_eq!(accounts[0]["profile"]["profile_id"], "go2");
        let devices: Vec<_> = accounts[0]["sources"]
            .as_array()
            .unwrap()
            .iter()
            .map(|s| s["device"].clone())
            .collect();
        assert_eq!(devices, vec![json!("A"), json!("B")]);
        assert_eq!(accounts[1]["sources"].as_array().unwrap().len(), 1);
        assert_eq!(accounts[1]["id"], "profile:b/solo");
    }

    fn catalog() -> Vec<Value> {
        vec![
            json!({"id":"opencode-go","label":"OpenCode Go",
                "billing":[{"id":"subscription","label":"OpenCode Go 订阅"}]}),
            json!({"id":"anthropic","label":"Anthropic","billing":[
                {"id":"usage","label":"API 按量"},{"id":"subscription","label":"Claude 订阅 (Pro/Max)"}]}),
        ]
    }

    fn titled(
        provider: &str,
        billing: &str,
        id: &str,
        name: Option<&str>,
        label: Option<&str>,
    ) -> ProfileInfo {
        serde_json::from_value(
            json!({"profile_id": id, "provider": provider, "billing": billing,
            "name": name, "account_label": label}),
        )
        .unwrap()
    }

    #[test]
    fn title_is_the_account_and_api_keys_keep_the_access_wording() {
        let providers = catalog();
        let login = titled(
            "anthropic",
            "subscription",
            "anthropic",
            None,
            Some("me@example.test"),
        );
        assert_eq!(
            connection_title(&login, &providers),
            ConnectionTitle {
                title: "me@example.test".into(),
                name: None
            }
        );
        let key = titled(
            "opencode-go",
            "subscription",
            "opencode-go",
            None,
            Some("···a1b2"),
        );
        assert_eq!(
            connection_title(&key, &providers).title,
            "OpenCode Go 订阅 · ···a1b2"
        );
    }

    #[test]
    fn only_a_name_the_user_set_shows_and_generated_names_never_do() {
        let providers = catalog();
        let custom = titled(
            "opencode-go",
            "subscription",
            "opencode-go",
            Some(" 工作 "),
            Some("···a1b2"),
        );
        assert_eq!(
            connection_title(&custom, &providers).name.as_deref(),
            Some("工作")
        );
        for generated in [
            "OpenCode-Go",
            "opencode-go",
            "OpenCode Go",
            "OpenCode Go 订阅",
            "opencode-go-2",
            "  ",
            "···a1b2",
        ] {
            let profile = titled(
                "opencode-go",
                "subscription",
                "opencode-go-2",
                Some(generated),
                Some("···a1b2"),
            );
            assert_eq!(
                connection_title(&profile, &providers).name,
                None,
                "{generated}"
            );
        }
        // Without a catalog the provider id and profile id still count as generated.
        let bare = titled(
            "opencode-go",
            "subscription",
            "go",
            Some("OpenCode-Go"),
            None,
        );
        assert_eq!(connection_title(&bare, &[]).name, None);
        let email = titled(
            "anthropic",
            "subscription",
            "claude",
            Some("Me@Example.test"),
            Some("me@example.test"),
        );
        assert_eq!(
            connection_title(&email, &providers).name,
            None,
            "repeats the title"
        );
    }

    #[test]
    fn unknown_accounts_fall_back_to_the_access_label_and_are_never_empty() {
        let providers = catalog();
        let unknown = titled(
            "anthropic",
            "subscription",
            "anthropic",
            Some("Claude"),
            None,
        );
        let title = connection_title(&unknown, &providers);
        assert_eq!(title.title, "Claude 订阅 (Pro/Max)");
        assert_eq!(title.name.as_deref(), Some("Claude"));
        assert_eq!(connection_title(&unknown, &[]).title, "anthropic");
        let blank = titled("", "", "", None, Some("  "));
        assert_eq!(connection_title(&blank, &[]).title, "模型连接");
    }

    #[test]
    fn merged_accounts_share_one_title_from_the_source_that_knows_the_account() {
        let providers = catalog();
        let older = titled(
            "opencode-go",
            "subscription",
            "opencode-go",
            Some("OpenCode-Go"),
            None,
        );
        let newer = titled(
            "opencode-go",
            "subscription",
            "go-2",
            Some("家里"),
            Some("···a1b2"),
        );
        let title = account_title([
            (&older, providers.as_slice()),
            (&newer, providers.as_slice()),
        ]);
        assert_eq!(title.title, "OpenCode Go 订阅 · ···a1b2");
        assert_eq!(title.name.as_deref(), Some("家里"));
        let devices = [
            device(
                "a",
                "A",
                &json!({"ready":true,"profiles_ready":true,"providers":providers,
                "profiles":[{"profile_id":"opencode-go","name":"OpenCode-Go","provider":"opencode-go",
                    "billing":"subscription","account_key":"opencode-go:k:1","account_label":"···a1b2"}]}),
                None,
                None,
            ),
            device(
                "b",
                "B",
                &json!({"ready":true,"profiles_ready":true,"providers":providers,
                "profiles":[{"profile_id":"opencode-go","provider":"opencode-go","billing":"subscription",
                    "account_key":"opencode-go:k:1","account_label":"···a1b2"}]}),
                None,
                None,
            ),
        ];
        let accounts = accounts(&devices);
        assert_eq!(accounts[0]["title"], "OpenCode Go 订阅 · ···a1b2");
        assert_eq!(accounts[0]["custom_name"], Value::Null);
        assert_eq!(accounts[0]["sources"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn device_profiles_carry_core_titles() {
        let mut profiles = vec![json!({"profile_id":"opencode-go","provider":"opencode-go",
            "billing":"subscription","name":"OpenCode-Go","account_label":"···a1b2"})];
        present_titles(&mut profiles, &catalog());
        assert_eq!(profiles[0]["title"], "OpenCode Go 订阅 · ···a1b2");
        assert_eq!(profiles[0]["custom_name"], Value::Null);
    }

    #[test]
    fn failed_read_keeps_cached_connections_and_reports_the_reason() {
        let snapshot = json!({"ready":true,"profiles":[{"profile_id":"p"}],"providers":[],"profiles_ready":true});
        let entry = device("a", "Studio", &snapshot, Some("连接超时"), Some(42));
        assert_eq!(entry["state"], "failed");
        assert_eq!(entry["error"], "连接超时");
        assert_eq!(entry["cached"], true);
        assert_eq!(entry["loaded_at_ms"], 42);
        assert_eq!(entry["profiles"][0]["profile_id"], "p");
    }

    #[test]
    fn never_loaded_device_is_loading_not_empty_and_failure_without_cache_is_not_cached() {
        let loading = device("a", "A", &json!({"ready":false}), None, None);
        assert_eq!(loading["state"], "loading");
        let failed = device("a", "A", &json!({"ready":false}), Some("离线"), None);
        assert_eq!(failed["state"], "failed");
        assert_eq!(failed["cached"], false);
        assert_eq!(failed["profiles"], json!([]));
    }

    #[test]
    fn profile_catalog_state_and_revocation_are_distinct() {
        let pending = json!({"ready":true,"profiles":[],"providers":[],"profiles_ready":false});
        assert_eq!(device("a", "A", &pending, None, None)["state"], "loading");
        let broken = json!({"ready":true,"profiles":[],"providers":[],"profiles_ready":true,"profiles_error":"boom"});
        assert_eq!(device("a", "A", &broken, None, None)["state"], "failed");
        let ready = json!({"ready":true,"profiles":[],"providers":[],"profiles_ready":true,"profiles_error":null});
        assert_eq!(device("a", "A", &ready, None, None)["state"], "ready");
        let revoked = device(
            "a",
            "A",
            &json!({"ready":false,"revoked":true}),
            None,
            Some(1),
        );
        assert_eq!(revoked["state"], "revoked");
        assert_eq!(revoked["profiles"], json!([]));
    }

    #[test]
    fn cached_lists_every_saved_device_with_its_own_state() {
        let root = tempfile::tempdir().unwrap();
        let store = ClientStore::open(root.path()).unwrap();
        store.save_node(&node("a", "Studio")).unwrap();
        store.save_node(&node("b", "mini1")).unwrap();
        store
            .put("a", "public-settings", &json!({"ready":true,"cached":true,"info":{},"agents":[],
                "profiles":[{"profile_id":"p","provider":"anthropic","verified":true}],"providers":[]}))
            .unwrap();
        store.put("a", LOADED_AT, &7u64).unwrap();
        let result = cached(&store).unwrap();
        let devices = result["devices"].as_array().unwrap();
        assert_eq!(devices.len(), 2);
        assert_eq!(devices[0]["name"], "Studio");
        assert_eq!(devices[0]["state"], "ready");
        assert_eq!(devices[0]["loaded_at_ms"], 7);
        assert_eq!(devices[0]["profiles"][0]["verified"], true);
        assert!(devices[0]["profiles"][0]["quota"].is_object());
        assert_eq!(devices[1]["state"], "loading");
        assert_eq!(devices[1]["profiles"], json!([]));
    }
}
