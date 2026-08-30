//! VersionControlPanel —— 版本管理面板 Entity 组件。
//!
//! 无 git 仓库时居中显示"初始化仓库"按钮（点击对项目根执行 `git init`）；
//! 有仓库时按 已暂存/未暂存 两组展示变更目录树，部分暂存文件同时出现在已暂存与未暂存两组。
//! 冲突文件暂不展示（待后续版本处理）。
//! 行尾复选框（或空格键）切换条目的暂存/取消暂存：已暂存组勾选、未暂存组未勾选。
//! 行模型由 GitStore 快照构建，订阅 Repositories/Statuses 事件重建。

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    App, Context, Div, ElementId, Entity, EventEmitter, FocusHandle, KeyContext, MouseButton,
    ScrollStrategy, UniformListScrollHandle, WeakEntity, Window, div, prelude::*, uniform_list,
};
use zcv_actions::{
    Activate, Collapse, Commit, Expand, InitRepository, SelectNext, SelectPrev, ToggleStaged,
    Uncommit,
};
use zcv_editor::Editor;
use zcv_git::{DiffStat, FileStatus, StatusCode};
use zcv_project::{GitStoreEvent, Project, RepositorySnapshot};
use zcv_theme::{color, space, typography};
use zcv_ui::tree::{self, TreeRow, TreeState};
use zcv_ui::{Button, ButtonSize, ButtonStyle, Checkbox, Scrollbar, SvgIcon};
use zcv_workspace::{Panel, PanelEvent};

use zcv_project_tree::git_status_color;

use crate::project_diff::ProjectDiffKind;

/// 打开 Git 项目差异并定位文件的回调（弱 Workspace 引用由装配层捕获）。
pub(crate) type OnOpenGitDiff =
    Rc<dyn Fn(ProjectDiffKind, PathBuf, bool, &mut Window, &mut gpui::App)>;

// 版本控制快捷键归属于 `VersionControl` 上下文，由统一快捷键注册表加载；组件内不重复注册。

// ═══ 分组与建树纯函数 ═══════════════════════════════════════════

/// 条目出现在哪些组：(已暂存, 未暂存)。
///
/// Ignored 由调用方过滤；冲突文件暂不展示（待后续版本处理）。
fn entry_sections(status: FileStatus) -> (bool, bool) {
    match status {
        FileStatus::Unmerged => (false, false),
        FileStatus::Untracked => (false, true),
        FileStatus::Tracked {
            index_status,
            worktree_status,
        } => (
            index_status != StatusCode::Unmodified,
            worktree_status != StatusCode::Unmodified,
        ),
        FileStatus::Ignored => (false, false),
    }
}

/// 按分组构建变更目录树（每组的根列表），并完成目录聚合与排序。
///
/// 所有仓库合并进同一棵分组树：嵌套仓库在父仓库的 status 中不展开（目录级 untracked 条目被解析器跳过），路径前缀互斥，合并无冲突。
fn build_section_trees<'a>(
    root: &Path,
    repositories: impl Iterator<Item = (&'a Path, &'a RepositorySnapshot)>,
) -> [Vec<GitTreeNode>; 2] {
    let mut roots = [Vec::new(), Vec::new()];
    for (workdir, snapshot) in repositories {
        for (relative, entry) in &snapshot.statuses_by_path {
            if entry.status.is_ignored() {
                continue;
            }
            let (in_staged, in_unstaged) = entry_sections(entry.status);
            if in_staged {
                insert_entry(
                    &mut roots[0],
                    root,
                    workdir,
                    relative,
                    entry.status,
                    entry.staged_diff_stat,
                );
            }
            if in_unstaged {
                insert_entry(
                    &mut roots[1],
                    root,
                    workdir,
                    relative,
                    entry.status,
                    entry.unstaged_diff_stat,
                );
            }
        }
    }
    for section_tree in &mut roots {
        for node in section_tree.iter_mut() {
            finalize_node(node);
        }
        // 顶层节点同样按（目录优先、名称）排序。
        section_tree.sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
    }
    roots
}

/// 把一条状态条目插入分组树，合成缺失的中间目录节点。
///
/// 树内路径键优先取项目根相对路径（深度紧凑）；
/// 仓库在项目根外时退化为绝对路径（路径前缀关系不变，分组与折叠语义保持正确）。
fn insert_entry(
    nodes: &mut Vec<GitTreeNode>,
    root: &Path,
    workdir: &Path,
    relative: &Path,
    status: FileStatus,
    diff_stat: DiffStat,
) {
    let absolute = workdir.join(relative);
    // 树内路径键优先取项目根相对路径；仓库在项目根外时 strip_prefix 失败，退化为绝对路径。
    let key_is_relative = absolute.strip_prefix(root).is_ok();
    let key = absolute.strip_prefix(root).unwrap_or(&absolute);
    let mut current = nodes;
    let mut components = key.components().peekable();
    let mut prefix = PathBuf::new();
    while let Some(component) = components.next() {
        let name = component.as_os_str().to_string_lossy().into_owned();
        prefix.push(&name);
        let is_last = components.peek().is_none();
        let node_absolute = if is_last {
            absolute.clone()
        } else if key_is_relative {
            root.join(&prefix)
        } else {
            prefix.clone()
        };
        if is_last {
            // 叶子：同名节点已存在（路径冲突的理论分支）时更新状态，不覆盖目录结构。
            match current.iter_mut().find(|node| node.name == name) {
                Some(node) => {
                    node.status = Some(status);
                    node.diff_stat = diff_stat;
                }
                None => current.push(GitTreeNode {
                    path: node_absolute,
                    name,
                    is_dir: false,
                    status: Some(status),
                    diff_stat,
                    children: Vec::new(),
                }),
            }
        } else {
            // 先取下标再借用，避免 match 分支里对同一节点列表的连续可变借用。
            if let Some(index) = current.iter().position(|node| node.name == name) {
                current = &mut current[index].children;
            } else {
                current.push(GitTreeNode {
                    path: node_absolute,
                    name,
                    is_dir: true,
                    status: None,
                    diff_stat: DiffStat::default(),
                    children: Vec::new(),
                });
                current = &mut current.last_mut().expect("刚插入的目录节点").children;
            }
        }
    }
}

/// 目录节点聚合子项状态（priority 最高）与 diff 统计（求和），并排序 children。
///
/// 排序规则：目录优先，再按名称。
fn finalize_node(node: &mut GitTreeNode) {
    if node.is_dir {
        let mut status: Option<FileStatus> = None;
        let mut diff_stat = DiffStat::default();
        for child in &mut node.children {
            finalize_node(child);
            diff_stat.added += child.diff_stat.added;
            diff_stat.deleted += child.diff_stat.deleted;
            if let Some(child_status) = child.status {
                status = Some(match status {
                    Some(current) if current.priority() >= child_status.priority() => current,
                    _ => child_status,
                });
            }
        }
        node.status = status;
        node.diff_stat = diff_stat;
    }
    node.children
        .sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then_with(|| a.name.cmp(&b.name)));
}

/// 树 → 有序行列表：分组头前置（空组也带头），组内树 DFS 先序；折叠的分区只留标题行。
fn flatten_rows(
    trees: &[Vec<GitTreeNode>; 2],
    expanded: &HashSet<(GitSection, PathBuf)>,
    collapsed: &HashSet<GitSection>,
) -> Vec<GitRow> {
    let mut rows = Vec::new();
    for (index, section) in GitSection::ALL.iter().enumerate() {
        rows.push(GitRow::Header(*section));
        if !collapsed.contains(section) {
            flatten_nodes(&mut rows, &trees[index], *section, 0, expanded);
        }
    }
    rows
}

fn flatten_nodes(
    rows: &mut Vec<GitRow>,
    nodes: &[GitTreeNode],
    section: GitSection,
    depth: usize,
    expanded: &HashSet<(GitSection, PathBuf)>,
) {
    for node in nodes {
        let is_expanded = node.is_dir && expanded.contains(&(section, node.path.clone()));
        rows.push(GitRow::Entry(GitTreeRow {
            section,
            path: node.path.clone(),
            name: node.name.clone(),
            depth,
            is_dir: node.is_dir,
            expanded: is_expanded,
            status: node.status,
            diff_stat: node.diff_stat,
        }));
        if is_expanded {
            flatten_nodes(rows, &node.children, section, depth + 1, expanded);
        }
    }
}

/// 收集所有分组树中的目录节点键（(分组, 绝对路径)），供默认全展开使用。
fn collect_directory_keys(trees: &[Vec<GitTreeNode>; 2]) -> HashSet<(GitSection, PathBuf)> {
    let mut keys = HashSet::new();
    for (index, section) in GitSection::ALL.iter().enumerate() {
        collect_dirs(&trees[index], *section, &mut keys);
    }
    keys
}

fn collect_dirs(
    nodes: &[GitTreeNode],
    section: GitSection,
    keys: &mut HashSet<(GitSection, PathBuf)>,
) {
    for node in nodes {
        if node.is_dir {
            keys.insert((section, node.path.clone()));
            collect_dirs(&node.children, section, keys);
        }
    }
}

// ═══ Entity ═══════════════════════════════════════════════════════

pub(crate) struct VersionControlPanel {
    focus: FocusHandle,
    focus_listeners_initialized: bool,
    project: Entity<Project>,
    state: Rc<RefCell<TreeState<(GitSection, PathBuf), GitRow>>>,
    /// 用户显式折叠的目录（(分组, 路径)）；未折叠的目录默认展开，新出现的目录自动展开。
    collapsed_dirs: HashSet<(GitSection, PathBuf)>,
    /// 折叠的分区（点击分区标题行首 chevron 切换；折叠时该分区条目不渲染）。
    collapsed_sections: Rc<RefCell<HashSet<GitSection>>>,
    scroll_handle: UniformListScrollHandle,
    scrollbar: Scrollbar<UniformListScrollHandle>,
    /// 底部提交信息编辑器。
    commit_editor: Entity<Editor>,
    /// 活动仓库最近一次提交的 subject（订阅 Repositories/Statuses/Head 时刷新）。
    last_commit_message: Option<String>,
    /// 自己发起的提交在途：Head 事件时清空编辑器并复位（外部 checkout/commit 不清草稿）。
    pending_commit: bool,
    on_open_file: Option<OnOpenGitDiff>,
}

