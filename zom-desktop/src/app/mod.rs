//! app —— 组合根（手册 2 / 13）。
//!
//! 组合根装配命令、工作区、配置、文本目标与后台拍点 runtime，并把输入统一收敛到 command 管线。
//!
//! 依赖方向：`shell` 只通过 [`App`] 调组合根；`app` 不 import `shell` 的 feature / workbench / editor 类型。
//! 反向接入走顶层共享协议 [`crate::ports`] 与 [`crate::text_target`]。
//! 本目录内的子模块（command_runtime / config_store / config_applier / text_target_runtime / pumps）都是 App 的私有协作者，对 shell 完全隔离；
//! shell 想"反向接入"App 走的是 [`crate::ports`] 里的 trait。

mod command_runtime;
mod config_applier;
mod config_store;
mod pumps;
mod text_target_runtime;

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::rc::Rc;

use zom_command::{
    ClipboardPort, CommandCatalogItem, CommandError, CommandId, FileTreeKeyMode, HostEffect,
    Invocation, KeyContext, KeymapResolution,
};
use zom_engine::{BufferVersion, ByteOffset, Selection, SelectionSet};
use zom_workspace::syntax::{SyntaxEngine, install_builtin_providers};
use zom_workspace::view::{RevealKind, ViewId, ViewSet, ViewportEditAnchor};
use zom_workspace::{BufferId, Workspace};

use self::command_runtime::CommandRuntime;
use self::config_applier::ConfigApplier;
use self::config_store::ConfigStore;
use self::pumps::BackgroundPumps;
use self::text_target_runtime::TextTargetRuntime;
use crate::config::{AppConfig, SettingsChange};
use crate::dispatch::KeyDispatchOutcome;
use crate::editor::{EditorViewportMeasurement, SettledViewportTop};
use crate::editor_state::{self, EditorState};
use crate::file_watcher::{FileWatcherService, FsEventKind};
use crate::focus::{AppFocus, FileTreeFocus, FocusStore, PanelSubFocus};
use crate::git_service::GitService;
use crate::host_intent::{InteractionIntent, PointerIntent};
use crate::lsp_host::LspHost;
use crate::ports::{
    FileTreeAction, FileTreeActionResult, FileTreeHost, FramePump, PostEditObserver, SearchAction,
    SearchHost,
};
use crate::text_target::{EditorRouter, EditorRouterMut, TextTargetOwner};
use crate::ui_id::SurfaceId;
use crate::workspace_session::WorkspaceSession;

pub struct App {
    command: CommandRuntime,
    session: WorkspaceSession,
    config: ConfigStore,
    background: BackgroundPumps,
    focus: FocusStore,
    project_root: Option<PathBuf>,
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
            // 启动时没有项目、没有 view，shell 显示的是 project picker；
            // 把初始焦点设成 picker 让 App-only 单测不依赖反向同步也能拿到正确语义。
            // 生产里 ShellView::render 的反向同步会把这值刷到与 GPUI 真实焦点一致。
            focus: FocusStore::new(AppFocus::project_picker()),
            project_root: None,
            text_targets: TextTargetRuntime::new(),
            // 初始化为空，项目打开后 FileTreeModel 会替换内部 GitService
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

    /// 把一个 shell runtime 持有的可嵌入编辑器 owner 注册进路由表。
    /// 由 shell 根视图在装配阶段调；运行期通常不会再注册。
    ///
    /// `Rc` 的生命周期归 runtime（真正拥有者）；本结构只持一份共享引用，
    /// 在路由阶段借出做 query / 命令写入。
    ///
    pub(crate) fn install_editor_owner(&mut self, owner: Rc<RefCell<dyn TextTargetOwner>>) {
        self.text_targets.install_editor_owner(owner);
    }

    /// 注册一个编辑后同步观察者。shell 装配阶段调；
    /// 之后每次活动 buffer 产生编辑事件，
    /// BackgroundPumps 把 built-in 的 post_edit 跑完后会按注册顺序调每个观察者的 `after_text_edit`。
    pub(crate) fn install_post_edit_observer(&mut self, observer: Box<dyn PostEditObserver>) {
        self.background.install_post_edit_observer(observer);
    }

    /// 注册一个每帧 drain 端口。shell 装配阶段调；
    /// 运行期由 [`Self::pump_frame_observers`] 按注册顺序调一次。
    pub(crate) fn install_frame_pump(&mut self, pump: Box<dyn FramePump>) {
        self.background.install_frame_pump(pump);
    }

    /// 获取共享的 git 状态服务句柄，供 FileTreeRuntime 等消费方初始化时注入。
    pub(crate) fn git_handle(&self) -> Rc<RefCell<GitService>> {
        self.git.clone()
    }

    /// 文件树脏标志：文件监听器发现有文件变更时置 true；FileTreeModel 消费后清回 false。
    pub(crate) fn fs_changed_handle(&self) -> Rc<Cell<bool>> {
        self.fs_changed.clone()
    }

    pub(crate) fn install_file_tree_host(&mut self, host: Box<dyn FileTreeHost>) {
        self.file_tree = Some(host);
    }

    /// 注册搜索动作端口。搜索输入状态归 search feature runtime 持有；
    /// app 在命令 effect 落地时带着 workspace/view 会话调用它。
    pub(crate) fn install_search_host(&mut self, host: Box<dyn SearchHost>) {
        self.search = Some(host);
    }

    /// 共享的软换行 cell——多行 [`EditorKernel`] 在装配时借这份 `Rc`。
    /// 一次写入对所有持有者立刻可见，下一帧 element 就走新渲染路径。
    pub(crate) fn soft_wrap_handle(&self) -> Rc<Cell<bool>> {
        self.config.soft_wrap_handle()
    }

    /// 翻转全局软换行。同时更新 [`AppConfig`] 字段并落盘——「切一次软换行」就是「改一次默认值」。
    /// 内存模式（`config_path` 为 `None`）仍在内存里翻，只是 save 是 no-op。
    pub(crate) fn toggle_soft_wrap(&mut self) {
        if let Err(error) = self.config.toggle_soft_wrap() {
            self.push_config_save_error(error);
        }
    }

    /// 把活动 tab 切到指定 view（对应 `HostEffect::EditorSelectTab`）。
    /// 由 shell 端 effect handler 调，不直接给业务 / 命令使用。
    pub(crate) fn activate_view_tab(&mut self, view_id: zom_workspace::view::ViewId) {
        self.session.set_active_view(view_id);
    }

    /// 把真实的 config.toml 打开到主编辑区。首次打开前先保存当前配置，保证文件存在；
    /// 后续语言识别、语法高亮、保存都走主工作区的普通文件路径。
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

    /// 把启动期累积的配置加载诊断收纳到 session 气泡队列，等待下一次 drain。
    pub(crate) fn pump_config_load_warnings(&mut self) {
        for warning in self.config.take_load_warnings() {
            self.session
                .push_bubble(zom_command::BubbleRequest::error(warning).dedupe("config.load"));
        }
    }

    pub(crate) fn focus(&self) -> &FocusStore {
        &self.focus
    }

    pub(crate) fn request_focus(&mut self, next: AppFocus) {
        self.focus.request(next);
    }

    pub(crate) fn request_focus_from_shell(&mut self, next: AppFocus) {
        let next = self.refine_focus(next);
        self.focus.request(next);
    }

