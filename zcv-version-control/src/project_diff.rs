//! 已暂存/未暂存变更的多文件编辑器。
//!
//! GitStore 的状态快照决定文件集合，Project/BufferStore 继续拥有真实文件文档，MultiBuffer 只组合这些文档。
//! 点击版本管理条目时按分组复用对应 Item 并定位文件，不为 Git 状态建立界面侧副本。

use std::any::TypeId;
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    AnyElement, AnyEntity, App, Context, Entity, EventEmitter, FocusHandle, Focusable,
    ParentElement, Render, SharedString, Styled, Subscription, Task, WeakEntity, Window, div,
    prelude::*,
};
use zcv_buffer_diff::{BufferDiff, BufferDiffInput};
use zcv_editor::{DiffHunkDelegate, Editor, EditorEvent, EditorHunk, HunkControlTarget};
use zcv_git::{
    ConflictChoice, FileStatus, GitHunkOperation, GitRevision, StatusCode, parse_conflict_regions,
};
use zcv_multi_buffer::{DiffFile, DiffHunkSource, DisplayHunk};
use zcv_multi_buffer::{ExcerptLocation, ExcerptRange, MultiBuffer};
use zcv_path::AbsolutePathBuf;
use zcv_project::{GitStoreEvent, Project};
use zcv_search::{SearchBar, SearchBarConfig, SearchBarSlots};
use zcv_text::{Anchor, BufferId, ByteOffset, Snapshot, TextRange};
use zcv_theme::{color, space};
use zcv_ui::{Button, ButtonSize, ButtonStyle, Checkbox, SvgIcon};
use zcv_workspace::{
    Item, ItemEvent, ItemHandle, SearchableItemHandle, SerializedItemProvider, SerializedPaneItem,
    ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView, Workspace,
};

const PROJECT_DIFF_SERIALIZED_KIND: &str = "project-diff";

#[derive(Clone)]
struct GitChangeFile {
    path: PathBuf,
    status: FileStatus,
}

struct ProjectDiffHunkDelegate {
    view: WeakEntity<ProjectDiffView>,
}

impl ProjectDiffHunkDelegate {
    fn render_conflict_controls(
        &self,
        block: &EditorHunk,
        row: usize,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let (path, conflict_index) = block.id.rsplit_once('\n')?;
        let conflict_index = conflict_index.parse::<usize>().ok()?;
        let path = PathBuf::from(path);
        let colors = *color::current(cx);
        let controls = div()
            .flex()
            .items_center()
            .gap(space::S2)
            .rounded_md()
            .overflow_hidden()
            .border_1()
            .border_color(colors.border_variant)
            .bg(colors.editor_background);
        let make_button = |id: String, label: &'static str, choice: ConflictChoice| {
            let view_for_click = self.view.clone();
            let path = path.clone();
            Button::text(id, label)
                .size(ButtonSize::Medium)
                .on_click(move |_, _, cx| {
                    if let Some(view) = view_for_click.upgrade() {
                        view.update(cx, |view, cx| {
                            view.resolve_conflict_for_path(&path, conflict_index, choice, cx)
                        });
                    }
                })
                .into_any_element()
        };
        Some(
            controls
                .child(make_button(
                    format!("project-conflict-ours-{row}"),
                    "保留当前内容",
                    ConflictChoice::Ours,
                ))
                .child(make_button(
                    format!("project-conflict-theirs-{row}"),
                    "保留传入内容",
                    ConflictChoice::Theirs,
                ))
                .child(make_button(
                    format!("project-conflict-both-{row}"),
                    "保留双方内容",
                    ConflictChoice::Both,
                ))
                .into_any_element(),
        )
    }
}

impl DiffHunkDelegate for ProjectDiffHunkDelegate {
    fn render_hunk_controls(
        &self,
        target: &HunkControlTarget,
        row: usize,
        _editor: &Entity<Editor>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let HunkControlTarget::Editor(hunk) = target else {
            let HunkControlTarget::Diff(diff) = target else {
                return None;
            };
            return Some(self.render_diff_hunk_controls(row, diff, cx));
        };
        self.render_conflict_controls(hunk, row, cx)
    }

    fn render_buffer_header_controls(
        &self,
        path: &Path,
        sticky: bool,
        row: usize,
        _editor: &Entity<Editor>,
        _window: &mut Window,
        cx: &mut App,
    ) -> Option<AnyElement> {
        let view = self.view.upgrade()?;
        let (kind, is_dirty) = {
            let view = view.read(cx);
            view.files.iter().find(|file| file.path == path)?;
            (
                view.kind,
                view.multi_buffer.read(cx).is_diff_file_dirty(path, cx),
            )
        };
        if is_dirty {
            return Some(
                SvgIcon::new("icons/circle.svg")
                    .color(color::current(cx).icon_accent)
                    .label("未保存修改")
                    .into_any_element(),
            );
        }
        if kind == ProjectDiffKind::Conflict {
            return None;
        }
        let checked = kind == ProjectDiffKind::Staged;
        let view_for_click = self.view.clone();
        let path = path.to_path_buf();
        Some(
            Checkbox::new(
                format!(
                    "project-diff-header-staged-{sticky}-{row}-{}",
                    path.display()
                ),
                checked,
            )
            .tooltip(if checked { "取消暂存" } else { "暂存" })
            .on_click(move |_window, cx| {
                if let Some(view) = view_for_click.upgrade() {
                    view.update(cx, |view, cx| {
                        let operation = if checked {
                            GitHunkOperation::Unstage
                        } else {
                            GitHunkOperation::Stage
                        };
                        view.toggle_file_staged(&path, operation, cx);
                    });
                }
            })
            .into_any_element(),
        )
    }
}

