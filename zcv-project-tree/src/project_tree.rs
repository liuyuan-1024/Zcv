//! ProjectTreePanel —— 项目文件树 Entity 组件。
//!
//! 持有 `Rc<RefCell<TreeState>>` 管理展开/选中状态和缓存行模型。
//! 目录遍历、排除规则与 git 状态合并由 `Project`（worktree 快照层）产出，渲染与键盘导航只消费行模型缓存。
//! 多选（shift/cmd 点击、shift 方向键）、复制/剪切/粘贴与拖拽移动共用同一套净化与冲突确认管线。
//! 模块划分：本文件承载实体与行模型管理；
//! 渲染见 render.rs，键盘动作见 actions.rs，名称编辑见 editing.rs，剪贴板与传输执行见 execute.rs，拖拽/传输纯逻辑见 drag.rs / transfer.rs。

use std::cell::RefCell;
use std::path::{Path, PathBuf};
use std::rc::Rc;
use std::time::Duration;

use gpui::{
    AsyncApp, Context, Entity, EventEmitter, KeyContext, MouseButton, ScrollStrategy, Task,
    UniformListScrollHandle, WeakEntity, Window, div, prelude::*, px, relative,
};
use zcv_actions::TreeActivate;
use zcv_editor::Editor;
use zcv_git::FileStatus;
use zcv_project::{Project, WorktreeEntry, translate_path};
use zcv_theme::color;
use zcv_ui::ConfirmOverlay;
use zcv_ui::Scrollbar;
use zcv_ui::tree::{self, TreeRow, TreeState};
use zcv_workspace::{Panel, PanelEvent};

use zcv_settings::SettingsStore;

pub mod git_status;

/// git 状态 → 展示颜色（消费方：项目树与版本控制面板共用）。
pub use git_status::git_status_color;

/// 打开文件回调：面板请求 Workspace 打开路径（弱引用防循环持有）。
pub type OnOpenFile = Rc<dyn Fn(PathBuf, bool, &mut Window, &mut gpui::App)>;

/// 重命名文件或目录回调。
pub type OnRename = Rc<dyn Fn(PathBuf, PathBuf, &mut gpui::App) -> anyhow::Result<()>>;

/// 新建文件或目录回调。
pub type OnCreate = Rc<dyn Fn(PathBuf, bool, &mut gpui::App) -> anyhow::Result<()>>;

/// 将文件或目录移到系统废纸篓回调。
///
/// 带 `Window`：删除文件后需要关闭打开它的 tab，工具栏更新需要 window。
pub type OnTrash = Rc<dyn Fn(PathBuf, &mut Window, &mut gpui::App) -> anyhow::Result<()>>;

/// 在项目内移动文件或目录回调（from, to, overwrite：覆盖已存在目标）。
pub type OnMove = Rc<dyn Fn(PathBuf, PathBuf, bool, &mut gpui::App) -> anyhow::Result<()>>;

mod actions;
mod drag;
mod editing;
mod execute;
mod render;
mod transfer;

use drag::TreeDrag;
use editing::EditState;
use render::{ProjectTreeRenderContext, render_empty_state, render_list};
use transfer::{ConflictSession, TreeClipboard};

#[cfg(test)]
mod tests;

// ── Entity ──────────────────────────────────────────────────────────

/// EntriesChanged 防抖窗口：批量文件操作的事件风暴合并为一次行模型重建。
const REFRESH_DEBOUNCE: Duration = Duration::from_millis(120);

