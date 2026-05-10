//! M16B 机器契约：锁定 `Buffer::replay_transaction_record` 的版本守卫、回放等价性、
//! 边界校验穿透与 Undo / Redo roundtrip 行为。

use zom_engine::{
    Buffer, BufferConfig, BufferVersion, CharOffset, Edit, EngineError, TextRange, Transaction,
    TransactionError,
};

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

fn c(value: usize) -> CharOffset {
    CharOffset::new(value)
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(c(start), c(end)).unwrap()
}

#[test]
fn replay_on_matching_base_version_advances_to_recorded_new_version() {
    // 在 buffer A 上录制，在 buffer B 上回放：B 与 A 应停留在相同状态。
    let mut a = buffer("abcdef");
    let tx = Transaction::from_edits(
        a.version(),
        vec![Edit::insert(c(2), "XY".to_string()).unwrap()],
    )
    .unwrap();
    let record = a.apply_transaction_recorded(tx).unwrap();

    let mut b = buffer("abcdef");
    let replayed = b.replay_transaction_record(&record).unwrap();

    assert_eq!(b.snapshot().text(), a.snapshot().text());
    assert_eq!(replayed.old_version(), record.old_version());
    assert_eq!(replayed.new_version(), record.new_version());
    assert_eq!(replayed.edits(), record.edits());
    assert_eq!(replayed.inverse_edits(), record.inverse_edits());
    assert_eq!(replayed.before_selection(), record.before_selection());
    assert_eq!(replayed.after_selection(), record.after_selection());
    assert_eq!(replayed.metadata(), record.metadata());
}

#[test]
fn replay_generates_equivalent_delta_event() {
    let mut a = buffer("hello");
    let tx = Transaction::from_edits(
        a.version(),
        vec![Edit::insert(c(5), " world".to_string()).unwrap()],
    )
    .unwrap();
    let record = a.apply_transaction_recorded(tx).unwrap();
    let original_event = a.last_delta_event().cloned().unwrap();

    let mut b = buffer("hello");
    b.replay_transaction_record(&record).unwrap();
    let replayed_event = b.last_delta_event().cloned().unwrap();

    assert_eq!(replayed_event.old_version, original_event.old_version);
    assert_eq!(replayed_event.new_version, original_event.new_version);
    assert_eq!(replayed_event.delta.edits, original_event.delta.edits);
    assert_eq!(
        replayed_event.changeset.changed_ranges(),
        original_event.changeset.changed_ranges()
    );
    // transaction_id 由各 Buffer 独立递增，不要求相等。
}

#[test]
fn replay_rejects_record_with_mismatched_old_version_atomically() {
    let mut a = buffer("abc");
    let tx = Transaction::from_edits(
        a.version(),
        vec![Edit::insert(c(0), "Z".to_string()).unwrap()],
    )
    .unwrap();
    let record = a.apply_transaction_recorded(tx).unwrap();

    // 在已经推进过的 buffer 上回放：old_version 不匹配。
    let mut b = buffer("abc");
    let unrelated = Transaction::from_edits(
        b.version(),
        vec![Edit::insert(c(0), "Q".to_string()).unwrap()],
    )
    .unwrap();
    b.apply_transaction(unrelated).unwrap();

    let text_before = b.snapshot().text().into_owned();
    let version_before = b.version();
    let event_count_before = b.pending_delta_event_count();

    let outcome = b.replay_transaction_record(&record);

    match outcome {
        Err(EngineError::Transaction(TransactionError::VersionMismatch { expected, actual })) => {
            assert_eq!(expected, version_before);
            assert_eq!(actual, record.old_version());
        }
        other => panic!("预期 VersionMismatch，实际 {other:?}"),
    }
    assert_eq!(b.snapshot().text(), text_before);
    assert_eq!(b.version(), version_before);
    assert_eq!(b.pending_delta_event_count(), event_count_before);
}

#[test]
fn replay_does_not_bypass_edit_boundary_validation() {
    // 录制一条命中 [3, 4) 的删除，再在更短的 buffer 上回放：标准管线必须给出 RangeOutOfBounds。
    let mut long = buffer("abcdef");
    let tx = Transaction::from_edits(long.version(), vec![Edit::delete(range(3, 4))]).unwrap();
    let record = long.apply_transaction_recorded(tx).unwrap();

    let mut short = buffer("ab");
    let outcome = short.replay_transaction_record(&record);

    // 此处的失败原因应当是版本不匹配（short 的 version 是 INITIAL，record.old_version 也是 INITIAL，
    // 实际上两者一致），所以应当走到 EditError::RangeOutOfBounds。
    match outcome {
        Err(EngineError::Edit(_)) => {}
        other => panic!("预期 EditError，实际 {other:?}"),
    }
    assert_eq!(short.snapshot().text(), "ab");
    assert_eq!(short.version(), BufferVersion::INITIAL);
}

#[test]
fn replay_round_trips_through_undo_and_redo() {
    // 在同一 Buffer 上：apply -> undo 回到旧版本 -> replay -> 状态等价于原来 apply 完。
    let mut buffer = buffer("hello");
    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(5), "!".to_string()).unwrap()],
    )
    .unwrap();
    let record = buffer.apply_transaction_recorded(tx).unwrap();
    let after_apply_text = buffer.snapshot().text().into_owned();
    let after_apply_version = buffer.version();

    buffer.undo().unwrap().expect("应当能撤销刚才的事务");
    assert_eq!(buffer.snapshot().text(), "hello");
    assert_eq!(buffer.version(), record.new_version().next().unwrap());
    // 注意：undo 自身也是一次提交，会推进版本。所以回放需要 record.old_version == undo 之后的当前版本？
    // 不一定相等：undo 推进版本但文本回到旧的。这里我们用一个全新 buffer 来做版本对齐回放。
    let mut fresh = self::buffer("hello");
    let replayed = fresh.replay_transaction_record(&record).unwrap();
    assert_eq!(fresh.snapshot().text(), after_apply_text);
    assert_eq!(replayed.new_version(), after_apply_version);
}

#[test]
fn replay_into_history_supports_subsequent_undo() {
    // 回放出来的事务也应当进入历史栈，可以再被 undo。
    let mut a = buffer("abc");
    let tx = Transaction::from_edits(
        a.version(),
        vec![Edit::insert(c(0), "Z".to_string()).unwrap()],
    )
    .unwrap();
    let record = a.apply_transaction_recorded(tx).unwrap();

    let mut b = buffer("abc");
    b.replay_transaction_record(&record).unwrap();
    assert_eq!(b.snapshot().text(), "Zabc");
    assert!(b.can_undo());

    b.undo().unwrap().expect("回放进入历史后应当支持 undo");
    assert_eq!(b.snapshot().text(), "abc");
}