impl VersionControlPanel {
    pub(crate) fn new(project: Entity<Project>, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let git_store = project.read(cx).git_store();
        let commit_editor = cx.new(|cx| {
            let mut editor = Editor::auto_height(6, Some(6), cx);
            editor.set_placeholder_text("输入提交信息…", cx);
            editor
        });
        // 编辑器内容变化即时重绘（按钮可提交态随文本刷新）。
        cx.observe(&commit_editor, |_, _, cx| cx.notify()).detach();
        cx.subscribe(&git_store, |panel, _, event, cx| {
            match event {
                GitStoreEvent::Repositories | GitStoreEvent::Statuses => {
                    panel.rebuild_rows(cx);
                    panel.refresh_last_commit_message(cx);
                }
                GitStoreEvent::Head => {
                    // HEAD 变化（提交/撤销提交/外部 checkout）：提交信息随之刷新；
                    // 只清空自己发起的提交，外部变更保留草稿。
                    panel.refresh_last_commit_message(cx);
                    if panel.pending_commit {
                        panel
                            .commit_editor
                            .update(cx, |editor, cx| editor.set_text("", cx));
                        panel.pending_commit = false;
                    }
                }
                // 撤销提交成功：事件直接携带被撤销消息，填回提交信息编辑器。
                GitStoreEvent::Uncommitted(message) => {
                    panel
                        .commit_editor
                        .update(cx, |editor, cx| editor.set_text(message, cx));
                }
                GitStoreEvent::ActiveRepositoryChanged => cx.notify(),
                GitStoreEvent::HunksChanged => {}
                GitStoreEvent::JobsUpdated => {}
            }
        })
        .detach();
        let scroll_handle = UniformListScrollHandle::default();
        let scrollbar = Scrollbar::vertical(scroll_handle.clone());
        let mut panel = Self {
            focus,
            focus_listeners_initialized: false,
            project,
            state: Rc::new(RefCell::new(TreeState::new(row_entry_key))),
            collapsed_dirs: HashSet::new(),
            collapsed_sections: Rc::new(RefCell::new(HashSet::new())),
            scroll_handle,
            scrollbar,
            commit_editor,
            last_commit_message: None,
            pending_commit: false,
            on_open_file: None,
        };
        panel.rebuild_rows(cx);
        panel
    }

    pub(crate) fn set_on_open_file(&mut self, callback: OnOpenGitDiff) {
        self.on_open_file = Some(callback);
    }

    /// 按焦点区分版本控制变更树与提交信息编辑器的快捷键上下文。
    fn dispatch_context(&self, window: &Window, cx: &Context<Self>) -> KeyContext {
        let mut context = KeyContext::new_with_defaults();
        if self
            .commit_editor
            .read(cx)
            .focus_handle()
            .is_focused(window)
        {
            context.add("VersionControlCommitEditor");
        } else {
            context.add("VersionControlChangesTree");
        }
        context
    }

    fn changes_tree_is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    /// 从 GitStore 快照重建行模型（订阅事件 / 折叠展开后调用）。
    ///
    /// 项目根实时读取（不缓存）：RootChanged 后树键基准跟随项目，避免与事件流不同步。
    /// GitStore 路径均 canonicalize，这里同样归一化保证前缀比较一致。
    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self
            .project
            .read(cx)
            .root()
            .map(|root| root.canonicalize().unwrap_or_else(|_| root.to_path_buf()))
        else {
            return;
        };
        let git_store = self.project.read(cx).git_store();
        let trees = {
            let store = git_store.read(cx);
            build_section_trees(&root, store.repositories())
        };
        let mut state = self.state.borrow_mut();
        // 目录默认展开：未显式折叠的目录（含新出现的目录）都展开，用户折叠状态保持。
        let directories = collect_directory_keys(&trees);
        state.expanded.extend(
            directories
                .into_iter()
                .filter(|key| !self.collapsed_dirs.contains(key)),
        );
        let rows = flatten_rows(&trees, &state.expanded, &self.collapsed_sections.borrow());
        state.replace_rows(rows);
    }

    /// 切换分区标题的折叠状态（点击标题行首 chevron）：折叠时该分区条目不渲染。
    fn toggle_section_collapsed(&mut self, section: GitSection, cx: &mut Context<Self>) {
        let mut collapsed = self.collapsed_sections.borrow_mut();
        if collapsed.contains(&section) {
            collapsed.remove(&section);
        } else {
            collapsed.insert(section);
        }
        drop(collapsed);
        self.rebuild_rows(cx);
        cx.notify();
    }

    /// 全选/取消全选分区（点击标题行右侧复选框）：未暂存组全部暂存，已暂存组全部取消暂存。
    fn toggle_section_all(&mut self, section: GitSection, cx: &mut Context<Self>) {
        let paths: Vec<PathBuf> = self
            .state
            .borrow()
            .rows
            .iter()
            .filter_map(|row| match row {
                GitRow::Entry(entry) if entry.section == section => Some(entry.path.clone()),
                _ => None,
            })
            .collect();
        if paths.is_empty() {
            return;
        }
        let store = self.project.read(cx).git_store();
        match section {
            GitSection::Staged => store.update(cx, |store, cx| store.unstage_paths(paths, cx)),
            GitSection::Unstaged => store.update(cx, |store, cx| store.stage_paths(paths, cx)),
        }
    }

    /// 激活选中行的共享逻辑：目录→展开/折叠；文件→打开。
    ///
    /// `focus_opened_item` 决定打开文件后是否把焦点交给编辑器：双击/键盘 enter 为 `true`（激活），鼠标单击为 `false`（临时标签，焦点留在面板）。
    fn activate_selected(
        &mut self,
        focus_opened_item: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let row = {
            let state = self.state.borrow();
            state.selected_idx().map(|idx| state.rows[idx].clone())
        };
        let Some(GitRow::Entry(entry)) = row else {
            return;
        };
        if entry.is_dir {
            let key = (entry.section, entry.path.clone());
            // 翻转展开标记，并同步"用户显式折叠"记录（决定后续重建是否保持折叠）。
            let was_expanded = self.state.borrow().expanded.contains(&key);
            self.state.borrow_mut().toggle_expand(&key);
            if was_expanded {
                self.collapsed_dirs.insert(key);
            } else {
                self.collapsed_dirs.remove(&key);
            }
            self.rebuild_rows(cx);
        } else if let Some(callback) = self.on_open_file.clone() {
            let kind = match entry.section {
                GitSection::Staged => ProjectDiffKind::Staged,
                GitSection::Unstaged => ProjectDiffKind::Unstaged,
            };
            callback(kind, entry.path, focus_opened_item, window, cx);
        }
        window.refresh();
    }

    fn handle_select_prev(&mut self, _: &SelectPrev, window: &mut Window, _: &mut Context<Self>) {
        self.state.borrow_mut().select_up();
        self.scroll_to_selection();
        window.refresh();
    }

    fn handle_select_next(&mut self, _: &SelectNext, window: &mut Window, _: &mut Context<Self>) {
        self.state.borrow_mut().select_down();
        self.scroll_to_selection();
        window.refresh();
    }

    /// 保持键盘选中项可见；行索引直接对应当前渲染列表。
    fn scroll_to_selection(&self) {
        if let Some(index) = self.state.borrow().selected_idx() {
            self.scroll_handle
                .scroll_to_item(index, ScrollStrategy::Center);
        }
    }

    fn handle_collapse(&mut self, _: &Collapse, window: &mut Window, cx: &mut Context<Self>) {
        // 记录被折叠的目录，使重建后保持折叠（其余目录仍默认展开）。
        if let Some(key) = self.selected_directory_key(true) {
            self.collapsed_dirs.insert(key);
        }
        let rebuild = self.state.borrow_mut().collapse_selection();
        if rebuild {
            self.rebuild_rows(cx);
        }
        self.scroll_to_selection();
        window.refresh();
    }

    fn handle_expand(&mut self, _: &Expand, window: &mut Window, cx: &mut Context<Self>) {
        // 展开的目录解除折叠记录。
        if let Some(key) = self.selected_directory_key(false) {
            self.collapsed_dirs.remove(&key);
        }
        let rebuild = self.state.borrow_mut().expand_selection();
        if rebuild {
            self.rebuild_rows(cx);
        }
        self.scroll_to_selection();
        window.refresh();
    }

    /// 选中行的目录键；`expanded` 为 true 时只取展开中的目录（折叠操作），否则只取折叠的目录（展开操作）。
    fn selected_directory_key(&self, expanded: bool) -> Option<(GitSection, PathBuf)> {
        let state = self.state.borrow();
        let idx = state.selected_idx()?;
        match state.rows.get(idx)? {
            GitRow::Entry(entry) if entry.is_dir && entry.expanded == expanded => {
                Some((entry.section, entry.path.clone()))
            }
            _ => None,
        }
    }

    fn handle_activate(&mut self, _: &Activate, window: &mut Window, cx: &mut Context<Self>) {
        self.activate_selected(true, window, cx);
    }

    fn handle_init_repository(
        &mut self,
        _: &InitRepository,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.project.update(cx, |project, cx| {
            project
                .git_store()
                .update(cx, |store, cx| store.git_init(cx));
        });
    }

    /// 切换指定行（分组 + 路径）的暂存状态：未暂存组 → 暂存，已暂存组 → 取消暂存。
    ///
    /// 复选框点击与空格键共用（交互规范：方法复用，不走 dispatch 合流）。
    /// 完成后 GitStore 自动重扫，Statuses 事件驱动行模型重建。
    fn toggle_staged_for(&mut self, section: GitSection, path: &Path, cx: &mut Context<Self>) {
        let store = self.project.read(cx).git_store();
        match section {
            GitSection::Unstaged => store.update(cx, |store, cx| {
                store.stage_paths(vec![path.to_path_buf()], cx);
            }),
            GitSection::Staged => store.update(cx, |store, cx| {
                store.unstage_paths(vec![path.to_path_buf()], cx);
            }),
        }
    }

    /// 空格键：切换选中行的暂存状态。
    fn handle_toggle_staged(
        &mut self,
        _: &ToggleStaged,
        _window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let row = {
            let state = self.state.borrow();
            state.selected_idx().map(|idx| state.rows[idx].clone())
        };
        let Some(GitRow::Entry(entry)) = row else {
            return;
        };
        self.toggle_staged_for(entry.section, &entry.path, cx);
    }

    /// 从 GitStore 读取活动仓库的最近提交 subject 更新显示（订阅事件时调用）。
    fn refresh_last_commit_message(&mut self, cx: &mut Context<Self>) {
        let store = self.project.read(cx).git_store();
        self.last_commit_message = store.read(cx).last_commit_message().map(str::to_string);
    }

    /// 存在已暂存改动时读取编辑器文本提交；空消息时焦点回到编辑器。
    fn handle_commit(&mut self, _: &Commit, window: &mut Window, cx: &mut Context<Self>) {
        let store = self.project.read(cx).git_store();
        if !store.read(cx).has_staged_changes() {
            return;
        }
        let message = self.commit_editor.read(cx).text(cx);
        if message.trim().is_empty() {
            let focus = self.commit_editor.read(cx).focus_handle();
            window.focus(&focus);
            return;
        }
        self.pending_commit = true;
        store.update(cx, |store, cx| store.commit(message, cx));
    }

    /// 撤销最近一次提交（上次提交行右侧按钮）：成功后 Uncommitted 事件把消息填回编辑器。
    fn handle_uncommit(&mut self, _: &Uncommit, _window: &mut Window, cx: &mut Context<Self>) {
        let store = self.project.read(cx).git_store();
        store.update(cx, |store, cx| store.uncommit(cx));
    }
}