impl ProjectDiffHunkDelegate {
    fn render_diff_hunk_controls(
        &self,
        row: usize,
        hunk: &DisplayHunk,
        cx: &mut App,
    ) -> AnyElement {
        let Some(view) = self.view.upgrade() else {
            return div().into_any_element();
        };
        let (kind, hunk_source, is_created_file) = {
            let view = view.read(cx);
            let Some(info) = view.diff_hunk_source_info(hunk, cx) else {
                return div().into_any_element();
            };
            let is_created = view
                .files
                .iter()
                .find(|file| file.path == info.path)
                .is_some_and(|file| view.kind.is_created(file.status));
            (view.kind, info, is_created)
        };
        let colors = *color::current(cx);
        let controls = div()
            .flex()
            .items_center()
            .gap(space::S2)
            .rounded_md()
            .overflow_hidden()
            .border_1()
            .border_color(colors.border_variant)
            .bg(colors.editor_background);

        match kind {
            ProjectDiffKind::Conflict => div().into_any_element(),
            ProjectDiffKind::Unstaged => {
                let stage_view = self.view.clone();
                let stage_hunk = hunk_source.clone();
                let restore_view = self.view.clone();
                let restore_hunk = hunk_source;
                controls
                    .child(
                        Button::text(("stage-hunk", row), "暂存")
                            .size(ButtonSize::Medium)
                            .label("暂存此变更块")
                            .on_click(move |_event, _window, cx| {
                                let _ = stage_view.update(cx, |view, cx| {
                                    if let Err(error) = view.apply_hunk_action(
                                        stage_hunk.clone(),
                                        GitHunkOperation::Stage,
                                        cx,
                                    ) {
                                        cx.emit(EditorEvent::Error(error));
                                    }
                                });
                            }),
                    )
                    .child(
                        Button::text(("restore-hunk", row), "重做")
                            .size(ButtonSize::Medium)
                            .label(if is_created_file {
                                "新建文件不能重做单个变更块"
                            } else {
                                "用暂存区内容重做此变更块"
                            })
                            .disabled(is_created_file)
                            .on_click(move |_event, _window, cx| {
                                let _ = restore_view.update(cx, |view, cx| {
                                    if let Err(error) = view.apply_hunk_action(
                                        restore_hunk.clone(),
                                        GitHunkOperation::Restore,
                                        cx,
                                    ) {
                                        cx.emit(EditorEvent::Error(error));
                                    }
                                });
                            }),
                    )
                    .into_any_element()
            }
            ProjectDiffKind::Staged => {
                let unstage_view = self.view.clone();
                let unstage_hunk = hunk_source;
                controls
                    .child(
                        Button::text(("unstage-hunk", row), "取消暂存")
                            .size(ButtonSize::Medium)
                            .label("取消暂存此变更块")
                            .on_click(move |_event, _window, cx| {
                                let _ = unstage_view.update(cx, |view, cx| {
                                    if let Err(error) = view.apply_hunk_action(
                                        unstage_hunk.clone(),
                                        GitHunkOperation::Unstage,
                                        cx,
                                    ) {
                                        cx.emit(EditorEvent::Error(error));
                                    }
                                });
                            }),
                    )
                    .into_any_element()
            }
        }
    }
}

/// 版本管理面板分组对应的比较范围。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectDiffKind {
    Staged,
    Unstaged,
    Conflict,
}

impl ProjectDiffKind {
    fn base_revision(self) -> GitRevision {
        match self {
            Self::Staged => GitRevision::Head,
            Self::Unstaged => GitRevision::Index,
            Self::Conflict => GitRevision::Index,
        }
    }

    pub(crate) fn includes(self, status: FileStatus) -> bool {
        match self {
            Self::Staged => status.has_staged(),
            Self::Unstaged => status.has_unstaged(),
            Self::Conflict => matches!(status, FileStatus::Unmerged),
        }
    }

    fn is_created(self, status: FileStatus) -> bool {
        matches!(
            (self, status),
            (Self::Unstaged, FileStatus::Untracked)
                | (
                    Self::Staged,
                    FileStatus::Tracked {
                        index_status: StatusCode::Added,
                        ..
                    },
                )
                | (
                    Self::Unstaged,
                    FileStatus::Tracked {
                        worktree_status: StatusCode::Added,
                        ..
                    },
                )
        )
    }

    fn is_deleted(self, status: FileStatus) -> bool {
        matches!(
            (self, status),
            (
                Self::Staged,
                FileStatus::Tracked {
                    index_status: StatusCode::Deleted,
                    ..
                },
            ) | (
                Self::Unstaged,
                FileStatus::Tracked {
                    worktree_status: StatusCode::Deleted,
                    ..
                },
            )
        )
    }

    fn title(self) -> &'static str {
        match self {
            Self::Staged => "已暂存更改",
            Self::Unstaged => "未暂存更改",
            Self::Conflict => "冲突解决",
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Staged => "icons/lock.svg",
            Self::Unstaged => "icons/diff.svg",
            Self::Conflict => "icons/warning.svg",
        }
    }

    fn serialized_name(self) -> &'static str {
        match self {
            Self::Staged => "staged",
            Self::Unstaged => "unstaged",
            Self::Conflict => "conflict",
        }
    }

    fn from_serialized_name(name: &str) -> Option<Self> {
        match name {
            "staged" => Some(Self::Staged),
            "unstaged" => Some(Self::Unstaged),
            "conflict" => Some(Self::Conflict),
            _ => None,
        }
    }
}

/// 每个 Git 变更块（hunk）上下各保留多少行未修改的上下文。
const DIFF_CONTEXT_LINES: usize = 2;

pub struct ProjectDiffView {
    kind: ProjectDiffKind,
    project: Entity<Project>,
    empty_focus: FocusHandle,
    editor: Entity<Editor>,
    multi_buffer: Entity<MultiBuffer>,
    files: Vec<GitChangeFile>,
    /// base 变更（HEAD 变化）后需要整体重建投影。
    rebase_projection: bool,
    pending_path: Option<PathBuf>,
    loading_revision_text: HashSet<(GitRevision, PathBuf)>,
    /// 共享搜索栏会话：查询、匹配选项、可见性与替换开关由它唯一持有。
    search_bar: Entity<SearchBar>,
    _subscriptions: Vec<Subscription>,
}

