//! 文件树运行模型。
//!
//! 负责项目目录树、展开状态、选中行、面板快照构造，以及从文件树激活文件。

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use zom_command::commands::file_tree::FileTreeKeyMode;
use zom_command::{EditTarget, KeyContext};
use zom_view::{ViewId, ViewSet};
use zom_workspace::{BufferId, EntryKind, ProjectTree, Workspace};

use crate::focus::{AppFocus, FileTreeFocus, PanelFocus};
use crate::shell::editor::{
    EditorSnapshot, EditorSnapshotRequest, ImeQueryTarget, ImeTarget, OwnedEditorTarget,
    TextTargetOwner, TextTargetQuery,
};

use super::{FileTreeActivation, FileTreeRow, FileTreeState, PendingDelete, PendingNewEntry};

pub(crate) struct FileTreeModel {
    project_tree: Option<ProjectTree>,
    selected: Option<PathBuf>,
    /// **已提交的选区**——过去 Shift+方向"笔画"沉淀下来的、通过普通方向键
    /// 提交的项。当前正在进行中的笔画不存在这里，而是放在
    /// [`stroke`](Self::stroke) 里，可随 Shift+↑/↓ 自由伸缩。对外暴露
    /// （[`state()`](Self::state) / 复制 / 粘贴 / 删除）总是看二者的并集。
    selection: BTreeSet<PathBuf>,
    /// 当前活跃的"扩选笔画"。第一次按 Shift+方向时建立、锚定在按键时的焦点
    /// 行；后续 Shift+↑/↓ 不再追加而是**重算 `[锚点, 新焦点]` 区间**，因此
    /// 可以缩。普通方向键（不带 Shift）会把它的 `items` 并入
    /// [`selection`](Self::selection) 然后清空 stroke——这一步称为"提交"。
    stroke: Option<Stroke>,
    /// 内部剪贴板。Copy / Cut 时拍下当时的选区（空选区降级到焦点单项）。
    /// 跨进程不参与——本阶段仅 zom 内部生效。
    clipboard: Option<FileTreeClipboard>,
    /// 正在键入名称的新建条目；`None` 表示不处于新建态。
    pending: Option<PendingEntry>,
    /// 正在等待确认的待删条目集合（路径 + 类型）。批量删（选区非空）与单删
    /// （焦点回退）共用同一份字段；`None` 表示无删除确认弹窗。
    pending_delete: Option<Vec<(PathBuf, EntryKind)>>,
}

/// 一次正在进行的扩选笔画。`anchor` 是笔画起点（按下第一个 Shift+方向时的
/// 焦点）；`items` 是当前 `[anchor, focus]` 区间在可见行里覆盖到的全部行。
#[derive(Clone, Debug)]
struct Stroke {
    anchor: PathBuf,
    items: BTreeSet<PathBuf>,
}

/// 内部剪贴板的两种模式。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum ClipboardMode {
    Copy,
    Cut,
}

/// 一次 copy / cut 拍下的路径集合与模式快照。
#[derive(Clone, Debug)]
struct FileTreeClipboard {
    mode: ClipboardMode,
    paths: Vec<PathBuf>,
}

/// 新建态的内部数据；缩进深度在 `state()` 快照时再算，故此处不存。
///
/// 名称由一个 [`OwnedEditorTarget`] 承载 —— 键入 / 删除 / undo / 选择都复用编辑命令。
struct PendingEntry {
    parent: PathBuf,
    editor: OwnedEditorTarget,
}

impl FileTreeModel {
    pub(crate) fn new() -> Self {
        Self {
            project_tree: None,
            selected: None,
            selection: BTreeSet::new(),
            stroke: None,
            clipboard: None,
            pending: None,
            pending_delete: None,
        }
    }

    pub(crate) fn open_project(&mut self, root: PathBuf) {
        self.project_tree = match ProjectTree::new(root.clone()) {
            Ok(tree) => Some(tree),
            Err(error) => {
                eprintln!("读取项目目录失败：{}：{error}", root.display());
                None
            }
        };
        self.selected = None;
        self.selection.clear();
        self.stroke = None;
        self.clipboard = None;
        self.pending = None;
        self.pending_delete = None;
    }

    /// 已提交选区与当前 stroke 的并集——所有"对外说选了什么"都看这里。
    fn effective_selection(&self) -> BTreeSet<PathBuf> {
        match &self.stroke {
            None => self.selection.clone(),
            Some(stroke) => self.selection.union(&stroke.items).cloned().collect(),
        }
    }

    /// 把当前笔画 `items` 沉淀到 `selection`，并清空 stroke。普通方向键、
    /// PageUp/PageDown 这些"打断笔画"的操作在动焦点前都要先调它一次。
    fn commit_stroke(&mut self) {
        if let Some(stroke) = self.stroke.take() {
            self.selection.extend(stroke.items);
        }
    }

