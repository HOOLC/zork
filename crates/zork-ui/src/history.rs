//! Platform-local clock formatting. Business event projection is shared.
#[cfg(test)]
use serde_json::Value;
pub use zork_client_types::history::{activity, entries, usage, Entry, Page, Record};
pub fn duration(ms: i64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{}s", (ms as f64 / 100.).round() / 10.)
    } else {
        format!("{}m {}s", ms / 60_000, (ms % 60_000) / 1000)
    }
}
#[cfg(target_arch = "wasm32")]
pub fn now() -> i64 {
    js_sys::Date::now() as i64
}
#[cfg(not(target_arch = "wasm32"))]
pub fn now() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as i64
}
#[cfg(not(target_family = "wasm"))]
pub fn clock(ms: Option<i64>) -> String {
    let Some(ms) = ms else { return "—".into() };
    // Local wall time without adding a timezone dependency.
    let seconds = (ms / 1000) as libc::time_t;
    let mut tm: libc::tm = unsafe { std::mem::zeroed() };
    if unsafe { libc::localtime_r(&seconds, &mut tm) }.is_null() {
        return "—".into();
    }
    format!("{:02}:{:02}:{:02}", tm.tm_hour, tm.tm_min, tm.tm_sec)
}
#[cfg(target_family = "wasm")]
pub fn clock(ms: Option<i64>) -> String {
    let Some(ms) = ms else { return "—".into() };
    let date = js_sys::Date::new(&(ms as f64).into());
    format!(
        "{:02}:{:02}:{:02}",
        date.get_hours(),
        date.get_minutes(),
        date.get_seconds()
    )
}
#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    fn r(id: &str, event: Value) -> Record {
        Record {
            event_id: id.into(),
            event,
            metadata: Default::default(),
        }
    }
    #[test]
    fn concurrent_same_name_and_paged_orphan_results_join_only_by_id() {
        let result = r(
            "3",
            json!({"kind":"tool_result","result":{"invocation_id":"b","tool":"shell.run","outcome":"succeeded","finished_at_ms":30,"data":{"content":"done"}}}),
        );
        let start = r(
            "2",
            json!({"kind":"step_completed","step_id":"s","completed_at_ms":10,"assistant_text":"thinking","invocations":[{"invocation_id":"a","tool":"shell.run","started_at_ms":10,"arguments":{"command":"pwd"}},{"invocation_id":"b","tool":"shell.run","started_at_ms":11,"arguments":{"command":"ls"}}]}),
        );
        let e = entries(&[result, start]);
        assert_eq!(e.len(), 3);
        let a = e.iter().find(|e| e.id == "tool:a").unwrap();
        let b = e.iter().find(|e| e.id == "tool:b").unwrap();
        assert_eq!(a.state, "running");
        assert_eq!(b.state, "succeeded");
        assert_eq!(b.summary, "ls");
        assert_eq!(b.duration(99), Some(19));
    }
    #[test]
    fn unknown_start_does_not_invent_duration() {
        let e = entries(&[r(
            "1",
            json!({"kind":"step_completed","step_id":"s","completed_at_ms":30}),
        )]);
        assert_eq!(e[0].duration(40), None);
    }
    #[test]
    fn complete_step_preserves_usage_and_measured_duration() {
        let records = [
            r(
                "1",
                json!({"kind":"step_started","step_id":"s","purpose":"compaction","started_at_ms":10}),
            ),
            r(
                "2",
                json!({"kind":"step_completed","step_id":"s","purpose":"compaction","completed_at_ms":50,"assistant_text":"context document","usage":{"input_tokens":20,"output_tokens":5}}),
            ),
        ];
        let e = entries(&records);
        assert_eq!(e.len(), 1);
        assert_eq!(e[0].action, "compaction");
        assert_eq!(e[0].duration(80), Some(40));
        assert_eq!(e[0].usage.as_ref().unwrap()["input_tokens"], 20);
        assert_eq!(e[0].raw[0]["event_id"], "1");
    }
    #[test]
    fn model_identity_uses_selection_at_request_start() {
        let all = [
            r(
                "1",
                json!({"kind":"session_created","selection":{"model":"first"}}),
            ),
            r(
                "2",
                json!({"kind":"step_started","step_id":"a","started_at_ms":1}),
            ),
            r(
                "3",
                json!({"kind":"selection_changed","selection":{"model":"second"}}),
            ),
            r(
                "4",
                json!({"kind":"step_completed","step_id":"a","completed_at_ms":2}),
            ),
            r(
                "5",
                json!({"kind":"step_started","step_id":"b","started_at_ms":2}),
            ),
        ];
        let projected = entries(&all);
        assert_eq!(projected[0].model.as_deref(), Some("first"));
        assert_eq!(projected[1].model.as_deref(), Some("second"));
        // A page that lacks earlier selection facts must not invent a model name.
        assert_eq!(entries(&all[1..])[0].model, None);
    }
}
