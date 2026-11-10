use xtask::{check_archive, check_embedded, ArchiveIdentity, CorpusError};

#[test]
fn archive_identity_binds_both_size_and_every_byte() {
    let identity = ArchiveIdentity {
        bytes: 3,
        sha256: "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad",
    };
    assert!(check_archive(b"abc", identity).is_ok());
    assert_eq!(
        check_archive(b"abd", identity).unwrap_err(),
        CorpusError::Digest
    );
    assert_eq!(
        check_archive(b"abcd", identity).unwrap_err(),
        CorpusError::Size
    );
}

#[test]
fn windows_comparison_requires_one_complete_identical_image() {
    let image = b"firmware image payload";
    assert_eq!(
        check_embedded(b"PEfirmware image payloadTAIL", image),
        Ok(2)
    );
    assert_eq!(
        check_embedded(b"PEfirmware image payloaDTAIL", image),
        Err(CorpusError::Embedded)
    );
    assert_eq!(check_embedded(image, image), Ok(0));
    assert_eq!(
        check_embedded(b"payloadpayload", b"payload"),
        Err(CorpusError::Embedded)
    );
    assert_eq!(check_embedded(b"PE", image), Err(CorpusError::Embedded));
    assert_eq!(check_embedded(b"PE", b""), Err(CorpusError::Embedded));
}
