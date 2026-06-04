//! 文本目标路由中心。
//!
//! 主编辑区、搜索输入框、文件树新建/重命名、设置 TOML 等都通过同一套 `TextTargetOwner` / `TextTargetQuery` 接口参与命令与 IME 路由。
//! 本模块集中保存运行期注册表和 search runtime 的窄接口，`App` 只在需要路由时调用这里。

use std::cell::RefCell;
use std::rc::Rc;

use zom_command::{CommandError, EditTarget, KeyContext};

use crate::focus::AppFocus;
use crate::shell::editor::{
    EditorRouter, EditorRouterMut, EditorTargetRegistry, TextTargetOwner, TextTargetQuery,
};
use crate::shell::features::panels::search::SearchRuntimeHandle;
use crate::shell::workbench::editor_area;
use crate::workspace_session::WorkspaceSession;

pub(crate) struct TextTargetHub {
    search_runtime: Option<SearchRuntimeHandle>,
    editor_targets: EditorTargetRegistry,
}

impl TextTargetHub {
    pub(crate) fn new() -> Self {
        Self {
            search_runtime: None,
            editor_targets: EditorTargetRegistry::new(),
        }
    }

    pub(crate) fn install_editor_owner(&mut self, owner: Rc<RefCell<dyn TextTargetOwner>>) {
        self.editor_targets.register(owner);
    }

    pub(crate) fn install_search_runtime(&mut self, runtime: SearchRuntimeHandle) {
        self.search_runtime = Some(runtime);
    }

    pub(crate) fn with_router<R>(
        &self,
        session: &WorkspaceSession,
        f: impl FnOnce(EditorRouter<'_>) -> R,
    ) -> R {
        let main = editor_area::MainEditorOwnerRef::new(session.workspace(), session.views());
        let registry_borrows = self.editor_targets.borrow_all();

        if let Some(search) = self.search_runtime.as_ref() {
            return search.with_query_owners(|query, replacement| {
                let mut owners: Vec<&dyn TextTargetQuery> = vec![query, replacement];
                for borrow in registry_borrows.iter() {
                    owners.push(&**borrow as &dyn TextTargetQuery);
                }
                owners.push(&main as &dyn TextTargetQuery);
                f(EditorRouter::new(owners))
            });
        }

        let mut owners: Vec<&dyn TextTargetQuery> = Vec::new();
        for borrow in registry_borrows.iter() {
            owners.push(&**borrow as &dyn TextTargetQuery);
        }
        owners.push(&main as &dyn TextTargetQuery);
        f(EditorRouter::new(owners))
    }

    pub(crate) fn with_router_mut<R>(
        &mut self,
        focus: AppFocus,
        session: &mut WorkspaceSession,
        f: impl FnOnce(EditorRouterMut<'_>) -> R,
    ) -> R {
        let (workspace, views) = session.parts_mut();
        let mut main = editor_area::MainEditorOwner::new(workspace, views);
        let mut registry_borrows = self.editor_targets.borrow_all_mut();
        let run = |search_owner: Option<&mut dyn TextTargetOwner>| {
            let mut owners: Vec<&mut dyn TextTargetOwner> = Vec::new();
            if let Some(owner) = search_owner {
                owners.push(owner);
            }
            for borrow in registry_borrows.iter_mut() {
                owners.push(&mut **borrow as &mut dyn TextTargetOwner);
            }
            owners.push(&mut main as &mut dyn TextTargetOwner);
            f(EditorRouterMut::new(owners))
        };

        if let Some(search) = self.search_runtime.as_ref() {
            return search.with_active_owner(focus, |owner| run(Some(owner)));
        }

        run(None)
    }

    pub(crate) fn key_contexts_for(
        &self,
        session: &WorkspaceSession,
        focus: AppFocus,
    ) -> Option<Vec<KeyContext>> {
        self.with_router(session, |router| router.key_contexts_for(focus))
    }

    pub(crate) fn is_composing(&self, session: &WorkspaceSession, focus: AppFocus) -> bool {
        self.with_router(session, |router| router.is_composing(focus))
    }

    pub(crate) fn with_edit_target_for_focus<R>(
        &mut self,
        focus: AppFocus,
        run: impl FnOnce(Option<EditTarget<'_>>) -> Result<(R, bool), CommandError>,
    ) -> Result<R, CommandError> {
        if let Some(search) = self
            .search_runtime
            .as_ref()
            .filter(|runtime| runtime.accepts_focus(focus))
        {
            let (result, _focused_field_changed) = search.with_edit_target_for_focus(focus, run)?;
            return Ok(result);
        }

        let mut registry_borrows = self.editor_targets.borrow_all_mut();
        let mut registry_matched: Option<usize> = None;
        let mut found = None;
        for (idx, borrow) in registry_borrows.iter_mut().enumerate() {
            if borrow.accepts_focus(focus) {
                if let Some(target) = borrow.edit_target() {
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

impl Default for TextTargetHub {
    fn default() -> Self {
        Self::new()
    }
}
