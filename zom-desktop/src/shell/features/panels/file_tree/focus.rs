//! 文件树焦点与显示策略。
//!
//! 文件树是否显示仍由 WorkbenchController 维护；本模块集中保存文件树面板
//! 自己的焦点语义，避免散落在 ShellView 里。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Context, FocusHandle, Window};

use crate::app::App;
use crate::focus::{AppFocus, FileTreeFocus};
use crate::shell::features::panels::focus_panel_handle;
use crate::shell::workbench::controller::WorkbenchController;
use crate::ui_id::PanelId;

use super::FileTreeModel;

/// 注册文件树焦点监听：获焦时初始化首个可见行，两端都刷新高亮状态。
pub(crate) fn install_focus_listeners<T: 'static>(
    app: Rc<RefCell<App>>,
    model: Rc<RefCell<FileTreeModel>>,
    focus: &FocusHandle,
    window: &mut Window,
    cx: &mut Context<T>,
) {
    cx.on_focus(focus, window, move |_, _, cx| {
        app.borrow_mut()
            .request_focus_from_shell(AppFocus::file_tree(FileTreeFocus::Navigate));
        model.borrow_mut().ensure_selection_initialized();
        cx.notify();
    })
    .detach();
    cx.on_blur(focus, window, |_, _, cx| {
        cx.notify();
    })
    .detach();
}

/// 打开项目后调用：确保文件树面板可见且为 active panel，然后把焦点交给它。
pub(crate) fn reveal_and_focus(
    workbench: &Rc<RefCell<WorkbenchController>>,
    file_tree_focus: &FocusHandle,
    window: &mut Window,
) {
    workbench.borrow_mut().show_panel(PanelId::FileTree);
    focus_panel_handle(file_tree_focus.clone(), window, true);
}
