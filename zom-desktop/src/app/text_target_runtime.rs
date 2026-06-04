//! 文本目标路由中心。
//!
//! 主编辑区、搜索输入框、文件树新建/重命名、设置 TOML 等都通过同一套
//! [`TextTargetOwner`] / [`TextTargetQuery`] 接口参与命令与 IME 路由——search
//! 的双输入框由 SearchModel 直接 impl trait 并按 focus 内部分派，宿主侧不再
//! 给任何 feature 留特殊分支。

use std::cell::RefCell;
use std::rc::Rc;

use zom_command::{CommandError, EditTarget, KeyContext};

use crate::focus::AppFocus;
use crate::text_target::{
    EditorRouter, EditorRouterMut, EditorTargetRegistry, MainEditorOwner, MainEditorOwnerRef,
    TextTargetOwner, TextTargetQuery,
};
use crate::workspace_session::WorkspaceSession;

pub(super) struct TextTargetRuntime {
    editor_targets: EditorTargetRegistry,
}

impl TextTargetRuntime {
    pub(super) fn new() -> Self {
        Self {
            editor_targets: EditorTargetRegistry::new(),
        }
    }

    pub(super) fn install_editor_owner(&mut self, owner: Rc<RefCell<dyn TextTargetOwner>>) {
        self.editor_targets.register(owner);
    }

    pub(super) fn with_router<R>(
        &self,
        session: &WorkspaceSession,
        f: impl FnOnce(EditorRouter<'_>) -> R,
    ) -> R {
        let main = MainEditorOwnerRef::new(session.workspace(), session.views());
        let registry_borrows = self.editor_targets.borrow_all();

        let mut owners: Vec<&dyn TextTargetQuery> = Vec::new();
        for borrow in registry_borrows.iter() {
            owners.push(&**borrow as &dyn TextTargetQuery);
        }
        owners.push(&main as &dyn TextTargetQuery);
        f(EditorRouter::new(owners))
    }

    pub(super) fn with_router_mut<R>(
        &mut self,
        _focus: AppFocus,
        session: &mut WorkspaceSession,
        f: impl FnOnce(EditorRouterMut<'_>) -> R,
    ) -> R {
        let (workspace, views) = session.parts_mut();
        let mut main = MainEditorOwner::new(workspace, views);
        let mut registry_borrows = self.editor_targets.borrow_all_mut();

        let mut owners: Vec<&mut dyn TextTargetOwner> = Vec::new();
        for borrow in registry_borrows.iter_mut() {
            owners.push(&mut **borrow as &mut dyn TextTargetOwner);
        }
        owners.push(&mut main as &mut dyn TextTargetOwner);
        f(EditorRouterMut::new(owners))
    }

    pub(super) fn key_contexts_for(
        &self,
        session: &WorkspaceSession,
        focus: AppFocus,
    ) -> Option<Vec<KeyContext>> {
        self.with_router(session, |router| router.key_contexts_for(focus))
    }

    pub(super) fn accepts_focus(&self, session: &WorkspaceSession, focus: AppFocus) -> bool {
        self.with_router(session, |router| router.accepts_focus(focus))
    }

    pub(super) fn is_composing(&self, session: &WorkspaceSession, focus: AppFocus) -> bool {
        self.with_router(session, |router| router.is_composing(focus))
    }

    pub(super) fn with_edit_target_for_focus<R>(
        &mut self,
        focus: AppFocus,
        run: impl FnOnce(Option<EditTarget<'_>>) -> Result<(R, bool), CommandError>,
    ) -> Result<R, CommandError> {
        let mut registry_borrows = self.editor_targets.borrow_all_mut();
        let mut registry_matched: Option<usize> = None;
        let mut found = None;
        for (idx, borrow) in registry_borrows.iter_mut().enumerate() {
            if borrow.accepts_focus(focus) {
                if let Some(target) = borrow.edit_target(focus) {
                    registry_matched = Some(idx);
                    found = Some(target);
                }
                break;
            }
        }

        let (result, focused_field_changed) = run(found)?;
        if focused_field_changed && let Some(idx) = registry_matched {
            registry_borrows[idx].after_text_changed();
        }
        Ok(result)
    }
}

impl Default for TextTargetRuntime {
    fn default() -> Self {
        Self::new()
    }
}
