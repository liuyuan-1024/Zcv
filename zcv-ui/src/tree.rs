//! 树行渲染辅助函数 —— 缩进、图标、名称、选中框。

use std::collections::HashSet;
use std::path::Path;

use gpui::{App, Pixels, div, prelude::*, px};
use zcv_theme::{FileIcons, color, space, typography};

use crate::SvgIcon;

/// 树行完整渲染：行骨架 + 缩进竖线 + 图标 + 行内容。
pub fn render_row_base(
    depth: usize,
    path: &Path,
    is_dir: bool,
    expanded: bool,
    content: impl IntoElement,
    cx: &App,
) -> gpui::Div {
    row_skeleton(depth)
        .children(guide_lines(depth, cx))
        .child(icon(path, is_dir, expanded))
        .child(label(content))
}

/// 选中框——absolute 覆盖整行，不参与行布局。
pub fn selection_border(cx: &App) -> gpui::Div {
    let m = metrics();
    div()
        .absolute()
        .top(Pixels::ZERO)
        .left(Pixels::ZERO)
        .right(Pixels::ZERO)
        .h(m.row_height)
        .rounded_xs()
        .border_1()
        .border_color(color::current(cx).border_focused)
}

/// 树行点击动作（对齐 Zed：目录每次点击都切换展开/折叠；文件单击预览、双击激活）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowClickAction {
    /// 目录：每次点击都执行（toggle 展开/折叠）。
    Toggle,
    /// 文件单击：以临时标签预览。
    Preview,
    /// 文件双击（及更多次连点）：打开并聚焦。
    Activate,
}

/// `click_count` → 行点击动作。
///
/// 连续点击时 `click_count` 递增（1,2,3…），目录必须每击响应，否则快速连点只有第一次生效（表现为"不跟手"）。项目树与变更树共用本决策。
pub fn row_click_action(is_dir: bool, click_count: usize) -> RowClickAction {
    if is_dir {
        RowClickAction::Toggle
    } else if click_count == 1 {
        RowClickAction::Preview
    } else {
        RowClickAction::Activate
    }
}

/// 行点击决策：树未聚焦时首击只聚焦并选中，不执行行动作（再击才生效）。
///
/// 返回 `None` 表示本次点击被消费为"聚焦"，调用方不应执行展开/打开等动作。
/// 项目树与变更树共用本决策，配合各自行上的焦点句柄使用。
pub fn row_mouse_down_action(
    is_dir: bool,
    click_count: usize,
    was_focused: bool,
) -> Option<RowClickAction> {
    was_focused.then(|| row_click_action(is_dir, click_count))
}

// ── 私有辅助函数 ─────────────────────────────────────────────────────

/// 树行骨架：relative + flex-row + items_center + 缩进 + 字型。
fn row_skeleton(depth: usize) -> gpui::Div {
    let m = metrics();
    div()
        .relative()
        .flex()
        .flex_row()
        .items_center()
        .gap(space::S6)
        .w_full()
        .h(m.row_height)
        .pl(m.indent_left(depth))
        .rounded_xs()
}

/// 渲染缩进竖线——每条线直接 absolute 定位在行上。
fn guide_lines(depth: usize, cx: &App) -> Vec<gpui::Div> {
    let m = metrics();
    let line_color = color::current(cx).border_variant;
    let line_w = px(1.0);

    (0..depth)
        .map(|k| {
            let x_center = m.guide_x(k);
            div()
                .absolute()
                .left(x_center - line_w / 2.0)
                .top(Pixels::ZERO)
                .w(line_w)
                .h_full()
                .bg(line_color)
        })
        .collect()
}

/// 根据条目类型和展开/折叠状态返回对应的图标元素。
fn icon(path: &Path, is_dir: bool, expanded: bool) -> impl IntoElement {
    let m = metrics();
    let path = if is_dir {
        FileIcons::get_folder_icon(expanded, path)
    } else {
        FileIcons::get_icon(path)
    };
    div().child(SvgIcon::new(path).size(m.icon_size))
}

