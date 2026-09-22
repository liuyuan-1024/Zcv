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
    assert_eq!(decode(&bytes).unwrap(), "hello\r\n");
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
fn write_normalizes_line_endings_to_lf() {
    let buffer = Buffer::from_text("a\nb\rc\r\nd".to_string(), BufferConfig::default()).unwrap();

    let mut output = Vec::new();
    write_buffer_to(&buffer.snapshot(), buffer.version(), &mut output).unwrap();
    assert_eq!(String::from_utf8(output).unwrap(), "a\nb\nc\nd");
}
