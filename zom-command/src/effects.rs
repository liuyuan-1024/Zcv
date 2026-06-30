//! 命令系统对宿主的请求语言。
//!
//! handler 不能直接调 GPUI `Window`、改 shell `DockState` 等宿主侧资源，那会让 zom-command 反向依赖 UI / 平台层。
//! 取而代之，handler **emit 一个 `HostEffect`**，宿主在派发结束后翻译成具体动作。
//!
//! `HostEffect` 按 **feature 分组**：每个 feature 拥有自己的子枚举，加新 feature 只需在此加一行包装变体，
//! 不需要改动已有 feature 的子枚举。宿主 dispacher 按子枚举类型路由到对应 handler。
//!
//! 不在这里出现的：**编辑文本**。文本类操作（插入、删除、移动、撤销...）
//! 全部直接操作 `CommandContext { workspace, views, queue }`，无需经过 HostEffect。

use zom_workspace::BufferId;
use zom_workspace::view::ViewId;

// ── 共享原语 ──────────────────────────────────────────────────

/// 搜索面板的开关选项。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchOption {
    CaseSensitive,
    WholeWord,
    Regex,
}

/// 设置界面的宿主侧变更请求。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsChangeRequest {
    AdjustUiFont(i16),
    AdjustEditorFont(i16),
    ToggleEditorSoftWrap,
    CycleEditorTabSize,
    CycleTheme,
}

/// 内建 panel 的稳定标识。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub enum PanelKind {
    FileTree,
    VersionControl,
    Outline,
    Terminal,
    Debug,
    KeyboardShortcuts,
}

impl PanelKind {
    pub const fn toggle_command_id(self) -> &'static str {
        match self {
            PanelKind::FileTree => "panel.toggle.file_tree",
            PanelKind::VersionControl => "panel.toggle.version_control",
            PanelKind::Outline => "panel.toggle.outline",
            PanelKind::Terminal => "panel.toggle.terminal",
            PanelKind::Debug => "panel.toggle.debug",
            PanelKind::KeyboardShortcuts => "panel.toggle.keyboard_shortcuts",
        }
    }

    pub const fn slug(self) -> &'static str {
        match self {
            PanelKind::FileTree => "file_tree",
            PanelKind::VersionControl => "version_control",
            PanelKind::Outline => "outline",
            PanelKind::Terminal => "terminal",
            PanelKind::Debug => "debug",
            PanelKind::KeyboardShortcuts => "keyboard_shortcuts",
        }
    }
}

/// 轻量气泡提示类型。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BubbleKind {
    Info,
    Success,
    Warning,
    Error,
}

/// 请求宿主显示一条轻量气泡提示。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BubbleRequest {
    pub kind: BubbleKind,
    pub message: String,
    pub dedupe_key: Option<String>,
    pub ttl_ms: Option<u64>,
}

impl BubbleRequest {
    pub fn info(message: impl Into<String>) -> Self {
        Self::new(BubbleKind::Info, message)
    }

    pub fn success(message: impl Into<String>) -> Self {
        Self::new(BubbleKind::Success, message)
    }

    pub fn warning(message: impl Into<String>) -> Self {
        Self::new(BubbleKind::Warning, message)
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self::new(BubbleKind::Error, message)
    }

    pub fn new(kind: BubbleKind, message: impl Into<String>) -> Self {
        Self {
            kind,
            message: message.into(),
            dedupe_key: None,
            ttl_ms: Some(2400),
        }
    }

    pub fn dedupe(mut self, key: impl Into<String>) -> Self {
        self.dedupe_key = Some(key.into());
        self
    }

    pub fn ttl_ms(mut self, ttl_ms: u64) -> Self {
        self.ttl_ms = Some(ttl_ms);
        self
    }

    pub fn persistent(mut self) -> Self {
        self.ttl_ms = None;
        self
    }
}

// ── Feature 子枚举 ─────────────────────────────────────────────

/// 窗口 / 平台控制。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WindowEffect {
    Quit,
    Minimize,
    ToggleMaximize,
}

/// 气泡提示。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum BubbleEffect {
    Show(BubbleRequest),
}

/// Dock Panel 显隐。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PanelEffect {
    Toggle(PanelKind, bool),
}

/// 搜索面板。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SearchEffect {
    ToggleOption(SearchOption),
    FindPrevious,
    FindNext,
    ReplaceNext,
    ReplaceAll,
    FocusNextField,
    FocusPreviousField,
    Dismiss,
    Toggle,
    ConfirmMatch,
}

/// 跳转到行。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GoToLineEffect {
    Activate,
    Dismiss,
    Jump(usize),
}

/// 编辑器视图设置与 tab 管理。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditorEffect {
    ToggleSoftWrap,
    SelectTab(ViewId),
    SelectAdjacentTab(bool),
    CloseTab(ViewId),
    OpenPreview(BufferId),
    CancelPointerSelection,
}

/// 项目 / 工作区。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ProjectEffect {
    ShowPicker,
    OpenLocalProject,
    StartGitClone,
    RemoveSelectedRecentProject,
    MovePickerSelection(isize),
    ActivatePicker,
}

/// 浮面（设置、诊断、语言服务器等）。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceEffect {
    ShowLanguageServers,
    ShowSettings,
    OpenSettingsToml,
    ApplySettingsChange(SettingsChangeRequest),
    ShowDiagnostics,
    Dismiss,
}

/// 文件树。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FileTreeEffect {
    MoveSelection(isize),
    ExtendSelection(isize),
    Escape,
    CollapseOrParent,
    ExpandOrInto,
    Activate,
    BeginNewEntry,
    CommitNewEntry,
    CancelNewEntry,
    BeginRename,
    CommitRename,
    CancelRename,
    RequestDelete,
    ConfirmDelete,
    CancelDelete,
    Copy,
    Cut,
    Paste,
}

/// 版本管理面板。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum VersionControlEffect {
    MoveSelection(isize),
    Toggle,
    Activate,
    CollapseOrParent,
    ExpandOrInto,
}

/// Git 状态刷新。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GitEffect {
    Refresh,
}

/// 命令处理器请求宿主执行的副作用。**按 feature 分组**。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum HostEffect {
    Window(WindowEffect),
    Bubble(BubbleEffect),
    Panel(PanelEffect),
    Search(SearchEffect),
    GoToLine(GoToLineEffect),
    Editor(EditorEffect),
    Project(ProjectEffect),
    Surface(SurfaceEffect),
    FileTree(FileTreeEffect),
    VersionControl(VersionControlEffect),
    Git(GitEffect),
}

// ── Effect 队列 ─────────────────────────────────────────────────

/// `CommandContext` 内的 effect 缓冲。
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

    pub fn drain(&mut self) -> Vec<HostEffect> {
        std::mem::take(&mut self.pending)
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bubble_effect_roundtrip() {
        let request = BubbleRequest::success("已保存")
            .dedupe("editor.save")
            .ttl_ms(1200);

        assert_eq!(
            HostEffect::Bubble(BubbleEffect::Show(request.clone())),
            HostEffect::Bubble(BubbleEffect::Show(BubbleRequest {
                kind: BubbleKind::Success,
                message: "已保存".to_string(),
                dedupe_key: Some("editor.save".to_string()),
                ttl_ms: Some(1200),
            }))
        );
    }
}