    pub(crate) fn state(&self, workspace: &Workspace) -> FileTreeState {
        let Some(tree) = self.project_tree.as_ref() else {
            return FileTreeState::default();
        };
        let rows: Vec<FileTreeRow> = tree
            .visible_rows()
            .into_iter()
            .map(|row| FileTreeRow {
                path: row.path.to_path_buf(),
                name: row.name.to_string(),
                depth: row.depth,
                kind: row.kind,
                expanded: row.expanded,
            })
            .collect();
        let active = workspace
            .active_buffer()
            .and_then(|buffer| buffer.path())
            .map(PathBuf::from);
        // 输入行缩进 = 父目录行 depth + 1；父目录必可见（新建前已展开它）。
        let pending = self.pending.as_ref().map(|pending| {
            let depth = rows
                .iter()
                .find(|row| row.path == pending.parent)
                .map(|row| row.depth + 1)
                .unwrap_or(1);
            PendingNewEntry {
                parent: pending.parent.clone(),
                kind: infer_entry_kind_from_input(&pending.editor.text()),
                depth,
            }
        });
        let pending_delete = self.pending_delete.as_ref().map(|items| {
            let first = items.first();
            PendingDelete {
                count: items.len(),
                first_name: first
                    .and_then(|(path, _)| path.file_name())
                    .map(|name| name.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                first_kind: first.map(|(_, kind)| *kind).unwrap_or(EntryKind::File),
                has_directory: items
                    .iter()
                    .any(|(_, kind)| matches!(kind, EntryKind::Directory)),
            }
        });
        // Cut 模式才把路径暴露给视图做半透明；Copy 不做视觉标记。
        let cut_paths = self
            .clipboard
            .as_ref()
            .filter(|c| matches!(c.mode, ClipboardMode::Cut))
            .map(|c| c.paths.iter().cloned().collect())
            .unwrap_or_default();
        FileTreeState {
            rows,
            selected: self.selected.clone(),
            selection: self.effective_selection(),
            cut_paths,
            active,
            pending,
            pending_delete,
        }
    }

    /// 开始新建一个文件或目录：确定目标父目录、展开它，并进入输入态。
    ///
    /// 目标父目录：选中目录则用它本身；选中文件则用其父目录；未选中用根。
    pub(crate) fn begin_new_entry(&mut self) {
        let Some(tree) = self.project_tree.as_mut() else {
            return;
        };
        let parent = match self.selected.as_ref() {
            None => tree.root().to_path_buf(),
            Some(selected) => match snapshot_row(tree, selected) {
                Some((EntryKind::Directory, _, _)) => selected.clone(),
                Some((EntryKind::File, _, _)) => selected
                    .parent()
                    .map(Path::to_path_buf)
                    .unwrap_or_else(|| tree.root().to_path_buf()),
                None => tree.root().to_path_buf(),
            },
        };
        if let Err(error) = tree.expand(&parent) {
            eprintln!("展开目录失败：{}：{error}", parent.display());
            return;
        }
        self.pending = Some(PendingEntry {
            parent,
            editor: OwnedEditorTarget::new(),
        });
    }

    pub(crate) fn pending_active(&self) -> bool {
        self.pending.is_some()
    }

    pub(crate) fn cancel_new_entry(&mut self) {
        self.pending = None;
    }

    /// 提交新建：名称为空则等同取消；以 `/` 结尾创建目录，否则创建文件。
    /// 输入中可以包含相对路径，创建时会补齐中间目录；非法路径或创建失败时保留输入态，供用户改名重试。
    ///
    /// 新建文件成功后立即打开它（返回 [`FileTreeActivation::OpenedFile`]，
    /// 由调用方把焦点切到编辑器）；新建目录无可打开内容，留在文件树。
    pub(crate) fn commit_new_entry(
        &mut self,
        workspace: &mut Workspace,
        views: &mut ViewSet,
    ) -> FileTreeActivation {
        let Some(pending) = self.pending.as_ref() else {
            return FileTreeActivation::Nothing;
        };
        let text = pending.editor.text();
        let name = text.trim();
        if name.is_empty() {
            self.pending = None;
            return FileTreeActivation::Nothing;
        }
        let Some((name, kind)) = parse_new_entry_input(name) else {
            eprintln!("新建条目路径无效：{name}");
            return FileTreeActivation::Nothing;
        };
        let parent = pending.parent.clone();
        let Some(tree) = self.project_tree.as_mut() else {
            self.pending = None;
            return FileTreeActivation::Nothing;
        };
        match tree.create_entry(&parent, &name, kind) {
            Ok(path) => {
                expand_parent_chain(tree, &parent, &path);
                self.selected = Some(path.clone());
                self.pending = None;
                match kind {
                    EntryKind::File => open_file(workspace, views, path),
                    EntryKind::Directory => FileTreeActivation::Nothing,
                }
            }
            Err(error) => {
                let label = match kind {
                    EntryKind::Directory => "目录",
                    EntryKind::File => "文件",
                };
                eprintln!("新建{label}失败：{}：{error}", parent.join(&name).display());
                FileTreeActivation::Nothing
            }
        }
    }

    pub(crate) fn pending_delete_active(&self) -> bool {
        self.pending_delete.is_some()
    }

    /// 请求删除：选区非空时把整个选区拍进 pending_delete；选区为空时降级到
    /// 焦点单项。项目根不可删——选区里包含它会被静默剔除，过滤后空集就不弹
    /// 确认窗。不在可见行里的路径（被外部移除 / 折叠隐藏到不可触达）一并跳过。
    pub(crate) fn request_delete(&mut self) {
        let Some(tree) = self.project_tree.as_ref() else {
            return;
        };
        // 删除以"合并视图"为准：已提交选区 + 当前未提交笔画都算数。
        let effective = self.effective_selection();
        let candidates: Vec<PathBuf> = if !effective.is_empty() {
            effective.into_iter().collect()
        } else if let Some(focus) = self.selected.clone() {
            vec![focus]
        } else {
            return;
        };
        let root = tree.root().to_path_buf();
        let mut items: Vec<(PathBuf, EntryKind)> = Vec::new();
        for path in candidates {
            if path == root {
                continue;
            }
            if let Some((kind, _, _)) = snapshot_row(tree, &path) {
                items.push((path, kind));
            }
        }
        if items.is_empty() {
            return;
        }
        self.pending_delete = Some(items);
    }

    /// 确认删除：把待删集合里的每一项移入回收站、关闭受影响的编辑器视图，并
    /// 按以下优先级决定新焦点：
    /// 1. **下一兄弟**：删除集合里第一项同父目录的下一项，且该项**不**在删除
    ///    集合里（否则跳过它继续往后找）；
    /// 2. **当前活动文件**：所有 close_buffers 操作之后 workspace 的 active
    ///    buffer 所在路径；
    /// 3. **首行**：项目根。
    ///
    /// 单项失败不阻塞其它项；失败的会留在磁盘上但选区与 clipboard 仍按全删
    /// 处理（清空），用户自己决定是否重试。
    pub(crate) fn confirm_delete(&mut self, workspace: &mut Workspace, views: &mut ViewSet) {
        let Some(items) = self.pending_delete.take() else {
            return;
        };
        let Some(tree) = self.project_tree.as_mut() else {
            return;
        };
        // 删除前先把下一兄弟候选拍下：取集合首项的下一兄弟，且该兄弟自己不在删除集合里。
        // 连续选删时，朴素的"下一行"也可能是待删项。
        let next_sibling = items.first().and_then(|(first_path, _)| {
            next_sibling_of(tree, first_path, |candidate| {
                items
                    .iter()
                    .any(|(deleted, _)| deleted.as_path() == candidate)
            })
        });
        for (path, _) in &items {
            match tree.delete_entry(path) {
                Ok(()) => close_buffers_under(workspace, views, path),
                Err(error) => {
                    eprintln!("删除失败：{}：{error}", path.display());
                }
            }
        }
        self.selected =
            next_sibling.or_else(|| selected_path_after_deleting_active(tree, workspace));
        self.selection.clear();
        self.stroke = None;
    }

    pub(crate) fn cancel_delete(&mut self) {
        self.pending_delete = None;
    }

    /// 焦点进入文件树时调用：若尚未选中任何行，默认落到第一行，让边框
    /// 立刻出现，无需用户先按一次 ↓。
    pub(crate) fn ensure_selection_initialized(&mut self) {
        if self.selected.is_some() {
            return;
        }
        let Some(tree) = self.project_tree.as_ref() else {
            return;
        };
        if let Some(first) = tree.visible_rows().first() {
            self.selected = Some(first.path.to_path_buf());
        }
    }

    pub(crate) fn move_selection(&mut self, delta: isize) {
        let Some(tree) = self.project_tree.as_ref() else {
            return;
        };
        let paths: Vec<PathBuf> = tree
            .visible_rows()
            .into_iter()
            .map(|row| row.path.to_path_buf())
            .collect();
        if paths.is_empty() {
            return;
        }
        // 普通方向键打断当前笔画：把 stroke.items 沉淀到 selection，再动焦点。
        // 这样下一次 Shift+方向 会从新焦点重新起锚，之前的笔画作为已提交选区被保留——非连续累加的基础。
        self.commit_stroke();
        let new_index = match self.selected.as_ref() {
            None => {
                if delta >= 0 {
                    0
                } else {
                    paths.len() - 1
                }
            }
            Some(current) => {
                let cur_idx = paths.iter().position(|p| p == current).unwrap_or(0) as isize;
                (cur_idx + delta).clamp(0, paths.len() as isize - 1) as usize
            }
        };
        self.selected = Some(paths[new_index].clone());
    }

    /// 扩展多选选区——"锚点 + 笔画"模型：
    /// 1. 没有 stroke 时（第一次按 Shift+方向，或上一笔画已被普通方向键提交），
    ///    以当前焦点为 `anchor` 新建笔画；
    /// 2. 焦点按 `delta` 移动（边界 clamp）；
    /// 3. **重算** `stroke.items` = 可见行里 `[anchor, focus]` 闭区间的全部行。
    ///    "重算"而非"追加"——所以反向 Shift+方向 会让区间自然收缩，痛点解决。
    ///
    /// 非连续累加仍然支持：用户先 Shift+方向建一段，按普通方向键提交该段，再
    /// 跳到别处按 Shift+方向开新笔画；两段都体现在 [`effective_selection`] 里。
    pub(crate) fn extend_selection(&mut self, delta: isize) {
        let Some(tree) = self.project_tree.as_ref() else {
            return;
        };
        let paths: Vec<PathBuf> = tree
            .visible_rows()
            .into_iter()
            .map(|row| row.path.to_path_buf())
            .collect();
        if paths.is_empty() {
            return;
        }
        // 无焦点起势：把焦点落到首/末并起锚，单项纳入笔画，不再额外位移。
        if self.selected.is_none() {
            let initial = if delta >= 0 { 0 } else { paths.len() - 1 };
            let path = paths[initial].clone();
            self.selected = Some(path.clone());
            self.stroke = Some(Stroke {
                anchor: path.clone(),
                items: std::iter::once(path).collect(),
            });
            return;
        }
        let cur_focus = self.selected.clone().expect("上面保证非空");
        let cur_idx = paths.iter().position(|p| p == &cur_focus).unwrap_or(0);
        // 锚点：复用现有 stroke 的 anchor；不存在或失效（树结构变了，锚已不在可见行里）则以当前焦点重锚。
        let anchor_idx = self
            .stroke
            .as_ref()
            .and_then(|s| paths.iter().position(|p| p == &s.anchor))
            .unwrap_or(cur_idx);
        let need_reset = self
            .stroke
            .as_ref()
            .map(|s| !paths.iter().any(|p| p == &s.anchor))
            .unwrap_or(true);
        if need_reset {
            self.stroke = Some(Stroke {
                anchor: paths[anchor_idx].clone(),
                items: BTreeSet::new(),
            });
        }
        // 焦点按 delta 位移。
        let new_idx = ((cur_idx as isize) + delta).clamp(0, paths.len() as isize - 1) as usize;
        self.selected = Some(paths[new_idx].clone());
        // 重算 stroke.items = [min, max] 闭区间。
        let (lo, hi) = if anchor_idx <= new_idx {
            (anchor_idx, new_idx)
        } else {
            (new_idx, anchor_idx)
        };
        let items: BTreeSet<PathBuf> = paths[lo..=hi].iter().cloned().collect();
        self.stroke.as_mut().expect("上面已确保 stroke 存在").items = items;
    }

    /// Esc 二段式：已提交选区或当前 stroke 任一非空时，全清并返回 `true`
    /// 表示已消化；都空时返回 `false`，让调用方走"焦点回编辑器"的原有路径。
    pub(crate) fn escape(&mut self) -> bool {
        if self.selection.is_empty() && self.stroke.is_none() {
            false
        } else {
            self.selection.clear();
            self.stroke = None;
            true
        }
    }

    /// 复制：把当前选区（空时降级到焦点单项）拍进内部剪贴板，模式 Copy。
    /// 选区与现有剪贴板内容不动——支持"拷一次粘贴多次"。
    pub(crate) fn copy_to_clipboard(&mut self) {
        let paths = self.clipboard_snapshot_source();
        if paths.is_empty() {
            return;
        }
        self.clipboard = Some(FileTreeClipboard {
            mode: ClipboardMode::Copy,
            paths,
        });
    }

    /// 剪切：与 [`copy_to_clipboard`](Self::copy_to_clipboard) 同理但模式 Cut。
    /// 不立即移动文件——真正的位置变化在 [`paste_from_clipboard`](Self::paste_from_clipboard) 里发生。
    pub(crate) fn cut_to_clipboard(&mut self) {
        let paths = self.clipboard_snapshot_source();
        if paths.is_empty() {
            return;
        }
        self.clipboard = Some(FileTreeClipboard {
            mode: ClipboardMode::Cut,
            paths,
        });
    }

    /// 粘贴：把剪贴板内容应用到"焦点所在目录"。
    ///
    /// 目标父目录解析：焦点目录→自身；焦点文件→其父；无焦点或焦点已失效→项目根。
    /// 粘贴永不静默覆盖，冲突由 [`ProjectTree::copy_entry`] / [`move_entry`] 自动改名。
    /// Cut 模式下每个成功移动的源还要把已打开的 buffer 路径 rebase 过去；
    /// 失败的条目记日志、跳过、其它继续。
    /// 收尾：Cut 清空剪贴板与选区；Copy 保留，方便"粘到多处"。两种模式都把
    /// 焦点落到第一个新路径。
    pub(crate) fn paste_from_clipboard(&mut self, workspace: &mut Workspace) {
        let Some(clipboard) = self.clipboard.clone() else {
            return;
        };
        // 先在不可变借用作用域里算好目标，作用域结束后再拿可变借用做 copy / move。
        let target_parent = {
            let Some(tree) = self.project_tree.as_ref() else {
                return;
            };
            compute_paste_target(tree, self.selected.as_ref())
        };
        let Some(tree) = self.project_tree.as_mut() else {
            return;
        };
        let mut new_paths = Vec::new();
        for src in &clipboard.paths {
            let result = match clipboard.mode {
                ClipboardMode::Copy => tree.copy_entry(src, &target_parent),
                ClipboardMode::Cut => tree.move_entry(src, &target_parent),
            };
            match result {
                Ok(new_path) => {
                    if matches!(clipboard.mode, ClipboardMode::Cut) && new_path != *src {
                        rebase_buffers_under(workspace, src, &new_path);
                    }
                    new_paths.push(new_path);
                }
                Err(error) => {
                    eprintln!(
                        "粘贴失败：{} → {}：{error}",
                        src.display(),
                        target_parent.display()
                    );
                }
            }
        }
        // 选区只是用来"圈出本次操作对象"，粘完使命已尽——两种模式都清空它（含未提交的当前笔画）。
        // 避免源条目仍亮着选区底色误导用户。
        // Copy 模式保留 clipboard，让"再粘一次到别处"靠 Cmd+V 单独工作，与选区无关。
        self.selection.clear();
        self.stroke = None;
        if matches!(clipboard.mode, ClipboardMode::Cut) {
            self.clipboard = None;
        }
        // 焦点落到"被粘贴处"——target_parent 自身。
        // 这样不论新条目在折叠子树里还是树顶，焦点永远可见、对用户来说就是"我刚才粘到这儿了"。
        // 同时顺手展开 target_parent，按 ↓ 一步就能看到新条目；已展开则无操作。
        if !new_paths.is_empty() {
            if let Err(error) = tree.expand(&target_parent) {
                eprintln!("粘贴后展开目录失败：{}：{error}", target_parent.display());
            }
            self.selected = Some(target_parent);
        }
    }

    /// 拍剪贴板源：合并视图（已提交选区 + 当前笔画）非空用合并视图；否则降级
    /// 到焦点单项；都没有则空。
    fn clipboard_snapshot_source(&self) -> Vec<PathBuf> {
        let effective = self.effective_selection();
        if !effective.is_empty() {
            effective.into_iter().collect()
        } else if let Some(focus) = self.selected.clone() {
            vec![focus]
        } else {
            Vec::new()
        }
    }

    pub(crate) fn collapse_or_parent(&mut self) {
        let Some(selected) = self.selected.clone() else {
            return;
        };
        let Some(tree) = self.project_tree.as_mut() else {
            return;
        };
        let snapshot = snapshot_row(tree, &selected);
        if let Some((kind, expanded, _depth)) = snapshot
            && matches!(kind, EntryKind::Directory)
            && expanded
        {
            tree.collapse(&selected);
            return;
        }
        // 不是展开目录：上跳到父行。根目录已经是最顶层，原地不动。
        if selected == tree.root() {
            return;
        }
        if let Some(parent) = selected.parent() {
            self.selected = Some(parent.to_path_buf());
        }
    }

    pub(crate) fn expand_or_into(&mut self) {
        let Some(selected) = self.selected.clone() else {
            return;
        };
        let Some(tree) = self.project_tree.as_mut() else {
            return;
        };
        let Some((kind, expanded, depth)) = snapshot_row(tree, &selected) else {
            return;
        };
        if !matches!(kind, EntryKind::Directory) {
            return;
        }
        if expanded {
            let next_child = tree
                .visible_rows()
                .iter()
                .enumerate()
                .find_map(|(idx, row)| {
                    if row.path == selected {
                        Some(idx)
                    } else {
                        None
                    }
                });
            if let Some(idx) = next_child {
                let next = tree
                    .visible_rows()
                    .get(idx + 1)
                    .filter(|row| row.depth > depth)
                    .map(|row| row.path.to_path_buf());
                if let Some(path) = next {
                    self.selected = Some(path);
                }
            }
        } else if let Err(error) = tree.expand(&selected) {
            eprintln!("展开目录失败：{}：{error}", selected.display());
        }
    }

    pub(crate) fn activate_selected(
        &mut self,
        workspace: &mut Workspace,
        views: &mut ViewSet,
    ) -> FileTreeActivation {
        let Some(selected) = self.selected.clone() else {
            return FileTreeActivation::Nothing;
        };
        let Some(tree) = self.project_tree.as_mut() else {
            return FileTreeActivation::Nothing;
        };
        let Some((kind, _, _)) = snapshot_row(tree, &selected) else {
            return FileTreeActivation::Nothing;
        };
        match kind {
            EntryKind::Directory => {
                if let Err(error) = tree.toggle(&selected) {
                    eprintln!("切换目录展开失败：{}：{error}", selected.display());
                }
                FileTreeActivation::ToggledDir
            }
            EntryKind::File => open_file(workspace, views, selected),
        }
    }
}

fn open_file(workspace: &mut Workspace, views: &mut ViewSet, path: PathBuf) -> FileTreeActivation {
    let existing = workspace.buffers().find_map(|(id, buffer)| {
        if buffer.path() == Some(path.as_path()) {
            Some(id)
        } else {
            None
        }
    });
    if let Some(buffer_id) = existing {
        let _ = workspace.set_active_buffer(buffer_id);
        focus_buffer_view(workspace, views, buffer_id);
        return FileTreeActivation::OpenedFile;
    }

    let buffer_id = match workspace.open_file(path.clone()) {
        Ok(buffer_id) => buffer_id,
        Err(error) => {
            eprintln!("打开文件失败：{}：{error}", path.display());
            return FileTreeActivation::Nothing;
        }
    };
    focus_buffer_view(workspace, views, buffer_id);
    FileTreeActivation::OpenedFile
}

fn infer_entry_kind_from_input(text: &str) -> EntryKind {
    match parse_new_entry_input(text.trim()) {
        Some((_, kind)) => kind,
        None => EntryKind::File,
    }
}

fn parse_new_entry_input(input: &str) -> Option<(String, EntryKind)> {
    if input.is_empty() || input.contains('\\') {
        return None;
    }
    let kind = if input.ends_with('/') {
        EntryKind::Directory
    } else {
        EntryKind::File
    };
    let path = input.trim_end_matches('/');
    if path.is_empty()
        || path
            .split('/')
            .any(|segment| segment.is_empty() || segment == "." || segment == "..")
    {
        return None;
    }
    Some((path.to_string(), kind))
}

fn expand_parent_chain(tree: &mut ProjectTree, base: &Path, path: &Path) {
    let Some(target_parent) = path.parent() else {
        return;
    };
    let mut dirs = Vec::new();
    let mut current = Some(target_parent);
    while let Some(dir) = current {
        if !dir.starts_with(base) {
            break;
        }
        dirs.push(dir.to_path_buf());
        if dir == base {
            break;
        }
        current = dir.parent();
    }
    dirs.reverse();
    for dir in dirs {
        if let Err(error) = tree.expand(&dir) {
            eprintln!("展开目录失败：{}：{error}", dir.display());
            break;
        }
    }
}

/// 确保 `buffer_id` 有对应视图，并把它切成活动视图。
///
/// `ViewSet::open_view` 只在当前无活动视图时才自动激活，所以打开新文件后显式
/// `set_active`，确保编辑区立即显示对应缓冲区。已存在视图时复用，不重复建。
fn focus_buffer_view(workspace: &Workspace, views: &mut ViewSet, buffer_id: BufferId) {
    let existing_view = views.views().find_map(|(id, view)| {
        if view.buffer() == buffer_id {
            Some(id)
        } else {
            None
        }
    });
    let view_id = match existing_view {
        Some(id) => id,
        None => {
            let version = workspace
                .buffer(buffer_id)
                .expect("刚打开的缓冲区必然存在")
                .buffer()
                .version();
            views.open_view(buffer_id, version)
        }
    };
    views.set_active(view_id);
}

/// 关闭路径落在 `deleted` 之下（含 `deleted` 自身）的全部 buffer 及其视图 ——
/// 删文件即关该文件，删目录即关其下所有已打开文件。
///
/// 文件已不在磁盘上，buffer 一并关闭（不像关标签那样保留）：再留着只是
/// 指向已删文件的孤儿。关完后把 workspace 活动 buffer 对齐到剩余活动视图，
/// 让文件树「活动文件」高亮跟随。
fn close_buffers_under(workspace: &mut Workspace, views: &mut ViewSet, deleted: &Path) {
    let victims: Vec<BufferId> = workspace
        .buffers()
        .filter_map(|(id, buffer)| {
            buffer
                .path()
                .filter(|path| path.starts_with(deleted))
                .map(|_| id)
        })
        .collect();
    for buffer_id in victims {
        let view_ids: Vec<ViewId> = views
            .views()
            .filter_map(|(view_id, view)| (view.buffer() == buffer_id).then_some(view_id))
            .collect();
        for view_id in view_ids {
            views.close_view(view_id);
        }
        let _ = workspace.close_buffer(buffer_id);
    }
    if let Some(buffer_id) = views.active_view().map(|view| view.buffer()) {
        let _ = workspace.set_active_buffer(buffer_id);
    }
}

fn first_visible_path(tree: &ProjectTree) -> Option<PathBuf> {
    tree.visible_rows()
        .first()
        .map(|row| row.path.to_path_buf())
}

fn selected_path_after_deleting_active(
    tree: &ProjectTree,
    workspace: &Workspace,
) -> Option<PathBuf> {
    workspace
        .active_buffer()
        .and_then(|buffer| buffer.path())
        .map(Path::to_path_buf)
        .or_else(|| first_visible_path(tree))
}

/// 在可见行序列里找 `path` 的**下一兄弟**：与 `path` 同父目录、排序在它之后
/// 的第一项。若 `path` 是一个展开的目录，由于 `find` 是按 `parent ==` 过滤的，
/// 它的展开后代自然不会被选中——`row.path.parent()` 等于该目录而不是 `path`
/// 的父目录。
///
/// `skip` 谓词允许调用方排除"也要被删的同辈"——典型场景是批量删时下一兄弟
/// 候选自己也在删除集合里，需要继续向后探。
///
/// 返回 `None` 当且仅当：`path` 不在可见行里、或它的同父后续项全都被 `skip`
/// 过滤掉、或它没有父（项目根本身不会被删除，这里保持防御性返回）。
fn next_sibling_of(
    tree: &ProjectTree,
    path: &Path,
    skip: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let parent = path.parent()?;
    let rows = tree.visible_rows();
    let idx = rows.iter().position(|row| row.path == path)?;
    rows.iter().skip(idx + 1).find_map(|row| {
        if row.path.parent() != Some(parent) {
            return None;
        }
        if skip(row.path) {
            return None;
        }
        Some(row.path.to_path_buf())
    })
}

/// 从一棵 [`ProjectTree`] 里抓出一行的 `(kind, expanded, depth)`，规避借用
/// 重叠：调用方拿到 owned 元组后即可继续对树做可变操作。
fn snapshot_row(tree: &ProjectTree, path: &Path) -> Option<(EntryKind, bool, usize)> {
    tree.visible_rows()
        .into_iter()
        .find(|row| row.path == path)
        .map(|row| (row.kind, row.expanded, row.depth))
}

/// 决定粘贴落点的父目录：
/// - 焦点是目录 → 目录自身
/// - 焦点是文件 → 文件父目录
/// - 无焦点 / 焦点已不在可见行里 → 项目根
fn compute_paste_target(tree: &ProjectTree, selected: Option<&PathBuf>) -> PathBuf {
    match selected {
        None => tree.root().to_path_buf(),
        Some(path) => match snapshot_row(tree, path) {
            Some((EntryKind::Directory, _, _)) => path.clone(),
            Some((EntryKind::File, _, _)) => path
                .parent()
                .map(Path::to_path_buf)
                .unwrap_or_else(|| tree.root().to_path_buf()),
            None => tree.root().to_path_buf(),
        },
    }
}

/// 文件树移动 `old_prefix` → `new_prefix` 之后，把所有以 `old_prefix` 开头的
/// 已打开 buffer 的绑定路径一并更新到新位置，不关闭它们。`old` 是文件本身时
/// `strip_prefix` 返回空，`new_prefix.join("")` 仍然等于 `new_prefix`，符合
/// 单文件移动的预期。失败仅记日志、继续处理其他 buffer。
fn rebase_buffers_under(workspace: &mut Workspace, old_prefix: &Path, new_prefix: &Path) {
    let updates: Vec<(BufferId, PathBuf)> = workspace
        .buffers()
        .filter_map(|(id, buffer)| {
            let path = buffer.path()?;
            let rest = path.strip_prefix(old_prefix).ok()?;
            Some((id, new_prefix.join(rest)))
        })
        .collect();
    for (id, new_path) in updates {
        if let Err(error) = workspace.rebind_buffer_path(id, new_path.clone()) {
            eprintln!("重绑定缓冲区路径失败：{}：{error}", new_path.display());
        }
    }
}

impl TextTargetQuery for FileTreeModel {
    fn accepts_focus(&self, focus: AppFocus) -> bool {
        matches!(
            focus,
            AppFocus::Panel(PanelFocus::FileTree(FileTreeFocus::NewEntryName))
        )
    }

