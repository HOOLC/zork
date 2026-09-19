//! Aggregates canonical, deduplicated step usage from the history projection.
use super::Entry;

/// Read-only statistics for the explicitly loaded history window. Cumulative
/// session aggregates keep their separate snapshot authority and coverage.
pub struct LoadedUsage {
    pub usage: UsageSummary,
    pub calls: usize,
    pub models: Vec<String>,
}
impl LoadedUsage {
    pub fn new<'a>(entries: impl DoubleEndedIterator<Item = &'a Entry> + Clone) -> Self {
        let usage = UsageSummary::new(entries.clone());
        let calls = entries
            .clone()
            .filter(|e| e.lane == 1 && e.state != "running")
            .count();
        let mut seen = std::collections::HashSet::new();
        let models = entries
            .rev()
            .filter_map(|entry| entry.model.as_ref())
            .filter(|model| !model.is_empty() && seen.insert((*model).clone()))
            .cloned()
            .collect();
        Self {
            usage,
            calls,
            models,
        }
    }
}

#[derive(Clone, Debug, Default)]
pub struct UsageSummary {
    pub input: u64,
    pub output: u64,
    pub cached: u64,
    pub reported_steps: usize,
    pub cache_reported_steps: usize,
    cache_input: u64,
}

impl UsageSummary {
    pub fn cache_input(&self) -> u64 {
        self.cache_input
    }
    pub fn from_totals(
        input: u64,
        output: u64,
        cached: u64,
        reported_steps: usize,
        cache_reported_steps: usize,
        cache_input: u64,
    ) -> Self {
        Self {
            input,
            output,
            cached,
            reported_steps,
            cache_reported_steps,
            cache_input,
        }
    }
    pub fn new<'a>(entries: impl IntoIterator<Item = &'a Entry>) -> Self {
        let mut result = Self::default();
        for usage in entries.into_iter().filter_map(|entry| entry.usage.as_ref()) {
            let (Some(input), Some(output)) = (
                usage["input_tokens"].as_u64(),
                usage["output_tokens"].as_u64(),
            ) else {
                continue;
            };
            result.reported_steps += 1;
            result.input = result.input.saturating_add(input);
            result.output = result.output.saturating_add(output);
            if let Some(cached) = usage["cached_input_tokens"]
                .as_u64()
                .filter(|cached| *cached <= input)
            {
                result.cache_reported_steps += 1;
                result.cached = result.cached.saturating_add(cached);
                result.cache_input = result.cache_input.saturating_add(input);
            }
        }
        result
    }

    /// Token-weighted rate, restricted to calls that report cache usage.
    pub fn cache_hit_rate(&self) -> Option<f64> {
        (self.cache_input > 0).then(|| self.cached as f64 / self.cache_input as f64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::history::{entries, Record};
    use serde_json::json;

    #[test]
    fn weighted_cache_rate_excludes_unknown_reports_and_deduplicates_steps() {
        let record = |id: &str, step: &str, usage| Record {
            event_id: id.into(),
            event: json!({"kind":"step_completed","step_id":step,"completed_at_ms":10,"usage":usage}),
            metadata: Default::default(),
        };
        let records = vec![
            record(
                "1",
                "a",
                json!({"input_tokens":100,"output_tokens":10,"cached_input_tokens":80}),
            ),
            record(
                "2",
                "b",
                json!({"input_tokens":900,"output_tokens":20,"cached_input_tokens":90}),
            ),
            record("3", "c", json!({"input_tokens":1000,"output_tokens":30})),
            record(
                "4",
                "a",
                json!({"input_tokens":100,"output_tokens":10,"cached_input_tokens":80}),
            ),
        ];
        let summary = UsageSummary::new(&entries(&records));
        assert_eq!(
            (summary.input, summary.output, summary.cached),
            (2000, 60, 170)
        );
        assert_eq!(
            (summary.reported_steps, summary.cache_reported_steps),
            (3, 2)
        );
        assert_eq!(summary.cache_hit_rate(), Some(0.17));
        assert_eq!(UsageSummary::default().cache_hit_rate(), None);
    }
    #[test]
    fn failed_calls_keep_reported_usage_and_zero_cache_is_known() {
        let records = vec![Record {
            event_id: "failure".into(),
            event: json!({"kind":"step_failed", "step_id":"failed", "failed_at_ms":10,
                "error":{"usage":{"input_tokens":200,"output_tokens":3,"cached_input_tokens":0}}}),
            metadata: Default::default(),
        }];
        let summary = UsageSummary::new(&entries(&records));
        assert_eq!((summary.input, summary.output), (200, 3));
        assert_eq!(summary.cache_hit_rate(), Some(0.));
    }
}
