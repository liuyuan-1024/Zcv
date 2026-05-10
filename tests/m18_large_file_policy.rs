//! M18A 机器契约：锁定 LargeFilePolicy 大文件 / 超长行阈值的事实暴露、
//! `LoadedTextInfo` 加载快照字段、以及 `auto_read_only_on_large_file`
//! 在 `from_loaded_text` / `reload_from_text` / `from_kind_text` 路径上的应用。

use zom_engine::{
    Buffer, BufferConfig, BufferKind, ByteOffset, LargeFilePolicy, LargeTransactionPolicy,
    LineEndingStyle,
};

fn policy_with_thresholds(
    large_file_threshold_bytes: usize,
    long_line_threshold_chars: usize,
    auto_read_only: bool,
) -> LargeFilePolicy {
    LargeFilePolicy {
        max_undo_history: 1000,
        max_undo_history_bytes: 64 * 1024 * 1024,
        large_transaction_threshold_bytes: 0,
        large_transaction_policy: LargeTransactionPolicy::SkipHistory,
        large_file_threshold_bytes,
        long_line_threshold_chars,
        auto_read_only_on_large_file: auto_read_only,
    }
}

fn config_with(policy: LargeFilePolicy) -> BufferConfig {
    let mut config = BufferConfig::default();
    config.large_file = policy;
    config
}

#[test]
fn default_policy_exposes_large_file_and_long_line_thresholds() {
    let policy = LargeFilePolicy::default();
    assert!(policy.large_file_threshold_bytes > 0);
    assert!(policy.long_line_threshold_chars > 0);
    assert!(!policy.auto_read_only_on_large_file);
}

#[test]
fn policy_helpers_recognize_large_byte_size_and_long_line() {
    let policy = policy_with_thresholds(100, 50, false);
    assert!(!policy.is_large_byte_size(0));
    assert!(!policy.is_large_byte_size(100));
    assert!(policy.is_large_byte_size(101));

    assert!(!policy.is_long_line(0));
    assert!(!policy.is_long_line(50));
    assert!(policy.is_long_line(51));

    let unlimited = policy_with_thresholds(0, 0, false);
    assert!(!unlimited.is_large_byte_size(usize::MAX));
    assert!(!unlimited.is_long_line(usize::MAX));
}

#[test]
fn buffer_is_large_file_reports_current_storage_size() {
    let config = config_with(policy_with_thresholds(8, 1000, false));
    let small = Buffer::from_text("abc".to_string(), config.clone()).unwrap();
    assert!(!small.is_large_file());

    let large_text = "x".repeat(16);
    let large = Buffer::from_text(large_text, config).unwrap();
    assert!(large.is_large_file());
    assert!(!large.is_read_only(), "auto_read_only=false 时不切只读");
}

#[test]
fn buffer_has_long_line_reports_current_storage() {
    let config = config_with(policy_with_thresholds(usize::MAX, 4, false));
    let short = Buffer::from_text("abc\ndef".to_string(), config.clone()).unwrap();
    assert!(!short.has_long_line());
    assert_eq!(short.longest_line_chars(), 3);

    let long = Buffer::from_text("ab\nxxxxx".to_string(), config).unwrap();
    assert!(long.has_long_line());
    assert_eq!(long.longest_line_chars(), 5);
}

#[test]
fn longest_line_chars_handles_crlf_and_trailing_content() {
    let config = BufferConfig::default();
    let buffer = Buffer::from_text("ab\r\ncde\r\nfghij".to_string(), config).unwrap();
    assert_eq!(buffer.longest_line_chars(), 5);

    let only_newlines = Buffer::from_text("\n\n\n".to_string(), BufferConfig::default()).unwrap();
    assert_eq!(only_newlines.longest_line_chars(), 0);

    let empty = Buffer::from_text(String::new(), BufferConfig::default()).unwrap();
    assert_eq!(empty.longest_line_chars(), 0);
}

#[test]
fn longest_line_chars_counts_unicode_scalars_not_bytes() {
    let buffer = Buffer::from_text("中文行\nshort".to_string(), BufferConfig::default()).unwrap();
    assert_eq!(
        buffer.longest_line_chars(),
        5,
        "「中文行」3 chars vs 「short」5 chars"
    );
}

#[test]
fn from_loaded_text_records_byte_size_and_longest_line_chars() {
    let bytes = b"hello\nworld!!".to_vec();
    let config = config_with(policy_with_thresholds(100, 100, false));
    let buffer = Buffer::from_loaded_text(BufferKind::Untitled, &bytes, config).unwrap();

    let info = buffer
        .loaded_text_info()
        .expect("from_loaded_text 必须填充 info");
    assert_eq!(info.loaded_byte_size, ByteOffset::new(bytes.len()));
    assert!(!info.is_large);
    assert_eq!(info.longest_line_chars, 7);
    assert!(!info.has_long_line);
    assert_eq!(info.line_ending_style, LineEndingStyle::Lf);
}

