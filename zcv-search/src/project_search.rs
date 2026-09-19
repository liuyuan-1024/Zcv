//! 项目级跨文件内容搜索视图。
//!
//! 与文件内搜索复用 SearchBar UI、快捷键和查询协议，但持有独立状态机；
//! 本 Item 搜索整个 Project，并把 ordered excerpts 写入 MultiBuffer。

use std::any::TypeId;
use std::path::PathBuf;
use std::time::Duration;

use gpui::{
    AnyEntity, App, Context, Entity, EventEmitter, FocusHandle, Focusable, ParentElement, Render,
    SharedString, Styled, Subscription, Task, WeakEntity, Window, div, prelude::*,
};
use zcv_actions::DeployProjectSearch;
use zcv_editor::{Editor, EditorEvent};
use zcv_multi_buffer::{ExcerptLocation, ExcerptRange, MultiBuffer};
use zcv_project::Project;
use zcv_project::SearchQuery;
use zcv_theme::color;
use zcv_ui::{Button, MatchOptions};
use zcv_workspace::{
    Direction, Item, ItemEvent, ItemHandle, SearchEvent, SearchableItem, SearchableItemHandle,
    SerializedItemProvider, SerializedPaneItem, StatusItemView, ToolbarItemEvent,
    ToolbarItemLocation, ToolbarItemView, WeakSearchableItemHandle, Workspace,
};

use crate::{SearchBar, SearchBarConfig, SearchBarSlots};

const PROJECT_SEARCH_SERIALIZED_KIND: &str = "project-search";

/// 项目搜索标签自身持久化的查询状态。
///
/// 选项以 [`MatchOptions`] 表达并扁平序列化,旧布局中的 `case_sensitive` 等 JSON 键保持不变。
#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
struct ProjectSearchState {
    query: String,
    #[serde(flatten)]
    options: MatchOptions,
}

#[derive(Clone, Debug)]
pub(crate) enum ProjectSearchEvent {
    Updated,
    Edited,
    DirtyChanged,
    OpenExcerptsRequested(Vec<ExcerptLocation>),
}

/// 输入防抖窗口：快速连续击键合并为一次全项目扫描。
const SEARCH_DEBOUNCE: Duration = Duration::from_millis(100);
/// 流式装配的批大小：收满该数量片段才追加一次 MultiBuffer。
const SEARCH_BATCH_SIZE: usize = 16;
/// 批次间的让出间隔：每批装配后给主循环一次重绘机会。
const SEARCH_BATCH_YIELD: Duration = Duration::from_millis(1);

pub(crate) struct ProjectSearchView {
    project: Entity<Project>,
    results_editor: Entity<Editor>,
    excerpts: Entity<MultiBuffer>,
    search_generation: u64,
    debounce_task: Option<Task<()>>,
    pending_search: Option<Task<()>>,
    /// 共享搜索栏会话：查询、匹配选项、可见性与按键接线由它唯一持有。
    search_bar: Entity<SearchBar>,
    _subscriptions: Vec<Subscription>,
}

/// 项目搜索的工具栏视图。
///
/// 作为 Pane 工具项存在：活动 Item 是项目搜索视图时显示搜索栏，否则隐藏；
/// 搜索目标是项目搜索视图本身（它实现 SearchableItem，驱动项目级搜索）。
pub(crate) struct ProjectSearchToolbar {
    active_view: Option<Entity<ProjectSearchView>>,
    search_bar: Option<Entity<SearchBar>>,
}

impl ProjectSearchToolbar {
    pub(crate) fn new() -> Self {
        Self {
            active_view: None,
            search_bar: None,
        }
    }
}

impl EventEmitter<ToolbarItemEvent> for ProjectSearchToolbar {}