pub struct ProjectTreePanel {
    pub focus: gpui::FocusHandle,
    /// 当前项目根目录路径；无 worktree 的空工作区为 None（面板显示空态）。
    root: Option<PathBuf>,
    /// 行模型与 git 状态查询（worktree 快照层由 Project 持有）。
    project: Entity<Project>,
    state: Rc<RefCell<TreeState<PathBuf, ProjectTreeRow>>>,
    /// 当前活动文件（编辑器焦点所在文件），与选中行相互独立。
    active_path: Option<PathBuf>,
    scroll_handle: UniformListScrollHandle,
    scrollbar: Scrollbar<UniformListScrollHandle>,
    entry_name_editor: Entity<Editor>,
    edit_state: Option<EditState>,
    on_open_file: Option<OnOpenFile>,
    on_rename: Option<OnRename>,
    on_create: Option<OnCreate>,
    on_trash: Option<OnTrash>,
    /// 在项目内移动文件或目录回调（剪切粘贴/拖拽共用）。
    on_move: Option<OnMove>,
    /// 项目树剪贴板：copy/cut 写入，paste 消费。
    clipboard: Option<TreeClipboard>,
    /// 进行中的冲突确认会话（浮层数据源；存在时阻塞其他命令）。
    conflict: Option<ConflictSession>,
    /// 按下时暂存的点击动作意图（行路径, 动作）：click（mouse_up 未拖拽）派发时消费执行打开/展开；
    /// 拖拽消费了 click 时意图残留，由下次按下清除。
    pending_click_intent: Option<(PathBuf, tree::RowClickAction)>,
    /// 冲突会话期间暂存的非冲突项：决策完成后与冲突项合成完整执行清单。
    pending_clean_items: Vec<(PathBuf, PathBuf)>,
    /// 复制进度（已完成数, 总数)：进度条数据源，None 时不渲染。
    active_transfer: Option<(usize, usize)>,
    /// 防抖刷新任务：新调度直接覆盖旧任务。
    refresh_task: Option<Task<()>>,
    /// 防抖会话代次：每次调度自增，到期任务校验代次未变才执行刷新。
    refresh_generation: u64,
    /// 行重建代次：每次派发自增，apply_rebuild 校验一致才应用（过期结果丢弃）。
    rebuild_generation: u64,
    /// 进行中的行重建任务：新派发覆盖旧任务（Task drop 即取消）。
    pending_rebuild: Option<Task<()>>,
    /// 待定 reveal：异步行重建期间记录目标路径，行进入行模型后滚动到可见（过期代次由重建代次校验丢弃）。
    pending_reveal: Option<PathBuf>,
    /// 行快照缓存：replace_rows / 行内容变更时重建，渲染每帧只做 Rc 克隆（不深拷贝）。
    row_snapshot: Rc<[ProjectTreeRow]>,
    /// 拖拽悬停展开计时：悬停折叠目录行约 500ms 自动展开；
    /// 悬停目标变化/放下/取消时 take 置空（Task drop 即取消）。
    hover_expand_task: Option<Task<()>>,
    /// 当前拖拽悬停的目录行路径：到期任务校验悬停未移开的依据。
    drag_hover_path: Option<PathBuf>,
}

impl ProjectTreePanel {
    pub fn new(project: Entity<Project>, cx: &mut Context<Self>) -> Self {
        let focus = cx.focus_handle();
        let entry_name_editor = cx.new(Editor::single_line);
        cx.observe(&entry_name_editor, |_, _, cx| cx.notify())
            .detach();
        let exclusions = SettingsStore::file_scan_exclusions(cx);
        project.update(cx, |project, _| project.set_exclusions(&exclusions));
        // 项目根从 Project 派生：无 worktree 时为空态，面板同样注册（对齐 Zed 无条件装配）。
        let mut state = TreeState::new(|row: &ProjectTreeRow| Some(row.path.clone()));
        if let Some(root) = project.read(cx).root() {
            state.expanded.insert(root.to_path_buf());
        }
        // git 状态变化（含忽略集变化）时刷新行颜色，不重扫目录。
        let git_store = project.read(cx).git_store();
        cx.subscribe(&git_store, |tree, _, _event, cx| {
            tree.refresh_git_statuses(cx);
        })
        .detach();
        let scroll_handle = UniformListScrollHandle::default();
        let scrollbar = Scrollbar::vertical(scroll_handle.clone());
        let mut this = Self {
            focus,
            root: project.read(cx).root().map(PathBuf::from),
            project,
            state: Rc::new(RefCell::new(state)),
            active_path: None,
            scroll_handle,
            scrollbar,
            entry_name_editor,
            edit_state: None,
            on_open_file: None,
            on_rename: None,
            on_create: None,
            on_trash: None,
            on_move: None,
            clipboard: None,
            conflict: None,
            pending_click_intent: None,
            pending_clean_items: Vec::new(),
            active_transfer: None,
            refresh_task: None,
            refresh_generation: 0,
            rebuild_generation: 0,
            pending_rebuild: None,
            pending_reveal: None,
            row_snapshot: Vec::new().into(),
            hover_expand_task: None,
            drag_hover_path: None,
        };
        this.rebuild_rows(cx);
        this
    }

