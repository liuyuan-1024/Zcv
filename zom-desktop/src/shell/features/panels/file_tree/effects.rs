//! 文件树 HostEffect 落地。
//!
//! view 层把 HostEffect 流过来，本模块只认 `FileTree*` 这批变体；其余
//! 一律返回 `false`，让 view 转给下一个 feature 试。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::Window;
use zom_command::HostEffect;

use crate::app::App;
use crate::focus::{AppFocus, FileTreeFocus};
use crate::shell::features::panels::PanelId;
use crate::shell::features::panels::file_tree::{FileTreeActivation, FileTreeRuntime};
use crate::shell::view::actions::request_focus;
use crate::shell::view::focus::FocusProjection;
use crate::shell::workbench::controller::WorkbenchController;

/// 试着把 effect 当成"文件树相关"处理；返回 `true` 表示已认领。
pub(crate) fn try_apply_effect(
    effect: &HostEffect,
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    file_tree: &FileTreeRuntime,
    focus: &FocusProjection,
    window: &mut Window,
) -> bool {
    match effect {
        HostEffect::FileTreeMoveSelection(delta) => {
            file_tree.with_model_mut(|ft| ft.move_selection(*delta));
        }
        HostEffect::FileTreeExtendSelection(delta) => {
            file_tree.with_model_mut(|ft| ft.extend_selection(*delta));
        }
        HostEffect::FileTreeEscape => {
            // 选区有内容时 model 清掉它并消化 Esc；否则按原有 focus_editor 路径把焦点交回主编辑区。
            // 逻辑写在宿主侧而非 model 是因为 focus 路由涉及 window，model 不该感知 UI。
            let consumed = file_tree.with_model_mut(|ft| ft.escape());
            if !consumed {
                request_focus(app, focus, AppFocus::editor(), window);
            }
        }
        HostEffect::FileTreeCollapseOrParent => {
            file_tree.with_model_mut(|ft| ft.collapse_or_parent());
        }
        HostEffect::FileTreeExpandOrInto => {
            file_tree.with_model_mut(|ft| ft.expand_or_into());
        }
        HostEffect::FileTreeActivate => {
            let activation = {
                let mut app = app.borrow_mut();
                file_tree.execute(&mut app, |ft| ft.activate_selected())
            };
            if matches!(activation, FileTreeActivation::OpenedFile) {
                request_focus(app, focus, AppFocus::editor(), window);
            }
        }
        HostEffect::FileTreeBeginNewEntry => {
            file_tree.with_model_mut(|ft| ft.begin_new_entry());
            // 文件树面板的 focus handle 也是新建输入框的 input handle。
            // 用同一句 move_to 既保证视觉焦点（行的蓝框 + caret 闪烁）出现在输入框，
            // 也让 IME / 文本命令路由到 FileTreePendingName。
            //
            // 不假设触发命令前文件树就在焦点：用户可能从命令面板、菜单或编辑器里发起。
            // 先 show_panel 把面板顶起，再聚一次焦保险。
            workbench.borrow_mut().show_panel(PanelId::FileTree);
            request_focus(
                app,
                focus,
                AppFocus::file_tree(FileTreeFocus::NewEntryName),
                window,
            );
        }
        HostEffect::FileTreeCommitNewEntry => {
            // 新建文件会被打开，焦点随之切到编辑器；新建目录留在文件树。
            let activation = {
                let mut app = app.borrow_mut();
                file_tree.execute(&mut app, |ft| ft.commit_new_entry())
            };
            if matches!(activation, FileTreeActivation::OpenedFile) {
                request_focus(app, focus, AppFocus::editor(), window);
            }
        }
        HostEffect::FileTreeCancelNewEntry => {
            file_tree.with_model_mut(|ft| ft.cancel_new_entry());
        }
        HostEffect::FileTreeBeginRename => {
            file_tree.with_model_mut(|ft| ft.begin_rename());
            // 与新建一样：保险起见 show_panel + 重新聚焦——用户可能从命令面板或菜单触发，此时文件树未必已经持焦。
            workbench.borrow_mut().show_panel(PanelId::FileTree);
            request_focus(
                app,
                focus,
                AppFocus::file_tree(FileTreeFocus::RenameEntry),
                window,
            );
        }
        HostEffect::FileTreeCommitRename => {
            // 与 CommitNewEntry 同构：文件被打开即把焦点切给编辑器；目录留在文件树。
            let activation = {
                let mut app = app.borrow_mut();
                file_tree.execute(&mut app, |ft| ft.commit_rename())
            };
            if matches!(activation, FileTreeActivation::OpenedFile) {
                request_focus(app, focus, AppFocus::editor(), window);
            }
        }
        HostEffect::FileTreeCancelRename => {
            file_tree.with_model_mut(|ft| ft.cancel_rename());
        }
        HostEffect::FileTreeRequestDelete => {
            file_tree.with_model_mut(|ft| ft.request_delete());
        }
        HostEffect::FileTreeConfirmDelete => {
            let mut app = app.borrow_mut();
            file_tree.execute(&mut app, |ft| ft.confirm_delete());
        }
        HostEffect::FileTreeCancelDelete => {
            file_tree.with_model_mut(|ft| ft.cancel_delete());
        }
        HostEffect::FileTreeCopy => {
            file_tree.with_model_mut(|ft| ft.copy_to_clipboard());
        }
        HostEffect::FileTreeCut => {
            file_tree.with_model_mut(|ft| ft.cut_to_clipboard());
        }
        HostEffect::FileTreePaste => {
            let mut app = app.borrow_mut();
            file_tree.execute(&mut app, |ft| ft.paste_from_clipboard());
        }
        _ => return false,
    }
    true
}
