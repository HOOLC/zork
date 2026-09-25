//! How a model thinks, in its provider's own terms.
//!
//! Station stores a model's thinking as a list of values plus a default, and the
//! agent maps each value onto the request. This module is the one place that
//! reads that list as a scheme (off only, always on, a toggle, native effort
//! levels, or a token budget) and writes a scheme back. Platforms show the
//! scheme; they never impose a fixed scale of their own.
use serde::{Deserialize, Serialize};

pub const OFF: &str = "off";
pub const DYNAMIC: &str = "dynamic";
const BUDGET_PREFIX: &str = "budget-";

#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum ThinkingScheme {
    /// The model does not think; requests send nothing.
    Unsupported,
    /// The model always thinks; `value` is what requests send.
    Always { value: String },
    /// Thinking can be switched on or off; `on` is the value sent when on.
    Toggle { on: String, default_on: bool },
    /// Native effort names in the provider's order, e.g. minimal…xhigh.
    Levels {
        values: Vec<String>,
        default: String,
    },
    /// A token budget. `presets` are in tokens; `dynamic` lets the provider
    /// choose (Gemini); `allow_off` offers no thinking at all.
    Budget {
        presets: Vec<u32>,
        default: BudgetChoice,
        #[serde(default)]
        dynamic: bool,
        #[serde(default)]
        allow_off: bool,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(tag = "kind", content = "tokens", rename_all = "snake_case")]
pub enum BudgetChoice {
    Off,
    Dynamic,
    Tokens(u32),
}

impl BudgetChoice {
    fn value(self) -> String {
        match self {
            Self::Off => OFF.into(),
            Self::Dynamic => DYNAMIC.into(),
            Self::Tokens(tokens) => budget_value(tokens),
        }
    }
}

pub fn budget_value(tokens: u32) -> String {
    format!("{BUDGET_PREFIX}{tokens}")
}
/// Token count of a stored budget value such as `budget-16384`.
pub fn budget_tokens(value: &str) -> Option<u32> {
    value.strip_prefix(BUDGET_PREFIX)?.parse().ok()
}

impl BudgetChoice {
    /// How an option reads in editors and the model panel.
    pub fn label(self) -> String {
        match self {
            Self::Off => "关".into(),
            Self::Dynamic => "自动".into(),
            Self::Tokens(tokens) => crate::model_edit::compact_tokens(u64::from(tokens)),
        }
    }
}

/// The five ways a model can think, in the order editors list them.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ThinkingKind {
    Unsupported,
    Always,
    Toggle,
    Levels,
    Budget,
}

impl ThinkingKind {
    pub const ALL: [Self; 5] = [
        Self::Unsupported,
        Self::Always,
        Self::Toggle,
        Self::Levels,
        Self::Budget,
    ];
    pub fn title(self) -> &'static str {
        match self {
            Self::Unsupported => "不思考",
            Self::Always => "总是思考",
            Self::Toggle => "可以开关",
            Self::Levels => "按档位调节",
            Self::Budget => "按 token 预算",
        }
    }
    pub fn example(self) -> &'static str {
        match self {
            Self::Unsupported => "例如 Qwen3 Coder、Kimi K2、GPT-4.1",
            Self::Always => "关不掉。例如 DeepSeek Reasoner、Kimi K2 Thinking、Grok 4",
            Self::Toggle => "例如 DeepSeek V3.2、GLM-4.6、Qwen Plus",
            Self::Levels => "例如 GPT-5 的 minimal…high、Gemini 3 的 low / high",
            Self::Budget => "例如 Claude、Gemini 2.5",
        }
    }
    /// Kind name for list rows, e.g. `可开关`.
    pub fn short(self) -> &'static str {
        match self {
            Self::Unsupported => "不思考",
            Self::Always => "总是思考",
            Self::Toggle => "可开关",
            Self::Levels => "档位",
            Self::Budget => "预算",
        }
    }
}

/// What the composer model panel shows for a scheme.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Panel {
    /// 不显示思考选项
    Hidden,
    /// 思考中（不可调）
    Fixed,
    /// A segmented control with the default selected.
    Options {
        options: Vec<String>,
        selected: usize,
    },
}

/// A validation failure of a thinking scheme.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct SchemeProblem {
    pub message: String,
    /// Budget presets that are not below the output limit.
    pub bad_budgets: Vec<u32>,
}

