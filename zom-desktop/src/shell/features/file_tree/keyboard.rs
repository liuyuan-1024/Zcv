//! 文件树键盘交互。
//!
//! 本模块只关心文件树自己的按键语义；未命中的键继续交给全局 keymap。
//! 处于「新建条目」输入态时，按键先被输入处理截获。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{FocusHandle, Window};
use zom_workspace::EntryKind;

use crate::app::App;
use crate::shell::editor::EditorLineMode;
use crate::shell::{KeyRequest, normalized_chord};

use super::{FileTreeActivation, FileTreeKeyRequest};

pub(crate) fn key_request(
    app: Rc<RefCell<App>>,
    editor_focus: FocusHandle,
    fallback: KeyRequest,
) -> FileTreeKeyRequest {
    Rc::new(move |keystroke, window, cx| {
        let chord = normalized_chord(keystroke);

        // 新建条目输入态：先交给嵌入编辑器判定；它只消费编辑行为，
        // 其余按键回到文件树处理确认 / 取消或继续走全局快捷键。
        if app.borrow().file_tree_pending_active() {
            return handle_pending_key(&app, &chord, &fallback, window, cx);
        }

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
            "mod-n" => {
                app.borrow_mut().file_tree_begin_new_entry(EntryKind::File);
                true
            }
            "mod-shift-n" => {
                app.borrow_mut()
                    .file_tree_begin_new_entry(EntryKind::Directory);
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

/// 输入态按键：先由嵌入编辑器消费编辑行为；未消费时，文件树解释 Enter /
/// Esc，其余继续交给全局 keymap。
fn handle_pending_key(
    app: &Rc<RefCell<App>>,
    chord: &str,
    fallback: &KeyRequest,
    window: &mut Window,
    cx: &mut gpui::App,
) -> bool {
    let outcome = match app
        .borrow_mut()
        .dispatch_embedded_editor_key_input(chord.to_string(), EditorLineMode::single_line())
    {
        Ok(outcome) => outcome,
        Err(error) => {
            eprintln!("输入框编辑失败：{error}");
            return false;
        }
    };
    if outcome.handled {
        window.refresh();
        return true;
    }

    let handled_by_file_tree = match chord {
        "enter" => {
            app.borrow_mut().file_tree_commit_new_entry();
            true
        }
        "escape" => {
            app.borrow_mut().file_tree_cancel_new_entry();
            true
        }
        _ => false,
    };

    if handled_by_file_tree {
        window.refresh();
        return true;
    }

    fallback(chord.to_string(), window, cx)
}
