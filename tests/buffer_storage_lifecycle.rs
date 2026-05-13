use zom_engine::*;

fn b(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

fn c(value: usize) -> CharOffset {
    CharOffset::new(value)
}

fn line(value: usize) -> Line {
    Line::new(value)
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(b(start), b(end)).unwrap()
}

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

#[test]
fn create_edit_delete_replace_should_update_text_version_dirty_and_line_index() {
    let mut buffer = buffer("helo\n世界");

    buffer.insert(b(2), "l").unwrap();
    buffer.replace(range(6, 12), "Rust").unwrap();
    buffer.delete(range(5, 6)).unwrap();

    assert_eq!(buffer.text().as_ref(), "helloRust");
    assert_eq!(buffer.version(), BufferVersion::new(3));
    assert!(buffer.is_dirty());
    assert_eq!(buffer.line_count(), 1);
    assert_eq!(buffer.len_bytes(), b(9));
    assert_eq!(buffer.len_chars(), c(9));
}

#[test]
fn apply_edit_at_invalid_utf8_boundary_should_fail_atomically() {
    let mut buffer = buffer("你a");
    let before_text = buffer.text().to_string();
    let before_version = buffer.version();

    let err = buffer.insert(b(1), "x").unwrap_err();

    assert!(
        matches!(
            err,
            EngineError::Coordinate(CoordinateError::InvalidByteBoundary(offset))
                | EngineError::Edit(EditError::InvalidBoundary { offset })
                if offset == b(1)
        ) || matches!(
            err,
            EngineError::Edit(EditError::RangeOutOfBounds { range }) if range == TextRange::new(b(1), b(1)).unwrap()
        )
    );
    assert_eq!(buffer.text().as_ref(), before_text);
    assert_eq!(buffer.version(), before_version);
    assert!(!buffer.is_dirty());
}

#[test]
fn read_only_state_should_reject_all_text_mutations_without_state_transition() {
    let mut buffer = buffer("abc").into_read_only();
    let version = buffer.version();

    let insert = buffer.insert(b(3), "x").unwrap_err();
    let delete = buffer.delete(range(0, 1)).unwrap_err();
    let replace = buffer.replace(range(0, 1), "A").unwrap_err();

    for err in [insert, delete, replace] {
        assert!(matches!(err, EngineError::Storage(StorageError::ReadOnly)));
    }
    assert_eq!(buffer.text().as_ref(), "abc");
    assert_eq!(buffer.version(), version);
    assert_eq!(buffer.state(), BufferState::ReadOnly);
}

#[test]
fn saved_and_external_sync_versions_should_track_independent_boundaries() {
    let mut buffer =
        Buffer::with_external("opaque://doc", "abc".to_string(), BufferConfig::default()).unwrap();

    assert_eq!(buffer.origin().handle(), Some("opaque://doc"));
    assert!(!buffer.is_synced_with_external());
    assert!(!buffer.is_dirty());

    buffer.insert(b(3), "!").unwrap();
    assert!(buffer.is_dirty());
    assert!(!buffer.is_synced_with_external());

    buffer.mark_saved();
    assert!(!buffer.is_dirty());
    assert!(!buffer.is_synced_with_external());

    buffer.mark_synced_external();
    assert!(buffer.is_synced_with_external());
    assert_eq!(buffer.saved_version(), buffer.version());
}

#[test]
fn snapshot_should_remain_version_bound_and_immutable_after_buffer_transition() {
    let mut buffer = buffer("one\ntwo");
    let snapshot = buffer.snapshot();

    buffer.replace(range(4, 7), "TWO").unwrap();

    assert_eq!(snapshot.text().as_ref(), "one\ntwo");
    assert_eq!(snapshot.version(), BufferVersion::INITIAL);
    assert!(snapshot.is_stale_for_version(buffer.version()));
    assert!(buffer.is_snapshot_stale(&snapshot));
    assert_eq!(buffer.text().as_ref(), "one\nTWO");
}

#[test]
fn loaded_text_boundary_should_record_bom_encoding_line_endings_and_invalid_utf8_policy() {
    let buffer = Buffer::from_loaded_text(
        BufferOrigin::external("loaded"),
        b"\xEF\xBB\xBFhello\r\n",
        BufferConfig::default(),
    )
    .unwrap();
    let info = buffer.loaded_text_info().unwrap();

    assert_eq!(buffer.text().as_ref(), "hello\r\n");
    assert_eq!(info.encoding, TextEncoding::Utf8);
    assert_eq!(info.bom_policy, BomPolicy::Strip);
    assert!(info.had_bom);
    assert!(!info.had_invalid_utf8);
    assert_eq!(info.line_ending_style, LineEndingStyle::Crlf);
    assert!(info.has_final_newline);

    let err = Buffer::from_loaded_text(
        BufferOrigin::external("bad"),
        b"a\xff",
        BufferConfig::default(),
    )
    .unwrap_err();
    assert!(matches!(
        err,
        EngineError::Storage(StorageError::InvalidUtf8 {
            valid_up_to: 1,
            error_len: Some(1)
        })
    ));
}

#[test]
fn reload_should_replace_storage_clear_history_reset_selection_and_mark_clean() {
    let mut buffer = buffer("old");
    buffer.insert(b(3), "!").unwrap();
    buffer.set_selection(SelectionSet::caret(b(4))).unwrap();
    assert!(buffer.can_undo());

    buffer.reload_from_text("new\n".to_string()).unwrap();

    assert_eq!(buffer.text().as_ref(), "new\n");
    assert_eq!(buffer.line_start(line(1)).unwrap(), c(4));
    assert!(!buffer.can_undo());
    assert!(!buffer.can_redo());
    assert!(!buffer.is_dirty());
    assert_eq!(buffer.selection(), &SelectionSet::default());
    assert!(buffer.loaded_text_info().is_none());
}

#[test]
fn to_save_text_should_reject_stale_version_and_normalize_configured_line_endings() {
    let mut buffer = buffer("a\nb");
    let stale = buffer.version();
    buffer.insert(b(3), "\r\nc").unwrap();

    let err = buffer.to_save_text(stale).unwrap_err();
    assert!(matches!(
        err,
        EngineError::Transaction(TransactionError::VersionMismatch { expected, actual })
            if expected == buffer.version() && actual == stale
    ));

    let crlf = Buffer::from_text(
        "a\nb\rc".to_string(),
        BufferConfig {
            line_ending: LineEndingConfig::Crlf,
            ..BufferConfig::default()
        },
    )
    .unwrap();
    assert_eq!(crlf.to_save_text(crlf.version()).unwrap(), "a\r\nb\r\nc");
}

#[test]
fn large_file_policy_should_report_storage_size_long_line_and_auto_read_only() {
    let policy = LargeFilePolicy {
        large_file_threshold_bytes: 3,
        long_line_threshold_chars: 3,
        auto_read_only_on_large_file: true,
        ..LargeFilePolicy::default()
    };
    let config = BufferConfig {
        large_file: policy,
        ..BufferConfig::default()
    };
    let buffer =
        Buffer::from_loaded_text(BufferOrigin::external("large"), b"abcd", config).unwrap();

    assert!(buffer.is_large_file());
    assert!(buffer.has_long_line());
    assert_eq!(buffer.longest_line_chars(), 4);
    assert!(buffer.is_read_only());
}
