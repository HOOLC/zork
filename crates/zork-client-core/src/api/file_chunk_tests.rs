use super::*;

fn chunk(bytes: &[u8], all: &[u8], offset: usize) -> ArtifactChunk {
    ArtifactChunk {
        reference: crate::files::FileRef {
            id: "file-test".into(),
            name: "data.bin".into(),
            byte_len: all.len(),
            content_root: content_root(all),
        },
        offset,
        next_offset: offset + bytes.len(),
        bytes: bytes.to_vec(),
    }
}

#[test]
fn immutable_chunk_stream_requires_progress_matching_reference_and_full_hash() {
    let mut bytes = vec![];
    let mut reference = None;
    assert!(!accept_artifact_chunk(
        "file-test",
        &mut reference,
        &mut bytes,
        chunk(b"ab", b"abcd", 0)
    )
    .unwrap());
    assert!(accept_artifact_chunk(
        "file-test",
        &mut reference,
        &mut bytes,
        chunk(b"cd", b"abcd", 2)
    )
    .unwrap());
    assert_eq!(bytes, b"abcd");
    for candidate in [
        chunk(b"", b"abcd", 0),
        chunk(b"ab", b"abcd", 1),
        chunk(b"bad!", b"abcd", 0),
        chunk(
            &vec![1; crate::files::CHUNK_BYTES + 1],
            &vec![1; crate::files::CHUNK_BYTES + 1],
            0,
        ),
    ] {
        assert!(accept_artifact_chunk("file-test", &mut None, &mut vec![], candidate).is_err());
    }
    let mut reference = None;
    let mut bytes = vec![];
    accept_artifact_chunk(
        "file-test",
        &mut reference,
        &mut bytes,
        chunk(b"ab", b"abcd", 0),
    )
    .unwrap();
    assert!(accept_artifact_chunk(
        "file-test",
        &mut reference,
        &mut bytes,
        chunk(b"xy", b"abxy", 2)
    )
    .is_err());
    assert!(accept_artifact_chunk(
        "file-other",
        &mut None,
        &mut vec![],
        chunk(b"abcd", b"abcd", 0)
    )
    .is_err());
    assert!(
        accept_artifact_chunk("file-test", &mut None, &mut vec![], chunk(b"", b"", 0)).unwrap()
    );
}
