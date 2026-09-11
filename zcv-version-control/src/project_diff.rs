//! 已暂存/未暂存变更的多文件编辑器。
//!
//! GitStore 的状态快照决定文件集合，Project/BufferStore 继续拥有真实文件文档，MultiBuffer 只组合这些文档。
//! 点击版本管理条目时按分组复用对应 Item 并定位文件，不为 Git 状态建立界面侧副本。

use std::any::TypeId;
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use gpui::{
    AnyElement, AnyEntity, AnyView, App, Context, Entity, EventEmitter, FocusHandle, Focusable,
    KeyContext, ParentElement, Render, SharedString, Styled, Subscription, Task, WeakEntity,
    Window, div, prelude::*,
};
use zcv_actions::{
    Backtab, FindNext, FindPrevious, ReplaceAll, ReplaceNext, Tab, ToggleCaseSensitive,
    ToggleRegex, ToggleReplace, ToggleWholeWord,
};
use zcv_editor::{
    DiffHunkDelegate, Editor, EditorEvent, EditorHunk, EditorHunkMarkerKind, EditorHunkPart,
    EditorScrollAnchor, HunkControlTarget,
};
use zcv_git::{FileStatus, GitHunkOperation, GitRevision, StatusCode};
use zcv_language::LanguageBuffer;
use zcv_multi_buffer::{BufferDiff, BufferDiffInput, DiffFile, DiffProjection, DisplayHunk};
use zcv_multi_buffer::{ExcerptLocation, MultiBuffer, MultiBufferExcerpt};
use zcv_project::{GitStoreEvent, Project};
use zcv_text::{Anchor, Buffer, BufferConfig, ByteOffset, SearchQuery, Snapshot};
use zcv_theme::{color, space};
use zcv_ui::{
    Button, ButtonSize, ButtonStyle, Checkbox, MatchOption, MatchOptions, ReplaceInput,
    SearchInput, SvgIcon,
};
use zcv_workspace::{
    Direction, Item, ItemEvent, SearchableItem, SearchableItemHandle, SerializedItemProvider,
    SerializedPaneItem, Workspace,
};

