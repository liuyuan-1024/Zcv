//! 命令运行时。
//!
//! 这里集中持有 `zom-command` 的注册表、keymap、executor、队列与剪贴板端口。
//! `App` 仍决定“当前焦点对应哪个文本目标”，但命令系统自己的状态不再散在组合根上。

use zom_command::commands;
use zom_command::commands::editor;
use zom_command::{
    ClipboardPort, CommandArgs, CommandContext, CommandError, CommandId, CommandQueue,
    CommandRegistry, DismissStacks, EditMergeKind, EffectQueue, HostEffect, KeyChord, KeyContext,
    Keymap, KeymapResolution, NoopClipboard,
};
use zom_engine::{BufferVersion, TransactionMergePolicy};

use crate::workspace_session::WorkspaceSession;

pub(super) struct CommandRuntime {
    registry: CommandRegistry,
    keymap: Keymap,
    queue: CommandQueue,
    clipboard: Box<dyn ClipboardPort>,
    dismiss: DismissStacks,
    edit_merge: Option<EditMergeState>,
}

impl CommandRuntime {
    pub(super) fn new() -> Self {
        let mut registry = zom_command::CommandRegistry::new();
        let mut keymap = Keymap::new();
        commands::install_all(&mut registry, &mut keymap);
        Self {
            registry,
            keymap,
            queue: CommandQueue::new(),
            clipboard: Box::new(NoopClipboard::new()),
            dismiss: DismissStacks::new(),
            edit_merge: None,
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

    pub(super) fn run_invocation(
        &mut self,
        id: CommandId,
        args: CommandArgs,
        session: &mut WorkspaceSession,
        focused_field: Option<zom_command::EditTarget<'_>>,
    ) -> Result<(Vec<HostEffect>, bool), CommandError> {
        let edit_kind = editor::edit_merge_kind(&id, &args);
        let target_before = edit_kind
            .as_ref()
            .and_then(|_| edit_target_before(session, focused_field.as_ref()));
        let edit_merge_policy = if let (Some(kind), Some(target)) = (&edit_kind, target_before)
            && self
                .edit_merge
                .as_ref()
                .is_some_and(|last| last.matches(target, kind))
        {
            TransactionMergePolicy::MergeWithPrevious
        } else {
            TransactionMergePolicy::Never
        };

        self.queue.enqueue(id, args);
        let field_version_before = focused_field
            .as_ref()
            .map(|target| target.buffer.snapshot().version());
        let mut effects = EffectQueue::new();
        let active_view_id = session.active_edit_view_id();
        let any_active_view_id = session.active_view_id();
        let (workspace, views) = session.parts_mut();
        let mut context = CommandContext {
            workspace,
            views,
            active_view_id,
            any_active_view_id,
            focused_field,
            queue: &mut self.queue,
            effects: &mut effects,
            clipboard: &mut *self.clipboard,
            dismiss: &mut self.dismiss,
            edit_merge_policy,
        };
        let result = zom_command::run(&self.registry, &mut context);
        let focused_field_changed = match (context.focused_field.as_ref(), field_version_before) {
            (Some(target), Some(before)) => target.buffer.snapshot().version() != before,
            _ => false,
        };
        let target_after = edit_kind
            .as_ref()
            .and_then(|_| edit_target_after(&context, target_before));
        if let Err(error) = result {
            self.edit_merge = None;
            return Err(error);
        }
        let host_effects = effects.drain();
        let changed_target = target_after
            .zip(target_before)
            .filter(|(after, before)| after.version != before.version);
        self.edit_merge = match (edit_kind, changed_target) {
            (Some(kind), Some((after, _))) => Some(EditMergeState {
                target: after.target,
                after_version: after.version,
                kind,
            }),
            _ => None,
        };
        Ok((host_effects, focused_field_changed))
    }

    pub(super) fn reconcile_after_input_mutation(&mut self, session: &mut WorkspaceSession) {
        self.edit_merge = None;

        let mut effects = EffectQueue::new();
        let active_view_id = session.active_edit_view_id();
        let any_active_view_id = session.active_view_id();
        let (workspace, views) = session.parts_mut();
        let mut context = CommandContext {
            workspace,
            views,
            active_view_id,
            any_active_view_id,
            focused_field: None,
            queue: &mut self.queue,
            effects: &mut effects,
            clipboard: &mut *self.clipboard,
            dismiss: &mut self.dismiss,
            edit_merge_policy: TransactionMergePolicy::Never,
        };
        zom_command::reconcile_after_input_mutation(&mut context);
    }

    pub(super) fn keymap(&self) -> &Keymap {
        &self.keymap
    }

    pub(super) fn registry(&self) -> &CommandRegistry {
        &self.registry
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EditTargetSnapshot {
    target: EditMergeTarget,
    version: BufferVersion,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EditMergeTarget(u64);

#[derive(Clone, Debug, Eq, PartialEq)]
struct EditMergeState {
    target: EditMergeTarget,
    after_version: BufferVersion,
    kind: EditMergeKind,
}

impl EditMergeState {
    fn matches(&self, target: EditTargetSnapshot, kind: &EditMergeKind) -> bool {
        self.target == target.target && self.after_version == target.version && &self.kind == kind
    }
}

fn edit_target_before(
    session: &WorkspaceSession,
    focused_field: Option<&zom_command::EditTarget<'_>>,
) -> Option<EditTargetSnapshot> {
    if let Some(target) = focused_field {
        return Some(EditTargetSnapshot {
            target: EditMergeTarget(target.buffer.id_u64()),
            version: target.buffer.version(),
        });
    }

    let view_id = session.active_edit_view_id()?;
    let buffer_id = session.views().edit_view(view_id)?.buffer();
    let buffer = session.workspace().buffer(buffer_id)?.buffer();
    Some(EditTargetSnapshot {
        target: EditMergeTarget(buffer.id_u64()),
        version: buffer.version(),
    })
}

fn edit_target_after(
    context: &CommandContext<'_>,
    before: Option<EditTargetSnapshot>,
) -> Option<EditTargetSnapshot> {
    let before = before?;
    if let Some(target) = context.focused_field.as_ref() {
        let snapshot = EditTargetSnapshot {
            target: EditMergeTarget(target.buffer.id_u64()),
            version: target.buffer.version(),
        };
        return (snapshot.target == before.target).then_some(snapshot);
    }

    let view_id = context.active_view_id?;
    let buffer_id = context.views.edit_view(view_id)?.buffer();
    let buffer = context.workspace.buffer(buffer_id)?.buffer();
    let snapshot = EditTargetSnapshot {
        target: EditMergeTarget(buffer.id_u64()),
        version: buffer.version(),
    };
    (snapshot.target == before.target).then_some(snapshot)
}

impl Default for CommandRuntime {
    fn default() -> Self {
        Self::new()
    }
}
