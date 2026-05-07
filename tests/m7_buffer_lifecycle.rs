//! M7 机器契约：聚合 Buffer 生命周期、文件加载边界、reload 与保存边界。
//!
//! 小阶段测试保留在本文件的子模块中，避免一个大阶段拆出多个 cargo test 入口。

mod m7a_m7b_buffer_lifecycle {
    //! M7 机器契约：锁定 Buffer 身份、来源类型、状态推导、只读防线和保存点 / dirty 查询。
    //!
    //! 本文件只验证生命周期 public API，不测试文件加载、编码探测、reload 或保存输出。

    use std::path::Path;

    use zom_engine::*;

    fn buffer(text: &str) -> Buffer {
        Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
    }

    fn range(start: usize, end: usize) -> TextRange {
        TextRange::new(CharOffset::new(start), CharOffset::new(end)).unwrap()
    }

    #[test]
    fn buffers_have_stable_distinct_engine_ids() {
        let first = Buffer::new(BufferConfig::default()).unwrap();
        let second = Buffer::new(BufferConfig::default()).unwrap();

        assert_ne!(first.id(), second.id());
        assert!(first.id().get() > 0);
        assert!(second.id().get() > 0);
    }

    #[test]
    fn default_text_buffer_is_clean_untitled_and_temporary() {
        let buffer = buffer("hello");

        assert_eq!(buffer.kind(), &BufferKind::Untitled);
        assert_eq!(buffer.path(), None);
        assert_eq!(buffer.uri(), None);
        assert!(buffer.is_temporary());
        assert_eq!(buffer.state(), BufferState::Clean);
        assert!(!buffer.has_unsaved_changes());
        assert!(buffer.can_close_without_prompt());
    }

    #[test]
    fn file_uri_and_scratch_buffers_expose_their_identity_boundary() {
        let file =
            Buffer::from_file_text("/tmp/zom.txt", "file".to_string(), BufferConfig::default())
                .unwrap();
        let uri = Buffer::from_uri_text(
            "zom://virtual/doc",
            "uri".to_string(),
            BufferConfig::default(),
        )
        .unwrap();
        let scratch = Buffer::scratch("scratch".to_string(), BufferConfig::default()).unwrap();

        assert_eq!(file.path(), Some(Path::new("/tmp/zom.txt")));
        assert_eq!(file.uri(), None);
        assert!(!file.is_temporary());

        assert_eq!(uri.path(), None);
        assert_eq!(uri.uri(), Some("zom://virtual/doc"));
        assert!(!uri.is_temporary());

        assert_eq!(scratch.kind(), &BufferKind::Scratch);
        assert!(scratch.is_temporary());
    }

    #[test]
    fn state_and_close_prompt_follow_dirty_and_saved_versions() {
        let mut buffer = buffer("hello");

        assert_eq!(buffer.state(), BufferState::Clean);
        assert_eq!(buffer.saved_version(), BufferVersion::INITIAL);
        assert_eq!(buffer.last_saved_version(), BufferVersion::INITIAL);
        assert!(buffer.can_close_without_prompt());

        buffer.insert(CharOffset::new(5), "!").unwrap();

        assert_eq!(buffer.state(), BufferState::Dirty);
        assert!(buffer.has_unsaved_changes());
        assert!(!buffer.can_close_without_prompt());

        buffer.mark_saved();

        assert_eq!(buffer.state(), BufferState::Clean);
        assert_eq!(buffer.saved_version(), buffer.version());
        assert_eq!(buffer.last_saved_version(), buffer.version());
        assert!(!buffer.has_unsaved_changes());
        assert!(buffer.can_close_without_prompt());
    }

    #[test]
    fn undo_and_redo_recompute_dirty_state_from_saved_baseline() {
        let mut buffer = buffer("hello");

        buffer.insert(CharOffset::new(5), "!").unwrap();
        buffer.mark_saved();
        let saved_version = buffer.saved_version();

        buffer.insert(CharOffset::new(6), "?").unwrap();
        assert_eq!(buffer.text().as_ref(), "hello!?");
        assert!(buffer.is_dirty());
        assert_eq!(buffer.state(), BufferState::Dirty);

        buffer
            .undo()
            .unwrap()
            .expect("undo should restore saved text");
        assert_eq!(buffer.text().as_ref(), "hello!");
        assert!(buffer.version() > saved_version);
        assert!(!buffer.is_dirty());
        assert_eq!(buffer.state(), BufferState::Clean);

        buffer
            .redo()
            .unwrap()
            .expect("redo should reapply dirty edit");
        assert_eq!(buffer.text().as_ref(), "hello!?");
        assert!(buffer.is_dirty());
        assert_eq!(buffer.saved_version(), saved_version);
    }

    #[test]
    fn external_sync_version_is_tracked_separately_from_save_point() {
        let mut buffer = buffer("hello");

        assert_eq!(buffer.last_synced_external_version(), None);
        assert!(!buffer.is_synced_with_external());

        buffer.mark_synced_external();
        assert_eq!(
            buffer.last_synced_external_version(),
            Some(BufferVersion::INITIAL)
        );
        assert!(buffer.is_synced_with_external());

        buffer.insert(CharOffset::new(5), "!").unwrap();
        assert_eq!(
            buffer.last_synced_external_version(),
            Some(BufferVersion::INITIAL)
        );
        assert!(!buffer.is_synced_with_external());
        assert!(buffer.is_dirty());

        buffer.mark_synced_external();
        assert_eq!(
            buffer.last_synced_external_version(),
            Some(buffer.version())
        );
        assert!(buffer.is_synced_with_external());
        assert!(buffer.is_dirty());
    }

