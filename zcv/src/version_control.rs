//! VersionControlPanel —— 版本管理面板 Entity 组件。
//!
//! 无 git 仓库时居中显示"初始化仓库"按钮（点击对项目根执行 `git init`）；
//! 有仓库时按 已暂存/未暂存 两组展示变更目录树（对齐 Zed git panel 的树模式），部分暂存文件同时出现在已暂存与未暂存两组。
//! 冲突文件暂不展示（待后续版本处理）。
//! 行尾复选框（或空格键）切换条目的暂存/取消暂存：已暂存组勾选、未暂存组未勾选。
//! 行模型由 GitStore 快照构建，订阅 Repositories/Statuses 事件重建。

use std::cell::RefCell;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use gpui::{
    App, Context, Div, ElementId, Entity, FocusHandle, MouseButton, UniformListScrollHandle,
    WeakEntity, Window, actions, div, prelude::*, uniform_list,
};

use crate::project::{GitStoreEvent, Project, RepositorySnapshot};
use crate::project_tree::OnOpenFile;
use crate::ui::{Checkbox, tree};
use crate::workspace::{Panel, ToggleVersionControl};
use zcv_git::{DiffStat, FileStatus, StatusCode};
use zcv_theme::{color, space, typography};

actions!(
    version_control,
    [
        SelectPrev,
        SelectNext,
        Collapse,
        Expand,
        Activate,
        InitRepository,
        ToggleStaged
    ]
);

// 快捷键在 assets/keymaps/default-{platform}.json 的 `VersionControl` 上下文分组声明，
// 组件内不写 key_bindings()。

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
    let key = absolute.strip_prefix(root).unwrap_or(&absolute);
    let key_is_relative = std::ptr::eq(key, absolute.strip_prefix(root).unwrap_or(&absolute));
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
/// 排序规则对齐项目树：目录优先，再按名称。
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

