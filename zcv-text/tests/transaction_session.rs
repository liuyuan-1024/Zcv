//! 事务会话：会话内多次编辑合并为单个历史节点（对齐 Zed 的 start/end_transaction 模型）。

use zcv_text::*;

pub mod common;
use common::*;

#[test]
fn session_groups_multiple_edits_into_one_undo_step() {
    let mut buffer = buffer("hello");
    buffer.start_transaction().unwrap().expect("应开启会话");
    buffer
        .edit(
            [Edit::insert(b(5), " world".to_string()).unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    // 第二次编辑的偏移基于第一次编辑后的文本（"hello world" = 11 字节）。
    let second = Edit::insert(b(11), "!".to_string()).unwrap();
    buffer
        .edit([second], TransactionMetadata::default())
        .unwrap();
    buffer.end_transaction().unwrap().expect("会话应提交");

    assert_eq!(buffer_text(&buffer), "hello world!");
    assert_eq!(
        buffer.history_status().undo_depth,
        1,
        "会话内两次编辑应合并为一个撤销步"
    );
    buffer.undo().unwrap().expect("应可撤销整个会话");
    assert_eq!(buffer_text(&buffer), "hello");
}

#[test]
fn session_edits_share_the_session_transaction_id() {
    let mut buffer = buffer("hello");
    let session_id = buffer.start_transaction().unwrap().expect("应开启会话");
    let first = buffer
        .edit(
            [Edit::insert(b(0), "A".to_string()).unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    let second = buffer
        .edit(
            [Edit::insert(b(1), "B".to_string()).unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    assert_eq!(first.history_transaction_id(), Some(session_id));
    assert_eq!(second.history_transaction_id(), Some(session_id));
    let ended = buffer.end_transaction().unwrap().expect("会话应提交");
    assert_eq!(ended, session_id, "会话节点沿用开启时分配的事务身份");

    let undone = buffer.undo().unwrap().expect("应可撤销");
    assert_eq!(
        undone.transaction_id(),
        session_id,
        "撤销返回被回放会话的事务身份"
    );
}

#[test]
fn empty_session_produces_no_history() {
    let mut buffer = buffer("hello");
    buffer.start_transaction().unwrap().expect("应开启会话");
    assert_eq!(
        buffer.end_transaction().unwrap(),
        None,
        "空会话不产生历史节点"
    );
    assert!(!buffer.can_undo());
}

#[test]
fn nested_start_transaction_is_idempotent() {
    let mut buffer = buffer("hello");
    buffer.start_transaction().unwrap().expect("首次开启应成功");
    assert_eq!(
        buffer.start_transaction().unwrap(),
        None,
        "会话已开启时重复 start 应幂等返回 None"
    );
    buffer.end_transaction().unwrap();
}

#[test]
fn session_with_merge_policy_merges_into_previous_node() {
    let mut buffer = buffer("hello");
    buffer
        .edit(
            [Edit::insert(b(5), "!".to_string()).unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    assert_eq!(buffer.history_status().undo_depth, 1);

    // 会话内的编辑带 MergeWithPrevious：整个会话合并到前一个节点，undo 深度不增加。
    buffer.start_transaction().unwrap();
    buffer
        .edit(
            [Edit::insert(b(6), "a".to_string()).unwrap()],
            TransactionMetadata::new(TransactionSource::Programmatic)
                .with_merge_policy(TransactionMergePolicy::MergeWithPrevious),
        )
        .unwrap();
    buffer.end_transaction().unwrap().expect("会话应提交");
    assert_eq!(
        buffer.history_status().undo_depth,
        1,
        "MergeWithPrevious 会话应合并入前节点"
    );

    // 一次撤销回退两个编辑（合并节点 + 会话文本）。
    buffer.undo().unwrap().expect("应可撤销");
    assert_eq!(buffer_text(&buffer), "hello");
}

#[test]
fn skip_history_edit_discards_whole_session_history() {
    let mut buffer = buffer("hello");
    buffer.start_transaction().unwrap();
    buffer
        .edit(
            [Edit::insert(b(5), " world".to_string()).unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    buffer
        .edit(
            [Edit::insert(b(11), "!".to_string()).unwrap()],
            TransactionMetadata::new(TransactionSource::Programmatic).without_history(),
        )
        .unwrap();
    assert_eq!(
        buffer.end_transaction().unwrap(),
        None,
        "会话内出现放弃历史的编辑时整个会话不入历史"
    );
    assert_eq!(buffer_text(&buffer), "hello world!");
    assert!(!buffer.can_undo(), "会话被放弃历史后不可撤销");
}

#[test]
fn session_survives_across_edit_failures() {
    let mut buffer = buffer("hello");
    buffer.start_transaction().unwrap();
    // 越界编辑失败：会话保持开启，后续编辑仍归入同一会话。
    let err = buffer
        .edit(
            [Edit::replace(range(1, 99), "x")],
            TransactionMetadata::default(),
        )
        .unwrap_err();
    assert!(matches!(
        err,
        TextError::Edit(EditError::RangeOutOfBounds { .. })
    ));
    buffer
        .edit(
            [Edit::insert(b(5), "!".to_string()).unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    buffer.end_transaction().unwrap().expect("会话应提交");
    assert_eq!(buffer_text(&buffer), "hello!");
    buffer.undo().unwrap().expect("应可撤销");
    assert_eq!(buffer_text(&buffer), "hello");
}

#[test]
fn undo_redo_across_a_session_boundary() {
    let mut buffer = buffer("hello");
    buffer.start_transaction().unwrap();
    buffer
        .edit(
            [Edit::insert(b(5), "!".to_string()).unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    buffer.end_transaction().unwrap().expect("会话应提交");

    buffer.undo().unwrap().expect("应可撤销会话");
    assert_eq!(buffer_text(&buffer), "hello");
    buffer.redo().unwrap().expect("应可重做会话");
    assert_eq!(buffer_text(&buffer), "hello!");
}
