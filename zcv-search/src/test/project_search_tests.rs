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

fn project_search_view(workspace: &Workspace, cx: &gpui::App) -> gpui::Entity<ProjectSearchView> {
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
