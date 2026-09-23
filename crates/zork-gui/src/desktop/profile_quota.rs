//! Cached presentation for profile usage; render never parses provider JSON or dates.
use crate::{api::ProfileInfo, i18n::Locale};

#[derive(Clone, Debug, Default)]
pub(crate) struct QuotaPresentation {
    pub summary: String,
    pub failed: bool,
    pub windows: Vec<WindowPresentation>,
    pub balance: Option<String>,
    pub checked: Option<String>,
}
#[derive(Clone, Debug)]
pub(crate) struct WindowPresentation {
    pub label: String,
    pub short_label: String,
    pub remaining: f32,
    pub center_value: String,
    pub value: String,
    pub reset: Option<String>,
}
impl QuotaPresentation {
    pub fn new(profile: &ProfileInfo, locale: Locale) -> Self {
        Self::new_at(profile, locale, chrono::Utc::now().timestamp())
    }
    pub fn new_at(profile: &ProfileInfo, locale: Locale, now: i64) -> Self {
        let quota = profile.quota();
        if quota.failed {
            return Self::failure(locale);
        }
        let windows: Vec<_> = quota
            .windows
            .into_iter()
            .map(|window| {
                let duration = match window.minutes {
                    Some(minutes) if minutes % 1440 == 0 => {
                        format!("{}{}", minutes / 1440, locale.text("quota_days"))
                    }
                    Some(minutes) if minutes % 60 == 0 => {
                        format!("{}{}", minutes / 60, locale.text("quota_hours"))
                    }
                    Some(minutes) => format!("{minutes}{}", locale.text("quota_minutes")),
                    None => locale.text("quota_window").into(),
                };
                let label = if window.name.is_empty() {
                    duration
                } else {
                    format!("{} · {duration}", window.name)
                };
                let short_label = match window.minutes {
                    Some(minutes) if minutes % 1440 == 0 => format!("{}D", minutes / 1440),
                    Some(minutes) if minutes % 60 == 0 => format!("{}H", minutes / 60),
                    Some(minutes) => format!("{minutes}M"),
                    None => "—".into(),
                };
                // Never round a partially consumed window back up to 100%.
                let percent = if window.remaining > 0. && window.remaining < 1. {
                    "<1".to_owned()
                } else {
                    (window.remaining.floor() as u32).to_string()
                };
                let value = format!("{} {percent}%", locale.text("quota_remaining"));
                let reset = window.resets_at.map(|time| {
                    let seconds = time.saturating_sub(now);
                    if seconds < 60 {
                        locale.text("quota_reset_pending").to_owned()
                    } else {
                        locale
                            .text("quota_resets")
                            .replace("{time}", &relative_duration(seconds, locale))
                    }
                });
                WindowPresentation {
                    label,
                    short_label,
                    remaining: window.remaining as f32,
                    center_value: if window.remaining >= 100. {
                        String::new()
                    } else {
                        percent.clone()
                    },
                    value,
                    reset,
                }
            })
            .collect();
        let balance = quota.balance.map(|(amount, unit)| {
            format!(
                "{} {amount:.2} {}",
                locale.text("quota_balance"),
                if unit == "credits" {
                    locale.text("quota_credits")
                } else {
                    &unit
                }
            )
        });
        let summary = windows
            .iter()
            .map(|w| format!("{} {}", w.label, w.value))
            .chain(balance.iter().cloned())
            .collect::<Vec<_>>()
            .join(" · ");
        let checked = if summary.is_empty() {
            None
        } else {
            profile
                .checked_at
                .as_deref()
                .and_then(|s| chrono::DateTime::parse_from_rfc3339(s).ok())
                .map(|date| {
                    let elapsed = now.saturating_sub(date.timestamp()).max(0);
                    let age = if elapsed < 60 {
                        locale.text("quota_just_now").to_owned()
                    } else {
                        locale
                            .text("quota_time_ago")
                            .replace("{time}", &relative_duration(elapsed, locale))
                    };
                    locale.text("quota_updated").replace("{time}", &age)
                })
        };
        Self {
            summary,
            failed: false,
            windows,
            balance,
            checked,
        }
    }
    pub fn failure(locale: Locale) -> Self {
        Self {
            summary: locale.text("quota_query_failed").into(),
            failed: true,
            ..Self::default()
        }
    }
    pub fn visible(&self) -> bool {
        !self.summary.is_empty()
    }
}

fn relative_duration(seconds: i64, locale: Locale) -> String {
    let (value, unit) = if seconds >= 86400 {
        (seconds / 86400, "quota_days")
    } else if seconds >= 3600 {
        (seconds / 3600, "quota_hours")
    } else {
        ((seconds / 60).max(1), "quota_minutes")
    };
    format!("{value}{}", locale.text(unit))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn display_uses_real_windows_and_units_and_hides_absent_data() {
        let mut profile: ProfileInfo = serde_json::from_value(json!({"profile_id":"p","provider":"test","checkedAt":"2026-09-07T22:55:00Z","rateLimits":{"ok":true,"rateLimits":{"primary":{"usedPercent":28,"windowDurationMins":300,"resetsAt":1788825600},"credits":{"balance":0,"unit":"USD"}}}})).unwrap();
        let display = QuotaPresentation::new_at(&profile, Locale::ZhCn, 1788822000);
        assert_eq!(display.windows[0].label, "5小时");
        assert_eq!(display.windows[0].short_label, "5H");
        assert_eq!(display.windows[0].center_value, "72");
        assert_eq!(display.windows[0].value, "剩余 72%");
        assert_eq!(display.windows[0].reset.as_deref(), Some("1小时后重置"));
        assert_eq!(display.checked.as_deref(), Some("额度更新于 5分钟前"));
        assert!(!profile.quota_stale_at(1788822000));
        assert!(!profile.quota_stale_at(1788823500));
        assert!(profile.quota_stale_at(1788823501));
        assert_eq!(display.balance.as_deref(), Some("余额 0.00 USD"));
        profile.rate_limits =
            json!({"ok":true,"rateLimits":{"primary":{"usedPercent":0,"windowDurationMins":300}}});
        assert_eq!(
            QuotaPresentation::new_at(&profile, Locale::ZhCn, 1788822000).windows[0].center_value,
            ""
        );
        profile.rate_limits["rateLimits"]["primary"]["usedPercent"] = json!(0.01);
        assert_eq!(
            QuotaPresentation::new_at(&profile, Locale::ZhCn, 1788822000).windows[0].center_value,
            "99"
        );
        profile.rate_limits = json!({"ok":true,"reported":false});
        assert!(!QuotaPresentation::new(&profile, Locale::En).visible());
        profile.rate_limits = json!({"ok":false,"error":"secret diagnostic"});
        assert_eq!(
            QuotaPresentation::new(&profile, Locale::ZhCn).summary,
            "查询失败"
        );
    }
}
