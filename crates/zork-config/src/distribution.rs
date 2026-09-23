//! Test package sources are explicit installation metadata, separate from Mesh services.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TestDistribution {
    pub base_url: String,
    pub version: String,
    #[serde(default)]
    pub install_page: Option<String>,
}

impl TestDistribution {
    pub fn fragment_fields(&self) -> String {
        url::form_urlencoded::Serializer::new(String::new())
            .append_pair("source", &self.base_url)
            .append_pair("version", &self.version)
            .finish()
    }
    pub fn load(root: &Path) -> Result<Self> {
        let local = root.join("test-distribution.json");
        let path = std::env::var_os("ZORK_TEST_DISTRIBUTION")
            .map(PathBuf::from)
            .or_else(|| local.is_file().then_some(local))
            .or_else(|| {
                std::env::current_exe()
                    .ok()?
                    .parent()?
                    .ancestors()
                    .filter(|p| p.file_name().is_some_and(|n| n == "Contents"))
                    .map(|p| p.join("Resources/test-distribution.json"))
                    .find(|p| p.is_file())
            })
            .context("test 安装包来源未配置；请配置 test-distribution.json")?;
        let source: Self = serde_json::from_slice(&std::fs::read(path)?)?;
        if let Some(page) = &source.install_page {
            let url = url::Url::parse(page)?;
            ensure!(
                matches!(url.scheme(), "http" | "https")
                    && url.host_str().is_some()
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.query().is_none()
                    && url.fragment().is_none(),
                "invalid test installation page"
            );
        }
        let url = url::Url::parse(&source.base_url)?;
        ensure!(
            matches!(url.scheme(), "https" | "http")
                && url.host_str().is_some()
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none(),
            "invalid test package source"
        );
        ensure!(
            !source.version.is_empty()
                && source
                    .version
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || ".-".contains(c)),
            "invalid test package version"
        );
        Ok(source)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_test_source_is_an_error_and_local_source_has_no_credentials() {
        let root = tempfile::tempdir().unwrap();
        assert!(TestDistribution::load(root.path()).is_err());
        let path = root.path().join("test-distribution.json");
        std::fs::write(
            &path,
            r#"{"base_url":"http://192.168.1.2:19000/test/build","version":"1.2.3"}"#,
        )
        .unwrap();
        let source = TestDistribution::load(root.path()).unwrap();
        assert!(source.fragment_fields().contains("source=http%3A"));
        std::fs::write(
            path,
            r#"{"base_url":"https://secret:password@example.com/test","version":"1.2.3"}"#,
        )
        .unwrap();
        assert!(TestDistribution::load(root.path()).is_err());
    }
}
