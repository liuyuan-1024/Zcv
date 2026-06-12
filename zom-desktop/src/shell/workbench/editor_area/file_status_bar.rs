//! FileStatusBar —— 活动文件状态栏（tab_bar 与 editor 之间的 sliver）。
//!
//! 上行常驻：左侧项目相对路径、右侧动作 glyph 槽（当前只有"打开文件搜索"）。
//! 文件级搜索唤起时，bar 内追加一行 search controls——搜索栏不带自己的 chrome，背景 / 内边距 / 底边都由本 bar 提供。
//!
//! 高度不写死：每行由内容（line_height / icon / 输入框）撑开。
//! 与 MEMORY 约定一致。

use std::rc::Rc;

use gpui::{AnyElement, Div, IntoElement, div, prelude::*};

use zom_view::ViewKind;

use crate::editor::TextEditorSlot;
use crate::editor_state::EditorTab;
use crate::host_intent::KeyRequest;
use crate::shell::FocusRequest;
use crate::shell::features::search::{SearchRuntime, SearchState};
use crate::shell::shared::Glyph;
use crate::shell::workbench::WorkbenchCommandRequests;
use crate::theme::{color, space, typography};

const FILE_SEARCH_ICON: &str = "icons/panels/search.svg";
const FILE_PREVIEW_ICON: &str = "icons/actions/preview.svg";

#[allow(clippy::too_many_arguments)]
pub(crate) fn render(
    active_tab: &EditorTab,
    key_request: &KeyRequest,
    search_runtime: &SearchRuntime,
    search_state: &SearchState,
    search_query_slot: &Rc<TextEditorSlot>,
    search_replacement_slot: &Rc<TextEditorSlot>,
    search_open: bool,
    focus_request: &FocusRequest,
    commands: &WorkbenchCommandRequests,
) -> Div {
    let mut bar = div()
        .w_full()
        .flex_shrink_0()
        .flex()
        .flex_col()
        .gap(space::s4())
        .px(space::s6())
        .py(space::s4())
        .bg(color::current().gray.s01)
        .border_b_1()
        .border_color(color::current().gray.s05)
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(color::current().gray.s09)
        .child(header_row(active_tab, commands));

    if search_open {
        bar = bar.child(search_runtime.render(
            search_state,
            key_request,
            &commands.search_intent,
            &commands.search_intent_presentation,
            search_query_slot,
            search_replacement_slot,
            focus_request,
        ));
    }

    bar
}

fn header_row(tab: &EditorTab, commands: &WorkbenchCommandRequests) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(space::s8())
        .child(path_label(tab))
        .child(div().flex_1())
        .child(action_slot(tab, commands))
}

/// 左侧路径标签：优先项目相对路径，回退到 tab 文件名。
/// 单行 + truncate 防止过长路径撑爆 bar。
fn path_label(tab: &EditorTab) -> Div {
    let text = tab
        .relative_path
        .clone()
        .unwrap_or_else(|| tab.title.clone());
    div()
        .flex_shrink_0()
        .overflow_hidden()
        .whitespace_nowrap()
        .text_color(color::current().gray.s08)
        .child(text)
}

/// 右侧动作槽。点击 glyph 与快捷键共享同一条命令路径。
fn action_slot(tab: &EditorTab, commands: &WorkbenchCommandRequests) -> AnyElement {
    let mut actions: Vec<AnyElement> = Vec::new();

    // Markdown 预览 glyph（仅在可预览文件上显示）
    if tab.language == "Markdown" {
        let preview_active = matches!(tab.kind, ViewKind::Preview);
        actions.push(
            Glyph::icon(
                "file-status-bar.preview",
                FILE_PREVIEW_ICON,
                commands.editor_open_preview_presentation.title.clone(),
            )
            .hint(commands.editor_open_preview_presentation.hint.clone())
            .active(preview_active)
            .on_press(Rc::clone(&commands.editor_open_preview))
            .render(),
        );
    }

    // 文件搜索 glyph（常驻）
    actions.push(
        Glyph::icon(
            "file-status-bar.search",
            FILE_SEARCH_ICON,
            commands.file_search_activate_presentation.title.clone(),
        )
        .hint(commands.file_search_activate_presentation.hint.clone())
        .active(false)
        .on_press(Rc::clone(&commands.file_search_activate))
        .render(),
    );

    let mut group = div().flex().flex_row().items_center().gap(space::s6());
    for action in actions {
        group = group.child(action);
    }
    group.into_any_element()
}
