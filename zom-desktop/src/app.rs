//! app —— 组合根（手册 2 / 13）。
//!
//! P2 最小编辑闭环已接入：组合根持有 `CommandRegistry`、`Keymap`、
//! `Workspace` 与 `ViewSet`，并把输入统一收敛到 command 管线。
//!
//! 依赖方向（手册 2.4）：`app` 可以 import `shell`；`shell` 不可反向 import `app`。
//! 本文件只做组合根职责；具体功能尽量回到各自 feature / editor / workbench。

use std::ops::Range;
use std::path::PathBuf;

use zom_command::commands::{self, editor};
use zom_command::{
    CommandArgs, CommandContext, CommandError, CommandExecutor, CommandId, CommandQueue,
    CommandRegistry, EffectQueue, FileTreeKeyMode, HostEffect, Invocation, KeyChord, KeyContext,
    Keymap, KeymapResolution, SearchOption, SearchScope,
};
use zom_view::ViewSet;
use zom_workspace::Workspace;

use crate::shell::CommandCatalogItem;
use crate::shell::editor::{
    EditorRouter, EditorRouterMut, MainEditorOwner, MainEditorOwnerRef, TextInputProfile,
    TextTargetId, TextTargetOwner, TextTargetQuery,
};
use crate::shell::features::panels::file_tree::{FileTreeActivation, FileTreeModel, FileTreeState};
use crate::shell::features::panels::search::{SearchModel, SearchState};
use crate::shell::features::project_picker::{
    ProjectPickerActivation, ProjectPickerMode, ProjectPickerModel, ProjectPickerState,
    RecentProject, RecentProjects,
};
use crate::shell::workbench::state::{EditorState, build_editor_state};

/// 一次按键派发的结果。`consumed=false` 表示这次按键没有匹配任何 keymap
/// 绑定，应当透传给系统输入法；否则会阻塞 IME 的整个文本输入路径。
pub(crate) struct KeyDispatchOutcome {
    pub(crate) consumed: bool,
    pub(crate) effects: Vec<HostEffect>,
}

/// 一次按键来自哪个交互面。组合根据此 + 运行态算出 keymap 上下文栈，
/// 命令与快捷键的定义本身全在 zom-command。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum KeySurface {
    /// 主编辑区。
    Editor,
    /// 普通面板。暂未接入专属键位时，只响应全局快捷键。
    Panel,
    /// 文件树面板（含新建条目输入态）。
    FileTree,
    /// 项目选择器浮面。
    ProjectPicker,
}

pub struct App {
    registry: CommandRegistry,
    keymap: Keymap,
    executor: CommandExecutor,
    queue: CommandQueue,
    workspace: Workspace,
    views: ViewSet,
    project_root: Option<PathBuf>,
    recent_projects: RecentProjects,
    file_tree: FileTreeModel,
    project_picker: ProjectPickerModel,
    search: SearchModel,
}

impl App {
    /// 内存模式（测试版使用，避免污染真实目录）
    pub fn new() -> Self {
        Self::new_with_recent_projects_path(None)
    }

    /// 持久化模式（发行版使用）
    pub fn new_persistent() -> Self {
        Self::new_with_recent_projects_path(RecentProjects::default_path())
    }

    pub(crate) fn new_with_recent_projects_path(path: Option<PathBuf>) -> Self {
        let mut registry = CommandRegistry::new();
        let mut keymap = Keymap::new();

        // 组合根只选择安装内建命令集；具体 feature catalog 的完整性由
        // zom-command 自己维护。宿主侧资源（窗口、Dock）走 HostEffect 反馈到 shell。
        commands::install_all(&mut registry, &mut keymap);

        let (workspace, views) = empty_workspace();

        Self {
            registry,
            keymap,
            executor: CommandExecutor::new(),
            queue: CommandQueue::new(),
            workspace,
            views,
            project_root: None,
            recent_projects: RecentProjects::load(path),
            file_tree: FileTreeModel::default(),
            project_picker: ProjectPickerModel::new(),
            search: SearchModel::new(),
        }
    }

    pub(crate) fn open_local_project(&mut self, root: PathBuf) {
        self.open_project(root, None);
    }

    pub(crate) fn open_git_project(&mut self, root: PathBuf, repo: String) {
        self.open_project(root, Some(repo));
    }

    fn open_project(&mut self, root: PathBuf, repo: Option<String>) {
        self.file_tree.open_project(root.clone());
        self.recent_projects.remember(root.clone(), repo);
        self.project_root = Some(root);
        let (workspace, views) = empty_workspace();
        self.workspace = workspace;
        self.views = views;
    }

    pub(crate) fn project_title(&self) -> String {
        self.project_root
            .as_deref()
            .and_then(|path| path.file_name().and_then(|name| name.to_str()))
            .filter(|name| !name.is_empty())
            .unwrap_or("打开项目")
            .to_string()
    }

    pub(crate) fn has_project(&self) -> bool {
        self.project_root.is_some()
    }

    pub(crate) fn recent_projects(&self) -> Vec<RecentProject> {
        self.recent_projects.items().to_vec()
    }

    pub(crate) fn remove_recent_project(&mut self, id: &str) {
        self.recent_projects.remove(id);
        self.project_picker
            .clamp_selection(self.recent_projects.items());
    }

    pub(crate) fn project_picker_reset(&mut self, mode: ProjectPickerMode) {
        self.project_picker.reset(mode);
    }

    pub(crate) fn project_picker_deactivate(&mut self) {
        self.project_picker.deactivate();
    }

    pub(crate) fn project_picker_state(&self) -> ProjectPickerState {
        self.project_picker.state()
    }

    pub(crate) fn project_picker_selected_project_id(&self) -> Option<String> {
        self.project_picker
            .selected_project_id(self.recent_projects.items())
    }

    pub(crate) fn project_picker_move_selection(&mut self, delta: isize) {
        self.project_picker
            .move_selection(delta, self.recent_projects.items());
    }

