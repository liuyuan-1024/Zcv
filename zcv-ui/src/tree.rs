//! 树行渲染辅助函数 —— 通用行框架、树节点行、选中框与树导航状态原语。

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use gpui::{AnyElement, App, ElementId, Pixels, Window, div, prelude::*};
use zcv_theme::{FileIcons, color, space, typography};

use crate::SvgIcon;

/// 通用树列表行框架。
///
/// 只负责所有行共有的几何：行高、内容槽、行尾槽位和两侧内边距。
/// 它不假设当前行是文件、目录、分组标题还是提示文本。
pub struct TreeRowFrame {
    left_padding: Pixels,
    content: Vec<AnyElement>,
    leading: Vec<AnyElement>,
    decorations: Vec<AnyElement>,
    trailing: Vec<AnyElement>,
}

impl TreeRowFrame {
    /// 按树深度设置缩进，并添加与项目树一致的层级引导线。
    ///
    /// 深度只描述几何，不决定行首图标或点击行为，因此文件树、大纲等不同树形视图可以共享。
    pub fn tree_depth(mut self, depth: usize, window: &Window, cx: &App) -> Self {
        self.left_padding = metrics(window.rem_size(), cx).indent_left(depth);
        self.decorations.extend(
            guide_lines(depth, window, cx)
                .into_iter()
                .map(IntoElement::into_any_element),
        );
        self
    }

    /// 添加内容槽；多个内容槽由框架统一排列。
    pub fn content(mut self, element: impl IntoElement) -> Self {
        self.content.push(element.into_any_element());
        self
    }

    /// 添加行首插槽；树节点、分组标题等行可在这里放置各自的前缀控件。
    pub fn leading(mut self, element: impl IntoElement) -> Self {
        self.leading.push(element.into_any_element());
        self
    }

    /// 添加一个行尾插槽；多个插槽由框架统一按间距排列。
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing.push(element.into_any_element());
        self
    }

    /// 渲染可交互树行的公共外壳。
    ///
    /// 选择框、拖拽和具体点击动作仍由树形消费方叠加；
    /// 这里仅统一行身份、指针和悬停背景。
    pub fn interactive(
        self,
        id: impl Into<ElementId>,
        window: &Window,
        cx: &App,
    ) -> gpui::Stateful<gpui::Div> {
        self.render(window, cx)
            .id(id)
            .cursor_pointer()
            .hover(|style| style.bg(color::current(cx).element_hover))
    }

    pub fn render(self, window: &Window, cx: &App) -> gpui::Div {
        let mut row = blank_row(window, cx)
            .relative()
            .pl(self.left_padding)
            .pr(metrics(window.rem_size(), cx).padding)
            .flex_row()
            .gap(space::S6)
            .children(self.decorations);

        if !self.leading.is_empty() {
            row = row.child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(space::S6)
                    .children(self.leading),
            );
        }

        row = row.child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .flex()
                .items_center()
                .gap(space::S6)
                .children(self.content),
        );

        if !self.trailing.is_empty() {
            row = row.child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(space::S6)
                    .children(self.trailing),
            );
        }

        row
    }
}

/// 树行主文本：占用剩余宽度，并在空间不足时以省略号截断。
///
/// 截断必须设置在直接承载文本的元素上；
/// 仅限制外层树行的溢出不会为内部文本生成省略号。
pub fn tree_row_label(element: impl IntoElement) -> gpui::Div {
    div().flex_1().min_w_0().truncate().child(element)
}

impl Default for TreeRowFrame {
    fn default() -> Self {
        Self {
            left_padding: space::S6,
            content: Vec::new(),
            leading: Vec::new(),
            decorations: Vec::new(),
            trailing: Vec::new(),
        }
    }
}

/// 树节点行：在通用行框架中组合层级引导线、文件图标和节点内容。
pub struct TreeNodeRow {
    depth: usize,
    path: PathBuf,
    is_dir: bool,
    expanded: bool,
    content: AnyElement,
    trailing: Vec<AnyElement>,
}

impl TreeNodeRow {
    pub fn new(
        depth: usize,
        path: &Path,
        is_dir: bool,
        expanded: bool,
        content: impl IntoElement,
    ) -> Self {
        Self {
            depth,
            path: path.to_path_buf(),
            is_dir,
            expanded,
            content: content.into_any_element(),
            trailing: Vec::new(),
        }
    }

    /// 添加一个行尾插槽；多个插槽由通用行框架统一按间距排列。
    pub fn trailing(mut self, element: impl IntoElement) -> Self {
        self.trailing.push(element.into_any_element());
        self
    }

