//! Parsing the `[<origin>:]<space>[/<path>]` references the CLI and the S3
//! gateway both take (§9.2, §9.4).

use std::str::FromStr;

use synch_core::{normalize_path, validate_space, OriginId};

use crate::error::EngineError;

/// A reference to a space, a directory, or a single path, optionally scoped to
/// one origin.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EntryRef {
    /// The origin, or `None` for the merged view across all origins.
    pub origin: Option<OriginId>,
    /// The space id.
    pub space: String,
    /// The normalized path within the space, empty for the space root.
    pub path: String,
}

impl EntryRef {
    /// True if this reference names the space root rather than a path.
    pub fn is_space_root(&self) -> bool {
        self.path.is_empty()
    }

    /// The prefix to scan for a listing: the path plus a trailing slash, or
    /// empty at the space root.
    pub fn dir_prefix(&self) -> String {
        if self.path.is_empty() || self.path.ends_with('/') {
            self.path.clone()
        } else {
            format!("{}/", self.path)
        }
    }

    /// Renders the reference back to its canonical text form.
    pub fn render(&self) -> String {
        let body = if self.path.is_empty() {
            self.space.clone()
        } else {
            format!("{}/{}", self.space, self.path)
        };
        match &self.origin {
            Some(origin) => format!("{}:{body}", origin.canonical()),
            None => body,
        }
    }
}

impl std::fmt::Display for EntryRef {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.render())
    }
}

impl FromStr for EntryRef {
    type Err = EngineError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        if text.is_empty() {
            return Err(EngineError::invalid("empty reference"));
        }
        // The origin is everything before the last ':' that precedes the first
        // '/', which keeps the `key:<z-base-32>` form unambiguous.
        let boundary = text.find('/').unwrap_or(text.len());
        let (origin, rest) = match text[..boundary].rfind(':') {
            Some(idx) => {
                let origin = OriginId::from_str(&text[..idx])
                    .map_err(|e| EngineError::invalid(format!("bad origin: {e}")))?;
                (Some(origin), &text[idx + 1..])
            }
            None => (None, text),
        };

        let (space, path) = match rest.split_once('/') {
            Some((space, path)) => (space, path),
            None => (rest, ""),
        };
        validate_space(space)?;
        let path = if path.is_empty() {
            String::new()
        } else {
            // A trailing slash names a directory; keep it off the normalized
            // form and let `dir_prefix` put it back.
            normalize_path(path.trim_end_matches('/'))
                .map_err(|e| EngineError::invalid(e.to_string()))?
        };
        Ok(EntryRef {
            origin,
            space: space.to_string(),
            path,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_full_reference() {
        // (input, origin, space, path, render). The `key:<z32>` colon boundary
        // is the case a misparse would misroute every address on.
        let key = iroh_base::SecretKey::generate().public();
        let key_text = format!("key:{}:media/a.txt", key.to_z32());
        let cases: &[(&str, Option<OriginId>, &str, &str, &str)] = &[
            (
                "nas@cluster.example:media/talks/keynote.mp4",
                Some(OriginId::named("nas", "cluster.example").unwrap()),
                "media",
                "talks/keynote.mp4",
                "nas@cluster.example:media/talks/keynote.mp4",
            ),
            ("media/talks", None, "media", "talks", "media/talks"),
            (
                &key_text,
                Some(OriginId::Key(key)),
                "media",
                "a.txt",
                &key_text,
            ),
            ("media/talks/", None, "media", "talks", "media/talks"),
            (
                "media/cafe\u{0301}.txt",
                None,
                "media",
                "caf\u{00e9}.txt",
                "media/caf\u{00e9}.txt",
            ),
        ];
        for (input, origin, space, path, render) in cases {
            let r: EntryRef = input.parse().unwrap();
            assert_eq!(r.origin.as_ref(), origin.as_ref(), "origin of {input}");
            assert_eq!(r.space, *space, "space of {input}");
            assert_eq!(r.path, *path, "path of {input}");
            assert_eq!(r.render(), *render, "render of {input}");
        }
        assert_eq!(
            "media/talks".parse::<EntryRef>().unwrap().dir_prefix(),
            "talks/"
        );
        assert!("media".parse::<EntryRef>().unwrap().is_space_root());

        for bad in [
            "",
            "media/../escape",
            "not an origin:media/a",
            "nas@x.example:/media",
        ] {
            assert!(bad.parse::<EntryRef>().is_err(), "{bad:?} must be rejected");
        }
    }
}
