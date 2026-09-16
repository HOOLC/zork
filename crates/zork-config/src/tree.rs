//! Stable addresses in a Synch space. These are file references, not installs
//! or an allowlist of application resource kinds.
use anyhow::{ensure, Context, Result};
use serde::{Deserialize, Serialize};

/// Preserve the namespace used by existing Station file references.
pub const STATION_FILES_SPACE: &str = "files";
pub const SHARED_FILES_SPACE: &str = "shared";
pub const SKILLS_SPACE: &str = "skills";

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Reference {
    pub space: String,
    pub path: String,
    pub origin: Option<String>,
    pub root: Option<String>,
    /// A committed origin-tree root, inherited by all relative reads.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub snapshot: Option<String>,
}
impl Reference {
    pub fn parse(value: &str) -> Result<Self> {
        let value = value
            .strip_prefix("synch://")
            .context("Synch file reference required")?;
        let (address, query) = value.split_once('?').unwrap_or((value, ""));
        let (space, path) = address.split_once('/').unwrap_or((address, ""));
        let mut result = Self {
            space: decode(space)?,
            path: decode(path)?,
            origin: None,
            root: None,
            snapshot: None,
        };
        for (key, value) in url::form_urlencoded::parse(query.as_bytes()) {
            match key.as_ref() {
                "origin" => {
                    ensure!(
                        result.origin.is_none() && !value.is_empty(),
                        "invalid origin selector"
                    );
                    result.origin = Some(value.into_owned());
                }
                "root" | "snapshot" => {
                    let selector = if key == "root" {
                        &mut result.root
                    } else {
                        &mut result.snapshot
                    };
                    ensure!(
                        selector.is_none()
                            && value.len() == 64
                            && value.bytes().all(|b| b.is_ascii_hexdigit()),
                        "invalid content root"
                    );
                    *selector = Some(value.into_owned());
                }
                _ => anyhow::bail!("unknown Synch reference parameter"),
            }
        }
        ensure!(
            result.snapshot.is_none() || result.origin.is_some(),
            "a tree snapshot requires its origin"
        );
        ensure!(
            !result.space.is_empty()
                && result.space.len() <= 255
                && !result.space.contains('/')
                && !result.space.chars().any(char::is_control),
            "invalid space"
        );
        ensure!(
            result.path.len() <= 4096
                && (result.path.is_empty()
                    || result.path.split('/').all(|p| !p.is_empty()
                        && !matches!(p, "." | "..")
                        && !p.chars().any(char::is_control))),
            "invalid tree path"
        );
        Ok(result)
    }
    pub fn uri(&self) -> String {
        let mut result = format!(
            "synch://{}/{}",
            encode(&self.space),
            self.path
                .split('/')
                .map(encode)
                .collect::<Vec<_>>()
                .join("/")
        );
        let mut query = url::form_urlencoded::Serializer::new(String::new());
        if let Some(origin) = &self.origin {
            query.append_pair("origin", origin);
        }
        if let Some(root) = &self.root {
            query.append_pair("root", root);
        }
        if let Some(snapshot) = &self.snapshot {
            query.append_pair("snapshot", snapshot);
        }
        let query = query.finish();
        if !query.is_empty() {
            result.push('?');
            result.push_str(&query);
        }
        result
    }
    pub fn child(&self, path: &str) -> Self {
        Self {
            space: self.space.clone(),
            path: if self.path.is_empty() {
                path.into()
            } else {
                format!("{}/{path}", self.path)
            },
            origin: self.origin.clone(),
            root: None,
            snapshot: self.snapshot.clone(),
        }
    }
}
fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| {
            if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
                (b as char).to_string()
            } else {
                format!("%{b:02X}")
            }
        })
        .collect()
}
fn decode(value: &str) -> Result<String> {
    let mut bytes = Vec::new();
    let mut input = value.bytes();
    while let Some(b) = input.next() {
        if b == b'%' {
            let a = input
                .next()
                .and_then(|b| (b as char).to_digit(16))
                .context("invalid path escape")?;
            let b = input
                .next()
                .and_then(|b| (b as char).to_digit(16))
                .context("invalid path escape")?;
            bytes.push((a * 16 + b) as u8);
        } else {
            bytes.push(b);
        }
    }
    Ok(String::from_utf8(bytes)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn references_preserve_namespace_case_names_and_version_identity() {
        let reference = Reference {
            space: "Design + Assets".into(),
            path: "skills/设计+验证/SKILL.md".into(),
            origin: Some("key:device".into()),
            root: Some("a".repeat(64)),
            snapshot: None,
        };
        assert_eq!(Reference::parse(&reference.uri()).unwrap(), reference);
        assert!(Reference::parse("synch://zork/a/%2E%2E/private").is_err());
        assert!(Reference::parse("synch://zork/skills?origin=a&origin=b").is_err());
    }
}
