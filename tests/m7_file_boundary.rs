//! M7C 机器契约：锁定外部 UTF-8 bytes 进入 Buffer 时的 BOM、非法字节、换行和末尾 newline 元信息。
//!
//! 本文件只测试文件文本加载边界，不测试 reload、保存输出或文件系统 I/O。

use std::path::Path;

use zom_engine::*;

fn load(bytes: &[u8], config: BufferConfig) -> Buffer {
    Buffer::from_loaded_text(BufferKind::file("/tmp/m7.txt"), bytes, config).unwrap()
}

#[test]
fn loaded_text_strips_utf8_bom_by_default_and_records_metadata() {
    let buffer = load(b"\xEF\xBB\xBFhello\n", BufferConfig::default());
    let info = buffer.loaded_text_info().expect("loaded text info");

    assert_eq!(buffer.text().as_ref(), "hello\n");
    assert_eq!(buffer.path(), Some(Path::new("/tmp/m7.txt")));
    assert!(!buffer.is_dirty());
    assert_eq!(
        buffer.last_synced_external_version(),
        Some(buffer.version())
    );

    assert_eq!(info.encoding, TextEncoding::Utf8);
    assert_eq!(info.bom_policy, BomPolicy::Strip);
    assert!(info.had_bom);
    assert!(!info.had_invalid_utf8);
    assert_eq!(info.line_ending_style, LineEndingStyle::Lf);
    assert!(info.has_final_newline);
}

#[test]
fn loaded_text_can_preserve_utf8_bom_as_text() {
    let config = BufferConfig {
        encoding: EncodingConfig::new(BomPolicy::Preserve, InvalidUtf8Policy::Reject),
        ..BufferConfig::default()
    };

    let buffer = load(b"\xEF\xBB\xBFhello", config);
    let info = buffer.loaded_text_info().expect("loaded text info");

    assert_eq!(buffer.text().as_ref(), "\u{feff}hello");
    assert!(info.had_bom);
    assert_eq!(info.bom_policy, BomPolicy::Preserve);
    assert_eq!(info.line_ending_style, LineEndingStyle::None);
    assert!(!info.has_final_newline);
}

#[test]
fn invalid_utf8_is_rejected_by_default() {
    let err = Buffer::from_loaded_text(
        BufferKind::file("/tmp/bad.txt"),
        b"a\xFFb",
        BufferConfig::default(),
    )
    .unwrap_err();

    assert!(matches!(
        err,
        EngineError::Storage(StorageError::InvalidUtf8 {
            valid_up_to: 1,
            error_len: Some(1),
        })
    ));
}

#[test]
fn invalid_utf8_can_be_recovered_with_replacement_character() {
    let config = BufferConfig {
        encoding: EncodingConfig::new(BomPolicy::Strip, InvalidUtf8Policy::Replace),
        ..BufferConfig::default()
    };

    let buffer = load(b"a\xFFb", config);
    let info = buffer.loaded_text_info().expect("loaded text info");

    assert_eq!(buffer.text().as_ref(), "a\u{fffd}b");
    assert!(info.had_invalid_utf8);
    assert_eq!(info.invalid_utf8_policy, InvalidUtf8Policy::Replace);
}

#[test]
fn loaded_text_records_line_ending_style_and_final_newline_state() {
    let lf = load(b"a\nb\n", BufferConfig::default());
    let crlf = load(b"a\r\nb\r\n", BufferConfig::default());
    let mixed = load(b"a\nb\r\nc", BufferConfig::default());
    let none = load(b"abc", BufferConfig::default());

    assert_eq!(
        lf.loaded_text_info().unwrap().line_ending_style,
        LineEndingStyle::Lf
    );
    assert!(lf.loaded_text_info().unwrap().has_final_newline);

    assert_eq!(
        crlf.loaded_text_info().unwrap().line_ending_style,
        LineEndingStyle::Crlf
    );
    assert!(crlf.loaded_text_info().unwrap().has_final_newline);

    assert_eq!(
        mixed.loaded_text_info().unwrap().line_ending_style,
        LineEndingStyle::Mixed
    );
    assert!(!mixed.loaded_text_info().unwrap().has_final_newline);

    assert_eq!(
        none.loaded_text_info().unwrap().line_ending_style,
        LineEndingStyle::None
    );
    assert!(!none.loaded_text_info().unwrap().has_final_newline);
}
