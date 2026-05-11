//! M16A 机器契约：锁定 `TransactionRecord` 的字段完整性、版本/选区/元数据捕获、
//! history merge boundary 派生与 `to_transaction()` 重建。

use zom_engine::{
    Buffer, BufferConfig, BufferVersion, CharOffset, Edit, SelectionSet, TextRange, Transaction,
    TransactionMergePolicy, TransactionMetadata, TransactionSource,
};

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
}

fn c(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

fn range(start: usize, end: usize) -> TextRange {
    TextRange::new(c(start), c(end)).unwrap()
}

fn cursor_at(offset: usize) -> SelectionSet {
    SelectionSet::caret(c(offset))
}

#[test]
fn record_captures_versions_edits_and_inverse_edits() {
    let mut buffer = buffer("abcdef");
    let base_version = buffer.version();
    let tx = Transaction::from_edits(
        base_version,
        vec![Edit::insert(c(2), "XY".to_string()).unwrap()],
    )
    .unwrap();

    let record = buffer.apply_transaction_recorded(tx).unwrap();

    assert_eq!(record.old_version(), base_version);
    assert_eq!(record.new_version(), buffer.version());
    assert_ne!(record.new_version(), record.old_version());

    let forward = record.edits().as_slice();
    assert_eq!(forward.len(), 1);
    assert_eq!(forward[0].range(), range(2, 2));
    assert_eq!(forward[0].replacement(), "XY");

    let inverse = record.inverse_edits().as_slice();
    assert_eq!(inverse.len(), 1);
    // 在新文本上 [2, 4) 对应插入的 "XY"，inverse 把它替换回空字符串。
    assert_eq!(inverse[0].range(), range(2, 4));
    assert_eq!(inverse[0].replacement(), "");
}

#[test]
fn record_captures_before_and_after_selection_with_position_map_default() {
    let mut buffer = buffer("abcdef");
    let initial = cursor_at(3);
    buffer.set_selection(initial.clone()).unwrap();

    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
    )
    .unwrap();
    let record = buffer.apply_transaction_recorded(tx).unwrap();

    assert_eq!(record.before_selection(), &initial);
    // 默认 after_selection 由 PositionMap 平移；插入 3 个字符，原 caret @ 3 应移到 6。
    assert_eq!(record.after_selection(), &cursor_at(6));
    assert_eq!(record.after_selection(), &buffer.selection().clone());
}

#[test]
fn record_captures_explicit_after_selection_when_provided() {
    let mut buffer = buffer("abcdef");
    let initial = cursor_at(0);
    buffer.set_selection(initial.clone()).unwrap();
    let explicit_after = cursor_at(2);

    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(0), "Z".to_string()).unwrap()],
    )
    .unwrap()
    .with_selection(Some(initial.clone()), Some(explicit_after.clone()));

    let record = buffer.apply_transaction_recorded(tx).unwrap();

    assert_eq!(record.before_selection(), &initial);
    assert_eq!(record.after_selection(), &explicit_after);
}

#[test]
fn record_carries_metadata_and_derives_merge_boundary() {
    let mut buffer = buffer("abc");
    let metadata = TransactionMetadata::new(TransactionSource::Programmatic)
        .with_description("first edit")
        .with_merge_policy(TransactionMergePolicy::Never);
    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(0), "X".to_string()).unwrap()],
    )
    .unwrap()
    .with_metadata(metadata.clone());

    let first = buffer.apply_transaction_recorded(tx).unwrap();
    assert_eq!(first.metadata(), &metadata);
    assert!(first.records_history());
    assert!(
        first.is_merge_boundary(),
        "Never 策略下是独立 Undo 步骤的边界"
    );

    let merge_metadata = TransactionMetadata::new(TransactionSource::Programmatic)
        .with_merge_policy(TransactionMergePolicy::MergeWithPrevious);
    let merge_tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(1), "Y".to_string()).unwrap()],
    )
    .unwrap()
    .with_metadata(merge_metadata);

    let merged = buffer.apply_transaction_recorded(merge_tx).unwrap();
    assert!(
        !merged.is_merge_boundary(),
        "MergeWithPrevious 策略并入上一节点"
    );
}

#[test]
fn record_skips_history_when_metadata_disables_recording() {
    let mut buffer = buffer("abc");
    let metadata = TransactionMetadata::new(TransactionSource::Programmatic).without_history();
    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(0), "Z".to_string()).unwrap()],
    )
    .unwrap()
    .with_metadata(metadata);

    let record = buffer.apply_transaction_recorded(tx).unwrap();

    assert!(!record.records_history());
    assert!(!buffer.can_undo());
}

#[test]
fn record_transaction_id_matches_last_delta_event() {
    let mut buffer = buffer("abc");
    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(0), "Z".to_string()).unwrap()],
    )
    .unwrap();

    let record = buffer.apply_transaction_recorded(tx).unwrap();
    let event = buffer.last_delta_event().unwrap();

    assert_eq!(record.transaction_id(), event.transaction_id);
    assert_eq!(record.old_version(), event.old_version);
    assert_eq!(record.new_version(), event.new_version);
}

#[test]
fn to_transaction_rebuilds_equivalent_transaction_payload() {
    let mut buffer = buffer("abc");
    buffer.set_selection(cursor_at(0)).unwrap();
    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(2), "XY".to_string()).unwrap()],
    )
    .unwrap()
    .with_metadata(
        TransactionMetadata::new(TransactionSource::Programmatic).with_description("note"),
    );

    let record = buffer.apply_transaction_recorded(tx).unwrap();
    let rebuilt = record.to_transaction();

    assert_eq!(rebuilt.base_version(), record.old_version());
    assert_eq!(rebuilt.edits(), record.edits());
    assert_eq!(rebuilt.metadata(), record.metadata());
    assert_eq!(rebuilt.before_selection(), Some(record.before_selection()));
    assert_eq!(rebuilt.after_selection(), Some(record.after_selection()));
}

#[test]
fn record_does_not_change_when_subsequent_transactions_run() {
    let mut buffer = buffer("abc");
    let tx_a = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(0), "A".to_string()).unwrap()],
    )
    .unwrap();
    let record_a = buffer.apply_transaction_recorded(tx_a).unwrap();
    let snapshot_record = record_a.clone();

    let tx_b = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(0), "B".to_string()).unwrap()],
    )
    .unwrap();
    let _ = buffer.apply_transaction_recorded(tx_b).unwrap();

    // 旧 record 是值类型快照，不会跟随 Buffer 推进。
    assert_eq!(record_a, snapshot_record);
}

#[test]
fn version_mismatch_returns_error_and_does_not_produce_record() {
    let mut buffer = buffer("abc");
    let stale = BufferVersion::new(buffer.version().get() + 99);
    let tx =
        Transaction::from_edits(stale, vec![Edit::insert(c(0), "Z".to_string()).unwrap()]).unwrap();

    let outcome = buffer.apply_transaction_recorded(tx);

    assert!(outcome.is_err());
    assert_eq!(buffer.version(), BufferVersion::INITIAL);
    assert_eq!(buffer.snapshot().text(), "abc");
}
