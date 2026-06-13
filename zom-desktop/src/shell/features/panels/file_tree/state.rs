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
    /// 正在重命名的条目。`None` 表示不处于重命名态。与 `pending` 互斥。
    pub(crate) pending_rename: Option<PendingRename>,
    /// 正在等待确认的「删除文件」。`None` 表示无删除确认弹窗。
    pub(crate) pending_delete: Option<PendingDelete>,
}

/// 一个正在输入名称的新建条目（owned 快照）。
///
/// 不含编辑器文本 / 光标 —— 输入框的渲染由 [`TextEditorSlot`] 自己向
/// [`EditorRouter`] 拉快照，state 这里只描述外壳（图标 / 缩进）。
#[derive(Clone, Debug)]
pub(crate) struct PendingNewEntry {
    /// 新条目将创建在该目录下。
    pub(crate) parent: PathBuf,
    pub(crate) kind: EntryKind,
    /// 输入行的缩进深度（父目录 depth + 1）。
    pub(crate) depth: usize,
}

/// 正在重命名某一行的 owned 快照。
/// 视图据此把对应行替换成内联输入框，输入框的文本 / 光标由 [`TextEditorSlot`] 自己向 [`EditorRouter`] 拉。
#[derive(Clone, Debug)]
pub(crate) struct PendingRename {
    pub(crate) path: PathBuf,
    pub(crate) kind: EntryKind,
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
    /// 第 k 位为 1 表示该行是其第 k 层祖先的最后一个可见后代。
    pub(crate) terminal_mask: u64,
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

/// 文件树写操作的领域结果。
///
/// 模型本身不持有 [`WorkspaceSession`] —— 文件操作可能伴随 buffer 重绑路径、
/// 关闭已删文件的 buffer、打开新建/重命名后的文件等编辑会话副作用。模型把
/// "需要做什么"装进本结构，由 runtime 解释成具体的 session 调用。
///
/// [`WorkspaceSession`]: crate::workspace_session::WorkspaceSession
#[derive(Debug)]
pub(crate) enum FileTreeOutcome {
    /// 无副作用（取消、空操作、目录粘贴的 Copy 模式落地等）。
    Nothing,
    /// 仅触发了目录展开 / 折叠。
    ToggledDir,
    /// 打开（或激活）单个文件。
    OpenFile(std::path::PathBuf),
    /// 重命名：把 `old` 下所有 buffer rebase 到 `new`；当 `opens_file` 为 true（即新路径是文件）时还要打开新路径。
    Rename {
        old: std::path::PathBuf,
        new: std::path::PathBuf,
        opens_file: bool,
    },
    /// 粘贴：对每对 `(src, dst)` 做一次 buffer rebase（仅 Cut 模式产生条目，Copy 模式 rebases 为空）。
    Paste {
        rebases: Vec<(std::path::PathBuf, std::path::PathBuf)>,
    },
    /// 批量删除：关闭每个 `paths` 下的 buffer；`picked_sibling` 是模型已计算好的下一焦点行（基于
    /// tree 内的下一兄弟）。`None` 时由 runtime 在应用完关 buffer 后从 session 的活动 buffer 路径回退。
    Delete {
        paths: Vec<std::path::PathBuf>,
        picked_sibling: Option<std::path::PathBuf>,
    },
}
