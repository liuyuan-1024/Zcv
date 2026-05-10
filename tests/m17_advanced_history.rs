//! M17A 机器契约：锁定历史图的节点身份、单调序号、撤销后产生本地分支、
//! 分支查询与显式分支切换。

use zom_engine::{
    Buffer, BufferConfig, CharOffset, Edit, EngineError, HistoryNodeId, Transaction,
    TransactionMergePolicy, TransactionMetadata, TransactionSource,
};

fn buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
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
fn empty_buffer_has_no_current_history_node() {
    let buffer = buffer("abc");
    assert!(buffer.current_history_node().is_none());
    assert!(buffer.parent_history_node().is_none());
    assert!(buffer.redo_branches().is_empty());
    assert!(!buffer.can_undo());
    assert!(!buffer.can_redo());
    let status = buffer.history_status();
    assert_eq!(status.undo_depth, 0);
    assert_eq!(status.redo_depth, 0);
    assert_eq!(status.current_node, None);
}

#[test]
fn each_committed_transaction_creates_a_node_with_monotonic_sequence() {
    let mut buffer = buffer("");
    commit_insert(&mut buffer, 0, "a");
    let first = buffer.current_history_node().unwrap();

    commit_insert(&mut buffer, 1, "b");
    let second = buffer.current_history_node().unwrap();

    commit_insert(&mut buffer, 2, "c");
    let third = buffer.current_history_node().unwrap();

    let v_first = buffer.history_node(first).unwrap();
    let v_second = buffer.history_node(second).unwrap();
    let v_third = buffer.history_node(third).unwrap();

    assert!(v_first.sequence_number < v_second.sequence_number);
    assert!(v_second.sequence_number < v_third.sequence_number);
    assert_eq!(v_first.parent, None);
    assert_eq!(v_second.parent, Some(first));
    assert_eq!(v_third.parent, Some(second));
    assert_eq!(v_third.children, Vec::<HistoryNodeId>::new());

    let status = buffer.history_status();
    assert_eq!(status.undo_depth, 3);
    assert_eq!(status.redo_depth, 0);
    assert_eq!(status.current_node, Some(third));
}

#[test]
fn undo_moves_current_to_parent_without_dropping_node() {
    let mut buffer = buffer("");
    commit_insert(&mut buffer, 0, "a");
    let leaf = buffer.current_history_node().unwrap();

    buffer.undo().unwrap().expect("有节点可 undo");
    assert!(buffer.current_history_node().is_none());
    // 节点本身仍在历史图里，可作为 redo 目标。
    assert!(buffer.history_node(leaf).is_some());
    assert_eq!(buffer.redo_branches(), vec![leaf]);

    buffer.redo().unwrap().expect("有节点可 redo");
    assert_eq!(buffer.current_history_node(), Some(leaf));
}

#[test]
fn new_commit_after_undo_creates_a_branch_instead_of_clearing_redo() {
    let mut buffer = buffer("");
    commit_insert(&mut buffer, 0, "a");
    let original = buffer.current_history_node().unwrap();

    buffer.undo().unwrap().expect("undo back to root");
    assert!(buffer.current_history_node().is_none());

    commit_insert(&mut buffer, 0, "Z");
    let branch = buffer.current_history_node().unwrap();
    assert_ne!(branch, original);

    // 原始节点和新分支节点都是根（parent = None）。
    let original_view = buffer.history_node(original).unwrap();
    let branch_view = buffer.history_node(branch).unwrap();
    assert_eq!(original_view.parent, None);
    assert_eq!(branch_view.parent, None);

    // 当前位于 branch；undo 到根后两个分支都可作为 redo 目标。
    buffer.undo().unwrap();
    let mut branches = buffer.redo_branches();
    branches.sort_by_key(|id| id.get());
    assert_eq!(branches, vec![original, branch]);
}

#[test]
fn redo_default_chooses_most_recent_branch() {
    let mut buffer = buffer("");
    commit_insert(&mut buffer, 0, "a");
    let original = buffer.current_history_node().unwrap();
    buffer.undo().unwrap();
    commit_insert(&mut buffer, 0, "Z");
    let recent_branch = buffer.current_history_node().unwrap();
    buffer.undo().unwrap();

    // 默认 redo 走最近创建的分支。
    buffer.redo().unwrap().expect("redo into the latest branch");
    assert_eq!(buffer.current_history_node(), Some(recent_branch));
    assert_eq!(buffer.snapshot().text(), "Z");
    assert_ne!(recent_branch, original);
}

#[test]
fn redo_to_branch_switches_into_a_specific_branch() {
    let mut buffer = buffer("");
    commit_insert(&mut buffer, 0, "a");
    let alpha = buffer.current_history_node().unwrap();

    buffer.undo().unwrap();
    commit_insert(&mut buffer, 0, "Z");
    let beta = buffer.current_history_node().unwrap();

    buffer.undo().unwrap();
    // 显式选择较早创建的 alpha 分支。
    buffer.redo_to_branch(alpha).unwrap();
    assert_eq!(buffer.current_history_node(), Some(alpha));
    assert_eq!(buffer.snapshot().text(), "a");

    // 切回 beta：先 undo 再 redo_to_branch(beta)。
    buffer.undo().unwrap();
    buffer.redo_to_branch(beta).unwrap();
    assert_eq!(buffer.current_history_node(), Some(beta));
    assert_eq!(buffer.snapshot().text(), "Z");
}