    pub(crate) fn project_picker_activation(&self) -> ProjectPickerActivation {
        self.project_picker
            .activation(self.recent_projects.items())
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

    pub(crate) fn file_tree_begin_new_entry(&mut self) {
        self.file_tree.begin_new_entry();
    }

    pub(crate) fn file_tree_cancel_new_entry(&mut self) {
        self.file_tree.cancel_new_entry();
    }

    /// 提交新建条目。新建文件会被立即打开，返回的 [`FileTreeActivation`] 让
    /// shell 据此把焦点切到编辑器。
    pub(crate) fn file_tree_commit_new_entry(&mut self) -> FileTreeActivation {
        self.file_tree
            .commit_new_entry(&mut self.workspace, &mut self.views)
    }

    pub(crate) fn file_tree_request_delete(&mut self) {
        self.file_tree.request_delete();
    }

    pub(crate) fn file_tree_confirm_delete(&mut self) {
        self.file_tree
            .confirm_delete(&mut self.workspace, &mut self.views);
    }

    pub(crate) fn file_tree_cancel_delete(&mut self) {
        self.file_tree.cancel_delete();
    }

    pub(crate) fn editor_state(&self) -> EditorState {
        build_editor_state(&self.workspace, &self.views)
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
    /// 宿主只传「按键来自哪个交互面」，由组合根按当前焦点 / 运行态算出
    /// `KeyContext` 栈交给 keymap 解析 —— 命令与快捷键的定义全在 zom-command，
    /// 宿主不持有任何 chord → 动作 的映射表。
    ///
    /// 文本输入不在这里 fallback：交给 GPUI 的 `EntityInputHandler` 路径，由
    /// 系统输入法或 NSTextInputClient 把文本喂给 `App::ime_*`。
    pub(crate) fn dispatch_key(
        &mut self,
        chord: String,
        surface: KeySurface,
    ) -> Result<KeyDispatchOutcome, CommandError> {
        // 组合态下宿主完全让位给系统输入法：不解析、不消费、不 stop_propagation。
        // 一旦拦下某个键（如 Esc → ime_cancel），系统 IME 会话就和我们脱节，
        // 它会再吞掉一个后续按键 —— 表现为「取消候选后要多按一次 Esc 才退出
        // 新建」。组合的更新 / 提交 / 取消都由 IME 回调（`ime_*`）驱动。
        if self.is_composing() {
            return Ok(KeyDispatchOutcome {
                consumed: false,
                effects: Vec::new(),
            });
        }
        let chord = KeyChord::new(chord)?;
        let contexts = self.key_contexts(surface);
        match self.keymap.resolve(&[chord], &contexts) {
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

    /// 把「当前焦点面 + 运行态」映射成 keymap 解析用的 `KeyContext` 优先级栈。
    ///
    /// 这是宿主该做的事 —— 告诉 zom-command「现在处于什么上下文」；至于哪个
    /// chord 对应哪条命令，仍由各 catalog 注册进 keymap 的绑定决定。
    ///
    /// `composing` 恒为 `false`：`dispatch_key` 在组合态直接让位给系统输入法，
    /// 根本不会走到这里。组合上下文（`KeyContext::text_edit` 的第二参）保留
    /// 在签名里，待将来真有「宿主侧处理组合键」的需求再启用。
    fn key_contexts(&self, surface: KeySurface) -> Vec<KeyContext> {
        // 文本输入类 surface 的上下文栈，由当前聚焦的 owner.profile() 提供 ——
        // 嵌入点活跃判定与 profile 选择全在 editor 子系统里。
        let focused_text_profile = || {
            self.with_router(|router| router.focused_profile())
                .unwrap_or(TextInputProfile::MainEditor)
                .key_contexts()
        };
        match surface {
            KeySurface::Editor => focused_text_profile(),
            KeySurface::Panel if self.search.active() => focused_text_profile(),
            KeySurface::Panel => vec![KeyContext::global()],
            KeySurface::ProjectPicker if self.project_picker.active() => focused_text_profile(),
            KeySurface::ProjectPicker => vec![KeyContext::project_picker(), KeyContext::global()],
            KeySurface::FileTree if self.file_tree.pending_delete_active() => vec![
                // 删除确认弹窗打开中：只解析确认 / 取消，导航键全部冻结。
                KeyContext::file_tree(FileTreeKeyMode::PendingDelete),
                KeyContext::global(),
            ],
            KeySurface::FileTree if self.file_tree.pending_active() => focused_text_profile(),
            KeySurface::FileTree => vec![
                KeyContext::file_tree(FileTreeKeyMode::Navigate),
                KeyContext::global(),
            ],
        }
    }

    /// 当前聚焦的编辑目标是否处于「有 preedit 的」输入法组合态。
    ///
    /// 空 preedit 不算 —— 系统输入法取消候选后会把 preedit 清空、但 composition
    /// 壳可能仍在。若空壳也算组合态，`dispatch_key` 会一直让位、keymap 再也
    /// 不接管，后续 Esc 永远到不了 `cancel_new_entry`。
    fn is_composing(&self) -> bool {
        self.with_router(|router| router.is_composing())
    }

    /// 构造一次只读路由视图。
    ///
    /// 这是 editor 子系统访问 App 内部状态的唯一桥：调用方拿到 [`EditorRouter`]
    /// 后直接做 IME 查询 / focused target / snapshot 等 —— App 不再为每种查询
    /// 包一层方法。
    pub(crate) fn with_router<R>(&self, f: impl FnOnce(EditorRouter<'_>) -> R) -> R {
        let main = MainEditorOwnerRef::new(&self.workspace, &self.views);
        let search_query = self.search.query_owner();
        let search_replacement = self.search.replacement_owner();
        let owners: Vec<&dyn TextTargetQuery> = vec![
            &self.project_picker as &dyn TextTargetQuery,
            &self.file_tree as &dyn TextTargetQuery,
            &search_query as &dyn TextTargetQuery,
            &search_replacement as &dyn TextTargetQuery,
            &main as &dyn TextTargetQuery,
        ];
        f(EditorRouter::new(owners))
    }

    /// 构造一次可写路由视图。Owner 顺序即优先级：picker → file_tree pending →
    /// 主编辑区。
    pub(crate) fn with_router_mut<R>(&mut self, f: impl FnOnce(EditorRouterMut<'_>) -> R) -> R {
        let mut main = MainEditorOwner::new(&mut self.workspace, &mut self.views);
        let mut search = self.search.active_owner();
        let owners: Vec<&mut dyn TextTargetOwner> = vec![
            &mut self.project_picker as &mut dyn TextTargetOwner,
            &mut self.file_tree as &mut dyn TextTargetOwner,
            &mut search as &mut dyn TextTargetOwner,
            &mut main as &mut dyn TextTargetOwner,
        ];
        f(EditorRouterMut::new(owners))
    }

    /// 查询某条命令的快捷键文案 —— 给 Glyph / 命令面板 / 菜单用。
    pub(crate) fn shortcut_for(&self, command_id: &str) -> Option<String> {
        let command = CommandId::new(command_id).ok()?;
        self.keymap.format_shortcut_for(&command)
    }

    /// 查询某条命令的显示标题 —— UI 不再为命令入口重复维护文案。
    pub(crate) fn command_title_for(&self, command_id: &str) -> Option<String> {
        let command = CommandId::new(command_id).ok()?;
        self.registry
            .command(&command)
            .map(|command| command.title.clone())
    }

    pub(crate) fn command_catalog_items(&self) -> Vec<CommandCatalogItem> {
        self.registry
            .commands()
            .map(|command| CommandCatalogItem {
                command_id: command.id.to_string(),
                title: command.title.clone(),
                description: command.description.clone(),
                visible_in_shortcuts: command.visible_in_shortcuts,
            })
            .collect()
    }

    pub(crate) fn search_state(&self) -> SearchState {
        self.search.state()
    }

    pub(crate) fn search_activate(&mut self, target: TextTargetId) {
        self.search.activate(target);
    }

    pub(crate) fn search_deactivate(&mut self, target: TextTargetId) {
        self.search.deactivate(target);
    }

    pub(crate) fn search_set_scope(&mut self, scope: SearchScope) {
        self.search.set_scope(scope, &self.workspace, &self.views);
    }

    pub(crate) fn search_toggle_option(&mut self, option: SearchOption) {
        self.search
            .toggle_option(option, &self.workspace, &self.views);
    }

    pub(crate) fn search_find_next(&mut self) {
        self.search.find_next(&mut self.workspace, &mut self.views);
    }

    pub(crate) fn search_find_previous(&mut self) {
        self.search
            .find_previous(&mut self.workspace, &mut self.views);
    }

    pub(crate) fn search_replace_next(&mut self) {
        self.search
            .replace_next(&mut self.workspace, &mut self.views);
    }

    pub(crate) fn search_replace_all(&mut self) {
        self.search
            .replace_all(&mut self.workspace, &mut self.views);
    }

    fn dispatch_command_id(
        &mut self,
        id: CommandId,
        args: CommandArgs,
    ) -> Result<Vec<HostEffect>, CommandError> {
        self.queue.dispatch(id, args);

        let mut effects = EffectQueue::new();
        // focused_field：当前活跃的内嵌输入框（picker > file_tree pending）。
        // 主编辑区不在这里走 —— `CommandContext::edit_target` 在 `focused_field`
        // 为 `None` 时自然 fallback 到 workspace + view，所以这里只问内嵌 owner。
        // 直接对 self 做分字段借用，避免引入封装方法把借用扩到整个 self。
        let focused_field = if self.project_picker.is_active() {
            self.project_picker.edit_target()
        } else if self.file_tree.is_active() {
            self.file_tree.edit_target()
        } else if self.search.active() {
            self.search.edit_target()
        } else {
            None
        };
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
        if self.project_picker.active() {
            self.project_picker.reset_selection();
        }
        // IME / 命令路径都汇到这里，搜索框文本变了就重跑一次搜索，避免在
        // 每个 owner 单独埋钩子。
        self.search
            .refresh_if_query_changed(&self.workspace, &self.views);
        Ok(host_effects)
    }

    /// 提交系统输入法文本。commit 走命令路径，保证进入 undo 历史。
    ///
    /// 写入成功后由 router 调 owner 的 `after_text_changed` 钩子 —— picker
    /// 等需要"文本变了就重置选区"的 owner 自己实现，宿主不必特判。
    pub(crate) fn ime_replace_text_for(
        &mut self,
        target_id: TextTargetId,
        replacement_range_utf16: Option<Range<usize>>,
        text: &str,
    ) -> Result<(), CommandError> {
        self.with_router_mut(|router| {
            router.with_ime_target(target_id, |mut target| {
                target.apply_replacement_range(replacement_range_utf16)
            })
        })?;

        self.dispatch(editor::ime_commit(text))?;
        Ok(())
    }

    /// 更新输入法 preedit。update 走直接通道，避免每次按键都过命令队列。
    pub(crate) fn ime_replace_and_mark_text_for(
        &mut self,
        target_id: TextTargetId,
        replacement_range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
    ) -> Result<(), CommandError> {
        let result = self.with_router_mut(|router| {
            router.with_ime_target(target_id, |mut target| {
                target.replace_and_mark_text(
                    replacement_range_utf16,
                    new_text,
                    new_selected_range_utf16,
                )
            })
        });
        // preedit 期间也走 live search —— 用户能边输入边看到结果收敛。
        self.search
            .refresh_if_query_changed(&self.workspace, &self.views);
        result
    }

    pub(crate) fn ime_unmark_for(&mut self, target_id: TextTargetId) -> Result<(), CommandError> {
        let Some(preedit) = self.with_router(|router| router.preedit_text(target_id)) else {
            return Ok(());
        };
        self.dispatch(editor::ime_commit(preedit))?;
        Ok(())
    }

}

/// 空工作区：不预建任何 buffer/view。
///
/// 早期版本会默认开一个空白 scratch buffer，但它对用户没有意义、还会让编辑区
/// 显示一个不存在的"文件"，误导用户。现在编辑区在无活动视图时走
/// `EditorState::default()`，文件从文件树打开后才有内容。
fn empty_workspace() -> (Workspace, ViewSet) {
    (Workspace::new(), ViewSet::new())
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    //! `App` 派发管线的 headless 单元测试。
    //!
    //! 这一层不接触 GPUI ——只覆盖 keymap 解析、命令派发、IME 流，以及
    //! 命令产出的 HostEffect。需要 GPUI 句柄（Entity / Window / 焦点等）的链路
    //! 在 `shell::view` 那一层做手工 / 集成测试，不进本文件。

    use crate::app::{App, KeySurface};
    use crate::shell::editor::{EditorSnapshot, TextTargetId};
    use crate::shell::features::panels::PanelId;
    use crate::shell::features::panels::file_tree::FileTreeActivation;
    use crate::shell::workbench::state::{EditorState, EditorTab};
    use std::fs::{File, create_dir_all};
    use std::path::PathBuf;
    use zom_command::commands::{
        diagnostics, editor, language_servers, project_picker as project_picker_commands, settings,
    };
    use zom_command::{HostEffect, SearchOption, SearchScope};
    use zom_workspace::EntryKind;

    /// 主编辑区文本 / 光标快照——断言"正文里到底是什么"用。
    fn main_snapshot(app: &App) -> EditorSnapshot {
        app.with_router(|router| router.snapshot_for(TextTargetId::MainEditor))
    }

    /// 文件树新建条目输入框的快照——断言用户键入的名称。
    fn pending_name_snapshot(app: &App) -> EditorSnapshot {
        app.with_router(|router| router.snapshot_for(TextTargetId::FileTreePendingName))
    }

    /// 某 target 是否处于"有 preedit"的 IME 组合态。
    fn has_marked_range(app: &App, target: TextTargetId) -> bool {
        app.with_router(|router| router.marked_range_utf16(target))
            .is_some()
    }

    /// 取当前活动标签——断言「编辑区正在显示哪个文件」用。
    fn active_tab(state: &EditorState) -> &EditorTab {
        state
            .tabs
            .iter()
            .find(|tab| tab.is_active)
            .expect("应有活动标签")
    }

    /// 构造一个已打开项目并激活了一个空文件的 `App`。
    ///
    /// 不再有默认空白 buffer，编辑管线测试必须先真实打开一个文件才有活动 buffer。
    /// 复用 `project_fixture`：rows 为 `[root, src, README.md]`，走到 README.md
    /// 并 activate。
    fn app_with_open_file(name: &str) -> App {
        let mut app = App::new();
        app.open_local_project(project_fixture(name));
        app.file_tree_move_selection(1); // root
        app.file_tree_move_selection(1); // src
        app.file_tree_move_selection(1); // README.md
        assert_eq!(app.file_tree_activate(), FileTreeActivation::OpenedFile);
        app
    }

    #[test]
    fn ime_and_key_input_should_drive_active_buffer_through_command_pipeline() {
        let mut app = app_with_open_file("ime-key");

        // 普通文本输入走 IME 通道（系统输入法或键盘的 NSTextInputClient 提交）。
        app.ime_replace_text_for(TextTargetId::MainEditor, None, "h")
            .unwrap();
        app.ime_replace_text_for(TextTargetId::MainEditor, None, "i")
            .unwrap();

        let state = app.editor_state();
        let snap = main_snapshot(&app);
        assert_eq!(snap.text, "hi");
        assert_eq!(snap.cursor_byte, 2);
        assert!(active_tab(&state).dirty);

        // 非文本按键仍走 keymap → 命令。
        assert!(
            app.dispatch_key("left".to_string(), KeySurface::Editor)
                .unwrap()
                .consumed
        );
        assert!(
            app.dispatch_key("backspace".to_string(), KeySurface::Editor)
                .unwrap()
                .consumed
        );

        let snap = main_snapshot(&app);
        assert_eq!(snap.text, "i");
        assert_eq!(snap.cursor_byte, 0);

        let outcome = app
            .dispatch_key("mod-z".to_string(), KeySurface::Editor)
            .unwrap();
        assert!(outcome.consumed);

        // 没绑定的字符必须返回未消费，让 IME 路径接管。
        assert!(
            !app.dispatch_key("a".to_string(), KeySurface::Editor)
                .unwrap()
                .consumed
        );

        let snap = main_snapshot(&app);
        assert_eq!(snap.text, "hi");
        assert_eq!(snap.cursor_byte, 1);
    }

    #[test]
    fn ime_preedit_update_and_commit_should_flow_through_engine() {
        let mut app = app_with_open_file("ime-preedit");

        // 先输入一个英文字符，确认 IME commit 走单独路径。
        app.ime_replace_text_for(TextTargetId::MainEditor, None, "x")
            .unwrap();

        // 模拟输入法 preedit：先 mark "ni"，再 mark "你"，最后 commit "你"。
        app.ime_replace_and_mark_text_for(TextTargetId::MainEditor, None, "ni", Some(2..2))
            .unwrap();
        assert_eq!(main_snapshot(&app).text, "xni");
        assert!(has_marked_range(&app, TextTargetId::MainEditor));

        app.ime_replace_and_mark_text_for(TextTargetId::MainEditor, None, "你", Some(1..1))
            .unwrap();
        assert_eq!(main_snapshot(&app).text, "x你");

        app.ime_replace_text_for(TextTargetId::MainEditor, None, "你")
            .unwrap();
        let snap = main_snapshot(&app);
        assert_eq!(snap.text, "x你");
        assert!(!has_marked_range(&app, TextTargetId::MainEditor));
        // commit 之后 cursor 落在 "你" 之后，对应 4 个 UTF-8 字节 + 1 (x)。
        assert_eq!(snap.cursor_byte, 1 + "你".len());

        // selected_range_utf16 用 UTF-16 计数：x 占 1，你 占 1，总长 2。
        let (sel, _) = app
            .with_router(|router| router.selected_range_utf16(TextTargetId::MainEditor))
            .unwrap();
        assert_eq!(sel, 2..2);
    }

    #[test]
    fn tab_and_enter_should_dispatch_editor_commands() {
        let mut app = app_with_open_file("tab-enter");

        assert!(
            app.dispatch_key("tab".to_string(), KeySurface::Editor)
                .unwrap()
                .consumed
        );
        assert!(
            app.dispatch_key("enter".to_string(), KeySurface::Editor)
                .unwrap()
                .consumed
        );
        assert!(
            app.dispatch_key("return".to_string(), KeySurface::Editor)
                .unwrap()
                .consumed
        );

        let state = app.editor_state();
        let snap = main_snapshot(&app);
        assert_eq!(snap.text, "    \n\n");
        assert_eq!(snap.cursor_byte, 6);
        assert!(active_tab(&state).dirty);
    }

    #[test]
    fn panel_toggle_command_should_emit_host_effect() {
        let mut app = App::new();

        // 命中 mod-shift-e → editor 区按下时应被 keymap 消费。
        let outcome = app
            .dispatch_key("mod-shift-e".to_string(), KeySurface::Editor)
            .expect("派发成功");
        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![HostEffect::TogglePanel("file_tree".to_string())]
        );
    }

    #[test]
    fn search_shortcuts_should_open_requested_scope_from_editor() {
        let mut app = App::new();

        let outcome = app
            .dispatch_key("mod-f".to_string(), KeySurface::Editor)
            .expect("派发成功");
        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![HostEffect::SearchActivateScope(SearchScope::CurrentFile)]
        );

        let outcome = app
            .dispatch_key("mod-shift-f".to_string(), KeySurface::Editor)
            .expect("派发成功");
        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![HostEffect::SearchActivateScope(SearchScope::Project)]
        );
    }

    #[test]
    fn panel_key_surface_should_keep_global_shortcuts_without_text_edit_context() {
        let mut app = App::new();

        let outcome = app
            .dispatch_key("mod-shift-e".to_string(), KeySurface::Panel)
            .expect("派发成功");
        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![HostEffect::TogglePanel("file_tree".to_string())]
        );

        let outcome = app
            .dispatch_key("mod-a".to_string(), KeySurface::Panel)
            .expect("派发成功");
        assert!(!outcome.consumed);
        assert!(outcome.effects.is_empty());
    }

    #[test]
    fn search_field_should_route_text_editing_through_panel_surface() {
        let mut app = App::new();
        app.search_activate(TextTargetId::SearchQuery);

        app.ime_replace_text_for(TextTargetId::SearchQuery, None, "needle")
            .unwrap();
        assert_eq!(app.search_state().query.text, "needle");

        let outcome = app
            .dispatch_key("backspace".to_string(), KeySurface::Panel)
            .expect("派发成功");
        assert!(outcome.consumed);
        assert_eq!(app.search_state().query.text, "needl");

        // 下一个匹配现在绑 down 键（替代旧的 enter）；按 enter 在搜索框里
        // 没有意义，预期不被消费。
        let outcome = app
            .dispatch_key("down".to_string(), KeySurface::Panel)
            .expect("派发成功");
        assert!(outcome.consumed);
        assert_eq!(outcome.effects, vec![HostEffect::SearchFindNext]);
        assert_eq!(app.search_state().query.text, "needl");

        let outcome = app
            .dispatch_key("tab".to_string(), KeySurface::Panel)
            .expect("派发成功");
        assert!(outcome.consumed);
        assert_eq!(outcome.effects, vec![HostEffect::SearchFocusNextField]);
        assert_eq!(app.search_state().query.text, "needl");

        let outcome = app
            .dispatch_key("mod-shift-f".to_string(), KeySurface::Panel)
            .expect("派发成功");
        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![HostEffect::SearchActivateScope(SearchScope::Project)]
        );

        app.search_set_scope(SearchScope::Project);
        assert_eq!(app.search_state().scope, SearchScope::Project);

        let outcome = app
            .dispatch_key("mod-f".to_string(), KeySurface::Panel)
            .expect("派发成功");
        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![HostEffect::SearchActivateScope(SearchScope::CurrentFile)]
        );

        let outcome = app
            .dispatch_key("alt-c".to_string(), KeySurface::Panel)
            .expect("派发成功");
        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![HostEffect::SearchToggleOption(SearchOption::CaseSensitive)]
        );

