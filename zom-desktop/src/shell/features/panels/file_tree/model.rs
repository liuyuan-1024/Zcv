//! 文件树运行模型。
//!
//! 本文件只保留 `FileTreeModel` 的状态骨架、项目切换与渲染快照构造。
//! 具体行为按变化原因拆到 sibling modules：
//! - `selection`：选择 / 扩选 / Esc 清选区；
//! - `inline_edit`：新建 / 重命名输入框与 TextTarget 适配；
//! - `clipboard`：内部 copy / cut 快照；
//! - `fs_ops`：文件系统操作与目录树动作；
//! - `WorkspaceSession`：文件操作后的 buffer / view 同步。

use std::collections::BTreeSet;
use std::path::PathBuf;

use zom_command::BubbleRequest;
use zom_workspace::{EntryKind, ProjectTree};

#[cfg(test)]
use crate::editor::text::EditorSnapshotRequest;

use super::clipboard::{ClipboardMode, FileTreeClipboard};
use super::fs_ops::infer_entry_kind_from_input;
#[cfg(test)]
use super::fs_ops::{compute_paste_target, next_sibling_of};
use super::inline_edit::{PendingEntry, PendingRenameEntry};
use super::selection::Stroke;
#[cfg(test)]
use super::{FileTreeActivation, FileTreeOutcome};
use super::{FileTreeRow, FileTreeState, PendingDelete, PendingNewEntry, PendingRename};

