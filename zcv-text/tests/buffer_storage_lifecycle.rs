use zcv_text::*;

#[path = "common/buffer.rs"]
mod buffer;
#[path = "common/byte_range.rs"]
mod byte_range;
#[path = "common/char_offset.rs"]
mod char_offset;
#[path = "common/full_text.rs"]
mod full_text;
#[path = "common/line.rs"]
mod line;

use buffer::buffer;
use byte_range::{b, range};
use char_offset::c;
use full_text::buffer_text;
use line::line;

#[test]
fn create_edit_delete_replace_should_update_text_version_dirty_and_line_index() {
    let mut buffer = buffer("helo\n世界");

    buffer
        .edit(
            [Edit::insert(b(2), "l").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    buffer
        .edit(
            [Edit::replace(range(6, 12), "Rust")],
            TransactionMetadata::default(),
        )
        .unwrap();
    buffer
        .edit([Edit::delete(range(5, 6))], TransactionMetadata::default())
        .unwrap();

    assert_eq!(buffer_text(&buffer), "helloRust");
    assert_eq!(buffer.version(), BufferVersion::new(3));
    assert!(
        buffer
            .snapshot()
            .has_edits_since(BufferVersion::INITIAL)
            .unwrap()
    );
    assert_eq!(buffer.line_count(), 1);
    assert_eq!(buffer.len_bytes(), b(9));
    assert_eq!(buffer.len_chars(), c(9));
}

#[test]
fn apply_edit_at_invalid_utf8_boundary_should_fail_atomically() {
    let mut buffer = buffer("你a");
    let before_text = buffer_text(&buffer);
    let before_version = buffer.version();

    let err = buffer
        .edit(
            [Edit::insert(b(1), "x").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap_err();

    assert!(
        matches!(
            err,
            TextError::Coordinate(CoordinateError::InvalidByteBoundary(offset))
                | TextError::Edit(EditError::InvalidBoundary { offset })
                if offset == b(1)
        ) || matches!(
            err,
            TextError::Edit(EditError::RangeOutOfBounds { range }) if range == TextRange::new(b(1), b(1)).unwrap()
        )
    );
    assert_eq!(buffer_text(&buffer), before_text);
    assert_eq!(buffer.version(), before_version);
    assert!(!buffer.snapshot().has_edits_since(before_version).unwrap());
}

#[test]
fn read_only_state_should_reject_all_text_mutations_without_state_transition() {
    let mut buffer = Buffer::from_text(
        "abc".to_string(),
        BufferConfig {
            large_file: LargeFilePolicy {
                large_file_threshold_bytes: 2,
                auto_read_only_on_large_file: true,
                ..LargeFilePolicy::default()
            },
        },
    )
    .unwrap();
    let version = buffer.version();

    let insert = buffer
        .edit(
            [Edit::insert(b(3), "x").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap_err();
    let delete = buffer
        .edit([Edit::delete(range(0, 1))], TransactionMetadata::default())
        .unwrap_err();
    let replace = buffer
        .edit(
            [Edit::replace(range(0, 1), "A")],
            TransactionMetadata::default(),
        )
        .unwrap_err();

    for err in [insert, delete, replace] {
        assert!(matches!(err, TextError::Storage(StorageError::ReadOnly)));
    }
    assert_eq!(buffer_text(&buffer), "abc");
    assert_eq!(buffer.version(), version);
    assert!(buffer.is_read_only());
}

#[test]
fn edits_since_a_saved_version_track_the_clean_baseline() {
    let mut buffer = buffer("abc");
    let saved_version = buffer.version();

    assert!(!buffer.snapshot().has_edits_since(saved_version).unwrap());

    buffer
        .edit(
            [Edit::insert(b(3), "!").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    assert!(buffer.snapshot().has_edits_since(saved_version).unwrap());

    // 保存点推进后再比较：当前版本即干净基线。
    let saved_version = buffer.version();
    assert!(!buffer.snapshot().has_edits_since(saved_version).unwrap());
}

#[test]
fn snapshot_should_remain_version_bound_and_immutable_after_buffer_transition() {
    let mut buffer = buffer("one\ntwo");
    let snapshot = buffer.snapshot();

    buffer
        .edit(
            [Edit::replace(range(4, 7), "TWO")],
            TransactionMetadata::default(),
        )
        .unwrap();

    assert_eq!(buffer_text(&snapshot), "one\ntwo");
    assert_eq!(snapshot.version(), BufferVersion::INITIAL);
    assert_eq!(buffer_text(&buffer), "one\nTWO");
}

#[test]
fn replace_text_updates_through_the_normal_history_and_anchor_pipeline() {
    let mut buffer = buffer("old");
    buffer
        .edit(
            [Edit::insert(b(3), "!").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    assert!(buffer.can_undo());

    buffer.replace_text("new\n".to_string()).unwrap();

    assert_eq!(buffer_text(&buffer), "new\n");
    assert_eq!(buffer.line_start_char(line(1)).unwrap(), c(4));
    assert!(buffer.can_undo());
    assert!(!buffer.can_redo());
    assert!(!buffer.snapshot().has_edits_since(buffer.version()).unwrap());
}

#[test]
fn replace_same_text_keeps_version_and_history() {
    let mut buffer = buffer("old");
    buffer
        .edit(
            [Edit::insert(b(3), "!").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    let version = buffer.version();
    assert!(
        buffer
            .snapshot()
            .has_edits_since(BufferVersion::INITIAL)
            .unwrap()
    );
    assert!(buffer.can_undo());

    // 文本相同：不产生新版本，也不改变历史。
    buffer.replace_text("old!".to_string()).unwrap();

    assert_eq!(buffer.version(), version);
    assert!(buffer.can_undo());
    buffer.undo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "old");
}

#[test]
fn large_file_policy_should_auto_mark_large_buffer_read_only() {
    let policy = LargeFilePolicy {
        large_file_threshold_bytes: 3,
        auto_read_only_on_large_file: true,
        ..LargeFilePolicy::default()
    };
    let config = BufferConfig { large_file: policy };
    let buffer = Buffer::from_text("abcd".to_string(), config).unwrap();

    assert!(buffer.is_large_file());
    assert!(buffer.is_read_only());
}