/// 项目差异的工具栏视图。
///
/// 作为 Pane 工具项存在：活动 Item 是本差异视图时显示搜索栏，否则隐藏；
/// 搜索目标解析为差异视图暴露的内层编辑器。
pub(crate) struct ProjectDiffToolbar {
    active_view: Option<Entity<ProjectDiffView>>,
    search_bar: Option<Entity<SearchBar>>,
}

impl ProjectDiffToolbar {
    pub(crate) fn new() -> Self {
        Self {
            active_view: None,
            search_bar: None,
        }
    }
}

impl EventEmitter<ToolbarItemEvent> for ProjectDiffToolbar {}

impl ToolbarItemView for ProjectDiffToolbar {
    fn set_active_pane_item(
        &mut self,
        item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        self.active_view = item.and_then(|item| item.act_as::<ProjectDiffView>(cx));
        let Some(view) = self.active_view.clone() else {
            if let Some(bar) = self.search_bar.take() {
                bar.update(cx, |bar, cx| bar.set_target(None, window, cx));
            }
            return ToolbarItemLocation::Hidden;
        };
        let bar = view.read(cx).search_bar.clone();
        // 切换差异视图（暂存/未暂存/冲突）时先清除旧栏目标，搜索会话仍归各视图所有。
        if let Some(previous) = self.search_bar.replace(bar.clone())
            && previous.entity_id() != bar.entity_id()
        {
            previous.update(cx, |bar, cx| bar.set_target(None, window, cx));
        }
        // 差异搜索的目标是内层编辑器：差异视图把它经 as_searchable 暴露出来。
        // 搜索栏保存弱目标，内层编辑器释放后搜索自然停止。
        let target = item
            .and_then(|item| item.as_searchable(cx))
            .map(|handle| handle.downgrade());
        bar.update(cx, |bar, cx| bar.set_target(target, window, cx));
        ToolbarItemLocation::PrimaryRight
    }
}

impl Render for ProjectDiffToolbar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(view) = self.active_view.clone() else {
            return div().into_any_element();
        };
        let Some(search_bar) = self.search_bar.clone() else {
            return div().into_any_element();
        };
        // 折叠全部文件：宿主领域逻辑，作为搜索栏左侧插槽。
        let leading = {
            let weak = view.downgrade();
            let editor = view.read(cx).editor.clone();
            let multi_buffer = view.read(cx).multi_buffer.clone();
            let buffer_ids = multi_buffer.update(cx, |buffer, cx| {
                let snapshot = buffer.snapshot(cx);
                snapshot
                    .excerpts()
                    .map(|excerpt| excerpt.buffer_id())
                    .collect::<Vec<_>>()
            });
            let expanded = buffer_ids
                .iter()
                .any(|buffer_id| !editor.read(cx).is_buffer_folded(*buffer_id, cx));
            Button::icon(
                "project-diff-expansion",
                if expanded {
                    "icons/chevron_down_up.svg"
                } else {
                    "icons/chevron_up_down.svg"
                },
            )
            .label(if expanded {
                "折叠全部文件"
            } else {
                "展开全部文件"
            })
            .on_click(move |_, _, cx| {
                if let Some(view) = weak.upgrade() {
                    view.update(cx, |view, cx| view.set_all_files_folded(expanded, cx));
                }
            })
            .into_any_element()
        };
        // 重做全部仅未暂存差异可用：作为搜索栏计数之后的宿主插槽。
        let external = (view.read(cx).kind == ProjectDiffKind::Unstaged).then(|| {
            let weak = view.downgrade();
            Button::text("project-diff-operation", "重做全部")
                .size(ButtonSize::Loose)
                .style(ButtonStyle::Solid)
                .label("双击还原所有未暂存修改")
                .on_click(move |event, _, cx| {
                    // 危险操作：仅鼠标双击确认，单击不生效。
                    if !matches!(event, gpui::ClickEvent::Mouse(event) if event.down.click_count >= 2)
                    {
                        return;
                    }
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |view, cx| view.restore_all(cx));
                    }
                })
                .into_any_element()
        });
        let slots = SearchBarSlots {
            leading: Some(leading),
            external: external.into_iter().collect(),
        };
        search_bar
            .update(cx, |bar, cx| bar.render(slots, window, cx))
            .into_any_element()
    }
}

impl ProjectDiffView {
    fn resolve_conflict_for_path(
        &mut self,
        path: &Path,
        conflict_index: usize,
        choice: ConflictChoice,
        cx: &mut Context<Self>,
    ) {
        let result = self.project.update(cx, |project, cx| {
            project.resolve_conflict(path, conflict_index, choice, cx)
        });
        if let Err(error) = result {
            cx.emit(EditorEvent::Error(format!("保存冲突解决结果失败：{error}")));
            return;
        }
        self.rebuild_conflict_projection(cx);
        cx.notify();
    }

    fn set_all_files_folded(&mut self, folded: bool, cx: &mut Context<Self>) {
        let buffer_ids = self.file_buffer_ids(cx);
        self.editor.update(cx, |editor, cx| {
            for buffer_id in buffer_ids {
                if editor.is_buffer_folded(buffer_id, cx) != folded {
                    editor.toggle_buffer_fold(buffer_id, cx);
                }
            }
        });
    }

    /// 当前投影中每个文件的显示实体身份。
    ///
    /// 折叠集合按 BufferId 归属，与 BlockMap 的分类使用同一身份；
    /// 一个文件的所有 excerpt（含 diff 旧侧）共享同一 id，因此只需要去重后的集合。
    fn file_buffer_ids(&self, cx: &mut App) -> Vec<BufferId> {
        let snapshot = self
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let mut buffer_ids = Vec::new();
        for excerpt in snapshot.excerpts() {
            let buffer_id = excerpt.buffer_id();
            if !buffer_ids.contains(&buffer_id) {
                buffer_ids.push(buffer_id);
            }
        }
        buffer_ids
    }

