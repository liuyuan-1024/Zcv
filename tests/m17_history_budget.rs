//! M17B 机器契约：锁定历史预算（节点数 + 字节）截断、超大事务策略
//! （Reject / SkipHistory）、运行时调整 LargeFilePolicy 触发截断、以及
//! `HistoryStatus` 的 `node_count` / `memory_bytes` 观测语义。

use zom_engine::{
    Buffer, BufferConfig, CharOffset, Edit, EditError, EngineError, LargeFilePolicy,
    LargeTransactionPolicy, TextRange, Transaction, TransactionMergePolicy, TransactionMetadata,
    TransactionSource,
};

fn buffer_with(config: BufferConfig) -> Buffer {
    Buffer::from_text(String::new(), config).unwrap()
}

fn small_budget_config(
    max_undo_history: usize,
    max_undo_history_bytes: usize,
    threshold: usize,
    policy: LargeTransactionPolicy,
) -> BufferConfig {
    let mut config = BufferConfig::default();
    config.large_file = LargeFilePolicy {
        max_undo_history,
        max_undo_history_bytes,
        large_transaction_threshold_bytes: threshold,
        large_transaction_policy: policy,
        ..LargeFilePolicy::default()
    };
    config
}

fn c(value: usize) -> CharOffset {
    CharOffset::new(value)
}

fn commit_insert(buffer: &mut Buffer, offset: usize, text: &str) {
    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(offset), text.to_string()).unwrap()],
    )
    .unwrap();
    buffer.apply_transaction(tx).unwrap();
}

fn commit_insert_with(
    buffer: &mut Buffer,
    offset: usize,
    text: &str,
    metadata: TransactionMetadata,
) {
    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(offset), text.to_string()).unwrap()],
    )
    .unwrap()
    .with_metadata(metadata);
    buffer.apply_transaction(tx).unwrap();
}

#[test]
fn default_policy_keeps_history_node_count_budget() {
    let policy = LargeFilePolicy::default();
    assert_eq!(policy.max_undo_history, 1000);
    assert!(policy.max_undo_history_bytes > 0);
    assert!(policy.large_transaction_threshold_bytes > 0);
    assert_eq!(
        policy.large_transaction_policy,
        LargeTransactionPolicy::SkipHistory
    );

    // 默认 64MiB 字节预算下，几十次小事务不会触发字节截断。
    let mut buffer = buffer_with(BufferConfig::default());
    for i in 0..50 {
        commit_insert(&mut buffer, i, "x");
    }
    assert_eq!(buffer.history_status().node_count, 50);
}

#[test]
fn byte_budget_drops_oldest_leaf_until_under_limit() {
    // 节点上限够大，仅按字节预算截断。
    let mut buffer = buffer_with(small_budget_config(
        100,
        20,
        0,
        LargeTransactionPolicy::SkipHistory,
    ));

    for i in 0..30 {
        commit_insert(&mut buffer, i, "x");
    }

    let status = buffer.history_status();
    assert!(
        status.memory_bytes <= 20,
        "字节占用应被截断到预算内: {}",
        status.memory_bytes
    );
    // current 节点必须保留。
    assert!(buffer.current_history_node().is_some());
    // 总节点数远小于 30，老叶子被丢。
    assert!(
        status.node_count < 30,
        "节点应被丢弃，实际剩余 {}",
        status.node_count
    );
}

#[test]
fn byte_budget_does_not_drop_current_when_only_node() {
    // 极小的字节预算（1 byte），但单次插入产生的 entry > 1 byte。
    let mut buffer = buffer_with(small_budget_config(
        100,
        1,
        0,
        LargeTransactionPolicy::SkipHistory,
    ));

    commit_insert(&mut buffer, 0, "long replacement text");
    let status = buffer.history_status();
    assert_eq!(status.node_count, 1, "current 节点不能被丢弃");
    assert!(buffer.current_history_node().is_some());
    // 字节预算被允许超出，因为没有可丢弃的非 current 叶子。
    assert!(status.memory_bytes > 1);
}