/// Effort names providers commonly use, weakest first.
pub const COMMON_LEVELS: [&str; 6] = ["minimal", "low", "medium", "high", "xhigh", "max"];
const LEVEL_NAME_MAX: usize = 40;

fn common_rank(name: &str) -> Option<usize> {
    COMMON_LEVELS.iter().position(|c| *c == name)
}

/// Why a level name cannot be used, if it cannot. Names are the provider's own
/// request values, so only `A-Z a-z 0-9 _ . -` are accepted.
pub fn level_name_error(name: &str) -> Option<String> {
    let name = name.trim();
    if name.is_empty() {
        return Some("写一个名称".into());
    }
    if name.chars().count() > LEVEL_NAME_MAX {
        return Some(format!("名称最多 {LEVEL_NAME_MAX} 个字符"));
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
    {
        return Some("只用字母、数字、.、- 和 _（供应商接口里的原名）".into());
    }
    if name == DYNAMIC || name.starts_with(BUDGET_PREFIX) {
        return Some(format!("{name} 有特殊含义，换一个名称"));
    }
    None
}

impl ThinkingScheme {
    /// The stored list and default Station keeps for this scheme.
    pub fn encode(&self) -> (Vec<String>, String) {
        match self {
            Self::Unsupported => (vec![OFF.into()], OFF.into()),
            Self::Always { value } => (vec![value.clone()], value.clone()),
            Self::Toggle { on, default_on } => (
                vec![OFF.into(), on.clone()],
                if *default_on { on.clone() } else { OFF.into() },
            ),
            Self::Levels { values, default } => (values.clone(), default.clone()),
            Self::Budget {
                presets,
                default,
                dynamic,
                allow_off,
            } => {
                let mut values = Vec::new();
                if *allow_off {
                    values.push(OFF.into());
                }
                if *dynamic {
                    values.push(DYNAMIC.into());
                }
                values.extend(presets.iter().map(|t| budget_value(*t)));
                (values, default.value())
            }
        }
    }

    /// Reads a stored list back into a scheme. Old free-form lists (including
    /// comma-separated editor input) become native levels unless they match a
    /// narrower shape exactly.
    pub fn decode(values: &[String], default: &str) -> Self {
        let values = dedupe(values);
        let default = default.trim();
        let non_off: Vec<&String> = values.iter().filter(|v| *v != OFF).collect();
        let has_off = values.len() != non_off.len();
        let budgets: Vec<u32> = non_off.iter().filter_map(|v| budget_tokens(v)).collect();
        let dynamic = non_off.iter().any(|v| *v == DYNAMIC);
        if !budgets.is_empty() && budgets.len() + usize::from(dynamic) == non_off.len() {
            let choice = if default == OFF {
                BudgetChoice::Off
            } else if default == DYNAMIC {
                BudgetChoice::Dynamic
            } else {
                budget_tokens(default)
                    .map(BudgetChoice::Tokens)
                    .unwrap_or(BudgetChoice::Tokens(budgets[0]))
            };
            return Self::Budget {
                presets: budgets,
                default: choice,
                dynamic,
                allow_off: has_off,
            };
        }
        match (has_off, non_off.as_slice()) {
            (_, []) => Self::Unsupported,
            (false, [only]) => Self::Always {
                value: (*only).clone(),
            },
            (true, [only]) => Self::Toggle {
                on: (*only).clone(),
                default_on: default == only.as_str(),
            },
            _ => Self::Levels {
                default: if values.iter().any(|v| v == default) {
                    default.into()
                } else {
                    values[0].clone()
                },
                values,
            },
        }
    }

    /// Parses legacy editor input such as `"off, low, high"`.
    pub fn from_legacy_input(list: &str, default: &str) -> Self {
        let values: Vec<String> = list
            .split([',', '，'])
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(str::to_owned)
            .collect();
        if values.is_empty() {
            return Self::Unsupported;
        }
        let default = default.trim();
        if !values.iter().any(|v| v == default) {
            // Keep the typed default so validation can point at it.
            return Self::Levels {
                values: dedupe(&values),
                default: default.into(),
            };
        }
        Self::decode(&values, default)
    }

    /// Validation shared by every editor. `output` is the model's output limit.
    pub fn error(&self, output: Option<u32>) -> Option<String> {
        self.problem(output).map(|p| p.message)
    }

    /// Like [`Self::error`], plus the budget presets that caused it so editors
    /// can mark those chips.
    pub fn problem(&self, output: Option<u32>) -> Option<SchemeProblem> {
        let fail = |message: String| {
            Some(SchemeProblem {
                message,
                bad_budgets: vec![],
            })
        };
        match self {
            Self::Unsupported => None,
            Self::Always { value } | Self::Toggle { on: value, .. } => {
                if value.trim().is_empty() || value == OFF {
                    fail("请填写思考时发送的值".into())
                } else {
                    None
                }
            }
            Self::Levels { values, default } => {
                if values.is_empty() {
                    return fail("至少保留一个档位".into());
                }
                if let Some(error) = values.iter().find_map(|v| level_name_error(v)) {
                    return fail(error);
                }
                if dedupe(values).len() != values.len() {
                    return fail("档位不能重复".into());
                }
                if !values.contains(default) {
                    return fail("默认档位须在可选档位中".into());
                }
                None
            }
            Self::Budget {
                presets,
                default,
                dynamic,
                allow_off,
            } => {
                if presets.is_empty() {
                    return fail("至少保留一个预算，或改成“不思考”".into());
                }
                if presets.contains(&0) {
                    return fail("预算需大于 0".into());
                }
                if let Some(output) = output {
                    let bad: Vec<u32> = presets.iter().copied().filter(|t| *t >= output).collect();
                    if !bad.is_empty() {
                        return Some(SchemeProblem {
                            message: format!(
                                "预算需小于最长输出 {}",
                                crate::model_edit::compact_tokens(u64::from(output))
                            ),
                            bad_budgets: bad,
                        });
                    }
                }
                let mut sorted = presets.clone();
                sorted.sort_unstable();
                sorted.dedup();
                if sorted.len() != presets.len() {
                    return fail("预算不能重复".into());
                }
                let valid = match default {
                    BudgetChoice::Off => *allow_off,
                    BudgetChoice::Dynamic => *dynamic,
                    BudgetChoice::Tokens(t) => presets.contains(t),
                };
                if valid {
                    None
                } else {
                    fail("默认预算须在可选项中".into())
                }
            }
        }
    }

    pub fn kind(&self) -> ThinkingKind {
        match self {
            Self::Unsupported => ThinkingKind::Unsupported,
            Self::Always { .. } => ThinkingKind::Always,
            Self::Toggle { .. } => ThinkingKind::Toggle,
            Self::Levels { .. } => ThinkingKind::Levels,
            Self::Budget { .. } => ThinkingKind::Budget,
        }
    }

    /// The starting configuration when a user picks a kind that has no cached
    /// or source configuration. Request values (`on`, `value`) are never shown.
    pub fn default_for(kind: ThinkingKind) -> Self {
        match kind {
            ThinkingKind::Unsupported => Self::Unsupported,
            ThinkingKind::Always => Self::Always {
                value: "high".into(),
            },
            ThinkingKind::Toggle => Self::Toggle {
                on: "high".into(),
                default_on: false,
            },
            ThinkingKind::Levels => Self::Levels {
                values: vec!["low".into(), "medium".into(), "high".into()],
                default: "medium".into(),
            },
            ThinkingKind::Budget => Self::Budget {
                presets: vec![4_000, 16_000, 32_000],
                default: BudgetChoice::Off,
                dynamic: false,
                allow_off: true,
            },
        }
    }

    /// The options a budget default (and the composer model panel) offers:
    /// 关 when thinking can be off, 自动 when dynamic, then the presets.
    pub fn budget_options(&self) -> Vec<BudgetChoice> {
        let Self::Budget {
            presets,
            dynamic,
            allow_off,
            ..
        } = self
        else {
            return vec![];
        };
        let mut options = Vec::new();
        if *allow_off {
            options.push(BudgetChoice::Off);
        }
        if *dynamic {
            options.push(BudgetChoice::Dynamic);
        }
        options.extend(presets.iter().map(|t| BudgetChoice::Tokens(*t)));
        options
    }

    /// Keeps a budget default valid after its option went away: the first
    /// remaining option wins. With no options left the default stays and
    /// validation reports the empty budget list.
    fn repair_budget_default(&mut self) {
        let options = self.budget_options();
        if let Self::Budget { default, .. } = self {
            if !options.contains(default) {
                if let Some(first) = options.first() {
                    *default = *first;
                }
            }
        }
    }

    pub fn set_budget_allow_off(&mut self, on: bool) {
        if let Self::Budget { allow_off, .. } = self {
            *allow_off = on;
        }
        self.repair_budget_default();
    }

    pub fn set_budget_dynamic(&mut self, on: bool) {
        if let Self::Budget { dynamic, .. } = self {
            *dynamic = on;
        }
        self.repair_budget_default();
    }

    pub fn set_budget_default(&mut self, choice: BudgetChoice) -> bool {
        if !self.budget_options().contains(&choice) {
            return false;
        }
        if let Self::Budget { default, .. } = self {
            *default = choice;
        }
        true
    }

    pub fn delete_budget(&mut self, index: usize) {
        if let Self::Budget { presets, .. } = self {
            if index < presets.len() {
                presets.remove(index);
            }
        }
        self.repair_budget_default();
    }

    /// Adds a budget from editor text such as `8K`. Presets stay sorted.
    pub fn add_budget(&mut self, text: &str, output: Option<u32>) -> Result<u32, String> {
        let Self::Budget { presets, .. } = self else {
            return Err("当前不是按预算思考".into());
        };
        let tokens = crate::model_edit::parse_tokens(text)
            .and_then(|t| u32::try_from(t).ok())
            .ok_or_else(|| "写成 8K 或 8192 这样的数字".to_owned())?;
        if presets.contains(&tokens) {
            return Err(format!(
                "已经有 {}",
                crate::model_edit::compact_tokens(u64::from(tokens))
            ));
        }
        if let Some(output) = output.filter(|o| tokens >= *o) {
            return Err(format!(
                "需小于最长输出 {}",
                crate::model_edit::compact_tokens(u64::from(output))
            ));
        }
        let at = presets.partition_point(|t| *t < tokens);
        presets.insert(at, tokens);
        let was_empty = presets.len() == 1;
        if was_empty {
            self.repair_budget_default();
        }
        Ok(tokens)
    }

    pub fn toggle_default_on(&mut self, on: bool) {
        if let Self::Toggle { default_on, .. } = self {
            *default_on = on;
        }
    }

    pub fn set_level_default(&mut self, name: &str) -> bool {
        match self {
            Self::Levels { values, default } if values.iter().any(|v| v == name) => {
                *default = name.into();
                true
            }
            _ => false,
        }
    }

    /// Adds a level. Common names go to their canonical place among the
    /// common names already present; custom names are appended.
    pub fn add_level(&mut self, name: &str) -> Result<(), String> {
        let Self::Levels { values, .. } = self else {
            return Err("当前不是按档位思考".into());
        };
        let name = name.trim();
        if let Some(error) = level_name_error(name) {
            return Err(error);
        }
        if values.iter().any(|v| v == name) {
            return Err(format!("已经有 {name}"));
        }
        match common_rank(name) {
            Some(rank) => {
                let at = values
                    .iter()
                    .position(|v| common_rank(v).is_some_and(|r| r > rank))
                    .unwrap_or(values.len());
                values.insert(at, name.into());
            }
            None => values.push(name.into()),
        }
        Ok(())
    }

    /// Deletes a level. The last one cannot go; deleting the default moves the
    /// default to its neighbour (the next one, or the previous at the end).
    pub fn delete_level(&mut self, index: usize) -> bool {
        let Self::Levels { values, default } = self else {
            return false;
        };
        if values.len() <= 1 || index >= values.len() {
            return false;
        }
        let gone = values.remove(index);
        if *default == gone {
            *default = values[index.min(values.len() - 1)].clone();
        }
        true
    }

    pub fn move_level(&mut self, from: usize, to: usize) -> bool {
        let Self::Levels { values, .. } = self else {
            return false;
        };
        if from >= values.len() || to >= values.len() || from == to {
            return false;
        }
        let value = values.remove(from);
        values.insert(to, value);
        true
    }

    /// Common level names not yet used, in canonical order.
    pub fn addable_levels(&self) -> Vec<&'static str> {
        match self {
            Self::Levels { values, .. } => COMMON_LEVELS
                .iter()
                .copied()
                .filter(|name| !values.iter().any(|v| v == name))
                .collect(),
            _ => vec![],
        }
    }

    /// What the composer's model panel shows for this scheme.
    pub fn panel(&self) -> Panel {
        match self {
            Self::Unsupported => Panel::Hidden,
            Self::Always { .. } => Panel::Fixed,
            Self::Toggle { default_on, .. } => Panel::Options {
                options: vec!["关".into(), "开".into()],
                selected: usize::from(*default_on),
            },
            Self::Levels { values, default } => Panel::Options {
                options: values.clone(),
                selected: values.iter().position(|v| v == default).unwrap_or(0),
            },
            Self::Budget { default, .. } => {
                let options = self.budget_options();
                Panel::Options {
                    selected: options.iter().position(|o| o == default).unwrap_or(0),
                    options: options.into_iter().map(BudgetChoice::label).collect(),
                }
            }
        }
    }

    /// Full description for an editor summary row, e.g.
    /// `档位 low / medium / high · 默认 medium`.
    pub fn describe(&self) -> String {
        match self {
            Self::Unsupported => "不思考".into(),
            Self::Always { .. } => "总是思考".into(),
            Self::Toggle { default_on, .. } => {
                format!("可以开关 · 默认{}", if *default_on { "开" } else { "关" })
            }
            Self::Levels { values, default } => {
                format!("档位 {} · 默认 {default}", values.join(" / "))
            }
            Self::Budget {
                presets,
                default,
                dynamic,
                ..
            } => {
                let mut parts: Vec<String> = presets
                    .iter()
                    .map(|t| crate::model_edit::compact_tokens(u64::from(*t)))
                    .collect();
                if *dynamic {
                    parts.push("自动".into());
                }
                format!("预算 {} · 默认 {}", parts.join(" / "), default.label())
            }
        }
    }

    /// Short description of where a value came from, e.g. `可开关 · 默认关`.
    pub fn short(&self) -> String {
        match self {
            Self::Toggle { default_on, .. } => {
                format!("可开关 · 默认{}", if *default_on { "开" } else { "关" })
            }
            Self::Levels { default, .. } => format!("档位 · 默认 {default}"),
            Self::Budget { default, .. } => format!("预算 · 默认 {}", default.label()),
            other => other.kind().short().into(),
        }
    }

    /// Short Chinese description for rows and summaries.
    pub fn summary(&self) -> String {
        match self {
            Self::Unsupported => "不支持思考".into(),
            Self::Always { .. } => "固定思考".into(),
            Self::Toggle { .. } => "思考开关".into(),
            Self::Levels { values, .. } => {
                let names: Vec<&str> = values
                    .iter()
                    .map(String::as_str)
                    .filter(|v| *v != OFF)
                    .collect();
                match names.as_slice() {
                    [] => "不支持思考".into(),
                    [one] => format!("思考 {one}"),
                    [first, .., last] => format!("思考 {first}–{last}"),
                }
            }
            Self::Budget { dynamic, .. } => {
                if *dynamic {
                    "思考预算 · 动态".into()
                } else {
                    "思考预算".into()
                }
            }
        }
    }
}