impl Render for VersionControlPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        if !self.focus_listeners_initialized {
            let focus = self.focus.clone();
            cx.on_focus(&focus, window, |_, _, cx| cx.notify()).detach();
            cx.on_blur(&focus, window, |_, _, cx| cx.notify()).detach();
            self.focus_listeners_initialized = true;
        }
        self.state.borrow_mut().ensure_selected();
        let has_repositories = self
            .project
            .read(cx)
            .git_store()
            .read(cx)
            .has_repositories();
        let has_staged_changes = self
            .project
            .read(cx)
            .git_store()
            .read(cx)
            .has_staged_changes();
        let changes_tree_focused = self.changes_tree_is_focused(window);
        let content = if has_repositories {
            let rows = self.state.borrow().rows.clone();
            let render_context = GitPanelRenderContext {
                state: Rc::clone(&self.state),
                non_empty_sections: rows
                    .iter()
                    .filter_map(|row| match row {
                        GitRow::Entry(entry) => Some(entry.section),
                        _ => None,
                    })
                    .collect(),
                rows: rows.into(),
                focus: self.focus.clone(),
                collapsed: Rc::clone(&self.collapsed_sections),
                weak: cx.weak_entity(),
            };
            // 列表占满剩余高度；底部提交区存在时收缩。
            div()
                .flex_1()
                .min_h_0()
                .overflow_hidden()
                .child(
                    render_list(
                        &self.scroll_handle,
                        &self.scrollbar,
                        render_context,
                        changes_tree_focused,
                    )
                    .into_any_element(),
                )
                .into_any_element()
        } else {
            render_empty_state(self.focus.clone(), cx).into_any_element()
        };

        // 字段先提出局部变量（闭包借用与 listener 的 cx 互不冲突）。
        let commit_editor = self.commit_editor.clone();
        let last_commit_message = self.last_commit_message.clone();
        let footer = if has_repositories {
            Some(render_commit_footer(
                &commit_editor,
                last_commit_message.as_deref(),
                has_staged_changes,
                cx,
            ))
        } else {
            None
        };

        // 面板顶部：加减号图标 + 总变更行数（有仓库时显示，Diff 图标 + DiffStat）。
        let header = has_repositories.then(|| {
            let total = self.project.read(cx).git_store().read(cx).total_diff_stat();
            render_total_diff_stat(total, cx)
        });

        // 顶部统计行、列表与提交区必须放进同一个 flex_col 容器（列表 flex_1 占满剩余高度）。
        let mut body = div().size_full().flex().flex_col();
        if let Some(header) = header {
            body = body.child(header);
        }
        body = body.child(content);
        if let Some(footer) = footer {
            body = body.child(footer);
        }
        div()
            .size_full()
            .track_focus(&self.focus)
            .key_context(self.dispatch_context(window, cx))
            .tab_index(0)
            .on_action(cx.listener(Self::handle_select_prev))
            .on_action(cx.listener(Self::handle_select_next))
            .on_action(cx.listener(Self::handle_collapse))
            .on_action(cx.listener(Self::handle_expand))
            .on_action(cx.listener(Self::handle_activate))
            .on_action(cx.listener(Self::handle_init_repository))
            .on_action(cx.listener(Self::handle_toggle_staged))
            .on_action(cx.listener(Self::handle_commit))
            .on_action(cx.listener(Self::handle_uncommit))
            .child(body)
    }
}

// ═══ 私有渲染辅助函数 ═══════════════════════════════════════════

/// 面板顶部统计行：加减号图标 + 总新增/删除行数（全零时只留图标）。
fn render_total_diff_stat(total: DiffStat, cx: &App) -> Div {
    let colors = color::current(cx);
    div()
        .flex_none()
        .h(typography::ui_line())
        .pl(space::S6)
        .pr(space::S6)
        .flex()
        .items_center()
        .gap(space::S2)
        .text_color(colors.text_muted)
        .child(
            SvgIcon::new("icons/diff.svg")
                .id(ElementId::Name("version-control-total-diff".into()))
                .label("变更行数统计")
                .color(colors.icon_muted),
        )
        .when(total.added > 0 || total.deleted > 0, |el| {
            el.child(
                div()
                    .text_color(colors.version_control_added)
                    .child(format!("+{}", total.added)),
            )
            .child(
                div()
                    .text_color(colors.version_control_deleted)
                    .child(format!("−{}", total.deleted)),
            )
        })
}

fn render_list(
    scroll_handle: &UniformListScrollHandle,
    scrollbar: &Scrollbar<UniformListScrollHandle>,
    render_context: GitPanelRenderContext,
    changes_tree_focused: bool,
) -> gpui::UniformList {
    let handle = scroll_handle.clone();
    let len = render_context.rows.len();
    uniform_list("version-control-list", len, move |range, _, cx| {
        let state = render_context.state.borrow();
        let rows = &render_context.rows;
        let selected = state.selected.clone();
        range
            .filter_map(|i| rows.get(i))
            .map(|row| {
                let sel = row_entry_key(row) == selected;
                render_row(row, sel, changes_tree_focused, &render_context, cx).into_any_element()
            })
            .collect()
    })
    .size_full()
    .track_scroll(handle)
    .with_decoration(scrollbar.clone())
}

fn render_row(
    row: &GitRow,
    sel: bool,
    changes_tree_focused: bool,
    render_context: &GitPanelRenderContext,
    cx: &mut App,
) -> Div {
    match row {
        // 分组头：不可选择；
        // 行首 chevron 折叠/展开分区，行尾复选框全选/取消全选（空分区不显示复选框）。
        GitRow::Header(section) => {
            let is_collapsed = render_context.collapsed.borrow().contains(section);
            let weak = render_context.weak.clone();
            let section = *section;
            let checkbox_weak = weak.clone();
            let section_has_entries = render_context.non_empty_sections.contains(&section);
            div()
                .w_full()
                .h(typography::ui_line())
                .pl(space::S6)
                .pr(space::S6)
                .flex()
                .items_center()
                .justify_between()
                .text_color(color::current(cx).text_muted)
                .cursor_pointer()
                .hover(|style| style.bg(color::current(cx).element_hover))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap(space::S6)
                        .child(
                            SvgIcon::new(if is_collapsed {
                                "icons/chevron_right.svg"
                            } else {
                                "icons/chevron_down.svg"
                            })
                            .id(ElementId::Name(
                                format!("version-control-section-{section:?}").into(),
                            ))
                            .label("折叠或展开分区")
                            .color(color::current(cx).icon_muted),
                        )
                        .child(section.label()),
                )
                .when(section_has_entries, |el| {
                    // 已暂存组显示勾选（点击 = 全部取消暂存）、未暂存组显示未勾选（点击 = 全部暂存）。
                    el.child(
                        Checkbox::new(
                            ElementId::Name(
                                format!("version-control-header-checkbox-{section:?}").into(),
                            ),
                            section == GitSection::Staged,
                        )
                        .tooltip(if section == GitSection::Staged {
                            "全部取消暂存"
                        } else {
                            "全部暂存"
                        })
                        .on_click(move |_window, cx| {
                            if let Some(panel) = checkbox_weak.upgrade() {
                                panel.update(cx, |panel, cx| {
                                    panel.toggle_section_all(section, cx);
                                });
                            }
                        }),
                    )
                })
                // 整行点击折叠/展开。
                .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                    if let Some(panel) = weak.upgrade() {
                        panel.update(cx, |panel, cx| {
                            panel.toggle_section_collapsed(section, cx);
                        });
                    }
                })
        }
        GitRow::Entry(entry) => {
            let section = entry.section;
            let path = entry.path.clone();
            let is_dir = entry.is_dir;
            let name = entry.name.clone();
            let status_color = entry.status.and_then(|status| git_status_color(status, cx));
            // 删除线只作用于文件行。
            let is_deleted = !is_dir && entry.status.is_some_and(|status| status.is_deleted());
            // 文件名按 git 状态着色（删除文件加删除线）。
            let content = div()
                .when_some(status_color, |label, label_color| {
                    label.text_color(label_color)
                })
                .when(is_deleted, |label| label.line_through())
                .child(name);
            let diff_stat = entry.diff_stat;
            // 行尾改动计数（目录行为子项求和；全零不显示，如 untracked 文件）。
            // 加减分别用 git 状态色：+ 新增色、− 删除色。
            let tail = if diff_stat.added > 0 || diff_stat.deleted > 0 {
                let colors = color::current(cx);
                div()
                    .flex_shrink_0()
                    .pl(space::S6)
                    .flex()
                    .items_center()
                    .gap(space::S2)
                    .child(
                        div()
                            .text_color(colors.version_control_added)
                            .child(format!("+{}", diff_stat.added)),
                    )
                    .child(
                        div()
                            .text_color(colors.version_control_deleted)
                            .child(format!("−{}", diff_stat.deleted)),
                    )
            } else {
                div().flex_shrink_0()
            };
            // 行尾暂存复选框（在改动计数之后）；行尾 6px 边距是面板布局职责，由消费方包一层。
            let checkbox = div()
                .mr(space::S6)
                .child(
                    Checkbox::new(
                        ElementId::Name(
                            // id 带分组：部分暂存文件同时在两组出现时，两个复选框共享元素 state 会互相干扰。
                            format!("version-control-checkbox-{:?}-{}", section, path.display())
                                .into(),
                        ),
                        section == GitSection::Staged,
                    )
                    .tooltip(if section == GitSection::Staged {
                        "取消暂存"
                    } else {
                        "暂存"
                    })
                    .shortcut(&ToggleStaged, cx)
                    .on_click({
                        let weak = render_context.weak.clone();
                        let path = path.clone();
                        move |_window, cx| {
                            if let Some(panel) = weak.upgrade() {
                                panel.update(cx, |panel, cx| {
                                    panel.toggle_staged_for(section, &path, cx);
                                });
                            }
                        }
                    })
                    .into_any_element(),
                )
                .into_any_element();
            tree::render_row_base(
                entry.depth,
                &entry.path,
                is_dir,
                entry.expanded,
                content,
                cx,
            )
            .cursor_pointer()
            .child(tail)
            .child(checkbox)
            .hover(|style| style.bg(color::current(cx).element_hover))
            .when(sel && changes_tree_focused, |el| {
                el.child(
                    tree::selection_border(cx)
                        .debug_selector(|| "version-control-selection-border".into()),
                )
            })
            .on_mouse_down(MouseButton::Left, {
                let focus = render_context.focus.clone();
                let weak = render_context.weak.clone();
                move |event, window, cx| {
                    let was_focused = focus.contains_focused(window, cx);
                    window.focus(&focus);
                    if let Some(panel) = weak.upgrade() {
                        panel.update(cx, |panel, cx| {
                            panel.state.borrow_mut().selected = Some((section, path.clone()));
                            match tree::row_mouse_down_action(
                                is_dir,
                                event.click_count,
                                was_focused,
                            ) {
                                Some(tree::RowClickAction::Toggle) => {
                                    panel.activate_selected(true, window, cx)
                                }
                                Some(tree::RowClickAction::Preview) => {
                                    panel.activate_selected(false, window, cx)
                                }
                                Some(tree::RowClickAction::Activate) => {
                                    panel.activate_selected(true, window, cx)
                                }
                                None => {}
                            }
                        });
                    }
                    cx.stop_propagation();
                }
            })
        }
    }
}

