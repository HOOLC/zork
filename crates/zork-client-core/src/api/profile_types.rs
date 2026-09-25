use serde::{Deserialize, Serialize};

/// One item of `/v1/profiles`.
#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ProfileInfo {
    pub profile_id: String,
    #[serde(default)]
    pub name: Option<String>,
    pub provider: String,
    #[serde(default)]
    pub billing: Option<String>,
    #[serde(default)]
    pub verified: bool,
    /// Secret-free identity of the provider account, equal on every device
    /// that holds the same login or key. Absent when unknown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub account_key: Option<String>,
    #[serde(default)]
    pub account: serde_json::Value,
    #[serde(default, rename = "rateLimits")]
    pub rate_limits: serde_json::Value,
    #[serde(default, rename = "checkedAt")]
    pub checked_at: Option<String>,
    #[serde(default)]
    pub models: Vec<ProfileModel>,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

#[derive(Clone, Debug, PartialEq, Deserialize, Serialize)]
pub struct ProfileModel {
    pub id: String,
    #[serde(default = "default_model_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub thinking: Vec<String>,
    #[serde(default)]
    pub default_thinking: String,
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

fn default_model_enabled() -> bool {
    true
}

/// Normalized quota data. Missing data is distinct from a failed query.
#[derive(Clone, Debug, Default, PartialEq, serde::Serialize)]
pub struct ProfileQuota {
    pub failed: bool,
    pub windows: Vec<QuotaWindow>,
    pub balance: Option<(f64, String)>,
}
#[derive(Clone, Debug, PartialEq, serde::Serialize)]
pub struct QuotaWindow {
    pub name: String,
    pub minutes: Option<u64>,
    pub remaining: f64,
    pub resets_at: Option<i64>,
}
impl ProfileInfo {
    pub fn quota_stale_at(&self, now: i64) -> bool {
        self.checked_at
            .as_deref()
            .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
            .is_none_or(|date| now.saturating_sub(date.timestamp()) > 30 * 60)
    }
    pub fn display_name(&self) -> &str {
        self.name
            .as_deref()
            .filter(|name| !name.trim().is_empty())
            .unwrap_or(&self.profile_id)
    }
    pub fn is_verified(&self) -> bool {
        self.account
            .get("verified")
            .and_then(serde_json::Value::as_bool)
            .or_else(|| self.account.get("ok").and_then(serde_json::Value::as_bool))
            .unwrap_or(self.verified)
    }
    pub fn quota(&self) -> ProfileQuota {
        ProfileQuota::from_value(&self.rate_limits)
    }
}
impl ProfileQuota {
    pub fn from_value(value: &serde_json::Value) -> Self {
        use serde_json::Value;
        if matches!(
            value["error"].as_str(),
            Some("not_probed" | "not_reported_by_provider")
        ) || value["reported"] == false
        {
            return Self::default();
        }
        if value["ok"] == false {
            return Self {
                failed: true,
                ..Self::default()
            };
        }
        let mut result = Self::default();
        let main = &value["rateLimits"];
        let mut add = |limits: &Value, name: &str| {
            for key in ["primary", "secondary"] {
                let w = &limits[key];
                if let Some(used) = w["usedPercent"]
                    .as_f64()
                    .filter(|v| v.is_finite() && *v >= 0.)
                {
                    result.windows.push(QuotaWindow {
                        name: name.into(),
                        minutes: w["windowDurationMins"].as_u64().filter(|v| *v > 0),
                        remaining: (100. - used).clamp(0., 100.),
                        resets_at: w["resetsAt"].as_i64().filter(|v| *v > 0),
                    });
                }
            }
        };
        add(main, "");
        for (id, limits) in value["rateLimitsByLimitId"]
            .as_object()
            .into_iter()
            .flatten()
        {
            if main["limitId"].as_str() == Some(id) || main == limits {
                continue;
            }
            add(limits, limits["limitName"].as_str().unwrap_or(id));
        }
        let credits = &main["credits"];
        // Older servers used unlimited=true merely to mean an API key was valid.
        // Only render an actual numeric balance with an explicit unit.
        if let Some(unit) = credits["unit"].as_str().filter(|v| !v.is_empty()) {
            if let Some(balance) = credits["balance"]
                .as_f64()
                .or_else(|| credits["balance"].as_str()?.parse().ok())
                .filter(|v| v.is_finite() && *v >= 0.)
            {
                result.balance = Some((balance, unit.into()));
            }
        }
        result
    }
}

#[cfg(test)]
mod quota_tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn absent_unreported_and_legacy_unlimited_are_hidden() {
        for value in [
            json!(null),
            json!({"ok":false,"error":"not_probed"}),
            json!({"ok":true,"reported":false}),
            json!({"ok":true,"rateLimits":{"credits":{"unlimited":true,"balance":null}}}),
        ] {
            assert_eq!(ProfileQuota::from_value(&value), ProfileQuota::default());
        }
        assert!(
            ProfileQuota::from_value(&json!({"ok":false,"error":"private provider error"})).failed
        );
    }
    #[test]
    fn zero_balance_and_exhausted_windows_are_visible_and_duplicates_are_removed() {
        let main = json!({"limitId":"xai","secondary":{"usedPercent":120,"windowDurationMins":10080,"resetsAt":1800000000},"credits":{"balance":"0.00","unit":"USD"}});
        let quota = ProfileQuota::from_value(
            &json!({"ok":true,"rateLimits":main,"rateLimitsByLimitId":{"xai":main}}),
        );
        assert_eq!(quota.windows.len(), 1);
        assert_eq!(quota.windows[0].remaining, 0.);
        assert_eq!(quota.balance, Some((0., "USD".into())));
    }
    #[test]
    fn account_status_controls_verification_and_old_profiles_still_load() {
        let profile: ProfileInfo = serde_json::from_value(json!({"profile_id":"test","provider":"openai","account":{"ok":true},"checkedAt":"2026-09-08T00:00:00Z"})).unwrap();
        assert!(profile.is_verified());
        assert_eq!(profile.checked_at.as_deref(), Some("2026-09-08T00:00:00Z"));
        assert_eq!(profile.quota(), ProfileQuota::default());
    }
}