    /// 重建可见行：后台收集目录条目，完成回 UI 线程应用；新派发覆盖旧任务（过期结果由代次校验丢弃）。
    fn rebuild_rows(&mut self, cx: &mut Context<Self>) {
        self.rebuild_generation += 1;
        let generation = self.rebuild_generation;
        if self.root.is_none() {
            // 无 worktree 的空态：行模型为空，无需后台收集。
            self.state.borrow_mut().replace_rows(Vec::new());
            self.row_snapshot = Vec::new().into();
            return;
        }
        let expanded = self.state.borrow().expanded.clone();
        self.pending_rebuild = Some(cx.spawn(async move |this, cx| {
            // 收集任务需在 UI 线程创建（读取 Project 实体），完成后回 UI 线程应用。
            let Ok(collect) = this.update(cx, |panel, cx| {
                panel.project.read(cx).collect_visible_rows(expanded, cx)
            }) else {
                return;
            };
            let entries = collect.await;
            let _ = this.update(cx, |panel, cx| panel.apply_rebuild(generation, entries, cx));
        }));
    }

    /// 应用后台收集结果：过期代次直接丢弃；转换行模型、批量回填 git 状态并刷新行快照。
    fn apply_rebuild(
        &mut self,
        generation: u64,
        entries: Vec<WorktreeEntry>,
        cx: &mut Context<Self>,
    ) {
        if generation != self.rebuild_generation {
            // 快速连续展开/折叠期间已有新重建派发，过期结果不落地。
            return;
        }
        let Some(root) = self.root.clone() else {
            return;
        };
        let expanded = self.state.borrow().expanded.clone();
        let mut rows: Vec<ProjectTreeRow> = entries
            .into_iter()
            .map(|entry| {
                // 深度 = 相对根的路径组件数（与递归收集的逐层 +1 等价）。
                let depth = entry
                    .path
                    .strip_prefix(&root)
                    .map_or(0, |relative| relative.components().count());
                ProjectTreeRow {
                    expanded: entry.is_dir && expanded.contains(&entry.path),
                    path: entry.path,
                    name: entry.name,
                    depth,
                    is_dir: entry.is_dir,
                    is_new: false,
                    git_status: None,
                }
            })
            .collect();
        // git 状态批量回填（UI 线程一次查询）：目录行聚合、文件行精确；根行保持无状态（既有配色行为）。
        let queries: Vec<(PathBuf, bool)> = rows
            .iter()
            .filter(|row| row.path != root)
            .map(|row| (row.path.clone(), row.is_dir))
            .collect();
        let statuses = self.project.read(cx).git_statuses_for_rows(&queries, cx);
        for row in &mut rows {
            row.git_status = statuses.get(&row.path).cloned();
        }
        self.state.borrow_mut().replace_rows(rows);
        // 行集已全量替换：同步重建行快照，此后每帧渲染只做 Rc 克隆。
        self.row_snapshot = self.state.borrow().rows.clone().into();
        self.try_scroll_pending_reveal();
        cx.notify();
    }