/// 无仓库空态：居中文案 + 初始化仓库按钮。
///
/// 按钮是命令载体，走 dispatch_action 合流；
/// 先收焦点保证 action 沿焦点链命中面板自身的 `InitRepository` handler。
/// 底部提交区：上次提交信息（含撤销按钮）→ 提交信息编辑器 → 提交按钮（仅在有仓库时渲染）。
fn render_commit_footer(
    editor: &Entity<Editor>,
    last_commit_message: Option<&str>,
    has_staged_changes: bool,
    cx: &App,
) -> Div {
    let colors = color::current(cx);
    let message = editor.read(cx).text(cx);
    // is_some 提前提取：when 闭包是 'static，不能捕获 &str 借用。
    let has_last_commit = last_commit_message.is_some();
    // child 接受 'static 内容，&str 借用先转为 owned。
    let last_commit_text = last_commit_message.unwrap_or("暂无提交").to_string();
    div()
        .border_t_1()
        .border_color(colors.border)
        .flex()
        .flex_col()
        // 提交信息编辑器（observe 已让按键即时触发重绘）。
        .child(
            div()
                .bg(colors.editor_background)
                .flex()
                .flex_col()
                .child(div().pt(space::S8).px(space::S8).child(editor.clone()))
                // 容器内底部 commit-footer：提交按钮（空消息时淡显，点击由 handler 兜底聚焦回编辑器）。
                .child(
                    div()
                        .id("version-control-commit-footer")
                        .p(space::S6)
                        .flex()
                        .justify_end()
                        .child(
                            Button::text("version-control-commit", "提交")
                                .size(ButtonSize::Loose)
                                .style(ButtonStyle::Solid)
                                .disabled(!has_staged_changes)
                                .label("提交当前暂存")
                                .shortcut(&Commit, cx)
                                .color(if message.trim().is_empty() {
                                    colors.text_muted
                                } else {
                                    colors.text
                                })
                                .on_click(move |_, window, cx| {
                                    window.dispatch_action(Box::new(Commit), cx);
                                }),
                        ),
                ),
        )
        // 上次提交信息。
        .child(
            div()
                .border_t_1()
                .border_color(colors.border_variant)
                .px(space::S8)
                .py(space::S6)
                .flex()
                .items_center()
                .gap(space::S6)
                .child(
                    div()
                        .flex_1()
                        .overflow_hidden()
                        .truncate()
                        .text_color(if has_last_commit {
                            colors.text
                        } else {
                            colors.text_muted
                        })
                        .child(last_commit_text),
                )
                // 撤销按钮：仅在有提交时显示（无提交时 uncommit 无意义）。
                // hover 提示"撤销提交"。
                .when(has_last_commit, |element| {
                    element.child(
                        Button::icon("version-control-uncommit", "icons/undo.svg")
                            .label("撤销提交")
                            .on_click(move |_, window, cx| {
                                window.dispatch_action(Box::new(Uncommit), cx);
                            }),
                    )
                }),
        )
}

fn render_empty_state(focus: FocusHandle, cx: &App) -> Div {
    let colors = color::current(cx);
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(space::S8)
        .text_color(colors.text_placeholder)
        .child("没有 Git 仓库")
        .child(
            div()
                .id("version-control-init")
                .px(space::S12)
                .py(space::S6)
                .rounded_md()
                .border_1()
                .border_color(colors.border_variant)
                .bg(colors.panel_background)
                .text_color(colors.text)
                .cursor_pointer()
                .hover(|style| style.bg(colors.element_hover))
                .child("初始化仓库")
                .on_click(move |_, window, cx| {
                    window.focus(&focus);
                    window.dispatch_action(Box::new(InitRepository), cx);
                }),
        )
}

impl EventEmitter<PanelEvent> for VersionControlPanel {}

impl Panel for VersionControlPanel {
    fn icon() -> &'static str {
        "icons/git_branch.svg"
    }
    fn label() -> &'static str {
        "版本控制"
    }
    fn persistent_name() -> &'static str {
        "version-control"
    }
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

// ═══ 内部类型 ════════════════════════════════════════════════════

/// 分组（冲突暂不展示）。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum GitSection {
    Staged,
    Unstaged,
}

impl GitSection {
    const ALL: [Self; 2] = [Self::Staged, Self::Unstaged];

    fn label(self) -> &'static str {
        match self {
            Self::Staged => "已暂存",
            Self::Unstaged => "未暂存",
        }
    }
}

/// 统一行模型：分组头不可选择、不可折叠。
#[derive(Clone, Debug)]
enum GitRow {
    Header(GitSection),
    Entry(GitTreeRow),
}

/// 变更树行。
#[derive(Clone, Debug)]
struct GitTreeRow {
    /// 所在分组（选中/展开键的一部分；(section, path) 在可见行内唯一）。
    section: GitSection,
    /// 绝对路径（打开回调用）。
    path: PathBuf,
    /// 显示名（仅末段，缩进由 depth 承担）。
    name: String,
    depth: usize,
    is_dir: bool,
    expanded: bool,
    /// 文件 = 自身状态；目录 = 该分组子集内后代的聚合。
    status: Option<FileStatus>,
    /// 该分组视角的 diff 统计（目录行 = 子项求和）。
    diff_stat: DiffStat,
}

/// 分组树的节点（含合成目录）。
#[derive(Debug)]
struct GitTreeNode {
    path: PathBuf,
    name: String,
    is_dir: bool,
    status: Option<FileStatus>,
    diff_stat: DiffStat,
    children: Vec<GitTreeNode>,
}

impl TreeRow for GitRow {
    fn is_dir(&self) -> bool {
        matches!(self, GitRow::Entry(entry) if entry.is_dir)
    }
    fn depth(&self) -> usize {
        match self {
            GitRow::Entry(entry) => entry.depth,
            GitRow::Header(_) => 0,
        }
    }
    fn expanded(&self) -> bool {
        matches!(self, GitRow::Entry(entry) if entry.expanded)
    }
}

/// 行 → 选中/展开键（Header 行为 None）。
fn row_entry_key(row: &GitRow) -> Option<(GitSection, PathBuf)> {
    match row {
        GitRow::Entry(entry) => Some((entry.section, entry.path.clone())),
        GitRow::Header(_) => None,
    }
}

#[derive(Clone)]
struct GitPanelRenderContext {
    state: Rc<RefCell<TreeState<(GitSection, PathBuf), GitRow>>>,
    rows: Rc<[GitRow]>,
    focus: FocusHandle,
    /// 折叠的分区（标题行 chevron 渲染与点击共享）。
    collapsed: Rc<RefCell<HashSet<GitSection>>>,
    /// 有条目的分区（空分区标题行不显示全选复选框）。
    non_empty_sections: HashSet<GitSection>,
    /// 条目点击直接调用 Entity 方法。
    weak: WeakEntity<VersionControlPanel>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    use gpui::{KeyBinding, TestAppContext, VisualTestContext, point, px};
    use tempfile::TempDir;

    use zcv_project::{Project, StatusEntry};

