//! 已暂存/未暂存变更的多文件编辑器。
//!
//! GitStore 的状态快照决定文件集合，Project/BufferStore 继续拥有真实文件文档，MultiBuffer 只组合这些文档。
//! 点击版本管理条目时按分组复用对应 Item 并定位文件，不为 Git 状态建立界面侧副本。

use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::ops::Range;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    AnyElement, AnyEntity, App, Context, Entity, EventEmitter, FocusHandle, Focusable, Pixels,
    Render, SharedString, Subscription, Task, WeakEntity, Window, div, prelude::*,
};
use zcv_editor::{DiffHunkDelegate, Editor, EditorEvent, EditorScrollAnchor, MaterializedDiffHunk};
use zcv_git::{
    DiffBase, DiffHunk, DiffHunkKind, FileStatus, GitHunkOperation, GitRevision, StatusCode,
};
use zcv_language::LanguageBuffer;
use zcv_multi_buffer::{ExcerptDiffKind, ExcerptLocation, MultiBuffer, MultiBufferExcerpt};
use zcv_project::{DiffRequest, GitStoreEvent, Project};
use zcv_text::{Buffer, BufferConfig, Line, Snapshot, TextRange};
use zcv_theme::{color, space};
use zcv_ui::{Button, ButtonSize};
use zcv_workspace::{Item, ItemEvent, SearchableItemHandle, ToolbarItemLocation, Workspace};

#[derive(Clone)]
struct GitChangeFile {
    path: PathBuf,
    status: FileStatus,
}

#[derive(Clone)]
struct ProjectDiffHunkTarget {
    displayed: DiffHunk,
    source: Option<DiffHunk>,
    path: PathBuf,
    is_created_file: bool,
}

struct ProjectDiffHunkDelegate {
    view: WeakEntity<ProjectDiffView>,
}

impl DiffHunkDelegate for ProjectDiffHunkDelegate {
    fn render_hunk_controls(
        &self,
        row: usize,
        hunk: &DiffHunk,
        _line_height: Pixels,
        _editor: &Entity<Editor>,
        _window: &mut Window,
        cx: &mut App,
    ) -> AnyElement {
        let Some(view) = self.view.upgrade() else {
            return div().into_any_element();
        };
        let (kind, is_created_file) = {
            let view = view.read(cx);
            let Some(target) = view.hunk_target(hunk) else {
                return div().into_any_element();
            };
            (view.kind, target.is_created_file)
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
            ProjectDiffKind::Unstaged => {
                let stage_view = self.view.clone();
                let stage_hunk = hunk.clone();
                let restore_view = self.view.clone();
                let restore_hunk = hunk.clone();
                controls
                    .child(
                        Button::text(("stage-hunk", row), "暂存")
                            .size(ButtonSize::Loose)
                            .label("暂存此变更块")
                            .on_click(move |_event, _window, cx| {
                                stage_view
                                    .update(cx, |view, cx| {
                                        view.apply_hunk_action(
                                            &stage_hunk,
                                            GitHunkOperation::Stage,
                                            cx,
                                        )
                                    })
                                    .ok();
                            }),
                    )
                    .child(
                        Button::text(("restore-hunk", row), "重做")
                            .size(ButtonSize::Loose)
                            .label(if is_created_file {
                                "新建文件不能重做单个变更块"
                            } else {
                                "用暂存区内容重做此变更块"
                            })
                            .disabled(is_created_file)
                            .on_click(move |_event, _window, cx| {
                                restore_view
                                    .update(cx, |view, cx| {
                                        view.apply_hunk_action(
                                            &restore_hunk,
                                            GitHunkOperation::Restore,
                                            cx,
                                        )
                                    })
                                    .ok();
                            }),
                    )
                    .into_any_element()
            }
            ProjectDiffKind::Staged => {
                let unstage_view = self.view.clone();
                let unstage_hunk = hunk.clone();
                controls
                    .child(
                        Button::text(("unstage-hunk", row), "取消暂存")
                            .size(ButtonSize::Loose)
                            .label("取消暂存此变更块")
                            .on_click(move |_event, _window, cx| {
                                unstage_view
                                    .update(cx, |view, cx| {
                                        view.apply_hunk_action(
                                            &unstage_hunk,
                                            GitHunkOperation::Unstage,
                                            cx,
                                        )
                                    })
                                    .ok();
                            }),
                    )
                    .into_any_element()
            }
        }
    }
}

/// 版本管理面板分组对应的比较范围。
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ProjectDiffKind {
    Staged,
    Unstaged,
}

impl ProjectDiffKind {
    fn diff_base(self) -> DiffBase {
        match self {
            Self::Staged => DiffBase::Staged,
            Self::Unstaged => DiffBase::Index,
        }
    }

    fn base_revision(self) -> GitRevision {
        match self {
            Self::Staged => GitRevision::Head,
            Self::Unstaged => GitRevision::Index,
        }
    }

    fn includes(self, status: FileStatus) -> bool {
        match self {
            Self::Staged => status.has_staged(),
            Self::Unstaged => status.has_unstaged(),
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
        }
    }

    fn icon(self) -> &'static str {
        match self {
            Self::Staged => "icons/lock.svg",
            Self::Unstaged => "icons/diff.svg",
        }
    }
}

/// 每个 Git 变更块（hunk）上下各保留多少行未修改的上下文。
const DIFF_CONTEXT_LINES: usize = 2;

/// 把列（Unicode scalar 计数）钳制到文本中指定行的有效长度（行 0-based）。
///
/// Deleted 片段换算出的列来自 Git 修订行，工作区对应行可能因修改而变短，越界列会导致行列导航失败，必须钳制到行尾。
fn clamp_column_to_line(text: &Snapshot, line: usize, column: usize) -> usize {
    let line = line.min(text.line_count().saturating_sub(1));
    let line_chars = text
        .line_content(Line::new(line), None)
        .map_or(0, |content| content.len_chars());
    column.min(line_chars)
}

pub(crate) struct ProjectDiffView {
    kind: ProjectDiffKind,
    project: Entity<Project>,
    editor: Entity<Editor>,
    multi_buffer: Entity<MultiBuffer>,
    files: Vec<GitChangeFile>,
    pending_path: Option<PathBuf>,
    refresh_scroll_anchor: Option<EditorScrollAnchor>,
    revision_sources: HashMap<(GitRevision, PathBuf), Entity<MultiBuffer>>,
    loading_revision_text: HashSet<(GitRevision, PathBuf)>,
    hunk_targets: Vec<ProjectDiffHunkTarget>,
    _subscriptions: Vec<Subscription>,
}