pub(crate) struct FileTreeModel {
    pub(super) project_tree: Option<ProjectTree>,
    pub(super) selected: Option<PathBuf>,
    /// **已提交的选区**——过去 Shift+方向"笔画"沉淀下来的、通过普通方向键提交的项。
    /// 当前正在进行中的笔画不存在这里，而是放在 [`stroke`](Self::stroke) 里，可随 Shift+↑/↓ 自由伸缩。
    /// 对外暴露（[`state()`](Self::state) / 复制 / 粘贴 / 删除）总是看二者的并集。
    pub(super) selection: BTreeSet<PathBuf>,
    /// 当前活跃的"扩选笔画"。
    /// 第一次按 Shift+方向时建立、锚定在按键时的焦点行；
    /// 后续 Shift+↑/↓ 不再追加而是**重算 `[锚点, 新焦点]` 区间**，因此可以缩。
    /// 普通方向键（不带 Shift）会把它的 `items` 并入[`selection`](Self::selection) 然后清空 stroke——这一步称为"提交"。
    pub(super) stroke: Option<Stroke>,
    /// 内部剪贴板。Copy / Cut 时拍下当时的选区（空选区降级到焦点单项）。
    /// 跨进程不参与——本阶段仅 zom 内部生效。
    pub(super) clipboard: Option<FileTreeClipboard>,
    /// 正在键入名称的新建条目；`None` 表示不处于新建态。
    pub(super) pending: Option<PendingEntry>,
    /// 正在重命名的条目；与 [`pending`](Self::pending) 互斥（同一时刻只能开一个输入框）。
    pub(super) pending_rename: Option<PendingRenameEntry>,
    /// 正在等待确认的待删条目集合（路径 + 类型）。
    /// 批量删（选区非空）与单删（焦点回退）共用同一份字段；
    /// `None` 表示无删除确认弹窗。
    pub(super) pending_delete: Option<Vec<(PathBuf, EntryKind)>>,
    /// 待发出的气泡（面向用户的错误 / 提示）。runtime 在调用模型动作后 drain。
    pub(super) pending_bubbles: Vec<BubbleRequest>,
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
            pending_rename: None,
            pending_delete: None,
            pending_bubbles: Vec::new(),
        }
    }

    /// 取走累积的气泡请求，留空队列给下一次动作。
    pub(crate) fn take_bubbles(&mut self) -> Vec<BubbleRequest> {
        std::mem::take(&mut self.pending_bubbles)
    }

    pub(crate) fn open_project(&mut self, root: PathBuf) {
        self.project_tree = match ProjectTree::new(root.clone()) {
            Ok(tree) => Some(tree),
            Err(error) => {
                self.pending_bubbles.push(
                    BubbleRequest::error(format!("读取项目目录失败：{}：{error}", root.display()))
                        .dedupe("file_tree.open_project"),
                );
                None
            }
        };
        self.selected = None;
        self.selection.clear();
        self.stroke = None;
        self.clipboard = None;
        self.pending = None;
        self.pending_rename = None;
        self.pending_delete = None;
    }

    pub(crate) fn state(&self, active_buffer_path: Option<PathBuf>) -> FileTreeState {
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
                terminal_mask: row.terminal_mask,
            })
            .collect();
        let active = active_buffer_path;
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
        // 重命名快照：从可见行里查目标行的 depth / kind，落不到（被外部移除 / 折叠不可触达）就丢弃，输入态自然降级回 Navigate。
        let pending_rename = self.pending_rename.as_ref().and_then(|pending| {
            rows.iter()
                .find(|row| row.path == pending.path)
                .map(|row| PendingRename {
                    path: pending.path.clone(),
                    kind: row.kind,
                    depth: row.depth,
                })
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
            pending_rename,
            pending_delete,
        }
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
    use crate::editor::text::OwnedEditorTarget;
    use crate::shell::features::panels::file_tree::fs_ops::{
        apply_outcome, selected_path_after_deleting_active,
    };
    use crate::workspace_session::WorkspaceSession;
    use std::fs::{File, create_dir_all};
    use zom_workspace::view::{ViewId, ViewSet};
    use zom_workspace::{BufferId, Workspace};

    /// 测试桥：跑模型动作 → 翻 outcome 到 session，得到 activation。
    /// 让 `model.foo(); apply_outcome(...)` 两步在测试里合成一行。
    fn apply<F>(
        model: &mut FileTreeModel,
        session: &mut WorkspaceSession,
        f: F,
    ) -> FileTreeActivation
    where
        F: FnOnce(&mut FileTreeModel) -> FileTreeOutcome,
    {
        let outcome = f(model);
        apply_outcome(model, outcome, session)
    }

    fn tmp_root(name: &str) -> PathBuf {
        let dir =
            std::env::temp_dir().join(format!("zom-file-tree-model-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        create_dir_all(&dir).unwrap();
        // model 层的测试聚焦选区 / 剪贴板 / 新建-重命名，断言「root 下只有显式创建的项」。
        // 生产里 `ProjectTree::new` 会自动生成 `.zom/.zomignore`，默认不忽略 `.zom/` 自身，会多出一行污染断言。
        // 这里预先写一份只忽略 `.zom/` 的 `.zomignore`——`ensure_zomignore_exists` 检测到文件已存在就不会覆盖，
        // 效果等价于让测试在「.zom/ 对文件树隐形」的项目里跑。
        let zom_dir = dir.join(".zom");
        create_dir_all(&zom_dir).unwrap();
        std::fs::write(zom_dir.join(".zomignore"), ".zom/\n").unwrap();
        dir
    }

    fn open_view_for(workspace: &Workspace, views: &mut ViewSet, buffer_id: BufferId) -> ViewId {
        let version = workspace
            .buffer(buffer_id)
            .expect("缓冲区应存在")
            .buffer()
            .version();
        views.open_edit_view(buffer_id, version)
    }

    fn workspace_session(workspace: Workspace, views: ViewSet) -> WorkspaceSession {
        WorkspaceSession::new(workspace, views)
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
        let _lib_view = open_view_for(&workspace, &mut views, lib_id);
        let mut session = workspace_session(workspace, views);
        // 模拟"lib.rs 是当前活动 view"——session 是活动视图的真相源。
        let lib_view_id = session
            .views()
            .find_edit_view_for_buffer(lib_id)
            .expect("lib.rs 的编辑视图应已存在");
        session.set_active_view(lib_view_id);

        session.close_buffers_under(&lib);

        assert_eq!(
            session
                .active_buffer_id()
                .and_then(|id| session.workspace().buffer(id))
                .and_then(|buffer| buffer.path()),
            Some(readme.as_path())
        );
        let active_path = session
            .active_buffer_id()
            .and_then(|id| session.workspace().buffer(id))
            .and_then(|wb| wb.path())
            .map(PathBuf::from);
        assert_eq!(
            selected_path_after_deleting_active(&tree, active_path).as_deref(),
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

    #[test]
    fn extend_selection_should_add_both_endpoints_first_call() {
        let (mut model, root) = model_with_three_files("extend-first-call");
        // 焦点初始化到第一行（root 自身在 visible_rows 中是第一行）。
        model.ensure_selection_initialized();
        assert_eq!(model.selected.as_deref(), Some(root.as_path()));

        // 第一次 Shift+↓：起点（root）与终点（a.txt）都应进选区。
        model.extend_selection(1);
        let state = model.state(None);
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
        let state = model.state(None);
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
        let state = model.state(None);
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
        let state = model.state(None);
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

        let state = model.state(None);
        assert!(state.cut_paths.contains(&root.join("a.txt")));
        assert_eq!(state.cut_paths.len(), 1);

        // Copy 模式不会暴露 cut_paths。
        model.copy_to_clipboard();
        let state = model.state(None);
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

        let mut session = workspace_session(Workspace::new(), ViewSet::new());
        apply(&mut model, &mut session, |m| m.paste_from_clipboard());

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

        let mut session = workspace_session(Workspace::new(), ViewSet::new());
        apply(&mut model, &mut session, |m| m.paste_from_clipboard());

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
        let mut session = workspace_session(workspace, ViewSet::new());
        assert_eq!(
            session.workspace().buffer_path(buffer_id).unwrap(),
            Some(root.join("a.txt").as_path())
        );

        // 把 a.txt cut 到 sub 下。
        model.move_selection(1);
        model.move_selection(1);
        model.move_selection(1);
        model.extend_selection(0);
        model.cut_to_clipboard();
        model.selected = Some(root.join("sub"));
        apply(&mut model, &mut session, |m| m.paste_from_clipboard());

        // buffer 的绑定路径已 rebase。
        assert_eq!(
            session.workspace().buffer_path(buffer_id).unwrap(),
            Some(root.join("sub/a.txt").as_path())
        );
        // 没把 buffer 标 dirty。
        assert!(!session.workspace().is_buffer_dirty(buffer_id).unwrap());
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
        let mut session = workspace_session(Workspace::new(), ViewSet::new());
        apply(&mut model, &mut session, |m| m.paste_from_clipboard());

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

        let mut session = workspace_session(Workspace::new(), ViewSet::new());
        apply(&mut model, &mut session, |m| m.paste_from_clipboard());

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
        let state = model.state(None);
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
        let state = model.state(None);
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
        let mut session = workspace_session(Workspace::new(), ViewSet::new());
        apply(&mut model, &mut session, |m| m.confirm_delete());

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

        let mut session = workspace_session(Workspace::new(), ViewSet::new());
        apply(&mut model, &mut session, |m| m.confirm_delete());

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
        let mut views = ViewSet::new();
        let a_view_id = open_view_for(&workspace, &mut views, a_id);
        let mut session = workspace_session(workspace, views);
        session.set_active_view(a_view_id);

        model.selected = Some(root.join("b.txt"));
        model.pending_delete = Some(vec![(root.join("b.txt"), EntryKind::File)]);
        apply(&mut model, &mut session, |m| m.confirm_delete());

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

        let mut session = workspace_session(Workspace::new(), ViewSet::new());
        apply(&mut model, &mut session, |m| m.confirm_delete());

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
        let mut session = workspace_session(workspace, views);

        session.close_buffers_under(&readme);

        assert!(session.active_buffer_id().is_none());
        assert_eq!(
            selected_path_after_deleting_active(&tree, None).as_deref(),
            Some(root.as_path())
        );
    }

    #[test]
    fn begin_rename_should_prefill_old_name_and_skip_root() {
        let (mut model, root) = model_with_three_files("rename-begin");
        // 焦点在 root：begin_rename 不应建立 pending_rename（项目根不可改名）。
        model.ensure_selection_initialized();
        assert_eq!(model.selected.as_deref(), Some(root.as_path()));
        model.begin_rename();
        assert!(model.pending_rename.is_none());

        // 焦点在 a.txt：建立 pending_rename，输入框文本 = 旧名。
        model.move_selection(1);
        assert_eq!(
            model.selected.as_deref(),
            Some(root.join("a.txt").as_path())
        );
        model.begin_rename();
        let pending = model.pending_rename.as_ref().expect("应有 pending_rename");
        let snapshot = pending
            .editor
            .snapshot(EditorSnapshotRequest::single_line());
        assert_eq!(pending.path, root.join("a.txt"));
        assert_eq!(pending.editor.text(), "a.txt");
        assert_eq!(snapshot.cursor_byte, "a.txt".len());
        assert!(snapshot.selection.primary().is_caret());
    }

    #[test]
    fn commit_rename_should_move_file_and_update_focus_and_rebase_buffer() {
        let (mut model, root) = model_with_three_files("rename-commit");
        let mut workspace = Workspace::new();
        // 先把 a.txt 打开成 buffer，验证 rebase。
        let a_id = workspace.open_file(root.join("a.txt")).unwrap();
        let mut session = workspace_session(workspace, ViewSet::new());

        model.selected = Some(root.join("a.txt"));
        model.begin_rename();
        // 模拟用户改名为 "renamed.txt"。
        {
            let pending = model.pending_rename.as_mut().unwrap();
            pending.editor = OwnedEditorTarget::with_text_all_selected("renamed.txt");
        }
        let activation = apply(&mut model, &mut session, |m| m.commit_rename());

        // 文件落到新路径、旧路径消失。
        assert!(root.join("renamed.txt").is_file());
        assert!(!root.join("a.txt").exists());
        // 焦点跟随到新路径；pending_rename 清空。
        assert_eq!(
            model.selected.as_deref(),
            Some(root.join("renamed.txt").as_path())
        );
        assert!(model.pending_rename.is_none());
        // buffer 路径已 rebase。
        assert_eq!(
            session.workspace().buffer_path(a_id).unwrap(),
            Some(root.join("renamed.txt").as_path())
        );
        // 文件改名后顺势把焦点切给编辑器：被 rebase 的 buffer 设为活动。
        assert_eq!(activation, FileTreeActivation::OpenedFile);
        assert_eq!(
            session
                .active_buffer_id()
                .and_then(|id| session.workspace().buffer(id))
                .and_then(|b| b.path()),
            Some(root.join("renamed.txt").as_path())
        );
    }

    #[test]
    fn commit_rename_should_open_renamed_file_without_prior_buffer() {
        let (mut model, root) = model_with_three_files("rename-commit-open");
        let mut session = workspace_session(Workspace::new(), ViewSet::new());

        model.selected = Some(root.join("a.txt"));
        model.begin_rename();
        {
            let pending = model.pending_rename.as_mut().unwrap();
            pending.editor = OwnedEditorTarget::with_text_all_selected("renamed.txt");
        }
        let activation = apply(&mut model, &mut session, |m| m.commit_rename());

        assert_eq!(activation, FileTreeActivation::OpenedFile);
        // 文件原本没有 buffer，commit 后应被打开并设为活动。
        assert_eq!(
            session
                .active_buffer_id()
                .and_then(|id| session.workspace().buffer(id))
                .and_then(|b| b.path()),
            Some(root.join("renamed.txt").as_path())
        );
    }

    #[test]
    fn commit_rename_on_directory_should_keep_focus_in_file_tree_and_expand_it() {
        // root/sub/ → 改名后没有可打开内容，activation = Nothing；目录展开以给出视觉反馈。
        let (mut model, root) = model_with_two_files_and_subdir("rename-dir");
        // 在 sub 下放一个子项，确保展开后能观察到 visible_rows 新增一行。
        File::create(root.join("sub/inner.txt")).unwrap();
        // 重 open 一次，让新建的子项进入缓存。
        model.open_project(root.clone());
        let mut session = workspace_session(Workspace::new(), ViewSet::new());

        model.selected = Some(root.join("sub"));
        model.begin_rename();
        {
            let pending = model.pending_rename.as_mut().unwrap();
            pending.editor = OwnedEditorTarget::with_text_all_selected("sub2");
        }
        let activation = apply(&mut model, &mut session, |m| m.commit_rename());

        assert_eq!(activation, FileTreeActivation::Nothing);
        assert!(root.join("sub2").is_dir());
        assert!(session.active_buffer_id().is_none());
        // 目录已展开：可见行里能看到 sub2/inner.txt。
        let tree = model.project_tree.as_ref().unwrap();
        assert!(tree.is_expanded(&root.join("sub2")));
        let names: Vec<_> = tree
            .visible_rows()
            .into_iter()
            .map(|row| row.name.to_string())
            .collect();
        assert!(names.contains(&"inner.txt".to_string()));
        // 焦点跳到下一行——展开后正好是 sub2 的首个子项。
        assert_eq!(
            model.selected.as_deref(),
            Some(root.join("sub2/inner.txt").as_path())
        );
    }

    #[test]
    fn commit_rename_on_empty_directory_should_fallback_focus_to_previous_row() {
        // root/sub/：只有一个空目录，重命名后展开也无子项；下一行不存在 → 退回上一行 = root。
        let root = tmp_root("rename-dir-empty-fallback");
        create_dir_all(root.join("sub")).unwrap();
        let mut model = FileTreeModel::default();
        model.open_project(root.clone());

        let mut session = workspace_session(Workspace::new(), ViewSet::new());
        model.selected = Some(root.join("sub"));
        model.begin_rename();
        {
            let pending = model.pending_rename.as_mut().unwrap();
            pending.editor = OwnedEditorTarget::with_text_all_selected("renamed");
        }
        let activation = apply(&mut model, &mut session, |m| m.commit_rename());

        assert_eq!(activation, FileTreeActivation::Nothing);
        assert!(root.join("renamed").is_dir());
        // visible_rows = [root, renamed]，renamed 是最后一行 → 焦点退回上一行 root。
        assert_eq!(model.selected.as_deref(), Some(root.as_path()));
    }

    #[test]
    fn commit_rename_should_keep_pending_on_conflict() {
        let (mut model, root) = model_with_three_files("rename-conflict");
        let mut session = workspace_session(Workspace::new(), ViewSet::new());
        model.selected = Some(root.join("a.txt"));
        model.begin_rename();
        // 改成已存在的 b.txt——磁盘冲突，pending_rename 保留供用户重试。
        {
            let pending = model.pending_rename.as_mut().unwrap();
            pending.editor = OwnedEditorTarget::with_text_all_selected("b.txt");
        }
        let _ = apply(&mut model, &mut session, |m| m.commit_rename());

        assert!(root.join("a.txt").is_file());
        assert!(root.join("b.txt").is_file());
        assert!(model.pending_rename.is_some());
    }

    #[test]
    fn commit_rename_with_unchanged_name_should_drop_pending_quietly() {
        let (mut model, root) = model_with_three_files("rename-same-name");
        let mut session = workspace_session(Workspace::new(), ViewSet::new());
        model.selected = Some(root.join("a.txt"));
        model.begin_rename();
        // 不动文本（默认就是 a.txt），直接提交——等价于取消。
        let _ = apply(&mut model, &mut session, |m| m.commit_rename());
        assert!(root.join("a.txt").is_file());
        assert!(model.pending_rename.is_none());
    }
}