use zcv_git::ConflictChoice;

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
        let (kind, is_created_file) = {
            let view = view.read(cx);
            let Some(info) = view.diff_hunk_source_info(hunk, cx) else {
                return div().into_any_element();
            };
            let is_created = view
                .files
                .iter()
                .find(|file| file.path == info.path)
                .is_some_and(|file| view.kind.is_created(file.status));
            (view.kind, is_created)
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
                let stage_hunk = hunk.clone();
                let restore_view = self.view.clone();
                let restore_hunk = hunk.clone();
                controls
                    .child(
                        Button::text(("stage-hunk", row), "暂存")
                            .size(ButtonSize::Medium)
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
                            .size(ButtonSize::Medium)
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
                            .size(ButtonSize::Medium)
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

    fn includes(self, status: FileStatus) -> bool {
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
    pending_path: Option<PathBuf>,
    refresh_scroll_anchor: Option<EditorScrollAnchor>,
    revision_sources: HashMap<(GitRevision, PathBuf), Entity<LanguageBuffer>>,
    loading_revision_text: HashSet<(GitRevision, PathBuf)>,
    search_options: MatchOptions,
    search_input: Option<Entity<Editor>>,
    replace_input: Option<Entity<Editor>>,
    show_replace: bool,
    search_subscriptions: Vec<Subscription>,
    _subscriptions: Vec<Subscription>,
    toolbar: Entity<ProjectDiffToolbar>,
}

struct ProjectDiffToolbar {
    view: WeakEntity<ProjectDiffView>,
}

impl Render for ProjectDiffToolbar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.view.upgrade().map_or_else(
            || div().into_any_element(),
            |view| {
                view.update(cx, |view, cx| view.ensure_search_input(window, cx));
                view.read(cx)
                    .render_search_bar(window, cx, self.view.clone())
            },
        )
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

    fn ensure_search_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.search_input.is_some() {
            return;
        }
        let search_input = cx.new(|cx| Editor::auto_height(1, Some(4), cx));
        let replace_input = cx.new(|cx| Editor::auto_height(1, Some(4), cx));
        search_input.update(cx, |editor, cx| editor.set_placeholder_text("搜索...", cx));
        replace_input.update(cx, |editor, cx| {
            editor.set_placeholder_text("替换为...", cx)
        });
        let weak = cx.weak_entity();
        self.search_subscriptions
            .push(window.subscribe(&search_input, cx, {
                let weak = weak.clone();
                move |_, event: &EditorEvent, window, cx| {
                    if *event == EditorEvent::Edited
                        && let Some(view) = weak.upgrade()
                    {
                        view.update(cx, |view, cx| view.run_search_from_input(window, cx));
                    }
                }
            }));
        self.search_input = Some(search_input);
        self.replace_input = Some(replace_input);
    }

    fn run_search_from_input(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        // 查询文本直接读输入框 Editor,不另存镜像。
        let query = SearchQuery {
            query: self
                .search_input
                .as_ref()
                .expect("搜索输入框应已创建")
                .read(cx)
                .text(cx),
            case_sensitive: self.search_options.case_sensitive,
            whole_word: self.search_options.whole_word,
            regex: self.search_options.regex,
        };
        self.editor.update(cx, |editor, cx| {
            SearchableItem::search(editor, &query, window, cx)
        });
        cx.notify();
    }

    fn toggle_search_option(
        &mut self,
        option: MatchOption,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.search_options = self.search_options.toggled(option);
        self.run_search_from_input(window, cx);
    }

    fn move_active_match(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.editor.update(cx, |editor, cx| {
            SearchableItem::activate_match_in_direction(editor, direction, 1, window, cx)
        });
        cx.notify();
    }

    fn toggle_replace(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.kind != ProjectDiffKind::Unstaged {
            return;
        }
        let closing = self.show_replace;
        self.show_replace = !self.show_replace;
        // 打开替换行时默认聚焦替换输入框。
        if self.show_replace
            && let Some(replace_input) = &self.replace_input
        {
            window.focus(&replace_input.read(cx).focus_handle(), cx);
        }
        // 收起替换行时把焦点还给搜索输入框。
        if closing && let Some(search_input) = &self.search_input {
            window.focus(&search_input.read(cx).focus_handle(), cx);
        }
        cx.notify();
    }

    /// 替换文本直接读替换输入框 Editor(实体常驻,展开/收起不销毁,无需镜像)。
    fn replacement_text(&self, cx: &Context<Self>) -> Option<String> {
        self.replace_input
            .as_ref()
            .map(|input| input.read(cx).text(cx))
    }

    fn replace_next(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(replacement) = self.replacement_text(cx) else {
            return;
        };
        let replaced = self.editor.update(cx, |editor, cx| {
            SearchableItem::replace_current(editor, &replacement, window, cx)
        });
        if replaced {
            self.move_active_match(Direction::Next, window, cx);
        }
    }

    fn replace_all(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(replacement) = self.replacement_text(cx) else {
            return;
        };
        self.editor.update(cx, |editor, cx| {
            SearchableItem::replace_all(editor, &replacement, window, cx)
        });
    }

    fn cycle_search_focus(
        &mut self,
        direction: Direction,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let mut handles = vec![
            self.search_input
                .as_ref()
                .expect("搜索输入框应已创建")
                .read(cx)
                .focus_handle(),
        ];
        if self.show_replace {
            handles.push(
                self.replace_input
                    .as_ref()
                    .expect("替换输入框应已创建")
                    .read(cx)
                    .focus_handle(),
            );
        }
        handles.push(self.focus_handle(cx));
        let Some(current) = handles.iter().position(|focus| focus.is_focused(window)) else {
            return;
        };
        let next = match direction {
            Direction::Next => (current + 1) % handles.len(),
            Direction::Prev => (current + handles.len() - 1) % handles.len(),
        };
        window.focus(&handles[next], cx);
        cx.stop_propagation();
    }

    fn render_search_bar(
        &self,
        _window: &mut Window,
        cx: &App,
        weak: WeakEntity<Self>,
    ) -> AnyElement {
        let colors = color::current(cx);
        let (match_count, active_match) = SearchableItem::search_count(self.editor.read(cx), cx);
        let expansion = {
            let editor = self.editor.read(cx);
            let expanded = self
                .files
                .iter()
                .any(|file| !editor.is_buffer_folded(&file.path));
            let weak = weak.clone();
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
        let restore = (self.kind == ProjectDiffKind::Unstaged).then(|| {
            let weak = weak.clone();
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
        let replace_toggle = (self.kind == ProjectDiffKind::Unstaged).then(|| {
            let weak = weak.clone();
            Button::icon("project-diff-toggle-replace", "icons/replace.svg")
                .label("替换")
                .shortcut(&ToggleReplace, cx)
                .color(if self.show_replace {
                    colors.icon_accent
                } else {
                    colors.text_muted
                })
                .on_click(move |_, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |view, cx| view.toggle_replace(window, cx));
                    }
                })
                .into_any_element()
        });
        // 替换行:替换开关打开时显示,通用替换输入框(外部插槽:替换 / 全部替换)。
        let replacement_line =
            (self.kind == ProjectDiffKind::Unstaged && self.show_replace).then(|| {
                let replace_action = {
                    let weak = weak.clone();
                    move |window: &mut Window, cx: &mut App| {
                        if let Some(view) = weak.upgrade() {
                            view.update(cx, |view, cx| view.replace_next(window, cx));
                        }
                    }
                };
                let replace_all_action = {
                    let weak = weak.clone();
                    move |window: &mut Window, cx: &mut App| {
                        if let Some(view) = weak.upgrade() {
                            view.update(cx, |view, cx| view.replace_all(window, cx));
                        }
                    }
                };
                ReplaceInput::new(
                    "project-diff-replace",
                    self.replace_input
                        .as_ref()
                        .expect("替换输入框应已创建")
                        .clone()
                        .into_any_element(),
                )
                .on_replace(replace_action)
                .on_replace_all(replace_all_action)
                .into_any_element()
            });
        let mut key_context = KeyContext::new_with_defaults();
        key_context.add("ProjectDiffSearchBar");
        // 替换行容器专属 in_replace 上下文:替换类快捷键只在替换输入框内生效(搜索输入框的焦点路径不携带该标签,行收起后自然失效)。
        let mut in_replace_context = KeyContext::new_with_defaults();
        in_replace_context.add("in_replace");
        div()
            .key_context(key_context)
            .flex()
            .flex_col()
            .gap(space::S6)
            .on_action({
                let weak = weak.clone();
                move |_: &FindNext, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |view, cx| {
                            view.move_active_match(Direction::Next, window, cx)
                        });
                    }
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &FindPrevious, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |view, cx| {
                            view.move_active_match(Direction::Prev, window, cx)
                        });
                    }
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &ToggleCaseSensitive, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |view, cx| {
                            view.toggle_search_option(MatchOption::CaseSensitive, window, cx)
                        });
                    }
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &ToggleWholeWord, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |view, cx| {
                            view.toggle_search_option(MatchOption::WholeWord, window, cx)
                        });
                    }
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &ToggleRegex, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |view, cx| {
                            view.toggle_search_option(MatchOption::Regex, window, cx)
                        });
                    }
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &ToggleReplace, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |view, cx| view.toggle_replace(window, cx));
                    }
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &ReplaceNext, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |view, cx| view.replace_next(window, cx));
                    }
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &ReplaceAll, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |view, cx| view.replace_all(window, cx));
                    }
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &Tab, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |view, cx| {
                            view.cycle_search_focus(Direction::Next, window, cx)
                        });
                    }
                }
            })
            .on_action({
                let weak = weak.clone();
                move |_: &Backtab, window, cx| {
                    if let Some(view) = weak.upgrade() {
                        view.update(cx, |view, cx| {
                            view.cycle_search_focus(Direction::Prev, window, cx)
                        });
                    }
                }
            })
            .child(
                // 行1:左侧展开控制 + 通用搜索输入框(内槽:匹配选项;外槽:计数与上/下导航;追加替换开关与重做全部)。
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .gap(space::S6)
                    .child(expansion)
                    .child(
                        div().flex_1().min_w_0().child(
                            SearchInput::new(
                                "project-diff",
                                self.search_input
                                    .as_ref()
                                    .expect("搜索输入框应已创建")
                                    .clone()
                                    .into_any_element(),
                            )
                            .options(self.search_options)
                            .on_toggle({
                                let weak = weak.clone();
                                move |option, window, cx| {
                                    if let Some(view) = weak.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.toggle_search_option(option, window, cx)
                                        });
                                    }
                                }
                            })
                            .count(active_match, match_count)
                            .on_previous({
                                let weak = weak.clone();
                                move |window, cx| {
                                    if let Some(view) = weak.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.move_active_match(Direction::Prev, window, cx)
                                        });
                                    }
                                }
                            })
                            .on_next({
                                let weak = weak.clone();
                                move |window, cx| {
                                    if let Some(view) = weak.upgrade() {
                                        view.update(cx, |view, cx| {
                                            view.move_active_match(Direction::Next, window, cx)
                                        });
                                    }
                                }
                            })
                            // 追加外部插槽:替换开关与重做全部(调用方按分组自选)。
                            .when_some(replace_toggle, |input, toggle| input.external(toggle))
                            .when_some(restore, |input, restore| input.external(restore)),
                        ),
                    ),
            )
            // 行2:替换开关展开后的替换行(通用替换输入框)。
            .when_some(replacement_line, |this, line| this.child(line))
            .into_any_element()
    }

    fn set_all_files_folded(&mut self, folded: bool, cx: &mut Context<Self>) {
        let paths = self
            .files
            .iter()
            .map(|file| file.path.clone())
            .collect::<Vec<_>>();
        self.editor.update(cx, |editor, cx| {
            for path in paths {
                if editor.is_buffer_folded(&path) != folded {
                    editor.toggle_buffer_fold(path, cx);
                }
            }
        });
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
            // 展开状态变化会改变组合片段：由 MultiBuffer 统一重建并通知宿主恢复视口。
            cx.subscribe(&editor, |view, _, event: &EditorEvent, cx| match event {
                EditorEvent::DiffHunksExpandedChanged => view.rebuild_projection(cx),
                EditorEvent::Edited
                | EditorEvent::PathChanged
                | EditorEvent::DirtyChanged
                | EditorEvent::OpenExcerptsRequested { .. }
                | EditorEvent::Error(_) => {}
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
                GitStoreEvent::HunkOperationFailed(message) => {
                    // 失败：GitStore 已清除 optimistic 状态，这里重建以恢复 hunk 并把错误交给宿主提示。
                    view.rebuild_projection(cx);
                    cx.emit(EditorEvent::Error(format!("变更块操作失败：{message}")));
                }
                GitStoreEvent::IndexText => view.rebuild_projection(cx),
                GitStoreEvent::ActiveRepositoryChanged
                | GitStoreEvent::JobsUpdated
                | GitStoreEvent::Uncommitted(_) => {}
            }),
        ];
        let toolbar_view = cx.entity().downgrade();
        let toolbar = cx.new(|_| ProjectDiffToolbar { view: toolbar_view });
        let mut view = Self {
            kind,
            project,
            empty_focus,
            editor,
            multi_buffer,
            files: Vec::new(),
            pending_path: None,
            refresh_scroll_anchor: None,
            revision_sources: Default::default(),
            loading_revision_text: Default::default(),
            search_options: MatchOptions::default(),
            search_input: None,
            replace_input: None,
            show_replace: false,
            search_subscriptions: Vec::new(),
            _subscriptions: subscriptions,
            toolbar,
        };
        view.refresh_files(cx);
        view
    }

    fn is_empty(&self, cx: &App) -> bool {
        self.multi_buffer
            .read(cx)
            .snapshot(cx)
            .excerpts()
            .is_empty()
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

        self.rebuild_projection(cx);
        self.load_missing_revision_text(cx);
    }

    /// 以 hunk 为核心重建可见 excerpts；旧侧与新侧都属于同一个 MultiBuffer 坐标空间。
    /// 收集所有变更文件的数据注入 MultiBuffer 统一投影；旧侧物化、上下文裁剪与显示坐标由本层统一完成。
    fn rebuild_projection(&mut self, cx: &mut Context<Self>) {
        if self.pending_path.is_none() && self.refresh_scroll_anchor.is_none() {
            self.refresh_scroll_anchor = self
                .editor
                .update(cx, |editor, cx| editor.capture_scroll_anchor(cx));
        }
        if !self.projection_data_ready(cx) {
            return;
        }
        if self.kind == ProjectDiffKind::Conflict {
            self.rebuild_conflict_projection(cx);
            if let Some(scroll_anchor) = self.refresh_scroll_anchor.take() {
                self.editor.update(cx, |editor, cx| {
                    editor.restore_scroll_anchor(scroll_anchor, cx);
                });
            }
            self.apply_pending_path(cx);
            cx.notify();
            return;
        }
        let root = self.project.read(cx).root().map(Path::to_path_buf);
        let mut diff_files = Vec::new();
        let files = self.files.clone();
        for file in &files {
            let Some(diff_file) = self.build_file_input(file, root.as_deref(), cx) else {
                continue;
            };
            diff_files.push(diff_file);
        }
        self.editor.update(cx, |editor, cx| {
            editor.set_editor_hunks(Vec::new(), cx);
            editor.set_diff_projection(Some(DiffProjection::new(diff_files)), cx)
        });
        if let Some(scroll_anchor) = self.refresh_scroll_anchor.take() {
            self.editor.update(cx, |editor, cx| {
                editor.restore_scroll_anchor(scroll_anchor, cx);
            });
        }
        self.apply_pending_path(cx);
        cx.notify();
    }

    /// 冲突视图使用完整工作区文本，不创建 BufferDiff；
    /// 冲突块和区域装饰由同一份解析结果注入 Editor。
    fn rebuild_conflict_projection(&mut self, cx: &mut Context<Self>) {
        let mut excerpts = Vec::new();
        for file in &self.files {
            let Ok(source) = self
                .project
                .update(cx, |project, cx| project.open_buffer(&file.path, cx))
            else {
                continue;
            };
            let source_len = source.read(cx).text_snapshot(cx).len_bytes();
            let Ok(source_range) = zcv_text::TextRange::new(ByteOffset::ZERO, source_len) else {
                continue;
            };
            excerpts.push(
                MultiBufferExcerpt::new(source, source_range, Vec::new())
                    .with_display_path(file.path.clone()),
            );
        }
        self.multi_buffer.update(cx, |buffer, cx| {
            buffer.set_diff_projection(Some(DiffProjection::empty()), cx);
            buffer.set_excerpts(excerpts, cx);
        });
        let hunks = self.conflict_editor_hunks(cx);
        self.editor.update(cx, |editor, cx| {
            editor.set_editor_hunks(hunks, cx);
        });
    }

    fn conflict_editor_hunks(&self, cx: &App) -> Vec<EditorHunk> {
        let snapshot = self.multi_buffer.read(cx).snapshot(cx);
        let mut hunks = Vec::new();
        for (buffer, path) in self.multi_buffer.read(cx).file_buffers(cx) {
            let source = buffer.read(cx).snapshot();
            let Ok(text_range) = zcv_text::TextRange::new(ByteOffset::ZERO, source.len_bytes())
            else {
                continue;
            };
            let Ok(text) = source.slice_text(text_range) else {
                continue;
            };
            let text = text.to_string();
            for (index, region) in zcv_git::parse_conflict_regions(&text).iter().enumerate() {
                let Some(excerpt) = snapshot.excerpts().iter().find(|excerpt| {
                    excerpt.path() == path
                        && excerpt
                            .source_range()
                            .contains(ByteOffset::new(region.outer.start))
                }) else {
                    continue;
                };
                let output_offset = |offset: usize| {
                    ByteOffset::new(
                        excerpt.output_range().start().get()
                            + offset.saturating_sub(excerpt.source_range().start().get()),
                    )
                };
                let Ok(range) = zcv_text::TextRange::new(
                    output_offset(region.outer.start),
                    output_offset(region.outer.end),
                ) else {
                    continue;
                };
                hunks.push(EditorHunk {
                    id: format!("{}\n{index}", path.display()).into(),
                    range,
                    parts: vec![
                        EditorHunkPart {
                            range: zcv_text::TextRange::new(
                                output_offset(region.outer.start),
                                output_offset(region.theirs.start),
                            )
                            .expect("冲突当前侧范围必须有效"),
                            content_kind: zcv_git::DiffHunkKind::Deleted,
                            marker_kind: EditorHunkMarkerKind::Conflict,
                        },
                        EditorHunkPart {
                            range: zcv_text::TextRange::new(
                                output_offset(region.theirs.start),
                                output_offset(region.outer.end),
                            )
                            .expect("冲突传入侧范围必须有效"),
                            content_kind: zcv_git::DiffHunkKind::Added,
                            marker_kind: EditorHunkMarkerKind::Conflict,
                        },
                    ]
                    .into(),
                });
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
            ProjectDiffKind::Staged => {
                let index_text = git_store
                    .read(cx)
                    .revision_text(GitRevision::Index, &file.path)
                    .map(|text| text.to_string())?;
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
        let base_text = if self.kind == ProjectDiffKind::Conflict {
            None
        } else {
            git_store
                .read(cx)
                .revision_text(self.kind.base_revision(), &file.path)
        };
        // index 参照：已暂存视图 working 就是 index，未暂存视图 base 就是 index；
        // 统一分类自然得到全部 Staged / 全部 Unstaged。
        let index_text = (self.kind != ProjectDiffKind::Conflict)
            .then(|| {
                git_store
                    .read(cx)
                    .revision_text(GitRevision::Index, &file.path)
            })
            .flatten();
        let display_path = root
            .and_then(|root| file.path.strip_prefix(root).ok())
            .unwrap_or(&file.path)
            .to_path_buf();
        let input = BufferDiffInput {
            working,
            base_text,
            index_text,
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
            show_file_header: true,
        })
    }

    /// Git 状态、hunk 与修订文本分批到达；全部就绪前保留旧投影，避免中间空态破坏视口锚点。
    fn projection_data_ready(&self, cx: &App) -> bool {
        let git_store = self.project.read(cx).git_store();
        let store = git_store.read(cx);
        self.files.iter().all(|file| {
            // 已加载即视为就绪：新建文件在 base 修订中缺失（值为 None）也是终态。
            (self.kind == ProjectDiffKind::Conflict
                || store.revision_text_loaded(self.kind.base_revision(), &file.path))
                && (self.kind != ProjectDiffKind::Staged
                    || store.revision_text_loaded(GitRevision::Index, &file.path))
        })
    }

    /// 显示 hunk 的源定位（hunk 操作与导航用）：按显示坐标反查源文件与源 hunk。
    fn diff_hunk_source_info(
        &self,
        displayed: &DisplayHunk,
        cx: &App,
    ) -> Option<zcv_multi_buffer::DiffHunkSource> {
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
        git_store.update(cx, |store, cx| match operation {
            GitHunkOperation::Stage => store.stage_paths(vec![path.to_path_buf()], cx),
            GitHunkOperation::Unstage => store.unstage_paths(vec![path.to_path_buf()], cx),
            GitHunkOperation::Restore => unreachable!("文件复选框不执行工作区还原"),
        });
    }

    fn apply_hunk_action(
        &mut self,
        displayed: &DisplayHunk,
        operation: GitHunkOperation,
        cx: &mut Context<Self>,
    ) {
        let Some(info) = self.diff_hunk_source_info(displayed, cx) else {
            return;
        };
        let is_created_file = info.diff.read(cx).is_created();
        let allowed = matches!(
            (self.kind, operation),
            (ProjectDiffKind::Unstaged, GitHunkOperation::Stage)
                | (ProjectDiffKind::Unstaged, GitHunkOperation::Restore)
                | (ProjectDiffKind::Staged, GitHunkOperation::Unstage)
        );
        if !allowed || (operation == GitHunkOperation::Restore && is_created_file) {
            return;
        }

        if let Some(range) = info.range {
            let diff = info.diff.clone();
            let Some(operations) = diff.read(cx).operations() else {
                return;
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
                }
            }
        } else {
            // 整文件新增块没有行级 hunk：按路径整体暂存/取消暂存。
            let git_store = self.project.read(cx).git_store();
            git_store.update(cx, |store, cx| match operation {
                GitHunkOperation::Stage => store.stage_paths(vec![info.path.clone()], cx),
                GitHunkOperation::Unstage => store.unstage_paths(vec![info.path.clone()], cx),
                GitHunkOperation::Restore => {}
            });
        }
        // 行级操作写入 optimistic pending 后由 BufferDiffEvent::DiffChanged 驱动物化；
        // 整文件路径操作由 GitStore 状态事件刷新。
    }

    fn load_missing_revision_text(&mut self, cx: &mut Context<Self>) {
        let git_store = self.project.read(cx).git_store();
        for file in &self.files {
            let mut revisions = if self.kind != ProjectDiffKind::Conflict {
                vec![self.kind.base_revision()]
            } else {
                Vec::new()
            };
            if self.kind == ProjectDiffKind::Staged && !revisions.contains(&GitRevision::Index) {
                revisions.push(GitRevision::Index);
            }
            for revision in revisions {
                if git_store
                    .read(cx)
                    .revision_text_loaded(revision, &file.path)
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
                        view.rebuild_projection(cx);
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
    ) -> Entity<LanguageBuffer> {
        let key = (revision, path.to_path_buf());
        if let Some(source) = self.revision_sources.get(&key) {
            let snapshot = source.read(cx).text_snapshot(cx);
            let source_text = snapshot
                .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
                .expect("Git 修订快照范围必须有效")
                .as_str()
                .to_owned();
            if source_text != text {
                let buffer = source.read(cx).buffer();
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
        let source = cx.new(|cx| LanguageBuffer::new(buffer, Some(path.to_path_buf()), cx));
        self.revision_sources.insert(key, source.clone());
        source
    }

    fn move_to_path(&mut self, path: PathBuf, cx: &mut Context<Self>) {
        self.refresh_scroll_anchor = None;
        self.pending_path = Some(path);
        self.apply_pending_path(cx);
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

    fn toolbar_view(&self, _self_handle: &Entity<Self>, _cx: &App) -> Option<AnyView> {
        Some(self.toolbar.clone().into())
    }

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
    ) -> Task<anyhow::Result<Box<dyn zcv_workspace::ItemHandle>>> {
        let result = project_diff_state(&state).map(|(kind, active_path)| {
            let view = cx.new(|cx| ProjectDiffView::new(kind, project, cx));
            if let Some(path) = active_path {
                view.update(cx, |view, cx| view.move_to_path(path, cx));
            }
            subscribe_to_open_excerpts(&view, window, cx);
            Box::new(view) as Box<dyn zcv_workspace::ItemHandle>
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
                let text = cx.read_entity(&buffer, |buffer, cx| buffer.text_snapshot(cx));
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
mod tests {
    use std::process::Command;

    use gpui::{AppContext as _, TestAppContext};

    use zcv_multi_buffer::ExcerptDiffKind;
    use zcv_text::Line;

    #[test]
    fn project_diff_persistence_state_keeps_group_and_active_path() {
        let (kind, active_path) = project_diff_state(&serde_json::json!({
            "kind": "unstaged",
            "active_path": "src/main.rs",
        }))
        .expect("有效的项目差异状态应能恢复");
        assert_eq!(kind, ProjectDiffKind::Unstaged);
        assert_eq!(active_path, Some(PathBuf::from("src/main.rs")));

        assert!(project_diff_state(&serde_json::json!({ "kind": "unknown" })).is_err());
    }

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

    use super::*;

    /// 测试辅助：按工作区源与 base 全文预创建普通编辑器 diff 注入项。
    fn plain_diff_file(
        working: Entity<LanguageBuffer>,
        base_text: &str,
        path: PathBuf,
        cx: &mut Context<Editor>,
    ) -> DiffFile {
        let diff = cx.new(|cx| {
            BufferDiff::new(
                BufferDiffInput {
                    working,
                    base_text: Some(Arc::from(base_text)),
                    index_text: None,
                    path: path.clone(),
                    operations: None,
                },
                cx,
            )
        });
        DiffFile {
            diff,
            display_path: path,
            context_lines: None,
            show_file_header: false,
        }
    }

    #[gpui::test]
    fn empty_project_diff_renders_blank_focusable_view(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let project = cx.new(|cx| Project::new(directory.path().to_path_buf(), cx));
        let (view, cx) = cx.add_window_view(move |_, cx| {
            ProjectDiffView::new(ProjectDiffKind::Staged, project, cx)
        });
        cx.run_until_parked();
        let _ = cx.refresh();
        cx.update(|_, _| {});
        cx.run_until_parked();

        let focus = cx.read_entity(&view, |view, cx| {
            assert!(view.is_empty(cx));
            assert!(view.focus_handle(cx) == view.empty_focus);
            assert!(
                view.editor.read(cx).cursor_text(cx).is_empty(),
                "无变更文件时底栏不应显示光标行列"
            );
            view.focus_handle(cx)
        });
        assert!(
            cx.debug_bounds("empty-project-diff-view").is_some(),
            "空项目差异应渲染纯空白容器"
        );
        assert!(
            cx.debug_bounds("project-diff-view").is_none(),
            "空项目差异不应渲染 Editor"
        );
        cx.update(|window, cx| window.focus(&focus, cx));
        cx.update(|window, _| {
            assert!(focus.is_focused(window), "空白区域仍应能持有 Item 焦点");
        });
    }

    #[gpui::test]
    fn project_diff_keeps_hunk_interest_while_its_multibuffer_is_empty(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时仓库");
        let root = directory.path().canonicalize().expect("应规范化仓库路径");
        run_in(&root, &["git", "init", "-q", "-b", "master"]);
        run_in(&root, &["git", "config", "user.email", "test@example.com"]);
        run_in(&root, &["git", "config", "user.name", "Test User"]);
        let path = root.join("tracked.txt");
        std::fs::write(&path, "line0\nline1\nline2\n原内容\nline4\nline5\nline6\n")
            .expect("应创建文件");
        run_in(&root, &["git", "add", "tracked.txt"]);
        run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
        std::fs::write(&path, "line0\nline1\nline2\n新内容\nline4\nline5\nline6\n")
            .expect("应修改文件");

        let project = cx.new(|cx| Project::new(root.clone(), cx));
        let view = cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Unstaged, project, cx));

        // ProjectDiffView 创建时 MultiBuffer 仍为空，但应立即向 GitStore 声明文件 hunk 需求。
        cx.run_until_parked();
        cx.run_until_parked();

        cx.read_entity(&view, |view, cx| {
            let multi_buffer = view.multi_buffer(cx).expect("项目差异应提供组合文档");
            let text = String::from_utf8(multi_buffer.read(cx).snapshot(cx).text_bytes())
                .expect("投影文本应为 UTF-8");
            assert_eq!(text, "line1\nline2\n原内容\n新内容\nline4\nline5\n");
        });
    }

    /// 端到端：git 删除文件中间一行（第 17 行，1-based）后，展开的被删行必须投影到其原始位置（组合第 17 行），上下文行顺序不重排。
    #[gpui::test]
    fn deleted_middle_row_projects_to_its_original_position(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时仓库");
        let root = directory.path().canonicalize().expect("应规范化仓库路径");
        run_in(&root, &["git", "init", "-q", "-b", "master"]);
        run_in(&root, &["git", "config", "user.email", "test@example.com"]);
        run_in(&root, &["git", "config", "user.name", "Test User"]);
        let path = root.join("readme.md");
        let original = (1..=20)
            .map(|line| format!("line {line}"))
            .collect::<Vec<_>>();
        std::fs::write(&path, original.join("\n")).expect("应创建文件");
        run_in(&root, &["git", "add", "."]);
        run_in(&root, &["git", "commit", "-q", "-m", "initial"]);
        // 删除第 17 行（1-based）。
        let mut changed = original.clone();
        changed.remove(16);
        std::fs::write(&path, changed.join("\n")).expect("应写入删除后的文件");

        let project = cx.new(|cx| Project::new(root.clone(), cx));
        let view = cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Unstaged, project, cx));
        cx.run_until_parked();
        cx.run_until_parked();

        // 默认展开（上下文裁剪 ±2 行）：被删行（line 17）显示在 line 16 之后、line 18 之前，即其原始位置，上下文行顺序不重排。
        cx.read_entity(&view, |view, cx| {
            let text = String::from_utf8(view.multi_buffer.read(cx).snapshot(cx).text_bytes())
                .expect("投影应为 UTF-8");
            let text_lines = text.split('\n').collect::<Vec<_>>();
            assert_eq!(
                text_lines.len(),
                6,
                "展开后应显示 hunk 上下文（±2 行）+ 旧侧行"
            );
            assert_eq!(text_lines[1], "line 16", "上下文第 16 行顺序保持");
            assert_eq!(
                text_lines[2], "line 17",
                "被删行应投影到 line 16 之后（原始位置）"
            );
            assert_eq!(text_lines[3], "line 18", "被删行后的行顺序保持");
        });

        // 折叠删除块：删除点锚定在 0-based 16 行（组合 16/17 行边界，line 18 行首）。
        cx.update_entity(&view, |view, cx| {
            let editor = view.editor.clone();
            editor.update(cx, |editor, cx| editor.toggle_diff_hunk_at(0, cx));
        });
        cx.run_until_parked();
        cx.read_entity(&view, |view, cx| {
            let text = String::from_utf8(view.multi_buffer.read(cx).snapshot(cx).text_bytes())
                .expect("投影应为 UTF-8");
            let text_lines = text.split('\n').collect::<Vec<_>>();
            assert_eq!(
                text_lines.len(),
                6,
                "折叠后应显示 hunk 上下文（±2 行）+ 删除点占位行"
            );
            assert!(!text_lines.contains(&"line 17"), "折叠后旧侧行应消失");
            assert_eq!(text_lines[1], "line 16", "折叠后第 16 行保持");
            assert_eq!(text_lines[2], "", "折叠后删除点占位行（原 line 17 位置）");
            assert_eq!(text_lines[3], "line 18", "折叠后原第 18 行紧跟删除点占位行");
            // 折叠删除块保留一个 hunk（显示坐标为组合坐标，不在此断言源行号）。
            let hunks = view.multi_buffer.read(cx).diff_hunks().to_vec();
            assert_eq!(hunks.len(), 1, "应保留一个删除 hunk");
        });
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
            let hunks = view.multi_buffer.read(cx).diff_hunks().to_vec();
            assert_eq!(hunks.len(), 2);
            let version = view
                .multi_buffer
                .read(cx)
                .snapshot(cx)
                .text()
                .version()
                .get();
            (hunks[0].clone(), version)
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
            assert_eq!(view.multi_buffer.read(cx).diff_hunks().len(), 1);
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
            assert!(
                view.multi_buffer
                    .read(cx)
                    .diff_hunk_expanded()
                    .iter()
                    .all(|&expanded| expanded),
                "Git hunk 多文件编辑器应默认展开修改块"
            );
            let text = String::from_utf8(view.multi_buffer.read(cx).snapshot(cx).text_bytes())
                .expect("投影应为 UTF-8");
            assert!(text.contains("line1"), "默认展开时应包含旧侧文本");
            assert!(text.contains("改过"), "默认展开时应包含新侧文本");
        });

        cx.update_entity(&view, |view, cx| view.set_all_files_folded(true, cx));
        cx.read_entity(&view, |view, cx| {
            assert!(
                view.editor.read(cx).is_buffer_folded(&modified_path),
                "折叠全部文件后应折叠文件块"
            );
        });
        cx.update_entity(&view, |view, cx| view.set_all_files_folded(false, cx));
        cx.read_entity(&view, |view, cx| {
            assert!(
                !view.editor.read(cx).is_buffer_folded(&modified_path),
                "展开全部文件后应展开文件块"
            );
        });

        // 用户折叠修改块。
        cx.update_entity(&view, |view, cx| {
            let editor = view.editor.clone();
            editor.update(cx, |editor, cx| editor.toggle_diff_hunk_at(0, cx));
        });
        cx.run_until_parked();
        cx.read_entity(&view, |view, cx| {
            assert!(
                !view
                    .multi_buffer
                    .read(cx)
                    .diff_hunk_expanded()
                    .iter()
                    .any(|&expanded| expanded)
            );
            let text = String::from_utf8(view.multi_buffer.read(cx).snapshot(cx).text_bytes())
                .expect("投影应为 UTF-8");
            assert!(!text.contains("line1"), "折叠后旧侧文本应消失");
            assert!(text.contains("改过"), "折叠后新侧文本应保留");
        });

        // 触发 Git 状态刷新 → MultiBuffer 由 base/working 快照统一重建投影。
        project.update(cx, |project, cx| {
            let store = project.git_store();
            store.update(cx, |store, cx| {
                store.refresh_statuses_for_paths(std::slice::from_ref(&modified_path), cx);
            });
        });
        cx.run_until_parked();
        cx.run_until_parked();
        cx.read_entity(&view, |view, cx| {
            assert!(
                !view.multi_buffer.read(cx).diff_hunks().is_empty(),
                "刷新后仍应保留项目差异映射"
            );
            assert!(
                !view
                    .multi_buffer
                    .read(cx)
                    .diff_hunk_expanded()
                    .iter()
                    .any(|&expanded| expanded),
                "刷新不能覆盖用户的折叠状态"
            );
        });
    }

    /// 复现：普通编辑器展开 hunk（singleton → excerpts）后触发 git hunks 刷新，与 ProjectDiffView 共享仓库时不应让统一投影重建 panic。
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
        // 普通编辑器：独立 excerpts 组合文档（整文件 excerpt，共享 LanguageBuffer 只作工作区源），与 item_provider 打开路径一致；展开修改块。
        let working = project
            .update(cx, |project, cx| project.open_buffer(&modified_path, cx))
            .expect("工作区文件应能打开");
        // 统一经 from_working_source 构建独立组合文档（与 item_provider 同一路径）。
        let combined = cx.new(|cx| MultiBuffer::from_working_source(working.clone(), cx));
        let editor = cx.new(|cx| Editor::for_multi_buffer(combined, cx));
        editor.update(cx, |editor, cx| {
            editor.set_diff_projection(
                Some(DiffProjection::new(vec![plain_diff_file(
                    working.clone(),
                    "line0\nline1\nline2\nline3\nline4\n",
                    modified_path.clone(),
                    cx,
                )])),
                cx,
            );
            editor.toggle_diff_hunk_at(0, cx);
        });
        // ProjectDiffView：同一仓库。
        let view =
            cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Unstaged, project.clone(), cx));
        cx.run_until_parked();
        cx.run_until_parked();

        // 触发 Git 状态刷新，两个视图各自重建。
        project.update(cx, |project, cx| {
            let store = project.git_store();
            store.update(cx, |store, cx| {
                store.refresh_statuses_for_paths(std::slice::from_ref(&modified_path), cx);
            });
        });
        cx.run_until_parked();
        cx.run_until_parked();
        cx.read_entity(&view, |view, cx| {
            assert!(
                !view.multi_buffer.read(cx).diff_hunks().is_empty(),
                "刷新后仍应保留项目差异映射"
            )
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
            editor.set_diff_projection(
                Some(DiffProjection::new(vec![plain_diff_file(
                    working.clone(),
                    "line0\nline1\nline2\nline3\nline4\nline5\nline6\nline7\n",
                    modified_path.clone(),
                    cx,
                )])),
                cx,
            );
            editor.toggle_diff_hunk_at(0, cx);
        });
        // ProjectDiffView：同一仓库。
        let view =
            cx.new(|cx| ProjectDiffView::new(ProjectDiffKind::Unstaged, project.clone(), cx));
        cx.run_until_parked();
        cx.run_until_parked();

        // 编辑工作区（删除 "改过" 行 → 行数变化）。
        let engine_buffer = cx.read_entity(&working, |working, _| working.buffer());
        engine_buffer.update(cx, |buffer, cx| {
            buffer
                .edit(
                    vec![zcv_text::Edit::delete(
                        zcv_text::TextRange::new(
                            zcv_text::ByteOffset::new(6),
                            zcv_text::ByteOffset::new(13),
                        )
                        .expect("删除范围应有效"),
                    )],
                    zcv_text::TransactionMetadata::default(),
                )
                .expect("工作区编辑应成功");
            cx.notify();
        });
        // 触发 Git 状态刷新。
        project.update(cx, |project, cx| {
            let store = project.git_store();
            store.update(cx, |store, cx| {
                store.refresh_statuses_for_paths(std::slice::from_ref(&modified_path), cx);
            });
        });
        cx.run_until_parked();
        cx.run_until_parked();
        cx.read_entity(&view, |view, cx| {
            assert!(
                !view.multi_buffer.read(cx).diff_hunks().is_empty(),
                "刷新后仍应保留项目差异映射"
            )
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
