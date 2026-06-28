//! 命令派发管线。
//!
//! 本模块将按键/命令/交互意图收敛为统一的命令派发入口，
//! 并负责派发后的 session 状态落地（tab 切换、预览、git 刷新）。
//!
//! 所有方法都是 [`App`] 的 impl 块——通过 `self.` 访问组合根的各个协作者。

use crate::dispatch::KeyDispatchOutcome;
use crate::host_intent::{InteractionIntent, PointerIntent};
use crate::workspace_session::WorkspaceSession;
use zom_command::{
    CommandError, EditorEffect, GitEffect, HostEffect, Invocation, KeymapResolution,
};

use super::App;

impl App {
    /// 派发一次命令调用。
    ///
    /// 调用方应当来自 typed builder（如 `editor::insert_text("hi")`），
    /// 从而避免在调用点手写 `CommandId::new(...)` 或 `CommandArgs::new().with(...)`。
    pub(crate) fn dispatch_command(
        &mut self,
        invocation: Invocation,
    ) -> Result<Vec<HostEffect>, CommandError> {
        let (id, args) = invocation;
        let focus = self.focus.current();
        let viewport_anchor = self.capture_active_viewport_edit_anchor();

        let run_command = |focused_field: Option<zom_command::EditTarget<'_>>| {
            self.command
                .run_invocation(id.clone(), args.clone(), &mut self.session, focused_field)
        };

        let mut host_effects = self
            .text_targets
            .with_edit_target_for_focus(focus, run_command)?;

        // 命令派发可能编辑了活动 buffer（产生 DeltaEvent），扇出给 BufferSearch 与 syntax provider 是无条件的。
        // 否则编辑后高亮 / 搜索命中都不跟版本。
        self.background.after_text_edit(
            &mut self.session,
            self.config.soft_wrap_enabled(),
            viewport_anchor,
        );

        // 纯 session 状态变更类 effect（tab 切换 / 关闭）在 App 层就近消化，
        // 不动 GPUI / DockState / 焦点，不必走 actions.rs。
        host_effects.retain(|effect| match effect {
            HostEffect::Editor(EditorEffect::SelectTab(view_id)) => {
                self.session.set_active_view(*view_id);
                false
            }
            HostEffect::Editor(EditorEffect::OpenPreview(buffer_id)) => {
                self.session.open_preview(*buffer_id);
                false
            }
            HostEffect::Editor(EditorEffect::SelectAdjacentTab(forward)) => {
                select_adjacent_tab(&mut self.session, *forward);
                false
            }
            HostEffect::Editor(EditorEffect::CloseTab(view_id)) => {
                self.session.close_view(*view_id);
                false
            }
            HostEffect::Git(GitEffect::Refresh) => {
                let _ = self.git.borrow_mut().refresh();
                false
            }
            _ => true,
        });
        Ok(host_effects)
    }

    /// 派发一次设备交互意图。
    ///
    /// 交互管线不经过 command catalog / keymap，但它和命令一样会在修改编辑状态后
    /// 对齐 dismiss、清掉编辑合并状态，并由 shell 统一刷新。
    pub(crate) fn dispatch_interaction(
        &mut self,
        intent: InteractionIntent,
    ) -> Result<Vec<HostEffect>, CommandError> {
        match intent {
            InteractionIntent::Pointer(intent) => self.dispatch_pointer_interaction(intent)?,
        }
        self.command
            .reconcile_after_input_mutation(&mut self.session);
        Ok(Vec::new())
    }

    fn dispatch_pointer_interaction(&mut self, intent: PointerIntent) -> Result<(), CommandError> {
        match intent {
            PointerIntent::SetSelection {
                focus,
                anchor,
                head,
            } => {
                self.request_focus(focus);
                self.with_router_mut(|mut router| router.set_pointer_selection(focus, anchor, head))
            }
            PointerIntent::ScrollViewport {
                focus,
                delta_visual_rows,
            } => {
                self.request_focus(focus);
                self.with_router_mut(|mut router| router.scroll_viewport(focus, delta_visual_rows))
            }
        }
    }

    /// 处理一次归一化按键。
    ///
    /// 组合根按当前唯一焦点 / 运行态算出 `KeyContext` 栈交给 keymap 解析 ——
    /// 命令与快捷键的定义全在 zom-command，宿主不持有任何 chord → 动作 的映射表。
    pub(crate) fn dispatch_key(
        &mut self,
        chord: String,
    ) -> Result<KeyDispatchOutcome, CommandError> {
        let contexts = self.key_contexts();
        match self.command.resolve_key(chord, &contexts)? {
            KeymapResolution::Matched { command, args } => {
                let effects = self.dispatch_command((command, args))?;
                Ok(KeyDispatchOutcome {
                    consumed: true,
                    effects,
                })
            }
            KeymapResolution::Pending => Ok(KeyDispatchOutcome {
                consumed: true,
                effects: Vec::new(),
            }),
            KeymapResolution::NoMatch => Ok(KeyDispatchOutcome {
                consumed: false,
                effects: Vec::new(),
            }),
        }
    }

    fn capture_active_viewport_edit_anchor(
        &self,
    ) -> Option<zom_workspace::view::ViewportEditAnchor> {
        let view_id = self.session.active_edit_view_id()?;
        let view = self.session.views().edit_view(view_id)?;
        let buffer = self.session.workspace().buffer(view.buffer())?;
        view.capture_viewport_edit_anchor(buffer.buffer())
    }
}

/// tab 顺序 = ViewSet 的 ViewId 升序（编辑视图与预览视图共占同一序列），循环导航。
fn select_adjacent_tab(session: &mut WorkspaceSession, forward: bool) {
    let view_ids: Vec<_> = session.views().views().map(|(id, _)| id).collect();
    let total = view_ids.len();
    if total == 0 {
        return;
    }
    let current = session
        .active_view_id()
        .and_then(|vid| view_ids.iter().position(|id| *id == vid));
    let target = if forward {
        match current {
            Some(i) => (i + 1) % total,
            None => 0,
        }
    } else {
        match current {
            Some(i) => (i + total - 1) % total,
            None => 0,
        }
    };
    session.set_active_view(view_ids[target]);
}