/// 条目名称内容，尾部溢出截断。
fn label(content: impl IntoElement) -> gpui::Div {
    div().flex_1().overflow_hidden().truncate().child(content)
}

// ── 树导航状态原语 ──────────────────────────────────────────────────

/// 树行契约：折叠/展开与祖先导航所需的行模型最小信息。
pub trait TreeRow {
    fn is_dir(&self) -> bool;
    fn depth(&self) -> usize;
    fn expanded(&self) -> bool;
}

/// 树导航状态原语：可见行缓存 + 展开集合 + 选中键。
///
/// `key_of` 决定行的身份键；返回 None 的行不可选中（如分组头）。
/// 选中分两层：`selected` 是游标（始终至多一条），`selected_set` 是多选标记集（空 = 纯单选模式）；
/// `anchor` 是 shift 区间扩展的起点，普通导航/点击会重置。
/// 只依赖标准库，不触碰数据源与渲染。
pub struct TreeState<K, Row> {
    pub expanded: HashSet<K>,
    pub selected: Option<K>,
    /// 多选标记集（空 = 纯单选模式）。
    pub selected_set: HashSet<K>,
    /// shift 区间扩展锚点；行消失时置空。
    pub anchor: Option<K>,
    pub rows: Vec<Row>,
    key_of: fn(&Row) -> Option<K>,
}

impl<K: Eq + std::hash::Hash + Clone, Row: TreeRow> TreeState<K, Row> {
    pub fn new(key_of: fn(&Row) -> Option<K>) -> Self {
        Self {
            expanded: HashSet::new(),
            selected: None,
            selected_set: HashSet::new(),
            anchor: None,
            rows: Vec::new(),
            key_of,
        }
    }

    pub fn rows(&self) -> &[Row] {
        &self.rows
    }

    /// 替换可见行；选中行消失时清空选中，多选集与锚点同步剪枝（幸存键保留）。
    pub fn replace_rows(&mut self, rows: Vec<Row>) {
        self.rows = rows;
        let key_of = self.key_of;
        // 先把当前行键收集为集合：三处剪枝从 O(行数 × 选中集) 降为集合查找。
        let alive_keys: HashSet<K> = self.rows.iter().filter_map(key_of).collect();
        if self
            .selected
            .as_ref()
            .is_some_and(|selected| !alive_keys.contains(selected))
        {
            self.selected = None;
        }
        self.selected_set.retain(|key| alive_keys.contains(key));
        if self
            .anchor
            .as_ref()
            .is_some_and(|anchor| !alive_keys.contains(anchor))
        {
            self.anchor = None;
        }
    }

    /// 无选中时选中第一个可选行。
    pub fn ensure_selected(&mut self) {
        if self.selected.is_some() {
            return;
        }
        if let Some(key) = self.rows.iter().find_map(|r| (self.key_of)(r)) {
            self.selected = Some(key);
        }
    }

    /// 直接设置选中键（鼠标点击等交互入口）；普通选中重置为单选态。
    pub fn select(&mut self, key: K) {
        self.selected = Some(key.clone());
        self.anchor = Some(key);
        self.selected_set.clear();
    }

    /// 当前选中行在可见行中的位置。
    pub fn selected_idx(&self) -> Option<usize> {
        let selected = self.selected.clone()?;
        self.rows
            .iter()
            .position(|r| (self.key_of)(r).as_ref() == Some(&selected))
    }

    /// 上移选中；无选中时选中最后一个可选行；普通导航重置为单选态。
    pub fn select_up(&mut self) {
        if let Some(key) = self.prev_selectable_key() {
            self.selected = Some(key);
        }
        self.anchor = self.selected.clone();
        self.selected_set.clear();
    }

    /// 下移选中；无选中时选中第一个可选行；普通导航重置为单选态。
    pub fn select_down(&mut self) {
        if let Some(key) = self.next_selectable_key() {
            self.selected = Some(key);
        }
        self.anchor = self.selected.clone();
        self.selected_set.clear();
    }