    /// 构造快照：路径 → 状态；diff 统计取固定样例值（staged/unstaged 可区分）。
    fn snapshot(entries: &[(&str, FileStatus)]) -> RepositorySnapshot {
        RepositorySnapshot {
            branch: None,
            head: None,
            last_commit_message: None,
            has_remote: false,
            ahead: 0,
            behind: 0,
            branch_list: Vec::new(),
            statuses_by_path: entries
                .iter()
                .map(|(path, status)| {
                    (
                        PathBuf::from(path),
                        StatusEntry {
                            status: *status,
                            diff_stat: DiffStat {
                                added: 2,
                                deleted: 1,
                            },
                            staged_diff_stat: DiffStat {
                                added: 1,
                                deleted: 0,
                            },
                            unstaged_diff_stat: DiffStat {
                                added: 0,
                                deleted: 1,
                            },
                        },
                    )
                })
                .collect(),
        }
    }

    fn build_rows(root: &Path, repos: &[(&Path, &RepositorySnapshot)]) -> Vec<GitRow> {
        let trees = build_section_trees(root, repos.iter().copied());
        flatten_rows(&trees, &HashSet::new(), &HashSet::new())
    }

    /// 行列表 → (分组, 显示名) 序列。
    fn entry_keys(rows: &[GitRow]) -> Vec<(GitSection, String)> {
        rows.iter()
            .filter_map(|row| match row {
                GitRow::Entry(entry) => Some((entry.section, entry.name.clone())),
                GitRow::Header(_) => None,
            })
            .collect()
    }

    #[test]
    fn entry_sections_assigns_each_status_to_sections() {
        // 冲突文件暂不展示，不进任何组。
        assert_eq!(entry_sections(FileStatus::Unmerged), (false, false));
        assert_eq!(entry_sections(FileStatus::Untracked), (false, true));
        assert_eq!(entry_sections(FileStatus::Ignored), (false, false));
        let staged = FileStatus::Tracked {
            index_status: StatusCode::Modified,
            worktree_status: StatusCode::Unmodified,
        };
        assert_eq!(entry_sections(staged), (true, false));
        let unstaged = FileStatus::Tracked {
            index_status: StatusCode::Unmodified,
            worktree_status: StatusCode::Modified,
        };
        assert_eq!(entry_sections(unstaged), (false, true));
        // 部分暂存：两组同时出现。
        let partial = FileStatus::Tracked {
            index_status: StatusCode::Added,
            worktree_status: StatusCode::Deleted,
        };
        assert_eq!(entry_sections(partial), (true, true));
    }

    #[test]
    fn headers_always_appear_and_partially_staged_entries_duplicate_across_sections() {
        let root = PathBuf::from("/project");
        let partial = FileStatus::Tracked {
            index_status: StatusCode::Modified,
            worktree_status: StatusCode::Modified,
        };
        let snapshot = snapshot(&[("src/a.rs", partial)]);
        let rows = build_rows(&root, &[(root.as_path(), &snapshot)]);

        let headers: Vec<_> = rows
            .iter()
            .filter_map(|row| match row {
                GitRow::Header(section) => Some(section.label()),
                GitRow::Entry(_) => None,
            })
            .collect();
        assert_eq!(headers, vec!["已暂存", "未暂存"]);

        // 条目位于 src/ 下，折叠时两组各出现一个 src 目录行。
        let entries = entry_keys(&rows);
        assert_eq!(
            entries,
            vec![
                (GitSection::Staged, "src".into()),
                (GitSection::Unstaged, "src".into())
            ]
        );
        // 两组各带对应视角的 diff 统计（目录聚合 = 子项求和）。
        let staged = rows.iter().find_map(|row| match row {
            GitRow::Entry(e) if e.section == GitSection::Staged => Some(e),
            _ => None,
        });
        let unstaged = rows.iter().find_map(|row| match row {
            GitRow::Entry(e) if e.section == GitSection::Unstaged => Some(e),
            _ => None,
        });
        assert_eq!(
            staged.unwrap().diff_stat,
            DiffStat {
                added: 1,
                deleted: 0
            }
        );
        assert_eq!(
            unstaged.unwrap().diff_stat,
            DiffStat {
                added: 0,
                deleted: 1
            }
        );
    }

    #[test]
    fn statuses_are_filtered_into_their_sections() {
        let root = PathBuf::from("/project");
        let snapshot = snapshot(&[
            ("conflict.txt", FileStatus::Unmerged),
            ("new.txt", FileStatus::Untracked),
            ("ignored.log", FileStatus::Ignored),
        ]);
        let rows = build_rows(&root, &[(root.as_path(), &snapshot)]);

        // Ignored 与冲突文件（暂不展示）完全过滤；untracked 归未暂存组。
        let entries = entry_keys(&rows);
        assert_eq!(entries, vec![(GitSection::Unstaged, "new.txt".into())]);
    }

    #[test]
    fn directories_aggregate_status_and_diff_and_respect_expansion() {
        let root = PathBuf::from("/project");
        let modified = FileStatus::Tracked {
            index_status: StatusCode::Unmodified,
            worktree_status: StatusCode::Modified,
        };
        let snapshot = snapshot(&[
            ("src/a.rs", FileStatus::Untracked),
            ("src/sub/b.rs", modified),
        ]);
        let trees = build_section_trees(&root, [(root.as_path(), &snapshot)].into_iter());

        // 折叠：只显示顶层目录 src。
        let rows = flatten_rows(&trees, &HashSet::new(), &HashSet::new());
        assert_eq!(
            entry_keys(&rows),
            vec![(GitSection::Unstaged, "src".into())]
        );
        let src = rows.iter().find_map(|row| match row {
            GitRow::Entry(e) if e.name == "src" => Some(e),
            _ => None,
        });
        let src = src.expect("应有 src 目录行");
        assert!(src.is_dir);
        assert!(!src.expanded);
        // 聚合：modified 的 priority 高于 untracked；
        // diff 取该分组视角的 unstaged 统计求和（每条 0 增 1 删，共 2 条）。
        assert_eq!(src.status, Some(modified));
        assert_eq!(
            src.diff_stat,
            DiffStat {
                added: 0,
                deleted: 2
            }
        );

        // 展开 src：sub（目录优先）与 a.rs 都出现，sub 未展开时其子项不可见。
        let mut expanded = HashSet::new();
        expanded.insert((GitSection::Unstaged, root.join("src")));
        let rows = flatten_rows(&trees, &expanded, &HashSet::new());
        assert_eq!(
            entry_keys(&rows),
            vec![
                (GitSection::Unstaged, "src".into()),
                (GitSection::Unstaged, "sub".into()),
                (GitSection::Unstaged, "a.rs".into())
            ]
        );

        // 再展开 sub：叶子出现，目录优先排序（sub 子树在 a.rs 之前）。
        expanded.insert((GitSection::Unstaged, root.join("src").join("sub")));
        let rows = flatten_rows(&trees, &expanded, &HashSet::new());
        assert_eq!(
            entry_keys(&rows),
            vec![
                (GitSection::Unstaged, "src".into()),
                (GitSection::Unstaged, "sub".into()),
                (GitSection::Unstaged, "b.rs".into()),
                (GitSection::Unstaged, "a.rs".into())
            ]
        );
    }

    #[test]
    fn nested_repositories_merge_into_one_tree() {
        let root = PathBuf::from("/project");
        let vendor = root.join("vendor");
        let outer = snapshot(&[("README.md", FileStatus::Untracked)]);
        let inner = snapshot(&[("lib.rs", FileStatus::Untracked)]);
        let repos = [(root.as_path(), &outer), (vendor.as_path(), &inner)];
        let trees = build_section_trees(&root, repos.into_iter());

        // vendor 为目录行，优先于根文件 README.md；展开后 lib.rs 归入其下。
        let mut expanded = HashSet::new();
        expanded.insert((GitSection::Unstaged, root.join("vendor")));
        let rows = flatten_rows(&trees, &expanded, &HashSet::new());
        let paths: Vec<_> = rows
            .iter()
            .filter_map(|row| match row {
                GitRow::Entry(entry) => Some((entry.path.clone(), entry.depth)),
                GitRow::Header(_) => None,
            })
            .collect();
        assert_eq!(
            paths,
            vec![
                (root.join("vendor"), 0),
                (root.join("vendor/lib.rs"), 1),
                (root.join("README.md"), 0)
            ]
        );
    }

    /// 创建带一个修改文件的临时 git 仓库。
    fn test_repo() -> (PathBuf, TempDir) {
        let temp_dir = tempfile::tempdir().expect("应创建临时目录");
        let root = temp_dir.path().to_path_buf();
        run_in(&root, &["git", "init", "-q", "-b", "master"]);
        run_in(&root, &["git", "config", "user.email", "test@example.com"]);
        run_in(&root, &["git", "config", "user.name", "Test User"]);
        std::fs::write(root.join("tracked.txt"), "第一行\n第二行\n").expect("应写入初始文件");
        run_in(&root, &["git", "add", "tracked.txt"]);
        run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
        std::fs::write(root.join("tracked.txt"), "修改后的内容\n").expect("应修改文件");
        (root, temp_dir)
    }

    fn run_in(dir: &Path, args: &[&str]) {
        let output = std::process::Command::new(args[0])
            .args(&args[1..])
            .current_dir(dir)
            .output()
            .expect("应执行成功");
        assert!(
            output.status.success(),
            "命令 {:?} 失败：{}",
            args,
            String::from_utf8_lossy(&output.stderr)
        );
    }

    #[gpui::test]
    fn empty_state_initializes_repository_and_builds_section_tree(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let project_root = directory.path().to_path_buf();
        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let project_for_panel = project.clone();
        let (panel, cx) =
            cx.add_window_view(move |_, cx| VersionControlPanel::new(project_for_panel, cx));
        cx.run_until_parked(); // 首次扫描完成：无仓库，行模型为空。

        // 初始化仓库（等价于空态按钮 dispatch 后的 handler 行为）。
        project.update(cx, |project, cx| {
            project
                .git_store()
                .update(cx, |store, cx| store.git_init(cx));
        });
        cx.run_until_parked(); // init job 完成
        cx.run_until_parked(); // 其触发的重扫落地 → Repositories 事件 → 重建行模型

        cx.read_entity(&panel, |panel, _| {
            let rows = panel.state.borrow().rows.clone();
            let headers: Vec<_> = rows
                .iter()
                .filter_map(|row| match row {
                    GitRow::Header(section) => Some(section.label()),
                    GitRow::Entry(_) => None,
                })
                .collect();
            assert_eq!(headers, vec!["已暂存", "未暂存"]);
        });
    }

