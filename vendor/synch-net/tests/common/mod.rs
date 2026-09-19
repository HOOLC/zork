//! Helpers the integration suites share: a tempfile with contents, a one-member TXT record set, and fixture readers.

/// A temp file holding `contents`; the caller keeps it alive by holding it.
#[allow(dead_code)] // each suite imports only the helpers it uses
pub(crate) fn write(contents: &str) -> tempfile::NamedTempFile {
    let file = tempfile::NamedTempFile::new().unwrap();
    std::fs::write(file.path(), contents).unwrap();
    file
}

/// One `v=sync1` membership record for a fresh node key on the test apex.
#[allow(dead_code)] // each suite imports only the helpers it uses
pub(crate) fn member_records() -> Vec<String> {
    vec![format!(
        "v=sync1 id=nas nk={} apex=cluster.example",
        iroh_base::SecretKey::generate().public().to_z32()
    )]
}

/// `dir` (relative to this crate's manifest) as a path.
#[allow(dead_code)] // each suite imports only the helpers it uses
pub(crate) fn fixture_dir(dir: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(dir)
}

/// The bytes of fixture `name` in `dir`.
#[allow(dead_code)] // each suite imports only the helpers it uses
pub(crate) fn fixture(dir: &str, name: &str) -> Vec<u8> {
    let path = fixture_dir(dir).join(name);
    std::fs::read(&path).unwrap_or_else(|e| panic!("fixture {}: {e}", path.display()))
}

/// The `name=value` field of `dir`'s `meta.txt`.
#[allow(dead_code)] // each suite imports only the helpers it uses
pub(crate) fn fixture_field(dir: &str, name: &str) -> String {
    let meta = String::from_utf8(fixture(dir, "meta.txt")).unwrap();
    meta.lines()
        .find_map(|line| line.strip_prefix(&format!("{name}=")))
        .unwrap_or_else(|| panic!("fixture meta has no {name}"))
        .to_string()
}
