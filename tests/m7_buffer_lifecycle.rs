//! M7A 机器契约：锁定 Buffer 身份、来源类型、状态推导、只读防线和关闭前 dirty 查询。
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
    let file = Buffer::from_file_text("/tmp/zom.txt", "file".to_string(), BufferConfig::default())
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
    assert!(buffer.can_close_without_prompt());

    buffer.insert(CharOffset::new(5), "!").unwrap();

    assert_eq!(buffer.state(), BufferState::Dirty);
    assert!(buffer.has_unsaved_changes());
    assert!(!buffer.can_close_without_prompt());

    buffer.mark_saved();

    assert_eq!(buffer.state(), BufferState::Clean);
    assert!(!buffer.has_unsaved_changes());
    assert!(buffer.can_close_without_prompt());
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