    #[gpui::test]
    fn first_click_focuses_unfocused_panel_and_second_click_opens(cx: &mut TestAppContext) {
        let (root, _temp) = test_repo();
        let project_root = root.clone();
        let open_count = Rc::new(Cell::new(0));
        let last_focus_opened = Rc::new(Cell::new(true));
        let callback_count = Rc::clone(&open_count);
        let callback_focus = Rc::clone(&last_focus_opened);

        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (panel, cx) = cx.add_window_view(move |_, cx| {
            let mut panel = VersionControlPanel::new(project, cx);
            panel.set_on_open_file(Rc::new(move |_, _, focus_opened_item, _, _| {
                callback_count.set(callback_count.get() + 1);
                callback_focus.set(focus_opened_item);
            }));
            panel
        });
        cx.run_until_parked(); // 扫描完成，行模型就绪。

        // 行布局：三个分组头 + 未暂存组一个文件行。
        let row_count = cx.read_entity(&panel, |panel, _| panel.state.borrow().rows.len());
        assert_eq!(row_count, 3);

        // 扫描完成后强制重绘：首帧是空态，点击命中测试需要最新帧的行布局。
        // refresh 只入队 effect，需要一次 update 周期 flush 后窗口才真正重绘。
        let _ = cx.refresh();
        cx.update(|_, _| {});
        cx.run_until_parked();

        // 单击第 4 行（tracked.txt）行内容区（x=100 避开行首复选框）：
        // 行高为 ui_line()，以临时标签打开（focus_opened_item=false）。
        let row_height = typography::ui_line();
        let click = |cx: &mut VisualTestContext| {
            // y 加 1 行偏移：顶部统计行占一行高度。
            cx.simulate_click(
                point(px(100.), px(f32::from(row_height) * 3.5)),
                gpui::Modifiers::default(),
            );
            cx.run_until_parked();
        };

        // 首击：面板未聚焦，只聚焦并选中，不打开文件。
        click(cx);
        assert_eq!(open_count.get(), 0, "未聚焦首击只聚焦，不应打开文件");
        let panel_focused =
            cx.update(|window, cx| panel.read(cx).focus.contains_focused(window, cx));
        assert!(panel_focused, "首击应聚焦变更面板");

        // 二击：已聚焦，单击预览打开（焦点留在面板）。
        click(cx);
        assert_eq!(open_count.get(), 1, "已聚焦后单击应打开文件");
        assert!(
            !last_focus_opened.get(),
            "单击应打开临时标签但焦点留在面板（focus_opened_item=false）"
        );
    }

    #[gpui::test]
    fn keyboard_navigation_opens_file_as_active(cx: &mut TestAppContext) {
        let (root, _temp) = test_repo();
        let project_root = root.clone();
        let open_count = Rc::new(Cell::new(0));
        let last_focus_opened = Rc::new(Cell::new(false));
        let callback_count = Rc::clone(&open_count);
        let callback_focus = Rc::clone(&last_focus_opened);

        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (panel, cx) = cx.add_window_view(move |_, cx| {
            cx.bind_keys([
                KeyBinding::new("down", SelectNext, Some("VersionControlChangesTree")),
                KeyBinding::new("enter", Activate, Some("VersionControlChangesTree")),
            ]);
            let mut panel = VersionControlPanel::new(project, cx);
            panel.set_on_open_file(Rc::new(move |_, _, focus_opened_item, _, _| {
                callback_count.set(callback_count.get() + 1);
                callback_focus.set(focus_opened_item);
            }));
            panel
        });
        cx.update(|window, cx| window.focus(&panel.read(cx).focus));
        cx.run_until_parked();

        // down 选中第一个条目行（跳过分组头），enter 激活打开。
        cx.simulate_keystrokes("down");
        cx.simulate_keystrokes("enter");

        assert_eq!(open_count.get(), 1, "enter 应打开选中的文件");
        assert!(
            last_focus_opened.get(),
            "enter 打开应为激活（focus_opened_item=true）"
        );
    }