    fn snapshot(&self) -> EditorSnapshot {
        self.pending
            .as_ref()
            .map(|pending| {
                pending
                    .editor
                    .snapshot(EditorSnapshotRequest::single_line())
            })
            .unwrap_or_default()
    }

    fn key_contexts(&self) -> Vec<KeyContext> {
        vec![
            KeyContext::text_edit(self.accepts_newline(), false),
            KeyContext::file_tree(FileTreeKeyMode::PendingName),
            KeyContext::global(),
        ]
    }

    fn ime_query_target(&self) -> Option<ImeQueryTarget<'_>> {
        self.pending
            .as_ref()
            .map(|pending| pending.editor.as_ime_query_target())
    }
}

impl TextTargetOwner for FileTreeModel {
    fn ime_target(&mut self) -> Option<ImeTarget<'_>> {
        self.pending
            .as_mut()
            .map(|pending| pending.editor.as_ime_target())
    }

    fn edit_target(&mut self) -> Option<EditTarget<'_>> {
        self.pending
            .as_mut()
            .map(|pending| pending.editor.as_edit_target())
    }
}

#[cfg(test)]
impl Default for FileTreeModel {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::{File, create_dir_all};

    fn tmp_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("zom-file-tree-model-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        create_dir_all(&dir).unwrap();
        dir
    }

