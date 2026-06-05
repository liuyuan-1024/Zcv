//! 命令运行时。
//!
//! 这里集中持有 `zom-command` 的注册表、keymap、executor、队列与剪贴板端口。
//! `App` 仍决定“当前焦点对应哪个文本目标”，但命令系统自己的状态不再散在组合根上。

use zom_command::commands;
use zom_command::{
    ClipboardPort, CommandArgs, CommandContext, CommandError, CommandExecutor, CommandId,
    CommandQueue, CommandRegistry, EffectQueue, HostEffect, KeyChord, KeyContext, Keymap,
    KeymapResolution, MockClipboard,
};

use crate::workspace_session::WorkspaceSession;

pub(super) struct CommandRuntime {
    registry: CommandRegistry,
    keymap: Keymap,
    executor: CommandExecutor,
    queue: CommandQueue,
    clipboard: Box<dyn ClipboardPort>,
}

impl CommandRuntime {
    pub(super) fn new() -> Self {
        let mut registry = zom_command::CommandRegistry::new();
        let mut keymap = Keymap::new();
        commands::install_all(&mut registry, &mut keymap);
        Self {
            registry,
            keymap,
            executor: CommandExecutor::new(),
            queue: CommandQueue::new(),
            clipboard: Box::new(MockClipboard::new()),
        }
    }

    pub(super) fn set_clipboard(&mut self, clipboard: Box<dyn ClipboardPort>) {
        self.clipboard = clipboard;
    }

    pub(super) fn resolve_key(
        &self,
        chord: String,
        contexts: &[KeyContext],
    ) -> Result<KeymapResolution, CommandError> {
        let chord = KeyChord::new(chord)?;
        Ok(self.keymap.resolve(&[chord], contexts))
    }

    pub(super) fn dispatch_command_id(
        &mut self,
        id: CommandId,
        args: CommandArgs,
        session: &mut WorkspaceSession,
        focused_field: Option<zom_command::EditTarget<'_>>,
    ) -> Result<(Vec<HostEffect>, bool), CommandError> {
        self.queue.dispatch(id, args);
        let field_version_before = focused_field
            .as_ref()
            .map(|target| target.buffer.snapshot().version());
        let mut effects = EffectQueue::new();
        let (workspace, views) = session.parts_mut();
        let mut context = CommandContext {
            workspace,
            views,
            focused_field,
            queue: &mut self.queue,
            effects: &mut effects,
            clipboard: &mut *self.clipboard,
        };
        let result = self.executor.run(&self.registry, &mut context);
        let focused_field_changed = match (context.focused_field.as_ref(), field_version_before) {
            (Some(target), Some(before)) => target.buffer.snapshot().version() != before,
            _ => false,
        };
        let host_effects = effects.drain();
        result?;
        Ok((host_effects, focused_field_changed))
    }

    pub(super) fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    pub(super) fn registry(&self) -> &CommandRegistry {
        &self.registry
    }
}

impl Default for CommandRuntime {
    fn default() -> Self {
        Self::new()
    }
}
