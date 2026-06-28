//! 搜索 HostEffect 落地。
//!
//! view 层把 HostEffect 流过来，本模块只认 `Search*` 这批变体；其余一律
//! 返回 `false`，让 view 转给下一个 feature 试。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::Window;
use zom_command::{HostEffect, SearchEffect};

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
        HostEffect::Search(SearchEffect::FocusNextField) => {
            focus_search_field(app, focus, FocusDirection::Next, window);
        }
        HostEffect::Search(SearchEffect::FocusPreviousField) => {
            focus_search_field(app, focus, FocusDirection::Previous, window);
        }
        HostEffect::Search(SearchEffect::Dismiss) => {
            dismiss_search(app, focus, window);
        }
        HostEffect::Search(SearchEffect::Toggle) => {
            if app.borrow().is_search_open() {
                dismiss_search(app, focus, window);
            } else {
                activate_search(app, focus, window);
            }
        }
        HostEffect::Search(SearchEffect::ConfirmMatch) => {
            app.borrow_mut()
                .apply_search_action_from_effect(SearchAction::ConfirmMatch);
            request_focus(app, focus, AppFocus::editor(), window);
            window.refresh();
        }
        HostEffect::Search(SearchEffect::ToggleOption(option)) => {
            apply_search(app, SearchAction::ToggleOption(*option), window);
        }
        HostEffect::Search(SearchEffect::FindNext) => {
            apply_search(app, SearchAction::FindNext, window);
        }
        HostEffect::Search(SearchEffect::FindPrevious) => {
            apply_search(app, SearchAction::FindPrevious, window);
        }
        HostEffect::Search(SearchEffect::ReplaceNext) => {
            apply_search(app, SearchAction::ReplaceNext, window);
        }
        HostEffect::Search(SearchEffect::ReplaceAll) => {
            apply_search(app, SearchAction::ReplaceAll, window);
        }
        _ => return false,
    }
    true
}

/// 打开 bar 并把焦点送到 query。收起走 [`dismiss_search`]。
fn activate_search(app: &Rc<RefCell<App>>, focus: &FocusProjection, window: &mut Window) {
    // Opened 是幂等的：set_open(true) no-op；sync 在 query/options 没变时
    // 返回 Idle、不动光标。所以即便 bar 已开也无脑发一次，省一次 is_open 读。
    app.borrow_mut()
        .apply_search_action_from_effect(SearchAction::Opened);
    request_focus(app, focus, AppFocus::search(SearchField::Query), window);
    window.refresh();
}

fn dismiss_search(app: &Rc<RefCell<App>>, focus: &FocusProjection, window: &mut Window) {
    app.borrow_mut()
        .apply_search_action_from_effect(SearchAction::ConfirmMatch);
    close_bar(app, focus, window);
}

fn apply_search(app: &Rc<RefCell<App>>, action: SearchAction, window: &mut Window) {
    app.borrow_mut().apply_search_action_from_effect(action);
    window.refresh();
}

fn close_bar(app: &Rc<RefCell<App>>, focus: &FocusProjection, window: &mut Window) {
    app.borrow_mut()
        .apply_search_action_from_effect(SearchAction::Closed);
    // 回上一个焦点（通常是编辑器，但如果搜索从别处被唤起也尊重那条路径），
    // 与 project picker dismiss 走的是同一套 restore_previous_focus 语义。
    let previous = app.borrow_mut().restore_previous_focus();
    focus.apply(previous, window);
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