    /// 消费待定 reveal：目标行已进入行模型时滚动到可见并清除标记。
    fn try_scroll_pending_reveal(&mut self) {
        let Some(path) = self.pending_reveal.clone() else {
            return;
        };
        let index = self
            .state
            .borrow()
            .rows
            .iter()
            .position(|row| row.path == path);
        if let Some(index) = index {
            self.pending_reveal = None;
            self.scroll_handle
                .scroll_to_item(index, ScrollStrategy::Center);
        }
    }

    /// 从 git 状态刷新行的忽略/颜色信息（git 事件驱动，不重扫目录）。
    fn refresh_git_statuses(&mut self, cx: &mut Context<Self>) {
        let entries: Vec<(PathBuf, bool)> = self
            .state
            .borrow()
            .rows
            .iter()
            .map(|row| (row.path.clone(), row.is_dir))
            .collect();
        let statuses = self.project.read(cx).git_statuses_for_rows(&entries, cx);
        {
            let mut state = self.state.borrow_mut();
            for row in &mut state.rows {
                row.git_status = statuses.get(&row.path).cloned();
            }
        }
        // 行内容已变更：行快照必须同步重建，否则颜色更新不进渲染。
        self.row_snapshot = self.state.borrow().rows.clone().into();
        cx.notify();
    }

    /// 重命名后迁移树状态（根/展开/选中/活动路径）。行模型重建由调用方负责：
    /// 单个重命名立即重建；批量移动（execute_move）先逐对迁移、完成后统一重建一次，避免 N 项移动派发 N 次后台收集。
    fn migrate_rename(&mut self, from: &Path, to: &Path) {
        let Some(root) = self.root.take() else {
            return;
        };
        self.root = Some(translate_path(&root, from, to));
        let mut state = self.state.borrow_mut();
        state.expanded = state
            .expanded
            .drain()
            .map(|path| translate_path(&path, from, to))
            .collect();
        state.selected = state
            .selected
            .take()
            .map(|path| translate_path(&path, from, to));
        state.selected_set = state
            .selected_set
            .drain()
            .map(|path| translate_path(&path, from, to))
            .collect();
        state.anchor = state
            .anchor
            .take()
            .map(|path| translate_path(&path, from, to));
        self.active_path = self
            .active_path
            .take()
            .map(|path| translate_path(&path, from, to));
        drop(state);
    }

    /// 重命名后迁移树状态并重建行模型。
    fn apply_rename(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        self.migrate_rename(from, to);
        self.rebuild_rows(cx);
    }

    /// 设置打开文件的回调（由 Workspace 在创建后调用）。
    pub fn set_on_open_file(&mut self, callback: OnOpenFile) {
        self.on_open_file = Some(callback);
    }

    /// 设置重命名回调（由 Workspace 在创建后调用）。
    pub fn set_on_rename(&mut self, callback: OnRename) {
        self.on_rename = Some(callback);
    }

    /// 设置新建条目回调（由 Workspace 在创建后调用）。
    pub fn set_on_create(&mut self, callback: OnCreate) {
        self.on_create = Some(callback);
    }

    /// 设置删除（移到废纸篓）回调（由 Workspace 在创建后调用）。
    pub fn set_on_trash(&mut self, callback: OnTrash) {
        self.on_trash = Some(callback);
    }

    /// 设置移动回调（由 Workspace 在创建后调用）。
    pub fn set_on_move(&mut self, callback: OnMove) {
        self.on_move = Some(callback);
    }

    /// 更换项目根目录（项目根被外部重命名时由 Workspace 调用）。
    pub fn set_root(&mut self, root: PathBuf, cx: &mut Context<Self>) {
        if self.root.as_ref() == Some(&root) {
            return;
        }
        self.root = Some(root.clone());
        let mut state = self.state.borrow_mut();
        state.expanded.clear();
        state.expanded.insert(root);
        state.selected = None;
        state.selected_set.clear();
        state.anchor = None;
        self.active_path = None;
        drop(state);
        // 行集合全变：重建时现查 git 状态，无需单独补齐。
        self.rebuild_rows(cx);
        cx.notify();
    }

