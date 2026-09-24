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
    Levels { values: Vec<String>, default: String },
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
        match self {
            Self::Unsupported => None,
            Self::Always { value } | Self::Toggle { on: value, .. } => {
                (value.trim().is_empty() || value == OFF).then(|| "请填写思考时发送的值。".into())
            }
            Self::Levels { values, default } => {
                if values.is_empty() || values.iter().any(|v| v.trim().is_empty()) {
                    Some("至少需要一个思考档位。".into())
                } else if dedupe(values).len() != values.len() {
                    Some("思考档位不能重复。".into())
                } else if !values.contains(default) {
                    Some("默认档位须在可选档位中。".into())
                } else {
                    None
                }
            }
            Self::Budget {
                presets,
                default,
                dynamic,
                allow_off,
            } => {
                if presets.is_empty() || presets.iter().any(|t| *t == 0) {
                    return Some("至少需要一个大于 0 的思考预算。".into());
                }
                if let Some(output) = output {
                    if let Some(max) = presets.iter().max().filter(|max| **max >= output) {
                        return Some(format!(
                            "思考预算（{}）需小于最长输出（{}）。",
                            crate::model_edit::compact_tokens(u64::from(*max)),
                            crate::model_edit::compact_tokens(u64::from(output))
                        ));
                    }
                }
                let valid = match default {
                    BudgetChoice::Off => *allow_off,
                    BudgetChoice::Dynamic => *dynamic,
                    BudgetChoice::Tokens(t) => presets.contains(t),
                };
                (!valid).then(|| "默认预算须在可选项中。".into())
            }
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
        assert!(budget.error(Some(65536)).is_some(), "budget must stay below output");
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
        assert_eq!(value_label("budget-16384"), "16.384K");
        assert_eq!(value_label("budget-16000"), "16K");
        assert_eq!(value_label("xhigh"), "xhigh");
    }
}
