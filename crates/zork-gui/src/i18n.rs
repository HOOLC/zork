//! Embedded UI catalogs. Keep IDs stable; translate presentation, never user data.
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, path::Path, sync::OnceLock};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum Locale {
    #[default]
    #[serde(rename = "zh-CN")]
    ZhCn,
    #[serde(rename = "en")]
    En,
}
impl Locale {
    pub const ALL: [Self; 2] = [Self::ZhCn, Self::En];
    pub fn code(self) -> &'static str {
        match self {
            Self::ZhCn => "zh-CN",
            Self::En => "en",
        }
    }
    pub fn parse(value: &str) -> Option<Self> {
        match value.replace('_', "-").to_ascii_lowercase().as_str() {
            "zh" | "zh-cn" | "zh-hans" => Some(Self::ZhCn),
            "en" | "en-us" | "en-gb" => Some(Self::En),
            _ => None,
        }
    }
    pub fn text<'a>(self, key: &'a str) -> &'a str {
        let catalogs = catalogs();
        let catalog = match self {
            Self::ZhCn => &catalogs.0,
            Self::En => &catalogs.1,
        };
        catalog
            .get(key)
            .or_else(|| catalogs.1.get(key))
            .map(String::as_str)
            .unwrap_or(key)
    }
}
type Catalog = BTreeMap<String, String>;
fn catalogs() -> &'static (Catalog, Catalog) {
    static CATALOGS: OnceLock<(Catalog, Catalog)> = OnceLock::new();
    CATALOGS.get_or_init(|| {
        (
            serde_json::from_str(include_str!("../locales/zh-CN.json"))
                .expect("invalid Chinese catalog"),
            serde_json::from_str(include_str!("../locales/en.json"))
                .expect("invalid English catalog"),
        )
    })
}

pub use zork_client_core::locale::preferences_path;
/// Explicit environment override, then the core preference, then Chinese.
pub fn load_locale(path: &Path, override_locale: Option<&str>) -> Locale {
    override_locale
        .and_then(Locale::parse)
        .or_else(|| {
            zork_client_core::locale::read_locale(path)
                .as_deref()
                .and_then(Locale::parse)
        })
        .unwrap_or_default()
}
pub fn save_locale(path: &Path, locale: Locale) -> std::io::Result<()> {
    zork_client_core::locale::save_locale(path, locale.code())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn catalogs_have_matching_nonempty_keys_and_cover_literal_calls() {
        let (zh, en) = catalogs();
        assert_eq!(zh.keys().collect::<Vec<_>>(), en.keys().collect::<Vec<_>>());
        assert!(zh.values().chain(en.values()).all(|s| !s.trim().is_empty()));
        for source in [
            include_str!("views.rs"),
            include_str!("views/drive.rs"),
            include_str!("shell.rs"),
        ] {
            for suffix in source.split(".text(\"").skip(1) {
                let key = suffix.split('"').next().unwrap();
                assert!(en.contains_key(key), "missing {key}");
            }
        }
    }
    #[test]
    fn locale_precedence_and_atomic_preferences_preserve_other_fields() {
        let dir = std::env::temp_dir().join(format!("zork-i18n-{}", ulid::Ulid::new()));
        let path = dir.join("preferences.json");
        assert_eq!(load_locale(&path, None), Locale::ZhCn);
        save_locale(&path, Locale::En).unwrap();
        assert_eq!(load_locale(&path, None), Locale::En);
        assert_eq!(load_locale(&path, Some("zh_Hans")), Locale::ZhCn);
        assert_eq!(load_locale(&path, Some("unsupported")), Locale::En);
        std::fs::write(&path, r#"{"locale":"en","other":42}"#).unwrap();
        save_locale(&path, Locale::ZhCn).unwrap();
        let value: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(value["other"], 42);
        std::fs::write(&path, b"invalid json").unwrap();
        assert_eq!(load_locale(&path, None), Locale::ZhCn);
        assert!(save_locale(&path, Locale::En).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"invalid json");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