    #[gpui::test]
    fn staged_section_opens_the_staged_project_diff(cx: &mut TestAppContext) {
        let (root, _temp) = test_repo();
        run_in(&root, &["git", "add", "tracked.txt"]);
        let opened_kind = Rc::new(Cell::new(None));
        let callback_kind = Rc::clone(&opened_kind);
        let project_root = root.clone();
        let project = cx.new(|cx| Project::new(project_root, cx));
        let (panel, cx) = cx.add_window_view(move |_, cx| {
            let mut panel = VersionControlPanel::new(project, cx);
            panel.set_on_open_file(Rc::new(move |kind, _, _, _, _| {
                callback_kind.set(Some(kind));
            }));
            panel
        });
        cx.run_until_parked();

        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel.state.borrow_mut().select_down();
                panel.activate_selected(true, window, cx);
            });
        });

        assert_eq!(opened_kind.get(), Some(ProjectDiffKind::Staged));
    }

    /// 面板行模型中各条目的 (分组, 显示名) 序列。
    fn section_entries(panel: &VersionControlPanel) -> Vec<(GitSection, String)> {
        panel
            .state
            .borrow()
            .rows
            .iter()
            .filter_map(|row| match row {
                GitRow::Entry(entry) => Some((entry.section, entry.name.clone())),
                GitRow::Header(_) => None,
            })
            .collect()
    }

    #[gpui::test]
    fn directories_expand_by_default(cx: &mut TestAppContext) {
        // 目录下有变更文件：首次构建后目录默认展开，子文件可见。
        let (root, _temp) = test_repo();
        std::fs::create_dir_all(root.join("src")).expect("应创建目录");
        std::fs::write(root.join("src/a.txt"), "改动\n").expect("应写入文件");

        let project_root = root.clone();
        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
        cx.run_until_parked(); // 扫描 + 重建 + 首次全展开

        let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
        assert_eq!(
            entries,
            vec![
                (GitSection::Unstaged, "src".into()),
                (GitSection::Unstaged, "a.txt".into()),
                (GitSection::Unstaged, "tracked.txt".into())
            ],
            "首次构建后目录应默认展开"
        );
    }

    #[gpui::test]
    fn new_directories_expand_by_default_while_manually_collapsed_stay(cx: &mut TestAppContext) {
        let (root, _temp) = test_repo();
        std::fs::create_dir_all(root.join("src")).expect("应创建目录");
        std::fs::write(root.join("src/a.txt"), "改动\n").expect("应写入文件");

        let project_root = root.clone();
        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
        cx.run_until_parked(); // 扫描 + 重建

        // 首次：src 默认展开，子文件可见。
        let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
        assert!(entries.contains(&(GitSection::Unstaged, "a.txt".into())));

        // 用户折叠 src（模拟点击目录行折叠）；树键用 canonicalize 后的根（macOS /var → /private/var）。
        let canonical_root = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
        cx.update_entity(&panel, |panel, cx| {
            let key = (GitSection::Unstaged, canonical_root.join("src"));
            panel.collapsed_dirs.insert(key.clone());
            panel.state.borrow_mut().expanded.remove(&key);
            panel.rebuild_rows(cx);
        });
        let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
        assert!(
            !entries.contains(&(GitSection::Unstaged, "a.txt".into())),
            "用户折叠的目录应保持折叠"
        );

        // 新目录 src2 出现：增量刷新 GitStore（fs watch 是真实线程，测试里走公开刷新入口）→ Statuses 事件 → 面板重建。
        std::fs::create_dir_all(root.join("src2")).expect("应创建目录");
        std::fs::write(root.join("src2/b.txt"), "改动\n").expect("应写入文件");
        let git_store = cx.read_entity(&panel, |panel, cx| {
            panel.project.read(cx).git_store().clone()
        });
        cx.update_entity(&git_store, |store, cx| {
            store.refresh_statuses_for_paths(&[canonical_root.join("src2")], cx);
        });
        cx.run_until_parked(); // 增量扫描落地
        cx.run_until_parked(); // Statuses 事件触发的面板重建落地
        let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
        assert!(entries.contains(&(GitSection::Unstaged, "src2".into())));
        assert!(
            entries.contains(&(GitSection::Unstaged, "b.txt".into())),
            "新出现的目录应默认展开"
        );
        assert!(
            !entries.contains(&(GitSection::Unstaged, "a.txt".into())),
            "用户折叠的 src 在重建后仍保持折叠"
        );
    }

    #[gpui::test]
    fn section_header_collapse_hides_entries_and_expand_restores(cx: &mut TestAppContext) {
        let (root, _temp) = test_repo();
        std::fs::write(root.join("tracked.txt"), "改动\n").expect("应写入文件");

        let project_root = root.clone();
        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
        cx.run_until_parked(); // 扫描 + 重建

        // 初始：未暂存组条目可见。
        let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
        assert!(entries.contains(&(GitSection::Unstaged, "tracked.txt".into())));

        // 折叠未暂存分区：条目不渲染，标题行保留。
        cx.update_entity(&panel, |panel, cx| {
            panel.toggle_section_collapsed(GitSection::Unstaged, cx);
        });
        let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
        assert!(entries.is_empty(), "折叠后该分区条目应隐藏（仅剩标题行）");

        // 再展开：条目恢复。
        cx.update_entity(&panel, |panel, cx| {
            panel.toggle_section_collapsed(GitSection::Unstaged, cx);
        });
        let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
        assert!(entries.contains(&(GitSection::Unstaged, "tracked.txt".into())));
    }

    #[gpui::test]
    fn header_checkbox_selects_all_entries_in_section(cx: &mut TestAppContext) {
        // 两个未暂存文件：header 全选 → 全部暂存，未暂存组清空、已暂存组出现两项。
        let (root, _temp) = test_repo();
        std::fs::write(root.join("tracked.txt"), "改动\n").expect("应写入文件");
        std::fs::write(root.join("second.txt"), "第二个文件\n").expect("应写入文件");

        let project_root = root.clone();
        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
        cx.run_until_parked();

        let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
        assert_eq!(
            entries
                .iter()
                .filter(|(s, _)| *s == GitSection::Unstaged)
                .count(),
            2,
            "两个文件都在未暂存组"
        );

        cx.update_entity(&panel, |panel, cx| {
            panel.toggle_section_all(GitSection::Unstaged, cx);
        });
        cx.run_until_parked(); // stage job 完成
        cx.run_until_parked(); // 其触发的重扫落地
        let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
        assert_eq!(
            entries
                .iter()
                .filter(|(s, _)| *s == GitSection::Staged)
                .count(),
            2,
            "全选后两个文件都应进入已暂存组"
        );
        assert!(
            entries.iter().all(|(s, _)| *s == GitSection::Staged),
            "未暂存组应清空"
        );

        // 已暂存组 header 全选：全部取消暂存，回到未暂存组。
        cx.update_entity(&panel, |panel, cx| {
            panel.toggle_section_all(GitSection::Staged, cx);
        });
        cx.run_until_parked();
        cx.run_until_parked();
        let entries = cx.read_entity(&panel, |panel, _| section_entries(panel));
        assert_eq!(
            entries
                .iter()
                .filter(|(s, _)| *s == GitSection::Unstaged)
                .count(),
            2,
            "取消全选后两个文件都应回到未暂存组"
        );
    }

    #[gpui::test]
    fn space_toggles_staging_and_moves_row_between_sections(cx: &mut TestAppContext) {
        let (root, _temp) = test_repo();
        let project_root = root.clone();
        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (panel, cx) = cx.add_window_view(move |_, cx| {
            cx.bind_keys([
                KeyBinding::new("down", SelectNext, Some("VersionControlChangesTree")),
                KeyBinding::new("space", ToggleStaged, Some("VersionControlChangesTree")),
            ]);
            VersionControlPanel::new(project, cx)
        });
        cx.update(|window, cx| window.focus(&panel.read(cx).focus));
        cx.run_until_parked();

        // down 选中未暂存组的 tracked.txt → space 暂存 → 行移到已暂存组。
        cx.simulate_keystrokes("down");
        cx.simulate_keystrokes("space");
        cx.run_until_parked(); // stage job 完成
        cx.run_until_parked(); // 其触发的重扫落地
        let sections = cx.read_entity(&panel, |panel, _| section_entries(panel));
        assert_eq!(sections, vec![(GitSection::Staged, "tracked.txt".into())]);

        // 再 space：取消暂存 → 回到未暂存组。
        // 重扫后选中被清空，先触发一次重绘让 ensure_selected 落到新行上。
        let _ = cx.refresh();
        cx.update(|_, _| {});
        cx.run_until_parked();
        cx.simulate_keystrokes("space");
        cx.run_until_parked();
        cx.run_until_parked();
        let sections = cx.read_entity(&panel, |panel, _| section_entries(panel));
        assert_eq!(sections, vec![(GitSection::Unstaged, "tracked.txt".into())]);
    }

    #[gpui::test]
    fn space_in_commit_editor_inserts_text_without_toggling_staging(cx: &mut TestAppContext) {
        let (root, _temp) = test_repo();
        let project_root = root.clone();
        let project = cx.new(|cx| Project::new(project_root, cx));
        let (panel, cx) = cx.add_window_view(move |_, cx| {
            zcv_keymap::init(cx).expect("应注册内置快捷键");
            VersionControlPanel::new(project, cx)
        });
        cx.run_until_parked();
        cx.run_until_parked();

        let editor_focus = cx.read_entity(&panel, |panel, cx| {
            panel.commit_editor.read(cx).focus_handle()
        });
        cx.update(|window, _| window.focus(&editor_focus));
        let _ = cx.refresh();
        cx.update(|_, _| {});

        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                let context = panel.dispatch_context(window, cx);
                assert!(context.contains("VersionControlCommitEditor"));
                assert!(!context.contains("VersionControlChangesTree"));
            });
        });

        let before = cx.read_entity(&panel, |panel, _| section_entries(panel));
        cx.simulate_keystrokes("space");
        cx.run_until_parked();

        cx.update(|_, cx| {
            assert_eq!(
                panel.read(cx).commit_editor.read(cx).text(cx),
                " ",
                "提交信息编辑器聚焦时，空格应输入文本"
            );
        });
        let after = cx.read_entity(&panel, |panel, _| section_entries(panel));
        assert_eq!(after, before, "提交信息中的空格不应切换暂存状态");
    }

    #[gpui::test]
    fn selection_border_only_shows_when_changes_tree_is_focused(cx: &mut TestAppContext) {
        let (root, _temp) = test_repo();
        let project = cx.new(|cx| Project::new(root, cx));
        let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
        cx.run_until_parked();
        cx.run_until_parked();

        let changes_tree_focus = cx.read_entity(&panel, |panel, _| panel.focus.clone());
        cx.update(|window, _| window.focus(&changes_tree_focus));
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("version-control-selection-border")
                .is_some(),
            "变更树聚焦时应显示选中框"
        );

        let commit_editor_focus = cx.read_entity(&panel, |panel, cx| {
            panel.commit_editor.read(cx).focus_handle()
        });
        cx.update(|window, _| window.focus(&commit_editor_focus));
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("version-control-selection-border")
                .is_none(),
            "提交信息编辑器聚焦时不应显示变更树选中框"
        );
        assert!(
            cx.read_entity(&panel, |panel, _| panel.state.borrow().selected.is_some()),
            "切换焦点只隐藏选中框，不应清除变更树选中状态"
        );
    }

    /// 悬停指定行尾的复选框并断言 tooltip 气泡出现。
    ///
    /// 测试时钟不会自动推进：手动拨过 500ms tooltip 显示延迟后再渲染一帧。
    /// 顶部统计行占一行高度，行坐标加 1 行偏移。
    fn assert_hover_tooltip(cx: &mut gpui::VisualTestContext, row_index: usize) {
        let row_height = typography::ui_line();
        cx.simulate_mouse_move(
            point(
                px(1907.),
                px(f32::from(row_height) * (row_index as f32 + 1.5)),
            ),
            None,
            gpui::Modifiers::default(),
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(600));
        cx.run_until_parked();
        let _ = cx.refresh();
        cx.update(|_, _| {}); // 触发一帧渲染，tooltip 请求进入 frame
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("tooltip-view").is_some(),
            "悬停复选框应显示 tooltip 气泡"
        );
    }

    #[gpui::test]
    fn hovering_unchecked_checkbox_shows_tooltip(cx: &mut TestAppContext) {
        let (root, _temp) = test_repo();
        let project_root = root.clone();
        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (_panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
        cx.run_until_parked();
        let _ = cx.refresh();
        cx.update(|_, _| {});
        cx.run_until_parked();

        // 第 4 行（tracked.txt，未暂存组，无对勾）行尾复选框。
        assert_hover_tooltip(cx, 2);
    }

    #[gpui::test]
    fn hovering_checkbox_shows_tooltip_with_many_rows(cx: &mut TestAppContext) {
        // 复现真实场景：大量变更文件（行数超过可视区域，触发 uniform_list 虚拟化）。
        let (root, _temp) = test_repo();
        for index in 0..40 {
            std::fs::write(
                root.join(format!("file-{index:02}.txt")),
                format!("内容 {index}\n"),
            )
            .expect("应写入文件");
        }
        let project_root = root.clone();
        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (_panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
        cx.run_until_parked();
        let _ = cx.refresh();
        cx.update(|_, _| {});
        cx.run_until_parked();

        // 悬停可视区第 5 行（未暂存组的一个文件）行尾复选框；顶部统计行占一行，坐标加偏移。
        let row_height = typography::ui_line();
        let hover_y = f32::from(row_height) * 5.5;
        cx.simulate_mouse_move(
            point(px(1907.), px(hover_y)),
            None,
            gpui::Modifiers::default(),
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(600));
        cx.run_until_parked();
        let _ = cx.refresh();
        cx.update(|_, _| {});
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("tooltip-view").is_some(),
            "多行场景悬停复选框也应显示 tooltip 气泡"
        );
    }

    #[gpui::test]
    fn hover_tooltip_survives_row_rebuild(cx: &mut TestAppContext) {
        // 复现真实场景：行重建（git 操作/滚动后 rebuild_rows）后再次 hover 复选框，
        // tooltip 应仍然显示。
        let (root, _temp) = test_repo();
        std::fs::write(root.join("second.txt"), "第二个文件\n").expect("应写入文件");
        let project_root = root.clone();
        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
        cx.run_until_parked();
        let _ = cx.refresh();
        cx.update(|_, _| {});
        cx.run_until_parked();

        // 先悬停第 4 行（tracked.txt，未暂存组）复选框，确认 tooltip 正常；顶部统计行占一行，坐标加偏移。
        let row_height = typography::ui_line();
        cx.simulate_mouse_move(
            point(px(1907.), px(f32::from(row_height) * 3.5)),
            None,
            gpui::Modifiers::default(),
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(600));
        cx.run_until_parked();
        let _ = cx.refresh();
        cx.update(|_, _| {});
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("tooltip-view").is_some(),
            "重建前悬停复选框应显示 tooltip"
        );

        // 触发行重建：暂存第二个文件（空格选中并暂存），行集合变化。
        cx.update(|window, cx| window.focus(&panel.read(cx).focus));
        cx.simulate_keystrokes("down");
        cx.simulate_keystrokes("space");
        cx.run_until_parked();
        cx.run_until_parked();
        let _ = cx.refresh();
        cx.update(|_, _| {});
        cx.run_until_parked();

        // 移开鼠标再移回 tracked.txt 的复选框（行号可能已变，取第 4 行；顶部统计行占一行）。
        cx.simulate_mouse_move(
            point(px(100.), px(f32::from(row_height) * 3.5)),
            None,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        cx.simulate_mouse_move(
            point(px(1907.), px(f32::from(row_height) * 3.5)),
            None,
            gpui::Modifiers::default(),
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(600));
        cx.run_until_parked();
        let _ = cx.refresh();
        cx.update(|_, _| {});
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("tooltip-view").is_some(),
            "行重建后悬停复选框应仍显示 tooltip"
        );
    }

    #[gpui::test]
    fn hover_tooltip_with_partially_staged_file(cx: &mut TestAppContext) {
        // 部分暂存（MM）：文件同时出现在已暂存与未暂存两组，两个复选框 id 相同
        // （按 path 生成）——验证两组的 tooltip 是否互相干扰。
        let (root, _temp) = test_repo();
        std::fs::write(root.join("tracked.txt"), "第一次修改\n").expect("应修改文件");
        run_in(&root, &["git", "add", "tracked.txt"]);
        std::fs::write(root.join("tracked.txt"), "第二次修改\n").expect("应再次修改文件");

        let project_root = root.clone();
        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (_panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
        cx.run_until_parked();
        let _ = cx.refresh();
        cx.update(|_, _| {});
        cx.run_until_parked();

        // 行布局：3 个分组头 + 已暂存组 tracked.txt（第 4 行）+ 未暂存组 tracked.txt（第 5 行）；
        // 顶部统计行占一行，坐标加偏移。
        let row_height = typography::ui_line();
        // 先悬停未暂存组的复选框（第 5 行）。
        cx.simulate_mouse_move(
            point(px(1907.), px(f32::from(row_height) * 4.5)),
            None,
            gpui::Modifiers::default(),
        );
        cx.executor()
            .advance_clock(std::time::Duration::from_millis(600));
        cx.run_until_parked();
        let _ = cx.refresh();
        cx.update(|_, _| {});
        cx.run_until_parked();
        assert!(
            cx.debug_bounds("tooltip-view").is_some(),
            "部分暂存场景悬停未暂存组复选框应显示 tooltip"
        );
    }

    #[gpui::test]
    fn hovering_checked_checkbox_shows_tooltip(cx: &mut TestAppContext) {
        let (root, _temp) = test_repo();
        let project_root = root.clone();
        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
        cx.run_until_parked();
        let _ = cx.refresh();
        cx.update(|_, _| {});
        cx.run_until_parked();

        // 先暂存文件（空格），行移到已暂存组（第 4 行，带对勾）。
        cx.update(|window, cx| window.focus(&panel.read(cx).focus));
        cx.simulate_keystrokes("down");
        cx.simulate_keystrokes("space");
        cx.run_until_parked();
        cx.run_until_parked();
        let _ = cx.refresh();
        cx.update(|_, _| {});
        cx.run_until_parked();

        // 已暂存组复选框（带对勾）。
        assert_hover_tooltip(cx, 2);
    }

    #[gpui::test]
    fn checkbox_click_stages_file_without_opening_it(cx: &mut TestAppContext) {
        let (root, _temp) = test_repo();
        let project_root = root.clone();
        let open_count = Rc::new(Cell::new(0));
        let callback_count = Rc::clone(&open_count);
        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (panel, cx) = cx.add_window_view(move |_, cx| {
            let mut panel = VersionControlPanel::new(project, cx);
            panel.set_on_open_file(Rc::new(move |_, _, _, _, _| {
                callback_count.set(callback_count.get() + 1);
            }));
            panel
        });
        cx.run_until_parked();
        let _ = cx.refresh();
        cx.update(|_, _| {});
        cx.run_until_parked();

        // 第 4 行（tracked.txt，未暂存组）行尾复选框：窗口 1920 宽，右边缘 6px + 复选框半宽 7px；
        // 顶部统计行占一行，坐标加偏移。
        let row_height = typography::ui_line();
        cx.simulate_click(
            point(px(1907.), px(f32::from(row_height) * 3.5)),
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        cx.run_until_parked();

        let sections = cx.read_entity(&panel, |panel, _| section_entries(panel));
        assert_eq!(
            sections,
            vec![(GitSection::Staged, "tracked.txt".into())],
            "点击复选框应暂存文件"
        );
        assert_eq!(
            open_count.get(),
            0,
            "点击复选框不应触发行的打开逻辑（stop_propagation）"
        );
    }

    #[gpui::test]
    fn commit_flow_clears_editor_and_refreshes_last_commit(cx: &mut TestAppContext) {
        let (root, _temp) = test_repo();
        run_in(&root, &["git", "add", "tracked.txt"]);
        let project_root = root.clone();
        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
        cx.run_until_parked(); // 首次扫描完成（tracked.txt 已暂存）。
        cx.run_until_parked();

        // 底部提交区显示初始提交的 subject。
        let initial = cx.read_entity(&panel, |panel, _| panel.last_commit_message.clone());
        assert_eq!(
            initial.as_deref(),
            Some("initial"),
            "应显示初始提交 subject"
        );

        // 写提交消息并提交（等价于 cmd-enter / 提交按钮的 handler 行为）。
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel
                    .commit_editor
                    .update(cx, |editor, cx| editor.set_text("改动提交", cx));
                panel.handle_commit(&Commit, window, cx);
            });
        });
        cx.run_until_parked(); // commit job 完成
        cx.run_until_parked(); // 重扫落地 → Head/Statuses 事件

        // 编辑器清空、上次提交信息更新、工作树干净。
        cx.update(|_, cx| {
            let text = panel.read(cx).commit_editor.read(cx).text(cx);
            assert!(text.is_empty(), "提交成功后编辑器应清空，实际：{text:?}");
            assert_eq!(
                panel.read(cx).last_commit_message.as_deref(),
                Some("改动提交"),
                "上次提交信息应刷新为新提交 subject"
            );
            let statuses = panel
                .read(cx)
                .project
                .read(cx)
                .git_store()
                .read(cx)
                .repositories()
                .next()
                .map(|(_, snapshot)| snapshot.statuses_by_path.len());
            assert_eq!(statuses, Some(0), "已暂存改动应随提交清空");
        });
    }

    #[gpui::test]
    fn commit_shortcut_submits_staged_changes_from_editor(cx: &mut TestAppContext) {
        let (root, _temp) = test_repo();
        run_in(&root, &["git", "add", "tracked.txt"]);
        let project_root = root.clone();
        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (panel, cx) = cx.add_window_view(move |_, cx| {
            zcv_keymap::init(cx).expect("应注册内置快捷键");
            VersionControlPanel::new(project, cx)
        });
        cx.run_until_parked();
        cx.run_until_parked();

        let editor_focus = cx.update(|_, cx| {
            panel.update(cx, |panel, cx| {
                panel
                    .commit_editor
                    .update(cx, |editor, cx| editor.set_text("快捷键提交", cx));
                panel.commit_editor.read(cx).focus_handle()
            })
        });
        cx.update(|window, _| window.focus(&editor_focus));

        #[cfg(target_os = "macos")]
        cx.simulate_keystrokes("cmd-enter");
        #[cfg(not(target_os = "macos"))]
        cx.simulate_keystrokes("ctrl-enter");
        cx.run_until_parked();
        cx.run_until_parked();

        cx.update(|_, cx| {
            assert_eq!(
                panel.read(cx).last_commit_message.as_deref(),
                Some("快捷键提交"),
                "提交信息框聚焦时，提交快捷键应提交当前暂存"
            );
        });
    }

    #[gpui::test]
    fn commit_without_staged_changes_is_ignored(cx: &mut TestAppContext) {
        let (root, _temp) = test_repo();
        std::fs::write(root.join("untracked.txt"), "新文件\n").expect("应创建未跟踪文件");

        let project_root = root.clone();
        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
        cx.run_until_parked();
        cx.run_until_parked();

        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| {
                panel
                    .commit_editor
                    .update(cx, |editor, cx| editor.set_text("改动提交", cx));
                panel.handle_commit(&Commit, window, cx);
            });
        });
        cx.run_until_parked();
        cx.run_until_parked();

        cx.update(|_, cx| {
            let panel = panel.read(cx);
            assert!(!panel.pending_commit, "无已暂存改动时不应发起提交");
            assert_eq!(
                panel.commit_editor.read(cx).text(cx),
                "改动提交",
                "未发起提交时应保留提交信息"
            );
            assert_eq!(
                panel.last_commit_message.as_deref(),
                Some("initial"),
                "无已暂存改动时 HEAD 不应变化"
            );
            let store = panel.project.read(cx).git_store();
            let store = store.read(cx);
            let snapshot = store
                .repositories()
                .next()
                .map(|(_, snapshot)| snapshot)
                .expect("应存在仓库快照");
            assert!(
                snapshot
                    .statuses_by_path
                    .get(Path::new("tracked.txt"))
                    .is_some_and(|entry| entry.status.has_unstaged()),
                "已跟踪改动应保持未暂存"
            );
            assert!(
                snapshot
                    .statuses_by_path
                    .get(Path::new("untracked.txt"))
                    .is_some_and(|entry| entry.status.is_untracked()),
                "未跟踪文件应保持未暂存"
            );
        });
    }

    #[gpui::test]
    fn uncommit_restores_previous_message_into_editor(cx: &mut TestAppContext) {
        let (root, _temp) = test_repo();
        // 第二次提交：多行消息（subject + 空行 + body），随后制造未暂存改动。
        std::fs::write(root.join("tracked.txt"), "第二次内容\n").expect("应修改文件");
        run_in(&root, &["git", "add", "tracked.txt"]);
        run_in(
            &root,
            &["git", "commit", "-q", "-m", "第二次提交", "-m", "详细说明"],
        );
        std::fs::write(root.join("tracked.txt"), "第三次内容\n").expect("应再次修改文件");

        let project_root = root.clone();
        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (panel, cx) = cx.add_window_view(move |_, cx| VersionControlPanel::new(project, cx));
        cx.run_until_parked();
        cx.run_until_parked();

        // 触发 uncommit（等价于"撤销"按钮的 handler 行为）。
        cx.update(|window, cx| {
            panel.update(cx, |panel, cx| panel.handle_uncommit(&Uncommit, window, cx));
        });
        cx.run_until_parked(); // uncommit job 完成
        cx.run_until_parked(); // 重扫落地 → Head 事件

        // 被撤销提交的完整消息（含 body）填回编辑器；上次提交信息回到 initial。
        cx.update(|_, cx| {
            let text = panel.read(cx).commit_editor.read(cx).text(cx);
            assert_eq!(
                text, "第二次提交\n\n详细说明",
                "uncommit 后应把被撤销提交的完整消息填回编辑器"
            );
            assert_eq!(
                panel.read(cx).last_commit_message.as_deref(),
                Some("initial"),
                "上次提交信息应回到被撤销提交之前的提交"
            );
        });
    }
}
