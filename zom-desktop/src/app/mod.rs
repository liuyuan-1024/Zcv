//! app —— 组合根（手册 2 / 13）。
//!
//! 组合根装配命令、工作区、配置、文本目标与后台拍点 runtime，并把输入统一收敛到 command 管线。
//!
//! 依赖方向：`shell` 只通过 [`App`] 调组合根；`app` 不 import `shell` 的 feature / workbench / editor 类型。
//! 反向接入走顶层共享协议 [`crate::ports`] 与 [`crate::text_target`]。
//!
//! 子模块职责：
//! - [`dispatch`]：命令/按键/交互派发管线
//! - [`focus`]：语义焦点管理与 key context 投影
//! - [`pumps`]：编辑后扇出、帧泵与文件监听
//! - [`command_runtime`] / [`config_store`] / [`config_applier`] / [`text_target_runtime`]：私有协作者

mod command_runtime;
mod config_applier;
mod config_store;
mod dispatch;
mod focus;
mod pumps;
mod text_target_runtime;

#[cfg(test)]
mod tests;

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use zom_command::{ClipboardPort, CommandCatalogItem, CommandId};
use zom_engine::{BufferVersion, ByteOffset, Selection, SelectionSet};
use zom_workspace::syntax::{SyntaxEngine, install_builtin_providers};
use zom_workspace::view::{RevealKind, ViewId, ViewSet};
use zom_workspace::{BufferId, Workspace};

use self::command_runtime::CommandRuntime;
use self::config_applier::ConfigApplier;
use self::config_store::ConfigStore;
use self::pumps::BackgroundPumps;
use self::text_target_runtime::TextTargetRuntime;
use crate::config::{AppConfig, SettingsChange};
use crate::editor::{EditorViewportMeasurement, SettledViewportTop};
use crate::editor_state::{self, EditorState};
use crate::file_watcher::FileWatcherService;
use crate::focus::{AppFocus, FocusStore};
use crate::git_service::GitService;
use crate::lsp_host::LspHost;
use crate::ports::{
    FileTreeAction, FileTreeActionResult, FileTreeHost, FramePump, PostEditObserver, SearchAction,
    SearchHost,
};
use crate::text_target::{EditorRouter, EditorRouterMut, TextTargetOwner};
use crate::workspace_session::WorkspaceSession;

pub struct App {
    command: CommandRuntime,
    pub(crate) session: WorkspaceSession,
    config: ConfigStore,
    background: BackgroundPumps,
    focus: FocusStore,
    project_root: Option<PathBuf>,
    /// 当前项目所在 git 仓库的分支名。打开项目时通过 `git rev-parse --abbrev-ref HEAD` 获取。
    project_branch: Option<String>,
    text_targets: TextTargetRuntime,
    /// git 状态服务：App 层持有，文件树 / editor gutter / Git Panel 共享查询。
    git: Rc<RefCell<GitService>>,
    file_tree: Option<Box<dyn FileTreeHost>>,
    search: Option<Box<dyn SearchHost>>,
    /// LSP 主机：管理语言服务器实例池、文档同步路由与诊断收集、每帧推进后台状态。
    lsp_host: LspHost,
    /// 预览文本缓存：跨帧复用，避免每帧全量重读 buffer。
    preview_cache: RefCell<BTreeMap<ViewId, (BufferVersion, String)>>,
    /// 预览滚动句柄：跨帧 / 跨 tab 切换复用，保持预览滚动位置。
    preview_scroll_handles: RefCell<BTreeMap<ViewId, gpui::ScrollHandle>>,
    /// 文件监听服务：项目打开时启动，关闭时释放。
    file_watcher: Option<FileWatcherService>,
    /// 文件系统变更标志：文件监听器检测到变更时置 true，FileTreeModel::state() 消费后清回 false。
    fs_changed: Rc<Cell<bool>>,
}