    /// 刷新行模型；同时从设置读取最新的扫描排除名单并重建过滤规则。
    pub fn refresh(&mut self, cx: &mut Context<Self>) {
        let exclusions = SettingsStore::file_scan_exclusions(cx);
        self.project
            .update(cx, |project, _| project.set_exclusions(&exclusions));
        self.rebuild_rows(cx);
        cx.notify();
    }

    /// 防抖刷新：120ms 内的连续 EntriesChanged（批量文件操作的事件风暴）合并为一次重建。
    ///
    /// 新调度直接覆盖旧任务；到期后校验会话代次未变才执行刷新，避免旧任务双重重建。
    pub fn schedule_refresh(&mut self, cx: &mut Context<Self>) {
        self.refresh_generation += 1;
        let generation = self.refresh_generation;
        let timer = cx.background_executor().timer(REFRESH_DEBOUNCE);
        self.refresh_task = Some(cx.spawn(
            move |this: WeakEntity<Self>, asynccx: &mut AsyncApp| {
                let mut cx = asynccx.clone();
                async move {
                    timer.await;
                    let _ = this.update(&mut cx, |tree, cx| {
                        if tree.refresh_generation == generation {
                            tree.refresh(cx);
                        }
                    });
                }
            },
        ));
    }

    /// 将活动文件标记，并在它不在视口内时滚动到可见区域。
    ///
    /// 展开祖先与选中同步完成；行重建异步进行，目标行进入行模型后完成滚动（快速连续 reveal 以最后一次为准，行已可见时立即滚动不等重建）。
    pub fn reveal_active_path(&mut self, path: Option<PathBuf>, cx: &mut Context<Self>) {
        let Some(root) = self.root.clone() else {
            self.active_path = None;
            self.pending_reveal = None;
            return;
        };
        let Some(path) = path.filter(|path| path.starts_with(&root)) else {
            self.active_path = None;
            self.pending_reveal = None;
            return;
        };
        {
            let mut state = self.state.borrow_mut();
            let mut ancestor = path.parent();
            while let Some(directory) = ancestor.filter(|directory| directory.starts_with(&root)) {
                state.expanded.insert(directory.to_path_buf());
                if directory == root {
                    break;
                }
                ancestor = directory.parent();
            }
            self.active_path = Some(path.to_path_buf());
            state.select(path.to_path_buf());
        }
        self.pending_reveal = Some(path);
        self.rebuild_rows(cx);
        self.try_scroll_pending_reveal();
        cx.notify();
    }

    /// 保持键盘选中项可见；仍在视口内时不改变当前滚动位置。
    fn scroll_to_selection(&self) {
        if let Some(index) = self.state.borrow().selected_idx() {
            self.scroll_handle
                .scroll_to_item(index, ScrollStrategy::Center);
        }
    }

    /// 渲染行：普通态直接 Rc 克隆行快照（每帧零深拷贝）；
    /// 仅 Create 编辑态克隆快照插入临时行（低频交互，允许克隆）。
    fn dispatch_context(&self, window: &Window, cx: &Context<Self>) -> KeyContext {
        let mut context = KeyContext::new_with_defaults();
        context.add("ProjectTree");
        context.add(
            if self
                .entry_name_editor
                .read(cx)
                .focus_handle()
                .is_focused(window)
            {
                "editing"
            } else {
                "not_editing"
            },
        );
        // 冲突会话进行中：附加专用上下文，供 escape 取消绑定命中。
        if self.conflict.is_some() {
            context.add("conflict");
        }
        context
    }

