//! app —— 组合根（手册 2 / 13）。
//!
//! P2 最小编辑闭环已接入：组合根持有 `CommandRegistry`、`Keymap`、
//! `Workspace` 与 `ViewSet`，并把输入统一收敛到 command 管线。
//!
//! 依赖方向（手册 2.4）：`app` 可以 import `shell`；`shell` 不可反向 import `app`。
//! 本文件只做组合根职责；具体功能尽量回到各自 feature / editor / workbench。

use std::ops::Range;
use std::path::{Path, PathBuf};

use zom_command::commands::{
    editor, language_server as language_server_commands, overlay as overlay_commands,
    panels as panel_commands, window as window_commands, workspace as workspace_commands,
};
use zom_command::{
    CommandArgs, CommandContext, CommandError, CommandExecutor, CommandId, CommandQueue,
    CommandRegistry, EffectQueue, HostEffect, Invocation, KeyChord, Keymap, KeymapResolution,
};
use zom_view::{ViewId, ViewSet};
use zom_workspace::{EntryKind, Workspace, WorkspaceBuffer};

use crate::shell::editor::{
    EditorKeyOutcome, EditorLineMode, ImeQueryTarget, ImeTarget, is_editing_command,
};
use crate::shell::features::file_tree::{FileTreeActivation, FileTreeModel, FileTreeState};

/// 主编辑区渲染快照：标签列表 + 当前活动 buffer 的正文。
#[derive(Clone, Debug, Default)]
pub(crate) struct EditorState {
    pub(crate) tabs: Vec<EditorTab>,
    pub(crate) text: String,
    pub(crate) cursor_byte: usize,
}

/// 编辑区一个标签的渲染摘要。
#[derive(Clone, Debug)]
pub(crate) struct EditorTab {
    /// 对应的 View；后续切换 / 关闭命令用它定位。
    pub(crate) id: ViewId,
    /// 标签显示名（文件名，无路径的 scratch 显示「未命名」）。
    pub(crate) title: String,
    pub(crate) dirty: bool,
    pub(crate) is_active: bool,
}

/// 一次按键派发的结果。`consumed=false` 表示这次按键没有匹配任何 keymap
/// 绑定，应当透传给系统输入法；否则会阻塞 IME 的整个文本输入路径。
pub(crate) struct KeyDispatchOutcome {
    pub(crate) consumed: bool,
    pub(crate) effects: Vec<HostEffect>,
}

pub struct App {
    registry: CommandRegistry,
    keymap: Keymap,
    executor: CommandExecutor,
    queue: CommandQueue,
    workspace: Workspace,
    views: ViewSet,
    project_root: Option<PathBuf>,
    file_tree: FileTreeModel,
}

impl App {
    pub fn new() -> Self {
        let mut registry = CommandRegistry::new();
        let mut keymap = Keymap::new();

        // 全部命令 + 默认键位都集中在 zom-command::commands 里声明，组合根只
        // 选要装哪些 catalog。handler 看不到的宿主侧资源（窗口、Dock）走
        // HostEffect 反馈到 shell。
        editor::install(&mut registry, &mut keymap);
        overlay_commands::install(&mut registry, &mut keymap);
        language_server_commands::install(&mut registry, &mut keymap);
        workspace_commands::install(&mut registry, &mut keymap);
        window_commands::install(&mut registry, &mut keymap);
        panel_commands::install(&mut registry, &mut keymap);

        let (workspace, views) = empty_workspace();

        Self {
            registry,
            keymap,
            executor: CommandExecutor::new(),
            queue: CommandQueue::new(),
            workspace,
            views,
            project_root: None,
            file_tree: FileTreeModel::default(),
        }
    }

    pub(crate) fn open_local_project(&mut self, root: PathBuf) {
        self.file_tree.open_project(root.clone());
        self.project_root = Some(root);
        let (workspace, views) = empty_workspace();
        self.workspace = workspace;
        self.views = views;
    }

    pub(crate) fn project_title(&self) -> String {
        self.project_root
            .as_deref()
            .and_then(project_name)
            .unwrap_or("打开项目")
            .to_string()
    }

    pub(crate) fn has_project(&self) -> bool {
        self.project_root.is_some()
    }

    pub(crate) fn file_tree_state(&self) -> FileTreeState {
        self.file_tree.state(&self.workspace)
    }

    pub(crate) fn file_tree_ensure_selection_initialized(&mut self) {
        self.file_tree.ensure_selection_initialized();
    }

    pub(crate) fn file_tree_move_selection(&mut self, delta: isize) {
        self.file_tree.move_selection(delta);
    }

    pub(crate) fn file_tree_collapse_or_parent(&mut self) {
        self.file_tree.collapse_or_parent();
    }

    pub(crate) fn file_tree_expand_or_into(&mut self) {
        self.file_tree.expand_or_into();
    }

