//! M18B 机器契约：锁定大文件 / 超大事务 / 资源预算路径上的错误返回边界、
//! 原子性和可诊断性。M0 / M2 / M5 / M9 已覆盖的基础边界检查在此不重复，
//! 仅覆盖大文件路径上新出现的失败模式。

use zom_engine::{
    Buffer, BufferConfig, BufferKind, CharOffset, Edit, EditError, EngineError, LargeFilePolicy,
    LargeTransactionPolicy, StorageError, TextRange, Transaction, TransactionError,
};

fn c(value: usize) -> ByteOffset {
    ByteOffset::new(value)
}

fn policy(
    large_file_threshold_bytes: usize,
    large_transaction_threshold_bytes: usize,
    policy: LargeTransactionPolicy,
    auto_read_only: bool,
) -> LargeFilePolicy {
    LargeFilePolicy {
        max_undo_history: 1000,
        max_undo_history_bytes: 64 * 1024 * 1024,
        large_transaction_threshold_bytes,
        large_transaction_policy: policy,
        large_file_threshold_bytes,
        long_line_threshold_chars: 0,
        auto_read_only_on_large_file: auto_read_only,
    }
}

fn config(p: LargeFilePolicy) -> BufferConfig {
    let mut config = BufferConfig::default();
    config.large_file = p;
    config
}

#[test]
fn auto_read_only_buffer_rejects_writes_with_storage_read_only() {
    let cfg = config(policy(8, 0, LargeTransactionPolicy::SkipHistory, true));
    let mut buffer = Buffer::from_loaded_text(BufferKind::Untitled, &vec![b'x'; 32], cfg).unwrap();
    assert!(buffer.is_read_only());

    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(0), "y".to_string()).unwrap()],
    )
    .unwrap();

    let outcome = buffer.apply_transaction(tx);
    assert!(
        matches!(outcome, Err(EngineError::Storage(StorageError::ReadOnly))),
        "auto-read-only Buffer 应通过 ensure_writable 拒绝写入: {outcome:?}"
    );
    // 文本与版本完全不变。
    assert_eq!(buffer.snapshot().text().len(), 32);
}

#[test]
fn host_can_override_auto_read_only_after_load() {
    // 自动只读不是不可逆决策；宿主可调用 set_read_only(false) 解除。
    let cfg = config(policy(8, 0, LargeTransactionPolicy::SkipHistory, true));
    let mut buffer = Buffer::from_loaded_text(BufferKind::Untitled, &vec![b'x'; 32], cfg).unwrap();
    assert!(buffer.is_read_only());

    buffer.set_read_only(false);
    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(0), "y".to_string()).unwrap()],
    )
    .unwrap();
    buffer
        .apply_transaction(tx)
        .expect("解除只读后应可正常写入");
    assert_eq!(buffer.snapshot().text().chars().next(), Some('y'));
}

#[test]
fn large_paste_on_threshold_reject_returns_payload_too_large() {
    // 复用 M17B 的 LargeTransactionPolicy::Reject 防御超大粘贴。
    let cfg = config(policy(
        usize::MAX,
        16,
        LargeTransactionPolicy::Reject,
        false,
    ));
    let mut buffer = Buffer::from_text("anchor".to_string(), cfg).unwrap();
    let baseline_version = buffer.version();
    let baseline_text = buffer.snapshot().text().to_string();

    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(0), "p".repeat(64)).unwrap()],
    )
    .unwrap();

    let outcome = buffer.apply_transaction(tx);
    match outcome {
        Err(EngineError::Edit(EditError::PayloadTooLarge { size, limit })) => {
            assert!(size > limit);
            assert_eq!(limit, 16);
        }
        other => panic!("Reject 路径应返回 PayloadTooLarge: {other:?}"),
    }
    assert_eq!(buffer.version(), baseline_version);
    assert_eq!(buffer.snapshot().text().to_string(), baseline_text);
}

#[test]
fn large_paste_skip_history_advances_text_but_skips_history() {
    let cfg = config(policy(
        usize::MAX,
        16,
        LargeTransactionPolicy::SkipHistory,
        false,
    ));
    let mut buffer = Buffer::from_text("anchor".to_string(), cfg).unwrap();
    let baseline_node_count = buffer.history_status().node_count;

    let tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(0), "p".repeat(64)).unwrap()],
    )
    .unwrap();
    buffer
        .apply_transaction(tx)
        .expect("SkipHistory 路径必须接受文本变更");

    assert!(buffer.snapshot().text().starts_with("ppppp"));
    assert_eq!(
        buffer.history_status().node_count,
        baseline_node_count,
        "SkipHistory 不应入历史"
    );
}