/// 树 → 有序行列表：分组头前置（空组也带头），组内树 DFS 先序。
fn flatten_rows(
    trees: &[Vec<GitTreeNode>; 2],
    expanded: &HashSet<(GitSection, PathBuf)>,
) -> Vec<GitRow> {
    let mut rows = Vec::new();
    for (index, section) in GitSection::ALL.iter().enumerate() {
        rows.push(GitRow::Header(*section));
        flatten_nodes(&mut rows, &trees[index], *section, 0, expanded);
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
    /// 项目根（树键相对它计算；GitStore 路径均 canonicalize，构造时归一化对齐）。
    root: PathBuf,
    project: Entity<Project>,
    state: Rc<RefCell<GitPanelState>>,
    scroll_handle: UniformListScrollHandle,
    on_open_file: Option<OnOpenFile>,
}

impl VersionControlPanel {
    pub(crate) fn new(root: PathBuf, project: Entity<Project>, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let root = root.canonicalize().unwrap_or(root);
        let git_store = project.read(cx).git_store();
        // Head/ActiveRepositoryChanged 不影响变更树展示，只订阅集合与条目变化。
        cx.subscribe(&git_store, |panel, _, event, cx| {
            if matches!(event, GitStoreEvent::Repositories | GitStoreEvent::Statuses) {
                panel.rebuild_rows(cx);
            }
        })
        .detach();
        let mut panel = Self {
            focus,
            root,
            project,
            state: Rc::new(RefCell::new(GitPanelState::new())),
            scroll_handle: UniformListScrollHandle::default(),
            on_open_file: None,
        };
        panel.rebuild_rows(cx);
        panel
    }

    pub(crate) fn set_on_open_file(&mut self, callback: OnOpenFile) {
        self.on_open_file = Some(callback);
    }

    /// 从 GitStore 快照重建行模型（订阅事件 / 折叠展开后调用）。
    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        let git_store = self.project.read(cx).git_store();
        let trees = {
            let store = git_store.read(cx);
            build_section_trees(&self.root, store.repositories())
        };
        let mut state = self.state.borrow_mut();
        let rows = flatten_rows(&trees, &state.expanded);
        state.set_rows(rows);
        // 首次见到目录时默认全部展开（空仓库/空态不置位，init 后仍会展开）；
        // 之后用户折叠状态保持，git 事件触发的重建不重置。
        if !state.initialized {
            let directories = collect_directory_keys(&trees);
            if !directories.is_empty() {
                state.initialized = true;
                state.expanded.extend(directories);
                let rows = flatten_rows(&trees, &state.expanded);
                state.set_rows(rows);
            }
        }
    }

    /// 激活选中行的共享逻辑：目录→展开/折叠；文件→打开。
    ///
    /// `focus_opened_item` 决定打开文件后是否把焦点交给编辑器：双击/键盘 enter 为 `true`（激活），鼠标单击为 `false`（预览，焦点留在面板）。
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
            let mut state = self.state.borrow_mut();
            let key = (entry.section, entry.path);
            if state.expanded.contains(&key) {
                state.expanded.remove(&key);
            } else {
                state.expanded.insert(key);
            }
            drop(state);
            self.rebuild_rows(cx);
        } else if let Some(callback) = self.on_open_file.clone() {
            callback(entry.path, focus_opened_item, window, cx);
        }
        window.refresh();
    }

    fn handle_select_prev(&mut self, _: &SelectPrev, window: &mut Window, _: &mut Context<Self>) {
        self.state.borrow_mut().select_up();
        window.refresh();
    }

    fn handle_select_next(&mut self, _: &SelectNext, window: &mut Window, _: &mut Context<Self>) {
        self.state.borrow_mut().select_down();
        window.refresh();
    }

    fn handle_collapse(&mut self, _: &Collapse, window: &mut Window, cx: &mut Context<Self>) {
        let mut state = self.state.borrow_mut();
        let rows = state.rows.clone();
        let Some(idx) = state.selected_idx() else {
            return;
        };
        let Some(GitRow::Entry(row)) = rows.get(idx) else {
            return;
        };
        if row.is_dir && row.expanded {
            state.expanded.remove(&(row.section, row.path.clone()));
            drop(state);
            self.rebuild_rows(cx);
        } else if row.depth > 0 {
            // 已折叠/叶子：把选中移到上层的祖先行（对齐项目树）。
            let parent_depth = row.depth - 1;
            if let Some(parent_idx) = rows[..idx]
                .iter()
                .rposition(|r| matches!(r, GitRow::Entry(e) if e.is_dir && e.depth == parent_depth))
            {
                if let Some(key) = row_entry_key(&rows[parent_idx]) {
                    state.selected = Some(key);
                }
            }
        }
        window.refresh();
    }

    fn handle_expand(&mut self, _: &Expand, window: &mut Window, cx: &mut Context<Self>) {
        let mut state = self.state.borrow_mut();
        let rows = state.rows.clone();
        let Some(idx) = state.selected_idx() else {
            return;
        };
        let Some(GitRow::Entry(row)) = rows.get(idx) else {
            return;
        };
        if row.is_dir && !row.expanded {
            state.expanded.insert((row.section, row.path.clone()));
            drop(state);
            self.rebuild_rows(cx);
        } else {
            state.select_down();
        }
        window.refresh();
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
}

impl Render for VersionControlPanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.state.borrow_mut().ensure_selected();
        let has_repositories = self
            .project
            .read(cx)
            .git_store()
            .read(cx)
            .has_repositories();
        let is_focused = self.focus.contains_focused(window, cx);
        let content = if has_repositories {
            let render_context = GitPanelRenderContext {
                state: Rc::clone(&self.state),
                rows: self.state.borrow().rows.clone().into(),
                focus: self.focus.clone(),
                weak: cx.weak_entity(),
            };
            render_list(&self.scroll_handle, render_context, is_focused).into_any_element()
        } else {
            render_empty_state(self.focus.clone(), cx).into_any_element()
        };

        div()
            .size_full()
            .track_focus(&self.focus)
            .key_context("VersionControl")
            .tab_index(0)
            .on_action(cx.listener(Self::handle_select_prev))
            .on_action(cx.listener(Self::handle_select_next))
            .on_action(cx.listener(Self::handle_collapse))
            .on_action(cx.listener(Self::handle_expand))
            .on_action(cx.listener(Self::handle_activate))
            .on_action(cx.listener(Self::handle_init_repository))
            .on_action(cx.listener(Self::handle_toggle_staged))
            .child(content)
    }
}

// ═══ 私有渲染辅助函数 ═══════════════════════════════════════════