    pub(crate) fn file_tree_activate(&mut self) -> FileTreeActivation {
        self.file_tree
            .activate_selected(&mut self.workspace, &mut self.views)
    }

    pub(crate) fn file_tree_begin_new_entry(&mut self, kind: EntryKind) {
        self.file_tree.begin_new_entry(kind);
    }

    pub(crate) fn file_tree_pending_active(&self) -> bool {
        self.file_tree.pending_active()
    }

    pub(crate) fn file_tree_cancel_new_entry(&mut self) {
        self.file_tree.cancel_new_entry();
    }

    pub(crate) fn file_tree_commit_new_entry(&mut self) {
        self.file_tree.commit_new_entry();
    }

    pub(crate) fn editor_state(&self) -> EditorState {
        let tabs = self.editor_tabs();

        let Some(view) = self.views.active_view() else {
            return EditorState {
                tabs,
                ..EditorState::default()
            };
        };
        let Some(buffer) = self.workspace.buffer(view.buffer()) else {
            return EditorState {
                tabs,
                ..EditorState::default()
            };
        };

        EditorState {
            tabs,
            text: buffer.buffer().text().into_owned(),
            cursor_byte: view.selection().primary().head().get(),
        }
    }

    /// 把 `ViewSet` 里的每个视图映射成一个标签摘要，顺序即打开顺序。
    fn editor_tabs(&self) -> Vec<EditorTab> {
        let active = self.views.active();
        self.views
            .views()
            .map(|(id, view)| {
                let buffer = self.workspace.buffer(view.buffer());
                EditorTab {
                    id,
                    title: buffer
                        .map(buffer_title)
                        .unwrap_or_else(|| "未命名".to_string()),
                    dirty: buffer.map(WorkspaceBuffer::is_dirty).unwrap_or(false),
                    is_active: Some(id) == active,
                }
            })
            .collect()
    }

    /// 派发一次命令调用。
    ///
    /// 调用方应当来自 typed builder（如 `editor::insert_text("hi")`），从而避免
    /// 在调用点手写 `CommandId::new(...)` 或 `CommandArgs::new().with(...)`。
    pub(crate) fn dispatch(
        &mut self,
        invocation: Invocation,
    ) -> Result<Vec<HostEffect>, CommandError> {
        let (id, args) = invocation;
        self.dispatch_command_id(id, args)
    }

