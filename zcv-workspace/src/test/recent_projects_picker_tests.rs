use std::{
    cell::Cell,
    fs,
    path::{Path, PathBuf},
    process::Command,
    sync::Arc,
    time::Duration,
};

use gpui::{AppContext, Context, Modifiers, TestAppContext, div, px, size};
use zcv_fs_watch::{FsEventStream, FsWatcher, Watcher};
use zcv_language::LanguageRegistry;
use zcv_settings::{SettingsStore, UserSettings};

use super::*;
use crate::test_support::empty_workspace;

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
    test_project_roots(vec![root], languages, cx)
}

fn test_project_roots(
    roots: Vec<PathBuf>,
    languages: Arc<LanguageRegistry>,
    cx: &mut App,
) -> Entity<Project> {
    cx.new(|cx| {
        Project::new_with_watcher(roots, Arc::new(QuietWatcher), languages, cx)
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
    let triggered = Rc::new(Cell::new(None::<Vec<PathBuf>>));
    let on_selected: OnProjectSelected = {
        let triggered = triggered.clone();
        Rc::new(move |paths, _window, _cx| triggered.set(Some(paths)))
    };
    let mut delegate = cx.update(|cx| {
        RecentProjectsDelegate::new(
            cx.new(|cx| {
                Project::empty_with_watcher(
                    Arc::new(LanguageRegistry::new()),
                    Arc::new(FsWatcher::new()),
                    cx,
                )
            }),
            vec![ProjectEntry {
                paths: vec!["/tmp/test-project".into()],
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
    assert_eq!(
        triggered.take(),
        Some(vec![PathBuf::from("/tmp/test-project")])
    );
}

#[gpui::test]
fn recent_projects_footer_orders_entries_and_dispatches(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let languages = Arc::new(LanguageRegistry::new());
    cx.update(|cx| zcv_editor::init(cx, languages));
    let file_triggered = Rc::new(Cell::new(false));
    let directory_triggered = Rc::new(Cell::new(false));
    let add_triggered = Rc::new(Cell::new(false));
    cx.update(|cx| {
        let file_triggered = file_triggered.clone();
        let directory_triggered = directory_triggered.clone();
        let add_triggered = add_triggered.clone();
        cx.on_action(move |_: &OpenLocalFile, _| file_triggered.set(true));
        cx.on_action(move |_: &OpenLocalDirectory, _| directory_triggered.set(true));
        cx.on_action(move |_: &AddDirectoryToProject, _| add_triggered.set(true));
    });
    let (_picker, cx) =
        cx.add_window_view(|window, cx| Picker::new(test_delegate(cx), PICKER_WIDTH, window, cx));
    let file = cx
        .debug_bounds("open-local-file-footer")
        .expect("最近项目列表应显示打开本地文件");
    let directory = cx
        .debug_bounds("open-local-directory-footer")
        .expect("最近项目列表应显示打开本地目录");
    let add = cx
        .debug_bounds("add-directory-to-project-footer")
        .expect("最近项目列表应显示添加目录到项目");
    assert!(directory.top() >= file.bottom(), "文件入口应排在目录入口前");
    assert!(
        add.top() >= directory.bottom(),
        "目录入口应排在添加目录入口前"
    );
    cx.simulate_click(file.center(), Modifiers::default());
    assert!(file_triggered.get());
    cx.simulate_click(directory.center(), Modifiers::default());
    assert!(directory_triggered.get());
    cx.simulate_click(add.center(), Modifiers::default());
    assert!(add_triggered.get());
}

#[gpui::test]
fn multi_root_entry_renders_each_path_on_its_own_line(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let languages = Arc::new(LanguageRegistry::new());
    cx.update(|cx| zcv_editor::init(cx, languages));
    let on_selected: OnProjectSelected = Rc::new(|_, _, _| {});
    let (_picker, cx) = cx.add_window_view(|window, cx| {
        Picker::new(
            RecentProjectsDelegate::new(
                cx.new(|cx| {
                    Project::empty_with_watcher(
                        Arc::new(LanguageRegistry::new()),
                        Arc::new(FsWatcher::new()),
                        cx,
                    )
                }),
                vec![ProjectEntry {
                    paths: vec!["/tmp/workspace-a".into(), "/tmp/workspace-b".into()],
                }],
                on_selected,
                Rc::new(|_, _| {}),
                cx,
            ),
            PICKER_WIDTH,
            window,
            cx,
        )
    });
    let first = cx
        .debug_bounds("recent-project-path-0-0")
        .expect("多根条目应逐行显示第一个路径");
    let second = cx
        .debug_bounds("recent-project-path-0-1")
        .expect("多根条目应逐行显示第二个路径");
    assert!(second.top() >= first.bottom(), "两个路径应各占一行");
}

/// 构造 3 个项目的数据源，默认选中第一项。
fn test_delegate(cx: &mut App) -> RecentProjectsDelegate {
    let on_selected: OnProjectSelected = Rc::new(|_, _, _| {});
    RecentProjectsDelegate::new(
        cx.new(|cx| {
            Project::empty_with_watcher(
                Arc::new(LanguageRegistry::new()),
                Arc::new(FsWatcher::new()),
                cx,
            )
        }),
        vec![
            ProjectEntry {
                paths: vec!["/tmp/a".into()],
            },
            ProjectEntry {
                paths: vec!["/tmp/b".into()],
            },
            ProjectEntry {
                paths: vec!["/tmp/c".into()],
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
    assert!(
        delegate
            .projects
            .iter()
            .all(|p| !p.paths.iter().any(|path| path == "/tmp/c"))
    );
    assert_eq!(delegate.filtered, vec![0, 1]);
}

#[gpui::test]
fn project_query_matches_noncontiguous_name_characters(cx: &mut TestAppContext) {
    let mut delegate = cx.update(|cx| {
        RecentProjectsDelegate::new(
            cx.new(|cx| {
                Project::empty_with_watcher(
                    Arc::new(LanguageRegistry::new()),
                    Arc::new(FsWatcher::new()),
                    cx,
                )
            }),
            vec![
                ProjectEntry {
                    paths: vec!["/tmp/alpha-project".into()],
                },
                ProjectEntry {
                    paths: vec!["/tmp/beta".into()],
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
            .main_root()
            .expect("当前项目有根目录")
            .to_string_lossy()
            .into_owned();
        let delegate = RecentProjectsDelegate::new(
            project,
            vec![
                ProjectEntry {
                    paths: vec![other_dir.path().to_string_lossy().into_owned()],
                },
                ProjectEntry {
                    paths: vec![current.clone()],
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
    assert_eq!(delegate.projects[1].paths, vec![current.clone()]);
    assert_eq!(reported.take().as_deref(), Some("不允许删除当前项目"));

    assert!(
        cx.update(|cx| delegate.remove_project(0, cx))
            .expect("其他项目应可移除")
    );
    assert_eq!(delegate.projects.len(), 1);
    assert_eq!(delegate.projects[0].paths, vec![current]);
    assert!(
        reported.take().is_none(),
        "移除其他项目不应提示当前项目错误"
    );
}

#[gpui::test]
fn current_project_is_selected_by_path_and_check_stays_after_its_name(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let directory = tempfile::tempdir().expect("应创建当前项目目录");
    let languages = Arc::new(LanguageRegistry::new());
    cx.update(|cx| zcv_editor::init(cx, languages.clone()));
    let (picker, cx) = cx.add_window_view(|window, cx| {
        let project = test_project(directory.path().to_path_buf(), languages, cx);
        let root = project
            .read(cx)
            .main_root()
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
                ProjectEntry {
                    paths: vec![same_name],
                },
                ProjectEntry {
                    paths: vec!["/tmp/other".into()],
                },
                ProjectEntry {
                    paths: vec![current],
                },
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
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let directory = tempfile::tempdir().expect("应创建当前项目目录");
    let languages = Arc::new(LanguageRegistry::new());
    cx.update(|cx| zcv_editor::init(cx, languages.clone()));
    let (selector, cx) = cx.add_window_view(|window, cx| {
        let workspace = cx.new(|cx| empty_workspace(languages.clone(), window, cx));
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
        let selected_paths: Vec<PathBuf> = selected
            .paths
            .iter()
            .map(|path| PathBuf::from(path.as_str()))
            .collect();
        let project_roots: Vec<PathBuf> = delegate
            .project
            .read(cx)
            .roots()
            .map(Path::to_path_buf)
            .collect();
        assert_eq!(selected_paths, project_roots);
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
        let selected_paths: Vec<PathBuf> = selected
            .paths
            .iter()
            .map(|path| PathBuf::from(path.as_str()))
            .collect();
        let project_roots: Vec<PathBuf> = delegate
            .project
            .read(cx)
            .roots()
            .map(Path::to_path_buf)
            .collect();
        assert_eq!(selected_paths, project_roots);
        assert_eq!(picker.search_input().text(cx), "");
    });
}

#[gpui::test]
fn project_button_uses_active_repository_root_and_remaining_count(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let frontend = directory.path().join("frontend");
    let backend = directory.path().join("backend");
    let shared = directory.path().join("shared");
    fs::create_dir_all(&frontend).expect("应创建前端项目目录");
    fs::create_dir_all(&backend).expect("应创建后端项目目录");
    fs::create_dir_all(&shared).expect("应创建共享项目目录");
    let frontend = zcv_path::AbsolutePathBuf::canonicalize(&frontend)
        .expect("前端项目路径应可规范化")
        .into_path_buf();
    let backend = zcv_path::AbsolutePathBuf::canonicalize(&backend)
        .expect("后端项目路径应可规范化")
        .into_path_buf();
    for root in [&frontend, &backend] {
        let output = Command::new("git")
            .args(["init", "-q"])
            .current_dir(root)
            .output()
            .expect("应初始化 Git 仓库");
        assert!(output.status.success(), "初始化 Git 仓库应成功");
    }

    let languages = Arc::new(LanguageRegistry::new());
    let project = cx.update(|cx| {
        test_project_roots(
            vec![frontend.clone(), backend.clone(), shared],
            languages,
            cx,
        )
    });
    // 扫描完成前没有活动仓库，按钮以首个根为显示基准；数量按项目根计算。
    assert_eq!(
        cx.update(|cx| {
            let (name, suffix) = project_button_parts(project.read(cx), cx);
            format!("{name}{}", suffix.expect("多项目应显示数量后缀"))
        }),
        "frontend...+2"
    );

    let store = project.read_with(cx, |project, _| project.git_store());
    for _ in 0..20 {
        cx.run_until_parked();
        if cx.read_entity(&store, |store, _| store.repositories().count()) == 2 {
            break;
        }
    }
    assert_eq!(
        cx.read_entity(&store, |store, _| store.repositories().count()),
        2
    );
    store.update(cx, |store, cx| {
        store.set_active_repository(
            zcv_path::AbsolutePathBuf::canonicalize(&backend).expect("后端项目路径应可规范化"),
            cx,
        );
    });
    cx.run_until_parked();

    assert_eq!(
        cx.update(|cx| {
            let (name, suffix) = project_button_parts(project.read(cx), cx);
            format!("{name}{}", suffix.expect("多项目应显示数量后缀"))
        }),
        "backend...+2"
    );
}

#[gpui::test]
fn project_button_label_has_a_width_limit(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let long_root = directory.path().join("x".repeat(120));
    let short_root = directory.path().join("short");
    fs::create_dir_all(&long_root).expect("应创建长名称项目目录");
    fs::create_dir_all(&short_root).expect("应创建短名称项目目录");
    let languages = Arc::new(LanguageRegistry::new());
    cx.update(|cx| zcv_editor::init(cx, languages.clone()));
    let (_selector, cx) = cx.add_window_view(|window, cx| {
        let project = test_project_roots(vec![long_root, short_root], languages.clone(), cx);
        let workspace = cx.new(|cx| empty_workspace(languages, window, cx));
        RecentProjects::new(
            Rc::new(|_, _, _| {}),
            project,
            workspace.downgrade(),
            PopoverLayer::default(),
            window,
            cx,
        )
    });
    let label = cx
        .debug_bounds("recent-projects-label")
        .expect("项目按钮名称应完成布局");
    assert!(
        label.size.width <= px(190.0),
        "长项目名必须限制在按钮宽度内，实际宽度为 {}",
        label.size.width
    );
    assert!(
        cx.debug_bounds("recent-projects-count").is_some(),
        "项目名截断时其余项目数量后缀仍应可见"
    );
}

#[gpui::test]
fn empty_workspace_does_not_mark_a_recent_project_as_current(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
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
