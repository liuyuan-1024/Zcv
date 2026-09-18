use std::io::Cursor;

use zcv_text::{
    Buffer, BufferConfig, ByteOffset, Edit, TextError, TransactionError, TransactionMetadata,
};

use super::*;

fn decode(bytes: &[u8], config: &EncodingConfig) -> Result<String, BufferLoadError> {
    decode_to_string(Cursor::new(bytes.to_vec()), config)
}

#[test]
fn bom_is_stripped_by_default() {
    let bytes = [0xEF, 0xBB, 0xBF, b'h', b'e', b'l', b'l', b'o', b'\r', b'\n'];
    assert_eq!(
        decode(&bytes, &EncodingConfig::default()).unwrap(),
        "hello\r\n"
    );
}

#[test]
fn bom_preserved_when_policy_says_so() {
    let config = EncodingConfig::new(BomPolicy::Preserve, InvalidUtf8Policy::Reject);
    let bytes = [0xEF, 0xBB, 0xBF, b'h', b'i'];
    assert_eq!(decode(&bytes, &config).unwrap(), "\u{FEFF}hi");
}

#[test]
fn invalid_utf8_rejected_by_default() {
    let bytes = [b'a', b'b', b'c', 0xFF, b'd', b'e', b'f'];
    let error = decode(&bytes, &EncodingConfig::default()).unwrap_err();
    assert!(matches!(
        error,
        BufferLoadError::InvalidUtf8 {
            valid_up_to: 3,
            error_len: Some(1)
        }
    ));
}

#[test]
fn invalid_utf8_replaced_when_policy_says_so() {
    let config = EncodingConfig::new(BomPolicy::Strip, InvalidUtf8Policy::Replace);
    let bytes = [b'a', 0xFF, b'b'];
    assert_eq!(decode(&bytes, &config).unwrap(), "a\u{FFFD}b");
}

#[test]
fn write_rejects_stale_version() {
    let mut buffer = Buffer::from_text("a\nb".to_string(), BufferConfig::default()).unwrap();
    let stale = buffer.version();
    buffer
        .edit(
            [Edit::insert(ByteOffset::new(3), "\r\nc").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();

    let mut output = Vec::new();
    let error =
        write_buffer_to(&buffer, stale, &mut output, LineEndingConfig::Preserve).unwrap_err();
    assert!(matches!(
        error,
        BufferSaveError::Text(TextError::Transaction(
            TransactionError::VersionMismatch { .. }
        ))
    ));
    assert!(output.is_empty());
}

#[test]
fn write_preserves_or_normalizes_line_endings() {
    let buffer = Buffer::from_text("a\nb\rc".to_string(), BufferConfig::default()).unwrap();

    let mut preserved = Vec::new();
    write_buffer_to(
        &buffer,
        buffer.version(),
        &mut preserved,
        LineEndingConfig::Preserve,
    )
    .unwrap();
    assert_eq!(String::from_utf8(preserved).unwrap(), "a\nb\rc");

    let mut crlf = Vec::new();
    write_buffer_to(&buffer, buffer.version(), &mut crlf, LineEndingConfig::Crlf).unwrap();
    assert_eq!(String::from_utf8(crlf).unwrap(), "a\r\nb\r\nc");
}