impl ProjectDiffView {
    fn new(kind: ProjectDiffKind, project: Entity<Project>, cx: &mut Context<Self>) -> Self {
        let weak_view = cx.weak_entity();
        let multi_buffer = match kind {
            ProjectDiffKind::Staged => cx.new(MultiBuffer::empty_read_only),
            ProjectDiffKind::Unstaged => cx.new(MultiBuffer::empty),
        };
        let editor = cx.new(|cx| {
            let mut editor = Editor::for_multi_buffer(multi_buffer.clone(), cx);
            editor.set_placeholder_text(format!("没有{}", kind.title()), cx);
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
            cx.subscribe(&editor, |_, _, event: &EditorEvent, cx| {
                cx.emit(event.clone());
            }),
            // 删除/修改块的展开折叠状态变化：按展开状态重建组合文档（与普通编辑器同一机制）。
            cx.subscribe(&editor, |view, _, event: &EditorEvent, cx| {
                if matches!(event, EditorEvent::DiffHunksExpandedChanged) {
                    view.rebuild_excerpts(cx);
                }
            }),
            cx.subscribe(&git_store, |view, _, event, cx| match event {
                GitStoreEvent::Repositories | GitStoreEvent::Statuses | GitStoreEvent::Head => {
                    if matches!(event, GitStoreEvent::Head) {
                        view.loading_revision_text.clear();
                        // HEAD 变化后旧 hunk 的旧侧坐标空间失效：按默认策略重置展开状态，避免新 diff 按失效的行号误迁移状态。
                        view.editor
                            .update(cx, |editor, cx| editor.reset_diff_hunk_expansion_state(cx));
                    }
                    view.refresh_files(cx);
                }
                GitStoreEvent::HunksChanged => {
                    view.rebuild_excerpts(cx);
                    view.load_missing_revision_text(cx);
                }
                GitStoreEvent::ActiveRepositoryChanged
                | GitStoreEvent::JobsUpdated
                | GitStoreEvent::Uncommitted(_) => {}
            }),
        ];
        let mut view = Self {
            kind,
            project,
            editor,
            multi_buffer,
            files: Vec::new(),
            pending_path: None,
            refresh_scroll_anchor: None,
            revision_sources: Default::default(),
            loading_revision_text: Default::default(),
            hunk_targets: Vec::new(),
            _subscriptions: subscriptions,
        };
        view.refresh_files(cx);
        view
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
                            path: workdir.join(relative),
                            status: entry.status,
                        })
                })
                .collect::<Vec<_>>();
            changed.sort_by(|left, right| left.path.cmp(&right.path));
            changed
        };

        self.files = changed;
        let visible_paths = self
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<HashSet<_>>();
        self.revision_sources
            .retain(|(_, path), _| visible_paths.contains(path));

        let paths = self
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();
        git_store.update(cx, |store, cx| {
            store.request_hunks(self.kind.diff_base(), &paths, cx)
        });
        self.rebuild_excerpts(cx);
        self.load_missing_revision_text(cx);
    }

    /// 以 hunk 为核心重建可见 excerpts；旧侧与新侧都属于同一个 MultiBuffer 坐标空间。
    fn rebuild_excerpts(&mut self, cx: &mut Context<Self>) {
        if self.pending_path.is_none() && self.refresh_scroll_anchor.is_none() {
            self.refresh_scroll_anchor = self
                .editor
                .update(cx, |editor, cx| editor.capture_scroll_anchor(cx));
        }
        if !self.projection_data_ready(cx) {
            return;
        }
        // 展开状态决定旧行片段是否进入组合（与普通编辑器同一机制）。
        let root = self.project.read(cx).root().map(Path::to_path_buf);
        let git_store = self.project.read(cx).git_store();
        let mut excerpts = Vec::new();

        let files = self.files.clone();
        for file in &files {
            let Some(file_hunks) = git_store
                .read(cx)
                .hunks_for_path(self.kind.diff_base(), &file.path)
                .map(|hunks| hunks.to_vec())
            else {
                continue;
            };
            let current_source = match self.kind {
                ProjectDiffKind::Staged => {
                    let Some(index_text) = git_store
                        .read(cx)
                        .revision_text(GitRevision::Index, &file.path)
                        .map(|text| text.to_string())
                    else {
                        continue;
                    };
                    self.revision_source(GitRevision::Index, &file.path, &index_text, cx)
                }
                ProjectDiffKind::Unstaged => {
                    let opened = self.project.update(cx, |project, cx| {
                        if self.kind.is_deleted(file.status) && !file.path.exists() {
                            project.open_deleted_buffer(&file.path, cx)
                        } else {
                            project.open_buffer(&file.path, cx)
                        }
                    });
                    let Ok(source) = opened else {
                        eprintln!(
                            "无法把 Git 变更文件加入多文件编辑器：{}",
                            file.path.display()
                        );
                        continue;
                    };
                    source
                }
            };
            let Some(base_text) = git_store
                .read(cx)
                .revision_text(self.kind.base_revision(), &file.path)
                .map(|text| text.to_string())
            else {
                continue;
            };
            let base_source =
                self.revision_source(self.kind.base_revision(), &file.path, &base_text, cx);
            let line_count = current_source.read(cx).snapshot(cx).text().line_count();
            let display_path = root
                .as_deref()
                .and_then(|root| file.path.strip_prefix(root).ok())
                .unwrap_or(&file.path)
                .to_path_buf();

            if file_hunks.is_empty() && self.kind.is_created(file.status) {
                push_projected_excerpt(
                    &mut excerpts,
                    current_source,
                    0..line_count,
                    &display_path,
                    true,
                    Some(ExcerptDiffKind::Added),
                    cx,
                );
                continue;
            }

            for context_range in excerpt_line_ranges(&file_hunks, line_count) {
                let mut current_line = context_range.start;
                let mut starts_new_excerpt = true;
                for hunk in file_hunks
                    .iter()
                    .filter(|hunk| hunk_is_inside_excerpt(hunk, &context_range))
                {
                    push_projected_excerpt(
                        &mut excerpts,
                        current_source.clone(),
                        current_line..hunk.range.start,
                        &display_path,
                        starts_new_excerpt,
                        None,
                        cx,
                    );
                    if current_line < hunk.range.start {
                        starts_new_excerpt = false;
                    }
                    // 旧行片段按展开状态进入组合：展开显示完整旧行，折叠用空占位行标记删除点（保持片段顺序与刷新映射一致，供展开按钮定位）。
                    let expanded =
                        self.editor
                            .read(cx)
                            .is_diff_hunk_expanded(hunk.kind, &hunk.old_range, cx);
                    if !hunk.old_range.is_empty() {
                        if expanded {
                            push_projected_excerpt(
                                &mut excerpts,
                                base_source.clone(),
                                hunk.old_range.clone(),
                                &display_path,
                                starts_new_excerpt,
                                Some(ExcerptDiffKind::Deleted),
                                cx,
                            );
                        } else {
                            // 折叠：删除点锚定处的空占位行。
                            if let Ok(anchor) = base_source
                                .read(cx)
                                .snapshot(cx)
                                .text()
                                .line_start_byte(Line::new(hunk.old_range.start))
                            {
                                excerpts.push(
                                    MultiBufferExcerpt::new(
                                        base_source.clone(),
                                        TextRange::new(anchor, anchor).expect("占位范围必须正序"),
                                        Vec::new(),
                                    )
                                    .with_display_path(display_path.clone())
                                    .with_editable(false)
                                    .with_diff_kind(ExcerptDiffKind::Deleted)
                                    .with_starts_new_excerpt(starts_new_excerpt),
                                );
                            }
                        }
                        starts_new_excerpt = false;
                    }
                    if !hunk.range.is_empty() {
                        push_projected_excerpt(
                            &mut excerpts,
                            current_source.clone(),
                            hunk.range.clone(),
                            &display_path,
                            starts_new_excerpt,
                            Some(ExcerptDiffKind::Added),
                            cx,
                        );
                        starts_new_excerpt = false;
                    }
                    current_line = hunk.range.end;
                }
                push_projected_excerpt(
                    &mut excerpts,
                    current_source.clone(),
                    current_line..context_range.end,
                    &display_path,
                    starts_new_excerpt,
                    None,
                    cx,
                );
            }
        }

        self.multi_buffer.update(cx, |multi_buffer, cx| {
            multi_buffer.set_excerpts(excerpts, cx)
        });
        self.refresh_diff_hunks(cx);
        if let Some(scroll_anchor) = self.refresh_scroll_anchor.take() {
            self.editor.update(cx, |editor, cx| {
                editor.restore_scroll_anchor(scroll_anchor, cx);
            });
        }
        self.apply_pending_path(cx);
        cx.notify();
    }

    /// Git 状态、hunk 与修订文本分批到达；全部就绪前保留旧投影，避免中间空态破坏视口锚点。
    fn projection_data_ready(&self, cx: &App) -> bool {
        let git_store = self.project.read(cx).git_store();
        let store = git_store.read(cx);
        self.files.iter().all(|file| {
            store
                .hunks_for_path(self.kind.diff_base(), &file.path)
                .is_some()
                && store
                    .revision_text(self.kind.base_revision(), &file.path)
                    .is_some()
                && (self.kind != ProjectDiffKind::Staged
                    || store
                        .revision_text(GitRevision::Index, &file.path)
                        .is_some())
        })
    }

    /// 将每个源文件的 hunk 对齐到已经物化的新旧侧片段。
    fn refresh_diff_hunks(&mut self, cx: &mut Context<Self>) {
        let snapshot = self.multi_buffer.read(cx).snapshot(cx);
        let git_store = self.project.read(cx).git_store();
        let store = git_store.read(cx);
        let mut hunks = Vec::new();
        let mut hunk_targets = Vec::new();

        for file in &self.files {
            let file_hunks = store
                .hunks_for_path(self.kind.diff_base(), &file.path)
                .map(|hunks| hunks.to_vec())
                .unwrap_or_default();
            let mut diff_excerpts = snapshot
                .excerpts()
                .iter()
                .filter(|excerpt| excerpt.path() == file.path && excerpt.diff_kind().is_some());

            // 未跟踪文件和无 HEAD 仓库不会出现在 `git diff HEAD` 输出中，但在项目差异里整个工作区文件就是新增内容。
            if file_hunks.is_empty() && self.kind.is_created(file.status) {
                for excerpt in diff_excerpts {
                    let displayed = DiffHunk {
                        range: excerpt.output_start_line()..excerpt.output_end_line(),
                        old_range: 0..0,
                        kind: DiffHunkKind::Added,
                    };
                    hunk_targets.push(ProjectDiffHunkTarget {
                        displayed: displayed.clone(),
                        source: None,
                        path: file.path.clone(),
                        is_created_file: true,
                    });
                    hunks.push(MaterializedDiffHunk {
                        hunk: displayed,
                        old_display_range: None,
                    });
                }
                continue;
            }

            for hunk in file_hunks {
                let old_excerpt = (!hunk.old_range.is_empty()).then(|| {
                    let excerpt = diff_excerpts
                        .next()
                        .expect("diff 旧侧必须存在对应的 MultiBuffer 片段");
                    assert_eq!(excerpt.diff_kind(), Some(ExcerptDiffKind::Deleted));
                    excerpt
                });
                let new_excerpt = (!hunk.range.is_empty()).then(|| {
                    let excerpt = diff_excerpts
                        .next()
                        .expect("diff 新侧必须存在对应的 MultiBuffer 片段");
                    assert_eq!(excerpt.diff_kind(), Some(ExcerptDiffKind::Added));
                    excerpt
                });
                let old_display_range = old_excerpt
                    .map(|excerpt| excerpt.output_start_line()..excerpt.output_end_line());
                let range = new_excerpt.map_or_else(
                    || {
                        let anchor = old_display_range
                            .as_ref()
                            .expect("纯删除 hunk 必须具有旧侧")
                            .end;
                        anchor..anchor
                    },
                    |excerpt| excerpt.output_start_line()..excerpt.output_end_line(),
                );
                let displayed = DiffHunk {
                    range,
                    old_range: hunk.old_range.clone(),
                    kind: hunk.kind,
                };
                hunk_targets.push(ProjectDiffHunkTarget {
                    displayed: displayed.clone(),
                    source: Some(hunk),
                    path: file.path.clone(),
                    is_created_file: self.kind.is_created(file.status),
                });
                hunks.push(MaterializedDiffHunk {
                    hunk: displayed,
                    old_display_range,
                });
            }
        }
        self.hunk_targets = hunk_targets;
        self.editor.update(cx, |editor, cx| {
            editor.set_deleted_hunk_text(None, cx);
            editor.set_materialized_diff_hunks(hunks, cx);
        });
    }

    fn hunk_target(&self, displayed: &DiffHunk) -> Option<&ProjectDiffHunkTarget> {
        self.hunk_targets
            .iter()
            .find(|target| target.displayed == *displayed)
    }

    /// 把打开请求中的 Deleted 片段换算为工作区文件中的合法定位行列（0-based）。
    ///
    /// Deleted 片段的内容来自 Git 修订文本，其字节坐标在打开的工作区文件中不存在；
    /// 经 hunk 把修订侧行号映射到工作区（新侧）行号，列沿用修订行内逻辑列，行与列都按工作区文件文本钳制到有效范围，返回值可直接用于行列导航。
    /// 非 Deleted 片段返回 `None`（坐标直接可用）。
    fn deleted_navigation_target(
        &self,
        location: &ExcerptLocation,
        working_text: &Snapshot,
        cx: &App,
    ) -> Option<(PathBuf, usize, usize)> {
        let snapshot = self.multi_buffer.read(cx).snapshot(cx);
        // 仅处理 Deleted 片段：修订文本坐标需换算，其余片段直接可用。
        let in_deleted_excerpt = snapshot.excerpts().iter().any(|excerpt| {
            excerpt.path() == location.path
                && excerpt.diff_kind() == Some(ExcerptDiffKind::Deleted)
                && excerpt.source_range().start() <= location.source_range.start()
                && location.source_range.end() <= excerpt.source_range().end()
        });
        if !in_deleted_excerpt {
            return None;
        }
        // 修订文本行号与列（列按 Unicode scalar 计数，与导航协议一致）。
        let revision_source = self
            .revision_sources
            .get(&(self.kind.base_revision(), location.path.clone()))?;
        let revision_text = revision_source.read(cx).snapshot(cx).text().clone();
        let Ok(position) = revision_text.byte_to_position(location.source_range.start()) else {
            return None;
        };
        let old_line = position.line().get();
        let column = position.column().get();
        // 包含该修订行的 hunk（旧侧行范围）。
        let hunk = self.hunk_targets.iter().find_map(|target| {
            (target.path == location.path)
                .then_some(target.source.as_ref())
                .flatten()
                .filter(|hunk| hunk.old_range.contains(&old_line))
        })?;
        // 修改行在 hunk 内按偏移映射；纯删除锚定变更块起点。
        let offset = old_line - hunk.old_range.start;
        let working_line = if hunk.range.is_empty() {
            hunk.range.start
        } else {
            (hunk.range.start + offset).min(hunk.range.end - 1)
        };
        // 行与列钳制到工作区文件有效范围（修改可能让行变短）。
        let line = working_line.min(working_text.line_count().saturating_sub(1));
        let column = clamp_column_to_line(working_text, line, column);
        Some((location.path.clone(), line, column))
    }

    fn apply_hunk_action(
        &mut self,
        displayed: &DiffHunk,
        operation: GitHunkOperation,
        cx: &mut Context<Self>,
    ) {
        let Some(target) = self.hunk_target(displayed).cloned() else {
            return;
        };
        let allowed = matches!(
            (self.kind, operation),
            (ProjectDiffKind::Unstaged, GitHunkOperation::Stage)
                | (ProjectDiffKind::Unstaged, GitHunkOperation::Restore)
                | (ProjectDiffKind::Staged, GitHunkOperation::Unstage)
        );
        if !allowed || (operation == GitHunkOperation::Restore && target.is_created_file) {
            return;
        }

        let git_store = self.project.read(cx).git_store();
        git_store.update(cx, |store, cx| {
            if let Some(source) = target.source {
                store.apply_hunk(operation, target.path, source, cx);
            } else if operation == GitHunkOperation::Stage {
                // 未跟踪文件没有 `git diff` hunk；它在视图中只有一个整文件新增块。
                store.stage_paths(vec![target.path], cx);
            }
        });
    }

    fn load_missing_revision_text(&mut self, cx: &mut Context<Self>) {
        let git_store = self.project.read(cx).git_store();
        for file in &self.files {
            let mut revisions = vec![self.kind.base_revision()];
            if self.kind == ProjectDiffKind::Staged && !revisions.contains(&GitRevision::Index) {
                revisions.push(GitRevision::Index);
            }
            for revision in revisions {
                if git_store
                    .read(cx)
                    .revision_text(revision, &file.path)
                    .is_some()
                    || !self
                        .loading_revision_text
                        .insert((revision, file.path.clone()))
                {
                    continue;
                }
                let path = file.path.clone();
                let load = git_store.read(cx).load_revision_text(revision, &path, cx);
                cx.spawn(async move |this, cx| {
                    let _ = load.await;
                    this.update(cx, |view, cx| {
                        view.loading_revision_text.remove(&(revision, path.clone()));
                        view.rebuild_excerpts(cx);
                        view.load_missing_revision_text(cx);
                    })
                    .ok();
                })
                .detach();
            }
        }
    }

    /// Git 修订来源由视图按“修订 + 路径”唯一拥有，并在对应文本变化后原位刷新。
    fn revision_source(
        &mut self,
        revision: GitRevision,
        path: &Path,
        text: &str,
        cx: &mut Context<Self>,
    ) -> Entity<MultiBuffer> {
        let key = (revision, path.to_path_buf());
        if let Some(source) = self.revision_sources.get(&key) {
            let source_text = String::from_utf8(source.read(cx).snapshot(cx).text_bytes())
                .expect("Git 修订投影必须是 UTF-8");
            if source_text != text {
                let buffer = source
                    .read(cx)
                    .as_singleton(cx)
                    .expect("Git 修订来源必须是 singleton");
                buffer.update(cx, |buffer, _| {
                    buffer
                        .reload_from_text(text.to_string())
                        .expect("Git 修订文本必须能原位刷新")
                });
            }
            return source.clone();
        }

        let buffer = Buffer::from_text(text.to_string(), BufferConfig::default())
            .expect("Git 修订文本必须能创建 Buffer");
        let buffer = cx.new(|_| buffer);
        let language_buffer =
            cx.new(|cx| LanguageBuffer::new(buffer, Some(path.to_path_buf()), cx));
        let source = cx.new(|cx| MultiBuffer::singleton(language_buffer, cx));
        self.revision_sources.insert(key, source.clone());
        source
    }

    fn move_to_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.refresh_scroll_anchor = None;
        self.pending_path = Some(path);
        self.apply_pending_path(cx);
    }

    pub(crate) fn diff_requests(&self) -> impl Iterator<Item = DiffRequest> + '_ {
        self.files
            .iter()
            .map(|file| DiffRequest::new(self.kind.diff_base(), file.path.clone()))
    }

    fn apply_pending_path(&mut self, cx: &mut Context<Self>) {
        let Some(path) = self.pending_path.as_ref() else {
            return;
        };
        let snapshot = self.multi_buffer.read(cx).snapshot(cx);
        let Some(excerpt) = snapshot
            .excerpts()
            .iter()
            .find(|excerpt| excerpt.path() == path)
        else {
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

/// 将 hunk 新侧范围扩展为带上下文的 excerpts，并合并重叠或相邻范围。
fn excerpt_line_ranges(hunks: &[DiffHunk], line_count: usize) -> Vec<Range<usize>> {
    let max_line = line_count.saturating_sub(1);
    let mut ranges = hunks
        .iter()
        .map(|hunk| {
            let start = hunk
                .range
                .start
                .min(max_line)
                .saturating_sub(DIFF_CONTEXT_LINES);
            // Zcv 的行范围右开；Zed 的 Point 终点位于最后一条变更行内。
            // 非空 hunk 先换算为最后一条变更行，才能得到真正的后两行上下文。
            let changed_end_line = if hunk.range.is_empty() {
                hunk.range.start
            } else {
                hunk.range.end.saturating_sub(1)
            };
            let end_line = changed_end_line
                .saturating_add(DIFF_CONTEXT_LINES)
                .min(max_line);
            start..end_line + 1
        })
        .collect::<Vec<_>>();
    ranges.sort_by_key(|range| range.start);

    let mut merged = Vec::<Range<usize>>::new();
    for range in ranges {
        if let Some(previous) = merged.last_mut()
            && range.start <= previous.end
        {
            previous.end = previous.end.max(range.end);
        } else {
            merged.push(range);
        }
    }
    merged
}

fn hunk_is_inside_excerpt(hunk: &DiffHunk, excerpt: &Range<usize>) -> bool {
    if hunk.range.is_empty() {
        excerpt.contains(&hunk.range.start)
    } else {
        hunk.range.start >= excerpt.start && hunk.range.end <= excerpt.end
    }
}

fn push_projected_excerpt(
    excerpts: &mut Vec<MultiBufferExcerpt>,
    source: Entity<MultiBuffer>,
    lines: Range<usize>,
    display_path: &Path,
    starts_new_excerpt: bool,
    diff_kind: Option<ExcerptDiffKind>,
    cx: &App,
) {
    if lines.is_empty() {
        return;
    }
    let mut excerpt = MultiBufferExcerpt::line_range(source, lines, cx);
    if excerpt.source_range().is_empty() && diff_kind.is_none() {
        return;
    }
    excerpt = excerpt
        .with_display_path(display_path.to_path_buf())
        .with_starts_new_excerpt(starts_new_excerpt)
        .with_editable(diff_kind != Some(ExcerptDiffKind::Deleted));
    if let Some(diff_kind) = diff_kind {
        excerpt = excerpt.with_diff_kind(diff_kind);
    }
    excerpts.push(excerpt);
}

impl EventEmitter<EditorEvent> for ProjectDiffView {}

impl Focusable for ProjectDiffView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.editor.read(cx).focus_handle()
    }
}

