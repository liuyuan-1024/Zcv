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
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;

use zom_command::commands::editor;
use zom_command::{
    ClipboardPort, CommandArgs, CommandCatalogItem, CommandError, CommandId, FileTreeKeyMode,
    HostEffect, Invocation, KeyContext, KeymapResolution,
};
use zom_view::ViewSet;
use zom_workspace::Workspace;
use zom_workspace::syntax::{SyntaxEngine, install_builtin_providers};

use self::command_runtime::CommandRuntime;
use self::config_applier::ConfigApplier;
use self::config_store::ConfigStore;
use self::pumps::BackgroundPumps;
use self::text_target_runtime::TextTargetRuntime;
use crate::config::{AppConfig, SettingsChange};
use crate::dispatch::KeyDispatchOutcome;
use crate::focus::{AppFocus, FileTreeFocus, FocusStore, PanelSubFocus};
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
    file_tree: Option<Box<dyn FileTreeHost>>,
    search: Option<Box<dyn SearchHost>>,
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
            file_tree: None,
            search: None,
        }
    }

    /// 把一个 shell runtime 持有的可嵌入编辑器 owner 注册进路由表。
    /// 由 shell 根视图在装配阶段调；运行期通常不会再注册。
    ///
    /// `Rc` 的生命周期归 runtime（真正拥有者）；本结构只持一份共享引用，
    /// 在路由阶段借出做 query / IME 写入。
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

    /// 注册文件树动作端口。文件树模型归 shell feature runtime 持有；
    /// app 只通过这个端口组合命令动作与 [`WorkspaceSession`]。
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
            self.session
                .push_bubble(zom_command::BubbleRequest::error(error).dedupe("config.save"));
        }
    }

    /// 把真实的 config.toml 打开到主编辑区。首次打开前先保存当前配置，保证文件存在；
    /// 后续语言识别、语法高亮、保存都走主工作区的普通文件路径。
    pub(crate) fn open_config_file(&mut self) -> bool {
        let Some(path) = self.config.path() else {
            self.session.push_bubble(
                zom_command::BubbleRequest::info("当前为内存配置模式，没有可打开的 config.toml")
                    .dedupe("config.open"),
            );
            return false;
        };
        if let Err(error) = self.config.save() {
            self.session
                .push_bubble(zom_command::BubbleRequest::error(error).dedupe("config.save"));
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

    pub(crate) fn apply_settings_change(&mut self, change: SettingsChange) {
        self.config.apply_change(change);
        ConfigApplier::apply_to_session(self.config.config(), &mut self.session);
        if let Err(error) = self.config.save() {
            self.session
                .push_bubble(zom_command::BubbleRequest::error(error).dedupe("config.save"));
        }
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
    /// headless 单测保持默认 [`MockClipboard`]。
    pub(crate) fn set_clipboard(&mut self, clipboard: Box<dyn ClipboardPort>) {
        self.command.set_clipboard(clipboard);
    }

    /// 把指定 root 切成当前活动项目：重建 workspace / view、聚焦编辑区。
    ///
    /// **不**负责把 root 写到"最近项目"列表 —— 那是 picker 自家的 UI 数据，归 project picker runtime 拥有；
    /// shell 在调用本方法后再调 `runtime.remember_project(root, repo)` 完成登记，repo 信息也由 shell 持有。
    pub(crate) fn open_project(&mut self, root: PathBuf) {
        self.project_root = Some(root.clone());
        self.session.reset_project(self.config.buffer_config());
        // 项目打开后焦点转入编辑区。生产里 picker 关闭后由 shell 的dismiss + 反向同步刷到这里；
        // 这里显式写一遍是给 App-only 单测兜底。
        self.request_focus(AppFocus::editor());
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

    /// 取走 session 累积的气泡请求；调用方负责把它们落到 BubbleRuntime。
    pub(crate) fn take_session_bubbles(&mut self) -> Vec<zom_command::BubbleRequest> {
        self.session.take_bubbles()
    }

    pub(crate) fn with_workspace_views<R>(&self, f: impl FnOnce(&Workspace, &ViewSet) -> R) -> R {
        f(self.session.workspace(), self.session.views())
    }

    pub(crate) fn apply_search_action(&mut self, action: SearchAction) {
        if let Some(search) = &self.search {
            search.apply_search_action(action, &mut self.session);
        }
    }

    pub(crate) fn apply_file_tree_action(
        &mut self,
        action: FileTreeAction,
    ) -> FileTreeActionResult {
        let Some(file_tree) = &self.file_tree else {
            return FileTreeActionResult::default();
        };
        file_tree.apply_file_tree_action(action, &mut self.session)
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
    /// 组合根按当前唯一焦点 / 运行态算出 `KeyContext` 栈交给 keymap 解析 ——
    /// 命令与快捷键的定义全在 zom-command，宿主不持有任何 chord → 动作 的映射表。
    ///
    /// 文本输入不在这里兜底：交给 GPUI 的 `EntityInputHandler` 路径，
    /// 由系统输入法或 NSTextInputClient 把文本喂给 `App::ime_*`。
    pub(crate) fn dispatch_key(
        &mut self,
        chord: String,
    ) -> Result<KeyDispatchOutcome, CommandError> {
        // 组合态下宿主完全让位给系统输入法：不解析、不消费、不 stop_propagation。
        // 一旦拦下某个键（如 Esc → ime_cancel），系统 IME 会话就和我们脱节，它会再吞掉一个后续按键 —— 表现为「取消候选后要多按一次 Esc 才退出新建」。
        // 组合的更新 / 提交 / 取消都由 IME 回调（`ime_*`）驱动。
        if self.is_composing() {
            return Ok(KeyDispatchOutcome {
                consumed: false,
                effects: Vec::new(),
            });
        }
        let contexts = self.key_contexts();
        match self.command.resolve_key(chord, &contexts)? {
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
    /// `composing` 恒为 `false`：`dispatch_key` 在组合态直接让位给系统输入法，根本不会走到这里。
    /// 组合上下文（`KeyContext::text_edit` 的第二参）保留在签名里，
    /// 待将来真有「宿主侧处理组合键」的需求再启用。
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
            AppFocus::Panel(p) => match p.sub {
                PanelSubFocus::Search(_) => vec![KeyContext::global()],
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
            },
        }
    }

    /// 当前聚焦的编辑目标是否处于「有 preedit 的」输入法组合态。
    ///
    /// 空 preedit 不算 —— 系统输入法取消候选后会把 preedit 清空、但 composition
    /// 壳可能仍在。若空壳也算组合态，`dispatch_key` 会一直让位、keymap 再也
    /// 不接管，后续 Esc 永远到不了 `cancel_new_entry`。
    fn is_composing(&self) -> bool {
        let focus = self.focus.current();
        self.text_targets.is_composing(&self.session, focus)
    }

    /// 构造一次只读路由视图。
    ///
    /// 这是 editor 子系统访问 App 内部状态的唯一桥：调用方拿到 [`EditorRouter`]
    /// 后直接做 IME 查询 / focused target / snapshot 等 —— App 不再为每种查询
    /// 包一层方法。
    ///
    /// Owners 顺序：先 search runtime 提供的双输入框 owner、再注册表里由 shell
    /// runtime 注入的 owner、最后兜底主编辑区。`accepts_focus` 对 `AppFocus`
    /// 精确匹配，各 owner 覆盖 disjoint 子集，顺序不影响命中。
    pub(crate) fn with_router<R>(&self, f: impl FnOnce(EditorRouter<'_>) -> R) -> R {
        self.text_targets.with_router(&self.session, f)
    }

    /// 构造一次可写路由视图。
    ///
    /// Owners 通过 `accepts_focus` 对 [`AppFocus`] 精确匹配，各自覆盖 disjoint 的
    /// focus 子集，vec 顺序与优先级无关。搜索面板在写路径由 search runtime
    /// 提供单个 active owner，同时承担 Query / Replacement 两个 field。
    pub(crate) fn with_router_mut<R>(&mut self, f: impl FnOnce(EditorRouterMut<'_>) -> R) -> R {
        let focus = self.focus.current();
        self.text_targets
            .with_router_mut(focus, &mut self.session, f)
    }

    /// 由主编辑区 element prepaint 末尾回写：把它实际测得的视口写回当前活动 view，
    /// 下一帧 `View::settle_viewport_y` 与 snapshot 切片用更准的行数 / sub-row。
    /// 无活动 view 时静默忽略。
    pub(crate) fn set_main_viewport(
        &mut self,
        viewport: zom_view::ViewportState,
        wrap_map: Option<zom_view::WrapMap>,
    ) {
        let Some(view) = self.session.views_mut().active_view_mut() else {
            return;
        };
        let current = view.viewport();
        if current != viewport {
            view.set_viewport(viewport);
        }
        view.set_wrap_map(wrap_map);
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

    /// 排空活动 buffer 自上次 dispatch 以来累积的 `DeltaEvent`，扇出到 `BufferSearch` 与 syntax provider。
    /// **无论搜索面板是否开**都要调——
    /// 否则编辑后 syntax layer 不重算 / 不 remap，渲染端读到旧版本的 span 与新字节叠在一起就是错位的着色。
    ///
    /// 在 dispatch_command_id 与 ime preedit update 两个尾部都装一次。
    /// 多调几次无害——`take_pending_events` 第二次返空。
    /// 每帧 prepaint 起手由 [`ShellView::render`] 调一次，把后台 `SyntaxWorker` 已就绪的高亮产物落到 workspace 各 buffer 的 `MetadataLayers`。
    ///
    /// 主工作区共享同一根后台 worker；本方法只 drain 主 workspace 的 sink。
    /// 不阻塞——内部全是「拿锁、看空、放锁」级操作，worker 没出新产物即 O(1) 无操作。
    /// 详见 [改造方案 §3.7](../../zom-workspace/docs/语法高亮异步增量改造.md)。
    pub fn pump_pending_highlights(&mut self) {
        self.background.pump_pending_highlights(&mut self.session);
    }

    /// 每帧 prepaint 起手再调一次：收割活动 buffer 的后台 BufferSearch 结果。
    /// 没有 in-flight 时 O(1) 早退。新结果落地时同时 reveal 首条命中——避免
    /// 用户输入查询后 UI 不刷新的"看上去卡住"假象。
    ///
    /// 与 `pump_pending_highlights` 平级：跑所有通过 [`install_frame_pump`] 注册的[`FramePump`]。
    /// 原本 search 的"收割后台命中"是这里第一个登记者，
    /// 后续 feature 想做同节奏的 drain 也走同一注册路径——BackgroundPumps 不认具体 feature。
    ///
    /// 统一由 shell 根视图的 render 拍点驱动。
    ///
    /// [`install_frame_pump`]: Self::install_frame_pump
    pub fn pump_frame_observers(&mut self) {
        self.background.pump_frame_observers(&mut self.session);
    }

    /// 把活动 view 的可见区间转成 byte range 后推给语法 worker，让 `on_edit`
    /// 走 viewport-scoped query + `ReplaceRange`（[改造方案 §3.6](
    /// ../../zom-workspace/docs/语法高亮异步增量改造.md)）。
    ///
    /// padding ±32 行：tree-sitter `set_byte_range` 只返回起止 byte 都落在范围内
    /// 的匹配——viewport 边缘的多行字符串、宏调用、属性等 capture 若起点恰
    /// 好被切在外面就会缺色，多吃几十行可视区域以外的查询代价换不撕裂；视觉上
    /// 1 帧 ~30 行 vs 32 行 padding 几乎一致，但语义安全。
    ///
    /// 每帧调一次；HighlightWorker 内部对相同 hint 去重，无变化时不再产物。
    pub fn pump_active_viewport_hint(&mut self) {
        self.background.pump_active_viewport_hint(&mut self.session);
    }

    fn dispatch_command_id(
        &mut self,
        id: CommandId,
        args: CommandArgs,
    ) -> Result<Vec<HostEffect>, CommandError> {
        let focus = self.focus.current();

        // 命令派发期需要给 CommandContext 填 `focused_field`。若命令真的改了
        // 这个输入目标的文本，派发后再让 owner 跑 `after_text_changed`。
        let run_command = |focused_field: Option<zom_command::EditTarget<'_>>| {
            self.command.dispatch_command_id(
                id.clone(),
                args.clone(),
                &mut self.session,
                focused_field,
            )
        };

        let host_effects = self
            .text_targets
            .with_edit_target_for_focus(focus, run_command)?;

        // 命令派发可能编辑了活动 buffer（产生 DeltaEvent），扇出给 BufferSearch 与 syntax provider 是无条件的。
        // 否则编辑后高亮 / 搜索命中都不跟版本。
        // 必须先于 `sync_active_buffer_search`：后者依赖搜索状态已被新事件推进。
        self.background.after_text_edit(&mut self.session);
        // 命令派发也可能改了 panel 的 query 文本（在搜索框内按键 / 退格 / 粘贴等）。
        // 把 panel 状态推进活动 buffer 的 BufferSearch 并 sync——一处做完，渲染 / 后续命令读到的都是新真值。
        Ok(host_effects)
    }

    /// 提交系统输入法文本。commit 走命令路径，保证进入 undo 历史。
    ///
    /// 写入成功后由 router 调 owner 的 `after_text_changed` 钩子 —— picker
    /// 等需要"文本变了就重置选区"的 owner 自己实现，宿主不必特判。
    pub(crate) fn ime_replace_text_for(
        &mut self,
        focus: AppFocus,
        replacement_range_utf16: Option<Range<usize>>,
        text: &str,
    ) -> Result<(), CommandError> {
        self.with_router_mut(|router| {
            router.with_ime_target(focus, |mut target| {
                target.apply_replacement_range(replacement_range_utf16)
            })
        })?;

        self.dispatch(editor::ime_commit(text))?;
        Ok(())
    }

    /// 更新输入法 preedit。update 走直接通道，避免每次按键都过命令队列。
    pub(crate) fn ime_replace_and_mark_text_for(
        &mut self,
        focus: AppFocus,
        replacement_range_utf16: Option<Range<usize>>,
        new_text: &str,
        new_selected_range_utf16: Option<Range<usize>>,
    ) -> Result<(), CommandError> {
        let result = self.with_router_mut(|router| {
            router.with_ime_target(focus, |mut target| {
                target.replace_and_mark_text(
                    replacement_range_utf16,
                    new_text,
                    new_selected_range_utf16,
                )
            })
        });
        // preedit 期间也走 live search——用户在搜索框中文输入时能边输入边看到结果收敛。
        // 同时把 buffer 上累积的 DeltaEvent 扇出到 syntax provider
        // （preedit replace 走 composition state，但 replace_and_mark 内部仍可能产生编辑事件）。
        self.background.after_text_edit(&mut self.session);
        result
    }

    pub(crate) fn ime_unmark_for(&mut self, focus: AppFocus) -> Result<(), CommandError> {
        let Some(preedit) = self.with_router(|router| router.preedit_text(focus)) else {
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
///
/// 启动期同时把内置 syntax provider 工厂注入共享 [`SyntaxEngine`]
/// ——否则后续 `open_file` 落 plain。注册需要在 `Rc::new(engine)` 之前完成。
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

#[cfg(test)]
mod tests {
    //! `App` 派发管线的 headless 单元测试。
    //!
    //! 这一层不接触 GPUI ——只覆盖 keymap 解析、命令派发、IME 流，以及命令产出的 HostEffect。
    //! 需要 GPUI 句柄（Entity / Window / 焦点等）的链路在 shell 根视图那一层做手工 / 集成测试，不进本文件。

    use crate::app::App;
    use crate::config::{SettingsChange, THEME_ONE_DARK};
    use crate::editor_state::{EditorState, EditorTab, build_editor_state};
    use crate::focus::{AppFocus, FileTreeFocus};
    use crate::text_target::{TextTargetOwner, TextTargetQuery};
    use crate::ui_id::PanelId;
    use std::cell::RefCell;
    use std::fs::{File, create_dir_all};
    use std::path::PathBuf;
    use std::rc::Rc;
    use zom_command::HostEffect;
    use zom_command::commands::{
        diagnostics, editor, language_servers, project_picker as project_picker_commands, settings,
    };
    use zom_command::{EditTarget, KeyContext};

    /// 取当前活动标签——断言「编辑区正在显示哪个文件」用。
    fn active_tab(state: &EditorState) -> &EditorTab {
        state
            .tabs
            .iter()
            .find(|tab| tab.is_active)
            .expect("应有活动标签")
    }

    fn editor_state(app: &App) -> EditorState {
        app.with_workspace_views(build_editor_state)
    }

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("zom-app-{tag}-{}.toml", std::process::id()))
    }

    /// 构造一个已打开项目并激活了一个空文件的 `App`。
    fn app_with_open_file(name: &str) -> App {
        let mut app = App::new();
        let root = project_fixture(name);
        app.open_project(root.clone());
        assert!(app.session.open_file(root.join("README.md")));
        app.request_focus(AppFocus::editor());
        app
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
        fn ime_target(&mut self, _focus: AppFocus) -> Option<crate::editor::text::ImeTarget<'_>> {
            Some(self.query.as_ime_target())
        }

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
    fn tab_and_enter_should_dispatch_editor_commands() {
        let mut app = app_with_open_file("tab-enter");

        assert!(app.dispatch_key("tab".to_string()).unwrap().consumed);
        assert!(app.dispatch_key("enter".to_string()).unwrap().consumed);
        assert!(app.dispatch_key("return".to_string()).unwrap().consumed);

        let state = editor_state(&app);

        assert!(active_tab(&state).dirty);
    }

    #[test]
    fn settings_changes_should_update_runtime_config_and_persist() {
        let path = temp_path("settings-change");
        let _ = std::fs::remove_file(&path);
        let mut app = App::new_with_paths(Some(path.clone()));

        app.apply_settings_change(SettingsChange::AdjustUiFont(1));
        app.apply_settings_change(SettingsChange::AdjustEditorFont(2));
        app.apply_settings_change(SettingsChange::ToggleEditorSoftWrap);

        let config = app.config_snapshot();
        assert_eq!(config.general.theme, THEME_ONE_DARK);
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

        assert!(app.open_config_file());
        assert!(path.exists());
        assert_eq!(app.focus().current(), AppFocus::editor());

        let state = editor_state(&app);
        let active = active_tab(&state);
        assert_eq!(
            active.title,
            path.file_name()
                .expect("临时配置路径应有文件名")
                .to_string_lossy()
                .into_owned()
        );
        assert_eq!(active.language, "TOML");

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn editor_tab_size_setting_should_reach_open_buffers() {
        let mut app = App::new();
        app.apply_settings_change(SettingsChange::CycleEditorTabSize);
        let root = project_fixture("tab-size-setting");
        app.open_project(root.clone());
        assert!(app.session.open_file(root.join("README.md")));

        let tab_width = app
            .workspace()
            .active_buffer()
            .expect("应有活动 buffer")
            .buffer()
            .config()
            .tab
            .tab_width();
        assert_eq!(tab_width, 6);
    }

    #[test]
    fn panel_toggle_command_should_emit_host_effect() {
        let mut app = App::new();

        // 命中 mod-shift-e → editor 区按下时应被 keymap 消费。
        let outcome = app
            .dispatch_key("mod-shift-e".to_string())
            .expect("派发成功");
        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![HostEffect::TogglePanel("file_tree".to_string())]
        );
    }

    #[test]
    fn search_shortcut_should_emit_activate_effect() {
        let mut app = App::new();

        let outcome = app.dispatch_key("mod-f".to_string()).expect("派发成功");
        assert!(outcome.consumed);
        assert_eq!(outcome.effects, vec![HostEffect::SearchActivate]);

        // mod-shift-f 当前没有绑定（项目级搜索尚未引入）；不被消费。
        let outcome = app
            .dispatch_key("mod-shift-f".to_string())
            .expect("派发成功");
        assert!(!outcome.consumed);
        assert!(outcome.effects.is_empty());
    }

    #[test]
    fn panel_key_surface_should_keep_global_shortcuts_without_text_edit_context() {
        let mut app = App::new();
        app.request_focus(AppFocus::panel(PanelId::Terminal));

        let outcome = app
            .dispatch_key("mod-shift-e".to_string())
            .expect("派发成功");
        assert!(outcome.consumed);
        assert_eq!(
            outcome.effects,
            vec![HostEffect::TogglePanel("file_tree".to_string())]
        );

        let outcome = app.dispatch_key("mod-a".to_string()).expect("派发成功");
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

        let outcome = app.dispatch_key("mod-o".to_string()).unwrap();

        assert!(outcome.consumed);
        assert_eq!(outcome.effects, vec![HostEffect::ShowProjectPicker]);
    }

    #[test]
    fn open_local_project_command_should_emit_window_action() {
        let mut app = App::new();
        app.request_focus(AppFocus::project_picker());

        let actions = app
            .dispatch(project_picker_commands::open_local_project())
            .unwrap();

        assert_eq!(actions, vec![HostEffect::OpenLocalProject]);
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
            vec![HostEffect::ProjectPickerMoveSelection(1)]
        );

        // backspace 落到 picker query 的 text_edit 上下文，由 DELETE 命令处理（删一个字符）。
        // 不是 picker 的导航动作，但仍由 keymap 消费。
        let outcome = app.dispatch_key("backspace".to_string()).unwrap();
        assert!(outcome.consumed);

        let outcome = app.dispatch_key("enter".to_string()).unwrap();
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
    fn file_tree_confirm_delete_focus_should_route_enter_and_escape_to_dialog_actions() {
        let mut app = App::new();
        app.request_focus(AppFocus::file_tree(FileTreeFocus::ConfirmDelete));
        app.request_focus_from_shell(AppFocus::file_tree(FileTreeFocus::Navigate));
        assert_eq!(
            app.focus().current(),
            AppFocus::file_tree(FileTreeFocus::ConfirmDelete)
        );

        let outcome = app.dispatch_key("enter".to_string()).unwrap();
        assert!(outcome.consumed);
        assert_eq!(outcome.effects, vec![HostEffect::FileTreeConfirmDelete]);

        app.request_focus(AppFocus::file_tree(FileTreeFocus::Navigate));
        assert_eq!(
            app.focus().current(),
            AppFocus::file_tree(FileTreeFocus::Navigate)
        );
        app.request_focus(AppFocus::file_tree(FileTreeFocus::ConfirmDelete));

        let outcome = app.dispatch_key("escape".to_string()).unwrap();
        assert!(outcome.consumed);
        assert_eq!(outcome.effects, vec![HostEffect::FileTreeCancelDelete]);
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

        let actions = app.dispatch(language_servers::open_status()).unwrap();

        assert_eq!(actions, vec![HostEffect::ShowLanguageServers]);
    }

    #[test]
    fn settings_and_diagnostics_commands_should_be_registered() {
        let mut app = App::new();

        let actions = app.dispatch(settings::open()).unwrap();
        assert_eq!(actions, vec![HostEffect::ShowSettings]);

        let actions = app.dispatch(settings::dismiss()).unwrap();
        assert_eq!(actions, vec![HostEffect::DismissSurface]);

        let actions = app.dispatch(diagnostics::show_problems()).unwrap();
        assert_eq!(actions, vec![HostEffect::ShowDiagnostics]);
    }

    #[test]
    fn settings_escape_should_dispatch_settings_dismiss_command() {
        let mut app = App::new();
        app.request_focus(AppFocus::settings());

        let outcome = app.dispatch_key("escape".to_string()).unwrap();

        assert!(outcome.consumed);
        assert_eq!(outcome.effects, vec![HostEffect::DismissSurface]);
    }

    #[test]
    fn language_servers_escape_should_dispatch_dismiss_command() {
        let mut app = App::new();
        app.request_focus(AppFocus::language_servers());

        let outcome = app.dispatch_key("escape".to_string()).unwrap();

        assert!(outcome.consumed);
        assert_eq!(outcome.effects, vec![HostEffect::DismissSurface]);
    }

    #[test]
    fn project_picker_escape_should_dispatch_project_picker_dismiss_command() {
        let mut app = App::new();
        let _picker = install_project_picker(&mut app);
        app.request_focus(AppFocus::project_picker());

        let outcome = app.dispatch_key("escape".to_string()).unwrap();

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
    fn tab_commands_should_switch_and_close_active_view() {
        let mut app = App::new();
        let root = project_fixture("tabs");
        app.open_project(root.clone());
        assert!(app.session.open_file(root.join("README.md")));
        assert!(app.session.open_file(root.join("src/lib.rs")));

        // 两个标签：README.md 先开、lib.rs 后开且为活动标签。
        let state = editor_state(&app);
        assert_eq!(state.tabs.len(), 2);
        assert_eq!(active_tab(&state).title, "lib.rs");
        assert!(state.tabs[1].is_active);

        // 切到上一个标签 → README.md。
        app.dispatch(editor::select_tab(editor::SelectTabTarget::Previous))
            .unwrap();
        let state = editor_state(&app);
        assert_eq!(active_tab(&state).title, "README.md");
        assert!(state.tabs[0].is_active);

        // 关闭当前标签 → 只剩 lib.rs。
        app.dispatch(editor::close_tab()).unwrap();
        let state = editor_state(&app);
        assert_eq!(state.tabs.len(), 1);
        assert_eq!(active_tab(&state).title, "lib.rs");
    }

    /// EditorTargetRegistry 集成契约：runtime 注册进来的 owner 能被 router
    /// 通过 `accepts_focus` 找到并落到 query / IME 写入路径上。
    ///
    /// 该测试是 §2 拆分 App 字段的基础——后续每个 model 迁出时只需把自己
    /// 注册到 registry，不再在 App struct 上长字段。本用例守住该机制本身。
    mod registry_integration {
        use super::*;
        use crate::editor::text::{
            EditorSnapshot, EditorSnapshotRequest, ImeQueryTarget, ImeTarget, OwnedEditorTarget,
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
            fn ime_target(&mut self, _focus: AppFocus) -> Option<ImeTarget<'_>> {
                None
            }
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
            fn ime_target(&mut self, _focus: AppFocus) -> Option<ImeTarget<'_>> {
                Some(self.target.as_ime_target())
            }

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

            let focus = app.focus().current();
            app.ime_replace_text_for(focus, None, "zom").unwrap();
            assert_eq!(owner.borrow().text(), "zom");
        }
    }
}