    /// 将文件树节点组装为通用树行框架。
    pub fn frame(self, window: &Window, cx: &App) -> TreeRowFrame {
        let mut frame = TreeRowFrame::default()
            .tree_depth(self.depth, window, cx)
            .content(self.content)
            .leading(icon(&self.path, self.is_dir, self.expanded, window, cx));
        for trailing in self.trailing {
            frame = frame.trailing(trailing);
        }
        frame
    }
}

/// 树行行高（= 空白行基座高度）：滚动计算、命中测试坐标等需要行高数值的场景读取。
pub fn tree_row_height(window: &Window, cx: &App) -> gpui::Pixels {
    metrics(window.rem_size(), cx).row_height
}

/// 选中框——absolute 覆盖整行，不参与行布局。
pub fn selection_border(window: &Window, cx: &App) -> gpui::Div {
    let m = metrics(window.rem_size(), cx);
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

/// 树行点击动作（目录每次点击都切换展开/折叠；文件单击预览、双击激活）。
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

// ── 私有辅助函数 ─────────────────────────────────────────────────────

/// 空白树行基座——树行几何（高度/主轴对齐）的唯一出处。
///
/// 行高只定义在 [`metrics`] 并经本函数落地：
/// 文件条目行与文本行都构建在它之上，uniform_list 按对第 0 行的实测决定槽高、要求列表内所有行等高，因此同一棵树内的行必须全部出自本基座。
/// 派生行不得覆写高度，也不得叠加垂直 padding（会撑出行盒，超出等宽槽）。
fn blank_row(window: &Window, cx: &App) -> gpui::Div {
    div()
        .w_full()
        .flex()
        .items_center()
        .h(metrics(window.rem_size(), cx).row_height)
}

/// 渲染缩进竖线——每条线直接 absolute 定位在行上。
fn guide_lines(depth: usize, window: &Window, cx: &App) -> Vec<gpui::Div> {
    let m = metrics(window.rem_size(), cx);
    let line_color = color::current(cx).border_variant;
    let line_w = space::S1;

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
fn icon(path: &Path, is_dir: bool, expanded: bool, window: &Window, cx: &App) -> impl IntoElement {
    let m = metrics(window.rem_size(), cx);
    let path = if is_dir {
        FileIcons::get_folder_icon(expanded, path)
    } else {
        FileIcons::get_icon(path)
    };
    div().child(SvgIcon::new(path).size(m.icon_size))
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

    /// 替换可见行；选中行消失时迁移到相邻可选行，多选集与锚点同步剪枝。
    pub fn replace_rows(&mut self, rows: Vec<Row>) {
        let previous_selected_idx = self.selected.as_ref().and_then(|selected| {
            self.rows
                .iter()
                .position(|row| (self.key_of)(row).as_ref() == Some(selected))
        });
        let next_selected = previous_selected_idx.map(|index| {
            self.rows
                .iter()
                .skip(index + 1)
                .filter_map(|row| (self.key_of)(row))
                .collect::<Vec<_>>()
        });
        let previous_selected = previous_selected_idx.map(|index| {
            self.rows
                .iter()
                .take(index)
                .rev()
                .filter_map(|row| (self.key_of)(row))
                .collect::<Vec<_>>()
        });
        self.rows = rows;
        let key_of = self.key_of;
        // 先把当前行键收集为集合：三处剪枝从 O(行数 × 选中集) 降为集合查找。
        let alive_keys: HashSet<K> = self.rows.iter().filter_map(key_of).collect();
        if self
            .selected
            .as_ref()
            .is_some_and(|selected| !alive_keys.contains(selected))
        {
            self.selected = next_selected
                .into_iter()
                .flatten()
                .find(|key| alive_keys.contains(key))
                .or_else(|| {
                    previous_selected
                        .into_iter()
                        .flatten()
                        .find(|key| alive_keys.contains(key))
                });
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
    /// 纯单选态首次打标记时，会把普通点击选中的当前游标行一并加入集合。
    ///
    /// 否则当前游标会落在集合之外，后续从该行拖拽会收拢为单选，只移动一项。
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

    /// 游标的上一个可选行键（无选中时为最后一个可选行，对应 select_up）。
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

    /// 游标的下一个可选行键（无选中时为第一个可选行，对应 select_down）。
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

fn metrics(ui_size: Pixels, cx: &App) -> TreeMetrics {
    TreeMetrics {
        row_height: typography::ui_line_at(ui_size, cx) + space::S6,
        indent: ui_size,
        padding: space::S6,
        icon_size: ui_size,
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
#[path = "test/tree_tests.rs"]
mod tests;