#[test]
fn count_and_byte_budget_apply_simultaneously() {
    let mut buffer = buffer_with(small_budget_config(
        3,
        u64::MAX as usize,
        0,
        LargeTransactionPolicy::SkipHistory,
    ));

    for i in 0..10 {
        commit_insert(&mut buffer, i, "x");
    }

    assert_eq!(
        buffer.history_status().node_count,
        3,
        "节点数预算先生效，截断到 3"
    );

    // 调小字节预算，触发额外截断。
    buffer.set_large_file_policy(LargeFilePolicy {
        max_undo_history: 3,
        max_undo_history_bytes: 1,
        large_transaction_threshold_bytes: 0,
        large_transaction_policy: LargeTransactionPolicy::SkipHistory,
        ..LargeFilePolicy::default()
    });

    let status = buffer.history_status();
    assert_eq!(status.node_count, 1, "字节预算继续截断，仅保留 current");
    assert!(buffer.current_history_node().is_some());
}

#[test]
fn large_transaction_reject_keeps_buffer_unchanged() {
    let mut buffer = buffer_with(small_budget_config(
        100,
        usize::MAX,
        4,
        LargeTransactionPolicy::Reject,
    ));
    commit_insert(&mut buffer, 0, "ab");
    let baseline_text = buffer.snapshot().text().to_string();
    let baseline_version = buffer.version();
    let baseline_status = buffer.history_status();
    let baseline_dirty = buffer.is_dirty();

    let payload = "x".repeat(64);
    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(2), payload.clone()).unwrap()],
    )
    .unwrap();
    let outcome = buffer.apply_transaction(tx);

    match outcome {
        Err(EngineError::Edit(EditError::PayloadTooLarge { size, limit })) => {
            assert!(size > limit, "size={size} limit={limit}");
            assert_eq!(limit, 4);
        }
        other => panic!("Reject 策略应返回 PayloadTooLarge，实际 {other:?}"),
    }

    assert_eq!(buffer.snapshot().text().to_string(), baseline_text);
    assert_eq!(buffer.version(), baseline_version);
    assert_eq!(buffer.history_status(), baseline_status);
    assert_eq!(buffer.is_dirty(), baseline_dirty);
}

#[test]
fn large_transaction_skip_history_advances_text_but_skips_history() {
    let mut buffer = buffer_with(small_budget_config(
        100,
        usize::MAX,
        4,
        LargeTransactionPolicy::SkipHistory,
    ));
    commit_insert(&mut buffer, 0, "ab");
    let recorded_node = buffer.current_history_node().unwrap();
    let baseline_status = buffer.history_status();

    let payload = "x".repeat(64);
    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(2), payload.clone()).unwrap()],
    )
    .unwrap();
    buffer
        .apply_transaction(tx)
        .expect("SkipHistory 仍应提交文本");

    let mut expected = String::from("ab");
    expected.push_str(&payload);
    assert_eq!(buffer.snapshot().text(), expected);
    // 文本前进但当前节点不变（未入历史）。
    assert_eq!(buffer.current_history_node(), Some(recorded_node));
    assert_eq!(
        buffer.history_status().node_count,
        baseline_status.node_count
    );
    assert_eq!(
        buffer.history_status().memory_bytes,
        baseline_status.memory_bytes
    );
}

#[test]
fn set_large_file_policy_triggers_immediate_truncation() {
    let mut buffer = buffer_with(BufferConfig::default());
    for i in 0..20 {
        commit_insert(&mut buffer, i, "x");
    }
    assert_eq!(buffer.history_status().node_count, 20);

    buffer.set_large_file_policy(LargeFilePolicy {
        max_undo_history: 5,
        max_undo_history_bytes: 0,
        large_transaction_threshold_bytes: 0,
        large_transaction_policy: LargeTransactionPolicy::SkipHistory,
        ..LargeFilePolicy::default()
    });

    let status = buffer.history_status();
    assert_eq!(status.node_count, 5);
    // current 节点保持，文本不变。
    assert!(buffer.current_history_node().is_some());
    assert_eq!(buffer.snapshot().text().len(), 20);
}