impl Render for ProjectDiffView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .key_context("ProjectDiffView")
            .size_full()
            .bg(color::current(cx).editor_background)
            .child(self.editor.clone())
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

    fn multi_buffer(&self, _cx: &App) -> Option<Entity<MultiBuffer>> {
        Some(self.multi_buffer.clone())
    }

    fn can_save(&self, cx: &App) -> bool {
        self.kind == ProjectDiffKind::Unstaged
            && <Editor as Item>::can_save(self.editor.read(cx), cx)
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.kind == ProjectDiffKind::Unstaged && self.editor.read(cx).is_dirty(cx)
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

    fn breadcrumb_location(&self, _cx: &App) -> ToolbarItemLocation {
        ToolbarItemLocation::Hidden
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

/// 打开或复用未提交变更 Item，并定位到版本管理面板选择的文件。
pub(crate) fn deploy_at(
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
        existing.update(cx, |view, cx| view.move_to_path(path, cx));
        pane.update(cx, |pane, cx| pane.activate_tab(item_id, window, cx));
        if focus_opened_item {
            window.focus(&existing.read(cx).focus_handle(cx));
        }
        return;
    }

    let project = workspace.project().clone();
    let view = cx.new(|cx| ProjectDiffView::new(kind, project, cx));
    view.update(cx, |view, cx| view.move_to_path(path, cx));
    cx.subscribe_in(&view, window, |workspace, view, event, window, cx| {
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
                let text = cx
                    .read_entity(&buffer, |buffer, _| buffer.snapshot(cx))
                    .text()
                    .clone();
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
    let focus = pane.update(cx, |pane, cx| {
        pane.open_item(Box::new(view), false, window, cx)
    });
    if focus_opened_item {
        window.focus(&focus);
    }
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use gpui::{AppContext as _, TestAppContext};

    use super::*;

    #[test]
    fn hunk_ranges_include_two_context_lines_and_merge_overlaps() {
        let hunks = vec![
            DiffHunk {
                range: 5..6,
                old_range: 5..6,
                kind: DiffHunkKind::Modified,
            },
            DiffHunk {
                range: 8..9,
                old_range: 8..9,
                kind: DiffHunkKind::Modified,
            },
            DiffHunk {
                range: 20..20,
                old_range: 20..22,
                kind: DiffHunkKind::Deleted,
            },
        ];

        assert_eq!(excerpt_line_ranges(&hunks, 30), vec![3..11, 18..23]);
    }

    #[gpui::test]
    fn git_status_drives_one_ordered_excerpt_per_changed_file(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时仓库");
        let root = directory.path().canonicalize().expect("应规范化仓库路径");
        run_in(&root, &["git", "init", "-q", "-b", "master"]);
        run_in(&root, &["git", "config", "user.email", "test@example.com"]);
        run_in(&root, &["git", "config", "user.name", "Test User"]);
        std::fs::write(root.join("deleted.txt"), "将被删除\n").expect("应创建文件");
        std::fs::write(
            root.join("modified.txt"),
            "line0\nline1\nline2\nline3\n修改前\nline5\nline6\nline7\n",
        )
        .expect("应创建文件");
        run_in(&root, &["git", "add", "."]);
        run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
        std::fs::remove_file(root.join("deleted.txt")).expect("应删除文件");
        std::fs::write(
            root.join("modified.txt"),
            "line0\nline1\nline2\nline3\n修改后\nline5\nline6\nline7\n",
        )
        .expect("应修改文件");
        std::fs::write(root.join("untracked.txt"), "新增\n").expect("应创建未跟踪文件");

        let project = cx.new(|cx| Project::new(root.clone(), cx));
        let view = cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Unstaged, project, cx));
        cx.run_until_parked();
        cx.run_until_parked();

        let (paths, text) = cx.read_entity(&view, |view, cx| {
            let snapshot = view.multi_buffer.read(cx).snapshot(cx);
            let paths = snapshot
                .excerpts()
                .iter()
                .filter(|excerpt| excerpt.starts_new_excerpt())
                .map(|excerpt| {
                    excerpt
                        .path()
                        .file_name()
                        .expect("变更应有文件名")
                        .to_string_lossy()
                        .into_owned()
                })
                .collect::<Vec<_>>();
            (paths, String::from_utf8(snapshot.text_bytes()).unwrap())
        });
        assert_eq!(paths, vec!["deleted.txt", "modified.txt", "untracked.txt"]);
        assert_eq!(
            text,
            "将被删除\nline2\nline3\n修改前\n修改后\nline5\nline6\n新增\n"
        );
    }

    /// 回归：从 Deleted 片段打开文件时，必须换算到工作区文件中的真实行列，而不是把 Git 修订文本的坐标直接套到工作区文件上。
    #[gpui::test]
    fn deleted_excerpt_maps_to_working_tree_hunk_position(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时仓库");
        let root = directory.path().canonicalize().expect("应规范化仓库路径");
        run_in(&root, &["git", "init", "-q", "-b", "master"]);
        run_in(&root, &["git", "config", "user.email", "test@example.com"]);
        run_in(&root, &["git", "config", "user.name", "Test User"]);
        let modified_path = root.join("modified.txt");
        std::fs::write(
            &modified_path,
            "line0\nline1\nline2\nline3\n修改前\nline5\nline6\nline7\n",
        )
        .expect("应创建文件");
        std::fs::write(root.join("removed.txt"), "将被删除\n").expect("应创建文件");
        run_in(&root, &["git", "add", "."]);
        run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
        // 工作区：修改第 5 行（0-based 4），并删除 removed.txt。
        std::fs::write(
            &modified_path,
            "line0\nline1\nline2\nline3\n修改后\nline5\nline6\nline7\n",
        )
        .expect("应修改文件");
        std::fs::remove_file(root.join("removed.txt")).expect("应删除文件");

        let project = cx.new(|cx| Project::new(root.clone(), cx));
        let view = cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Unstaged, project, cx));
        cx.run_until_parked();
        cx.run_until_parked();

        // 工作区文件文本（与磁盘内容一致），供换算钳制行列。
        let working_text = Buffer::scratch(
            "line0\nline1\nline2\nline3\n修改后\nline5\nline6\nline7\n".to_owned(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建")
        .snapshot();

        cx.read_entity(&view, |view, cx| {
            let snapshot = view.multi_buffer.read(cx).snapshot(cx);
            // 修改行的 Deleted 片段：首行（旧侧 "修改前"）→ 工作区第 5 行（0-based 4）。
            let modified_excerpt = snapshot
                .excerpts()
                .iter()
                .find(|excerpt| {
                    excerpt.path() == modified_path
                        && excerpt.diff_kind() == Some(ExcerptDiffKind::Deleted)
                })
                .expect("修改行应有 Deleted 片段");
            let location = ExcerptLocation {
                path: modified_path.clone(),
                source_range: modified_excerpt.source_range(),
            };
            let target = view
                .deleted_navigation_target(&location, &working_text, cx)
                .expect("Deleted 片段应能换算到工作区行列");
            assert_eq!(target.0, modified_path);
            assert_eq!(target.1, 4, "修改行映射到工作区第 5 行（0-based 4）");
            assert_eq!(target.2, 0);
            // 行内位置：旧行 "修改前" 第 2 个字符（逻辑列 1）→ 工作区同列。
            let inner_location = ExcerptLocation {
                path: modified_path.clone(),
                source_range: zcv_text::TextRange::new(
                    zcv_text::ByteOffset::new(27),
                    zcv_text::ByteOffset::new(34),
                )
                .expect("旧行内范围"),
            };
            let inner_target = view
                .deleted_navigation_target(&inner_location, &working_text, cx)
                .expect("Deleted 片段行内位置应能换算");
            assert_eq!(
                (inner_target.1, inner_target.2),
                (4, 1),
                "修订行内列应映射到工作区同列"
            );
            assert_eq!(
                modified_excerpt.source_range(),
                zcv_text::TextRange::new(
                    zcv_text::ByteOffset::new(24),
                    zcv_text::ByteOffset::new(34),
                )
                .expect("旧侧第 5 行范围"),
                "夹具应让 Deleted 片段正好覆盖被修改的旧行（含行尾换行）"
            );
            // 整文件删除：纯删除 hunk 的 range 为空，锚定到变更块起点（0-based 0）。
            let removed_excerpt = snapshot
                .excerpts()
                .iter()
                .find(|excerpt| {
                    excerpt.path().file_name().and_then(|name| name.to_str()) == Some("removed.txt")
                        && excerpt.diff_kind() == Some(ExcerptDiffKind::Deleted)
                })
                .expect("删除文件应有 Deleted 片段");
            let removed_location = ExcerptLocation {
                path: removed_excerpt.path().to_path_buf(),
                source_range: removed_excerpt.source_range(),
            };
            // 已删除文件的工作区文本为空。
            let empty_text = Buffer::scratch(String::new(), BufferConfig::default())
                .expect("空 Buffer 应能创建")
                .snapshot();
            let removed_target = view
                .deleted_navigation_target(&removed_location, &empty_text, cx)
                .expect("删除片段应锚定到变更块起点");
            assert_eq!(removed_target.1, 0);
            assert_eq!(removed_target.2, 0);
        });
    }

    #[test]
    fn clamp_column_to_line_caps_at_line_length() {
        let snapshot = Buffer::scratch(
            "abc\n一个很长的中文行\n".to_owned(),
            BufferConfig::default(),
        )
        .expect("测试 Buffer 应能创建")
        .snapshot();
        assert_eq!(clamp_column_to_line(&snapshot, 0, 0), 0);
        assert_eq!(clamp_column_to_line(&snapshot, 0, 99), 3, "列应钳制到行尾");
        assert_eq!(clamp_column_to_line(&snapshot, 1, 1), 1);
        assert_eq!(
            clamp_column_to_line(&snapshot, 1, 99),
            8,
            "中文行按字符计数钳制"
        );
        assert_eq!(
            clamp_column_to_line(&snapshot, 99, 5),
            0,
            "越界行钳制到最后一行"
        );
    }

    #[gpui::test]
    fn partially_staged_file_has_distinct_staged_and_unstaged_views(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时仓库");
        let root = directory.path().canonicalize().expect("应规范化仓库路径");
        run_in(&root, &["git", "init", "-q", "-b", "master"]);
        run_in(&root, &["git", "config", "user.email", "test@example.com"]);
        run_in(&root, &["git", "config", "user.name", "Test User"]);
        let path = root.join("partial.txt");
        let original = (0..12)
            .map(|line| format!("line{line}"))
            .collect::<Vec<_>>();
        std::fs::write(&path, format!("{}\n", original.join("\n"))).expect("应创建文件");
        run_in(&root, &["git", "add", "partial.txt"]);
        run_in(&root, &["git", "commit", "-q", "-m", "initial"]);

        let mut staged = original.clone();
        staged[2] = "已暂存内容".into();
        std::fs::write(&path, format!("{}\n", staged.join("\n"))).expect("应写入暂存版本");
        run_in(&root, &["git", "add", "partial.txt"]);

        let mut worktree = staged;
        worktree[9] = "未暂存内容".into();
        std::fs::write(&path, format!("{}\n", worktree.join("\n"))).expect("应写入工作区版本");

        let project = cx.new(|cx| Project::new(root.clone(), cx));
        let staged_view =
            cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Staged, project.clone(), cx));
        let unstaged_view =
            cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Unstaged, project, cx));
        cx.run_until_parked();
        cx.run_until_parked();
        cx.run_until_parked();

        let staged_text = cx.read_entity(&staged_view, |view, cx| {
            String::from_utf8(view.multi_buffer.read(cx).snapshot(cx).text_bytes())
                .expect("已暂存投影应为 UTF-8")
        });
        let unstaged_text = cx.read_entity(&unstaged_view, |view, cx| {
            String::from_utf8(view.multi_buffer.read(cx).snapshot(cx).text_bytes())
                .expect("未暂存投影应为 UTF-8")
        });

        assert!(staged_text.contains("已暂存内容"));
        assert!(!staged_text.contains("未暂存内容"));
        assert!(unstaged_text.contains("未暂存内容"));
        assert!(!unstaged_text.contains("line2"));
        cx.read_entity(&staged_view, |view, cx| {
            assert!(view.multi_buffer.read(cx).is_read_only());
            assert_eq!(view.tab_content_text(cx), "已暂存更改");
            assert_eq!(
                view.tab_icon(cx).map(|icon| icon.to_string()),
                Some("icons/lock.svg".into())
            );
        });
        cx.read_entity(&unstaged_view, |view, cx| {
            assert!(!view.multi_buffer.read(cx).is_read_only());
            assert_eq!(view.tab_content_text(cx), "未暂存更改");
            assert_eq!(
                view.tab_icon(cx).map(|icon| icon.to_string()),
                Some("icons/diff.svg".into())
            );
        });
    }

    #[gpui::test]
    fn staging_one_hunk_rebuilds_the_projection_once_after_refresh(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时仓库");
        let root = directory.path().canonicalize().expect("应规范化仓库路径");
        run_in(&root, &["git", "init", "-q", "-b", "master"]);
        run_in(&root, &["git", "config", "user.email", "test@example.com"]);
        run_in(&root, &["git", "config", "user.name", "Test User"]);
        let path = root.join("two-hunks.txt");
        let original = (0..30)
            .map(|line| format!("line{line}"))
            .collect::<Vec<_>>();
        std::fs::write(&path, format!("{}\n", original.join("\n"))).expect("应创建文件");
        run_in(&root, &["git", "add", "two-hunks.txt"]);
        run_in(&root, &["git", "commit", "-q", "-m", "initial"]);

        let mut changed = original;
        changed[3] = "第一个变更块".into();
        changed[25] = "第二个变更块".into();
        std::fs::write(&path, format!("{}\n", changed.join("\n"))).expect("应修改文件");

        let project = cx.new(|cx| Project::new(root.clone(), cx));
        let view = cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Unstaged, project, cx));
        cx.run_until_parked();
        cx.run_until_parked();
        cx.run_until_parked();

        let (hunk, initial_version) = cx.read_entity(&view, |view, cx| {
            assert_eq!(view.hunk_targets.len(), 2);
            let version = view
                .multi_buffer
                .read(cx)
                .snapshot(cx)
                .text()
                .version()
                .get();
            (view.hunk_targets[0].displayed.clone(), version)
        });
        view.update(cx, |view, cx| {
            view.apply_hunk_action(&hunk, GitHunkOperation::Stage, cx)
        });
        cx.run_until_parked();
        cx.run_until_parked();
        cx.run_until_parked();

        cx.read_entity(&view, |view, cx| {
            let snapshot = view.multi_buffer.read(cx).snapshot(cx);
            let text = String::from_utf8(snapshot.text_bytes()).expect("投影应为 UTF-8");
            assert_eq!(snapshot.text().version().get(), initial_version + 1);
            assert_eq!(view.hunk_targets.len(), 1);
            assert!(!text.contains("第一个变更块"));
            assert!(text.contains("第二个变更块"));
        });
    }

    /// Git hunk 多文件编辑器默认展开；用户折叠后刷新仍保持折叠，且映射保持一致。
    #[gpui::test]
    fn expanding_hunk_then_refreshing_hunks_keeps_mapping_consistent(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时仓库");
        let root = directory.path().canonicalize().expect("应规范化仓库路径");
        run_in(&root, &["git", "init", "-q", "-b", "master"]);
        run_in(&root, &["git", "config", "user.email", "test@example.com"]);
        run_in(&root, &["git", "config", "user.name", "Test User"]);
        let modified_path = root.join("modified.txt");
        std::fs::write(&modified_path, "line0\nline1\nline2\nline3\nline4").expect("应创建文件");
        run_in(&root, &["git", "add", "."]);
        run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
        std::fs::write(&modified_path, "line0\n改过\nline2\nline3\nline4").expect("应修改文件");

        let project = cx.new(|cx| Project::new(root.clone(), cx));
        let view =
            cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Unstaged, project.clone(), cx));
        cx.run_until_parked();
        cx.run_until_parked();

        cx.read_entity(&view, |view, cx| {
            assert_eq!(
                view.editor.read(cx).expanded_modified_hunks(cx),
                &[1..2],
                "Git hunk 多文件编辑器应默认展开修改块"
            );
            let text = String::from_utf8(view.multi_buffer.read(cx).snapshot(cx).text_bytes())
                .expect("投影应为 UTF-8");
            assert!(text.contains("line1"), "默认展开时应包含旧侧文本");
            assert!(text.contains("改过"), "默认展开时应包含新侧文本");
        });

        // 用户折叠修改块。
        cx.update_entity(&view, |view, cx| {
            let editor = view.editor.clone();
            editor.update(cx, |editor, cx| editor.toggle_modified_hunk(1..2, cx));
        });
        cx.run_until_parked();
        cx.read_entity(&view, |view, cx| {
            assert!(view.editor.read(cx).expanded_modified_hunks(cx).is_empty());
            let text = String::from_utf8(view.multi_buffer.read(cx).snapshot(cx).text_bytes())
                .expect("投影应为 UTF-8");
            assert!(!text.contains("line1"), "折叠后旧侧文本应消失");
            assert!(text.contains("改过"), "折叠后新侧文本应保留");
        });

        // 触发 git hunks 刷新（模拟 git 状态变化）→ rebuild_excerpts → refresh_diff_hunks。
        project.update(cx, |project, cx| {
            let store = project.git_store();
            store.update(cx, |store, cx| {
                store.request_hunks(DiffBase::Index, &[modified_path.clone()], cx);
            });
        });
        cx.run_until_parked();
        cx.run_until_parked();
        cx.read_entity(&view, |view, cx| {
            assert!(!view.hunk_targets.is_empty(), "刷新后仍应保留项目差异映射");
            assert!(
                view.editor.read(cx).expanded_modified_hunks(cx).is_empty(),
                "刷新不能覆盖用户的折叠状态"
            );
        });
    }

    /// 复现：普通编辑器展开 hunk（singleton → excerpts）后触发 git hunks 刷新，
    /// 与 ProjectDiffView 共享仓库时不应让 refresh_diff_hunks 的片段匹配 panic。
    #[gpui::test]
    fn plain_editor_expansion_then_git_refresh_keeps_diff_view_consistent(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时仓库");
        let root = directory.path().canonicalize().expect("应规范化仓库路径");
        run_in(&root, &["git", "init", "-q", "-b", "master"]);
        run_in(&root, &["git", "config", "user.email", "test@example.com"]);
        run_in(&root, &["git", "config", "user.name", "Test User"]);
        let modified_path = root.join("modified.txt");
        std::fs::write(&modified_path, "line0\nline1\nline2\nline3\nline4\n").expect("应创建文件");
        run_in(&root, &["git", "add", "."]);
        run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
        std::fs::write(&modified_path, "line0\n改过\nline2\nline3\nline4\n").expect("应修改文件");

        let project = cx.new(|cx| Project::new(root.clone(), cx));
        // 普通编辑器：独立 excerpts 组合文档（整文件 excerpt，共享 singleton 只作工作区源），
        // 与 item_provider 打开路径一致；展开修改块。
        let working = project
            .update(cx, |project, cx| project.open_buffer(&modified_path, cx))
            .expect("工作区文件应能打开");
        // 统一经 from_working_source 构建独立组合文档（与 item_provider 同一路径）。
        let combined = cx.new(|cx| MultiBuffer::from_working_source(working.clone(), cx));
        let editor = cx.new(|cx| Editor::for_multi_buffer(combined, cx));
        editor.update(cx, |editor, cx| {
            editor.set_diff_hunks(
                Some(vec![DiffHunk {
                    range: 1..2,
                    old_range: 1..2,
                    kind: DiffHunkKind::Modified,
                }]),
                cx,
            );
            editor
                .set_deleted_hunk_text(Some(Arc::from("line0\nline1\nline2\nline3\nline4\n")), cx);
            editor.toggle_modified_hunk(1..2, cx);
        });
        // ProjectDiffView：同一仓库。
        let view =
            cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Unstaged, project.clone(), cx));
        cx.run_until_parked();
        cx.run_until_parked();

        // 触发 git hunks 刷新 → HunksChanged → 两个视图各自重建。
        project.update(cx, |project, cx| {
            let store = project.git_store();
            store.update(cx, |store, cx| {
                store.request_hunks(DiffBase::Index, &[modified_path.clone()], cx);
            });
        });
        cx.run_until_parked();
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert!(!view.hunk_targets.is_empty(), "刷新后仍应保留项目差异映射")
        });
    }

    /// 复现：普通编辑器展开 hunk 后编辑工作区（行数变化）再触发 git hunks 刷新，
    /// ProjectDiffView 的片段映射不应 panic。
    #[gpui::test]
    fn expansion_edit_then_refresh_keeps_diff_view_consistent(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时仓库");
        let root = directory.path().canonicalize().expect("应规范化仓库路径");
        run_in(&root, &["git", "init", "-q", "-b", "master"]);
        run_in(&root, &["git", "config", "user.email", "test@example.com"]);
        run_in(&root, &["git", "config", "user.name", "Test User"]);
        let modified_path = root.join("modified.txt");
        std::fs::write(
            &modified_path,
            "line0\nline1\nline2\nline3\nline4\nline5\nline6\nline7\n",
        )
        .expect("应创建文件");
        run_in(&root, &["git", "add", "."]);
        run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
        std::fs::write(
            &modified_path,
            "line0\n改过\nline2\nline3\nline4\nline5\nline6\nline7\n",
        )
        .expect("应修改文件");

        let project = cx.new(|cx| Project::new(root.clone(), cx));
        // 普通编辑器：独立 excerpts（item_provider 路径）+ 展开修改块。
        let working = project
            .update(cx, |project, cx| project.open_buffer(&modified_path, cx))
            .expect("工作区文件应能打开");
        // 统一经 from_working_source 构建独立组合文档（与 item_provider 同一路径）。
        let combined = cx.new(|cx| MultiBuffer::from_working_source(working.clone(), cx));
        let editor = cx.new(|cx| Editor::for_multi_buffer(combined, cx));
        editor.update(cx, |editor, cx| {
            editor.set_diff_hunks(
                Some(vec![DiffHunk {
                    range: 1..2,
                    old_range: 1..2,
                    kind: DiffHunkKind::Modified,
                }]),
                cx,
            );
            editor.set_deleted_hunk_text(
                Some(Arc::from(
                    "line0\nline1\nline2\nline3\nline4\nline5\nline6\nline7\n",
                )),
                cx,
            );
            editor.toggle_modified_hunk(1..2, cx);
        });
        // ProjectDiffView：同一仓库。
        let view =
            cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Unstaged, project.clone(), cx));
        cx.run_until_parked();
        cx.run_until_parked();

        // 编辑工作区（删除 "改过" 行 → 行数变化）。
        working.update(cx, |mb, cx| {
            mb.edit(
                vec![zcv_text::Edit::delete(
                    zcv_text::TextRange::new(
                        zcv_text::ByteOffset::new(6),
                        zcv_text::ByteOffset::new(13),
                    )
                    .expect("删除范围应有效"),
                )],
                zcv_text::TransactionMetadata::default(),
                cx,
            )
            .expect("工作区编辑应成功");
            cx.notify();
        });
        // 触发 git hunks 刷新。
        project.update(cx, |project, cx| {
            let store = project.git_store();
            store.update(cx, |store, cx| {
                store.request_hunks(DiffBase::Index, &[modified_path.clone()], cx);
            });
        });
        cx.run_until_parked();
        cx.run_until_parked();
        cx.read_entity(&view, |view, _| {
            assert!(!view.hunk_targets.is_empty(), "刷新后仍应保留项目差异映射")
        });
    }

    fn run_in(dir: &Path, args: &[&str]) {
        let output = Command::new(args[0])
            .args(&args[1..])
            .current_dir(dir)
            .output()
            .expect("应执行 Git 命令");
        assert!(
            output.status.success(),
            "命令 {args:?} 失败：{}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
