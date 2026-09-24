use super::*;

#[derive(Clone, Default)]
pub struct Runtime {
    pub name: String,
    pub role: Option<String>,
    pub environment: Option<String>,
    pub provider: Option<String>,
    pub model: Option<String>,
}

pub struct Statistics {
    pub usage: model::usage::UsageSummary,
    pub calls: usize,
    pub models: Vec<String>,
    pub runtime: Runtime,
}

fn token_count(value: u64) -> String {
    let (number, suffix) = if value >= 1_000_000 {
        (value as f64 / 1_000_000., "M")
    } else if value >= 1_000 {
        (value as f64 / 1_000., "K")
    } else {
        return value.to_string();
    };
    let formatted = format!("{number:.1}");
    format!(
        "{}{suffix}",
        formatted.strip_suffix(".0").unwrap_or(&formatted)
    )
}

impl Statistics {
    pub fn render(&self, text: Text, width: f32) -> impl IntoElement {
        let usage = &self.usage;
        let runtime = &self.runtime;
        let reported = usage.reported_steps > 0;
        let cache_rate = (reported && usage.cache_reported_steps == usage.reported_steps)
            .then(|| usage.cache_hit_rate())
            .flatten();
        let count = |value| {
            if reported {
                token_count(value)
            } else {
                "—".to_owned()
            }
        };
        let total = usage.input.saturating_add(usage.output);
        let cache = cache_rate
            .map(|rate| {
                let value = format!("{:.1}", rate * 100.);
                format!("{}%", value.strip_suffix(".0").unwrap_or(&value))
            })
            .unwrap_or_else(|| "—".into());
        let model = self
            .models
            .first()
            .cloned()
            .or_else(|| runtime.model.clone())
            .unwrap_or_else(|| "—".into());
        let name = runtime.name.clone();
        let identity = div()
            .id("history-identity")
            .flex()
            .items_center()
            .flex_wrap()
            .gap(px(6.))
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(rgb(SUBTLE()))
            .child(
                div()
                    .max_w(px((width - 56.).max(1.)))
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(TEXT()))
                    .child(name.clone()),
            )
            .when_some(
                runtime
                    .role
                    .clone()
                    .filter(|role| role.to_lowercase() != name.to_lowercase()),
                |v, role| v.child(role),
            )
            .child(
                div()
                    .id("history-runtime")
                    .max_w_full()
                    .min_w_0()
                    .truncate()
                    .ml(px(8.))
                    .text_color(rgb(DIM()))
                    .child(
                        runtime
                            .environment
                            .clone()
                            .unwrap_or_else(|| text.text("history_environment_unknown")),
                    )
                    .automation(
                        AutomationRole::Status,
                        runtime.environment.clone().unwrap_or_default(),
                    ),
            )
            .when_some(runtime.provider.clone(), |v, provider| v.child(provider))
            .automation(AutomationRole::Status, name);
        let fact = |id: &'static str,
                    label: String,
                    value: String,
                    note: Option<String>,
                    compact: bool| {
            div()
                .id(id)
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .child(
                    div()
                        .text_size(px(12.))
                        .line_height(px(16.))
                        .text_color(rgb(SUBTLE()))
                        .child(label),
                )
                .child(
                    div()
                        .mt(px(2.))
                        .text_size(px(if compact { 13. } else { 15. }))
                        .line_height(px(20.))
                        .font_weight(FontWeight::MEDIUM)
                        .text_color(rgb(TEXT()))
                        .font_features(crate::components::history::tabular_nums())
                        .when(compact, |v| v.line_clamp(2))
                        .child(value.clone()),
                )
                .when_some(note, |v, note| {
                    v.child(
                        div()
                            .text_size(px(12.))
                            .line_height(px(16.))
                            .text_color(rgb(SUBTLE()))
                            .truncate()
                            .child(note),
                    )
                })
                .automation(AutomationRole::Status, value)
        };
        let mut model_value = model;
        if self.models.len() > 1 {
            model_value.push_str(&format!(" +{}", self.models.len() - 1));
        }
        let model_fact = fact(
            "history-model",
            text.text("history_model"),
            model_value,
            None,
            true,
        );
        let tokens = fact(
            "history-tokens",
            text.text("history_total_tokens"),
            count(total),
            Some(if reported {
                text.text("history_token_breakdown")
                    .replace("{input}", &count(usage.input))
                    .replace("{output}", &count(usage.output))
            } else {
                text.text("history_usage_unreported")
            }),
            false,
        );
        let cache_fact = fact(
            "history-cache",
            text.text("history_cache_rate"),
            cache.clone(),
            Some(text.text(if cache_rate.is_some() {
                "history_cache_definition"
            } else {
                "history_cache_unreported"
            })),
            false,
        );
        let facts = div().w_full().flex().gap(px(12.)).mt(px(10.));
        let facts = if width <= 330. {
            facts
                .flex_col()
                .child(model_fact)
                .child(div().flex().gap(px(12.)).child(tokens).child(cache_fact))
        } else {
            facts.child(model_fact).child(tokens).child(cache_fact)
        };
        div()
            .id("history-usage-overview")
            .w_full()
            .flex_shrink_0()
            .px(px(16.))
            .pt(px(12.))
            .pb(px(16.))
            .flex()
            .flex_col()
            .child(identity)
            .child(facts)
            .child(
                div()
                    .mt(px(8.))
                    .text_size(px(12.))
                    .line_height(px(16.))
                    .text_color(rgb(SUBTLE()))
                    .child(
                        text.text("history_usage_scope")
                            .replace("{reported}", &usage.reported_steps.to_string())
                            .replace("{calls}", &self.calls.to_string()),
                    ),
            )
            .automation(
                AutomationRole::Status,
                format!(
                    "{}: {} · {}: {}",
                    text.text("history_total_tokens"),
                    total,
                    text.text("history_cache_rate"),
                    cache
                ),
            )
    }
}
