//! Public service endpoints: packaged defaults, then a device-local replacement.
//! This file never contains model credentials or account tokens.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ServicesConfig {
    pub relay_urls: Option<Vec<String>>,
    pub discovery_url: Option<String>,
}
impl ServicesConfig {
    pub fn packaged_defaults() -> Self {
        serde_json::from_str(include_str!("services.default.json"))
            .expect("packaged service defaults")
    }
    pub fn load_from_install() -> Result<Self> {
        Self::installed(None)
    }
    pub fn load_for_data_root(root: &Path) -> Result<Self> {
        Self::installed(Some(root))
    }
    fn installed(root: Option<&Path>) -> Result<Self> {
        let bundled = std::env::current_exe().ok().and_then(|exe| {
            exe.parent()
                .map(|dir| dir.join("../Resources/services.json"))
        });
        let explicit = std::env::var_os("ZORK_SERVICES_CONFIG").map(std::path::PathBuf::from);
        if let Some(path) = &explicit {
            ensure!(path.is_file(), "ZORK_SERVICES_CONFIG file does not exist");
        }
        let user = explicit.or_else(|| root.map(|root| root.join("services.json")));
        Self::load(bundled.as_deref(), user.as_deref())
    }

    /// The override replaces individual top-level endpoint fields.
    pub fn load(bundled: Option<&Path>, user: Option<&Path>) -> Result<Self> {
        let mut merged =
            serde_json::to_value(Self::packaged_defaults()).context("packaged service defaults")?;
        for path in [bundled, user].into_iter().flatten() {
            if !path.exists() {
                continue;
            }
            let value: serde_json::Value = serde_json::from_slice(&std::fs::read(path)?)
                .with_context(|| format!("read service configuration {}", path.display()))?;
            let fields = value
                .as_object()
                .context("services configuration must be an object")?;
            // Validate each source as well as the effective result; typos in a
            // packaged file must not disappear behind an override.
            serde_json::from_value::<Self>(value.clone())?;
            for (key, value) in fields {
                merged[key] = value.clone();
            }
        }
        let result: Self = serde_json::from_value(merged)?;
        result.validate()?;
        Ok(result)
    }
    /// Explicit service endpoints override persisted transport settings. Omitted
    /// fields preserve invitation/node settings; identity and grants are untouched.
    pub fn apply_network(&self, mesh: &mut crate::MeshConfig) -> Result<()> {
        self.validate()?;
        if let Some(relays) = &self.relay_urls {
            mesh.relay_urls = Some(relays.clone());
        }
        if let Some(discovery) = &self.discovery_url {
            mesh.discovery_url = Some(discovery.clone());
        }
        Ok(())
    }

    pub fn validate(&self) -> Result<()> {
        if let Some(relays) = &self.relay_urls {
            ensure!(
                !relays.is_empty() && relays.len() <= 8,
                "relay_urls must contain 1–8 URLs; use mesh.offline for offline mode"
            );
            for relay in relays {
                validate_endpoint(relay)?;
            }
        }
        if let Some(url) = &self.discovery_url {
            validate_endpoint(url)?;
        }
        Ok(())
    }
}
pub fn validate_endpoint(value: &str) -> Result<()> {
    let url = url::Url::parse(value).context("invalid service URL")?;
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "service URLs must not contain credentials, queries or fragments"
    );
    let loopback = matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"));
    ensure!(
        url.scheme() == "https" || (url.scheme() == "http" && loopback),
        "service URLs require HTTPS (HTTP is allowed for local tests)"
    );
    ensure!(url.host_str().is_some(), "service URL requires a host");
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn overrides_replace_endpoint_sets() {
        let dir = tempfile::tempdir().unwrap();
        let bundled = dir.path().join("bundled.json");
        let user = dir.path().join("user.json");
        std::fs::write(&bundled, r#"{"relay_urls":["https://old.example"]}"#).unwrap();
        std::fs::write(&user, r#"{"relay_urls":["http://127.0.0.1:3340"]}"#).unwrap();
        let effective = ServicesConfig::load(Some(&bundled), Some(&user)).unwrap();
        assert_eq!(effective.relay_urls.unwrap(), vec!["http://127.0.0.1:3340"]);
        std::fs::write(
            &user,
            r#"{"account":{"issuer":"https://unrelated.example"}}"#,
        )
        .unwrap();
        assert!(ServicesConfig::load(Some(&bundled), Some(&user)).is_err());
    }
    #[test]
    fn network_overrides_replace_stale_endpoints_without_changing_node_policy() {
        let mut mesh = crate::MeshConfig {
            name: "existing node".into(),
            enabled: true,
            offline: true,
            bind: Some("127.0.0.1:1234".into()),
            relay_urls: Some(vec!["https://old.example".into()]),
            discovery_url: Some("https://old.example/pkarr".into()),
            ..Default::default()
        };
        let original = mesh.clone();
        let services = ServicesConfig {
            relay_urls: Some(vec!["https://new.example".into()]),
            ..Default::default()
        };
        services.apply_network(&mut mesh).unwrap();
        assert_eq!(mesh.relay_urls, services.relay_urls);
        let mut expected = original.clone();
        expected.relay_urls = services.relay_urls;
        assert_eq!(mesh, expected);
        ServicesConfig::default().apply_network(&mut mesh).unwrap();
        assert_eq!(mesh, expected);
        let invalid = ServicesConfig {
            relay_urls: Some(vec!["https://another.example".into()]),
            discovery_url: Some("http://insecure.example".into()),
            ..Default::default()
        };
        assert!(invalid.apply_network(&mut mesh).is_err());
        assert_eq!(
            mesh, expected,
            "invalid settings partially changed the node"
        );
    }

    #[test]
    fn rejects_insecure_remote_and_credential_urls() {
        for url in [
            "http://remote.example",
            "https://a:b@example.com",
            "https://example.com?token=secret",
            "file:///tmp/x",
        ] {
            assert!(validate_endpoint(url).is_err(), "{url}");
        }
        assert!(validate_endpoint("http://127.0.0.1:4200").is_ok());
        assert!(validate_endpoint("https://relay.example").is_ok());
    }
}
