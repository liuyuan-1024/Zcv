use std::{
    cell::Cell,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use gpui::{AppContext, Context, TestAppContext, div, px, size};
use zcv_fs_watch::{FsEventStream, Watcher};
use zcv_language::LanguageRegistry;

use super::*;

struct QuietWatcher;

impl Watcher for QuietWatcher {
    fn add(&self, _path: &Path) -> anyhow::Result<()> {
        Ok(())
    }

    fn remove(&self, _path: &Path) -> anyhow::Result<()> {
        Ok(())
    }

    fn watch(&self, _latency: Duration) -> FsEventStream {
        Box::pin(futures::stream::pending())
    }
}

fn test_project(root: PathBuf, languages: Arc<LanguageRegistry>, cx: &mut App) -> Entity<Project> {
    cx.new(|cx| {
        Project::new_with_watcher(root, Arc::new(QuietWatcher), languages, cx)
            .expect("测试项目根目录应可规范化")
    })
}

#[derive(Default)]
struct TestView;

impl Render for TestView {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[gpui::test]
fn confirm_invokes_on_selected(cx: &mut TestAppContext) {
    let triggered = Rc::new(Cell::new(None::<String>));
    let on_selected: OnProjectSelected = {
        let triggered = triggered.clone();
        Rc::new(move |path, _window, _cx| triggered.set(Some(path)))
    };
    let mut delegate = cx.update(|cx| {
        RecentProjectsDelegate::new(
            cx.new(|cx| Project::empty(Arc::new(LanguageRegistry::new()), cx)),
            vec![ProjectEntry {
                path: "/tmp/test-project".into(),
            }],
            on_selected,
            Rc::new(|_, _| {}),
            cx,
        )
    });
    let window = cx.add_window(|_window, _cx| TestView);
    let _ = window.update(cx, |_, window, cx| {
        delegate.confirm(window, cx);
    });
    assert_eq!(triggered.take().as_deref(), Some("/tmp/test-project"));
}

/// 构造 3 个项目的数据源，默认选中第一项。
fn test_delegate(cx: &mut App) -> RecentProjectsDelegate {
    let on_selected: OnProjectSelected = Rc::new(|_, _, _| {});
    RecentProjectsDelegate::new(
        cx.new(|cx| Project::empty(Arc::new(LanguageRegistry::new()), cx)),
        vec![
            ProjectEntry {
                path: "/tmp/a".into(),
            },
            ProjectEntry {
                path: "/tmp/b".into(),
            },
            ProjectEntry {
                path: "/tmp/c".into(),
            },
        ],
        on_selected,
        Rc::new(|_, _| {}),
        cx,
    )
}

#[gpui::test]
fn remove_project_drops_entry_and_keeps_filter(cx: &mut TestAppContext) {
    let mut delegate = cx.update(test_delegate);
    delegate.update_matches("tmp".into());
    delegate.remove_project_in_memory(2);
    assert_eq!(delegate.projects.len(), 2);
    assert!(delegate.projects.iter().all(|p| p.path != "/tmp/c"));
    assert_eq!(delegate.filtered, vec![0, 1]);
}

#[gpui::test]
fn project_query_matches_noncontiguous_name_characters(cx: &mut TestAppContext) {
    let mut delegate = cx.update(|cx| {
        RecentProjectsDelegate::new(
            cx.new(|cx| Project::empty(Arc::new(LanguageRegistry::new()), cx)),
            vec![
                ProjectEntry {
                    path: "/tmp/alpha-project".into(),
                },
                ProjectEntry {
                    path: "/tmp/beta".into(),
                },
            ],
            Rc::new(|_, _, _| {}),
            Rc::new(|_, _| {}),
            cx,
        )
    });
    delegate.update_matches("apj".into());
    assert_eq!(delegate.filtered, vec![0]);
}

#[gpui::test]
fn remove_selected_project_selects_the_next_entry(cx: &mut TestAppContext) {
    let mut delegate = cx.update(test_delegate);
    delegate.selected_index = 1;
    delegate.remove_project_in_memory(1);
    assert_eq!(delegate.projects.len(), 2);
    assert_eq!(delegate.selected_index, 1);
    assert_eq!(delegate.projects[delegate.selected_index].label(), "c");
}

#[gpui::test]
fn remove_last_project_clamps_selection(cx: &mut TestAppContext) {
    let mut delegate = cx.update(test_delegate);
    delegate.selected_index = 1;
    delegate.remove_project_in_memory(2);
    assert_eq!(delegate.selected_index, 1);
    assert_eq!(delegate.projects[delegate.selected_index].label(), "b");
}

#[gpui::test]
fn current_project_cannot_be_removed_from_recent_projects(cx: &mut TestAppContext) {
    let current_dir = tempfile::tempdir().expect("应创建当前项目目录");
    let other_dir = tempfile::tempdir().expect("应创建其他项目目录");
    let languages = Arc::new(LanguageRegistry::new());
    let reported = Rc::new(Cell::new(None::<String>));
    let (mut delegate, current) = cx.update(|cx| {
        let project = test_project(current_dir.path().to_path_buf(), languages, cx);
        let current = project
            .read(cx)
            .root()
            .expect("当前项目有根目录")
            .to_string_lossy()
            .into_owned();
        let delegate = RecentProjectsDelegate::new(
            project,
            vec![
                ProjectEntry {
                    path: other_dir.path().to_string_lossy().into_owned(),
                },
                ProjectEntry {
                    path: current.clone(),
                },
            ],
            Rc::new(|_, _, _| {}),
            {
                let reported = reported.clone();
                Rc::new(move |message, _| reported.set(Some(message)))
            },
            cx,
        );
        (delegate, current)
    });

    assert!(
        !cx.update(|cx| delegate.remove_project(1, cx))
            .expect("移除当前项目应被忽略")
    );
    assert_eq!(delegate.projects.len(), 2);
    assert_eq!(delegate.selected_index, 1);
    assert_eq!(delegate.projects[1].path, current);
    assert_eq!(reported.take().as_deref(), Some("不允许删除当前项目"));

    assert!(
        cx.update(|cx| delegate.remove_project(0, cx))
            .expect("其他项目应可移除")
    );
    assert_eq!(delegate.projects.len(), 1);
    assert_eq!(delegate.projects[0].path, current);
    assert!(
        reported.take().is_none(),
        "移除其他项目不应提示当前项目错误"
    );
}

#[gpui::test]
fn current_project_is_selected_by_path_and_check_stays_after_its_name(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建当前项目目录");
    let languages = Arc::new(LanguageRegistry::new());
    cx.update(|cx| zcv_editor::init(cx, languages.clone()));
    let (picker, cx) = cx.add_window_view(|window, cx| {
        let project = test_project(directory.path().to_path_buf(), languages, cx);
        let root = project
            .read(cx)
            .root()
            .expect("当前项目有根目录")
            .to_owned();
        let current = root.to_string_lossy().into_owned();
        let same_name = format!(
            "/another-parent/{}",
            root.file_name().unwrap().to_string_lossy()
        );
        let delegate = RecentProjectsDelegate::new(
            project,
            vec![
                ProjectEntry { path: same_name },
                ProjectEntry {
                    path: "/tmp/other".into(),
                },
                ProjectEntry { path: current },
            ],
            Rc::new(|_, _, _| {}),
            Rc::new(|_, _| {}),
            cx,
        );
        Picker::new(delegate, PICKER_WIDTH, window, cx)
    });
    cx.simulate_window_resize(cx.windows()[0], size(px(700.0), px(600.0)));
    assert_eq!(
        cx.read_entity(&picker, |picker, _| picker.delegate().selected_index()),
        2
    );
    let current_name = cx
        .debug_bounds("project-name-2")
        .expect("当前项目名称应可见");
    let check = cx
        .debug_bounds("current-project-check")
        .expect("当前项目应显示对勾");
    assert!(check.left() >= current_name.right(), "对勾应紧跟项目名");
    assert_eq!(
        check.center().y,
        current_name.center().y,
        "对勾应与名称纵向对齐"
    );

    picker.update(cx, |picker, cx| {
        picker.delegate_mut().set_selected_index(0);
        cx.notify();
    });
    let check = cx
        .debug_bounds("current-project-check")
        .expect("改变高亮后仍显示当前项目对勾");
    let current_name = cx.debug_bounds("project-name-2").expect("当前项目仍可见");
    assert!(check.left() >= current_name.right());
    assert_eq!(
        check.center().y,
        current_name.center().y,
        "对勾不能跟随高亮移到同名项目"
    );

    picker.update(cx, |picker, cx| picker.set_query("/tmp/other", cx));
    assert_eq!(
        cx.read_entity(&picker, |picker, _| picker.delegate().match_count()),
        1
    );
    assert!(
        cx.debug_bounds("current-project-check").is_none(),
        "过滤掉当前项目后，不应给其他项目显示对勾"
    );
}

#[gpui::test]
fn reopening_selects_current_project_and_clears_previous_search(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建当前项目目录");
    let languages = Arc::new(LanguageRegistry::new());
    cx.update(|cx| zcv_editor::init(cx, languages.clone()));
    let (selector, cx) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| Workspace::new_empty(languages.clone(), window, cx));
        let project = test_project(directory.path().to_path_buf(), languages, cx);
        RecentProjects::new(
            Rc::new(|_, _, _| {}),
            project,
            workspace.downgrade(),
            PopoverLayer::default(),
            window,
            cx,
        )
    });
    selector.update(cx, |selector, cx| {
        selector
            .picker
            .update(cx, |picker, cx| picker.set_query("不会匹配的搜索", cx))
    });
    cx.update(|window, cx| selector.update(cx, |selector, cx| selector.toggle(window, cx)));
    let picker = cx.read_entity(&selector, |selector, _| selector.picker.clone());
    cx.read_entity(&picker, |picker, cx| {
        let delegate = picker.delegate();
        let selected = &delegate.projects[delegate.filtered[delegate.selected_index()]];
        assert_eq!(
            Some(Path::new(&selected.path)),
            delegate.project.read(cx).root()
        );
        assert_eq!(picker.search_input().text(cx), "");
    });
    assert!(
        cx.debug_bounds("current-project-check").is_some(),
        "打开后当前项目应有可见对勾"
    );
    cx.update(|window, cx| selector.update(cx, |selector, cx| selector.toggle(window, cx)));
    selector.update(cx, |selector, cx| {
        selector
            .picker
            .update(cx, |picker, cx| picker.set_query("另一次搜索", cx))
    });
    cx.update(|window, cx| selector.update(cx, |selector, cx| selector.toggle(window, cx)));
    cx.read_entity(&picker, |picker, cx| {
        let delegate = picker.delegate();
        let selected = &delegate.projects[delegate.filtered[delegate.selected_index()]];
        assert_eq!(
            Some(Path::new(&selected.path)),
            delegate.project.read(cx).root()
        );
        assert_eq!(picker.search_input().text(cx), "");
    });
}

#[gpui::test]
fn empty_workspace_does_not_mark_a_recent_project_as_current(cx: &mut TestAppContext) {
    let languages = Arc::new(LanguageRegistry::new());
    cx.update(|cx| zcv_editor::init(cx, languages));
    let (picker, cx) =
        cx.add_window_view(|window, cx| Picker::new(test_delegate(cx), PICKER_WIDTH, window, cx));
    assert_eq!(
        cx.read_entity(&picker, |picker, _| picker.delegate().selected_index()),
        0
    );
    assert!(cx.debug_bounds("current-project-check").is_none());
}