    fn activate_selected(
        &mut self,
        focus_opened_item: bool,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let (path, is_dir) = {
            let state = self.state.borrow();
            match state.selected_idx() {
                Some(idx) => (Some(state.rows[idx].path.clone()), state.rows[idx].is_dir),
                None => (None, false),
            }
        };
        let Some(path) = path else {
            return;
        };
        // 游标已在本行时才走到这里（mouse_down 已收拢/保持）；
        // 切勿再调 select()——它会清空多选集合，使集合内行的激活把拖拽载荷打回单项。
        if is_dir {
            self.state.borrow_mut().toggle_expand(&path);
            self.rebuild_rows(cx);
        } else if let Some(callback) = self.on_open_file.clone() {
            callback(path, focus_opened_item, window, cx);
        }
        window.refresh();
    }

    /// 激活选中行（打开文件并聚焦编辑器）。键盘 enter 与鼠标双击走这里。
    fn handle_tree_activate(
        &mut self,
        _: &TreeActivate,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.activate_selected(true, window, cx);
    }
}

impl gpui::Render for ProjectTreePanel {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        self.state.borrow_mut().ensure_selected();
        let content = if self.root.is_none() {
            render_empty_state(cx).into_any_element()
        } else {
            let display_rows = self.display_rows(cx);
            let len = display_rows.len();
            let is_focused = self.focus.contains_focused(window, cx);
            // 剪切剪贴板路径快照：命中行淡显（Copy 无淡显）。
            let clipboard_cut: Rc<[PathBuf]> = match &self.clipboard {
                Some(TreeClipboard::Cut(paths)) => paths.clone().into(),
                _ => Vec::new().into(),
            };
            let render_context = ProjectTreeRenderContext {
                state: Rc::clone(&self.state),
                rows: display_rows,
                focus: self.focus.clone(),
                weak: cx.weak_entity(),
                edit_state: self.edit_state.clone(),
                entry_name_editor: self.entry_name_editor.clone(),
                active_path: self.active_path.clone(),
                clipboard_cut,
                // 渲染期由 render_list 按行序重建（多选标记快照）。
                drag_marked: Vec::new().into(),
                drag_blocked: self.edit_state.is_some() || self.conflict.is_some(),
            };
            render_list(
                &self.scroll_handle,
                &self.scrollbar,
                len,
                is_focused,
                render_context,
            )
            .into_any_element()
        };

        // 复制进度：面板底部 2px 细条（宽度为完成百分比）+ 「复制中 n/N」状态文本。
        let progress = self.active_transfer.map(|(done, total)| {
            let fraction = if total == 0 {
                1.
            } else {
                done as f32 / total as f32
            };
            div()
                .absolute()
                .bottom_0()
                .left_0()
                .w_full()
                // 进度文本：小字号、次要色，悬浮于进度条上方右侧（与面板状态提示风格一致）。
                .child(
                    div()
                        .absolute()
                        .bottom(px(4.))
                        .right(px(6.))
                        .text_size(px(11.))
                        .text_color(color::current(cx).text_muted)
                        .child(format!("复制中 {done}/{total}")),
                )
                .child(
                    div()
                        .absolute()
                        .bottom_0()
                        .left_0()
                        .h(px(2.))
                        .w(relative(fraction))
                        .bg(color::current(cx).border_focused),
                )
        });

        // 冲突确认浮层：会话进行中覆盖渲染，按钮回调直连面板方法（数据型交互，weak 升级防悬挂）。
        let weak = cx.weak_entity();
        let conflict_overlay = self
            .conflict
            .as_ref()
            .filter(|session| !session.is_empty())
            .and_then(|session| {
                session
                    .current_conflict()
                    .map(|(source, _)| (session, source))
            })
            .map(|(session, source)| {
                let name = source
                    .file_name()
                    .map(|name| name.to_string_lossy().to_string())
                    .unwrap_or_else(|| source.display().to_string());
                let index = session.decisions().len() + 1;
                let weak = weak.clone();
                ConfirmOverlay::new("tree-conflict", format!("目标已存在：{name}"))
                    .detail(format!("第 {index}/{} 项", session.len()))
                    .confirm_label("覆盖")
                    .skip_label("跳过")
                    .cancel_label("取消")
                    .on_answer(Rc::new(move |answer, _window, cx| {
                        if let Some(tree) = weak.upgrade() {
                            tree.update(cx, |tree, cx| tree.resolve_conflict(answer, cx));
                        }
                    }))
            });