    fn open_view_for(workspace: &Workspace, views: &mut ViewSet, buffer_id: BufferId) -> ViewId {
        let version = workspace
            .buffer(buffer_id)
            .expect("缓冲区应存在")
            .buffer()
            .version();
        views.open_view(buffer_id, version)
    }

    #[test]
    fn deleting_active_file_should_select_next_active_file_path() {
        let root = tmp_root("delete-active-selects-next");
        let readme = root.join("README.md");
        let lib = root.join("src/lib.rs");
        create_dir_all(root.join("src")).unwrap();
        File::create(&readme).unwrap();
        File::create(&lib).unwrap();

        let tree = ProjectTree::new(root).unwrap();
        let mut workspace = Workspace::new();
        let readme_id = workspace.open_file(readme.clone()).unwrap();
        let lib_id = workspace.open_file(lib.clone()).unwrap();
        let mut views = ViewSet::new();
        open_view_for(&workspace, &mut views, readme_id);
        let lib_view = open_view_for(&workspace, &mut views, lib_id);
        views.set_active(lib_view);

        close_buffers_under(&mut workspace, &mut views, &lib);

        assert_eq!(
            workspace.active_buffer().and_then(|buffer| buffer.path()),
            Some(readme.as_path())
        );
        assert_eq!(
            selected_path_after_deleting_active(&tree, &workspace).as_deref(),
            Some(readme.as_path())
        );
    }

