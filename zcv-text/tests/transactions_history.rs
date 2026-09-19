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

fn metadata(description: &str) -> TransactionMetadata {
    TransactionMetadata::new(TransactionSource::Programmatic).with_description(description)
}

fn merge_metadata(description: &str) -> TransactionMetadata {
    metadata(description).with_merge_policy(TransactionMergePolicy::MergeWithPrevious)
}

#[test]
fn edit_should_emit_delta_changeset_position_map_and_subscription_patch() {
    let mut buffer = buffer("abc def");
    let subscription = buffer.subscribe();
    let base = buffer.version();
    let outcome = buffer
        .edit(
            [
                Edit::insert(b(3), "!".to_string()).unwrap(),
                Edit::replace(range(4, 7), "XYZ".to_string()),
            ],
            TransactionMetadata::default(),
        )
        .unwrap();
    let event = outcome.event();
    let delta = event.delta();
    let changeset = event.changeset();
    let changes = subscription.consume();

    assert_eq!(buffer_text(&buffer), "abc! XYZ");
    assert_eq!(delta.old_version(), base);
    assert_eq!(delta.new_version(), buffer.version());
    assert_eq!(delta.edits().len(), 2);
    assert_eq!(
        changeset.changed_ranges().unwrap(),
        vec![range(3, 4), range(5, 8)]
    );
    assert_eq!(event.position_map().map_old_position(b(7)).value(), b(8));
    assert_eq!(event.old_version(), base);
    assert_eq!(event.new_version(), buffer.version());
    assert_eq!(event.source(), TransactionSource::Programmatic);
    assert_eq!(event.position_map().map_old_position(b(7)).value(), b(8));
    assert_eq!(changes.old_version(), Some(base));
    assert_eq!(changes.new_version(), Some(buffer.version()));
    let event_changes = TextChangeBatch::from_event(event);
    assert_eq!(event_changes.old_version(), changes.old_version());
    assert_eq!(event_changes.new_version(), changes.new_version());
    assert_eq!(event_changes.transaction_id(), changes.transaction_id());
    assert_eq!(event_changes.patch(), changes.patch());
    assert_eq!(changes.patch().edits().len(), 2);
}