    /// 重做全部文件：先按文件聚合、解析出全部源 hunk，再逐个文件一次性提交。
    ///
    /// 逐个 hunk 应用会因投影重建让后续 hunk 的显示坐标失配，且同一文件的 pending 会互相覆盖，结果只重做了第一个文件的一部分。
    fn restore_all(&mut self, cx: &mut Context<Self>) {
        let mut grouped: Vec<(Entity<BufferDiff>, Vec<std::ops::Range<Anchor>>)> = Vec::new();
        for hunk in self.editor.read(cx).diff_hunks(cx).to_vec() {
            let Some(info) = self.diff_hunk_source_info(&hunk, cx) else {
                continue;
            };
            // 新增文件没有可还原的旧侧内容，重做会清空文件。
            if info.diff.read(cx).is_created() {
                continue;
            }
            let Some(range) = info.range else {
                continue;
            };
            match grouped
                .iter_mut()
                .find(|(diff, _)| diff.entity_id() == info.diff.entity_id())
            {
                Some((_, ranges)) => ranges.push(range),
                None => grouped.push((info.diff, vec![range])),
            }
        }
        for (diff, ranges) in grouped {
            let Some(operations) = diff.read(cx).operations() else {
                continue;
            };
            if operations.supports_restore() {
                operations.restore(diff, ranges, cx);
            }
        }
    }

    fn new(kind: ProjectDiffKind, project: Entity<Project>, cx: &mut Context<Self>) -> Self {
        let weak_view = cx.weak_entity();
        let empty_focus = cx.focus_handle();
        let multi_buffer = match kind {
            ProjectDiffKind::Staged => cx.new(MultiBuffer::empty_read_only),
            ProjectDiffKind::Unstaged => cx.new(MultiBuffer::empty),
            ProjectDiffKind::Conflict => cx.new(MultiBuffer::empty),
        };
        let editor = cx.new(|cx| {
            let mut editor = Editor::for_multi_buffer(multi_buffer.clone(), cx);
            editor.set_diff_hunks_expanded_by_default(true, cx);
            editor.set_diff_hunk_delegate(
                Some(Arc::new(ProjectDiffHunkDelegate {
                    view: weak_view.clone(),
                })),
                cx,
            );
            editor
        });
        let git_store = project.read(cx).git_store();
        let subscriptions = vec![
            cx.observe(&editor, |_, _, cx| cx.notify()),
            // diff hunk 在后台完成后由 MultiBuffer 重新物化 excerpts；
            // 此时重试待定位路径，避免首次点击只能停在默认的第一个文件，第二次点击才生效。
            cx.observe(&multi_buffer, |view, _, cx| view.apply_pending_path(cx)),
            cx.subscribe(&editor, |_, _, event: &EditorEvent, cx| {
                cx.emit(event.clone());
            }),
            // 展开状态变化由 MultiBuffer 自己按文件重物化并推显示链；视图只需跟随重绘，不再整体重建投影。
            cx.subscribe(&editor, |_view, _, event: &EditorEvent, cx| match event {
                EditorEvent::DiffHunksExpandedChanged => cx.notify(),
                EditorEvent::Edited { .. }
                | EditorEvent::PathChanged
                | EditorEvent::DirtyChanged
                | EditorEvent::OpenExcerptsRequested { .. }
                | EditorEvent::Error(_) => {}
            }),
            cx.subscribe(&git_store, |view, _, event, cx| match event {
                GitStoreEvent::Repositories | GitStoreEvent::Statuses | GitStoreEvent::Head => {
                    if matches!(event, GitStoreEvent::Head) {
                        view.loading_revision_text.clear();
                        view.rebase_projection = true;
                        // HEAD 变化后旧 hunk 的旧侧坐标空间失效：按默认策略重置展开状态，避免新 diff 按失效的行号误迁移状态。
                        view.editor
                            .update(cx, |editor, cx| editor.reset_diff_hunk_expansion_state(cx));
                    }
                    view.refresh_files(cx);
                }
                GitStoreEvent::HunkOperationFailed(message) => {
                    // 失败：GitStore 已清除 optimistic 状态，这里重建以恢复 hunk 并把错误交给宿主提示。
                    view.rebuild_projection(cx);
                    cx.emit(EditorEvent::Error(format!("变更块操作失败：{message}")));
                }
                GitStoreEvent::IndexText { path } => view.refresh_diff_path(path, cx),
                GitStoreEvent::ActiveRepositoryChanged
                | GitStoreEvent::JobsUpdated
                | GitStoreEvent::Uncommitted(_)
                | GitStoreEvent::UncommitFailed(_) => {}
            }),
        ];
        let mut view = Self {
            kind,
            project,
            empty_focus,
            editor,
            multi_buffer,
            files: Vec::new(),
            rebase_projection: false,
            pending_path: None,
            loading_revision_text: Default::default(),
            search_bar: cx.new(|cx| {
                SearchBar::new(
                    SearchBarConfig {
                        id_prefix: "project-diff",
                        key_context: "ProjectDiffSearchBar",
                        // 仅未暂存差异支持替换。
                        supports_replace: kind == ProjectDiffKind::Unstaged,
                        query_placeholder: "搜索...",
                        replace_placeholder: "替换为...",
                        dismissible: false,
                    },
                    cx,
                )
            }),
            _subscriptions: subscriptions,
        };
        view.refresh_files(cx);
        view
    }

    fn is_empty(&self, _cx: &App) -> bool {
        self.files.is_empty()
    }

