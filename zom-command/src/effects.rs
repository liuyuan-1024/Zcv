//! 命令系统对宿主的请求语言。
//!
//! handler 不能直接调 GPUI `Window`、改 shell `DockState` 等宿主侧资源，那会让 zom-command 反向依赖 UI / 平台层。
//! 取而代之，handler **emit 一个 `HostEffect`**，宿主在派发结束后翻译成具体动作。
//!
//! `HostEffect` 是**闭合枚举**：新增宿主能力 = 必须在此添加变体，并由宿主补 `apply_host_effect` 的 match 分支。
//! 这是"全部命令集中在 zom-command"的必然代价。
//! 但相比"宿主各处自己 register_*_commands"，集中一份 enum 更可控、可枚举、易测。
//!
//! 不在这里出现的：**编辑文本**。文本类操作（插入、删除、移动、撤销...）
//! 全部直接操作 `CommandContext { workspace, views, queue }`，无需经过
//! HostEffect —— 这些资源本来就在 zom-command 看得到。

/// 搜索面板的开关选项。当前只驱动 UI 状态，搜索后端接入后再参与匹配规则。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchOption {
    CaseSensitive,
    WholeWord,
    Regex,
}

/// 命令处理器请求宿主执行的副作用。**按域分组**，加新变体时贴在对应组下。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostEffect {
    // ===== Window / 平台 =====
    /// 退出整个应用。
    Quit,
    /// 最小化当前窗口。
    Minimize,
    /// 切换当前窗口最大化 / 还原。
    ToggleMaximize,

    // ===== Dock / Panel =====
    /// 切换某个 panel 的显隐。
    ///
    /// `panel_id` 是宿主侧 PanelId 的**字符串形式**，
    /// 例如 `"file_tree"` / `"terminal"`。
    /// 由宿主的 `PanelId::from_str` 解析。
    /// zom-command 不 import 宿主枚举，靠字符串桥接。
    TogglePanel(String),

    // ===== Search =====
    /// 打开搜索面板并把焦点送到查询输入框。
    ///
    /// 行为矩阵（由宿主侧 handler 实现）：
    /// - 隐藏 → 显示 + 聚焦 query
    /// - 已显示 + 焦点不在面板 → 把焦点搬到 query
    /// - 已显示 + 焦点在面板 → 收起，焦点回编辑器
    ///
    /// 第一版只有单文件搜索（per-buffer），不带 scope。
    /// 跨文件搜索后续作为独立 workspace 服务再加，
    /// 那时会引入各自的命令与 effect，不复用本变体。
    SearchActivate,
    /// 切换搜索选项。
    SearchToggleOption(SearchOption),
    /// 选中上一个搜索结果。
    SearchFindPrevious,
    /// 选中下一个搜索结果。
    SearchFindNext,
    /// 替换当前搜索结果。
    SearchReplaceNext,
    /// 替换全部搜索结果。
    SearchReplaceAll,
    /// 把搜索面板焦点移动到下一个输入框。
    SearchFocusNextField,
    /// 把搜索面板焦点移动到上一个输入框。
    SearchFocusPreviousField,
    /// 把焦点从搜索面板退回当前活动编辑器。
    SearchFocusEditor,

    // ===== Editor 视图设置 =====
    /// 翻转主编辑区的软换行开关。
    ///
    /// 这是「视图设置」类副作用：宿主侧维护具体编辑器的渲染开关，
    /// zom-command 只知道「请求翻转」。具体哪个编辑器（主编辑 / 输入框）
    /// 适用由宿主自行规定——MVP 只挂主编辑区。
    EditorToggleSoftWrap,

    // ===== Workspace / Project =====
    /// 顶栏"切换项目"入口；宿主弹出最近项目。
    ShowProjectPicker,
    /// 从本机选择一个文件夹作为当前项目根目录。
    OpenLocalProject,
    /// 进入 Git 地址克隆流程。
    StartGitClone,
    /// 从项目选择器移除当前高亮的最近项目记录。
    RemoveSelectedRecentProject,
    /// 移动项目选择器的高亮项。
    ProjectPickerMoveSelection(isize),
    /// 激活项目选择器当前输入或高亮项。
    ProjectPickerActivate,

    // ===== Surface =====
    /// 打开语言服务器。
    ShowLanguageServers,
    /// 打开设置界面。
    ShowSettings,
    /// 打开诊断问题列表。
    ShowDiagnostics,
    /// 关闭当前浮面。
    DismissSurface,

    // ===== File tree =====
    /// 移动文件树选中行（焦点）。绑定 Up/Down 走 ±1，PageUp/PageDown 走更大步长。
    FileTreeMoveSelection(isize),
    /// 扩展多选选区：当前焦点行加入选区 → 焦点按 delta 移动 → 新焦点行也加入。
    /// 绑定 Shift+Up/Down / Shift+PageUp/PageDown。
    FileTreeExtendSelection(isize),
    /// Esc 二段式：选区非空时清空选区（不离开面板）；否则把焦点交回编辑器。
    /// 是否消化由 model 决定，宿主据此选择是否再走 focus_editor。
    FileTreeEscape,
    /// 折叠当前目录或跳到父目录。
    FileTreeCollapseOrParent,
    /// 展开当前目录或进入子项。
    FileTreeExpandOrInto,
    /// 激活当前文件树条目。
    FileTreeActivate,
    /// 开始新建文件或目录（提交时按输入末尾 `/` 推断类型）。
    FileTreeBeginNewEntry,
    /// 提交正在输入的新建条目。
    FileTreeCommitNewEntry,
    /// 取消正在输入的新建条目。
    FileTreeCancelNewEntry,
    /// 开始重命名当前选中条目（输入框预填原名并全选）。
    FileTreeBeginRename,
    /// 提交正在输入的重命名。
    FileTreeCommitRename,
    /// 取消重命名。
    FileTreeCancelRename,
    /// 请求删除当前选中条目：打开确认弹窗。
    FileTreeRequestDelete,
    /// 确认删除：把待删条目移入回收站。
    FileTreeConfirmDelete,
    /// 取消删除：关闭确认弹窗。
    FileTreeCancelDelete,
    /// 把当前选区（空时降级到焦点）拍进内部剪贴板，模式 Copy。
    FileTreeCopy,
    /// 把当前选区（空时降级到焦点）拍进内部剪贴板，模式 Cut。
    FileTreeCut,
    /// 把剪贴板内容粘贴到焦点所在目录（焦点是文件则其父，无焦点用项目根）。
    /// Cut 模式粘贴完清空剪贴板与选区；Copy 模式保留两者以便连续粘到多处。
    FileTreePaste,
}

/// `CommandContext` 内的 effect 缓冲。
///
/// handler 调用 `ctx.effects.push(...)` emit；宿主在 `CommandExecutor::run` 返回后调用 `drain` 把全部 effect 应用出去。
/// **不在 handler 中应用** —— 那会要求 handler 持 `&mut Host`，破坏命令系统的解耦。
#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct EffectQueue {
    pending: Vec<HostEffect>,
}

impl EffectQueue {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, effect: HostEffect) {
        self.pending.push(effect);
    }

    /// 取走全部待处理 effect，按 push 顺序返回。宿主调用一次后 queue 清空。
    pub fn drain(&mut self) -> Vec<HostEffect> {
        std::mem::take(&mut self.pending)
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}