#[test]
fn from_loaded_text_marks_is_large_when_above_threshold() {
    let bytes = vec![b'x'; 200];
    let config = config_with(policy_with_thresholds(100, 1000, false));
    let buffer = Buffer::from_loaded_text(BufferKind::Untitled, &bytes, config).unwrap();

    let info = buffer.loaded_text_info().unwrap();
    assert!(info.is_large);
    assert_eq!(info.loaded_byte_size, ByteOffset::new(200));
}

#[test]
fn from_loaded_text_marks_has_long_line_when_above_threshold() {
    let bytes = "ok\n".to_string() + &"y".repeat(80);
    let config = config_with(policy_with_thresholds(usize::MAX, 50, false));
    let buffer = Buffer::from_loaded_text(BufferKind::Untitled, bytes.as_bytes(), config).unwrap();

    let info = buffer.loaded_text_info().unwrap();
    assert!(info.has_long_line);
    assert_eq!(info.longest_line_chars, 80);
    assert!(!info.is_large);
}

#[test]
fn auto_read_only_on_large_file_triggers_for_loaded_text() {
    let bytes = vec![b'x'; 200];
    let config = config_with(policy_with_thresholds(100, 1000, true));
    let buffer = Buffer::from_loaded_text(BufferKind::Untitled, &bytes, config).unwrap();
    assert!(buffer.is_read_only(), "大文件 + auto_read_only 应自动只读");
    assert!(buffer.is_large_file());
}

#[test]
fn auto_read_only_does_not_engage_when_below_threshold() {
    let bytes = vec![b'x'; 50];
    let config = config_with(policy_with_thresholds(100, 1000, true));
    let buffer = Buffer::from_loaded_text(BufferKind::Untitled, &bytes, config).unwrap();
    assert!(!buffer.is_read_only());
}

#[test]
fn auto_read_only_applies_to_from_kind_text_constructor() {
    let config = config_with(policy_with_thresholds(8, 1000, true));
    let buffer = Buffer::from_text("x".repeat(20), config).unwrap();
    assert!(
        buffer.is_read_only(),
        "from_text 走 from_kind_text 也应触发 auto_read_only"
    );
    assert!(buffer.is_large_file());
}

#[test]
fn auto_read_only_engages_after_reload_to_large_text() {
    // 先加载小文本 → 不只读；reload 大文本 → 自动只读。
    let config = config_with(policy_with_thresholds(8, 1000, true));
    let mut buffer = Buffer::from_text("ok".to_string(), config).unwrap();
    assert!(!buffer.is_read_only());

    buffer.reload_from_text("x".repeat(20)).unwrap();
    assert!(buffer.is_read_only());
    assert!(buffer.is_large_file());
}

#[test]
fn reload_keeps_existing_read_only_when_text_is_small() {
    // 既有 read_only 不会因为 reload 小文本而被取消；引擎只单向加固。
    let config = config_with(policy_with_thresholds(8, 1000, true));
    let mut buffer = Buffer::from_text("x".repeat(20), config).unwrap();
    assert!(buffer.is_read_only());

    buffer.reload_from_text("ok".to_string()).unwrap();
    assert!(buffer.is_read_only());
    assert!(!buffer.is_large_file());
}

#[test]
fn reload_clears_loaded_text_info_even_when_auto_read_only_engages() {
    // reload_from_text 明确清空 LoadedTextInfo（不知道编码 / BOM 等加载事实），
    // 但仍按当前 storage 的字节数应用 auto_read_only 策略。
    let config = config_with(policy_with_thresholds(8, 1000, true));
    let mut buffer = Buffer::from_text("ok".to_string(), config).unwrap();

    buffer.reload_from_text("x".repeat(20)).unwrap();
    assert!(buffer.loaded_text_info().is_none());
    assert!(buffer.is_read_only());
}

#[test]
fn long_line_detection_recognizes_unicode_and_crlf() {
    let mixed = "ascii\r\n".to_string() + &"中".repeat(60);
    let config = config_with(policy_with_thresholds(usize::MAX, 50, false));
    let buffer = Buffer::from_loaded_text(BufferKind::Untitled, mixed.as_bytes(), config).unwrap();

    let info = buffer.loaded_text_info().unwrap();
    assert_eq!(info.longest_line_chars, 60);
    assert!(info.has_long_line);
    assert_eq!(info.line_ending_style, LineEndingStyle::Crlf);
}

#[test]
fn unlimited_thresholds_disable_large_file_and_long_line_facts() {
    let bytes = vec![b'x'; 10_000];
    let config = config_with(policy_with_thresholds(0, 0, true));
    let buffer = Buffer::from_loaded_text(BufferKind::Untitled, &bytes, config).unwrap();

    assert!(!buffer.is_large_file());
    assert!(!buffer.has_long_line());
    let info = buffer.loaded_text_info().unwrap();
    assert!(!info.is_large);
    assert!(!info.has_long_line);
    assert!(!buffer.is_read_only(), "阈值=0 时即便 auto=true 也不切只读");
}