    /// 从 GitStore 权威快照重建文件集合；真实内容始终复用 Project 的文档实体。
    fn refresh_files(&mut self, cx: &mut Context<Self>) {
        let git_store = self.project.read(cx).git_store();
        let changed = {
            let store = git_store.read(cx);
            let mut changed = store
                .repositories()
                .flat_map(|(workdir, snapshot)| {
                    snapshot
                        .statuses_by_path
                        .iter()
                        .filter(|(_, entry)| self.kind.includes(entry.status))
                        .map(move |(relative, entry)| GitChangeFile {
                            path: workdir.join(relative.as_path()),
                            status: entry.status,
                        })
                })
                .collect::<Vec<_>>();
            changed.sort_by(|left, right| left.path.cmp(&right.path));
            changed
        };

        self.files = changed;

        if self.kind == ProjectDiffKind::Conflict || std::mem::take(&mut self.rebase_projection) {
            // 冲突视图与 base 变更需要整体重建；普通状态刷新只做路径增量。
            self.rebuild_projection(cx);
        } else {
            self.sync_projection_paths(cx);
        }
        self.load_all_revision_text(cx);
    }

    /// 状态刷新时按路径增量同步投影：
    /// 移除消失文件、追加未挂接的就绪文件；
    /// 未变化的 diff 不重建，按路径更新。
    fn sync_projection_paths(&mut self, cx: &mut Context<Self>) {
        let root = self.project.read(cx).root().map(Path::to_path_buf);
        let visible = self
            .files
            .iter()
            .map(|file| {
                root.as_deref()
                    .and_then(|root| file.path.strip_prefix(root).ok())
                    .unwrap_or(&file.path)
                    .to_path_buf()
            })
            .collect::<HashSet<_>>();
        let attached = self.editor.read(cx).diff_paths(cx);
        for path in attached {
            if !visible.contains(&path) {
                self.editor.update(cx, |editor, cx| {
                    editor.remove_diff(&path, cx);
                });
            }
        }
        self.register_ready_files(cx);
    }

    /// 乐观 index 更新只影响单个路径：只重挂该路径的 diff，其余文件保持不变。
    ///
    /// 该路径的共享 diff 已被 GitStore 失效，这里按新 index 文档重新请求 diff 实体；
    /// 尚未算完时保留旧 excerpts，等 DiffChanged 增量替换。
    fn refresh_diff_path(&mut self, path: &AbsolutePathBuf, cx: &mut Context<Self>) {
        let Some(file) = self
            .files
            .iter()
            .find(|file| file.path.as_path() == path.as_path())
            .cloned()
        else {
            return;
        };
        let root = self.project.read(cx).root().map(Path::to_path_buf);
        let Some(diff_file) = self.build_file_input(&file, root.as_deref(), cx) else {
            return;
        };
        self.editor.update(cx, |editor, cx| {
            editor.add_diff(diff_file, cx);
        });
        self.apply_pending_path(cx);
        cx.notify();
    }

    /// 以 hunk 为核心重建已就绪文件的 excerpts；旧侧与新侧都属于同一个 MultiBuffer 坐标空间。
    /// 修订读取由文件游标推进，已加载文件先进入统一投影，避免把整个暂存区一次性物化。
    fn rebuild_projection(&mut self, cx: &mut Context<Self>) {
        if self.kind == ProjectDiffKind::Conflict {
            self.rebuild_conflict_projection(cx);
            self.apply_pending_path(cx);
            cx.notify();
            return;
        }
        let root = self.project.read(cx).root().map(Path::to_path_buf);
        let mut diff_files = Vec::new();
        let files = self.files.clone();
        for file in &files {
            if !self.revision_requirements_ready(file, cx) {
                continue;
            }
            let Some(diff_file) = self.build_file_input(file, root.as_deref(), cx) else {
                continue;
            };
            diff_files.push(diff_file);
        }
        self.editor.update(cx, |editor, cx| {
            editor.set_editor_hunks(Vec::new(), cx);
            editor.set_diff_files(diff_files, cx)
        });
        self.apply_pending_path(cx);
        cx.notify();
    }

    /// 冲突视图使用完整工作区文本，不创建 BufferDiff；
    /// 冲突块和区域装饰由同一份解析结果注入 Editor。
    fn rebuild_conflict_projection(&mut self, cx: &mut Context<Self>) {
        // 每个文件是一个路径批次：按路径写入，文档顺序由 MultiBuffer 维护。
        let mut groups: Vec<Vec<ExcerptRange>> = Vec::new();
        for file in &self.files {
            let Ok(source) = self
                .project
                .update(cx, |project, cx| project.open_buffer(&file.path, cx))
            else {
                continue;
            };
            let source_len = source.read(cx).text_snapshot().len_bytes();
            let Ok(source_range) = TextRange::new(ByteOffset::ZERO, source_len) else {
                continue;
            };
            groups.push(vec![
                ExcerptRange::new(source, source_range, Vec::new())
                    .with_display_path(file.path.clone()),
            ]);
        }
        self.multi_buffer.update(cx, |buffer, cx| {
            buffer.clear_diffs(cx);
            buffer.clear(cx);
            for group in groups {
                buffer.set_excerpts_for_path(group, cx);
            }
        });
        let hunks = self.conflict_editor_hunks(cx);
        self.editor.update(cx, |editor, cx| {
            editor.set_editor_hunks(hunks, cx);
        });
    }

    fn conflict_editor_hunks(&mut self, cx: &mut Context<Self>) -> Vec<EditorHunk> {
        let snapshot = self
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let mut excerpts = snapshot.excerpts();
        let mut hunks = Vec::new();
        for (buffer, path) in self.multi_buffer.read(cx).file_buffers(cx) {
            let source = buffer.read(cx).text_snapshot();
            let Ok(text_range) = TextRange::new(ByteOffset::ZERO, source.len_bytes()) else {
                continue;
            };
            let Ok(text) = source.slice_text(text_range) else {
                continue;
            };
            let text = text.to_string();
            for (index, region) in parse_conflict_regions(&text).iter().enumerate() {
                let Some(excerpt) = excerpts.find(|excerpt| {
                    excerpt.path() == path
                        && excerpt
                            .source_range()
                            .contains(ByteOffset::new(region.outer.start))
                }) else {
                    continue;
                };
                let output_offset = |offset: usize| {
                    excerpt.output_range().start().get()
                        + offset.saturating_sub(excerpt.source_range().start().get())
                };
                let Some(hunk) = EditorHunk::conflict(
                    format!("{}\n{index}", path.display()),
                    region.outer.clone(),
                    region.theirs.start,
                    output_offset,
                ) else {
                    continue;
                };
                hunks.push(hunk);
            }
        }
        hunks
    }

