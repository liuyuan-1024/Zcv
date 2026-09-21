//! VersionControlPanel —— 版本管理面板 Entity 组件。
//!
//! 无 git 仓库时居中显示"初始化仓库"按钮（点击对项目根执行 `git init`）；
//! 有仓库时按 冲突/已暂存/未暂存 三组展示变更目录树；无冲突时保留普通变更的原有顺序。
//! 部分暂存文件同时出现在已暂存与未暂存两组；未解决的合并冲突展示在最上方的独立组中，不能直接暂存。
//! 行尾复选框（或空格键）切换条目的暂存/取消暂存：冲突组不提供暂存操作。
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
use zcv_git::{DiffStat, FileStatus};
use zcv_path::{AbsolutePathBuf, RelativePathBuf};
use zcv_project::{GitStoreEvent, Project, RepositorySnapshot};
use zcv_theme::{color, space};
use zcv_ui::{
    Button, ButtonLike, ButtonSize, ButtonStyle, Checkbox, RowClickAction, Scrollbar, SvgIcon,
    TooltipSpec, TreeNodeRow, TreeRow, TreeRowFrame, TreeState, row_click_action, selection_border,
    tree_row_label,
};
use zcv_workspace::{Panel, PanelEvent, git_status_color};

use crate::project_diff::ProjectDiffKind;

/// 打开 Git 项目差异并定位文件的回调（弱 Workspace 引用由装配层捕获）。
pub type OnOpenGitDiff = Rc<dyn Fn(ProjectDiffKind, PathBuf, bool, &mut Window, &mut gpui::App)>;

/// 打开版本控制图 Item 的回调（弱 Workspace 引用由装配层捕获）。
pub type OnOpenGitGraph = Rc<dyn Fn(&mut Window, &mut gpui::App)>;

// 版本控制快捷键归属于 `VersionControl` 上下文，由统一快捷键注册表加载；组件内不重复注册。

// ═══ 分组与建树纯函数 ═══════════════════════════════════════════

/// 按用户界面顺序持有三类 Git 变更数据，字段顺序就是展示顺序。
///
/// 使用显式字段而不是数组下标，避免存储顺序与界面顺序分离后发生分组错位。
#[derive(Clone, Debug, Default)]
struct GitSections<T> {
    conflict: T,
    staged: T,
    unstaged: T,
}

impl<T> GitSections<T> {
    fn iter(&self) -> impl Iterator<Item = (GitSection, &T)> {
        [
            (GitSection::Conflict, &self.conflict),
            (GitSection::Staged, &self.staged),
            (GitSection::Unstaged, &self.unstaged),
        ]
        .into_iter()
    }

    fn iter_mut(&mut self) -> impl Iterator<Item = (GitSection, &mut T)> {
        [
            (GitSection::Conflict, &mut self.conflict),
            (GitSection::Staged, &mut self.staged),
            (GitSection::Unstaged, &mut self.unstaged),
        ]
        .into_iter()
    }

    fn get(&self, section: GitSection) -> &T {
        match section {
            GitSection::Conflict => &self.conflict,
            GitSection::Staged => &self.staged,
            GitSection::Unstaged => &self.unstaged,
        }
    }
}