fn render_list(
    scroll_handle: &UniformListScrollHandle,
    render_context: GitPanelRenderContext,
    is_focused: bool,
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
                render_row(row, sel, is_focused, &render_context, cx).into_any_element()
            })
            .collect()
    })
    .size_full()
    .track_scroll(handle)
}

fn render_row(
    row: &GitRow,
    sel: bool,
    focused: bool,
    render_context: &GitPanelRenderContext,
    cx: &mut App,
) -> Div {
    match row {
        // 分组头：不可选择、不可折叠（对齐 Zed Section Header）。
        GitRow::Header(section) => div()
            .h(typography::ui_line())
            .pl(space::S6)
            .flex()
            .items_center()
            .text_color(color::current(cx).text_muted)
            .child(section.label()),
        GitRow::Entry(entry) => {
            let section = entry.section;
            let path = entry.path.clone();
            let is_dir = entry.is_dir;
            let name = entry.name.clone();
            let status_color = entry
                .status
                .and_then(|status| tree::git_status_color(status, cx));
            let is_deleted = entry.status.is_some_and(|status| status.is_deleted());
            // 文件名按 git 状态着色（对齐项目树；删除文件加删除线）。
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
                            .text_color(colors.status_created)
                            .child(format!("+{}", diff_stat.added)),
                    )
                    .child(
                        div()
                            .text_color(colors.status_deleted)
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
                    .shortcut("version_control::ToggleStaged")
                    .on_click({
                        let weak = render_context.weak.clone();
                        let section = section;
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
            tree::render_row_base(entry.depth, is_dir, entry.expanded, content, cx)
                .cursor_pointer()
                .child(tail)
                .child(checkbox)
                .hover(|style| style.bg(color::current(cx).element_hover))
                .when(sel && focused, |el| el.child(tree::selection_border(cx)))
                .on_mouse_down(MouseButton::Left, {
                    let focus = render_context.focus.clone();
                    let weak = render_context.weak.clone();
                    move |event, window, cx| {
                        // 单击/双击都把焦点收到面板（对齐项目树：交互直接调 Entity 方法）。
                        window.focus(&focus);
                        if let Some(panel) = weak.upgrade() {
                            panel.update(cx, |panel, cx| {
                                panel.state.borrow_mut().selected = Some((section, path.clone()));
                                match event.click_count {
                                    // 单击：目录展开/折叠、文件预览（焦点留在面板）；
                                    // 双击：文件打开并聚焦编辑器；目录不重复，避免"展开→折叠"抵消。
                                    1 => panel.activate_selected(false, window, cx),
                                    _ if is_dir => {}
                                    _ => panel.activate_selected(true, window, cx),
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

impl Panel for VersionControlPanel {
    type ToggleAction = ToggleVersionControl;

    fn icon() -> &'static str {
        "icons/panels/version_control.svg"
    }
    fn label() -> &'static str {
        "版本控制"
    }
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

// ═══ 内部类型 ════════════════════════════════════════════════════

/// 分组（对齐 Zed Section 的 Staging 模式；冲突暂不展示）。
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

/// 变更树行（对齐项目树 TreeRow 的形态）。
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

struct GitPanelState {
    expanded: HashSet<(GitSection, PathBuf)>,
    selected: Option<(GitSection, PathBuf)>,
    rows: Vec<GitRow>,
    /// 是否已执行过"首次全展开"（见 `rebuild_rows`；空态不置位）。
    initialized: bool,
}

impl GitPanelState {
    fn new() -> Self {
        Self {
            expanded: HashSet::new(),
            selected: None,
            rows: Vec::new(),
            initialized: false,
        }
    }

    /// 替换行模型；选中条目消失时清空选中。
    fn set_rows(&mut self, rows: Vec<GitRow>) {
        self.rows = rows;
        let selected = self.selected.clone();
        if selected.as_ref().is_some_and(|sel| {
            !self
                .rows
                .iter()
                .any(|row| row_entry_key(row).as_ref() == Some(sel))
        }) {
            self.selected = None;
        }
    }

    /// 无选中时选中第一个条目行（Header 不可选）。
    fn ensure_selected(&mut self) {
        if self.selected.is_some() {
            return;
        }
        if let Some(key) = self.rows.iter().find_map(row_entry_key) {
            self.selected = Some(key);
        }
    }

    fn selected_idx(&self) -> Option<usize> {
        let selected = self.selected.clone()?;
        self.rows
            .iter()
            .position(|row| row_entry_key(row).as_ref() == Some(&selected))
    }

    /// 上移选择，跳过分组头；无选中时选中最后一个条目行。
    fn select_up(&mut self) {
        match self.selected_idx() {
            None => {
                if let Some(idx) = self
                    .rows
                    .iter()
                    .rposition(|row| matches!(row, GitRow::Entry(_)))
                {
                    self.selected = row_entry_key(&self.rows[idx]);
                }
            }
            Some(idx) => {
                if let Some(prev) = self.rows[..idx]
                    .iter()
                    .rposition(|row| matches!(row, GitRow::Entry(_)))
                {
                    self.selected = row_entry_key(&self.rows[prev]);
                }
            }
        }
    }

    /// 下移选择，跳过分组头；无选中时选中第一个条目行。
    fn select_down(&mut self) {
        match self.selected_idx() {
            None => {
                if let Some(idx) = self
                    .rows
                    .iter()
                    .position(|row| matches!(row, GitRow::Entry(_)))
                {
                    self.selected = row_entry_key(&self.rows[idx]);
                }
            }
            Some(idx) => {
                if let Some(offset) = self.rows[idx + 1..]
                    .iter()
                    .position(|row| matches!(row, GitRow::Entry(_)))
                {
                    self.selected = row_entry_key(&self.rows[idx + 1 + offset]);
                }
            }
        }
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
    state: Rc<RefCell<GitPanelState>>,
    rows: Rc<[GitRow]>,
    focus: FocusHandle,
    /// 条目点击直接调用 Entity 方法（对齐 Zed 的 `cx.listener` 路径）。
    weak: WeakEntity<VersionControlPanel>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;

    use gpui::{KeyBinding, TestAppContext, point, px};
    use tempfile::TempDir;

    use crate::project::{Project, StatusEntry};

    /// 构造快照：路径 → 状态；diff 统计取固定样例值（staged/unstaged 可区分）。
    fn snapshot(entries: &[(&str, FileStatus)]) -> RepositorySnapshot {
        RepositorySnapshot {
            branch: None,
            head: None,
            has_remote: false,
            ahead: 0,
            behind: 0,
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
                            hunks: None,
                        },
                    )
                })
                .collect(),
        }
    }

    fn build_rows(root: &Path, repos: &[(&Path, &RepositorySnapshot)]) -> Vec<GitRow> {
        let trees = build_section_trees(root, repos.iter().copied());
        flatten_rows(&trees, &HashSet::new())
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
        let rows = flatten_rows(&trees, &HashSet::new());
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
        let rows = flatten_rows(&trees, &expanded);
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
        let rows = flatten_rows(&trees, &expanded);
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
        let rows = flatten_rows(&trees, &expanded);
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

    /// 创建带一个修改文件的临时 git 仓库（对齐 zcv-git 测试模式）。
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
        let (panel, cx) = cx.add_window_view(move |_, cx| {
            VersionControlPanel::new(project_root.clone(), project_for_panel, cx)
        });
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
    fn clicking_entry_opens_file_as_preview(cx: &mut TestAppContext) {
        let (root, _temp) = test_repo();
        let project_root = root.clone();
        let open_count = Rc::new(Cell::new(0));
        let last_focus_opened = Rc::new(Cell::new(true));
        let callback_count = Rc::clone(&open_count);
        let callback_focus = Rc::clone(&last_focus_opened);

        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (panel, cx) = cx.add_window_view(move |_, cx| {
            let mut panel = VersionControlPanel::new(project_root, project, cx);
            panel.set_on_open_file(Rc::new(move |_, focus_opened_item, _, _| {
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
        // 行高为 ui_line()，预览打开（focus_opened_item=false）。
        let row_height = typography::ui_line();
        cx.simulate_click(
            point(px(100.), px(f32::from(row_height) * 2.5)),
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();

        assert_eq!(open_count.get(), 1, "单击文件行应打开文件");
        assert!(
            !last_focus_opened.get(),
            "单击应为预览：打开文件但焦点留在面板（focus_opened_item=false）"
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
                KeyBinding::new("down", SelectNext, Some("VersionControl")),
                KeyBinding::new("enter", Activate, Some("VersionControl")),
            ]);
            let mut panel = VersionControlPanel::new(project_root, project, cx);
            panel.set_on_open_file(Rc::new(move |_, focus_opened_item, _, _| {
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
        let (panel, cx) =
            cx.add_window_view(move |_, cx| VersionControlPanel::new(project_root, project, cx));
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
    fn space_toggles_staging_and_moves_row_between_sections(cx: &mut TestAppContext) {
        let (root, _temp) = test_repo();
        let project_root = root.clone();
        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (panel, cx) = cx.add_window_view(move |_, cx| {
            cx.bind_keys([
                KeyBinding::new("down", SelectNext, Some("VersionControl")),
                KeyBinding::new("space", ToggleStaged, Some("VersionControl")),
            ]);
            VersionControlPanel::new(project_root, project, cx)
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

    /// 悬停指定行尾的复选框并断言 tooltip 气泡出现。
    ///
    /// 测试时钟不会自动推进：手动拨过 500ms tooltip 显示延迟后再渲染一帧。
    fn assert_hover_tooltip(cx: &mut gpui::VisualTestContext, row_index: usize) {
        let row_height = typography::ui_line();
        cx.simulate_mouse_move(
            point(
                px(1907.),
                px(f32::from(row_height) * (row_index as f32 + 0.5)),
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
        let (_panel, cx) =
            cx.add_window_view(move |_, cx| VersionControlPanel::new(project_root, project, cx));
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
        let (_panel, cx) =
            cx.add_window_view(move |_, cx| VersionControlPanel::new(project_root, project, cx));
        cx.run_until_parked();
        let _ = cx.refresh();
        cx.update(|_, _| {});
        cx.run_until_parked();

        // 悬停可视区第 5 行（未暂存组的一个文件）行尾复选框。
        let row_height = typography::ui_line();
        let hover_y = f32::from(row_height) * 4.5;
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
        let (panel, cx) =
            cx.add_window_view(move |_, cx| VersionControlPanel::new(project_root, project, cx));
        cx.run_until_parked();
        let _ = cx.refresh();
        cx.update(|_, _| {});
        cx.run_until_parked();

        // 先悬停第 4 行（tracked.txt，未暂存组）复选框，确认 tooltip 正常。
        let row_height = typography::ui_line();
        cx.simulate_mouse_move(
            point(px(1907.), px(f32::from(row_height) * 2.5)),
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

        // 移开鼠标再移回 tracked.txt 的复选框（行号可能已变，取第 4 行）。
        cx.simulate_mouse_move(
            point(px(100.), px(f32::from(row_height) * 2.5)),
            None,
            gpui::Modifiers::default(),
        );
        cx.run_until_parked();
        cx.simulate_mouse_move(
            point(px(1907.), px(f32::from(row_height) * 2.5)),
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
        let (_panel, cx) =
            cx.add_window_view(move |_, cx| VersionControlPanel::new(project_root, project, cx));
        cx.run_until_parked();
        let _ = cx.refresh();
        cx.update(|_, _| {});
        cx.run_until_parked();

        // 行布局：3 个分组头 + 已暂存组 tracked.txt（第 4 行）+ 未暂存组 tracked.txt（第 5 行）。
        let row_height = typography::ui_line();
        // 先悬停未暂存组的复选框（第 5 行）。
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
            "部分暂存场景悬停未暂存组复选框应显示 tooltip"
        );
    }

    #[gpui::test]
    fn hovering_checked_checkbox_shows_tooltip(cx: &mut TestAppContext) {
        let (root, _temp) = test_repo();
        let project_root = root.clone();
        let project = cx.new(|cx| Project::new(project_root.clone(), cx));
        let (panel, cx) =
            cx.add_window_view(move |_, cx| VersionControlPanel::new(project_root, project, cx));
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
            let mut panel = VersionControlPanel::new(project_root, project, cx);
            panel.set_on_open_file(Rc::new(move |_, _, _, _| {
                callback_count.set(callback_count.get() + 1);
            }));
            panel
        });
        cx.run_until_parked();
        let _ = cx.refresh();
        cx.update(|_, _| {});
        cx.run_until_parked();

        // 第 4 行（tracked.txt，未暂存组）行尾复选框：窗口 1920 宽，右边缘 6px + 复选框半宽 7px。
        let row_height = typography::ui_line();
        cx.simulate_click(
            point(px(1907.), px(f32::from(row_height) * 2.5)),
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
}