    /// 构造单个变更文件的统一投影项（预创建 diff 实体 + 显示策略）。
    fn build_file_input(
        &mut self,
        file: &GitChangeFile,
        root: Option<&Path>,
        cx: &mut Context<Self>,
    ) -> Option<DiffFile> {
        let git_store = self.project.read(cx).git_store();
        let working = match self.kind {
            ProjectDiffKind::Staged => git_store
                .read(cx)
                .revision_document(GitRevision::Index, &file.path)?,
            ProjectDiffKind::Unstaged => {
                let opened = self.project.update(cx, |project, cx| {
                    if self.kind.is_deleted(file.status) && !file.path.exists() {
                        project.open_deleted_buffer(&file.path, cx)
                    } else {
                        project.open_buffer(&file.path, cx)
                    }
                });
                let Ok(source) = opened else {
                    cx.emit(EditorEvent::Error(format!(
                        "无法把 Git 变更文件加入多文件编辑器：{}",
                        file.path.display()
                    )));
                    return None;
                };
                source
            }
            ProjectDiffKind::Conflict => {
                let opened = self
                    .project
                    .update(cx, |project, cx| project.open_buffer(&file.path, cx));
                let Ok(source) = opened else {
                    cx.emit(EditorEvent::Error(format!(
                        "无法打开冲突文件：{}",
                        file.path.display()
                    )));
                    return None;
                };
                source
            }
        };
        // base / index 参照都由 GitStore 持有的修订文档提供：
        // 已暂存视图 base=HEAD、index=Index；未暂存视图 base=index=Index；冲突视图不建立 diff。
        let (base, index) = if self.kind == ProjectDiffKind::Conflict {
            (None, None)
        } else {
            let store = git_store.read(cx);
            (
                store.revision_document(self.kind.base_revision(), &file.path),
                store.revision_document(GitRevision::Index, &file.path),
            )
        };
        let display_path = root
            .and_then(|root| file.path.strip_prefix(root).ok())
            .unwrap_or(&file.path)
            .to_path_buf();
        let input = BufferDiffInput {
            working,
            base,
            index,
            path: file.path.clone(),
            operations: (self.kind != ProjectDiffKind::Conflict).then(|| {
                git_store
                    .read(cx)
                    .diff_operations(self.kind.base_revision())
            }),
        };
        // GitStore 预创建并按 (working, base, index) 共享；同一文件跨视图复用 diff 实体。
        let diff = git_store.update(cx, |store, cx| store.file_diff(&input, cx));
        Some(DiffFile {
            diff,
            display_path,
            context_lines: Some(DIFF_CONTEXT_LINES),
        })
    }

    /// 显示 hunk 的源定位（hunk 操作与导航用）：按显示坐标反查源文件与源 hunk。
    fn diff_hunk_source_info(&self, displayed: &DisplayHunk, cx: &App) -> Option<DiffHunkSource> {
        let index = self
            .multi_buffer
            .read(cx)
            .diff_hunks()
            .iter()
            .position(|hunk| hunk == displayed)?;
        self.multi_buffer.read(cx).buffer_diff_hunk_at(index, cx)
    }

    /// 把打开请求中的 Deleted 片段换算为工作区文件中的合法定位行列（0-based）。
    ///
    /// Deleted 片段的内容来自 Git 修订文本，其字节坐标在打开的工作区文件中不存在；
    /// 换算由 MultiBuffer 按投影数据完成（修订行 → hunk → 工作区行 + 列钳制）。
    fn deleted_navigation_target(
        &self,
        location: &ExcerptLocation,
        working_text: &Snapshot,
        cx: &App,
    ) -> Option<(PathBuf, usize, usize)> {
        self.multi_buffer
            .read(cx)
            .deleted_navigation_target(location, working_text, cx)
            .map(|(line, column)| (location.path.clone(), line, column))
    }

    fn toggle_file_staged(
        &mut self,
        path: &Path,
        operation: GitHunkOperation,
        cx: &mut Context<Self>,
    ) {
        let git_store = self.project.read(cx).git_store();
        let path = AbsolutePathBuf::new(path.to_path_buf()).expect("Git 变更路径必须是绝对路径");
        git_store.update(cx, |store, cx| match operation {
            GitHunkOperation::Stage => store.stage_paths(vec![path.clone()], cx),
            GitHunkOperation::Unstage => store.unstage_paths(vec![path], cx),
            GitHunkOperation::Restore => unreachable!("文件复选框不执行工作区还原"),
        });
    }