impl App {
    /// 内存模式（测试版使用，避免污染真实目录）
    pub fn new() -> Self {
        Self::new_with_paths(None)
    }

    /// 持久化模式（发行版使用）
    pub fn new_persistent() -> Self {
        Self::new_with_paths(AppConfig::default_path())
    }

    pub(crate) fn new_with_paths(config_path: Option<PathBuf>) -> Self {
        let config = ConfigStore::new(config_path);
        let (_engine, mut workspace, views) = empty_workspace();
        workspace.set_buffer_config(config.buffer_config());

        Self {
            command: CommandRuntime::new(),
            session: WorkspaceSession::new(workspace, views),
            config,
            background: BackgroundPumps::new(),
            focus: FocusStore::new(AppFocus::project_picker()),
            project_root: None,
            project_branch: None,
            text_targets: TextTargetRuntime::new(),
            git: Rc::new(RefCell::new(GitService::new(std::path::Path::new("")))),
            file_tree: None,
            search: None,
            lsp_host: LspHost::new(),
            preview_cache: RefCell::new(BTreeMap::new()),
            preview_scroll_handles: RefCell::new(BTreeMap::new()),
            file_watcher: None,
            fs_changed: Rc::new(Cell::new(false)),
        }
    }

    // ── 装配期注册 ──────────────────────────────────────────────

    pub(crate) fn install_editor_owner(&mut self, owner: Rc<RefCell<dyn TextTargetOwner>>) {
        self.text_targets.install_editor_owner(owner);
    }

    pub(crate) fn install_post_edit_observer(&mut self, observer: Box<dyn PostEditObserver>) {
        self.background.install_post_edit_observer(observer);
    }

    pub(crate) fn install_frame_pump(&mut self, pump: Box<dyn FramePump>) {
        self.background.install_frame_pump(pump);
    }

    pub(crate) fn git_handle(&self) -> Rc<RefCell<GitService>> {
        self.git.clone()
    }

    pub(crate) fn fs_changed_handle(&self) -> Rc<Cell<bool>> {
        self.fs_changed.clone()
    }

    pub(crate) fn install_file_tree_host(&mut self, host: Box<dyn FileTreeHost>) {
        self.file_tree = Some(host);
    }

    pub(crate) fn install_search_host(&mut self, host: Box<dyn SearchHost>) {
        self.search = Some(host);
    }

    // ── 配置 ────────────────────────────────────────────────────

    pub(crate) fn soft_wrap_handle(&self) -> Rc<Cell<bool>> {
        self.config.soft_wrap_handle()
    }

    pub(crate) fn toggle_soft_wrap(&mut self) {
        if let Err(error) = self.config.toggle_soft_wrap() {
            self.push_config_save_error(error);
        }
    }

    pub(crate) fn apply_open_config_file_from_effect(&mut self) -> bool {
        let Some(path) = self.config.path() else {
            self.session.push_bubble(
                zom_command::BubbleRequest::info("当前为内存配置模式，没有可打开的 config.toml")
                    .dedupe("config.open"),
            );
            return false;
        };
        if let Err(error) = self.config.save() {
            self.push_config_save_error(error);
            return false;
        }
        if !self.session.open_file(path) {
            return false;
        }
        self.request_focus(AppFocus::editor());
        true
    }

    pub(crate) fn config_snapshot(&self) -> AppConfig {
        self.config.snapshot()
    }

    pub(crate) fn config_path(&self) -> Option<PathBuf> {
        self.config.path()
    }

    pub(crate) fn apply_settings_change_from_effect(&mut self, change: SettingsChange) {
        self.config.apply_change(change);
        ConfigApplier::apply_to_session(self.config.config(), &mut self.session);
        if let Err(error) = self.config.save() {
            self.push_config_save_error(error);
        }
    }

    fn push_config_save_error(&mut self, error: impl Into<String>) {
        self.session
            .push_bubble(zom_command::BubbleRequest::error(error).dedupe("config.save"));
    }