    /// 折叠选中行；返回 true 表示行模型需要重建（展开的目录被折叠）。
    pub fn collapse_selection(&mut self) -> bool {
        let Some(idx) = self.selected_idx() else {
            return false;
        };
        let Some(row) = self.rows.get(idx) else {
            return false;
        };
        if row.is_dir() && row.expanded() {
            if let Some(key) = (self.key_of)(row) {
                self.expanded.remove(&key);
                return true;
            }
            return false;
        }
        if row.depth() > 0 {
            // 已折叠/叶子：选中上移到上层祖先行。
            let parent_depth = row.depth() - 1;
            if let Some(parent_idx) = self.rows[..idx].iter().rposition(|r| {
                r.is_dir() && r.depth() == parent_depth && (self.key_of)(r).is_some()
            }) && let Some(key) = (self.key_of)(&self.rows[parent_idx])
            {
                self.selected = Some(key);
            }
            return false;
        }
        false
    }

    /// 展开选中行；返回 true 表示行模型需要重建（折叠的目录被展开）。
    pub fn expand_selection(&mut self) -> bool {
        let Some(idx) = self.selected_idx() else {
            return false;
        };
        let Some(row) = self.rows.get(idx) else {
            return false;
        };
        if row.is_dir() && !row.expanded() {
            if let Some(key) = (self.key_of)(row) {
                self.expanded.insert(key);
                return true;
            }
            return false;
        }
        self.select_down();
        false
    }

    /// 翻转展开标记（鼠标激活目录时用）。
    pub fn toggle_expand(&mut self, key: &K) {
        if self.expanded.contains(key) {
            self.expanded.remove(key);
        } else {
            self.expanded.insert(key.clone());
        }
    }

    /// shift 扩展到目标行：以锚点为起点整体重算区间集合，游标移到目标行，锚点不动。
    ///
    /// 锚点缺失时取当前游标为锚（仍无则取目标行自身）；
    /// 区间为可见行序中两键之间（闭区间，两个方向均支持）。
    pub fn extend_to(&mut self, target: &K) {
        let anchor = self
            .anchor
            .clone()
            .or_else(|| self.selected.clone())
            .unwrap_or_else(|| target.clone());
        self.selected_set = self.range_keys(&anchor, target);
        self.anchor = Some(anchor);
        self.selected = Some(target.clone());
    }

    /// shift+上方向：游标按 select_up 语义上移一步并按锚点重算区间集合；返回游标是否移动。
    pub fn extend_up(&mut self) -> bool {
        let Some(target) = self.prev_selectable_key() else {
            return false;
        };
        self.extend_to(&target);
        true
    }

    /// shift+下方向：游标按 select_down 语义下移一步并按锚点重算区间集合；返回游标是否移动。
    pub fn extend_down(&mut self) -> bool {
        let Some(target) = self.next_selectable_key() else {
            return false;
        };
        self.extend_to(&target);
        true
    }

    /// cmd/ctrl+点击：切换目标行的多选标记（对称差），游标移到目标行，锚点不动。
    ///
    /// 纯单选态首次打标记时把当前游标行一并入集合：普通点击/未聚焦首击选中的首项
    /// 否则留在集合之外，用户从它发起多选拖拽会被误判为集合外行而收拢为单选，
    /// 只移动单项（与主流编辑器「cmd 点击逐项追加选中」的语义对齐）。
    pub fn toggle_selection(&mut self, key: &K) {
        if self.selected_set.is_empty()
            && let Some(cursor) = self.selected.as_ref()
            && cursor != key
        {
            self.selected_set.insert(cursor.clone());
        }
        if !self.selected_set.remove(key) {
            self.selected_set.insert(key.clone());
        }
        self.selected = Some(key.clone());
    }

    /// 有效选中集：集合非空时按可见行序返回集合元素，否则回退为游标单条（可为空）。
    pub fn effective_selection(&self) -> Vec<K> {
        if self.selected_set.is_empty() {
            return self.selected.clone().into_iter().collect();
        }
        self.rows
            .iter()
            .filter_map(|r| (self.key_of)(r))
            .filter(|key| self.selected_set.contains(key))
            .collect()
    }

