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
use crate::shell::features::panels::file_tree::FileTreeActivation;
use crate::shell::view::actions::request_focus;
use crate::shell::view::focus::FocusProjection;
use crate::shell::workbench::controller::WorkbenchController;

/// 试着把 effect 当成"文件树相关"处理；返回 `true` 表示已认领。
pub(crate) fn try_apply_effect(
    effect: &HostEffect,
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    focus: &FocusProjection,
    window: &mut Window,
) -> bool {
    match effect {
        HostEffect::FileTreeMoveSelection(delta) => {
            app.borrow_mut().file_tree_move_selection(*delta);
        }
        HostEffect::FileTreeExtendSelection(delta) => {
            app.borrow_mut().file_tree_extend_selection(*delta);
        }
        HostEffect::FileTreeEscape => {
            // 选区有内容时 model 清掉它并消化 Esc；否则按原有 focus_editor 路径
            // 把焦点交回主编辑区。逻辑写在宿主侧而非 model 是因为 focus 路由
            // 涉及 window，model 不该感知 UI。
            let consumed = app.borrow_mut().file_tree_escape();
            if !consumed {
                request_focus(app, focus, AppFocus::editor(), window);
            }
        }
        HostEffect::FileTreeCollapseOrParent => {
            app.borrow_mut().file_tree_collapse_or_parent();
        }
        HostEffect::FileTreeExpandOrInto => {
            app.borrow_mut().file_tree_expand_or_into();
        }
        HostEffect::FileTreeActivate => {
            let activation = app.borrow_mut().file_tree_activate();
            if matches!(activation, FileTreeActivation::OpenedFile) {
                request_focus(app, focus, AppFocus::editor(), window);
            }
        }
        HostEffect::FileTreeBeginNewEntry => {
            app.borrow_mut().file_tree_begin_new_entry();
            // 文件树面板的 focus handle 也是新建输入框的 input handle —— 用同
            // 一句 move_to 既保证视觉焦点（行的蓝框 + caret 闪烁）出现在
            // 输入框，也让 IME / 文本命令路由到 FileTreePendingName。
            //
            // 不假设触发命令前文件树就在焦点：用户可能从命令面板、菜单或
            // 编辑器里发起，先 show_panel 把面板顶起，再聚一次焦保险。
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
            let activation = app.borrow_mut().file_tree_commit_new_entry();
            if matches!(activation, FileTreeActivation::OpenedFile) {
                request_focus(app, focus, AppFocus::editor(), window);
            }
        }
        HostEffect::FileTreeCancelNewEntry => {
            app.borrow_mut().file_tree_cancel_new_entry();
        }
        HostEffect::FileTreeRequestDelete => {
            app.borrow_mut().file_tree_request_delete();
        }
        HostEffect::FileTreeConfirmDelete => {
            app.borrow_mut().file_tree_confirm_delete();
        }
        HostEffect::FileTreeCancelDelete => {
            app.borrow_mut().file_tree_cancel_delete();
        }
        HostEffect::FileTreeCopy => {
            app.borrow_mut().file_tree_copy();
        }
        HostEffect::FileTreeCut => {
            app.borrow_mut().file_tree_cut();
        }
        HostEffect::FileTreePaste => {
            app.borrow_mut().file_tree_paste();
        }
        _ => return false,
    }
    true
}