    /// 构造一棵 root/{a,b,c}.txt 的 model，已展开 root。
    fn model_with_three_files(name: &str) -> (FileTreeModel, PathBuf) {
        let root = tmp_root(name);
        File::create(root.join("a.txt")).unwrap();
        File::create(root.join("b.txt")).unwrap();
        File::create(root.join("c.txt")).unwrap();
        let mut model = FileTreeModel::default();
        model.open_project(root.clone());
        (model, root)
    }

    fn workspace_for_state() -> Workspace {
        Workspace::new()
    }

    #[test]
    fn extend_selection_should_add_both_endpoints_first_call() {
        let (mut model, root) = model_with_three_files("extend-first-call");
        // 焦点初始化到第一行（root 自身在 visible_rows 中是第一行）。
        model.ensure_selection_initialized();
        assert_eq!(model.selected.as_deref(), Some(root.as_path()));

        // 第一次 Shift+↓：起点（root）与终点（a.txt）都应进选区。
        model.extend_selection(1);
        let ws = workspace_for_state();
        let state = model.state(&ws);
        assert!(state.selection.contains(&root));
        assert!(state.selection.contains(&root.join("a.txt")));
        assert_eq!(
            state.selected.as_deref(),
            Some(root.join("a.txt").as_path())
        );
    }

    #[test]
    fn extend_selection_continuous_should_grow_like_brush() {
        let (mut model, root) = model_with_three_files("extend-continuous");
        model.ensure_selection_initialized();
        model.extend_selection(1); // root, a
        model.extend_selection(1); // + b
        model.extend_selection(1); // + c
        let state = model.state(&workspace_for_state());
        assert_eq!(state.selection.len(), 4);
        for name in ["a.txt", "b.txt", "c.txt"] {
            assert!(state.selection.contains(&root.join(name)));
        }
        assert!(state.selection.contains(&root));
        assert_eq!(
            state.selected.as_deref(),
            Some(root.join("c.txt").as_path())
        );
    }

