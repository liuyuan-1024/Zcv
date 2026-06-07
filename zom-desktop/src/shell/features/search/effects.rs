//! 搜索 HostEffect 落地。
//!
//! view 层把 HostEffect 流过来，本模块只认 `Search*` 这批变体；其余一律
//! 返回 `false`，让 view 转给下一个 feature 试。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::Window;
use zom_command::HostEffect;

use crate::app::App;
use crate::focus::{AppFocus, SearchField};
use crate::ports::SearchAction;
use crate::shell::view::actions::request_focus;
use crate::shell::view::focus::FocusProjection;

pub(crate) fn try_apply_effect(
    effect: &HostEffect,
    app: &Rc<RefCell<App>>,
    focus: &FocusProjection,
    window: &mut Window,
) -> bool {
    match effect {
        HostEffect::SearchActivate => {
            activate_search(app, focus, window);
        }
        HostEffect::SearchFocusNextField => {
            focus_search_field(app, focus, FocusDirection::Next, window);
        }
        HostEffect::SearchFocusPreviousField => {
            focus_search_field(app, focus, FocusDirection::Previous, window);
        }
        HostEffect::SearchFocusEditor => {
            // Esc 路径：先把光标折叠到命中末尾，再收起 bar、焦点回编辑器。
            // close_bar 里的 Closed 会清掉高亮，但 selection 已经在前一步定下来。
            app.borrow_mut()
                .apply_search_action(SearchAction::ConfirmMatch);
            close_bar(app, focus, window);
        }
        HostEffect::SearchConfirmMatch => {
            // Enter 路径：光标折叠到命中末尾，焦点回编辑器；**bar 保留**。
            // 想继续改 query，用户从编辑器按 mod-f 即可回到输入框。
            app.borrow_mut()
                .apply_search_action(SearchAction::ConfirmMatch);
            request_focus(app, focus, AppFocus::editor(), window);
            window.refresh();
        }
        HostEffect::SearchToggleOption(option) => {
            app.borrow_mut()
                .apply_search_action(SearchAction::ToggleOption(*option));
            window.refresh();
        }
        HostEffect::SearchFindNext => {
            app.borrow_mut().apply_search_action(SearchAction::FindNext);
            window.refresh();
        }
        HostEffect::SearchFindPrevious => {
            app.borrow_mut()
                .apply_search_action(SearchAction::FindPrevious);
            window.refresh();
        }
        HostEffect::SearchReplaceNext => {
            app.borrow_mut()
                .apply_search_action(SearchAction::ReplaceNext);
            window.refresh();
        }
        HostEffect::SearchReplaceAll => {
            app.borrow_mut()
                .apply_search_action(SearchAction::ReplaceAll);
            window.refresh();
        }
        _ => return false,
    }
    true
}

/// `mod-f`：打开 bar 并把焦点送到 query。已开则只搬焦点（幂等）。
/// 收起由 Esc（[`HostEffect::SearchFocusEditor`]）显式触发，不在本函数里走切换。
///
/// 当前只有单文件搜索（per-buffer），没有 scope 维度；跨文件搜索是
/// `search.project_activate`，独立路径。
fn activate_search(app: &Rc<RefCell<App>>, focus: &FocusProjection, window: &mut Window) {
    // Opened 是幂等的：set_open(true) no-op；sync 在 query/options 没变时
    // 返回 Idle、不动光标。所以即便 bar 已开也无脑发一次，省一次 is_open 读。
    app.borrow_mut().apply_search_action(SearchAction::Opened);
    request_focus(app, focus, AppFocus::search(SearchField::Query), window);
    window.refresh();
}

fn close_bar(app: &Rc<RefCell<App>>, focus: &FocusProjection, window: &mut Window) {
    app.borrow_mut().apply_search_action(SearchAction::Closed);
    request_focus(app, focus, AppFocus::editor(), window);
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
    let current_field = app.borrow().focus().current().as_search();
    let target = match (direction, current_field) {
        (FocusDirection::Next, Some(SearchField::Query)) => {
            AppFocus::search(SearchField::Replacement)
        }
        (FocusDirection::Previous, Some(SearchField::Replacement)) => {
            AppFocus::search(SearchField::Query)
        }
        _ => AppFocus::search(SearchField::Query),
    };
    request_focus(app, focus, target, window);
    window.refresh();
}