    pub(crate) fn pump_config_load_warnings(&mut self) {
        for warning in self.config.take_load_warnings() {
            self.session
                .push_bubble(zom_command::BubbleRequest::error(warning).dedupe("config.load"));
        }
    }

    // ── 会话访问 ────────────────────────────────────────────────

    pub(crate) fn activate_view_tab(&mut self, view_id: ViewId) {
        self.session.set_active_view(view_id);
    }

    pub(crate) fn workspace(&self) -> &Workspace {
        self.session.workspace()
    }

    pub(crate) fn views(&self) -> &ViewSet {
        self.session.views()
    }

    pub(crate) fn active_view_id(&self) -> Option<ViewId> {
        self.session.active_view_id()
    }

    pub(crate) fn active_buffer_id(&self) -> Option<BufferId> {
        self.session.active_buffer_id()
    }

    pub(crate) fn take_session_bubbles(&mut self) -> Vec<zom_command::BubbleRequest> {
        self.session.take_bubbles()
    }

    // ── 焦点 ────────────────────────────────────────────────────

    pub(crate) fn focus(&self) -> &FocusStore {
        &self.focus
    }

    // ── 项目 ────────────────────────────────────────────────────

    pub(crate) fn set_clipboard(&mut self, clipboard: Box<dyn ClipboardPort>) {
        self.command.set_clipboard(clipboard);
    }