    /// 行是否命中多选标记集。
    pub fn is_in_selection_set(&self, key: &K) -> bool {
        self.selected_set.contains(key)
    }

    /// 游标的上一个可选行键（无选中时为最后一个可选行，语义对齐 select_up）。
    fn prev_selectable_key(&self) -> Option<K> {
        match self.selected_idx() {
            None => self
                .rows
                .iter()
                .rposition(|r| (self.key_of)(r).is_some())
                .and_then(|idx| (self.key_of)(&self.rows[idx])),
            Some(idx) => self.rows[..idx]
                .iter()
                .rposition(|r| (self.key_of)(r).is_some())
                .and_then(|prev| (self.key_of)(&self.rows[prev])),
        }
    }

    /// 游标的下一个可选行键（无选中时为第一个可选行，语义对齐 select_down）。
    fn next_selectable_key(&self) -> Option<K> {
        match self.selected_idx() {
            None => self
                .rows
                .iter()
                .position(|r| (self.key_of)(r).is_some())
                .and_then(|idx| (self.key_of)(&self.rows[idx])),
            Some(idx) => self.rows[idx + 1..]
                .iter()
                .position(|r| (self.key_of)(r).is_some())
                .map(|offset| idx + 1 + offset)
                .and_then(|next| (self.key_of)(&self.rows[next])),
        }
    }

    /// 可见行序中 `from` 与 `to` 之间（闭区间）的可选行键集合；
    /// 任一端不在可见行中时为空集。
    fn range_keys(&self, from: &K, to: &K) -> HashSet<K> {
        let index_of = |key: &K| {
            self.rows
                .iter()
                .position(|r| (self.key_of)(r).as_ref() == Some(key))
        };
        let (Some(start), Some(end)) = (index_of(from), index_of(to)) else {
            return HashSet::new();
        };
        let (lo, hi) = if start <= end {
            (start, end)
        } else {
            (end, start)
        };
        self.rows[lo..=hi]
            .iter()
            .filter_map(|r| (self.key_of)(r))
            .collect()
    }
}

// ── 内部类型 ─────────────────────────────────────────────────────────

/// 树行布局度量。
struct TreeMetrics {
    row_height: gpui::Pixels,
    indent: gpui::Pixels,
    padding: gpui::Pixels,
    icon_size: gpui::Pixels,
}

fn metrics() -> TreeMetrics {
    TreeMetrics {
        row_height: typography::ui_line(),
        indent: typography::ui(),
        padding: space::S6,
        icon_size: typography::ui(),
    }
}

impl TreeMetrics {
    fn indent_left(&self, depth: usize) -> gpui::Pixels {
        self.indent * (depth as f32) + self.padding
    }

    fn guide_x(&self, depth: usize) -> gpui::Pixels {
        self.indent * (depth as f32) + self.icon_size / 2.0 + self.padding
    }
}

#[cfg(test)]
mod tests {
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
    fn replace_rows_clears_selection_when_row_disappears() {
        let mut state = test_state(sample_rows());
        state.select(3);
        state.replace_rows(vec![row(0, false), row(1, true)]);
        assert_eq!(state.selected, None);
        state.replace_rows(sample_rows());
        assert_eq!(state.selected, None);
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
    fn row_mouse_down_action_consumes_first_click_when_not_focused() {
        // 未聚焦：首击只聚焦，任何点击都不执行行动作。
        assert_eq!(
            row_mouse_down_action(false, 1, false),
            None,
            "未聚焦首击不应预览"
        );
        assert_eq!(
            row_mouse_down_action(true, 1, false),
            None,
            "未聚焦首击不应展开"
        );
        // 已聚焦：动作与 click_count 决策一致。
        assert_eq!(
            row_mouse_down_action(false, 1, true),
            Some(RowClickAction::Preview)
        );
        assert_eq!(
            row_mouse_down_action(false, 2, true),
            Some(RowClickAction::Activate)
        );
        assert_eq!(
            row_mouse_down_action(true, 1, true),
            Some(RowClickAction::Toggle)
        );
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
}
