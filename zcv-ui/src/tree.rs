//! 树行渲染辅助函数。

use std::path::{Path, PathBuf};

use gpui::{AnyElement, App, DefiniteLength, Div, ElementId, Pixels, Window, div, prelude::*};
use zcv_theme::{FileIcons, color, fixed, scale, typography};

use crate::SvgIcon;

/// 通用树列表行框架。
///
/// 只负责所有行共有的几何：行高、层级缩进、行首列、内容槽与行尾槽。
/// 它不假设当前行是文件、目录、分组标题还是提示文本。
pub struct TreeRowFrame {
    left_padding: DefiniteLength,
    content: Vec<AnyElement>,
    leading: Option<AnyElement>,
    tree_gutter: bool,
    decorations: Vec<AnyElement>,
    trailing: Vec<AnyElement>,
}

impl TreeRowFrame {
    /// 按树深度设置缩进、行首列，并添加层级引导线。
    ///
    /// 树形行始终保留一个图标宽的行首列，同层级标题不因折叠控件有无而移动。
    /// 深度只描述几何，不决定行首图标或点击行为。
    pub fn tree_depth(mut self, depth: usize, window: &Window, cx: &App) -> Self {
        self.left_padding = metrics(window, cx).indent_left(depth).into();
        self.tree_gutter = true;
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
        self.leading = Some(element.into_any_element());
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
    ) -> gpui::Stateful<Div> {
        self.render(window, cx)
            .id(id)
            .cursor_pointer()
            .hover(|style| style.bg(color::current(cx).element_hover))
    }

    pub fn render(self, window: &Window, cx: &App) -> Div {
        let mut row = blank_row(window, cx)
            .relative()
            .pl(self.left_padding)
            .pr(metrics(window, cx).padding)
            .flex_row()
            .gap(if self.tree_gutter {
                scale::S2
            } else {
                scale::S6
            })
            .children(self.decorations);

        if self.tree_gutter {
            row = row.child(
                div()
                    .w(metrics(window, cx).icon_size)
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when_some(self.leading, |slot, leading| slot.child(leading)),
            );
        } else if let Some(leading) = self.leading {
            row = row.child(div().flex_shrink_0().flex().items_center().child(leading));
        }

        row = row.child(
            div()
                .flex_1()
                .min_w_0()
                .overflow_hidden()
                .whitespace_nowrap()
                .flex()
                .items_center()
                .gap(scale::S6)
                .children(self.content),
        );

        if !self.trailing.is_empty() {
            row = row.child(
                div()
                    .flex_shrink_0()
                    .flex()
                    .items_center()
                    .gap(scale::S6)
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
pub fn tree_row_label(element: impl IntoElement) -> Div {
    div().flex_1().min_w_0().truncate().child(element)
}

impl Default for TreeRowFrame {
    fn default() -> Self {
        Self {
            left_padding: scale::S6,
            content: Vec::new(),
            leading: None,
            tree_gutter: false,
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

// ── 目录链自动折叠 ──────────────────────────────────────────────────

/// 单子目录链自动折叠所需的节点视图。
///
/// 调用方只描述「这个目录叫什么、它唯一的子目录是谁」；
/// 是否继续折叠由 [`auto_fold_dirs`] 结合展开状态统一决定。
pub trait AutoFoldDir {
    /// 目录的单个路径分量名。
    fn dir_name(&self) -> &str;
    /// 唯一的子目录；没有子项、有多个子项或子项不是目录时返回 `None`。
    fn single_dir_child(&self) -> Option<&Self>;
}

/// 把连续的单子目录合并为一行显示名（如 `a/b/c`）。
///
/// 只有当目录本身处于展开状态时才继续折叠，避免吞掉用户明确折叠的边界；
/// 展开状态由调用方提供，因为不同面板的展开集合所有权不同。
pub fn auto_fold_dirs<N: AutoFoldDir>(node: &N, expanded: impl Fn(&N) -> bool) -> (&N, String) {
    let mut deepest = node;
    let mut name = node.dir_name().to_owned();
    while expanded(deepest) {
        let Some(child) = deepest.single_dir_child() else {
            break;
        };
        name.push('/');
        name.push_str(child.dir_name());
        deepest = child;
    }
    (deepest, name)
}

/// 树行行高（= 空白行基座高度）：滚动计算、命中测试坐标等需要行高数值的场景读取。
pub fn tree_row_height(window: &Window, cx: &App) -> Pixels {
    metrics(window, cx).row_height
}

/// 选中框——absolute 覆盖整行，不参与行布局。
pub fn selection_border(window: &Window, cx: &App) -> Div {
    let m = metrics(window, cx);
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
fn blank_row(window: &Window, cx: &App) -> Div {
    div()
        .w_full()
        .flex()
        .items_center()
        .h(metrics(window, cx).row_height)
}

/// 渲染缩进竖线——每条线直接 absolute 定位在行上。
fn guide_lines(depth: usize, window: &Window, cx: &App) -> Vec<Div> {
    let m = metrics(window, cx);
    let line_color = color::current(cx).border;
    let line_w = fixed::HAIRLINE;

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
    let m = metrics(window, cx);
    let path = if is_dir {
        FileIcons::get_folder_icon(expanded, path)
    } else {
        FileIcons::get_icon(path)
    };
    div().child(SvgIcon::new(path).size(m.icon_size))
}

// ── 内部类型 ─────────────────────────────────────────────────────────

/// 树行布局度量。
struct TreeMetrics {
    row_height: Pixels,
    indent: Pixels,
    padding: Pixels,
    icon_size: Pixels,
}

fn metrics(window: &Window, cx: &App) -> TreeMetrics {
    let ui_size = window.rem_size();
    let padding = scale::to_pixels(scale::S6, window);
    TreeMetrics {
        row_height: typography::ui_line_at(ui_size, cx) + padding,
        indent: ui_size,
        padding,
        icon_size: ui_size,
    }
}

impl TreeMetrics {
    fn indent_left(&self, depth: usize) -> Pixels {
        self.indent * (depth as f32) + self.padding
    }

    fn guide_x(&self, depth: usize) -> Pixels {
        self.indent * (depth as f32) + self.icon_size / 2.0 + self.padding
    }
}

#[cfg(test)]
#[path = "test/tree_tests.rs"]
mod tests;
