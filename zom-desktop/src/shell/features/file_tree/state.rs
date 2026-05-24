//! 文件树面板的数据模型。
//!
//! 这些类型是文件树 feature 对外暴露的 owned 快照与操作反馈。App 组合根负责
//! 填充它们，shell 文件树面板负责消费它们。

use std::path::PathBuf;

use zom_workspace::EntryKind;

/// 文件树面板的渲染快照（owned）。
///
/// 把 `ProjectTree::visible_rows` 的借用形式转成 owned，匹配既有的
/// `WorkbenchState` 快照模式：渲染期间不再持有 App 借用。
#[derive(Clone, Debug, Default)]
pub(crate) struct FileTreeState {
    pub(crate) rows: Vec<FileTreeRow>,
    /// 键盘焦点行（光标）。
    pub(crate) selected: Option<PathBuf>,
    /// 当前活动 buffer 对应的文件路径，用于做“活动文件高亮”。
    pub(crate) active: Option<PathBuf>,
    /// 正在键入名称的「新建文件 / 目录」。`None` 表示不处于新建态。
    pub(crate) pending: Option<PendingNewEntry>,
    /// 正在等待确认的「删除文件」。`None` 表示无删除确认弹窗。
    pub(crate) pending_delete: Option<PendingDelete>,
}

/// 一个正在输入名称的新建条目（owned 快照）。
///
/// 不含编辑器文本 / 光标 —— 输入框的渲染由 [`TextEditorSlot`] 自己向
/// [`EditorRouter`] 拉快照，state 这里只描述外壳（图标 / 缩进）。
///
/// [`TextEditorSlot`]: crate::shell::editor::TextEditorSlot
/// [`EditorRouter`]: crate::shell::editor::EditorRouter
#[derive(Clone, Debug)]
pub(crate) struct PendingNewEntry {
    /// 新条目将创建在该目录下。
    pub(crate) parent: PathBuf,
    pub(crate) kind: EntryKind,
    /// 输入行的缩进深度（父目录 depth + 1）。
    pub(crate) depth: usize,
}

/// 一个正在等待删除确认的条目（owned 快照）。
#[derive(Clone, Debug)]
pub(crate) struct PendingDelete {
    /// 待删条目的显示名，确认弹窗用。
    pub(crate) name: String,
    /// 待删条目类型，决定确认弹窗措辞（目录会连同内容删除）。
    pub(crate) kind: EntryKind,
}

#[derive(Clone, Debug)]
pub(crate) struct FileTreeRow {
    pub(crate) path: PathBuf,
    pub(crate) name: String,
    pub(crate) depth: usize,
    pub(crate) kind: EntryKind,
    pub(crate) expanded: bool,
}

/// `file_tree_activate` 的反馈，用于让 shell 决定是否把焦点切回 editor。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum FileTreeActivation {
    /// 选中行不存在或无项目，什么都没做。
    Nothing,
    /// 触发了目录展开/折叠，焦点应留在文件树。
    ToggledDir,
    /// 打开了文件，shell 应当把焦点切回 editor。
    OpenedFile,
}