impl ToolbarItemView for ProjectSearchToolbar {
    fn set_active_pane_item(
        &mut self,
        item: Option<&dyn ItemHandle>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        self.active_view = item.and_then(|item| item.act_as::<ProjectSearchView>(cx));
        let Some(view) = self.active_view.clone() else {
            if let Some(bar) = self.search_bar.take() {
                bar.update(cx, |bar, cx| bar.set_target(None, window, cx));
            }
            return ToolbarItemLocation::Hidden;
        };
        let bar = view.read(cx).search_bar.clone();
        // 同一视图重复激活时保留搜索会话；
        // 切换到另一视图时解除旧栏目标绑定，仅保留各视图自身的会话状态。
        if let Some(previous) = self.search_bar.replace(bar.clone())
            && previous.entity_id() != bar.entity_id()
        {
            previous.update(cx, |bar, cx| bar.set_target(None, window, cx));
        }
        // 项目搜索的搜索目标是视图自身：它的 SearchableItem 驱动全项目扫描。
        // 目标以弱句柄保存，视图持有搜索栏也不会与其构成强引用环。
        let target: Box<dyn WeakSearchableItemHandle> = Box::new(view.downgrade());
        bar.update(cx, |bar, cx| bar.set_target(Some(target), window, cx));
        ToolbarItemLocation::PrimaryLeft
    }
}

impl Render for ProjectSearchToolbar {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let Some(view) = self.active_view.clone() else {
            return div().into_any_element();
        };
        let Some(search_bar) = self.search_bar.clone() else {
            return div().into_any_element();
        };
        // 折叠全部文件属于宿主领域逻辑，作为搜索栏左侧插槽。
        let leading = {
            let weak = view.downgrade();
            let results_editor = view.read(cx).results_editor.clone();
            let snapshot = view.read(cx).excerpts.read(cx).snapshot(cx);
            let expanded = snapshot
                .excerpts()
                .any(|excerpt| !results_editor.read(cx).is_buffer_folded(excerpt.path(), cx));
            Button::icon(
                "project-search-expansion",
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
        let slots = SearchBarSlots {
            leading: Some(leading),
            external: Vec::new(),
        };
        search_bar
            .update(cx, |bar, cx| bar.render(slots, window, cx))
            .into_any_element()
    }
}

pub(crate) struct ProjectSearchSerializedItemProvider;

impl SerializedItemProvider for ProjectSearchSerializedItemProvider {
    fn kind(&self) -> &'static str {
        PROJECT_SEARCH_SERIALIZED_KIND
    }

    fn restore(
        &self,
        state: serde_json::Value,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Task<anyhow::Result<Box<dyn ItemHandle>>> {
        let state: ProjectSearchState = match serde_json::from_value(state) {
            Ok(state) => state,
            Err(error) => {
                return Task::ready(Err(anyhow::anyhow!("项目搜索标签状态无效：{error}")));
            }
        };
        let view = cx.new(|cx| ProjectSearchView::new(project, cx));
        view.update(cx, |view, cx| {
            let search_bar = view.search_bar.clone();
            search_bar.update(cx, |bar, cx| bar.restore(&state.query, state.options, cx));
            search_bar.update(cx, |bar, cx| bar.deploy(None, window, cx));
        });
        subscribe_to_open_excerpts(&view, window, cx);
        Task::ready(Ok(Box::new(view) as Box<dyn ItemHandle>))
    }
}

impl ProjectSearchView {
    pub(crate) fn new(project: Entity<Project>, cx: &mut Context<Self>) -> Self {
        let excerpts = cx.new(MultiBuffer::empty);
        let results_editor = cx.new(|cx| Editor::for_multi_buffer(excerpts.clone(), cx));
        let subscriptions = vec![
            cx.observe(&results_editor, |_, _, cx| cx.notify()),
            cx.subscribe(
                &results_editor,
                |_, _, event: &EditorEvent, cx| match event {
                    EditorEvent::Edited { .. } => cx.emit(ProjectSearchEvent::Edited),
                    EditorEvent::DirtyChanged => cx.emit(ProjectSearchEvent::DirtyChanged),
                    EditorEvent::OpenExcerptsRequested { locations, .. } => {
                        cx.emit(ProjectSearchEvent::OpenExcerptsRequested(locations.clone()));
                    }
                    EditorEvent::PathChanged => {}
                    EditorEvent::DiffHunksExpandedChanged => {}
                    EditorEvent::Error(_) => {}
                },
            ),
        ];
        Self {
            project,
            results_editor,
            excerpts,
            search_generation: 0,
            debounce_task: None,
            pending_search: None,
            search_bar: cx.new(|cx| {
                SearchBar::new(
                    SearchBarConfig {
                        id_prefix: "project-search",
                        key_context: "ProjectSearchBar",
                        supports_replace: false,
                        query_placeholder: "搜索...",
                        replace_placeholder: "替换为...",
                        dismissible: true,
                    },
                    cx,
                )
            }),
            _subscriptions: subscriptions,
        }
    }

