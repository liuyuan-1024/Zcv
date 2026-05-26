//! 文件树面板的数据模型。
//!
//! 这些类型是文件树 feature 对外暴露的 owned 快照与操作反馈。App 组合根负责
//! 填充它们，shell 文件树面板负责消费它们。

use std::collections::BTreeSet;
use std::path::PathBuf;

use zom_workspace::EntryKind;

/// 文件树面板的渲染快照（owned）。
///
/// 把 `ProjectTree::visible_rows` 的借用形式转成 owned，匹配既有的
/// `WorkbenchState` 快照模式：渲染期间不再持有 App 借用。
#[derive(Clone, Debug, Default)]
pub(crate) struct FileTreeState {
    pub(crate) rows: Vec<FileTreeRow>,
    /// 键盘焦点行（光标）。与 [`selection`](Self::selection) 解耦 —— 焦点是
    /// 唯一的"光标位置"，选区可以为空、可以有多个成员、也可以与焦点重叠。
    pub(crate) selected: Option<PathBuf>,
    /// 多选集合：用户主动累加进来的目录 / 文件。后续的复制 / 剪切 / 删除等
    /// 批量操作以此为目标；为空时操作降级到 [`selected`](Self::selected) 单项。
    /// 渲染时使用独立背景色，与"活动文件"灰底区分。
    pub(crate) selection: BTreeSet<PathBuf>,
    /// 处于"剪切待粘贴"状态的路径集合。仅当剪贴板模式为 Cut 时非空——
    /// 视图据此把这些行做半透明处理，提示用户它们将被移动。Copy 模式不
    /// 加视觉标记（原地数据没动）。
    pub(crate) cut_paths: BTreeSet<PathBuf>,
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

/// 一个正在等待删除确认的快照，可以是单项也可以是批量。
///
/// 单项删（选区为空时走焦点）与批量删（选区非空）共用同一份结构，UI 据
/// `count` / `has_directory` 切换文案。
#[derive(Clone, Debug)]
pub(crate) struct PendingDelete {
    /// 总条数。`1` 是单删，`>1` 是批量。
    pub(crate) count: usize,
    /// 待删集合里第一个条目的显示名——用于"foo 等 N 项"的句式与单删句式。
    pub(crate) first_name: String,
    /// 单删时取这一项的类型；多删时取第一项的类型（仅用于单删句式回路）。
    pub(crate) first_kind: EntryKind,
    /// 是否含至少一个目录——批量删句式中决定是否强调"及其全部内容"。
    pub(crate) has_directory: bool,
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