        app.search_toggle_option(SearchOption::CaseSensitive);
        assert!(app.search_state().options.case_sensitive);
        app.search_toggle_option(SearchOption::CaseSensitive);
        assert!(!app.search_state().options.case_sensitive);
    }

    #[test]
    fn shortcut_for_should_return_formatted_keymap_binding() {
        let app = App::new();

        // 已绑定的命令：返回格式化后的快捷键。
        let undo = app.shortcut_for(editor::UNDO).expect("undo 必有快捷键");
        let save = app.shortcut_for(editor::SAVE).expect("save 必有快捷键");
        let file_tree = app
            .shortcut_for(PanelId::FileTree.toggle_command_id())
            .expect("file_tree 切换必有快捷键");

        // 平台差异化校验在专门的格式化测试里做；这里只关心"能查到、非空"。
        assert!(!undo.is_empty());
        assert!(!save.is_empty());
        assert!(!file_tree.is_empty());

        let settings = app
            .shortcut_for(settings::OPEN)
            .expect("settings.open 必有快捷键");
        assert!(!settings.is_empty());

        // 未注册的命令：返回 None。
        assert!(app.shortcut_for("不存在的命令").is_none());
    }

    #[test]
    fn command_title_for_should_read_registered_command_metadata() {
        let app = App::new();

        assert_eq!(
            app.command_title_for(project_picker_commands::SHOW_PROJECTS_PICKER)
                .as_deref(),
            Some("切换项目")
        );
        assert_eq!(
            app.command_title_for(PanelId::FileTree.toggle_command_id())
                .as_deref(),
            Some("文件树")
        );

        assert_eq!(
            app.command_title_for(settings::OPEN).as_deref(),
            Some("设置")
        );
        assert_eq!(
            app.command_title_for(diagnostics::SHOW_PROBLEMS).as_deref(),
            Some("诊断")
        );
    }

    #[test]
    fn project_picker_command_should_emit_open_surface_window_action() {
        let mut app = App::new();

        let outcome = app
            .dispatch_key("mod-o".to_string(), KeySurface::Editor)
            .unwrap();

        assert!(outcome.consumed);
        assert_eq!(outcome.effects, vec![HostEffect::ShowProjectPicker]);
    }

    #[test]
    fn open_local_project_command_should_emit_window_action() {
        let mut app = App::new();

        let actions = app
            .dispatch(project_picker_commands::open_local_project())
            .unwrap();

        assert_eq!(actions, vec![HostEffect::OpenLocalProject]);
    }

    #[test]
    fn project_action_commands_should_have_shortcuts_and_emit_effects() {
        let mut app = App::new();

        assert!(
            app.shortcut_for(project_picker_commands::OPEN_LOCAL_PROJECT)
                .is_some()
        );
        assert!(
            app.shortcut_for(project_picker_commands::START_GIT_CLONE)
                .is_some()
        );
        assert!(
            app.shortcut_for(project_picker_commands::REMOVE_RECENT_PROJECT)
                .is_some()
        );

        let outcome = app
            .dispatch_key("down".to_string(), KeySurface::ProjectPicker)
            .unwrap();
        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![HostEffect::ProjectPickerMoveSelection(1)]
        );

        let outcome = app
            .dispatch_key("backspace".to_string(), KeySurface::ProjectPicker)
            .unwrap();
        assert!(!outcome.consumed);
        assert!(outcome.effects.is_empty());

        let outcome = app
            .dispatch_key("enter".to_string(), KeySurface::ProjectPicker)
            .unwrap();
        assert!(outcome.consumed);
        assert_eq!(outcome.effects, vec![HostEffect::ProjectPickerActivate]);

        let actions = app
            .dispatch(project_picker_commands::start_git_clone())
            .unwrap();
        assert_eq!(actions, vec![HostEffect::StartGitClone]);

        let actions = app
            .dispatch(project_picker_commands::remove_recent_project())
            .unwrap();
        assert_eq!(actions, vec![HostEffect::RemoveSelectedRecentProject]);
    }

    #[test]
    fn project_title_should_prompt_when_no_project_is_open() {
        let app = App::new();

        assert_eq!(app.project_title(), "打开项目");
    }

    #[test]
    fn open_local_project_should_update_project_title_and_reset_workspace() {
        let mut app = app_with_open_file("reset");
        app.ime_replace_text_for(TextTargetId::MainEditor, None, "临时内容")
            .unwrap();
        assert!(!main_snapshot(&app).text.is_empty());

        app.open_local_project(PathBuf::from("/tmp/zom-local-project"));

        assert_eq!(app.project_title(), "zom-local-project");
        // 重开项目后工作区清空：没有默认 buffer / 视图，也就没有任何标签。
        assert!(app.editor_state().tabs.is_empty());
        assert!(main_snapshot(&app).text.is_empty());
    }

    #[test]
    fn opening_projects_should_maintain_recent_project_records() {
        let mut app = App::new();
        let local = project_fixture("recent-local");
        let cloned = project_fixture("recent-git");

        app.open_local_project(local.clone());
        app.open_git_project(
            cloned.clone(),
            "https://example.com/org/recent-git.git".to_string(),
        );

        let recent = app.recent_projects();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].path, cloned);
        assert_eq!(
            recent[0].identifier,
            "https://example.com/org/recent-git.git"
        );
        assert_eq!(recent[1].path, local);

        let id = recent[0].id.clone();
        app.remove_recent_project(&id);
        let recent = app.recent_projects();
        assert_eq!(recent.len(), 1);
        assert_eq!(recent[0].path, local);
    }

    #[test]
    fn recent_projects_should_persist_to_file() {
        let store = std::env::temp_dir().join(format!(
            "zom-recent-projects-{}-{}.toml",
            std::process::id(),
            "persist"
        ));
        let _ = std::fs::remove_file(&store);
        let local = project_fixture("persist-local");
        let cloned = project_fixture("persist-git");

        {
            let mut app = App::new_with_recent_projects_path(Some(store.clone()));
            app.open_local_project(local.clone());
            app.open_git_project(
                cloned.clone(),
                "https://example.com/org/persist-git.git".to_string(),
            );
        }

        let app = App::new_with_recent_projects_path(Some(store.clone()));
        let recent = app.recent_projects();
        assert_eq!(recent.len(), 2);
        assert_eq!(recent[0].path, cloned);
        assert_eq!(
            recent[0].repo.as_deref(),
            Some("https://example.com/org/persist-git.git")
        );
        assert_eq!(recent[1].path, local);

        let _ = std::fs::remove_file(store);
    }

    #[test]
    fn language_server_status_command_should_emit_open_surface_window_action() {
        let mut app = App::new();

        let actions = app.dispatch(language_servers::open_status()).unwrap();

        assert_eq!(actions, vec![HostEffect::ShowLanguageServers]);
    }

    #[test]
    fn settings_and_diagnostics_commands_should_be_registered() {
        let mut app = App::new();

        let actions = app.dispatch(settings::open()).unwrap();
        assert_eq!(actions, vec![HostEffect::ShowSettings]);

        let actions = app.dispatch(diagnostics::show_problems()).unwrap();
        assert_eq!(actions, vec![HostEffect::ShowDiagnostics]);
    }

    #[test]
    fn escape_should_dispatch_surface_dismiss_command() {
        let mut app = App::new();

        let outcome = app
            .dispatch_key("escape".to_string(), KeySurface::Editor)
            .unwrap();

        assert!(outcome.consumed);
        assert_eq!(outcome.effects, vec![HostEffect::DismissSurface]);
    }

    fn project_fixture(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("zom-file-tree-app-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        create_dir_all(dir.join("src/inner")).unwrap();
        File::create(dir.join("README.md")).unwrap();
        File::create(dir.join("src/lib.rs")).unwrap();
        File::create(dir.join("src/inner/mod.rs")).unwrap();
        dir
    }

    #[test]
    fn file_tree_move_selection_should_walk_visible_rows_in_order() {
        let mut app = App::new();
        app.open_local_project(project_fixture("move"));

        assert!(app.file_tree_state().selected.is_none());

        // rows: [root, src, README.md]
        app.file_tree_move_selection(1);
        let state = app.file_tree_state();
        assert_eq!(state.selected.as_ref(), Some(&state.rows[0].path));

        app.file_tree_move_selection(1);
        let state = app.file_tree_state();
        assert_eq!(state.selected.as_ref(), Some(&state.rows[1].path));

        app.file_tree_move_selection(1);
        let state = app.file_tree_state();
        assert_eq!(state.selected.as_ref(), Some(&state.rows[2].path));

        // 已在末位时再 down 不会越界。
        app.file_tree_move_selection(1);
        let state = app.file_tree_state();
        assert_eq!(state.selected.as_ref(), Some(&state.rows[2].path));
    }

    #[test]
    fn file_tree_focus_initialization_should_select_first_visible_row() {
        let mut app = App::new();
        app.open_local_project(project_fixture("focus-init"));

        app.file_tree_ensure_selection_initialized();

        let state = app.file_tree_state();
        assert_eq!(state.selected.as_ref(), Some(&state.rows[0].path));
    }

    #[test]
    fn file_tree_expand_then_collapse_should_round_trip_via_selection_keys() {
        let mut app = App::new();
        let root = project_fixture("expand");
        app.open_local_project(root.clone());

        // 初始 rows: [root, src, README.md]，根默认展开。
        let state = app.file_tree_state();
        assert_eq!(state.rows.len(), 3);

        // 选到 src（root → src）。
        app.file_tree_move_selection(1);
        app.file_tree_move_selection(1);
        assert_eq!(
            app.file_tree_state().selected.as_deref(),
            Some(root.join("src").as_path())
        );

        app.file_tree_expand_or_into();
        let state = app.file_tree_state();
        // 展开 src 后 rows: [root, src, inner, lib.rs, README.md]
        assert_eq!(state.rows.len(), 5);
        assert!(
            state
                .rows
                .iter()
                .find(|r| r.path == root.join("src"))
                .map(|r| r.expanded)
                .unwrap_or(false)
        );

        app.file_tree_collapse_or_parent();
        assert_eq!(app.file_tree_state().rows.len(), 3);
    }

    #[test]
    fn file_tree_activate_on_file_should_open_buffer_and_report_opened() {
        let mut app = App::new();
        let root = project_fixture("activate");
        app.open_local_project(root.clone());

        // rows: [root, src, README.md] —— 走到 README.md。
        app.file_tree_move_selection(1); // root
        app.file_tree_move_selection(1); // src
        app.file_tree_move_selection(1); // README.md
        let selected = app.file_tree_state().selected.clone();
        assert_eq!(selected.as_deref(), Some(root.join("README.md").as_path()));

        let action = app.file_tree_activate();
        assert_eq!(action, FileTreeActivation::OpenedFile);

        let state = app.file_tree_state();
        assert_eq!(
            state.active.as_deref(),
            Some(root.join("README.md").as_path())
        );
    }

    #[test]
    fn file_tree_activate_on_directory_should_toggle_expanded() {
        let mut app = App::new();
        let root = project_fixture("activate-dir");
        app.open_local_project(root.clone());

        // rows: [root, src, README.md] —— 选到 src。
        app.file_tree_move_selection(1); // root
        app.file_tree_move_selection(1); // src
        let action = app.file_tree_activate();
        assert_eq!(action, FileTreeActivation::ToggledDir);

        let state = app.file_tree_state();
        let src_row = state
            .rows
            .iter()
            .find(|r| r.path == root.join("src"))
            .unwrap();
        assert!(matches!(src_row.kind, EntryKind::Directory));
        assert!(src_row.expanded);
    }

    #[test]
    fn file_tree_pending_editor_keys_route_through_keymap_by_context() {
        let mut app = App::new();
        app.open_local_project(project_fixture("pending-editor"));
        app.file_tree_begin_new_entry();

        app.ime_replace_text_for(TextTargetId::FileTreePendingName, None, "alpha")
            .unwrap();

        // 全局快捷键不被单行新建输入框吞掉：在 Global 上下文照常解析成 panel 命令。
        let outcome = app
            .dispatch_key("mod-shift-e".to_string(), KeySurface::FileTree)
            .unwrap();
        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![HostEffect::TogglePanel("file_tree".to_string())]
        );

        // 编辑键在 text_edit 上下文命中，作用到新建输入框（focused_field 路由）。
        let outcome = app
            .dispatch_key("mod-a".to_string(), KeySurface::FileTree)
            .unwrap();
        assert!(outcome.consumed);
        assert!(outcome.effects.is_empty());

        app.ime_replace_text_for(TextTargetId::FileTreePendingName, None, "beta")
            .unwrap();
        assert!(
            app.file_tree_state().pending.is_some(),
            "新建输入框仍在编辑态"
        );
        assert_eq!(pending_name_snapshot(&app).text, "beta");

        // Tab / Shift-Tab：单行编辑器不接受缩进 / 反缩进，和 Enter 不接受换行一样。
        let outcome = app
            .dispatch_key("tab".to_string(), KeySurface::FileTree)
            .unwrap();
        assert!(!outcome.consumed);
        assert!(outcome.effects.is_empty());
        assert_eq!(pending_name_snapshot(&app).text, "beta");

        let outcome = app
            .dispatch_key("shift-tab".to_string(), KeySurface::FileTree)
            .unwrap();
        assert!(!outcome.consumed);
        assert!(outcome.effects.is_empty());
        assert_eq!(pending_name_snapshot(&app).text, "beta");

        // Enter：单行编辑器不接受换行 → text_edit 落空 → 命中 FileTree 的提交命令。
        let outcome = app
            .dispatch_key("enter".to_string(), KeySurface::FileTree)
            .unwrap();
        assert!(outcome.consumed);
        assert_eq!(outcome.effects, vec![HostEffect::FileTreeCommitNewEntry]);
    }

    #[test]
    fn file_tree_pending_editor_does_not_intercept_keys_while_composing() {
        let mut app = App::new();
        app.open_local_project(project_fixture("pending-ime"));
        app.file_tree_begin_new_entry();

        app.ime_replace_and_mark_text_for(
            TextTargetId::FileTreePendingName,
            None,
            "ni",
            Some(2..2),
        )
        .unwrap();
        assert!(has_marked_range(&app, TextTargetId::FileTreePendingName));

        // 组合态下 dispatch_key 一律不消费、不拦截：Enter / Esc 等都透传给系统
        // 输入法，由它驱动候选的提交 / 取消。宿主在这里抢键会让 IME 会话脱节。
        let outcome = app
            .dispatch_key("enter".to_string(), KeySurface::FileTree)
            .unwrap();
        assert!(!outcome.consumed);
        assert!(outcome.effects.is_empty());

        // 这次 Enter 没动到任何状态：组合还在，文件树新建也还在。
        assert!(has_marked_range(&app, TextTargetId::FileTreePendingName));
        assert!(app.file_tree_state().pending.is_some());
    }

    #[test]
    fn file_tree_pending_editor_escape_exits_right_after_ime_preedit_cleared() {
        let mut app = App::new();
        app.open_local_project(project_fixture("pending-ime-esc"));
        app.file_tree_begin_new_entry();

        // 输入中文候选。
        app.ime_replace_and_mark_text_for(
            TextTargetId::FileTreePendingName,
            None,
            "ni",
            Some(2..2),
        )
        .unwrap();
        assert!(has_marked_range(&app, TextTargetId::FileTreePendingName));

        // 系统输入法取消候选 = 把 marked text 置空。composition 必须彻底结束，
        // 不留空壳 —— 否则 marked_text_range 仍报 Some，系统 IME 会吞掉后续按键。
        app.ime_replace_and_mark_text_for(TextTargetId::FileTreePendingName, None, "", None)
            .unwrap();
        assert!(
            !has_marked_range(&app, TextTargetId::FileTreePendingName),
            "preedit 清空后 composition 必须彻底结束，不能留空壳"
        );

        // 紧接着一次 Esc 就该真正退出新建。
        let outcome = app
            .dispatch_key("escape".to_string(), KeySurface::FileTree)
            .unwrap();
        assert!(outcome.consumed);
        assert_eq!(outcome.effects, vec![HostEffect::FileTreeCancelNewEntry]);
    }

    #[test]
    fn file_tree_new_entry_should_use_yazi_file_path_rules() {
        let mut app = App::new();
        let root = project_fixture("new-entry-file-path");
        app.open_local_project(root.clone());

        app.file_tree_begin_new_entry();
        app.ime_replace_text_for(
            TextTargetId::FileTreePendingName,
            None,
            "generated/deep/new.txt",
        )
        .unwrap();

        assert_eq!(
            app.file_tree_commit_new_entry(),
            FileTreeActivation::OpenedFile
        );
        assert!(root.join("generated/deep/new.txt").is_file());
        assert_eq!(
            app.file_tree_state().active.as_deref(),
            Some(root.join("generated/deep/new.txt").as_path())
        );
        let state = app.editor_state();
        assert_eq!(active_tab(&state).title, "new.txt");
    }

    #[test]
    fn file_tree_new_entry_should_use_yazi_directory_path_rules() {
        let mut app = App::new();
        let root = project_fixture("new-entry-dir-path");
        app.open_local_project(root.clone());

        app.file_tree_begin_new_entry();
        app.ime_replace_text_for(TextTargetId::FileTreePendingName, None, "generated/deep/")
            .unwrap();

        assert_eq!(
            app.file_tree_commit_new_entry(),
            FileTreeActivation::Nothing
        );
        assert!(root.join("generated/deep").is_dir());
        assert_eq!(
            app.file_tree_state().selected.as_deref(),
            Some(root.join("generated/deep").as_path())
        );
        assert!(app.editor_state().tabs.is_empty());
    }

    #[test]
    fn tab_commands_should_switch_and_close_active_view() {
        let mut app = App::new();
        app.open_local_project(project_fixture("tabs"));

        // 打开 README.md：rows = [root, src, README.md]。
        app.file_tree_move_selection(1); // root
        app.file_tree_move_selection(1); // src
        app.file_tree_move_selection(1); // README.md
        assert_eq!(app.file_tree_activate(), FileTreeActivation::OpenedFile);

        // 展开 src 并打开 src/lib.rs：
        // 展开后 rows = [root, src, inner, lib.rs, README.md]。
        app.file_tree_move_selection(-1); // 回到 src
        app.file_tree_expand_or_into(); // 展开 src
        app.file_tree_move_selection(1); // inner
        app.file_tree_move_selection(1); // lib.rs
        assert_eq!(app.file_tree_activate(), FileTreeActivation::OpenedFile);

        // 两个标签：README.md 先开、lib.rs 后开且为活动标签。
        let state = app.editor_state();
        assert_eq!(state.tabs.len(), 2);
        assert_eq!(active_tab(&state).title, "lib.rs");
        assert!(state.tabs[1].is_active);

        // 切到上一个标签 → README.md。
        app.dispatch(editor::select_tab(editor::SelectTabTarget::Previous))
            .unwrap();
        let state = app.editor_state();
        assert_eq!(active_tab(&state).title, "README.md");
        assert!(state.tabs[0].is_active);

        // 关闭当前标签 → 只剩 lib.rs。
        app.dispatch(editor::close_tab()).unwrap();
        let state = app.editor_state();
        assert_eq!(state.tabs.len(), 1);
        assert_eq!(active_tab(&state).title, "lib.rs");
    }
}
