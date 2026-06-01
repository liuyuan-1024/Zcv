//! 命令执行上下文、队列与执行器。

use std::collections::VecDeque;

use zom_view::ViewSet;
use zom_workspace::Workspace;

use crate::clipboard::ClipboardPort;
use crate::{CommandArgs, CommandError, CommandId, CommandRegistry, EffectQueue};

/// 命令执行上下文。
///
/// 具体结构体而非 trait —— `zom-command` 本就依赖 `workspace` / `view`，
/// 直接命名它们，不做 trait 抽象体操。
///
/// handler 接触不到的宿主资源（GPUI Window、shell DockState 等）通过
/// `effects` emit 出去，宿主在派发结束后翻译。详见 [`crate::effects`]。
pub struct CommandContext<'a> {
    pub workspace: &'a mut Workspace,
    pub views: &'a mut ViewSet,
    /// 聚焦的输入框编辑目标。`Some` 时编辑命令作用于它而非主编辑区
    /// —— 由宿主（组合根）在派发前按 GPUI 焦点决定。
    pub focused_field: Option<EditTarget<'a>>,
    pub queue: &'a mut CommandQueue,
    pub effects: &'a mut EffectQueue,
    /// 剪贴板端口：copy / cut / paste handler 读写宿主剪贴板。
    /// engine 不持有剪贴板状态，宿主在派发前注入实现（GPUI 适配器或
    /// 测试用 `MockClipboard`）。
    pub clipboard: &'a mut dyn ClipboardPort,
}

/// 一次编辑命令作用的目标：文本缓冲 + 选区。
///
/// 把编辑命令与「buffer / selection 存放在哪」解耦 —— 主编辑区是 workspace
/// buffer + view selection，输入框是各自私有的 buffer + selection。
/// 编辑 handler 只认这个目标，不再直接穿 `workspace` / `views` 结构。
pub struct EditTarget<'a> {
    pub buffer: &'a mut zom_engine::Buffer,
    pub selection: &'a mut zom_engine::SelectionSet,
}

impl<'a> CommandContext<'a> {
    /// 解析当前编辑命令的作用目标：
    /// 有聚焦输入框则作用于它，否则作用于主编辑区的活动视图。
    pub fn edit_target(&mut self) -> Result<EditTarget<'_>, CommandError> {
        if let Some(field) = &mut self.focused_field {
            return Ok(EditTarget {
                buffer: &mut *field.buffer,
                selection: &mut *field.selection,
            });
        }
        let buffer_id = self
            .views
            .active_view()
            .map(|view| view.buffer())
            .ok_or(CommandError::NoActiveView)?;
        let buffer = self
            .workspace
            .buffer_mut(buffer_id)
            .ok_or(CommandError::BufferNotFound(buffer_id))?
            .buffer_mut();
        let selection = self
            .views
            .active_view_mut()
            .ok_or(CommandError::NoActiveView)?
            .selection_mut();
        Ok(EditTarget { buffer, selection })
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

    pub fn dispatch(&mut self, id: CommandId, args: CommandArgs) {
        self.pending.push_back((id, args));
    }

    pub fn pop(&mut self) -> Option<(CommandId, CommandArgs)> {
        self.pending.pop_front()
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

/// 命令执行器：排空队列，对注册表 + 上下文执行。
#[derive(Default)]
pub struct CommandExecutor;

impl CommandExecutor {
    pub fn new() -> Self {
        Self
    }

    pub fn run(
        &self,
        registry: &CommandRegistry,
        context: &mut CommandContext<'_>,
    ) -> Result<(), CommandError> {
        while let Some((id, args)) = context.queue.pop() {
            let handler = registry
                .handler(&id)
                .ok_or_else(|| CommandError::UnknownCommand(id.clone()))?;
            handler(context, args)?;
        }

        Ok(())
    }
}

/// 取活动视图指向的 buffer id。
///
/// `editor.save` 等"作用于活动 buffer"的命令共用。
/// 需要 buffer + selection 一起编辑的请走 [`CommandContext::edit_target`]。
pub(crate) fn active_view_buffer_id(
    ctx: &CommandContext<'_>,
) -> Result<zom_workspace::BufferId, CommandError> {
    ctx.views
        .active_view()
        .map(|view| view.buffer())
        .ok_or(CommandError::NoActiveView)
}
