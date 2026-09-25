use super::*;

/// The device a Session runs on, as every device-name surface shows it.
#[derive(Clone, Default)]
pub struct Environment {
    pub name: crate::device_name::DeviceName,
    pub status: crate::device_name::DeviceStatus,
}

#[derive(Clone, Default)]
pub struct Runtime {
    pub name: String,
    pub role: Option<String>,
    pub environment: Option<Environment>,
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
    pub fn render<V: 'static>(
        &self,
        text: Text,
        width: f32,
        window: &mut Window,
        cx: &mut Context<V>,
    ) -> impl IntoElement {
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
        let mut model_value = model;
        if self.models.len() > 1 {
            model_value.push_str(&format!(" +{}", self.models.len() - 1));
        }
        let dot = || div().text_color(rgb(SUBTLE())).child("·");
        // One line: who, which model, how many tokens. Breakdown, cache rate
        // and runtime details wait behind 用量.
        let line = div()
            .id("history-identity")
            .flex()
            .items_center()
            .gap(px(6.))
            // Shrinks before the 用量 toggle: the model, then the name, truncate.
            .flex_1()
            .min_w_0()
            .overflow_hidden()
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(rgb(DIM()))
            .child(crate::device_name::mark(&name, 16.))
            .child(
                div()
                    .max_w(px((width * 0.4).max(80.)))
                    .min_w(px(24.))
                    .flex_shrink(1.)
                    .truncate()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(rgb(TEXT()))
                    .child(name.clone()),
            )
            .child(dot())
            .child(
                div()
                    .id("history-model")
                    .min_w_0()
                    .truncate()
                    .child(model_value.clone())
                    .automation(AutomationRole::Status, model_value),
            )
            .child(dot())
            .child(
                div()
                    .id("history-tokens")
                    .flex_shrink_0()
                    .font_features(crate::components::history::tabular_nums())
                    .child(format!("{} tokens", count(total)))
                    .automation(AutomationRole::Status, count(total)),
            )
            .automation(AutomationRole::Status, name.clone());
        let breakdown = if reported {
            text.text("history_token_breakdown")
                .replace("{input}", &count(usage.input))
                .replace("{output}", &count(usage.output))
        } else {
            text.text("history_usage_unreported")
        };
        let mut details = vec![
            breakdown,
            format!("{} {}", text.text("history_cache_rate"), cache),
        ];
        // Role · device · provider. The device uses the shared device-name
        // component, so its status keeps its own dot, colour and wording.
        let role = runtime
            .role
            .clone()
            .filter(|role| role.to_lowercase() != name.to_lowercase());
        let device = match &runtime.environment {
            Some(environment) => crate::device_name::label(
                "history-device",
                environment.name.clone(),
                &environment.status,
                Some(&text),
            )
            .into_any_element(),
            None => div()
                .child(text.text("history_environment_unknown"))
                .into_any_element(),
        };
        let mut parts = vec![];
        if let Some(role) = role {
            parts.push(div().flex_shrink_0().child(role).into_any_element());
        }
        parts.push(device);
        if let Some(provider) = runtime.provider.clone() {
            parts.push(div().flex_shrink_0().child(provider).into_any_element());
        }
        let mut runtime_line = div()
            .id("history-runtime")
            .flex()
            .items_center()
            .gap(px(6.))
            .min_w_0()
            .overflow_hidden();
        for (index, part) in parts.into_iter().enumerate() {
            if index > 0 {
                runtime_line = runtime_line.child(dot());
            }
            runtime_line = runtime_line.child(part);
        }
        let runtime_label = runtime
            .environment
            .as_ref()
            .map(|e| crate::device_name::accessible_summary(e.name.clone(), &e.status, Some(&text)))
            .unwrap_or_default();
        let usage_panel = div()
            .flex()
            .flex_col()
            .gap(px(2.))
            .text_size(px(12.))
            .line_height(px(18.))
            .text_color(rgb(DIM()))
            .child(div().id("history-cache").child(details[1].clone()).automation(AutomationRole::Status, cache.clone()))
            .child(details[0].clone())
            .child(runtime_line.automation(AutomationRole::Status, runtime_label));
        // The toggle sits in the summary line and the panel opens below it,
        // both driven by the same disclosure so the panel grows with it.
        let (usage_toggle, usage_body) = crate::components::disclosure::expander_parts(
            "history-usage-details",
            text.text("history_usage_label"),
            usage_panel,
            window,
            cx,
        );
        div()
            .id("history-usage-overview")
            .w_full()
            .flex_shrink_0()
            .px(px(16.))
            .pt(px(12.))
            .pb(px(12.))
            .flex()
            .flex_col()
            .gap(px(2.))
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .min_w_0()
                    .child(line)
                    .child(div().flex_shrink_0().child(usage_toggle)),
            )
            .children(usage_body)
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
