//! Public service endpoints: packaged defaults, then a device-local replacement.
//! Public endpoints only; account credentials are never service configuration.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

fn bundled_services(executable: &Path) -> Option<PathBuf> {
    executable
        .parent()?
        .ancestors()
        .filter(|path| path.file_name().is_some_and(|name| name == "Contents"))
        .map(|contents| contents.join("Resources/services.json"))
        .find(|path| path.is_file())
}

#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default, deny_unknown_fields)]
pub struct ServicesConfig {
    pub relay_urls: Option<Vec<String>>,
    pub relay_quic_port: Option<u16>,
    pub discovery_url: Option<String>,
    pub quic_discovery_urls: Option<Vec<String>>,
    pub cue: Option<CueAccountConfig>,
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct CueAccountConfig {
    pub issuer: String,
    pub client_id: String,
    #[serde(default = "default_cue_redirect")]
    pub redirect_uri: String,
}
fn default_cue_redirect() -> String {
    "http://127.0.0.1:43025/oauth/callback".into()
}
pub fn validate_cue_redirect(value: &str) -> Result<()> {
    let url = url::Url::parse(value)?;
    ensure!(
        url.scheme() == "http"
            && url.host_str() == Some("127.0.0.1")
            && url.port().is_some_and(|p| p > 0)
            && url.path() == "/oauth/callback"
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "Cue redirect_uri must be http://127.0.0.1:<port>/oauth/callback and registered with Cue"
    );
    Ok(())
}
impl ServicesConfig {
    /// Compiled-in public Mesh endpoints. Packaged and user files overlay this.
    pub fn packaged_defaults() -> Self {
        serde_json::from_str(include_str!("services.default.json"))
            .expect("packaged service defaults")
    }

    /// Packaged defaults, then the app bundle file, then `ZORK_SERVICES_CONFIG`.
    pub fn load_from_install() -> Result<Self> {
        let bundled = std::env::current_exe()
            .ok()
            .and_then(|exe| bundled_services(&exe));
        let user = std::env::var_os("ZORK_SERVICES_CONFIG").map(std::path::PathBuf::from);
        Self::load(bundled.as_deref(), user.as_deref())
    }

    /// The override replaces individual top-level fields. In particular `cue`
    /// is replaced atomically so an issuer never inherits another client's ID.
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
        if let Some(port) = self.relay_quic_port {
            mesh.relay_quic_port = Some(port);
        }
        if let Some(discovery) = &self.discovery_url {
            mesh.discovery_url = Some(discovery.clone());
        }
        if let Some(urls) = &self.quic_discovery_urls {
            mesh.quic_discovery_urls = Some(urls.clone());
        }
        Ok(())
    }