    fn apply_hunk_action(
        &mut self,
        info: DiffHunkSource,
        operation: GitHunkOperation,
        cx: &mut Context<Self>,
    ) -> Result<(), String> {
        let is_created_file = info.diff.read(cx).is_created();
        let allowed = matches!(
            (self.kind, operation),
            (ProjectDiffKind::Unstaged, GitHunkOperation::Stage)
                | (ProjectDiffKind::Unstaged, GitHunkOperation::Restore)
                | (ProjectDiffKind::Staged, GitHunkOperation::Unstage)
        );
        if !allowed || (operation == GitHunkOperation::Restore && is_created_file) {
            return Err("当前变更块不支持此操作".into());
        }

        if let Some(range) = info.range {
            let diff = info.diff.clone();
            let Some(operations) = diff.read(cx).operations() else {
                return Err("变更块操作已失效，请刷新后重试".into());
            };
            match operation {
                GitHunkOperation::Stage if operations.supports_staging() => {
                    operations.stage(diff, vec![range], cx)
                }
                GitHunkOperation::Unstage if operations.supports_unstaging() => {
                    operations.unstage(diff, vec![range], cx)
                }
                GitHunkOperation::Restore if operations.supports_restore() => {
                    operations.restore(diff, vec![range], cx)
                }
                GitHunkOperation::Stage | GitHunkOperation::Unstage | GitHunkOperation::Restore => {
                    return Err("当前 diff 不支持此变更块操作".into());
                }
            }
        } else {
            // 整文件新增块没有行级 hunk：按路径整体暂存/取消暂存。
            let git_store = self.project.read(cx).git_store();
            let path = AbsolutePathBuf::new(info.path.clone()).expect("Git 变更路径必须是绝对路径");
            git_store.update(cx, |store, cx| match operation {
                GitHunkOperation::Stage => store.stage_paths(vec![path.clone()], cx),
                GitHunkOperation::Unstage => store.unstage_paths(vec![path], cx),
                GitHunkOperation::Restore => {}
            });
        }
        // 行级操作写入 optimistic pending 后由 BufferDiffEvent::DiffChanged 驱动物化；
        // 整文件路径操作由 GitStore 状态事件刷新。
        Ok(())
    }

    fn revision_requirements_ready(&self, file: &GitChangeFile, cx: &App) -> bool {
        let git_store = self.project.read(cx).git_store();
        let store = git_store.read(cx);
        (self.kind == ProjectDiffKind::Conflict
            || store.revision_document_loaded(self.kind.base_revision(), &file.path))
            && (self.kind != ProjectDiffKind::Staged
                || store.revision_document_loaded(GitRevision::Index, &file.path))
    }