/// 按分组构建变更目录树（每组的根列表），并完成目录聚合与排序。
///
/// 所有仓库合并进同一棵分组树：嵌套仓库在父仓库的 status 中不展开（目录级 untracked 条目被解析器跳过），路径前缀互斥，合并无冲突。
fn build_section_trees<'a>(
    root: &Path,
    repositories: impl Iterator<Item = (&'a Path, &'a RepositorySnapshot)>,
) -> GitSections<Vec<GitTreeNode>> {
    let mut roots = GitSections::default();
    for (workdir, snapshot) in repositories {
        for (relative, entry) in &snapshot.statuses_by_path {
            if entry.status.is_ignored() {
                continue;
            }
            let in_staged = ProjectDiffKind::Staged.includes(entry.status);
            let in_unstaged = ProjectDiffKind::Unstaged.includes(entry.status);
            if in_staged {
                insert_entry(
                    &mut roots.staged,
                    root,
                    workdir,
                    relative,
                    entry.status,
                    entry.staged_diff_stat,
                );
            }
            if in_unstaged {
                insert_entry(
                    &mut roots.unstaged,
                    root,
                    workdir,
                    relative,
                    entry.status,
                    entry.unstaged_diff_stat,
                );
            }
            if ProjectDiffKind::Conflict.includes(entry.status) {
                insert_entry(
                    &mut roots.conflict,
                    root,
                    workdir,
                    relative,
                    entry.status,
                    entry.unstaged_diff_stat,
                );
            }
        }
    }
    for (_, section_tree) in roots.iter_mut() {
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
    relative: &RelativePathBuf,
    status: FileStatus,
    diff_stat: DiffStat,
) {
    let absolute = AbsolutePathBuf::new(workdir.join(relative.as_path()))
        .expect("Git 状态路径必须解析为绝对路径");
    // 树内路径键优先取项目根相对路径；仓库在项目根外时 strip_prefix 失败，退化为绝对路径。
    let key_is_relative = absolute.as_path().strip_prefix(root).is_ok();
    let key = absolute
        .as_path()
        .strip_prefix(root)
        .unwrap_or(absolute.as_path());
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
            AbsolutePathBuf::new(root.join(&prefix)).expect("Git 目录路径必须解析为绝对路径")
        } else {
            AbsolutePathBuf::new(prefix.clone()).expect("Git 目录路径必须解析为绝对路径")
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

/// 树 → 有序行列表：分组头前置，展开的空组显示一行提示，非空组按 DFS 先序展开；折叠的分区只留标题行。
fn flatten_rows(
    trees: &GitSections<Vec<GitTreeNode>>,
    expanded: &HashSet<(GitSection, AbsolutePathBuf)>,
    collapsed: &HashSet<GitSection>,
) -> Vec<GitRow> {
    let mut rows = Vec::new();
    for (section, tree) in trees.iter() {
        rows.push(GitRow::Header(section));
        if !collapsed.contains(&section) {
            if tree.is_empty() {
                rows.push(GitRow::Empty(section));
            } else {
                flatten_nodes(&mut rows, tree, section, 0, expanded);
            }
        }
    }
    rows
}

fn flatten_nodes(
    rows: &mut Vec<GitRow>,
    nodes: &[GitTreeNode],
    section: GitSection,
    depth: usize,
    expanded: &HashSet<(GitSection, AbsolutePathBuf)>,
) {
    for node in nodes {
        let mut folded_name = node.name.clone();
        let mut visible_node = node;
        // 变更树只包含有变更的路径，因此连续的单子目录可以在行模型中压缩。
        // 只有子目录本身处于展开状态时才继续压缩，避免吞掉用户明确折叠的边界。
        while visible_node.is_dir
            && expanded.contains(&(section, visible_node.path.clone()))
            && visible_node.children.len() == 1
            && visible_node.children[0].is_dir
        {
            let child = &visible_node.children[0];
            folded_name.push('/');
            folded_name.push_str(&child.name);
            visible_node = child;
        }

        let is_expanded =
            visible_node.is_dir && expanded.contains(&(section, visible_node.path.clone()));
        rows.push(GitRow::Entry(GitTreeRow {
            section,
            path: visible_node.path.clone(),
            name: folded_name,
            depth,
            is_dir: node.is_dir,
            expanded: is_expanded,
            status: visible_node.status,
            diff_stat: visible_node.diff_stat,
        }));
        if is_expanded {
            flatten_nodes(rows, &visible_node.children, section, depth + 1, expanded);
        }
    }
}

/// 收集所有分组树中的目录节点键（(分组, 绝对路径)），供默认全展开使用。
fn collect_directory_keys(
    trees: &GitSections<Vec<GitTreeNode>>,
) -> HashSet<(GitSection, AbsolutePathBuf)> {
    let mut keys = HashSet::new();
    for (section, tree) in trees.iter() {
        collect_dirs(tree, section, &mut keys);
    }
    keys
}

fn collect_dirs(
    nodes: &[GitTreeNode],
    section: GitSection,
    keys: &mut HashSet<(GitSection, AbsolutePathBuf)>,
) {
    for node in nodes {
        if node.is_dir {
            keys.insert((section, node.path.clone()));
            collect_dirs(&node.children, section, keys);
        }
    }
}

// ═══ Entity ═══════════════════════════════════════════════════════

pub struct VersionControlPanel {
    focus: FocusHandle,
    focus_listeners_initialized: bool,
    project: Entity<Project>,
    state: Rc<RefCell<TreeState<(GitSection, AbsolutePathBuf), GitRow>>>,
    /// 用户显式折叠的目录（(分组, 路径)）；未折叠的目录默认展开，新出现的目录自动展开。
    collapsed_dirs: HashSet<(GitSection, AbsolutePathBuf)>,
    /// 折叠的分区（点击分区标题行首 chevron 切换；折叠时该分区条目不渲染）。
    collapsed_sections: Rc<RefCell<HashSet<GitSection>>>,
    /// 各分组的顶层变更路径；标题行复选框可见性与全选以此为准，不随折叠变化。
    section_paths: GitSections<Vec<AbsolutePathBuf>>,
    scroll_handle: UniformListScrollHandle,
    scrollbar: Scrollbar<UniformListScrollHandle>,
    /// 底部提交信息编辑器。
    commit_editor: Entity<Editor>,
    /// 活动仓库最近一次提交的 subject（订阅 Repositories/Statuses/Head 时刷新）。
    last_commit_message: Option<String>,
    /// 自己发起的提交在途：Head 事件时清空编辑器并复位（外部 checkout/commit 不清草稿）。
    pending_commit: bool,
    on_open_file: Option<OnOpenGitDiff>,
    on_open_graph: Option<OnOpenGitGraph>,
}

impl VersionControlPanel {
    pub fn new(project: Entity<Project>, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let git_store = project.read(cx).git_store();
        // 注册表由装配层（Project 持有的唯一语言注册表）注入，编辑器不再自建。
        let language_registry = project.read(cx).language_registry();
        let commit_editor = cx.new(move |cx| {
            let mut editor = Editor::auto_height(7, Some(7), language_registry, cx);
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
                // 变更块操作失败由项目差异视图负责提示与恢复。
                GitStoreEvent::IndexText { .. }
                | GitStoreEvent::JobsUpdated
                | GitStoreEvent::HunkOperationFailed(_)
                | GitStoreEvent::UncommitFailed(_) => {}
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
            section_paths: GitSections::default(),
            scroll_handle,
            scrollbar,
            commit_editor,
            last_commit_message: None,
            pending_commit: false,
            on_open_file: None,
            on_open_graph: None,
        };
        panel.rebuild_rows(cx);
        panel
    }

    pub fn set_on_open_file(&mut self, callback: OnOpenGitDiff) {
        self.on_open_file = Some(callback);
    }

    pub fn set_on_open_graph(&mut self, callback: OnOpenGitGraph) {
        self.on_open_graph = Some(callback);
    }

    /// 打开版本控制图（回调由装配层注入；未注入时静默忽略）。
    fn open_git_graph(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(callback) = self.on_open_graph.clone() {
            callback(window, cx);
        }
    }

    /// 快捷键上下文：面板标识 + 按焦点区分的子状态标签。
    fn dispatch_context(&self, window: &Window, cx: &Context<Self>) -> KeyContext {
        let mut context = KeyContext::new_with_defaults();
        context.add("GitPanel");
        context.add(
            if self
                .commit_editor
                .read(cx)
                .focus_handle()
                .is_focused(window)
            {
                "CommitEditor"
            } else {
                "ChangesList"
            },
        );
        context
    }

    fn changes_tree_is_focused(&self, window: &Window) -> bool {
        self.focus.is_focused(window)
    }

    /// 从 GitStore 快照重建行模型（订阅事件 / 折叠展开后调用）。
    ///
    /// 项目根实时读取（不缓存）：RootChanged 后树键基准跟随项目，避免与事件流不同步。
    /// Project 已在创建阶段保存绝对路径，这里只读取路径身份，不重新访问文件系统。
    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let Some(root) = self.project.read(cx).root().map(|root| {
            AbsolutePathBuf::new(root.to_path_buf())
                .expect("项目根目录必须是绝对路径")
                .into_path_buf()
        }) else {
            return;
        };
        let git_store = self.project.read(cx).git_store();
        let trees = {
            let store = git_store.read(cx);
            build_section_trees(&root, store.repositories())
        };
        // 各分组顶层路径：暂存/取消暂存按目录前缀覆盖其下全部变更文件，与展开折叠无关。
        let section_paths = GitSections {
            conflict: trees
                .conflict
                .iter()
                .map(|node| node.path.clone())
                .collect(),
            staged: trees.staged.iter().map(|node| node.path.clone()).collect(),
            unstaged: trees
                .unstaged
                .iter()
                .map(|node| node.path.clone())
                .collect(),
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
        self.section_paths = section_paths;
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
    ///
    /// 以分组树的全部条目为准，折叠时不可见的分区同样能整组操作。
    fn toggle_section_all(&mut self, section: GitSection, cx: &mut Context<Self>) {
        let paths: Vec<AbsolutePathBuf> = self.section_paths.get(section).to_vec();
        if paths.is_empty() {
            return;
        }
        let store = self.project.read(cx).git_store();
        match section {
            GitSection::Staged => store.update(cx, |store, cx| store.unstage_paths(paths, cx)),
            GitSection::Unstaged => store.update(cx, |store, cx| store.stage_paths(paths, cx)),
            GitSection::Conflict => {}
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
            let kind = ProjectDiffKind::from(entry.section);
            callback(
                kind,
                entry.path.into_path_buf(),
                focus_opened_item,
                window,
                cx,
            );
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
    fn selected_directory_key(&self, expanded: bool) -> Option<(GitSection, AbsolutePathBuf)> {
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
        self.init_repository(cx);
    }

    fn init_repository(&mut self, cx: &mut Context<Self>) {
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
    fn toggle_staged_for(
        &mut self,
        section: GitSection,
        path: &AbsolutePathBuf,
        cx: &mut Context<Self>,
    ) {
        let store = self.project.read(cx).git_store();
        match section {
            GitSection::Unstaged => store.update(cx, |store, cx| {
                store.stage_paths(vec![path.clone()], cx);
            }),
            GitSection::Staged => store.update(cx, |store, cx| {
                store.unstage_paths(vec![path.clone()], cx);
            }),
            GitSection::Conflict => {}
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
            window.focus(&focus, cx);
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
                non_empty_sections: self
                    .section_paths
                    .iter()
                    .filter(|(_, paths)| !paths.is_empty())
                    .map(|(section, _)| section)
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
            render_empty_state(cx.weak_entity(), cx).into_any_element()
        };

        // 字段先提出局部变量（闭包借用与 listener 的 cx 互不冲突）。
        let commit_editor = self.commit_editor.clone();
        let last_commit_message = self.last_commit_message.clone();
        let footer = if has_repositories {
            Some(render_commit_footer(
                &commit_editor,
                last_commit_message.as_deref(),
                has_staged_changes,
                cx.weak_entity(),
                cx,
            ))
        } else {
            None
        };

        // 面板顶部：加减号图标 + 总变更行数（有仓库时显示，Diff 图标 + DiffStat）。
        let header = has_repositories.then(|| {
            let total = self.project.read(cx).git_store().read(cx).total_diff_stat();
            render_total_diff_stat(total, window, cx)
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
fn render_total_diff_stat(total: DiffStat, window: &Window, cx: &App) -> Div {
    let colors = color::current(cx);
    let mut frame = TreeRowFrame::default().leading(
        SvgIcon::new("icons/diff.svg")
            .id(ElementId::Name("version-control-total-diff".into()))
            .label("变更行数统计")
            .color(colors.icon_muted),
    );
    if total.added > 0 || total.deleted > 0 {
        frame = frame
            .content(
                div()
                    .text_color(colors.version_control_added)
                    .child(format!("+{}", total.added)),
            )
            .content(
                div()
                    .text_color(colors.version_control_deleted)
                    .child(format!("−{}", total.deleted)),
            );
    }
    frame.render(window, cx).text_color(colors.text_muted)
}

fn render_list(
    scroll_handle: &UniformListScrollHandle,
    scrollbar: &Scrollbar<UniformListScrollHandle>,
    render_context: GitPanelRenderContext,
    changes_tree_focused: bool,
) -> gpui::UniformList {
    let handle = scroll_handle.clone();
    let len = render_context.rows.len();
    uniform_list("version-control-list", len, move |range, window, cx| {
        let state = render_context.state.borrow();
        let rows = &render_context.rows;
        let selected = state.selected.clone();
        range
            .filter_map(|i| rows.get(i))
            .map(|row| {
                let sel = row_entry_key(row) == selected;
                render_row(row, sel, changes_tree_focused, &render_context, window, cx)
                    .into_any_element()
            })
            .collect()
    })
    .size_full()
    .track_scroll(&handle)
    .with_decoration(scrollbar.clone())
}

fn render_row(
    row: &GitRow,
    sel: bool,
    changes_tree_focused: bool,
    render_context: &GitPanelRenderContext,
    window: &Window,
    cx: &mut App,
) -> impl IntoElement {
    match row {
        // 分组头：不可选择；
        // 行首 chevron 折叠/展开分区，行尾复选框全选/取消全选（空分区不显示复选框）。
        GitRow::Header(section) => {
            let is_collapsed = render_context.collapsed.borrow().contains(section);
            let weak = render_context.weak.clone();
            let section = *section;
            let checkbox_weak = weak.clone();
            let section_has_entries = section != GitSection::Conflict
                && render_context.non_empty_sections.contains(&section);
            let mut frame = TreeRowFrame::default()
                .leading(
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
                .content(section.label());
            if section_has_entries {
                // 已暂存组显示勾选（点击 = 全部取消暂存）、未暂存组显示未勾选（点击 = 全部暂存）。
                frame = frame.trailing(
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
                );
            }
            frame
                .render(window, cx)
                .id(ElementId::Name(
                    format!("version-control-header-row-{section:?}").into(),
                ))
                .text_color(color::current(cx).text_muted)
                .cursor_pointer()
                .hover(|style| style.bg(color::current(cx).element_hover))
                // 整行点击折叠/展开。
                .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
                    if let Some(panel) = weak.upgrade() {
                        panel.update(cx, |panel, cx| {
                            panel.toggle_section_collapsed(section, cx);
                        });
                    }
                })
                .into_any_element()
        }
        // 空分组提示只是树内容的一部分，不参与选择、焦点或鼠标交互。
        GitRow::Empty(section) => TreeRowFrame::default()
            .content(section.empty_message())
            .render(window, cx)
            .text_color(color::current(cx).text_placeholder)
            .into_any_element(),
        GitRow::Entry(entry) => {
            let section = entry.section;
            let path = entry.path.clone();
            let is_dir = entry.is_dir;
            let name = entry.name.clone();
            let status_color = entry.status.and_then(|status| git_status_color(status, cx));
            // 删除线只作用于文件行。
            let is_deleted = !is_dir && entry.status.is_some_and(|status| status.is_deleted());
            // 文件名按 git 状态着色（删除文件加删除线）。
            let content = tree_row_label(name)
                .when_some(status_color, |label, label_color| {
                    label.text_color(label_color)
                })
                .when(is_deleted, |label| label.line_through());
            let diff_stat = entry.diff_stat;
            // 行尾改动计数（目录行为子项求和；全零不显示，如 untracked 文件）。
            // 加减分别用 git 状态色：+ 新增色、− 删除色。
            let colors = color::current(cx);
            // 行尾暂存复选框（在改动计数之后）。
            let checkbox = if entry.status == Some(FileStatus::Unmerged) {
                None
            } else {
                Some(
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
                    .shortcut(zcv_keymap::display_shortcut(&ToggleStaged, cx))
                    .on_click({
                        let weak = render_context.weak.clone();
                        let path = path.clone();
                        move |_window, cx| {
                            if let Some(panel) = weak.upgrade() {
                                panel.update(cx, |panel, cx| {
                                    // 复选框会阻止行点击事件，必须显式把被操作的行设为选中项；
                                    // 目录暂存后整棵子树会重排，否则选中迁移会基于旧行。
                                    panel.state.borrow_mut().select((section, path.clone()));
                                    panel.toggle_staged_for(section, &path, cx);
                                });
                            }
                        }
                    })
                    .into_any_element(),
                )
            };
            let mut node =
                TreeNodeRow::new(entry.depth, &entry.path, is_dir, entry.expanded, content);
            if diff_stat.added > 0 || diff_stat.deleted > 0 {
                node = node
                    .trailing(
                        div()
                            .text_color(colors.version_control_added)
                            .child(format!("+{}", diff_stat.added)),
                    )
                    .trailing(
                        div()
                            .text_color(colors.version_control_deleted)
                            .child(format!("−{}", diff_stat.deleted)),
                    );
            }
            if let Some(checkbox) = checkbox {
                node = node.trailing(checkbox);
            }
            node.frame(window, cx)
                .interactive(
                    ElementId::Name(
                        format!("version-control-row-{:?}-{}", section, entry.path.display())
                            .into(),
                    ),
                    window,
                    cx,
                )
                .when(sel && changes_tree_focused, |el| {
                    el.child(
                        selection_border(window, cx)
                            .debug_selector(|| "version-control-selection-border".into()),
                    )
                })
                .on_mouse_down(MouseButton::Left, {
                    let focus = render_context.focus.clone();
                    let weak = render_context.weak.clone();
                    move |event, window, cx| {
                        window.focus(&focus, cx);
                        if let Some(panel) = weak.upgrade() {
                            panel.update(cx, |panel, cx| {
                                panel.state.borrow_mut().selected = Some((section, path.clone()));
                                match row_click_action(is_dir, event.click_count) {
                                    RowClickAction::Toggle => {
                                        panel.activate_selected(true, window, cx)
                                    }
                                    RowClickAction::Preview => {
                                        panel.activate_selected(false, window, cx)
                                    }
                                    RowClickAction::Activate => {
                                        panel.activate_selected(true, window, cx)
                                    }
                                }
                            });
                        }
                        cx.stop_propagation();
                    }
                })
                .into_any_element()
        }
    }
}

fn render_commit_footer(
    editor: &Entity<Editor>,
    last_commit_message: Option<&str>,
    has_staged_changes: bool,
    weak: WeakEntity<VersionControlPanel>,
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
                .child(div().pt(space::S6).px(space::S6).child(editor.clone()))
                // 容器内底部 commit-footer：提交按钮（空消息时淡显，点击由 handler 兜底聚焦回编辑器）。
                .child(
                    div()
                        .id("version-control-commit-footer")
                        .p(space::S6)
                        .flex()
                        .justify_end()
                        .child(
                            div()
                                .debug_selector(|| "version-control-commit-button".into())
                                .child(
                                    Button::text("version-control-commit", "提交")
                                        .size(ButtonSize::Loose)
                                        .style(ButtonStyle::Solid)
                                        .disabled(!has_staged_changes)
                                        .label("提交当前暂存")
                                        .shortcut(zcv_keymap::display_shortcut(&Commit, cx))
                                        .color(if message.trim().is_empty() {
                                            colors.text_muted
                                        } else {
                                            colors.text
                                        })
                                        .on_click({
                                            let weak = weak.clone();
                                            move |_, window, cx| {
                                                if let Some(panel) = weak.upgrade() {
                                                    panel.update(cx, |panel, cx| {
                                                        panel.handle_commit(&Commit, window, cx);
                                                    });
                                                }
                                            }
                                        }),
                                ),
                        ),
                ),
        )
        // 上次提交信息。
        .child(
            div()
                .border_t_1()
                .border_color(colors.border_variant)
                .p(space::S6)
                .flex()
                .items_center()
                .gap(space::S6)
                .child(
                    ButtonLike::new("version-control-last-commit")
                        .flex_grow()
                        .tooltip(if has_last_commit {
                            TooltipSpec::from_lines([format!("最近提交：{last_commit_text}")])
                        } else {
                            TooltipSpec::default()
                        })
                        .child(
                            div()
                                .overflow_hidden()
                                .truncate()
                                .text_color(if has_last_commit {
                                    colors.text
                                } else {
                                    colors.text_muted
                                })
                                .child(last_commit_text),
                        ),
                )
                // 撤销按钮：仅在有提交时显示（无提交时 uncommit 无意义）。
                // hover 提示"撤销提交"。
                .when(has_last_commit, |element| {
                    let element = element.child(
                        div()
                            .debug_selector(|| "version-control-uncommit-button".into())
                            .child(
                                Button::icon("version-control-uncommit", "icons/undo.svg")
                                    .label("撤销提交")
                                    .on_click({
                                        let weak = weak.clone();
                                        move |_, window, cx| {
                                            if let Some(panel) = weak.upgrade() {
                                                panel.update(cx, |panel, cx| {
                                                    panel.handle_uncommit(&Uncommit, window, cx);
                                                });
                                            }
                                        }
                                    }),
                            ),
                    );
                    // 版本控制图入口：撤销按钮右侧，打开只读图形化提交历史 Item。
                    element.child(
                        div()
                            .debug_selector(|| "version-control-git-graph-button".into())
                            .child(
                                Button::icon("version-control-git-graph", "icons/git_graph.svg")
                                    .label("版本控制图")
                                    .on_click({
                                        let weak = weak.clone();
                                        move |_, window, cx| {
                                            if let Some(panel) = weak.upgrade() {
                                                panel.update(cx, |panel, cx| {
                                                    panel.open_git_graph(window, cx);
                                                });
                                            }
                                        }
                                    }),
                            ),
                    )
                }),
        )
}

fn render_empty_state(panel: WeakEntity<VersionControlPanel>, cx: &App) -> Div {
    let colors = color::current(cx);
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(space::S6)
        .text_color(colors.text_placeholder)
        .child("没有 Git 仓库")
        .child(
            div()
                .id("version-control-init")
                .debug_selector(|| "version-control-init".into())
                .p(space::S6)
                .rounded_md()
                .border_1()
                .border_color(colors.border_variant)
                .bg(colors.panel_background)
                .text_color(colors.text)
                .cursor_pointer()
                .hover(|style| style.bg(colors.element_hover))
                .child("初始化仓库")
                .on_click(move |_, _, cx| {
                    panel.update(cx, |panel, cx| panel.init_repository(cx)).ok();
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

/// 变更分组。
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
enum GitSection {
    Staged,
    Unstaged,
    Conflict,
}

impl GitSection {
    fn label(self) -> &'static str {
        match self {
            Self::Staged => "已暂存",
            Self::Unstaged => "未暂存",
            Self::Conflict => "冲突",
        }
    }

    fn empty_message(self) -> &'static str {
        match self {
            Self::Staged => "没有已暂存的更改",
            Self::Unstaged => "没有未暂存的更改",
            Self::Conflict => "没有未解决的冲突",
        }
    }
}

impl From<GitSection> for ProjectDiffKind {
    fn from(section: GitSection) -> Self {
        match section {
            GitSection::Staged => Self::Staged,
            GitSection::Unstaged => Self::Unstaged,
            GitSection::Conflict => Self::Conflict,
        }
    }
}

/// 统一行模型：分组头和空分组提示不可选择，只有条目行参与树交互。
#[derive(Clone, Debug)]
enum GitRow {
    Header(GitSection),
    Empty(GitSection),
    Entry(GitTreeRow),
}

/// 变更树行。
#[derive(Clone, Debug)]
struct GitTreeRow {
    /// 所在分组（选中/展开键的一部分；(section, path) 在可见行内唯一）。
    section: GitSection,
    /// 绝对路径（打开回调用）。
    path: AbsolutePathBuf,
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
    path: AbsolutePathBuf,
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
            GitRow::Header(_) | GitRow::Empty(_) => 0,
        }
    }
    fn expanded(&self) -> bool {
        matches!(self, GitRow::Entry(entry) if entry.expanded)
    }
}

/// 行 → 选中/展开键（分组头和空分组提示为 None）。
fn row_entry_key(row: &GitRow) -> Option<(GitSection, AbsolutePathBuf)> {
    match row {
        GitRow::Entry(entry) => Some((entry.section, entry.path.clone())),
        GitRow::Header(_) | GitRow::Empty(_) => None,
    }
}

#[derive(Clone)]
struct GitPanelRenderContext {
    state: Rc<RefCell<TreeState<(GitSection, AbsolutePathBuf), GitRow>>>,
    rows: Rc<[GitRow]>,
    focus: FocusHandle,
    /// 折叠的分区（标题行 chevron 渲染与点击共享）。
    collapsed: Rc<RefCell<HashSet<GitSection>>>,
    /// 有条目的分区（由分组树派生，与折叠无关；空分区标题行不显示全选复选框）。
    non_empty_sections: HashSet<GitSection>,
    /// 条目点击直接调用 Entity 方法。
    weak: WeakEntity<VersionControlPanel>,
}

#[cfg(test)]
#[path = "test/version_control_tests.rs"]
mod tests;
