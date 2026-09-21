//! 基线派生快照：在快照副本上应用编辑，并在版本校验后安装。

use zcv_text::*;

#[path = "common/buffer.rs"]
mod buffer;
#[path = "common/byte_range.rs"]
mod byte_range;
#[path = "common/full_text.rs"]
mod full_text;

use buffer::buffer;
use byte_range::{b, range};
use full_text::buffer_text;

#[test]
fn snapshot_with_edits_derives_without_mutating_the_buffer() {
    let buffer = buffer("abc");
    let v0 = buffer.version();
    let edited = buffer
        .snapshot_with_edits([Edit::replace(range(0, 1), "X".to_string())])
        .unwrap();

    assert!(edited.did_edit());
    assert_eq!(edited.base_version(), v0);
    assert_eq!(edited.snapshot().version(), v0.next().unwrap());
    assert_eq!(buffer_text(edited.snapshot()), "Xbc");

    // 派生快照自带连续版本链，可以独立查询增量。
    let batch = edited.snapshot().edits_since(v0).unwrap();
    assert_eq!(batch.new_version(), Some(v0.next().unwrap()));
    assert!(!batch.patch().is_empty());

    // 主文档在派生期间保持不变。
    assert_eq!(buffer_text(&buffer), "abc");
    assert_eq!(buffer.version(), v0);
    assert!(!buffer.can_undo());
}

#[test]
fn fast_forward_installs_the_derived_snapshot_through_the_transaction_path() {
    let mut buffer = buffer("abc");
    let v0 = buffer.version();
    let subscription = buffer.subscribe();

    let edited = buffer
        .snapshot_with_edits([
            Edit::replace(range(0, 1), "X".to_string()),
            Edit::insert(b(3), "!".to_string()).unwrap(),
        ])
        .unwrap();
    buffer.fast_forward(edited).unwrap();

    assert_eq!(buffer_text(&buffer), "Xbc!");
    assert_eq!(buffer.version(), v0.next().unwrap());
    assert!(buffer.can_undo(), "派生安装应进入历史");

    let batch = subscription.consume();
    assert_eq!(batch.old_version(), Some(v0));
    assert_eq!(batch.new_version(), Some(buffer.version()));
    assert!(!batch.patch().is_empty());
}

#[test]
fn fast_forward_rejects_a_stale_derivation_after_the_buffer_advances() {
    let mut buffer = buffer("abc");
    let v0 = buffer.version();
    let edited = buffer
        .snapshot_with_edits([Edit::replace(range(0, 1), "X".to_string())])
        .unwrap();

    buffer
        .edit(
            [Edit::insert(b(3), "z".to_string()).unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    let current = buffer.version();

    let error = buffer.fast_forward(edited).unwrap_err();
    assert!(matches!(
        error,
        TextError::Transaction(TransactionError::VersionMismatch { expected, actual })
            if expected == current && actual == v0
    ));
    assert_eq!(buffer_text(&buffer), "abcz", "被拒绝的安装不改变文本");
}

#[test]
fn snapshot_with_edits_without_edits_is_a_no_op_install() {
    let mut buffer = buffer("abc");
    let v0 = buffer.version();
    let edited = buffer.snapshot_with_edits(Vec::<Edit>::new()).unwrap();

    assert!(!edited.did_edit());
    assert_eq!(edited.snapshot().version(), v0);

    buffer.fast_forward(edited).unwrap();
    assert_eq!(buffer.version(), v0);
    assert_eq!(buffer_text(&buffer), "abc");
    assert!(!buffer.can_undo());
}

#[test]
fn snapshot_with_edits_rejects_invalid_edits_without_touching_the_baseline() {
    let buffer = buffer("abc");
    let v0 = buffer.version();

    assert!(matches!(
        buffer.snapshot_with_edits([
            Edit::replace(range(0, 2), "X".to_string()),
            Edit::replace(range(1, 2), "Y".to_string()),
        ]),
        Err(TextError::Edit(EditError::OverlappingEdits { .. }))
    ));
    assert!(matches!(
        buffer.snapshot_with_edits([Edit::replace(range(1, 9), "X".to_string())]),
        Err(TextError::Edit(EditError::RangeOutOfBounds { .. }))
    ));
    assert_eq!(buffer.version(), v0);
}