    /// 处理一次归一化按键。
    ///
    /// 文本输入不在这里 fallback：交给 GPUI 的 `EntityInputHandler` 路径，由
    /// 系统输入法或 NSTextInputClient 把文本喂给 `App::ime_*`。
    pub(crate) fn dispatch_key_input(
        &mut self,
        chord: String,
    ) -> Result<KeyDispatchOutcome, CommandError> {
        let chord = KeyChord::new(chord)?;
        match self.keymap.resolve(&[chord], &[]) {
            KeymapResolution::Matched { command, args } => {
                let effects = self.dispatch_command_id(command, args)?;
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

    /// 处理嵌入编辑器的按键。
    ///
    /// 这里故意只消费“文本编辑相关”的命令：移动 / 选择 / 删除 / undo /
    /// redo / IME 等。未命中的按键，以及命中但不属于编辑行为的快捷键，
    /// 都交回父组件处理。
    pub(crate) fn dispatch_embedded_editor_key_input(
        &mut self,
        chord: String,
        mode: EditorLineMode,
    ) -> Result<EditorKeyOutcome, CommandError> {
        if !self.file_tree.pending_active() {
            return Ok(EditorKeyOutcome::bubble());
        }

        if self.embedded_editor_is_composing() {
            return self.dispatch_composition_key(chord.as_str());
        }

        let chord = KeyChord::new(chord)?;
        match self.keymap.resolve(&[chord], &[]) {
            KeymapResolution::Matched { command, args } if is_editing_command(&command, mode) => {
                let effects = self.dispatch_command_id(command, args)?;
                Ok(EditorKeyOutcome::handled(effects))
            }
            KeymapResolution::Pending
            | KeymapResolution::Matched { .. }
            | KeymapResolution::NoMatch => Ok(EditorKeyOutcome::bubble()),
        }
    }

    fn embedded_editor_is_composing(&self) -> bool {
        self.file_tree
            .pending_editor()
            .map(|editor| editor.is_composing())
            .unwrap_or(false)
    }

    fn dispatch_composition_key(&mut self, chord: &str) -> Result<EditorKeyOutcome, CommandError> {
        match chord {
            "escape" => {
                let effects = self.dispatch(editor::ime_cancel())?;
                Ok(EditorKeyOutcome::handled(effects))
            }
            "enter" | "return" => {
                self.ime_unmark()?;
                Ok(EditorKeyOutcome::handled(Vec::new()))
            }
            _ => Ok(EditorKeyOutcome::handled(Vec::new())),
        }
    }

    /// 查询某条命令的快捷键文案 —— 给 Glyph / 命令面板 / 菜单用。
    pub(crate) fn shortcut_for(&self, command_id: &str) -> Option<String> {
        let command = CommandId::new(command_id).ok()?;
        self.keymap.format_shortcut_for(&command)
    }

    fn dispatch_command_id(
        &mut self,
        id: CommandId,
        args: CommandArgs,
    ) -> Result<Vec<HostEffect>, CommandError> {
        self.queue.dispatch(id, args);

        let mut effects = EffectQueue::new();
        let focused_field = self.file_tree.pending_edit_target();
        let mut context = CommandContext {
            workspace: &mut self.workspace,
            views: &mut self.views,
            focused_field,
            queue: &mut self.queue,
            effects: &mut effects,
        };
        let result = self.executor.run(&self.registry, &mut context);

        let host_effects = effects.drain();
        result?;
        Ok(host_effects)
    }

    /// 提交系统输入法文本。commit 走命令路径，保证进入 undo 历史。
    pub(crate) fn ime_replace_text(
        &mut self,
        replacement_range_utf16: Option<Range<usize>>,
        text: &str,
    ) -> Result<(), CommandError> {
        self.with_focused_ime_target(|mut target| {
            target.apply_replacement_range(replacement_range_utf16)
        })?;

        self.dispatch(editor::ime_commit(text))?;
        Ok(())
    }

    /// 更新输入法 preedit。update 走直接通道，避免每次按键都过命令队列。
    pub(crate) fn ime_replace_and_mark_text(
        &mut self,
        replacement_range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
    ) -> Result<(), CommandError> {
        self.with_focused_ime_target(|mut target| {
            target.replace_and_mark_text(
                replacement_range_utf16,
                new_text,
                new_selected_range_utf16,
            )
        })
    }

    pub(crate) fn ime_unmark(&mut self) -> Result<(), CommandError> {
        let preedit = self
            .focused_ime_query_target()
            .and_then(|target| target.preedit_text());
        let Some(preedit) = preedit else {
            return Ok(());
        };
        self.dispatch(editor::ime_commit(preedit))?;
        Ok(())
    }

    pub(crate) fn ime_marked_range_utf16(&self) -> Option<Range<usize>> {
        self.focused_ime_query_target()
            .and_then(|target| target.marked_range_utf16())
    }

    pub(crate) fn ime_selected_range_utf16(&self) -> Option<(Range<usize>, bool)> {
        self.focused_ime_query_target()
            .map(|target| target.selected_range_utf16())
    }

    pub(crate) fn ime_text_for_range_utf16(&self, range_utf16: Range<usize>) -> Option<String> {
        self.focused_ime_query_target()
            .and_then(|target| target.text_for_range_utf16(range_utf16))
    }

    fn with_focused_ime_target<R>(
        &mut self,
        f: impl FnOnce(ImeTarget<'_>) -> Result<R, CommandError>,
    ) -> Result<R, CommandError> {
        if let Some(pending_editor) = self.file_tree.pending_editor_mut() {
            return f(pending_editor.as_ime_target());
        }
        self.with_active_ime_target(f)
    }

    fn with_active_ime_target<R>(
        &mut self,
        f: impl FnOnce(ImeTarget<'_>) -> Result<R, CommandError>,
    ) -> Result<R, CommandError> {
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
        f(ImeTarget::new(buffer, selection))
    }

    fn focused_ime_query_target(&self) -> Option<ImeQueryTarget<'_>> {
        if let Some(pending_editor) = self.file_tree.pending_editor() {
            return Some(pending_editor.as_ime_query_target());
        }
        self.active_ime_query_target()
    }

    fn active_ime_query_target(&self) -> Option<ImeQueryTarget<'_>> {
        let view = self.views.active_view()?;
        let buffer = self.workspace.buffer(view.buffer())?.buffer();
        Some(ImeQueryTarget::new(buffer, view.selection()))
    }
}

/// 取 buffer 的标签显示名：有路径用文件名，无路径（scratch）显示「未命名」。
fn buffer_title(buffer: &WorkspaceBuffer) -> String {
    buffer
        .path()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "未命名".to_string())
}

/// 空工作区：不预建任何 buffer/view。
///
/// 早期版本会默认开一个空白 scratch buffer，但它对用户没有意义、还会让编辑区
/// 显示一个不存在的"文件"，误导用户。现在编辑区在无活动视图时走
/// `EditorState::default()`，文件从文件树打开后才有内容。
fn empty_workspace() -> (Workspace, ViewSet) {
    (Workspace::new(), ViewSet::new())
}

fn project_name(path: &Path) -> Option<&str> {
    path.file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}