    /// Fills omitted endpoint choices without rewriting a persisted configuration.
    pub fn apply_defaults(&self, mesh: &mut crate::MeshConfig) -> Result<()> {
        self.validate()?;
        if mesh.relay_urls.is_none() {
            mesh.relay_urls = self.relay_urls.clone();
        }
        if mesh.relay_quic_port.is_none() {
            mesh.relay_quic_port = self.relay_quic_port;
        }
        if mesh.discovery_url.is_none() {
            mesh.discovery_url = self.discovery_url.clone();
        }
        if mesh.quic_discovery_urls.is_none() {
            mesh.quic_discovery_urls = self.quic_discovery_urls.clone();
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
        ensure!(
            self.relay_quic_port != Some(0),
            "relay_quic_port must be a real UDP port"
        );
        if let Some(url) = &self.discovery_url {
            validate_endpoint(url)?;
        }
        if let Some(urls) = &self.quic_discovery_urls {
            ensure!(
                urls.len() <= 8,
                "quic_discovery_urls supports at most 8 servers"
            );
            for url in urls {
                validate_endpoint(url)?;
            }
        }
        if let Some(cue) = &self.cue {
            validate_endpoint(&cue.issuer)?;
            validate_cue_redirect(&cue.redirect_uri)?;
            ensure!(
                !cue.client_id.trim().is_empty() && cue.client_id.len() <= 512,
                "Cue client_id is required"
            );
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
    fn embedded_helpers_inherit_the_outer_app_services() {
        let root = tempfile::tempdir().unwrap();
        let contents = root.path().join("Zork.app/Contents");
        let services = contents.join("Resources/services.json");
        std::fs::create_dir_all(services.parent().unwrap()).unwrap();
        std::fs::write(&services, r#"{"relay_urls":["https://relay.example"]}"#).unwrap();
        for executable in [
            contents.join("MacOS/zork-gui"),
            contents.join("Helpers/ZorkStation.app/Contents/MacOS/zork-station"),
        ] {
            let bundled = bundled_services(&executable).unwrap();
            assert_eq!(
                ServicesConfig::load(Some(&bundled), None)
                    .unwrap()
                    .relay_urls,
                Some(vec!["https://relay.example".into()])
            );
        }
    }
    #[test]
    fn packaged_defaults_are_the_zork_relay() {
        let defaults = ServicesConfig::packaged_defaults();
        assert_eq!(
            defaults.relay_urls,
            Some(vec!["https://relay.zork.ing".into()])
        );
        assert_eq!(
            defaults.discovery_url.as_deref(),
            Some("https://relay.zork.ing/pkarr")
        );
        let empty = tempfile::tempdir().unwrap();
        let bundled = empty.path().join("services.json");
        std::fs::write(&bundled, "{}").unwrap();
        let loaded = ServicesConfig::load(Some(&bundled), None).unwrap();
        assert_eq!(loaded.relay_urls, defaults.relay_urls);
        assert_eq!(loaded.discovery_url, defaults.discovery_url);
    }

    #[test]
    fn overrides_replace_endpoint_sets_and_account_together() {
        let dir = tempfile::tempdir().unwrap();
        let bundled = dir.path().join("bundled.json");
        let user = dir.path().join("user.json");
        std::fs::write(&bundled,r#"{"relay_urls":["https://old.example"],"cue":{"issuer":"https://old.example","client_id":"old"}}"#).unwrap();
        std::fs::write(&user,r#"{"relay_urls":["http://127.0.0.1:3340"],"cue":{"issuer":"http://127.0.0.1:4200","client_id":"local-zork"}}"#).unwrap();
        let effective = ServicesConfig::load(Some(&bundled), Some(&user)).unwrap();
        assert_eq!(effective.relay_urls.unwrap(), vec!["http://127.0.0.1:3340"]);
        assert_eq!(effective.cue.unwrap().client_id, "local-zork");
        std::fs::write(&user, r#"{"cue":{"issuer":"https://new.example"}}"#).unwrap();
        assert!(ServicesConfig::load(Some(&bundled), Some(&user)).is_err());
        std::fs::write(&user, r#"{"cue":null}"#).unwrap();
        assert!(ServicesConfig::load(Some(&bundled), Some(&user))
            .unwrap()
            .cue
            .is_none());
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
        assert!(validate_endpoint("https://cue.example").is_ok());
    }

    #[test]
    fn a_relay_quic_port_reaches_the_mesh_and_must_be_real() {
        let mut mesh = crate::MeshConfig::default();
        let services = ServicesConfig {
            relay_urls: Some(vec!["https://relay.example".into()]),
            relay_quic_port: Some(3478),
            ..Default::default()
        };
        services.apply_network(&mut mesh).unwrap();
        assert_eq!(mesh.relay_quic_port, Some(3478));
        let portless = ServicesConfig {
            relay_urls: Some(vec!["https://relay.example".into()]),
            ..Default::default()
        };
        portless.apply_network(&mut mesh).unwrap();
        assert_eq!(
            mesh.relay_quic_port,
            Some(3478),
            "a URL-only override keeps the port"
        );
        let zero = ServicesConfig {
            relay_quic_port: Some(0),
            ..Default::default()
        };
        assert!(zero.apply_network(&mut mesh).is_err());
    }
}
