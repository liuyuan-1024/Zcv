//! 文件树的窗口运行态。
//!
//! `FileTreeModel` 是应用数据；`FileTreeRuntime` 是每个窗口自己的焦点句柄与
//! 交互装配。ShellView 只持有这个 feature 实例，不再散落保存 file_tree_* 细节。

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::app::App;
use crate::shell::KeyRequest;
use crate::shell::editor::{TextEditorSlot, TextTargetOwner};
use crate::shell::shared::scroll::ScrollHandle;
use crate::shell::workbench::controller::WorkbenchController;
use gpui::{Context, FocusHandle, Window};

use super::fs_ops::apply_outcome;
use super::{FileTreeActivation, FileTreeModel, FileTreeOutcome, FileTreePanel, FileTreeState};

#[derive(Clone)]
pub(crate) struct FileTreeRuntime {
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// 文件树 model 的真正拥有者。App 只借共享 handle 组合 workspace/views，
    /// editor router 通过 `owner_handle` 访问 pending name/rename 输入框。
    model: Rc<RefCell<FileTreeModel>>,
}

impl FileTreeRuntime {
    pub(crate) fn new<T>(cx: &mut Context<T>) -> Self {
        Self {
            focus: cx.focus_handle(),
            scroll: ScrollHandle::new(),
            model: Rc::new(RefCell::new(FileTreeModel::new())),
        }
    }

    pub(crate) fn model_handle(&self) -> Rc<RefCell<FileTreeModel>> {
        self.model.clone()
    }

    pub(crate) fn owner_handle(&self) -> Rc<RefCell<dyn TextTargetOwner>> {
        self.model.clone()
    }

    pub(crate) fn with_model_mut<R>(&self, f: impl FnOnce(&mut FileTreeModel) -> R) -> R {
        f(&mut self.model.borrow_mut())
    }

    /// 跑一个产出 [`FileTreeOutcome`] 的模型动作，并把 outcome 通过 [`apply_outcome`]
    /// 翻成 session 调用与 [`FileTreeActivation`]。effects.rs 的所有"模型 + 会话"
    /// 复合 effect 都走这一条路径，模型本身不再持有 [`WorkspaceSession`]。
    pub(crate) fn execute(
        &self,
        app: &mut App,
        f: impl FnOnce(&mut FileTreeModel) -> FileTreeOutcome,
    ) -> FileTreeActivation {
        let mut model = self.model.borrow_mut();
        let outcome = f(&mut model);
        app.with_workspace_session_mut(|session| apply_outcome(&mut model, outcome, session))
    }

    pub(crate) fn open_project(&self, root: PathBuf) {
        self.model.borrow_mut().open_project(root);
    }

    pub(crate) fn state(&self, app: &App) -> FileTreeState {
        self.model.borrow().state(app.workspace())
    }

    pub(crate) fn install_listeners<T: 'static>(
        &self,
        app: Rc<RefCell<App>>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        super::focus::install_focus_listeners(app, self.model_handle(), &self.focus, window, cx);
    }

    pub(crate) fn panel<'a>(
        &'a self,
        state: &'a FileTreeState,
        key_request: &'a KeyRequest,
        slot: &'a Rc<TextEditorSlot>,
        window: &Window,
    ) -> FileTreePanel<'a> {
        FileTreePanel {
            state,
            focus: &self.focus,
            key_request,
            slot,
            scroll: &self.scroll,
            is_focused: self.focus.is_focused(window),
        }
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(crate) fn reveal_after_project_open(
        &self,
        workbench: &Rc<RefCell<WorkbenchController>>,
        window: &mut Window,
    ) {
        self.model.borrow_mut().ensure_selection_initialized();
        super::focus::reveal_and_focus(workbench, &self.focus, window);
    }
}