        div()
            .size_full()
            .relative()
            .track_focus(&self.focus)
            .key_context(self.dispatch_context(window, cx))
            .tab_index(0)
            .on_action(cx.listener(Self::handle_tree_select_prev))
            .on_action(cx.listener(Self::handle_tree_select_next))
            .on_action(cx.listener(Self::handle_tree_select_prev_extend))
            .on_action(cx.listener(Self::handle_tree_select_next_extend))
            .on_action(cx.listener(Self::handle_tree_collapse))
            .on_action(cx.listener(Self::handle_tree_expand))
            .on_action(cx.listener(Self::handle_tree_activate))
            .on_action(cx.listener(Self::handle_tree_rename))
            .on_action(cx.listener(Self::handle_tree_new_entry))
            .on_action(cx.listener(Self::handle_tree_trash))
            .on_action(cx.listener(Self::handle_tree_copy))
            .on_action(cx.listener(Self::handle_tree_cut))
            .on_action(cx.listener(Self::handle_tree_paste))
            .on_action(cx.listener(Self::handle_tree_clear_clipboard))
            .on_action(cx.listener(Self::handle_tree_cancel_conflict))
            .on_action(cx.listener(Self::handle_tree_confirm_edit))
            .on_action(cx.listener(Self::handle_tree_cancel_edit))
            // 拖拽悬停离开行区域（列表空白处）：清掉待定的悬停展开计时。
            // on_drag_move 在捕获阶段对全部注册元素派发且不做命中检测，需自行校验光标位于本容器内；容器先于各行注册、先于各行执行，行级处理器随后按命中结果覆盖（悬停行重新调度计时）。
            .on_drag_move::<TreeDrag>({
                let weak = cx.weak_entity();
                move |drag_event, _window, cx| {
                    if !drag_event.bounds.contains(&drag_event.event.position) {
                        return;
                    }
                    if let Some(tree) = weak.upgrade() {
                        tree.update(cx, |tree, _| tree.reset_drag_hover());
                    }
                }
            })
            // 拖拽取消（在全部落点行之外释放鼠标）：drop 监听未命中时事件冒泡到根节点，此刻 active_drag 尚未被框架清除，据此识别取消并清理悬停状态。
            .on_mouse_up(MouseButton::Left, {
                let weak = cx.weak_entity();
                move |_event, _window, cx| {
                    if cx.has_active_drag()
                        && let Some(tree) = weak.upgrade()
                    {
                        tree.update(cx, |tree, _| tree.reset_drag_hover());
                    }
                }
            })
            .child(content)
            .children(progress)
            .children(conflict_overlay)
    }
}

/// 无 worktree 的空态提示。
impl EventEmitter<PanelEvent> for ProjectTreePanel {}

impl Panel for ProjectTreePanel {
    fn icon() -> &'static str {
        "icons/file_tree.svg"
    }
    fn label() -> &'static str {
        "项目树"
    }
    fn persistent_name() -> &'static str {
        "project-tree"
    }
    fn focus_handle(&self, _cx: &gpui::App) -> gpui::FocusHandle {
        self.focus.clone()
    }
}

// ── 内部类型 ────────────────────────────────────────────────────────

#[derive(Clone)]
struct ProjectTreeRow {
    path: PathBuf,
    name: String,
    depth: usize,
    is_dir: bool,
    expanded: bool,
    is_new: bool,
    /// git 状态（决定文件名颜色与忽略淡显；None 表示无状态）。
    git_status: Option<FileStatus>,
}

impl TreeRow for ProjectTreeRow {
    fn is_dir(&self) -> bool {
        self.is_dir
    }
    fn depth(&self) -> usize {
        self.depth
    }
    fn expanded(&self) -> bool {
        self.expanded
    }
}
