//! 命令系统对宿主的请求语言。
//!
//! handler 不能直接调 GPUI `Window`、改 shell `DockState` 等宿主侧资源 ——
//! 那会让 zom-command 反向依赖 UI / 平台层。取而代之，handler **emit 一个
//! `HostEffect`**，宿主在派发结束后翻译成具体动作。
//!
//! `HostEffect` 是**闭合枚举**：新增宿主能力 = 必须在此添加变体 + 宿主补
//! `apply_host_effect` 的 match 分支。这是"全部命令集中在 zom-command"的
//! 必然代价 —— 但相比"宿主各处自己 register_*_commands"，集中一份 enum
//! 更可控、可枚举、易测。
//!
//! 不在这里出现的：**编辑文本**。文本类操作（插入、删除、移动、撤销...）
//! 全部直接操作 `CommandContext { workspace, views, queue }`，无需经过
//! HostEffect —— 这些资源本来就在 zom-command 看得到。

use zom_workspace::EntryKind;

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
    /// `panel_id` 是宿主侧 PanelId 的**字符串形式**（如 `"file_tree"` /
    /// `"terminal"`），由宿主的 `PanelId::from_str` 解析。zom-command
    /// 不 import 宿主枚举，靠字符串桥接。
    TogglePanel(String),

    // ===== Workspace / Project =====
    /// 顶栏"切换项目"入口；宿主弹出最近项目悬浮面板。
    ShowProjectPicker,
    /// 从本机选择一个文件夹作为当前项目根目录。
    OpenLocalProject,

    // ===== Overlay =====
    /// 打开语言服务器悬浮层。
    ShowLanguageServers,
    /// 关闭当前悬浮层。
    DismissOverlay,

    // ===== File tree =====
    /// 移动文件树选中行。
    FileTreeMoveSelection(isize),
    /// 折叠当前目录或跳到父目录。
    FileTreeCollapseOrParent,
    /// 展开当前目录或进入子项。
    FileTreeExpandOrInto,
    /// 激活当前文件树条目。
    FileTreeActivate,
    /// 把焦点交回主编辑区。
    FileTreeFocusEditor,
    /// 开始新建文件 / 目录。
    FileTreeBeginNewEntry(EntryKind),
    /// 提交正在输入的新建条目。
    FileTreeCommitNewEntry,
    /// 取消正在输入的新建条目。
    FileTreeCancelNewEntry,
}

/// `CommandContext` 内的 effect 缓冲。
///
/// handler 调用 `ctx.effects.push(...)` emit；宿主在 `CommandExecutor::run`
/// 返回后调用 `drain` 把全部 effect 应用出去。**不在 handler 中应用** ——
/// 那会要求 handler 持 `&mut Host`，破坏命令系统的解耦。
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
