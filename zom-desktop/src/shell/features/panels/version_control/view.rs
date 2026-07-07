//! VersionControl 面板的行渲染。
//!
//! 输入：[`VersionControlState`] 已是 DFS 扁平化后的可见行序列，本文件负责"画出来"。
//! 缩进连线与图标复用文件树的渲染模式。

use std::path::PathBuf;
use std::rc::Rc;

use gpui::{Div, MouseButton, Window, div, prelude::*, px, rgba, svg, uniform_list};

use crate::editor::TextEditorSlot;
use crate::git_service::ColorKind;
use crate::shell::shared::Glyph;
use crate::shell::shared::scroll;
use crate::shell::shared::tree::{self};
use crate::theme::{color, space, typography};

use super::{StageStatus, VersionControlRow, VersionControlState};

/// 渲染整个变更文件列表。
pub(super) fn render_list(
    state: &VersionControlState,
    selected: Option<PathBuf>,
    scroll_handle: &scroll::ScrollHandle,
    on_click: impl Fn(PathBuf, &mut Window, &mut gpui::App) + 'static,
    on_checkbox_click: impl Fn(PathBuf, &mut Window, &mut gpui::App) + 'static,
) -> Div {
    let rows = state.rows.clone();
    let on_click = Rc::new(on_click);
    let on_checkbox_click = Rc::new(on_checkbox_click);
    let selected = Rc::new(selected);

    let selected_index = (*selected)
        .as_ref()
        .and_then(|sel| rows.iter().position(|r| &r.path == sel));

    div()
        .relative()
        .size_full()
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::current().gray.s08)
        .child(
            uniform_list("version-control-list", rows.len(), move |range, _, _| {
                range
                    .filter_map(|index| rows.get(index))
                    .map(|row| {
                        let on_click = Rc::clone(&on_click);
                        let on_checkbox_click = Rc::clone(&on_checkbox_click);
                        let path = row.path.clone();
                        render_row(row, move |window, cx| on_click(path.clone(), window, cx), {
                            let path = row.path.clone();
                            move |window, cx| on_checkbox_click(path.clone(), window, cx)
                        })
                        .into_any_element()
                    })
                    .collect()
            })
            .size_full()
            .track_scroll(scroll_handle.inner()),
        )
        .child(scroll::scrollbar(scroll_handle))
        .children(tree::list_selection_overlay(selected_index, scroll_handle))
}

/// 渲染单行。
fn render_row<F, G>(row: &VersionControlRow, on_click: F, on_checkbox_click: G) -> Div
where
    F: Fn(&mut Window, &mut gpui::App) + 'static,
    G: Fn(&mut Window, &mut gpui::App) + 'static,
{
    let mut row_div = tree::render_row_base(row.depth, row.is_dir, row.expanded, &row.name);

    // 行尾渲染暂存复选框。
    row_div = row_div.child(render_checkbox(row.stage_status, on_checkbox_click));

    if let Some(kind) = row.git_color {
        row_div = row_div.text_color(color::git_status(kind));
    }

    row_div = row_div.hover(|style| style.bg(color::current().gray.s04));

    row_div
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            on_click(window, cx);
        })
}

/// 渲染暂存复选框——三态：未暂存、已暂存、部分暂存。
///
/// 点击时调用 `on_click` 并阻止事件冒泡，避免触发行激活（打开文件）。
fn render_checkbox(
    stage_status: StageStatus,
    on_click: impl Fn(&mut Window, &mut gpui::App) + 'static,
) -> impl IntoElement {
    let size = typography::ui_line() * 0.75;
    let rounded = px(3.0);

    let has_staged = stage_status != StageStatus::Unstaged;
    let icon_color = if has_staged {
        color::current().blue.s07
    } else {
        rgba(0)
    };

    div()
        .flex_shrink_0()
        .size(size)
        .rounded(rounded)
        .border_1()
        .border_color(color::current().gray.s06)
        .bg(rgba(0))
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .when(stage_status == StageStatus::Staged, |el| {
            el.child(
                svg()
                    .path("icons/actions/check.svg")
                    .size(size - px(2.0))
                    .text_color(icon_color),
            )
        })
        .when(stage_status == StageStatus::Partial, |el| {
            el.child(
                svg()
                    .path("icons/actions/dash.svg")
                    .size(size - px(2.0))
                    .text_color(icon_color),
            )
        })
        .on_mouse_down(MouseButton::Left, move |_, window, cx| {
            cx.stop_propagation();
            on_click(window, cx);
        })
}