#[test]
fn anchor_follows_continuous_delta_events_without_reinterpreting_coordinates() {
    let mut buffer = buffer("abcd");
    let mut anchor = Anchor::new(BufferGeneration::INITIAL, buffer.version(), b(2))
        .with_affinity(Affinity::After);

    let first = buffer
        .edit(
            [Edit::insert(b(2), "XY".to_string()).unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    anchor
        .update_through_delta_event(first.event())
        .expect("连续事件的首个版本应匹配");
    assert_eq!(anchor.offset(), b(4));

    let second = buffer
        .edit(
            [Edit::insert(b(4), "!".to_string()).unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    anchor
        .update_through_delta_event(second.event())
        .expect("连续事件的后续版本应匹配");
    assert_eq!(anchor.version(), buffer.version());
    assert_eq!(anchor.offset(), b(5));
}

#[test]
fn anchors_map_through_a_multi_edit_transaction_with_their_affinity() {
    let mut buffer = buffer("abcdef");
    let version = buffer.version();
    let mut before_insert =
        Anchor::new(BufferGeneration::INITIAL, version, b(1)).with_affinity(Affinity::Before);
    let mut after_insert =
        Anchor::new(BufferGeneration::INITIAL, version, b(1)).with_affinity(Affinity::After);
    let mut before_replace =
        Anchor::new(BufferGeneration::INITIAL, version, b(3)).with_affinity(Affinity::Before);
    let mut after_replace =
        Anchor::new(BufferGeneration::INITIAL, version, b(5)).with_affinity(Affinity::After);

    let outcome = buffer
        .edit(
            [
                Edit::insert(b(1), "XY".to_string()).unwrap(),
                Edit::replace(range(3, 5), "Z".to_string()),
            ],
            TransactionMetadata::default(),
        )
        .unwrap();

    for anchor in [
        &mut before_insert,
        &mut after_insert,
        &mut before_replace,
        &mut after_replace,
    ] {
        anchor
            .update_through_delta_event(outcome.event())
            .expect("同一事务的所有锚点版本必须一致");
        assert_eq!(anchor.version(), buffer.version());
    }

    assert_eq!(buffer_text(&buffer), "aXYbcZf");
    assert_eq!(before_insert.offset(), b(1), "贴前插入的锚点不跟随插入");
    assert_eq!(after_insert.offset(), b(3), "贴后插入的锚点跟随插入");
    assert_eq!(
        before_replace.offset(),
        b(5),
        "替换起点前的锚点落在替换文本前"
    );
    assert_eq!(
        after_replace.offset(),
        b(6),
        "替换终点后的锚点落在替换文本后"
    );
}

#[test]
fn failed_multi_edit_boundary_should_keep_transaction_atomic() {
    let mut buffer = buffer("a\r\nb");
    let text = buffer_text(&buffer);
    let version = buffer.version();
    let err = buffer
        .edit(
            [
                Edit::insert(buffer.len_bytes(), "!".to_string()).unwrap(),
                Edit::insert(b(2), "x".to_string()).unwrap(),
            ],
            TransactionMetadata::default(),
        )
        .unwrap_err();

    assert!(matches!(
        err,
        TextError::Edit(EditError::InvalidBoundary { offset }) if offset == b(2)
    ));
    assert_eq!(buffer_text(&buffer), text);
    assert_eq!(buffer.version(), version);
}

#[test]
fn undo_redo_should_restore_text_and_dirty_state_and_return_history_identity() {
    let mut buffer = buffer("abc");
    let outcome = buffer
        .edit([Edit::insert(b(1), "X").unwrap()], metadata("insert"))
        .unwrap();
    let selection_transaction_id = outcome.history_transaction_id().unwrap();
    buffer.mark_saved();
    buffer
        .edit(
            [Edit::insert(b(4), "!").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();

    assert_eq!(buffer_text(&buffer), "aXbc!");
    assert!(buffer.is_dirty());
    assert!(buffer.can_undo());

    let undo = buffer.undo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "aXbc");
    assert!(!buffer.is_dirty());
    assert_ne!(undo.transaction_id(), selection_transaction_id);

    let redo = buffer.redo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "aXbc!");
    assert!(buffer.is_dirty());
    assert_eq!(redo.transaction_id(), undo.transaction_id());
}

#[test]
fn explicit_history_merge_should_return_one_canonical_identity_for_editor_selection_history() {
    let mut buffer = buffer("");
    let mut canonical_transaction_id = None;

    for (index, text) in ["a", "b", "c"].into_iter().enumerate() {
        let metadata = if index == 0 {
            metadata("insert")
        } else {
            merge_metadata("insert")
        };
        let outcome = buffer
            .edit([Edit::insert(buffer.len_bytes(), text).unwrap()], metadata)
            .unwrap();
        let history_transaction_id = outcome.history_transaction_id().unwrap();
        if let Some(expected) = canonical_transaction_id {
            assert_eq!(history_transaction_id, expected);
        } else {
            canonical_transaction_id = Some(history_transaction_id);
        }
    }

    assert_eq!(buffer_text(&buffer), "abc");
    assert!(buffer.can_undo());

    let undo = buffer.undo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "");
    assert_eq!(undo.transaction_id(), canonical_transaction_id.unwrap());

    let redo = buffer.redo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "abc");
    assert_eq!(redo.transaction_id(), canonical_transaction_id.unwrap());
}

#[test]
fn merged_transaction_undo_publishes_one_composed_change_batch() {
    let mut buffer = buffer("ab");
    let changes = buffer.subscribe();

    buffer
        .edit(
            [Edit::insert(b(1), "z").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    changes.consume();
    for (range, replacement) in [(range(1, 2), "zh"), (range(1, 3), "中")] {
        buffer
            .edit(
                [Edit::replace(range, replacement)],
                TransactionMetadata::new(TransactionSource::Programmatic)
                    .with_merge_policy(TransactionMergePolicy::MergeWithPrevious),
            )
            .unwrap();
        changes.consume();
    }

    buffer.undo().unwrap().expect("合并事务应可撤销");
    let undo_changes = changes.consume();

    assert_eq!(buffer_text(&buffer), "ab");
    assert_eq!(undo_changes.old_version(), Some(BufferVersion::new(3)));
    assert_eq!(undo_changes.new_version(), Some(BufferVersion::new(6)));
    assert_eq!(undo_changes.patch().edits().len(), 1);
    assert_eq!(undo_changes.patch().edits()[0].old_range(), range(1, 4));
    assert_eq!(undo_changes.patch().edits()[0].new_range(), range(1, 1));
}

#[test]
fn default_transactions_should_stay_separate() {
    let mut buffer = buffer("");
    buffer
        .edit(
            [Edit::insert(ByteOffset::ZERO, "a").unwrap()],
            metadata("insert"),
        )
        .unwrap();
    buffer
        .edit([Edit::delete(range(0, 1))], metadata("delete"))
        .unwrap();

    assert_eq!(buffer_text(&buffer), "");
    assert!(buffer.can_undo());

    buffer.undo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "a");

    buffer.undo().unwrap().unwrap();
    assert_eq!(buffer_text(&buffer), "");
}

#[test]
fn set_config_should_apply_the_new_history_budget_immediately() {
    let mut buffer = buffer("");
    buffer
        .edit(
            [Edit::insert(ByteOffset::ZERO, "a").unwrap()],
            metadata("insert"),
        )
        .unwrap();
    assert!(buffer.can_undo());

    let mut config = buffer.config().clone();
    config.large_file.max_undo_history = 0;
    buffer.set_config(config);

    assert!(!buffer.can_undo());
}

#[test]
fn transaction_should_not_report_history_identity_when_history_is_disabled() {
    let mut buffer = Buffer::from_text(
        String::new(),
        BufferConfig {
            large_file: LargeFilePolicy {
                max_undo_history: 0,
                ..LargeFilePolicy::default()
            },
        },
    )
    .unwrap();

    let outcome = buffer
        .edit(
            [Edit::insert(ByteOffset::ZERO, "a").unwrap()],
            metadata("insert"),
        )
        .unwrap();

    assert_eq!(outcome.event().transaction_id(), TransactionId::INITIAL);
    assert!(outcome.history_transaction_id().is_none());
    assert!(!buffer.can_undo());
}

#[test]
fn editing_after_undo_creates_a_new_branch_and_redo_follows_the_latest_one() {
    let mut buffer = buffer("a");

    buffer
        .edit(
            [Edit::insert(b(1), "b").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    buffer.undo().unwrap().unwrap();
    buffer
        .edit(
            [Edit::insert(b(1), "c").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap();
    buffer.undo().unwrap().unwrap();

    // 撤销后重新编辑形成分支；redo 沿最近创建的默认分支回放。
    buffer.redo().unwrap().expect("应可 redo 默认分支");
    assert_eq!(buffer_text(&buffer), "ac");
}

#[test]
fn large_transaction_reject_policy_should_preserve_history_and_state() {
    let mut buffer = Buffer::from_text(
        "abc".to_string(),
        BufferConfig {
            large_file: LargeFilePolicy {
                large_transaction_threshold_bytes: 2,
                large_transaction_policy: LargeTransactionPolicy::Reject,
                ..LargeFilePolicy::default()
            },
        },
    )
    .unwrap();
    let version = buffer.version();

    let err = buffer
        .edit(
            [Edit::insert(b(3), "long").unwrap()],
            TransactionMetadata::default(),
        )
        .unwrap_err();

    assert!(matches!(
        err,
        TextError::Edit(EditError::PayloadTooLarge { size, limit }) if size > limit
    ));
    assert_eq!(buffer_text(&buffer), "abc");
    assert_eq!(buffer.version(), version);
    assert!(!buffer.can_undo());
}