    fn set_all_files_folded(&mut self, folded: bool, cx: &mut Context<Self>) {
        let mut paths = Vec::new();
        for excerpt in self.excerpts.read(cx).snapshot(cx).excerpts() {
            if !paths.iter().any(|path| path == excerpt.path()) {
                paths.push(excerpt.path().to_path_buf());
            }
        }
        self.results_editor.update(cx, |editor, cx| {
            for path in paths {
                if editor.is_buffer_folded(&path, cx) != folded {
                    editor.toggle_buffer_fold(path, cx);
                }
            }
        });
    }

    fn run_search(&mut self, query: SearchQuery, window: &mut Window, cx: &mut Context<Self>) {
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;
        if query.query.is_empty() {
            self.reset_results(window, cx);
            return;
        }

        let results = self
            .project
            .update(cx, |project, cx| project.search(query.clone(), cx));
        let search_task = results.task;
        let results_rx = results.rx;
        self.excerpts.update(cx, |buffer, cx| buffer.clear(cx));
        self.results_editor.update(cx, |editor, cx| {
            SearchableItem::clear_search(editor, window, cx)
        });
        cx.emit(SearchEvent::MatchesInvalidated);
        cx.notify();

        let project = self.project.clone();
        let results_editor = self.results_editor.clone();
        self.pending_search = Some(cx.spawn_in(window, async move |this, cx| {
            let _search_task = search_task;
            let mut batched = Vec::<ExcerptRange>::new();
            loop {
                // 被更新的查询取代时放弃本次流式装配；
                // 放弃通道会让后台在下次发送时感知并提前结束扫描。
                if this
                    .update_in(cx, |this, _, _| this.search_generation != generation)
                    .unwrap_or(true)
                {
                    break;
                }
                let item = match results_rx.recv().await {
                    Ok(item) => item,
                    // 通道关闭：后台扫描结束，装配剩余批次。
                    Err(_) => break,
                };
                // Project 已释放或文档注册失败时跳过该文件。
                let Ok(source) = project.update(cx, |project, cx| {
                    if let Some(buffer) = item.loaded_buffer {
                        project.register_loaded_buffer(item.path, buffer, cx)
                    } else {
                        project.open_buffer(&item.path, cx)
                    }
                }) else {
                    continue;
                };
                for excerpt in item.excerpts {
                    batched.push(ExcerptRange::new(
                        source.clone(),
                        excerpt.range,
                        excerpt.matches,
                    ));
                }
                if batched.len() < SEARCH_BATCH_SIZE {
                    continue;
                }
                let batch = std::mem::take(&mut batched);
                this.update_in(cx, |this, _window, cx| {
                    this.append_search_batch(batch, &results_editor, query.clone(), cx);
                })
                .ok();
                // 让出主循环：每批装配后重绘，结果渐进可见。
                cx.background_executor().timer(SEARCH_BATCH_YIELD).await;
            }
            this.update_in(cx, |this, _window, cx| {
                if !batched.is_empty() {
                    this.append_search_batch(batched, &results_editor, query, cx);
                }
                this.pending_search = None;
                cx.emit(SearchEvent::MatchesInvalidated);
                cx.emit(ProjectSearchEvent::Updated);
                cx.notify();
            })
            .ok();
        }));
    }