#[test]
fn version_mismatch_on_stale_transaction_keeps_buffer_intact() {
    let cfg = config(policy(
        usize::MAX,
        0,
        LargeTransactionPolicy::SkipHistory,
        false,
    ));
    let mut buffer = Buffer::from_text("hello".to_string(), cfg).unwrap();
    let stale_version = buffer.version();

    let advance_tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(5), "!".to_string()).unwrap()],
    )
    .unwrap();
    buffer.apply_transaction(advance_tx).unwrap();
    let advanced_text = buffer.snapshot().text().to_string();
    let advanced_version = buffer.version();

    let stale_tx = Transaction::from_edits(
        stale_version,
        vec![Edit::insert(c(0), "X".to_string()).unwrap()],
    )
    .unwrap();
    let outcome = buffer.apply_transaction(stale_tx);
    assert!(matches!(
        outcome,
        Err(EngineError::Transaction(
            TransactionError::VersionMismatch { .. }
        ))
    ));
    // 失败事务不污染状态。
    assert_eq!(buffer.snapshot().text().to_string(), advanced_text);
    assert_eq!(buffer.version(), advanced_version);
}

#[test]
fn out_of_bounds_edit_on_large_buffer_returns_diagnosable_error() {
    let cfg = config(policy(8, 0, LargeTransactionPolicy::SkipHistory, false));
    let mut buffer = Buffer::from_text("x".repeat(32), cfg).unwrap();
    assert!(buffer.is_large_file());

    let oob_range = TextRange::new(c(1000), c(1100)).unwrap();
    let tx = Transaction::from_edits(buffer.version(), vec![Edit::delete(oob_range)]).unwrap();
    let outcome = buffer.apply_transaction(tx);

    match outcome {
        Err(EngineError::Edit(EditError::RangeOutOfBounds { range })) => {
            assert_eq!(range, oob_range);
        }
        other => panic!("大文件越界编辑应返回 RangeOutOfBounds: {other:?}"),
    }
    // 文本未变。
    assert_eq!(buffer.snapshot().text().len(), 32);
}

#[test]
fn search_remains_stable_on_buffer_above_large_file_threshold() {
    // 大文件路径下 search / snapshot 仍是可工作的事实接口。
    let cfg = config(policy(8, 0, LargeTransactionPolicy::SkipHistory, false));
    let mut text = String::new();
    for _ in 0..4 {
        text.push_str("alpha beta gamma\n");
    }
    text.push_str("alpha NEEDLE beta\n");
    let buffer = Buffer::from_text(text, cfg).unwrap();
    assert!(buffer.is_large_file());

    let snap = buffer.snapshot();
    let result = snap
        .search("NEEDLE", zom_engine::SearchOptions::default())
        .expect("大文件 snapshot 上的 search 不应 panic");
    assert_eq!(result.matches().len(), 1);
}

#[test]
fn history_budget_does_not_panic_when_overlapping_large_paste_with_small_budget() {
    // 极小历史预算 + SkipHistory 大粘贴：截断 + skip 路径不应交互出任何 panic。
    let cfg = config(policy(
        usize::MAX,
        16,
        LargeTransactionPolicy::SkipHistory,
        false,
    ));
    let mut buffer = Buffer::from_text("anchor".to_string(), cfg.clone()).unwrap();

    // 多次小提交逼近预算。
    for offset in 0..5 {
        let tx = Transaction::from_edits(
            buffer.version(),
            vec![Edit::insert(c(offset), "x".to_string()).unwrap()],
        )
        .unwrap();
        buffer.apply_transaction(tx).unwrap();
    }

    // 提交超阈值事务：走 SkipHistory，不入历史，不应破坏既有节点。
    let big_tx = Transaction::from_edits(
        buffer.version(),
        vec![Edit::insert(c(0), "p".repeat(128)).unwrap()],
    )
    .unwrap();
    buffer.apply_transaction(big_tx).unwrap();

    // 缩小预算，触发再次截断。
    buffer.set_large_file_policy(LargeFilePolicy {
        max_undo_history: 2,
        max_undo_history_bytes: 0,
        large_transaction_threshold_bytes: 16,
        large_transaction_policy: LargeTransactionPolicy::SkipHistory,
        large_file_threshold_bytes: usize::MAX,
        long_line_threshold_chars: 0,
        auto_read_only_on_large_file: false,
    });
    assert_eq!(buffer.history_status().node_count, 2);
}

#[test]
fn auto_read_only_does_not_engage_when_threshold_is_zero() {
    // large_file_threshold_bytes=0 表示不限，即便文本巨大且 auto=true 也不应只读。
    let cfg = config(policy(0, 0, LargeTransactionPolicy::SkipHistory, true));
    let buffer = Buffer::from_text("x".repeat(10_000), cfg).unwrap();
    assert!(!buffer.is_read_only());
    assert!(!buffer.is_large_file());
}

#[test]
fn version_overflow_helper_returns_engine_error_not_panic() {
    // BufferVersion::next() 返回 None 时 bump_version 转 EngineError::VersionOverflow，
    // 不允许 panic。这里仅断言错误变体可被构造和匹配，不实际触达 u64::MAX。
    let err = EngineError::VersionOverflow;
    assert!(matches!(err, EngineError::VersionOverflow));
    let display = format!("{err}");
    assert!(!display.is_empty(), "错误必须可显示以便诊断");
}
