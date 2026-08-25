//! 全局 action 集中声明。
//! 此文件是 `zcv-actions` crate 的公共入口。
//!
//! 对齐 Zed 的 `zed_actions`：全部 action 在此集中声明、零实现依赖，供各 crate 与 keymap 共用；
//! 各命名空间与 keymap JSON 保持一致，action 定义位置迁移不产生 keymap 改动。
//! （Zed 只集中跨 crate 的 action，其余定义在各自 crate；Zcv 单宿主简化，全部集中便于 keymap 统一引用。）

use gpui::{Action, actions};
use schemars::JsonSchema;
use serde::Deserialize;

/// 执行 Panel 的键盘三态命令，`panel` 是 Panel 的稳定 ID。
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, JsonSchema, Action)]
#[action(namespace = dock)]
#[serde(deny_unknown_fields)]
pub struct FocusOrHidePanel {
    pub panel: String,
}

impl FocusOrHidePanel {
    pub fn new(panel: impl Into<String>) -> Self {
        Self {
            panel: panel.into(),
        }
    }
}

actions!(
    editor,
    [
        MoveLeft,
        MoveRight,
        MoveUp,
        MoveDown,
        MoveToPreviousWord,
        MoveToNextWord,
        MoveToBeginningOfLine,
        MoveToEndOfLine,
        MoveToBeginning,
        MoveToEnd,
        MovePageUp,
        MovePageDown,
        SelectLeft,
        SelectRight,
        SelectUp,
        SelectDown,
        SelectToPreviousWord,
        SelectToNextWord,
        SelectToBeginningOfLine,
        SelectToEndOfLine,
        SelectToBeginning,
        SelectToEnd,
        SelectPageUp,
        SelectPageDown,
        SelectAll,
        ExpandSelection,
        Backspace,
        Delete,
        DeleteToPreviousWordStart,
        DeleteToNextWordEnd,
        DeleteToBeginningOfLine,
        DeleteToEndOfLine,
        Newline,
        MoveLineUp,
        MoveLineDown,
        Undo,
        Redo,
        Cut,
        Copy,
        Paste,
        Indent,
        Outdent,
        ToggleFold,
        UnfoldAll,
        OpenExcerpts,
        IncreaseFontSize,
        DecreaseFontSize,
        ResetFontSize,
    ]
);

actions!(
    picker,
    [
        PickerSelectNext,
        PickerSelectPrev,
        PickerConfirm,
        PickerCancel
    ]
);

actions!(
    workspace,
    [
        Save,
        OpenSettings,
        GitFetch,
        GitPull,
        GitPush,
        IncreaseUiFontSize,
        DecreaseUiFontSize,
        ResetUiFontSize
    ]
);

actions!(terminal, [NewTerminal, Clear, Interrupt]);

actions!(dock, [ToggleLeftDock, ToggleBottomDock, ToggleRightDock,]);

actions!(
    pane,
    [CloseTab, NextTab, PrevTab, TogglePreview, DeploySearch]
);

// ── 文件内搜索（搜索条的全部动作；handler 由 SearchBar 统一持有）──

actions!(
    search,
    [
        FindNext,
        FindPrevious,
        ToggleReplace,
        ReplaceNext,
        ReplaceAll,
        ClearSearch,
        ToggleCaseSensitive,
        ToggleWholeWord,
        ToggleRegex,
        Tab,
        Backtab
    ]
);

actions!(
    window_controls,
    [QuitWindow, MinimizeWindow, ToggleMaximizeWindow]
);

actions!(harness, [ToggleHarnessMode]);

actions!(branch_picker, [SelectGitBranch]);

actions!(project_search, [Deploy]);

actions!(
    project_picker,
    [ToggleProjectPicker, OpenLocalProject, DeleteRecentProject]
);

actions!(
    version_control,
    [
        SelectPrev,
        SelectNext,
        Collapse,
        Expand,
        Activate,
        InitRepository,
        ToggleStaged,
        Commit,
        Uncommit
    ]
);

actions!(
    project_tree,
    [
        TreeSelectPrev,
        TreeSelectNext,
        TreeCollapse,
        TreeExpand,
        TreeActivate,
        TreeRename,
        TreeNewEntry,
        TreeTrash,
        TreeConfirmEdit,
        TreeCancelEdit
    ]
);
