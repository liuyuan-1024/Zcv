//! 命令执行上下文、队列与执行器。

use std::collections::VecDeque;

use zom_engine::TransactionMergePolicy;
use zom_view::{EditView, ViewId, ViewSet, VisualPosition, WrapMap};
use zom_workspace::Workspace;

use crate::clipboard::ClipboardPort;
use crate::{CommandArgs, CommandError, CommandId, CommandRegistry, DismissStacks, EffectQueue};

/// 命令执行上下文。
///
/// 具体结构体而非 trait —— `zom-command` 本就依赖 `workspace` / `view`，直接命名它们，不做 trait 抽象体操。
///
/// handler 接触不到的宿主资源（GPUI Window、shell DockState 等）通过 `effects` emit 出去，宿主在派发结束后翻译。
/// 详见 [`crate::effects`]。
pub struct CommandContext<'a> {
    pub workspace: &'a mut Workspace,
    pub views: &'a mut ViewSet,
    /// 主编辑区当前活动**编辑** view 的 id。
    ///
    /// 由宿主（组合根）在派发前从 `WorkspaceSession::active_edit_view_id()` 取得并填入。
    /// `None` 表示当前没有活动编辑 view（如所有 tab 关闭或活动的是预览 tab）。
    /// 编辑命令通过 [`Self::active_view`] / [`Self::active_view_mut`] / [`Self::active_view_buffer`] 访问对应 view。
    pub active_view_id: Option<ViewId>,
    /// 聚焦的输入框编辑目标。`Some` 时编辑命令作用于它而非主编辑区 —— 由宿主（组合根）在派发前按 GPUI 焦点决定。
    pub focused_field: Option<EditTarget<'a>>,
    pub queue: &'a mut CommandQueue,
    pub effects: &'a mut EffectQueue,
    /// 剪贴板端口：copy / cut / paste handler 读写宿主剪贴板。
    /// engine 不持有剪贴板状态，宿主在派发前注入实现（GPUI 适配器或 headless noop 实现）。
    pub clipboard: &'a mut dyn ClipboardPort,
    /// 各上下文家族的 dismiss 栈：
    /// esc 路径与"begin/cancel/commit"瞬态命令通过 [`DismissStacks::push`] / [`DismissStacks::pop_top`] / [`DismissStacks::remove`] 协调,
    /// 避免在同一上下文里两个命令都静态绑 esc 造成冲突。详见 [`crate::dismiss`]。
    pub dismiss: &'a mut DismissStacks,
    /// 宿主在派发前决定的编辑历史合并策略；
    /// 命令只负责把它写入事务 metadata。
    pub edit_merge_policy: TransactionMergePolicy,
}

/// 一次编辑命令作用的目标：文本缓冲 + 选区。
///
/// 把编辑命令与「buffer / selection 存放在哪」解耦 —— 主编辑区是 workspace buffer + view selection，输入框是各自私有的 buffer + selection。
/// 编辑 handler 只认这个目标。
pub struct EditTarget<'a> {
    pub buffer: &'a mut zom_engine::Buffer,
    pub selection: &'a mut zom_engine::SelectionSet,
    /// 渲染端写入的视觉行模型；整篇同步，与本帧渲染解耦。
    /// 不软换行时所有行 breaks 为空，行为退化为按逻辑行——调用端不必区分。
    pub wrap_map: Option<&'a WrapMap>,
    /// primary caret 的视觉投影。
    ///
    /// Selection 只保存 byte；软换行边界处同一个 byte 可能是上一段行尾，也可能是下一段行首。
    /// 这里记录命令上一次落定的视觉位置；下次垂直移动从这里出发，不再去 WrapMap 重新猜 affinity。
    /// 横向移动、文本编辑、select-all、undo/redo、IME、cut/paste 等会调用 [`EditTarget::clear_visual_caret`] 清掉。
    pub visual_caret: Option<&'a mut Option<VisualPosition>>,
    /// 连续上下移动的 sticky 列；None 表示需要重新取列。
    pub goal_column: Option<&'a mut Option<u32>>,
}

impl<'a> CommandContext<'a> {
    /// 当前活动编辑 view 的不可变引用。
    /// `None` 表示无活动编辑 view 或活动 view 已不在 ViewSet 中。
    pub fn active_view(&self) -> Option<&EditView> {
        self.active_view_id.and_then(|id| self.views.edit_view(id))
    }

    /// 当前活动编辑 view 的可变引用。
    pub fn active_view_mut(&mut self) -> Option<&mut EditView> {
        self.active_view_id
            .and_then(move |id| self.views.edit_view_mut(id))
    }

    /// 当前活动编辑 view 关联的 buffer id。等价于 `active_view().map(EditView::buffer)`，
    /// 但避免 handler 端为了一个 BufferId 而借走整段 view。
    pub fn active_view_buffer(&self) -> Option<zom_workspace::BufferId> {
        self.active_view().map(|view| view.buffer())
    }

