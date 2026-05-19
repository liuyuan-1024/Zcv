//! 文件树键盘交互。
//!
//! 本模块只关心文件树自己的按键语义；未命中的键继续交给全局 keymap。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::FocusHandle;

use crate::app::App;
use crate::shell::KeyRequest;

use super::{FileTreeActivation, FileTreeKeyRequest};

pub(crate) fn key_request(
    app: Rc<RefCell<App>>,
    editor_focus: FocusHandle,
    fallback: KeyRequest,
) -> FileTreeKeyRequest {
    Rc::new(move |chord, window, cx| {
        let consumed = match chord.as_str() {
            "up" => {
                app.borrow_mut().file_tree_move_selection(-1);
                true
            }
            "down" => {
                app.borrow_mut().file_tree_move_selection(1);
                true
            }
            "left" => {
                app.borrow_mut().file_tree_collapse_or_parent();
                true
            }
            "right" => {
                app.borrow_mut().file_tree_expand_or_into();
                true
            }
            "enter" => {
                let action = app.borrow_mut().file_tree_activate();
                if matches!(action, FileTreeActivation::OpenedFile) {
                    window.focus(&editor_focus);
                }
                true
            }
            "escape" => {
                window.focus(&editor_focus);
                true
            }
            _ => false,
        };

        if consumed {
            window.refresh();
            return true;
        }

        // 未匹配文件树自己的按键时，退回全局 keymap，让 mod-shift-e 等
        // workbench 级快捷键照常工作。
        fallback(chord, window, cx)
    })
}
