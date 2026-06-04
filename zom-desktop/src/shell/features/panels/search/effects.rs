//! 搜索面板 HostEffect 落地。
//!
//! view 层把 HostEffect 流过来，本模块只认 `Search*` 这批变体；其余一律
//! 返回 `false`，让 view 转给下一个 feature 试。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::Window;
use zom_command::HostEffect;

use crate::app::App;
use crate::focus::{AppFocus, PanelFocus, SearchField};
use crate::shell::features::panels::PanelId;
use crate::shell::view::actions::request_focus;
use crate::shell::view::focus::FocusProjection;
use crate::shell::workbench::controller::WorkbenchController;

pub(crate) fn try_apply_effect(
    effect: &HostEffect,
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    panel_runtimes: &crate::shell::features::panels::PanelRuntimes,
    focus: &FocusProjection,
    window: &mut Window,
) -> bool {
    match effect {
        HostEffect::SearchActivate => {
            activate_search(app, workbench, panel_runtimes, focus, window);
        }
        HostEffect::SearchFocusNextField => {
            focus_search_field(app, focus, FocusDirection::Next, window);
        }
        HostEffect::SearchFocusPreviousField => {
            focus_search_field(app, focus, FocusDirection::Previous, window);
        }
        HostEffect::SearchFocusEditor => {
            request_focus(app, focus, AppFocus::editor(), window);
        }
        HostEffect::SearchToggleOption(option) => {
            let option = *option;
            let search = panel_runtimes.search_runtime_handle();
            app.borrow_mut()
                .with_workspace_views_mut(|workspace, views| {
                    search.toggle_option(workspace, views, option)
                });
            window.refresh();
        }
        HostEffect::SearchFindNext => {
            let search = panel_runtimes.search_runtime_handle();
            app.borrow_mut()
                .with_workspace_views_mut(|workspace, views| search.find_next(workspace, views));
            window.refresh();
        }
        HostEffect::SearchFindPrevious => {
            let search = panel_runtimes.search_runtime_handle();
            app.borrow_mut()
                .with_workspace_views_mut(|workspace, views| {
                    search.find_previous(workspace, views)
                });
            window.refresh();
        }
        HostEffect::SearchReplaceNext => {
            let search = panel_runtimes.search_runtime_handle();
            app.borrow_mut()
                .with_workspace_views_mut(|workspace, views| search.replace_next(workspace, views));
            window.refresh();
        }
        HostEffect::SearchReplaceAll => {
            let search = panel_runtimes.search_runtime_handle();
            app.borrow_mut()
                .with_workspace_views_mut(|workspace, views| search.replace_all(workspace, views));
            window.refresh();
        }
        _ => return false,
    }
    true
}

/// `mod-f` 行为矩阵：
///
/// | 可见 | 焦点在面板 | 行为 |
/// |---|---|---|
/// | 否 | * | 显示 + 聚焦 query |
/// | 是 | 在 | 隐藏，焦点回编辑器 |
/// | 是 | 不在 | 把焦点搬到面板 |
///
/// 当前只有单文件搜索（per-buffer），没有 scope 维度；跨文件搜索作为
/// 独立 workspace 服务再加，会引入各自的命令与 effect，不复用本路径。
fn activate_search(
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    panel_runtimes: &crate::shell::features::panels::PanelRuntimes,
    focus: &FocusProjection,
    window: &mut Window,
) {
    let panel = PanelId::Search;
    let visible = workbench.borrow().is_panel_active(panel);

    if !visible {
        // 隐藏 → 显示 + 聚焦
        workbench.borrow_mut().show_panel(panel);
        let search = panel_runtimes.search_runtime_handle();
        app.borrow_mut()
            .with_workspace_views_mut(|workspace, views| search.on_panel_opened(workspace, views));
        request_focus(app, focus, AppFocus::search(SearchField::Query), window);
        window.refresh();
        return;
    }

    // 已显示：query / replacement 任一输入框聚焦都算"焦点在搜索面板"。
    let focus_in_panel = focus.is_at_panel(panel, window);

    if focus_in_panel {
        // 已显示 + 焦点在面板 → 收起，焦点回编辑器。
        // 同时清掉活动 buffer 的 search 高亮，标记 panel 关闭。
        workbench.borrow_mut().hide_panel(panel);
        let search = panel_runtimes.search_runtime_handle();
        app.borrow_mut()
            .with_workspace_views_mut(|workspace, _views| search.on_panel_closed(workspace));
        request_focus(app, focus, AppFocus::editor(), window);
    } else {
        // 已显示 + 焦点不在 → 把焦点搬过去
        request_focus(app, focus, AppFocus::search(SearchField::Query), window);
    }
    window.refresh();
}

#[derive(Clone, Copy)]
enum FocusDirection {
    Next,
    Previous,
}

fn focus_search_field(
    app: &Rc<RefCell<App>>,
    focus: &FocusProjection,
    direction: FocusDirection,
    window: &mut Window,
) {
    let target = match (direction, app.borrow().focus().current()) {
        (FocusDirection::Next, AppFocus::Panel(PanelFocus::Search(SearchField::Query))) => {
            AppFocus::search(SearchField::Replacement)
        }
        (
            FocusDirection::Previous,
            AppFocus::Panel(PanelFocus::Search(SearchField::Replacement)),
        ) => AppFocus::search(SearchField::Query),
        (FocusDirection::Next | FocusDirection::Previous, _) => {
            AppFocus::search(SearchField::Query)
        }
    };
    request_focus(app, focus, target, window);
    window.refresh();
}