    /// 解析当前编辑命令的作用目标：
    /// 有聚焦输入框则作用于它，否则作用于主编辑区的活动视图。
    pub fn edit_target(&mut self) -> Result<EditTarget<'_>, CommandError> {
        if let Some(field) = &mut self.focused_field {
            return Ok(EditTarget {
                buffer: &mut *field.buffer,
                selection: &mut *field.selection,
                wrap_map: field.wrap_map,
                visual_caret: field.visual_caret.as_deref_mut(),
                goal_column: field.goal_column.as_deref_mut(),
            });
        }
        let view_id = self.active_view_id.ok_or(CommandError::NoActiveView)?;
        let buffer_id = self
            .views
            .edit_view(view_id)
            .map(|view| view.buffer())
            .ok_or(CommandError::NoActiveView)?;
        let buffer = self
            .workspace
            .buffer_mut(buffer_id)
            .ok_or(CommandError::BufferNotFound(buffer_id))?
            .buffer_mut();
        let view = self
            .views
            .edit_view_mut(view_id)
            .ok_or(CommandError::NoActiveView)?;
        let (selection, visual_caret, goal_column, wrap_map) = view.vertical_movement_state_mut();
        Ok(EditTarget {
            buffer,
            selection,
            wrap_map,
            visual_caret: Some(visual_caret),
            goal_column: Some(goal_column),
        })
    }
}

impl EditTarget<'_> {
    /// 设置当前编辑目标的 selection，并清除连续视觉移动状态。
    ///
    /// 鼠标 click/drag、select-all、clear-selection 等“直接指定 selection”的能力都应走这里，
    /// 从而保证 buffer 侧校验与 view/field 侧 selection 同步只在一处实现。
    pub fn set_selection(
        &mut self,
        selection: zom_engine::SelectionSet,
    ) -> Result<(), CommandError> {
        self.clear_visual_caret();
        self.set_selection_preserving_visual_state(selection)
    }

    /// 设置 selection 但保留/交由调用方维护视觉移动状态。
    ///
    /// 仅供软换行视觉移动这类命令使用：它们会在写入 selection 后显式更新
    /// `visual_caret` / `goal_column`，不能由通用入口提前清掉。
    pub fn set_selection_preserving_visual_state(
        &mut self,
        selection: zom_engine::SelectionSet,
    ) -> Result<(), CommandError> {
        self.buffer
            .set_selection(selection.clone())
            .map_err(|error| CommandError::ExecutionFailed(error.to_string()))?;
        *self.selection = selection;
        Ok(())
    }

    /// 清除 primary caret 的视觉投影与 sticky 列。
    ///
    /// 横向移动、编辑、select-all、undo/redo、IME、cut/paste 等离开"连续上下移动"语义时调用。
    pub fn clear_visual_caret(&mut self) {
        if let Some(caret) = self.visual_caret.as_deref_mut() {
            *caret = None;
        }
        if let Some(goal) = self.goal_column.as_deref_mut() {
            *goal = None;
        }
    }
}

/// 命令执行的产出，用于告知外壳后续动作（重绘、焦点变化等）。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CommandOutcome {}

/// 命令队列。
///
/// handler 想触发子命令时入队，执行器排空 —— 不重入。
#[derive(Clone, Debug, Default)]
pub struct CommandQueue {
    pending: VecDeque<(CommandId, CommandArgs)>,
}

impl CommandQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn enqueue(&mut self, id: CommandId, args: CommandArgs) {
        self.pending.push_back((id, args));
    }

    pub fn pop(&mut self) -> Option<(CommandId, CommandArgs)> {
        self.pending.pop_front()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// 排空命令队列：对注册表 + 上下文执行；handler 报错时也会先对齐 dismiss 栈再返回。
pub fn run(
    registry: &CommandRegistry,
    context: &mut CommandContext<'_>,
) -> Result<(), CommandError> {
    let result = drain(registry, context);
    reconcile_after_input_mutation(context);
    result
}

/// 对齐一次输入造成的运行时瞬态。
///
/// 命令 dispatch 结束后会调用它；
/// 宿主侧 interaction 管线如果直接修改了 selection / viewport，也应复用这里，避免 Esc dismiss 等运行态只跟命令路径同步。
pub fn reconcile_after_input_mutation(context: &mut CommandContext<'_>) {
    crate::commands::reconcile::after_dispatch(context);
}

fn drain(registry: &CommandRegistry, context: &mut CommandContext<'_>) -> Result<(), CommandError> {
    while let Some((id, args)) = context.queue.pop() {
        let handler = registry
            .handler(&id)
            .ok_or_else(|| CommandError::UnknownCommand(id.clone()))?;
        handler(context, args)?;
    }
    Ok(())
}

/// 取活动视图指向的 buffer id。
///
/// `editor.save` 等"作用于活动 buffer"的命令共用。
/// 需要 buffer + selection 一起编辑的请走 [`CommandContext::edit_target`]。
pub(crate) fn active_view_buffer_id(
    ctx: &CommandContext<'_>,
) -> Result<zom_workspace::BufferId, CommandError> {
    ctx.active_view_buffer().ok_or(CommandError::NoActiveView)
}