    #[test]
    fn plain_move_should_not_touch_selection() {
        let (mut model, root) = model_with_three_files("plain-move-keeps-selection");
        model.ensure_selection_initialized();
        model.extend_selection(1); // 选区 = {root, a}
        // 普通方向键：焦点继续往下，但选区不变。
        model.move_selection(1); // 焦点到 b
        model.move_selection(1); // 焦点到 c
        let state = model.state(&workspace_for_state());
        assert_eq!(state.selection.len(), 2);
        assert!(state.selection.contains(&root));
        assert!(state.selection.contains(&root.join("a.txt")));
        assert_eq!(
            state.selected.as_deref(),
            Some(root.join("c.txt").as_path())
        );
    }

    #[test]
    fn extend_selection_after_jumping_should_accumulate_noncontiguous() {
        let (mut model, root) = model_with_three_files("extend-noncontiguous");
        model.ensure_selection_initialized();
        // 累加前两行入选区。
        model.extend_selection(1); // 选区 = {root, a}
        // 普通方向键跳到 c.txt（中间不留痕迹）。
        model.move_selection(1); // 焦点 b
        model.move_selection(1); // 焦点 c
        // Shift+↑ 在 c 处累加：起点 c 入选区，终点 b 也入选区。
        model.extend_selection(-1);
        let state = model.state(&workspace_for_state());
        assert!(state.selection.contains(&root));
        assert!(state.selection.contains(&root.join("a.txt")));
        assert!(state.selection.contains(&root.join("b.txt")));
        assert!(state.selection.contains(&root.join("c.txt")));
        assert_eq!(state.selection.len(), 4);
        // 起点跳跃留下的"中间区"在这棵小树里恰好没空隙，用更大树测才能完美演示非连续。
        // 这里至少验证 jump 后累加并未"补齐中间所有路径"——root 与 a 是因为之前累加过、b/c 是这次累加。
        // 不存在"多余的中间填充"逻辑。
    }

    #[test]
    fn shift_arrow_reverse_should_shrink_stroke() {
        // 笔画过头再回来：Shift+↓ ×3 → 笔画覆盖 {root, a, b, c}。
        // 接着 Shift+↑ 一次，笔画应缩回 {root, a, b}，焦点 b。
        let (mut model, root) = model_with_three_files("stroke-shrink");
        model.ensure_selection_initialized();
        // visible_rows: root, a.txt, b.txt, c.txt
        model.extend_selection(1); // {root, a}
        model.extend_selection(1); // {root, a, b}
        model.extend_selection(1); // {root, a, b, c}
        assert_eq!(model.effective_selection().len(), 4);
        model.extend_selection(-1); // 缩回 {root, a, b}
        let view = model.effective_selection();
        assert_eq!(view.len(), 3);
        assert!(view.contains(&root));
        assert!(view.contains(&root.join("a.txt")));
        assert!(view.contains(&root.join("b.txt")));
        assert!(!view.contains(&root.join("c.txt")));
        assert_eq!(
            model.selected.as_deref(),
            Some(root.join("b.txt").as_path())
        );
    }

    #[test]
    fn plain_arrow_should_commit_stroke_into_selection() {
        // Shift+↓ ×2 后按普通 ↓：当前笔画沉淀到已提交选区。
        // 再次 Shift+方向应从新焦点重新起锚，不影响已提交部分。
        let (mut model, root) = model_with_three_files("stroke-commit");
        model.ensure_selection_initialized();
        model.extend_selection(1); // 笔画 = {root, a}
        model.extend_selection(1); // 笔画 = {root, a, b}
        assert!(model.stroke.is_some());
        model.move_selection(1); // 提交：selection = {root, a, b}, stroke = None; focus = c
        assert!(model.stroke.is_none());
        // 已提交选区独立于"现在按 Shift+↑ 会做什么"：新笔画从 c 重新起锚。
        let committed = model.selection.clone();
        let expected: BTreeSet<_> = [root.clone(), root.join("a.txt"), root.join("b.txt")]
            .into_iter()
            .collect();
        assert_eq!(committed, expected);
        model.extend_selection(-1); // 新笔画：anchor=c, focus=b。笔画 = {b, c}
        let view = model.effective_selection();
        // 合并 = 已提交 {root,a,b} ∪ 新笔画 {b,c} = {root,a,b,c}
        assert_eq!(view.len(), 4);
        assert!(view.contains(&root.join("c.txt")));
    }

    #[test]
    fn escape_should_clear_both_committed_selection_and_active_stroke() {
        let (mut model, _root) = model_with_three_files("esc-clears-both");
        model.ensure_selection_initialized();
        model.extend_selection(1); // 形成笔画
        model.move_selection(1); // 提交一段
        model.extend_selection(1); // 又起新笔画
        assert!(!model.selection.is_empty());
        assert!(model.stroke.is_some());
        assert!(model.escape());
        assert!(model.selection.is_empty());
        assert!(model.stroke.is_none());
    }

    #[test]
    fn escape_should_consume_only_when_selection_non_empty() {
        let (mut model, _root) = model_with_three_files("escape-two-stage");
        model.ensure_selection_initialized();
        // 合并视图空：Esc 不消化。
        assert!(!model.escape());
        // 累加（笔画形式）非空：Esc 把已提交选区与当前笔画全清并消化。
        model.extend_selection(1);
        assert!(!model.effective_selection().is_empty());
        assert!(model.escape());
        assert!(model.effective_selection().is_empty());
        assert!(model.stroke.is_none());
        // 再次 Esc：又回到空集不消化。
        assert!(!model.escape());
    }

    #[test]
    fn open_project_should_clear_selection() {
        let (mut model, _root) = model_with_three_files("open-clears-selection");
        model.ensure_selection_initialized();
        model.extend_selection(1);
        assert!(!model.effective_selection().is_empty());
        let other = tmp_root("open-clears-selection-target");
        File::create(other.join("x.txt")).unwrap();
        model.open_project(other);
        assert!(model.effective_selection().is_empty());
        assert!(model.stroke.is_none());
    }

    /// 构造 root/{a,b}.txt + root/sub/ 的小项目。
    fn model_with_two_files_and_subdir(name: &str) -> (FileTreeModel, PathBuf) {
        let root = tmp_root(name);
        File::create(root.join("a.txt")).unwrap();
        File::create(root.join("b.txt")).unwrap();
        create_dir_all(root.join("sub")).unwrap();
        let mut model = FileTreeModel::default();
        model.open_project(root.clone());
        (model, root)
    }

    #[test]
    fn copy_snapshot_should_capture_selection_when_non_empty() {
        let (mut model, _root) = model_with_two_files_and_subdir("clip-copy-selection");
        model.ensure_selection_initialized();
        model.extend_selection(1);
        model.extend_selection(1); // 笔画覆盖 3 项；通过 state 看合并视图。

        let snapshot_selection = model.effective_selection();
        assert_eq!(snapshot_selection.len(), 3);
        model.copy_to_clipboard();

        let clip = model.clipboard.as_ref().expect("应有剪贴板");
        assert_eq!(clip.mode, ClipboardMode::Copy);
        let paths: BTreeSet<_> = clip.paths.iter().cloned().collect();
        // 剪贴板拍下的路径集合 == 操作时的合并视图。
        assert_eq!(paths, snapshot_selection);
    }