    pub(crate) fn restore_previous_focus(&mut self) -> AppFocus {
        self.focus.restore_previous()
    }

    fn refine_focus(&self, focus: AppFocus) -> AppFocus {
        if focus == AppFocus::file_tree(FileTreeFocus::Navigate) {
            let current = self.focus.current();
            let current_ft = match current {
                AppFocus::Panel(p) => p.as_file_tree(),
                _ => None,
            };
            if let Some(sub) = current_ft {
                let is_pending = matches!(
                    sub,
                    FileTreeFocus::NewEntryName
                        | FileTreeFocus::RenameEntry
                        | FileTreeFocus::ConfirmDelete,
                );
                if is_pending
                    && (sub == FileTreeFocus::ConfirmDelete
                        || self.text_targets.accepts_focus(&self.session, current))
                {
                    return current;
                }
            }

            for candidate in [
                AppFocus::file_tree(FileTreeFocus::RenameEntry),
                AppFocus::file_tree(FileTreeFocus::NewEntryName),
            ] {
                if self.text_targets.accepts_focus(&self.session, candidate) {
                    return candidate;
                }
            }
        }
        focus
    }

    /// 替换默认剪贴板端口。
    /// shell 启动时注入 `GpuiClipboard`，让 copy / cut / paste 走系统剪贴板；
    /// headless 路径保持默认 [`zom_command::NoopClipboard`]。
    pub(crate) fn set_clipboard(&mut self, clipboard: Box<dyn ClipboardPort>) {
        self.command.set_clipboard(clipboard);
    }

    /// 把指定 root 切成当前活动项目：重建 workspace / view、聚焦编辑区。
    ///
    /// **不**负责把 root 写到"最近项目"列表 —— 那是 picker 自家的 UI 数据，归 project picker runtime 拥有；
    /// shell 在调用本方法后再调 `runtime.remember_project(root, repo)` 完成登记，repo 信息也由 shell 持有。
    /// HostEffect/project_session 落地入口：把指定 root 切成当前活动项目。
    pub(crate) fn apply_open_project_from_effect(&mut self, root: PathBuf) {
        self.project_root = Some(root.clone());
        self.lsp_host.set_project_root(Some(&root));
        self.session.reset_project(self.config.buffer_config());
        self.request_focus(AppFocus::editor());
        // 启动文件监听。失败时静默降级——文件树仍可通过手动操作刷新。
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

    pub(crate) fn project_root(&self) -> Option<&std::path::Path> {
        self.project_root.as_deref()
    }

    pub(crate) fn project_picker_deactivate(&mut self) {
        if matches!(
            self.focus.current(),
            AppFocus::Surface(s) if s.surface == SurfaceId::ProjectPicker
        ) {
            self.restore_previous_focus();
        }
    }

    pub(crate) fn workspace(&self) -> &Workspace {
        self.session.workspace()
    }

    pub(crate) fn views(&self) -> &ViewSet {
        self.session.views()
    }

    pub(crate) fn active_view_id(&self) -> Option<zom_workspace::view::ViewId> {
        self.session.active_view_id()
    }

    /// 当前活动视图对应的 buffer id——文件树等"跟随活动文件"的 UI 从此投影。
    pub(crate) fn active_buffer_id(&self) -> Option<BufferId> {
        self.session.active_buffer_id()
    }

    fn capture_active_viewport_edit_anchor(&self) -> Option<ViewportEditAnchor> {
        let view_id = self.session.active_edit_view_id()?;
        let view = self.session.views().edit_view(view_id)?;
        let buffer = self.session.workspace().buffer(view.buffer())?;
        view.capture_viewport_edit_anchor(buffer.buffer())
    }

    /// 取走 session 累积的气泡请求；调用方负责把它们落到 BubbleRuntime。
    pub(crate) fn take_session_bubbles(&mut self) -> Vec<zom_command::BubbleRequest> {
        self.session.take_bubbles()
    }

    /// 构造一份 tab bar 渲染快照。
    /// 集中读 session.workspace / views / active_view，调用方不必再手拼这几样。
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

        // 命令派发期需要给 CommandContext 填 `focused_field`。
        // 若命令真的改了这个输入目标的文本，派发后再让 owner 跑 `after_text_changed`。
        let run_command = |focused_field: Option<zom_command::EditTarget<'_>>| {
            self.command
                .run_invocation(id.clone(), args.clone(), &mut self.session, focused_field)
        };

        let mut host_effects = self
            .text_targets
            .with_edit_target_for_focus(focus, run_command)?;

        // 命令派发可能编辑了活动 buffer（产生 DeltaEvent），扇出给 BufferSearch 与 syntax provider 是无条件的。
        // 否则编辑后高亮 / 搜索命中都不跟版本。
        // 必须先于 `sync_active_buffer_search`：后者依赖搜索状态已被新事件推进。
        self.background.after_text_edit(
            &mut self.session,
            self.config.soft_wrap_enabled(),
            viewport_anchor,
        );
        // 命令派发也可能改了 panel 的 query 文本（在搜索框内按键 / 退格 / 粘贴等）。
        // 把 panel 状态推进活动 buffer 的 BufferSearch 并 sync——一处做完，渲染 / 后续命令读到的都是新真值。

