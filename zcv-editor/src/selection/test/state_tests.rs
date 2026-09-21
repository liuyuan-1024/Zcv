//! 选择历史的有界化与记录清理。

use super::*;
use zcv_text::{Affinity, Buffer, BufferConfig};

fn anchor_selections() -> SelectionSet<MultiBufferAnchor> {
    let snapshot = MultiBufferSnapshot::from(
        Buffer::from_text("hello".to_owned(), BufferConfig::default())
            .expect("测试 Buffer 应能创建")
            .snapshot(),
    );
    let anchor = snapshot.anchor_at(MultiBufferOffset::new(0), Affinity::Before);
    SelectionSet::caret(anchor)
}

#[test]
fn selection_history_drops_oldest_transactions_past_the_limit() {
    let mut history = SelectionHistory::default();
    let total = MAX_SELECTION_HISTORY_ENTRIES + 8;
    for id in 0..total as u64 {
        history.insert_transaction(TransactionId::new(id), anchor_selections());
    }
    assert_eq!(
        history.selections_by_transaction.len(),
        MAX_SELECTION_HISTORY_ENTRIES,
        "选择历史必须按上限有界"
    );
    assert!(
        history.transaction(TransactionId::new(0)).is_none(),
        "超出上限时最老事务必须被丢弃"
    );
    assert!(
        history
            .transaction(TransactionId::new(total as u64 - 1))
            .is_some(),
        "最新事务必须保留"
    );
}

#[test]
fn remove_transaction_drops_only_its_own_record() {
    let mut history = SelectionHistory::default();
    history.insert_transaction(TransactionId::new(1), anchor_selections());
    history.insert_transaction(TransactionId::new(2), anchor_selections());
    history.remove_transaction(TransactionId::new(1));
    assert!(history.transaction(TransactionId::new(1)).is_none());
    assert!(history.transaction(TransactionId::new(2)).is_some());
}