    #[test]
    fn copy_with_empty_selection_should_fallback_to_focus_singleton() {
        let (mut model, root) = model_with_two_files_and_subdir("clip-copy-focus");
        // 焦点跳到 a.txt，但选区为空。
        model.move_selection(1); // root
        model.move_selection(1); // sub (目录优先排序)
        model.move_selection(1); // a.txt
        assert_eq!(
            model.selected.as_deref(),
            Some(root.join("a.txt").as_path())
        );
        model.copy_to_clipboard();

        let clip = model.clipboard.as_ref().expect("应有剪贴板");
        assert_eq!(clip.paths, vec![root.join("a.txt")]);
    }

    #[test]
    fn cut_should_record_cut_mode_and_state_exposes_cut_paths() {
        let (mut model, root) = model_with_two_files_and_subdir("clip-cut-state");
        model.move_selection(1); // root
        model.move_selection(1); // sub
        model.move_selection(1); // a.txt
        model.cut_to_clipboard();

        let state = model.state(&Workspace::new());
        assert!(state.cut_paths.contains(&root.join("a.txt")));
        assert_eq!(state.cut_paths.len(), 1);

        // Copy 模式不会暴露 cut_paths。
        model.copy_to_clipboard();
        let state = model.state(&Workspace::new());
        assert!(state.cut_paths.is_empty());
    }

    #[test]
    fn paste_target_should_be_focused_directory_or_parent_of_file() {
        let root = tmp_root("paste-target");
        create_dir_all(root.join("sub")).unwrap();
        File::create(root.join("note.txt")).unwrap();
        let tree = ProjectTree::new(root.clone()).unwrap();

        // 焦点目录 → 目录自身。
        let target = compute_paste_target(&tree, Some(&root.join("sub")));
        assert_eq!(target, root.join("sub"));
        // 焦点文件 → 文件父目录。
        let target = compute_paste_target(&tree, Some(&root.join("note.txt")));
        assert_eq!(target, root);
        // 无焦点 → 项目根。
        let target = compute_paste_target(&tree, None);
        assert_eq!(target, root);
    }

    #[test]
    fn copy_paste_should_keep_clipboard_but_clear_selection() {
        let (mut model, root) = model_with_two_files_and_subdir("paste-copy");
        // 选 a.txt 复制。
        model.move_selection(1); // root
        model.move_selection(1); // sub
        model.move_selection(1); // a.txt
        model.extend_selection(0); // 把 a.txt 加入选区（delta=0 → 起点 + 终点都是 a.txt）
        model.copy_to_clipboard();
        // 把焦点移到 sub，然后粘贴到 sub 目录里。
        model.selected = Some(root.join("sub"));

        let mut workspace = Workspace::new();
        model.paste_from_clipboard(&mut workspace);

        // 副本落在 sub/a.txt，源 a.txt 保留。
        assert!(root.join("sub/a.txt").is_file());
        assert!(root.join("a.txt").is_file());
        // Copy 模式：剪贴板保留（支持连续 Cmd+V 粘到别处），但选区清空。
        // 选区只是"圈出本次操作对象"的临时标记，粘完即清。
        assert!(model.clipboard.is_some());
        assert!(model.selection.is_empty());
        // 焦点 = "被粘贴处" target_parent，可见且就是用户按 Cmd+V 时所在位置。
        assert_eq!(model.selected.as_deref(), Some(root.join("sub").as_path()));
    }

    #[test]
    fn cut_paste_should_move_file_and_clear_clipboard_and_selection() {
        let (mut model, root) = model_with_two_files_and_subdir("paste-cut");
        // 选 a.txt 剪切。
        model.move_selection(1);
        model.move_selection(1);
        model.move_selection(1); // a.txt
        model.extend_selection(0); // 入选区
        model.cut_to_clipboard();
        // 粘到 sub。
        model.selected = Some(root.join("sub"));

        let mut workspace = Workspace::new();
        model.paste_from_clipboard(&mut workspace);

        // a.txt 已从原处消失。
        assert!(!root.join("a.txt").exists());
        assert!(root.join("sub/a.txt").is_file());
        // Cut 模式：剪贴板清空、选区清空。
        assert!(model.clipboard.is_none());
        assert!(model.selection.is_empty());
        // 焦点 = target_parent（sub）；之前用户就在这一行按了 Cmd+V，留在原地最直观。
        assert_eq!(model.selected.as_deref(), Some(root.join("sub").as_path()));
    }

    #[test]
    fn cut_paste_should_rebase_open_buffers_to_new_path() {
        let (mut model, root) = model_with_two_files_and_subdir("paste-rebase-buffer");
        // 假设用户已经把 a.txt 打开成 buffer。
        let mut workspace = Workspace::new();
        let buffer_id = workspace.open_file(root.join("a.txt")).unwrap();
        assert_eq!(
            workspace.buffer_path(buffer_id).unwrap(),
            Some(root.join("a.txt").as_path())
        );

        // 把 a.txt cut 到 sub 下。
        model.move_selection(1);
        model.move_selection(1);
        model.move_selection(1);
        model.extend_selection(0);
        model.cut_to_clipboard();
        model.selected = Some(root.join("sub"));
        model.paste_from_clipboard(&mut workspace);

        // buffer 的绑定路径已 rebase。
        assert_eq!(
            workspace.buffer_path(buffer_id).unwrap(),
            Some(root.join("sub/a.txt").as_path())
        );
        // 没把 buffer 标 dirty。
        assert!(!workspace.is_buffer_dirty(buffer_id).unwrap());
    }

    #[test]
    fn copy_paste_to_same_directory_should_auto_rename() {
        let (mut model, root) = model_with_two_files_and_subdir("paste-rename");
        // 焦点在 a.txt，选它并复制。
        model.move_selection(1);
        model.move_selection(1);
        model.move_selection(1); // a.txt
        model.extend_selection(0);
        model.copy_to_clipboard();
        // 粘贴目标 = root（焦点是 a.txt 文件，target = 父=root）。
        let mut workspace = Workspace::new();
        model.paste_from_clipboard(&mut workspace);

        // 同目录复制走自动改名 → "a (1).txt"。
        assert!(root.join("a (1).txt").is_file());
        assert!(root.join("a.txt").is_file());
    }

    #[test]
    fn paste_into_collapsed_directory_should_expand_it_and_focus_target_parent() {
        // 粘到 sub（初始折叠态）：粘贴后 sub 应展开、焦点应停在 sub 上。
        let (mut model, root) = model_with_two_files_and_subdir("paste-expand-target");
        // 选 a.txt（同前几个测试套路）。
        model.move_selection(1);
        model.move_selection(1);
        model.move_selection(1); // a.txt
        model.extend_selection(0);
        model.copy_to_clipboard();
        model.selected = Some(root.join("sub"));

        // 验证粘贴前 sub 是折叠态。
        {
            let tree = model.project_tree.as_ref().unwrap();
            assert!(!tree.is_expanded(&root.join("sub")));
        }

        let mut workspace = Workspace::new();
        model.paste_from_clipboard(&mut workspace);

        // 焦点 = sub（target_parent）。
        assert_eq!(model.selected.as_deref(), Some(root.join("sub").as_path()));
        // sub 应已展开，按 ↓ 一步即可看到 sub/a.txt。
        let tree = model.project_tree.as_ref().unwrap();
        assert!(tree.is_expanded(&root.join("sub")));
        let names: Vec<_> = tree
            .visible_rows()
            .into_iter()
            .map(|row| row.name.to_string())
            .collect();
        assert!(names.contains(&"a.txt".to_string()));
    }

