//! app —— 组合根（手册 2 / 13）。
//!
//! 组合根持有 `CommandRegistry`、`Keymap`、`Workspace` 与 `ViewSet`，
//! 并把输入统一收敛到 command 管线。
//!
//! 依赖方向（手册 2.4）：`app` 可以 import `shell`；`shell` 不可反向 import `app`。
//! 本文件只做组合根职责；具体功能尽量回到各自 feature / editor / workbench。

use std::cell::{Cell, RefCell};
use std::ops::Range;
use std::path::PathBuf;
use std::rc::Rc;

use zom_command::commands::{self, editor};
use zom_command::{
    ClipboardPort, CommandArgs, CommandContext, CommandError, CommandExecutor, CommandId,
    CommandQueue, CommandRegistry, EffectQueue, FileTreeKeyMode, HostEffect, Invocation, KeyChord,
    KeyContext, Keymap, KeymapResolution, MockClipboard,
};
use zom_view::ViewSet;
use zom_workspace::Workspace;
use zom_workspace::syntax::SyntaxEngine;

use crate::config::{AppConfig, SettingsChange};
use crate::focus::{
    AppFocus, FileTreeFocus, FocusStore, PanelFocus, ProjectPickerFocus, SurfaceFocus,
};
use crate::shell::CommandCatalogItem;
use crate::shell::editor::{
    EditorRouter, EditorRouterMut, EditorTargetRegistry, TextTargetOwner, TextTargetQuery,
};
use crate::shell::features::panels::file_tree::{FileTreeModel, FileTreeState};
use crate::shell::features::panels::search::{self as search_panel, SearchModel, SearchState};
use crate::shell::features::project_picker::{ProjectPickerMode, ProjectPickerModel};
use crate::shell::workbench::editor_area::{MainEditorOwner, MainEditorOwnerRef};
use crate::shell::workbench::state::{EditorState, build_editor_state};

