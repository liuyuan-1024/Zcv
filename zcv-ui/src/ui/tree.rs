//! 树行渲染辅助函数 —— 缩进、图标、名称、选中框、git 状态着色。

use std::path::Path;

use gpui::{App, Pixels, div, prelude::*, px};

use crate::ui::SvgIcon;
use zcv_git::{FileStatus, StatusCode};
use zcv_theme::{FileIcons, color, space, typography};

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

/// git 状态 → 文本颜色（对齐 Zed `entry_git_aware_label_color` 的优先级）。
///
/// conflict > deleted > modified > added/untracked > ignored（渲染层淡显）。
pub fn git_status_color(status: FileStatus, cx: &App) -> Option<gpui::Rgba> {
    let colors = color::current(cx);
    match status {
        FileStatus::Unmerged => Some(colors.status_conflict),
        FileStatus::Untracked => Some(colors.status_created),
        FileStatus::Ignored => None,
        FileStatus::Tracked {
            index_status,
            worktree_status,
        } => {
            let deleted = matches!(index_status, StatusCode::Deleted)
                || matches!(worktree_status, StatusCode::Deleted);
            let modified = matches!(index_status, StatusCode::Modified | StatusCode::TypeChanged)
                || matches!(
                    worktree_status,
                    StatusCode::Modified | StatusCode::TypeChanged
                );
            let added = matches!(index_status, StatusCode::Added)
                || matches!(worktree_status, StatusCode::Added);
            if deleted {
                Some(colors.status_deleted)
            } else if modified {
                Some(colors.status_modified)
            } else if added {
                Some(colors.status_created)
            } else {
                None
            }
        }
    }
}

// ── 私有辅助函数 ─────────────────────────────────────────────────────

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

#[cfg(test)]
mod tests {
    use super::*;

    #[gpui::test]
    fn git_status_color_follows_zed_priority(cx: &mut gpui::TestAppContext) {
        cx.read(|cx| {
            // palette 未初始化时默认 one_dark，语义色可直接取。
            let colors = color::current(cx);
            let color = |status| git_status_color(status, cx);
            // 特殊态。
            assert_eq!(color(FileStatus::Untracked), Some(colors.status_created));
            assert_eq!(color(FileStatus::Unmerged), Some(colors.status_conflict));
            assert_eq!(color(FileStatus::Ignored), None);
            // 已跟踪：deleted > modified > added 优先级。
            let tracked = |index, worktree| FileStatus::Tracked {
                index_status: index,
                worktree_status: worktree,
            };
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::Modified)),
                Some(colors.status_modified)
            );
            assert_eq!(
                color(tracked(StatusCode::Modified, StatusCode::Unmodified)),
                Some(colors.status_modified)
            );
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::TypeChanged)),
                Some(colors.status_modified)
            );
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::Added)),
                Some(colors.status_created)
            );
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::Deleted)),
                Some(colors.status_deleted)
            );
            // 部分暂存：modified 优先于 added。
            assert_eq!(
                color(tracked(StatusCode::Added, StatusCode::Modified)),
                Some(colors.status_modified)
            );
            assert_eq!(
                color(tracked(StatusCode::Unmodified, StatusCode::Unmodified)),
                None
            );
        });
    }
}