    #[test]
    fn request_delete_should_snapshot_selection_when_non_empty() {
        let (mut model, root) = model_with_two_files_and_subdir("request-del-from-selection");
        // 累积选区到 {root, sub, a.txt}（典型 Shift+↓ 三次得到的状态）。
        model.ensure_selection_initialized();
        model.extend_selection(1);
        model.extend_selection(1);
        // 焦点也置一个；但选区非空，应优先用选区。
        model.selected = Some(root.join("b.txt"));

        model.request_delete();
        let items = model.pending_delete.as_ref().expect("应有 pending_delete");
        let paths: BTreeSet<_> = items.iter().map(|(p, _)| p.clone()).collect();
        // 项目根被剔除（不可删），剩下 sub + a.txt。
        let expected: BTreeSet<_> = [root.join("sub"), root.join("a.txt")].into_iter().collect();
        assert_eq!(paths, expected);
        // first_kind/has_directory 由 state() 计算；含 sub（目录）。
        let state = model.state(&Workspace::new());
        let pending = state.pending_delete.expect("状态应有待删除项");
        assert_eq!(pending.count, 2);
        assert!(pending.has_directory);
    }

    #[test]
    fn request_delete_should_fallback_to_focus_when_selection_empty() {
        let (mut model, root) = model_with_two_files_and_subdir("request-del-fallback");
        model.selected = Some(root.join("a.txt"));
        // 选区空。
        model.request_delete();
        let items = model.pending_delete.as_ref().expect("应有 pending_delete");
        assert_eq!(items.len(), 1);
        assert_eq!(items[0].0, root.join("a.txt"));
        let state = model.state(&Workspace::new());
        let pending = state.pending_delete.expect("状态应有待删除项");
        assert_eq!(pending.count, 1);
        assert!(!pending.has_directory);
    }

    #[test]
    fn request_delete_should_skip_when_only_root_selected() {
        let (mut model, root) = model_with_two_files_and_subdir("request-del-only-root");
        // 选区里只有项目根——过滤后空集，不应产生 pending_delete。
        model.selection.insert(root.clone());
        model.request_delete();
        assert!(model.pending_delete.is_none());
    }

    #[test]
    fn confirm_delete_batch_should_remove_all_clear_selection_and_pick_safe_sibling() {
        // root/{a,b,c}.txt：选 {a,b} 删除 → 都消失，焦点跳到 c（跳过待删的 b）。
        let root = tmp_root("confirm-del-batch");
        File::create(root.join("a.txt")).unwrap();
        File::create(root.join("b.txt")).unwrap();
        File::create(root.join("c.txt")).unwrap();
        let mut model = FileTreeModel::default();
        model.open_project(root.clone());
        model.selection.insert(root.join("a.txt"));
        model.selection.insert(root.join("b.txt"));
        model.selected = Some(root.join("a.txt"));

        model.request_delete();
        let mut workspace = Workspace::new();
        let mut views = ViewSet::new();
        model.confirm_delete(&mut workspace, &mut views);

        assert!(!root.join("a.txt").exists());
        assert!(!root.join("b.txt").exists());
        assert!(root.join("c.txt").is_file());
        // 选区被清空，焦点跳到 c.txt（首项 a.txt 的下一兄弟，跳过同被删的 b.txt）。
        assert!(model.selection.is_empty());
        assert_eq!(
            model.selected.as_deref(),
            Some(root.join("c.txt").as_path())
        );
    }

    #[test]
    fn confirm_delete_should_focus_next_sibling_when_available() {
        // root/{a,b,c}.txt：删 b → 焦点跳到 c（按字母序的下一兄弟）。
        let root = tmp_root("delete-focus-next-sibling");
        File::create(root.join("a.txt")).unwrap();
        File::create(root.join("b.txt")).unwrap();
        File::create(root.join("c.txt")).unwrap();
        let mut model = FileTreeModel::default();
        model.open_project(root.clone());
        model.selected = Some(root.join("b.txt"));
        model.pending_delete = Some(vec![(root.join("b.txt"), EntryKind::File)]);

        let mut workspace = Workspace::new();
        let mut views = ViewSet::new();
        model.confirm_delete(&mut workspace, &mut views);

        assert_eq!(
            model.selected.as_deref(),
            Some(root.join("c.txt").as_path())
        );
    }

    #[test]
    fn confirm_delete_should_fallback_to_active_buffer_when_no_next_sibling() {
        // root/{a,b}.txt + 打开 a 作为活动文件；删 b（同父最后一项）→ 焦点跳到 a。
        let root = tmp_root("delete-fallback-active");
        File::create(root.join("a.txt")).unwrap();
        File::create(root.join("b.txt")).unwrap();
        let mut model = FileTreeModel::default();
        model.open_project(root.clone());

        let mut workspace = Workspace::new();
        let a_id = workspace.open_file(root.join("a.txt")).unwrap();
        workspace.set_active_buffer(a_id).unwrap();
        let mut views = ViewSet::new();
        open_view_for(&workspace, &mut views, a_id);

        model.selected = Some(root.join("b.txt"));
        model.pending_delete = Some(vec![(root.join("b.txt"), EntryKind::File)]);
        model.confirm_delete(&mut workspace, &mut views);

        assert_eq!(
            model.selected.as_deref(),
            Some(root.join("a.txt").as_path())
        );
    }

    #[test]
    fn confirm_delete_should_fallback_to_root_when_no_sibling_and_no_active() {
        // root/only.txt：删 only → 既无下一兄弟也无活动 buffer，落到项目根（首行）。
        let root = tmp_root("delete-fallback-root");
        File::create(root.join("only.txt")).unwrap();
        let mut model = FileTreeModel::default();
        model.open_project(root.clone());

        model.selected = Some(root.join("only.txt"));
        model.pending_delete = Some(vec![(root.join("only.txt"), EntryKind::File)]);

        let mut workspace = Workspace::new();
        let mut views = ViewSet::new();
        model.confirm_delete(&mut workspace, &mut views);

        assert_eq!(model.selected.as_deref(), Some(root.as_path()));
    }

    #[test]
    fn next_sibling_should_skip_descendants_of_expanded_directory() {
        // root/{adir/inner.txt, b.txt}：展开 adir 后删 adir，应跳到 b 而不是 inner。
        let root = tmp_root("next-sibling-skip-descendants");
        create_dir_all(root.join("adir")).unwrap();
        File::create(root.join("adir/inner.txt")).unwrap();
        File::create(root.join("b.txt")).unwrap();
        let mut tree = ProjectTree::new(root.clone()).unwrap();
        tree.expand(&root.join("adir")).unwrap();

        let sibling = next_sibling_of(&tree, &root.join("adir"), |_| false);
        assert_eq!(sibling, Some(root.join("b.txt")));
    }

    #[test]
    fn deleting_only_active_file_should_select_first_file_tree_row() {
        let root = tmp_root("delete-active-selects-root");
        let readme = root.join("README.md");
        File::create(&readme).unwrap();

        let tree = ProjectTree::new(root.clone()).unwrap();
        let mut workspace = Workspace::new();
        let readme_id = workspace.open_file(readme.clone()).unwrap();
        let mut views = ViewSet::new();
        open_view_for(&workspace, &mut views, readme_id);

        close_buffers_under(&mut workspace, &mut views, &readme);

        assert!(workspace.active_buffer().is_none());
        assert_eq!(
            selected_path_after_deleting_active(&tree, &workspace).as_deref(),
            Some(root.as_path())
        );
    }
}