/// 一次按键派发的结果。
/// `consumed=false` 表示这次按键没有匹配任何 keymap 绑定，应当透传给系统输入法；
/// 否则会阻塞 IME 的整个文本输入路径。
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
    focus: FocusStore,
    project_root: Option<PathBuf>,
    file_tree: FileTreeModel,
    project_picker: ProjectPickerModel,
    search: SearchModel,
    /// 剪贴板端口：默认走 [`MockClipboard`]（headless 单测够用且不污染系统剪贴板）。
    /// shell 启动时通过 [`Self::set_clipboard`] 换成 GPUI 适配器，
    /// 使主程序与系统剪贴板互通。
    clipboard: Box<dyn ClipboardPort>,
    /// 全局软换行状态——主编辑区与所有多行嵌入式编辑器都借用本 cell。
    ///
    /// App 是唯一所有者：boot 时按 [`AppConfig::editor::soft_wrap`] 初始化；
    /// `HostEffect::EditorToggleSoftWrap` 与 settings 面板的写入都翻它一次，
    /// 下一帧所有持 [`EditorKernel`] clone 的多行 element 都看到新值。
    soft_wrap: Rc<Cell<bool>>,
    /// 全局用户偏好；运行时翻转的开关（如软换行）以此为初值，
    /// 后续命令路径可调 [`Self::save_config`] 把新值落盘。
    config: AppConfig,
    /// 全局配置的落盘路径；`None` 表示内存模式（单测）。
    config_path: Option<PathBuf>,
    /// 外部 owner 注册表 —— shell runtime 在 [`ShellView::new`] 调
    /// [`Self::install_editor_owner`] 把自家 owner 注册进来；`with_router(_mut)`
    /// 在自家 owner 之外再叠这一批。本字段是 §2 拆分 App 字段的承载点：
    /// 每个迁出的 model 不再需要在 [`App`] struct 上新增一条字段。
    ///
    /// [`ShellView::new`]: crate::shell::view::ShellView
    editor_targets: EditorTargetRegistry,
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
        let mut registry = CommandRegistry::new();
        let mut keymap = Keymap::new();

        // 组合根只选择安装内建命令集；
        // 具体 feature catalog 的完整性由 zom-command 自己维护。
        // 宿主侧资源（窗口、Dock）走 HostEffect 反馈到 shell。
        commands::install_all(&mut registry, &mut keymap);

        let config = AppConfig::load(config_path.as_deref());
        let (_engine, mut workspace, views) = empty_workspace();
        workspace.set_buffer_config(config.buffer_config());
        config.apply_runtime_visuals();
        let soft_wrap = Rc::new(Cell::new(config.editor.soft_wrap));

        Self {
            registry,
            keymap,
            executor: CommandExecutor::new(),
            queue: CommandQueue::new(),
            workspace,
            views,
            // 启动时没有项目、没有 view，shell 显示的是 project picker；
            // 把初始焦点设成 picker 让 App-only 单测不依赖反向同步也能拿到正确语义。
            // 生产里 ShellView::render 的反向同步会把这值刷到与 GPUI 真实焦点一致。
            focus: FocusStore::new(AppFocus::project_picker(ProjectPickerFocus::Query)),
            project_root: None,
            file_tree: FileTreeModel::new(),
            project_picker: ProjectPickerModel::new(),
            search: SearchModel::new(),
            clipboard: Box::new(MockClipboard::new()),
            soft_wrap,
            config,
            config_path,
            editor_targets: EditorTargetRegistry::new(),
        }
    }

    /// 把一个 shell runtime 持有的可嵌入编辑器 owner 注册进路由表。
    /// 由 [`ShellView::new`] 在装配阶段调；运行期通常不会再注册。
    ///
    /// `Rc` 的生命周期归 runtime（真正拥有者）；本结构只持一份共享引用，
    /// 在路由阶段借出做 query / IME 写入。
    ///
    /// [`ShellView::new`]: crate::shell::view::ShellView
    pub(crate) fn install_editor_owner(&mut self, owner: Rc<RefCell<dyn TextTargetOwner>>) {
        self.editor_targets.register(owner);
    }

    /// 把 workspace 当前持有的 `SyntaxEngine` 借给需要它的 shell runtime（首位是 [`SettingsRuntime`]，构造 TOML 编辑器要 engine）。
    ///
    /// 进程里只有一根 engine，由 [`empty_workspace`] 创建并随 `open_project` 跨 workspace 重建持续复用；
    /// 返回 `Rc` clone 不复制内部 state。
    ///
    /// [`SettingsRuntime`]: crate::shell::features::settings::SettingsRuntime
    pub(crate) fn syntax_engine_handle(&self) -> Rc<SyntaxEngine> {
        self.workspace.engine().clone()
    }

    /// 共享的软换行 cell——多行 [`EditorKernel`] 在装配时借这份 `Rc`。
    /// 一次写入对所有持有者立刻可见，下一帧 element 就走新渲染路径。
    pub(crate) fn soft_wrap_handle(&self) -> Rc<Cell<bool>> {
        self.soft_wrap.clone()
    }

    /// 翻转全局软换行。同时更新 [`AppConfig`] 字段并落盘——「切一次软换行」就是「改一次默认值」。
    /// 内存模式（`config_path` 为 `None`）仍在内存里翻，只是 save 是 no-op。
    pub(crate) fn toggle_soft_wrap(&mut self) {
        let next = !self.soft_wrap.get();
        self.soft_wrap.set(next);
        self.config.editor.soft_wrap = next;
        self.save_config();
    }

    /// 把当前内存中的偏好写盘；命令路径如需「显式持久化当前会话偏好」
    /// 可调本入口。toggle_* 类命令已在翻转时各自调过 save，平时无需再喊。
    pub(crate) fn save_config(&self) {
        self.config.save(self.config_path.as_deref());
    }

    pub(crate) fn config_snapshot(&self) -> AppConfig {
        self.config.clone()
    }

    pub(crate) fn config_path(&self) -> Option<PathBuf> {
        self.config_path.clone()
    }

    pub(crate) fn apply_settings_change(&mut self, change: SettingsChange) {
        self.config.apply_change(change);
        self.apply_runtime_config();
        self.save_config();
    }

    /// 用一份外部传入的 `AppConfig` 替换当前 config，并把变化应用到运行时
    /// 视觉 / workspace / soft_wrap 上，最后落盘。
    ///
    /// settings TOML 视图关闭时由 [`ShellView`] 调：[`SettingsRuntime::close_toml_and_parse`]
    /// 解出新 config 后通过本入口提交，避免 shell 侧自己重做"配置应用 + 落盘"。
    ///
    /// [`ShellView`]: crate::shell::view::ShellView
    /// [`SettingsRuntime::close_toml_and_parse`]: crate::shell::features::settings::SettingsRuntime::close_toml_and_parse
    pub(crate) fn replace_config(&mut self, next: AppConfig) {
        self.config = next;
        self.apply_runtime_config();
        self.save_config();
    }

    fn apply_runtime_config(&mut self) {
        self.config.apply_runtime_visuals();
        self.workspace
            .set_buffer_config(self.config.buffer_config());
        self.soft_wrap.set(self.config.editor.soft_wrap);
    }

    pub(crate) fn focus(&self) -> &FocusStore {
        &self.focus
    }

    pub(crate) fn request_focus(&mut self, next: AppFocus) {
        let next = self.refine_focus(next);
        self.focus.request(next);
    }

    pub(crate) fn request_focus_from_shell(&mut self, next: AppFocus) {
        self.request_focus(next);
    }

    pub(crate) fn restore_previous_focus(&mut self) -> AppFocus {
        self.focus.restore_previous()
    }

    fn refine_focus(&self, focus: AppFocus) -> AppFocus {
        match focus {
            AppFocus::Panel(PanelFocus::FileTree(_)) if self.file_tree.pending_delete_active() => {
                AppFocus::file_tree(FileTreeFocus::ConfirmDelete)
            }
            AppFocus::Panel(PanelFocus::FileTree(_)) if self.file_tree.pending_rename_active() => {
                AppFocus::file_tree(FileTreeFocus::RenameEntry)
            }
            AppFocus::Panel(PanelFocus::FileTree(_)) if self.file_tree.pending_active() => {
                AppFocus::file_tree(FileTreeFocus::NewEntryName)
            }
            AppFocus::Panel(PanelFocus::FileTree(_)) => {
                AppFocus::file_tree(FileTreeFocus::Navigate)
            }
            other => other,
        }
    }

    /// 替换默认剪贴板端口。
    /// shell 启动时注入 `GpuiClipboard`，让 copy / cut / paste 走系统剪贴板；
    /// headless 单测保持默认 [`MockClipboard`]。
    pub(crate) fn set_clipboard(&mut self, clipboard: Box<dyn ClipboardPort>) {
        self.clipboard = clipboard;
    }

    /// 把指定 root 切成当前活动项目：重置文件树、重建 workspace / view、聚焦编辑区。
    ///
    /// **不**负责把 root 写到"最近项目"列表 —— 那是 picker 自家的 UI 数据，归
    /// shell 侧的 [`ProjectPickerRuntime`] 拥有；shell 在调用本方法后再调
    /// `runtime.remember_project(root, repo)` 完成登记，repo 信息也由 shell 持有。
    ///
    /// [`ProjectPickerRuntime`]: crate::shell::features::project_picker::ProjectPickerRuntime
    pub(crate) fn open_project(&mut self, root: PathBuf) {
        self.file_tree.open_project(root.clone());
        self.project_root = Some(root);
        // 复用现有 SyntaxEngine——不再新开 worker 线程也不再重注 Tier 1。
        let engine = self.workspace.engine().clone();
        let mut workspace = Workspace::with_engine(engine);
        workspace.set_buffer_config(self.config.buffer_config());
        self.workspace = workspace;
        self.views = ViewSet::new();
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

    pub(crate) fn project_picker_reset(&mut self, mode: ProjectPickerMode) {
        self.project_picker.reset(mode);
        self.request_focus(AppFocus::project_picker(ProjectPickerFocus::Query));
    }

    pub(crate) fn project_picker_deactivate(&mut self) {
        if matches!(
            self.focus.current(),
            AppFocus::Surface(SurfaceFocus::ProjectPicker(_))
        ) {
            self.restore_previous_focus();
        }
    }

    /// 项目选择器 model 的只读视图。shell 拿到后直接调
    /// `.state()` / `.query_text()` / `.selected_project_id(&recent)` 等纯模型查询；
    /// `recent` 列表由调用方从 [`ProjectPickerRuntime`] 取出后传入，App 不再代为
    /// 持有这份 picker UI 数据。
    ///
    /// [`ProjectPickerRuntime`]: crate::shell::features::project_picker::ProjectPickerRuntime
    pub(crate) fn project_picker(&self) -> &ProjectPickerModel {
        &self.project_picker
    }

    /// 项目选择器 model 的可变引用——`move_selection(delta, &recent)` 等需要
    /// recent 列表的写操作，调用方先从 runtime 取 recent 再传入。
    pub(crate) fn project_picker_mut(&mut self) -> &mut ProjectPickerModel {
        &mut self.project_picker
    }

    /// 可变借用文件树 model——仅限不需要 workspace/views 的纯模型操作
    /// （`move_selection`、`escape`、`begin_new_entry`...）。需要 workspace/views
    /// 的写操作走 [`Self::with_file_tree`]。
    pub(crate) fn file_tree_mut(&mut self) -> &mut FileTreeModel {
        &mut self.file_tree
    }

    /// 把文件树 model + workspace + views 三家的可变引用打包递给闭包。
    /// `activate_selected` / `commit_new_entry` / `confirm_delete` /
    /// `paste_from_clipboard` 等需要同时改 model 与 workspace/views 的写操作走这里。
    pub(crate) fn with_file_tree<R>(
        &mut self,
        f: impl FnOnce(&mut FileTreeModel, &mut Workspace, &mut ViewSet) -> R,
    ) -> R {
        f(&mut self.file_tree, &mut self.workspace, &mut self.views)
    }

    pub(crate) fn file_tree_state(&self) -> FileTreeState {
        self.file_tree.state(&self.workspace)
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
        let chord = KeyChord::new(chord)?;
        let contexts = self.key_contexts();
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
    /// `composing` 恒为 `false`：`dispatch_key` 在组合态直接让位给系统输入法，根本不会走到这里。
    /// 组合上下文（`KeyContext::text_edit` 的第二参）保留在签名里，
    /// 待将来真有「宿主侧处理组合键」的需求再启用。
    fn key_contexts(&self) -> Vec<KeyContext> {
        let focus = self.focus.current();
        // 先问 router —— 文本输入类 owner（主编辑区、文件树新建/重命名、搜索框、
        // picker 查询框、settings TOML 视图）通过 `accepts_focus` 自报家门，
        // 由 owner 自己说"我的栈是什么"。owner 不接（focus 不属于它 / settings
        // TOML 视图未开）才落到下方按焦点类别给的兜底栈。
        if let Some(stack) = self.with_router(|router| router.key_contexts_for(focus)) {
            return stack;
        }
        match focus {
            AppFocus::None => vec![KeyContext::global()],
            AppFocus::Editor(_) => vec![KeyContext::global()],
            AppFocus::Panel(PanelFocus::Search(_)) => vec![KeyContext::global()],
            AppFocus::Surface(SurfaceFocus::ProjectPicker(ProjectPickerFocus::Query)) => {
                vec![KeyContext::global()]
            }
            AppFocus::Surface(SurfaceFocus::ProjectPicker(_)) => {
                vec![KeyContext::project_picker(), KeyContext::global()]
            }
            // settings TOML 未开（accepts_focus 返回 false）→ 落到面板自身的栈。
            AppFocus::Surface(SurfaceFocus::Settings) => {
                vec![KeyContext::settings(), KeyContext::global()]
            }
            AppFocus::Panel(PanelFocus::FileTree(FileTreeFocus::ConfirmDelete)) => vec![
                // 删除确认弹窗打开中：只解析确认 / 取消，导航键全部冻结。
                KeyContext::file_tree(FileTreeKeyMode::PendingDelete),
                KeyContext::global(),
            ],
            AppFocus::Panel(PanelFocus::FileTree(
                FileTreeFocus::NewEntryName | FileTreeFocus::RenameEntry,
            )) => vec![KeyContext::global()],
            AppFocus::Panel(PanelFocus::FileTree(_)) => vec![
                KeyContext::file_tree(FileTreeKeyMode::Navigate),
                KeyContext::global(),
            ],
            AppFocus::Panel(_) | AppFocus::Surface(_) => vec![KeyContext::global()],
        }
    }

    /// 当前聚焦的编辑目标是否处于「有 preedit 的」输入法组合态。
    ///
    /// 空 preedit 不算 —— 系统输入法取消候选后会把 preedit 清空、但 composition
    /// 壳可能仍在。若空壳也算组合态，`dispatch_key` 会一直让位、keymap 再也
    /// 不接管，后续 Esc 永远到不了 `cancel_new_entry`。
    fn is_composing(&self) -> bool {
        let focus = self.focus.current();
        self.with_router(|router| router.is_composing(focus))
    }

    /// 构造一次只读路由视图。
    ///
    /// 这是 editor 子系统访问 App 内部状态的唯一桥：调用方拿到 [`EditorRouter`]
    /// 后直接做 IME 查询 / focused target / snapshot 等 —— App 不再为每种查询
    /// 包一层方法。
    ///
    /// Owners 顺序：先 App 自己持有的 model（迁出过程中陆续清空）、再注册表里
    /// 由 shell runtime 注入的 owner、最后兜底主编辑区。`accepts_focus` 对
    /// `AppFocus` 精确匹配，各 owner 覆盖 disjoint 子集，顺序不影响命中。
    pub(crate) fn with_router<R>(&self, f: impl FnOnce(EditorRouter<'_>) -> R) -> R {
        let main = MainEditorOwnerRef::new(&self.workspace, &self.views);
        let search_query = self.search.query_owner();
        let search_replacement = self.search.replacement_owner();
        // 注册表 owner 的借用 guard 必须比下方 owners 引用活得长。
        let registry_borrows = self.editor_targets.borrow_all();
        let mut owners: Vec<&dyn TextTargetQuery> = vec![
            &self.project_picker as &dyn TextTargetQuery,
            &self.file_tree as &dyn TextTargetQuery,
            &search_query as &dyn TextTargetQuery,
            &search_replacement as &dyn TextTargetQuery,
        ];
        // settings_toml 由 SettingsRuntime 通过 `install_editor_owner` 注册到
        // registry，不再出现在这条手工列表里。
        for borrow in registry_borrows.iter() {
            owners.push(&**borrow as &dyn TextTargetQuery);
        }
        owners.push(&main as &dyn TextTargetQuery);
        f(EditorRouter::new(owners))
    }

    /// 构造一次可写路由视图。
    ///
    /// Owners 通过 `accepts_focus` 对 [`AppFocus`] 精确匹配，各自覆盖 disjoint 的
    /// focus 子集，vec 顺序与优先级无关。搜索面板在写路径由单个 `SearchActiveOwner`
    /// 同时承担 Query / Replacement 两个 field——这是 `&mut self.search` 借用无法
    /// 拆分导致的，不是优先级问题。
    pub(crate) fn with_router_mut<R>(&mut self, f: impl FnOnce(EditorRouterMut<'_>) -> R) -> R {
        let focus = self.focus.current();
        let mut main = MainEditorOwner::new(&mut self.workspace, &mut self.views);
        let mut search = self.search.active_owner(focus);
        // 注册表 owner 的可变借用 guard 必须比下方 owners 引用活得长；每个 RefMut
        // 独立锁住自己那一格 RefCell，互不冲突。
        let mut registry_borrows = self.editor_targets.borrow_all_mut();
        let mut owners: Vec<&mut dyn TextTargetOwner> = vec![
            &mut self.project_picker as &mut dyn TextTargetOwner,
            &mut self.file_tree as &mut dyn TextTargetOwner,
            &mut search as &mut dyn TextTargetOwner,
        ];
        // settings_toml 由 SettingsRuntime 通过 `install_editor_owner` 注册到
        // registry，不再出现在这条手工列表里。
        for borrow in registry_borrows.iter_mut() {
            owners.push(&mut **borrow as &mut dyn TextTargetOwner);
        }
        owners.push(&mut main as &mut dyn TextTargetOwner);
        f(EditorRouterMut::new(owners))
    }

    /// 由主编辑区 element prepaint 末尾回写：把它实际测得的视口写回当前活动 view，
    /// 下一帧 `View::settle_viewport_y` 与 snapshot 切片用更准的行数 / sub-row。
    /// 无活动 view 时静默忽略。
    pub(crate) fn set_main_viewport(
        &mut self,
        viewport: zom_view::ViewportState,
        wrap_map: Option<zom_view::WrapMap>,
    ) {
        let Some(view) = self.views.active_view_mut() else {
            return;
        };
        let current = view.viewport();
        if current != viewport {
            view.set_viewport(viewport);
        }
        view.set_wrap_map(wrap_map);
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
        // 读路径走 `&self`，不在这里跑 sync——sync 在写路径（dispatch_command_id / IME）完成。
        // 这里读到的就是上次 sync 完的真值。
        let mut state = self.search.state();
        state.hit_count = search_panel::coordinator::current_hit_count(&self.workspace);
        state
    }

    /// 把 panel + 活动 buffer + 活动 view 三家的可变引用打包递给协调器。
    /// effects.rs 收到 `Search*` HostEffect 后通过它调用
    /// [`search_panel::coordinator`] 里的具体动作（find_next / replace_all /
    /// on_panel_opened ...），App 自身不再持有搜索协调逻辑。
    pub(crate) fn with_search_coordinator<R>(
        &mut self,
        f: impl FnOnce(&mut SearchModel, &mut Workspace, &mut ViewSet) -> R,
    ) -> R {
        f(&mut self.search, &mut self.workspace, &mut self.views)
    }

    /// 命令派发尾部 / IME preedit 更新后必跑一次：把 panel 状态推进活动 buffer
    /// 的 BufferSearch。算法在
    /// [`search_panel::coordinator::sync_active_buffer_search`]，这里只是组合
    /// 根侧的私有触发点。
    fn sync_active_buffer_search(&mut self) {
        search_panel::coordinator::sync_active_buffer_search(
            &mut self.search,
            &mut self.workspace,
            &mut self.views,
        );
    }

    /// 排空活动 buffer 自上次 dispatch 以来累积的 `DeltaEvent`，扇出到
    /// `BufferSearch` 与 syntax provider。**无论搜索面板是否开**都要调——
    /// 否则编辑后 syntax layer 不重算 / 不 remap，渲染端读到旧版本的 span
    /// 与新字节叠在一起就是错位的着色。
    ///
    /// 在 dispatch_command_id 与 ime preedit update 两个尾部都装一次。
    /// 多调几次无害——`take_pending_events` 第二次返空。
    fn pump_active_buffer_post_edit(&mut self) {
        if let Some(wb) = self.workspace.active_buffer_mut() {
            let _ = wb.pump_post_edit();
        }
    }

    /// 每帧 prepaint 起手由 [`ShellView::render`] 调一次，把后台
    /// `SyntaxWorker` 已就绪的高亮产物落到 workspace 各 buffer 的 `MetadataLayers`。
    ///
    /// 主工作区 + 嵌入式编辑器共享同一根后台 worker；本方法只 drain 主 workspace
    /// 的 sink。settings 面板的 toml 编辑器在
    /// [`SettingsRuntime::pump_pending_highlights`] 独立 drain，由 shell 渲染期
    /// 并排调一次；两边互不重入。不阻塞——内部全是「拿锁、看空、放锁」级
    /// 操作，worker 没出新产物即 O(1) 无操作。详见
    /// [改造方案 §3.7](../../zom-workspace/docs/语法高亮异步增量改造.md)。
    ///
    /// [`SettingsRuntime::pump_pending_highlights`]: crate::shell::features::settings::SettingsRuntime::pump_pending_highlights
    pub fn pump_pending_highlights(&mut self) {
        self.workspace.pump_pending_highlights();
    }

    /// 每帧 prepaint 起手再调一次：收割活动 buffer 的后台 BufferSearch 结果。
    /// 没有 in-flight 时 O(1) 早退。新结果落地时同时 reveal 首条命中——避免
    /// 用户输入查询后 UI 不刷新的"看上去卡住"假象。
    ///
    /// 与 `pump_pending_highlights` 平级：两个独立后台子系统，各自有"主线程收割"
    /// 入口，统一在 [`crate::shell::view::ShellView::render`] 拍点驱动。
    pub fn pump_pending_search(&mut self) {
        search_panel::coordinator::pump_active_buffer_search(&mut self.workspace, &mut self.views);
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
        let Some(view) = self.views.active_view() else {
            return;
        };
        let buffer_id = view.buffer();
        let viewport = view.viewport();
        let Some(wb) = self.workspace.buffer(buffer_id) else {
            return;
        };
        let snapshot = wb.buffer().snapshot();
        let total_lines = snapshot.line_count();
        if total_lines == 0 {
            return;
        }
        const PAD_LINES: u64 = 32;
        let start_line = viewport.top_line.saturating_sub(PAD_LINES);
        let raw_end = viewport
            .top_line
            .saturating_add(viewport.visible_logical_lines)
            .saturating_add(PAD_LINES);
        let end_line = raw_end.min(total_lines as u64);
        if start_line >= end_line {
            return;
        }
        let Ok(start_byte) = snapshot.line_start_byte(zom_engine::Line::new(start_line as usize))
        else {
            return;
        };
        let end_byte = if end_line >= total_lines as u64 {
            snapshot.len_bytes()
        } else {
            match snapshot.line_start_byte(zom_engine::Line::new(end_line as usize)) {
                Ok(b) => b,
                Err(_) => snapshot.len_bytes(),
            }
        };
        if start_byte >= end_byte {
            return;
        }
        let Ok(range) = zom_engine::TextRange::new(start_byte, end_byte) else {
            return;
        };
        self.workspace
            .set_buffer_viewport_hint(buffer_id, Some(range));
    }

    fn dispatch_command_id(
        &mut self,
        id: CommandId,
        args: CommandArgs,
    ) -> Result<Vec<HostEffect>, CommandError> {
        self.queue.dispatch(id, args);

        let focus = self.focus.current();

        // picker 焦点下，命令执行可能改了 query 文本
        // （DELETE / 粘贴等走 edit_target，绕过 router 的 after_text_changed 钩子）。
        // 派发前后比一次 query 文本：变了才 reset_selection，否则保留。
        // 否则 MOVE_SELECTION 自己也会被无差别 reset，选中项只能在 0 / 1 之间来回。
        let picker_query_before =
            matches!(focus, AppFocus::Surface(SurfaceFocus::ProjectPicker(_)))
                .then(|| self.project_picker.query_text());

        // 命令派发期需要给 CommandContext 填 `focused_field`，并在 executor 跑完
        // 之后给 owner 调 `after_text_changed`（picker 走另一条 query diff 路径）。
        //
        // 自家字段（picker / file_tree / search）直接借；其它语义焦点（Settings 之类）
        // 走 editor_targets 注册表 —— 先借出所有 RefMut，找到 accepts_focus 命中的
        // 那格，记下索引，跑完 executor 后回该 owner 调 after_text_changed。
        //
        // 整段包在内层 block：registry_borrows 出 block 才 drop，RefCell 借用释放，
        // 外层后续的 `pump_active_buffer_post_edit` / `sync_active_buffer_search`
        // 才能拿到 `&mut self`。
        let host_effects = {
            let mut effects = EffectQueue::new();
            let mut registry_borrows = self.editor_targets.borrow_all_mut();
            let mut registry_matched: Option<usize> = None;

            let focused_field = match focus {
                AppFocus::Surface(SurfaceFocus::ProjectPicker(_)) => {
                    self.project_picker.edit_target()
                }
                AppFocus::Panel(PanelFocus::FileTree(_)) => self.file_tree.edit_target(),
                AppFocus::Panel(PanelFocus::Search(_)) => self.search.edit_target_for_focus(focus),
                _ => {
                    // 在 registry 找第一个声明接管该 focus 的 owner，borrow_mut 拿 edit_target。
                    // 没有命中（focus 不是文本输入类）返回 None，commands 自己处理。
                    let mut found = None;
                    for (idx, borrow) in registry_borrows.iter_mut().enumerate() {
                        if borrow.accepts_focus(focus) {
                            if let Some(target) = borrow.edit_target() {
                                registry_matched = Some(idx);
                                found = Some(target);
                            }
                            break;
                        }
                    }
                    found
                }
            };
            let mut context = CommandContext {
                workspace: &mut self.workspace,
                views: &mut self.views,
                focused_field,
                queue: &mut self.queue,
                effects: &mut effects,
                clipboard: &mut *self.clipboard,
            };
            let result = self.executor.run(&self.registry, &mut context);

            let host_effects = effects.drain();
            result?;
            if let Some(idx) = registry_matched {
                registry_borrows[idx].after_text_changed();
            }
            host_effects
        };

        if let Some(before) = picker_query_before {
            if self.project_picker.query_text() != before {
                self.project_picker.reset_selection();
            }
        }

        // 命令派发可能编辑了活动 buffer（产生 DeltaEvent），扇出给 BufferSearch 与 syntax provider 是无条件的。
        // 否则编辑后高亮 / 搜索命中都不跟版本。
        // 必须先于 `sync_active_buffer_search`：后者依赖搜索状态已被新事件推进。
        self.pump_active_buffer_post_edit();
        // 命令派发也可能改了 panel 的 query 文本（在搜索框内按键 / 退格 / 粘贴等）。
        // 把 panel 状态推进活动 buffer 的 BufferSearch 并 sync——一处做完，渲染 / 后续命令读到的都是新真值。
        self.sync_active_buffer_search();
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
        self.pump_active_buffer_post_edit();
        self.sync_active_buffer_search();
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
/// 启动期同时把 Tier 1 syntax provider 工厂注入共享 [`SyntaxEngine`]——
/// 否则后续 `open_file` 落 plain。注册需要在 `Rc::new(engine)` 之前完成。
/// 主工作区与 [`SettingsTomlEditor`] 共享同一根 `Rc`，进程里只有这一份
/// 语言注册表与一根后台 worker 线程。
fn empty_workspace() -> (Rc<SyntaxEngine>, Workspace, ViewSet) {
    let mut engine = SyntaxEngine::new();
    crate::shell::editor::highlight::install_tier1(&mut engine);
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
    //! 这一层不接触 GPUI ——只覆盖 keymap 解析、命令派发、IME 流，以及
    //! 命令产出的 HostEffect。需要 GPUI 句柄（Entity / Window / 焦点等）的链路
    //! 在 `shell::view` 那一层做手工 / 集成测试，不进本文件。

    use crate::app::App;
    use crate::config::{SettingsChange, THEME_ONE_DARK};
    use crate::focus::{AppFocus, PanelFocus, ProjectPickerFocus};
    use crate::shell::editor::TextTargetOwner;
    use crate::shell::features::panels::PanelId;
    use crate::shell::features::panels::file_tree::FileTreeActivation;
    use crate::shell::features::settings::SettingsTomlEditor;
    use crate::shell::workbench::state::{EditorState, EditorTab};
    use std::cell::RefCell;
    use std::fs::{File, create_dir_all};
    use std::path::PathBuf;
    use std::rc::Rc;
    use zom_command::HostEffect;
    use zom_command::commands::{
        diagnostics, editor, language_servers, project_picker as project_picker_commands, settings,
    };
    use zom_workspace::EntryKind;

    /// 取当前活动标签——断言「编辑区正在显示哪个文件」用。
    fn active_tab(state: &EditorState) -> &EditorTab {
        state
            .tabs
            .iter()
            .find(|tab| tab.is_active)
            .expect("应有活动标签")
    }

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!("zom-app-{tag}-{}.toml", std::process::id()))
    }

    /// 构造一个已打开项目并激活了一个空文件的 `App`。
    ///
    /// 不再有默认空白 buffer，编辑管线测试必须先真实打开一个文件才有活动 buffer。
    /// 复用 `project_fixture`：rows 为 `[root, src, README.md]`，走到 README.md
    /// 并 activate。
    fn app_with_open_file(name: &str) -> App {
        let mut app = App::new();
        app.open_project(project_fixture(name));
        app.file_tree_mut().move_selection(1); // root
        app.file_tree_mut().move_selection(1); // src
        app.file_tree_mut().move_selection(1); // README.md
        assert_eq!(
            app.with_file_tree(|ft, ws, vs| ft.activate_selected(ws, vs)),
            FileTreeActivation::OpenedFile
        );
        app
    }

    #[test]
    fn tab_and_enter_should_dispatch_editor_commands() {
        let mut app = app_with_open_file("tab-enter");

        assert!(app.dispatch_key("tab".to_string()).unwrap().consumed);
        assert!(app.dispatch_key("enter".to_string()).unwrap().consumed);
        assert!(app.dispatch_key("return".to_string()).unwrap().consumed);

        let state = app.editor_state();

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

        let loaded = crate::config::AppConfig::load(Some(&path));
        assert_eq!(loaded, config);

        let _ = std::fs::remove_file(path);
    }

    /// 在 headless 测试里模拟 SettingsRuntime 装配过：直接 new 一个编辑器，
    /// 走 install_editor_owner 进 App 的路由表，返回 handle 给测试驱动 open/close。
    ///
    /// 生产路径是 ShellView::new 调 `SettingsRuntime::new(engine, cx)` + register。
    /// 这里跳过 GPUI 部分，直接复刻"register 一个 owner + 拿 handle 操作"两步。
    fn install_settings_toml(app: &mut App) -> Rc<RefCell<SettingsTomlEditor>> {
        let editor = Rc::new(RefCell::new(SettingsTomlEditor::new(
            app.syntax_engine_handle(),
        )));
        app.install_editor_owner(editor.clone() as Rc<RefCell<dyn TextTargetOwner>>);
        editor
    }

    #[test]
    fn settings_toml_editor_should_apply_and_persist_on_return() {
        // 验证「编辑器里编辑过的 TOML → 解析回 AppConfig → 替换 App.config →
        // 落盘 → 重新 load 仍是新值」这条端到端语义。
        //
        // open/close 的 lifecycle 编排归 shell（SettingsRuntime + 设置面板 action
        // handler），App 只暴露原语；测试手动走等价的几步：
        //   1. open_from_disk —— 把当前内存 config 灌进编辑器
        //   2. target_mut().replace_text —— 直接改文本
        //   3. close_and_parse —— 关闭并解析
        //   4. replace_config —— App 替换全局 config 并落盘
        let path = temp_path("settings-toml-editor");
        let _ = std::fs::remove_file(&path);
        let mut app = App::new_with_paths(Some(path.clone()));
        let editor = install_settings_toml(&mut app);

        let initial = app.config_snapshot();
        {
            let mut e = editor.borrow_mut();
            e.open_from_disk(&path, &initial);
            assert!(e.is_open());
            e.target_mut().replace_text(
                r#"
[general]
theme = "one-dark"

[ui]
font_size = 15

[editor]
soft_wrap = false
font_size = 19
tab_size = 8
"#,
            );
        }
        let parsed = editor
            .borrow_mut()
            .close_and_parse()
            .expect("合法 TOML 应当解析成功");
        app.replace_config(parsed);

        let config = app.config_snapshot();
        assert!(!editor.borrow().is_open());
        assert_eq!(config.ui.font_size, 15);
        assert!(!config.editor.soft_wrap);
        assert_eq!(config.editor.font_size, 19);
        assert_eq!(config.editor.tab_size, 8);
        assert_eq!(crate::config::AppConfig::load(Some(&path)), config);

        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn settings_toml_editor_snapshot_should_include_syntax_decorations() {
        let mut app = App::new();
        let editor = install_settings_toml(&mut app);
        {
            let mut e = editor.borrow_mut();
            e.open_with_text("[editor]\nsoft_wrap = false\nfont_size = 19 # comment\n");
            e.target_mut().wait_for_syntax_idle();
        }
        // 手动推一次高亮 sink：app.pump 只 drain workspace，settings 编辑器自带
        // pump（生产里 SettingsRuntime.pump_pending_highlights 调）。
        editor.borrow_mut().pump_pending_highlights();
        let snapshot = app.with_router(|router| router.snapshot_for_focus(AppFocus::settings()));

        assert!(
            snapshot.decorations.iter().any(|decoration| {
                matches!(
                    decoration.kind,
                    crate::shell::editor::highlight::DecorationKind::Foreground
                )
            }),
            "settings TOML editor should push syntax foreground decorations"
        );
    }

    #[test]
    fn editor_tab_size_setting_should_reach_open_buffers() {
        let mut app = App::new();
        app.apply_settings_change(SettingsChange::CycleEditorTabSize);
        app.open_project(project_fixture("tab-size-setting"));
        app.file_tree_mut().move_selection(1); // root
        app.file_tree_mut().move_selection(1); // src
        app.file_tree_mut().move_selection(1); // README.md

        assert_eq!(
            app.with_file_tree(|ft, ws, vs| ft.activate_selected(ws, vs)),
            FileTreeActivation::OpenedFile
        );

        let tab_width = app
            .workspace
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
        app.request_focus(AppFocus::Panel(PanelFocus::Terminal));

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

        let outcome = app.dispatch_key("mod-o".to_string()).unwrap();

        assert!(outcome.consumed);
        assert_eq!(outcome.effects, vec![HostEffect::ShowProjectPicker]);
    }

    #[test]
    fn open_local_project_command_should_emit_window_action() {
        let mut app = App::new();
        app.request_focus(AppFocus::project_picker(ProjectPickerFocus::RecentList));

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
    fn project_title_should_prompt_when_no_project_is_open() {
        let app = App::new();

        assert_eq!(app.project_title(), "打开项目");
    }

    // RecentProjects 的 remember / remove / 落盘语义现在归 picker runtime 拥有，
    // 单测落在 `shell::features::project_picker::recent::tests`，App 不再覆盖。

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
    fn project_picker_escape_should_dispatch_project_picker_dismiss_command() {
        let mut app = App::new();
        app.request_focus(AppFocus::project_picker(ProjectPickerFocus::Query));

        let outcome = app.dispatch_key("escape".to_string()).unwrap();

        assert!(outcome.consumed);
        assert_eq!(outcome.effects, vec![HostEffect::DismissSurface]);
    }

    #[test]
    fn settings_escape_should_dispatch_settings_dismiss_command() {
        let mut app = App::new();
        app.request_focus(AppFocus::settings());

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
    fn file_tree_move_selection_should_walk_visible_rows_in_order() {
        let mut app = App::new();
        app.open_project(project_fixture("move"));

        assert!(app.file_tree_state().selected.is_none());

        // rows: [root, src, README.md]
        app.file_tree_mut().move_selection(1);
        let state = app.file_tree_state();
        assert_eq!(state.selected.as_ref(), Some(&state.rows[0].path));

        app.file_tree_mut().move_selection(1);
        let state = app.file_tree_state();
        assert_eq!(state.selected.as_ref(), Some(&state.rows[1].path));

        app.file_tree_mut().move_selection(1);
        let state = app.file_tree_state();
        assert_eq!(state.selected.as_ref(), Some(&state.rows[2].path));

        // 已在末位时再 down 不会越界。
        app.file_tree_mut().move_selection(1);
        let state = app.file_tree_state();
        assert_eq!(state.selected.as_ref(), Some(&state.rows[2].path));
    }

    #[test]
    fn file_tree_focus_initialization_should_select_first_visible_row() {
        let mut app = App::new();
        app.open_project(project_fixture("focus-init"));

        app.file_tree_mut().ensure_selection_initialized();

        let state = app.file_tree_state();
        assert_eq!(state.selected.as_ref(), Some(&state.rows[0].path));
    }

    #[test]
    fn file_tree_expand_then_collapse_should_round_trip_via_selection_keys() {
        let mut app = App::new();
        let root = project_fixture("expand");
        app.open_project(root.clone());

        // 初始 rows: [root, src, README.md]，根默认展开。
        let state = app.file_tree_state();
        assert_eq!(state.rows.len(), 3);

        // 选到 src（root → src）。
        app.file_tree_mut().move_selection(1);
        app.file_tree_mut().move_selection(1);
        assert_eq!(
            app.file_tree_state().selected.as_deref(),
            Some(root.join("src").as_path())
        );

        app.file_tree_mut().expand_or_into();
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

        app.file_tree_mut().collapse_or_parent();
        assert_eq!(app.file_tree_state().rows.len(), 3);
    }

    #[test]
    fn file_tree_activate_on_file_should_open_buffer_and_report_opened() {
        let mut app = App::new();
        let root = project_fixture("activate");
        app.open_project(root.clone());

        // rows: [root, src, README.md] —— 走到 README.md。
        app.file_tree_mut().move_selection(1); // root
        app.file_tree_mut().move_selection(1); // src
        app.file_tree_mut().move_selection(1); // README.md
        let selected = app.file_tree_state().selected.clone();
        assert_eq!(selected.as_deref(), Some(root.join("README.md").as_path()));

        let action = app.with_file_tree(|ft, ws, vs| ft.activate_selected(ws, vs));
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
        app.open_project(root.clone());

        // rows: [root, src, README.md] —— 选到 src。
        app.file_tree_mut().move_selection(1); // root
        app.file_tree_mut().move_selection(1); // src
        let action = app.with_file_tree(|ft, ws, vs| ft.activate_selected(ws, vs));
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
    fn tab_commands_should_switch_and_close_active_view() {
        let mut app = App::new();
        app.open_project(project_fixture("tabs"));

        // 打开 README.md：rows = [root, src, README.md]。
        app.file_tree_mut().move_selection(1); // root
        app.file_tree_mut().move_selection(1); // src
        app.file_tree_mut().move_selection(1); // README.md
        assert_eq!(
            app.with_file_tree(|ft, ws, vs| ft.activate_selected(ws, vs)),
            FileTreeActivation::OpenedFile
        );

        // 展开 src 并打开 src/lib.rs：
        // 展开后 rows = [root, src, inner, lib.rs, README.md]。
        app.file_tree_mut().move_selection(-1); // 回到 src
        app.file_tree_mut().expand_or_into(); // 展开 src
        app.file_tree_mut().move_selection(1); // inner
        app.file_tree_mut().move_selection(1); // lib.rs
        assert_eq!(
            app.with_file_tree(|ft, ws, vs| ft.activate_selected(ws, vs)),
            FileTreeActivation::OpenedFile
        );

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

    /// EditorTargetRegistry 集成契约：runtime 注册进来的 owner 能被 router
    /// 通过 `accepts_focus` 找到并落到 query / IME 写入路径上。
    ///
    /// 该测试是 §2 拆分 App 字段的基础——后续每个 model 迁出时只需把自己
    /// 注册到 registry，不再在 App struct 上长字段。本用例守住该机制本身。
    mod registry_integration {
        use super::*;
        use crate::shell::editor::{
            EditorSnapshot, ImeQueryTarget, ImeTarget, TextTargetOwner, TextTargetQuery,
        };
        use std::cell::RefCell;
        use std::rc::Rc;
        use zom_command::{EditTarget, KeyContext};

        /// 自定义 focus 的桩 owner：accepts_focus 只命中 Settings；
        /// after_text_changed 翻一个 flag 让 router 写路径可观察。
        struct StubSettingsOwner {
            flag: std::cell::Cell<bool>,
        }

        impl StubSettingsOwner {
            fn new() -> Self {
                Self {
                    flag: std::cell::Cell::new(false),
                }
            }
        }

        impl TextTargetQuery for StubSettingsOwner {
            fn accepts_focus(&self, focus: AppFocus) -> bool {
                focus == AppFocus::settings()
            }
            fn snapshot(&self) -> EditorSnapshot {
                EditorSnapshot::default()
            }
            fn key_contexts(&self) -> Vec<KeyContext> {
                vec![KeyContext::settings(), KeyContext::global()]
            }
            fn ime_query_target(&self) -> Option<ImeQueryTarget<'_>> {
                None
            }
        }

        impl TextTargetOwner for StubSettingsOwner {
            fn ime_target(&mut self) -> Option<ImeTarget<'_>> {
                None
            }
            fn edit_target(&mut self) -> Option<EditTarget<'_>> {
                None
            }
            fn after_text_changed(&mut self) {
                self.flag.set(true);
            }
        }

        #[test]
        fn registered_owner_is_reachable_via_router_key_contexts() {
            let mut app = App::new();
            let owner = Rc::new(RefCell::new(StubSettingsOwner::new()));
            let dyn_owner: Rc<RefCell<dyn TextTargetOwner>> = owner.clone();
            app.install_editor_owner(dyn_owner);

            // router 现在该把 Settings focus 路由到注册进来的 owner，并取它的 key_contexts。
            let contexts = app.with_router(|router| router.key_contexts_for(AppFocus::settings()));
            // 当前 App 仍持有 settings_toml model（未迁），accepts_focus 在 toml 未开时返回 false，
            // 因此 Settings focus 由 stub owner 接管。
            let contexts = contexts.expect("Settings focus 应被 stub owner 接管");
            assert!(contexts.iter().any(|c| c == &KeyContext::settings()));
        }

        #[test]
        fn registered_owner_does_not_steal_other_focuses() {
            let mut app = App::new();
            let owner: Rc<RefCell<dyn TextTargetOwner>> =
                Rc::new(RefCell::new(StubSettingsOwner::new()));
            app.install_editor_owner(owner);

            // Editor focus 不在 stub 的 accepts_focus 范围内——应当落到主编辑区 owner，
            // 主编辑区无活动 view 时仍返回它自己的 key_contexts（accepts_newline=true 的 text_edit 栈）。
            let contexts = app.with_router(|router| router.key_contexts_for(AppFocus::editor()));
            assert!(
                contexts.is_some(),
                "Editor focus 应仍由主编辑区 owner 接管，不被 stub 抢走"
            );
        }
    }
}