    pub(crate) fn apply_open_project_from_effect(&mut self, root: PathBuf, branch: Option<String>) {
        self.project_root = Some(root.clone());
        self.project_branch = branch;
        self.lsp_host.set_project_root(Some(&root));
        self.session.reset_project(self.config.buffer_config());
        self.request_focus(AppFocus::editor());
        self.file_watcher = FileWatcherService::start(&root).ok();
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

    pub(crate) fn current_branch(&self) -> Option<String> {
        self.project_branch.clone()
    }

    pub(crate) fn set_branch(&mut self, branch: String) {
        self.project_branch = Some(branch);
    }

    pub(crate) fn project_root(&self) -> Option<&std::path::Path> {
        self.project_root.as_deref()
    }

    pub(crate) fn project_picker_deactivate(&mut self) {
        if matches!(
            self.focus.current(),
            AppFocus::Surface(s) if s.surface == crate::ui_id::SurfaceId::ProjectPicker
        ) {
            self.restore_previous_focus();
        }
    }

    // ── 编辑区渲染 ──────────────────────────────────────────────

    pub(crate) fn editor_state(&self) -> EditorState {
        let project_root = self.project_root().map(|p| p.to_path_buf());
        let prev_cache = self.preview_cache.borrow();
        let prev_scroll_handles = self.preview_scroll_handles.borrow();
        let state = editor_state::build_editor_state(
            self.session.workspace(),
            self.session.views(),
            self.session.active_view_id(),
            project_root.as_deref(),
            &prev_cache,
            &prev_scroll_handles,
        );
        drop(prev_cache);
        drop(prev_scroll_handles);
        *self.preview_cache.borrow_mut() = state.preview_cache.clone();
        *self.preview_scroll_handles.borrow_mut() = state.preview_scroll_handles.clone();
        state
    }

    pub(crate) fn sync_main_viewport_measurement(
        &mut self,
        measured: EditorViewportMeasurement,
        wrap_map: Option<zom_workspace::view::WrapMap>,
    ) -> Option<SettledViewportTop> {
        let active_view_id = self.session.active_edit_view_id()?;
        let (workspace, views) = self.session.parts_mut();
        let view = views.edit_view_mut(active_view_id)?;
        let current = view.viewport();
        let viewport = zom_workspace::view::ViewportState {
            top_line: current.top_line,
            top_subrow: current.top_subrow,
            visible_visual_rows: measured.visible_visual_rows,
            visible_logical_lines: measured.visible_logical_lines,
        };
        if current != viewport {
            view.set_viewport(viewport);
        }
        view.set_wrap_map(wrap_map);

        let buffer = workspace.buffer(view.buffer())?;
        let cursor = view.selection().primary().head();
        let settlement = view.settle_viewport_y(buffer.buffer(), cursor);
        Some(SettledViewportTop {
            top_line: settlement.viewport.top_line,
            top_subrow: settlement.viewport.top_subrow,
        })
    }

    // ── Feature 桥接 ────────────────────────────────────────────

    pub(crate) fn apply_search_action_from_effect(&mut self, action: SearchAction) {
        if let Some(search) = &self.search {
            search.apply_search_action_from_effect(action, &mut self.session);
        }
    }

    pub(crate) fn is_search_open(&self) -> bool {
        self.search.as_ref().map_or(false, |s| s.is_open())
    }

    pub(crate) fn go_to_line_jump(&mut self, target_byte: usize) {
        let Some(view_id) = self.session.active_edit_view_id() else {
            return;
        };
        let (workspace, views) = self.session.parts_mut();
        let Some(view) = views.edit_view_mut(view_id) else {
            return;
        };
        let buffer_id = view.buffer();
        let Some(wb) = workspace.buffer_mut(buffer_id) else {
            return;
        };
        let buf = wb.buffer_mut();
        let offset = ByteOffset::new(target_byte);
        let selection = SelectionSet::new(vec![Selection::caret(offset)]);
        if buf.set_selection(selection.clone()).is_ok() {
            let (v_sel, _, _, _) = view.vertical_movement_state_mut();
            *v_sel = selection;
            view.request_reveal(offset, RevealKind::Jump);
        }
    }

    pub(crate) fn apply_file_tree_action_from_effect(
        &mut self,
        action: FileTreeAction,
    ) -> FileTreeActionResult {
        let Some(file_tree) = &self.file_tree else {
            return FileTreeActionResult::default();
        };
        file_tree.apply_file_tree_action_from_effect(action, &mut self.session)
    }

    // ── 文本路由 ────────────────────────────────────────────────

    pub(crate) fn with_router<R>(&self, f: impl FnOnce(EditorRouter<'_>) -> R) -> R {
        self.text_targets.with_router(&self.session, f)
    }

    pub(crate) fn with_router_mut<R>(&mut self, f: impl FnOnce(EditorRouterMut<'_>) -> R) -> R {
        let focus = self.focus.current();
        self.text_targets
            .with_router_mut(focus, &mut self.session, f)
    }

    // ── 命令信息查询 ────────────────────────────────────────────

    pub(crate) fn shortcuts_for(&self, command_id: &str) -> Option<String> {
        let command = CommandId::new(command_id).ok()?;
        self.command.keymap().format_shortcuts_for(&command)
    }

    pub(crate) fn command_title_for(&self, command_id: &str) -> Option<String> {
        let command = CommandId::new(command_id).ok()?;
        self.command
            .registry()
            .command(&command)
            .map(|command| command.title.clone())
    }

    pub(crate) fn command_catalog_items(&self) -> Vec<CommandCatalogItem> {
        self.command.registry().commands().map(Into::into).collect()
    }
}

// ── 自由函数 ────────────────────────────────────────────────────

/// 空工作区：不预建任何 buffer/view。
fn empty_workspace() -> (Rc<SyntaxEngine>, Workspace, ViewSet) {
    let mut engine = SyntaxEngine::new();
    install_builtin_providers(&mut engine);
    let engine = Rc::new(engine);
    let workspace = Workspace::with_engine(engine.clone());
    (engine, workspace, ViewSet::new())
}

impl Default for App {
    fn default() -> Self {
        Self::new()
    }
}

// 从 app/mod.rs 拆出的 impl 块分布在：
// - dispatch.rs：命令派发、按键派发、交互派发
// - focus.rs：焦点精化、key context 投影
// - pumps.rs：帧泵、文件监听、LSP 推进