    #[test]
    fn read_only_buffer_reports_read_only_state_and_rejects_basic_edits() {
        let mut buffer = buffer("hello").into_read_only();
        let before_text = buffer.text().to_string();
        let before_version = buffer.version();

        assert!(buffer.is_read_only());
        assert_eq!(buffer.state(), BufferState::ReadOnly);

        let insert_err = buffer.insert(CharOffset::new(5), "!").unwrap_err();
        assert!(matches!(
            insert_err,
            EngineError::Storage(StorageError::ReadOnly)
        ));

        let delete_err = buffer.delete(range(0, 1)).unwrap_err();
        assert!(matches!(
            delete_err,
            EngineError::Storage(StorageError::ReadOnly)
        ));

        let replace_err = buffer.replace(range(0, 5), "world").unwrap_err();
        assert!(matches!(
            replace_err,
            EngineError::Storage(StorageError::ReadOnly)
        ));

        assert_eq!(buffer.text().as_ref(), before_text);
        assert_eq!(buffer.version(), before_version);
    }

    #[test]
    fn read_only_buffer_rejects_transactions_undo_redo_and_composition() {
        let mut buffer = buffer("hello");
        buffer.insert(CharOffset::new(5), "!").unwrap();
        buffer.set_read_only(true);

        let tx = Transaction::from_edits(
            buffer.version(),
            vec![Edit::replace(range(0, 1), "H".to_string())],
        )
        .unwrap();

        let tx_err = buffer.apply_transaction(tx).unwrap_err();
        assert!(matches!(
            tx_err,
            EngineError::Storage(StorageError::ReadOnly)
        ));

        let undo_err = buffer.undo().unwrap_err();
        assert!(matches!(
            undo_err,
            EngineError::Storage(StorageError::ReadOnly)
        ));

        let redo_err = buffer.redo().unwrap_err();
        assert!(matches!(
            redo_err,
            EngineError::Storage(StorageError::ReadOnly)
        ));

        let composition_err = buffer.start_composition().unwrap_err();
        assert!(matches!(
            composition_err,
            EngineError::Storage(StorageError::ReadOnly)
        ));

        assert_eq!(buffer.text().as_ref(), "hello!");
        assert_eq!(buffer.version(), BufferVersion::new(1));
    }
}

mod m7c_m7d_file_boundary {
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

    #[test]
    fn reload_from_text_rebuilds_text_state_and_resets_edit_history() {
        let mut buffer = load(b"old", BufferConfig::default());
        buffer.insert(CharOffset::new(3), "!").unwrap();
        buffer
            .set_selection(SelectionSet::caret(CharOffset::new(4)))
            .unwrap();

        assert!(buffer.is_dirty());
        assert!(buffer.can_undo());

        let before_reload_version = buffer.version();
        buffer.reload_from_text("new\n".to_string()).unwrap();

        assert_eq!(buffer.text().as_ref(), "new\n");
        assert!(buffer.version() > before_reload_version);
        assert!(!buffer.is_dirty());
        assert_eq!(buffer.saved_version(), buffer.version());
        assert_eq!(
            buffer.last_synced_external_version(),
            Some(buffer.version())
        );
        assert!(!buffer.can_undo());
        assert!(!buffer.can_redo());
        assert_eq!(buffer.selection(), &SelectionSet::default());
        assert!(buffer.loaded_text_info().is_none());
    }

    #[test]
    fn reload_from_snapshot_uses_snapshot_text_without_changing_buffer_identity() {
        let source =
            Buffer::from_text("snapshot text".to_string(), BufferConfig::default()).unwrap();
        let snapshot = source.snapshot();
        let mut target = load(b"target", BufferConfig::default());
        let target_id = target.id();
        let target_kind = target.kind().clone();

        target.reload_from_snapshot(&snapshot).unwrap();

        assert_eq!(target.text().as_ref(), "snapshot text");
        assert_eq!(target.id(), target_id);
        assert_eq!(target.kind(), &target_kind);
        assert!(!target.is_dirty());
    }

    #[test]
    fn to_save_text_checks_version_before_returning_text() {
        let mut buffer = load(b"hello", BufferConfig::default());
        let stale = buffer.version();
        buffer.insert(CharOffset::new(5), "!").unwrap();

        let err = buffer.to_save_text(stale).unwrap_err();
        assert!(matches!(
            err,
            EngineError::Transaction(TransactionError::VersionMismatch {
                expected,
                actual,
            }) if expected == buffer.version() && actual == stale
        ));
    }

    #[test]
    fn to_save_text_preserves_or_normalizes_line_endings_from_config() {
        let preserve = load(b"a\nb\r\nc\rd", BufferConfig::default());
        assert_eq!(
            preserve.to_save_text(preserve.version()).unwrap(),
            "a\nb\r\nc\rd"
        );

        let lf_config = BufferConfig {
            line_ending: LineEndingConfig::Lf,
            ..BufferConfig::default()
        };
        let lf = load(b"a\nb\r\nc\rd", lf_config);
        assert_eq!(lf.to_save_text(lf.version()).unwrap(), "a\nb\nc\nd");

        let crlf_config = BufferConfig {
            line_ending: LineEndingConfig::Crlf,
            ..BufferConfig::default()
        };
        let crlf = load(b"a\nb\r\nc\rd", crlf_config);
        assert_eq!(
            crlf.to_save_text(crlf.version()).unwrap(),
            "a\r\nb\r\nc\r\nd"
        );
    }
}
