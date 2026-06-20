//! FileStatusBar —— 活动文件状态栏（tab_bar 与 editor 之间的 sliver）。
//!
//! 上行常驻：左侧项目相对路径、右侧动作 glyph 槽（当前只有"打开文件搜索"）。
//! 文件级搜索唤起时，bar 内追加一行 search controls——搜索栏不带自己的 chrome，背景 / 内边距 / 底边都由本 bar 提供。
//!
//! 高度不写死：每行由内容（line_height / icon / 输入框）撑开。
//! 与 MEMORY 约定一致。

use std::rc::Rc;

use gpui::{AnyElement, Div, IntoElement, div, prelude::*};

use zom_workspace::view::ViewKind;

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
        .child(header_row(active_tab, search_open, commands));

    if search_open {
        bar = bar.child(search_runtime.render(
            search_state,
            key_request,
            &commands.search_intent,
            &commands.title_lookup,
            &commands.shortcut_lookup,
            search_query_slot,
            search_replacement_slot,
            focus_request,
        ));
    }

    bar
}

fn header_row(tab: &EditorTab, search_open: bool, commands: &WorkbenchCommandRequests) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(space::s8())
        .child(path_label(tab))
        .child(div().flex_1())
        .child(action_slot(tab, search_open, commands))
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
fn action_slot(
    tab: &EditorTab,
    search_open: bool,
    commands: &WorkbenchCommandRequests,
) -> AnyElement {
    let mut actions: Vec<AnyElement> = Vec::new();

    let active_color = color::glyph_active();
    let default_color = color::glyph_default();

    // Markdown 预览 glyph（仅在可预览文件上显示）
    if tab.language == "Markdown" {
        let preview_active = matches!(tab.kind, ViewKind::Preview);
        let color = if preview_active {
            active_color
        } else {
            default_color
        };
        actions.push(
            Glyph::icon("file-status-bar.preview", FILE_PREVIEW_ICON)
                .color(color)
                .command(commands.editor_open_preview.clone())
                .render(),
        );
    }

    // 文件搜索 glyph（常驻）：打开时高亮，点击可关闭。
    let search_command = if search_open {
        commands.file_search_dismiss.clone()
    } else {
        commands.file_search_activate.clone()
    };
    let color = if search_open {
        active_color
    } else {
        default_color
    };
    actions.push(
        Glyph::icon("file-status-bar.search", FILE_SEARCH_ICON)
            .color(color)
            .command(search_command)
            .render(),
    );

    let mut group = div().flex().flex_row().items_center().gap(space::s6());
    for action in actions {
        group = group.child(action);
    }
    group.into_any_element()
}
