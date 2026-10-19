use zcv_text::{Buffer, BufferConfig, BufferVersion, ByteOffset, Edit, TransactionMetadata};

fn text(buffer: &Buffer) -> String {
    let snapshot = buffer.snapshot();
    snapshot
        .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
        .unwrap()
        .as_str()
        .to_owned()
}

#[test]
fn construction_uses_lf_without_creating_an_edit() {
    let buffer = Buffer::from_text("甲\r\n乙\r丙\n".to_owned(), BufferConfig::default()).unwrap();

    assert_eq!(text(&buffer), "甲\n乙\n丙\n");
    assert_eq!(buffer.version(), BufferVersion::INITIAL);
    assert!(!buffer.can_undo());
}

#[test]
fn transaction_normalizes_replacements_before_recording_history() {
    let mut buffer = Buffer::from_text("甲".to_owned(), BufferConfig::default()).unwrap();
    buffer
        .edit(
            [Edit::insert(buffer.len_bytes(), "\r\n乙\r丙").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();

    assert_eq!(text(&buffer), "甲\n乙\n丙");
    buffer.undo().unwrap();
    assert_eq!(text(&buffer), "甲");
    buffer.redo().unwrap();
    assert_eq!(text(&buffer), "甲\n乙\n丙");
}

#[test]
fn external_replacement_uses_canonical_text_for_diff_and_version() {
    let mut buffer = Buffer::from_text("甲\n乙".to_owned(), BufferConfig::default()).unwrap();
    buffer.replace_text("甲\r\n乙".to_owned()).unwrap();
    assert_eq!(buffer.version(), BufferVersion::INITIAL);

    buffer.replace_text("甲\r\n丙\r".to_owned()).unwrap();
    assert_eq!(text(&buffer), "甲\n丙\n");
    assert_eq!(buffer.version(), BufferVersion::new(1));
}

#[test]
fn derived_snapshot_and_installed_buffer_share_normalized_text() {
    let mut buffer = Buffer::from_text("甲".to_owned(), BufferConfig::default()).unwrap();
    let edited = buffer
        .snapshot_with_edits([Edit::insert(buffer.len_bytes(), "\r\n乙").unwrap()])
        .unwrap();
    let snapshot = edited.snapshot();
    assert_eq!(
        snapshot
            .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
            .unwrap()
            .as_str(),
        "甲\n乙"
    );

    buffer.fast_forward(edited).unwrap();
    assert_eq!(text(&buffer), "甲\n乙");
}
