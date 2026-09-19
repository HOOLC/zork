use super::*;
#[derive(Clone, Default)]
pub struct Runtime {
    pub profile: Option<String>,
    pub model: Option<String>,
    pub thinking: Option<String>,
    pub context_tokens: Option<u64>,
    pub context_limit: Option<u64>,
    pub windows: Vec<(String, String)>,
    pub balance: Option<String>,
    pub failed: bool,
    pub summary: String,
}
pub struct Statistics {
    pub usage: model::usage::UsageSummary,
    pub complete: bool,
    pub loaded: bool,
    pub runtime: Runtime,
}
fn token_count(value: u64) -> String {
    let (number, suffix) = if value >= 1_000_000 {
        (format!("{:.2}", value as f64 / 1_000_000.), "M")
    } else if value >= 10_000 {
        (format!("{:.1}", value as f64 / 1_000.), "k")
    } else {
        return value.to_string();
    };
    format!(
        "{}{suffix}",
        number.trim_end_matches('0').trim_end_matches('.')
    )
}

/// Cue's page heading is `.cue-session-history-heading`: a 16/16/10 padded
/// block over the session identity line, the three-column usage facts and the
/// usage scope. The heading is flat, so it carries no fill and no radius.
impl Statistics {
    pub fn render(&self, text: Text) -> impl IntoElement {
        let usage = &self.usage;
        let runtime = &self.runtime;
        let complete = self.complete;
        let cache_rate = complete.then(|| usage.cache_hit_rate()).flatten();
        let reported = complete && usage.reported_steps > 0;
        let count = |value| {
            if !reported {
                "—".to_owned()
            } else {
                token_count(value)
            }
        };
        let total = usage.input.saturating_add(usage.output);
        let raw_total = if reported {
            total.to_string()
        } else {
            "—".to_owned()
        };
        let raw_cache = cache_rate
            .map(|rate| format!("{:.1}%", rate * 100.))
            .unwrap_or_else(|| "—".into());
        let model = runtime
            .model
            .clone()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "—".into());
        let name = runtime
            .profile
            .clone()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| model.clone());
        let thinking = runtime
            .thinking
            .clone()
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "—".into());
        let context = format!(
            "{} / {}",
            runtime
                .context_tokens
                .map(token_count)
                .unwrap_or_else(|| "—".into()),
            runtime
                .context_limit
                .map(token_count)
                .unwrap_or_else(|| "—".into())
        );
        // `.cue-session-identity`: a 6px gap, an 11px tertiary line, a 500
        // secondary name and a bordered environment field.
        let identity = div()
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(6.))
            .mt(px(10.))
            .text_size(px(11.))
            .text_color(rgb(SUBTLE))
            .child(
                div()
                    .min_w_0()
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(DIM))
                    .child(name),
            )
            .child(div().flex_none().child(thinking.clone()))
            .child(
                div()
                    .id("history-runtime")
                    .flex_none()
                    .ml(px(4.))
                    .pl(px(10.))
                    .border_l(px(1.))
                    .border_color(rgb(CUE_UI.palette.border))
                    .text_color(rgb(DIM))
                    .child(context)
                    .automation(
                        AutomationRole::Status,
                        format!(
                            "{} · {} / {}",
                            model,
                            runtime
                                .context_tokens
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "—".into()),
                            runtime
                                .context_limit
                                .map(|value| value.to_string())
                                .unwrap_or_else(|| "—".into())
                        ),
                    ),
            )
            .when_some(runtime.balance.clone(), |v, balance| {
                v.child(div().flex_none().child(balance))
            });
        // `.cue-session-overview-facts`: three equal columns, a 10px tertiary
        // `dt` and a 17px primary 500 `dd` (13px for the model) with a 9px note.
        let fact = |label: String, value: String, note: String, compact: bool| {
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(10.))
                        .text_color(rgb(SUBTLE))
                        .child(label),
                )
                .child(
                    div()
                        .my(px(5.))
                        .text_size(px(if compact { 13. } else { 17. }))
                        .line_height(px(if compact { 18. } else { 24. }))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(TEXT))
                        .font_features(crate::components::history::tabular_nums())
                        .truncate()
                        .child(value),
                )
                .child(
                    div()
                        .text_size(px(9.))
                        .text_color(rgb(SUBTLE))
                        .child(note),
                )
        };
        let token_note = format!(
            "{} {} · {} {}",
            text.text("history_input_tokens"),
            count(usage.input),
            text.text("history_output_tokens"),
            count(usage.output)
        );
        let cache_note = if complete
            && usage.cache_reported_steps > 0
            && usage.cache_reported_steps < usage.reported_steps
        {
            text.text("history_cache_partial")
        } else {
            format!(
                "{} {}",
                text.text("history_uncached_input"),
                count(usage.input.saturating_sub(usage.cached))
            )
        };
        let facts = div()
            .w_full()
            .flex()
            .gap(px(12.))
            .mt(px(18.))
            .child(fact(
                text.text("history_model"),
                model.clone(),
                format!("{} {}", text.text("history_item_thinking"), thinking),
                true,
            ))
            .child(fact(
                text.text("history_total_tokens"),
                count(total),
                token_note,
                false,
            ))
            .child(fact(
                text.text("history_cache_rate"),
                raw_cache.clone(),
                cache_note,
                false,
            ));
        // `.cue-session-usage-scope`: a 9px tertiary line 12px below the facts.
        let scope = div()
            .mt(px(12.))
            .text_size(px(9.))
            .text_color(rgb(SUBTLE))
            .child(format!(
                "{} · {}",
                text.text("history_usage_scope"),
                usage.reported_steps
            ))
            .children(
                runtime
                    .windows
                    .iter()
                    .map(|(label, value)| div().child(format!("{label} {value}"))),
            )
            .when_some(runtime.balance.clone(), |v, balance| {
                v.child(div().child(balance))
            })
            .when(runtime.failed, |v| {
                v.child(div().child(runtime.summary.clone()))
            })
            .when(self.loaded && !complete, |v| {
                v.child(div().child(text.text("history_usage_incomplete")))
            });
        div()
            .id("history-usage-overview")
            .w_full()
            .flex_shrink_0()
            .px(px(16.))
            .pt(px(16.))
            .pb(px(10.))
            .flex()
            .flex_col()
            .child(identity)
            .child(facts)
            .child(scope)
            .automation(
                AutomationRole::Status,
                format!(
                    "{}: {} · {}: {}",
                    text.text("history_total_tokens"),
                    raw_total,
                    text.text("history_cache_rate"),
                    raw_cache
                ),
            )
    }
}
