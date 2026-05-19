//! 文件树的窗口运行态。
//!
//! `FileTreeModel` 是应用数据；`FileTreeRuntime` 是每个窗口自己的焦点句柄与
//! 交互装配。ShellView 只持有这个 feature 实例，不再散落保存 file_tree_* 细节。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Context, FocusHandle, Window};

use crate::app::App;
use crate::shell::KeyRequest;
use crate::shell::workbench::controller::WorkbenchController;

use super::{FileTreeKeyRequest, FileTreePanel, FileTreeState};

#[derive(Clone)]
pub(crate) struct FileTreeRuntime {
    focus: FocusHandle,
}

impl FileTreeRuntime {
    pub(crate) fn new<T>(cx: &mut Context<T>) -> Self {
        Self {
            focus: cx.focus_handle(),
        }
    }

    pub(crate) fn install_listeners<T: 'static>(
        &self,
        app: Rc<RefCell<App>>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        super::focus::install_focus_listeners(app, &self.focus, window, cx);
    }

    pub(crate) fn panel<'a>(
        &'a self,
        state: &'a FileTreeState,
        key_request: &'a FileTreeKeyRequest,
        window: &Window,
    ) -> FileTreePanel<'a> {
        FileTreePanel {
            state,
            focus: &self.focus,
            key_request,
            is_focused: self.focus.is_focused(window),
        }
    }

    pub(crate) fn key_request(
        &self,
        app: Rc<RefCell<App>>,
        editor_focus: FocusHandle,
        fallback: KeyRequest,
    ) -> FileTreeKeyRequest {
        super::keyboard::key_request(app, editor_focus, fallback)
    }

    pub(crate) fn handle_toggle_request(
        &self,
        workbench: &Rc<RefCell<WorkbenchController>>,
        editor_focus_fallback: &FocusHandle,
        window: &mut Window,
    ) {
        super::focus::handle_toggle_request(workbench, editor_focus_fallback, &self.focus, window);
    }

    pub(crate) fn reveal_after_project_open(
        &self,
        app: &Rc<RefCell<App>>,
        workbench: &Rc<RefCell<WorkbenchController>>,
        window: &mut Window,
    ) {
        app.borrow_mut().file_tree_ensure_selection_initialized();
        super::focus::reveal_and_focus(workbench, &self.focus, window);
    }
}