    /// 将新增片段追加到组合文档，并更新匹配高亮。
    fn append_search_batch(
        &mut self,
        excerpts: Vec<ExcerptRange>,
        results_editor: &Entity<Editor>,
        query: SearchQuery,
        cx: &mut Context<Self>,
    ) {
        let match_ranges = self
            .excerpts
            .update(cx, |buffer, cx| buffer.append_excerpts(excerpts, cx));
        results_editor.update(cx, |editor, cx| {
            editor.append_search_ranges(
                query,
                match_ranges.into_iter().map(Into::into).collect(),
                cx,
            )
        });
        cx.emit(SearchEvent::MatchesInvalidated);
        cx.notify();
    }

    fn reset_results(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.pending_search = None;
        self.excerpts.update(cx, |buffer, cx| buffer.clear(cx));
        self.results_editor.update(cx, |editor, cx| {
            SearchableItem::clear_search(editor, window, cx)
        });
        cx.emit(SearchEvent::MatchesInvalidated);
        cx.emit(ProjectSearchEvent::Updated);
        cx.notify();
    }
}

impl EventEmitter<ProjectSearchEvent> for ProjectSearchView {}
impl EventEmitter<SearchEvent> for ProjectSearchView {}

impl Focusable for ProjectSearchView {
    fn focus_handle(&self, cx: &App) -> FocusHandle {
        self.results_editor.read(cx).focus_handle()
    }
}

impl Render for ProjectSearchView {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let colors = color::current(cx);
        let (match_count, _) = SearchableItem::search_count(self, cx);
        let has_results = match_count > 0;
        // 已有查询但无匹配时显示空态；尚未输入查询时保持空白。
        let show_empty = match_count == 0 && !self.search_bar.read(cx).query_text(cx).is_empty();

        div()
            .key_context("ProjectSearchView")
            .size_full()
            .flex()
            .flex_col()
            .bg(colors.editor_background)
            .child(
                div()
                    .flex_1()
                    .overflow_hidden()
                    .when(has_results, |element| {
                        element.child(self.results_editor.clone())
                    })
                    .when(show_empty, |element| {
                        element
                            .flex()
                            .items_center()
                            .justify_center()
                            .text_color(colors.text_muted)
                            .child("没有搜索结果")
                    }),
            )
    }
}

impl Item for ProjectSearchView {
    type Event = ProjectSearchEvent;

    fn tab_content_text(&self, _cx: &App) -> SharedString {
        "项目搜索".into()
    }

    fn tab_icon(&self, _cx: &App) -> Option<SharedString> {
        Some("icons/magnifying_glass.svg".into())
    }

    fn serialized_pane_item(&self, cx: &App) -> Option<SerializedPaneItem> {
        // 查询与选项的唯一权威在共享搜索栏；序列化时读取，不另存副本。
        let state = ProjectSearchState {
            query: self.search_bar.read(cx).query_text(cx),
            options: self.search_bar.read(cx).options(),
        };
        Some(SerializedPaneItem::Custom {
            kind: PROJECT_SEARCH_SERIALIZED_KIND.into(),
            state: serde_json::to_value(state).ok()?,
        })
    }

    fn to_item_events(event: &Self::Event, emit: &mut dyn FnMut(ItemEvent)) {
        match event {
            ProjectSearchEvent::Updated => emit(ItemEvent::UpdateTab),
            ProjectSearchEvent::Edited => emit(ItemEvent::Edit),
            ProjectSearchEvent::DirtyChanged => emit(ItemEvent::UpdateTab),
            ProjectSearchEvent::OpenExcerptsRequested(_) => {}
        }
    }