#[test]
fn redo_to_branch_rejects_non_child_node() {
    let mut buffer = buffer("");
    commit_insert(&mut buffer, 0, "a");
    let leaf = buffer.current_history_node().unwrap();
    commit_insert(&mut buffer, 1, "b");
    let after = buffer.current_history_node().unwrap();

    // current = after；leaf 是 after 的父节点，不是子节点，不能作为 redo 目标。
    let outcome = buffer.redo_to_branch(leaf);
    assert!(matches!(outcome, Err(EngineError::InvalidHistoryBranch(_))));
    assert_eq!(buffer.current_history_node(), Some(after));
}

#[test]
fn merge_with_previous_extends_current_node_instead_of_creating_branch() {
    let mut buffer = buffer("");
    commit_insert(&mut buffer, 0, "a");
    let first = buffer.current_history_node().unwrap();

    let merge_metadata = TransactionMetadata::new(TransactionSource::Programmatic)
        .with_merge_policy(TransactionMergePolicy::MergeWithPrevious);
    commit_insert_with(&mut buffer, 1, "b", merge_metadata);

    // 同一个节点，没有产生新节点。
    assert_eq!(buffer.current_history_node(), Some(first));
    assert_eq!(buffer.history_status().undo_depth, 1);

    // Undo 一次直接回到根。
    buffer.undo().unwrap();
    assert!(buffer.current_history_node().is_none());
    assert_eq!(buffer.snapshot().text(), "");

    // Redo 一次又回到完整状态。
    buffer.redo().unwrap();
    assert_eq!(buffer.snapshot().text(), "ab");
}

#[test]
fn redo_branches_listed_in_recent_first_order() {
    let mut buffer = buffer("");
    commit_insert(&mut buffer, 0, "a");
    let alpha = buffer.current_history_node().unwrap();

    buffer.undo().unwrap();
    commit_insert(&mut buffer, 0, "B");
    let beta = buffer.current_history_node().unwrap();

    buffer.undo().unwrap();
    commit_insert(&mut buffer, 0, "C");
    let gamma = buffer.current_history_node().unwrap();

    buffer.undo().unwrap();
    let branches = buffer.redo_branches();
    assert_eq!(branches.first(), Some(&gamma), "最近创建的分支排在最前");
    assert!(branches.contains(&alpha));
    assert!(branches.contains(&beta));
    assert!(branches.contains(&gamma));
    assert_eq!(branches.len(), 3);
}

#[test]
fn history_node_view_carries_selection_and_description() {
    let mut buffer = buffer("");
    let metadata =
        TransactionMetadata::new(TransactionSource::Programmatic).with_description("first commit");
    commit_insert_with(&mut buffer, 0, "a", metadata);

    let id = buffer.current_history_node().unwrap();
    let view = buffer.history_node(id).unwrap();

    assert_eq!(view.id, id);
    assert_eq!(view.parent, None);
    assert!(view.children.is_empty());
    assert_eq!(view.description.as_deref(), Some("first commit"));
    // selection 至少存在；具体内容取决于 set_selection 默认行为。
    assert_eq!(view.before_selection.as_slice().len(), 1);
    assert_eq!(view.after_selection.as_slice().len(), 1);
}

#[test]
fn undo_leaves_parent_branches_intact_for_subsequent_switch() {
    let mut buffer = buffer("");
    commit_insert(&mut buffer, 0, "a");
    let alpha = buffer.current_history_node().unwrap();
    buffer.undo().unwrap();
    commit_insert(&mut buffer, 0, "B");
    let beta = buffer.current_history_node().unwrap();
    buffer.undo().unwrap();

    // 即使我们在分支之间切换很多次，每个分支节点仍可访问。
    for _ in 0..3 {
        buffer.redo_to_branch(alpha).unwrap();
        assert_eq!(buffer.snapshot().text(), "a");
        buffer.undo().unwrap();
        buffer.redo_to_branch(beta).unwrap();
        assert_eq!(buffer.snapshot().text(), "B");
        buffer.undo().unwrap();
    }
    assert_eq!(buffer.redo_branches().len(), 2);
    assert!(buffer.history_node(alpha).is_some());
    assert!(buffer.history_node(beta).is_some());
}

#[test]
fn unrecorded_transaction_drops_redo_branches_under_current() {
    let mut buffer = buffer("");
    commit_insert(&mut buffer, 0, "a");
    let leaf = buffer.current_history_node().unwrap();
    buffer.undo().unwrap();
    assert_eq!(buffer.redo_branches(), vec![leaf]);

    // record_history=false 提交：不入历史，但作废当前节点之下的 redo 分支。
    let metadata = TransactionMetadata::new(TransactionSource::Programmatic).without_history();
    commit_insert_with(&mut buffer, 0, "Z", metadata);

    assert!(buffer.current_history_node().is_none());
    assert!(buffer.redo_branches().is_empty());
    assert!(buffer.history_node(leaf).is_none());
}
