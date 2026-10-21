//! 全局 action 集中声明。
//! 此文件是 `zcv-actions` crate 的公共入口。
//!
//! 全部 action 在此集中声明、零实现依赖，供各 crate 与 keymap 共用；
//! 各命名空间与 keymap JSON 保持一致，action 定义位置迁移不产生 keymap 改动。

use gpui::{Action, actions};
use schemars::JsonSchema;
use serde::Deserialize;

/// 可在统一入口执行的命令及其用户可见说明。
pub struct CommandMetadata {
    pub action_name: &'static str,
    pub title: &'static str,
    pub description: &'static str,
}

macro_rules! described_actions {
    ($namespace:ident, $catalog:ident, [$($action:ident $(=> ($title:literal, $description:literal))?),* $(,)?]) => {
        actions!($namespace, [$($action),*]);
        const $catalog: &[CommandMetadata] = &[
            $(
                $(CommandMetadata {
                    action_name: concat!(stringify!($namespace), "::", stringify!($action)),
                    title: $title,
                    description: $description,
                },)?
            )*
        ];
    };
}

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

described_actions!(
    editor,
    EDITOR_COMMANDS,
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
        SelectAll => ("全选", "选中当前文档的全部内容"),
        SelectLargerSyntaxNode,
        SelectSmallerSyntaxNode,
        RenameLocal,
        ConfirmLocalRename,
        CancelLocalRename,
        Backspace,
        Delete,
        DeleteToPreviousWordStart,
        DeleteToNextWordEnd,
        DeleteToBeginningOfLine,
        DeleteToEndOfLine,
        Newline,
        MoveLineUp,
        MoveLineDown,
        Undo => ("撤销", "撤销最近一次编辑"),
        Redo => ("重做", "重新执行已撤销的编辑"),
        Cut => ("剪切", "剪切选中的内容"),
        Copy => ("复制", "复制选中的内容"),
        Paste => ("粘贴", "在光标处粘贴剪贴板内容"),
        Indent => ("增加缩进", "增加选中行的缩进"),
        Outdent => ("减少缩进", "减少选中行的缩进"),
        ToggleIndentGuides => ("切换缩进参考线", "显示或隐藏编辑器中的缩进参考线"),
        UnfoldAll => ("展开全部折叠", "展开当前文档中折叠的内容"),
        OpenExcerpts,
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

described_actions!(
    workspace,
    WORKSPACE_COMMANDS,
    [
        Save => ("保存", "保存当前文档"),
        OpenSettings => ("打开设置", "打开设置文件"),
        FocusQuickAccess,
        GitFetch => ("获取远程更新", "从远程仓库获取最新提交"),
        GitPull => ("拉取远程更新", "将远程更新合入当前分支"),
        GitPush => ("推送当前分支", "将本地提交推送到远程仓库"),
        IncreaseContentFontSize => ("增大内容字号", "放大编辑器和内容区域的文字"),
        DecreaseContentFontSize => ("减小内容字号", "缩小编辑器和内容区域的文字"),
        ResetContentFontSize => ("重置内容字号", "恢复内容区域的默认字号"),
        IncreaseUiFontSize => ("增大界面字号", "放大界面文字"),
        DecreaseUiFontSize => ("减小界面字号", "缩小界面文字"),
        ResetUiFontSize => ("重置界面字号", "恢复界面文字的默认字号"),
        RestartToUpdate
    ]
);

described_actions!(
    terminal,
    TERMINAL_COMMANDS,
    [
        NewTerminal => ("新建终端", "打开一个新的终端会话"),
        Clear,
        Interrupt,
        IncreaseFontSize,
        DecreaseFontSize,
        ResetFontSize
    ]
);

described_actions!(
    dock,
    DOCK_COMMANDS,
    [
        ToggleLeftDock => ("切换左侧面板", "显示或隐藏左侧面板"),
        ToggleBottomDock => ("切换底部面板", "显示或隐藏底部面板"),
        ToggleRightDock => ("切换右侧面板", "显示或隐藏右侧面板")
    ]
);

described_actions!(
    pane,
    PANE_COMMANDS,
    [
        CloseTab => ("关闭标签页", "关闭当前标签页"),
        NextTab => ("切换到下一个标签页", "激活右侧的标签页"),
        PrevTab => ("切换到上一个标签页", "激活左侧的标签页"),
        TogglePreview => ("切换预览", "显示或隐藏当前文档的预览"),
        DeployBufferSearch => ("查找当前文档", "在当前文档中搜索文本")
    ]
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

described_actions!(
    window_controls,
    WINDOW_COMMANDS,
    [
        QuitWindow => ("关闭窗口", "关闭当前应用窗口"),
        MinimizeWindow => ("最小化窗口", "将当前窗口缩到任务栏或 Dock"),
        ToggleMaximizeWindow => ("切换窗口最大化", "最大化或恢复当前窗口")
    ]
);

actions!(harness, [ToggleHarnessMode]);

described_actions!(
    branch_picker,
    BRANCH_PICKER_COMMANDS,
    [
        SelectGitBranch => ("切换 Git 分支", "打开当前项目的分支选择器"),
        DeleteGitBranch
    ]
);

described_actions!(
    project_search,
    PROJECT_SEARCH_COMMANDS,
    [DeployProjectSearch => ("搜索项目内容", "在项目文件中搜索文本")]
);

described_actions!(
    project_picker,
    PROJECT_PICKER_COMMANDS,
    [
        ToggleProjectPicker => ("切换项目", "打开最近项目选择器"),
        OpenLocalProject,
        DeleteRecentProject
    ]
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
        TreeSelectPrevExtend,
        TreeSelectNextExtend,
        TreeCollapse,
        TreeExpand,
        TreeActivate,
        TreeRename,
        TreeNewEntry,
        TreeTrash,
        TreeConfirmEdit,
        TreeCancelEdit,
        TreeCopy,
        TreeCut,
        TreePaste,
        TreeCancelConflict,
        // 取消挂起的复制/剪切状态，清空面板剪贴板（非编辑态按 escape 触发）。
        TreeClearClipboard
    ]
);

/// 返回声明为用户可执行命令的 action 元数据。
pub fn commands() -> impl Iterator<Item = &'static CommandMetadata> {
    [
        EDITOR_COMMANDS,
        WORKSPACE_COMMANDS,
        TERMINAL_COMMANDS,
        DOCK_COMMANDS,
        PANE_COMMANDS,
        WINDOW_COMMANDS,
        BRANCH_PICKER_COMMANDS,
        PROJECT_SEARCH_COMMANDS,
        PROJECT_PICKER_COMMANDS,
    ]
    .into_iter()
    .flatten()
}

#[cfg(test)]
#[path = "test/actions_tests.rs"]
mod tests;
