//! 跨 crate 的 action 集中声明。
//!
//! 对齐 Zed 的 `zed_actions`：所有被多个 crate 引用的 action 在此集中声明、零实现依赖，供编辑器、输入组件、选择器与工作区框架共用。
//! 各命名空间与 keymap 保持一致，action 定义位置迁移不产生 keymap 改动。

use gpui::actions;

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

actions!(workspace, [Save, OpenSettings, GitFetch, GitPull, GitPush]);

actions!(
    dock,
    [
        ToggleProjectTree,
        ToggleVersionControl,
        ToggleOutline,
        ToggleLanguageServer,
        ToggleDiagnostics,
        ToggleProjectSearch,
        ToggleTerminal,
        ToggleDebug,
        ToggleKeyboardShortcuts,
    ]
);

actions!(
    pane,
    [CloseTab, NextTab, PrevTab, TogglePreview, ToggleFileSearch]
);

actions!(
    window_controls,
    [QuitWindow, MinimizeWindow, ToggleMaximizeWindow]
);

actions!(branch_picker, [SelectGitBranch]);

actions!(
    project_picker,
    [ToggleProjectPicker, OpenLocalProject, DeleteRecentProject]
);