#[test]
fn set_large_file_policy_zero_max_clears_history() {
    let mut buffer = buffer_with(BufferConfig::default());
    for i in 0..5 {
        commit_insert(&mut buffer, i, "x");
    }
    assert!(buffer.current_history_node().is_some());

    buffer.set_large_file_policy(LargeFilePolicy {
        max_undo_history: 0,
        max_undo_history_bytes: 0,
        large_transaction_threshold_bytes: 0,
        large_transaction_policy: LargeTransactionPolicy::SkipHistory,
        ..LargeFilePolicy::default()
    });

    let status = buffer.history_status();
    assert_eq!(status.node_count, 0);
    assert_eq!(status.memory_bytes, 0);
    assert!(buffer.current_history_node().is_none());
    // 文本本身不被清空。
    assert_eq!(buffer.snapshot().text().len(), 5);
}

#[test]
fn merge_with_previous_accumulates_byte_size() {
    let mut buffer = buffer_with(BufferConfig::default());
    commit_insert(&mut buffer, 0, "abc");
    let bytes_after_first = buffer.history_status().memory_bytes;

    let merge_metadata = TransactionMetadata::new(TransactionSource::Programmatic)
        .with_merge_policy(TransactionMergePolicy::MergeWithPrevious);
    commit_insert_with(&mut buffer, 3, "defgh", merge_metadata);

    let status = buffer.history_status();
    assert_eq!(status.node_count, 1, "MergeWithPrevious 不开新节点");
    assert!(
        status.memory_bytes > bytes_after_first,
        "合并后字节累加: 之前 {}, 之后 {}",
        bytes_after_first,
        status.memory_bytes
    );
}

#[test]
fn history_status_exposes_node_count_and_memory_bytes() {
    let mut buffer = buffer_with(BufferConfig::default());
    let initial = buffer.history_status();
    assert_eq!(initial.node_count, 0);
    assert_eq!(initial.memory_bytes, 0);

    commit_insert(&mut buffer, 0, "hello");
    let after_first = buffer.history_status();
    assert_eq!(after_first.node_count, 1);
    assert!(after_first.memory_bytes >= "hello".len());

    commit_insert(&mut buffer, 5, ", world");
    let after_second = buffer.history_status();
    assert_eq!(after_second.node_count, 2);
    assert!(after_second.memory_bytes > after_first.memory_bytes);
}

#[test]
fn byte_threshold_zero_disables_large_transaction_policy() {
    // threshold = 0 表示不限，超大事务正常入历史。
    let mut buffer = buffer_with(small_budget_config(
        100,
        usize::MAX,
        0,
        LargeTransactionPolicy::Reject,
    ));

    let payload = "x".repeat(1024);
    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(0), payload.clone()).unwrap()],
    )
    .unwrap();
    buffer
        .apply_transaction(tx)
        .expect("threshold=0 时不应触发 Reject");

    assert_eq!(buffer.snapshot().text(), payload);
    assert_eq!(buffer.history_status().node_count, 1);
}