        // 纯 session 状态变更类 effect（tab 切换 / 关闭）在 App 层就近消化，
        // 不动 GPUI / DockState / 焦点，不必走 actions.rs。
        // 这样 `App::dispatch` 的程序化调用方（含集成测试）拿回去的就是已经落地的新状态。
        host_effects.retain(|effect| match effect {
            HostEffect::EditorSelectTab(view_id) => {
                self.session.set_active_view(*view_id);
                false
            }
            HostEffect::EditorOpenPreview(buffer_id) => {
                self.session.open_preview(*buffer_id);
                false
            }
            HostEffect::EditorSelectAdjacentTab(forward) => {
                select_adjacent_tab(&mut self.session, *forward);
                false
            }
            HostEffect::EditorCloseTab(view_id) => {
                self.session.close_view(*view_id);
                false
            }
            HostEffect::RefreshGitStatus => {
                // 手动触发 git 状态刷新（命令/快捷键入口）。
                // 文件监听器也会在 .git/ 变更时自动刷新——二者互为补充。
                let _ = self.git.borrow_mut().refresh();
                false
            }
            _ => true,
        });
        Ok(host_effects)
    }

    /// 派发一次设备交互意图。
    ///
    /// 交互管线不经过 command catalog / keymap，但它和命令一样会在修改编辑状态后对齐 dismiss、清掉编辑合并状态，并由 shell 统一刷新。
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
    ///
    /// 文本输入不在这里兜底：交给 GPUI 的 `EntityInputHandler` 路径，
    /// 系统输入法或 NSTextInputClient 回调会被输入适配层转换为 `editor.ime_*` 命令。
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
            // IME 组合态下 unbound 按键仍需放行给系统输入法，因此 consumed: false 不变。
            // 已命中 keymap 的命令（如 Cmd+V/Cmd+Z）则正常派发——它们内部有
            // cancel_composition_before_text_edit 保护，不会造成 IME 会话脱节。
            KeymapResolution::NoMatch => Ok(KeyDispatchOutcome {
                consumed: false,
                effects: Vec::new(),
            }),
        }
    }

    /// 把「当前焦点面 + 运行态」映射成 keymap 解析用的 `KeyContext` 优先级栈。
    ///
    /// 这是宿主该做的事 —— 告诉 zom-command「现在处于什么上下文」；
    /// 至于哪个 chord 对应哪条命令，仍由各 catalog 注册进 keymap 的绑定决定。
    ///
    /// `composing` 恒为 `false`：keymap 已通过 `text_edit` / `text_edit_composition` 上下文区分组合态，
    /// IME 专属按键（Esc/Enter 在 composition 态）绑定在 composition 上下文，未命中才放行给系统输入法。
    fn key_contexts(&self) -> Vec<KeyContext> {
        let focus = self.focus.current();
        // 先问 router —— 文本输入类 owner（主编辑区、文件树新建/重命名、搜索框、picker 查询框）
        // 通过 `accepts_focus` 自报家门，由 owner 自己说"我的栈是什么"。
        // owner 不接才落到下方按焦点类别给的兜底栈。
        if let Some(stack) = self.text_targets.key_contexts_for(&self.session, focus) {
            return stack;
        }
        match focus {
            AppFocus::None | AppFocus::Editor(_) => vec![KeyContext::global()],
            // 不变式：SearchModel 是 SearchBar 焦点的 TextTargetOwner，router 必定在前一步接管。
            // 若装配链路漏注册 owner，停在这里比静默走 global 容易抓。
            AppFocus::SearchBar(_) => {
                unreachable!("SearchModel 是 SearchBar 焦点的 TextTargetOwner，router 必定接管")
            }
            AppFocus::GoToLine => {
                unreachable!("GoToLineModel 是 GoToLine 焦点的 TextTargetOwner，router 必定接管")
            }
            AppFocus::Panel(p) => match p.sub {
                PanelSubFocus::FileTree(FileTreeFocus::ConfirmDelete) => vec![
                    // 删除确认弹窗打开中：只解析确认 / 取消，导航键全部冻结。
                    KeyContext::file_tree(FileTreeKeyMode::PendingDelete),
                    KeyContext::global(),
                ],
                PanelSubFocus::FileTree(
                    FileTreeFocus::NewEntryName | FileTreeFocus::RenameEntry,
                ) => vec![KeyContext::global()],
                PanelSubFocus::FileTree(_) => vec![
                    KeyContext::file_tree(FileTreeKeyMode::Navigate),
                    KeyContext::global(),
                ],
                PanelSubFocus::Bare => vec![KeyContext::global()],
            },
            AppFocus::Surface(s) => match s.surface {
                // picker focus 永远在 query 输入框上，
                // picker 自己的 key context 由 text target owner 通过 router 提供；
                // 这里只兜底返回 global。
                SurfaceId::ProjectPicker => vec![KeyContext::global()],
                SurfaceId::Settings => vec![KeyContext::settings(), KeyContext::global()],
                SurfaceId::LanguageServers => {
                    vec![KeyContext::language_servers(), KeyContext::global()]
                }
                SurfaceId::GoToLine => vec![KeyContext::global()],
            },
        }
    }

    /// 构造一次只读路由视图。
    ///
    /// 这是 editor 子系统访问 App 内部状态的唯一桥：调用方拿到 [`EditorRouter`] 后直接做 IME 查询 / focused target / snapshot。
    ///
    /// Owners 顺序：先 search runtime 提供的双输入框 owner、再注册表里由 shell runtime 注入的 owner、最后兜底主编辑区。`accepts_focus` 对 `AppFocus` 精确匹配，各 owner 覆盖 disjoint 子集，顺序不影响命中。
    pub(crate) fn with_router<R>(&self, f: impl FnOnce(EditorRouter<'_>) -> R) -> R {
        self.text_targets.with_router(&self.session, f)
    }

    /// 构造一次可写路由视图。
    ///
    /// Owners 通过 `accepts_focus` 对 [`AppFocus`] 精确匹配，各自覆盖 disjoint 的 focus 子集，vec 顺序与优先级无关。
    /// 搜索面板在写路径由 search runtime 提供单个 active owner，同时承担 Query / Replacement 两个 field。
    pub(crate) fn with_router_mut<R>(&mut self, f: impl FnOnce(EditorRouterMut<'_>) -> R) -> R {
        let focus = self.focus.current();
        self.text_targets
            .with_router_mut(focus, &mut self.session, f)
    }

    /// 由主编辑区 element prepaint 中段回写测量值并即时落定视口顶端。
    ///
    /// 1. 把本帧测得的 `visible_visual_rows` 与新 `wrap_map` 写回 view；
    /// 2. 立即用新 wrap_map 跑一次 [`zom_workspace::view::View::settle_viewport_y`]，把 edit / soft-wrap 触发的新视觉行同帧消化掉——不再依赖下一帧补 settle。
    ///
    /// 返回 settle 后的视口顶端；element 拿到后用它解析本帧的 `top_visual_row`。
    /// 无活动 view 时返回 `None`，element 退回 view 当前 top（与首帧一致）。
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

    /// 查询某条命令的快捷键文案 —— 给 Glyph / 命令面板 / 菜单用。
    ///
    /// 一条命令可能绑了多个 chord（不同 args 或别名）：多个 chord 用 ` / ` 拼接成单串返回。
    pub(crate) fn shortcuts_for(&self, command_id: &str) -> Option<String> {
        let command = CommandId::new(command_id).ok()?;
        self.command.keymap().format_shortcuts_for(&command)
    }

    /// 查询某条命令的显示标题 —— UI 不再为命令入口重复维护文案。
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

    /// 每帧 prepaint 起手调一次：收割活动 buffer 的后台 BufferSearch 结果。
    /// 没有 in-flight 时 O(1) 早退。
    /// 新结果落地时同时 reveal 首条命中——避免用户输入查询后 UI 不刷新的"看上去卡住"假象。
    ///
    /// 跑所有通过 [`install_frame_pump`] 注册的[`FramePump`]。
    /// 原本 search 的"收割后台命中"是这里第一个登记者，后续 feature 想做同节奏的 drain 也走同一注册路径——BackgroundPumps 不认具体 feature。
    ///
    /// 统一由 shell 根视图的 render 拍点驱动。
    ///
    /// 语法高亮没有需要 drain 的中间产物 —— paint 阶段直接从共享 [`SyntaxHighlightsSlot`] 现查统一 Query。
    ///
    /// [`install_frame_pump`]: Self::install_frame_pump
    pub fn pump_frame_observers(&mut self) {
        self.background.pump_frame_observers(&mut self.session);
    }

    /// 每帧排空文件监听事件。
    ///
    /// 只处理必须由 App 做的事（buffer 重载需要 WorkspaceSession）。
    /// git 状态刷新也在此提前完成——不依赖 FileTreeModel::state() 的 dirty flag 时序。
    pub fn pump_file_watcher(&mut self) {
        let Some(watcher) = self.file_watcher.as_mut() else {
            return;
        };
        let events = watcher.drain_events();
        if events.is_empty() {
            return;
        }

        // buffer 外部修改重载：需要 WorkspaceSession。
        for event in &events {
            if event.kind == FsEventKind::Modified {
                self.session.reload_if_externally_changed(&event.path);
            }
        }

        // 提前刷新 git 状态——确保 state() 无论何时被调用都能拿到最新颜色。
        let _ = self.git.borrow_mut().refresh();

        // 通知文件树重载目录缓存（reload_expanded_dirs）。
        self.fs_changed.set(true);
    }

    /// 每帧推进 LSP 状态：收割 server 启动结果 → semantic tokens 响应 → 文档同步 → 请求新 tokens。
    pub fn pump_lsp_tokens(&mut self) {
        let workspace = self.session.workspace();
        self.lsp_host.pump(workspace);
    }
}

/// 空工作区：不预建任何 buffer/view。
///
/// 早期版本会默认开一个空白 scratch buffer，但它对用户没有意义、还会让编辑区显示一个不存在的"文件"，误导用户。
/// 现在编辑区在无活动视图时走 `EditorState::default()`，文件从文件树打开后才有内容。
///
/// 启动期同时把内置 syntax provider 工厂注入共享 [`SyntaxEngine`]——否则后续 `open_file` 落 plain。
/// 注册需要在 `Rc::new(engine)` 之前完成。
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

#[cfg(test)]
mod tests {
    //! `App` 派发管线的 headless 单元测试。
    //!
    //! 这一层不接触 GPUI ——只覆盖 keymap 解析、命令派发、IME 流，以及命令产出的 HostEffect。
    //! 需要 GPUI 句柄（Entity / Window / 焦点等）的链路在 shell 根视图那一层做手工 / 集成测试，不进本文件。

    use crate::app::App;
    use crate::config::SettingsChange;
    use crate::editor_state::{EditorState, EditorTab};
    use crate::focus::{AppFocus, FileTreeFocus};
    use crate::host_intent::{InteractionIntent, PointerIntent};
    use crate::text_target::{TextTargetOwner, TextTargetQuery};
    use crate::theme::Theme;
    use crate::ui_id::PanelId;
    use std::cell::RefCell;
    use std::fs::{File, create_dir_all};
    use std::path::PathBuf;
    use std::rc::Rc;
    use zom_command::HostEffect;
    use zom_command::commands::{
        diagnostics, editor, file_tree, language_servers,
        project_picker as project_picker_commands, settings,
    };
    use zom_command::{EditTarget, KeyContext};
    use zom_engine::{ByteOffset, SelectionSet};
    use zom_workspace::view::{ViewportState, WrapMap};

    /// 取当前活动标签——断言「编辑区正在显示哪个文件」用。
    fn active_tab(state: &EditorState) -> &EditorTab {
        state
            .tabs
            .iter()
            .find(|tab| tab.is_active())
            .expect("应有活动标签")
    }

    fn editor_state(app: &App) -> EditorState {
        app.editor_state()
    }

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("zom-app-{tag}-{}.toml", std::process::id()))
    }

    /// 构造一个已打开项目并激活了一个空文件的 `App`。
    fn app_with_open_file(name: &str) -> App {
        let mut app = App::new();
        let root = project_fixture(name);
        app.apply_open_project_from_effect(root.clone());
        assert!(app.session.open_file(root.join("README.md")));
        app.request_focus(AppFocus::editor());
        app
    }

    fn app_with_markdown_text(name: &str, text: &str) -> App {
        let mut app = App::new();
        let root = project_fixture(name);
        std::fs::write(root.join("README.md"), text).unwrap();
        app.apply_open_project_from_effect(root.clone());
        assert!(app.session.open_file(root.join("README.md")));
        app.request_focus(AppFocus::editor());
        app.session
            .workspace()
            .syntax_worker()
            .wait_for_idle_for_test_or_bench();
        app
    }

    fn active_buffer_text(app: &App) -> String {
        let buffer_id = app.active_buffer_id().expect("应有活动 buffer");
        let buffer = app
            .session
            .workspace()
            .buffer(buffer_id)
            .expect("活动 buffer 应存在")
            .buffer();
        buffer
            .slice_byte_range(ByteOffset::ZERO, buffer.len_bytes())
            .unwrap()
            .into_text()
            .into_owned()
    }

    struct StubProjectPickerOwner {
        query: crate::editor::text::OwnedEditorTarget,
    }

    impl TextTargetQuery for StubProjectPickerOwner {
        fn accepts_focus(&self, focus: AppFocus) -> bool {
            focus == AppFocus::project_picker()
        }

        fn snapshot(&self, _focus: AppFocus) -> crate::editor::text::EditorSnapshot {
            self.query
                .snapshot(crate::editor::text::EditorSnapshotRequest::single_line())
        }

        fn key_contexts(&self) -> Vec<KeyContext> {
            vec![
                KeyContext::project_picker(),
                KeyContext::text_edit(false, false),
                KeyContext::global(),
            ]
        }

        fn ime_query_target(
            &self,
            _focus: AppFocus,
        ) -> Option<crate::editor::text::ImeQueryTarget<'_>> {
            Some(self.query.as_ime_query_target())
        }
    }

    impl TextTargetOwner for StubProjectPickerOwner {
        fn edit_target(&mut self, _focus: AppFocus) -> Option<EditTarget<'_>> {
            Some(self.query.as_edit_target())
        }
    }

    /// 在 headless 测试里模拟 project picker 注册过 text target owner。
    fn install_project_picker(app: &mut App) -> Rc<RefCell<StubProjectPickerOwner>> {
        let picker = Rc::new(RefCell::new(StubProjectPickerOwner {
            query: crate::editor::text::OwnedEditorTarget::new(),
        }));
        app.install_editor_owner(picker.clone() as Rc<RefCell<dyn TextTargetOwner>>);
        picker
    }

    #[test]
    fn esc_should_collapse_extended_selection_via_dismiss_stack() {
        // 没有 picker / search bar / pending dialog 等更高瞬态在前，esc 应该塌掉编辑区的非空选区。
        // 这条路径由 command_runtime 末尾的 reconcile_text_edit_dismiss 自动 push 一条 editor.clear_selection token；
        // esc 在 text_edit 上下文走 system.dismiss_top(TextEdit) 把它弹出再派发。
        let mut app = app_with_markdown_text("esc-clear-selection", "hello world");

        app.dispatch_command(editor::select_all()).unwrap();
        let extent_before = !app
            .session
            .active_edit_view()
            .unwrap()
            .selection()
            .primary()
            .is_caret();
        assert!(extent_before, "select_all 之后选区必须非空");

        let outcome = app.dispatch_key("escape".to_string()).unwrap();
        assert!(outcome.consumed, "esc 必须被 dismiss_top 消化");

        let is_caret_after = app
            .session
            .active_edit_view()
            .unwrap()
            .selection()
            .primary()
            .is_caret();
        assert!(is_caret_after, "esc 应当把扩展选区塌成 caret");
    }

    #[test]
    fn esc_should_collapse_pointer_selection_on_first_press() {
        let mut app = app_with_markdown_text("esc-pointer-selection", "hello world");

        app.request_focus(AppFocus::panel(PanelId::Terminal));
        // Pointer interaction 必须同步 slot 对应的语义焦点；
        // 否则 selection 虽然会落到 active view，但后续 Esc 仍按旧焦点解析，第一下不会清选区。
        app.dispatch_interaction(InteractionIntent::Pointer(PointerIntent::SetSelection {
            focus: AppFocus::editor(),
            anchor: ByteOffset::new(1),
            head: ByteOffset::new(5),
        }))
        .unwrap();
        assert!(
            !app.session
                .active_edit_view()
                .unwrap()
                .selection()
                .primary()
                .is_caret(),
            "pointer interaction 产生的选区必须立刻进入 dismiss 栈"
        );

        let outcome = app.dispatch_key("escape".to_string()).unwrap();
        assert!(outcome.consumed, "第一下 esc 必须清掉 pointer 选区");
        assert_eq!(
            outcome.effects,
            vec![HostEffect::EditorCancelPointerSelection],
            "第一下 esc 还必须取消宿主侧鼠标拖选会话，避免 stale mousemove 复活选区"
        );
        assert!(
            app.session
                .active_edit_view()
                .unwrap()
                .selection()
                .primary()
                .is_caret(),
            "pointer 选区不应需要第二下 esc"
        );
    }

    #[test]
    fn pointer_scroll_should_move_viewport_through_interaction_pipeline() {
        let text = (0..100)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>()
            .join("\n");
        let mut app = app_with_markdown_text("pointer-scroll", &text);
        app.session
            .active_edit_view_mut()
            .expect("应有活动视图")
            .set_viewport(ViewportState {
                top_line: 0,
                top_subrow: 0,
                visible_visual_rows: 5,
                visible_logical_lines: 5,
            });

        app.request_focus(AppFocus::panel(PanelId::Terminal));
        app.dispatch_interaction(InteractionIntent::Pointer(PointerIntent::ScrollViewport {
            focus: AppFocus::editor(),
            delta_visual_rows: 3,
        }))
        .unwrap();

        let viewport = app
            .session
            .active_edit_view()
            .expect("应有活动视图")
            .viewport();
        assert_eq!(viewport.top_line, 3);
        assert_eq!(app.focus().current(), AppFocus::editor());
    }

    #[test]
    fn tab_and_enter_should_dispatch_editor_commands() {
        let mut app = app_with_open_file("tab-enter");

        assert!(app.dispatch_key("tab".to_string()).unwrap().consumed);
        assert!(app.dispatch_key("enter".to_string()).unwrap().consumed);
        assert!(app.dispatch_key("return".to_string()).unwrap().consumed);

        let state = editor_state(&app);

        assert!(matches!(
            active_tab(&state),
            EditorTab::Edit(t) if t.dirty
        ));
    }

    #[test]
    fn settings_changes_should_update_runtime_config_and_persist() {
        let path = temp_path("settings-change");
        let _ = std::fs::remove_file(&path);
        let mut app = App::new_with_paths(Some(path.clone()));

        app.apply_settings_change_from_effect(SettingsChange::AdjustUiFont(1));
        app.apply_settings_change_from_effect(SettingsChange::AdjustEditorFont(2));
        app.apply_settings_change_from_effect(SettingsChange::ToggleEditorSoftWrap);

        let config = app.config_snapshot();
        assert_eq!(config.general.theme, Theme::System.as_config());
        assert_eq!(config.ui.font_size, 14);
        assert_eq!(config.editor.font_size, 18);
        assert!(!config.editor.soft_wrap);

        let (loaded, warnings) = crate::config::AppConfig::load(Some(&path));
        assert_eq!(loaded, config);
        assert!(warnings.is_empty());

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn open_config_file_should_create_and_focus_main_editor_tab() {
        let path = temp_path("open-config-file");
        let _ = std::fs::remove_file(&path);
        let mut app = App::new_with_paths(Some(path.clone()));

        assert!(app.apply_open_config_file_from_effect());
        assert!(path.exists());
        assert_eq!(app.focus().current(), AppFocus::editor());

        let state = editor_state(&app);
        let active = active_tab(&state);
        assert_eq!(
            active.title(),
            path.file_name()
                .expect("临时配置路径应有文件名")
                .to_string_lossy()
                .into_owned()
                .as_str()
        );
        assert!(matches!(active, EditorTab::Edit(t) if t.language == "TOML"));

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn editor_tab_size_setting_should_reach_open_buffers() {
        let mut app = App::new();
        // 从默认 4 切到 2。
        app.apply_settings_change_from_effect(SettingsChange::CycleEditorTabSize);
        let root = project_fixture("tab-size-setting");
        app.apply_open_project_from_effect(root.clone());
        assert!(app.session.open_file(root.join("README.md")));

        let tab_width = app
            .active_buffer_id()
            .and_then(|id| app.workspace().buffer(id))
            .expect("应有活动 buffer")
            .buffer()
            .config()
            .tab
            .tab_width();
        assert_eq!(tab_width, 2);
    }

    #[test]
    fn text_edit_should_preserve_conservative_active_view_wrap_map() {
        let mut app = app_with_markdown_text("preserve-wrap-map", "abcdefghij\nklmnopqrst");
        app.session
            .active_edit_view_mut()
            .expect("应有活动视图")
            .set_wrap_map(Some(WrapMap::new(true, vec![vec![5], vec![4]])));

        app.dispatch_command(editor::insert_text("X")).unwrap();

        let wrap_map = app
            .session
            .active_edit_view()
            .expect("应有活动视图")
            .wrap_map()
            .expect("文本变化后应保留一份保守 wrap map");
        assert_eq!(wrap_map.logical_line_count(), 2);
        assert!(wrap_map.breaks(0).is_empty());
        assert_eq!(wrap_map.breaks(1), &[4]);
    }

    #[test]
    fn desktop_text_input_should_merge_consecutive_same_edit_commands_for_undo_redo() {
        let mut app = app_with_markdown_text("merge-text-input-history", "");

        app.dispatch_command(editor::insert_text("a")).unwrap();
        app.dispatch_command(editor::insert_text("b")).unwrap();
        app.dispatch_command(editor::insert_text("c")).unwrap();

        assert_eq!(active_buffer_text(&app), "abc");

        app.dispatch_command(editor::undo()).unwrap();
        assert_eq!(active_buffer_text(&app), "");

        app.dispatch_command(editor::redo()).unwrap();
        assert_eq!(active_buffer_text(&app), "abc");
    }

    #[test]
    fn text_edit_without_soft_wrap_should_refresh_wrap_map_line_count_immediately() {
        let mut app = app_with_open_file("refresh-nowrap-map");
        app.apply_settings_change_from_effect(SettingsChange::ToggleEditorSoftWrap);
        assert!(!app.config_snapshot().editor.soft_wrap);
        app.session
            .active_edit_view_mut()
            .expect("应有活动视图")
            .set_wrap_map(Some(WrapMap::sparse(false, 1, [])));

        app.dispatch_command(editor::insert_newline()).unwrap();

        let wrap_map = app
            .session
            .active_edit_view()
            .expect("应有活动视图")
            .wrap_map()
            .expect("关闭软换行时应立即刷新逻辑行视觉模型");
        assert!(!wrap_map.soft_wrap());
        assert_eq!(wrap_map.logical_line_count(), 2);
        assert_eq!(wrap_map.total_visual_rows(), 2);
    }

    /// 收集一份 snapshot 内属于 syntax 的 Foreground decoration（`(start, end, name)`）。
    /// 单独抽出 syntax 段方便对比 edit-frame 与 reparse-frame。
    fn syntax_decorations(
        snapshot: &crate::editor::text::EditorSnapshot,
    ) -> Vec<(usize, usize, String)> {
        let mut out: Vec<_> = snapshot
            .decorations
            .iter()
            .filter(|d| {
                d.kind == crate::editor::highlight::DecorationKind::Foreground
                    && d.priority == crate::editor::highlight::priority::SYNTAX
            })
            .filter_map(|d| match &d.style {
                crate::editor::highlight::DecorationStyle(
                    crate::editor::highlight::StyleClass::Syntax(name),
                ) => Some((d.range.start().get(), d.range.end().get(), name.clone())),
                _ => None,
            })
            .collect();
        out.sort();
        out
    }

    /// 不变量：在 token 内部插入字符（结构未变）后立即取 snapshot，syntax
    /// decoration 必须覆盖新插入字节 —— 主线程 `tree.edit` 把 slot 推进到新版本，
    /// paint 端按 viewport 现查 Query 就能命中 shifted node。
    ///
    /// 这条钉死「token 内插入不会闪默认前景色」—— 主线程 `tree_slot.try_edit` 的存在理由。
    #[test]
    fn edit_immediately_extends_syntax_decoration_inside_heading_token() {
        // 在 `# zom 文档规范` 的 `zom` 中间插入字符：byte 4（'z' 与 'o' 之间）。
        // heading_content 节点 [2..18] 跨越插入点，tree.edit 后变为 [2..19]，
        // 第一帧 paint 跑 query 应当命中扩展后的 heading 段，新字节 [4..5) 在内。
        let mut app = app_with_markdown_text("token-inside-edit", "# zom 文档规范\n\n正文。\n");
        *app.session
            .active_edit_view_mut()
            .expect("应有活动视图")
            .selection_mut() = SelectionSet::caret(ByteOffset::new(4));

        app.dispatch_command(editor::replace_selection("X"))
            .unwrap();

        let snapshot = app.with_router(|router| router.snapshot_for_focus(AppFocus::editor()));
        assert!(
            snapshot.decorations.iter().any(|d| d.kind
                == crate::editor::highlight::DecorationKind::Foreground
                && d.range.start().get() <= 4
                && d.range.end().get() >= 5
                && d.priority == crate::editor::highlight::priority::SYNTAX),
            "dispatch 后立即 snapshot 必须包含覆盖新字节 [4, 5) 的 syntax decoration，实际 {:?}",
            snapshot.decorations,
        );
    }

    /// 关键不变量：结构未变的小编辑下，edit 后立即 paint 的 syntax decoration
    /// 必须**逐项等于** worker reparse 完成后的 paint 结果。
    ///
    /// `tree.edit` 只推坐标不改结构 —— interpolate tree 跑出的 query 与重 parse
    /// 后的 query 在 viewport 上应当命中同一组 node。一帧不闪、不糊。
    #[test]
    fn edit_frame_decorations_equal_reparse_frame_for_structure_preserving_edit() {
        let mut app = app_with_markdown_text("no-flash", "# zom 文档规范\n\n正文段落。\n");
        *app.session
            .active_edit_view_mut()
            .expect("应有活动视图")
            .selection_mut() = SelectionSet::caret(ByteOffset::new(4));

        app.dispatch_command(editor::replace_selection("X"))
            .unwrap();

        // edit-frame：worker 还没回包，slot 里只有主线程 tree.edit 推进的 interpolate tree。
        let edit_frame = app.with_router(|router| router.snapshot_for_focus(AppFocus::editor()));
        let edit_frame_syntax = syntax_decorations(&edit_frame);

        // reparse-frame：等 worker 把真正的重 parse 结果 store 回 slot。
        app.session
            .workspace()
            .syntax_worker()
            .wait_for_idle_for_test_or_bench();
        let reparse_frame = app.with_router(|router| router.snapshot_for_focus(AppFocus::editor()));
        let reparse_frame_syntax = syntax_decorations(&reparse_frame);

        assert_eq!(
            edit_frame_syntax, reparse_frame_syntax,
            "结构未变的小编辑下 edit-frame 与 reparse-frame 必须产出相同 syntax decoration —— \
             否则就会出现一帧错色 flash。\n  edit-frame: {edit_frame_syntax:?}\n  reparse-frame: {reparse_frame_syntax:?}",
        );
    }

    #[test]
    fn panel_toggle_command_should_emit_host_effect() {
        let mut app = App::new();

        // 命中 mod shift e → editor 区按下时应被 keymap 消费。
        let outcome = app
            .dispatch_key("mod shift e".to_string())
            .expect("派发成功");
        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![
                HostEffect::TogglePanel(PanelId::FileTree, false),
                HostEffect::EditorCancelPointerSelection,
            ]
        );
    }

    #[test]
    fn search_shortcut_should_emit_activate_effect() {
        let mut app = App::new();
        // 搜索快捷键限定在 text_edit 上下文内；空 focus 不响应。
        app.request_focus(AppFocus::editor());

        let outcome = app.dispatch_key("mod f".to_string()).expect("派发成功");
        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![
                HostEffect::SearchToggle,
                HostEffect::EditorCancelPointerSelection
            ]
        );

        // mod shift f 绑到项目搜索占位命令：弹一条"敬请期待"气泡。
        let outcome = app
            .dispatch_key("mod shift f".to_string())
            .expect("派发成功");
        assert!(outcome.consumed);
        assert_eq!(outcome.effects.len(), 2);
        assert!(matches!(outcome.effects[0], HostEffect::ShowBubble(_)));
        assert!(matches!(
            outcome.effects[1],
            HostEffect::EditorCancelPointerSelection
        ));
    }

    #[test]
    fn panel_key_surface_should_keep_global_shortcuts_without_text_edit_context() {
        let mut app = App::new();
        app.request_focus(AppFocus::panel(PanelId::Terminal));

        let outcome = app
            .dispatch_key("mod shift e".to_string())
            .expect("派发成功");
        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![
                HostEffect::TogglePanel(PanelId::FileTree, false),
                HostEffect::EditorCancelPointerSelection,
            ]
        );

        let outcome = app.dispatch_key("mod a".to_string()).expect("派发成功");
        assert!(!outcome.consumed);
        assert!(outcome.effects.is_empty());
    }

    #[test]
    fn shortcuts_for_should_return_formatted_keymap_binding() {
        let app = App::new();

        // 已绑定的命令：返回格式化后的快捷键。
        let undo = app.shortcuts_for(editor::UNDO).expect("undo 必有快捷键");
        let save = app.shortcuts_for(editor::SAVE).expect("save 必有快捷键");
        let file_tree = app
            .shortcuts_for(PanelId::FileTree.toggle_command_id())
            .expect("file_tree 切换必有快捷键");

        // 平台差异化校验在专门的格式化测试里做；这里只关心"能查到、非空"。
        assert!(!undo.is_empty());
        assert!(!save.is_empty());
        assert!(!file_tree.is_empty());

        let settings = app
            .shortcuts_for(settings::OPEN)
            .expect("settings.open 必有快捷键");
        assert!(!settings.is_empty());

        // 未注册的命令：返回 None。
        assert!(app.shortcuts_for("不存在的命令").is_none());
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

        let outcome = app.dispatch_key("mod o".to_string()).unwrap();

        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![
                HostEffect::ShowProjectPicker,
                HostEffect::EditorCancelPointerSelection,
            ]
        );
    }

    #[test]
    fn open_local_project_command_should_emit_window_action() {
        let mut app = App::new();
        app.request_focus(AppFocus::project_picker());

        let actions = app
            .dispatch_command(project_picker_commands::open_local_project())
            .unwrap();

        assert_eq!(
            actions,
            vec![
                HostEffect::OpenLocalProject,
                HostEffect::EditorCancelPointerSelection,
            ]
        );
    }

    #[test]
    fn project_action_commands_should_have_shortcuts_and_emit_effects() {
        let mut app = App::new();
        let _picker = install_project_picker(&mut app);

        assert!(
            app.shortcuts_for(project_picker_commands::OPEN_LOCAL_PROJECT)
                .is_some()
        );
        assert!(
            app.shortcuts_for(project_picker_commands::START_GIT_CLONE)
                .is_some()
        );
        assert!(
            app.shortcuts_for(project_picker_commands::REMOVE_RECENT_PROJECT)
                .is_some()
        );

        let outcome = app.dispatch_key("down".to_string()).unwrap();
        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![
                HostEffect::ProjectPickerMoveSelection(1),
                HostEffect::EditorCancelPointerSelection,
            ]
        );

        // backspace 落到 picker query 的 text_edit 上下文，由 DELETE 命令处理（删一个字符）。
        // 不是 picker 的导航动作，但仍由 keymap 消费。
        let outcome = app.dispatch_key("backspace".to_string()).unwrap();
        assert!(outcome.consumed);

        let outcome = app.dispatch_key("enter".to_string()).unwrap();
        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![
                HostEffect::ProjectPickerActivate,
                HostEffect::EditorCancelPointerSelection,
            ]
        );

        let actions = app
            .dispatch_command(project_picker_commands::start_git_clone())
            .unwrap();
        assert_eq!(
            actions,
            vec![
                HostEffect::StartGitClone,
                HostEffect::EditorCancelPointerSelection,
            ]
        );

        let actions = app
            .dispatch_command(project_picker_commands::remove_recent_project())
            .unwrap();
        assert_eq!(
            actions,
            vec![
                HostEffect::RemoveSelectedRecentProject,
                HostEffect::EditorCancelPointerSelection,
            ]
        );
    }

    #[test]
    fn file_tree_confirm_delete_focus_should_route_enter_and_escape_to_dialog_actions() {
        let mut app = App::new();
        // Esc 改走 system.dismiss_top 后，必须经 request_delete() 才能把 cancel token 推上栈。
        let _ = app.dispatch_command(file_tree::request_delete()).unwrap();
        app.request_focus(AppFocus::file_tree(FileTreeFocus::ConfirmDelete));
        app.request_focus_from_shell(AppFocus::file_tree(FileTreeFocus::Navigate));
        assert_eq!(
            app.focus().current(),
            AppFocus::file_tree(FileTreeFocus::ConfirmDelete)
        );

        let outcome = app.dispatch_key("enter".to_string()).unwrap();
        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![
                HostEffect::FileTreeConfirmDelete,
                HostEffect::EditorCancelPointerSelection,
            ]
        );

        // enter 提交后栈已被 commit 清空；再 esc 必须重新经 request_delete() 才有 token。
        let _ = app.dispatch_command(file_tree::request_delete()).unwrap();
        app.request_focus(AppFocus::file_tree(FileTreeFocus::Navigate));
        assert_eq!(
            app.focus().current(),
            AppFocus::file_tree(FileTreeFocus::Navigate)
        );
        app.request_focus(AppFocus::file_tree(FileTreeFocus::ConfirmDelete));

        let outcome = app.dispatch_key("escape".to_string()).unwrap();
        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![
                HostEffect::FileTreeCancelDelete,
                HostEffect::EditorCancelPointerSelection,
            ]
        );
    }

    #[test]
    fn project_title_should_prompt_when_no_project_is_open() {
        let app = App::new();

        assert_eq!(app.project_title(), "打开项目");
    }

    // RecentProjects 的 remember / remove / 落盘语义现在归 picker runtime 拥有，
    // 单测落在 project picker recent 模块，App 不再覆盖。

    #[test]
    fn language_server_status_command_should_emit_open_surface_window_action() {
        let mut app = App::new();

        let actions = app.dispatch_command(language_servers::open()).unwrap();

        assert_eq!(
            actions,
            vec![
                HostEffect::ShowLanguageServers,
                HostEffect::EditorCancelPointerSelection,
            ]
        );
    }

    #[test]
    fn settings_and_diagnostics_commands_should_be_registered() {
        let mut app = App::new();

        let actions = app.dispatch_command(settings::open()).unwrap();
        assert_eq!(
            actions,
            vec![
                HostEffect::ShowSettings,
                HostEffect::EditorCancelPointerSelection,
            ]
        );

        let actions = app.dispatch_command(settings::dismiss()).unwrap();
        assert_eq!(
            actions,
            vec![
                HostEffect::DismissSurface,
                HostEffect::EditorCancelPointerSelection,
            ]
        );

        let actions = app.dispatch_command(diagnostics::show_problems()).unwrap();
        assert_eq!(
            actions,
            vec![
                HostEffect::ShowDiagnostics,
                HostEffect::EditorCancelPointerSelection,
            ]
        );
    }

    #[test]
    fn settings_escape_should_dispatch_settings_dismiss_command() {
        // Esc 现在走 system.dismiss_top —— 必须先经 settings::open()
        // push 一条 dismiss token，否则栈空 esc 静默。
        let mut app = App::new();
        let _ = app.dispatch_command(settings::open()).unwrap();
        app.request_focus(AppFocus::settings());

        let outcome = app.dispatch_key("escape".to_string()).unwrap();

        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![
                HostEffect::DismissSurface,
                HostEffect::EditorCancelPointerSelection,
            ]
        );
    }

    #[test]
    fn language_servers_escape_should_dispatch_dismiss_command() {
        let mut app = App::new();
        let _ = app.dispatch_command(language_servers::open()).unwrap();
        app.request_focus(AppFocus::language_servers());

        let outcome = app.dispatch_key("escape".to_string()).unwrap();

        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![
                HostEffect::DismissSurface,
                HostEffect::EditorCancelPointerSelection,
            ]
        );
    }

    #[test]
    fn project_picker_escape_should_dispatch_project_picker_dismiss_command() {
        // Esc 不再静态绑到 DISMISS；现在它走 DISMISS_TOP，先弹 DismissScope::ProjectPicker 的栈顶。
        // 因此真正的取消能力依赖于 SHOW_PROJECTS_PICKER 在打开 picker 时 push 一条
        // dismiss token；没 push（host 走非命令路径直接打开 picker）esc 就静默。
        let mut app = App::new();
        let _picker = install_project_picker(&mut app);
        // 走命令路径打开 picker：SHOW_PROJECTS_PICKER push token + emit ShowProjectPicker。
        let _ = app
            .dispatch_command(project_picker_commands::show_projects_picker())
            .unwrap();
        app.request_focus(AppFocus::project_picker());

        let outcome = app.dispatch_key("escape".to_string()).unwrap();

        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![
                HostEffect::DismissSurface,
                HostEffect::EditorCancelPointerSelection,
            ]
        );
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
    fn tab_commands_should_switch_and_close_active_view() {
        let mut app = App::new();
        let root = project_fixture("tabs");
        app.apply_open_project_from_effect(root.clone());
        assert!(app.session.open_file(root.join("README.md")));
        assert!(app.session.open_file(root.join("src/lib.rs")));

        // 两个标签：README.md 先开、lib.rs 后开且为活动标签。
        let state = editor_state(&app);
        assert_eq!(state.tabs.len(), 2);
        assert_eq!(active_tab(&state).title(), "lib.rs");
        assert!(state.tabs[1].is_active());

        // 切到上一个标签 → README.md。
        app.dispatch_command(editor::select_tab(editor::SelectTabTarget::Previous))
            .unwrap();
        let state = editor_state(&app);
        assert_eq!(active_tab(&state).title(), "README.md");
        assert!(state.tabs[0].is_active());

        // 关闭当前标签 → 只剩 lib.rs。
        app.dispatch_command(editor::close_active_tab()).unwrap();
        let state = editor_state(&app);
        assert_eq!(state.tabs.len(), 1);
        assert_eq!(active_tab(&state).title(), "lib.rs");
    }

    /// EditorTargetRegistry 集成契约：runtime 注册进来的 owner 能被 router
    /// 通过 `accepts_focus` 找到并落到 query / 命令写入路径上。
    ///
    /// 该测试是 §2 拆分 App 字段的基础——后续每个 model 迁出时只需把自己
    /// 注册到 registry，不再在 App struct 上长字段。本用例守住该机制本身。
    mod registry_integration {
        use super::*;
        use crate::editor::text::{
            EditorSnapshot, EditorSnapshotRequest, ImeQueryTarget, OwnedEditorTarget,
        };
        use crate::focus::FileTreeFocus;
        use crate::text_target::{TextTargetOwner, TextTargetQuery};
        use std::cell::RefCell;
        use std::rc::Rc;
        use zom_command::{EditTarget, FileTreeKeyMode, KeyContext};

        /// 自定义 focus 的桩 owner：accepts_focus 只命中一个普通 panel；
        /// after_text_changed 翻一个 flag 让 router 写路径可观察。
        struct StubPanelOwner {
            flag: std::cell::Cell<bool>,
        }

        impl StubPanelOwner {
            fn new() -> Self {
                Self {
                    flag: std::cell::Cell::new(false),
                }
            }
        }

        impl TextTargetQuery for StubPanelOwner {
            fn accepts_focus(&self, focus: AppFocus) -> bool {
                focus == AppFocus::panel(PanelId::KeyboardShortcuts)
            }
            fn snapshot(&self, _focus: AppFocus) -> EditorSnapshot {
                EditorSnapshot::default()
            }
            fn key_contexts(&self) -> Vec<KeyContext> {
                vec![KeyContext::settings(), KeyContext::global()]
            }
            fn ime_query_target(&self, _focus: AppFocus) -> Option<ImeQueryTarget<'_>> {
                None
            }
        }

        impl TextTargetOwner for StubPanelOwner {
            fn edit_target(&mut self, _focus: AppFocus) -> Option<EditTarget<'_>> {
                None
            }
            fn after_text_changed(&mut self) {
                self.flag.set(true);
            }
        }

        struct StubFileTreeNameOwner {
            active: std::cell::Cell<bool>,
            target: OwnedEditorTarget,
        }

        impl StubFileTreeNameOwner {
            fn new() -> Self {
                Self {
                    active: std::cell::Cell::new(false),
                    target: OwnedEditorTarget::new(),
                }
            }

            fn set_active(&self, active: bool) {
                self.active.set(active);
            }

            fn text(&self) -> String {
                self.target.text()
            }
        }

        impl TextTargetQuery for StubFileTreeNameOwner {
            fn accepts_focus(&self, focus: AppFocus) -> bool {
                self.active.get() && focus == AppFocus::file_tree(FileTreeFocus::NewEntryName)
            }

            fn snapshot(&self, _focus: AppFocus) -> EditorSnapshot {
                self.target.snapshot(EditorSnapshotRequest::single_line())
            }

            fn key_contexts(&self) -> Vec<KeyContext> {
                vec![
                    KeyContext::text_edit(false, false),
                    KeyContext::file_tree(FileTreeKeyMode::PendingName),
                    KeyContext::global(),
                ]
            }

            fn ime_query_target(&self, _focus: AppFocus) -> Option<ImeQueryTarget<'_>> {
                Some(self.target.as_ime_query_target())
            }
        }

        impl TextTargetOwner for StubFileTreeNameOwner {
            fn edit_target(&mut self, _focus: AppFocus) -> Option<EditTarget<'_>> {
                Some(self.target.as_edit_target())
            }
        }

        #[test]
        fn registered_owner_is_reachable_via_router_key_contexts() {
            let mut app = App::new();
            let owner = Rc::new(RefCell::new(StubPanelOwner::new()));
            let dyn_owner: Rc<RefCell<dyn TextTargetOwner>> = owner.clone();
            app.install_editor_owner(dyn_owner);

            let focus = AppFocus::panel(PanelId::KeyboardShortcuts);
            let contexts = app.with_router(|router| router.key_contexts_for(focus));
            let contexts = contexts.expect("focus 应被 stub owner 接管");
            assert!(contexts.iter().any(|c| c == &KeyContext::settings()));
        }

        #[test]
        fn registered_owner_does_not_steal_other_focuses() {
            let mut app = App::new();
            let owner: Rc<RefCell<dyn TextTargetOwner>> =
                Rc::new(RefCell::new(StubPanelOwner::new()));
            app.install_editor_owner(owner);

            // Editor focus 不在 stub 的 accepts_focus 范围内——应当落到主编辑区 owner，
            // 主编辑区无活动 view 时仍返回它自己的 key_contexts（accepts_newline=true 的 text_edit 栈）。
            let contexts = app.with_router(|router| router.key_contexts_for(AppFocus::editor()));
            assert!(
                contexts.is_some(),
                "Editor focus 应仍由主编辑区 owner 接管，不被 stub 抢走"
            );
        }

        #[test]
        fn file_tree_inline_focus_survives_coarse_shell_projection() {
            let mut app = App::new();
            let owner = Rc::new(RefCell::new(StubFileTreeNameOwner::new()));
            let dyn_owner: Rc<RefCell<dyn TextTargetOwner>> = owner.clone();
            app.install_editor_owner(dyn_owner);

            owner.borrow().set_active(true);
            app.request_focus(AppFocus::file_tree(FileTreeFocus::NewEntryName));

            // 文件树导航、内联新建、内联重命名共用同一个 GPUI FocusHandle。
            // Shell 反向同步只能看出粗粒度 Navigate；App 需要保留仍有效的输入态，
            // 否则 IME commit 会落回主编辑区并在空工作区报 NoActiveView。
            app.request_focus_from_shell(AppFocus::file_tree(FileTreeFocus::Navigate));
            assert_eq!(
                app.focus().current(),
                AppFocus::file_tree(FileTreeFocus::NewEntryName)
            );

            app.dispatch_command(editor::ime_commit(None, "zom"))
                .unwrap();
            assert_eq!(owner.borrow().text(), "zom");
        }
    }
}
