use std::io::Cursor;

use zcv_text::{
    Buffer, BufferConfig, ByteOffset, Edit, TextError, TransactionError, TransactionMetadata,
};

use super::*;

fn decode(bytes: &[u8]) -> Result<String, BufferLoadError> {
    decode_to_string(Cursor::new(bytes.to_vec()))
}

#[test]
fn bom_is_stripped() {
    let bytes = [0xEF, 0xBB, 0xBF, b'h', b'e', b'l', b'l', b'o', b'\r', b'\n'];
    assert_eq!(decode(&bytes).unwrap(), "hello\n");
}

#[test]
fn decoding_normalizes_line_endings_across_read_boundaries() {
    let mut bytes = vec![b'a'; READ_BUFFER_SIZE - 1];
    bytes.extend_from_slice(b"\r\nb\rc");

    let decoded = decode(&bytes).expect("文本应可解码");
    assert_eq!(
        decoded,
        format!("{}\nb\nc", "a".repeat(READ_BUFFER_SIZE - 1))
    );
}

#[test]
fn invalid_utf8_is_rejected() {
    let bytes = [b'a', b'b', b'c', 0xFF, b'd', b'e', b'f'];
    let error = decode(&bytes).unwrap_err();
    assert!(matches!(
        error,
        BufferLoadError::InvalidUtf8 {
            valid_up_to: 3,
            error_len: Some(1)
        }
    ));
}

#[test]
fn binary_content_is_rejected_even_when_utf8_is_valid() {
    let error = decode(b"text\0more text").unwrap_err();
    assert!(matches!(error, BufferLoadError::Binary));
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
    let error = write_buffer_to(&buffer.snapshot(), stale, &mut output).unwrap_err();
    assert!(matches!(
        error,
        BufferSaveError::Text(TextError::Transaction(
            TransactionError::VersionMismatch { .. }
        ))
    ));
    assert!(output.is_empty());
}

#[test]
fn write_uses_the_buffer_snapshot_without_line_ending_conversion() {
    let buffer = Buffer::from_text("a\nb\rc\r\nd".to_string(), BufferConfig::default()).unwrap();
    let snapshot = buffer.snapshot();
    let expected = snapshot
        .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
        .unwrap()
        .as_str()
        .to_owned();
    assert_eq!(expected, "a\nb\nc\nd");

    let mut output = Vec::new();
    write_buffer_to(&snapshot, buffer.version(), &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), expected);
}
