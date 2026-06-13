//! 文件树的窗口运行态。
//!
//! `FileTreeModel` 是应用数据；`FileTreeRuntime` 是每个窗口自己的焦点句柄与
//! 交互装配。ShellView 只持有这个 feature 实例，不再散落保存 file_tree_* 细节。

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::app::App;
use crate::editor::TextEditorSlot;
use crate::host_intent::KeyRequest;
use crate::ports::{FileTreeAction, FileTreeActionResult, FileTreeHost};
use crate::shell::shared::scroll::ScrollHandle;
use crate::shell::workbench::controller::WorkbenchController;
use crate::text_target::TextTargetOwner;
use crate::workspace_session::WorkspaceSession;
use gpui::{Context, FocusHandle, Window};

use super::fs_ops::apply_outcome;
use super::{FileTreeActivation, FileTreeModel, FileTreePanel, FileTreeState};

#[derive(Clone)]
pub(crate) struct FileTreeRuntime {
    focus: FocusHandle,
    scroll: ScrollHandle,
    /// 文件树 model 的真正拥有者。
    /// App 只借共享 handle 组合 workspace/views，editor router 通过 `owner_handle` 访问 pending name/rename 输入框。
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

    pub(crate) fn open_project(&self, root: PathBuf) {
        self.model.borrow_mut().open_project(root);
    }

    pub(crate) fn state(&self, app: &App) -> FileTreeState {
        let active_path = app
            .active_buffer_id()
            .and_then(|id| app.workspace().buffer(id))
            .and_then(|wb| wb.path())
            .map(std::path::PathBuf::from);
        self.model.borrow().state(active_path)
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
        new_entry_slot: &'a Rc<TextEditorSlot>,
        rename_slot: &'a Rc<TextEditorSlot>,
        on_item_click: &'a Rc<dyn Fn(PathBuf, &mut Window, &mut gpui::App)>,
        window: &Window,
    ) -> FileTreePanel<'a> {
        FileTreePanel {
            state,
            focus: &self.focus,
            key_request,
            new_entry_slot,
            rename_slot,
            scroll: &self.scroll,
            is_focused: self.focus.is_focused(window),
            on_item_click,
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

    pub(crate) fn select(&self, path: PathBuf) {
        self.model.borrow_mut().selected = Some(path);
    }

    pub(crate) fn move_selection(&self, delta: isize) {
        self.model.borrow_mut().move_selection(delta);
    }

    pub(crate) fn extend_selection(&self, delta: isize) {
        self.model.borrow_mut().extend_selection(delta);
    }

    pub(crate) fn escape(&self) -> bool {
        self.model.borrow_mut().escape()
    }

    pub(crate) fn collapse_or_parent(&self) {
        self.model.borrow_mut().collapse_or_parent();
    }

    pub(crate) fn expand_or_into(&self) {
        self.model.borrow_mut().expand_or_into();
    }

    pub(crate) fn begin_new_entry(&self) {
        self.model.borrow_mut().begin_new_entry();
    }

    pub(crate) fn cancel_new_entry(&self) {
        self.model.borrow_mut().cancel_new_entry();
    }

    pub(crate) fn begin_rename(&self) {
        self.model.borrow_mut().begin_rename();
    }

    pub(crate) fn cancel_rename(&self) {
        self.model.borrow_mut().cancel_rename();
    }

    pub(crate) fn request_delete(&self) -> bool {
        let mut model = self.model.borrow_mut();
        model.request_delete();
        model.pending_delete.is_some()
    }

    pub(crate) fn cancel_delete(&self) {
        self.model.borrow_mut().cancel_delete();
    }

    pub(crate) fn copy_to_clipboard(&self) {
        self.model.borrow_mut().copy_to_clipboard();
    }

    pub(crate) fn cut_to_clipboard(&self) {
        self.model.borrow_mut().cut_to_clipboard();
    }

    pub(crate) fn take_bubbles(&self) -> Vec<zom_command::BubbleRequest> {
        self.model.borrow_mut().take_bubbles()
    }
}

impl FileTreeHost for FileTreeRuntime {
    fn apply_file_tree_action_from_effect(
        &self,
        action: FileTreeAction,
        session: &mut WorkspaceSession,
    ) -> FileTreeActionResult {
        let mut model = self.model.borrow_mut();
        let mut result = FileTreeActionResult::default();

        let activation = match action {
            FileTreeAction::Activate => {
                let outcome = model.activate_selected();
                apply_outcome(&mut model, outcome, session)
            }
            FileTreeAction::CommitNewEntry => {
                let outcome = model.commit_new_entry();
                apply_outcome(&mut model, outcome, session)
            }
            FileTreeAction::CommitRename => {
                let outcome = model.commit_rename();
                apply_outcome(&mut model, outcome, session)
            }
            FileTreeAction::ConfirmDelete => {
                let outcome = model.confirm_delete();
                apply_outcome(&mut model, outcome, session)
            }
            FileTreeAction::Paste => {
                let outcome = model.paste_from_clipboard();
                apply_outcome(&mut model, outcome, session)
            }
        };

        result.opened_file = matches!(activation, FileTreeActivation::OpenedFile);
        result.bubbles = model.take_bubbles();
        result
    }
}