#[test]
fn large_transaction_skip_history_drops_pending_redo_branches() {
    let mut buffer = buffer_with(small_budget_config(
        100,
        usize::MAX,
        4,
        LargeTransactionPolicy::SkipHistory,
    ));
    commit_insert(&mut buffer, 0, "ab");
    let leaf = buffer.current_history_node().unwrap();
    buffer.undo().unwrap();
    assert_eq!(buffer.redo_branches(), vec![leaf]);

    // 触发 SkipHistory 路径：超阈值事务提交后，redo 分支被作废
    // （与 record_history=false 路径一致）。
    let payload = "y".repeat(64);
    let tx = Transaction::from_edits(buffer.version(), vec![Edit::insert(c(0), payload).unwrap()])
        .unwrap();
    buffer.apply_transaction(tx).unwrap();

    assert!(buffer.current_history_node().is_none(), "未入历史");
    assert!(buffer.redo_branches().is_empty(), "redo 分支被丢弃");
    assert!(buffer.history_node(leaf).is_none(), "原 leaf 被裁掉");
}

#[test]
fn byte_budget_truncation_preserves_current_chain_text() {
    // 写一系列不同位置的 insert，再 undo 一次：current 不在最尾时仍不丢 current。
    let mut buffer = buffer_with(small_budget_config(
        100,
        4,
        0,
        LargeTransactionPolicy::SkipHistory,
    ));
    for i in 0..6 {
        commit_insert(&mut buffer, i, "z");
    }
    let original_text = buffer.snapshot().text().to_string();
    buffer.undo().unwrap();
    let undone_version = buffer.version();
    let current_after_undo = buffer.current_history_node();

    // 触发再一次 truncate（policy 没变，但每次提交都会调用 truncate）。
    // 直接调用 set_large_file_policy 重新触发即可观察。
    buffer.set_large_file_policy(LargeFilePolicy {
        max_undo_history: 100,
        max_undo_history_bytes: 4,
        large_transaction_threshold_bytes: 0,
        large_transaction_policy: LargeTransactionPolicy::SkipHistory,
        ..LargeFilePolicy::default()
    });

    assert_eq!(buffer.current_history_node(), current_after_undo);
    assert_eq!(buffer.version(), undone_version);
    // 文本本身不受历史截断影响。
    assert_ne!(buffer.snapshot().text(), original_text); // undo 已生效
}

#[test]
fn merge_with_previous_byte_budget_can_evict_after_growth() {
    // MergeWithPrevious 后字节增大；如果新预算低于增大后的字节，截断不应丢 current。
    let mut buffer = buffer_with(BufferConfig::default());
    commit_insert(&mut buffer, 0, "ab");
    let merge_metadata = TransactionMetadata::new(TransactionSource::Programmatic)
        .with_merge_policy(TransactionMergePolicy::MergeWithPrevious);
    commit_insert_with(&mut buffer, 2, "cd", merge_metadata);

    let status = buffer.history_status();
    assert_eq!(status.node_count, 1);
    let merged_bytes = status.memory_bytes;

    // 把字节预算调到比合并后字节小，但只有 current 节点 → 不被丢弃。
    buffer.set_large_file_policy(LargeFilePolicy {
        max_undo_history: 100,
        max_undo_history_bytes: merged_bytes / 2,
        large_transaction_threshold_bytes: 0,
        large_transaction_policy: LargeTransactionPolicy::SkipHistory,
        ..LargeFilePolicy::default()
    });
    assert_eq!(buffer.history_status().node_count, 1);
    assert!(buffer.current_history_node().is_some());
}

#[test]
fn replace_edit_byte_size_includes_inverse_replacement() {
    // 一次 replace：edit.replacement() 为空（pure delete）也会因 inverse_edits 占用字节。
    let mut buffer = Buffer::from_text("hello".into(), BufferConfig::default()).unwrap();
    let pre_status = buffer.history_status();
    assert_eq!(pre_status.memory_bytes, 0);

    let range = TextRange::new(c(0), c(5)).unwrap();
    let tx = Transaction::from_edits(buffer.version(), vec![Edit::delete(range)]).unwrap();
    buffer.apply_transaction(tx).unwrap();

    let status = buffer.history_status();
    assert!(
        status.memory_bytes >= "hello".len(),
        "deletion 的字节占用来自 inverse_edits.replacement: {}",
        status.memory_bytes
    );
}
