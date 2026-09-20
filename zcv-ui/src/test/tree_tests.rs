use super::*;

/// 测试行模型：key 唯一；selectable=false 模拟不可选行（如分组头）。
#[derive(Clone)]
struct TestRow {
    key: usize,
    selectable: bool,
    is_dir: bool,
    depth: usize,
    expanded: bool,
}

impl TreeRow for TestRow {
    fn is_dir(&self) -> bool {
        self.is_dir
    }
    fn depth(&self) -> usize {
        self.depth
    }
    fn expanded(&self) -> bool {
        self.expanded
    }
}

fn row(key: usize, selectable: bool) -> TestRow {
    TestRow {
        key,
        selectable,
        is_dir: false,
        depth: 0,
        expanded: false,
    }
}

/// 构造 [Header0, Entry1, Header2, Entry3, Entry4] 形状的行集。
fn sample_rows() -> Vec<TestRow> {
    vec![
        row(0, false),
        row(1, true),
        row(2, false),
        row(3, true),
        row(4, true),
    ]
}

fn test_state(rows: Vec<TestRow>) -> TreeState<usize, TestRow> {
    let mut state = TreeState::new(|r: &TestRow| r.selectable.then_some(r.key));
    state.replace_rows(rows);
    state
}

#[test]
fn select_up_without_selection_moves_to_last_selectable_row() {
    let mut state = test_state(sample_rows());
    state.select_up();
    assert_eq!(state.selected, Some(4));
}

#[test]
fn select_up_skips_unselectable_rows() {
    let mut state = test_state(sample_rows());
    state.select(3);
    state.select_up();
    assert_eq!(state.selected, Some(1));
}

#[test]
fn select_down_without_selection_moves_to_first_selectable_row() {
    let mut state = test_state(sample_rows());
    state.select_down();
    assert_eq!(state.selected, Some(1));
}

#[test]
fn select_down_skips_unselectable_rows() {
    let mut state = test_state(sample_rows());
    state.select(1);
    state.select_down();
    assert_eq!(state.selected, Some(3));
}

#[test]
fn select_down_at_last_row_stays_put() {
    let mut state = test_state(sample_rows());
    state.select(4);
    state.select_down();
    assert_eq!(state.selected, Some(4));
}

#[test]
fn replace_rows_moves_selection_to_adjacent_row_when_row_disappears() {
    let mut state = test_state(sample_rows());
    state.select(3);
    state.replace_rows(vec![row(0, false), row(1, true)]);
    assert_eq!(state.selected, Some(1));
    state.replace_rows(sample_rows());
    assert_eq!(state.selected, Some(1));
}

#[test]
fn replace_rows_prefers_surviving_row_below_when_rows_reorder() {
    let mut state = test_state(vec![row(1, true), row(2, true), row(3, true)]);
    state.select(2);
    // 模拟选中目录移动到上方分组后，原选中项下面的行仍然存在。
    state.replace_rows(vec![row(4, true), row(3, true), row(1, true)]);
    assert_eq!(state.selected, Some(3));
}

#[test]
fn replace_rows_skips_disappeared_directory_children() {
    let mut state = test_state(vec![
        row(1, true),
        row(2, true),
        row(20, true),
        row(3, true),
    ]);
    state.select(2);
    // 目录及其子文件移动到上方分组后，应继续选择原目录子树之后的兄弟行。
    state.replace_rows(vec![row(10, true), row(200, true), row(3, true)]);
    assert_eq!(state.selected, Some(3));
}

#[test]
fn replace_rows_keeps_selection_when_row_survives() {
    let mut state = test_state(sample_rows());
    state.select(3);
    state.replace_rows(vec![row(2, false), row(3, true)]);
    assert_eq!(state.selected, Some(3));
}

#[test]
fn ensure_selected_picks_first_selectable_row() {
    let mut state = test_state(sample_rows());
    state.ensure_selected();
    assert_eq!(state.selected, Some(1));
}

#[test]
fn collapse_expanded_directory_returns_rebuild() {
    let mut state = TreeState::new(|r: &TestRow| r.selectable.then_some(r.key));
    state.replace_rows(vec![TestRow {
        key: 0,
        selectable: true,
        is_dir: true,
        depth: 0,
        expanded: true,
    }]);
    state.select(0);
    state.expanded.insert(0);
    assert!(state.collapse_selection());
    assert!(!state.expanded.contains(&0));
}

#[test]
fn collapse_leaf_moves_selection_to_ancestor_directory() {
    let mut state = TreeState::new(|r: &TestRow| r.selectable.then_some(r.key));
    state.replace_rows(vec![
        TestRow {
            key: 0,
            selectable: true,
            is_dir: true,
            depth: 0,
            expanded: true,
        },
        TestRow {
            key: 1,
            selectable: true,
            is_dir: false,
            depth: 1,
            expanded: false,
        },
    ]);
    state.select(1);
    assert!(!state.collapse_selection());
    assert_eq!(state.selected, Some(0));
}

#[test]
fn expand_folded_directory_returns_rebuild() {
    let mut state = TreeState::new(|r: &TestRow| r.selectable.then_some(r.key));
    state.replace_rows(vec![TestRow {
        key: 0,
        selectable: true,
        is_dir: true,
        depth: 0,
        expanded: false,
    }]);
    state.select(0);
    assert!(state.expand_selection());
    assert!(state.expanded.contains(&0));
}

#[test]
fn expand_leaf_moves_selection_down() {
    let mut state = test_state(sample_rows());
    state.select(1);
    assert!(!state.expand_selection());
    assert_eq!(state.selected, Some(3));
}