/// Display label for one stored value: native names stay as they are, budgets
/// read as token sizes.
pub fn value_label(value: &str) -> String {
    match value {
        OFF => "关".into(),
        DYNAMIC => "动态".into(),
        _ => budget_tokens(value)
            .map(|t| crate::model_edit::compact_tokens(u64::from(t)))
            .unwrap_or_else(|| value.into()),
    }
}

fn dedupe(values: &[String]) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    for value in values.iter().map(|v| v.trim()).filter(|v| !v.is_empty()) {
        if !out.iter().any(|v| v == value) {
            out.push(value.into());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    fn owned(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| (*v).into()).collect()
    }

    #[test]
    fn stored_lists_round_trip_through_each_scheme() {
        for scheme in [
            ThinkingScheme::Unsupported,
            ThinkingScheme::Always {
                value: "high".into(),
            },
            ThinkingScheme::Toggle {
                on: "high".into(),
                default_on: true,
            },
            ThinkingScheme::Levels {
                values: owned(&["minimal", "low", "medium", "high"]),
                default: "medium".into(),
            },
            ThinkingScheme::Budget {
                presets: vec![4096, 16384],
                default: BudgetChoice::Dynamic,
                dynamic: true,
                allow_off: true,
            },
        ] {
            let (values, default) = scheme.encode();
            assert_eq!(ThinkingScheme::decode(&values, &default), scheme);
        }
    }

    #[test]
    fn legacy_comma_lists_migrate_to_native_levels() {
        assert_eq!(
            ThinkingScheme::from_legacy_input("low, medium, high, xhigh, max", "medium"),
            ThinkingScheme::Levels {
                values: owned(&["low", "medium", "high", "xhigh", "max"]),
                default: "medium".into(),
            }
        );
        assert_eq!(
            ThinkingScheme::from_legacy_input("off，low， high", "low"),
            ThinkingScheme::Levels {
                values: owned(&["off", "low", "high"]),
                default: "low".into(),
            }
        );
        assert_eq!(
            ThinkingScheme::from_legacy_input("off", "off"),
            ThinkingScheme::Unsupported
        );
        assert_eq!(
            ThinkingScheme::from_legacy_input("", ""),
            ThinkingScheme::Unsupported
        );
        assert!(ThinkingScheme::from_legacy_input("low, high", "max")
            .error(None)
            .is_some());
        assert_eq!(
            ThinkingScheme::from_legacy_input("off, high", "off"),
            ThinkingScheme::Toggle {
                on: "high".into(),
                default_on: false,
            }
        );
    }

    #[test]
    fn validation_checks_defaults_and_budgets() {
        let levels = ThinkingScheme::Levels {
            values: owned(&["low", "high"]),
            default: "max".into(),
        };
        assert!(levels.error(None).is_some());
        let budget = ThinkingScheme::Budget {
            presets: vec![16384, 65536],
            default: BudgetChoice::Tokens(16384),
            dynamic: false,
            allow_off: true,
        };
        assert!(
            budget.error(Some(65536)).is_some(),
            "budget must stay below output"
        );
        assert!(budget.error(Some(128000)).is_none());
        let off_not_allowed = ThinkingScheme::Budget {
            presets: vec![8192],
            default: BudgetChoice::Off,
            dynamic: true,
            allow_off: false,
        };
        assert!(off_not_allowed.error(None).is_some());
    }

    #[test]
    fn summaries_use_native_names() {
        let levels = ThinkingScheme::Levels {
            values: owned(&["minimal", "low", "medium", "high"]),
            default: "medium".into(),
        };
        assert_eq!(levels.summary(), "思考 minimal–high");
        assert_eq!(value_label("budget-16384"), "16,384");
        assert_eq!(value_label("budget-16000"), "16K");
        assert_eq!(value_label("xhigh"), "xhigh");
    }
    fn budget(
        presets: &[u32],
        default: BudgetChoice,
        dynamic: bool,
        allow_off: bool,
    ) -> ThinkingScheme {
        ThinkingScheme::Budget {
            presets: presets.to_vec(),
            default,
            dynamic,
            allow_off,
        }
    }

    #[test]
    fn picking_a_kind_starts_from_documented_defaults() {
        assert_eq!(
            ThinkingScheme::default_for(ThinkingKind::Toggle),
            ThinkingScheme::Toggle {
                on: "high".into(),
                default_on: false
            }
        );
        assert_eq!(
            ThinkingScheme::default_for(ThinkingKind::Always),
            ThinkingScheme::Always {
                value: "high".into()
            }
        );
        assert_eq!(
            ThinkingScheme::default_for(ThinkingKind::Levels),
            ThinkingScheme::Levels {
                values: owned(&["low", "medium", "high"]),
                default: "medium".into()
            }
        );
        assert_eq!(
            ThinkingScheme::default_for(ThinkingKind::Budget),
            budget(&[4_000, 16_000, 32_000], BudgetChoice::Off, false, true)
        );
        for kind in ThinkingKind::ALL {
            let scheme = ThinkingScheme::default_for(kind);
            assert_eq!(scheme.kind(), kind);
            assert_eq!(scheme.error(Some(64_000)), None, "{kind:?}");
            let (values, default) = scheme.encode();
            assert_eq!(ThinkingScheme::decode(&values, &default), scheme);
        }
    }

    #[test]
    fn budget_defaults_follow_the_available_options() {
        let mut s = budget(&[4_000, 16_000], BudgetChoice::Off, true, true);
        assert_eq!(
            s.budget_options(),
            vec![
                BudgetChoice::Off,
                BudgetChoice::Dynamic,
                BudgetChoice::Tokens(4_000),
                BudgetChoice::Tokens(16_000)
            ]
        );
        // Turning off the switch behind the default moves it to the first option.
        s.set_budget_allow_off(false);
        assert_eq!(
            s,
            budget(&[4_000, 16_000], BudgetChoice::Dynamic, true, false)
        );
        s.set_budget_dynamic(false);
        assert_eq!(
            s,
            budget(&[4_000, 16_000], BudgetChoice::Tokens(4_000), false, false)
        );
        // Deleting the default preset moves to the first remaining option.
        s.delete_budget(0);
        assert_eq!(
            s,
            budget(&[16_000], BudgetChoice::Tokens(16_000), false, false)
        );
        s.set_budget_allow_off(true);
        assert!(s.set_budget_default(BudgetChoice::Off));
        assert!(!s.set_budget_default(BudgetChoice::Dynamic));
        s.delete_budget(0);
        assert_eq!(s, budget(&[], BudgetChoice::Off, false, true));
        assert_eq!(s.error(None).unwrap(), "至少保留一个预算，或改成“不思考”");
        // Nothing left at all: the default stays and validation reports it.
        s.set_budget_allow_off(false);
        assert_eq!(s.budget_options(), vec![]);
        assert!(s.error(None).is_some());
        assert_eq!(s.add_budget("8K", None), Ok(8_000));
        assert_eq!(
            s,
            budget(&[8_000], BudgetChoice::Tokens(8_000), false, false)
        );
        s.delete_budget(9);
        assert_eq!(
            s,
            budget(&[8_000], BudgetChoice::Tokens(8_000), false, false)
        );
    }

    #[test]
    fn adding_budgets_checks_text_duplicates_and_output() {
        let mut s = budget(&[4_000, 32_000], BudgetChoice::Off, false, true);
        assert_eq!(s.add_budget("16K", Some(64_000)), Ok(16_000));
        assert_eq!(s.add_budget("1K", Some(64_000)), Ok(1_000));
        assert_eq!(
            s,
            budget(
                &[1_000, 4_000, 16_000, 32_000],
                BudgetChoice::Off,
                false,
                true
            ),
            "presets stay sorted"
        );
        assert_eq!(
            s.add_budget("8千", None),
            Err("写成 8K 或 8192 这样的数字".into())
        );
        assert_eq!(
            s.add_budget("", None),
            Err("写成 8K 或 8192 这样的数字".into())
        );
        assert_eq!(s.add_budget("16000", None), Err("已经有 16K".into()));
        assert_eq!(
            s.add_budget("64K", Some(64_000)),
            Err("需小于最长输出 64K".into())
        );
        let mut levels = ThinkingScheme::default_for(ThinkingKind::Levels);
        assert!(levels.add_budget("8K", None).is_err());
        // Validation names every preset that is not below the output limit.
        let problem = s.problem(Some(16_000)).unwrap();
        assert_eq!(problem.message, "预算需小于最长输出 16K");
        assert_eq!(problem.bad_budgets, vec![16_000, 32_000]);
        assert_eq!(s.problem(Some(64_000)), None);
    }

    #[test]
    fn levels_insert_in_canonical_order_and_keep_a_default() {
        let mut s = ThinkingScheme::Levels {
            values: owned(&["low", "high"]),
            default: "high".into(),
        };
        assert_eq!(
            s.addable_levels(),
            vec!["minimal", "medium", "xhigh", "max"]
        );
        s.add_level("medium").unwrap();
        s.add_level("minimal").unwrap();
        s.add_level("max").unwrap();
        s.add_level("turbo").unwrap();
        s.add_level("xhigh").unwrap();
        assert_eq!(
            s,
            ThinkingScheme::Levels {
                values: owned(&["minimal", "low", "medium", "high", "xhigh", "max", "turbo"]),
                default: "high".into(),
            }
        );
        assert_eq!(s.add_level("  "), Err("写一个名称".into()));
        assert_eq!(s.add_level("low"), Err("已经有 low".into()));
        assert!(s.add_level("very high").unwrap_err().contains("只用字母"));
        assert!(s.add_level("高").unwrap_err().contains("只用字母"));
        assert!(s.add_level("dynamic").is_err());
        assert!(s.add_level("budget-100").is_err());
        assert!(s.add_level(&"x".repeat(41)).is_err());
        s.add_level(&"x".repeat(40)).unwrap();

        // Deleting the default moves it to the next level, or the previous at the end.
        let mut s = ThinkingScheme::Levels {
            values: owned(&["low", "medium", "high"]),
            default: "medium".into(),
        };
        assert!(s.delete_level(1));
        assert_eq!(
            s,
            ThinkingScheme::Levels {
                values: owned(&["low", "high"]),
                default: "high".into()
            }
        );
        assert!(s.delete_level(1));
        assert_eq!(
            s,
            ThinkingScheme::Levels {
                values: owned(&["low"]),
                default: "low".into()
            }
        );
        assert!(!s.delete_level(0), "the last level stays");
        assert!(!s.delete_level(5));

        let mut s = ThinkingScheme::Levels {
            values: owned(&["low", "medium", "high"]),
            default: "medium".into(),
        };
        assert!(s.move_level(0, 2));
        assert!(!s.move_level(0, 3));
        assert!(!s.move_level(1, 1));
        assert!(s.set_level_default("low"));
        assert!(!s.set_level_default("max"));
        assert_eq!(
            s,
            ThinkingScheme::Levels {
                values: owned(&["medium", "high", "low"]),
                default: "low".into()
            }
        );
    }

    #[test]
    fn level_validation_covers_empty_duplicate_and_invalid_names() {
        let levels = |values: &[&str], default: &str| ThinkingScheme::Levels {
            values: owned(values),
            default: default.into(),
        };
        assert_eq!(levels(&[], "").error(None).unwrap(), "至少保留一个档位");
        assert_eq!(
            levels(&["low", "low"], "low").error(None).unwrap(),
            "档位不能重复"
        );
        assert!(levels(&["low", "a b"], "low")
            .error(None)
            .unwrap()
            .contains("只用字母"));
        assert!(levels(&["low", ""], "low").error(None).is_some());
        assert_eq!(levels(&["low", "high"], "low").error(None), None);
    }

    #[test]
    fn panel_and_descriptions_read_like_the_composer() {
        assert_eq!(ThinkingScheme::Unsupported.panel(), Panel::Hidden);
        assert_eq!(
            ThinkingScheme::default_for(ThinkingKind::Always).panel(),
            Panel::Fixed
        );
        assert_eq!(
            ThinkingScheme::default_for(ThinkingKind::Toggle).panel(),
            Panel::Options {
                options: owned(&["关", "开"]),
                selected: 0
            }
        );
        assert_eq!(
            ThinkingScheme::default_for(ThinkingKind::Levels).panel(),
            Panel::Options {
                options: owned(&["low", "medium", "high"]),
                selected: 1
            }
        );
        let s = budget(&[4_000, 16_384], BudgetChoice::Tokens(16_384), true, true);
        assert_eq!(
            s.panel(),
            Panel::Options {
                options: owned(&["关", "自动", "4K", "16,384"]),
                selected: 3
            }
        );
        assert_eq!(s.describe(), "预算 4K / 16,384 / 自动 · 默认 16,384");
        assert_eq!(s.short(), "预算 · 默认 16,384");
        let toggle = ThinkingScheme::Toggle {
            on: "high".into(),
            default_on: true,
        };
        assert_eq!(toggle.describe(), "可以开关 · 默认开");
        assert_eq!(toggle.short(), "可开关 · 默认开");
        let levels = ThinkingScheme::default_for(ThinkingKind::Levels);
        assert_eq!(levels.describe(), "档位 low / medium / high · 默认 medium");
        assert_eq!(levels.short(), "档位 · 默认 medium");
        assert_eq!(ThinkingScheme::Unsupported.describe(), "不思考");
        assert_eq!(ThinkingScheme::Unsupported.short(), "不思考");
        assert_eq!(
            ThinkingScheme::default_for(ThinkingKind::Always).short(),
            "总是思考"
        );
        assert_eq!(
            budget(&[4_000], BudgetChoice::Off, false, true).short(),
            "预算 · 默认 关"
        );
    }
}