    fn active_path(&self, cx: &App) -> Option<PathBuf> {
        self.results_editor
            .read(cx)
            .excerpt_location(cx)
            .map(|location| location.path)
    }

    fn multi_buffer(&self, _cx: &App) -> Option<Entity<MultiBuffer>> {
        Some(self.excerpts.clone())
    }

    /// 搜索结果同样是编辑器：
    /// 把内层 `results_editor` 暴露给编辑器级消费者（状态栏、代码大纲等）。
    fn act_as_type(
        &self,
        type_id: TypeId,
        self_handle: &Entity<Self>,
        _cx: &App,
    ) -> Option<AnyEntity> {
        if type_id == TypeId::of::<Self>() {
            Some(self_handle.clone().into())
        } else if type_id == TypeId::of::<Editor>() {
            Some(self.results_editor.clone().into())
        } else {
            None
        }
    }

    /// 搜索栏由 `ProjectSearchToolbar` 承担，不使用编辑器通用文档工具栏。
    fn uses_editor_document_toolbar(&self, _cx: &App) -> bool {
        false
    }

    /// 搜索结果由多源派生，通用按文件 git 投影不适用。
    fn receives_git_projection(&self) -> bool {
        false
    }

    fn can_save(&self, cx: &App) -> bool {
        <Editor as Item>::can_save(self.results_editor.read(cx), cx)
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.results_editor.read(cx).is_dirty(cx)
    }

    fn save(
        &mut self,
        project: Entity<Project>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Task<anyhow::Result<()>> {
        self.results_editor.update(cx, |editor, cx| {
            <Editor as Item>::save(editor, project, window, cx)
        })
    }

    /// 缓冲区级搜索目标是结果编辑器本身；
    /// 项目搜索栏的目标是视图自身（它的 `SearchableItem` 驱动项目级搜索），由 `ProjectSearchToolbar` 注入，不经过这里。
    fn as_searchable(
        &self,
        _self_handle: &Entity<Self>,
        _cx: &App,
    ) -> Option<Box<dyn SearchableItemHandle>> {
        Some(Box::new(self.results_editor.clone()))
    }
}

impl SearchableItem for ProjectSearchView {
    fn supports_replace(&self) -> bool {
        false
    }

    fn search(&mut self, query: &SearchQuery, window: &mut Window, cx: &mut Context<Self>) {
        // 防抖合并击键；等窗内出现更新的查询（或搜索被清空）时放弃本次搜索。
        self.search_generation = self.search_generation.wrapping_add(1);
        let generation = self.search_generation;
        let query = query.clone();
        self.debounce_task = Some(cx.spawn_in(window, async move |this, cx| {
            cx.background_executor().timer(SEARCH_DEBOUNCE).await;
            this.update_in(cx, |this, window, cx| {
                if this.search_generation == generation {
                    this.run_search(query, window, cx);
                }
            })
            .ok();
        }));
    }

    fn clear_search(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        self.search_generation = self.search_generation.wrapping_add(1);
        self.reset_results(window, cx);
    }

    fn search_count(&self, cx: &App) -> (usize, Option<usize>) {
        SearchableItem::search_count(self.results_editor.read(cx), cx)
    }

    fn activate_match_in_direction(
        &mut self,
        direction: Direction,
        count: usize,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.results_editor.update(cx, |editor, cx| {
            SearchableItem::activate_match_in_direction(editor, direction, count, window, cx)
        });
        cx.emit(SearchEvent::ActiveMatchChanged);
        cx.notify();
    }