#[test]
fn toggle_expand_flips_marker() {
    let mut state = test_state(Vec::new());
    state.toggle_expand(&1);
    assert!(state.expanded.contains(&1));
    state.toggle_expand(&1);
    assert!(!state.expanded.contains(&1));
}

#[test]
fn row_click_action_toggles_directory_on_every_click() {
    // 目录：每次点击都切换（click_count 连续递增，2+ 次点击不能吞）。
    assert_eq!(row_click_action(true, 1), RowClickAction::Toggle);
    assert_eq!(row_click_action(true, 2), RowClickAction::Toggle);
    assert_eq!(row_click_action(true, 3), RowClickAction::Toggle);
    // 文件：单击预览、双击（及更多次连点）激活。
    assert_eq!(row_click_action(false, 1), RowClickAction::Preview);
    assert_eq!(row_click_action(false, 2), RowClickAction::Activate);
    assert_eq!(row_click_action(false, 3), RowClickAction::Activate);
}

#[test]
fn extend_to_recomputes_range_from_immutable_anchor() {
    let mut state = test_state(vec![row(1, true), row(2, true), row(3, true), row(4, true)]);
    state.select(1);
    state.extend_to(&3);
    assert_eq!(state.selected_set, HashSet::from([1, 2, 3]));
    assert_eq!(state.selected, Some(3));
    // 再扩展到 2：区间按锚点 1 整体重算为 {1, 2}，锚点不动。
    state.extend_to(&2);
    assert_eq!(state.selected_set, HashSet::from([1, 2]));
    assert_eq!(state.selected, Some(2));
    assert_eq!(state.anchor, Some(1));
}

#[test]
fn extend_to_supports_backward_range() {
    let mut state = test_state(vec![row(1, true), row(2, true), row(3, true)]);
    state.select(3);
    state.extend_to(&1);
    assert_eq!(state.selected_set, HashSet::from([1, 2, 3]));
    assert_eq!(state.selected, Some(1));
    assert_eq!(state.anchor, Some(3));
}

#[test]
fn extend_down_then_extend_up_shrinks_range() {
    let mut state = test_state(vec![row(1, true), row(2, true), row(3, true)]);
    state.select(1);
    assert!(state.extend_down());
    assert_eq!(state.selected_set, HashSet::from([1, 2]));
    assert!(state.extend_down());
    assert_eq!(state.selected_set, HashSet::from([1, 2, 3]));
    // 上移一步：区间收缩回 {1, 2}，锚点仍为 1。
    assert!(state.extend_up());
    assert_eq!(state.selected_set, HashSet::from([1, 2]));
    assert_eq!(state.selected, Some(2));
    assert_eq!(state.anchor, Some(1));
}

#[test]
fn extend_up_at_first_row_keeps_cursor_and_range() {
    let mut state = test_state(vec![row(1, true), row(2, true), row(3, true)]);
    state.select(2);
    state.extend_up();
    assert_eq!(state.selected_set, HashSet::from([1, 2]));
    // 游标已在首行：上移返回 false，游标与集合均保持不变。
    assert!(!state.extend_up());
    assert_eq!(state.selected, Some(1));
    assert_eq!(state.selected_set, HashSet::from([1, 2]));
}

#[test]
fn toggle_selection_adds_then_removes_and_keeps_cursor_on_row() {
    let mut state = test_state(vec![row(1, true), row(2, true), row(3, true)]);
    state.select(1);
    state.toggle_selection(&3);
    // 首次打标记把当前游标行（1）一并入集合：多选集与实际选中感知一致，
    // 从首项发起多选拖拽/批量操作才不会退化为单项。
    assert_eq!(state.selected_set, HashSet::from([1, 3]));
    assert_eq!(state.selected, Some(3));
    state.toggle_selection(&3);
    assert_eq!(
        state.selected_set,
        HashSet::from([1]),
        "再次 toggle 应仅移除目标行标记，并入的游标行保留"
    );
    assert_eq!(state.selected, Some(3), "toggle 移除后游标仍在该行");
    assert_eq!(state.anchor, Some(1), "toggle 不动锚点");
}

#[test]
fn select_and_navigation_reset_anchor_and_selection_set() {
    let mut state = test_state(vec![row(1, true), row(2, true), row(3, true)]);
    state.select(1);
    state.extend_to(&3);
    assert!(!state.selected_set.is_empty());
    state.select_up();
    assert!(state.selected_set.is_empty(), "普通导航应清空多选集合");
    assert_eq!(state.anchor, state.selected, "锚点应重置为游标");

    // toggle 后再 select：同样重置为单选态。
    state.toggle_selection(&1);
    state.select(2);
    assert!(state.selected_set.is_empty());
    assert_eq!(state.anchor, Some(2));
}

#[test]
fn replace_rows_prunes_selection_set_and_anchor() {
    let mut state = test_state(vec![row(1, true), row(2, true), row(3, true)]);
    state.select(1);
    state.extend_to(&3);
    state.replace_rows(vec![row(2, true), row(3, true)]);
    assert_eq!(
        state.selected_set,
        HashSet::from([2, 3]),
        "消失键应剔除、幸存键应保留"
    );
    assert_eq!(state.anchor, None, "锚点消失应置空");
    assert_eq!(state.selected, Some(3));
}

#[test]
fn effective_selection_falls_back_to_cursor_when_set_empty() {
    let mut state = test_state(vec![row(1, true), row(2, true), row(3, true)]);
    state.select(2);
    assert_eq!(state.effective_selection(), vec![2]);
    // 集合非空：按可见行序返回集合元素（首次标记已并入游标行 2）。
    state.toggle_selection(&3);
    assert_eq!(state.effective_selection(), vec![2, 3]);
    assert!(state.is_in_selection_set(&3));
    assert!(state.is_in_selection_set(&2));
}