    /// 一次性为全部变更文件发起修订读取（同一文件的 base/index 并行）。
    ///
    /// 读取结果按路径顺序增量登记，不再依赖视口逐个推进；
    /// 首屏不会被逐文件等待拖长。
    fn load_all_revision_text(&mut self, cx: &mut Context<Self>) {
        if self.kind == ProjectDiffKind::Conflict {
            return;
        }
        let git_store = self.project.read(cx).git_store();
        let mut revisions = vec![self.kind.base_revision()];
        if self.kind == ProjectDiffKind::Staged {
            revisions.push(GitRevision::Index);
        }
        let files = self.files.clone();
        for file in &files {
            for revision in &revisions {
                if git_store
                    .read(cx)
                    .revision_document_loaded(*revision, &file.path)
                    || !self
                        .loading_revision_text
                        .insert((*revision, file.path.clone()))
                {
                    continue;
                }
                let path = file.path.clone();
                let revision = *revision;
                let load = git_store
                    .read(cx)
                    .load_revision_document(revision, &path, cx);
                cx.spawn(async move |this, cx| {
                    let _ = load.await;
                    this.update(cx, |view, cx| {
                        view.loading_revision_text.remove(&(revision, path.clone()));
                        if view
                            .loading_revision_text
                            .iter()
                            .any(|(_, loading_path)| loading_path == &path)
                        {
                            return;
                        }
                        view.register_ready_files(cx);
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    /// 按路径顺序把已就绪文件增量追加进投影；
    /// 遇到未就绪文件即停止，等待后续读取或 diff 结果事件。
    fn register_ready_files(&mut self, cx: &mut Context<Self>) {
        if self.kind == ProjectDiffKind::Conflict {
            return;
        }
        let root = self.project.read(cx).root().map(Path::to_path_buf);
        let attached = self
            .editor
            .read(cx)
            .diff_paths(cx)
            .into_iter()
            .collect::<HashSet<_>>();
        let files = self.files.clone();
        let mut appended = Vec::new();
        for file in &files {
            let display_path = root
                .as_deref()
                .and_then(|root| file.path.strip_prefix(root).ok())
                .unwrap_or(&file.path)
                .to_path_buf();
            if attached.contains(&display_path) {
                continue;
            }
            if !self.revision_requirements_ready(file, cx) {
                break;
            }
            let Some(diff_file) = self.build_file_input(file, root.as_deref(), cx) else {
                continue;
            };
            appended.push(diff_file);
        }
        if appended.is_empty() {
            return;
        }
        self.editor.update(cx, |editor, cx| {
            let mut rebuilt = false;
            for file in appended {
                rebuilt |= editor.add_diff(file, cx);
            }
            rebuilt
        });
    }

    fn move_to_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.pending_path = Some(path);
        self.apply_pending_path(cx);
    }

    fn apply_pending_path(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.pending_path.as_ref() else {
            return;
        };
        let snapshot = self
            .multi_buffer
            .update(cx, |buffer, cx| buffer.snapshot(cx));
        let mut excerpts = snapshot.excerpts();
        let Some(excerpt) = excerpts.find(|excerpt| excerpt.path() == path) else {
            return;
        };
        let offset = excerpt.output_range().start().get();
        let moved = self.editor.update(cx, |editor, cx| {
            <Editor as Item>::navigate_to_byte_range(editor, offset..offset, cx)
        });
        if moved {
            self.pending_path = None;
        }
    }
}
impl EventEmitter<EditorEvent> for ProjectDiffView {}

impl Focusable for ProjectDiffView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        if self.is_empty(cx) {
            self.empty_focus.clone()
        } else {
            self.editor.read(cx).focus_handle()
        }
    }
}

impl Render for ProjectDiffView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let is_empty = self.is_empty(cx);
        div()
            .debug_selector(move || {
                if is_empty {
                    "empty-project-diff-view".into()
                } else {
                    "project-diff-view".into()
                }
            })
            .track_focus(&self.empty_focus)
            .key_context("ProjectDiffView")
            .size_full()
            .bg(color::current(cx).editor_background)
            .when(!is_empty, |view| view.child(self.editor.clone()))
    }
}

impl Item for ProjectDiffView {
    type Event = EditorEvent;

    fn tab_content_text(&self, _cx: &App) -> SharedString {
        self.kind.title().into()
    }

    fn tab_icon(&self, _cx: &App) -> Option<SharedString> {
        Some(self.kind.icon().into())
    }

    fn to_item_events(event: &Self::Event, emit: &mut dyn FnMut(ItemEvent)) {
        <Editor as Item>::to_item_events(event, emit);
    }

    fn active_path(&self, cx: &App) -> Option<PathBuf> {
        self.editor
            .read(cx)
            .excerpt_location(cx)
            .map(|location| location.path)
    }

    fn serialized_pane_item(&self, cx: &App) -> Option<SerializedPaneItem> {
        Some(SerializedPaneItem::Custom {
            kind: PROJECT_DIFF_SERIALIZED_KIND.into(),
            state: serde_json::json!({
                "kind": self.kind.serialized_name(),
                "active_path": self.active_path(cx),
            }),
        })
    }

    fn multi_buffer(&self, _cx: &App) -> Option<Entity<MultiBuffer>> {
        Some(self.multi_buffer.clone())
    }

    fn can_save(&self, cx: &App) -> bool {
        self.kind != ProjectDiffKind::Staged && <Editor as Item>::can_save(self.editor.read(cx), cx)
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.kind != ProjectDiffKind::Staged && self.editor.read(cx).is_dirty(cx)
    }

    fn save(
        &mut self,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        self.editor.update(cx, |editor, cx| {
            <Editor as Item>::save(editor, project, window, cx)
        })
    }

    fn as_searchable(
        &self,
        _self_handle: &Entity<Self>,
        _cx: &App,
    ) -> Option<Box<dyn SearchableItemHandle>> {
        Some(Box::new(self.editor.clone()))
    }

    fn act_as_type(
        &self,
        type_id: TypeId,
        self_handle: &Entity<Self>,
        _cx: &App,
    ) -> Option<AnyEntity> {
        if type_id == TypeId::of::<Self>() {
            Some(self_handle.clone().into())
        } else if type_id == TypeId::of::<Editor>() {
            Some(self.editor.clone().into())
        } else {
            None
        }
    }
}

/// 从布局恢复 Git 组合文档；
/// 内容和文件集合始终由当前 GitStore 状态重新生成。
pub struct ProjectDiffSerializedItemProvider;

impl SerializedItemProvider for ProjectDiffSerializedItemProvider {
    fn kind(&self) -> &'static str {
        PROJECT_DIFF_SERIALIZED_KIND
    }

    fn restore(
        &self,
        state: serde_json::Value,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
        let result = project_diff_state(&state).map(|(kind, active_path)| {
            let view = cx.new(|cx| ProjectDiffView::new(kind, project, cx));
            if let Some(path) = active_path {
                view.update(cx, |view, cx| view.move_to_path(path, cx));
            }
            subscribe_to_open_excerpts(&view, window, cx);
            Box::new(view) as Box<dyn ItemHandle>
        });
        Task::ready(result)
    }
}

fn project_diff_state(
    state: &serde_json::Value,
) -> anyhow::Result<(ProjectDiffKind, Option<PathBuf>)> {
    let kind = state
        .get("kind")
        .and_then(serde_json::Value::as_str)
        .and_then(ProjectDiffKind::from_serialized_name)
        .ok_or_else(|| anyhow::anyhow!("项目差异标签缺少有效分组"))?;
    let active_path = state
        .get("active_path")
        .and_then(serde_json::Value::as_str)
        .map(PathBuf::from);
    Ok((kind, active_path))
}

fn subscribe_to_open_excerpts(
    view: &Entity<ProjectDiffView>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    cx.subscribe_in(view, window, |workspace, view, event, window, cx| {
        let EditorEvent::OpenExcerptsRequested { locations, .. } = event else {
            return;
        };
        for location in locations {
            // Deleted 片段的内容来自 Git 修订文本，换算为工作区文件的真实行列；
            // 其余片段坐标直接可用，保持字节导航。
            let navigation = workspace.project().update(cx, |project, cx| {
                let Ok(buffer) = project.open_buffer(&location.path, cx) else {
                    return None;
                };
                let text = cx.read_entity(&buffer, |buffer, _| buffer.text_snapshot());
                cx.read_entity(view, |view, cx| {
                    view.deleted_navigation_target(location, &text, cx)
                })
            });
            if let Some((path, line, column)) = navigation {
                workspace.open_path_at_line_column(path, line, column, window, cx);
            } else {
                workspace.open_path_at(
                    location.path.clone(),
                    location.source_range.start().get()..location.source_range.end().get(),
                    window,
                    cx,
                );
            }
        }
    })
    .detach();
}

/// 打开或复用未提交变更 Item，并定位到版本管理面板选择的文件。
pub fn deploy_at(
    workspace: &mut Workspace,
    kind: ProjectDiffKind,
    path: PathBuf,
    focus_opened_item: bool,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let pane = workspace.pane().clone();
    if let Some(existing) = pane.read(cx).tabs().iter().find_map(|item| {
        item.act_as::<ProjectDiffView>(cx)
            .filter(|view| view.read(cx).kind == kind)
    }) {
        let item_id = existing.entity_id();
        pane.update(cx, |pane, cx| pane.activate_tab(item_id, window, cx));
        existing.update(cx, |view, cx| view.move_to_path(path, cx));
        if focus_opened_item {
            window.focus(&existing.read(cx).focus_handle(cx), cx);
        }
        return;
    }

    let project = workspace.project().clone();
    let view = cx.new(|cx| ProjectDiffView::new(kind, project, cx));
    view.update(cx, |view, cx| view.move_to_path(path, cx));
    subscribe_to_open_excerpts(&view, window, cx);
    let focus = pane.update(cx, |pane, cx| {
        pane.open_item(Box::new(view), false, window, cx)
    });
    if focus_opened_item {
        window.focus(&focus, cx);
    }
}
#[cfg(test)]
#[path = "test/project_diff_tests.rs"]
mod tests;
