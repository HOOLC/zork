use super::*;
impl RootView {
    pub(super) fn history_statistics_data(&mut self) -> zork_ui::history_page::Statistics {
        if self
            .history
            .quota
            .as_ref()
            .is_none_or(|(locale, _)| *locale != self.locale)
        {
            self.history.quota = self
                .history
                .runtime
                .as_ref()
                .and_then(|r| r.profile.as_ref())
                .map(|profile| {
                    let mut quota =
                        crate::desktop::profile_quota::QuotaPresentation::new(profile, self.locale);
                    quota.checked = profile
                        .checked_at
                        .as_deref()
                        .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                        .map(|date| {
                            self.locale.text("history_quota_checked").replace(
                                "{time}",
                                &date
                                    .with_timezone(&chrono::Local)
                                    .format("%m-%d %H:%M")
                                    .to_string(),
                            )
                        });
                    (self.locale, quota)
                });
        }
        let raw = self.history.runtime.as_ref();
        let quota = self.history.quota.as_ref().map(|(_, q)| q);
        zork_ui::history_page::Statistics {
            usage: self.history.usage.clone(),
            complete: self.history.usage_complete,
            loaded: self.history.overview_loaded,
            runtime: zork_ui::history_page::Runtime {
                profile: raw
                    .and_then(|r| r.profile.as_ref())
                    .map(|p| p.display_name().to_owned()),
                model: raw.and_then(|r| r.model.clone()),
                thinking: raw.and_then(|r| r.thinking.clone()),
                context_tokens: raw.and_then(|r| r.context_tokens),
                context_limit: raw.and_then(|r| r.context_limit),
                windows: quota
                    .map(|q| {
                        q.windows
                            .iter()
                            .map(|w| (w.label.clone(), w.value.clone()))
                            .collect()
                    })
                    .unwrap_or_default(),
                balance: quota.and_then(|q| q.balance.clone()),
                failed: quota.is_some_and(|q| q.failed),
                summary: quota.map(|q| q.summary.clone()).unwrap_or_default(),
            },
        }
    }
}
