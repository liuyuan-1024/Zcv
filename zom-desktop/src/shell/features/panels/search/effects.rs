//! 搜索面板 HostEffect 落地。
//!
//! view 层把 HostEffect 流过来，本模块只认 `Search*` 这批变体；其余一律
//! 返回 `false`，让 view 转给下一个 feature 试。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::Window;
use zom_command::{HostEffect, SearchScope};

use crate::app::App;
use crate::shell::features::panels::{PanelId, PanelRuntimes};
use crate::shell::view::focus::{FocusRouter, FocusTarget};
use crate::shell::workbench::controller::WorkbenchController;

pub(crate) fn try_apply_effect(
    effect: &HostEffect,
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    panel_runtimes: &PanelRuntimes,
    focus: &FocusRouter<'_>,
    window: &mut Window,
) -> bool {
    match effect {
        HostEffect::SearchActivateScope(scope) => {
            activate_search_scope(*scope, app, workbench, panel_runtimes, focus, window);
        }
        HostEffect::SearchFocusNextField => {
            focus_search_field(panel_runtimes, FocusDirection::Next, window);
        }
        HostEffect::SearchFocusPreviousField => {
            focus_search_field(panel_runtimes, FocusDirection::Previous, window);
        }
        HostEffect::SearchFocusEditor => focus.move_to(FocusTarget::Editor, window),
        HostEffect::SearchToggleOption(option) => {
            app.borrow_mut().search_toggle_option(*option);
            window.refresh();
        }
        HostEffect::SearchFindNext => {
            app.borrow_mut().search_find_next();
            window.refresh();
        }
        HostEffect::SearchFindPrevious => {
            app.borrow_mut().search_find_previous();
            window.refresh();
        }
        HostEffect::SearchReplaceNext => {
            app.borrow_mut().search_replace_next();
            window.refresh();
        }
        HostEffect::SearchReplaceAll => {
            app.borrow_mut().search_replace_all();
            window.refresh();
        }
        _ => return false,
    }
    true
}

/// `mod-f` / `mod-shift-f` 的行为矩阵：
///
/// | 可见 | 请求 scope | 焦点在面板 | 行为 |
/// |---|---|---|---|
/// | 否 | 任一 | * | 显示 + 设 scope + 聚焦 query |
/// | 是 | == 当前 | 在 | 隐藏，焦点回编辑器 |
/// | 是 | == 当前 | 不在 | 把焦点搬到面板 |
/// | 是 | != 当前 | 在 | 切 scope，焦点不动（用户可能正在打字） |
/// | 是 | != 当前 | 不在 | 切 scope + 把焦点搬到面板 |
///
/// 两个快捷键各自带一个固定 scope —— `mod-f` 永远代表"我要文件级"、
/// `mod-shift-f` 永远代表"我要项目级"；至于是开是关、是切是聚焦，由当前
/// 状态 + 请求 scope + 焦点位置共同决定，调用方不需要分支。
fn activate_search_scope(
    scope: SearchScope,
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    panel_runtimes: &PanelRuntimes,
    focus: &FocusRouter<'_>,
    window: &mut Window,
) {
    let panel = PanelId::Search;
    let visible = workbench.borrow().is_panel_active(panel);

    if !visible {
        // 隐藏 → 显示 + 设 scope + 聚焦
        app.borrow_mut().search_set_scope(scope);
        workbench.borrow_mut().show_panel(panel);
        focus.move_to(FocusTarget::Panel(panel), window);
        window.refresh();
        return;
    }

    // 已显示：用 query / replacement 两个焦点宿主判定"焦点是否在搜索面板"。
    // 只问 FocusRouter 会漏 replacement —— router 只认面板的主 focus handle。
    let focus_in_panel = panel_runtimes
        .search_query_focus_handle()
        .is_focused(window)
        || panel_runtimes
            .search_replacement_focus_handle()
            .is_focused(window);
    let current = app.borrow().search_state().scope;

    if current == scope {
        if focus_in_panel {
            // 已显示 + 同 scope + 焦点在面板 → 收起，焦点回编辑器
            workbench.borrow_mut().hide_panel(panel);
            focus.move_to(FocusTarget::Editor, window);
        } else {
            // 已显示 + 同 scope + 焦点不在 → 把焦点搬过去
            focus.move_to(FocusTarget::Panel(panel), window);
        }
    } else {
        // 已显示 + 切 scope：focus 在面板里就别动（用户可能正在打字），
        // 不在则顺手聚到 query。
        app.borrow_mut().search_set_scope(scope);
        if !focus_in_panel {
            focus.move_to(FocusTarget::Panel(panel), window);
        }
    }
    window.refresh();
}

#[derive(Clone, Copy)]
enum FocusDirection {
    Next,
    Previous,
}

fn focus_search_field(
    panel_runtimes: &PanelRuntimes,
    direction: FocusDirection,
    window: &mut Window,
) {
    let query = panel_runtimes.search_query_focus_handle();
    let replacement = panel_runtimes.search_replacement_focus_handle();
    let target = match direction {
        FocusDirection::Next if query.is_focused(window) => replacement,
        FocusDirection::Next if replacement.is_focused(window) => query,
        FocusDirection::Previous if replacement.is_focused(window) => query,
        FocusDirection::Previous if query.is_focused(window) => replacement,
        FocusDirection::Next | FocusDirection::Previous => query,
    };
    window.focus(&target);
    window.refresh();
}