// ── 顶栏 ──

/// 渲染面板顶栏：diff 图标 + 变更统计 + 暂存全部复选框。
pub(super) fn render_top_bar(
    diff_stats: (u32, u32),
    all_stage_status: StageStatus,
    on_toggle_stage_all: impl Fn(&mut Window, &mut gpui::App) + 'static,
) -> Div {
    let (added, deleted) = diff_stats;
    div()
        .flex()
        .flex_row()
        .items_center()
        .justify_between()
        .px(space::s4())
        .h(typography::ui_line())
        .bg(color::current().gray.s02)
        .border_b_1()
        .border_color(color::current().gray.s05)
        .text_size(typography::ui())
        .text_color(color::current().gray.s09)
        .child(
            div()
                .flex()
                .items_center()
                .gap(space::s4())
                // 变更统计图标：只展示 tooltip，不绑定点击动作。
                .child(
                    Glyph::icon("vc.diff-icon", "icons/status/diff.svg")
                        .command(crate::shell::shared::CommandBinding {
                            id: "vc.diff_stats".into(),
                            title: std::rc::Rc::new(|_| Some("变更统计".into())),
                            shortcut: std::rc::Rc::new(|_| None),
                            request: std::rc::Rc::new(|_, _| {}),
                        })
                        .render(),
                )
                .child(
                    div()
                        .flex()
                        .items_center()
                        .child(
                            div()
                                .text_color(color::git_status(ColorKind::Added))
                                .child(format!("+{added}")),
                        )
                        .child(
                            div()
                                .text_color(color::git_status(ColorKind::Deleted))
                                .child(format!("-{deleted}")),
                        ),
                ),
        )
        // 右侧：暂存全部复选框
        .child(render_checkbox(all_stage_status, on_toggle_stage_all))
}

// ── 提交信息编辑区 ──

/// 提交编辑器组件：分隔线 + 多行编辑区 + 内嵌提交按钮。
///
/// ```text
/// ─────────────────  ← 分隔线
/// │ placeholder…   │
/// │                │  ← 编辑区（固定 N 行）
/// │          ✓ 提交 │  ← 内嵌按钮
/// ─────────────────
/// ```
pub(super) struct CommitEditor;

impl CommitEditor {
    const LINES: f32 = 12.0;

    /// 渲染完整组件（分隔线 + 编辑区 + 提交按钮）。
    pub(super) fn render(
        slot: Option<&Rc<TextEditorSlot>>,
        show_placeholder: bool,
        on_commit: impl Fn(&mut Window, &mut gpui::App) + 'static,
    ) -> Div {
        let line_h = typography::ui_line();
        let editor_h = line_h * Self::LINES + space::s4() * 2.0;

        let mut editor = div()
            .relative()
            .w_full()
            .h(editor_h)
            .bg(color::current().gray.s01)
            .px(space::s8())
            .py(space::s4())
            .text_size(typography::ui())
            .line_height(line_h)
            .text_color(color::current().gray.s09);

        if let Some(s) = slot {
            editor = editor.child(s.embed());
        }

        if show_placeholder {
            editor = editor.child(
                div()
                    .absolute()
                    .top_0()
                    .left_0()
                    .pl(space::s8())
                    .pt(space::s4())
                    .text_color(color::current().gray.s06)
                    .child("输入提交信息…"),
            );
        }

        editor = editor.child(Self::commit_button(line_h, on_commit));

        div()
            .flex_col()
            .child(div().w_full().h(px(1.0)).bg(color::current().gray.s05))
            .child(editor)
    }

    fn commit_button(
        line_h: gpui::Pixels,
        on_commit: impl Fn(&mut Window, &mut gpui::App) + 'static,
    ) -> Div {
        div()
            .absolute()
            .bottom_0()
            .right_0()
            .pr(space::s8())
            .pb(space::s4())
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap(space::s4())
                    .px(space::s8())
                    .h(line_h * 1.2)
                    .rounded(px(3.0))
                    .bg(color::current().blue.s03)
                    .text_color(color::current().blue.s08)
                    .text_size(typography::ui())
                    .cursor_pointer()
                    .hover(|style| style.bg(color::current().blue.s05))
                    .child(
                        svg()
                            .path("icons/actions/check.svg")
                            .size(px(12.0))
                            .text_color(color::current().blue.s08),
                    )
                    .child("提交")
                    .on_mouse_down(MouseButton::Left, move |_, window, cx| {
                        cx.stop_propagation();
                        on_commit(window, cx);
                    }),
            )
    }
}