    fn replace_current(
        &mut self,
        _replacement: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> bool {
        false
    }

    fn replace_all(
        &mut self,
        _replacement: &str,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> usize {
        0
    }
}

/// 把项目搜索的「打开片段」请求接到工作区打开文件。
///
/// 新建（deploy）与布局恢复（restore）两条创建路径都必须接线：
/// 订阅属于 Workspace，视图自身只发事件，恢复出的标签漏接就再没有人处理打开请求。
fn subscribe_to_open_excerpts(
    view: &Entity<ProjectSearchView>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    cx.subscribe_in(view, window, |workspace, _, event, window, cx| {
        let ProjectSearchEvent::OpenExcerptsRequested(locations) = event else {
            return;
        };
        for location in locations {
            workspace.open_path_at(
                location.path.clone(),
                location.source_range.start().get()..location.source_range.end().get(),
                window,
                cx,
            );
        }
    })
    .detach();
}

pub(crate) fn deploy(
    workspace: &mut Workspace,
    seed: Option<String>,
    window: &mut Window,
    cx: &mut Context<Workspace>,
) {
    let pane = workspace.pane().clone();
    if let Some(existing) = pane
        .read(cx)
        .tabs()
        .iter()
        .find_map(|item| item.act_as::<ProjectSearchView>(cx))
    {
        let item_id = existing.entity_id();
        pane.update(cx, |pane, cx| pane.activate_tab(item_id, window, cx));
        // 已有标签重新打开搜索栏并聚焦查询框；activate_tab 自身不改变焦点。
        existing.update(cx, |view, cx| {
            let search_bar = view.search_bar.clone();
            search_bar.update(cx, |bar, cx| bar.deploy(None, window, cx));
        });
        return;
    }

    let project = workspace.project().clone();
    let view = cx.new(|cx| ProjectSearchView::new(project, cx));
    view.update(cx, |view, cx| {
        let search_bar = view.search_bar.clone();
        search_bar.update(cx, |bar, cx| bar.deploy(seed, window, cx));
    });
    subscribe_to_open_excerpts(&view, window, cx);
    let view_handle = view.clone();
    workspace.open_item(Box::new(view), window, cx);
    // open_item 会把焦点交给 Item 主内容（结果编辑器）；项目搜索打开后应直接可输入。
    view_handle.update(cx, |view, cx| {
        let search_bar = view.search_bar.clone();
        search_bar.update(cx, |bar, cx| bar.focus_query(window, cx));
    });
}

/// 状态栏中的项目搜索入口。
pub(crate) struct ProjectSearchButton {
    workspace: WeakEntity<Workspace>,
}

impl ProjectSearchButton {
    pub(crate) fn new(workspace: WeakEntity<Workspace>) -> Self {
        Self { workspace }
    }
}

impl StatusItemView for ProjectSearchButton {
    fn set_active_pane_item(&mut self, _item: Option<&dyn ItemHandle>, _cx: &mut Context<Self>) {}
}

impl Render for ProjectSearchButton {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl gpui::IntoElement {
        let workspace = self.workspace.clone();
        Button::icon("search-button", "icons/magnifying_glass.svg")
            .label("项目搜索")
            .shortcut(zcv_keymap::display_shortcut(&DeployProjectSearch, cx))
            .on_click(move |_, window, cx| {
                workspace
                    .update(cx, |workspace, cx| {
                        crate::deploy_project_search(workspace, window, cx);
                    })
                    .ok();
            })
            .into_any_element()
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use gpui::{AppContext as _, Context, TestAppContext, VisualTestContext, Window};
    use zcv_fs_watch::{FsEventStream, FsWatcher, Watcher};
    use zcv_language::LanguageRegistry;
    use zcv_path::AbsolutePathBuf;
    use zcv_project::Project;
    use zcv_text::{ByteOffset, TextRange};

    use super::*;

    struct PassiveWatcher {
        watcher: FsWatcher,
    }

    impl PassiveWatcher {
        fn new() -> Self {
            Self {
                watcher: FsWatcher::new(),
            }
        }
    }

    impl Watcher for PassiveWatcher {
        fn add(&self, _path: &Path) -> anyhow::Result<()> {
            Ok(())
        }

        fn remove(&self, _path: &Path) -> anyhow::Result<()> {
            Ok(())
        }

        fn events(&self) -> FsEventStream {
            self.watcher.events()
        }
    }

    fn test_workspace(
        root: std::path::PathBuf,
        window: &mut Window,
        cx: &mut Context<Workspace>,
    ) -> Workspace {
        let project = cx.new(|cx| {
            Project::new_with_watcher(
                root,
                Arc::new(PassiveWatcher::new()),
                Arc::new(LanguageRegistry::new()),
                cx,
            )
        });
        Workspace::new_with_project(project, window, cx)
    }

    /// 回归：布局恢复出的项目搜索标签必须与 deploy 新建的一样接上工作区的打开订阅。
    ///
    /// 漏接时点击「打开文件」与 alt-enter 都只发出事件而无人处理，表现为搜索结果无法打开文件。
    #[gpui::test]
    async fn restored_project_search_tab_opens_excerpt_files(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let root = AbsolutePathBuf::canonicalize(directory.path())
            .expect("项目根应可规范化")
            .into_path_buf();
        let file = root.join("needle.txt");
        std::fs::write(&file, "needle").expect("应创建测试文件");
        // 打开文件经 ItemProvider 注册表分发，测试同样需要文本 Provider。
        cx.update(zcv_editor::init);

        let provider = ProjectSearchSerializedItemProvider;
        let (workspace, cx) = cx.add_window_view({
            let root = root.clone();
            move |window, cx| test_workspace(root, window, cx)
        });

        // 按布局恢复路径重建项目搜索标签，再像 restore_pane 一样放进 Pane。
        let state = serde_json::to_value(ProjectSearchState {
            query: "needle".into(),
            options: MatchOptions::default(),
        })
        .expect("搜索栏状态应可序列化");
        let restored = workspace.update_in(cx, |workspace, window, cx| {
            provider.restore(state, workspace.project().clone(), window, cx)
        });
        let item = restored.await.expect("项目搜索标签应可恢复");
        workspace.update_in(cx, |workspace, window, cx| {
            workspace.open_item(item, window, cx)
        });

        let view = cx.read_entity(&workspace, |workspace, cx| {
            workspace
                .pane()
                .read(cx)
                .tabs()
                .iter()
                .find_map(|item| item.act_as::<ProjectSearchView>(cx))
                .expect("恢复出的标签应是项目搜索视图")
        });

        cx.read_entity(&view, |view, cx| {
            assert!(
                view.search_bar.read(cx).visible(),
                "恢复的标签应重新打开搜索栏"
            );
            assert_eq!(
                view.search_bar.read(cx).query_text(cx),
                "needle",
                "查询状态应由共享搜索栏恢复"
            );
        });

        // 命中片段请求打开源文件：与点击「打开文件」和 alt-enter 发出的事件同一条路径。
        view.update(cx, |_, cx| {
            cx.emit(ProjectSearchEvent::OpenExcerptsRequested(vec![
                ExcerptLocation {
                    path: file.clone(),
                    source_range: TextRange::new(ByteOffset::ZERO, ByteOffset::ZERO)
                        .expect("同点源范围必须有效"),
                },
            ]));
        });
        cx.run_until_parked();

        cx.read_entity(&workspace, |workspace, cx| {
            let opened: Vec<_> = workspace
                .pane()
                .read(cx)
                .tabs()
                .iter()
                .filter_map(|item| item.item_path(cx))
                .collect();
            assert!(
                opened.contains(&file),
                "恢复的项目搜索标签应能打开命中文件，实际标签：{opened:?}"
            );
        });
    }

    /// 回归：项目搜索视图与其搜索栏之间不得互相强引用。
    ///
    /// 视图持有 SearchBar，SearchBar 又曾强持有视图作为搜索目标，构成环；
    /// 关闭标签/面板（不触发活动 Item 变化、因而不会清 target）时两者都无法释放。
    /// 目标改为弱句柄后，释放外部强引用即可让视图与搜索栏一起释放。
    #[gpui::test]
    fn project_search_view_and_search_bar_release_together(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let project = cx.new(|cx| {
            Project::new_with_watcher(
                directory.path().to_path_buf(),
                Arc::new(PassiveWatcher::new()),
                Arc::new(LanguageRegistry::new()),
                cx,
            )
        });
        let view = cx.new(|cx| ProjectSearchView::new(project, cx));
        let search_bar = cx.read_entity(&view, |view, _| view.search_bar.clone());
        let weak_view = view.downgrade();
        let weak_search_bar = search_bar.downgrade();

        // 模拟工具项激活：搜索栏把视图登记为自搜索目标。
        let (_, visual) = cx.add_window_view(|window, cx| {
            let toolbar = cx.new(|_| ProjectSearchToolbar::new());
            toolbar.update(cx, |toolbar, cx| {
                toolbar.set_active_pane_item(Some(&view as &dyn ItemHandle), window, cx);
            });
            gpui::Empty
        });
        visual.run_until_parked();

        drop(search_bar);
        drop(view);
        // 实体释放分多轮 effect 完成：逐轮刷新直到视图与搜索栏都被回收。
        for _ in 0..4 {
            visual.update(|_, _| {});
            visual.run_until_parked();
        }

        assert!(
            weak_view.upgrade().is_none(),
            "视图在外部强引用释放后应被回收（不再被搜索栏强持有）"
        );
        assert!(
            weak_search_bar.upgrade().is_none(),
            "搜索栏应随视图一起释放"
        );
    }

    /// 打开项目搜索后应直接聚焦查询输入框；Item 主焦点默认落在结果编辑器上。
    #[gpui::test]
    async fn deploying_project_search_focuses_query_input(cx: &mut TestAppContext) {
        let directory = tempfile::tempdir().expect("应创建临时项目目录");
        let root = directory.path().canonicalize().expect("项目根应可规范化");
        cx.update(zcv_editor::init);

        let (workspace, cx) = cx.add_window_view({
            let root = root.clone();
            move |window, cx| test_workspace(root, window, cx)
        });

        // 首次打开：新建项目搜索标签。
        workspace.update_in(cx, |workspace, window, cx| {
            deploy(workspace, None, window, cx);
        });
        cx.run_until_parked();
        assert_query_input_focused(&workspace, cx, "新建标签后");

        // 已有标签：先把焦点移回结果编辑器，再重新打开项目搜索。
        let results_focus = cx.read_entity(&workspace, |workspace, cx| {
            project_search_view(workspace, cx)
                .read(cx)
                .results_editor
                .read(cx)
                .focus_handle()
        });
        cx.update(|window, cx| window.focus(&results_focus, cx));
        workspace.update_in(cx, |workspace, window, cx| {
            deploy(workspace, None, window, cx);
        });
        cx.run_until_parked();
        assert_query_input_focused(&workspace, cx, "已有标签重新打开后");
    }

    fn project_search_view(
        workspace: &Workspace,
        cx: &gpui::App,
    ) -> gpui::Entity<ProjectSearchView> {
        workspace
            .pane()
            .read(cx)
            .tabs()
            .iter()
            .find_map(|item| item.act_as::<ProjectSearchView>(cx))
            .expect("应存在项目搜索标签")
    }

    fn assert_query_input_focused(
        workspace: &gpui::Entity<Workspace>,
        cx: &mut VisualTestContext,
        context: &str,
    ) {
        let input_focus = cx.read_entity(workspace, |workspace, cx| {
            project_search_view(workspace, cx)
                .read(cx)
                .search_bar
                .read(cx)
                .query_focus_handle(cx)
        });
        cx.update(|window, _| {
            assert!(input_focus.is_focused(window), "{context}应聚焦查询输入框");
        });
    }
}
