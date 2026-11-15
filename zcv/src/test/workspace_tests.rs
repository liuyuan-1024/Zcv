use std::{fs, sync::Arc};

use futures::channel::oneshot;
use gpui::{AppContext, BorrowAppContext as _, TestAppContext};
use url::Url;

use crate::app_menus;
use zcv_actions::{About, AddDirectoryToProject, OpenLocalFile, OpenSettings};
use zcv_language::LanguageRegistry;
use zcv_settings::SettingsStore;

use super::{
    DockPosition, Workspace, build_workspace, file_path_from_url, handle_open_urls,
    retain_prompt_until_answer,
};
use crate::test_support::{init_test_settings, quiet_watcher, quiet_watcher_factory};

#[gpui::test]
fn prompt_remains_answerable_until_dismissed(cx: &mut TestAppContext) {
    let (answer, prompt) = oneshot::channel();
    cx.update(|cx| retain_prompt_until_answer(prompt, cx));
    cx.run_until_parked();
    assert!(answer.send(0).is_ok());
}

#[gpui::test]
fn open_settings_opens_a_single_settings_window(cx: &mut TestAppContext) {
    cx.update(|cx| {
        init_test_settings(cx);
        zcv_editor::init(cx, test_languages());
        app_menus::install(cx);
        crate::settings_window::install(cx, test_languages(), quiet_watcher_factory());
    });
    let (_workspace, visual) = cx.add_window_view(|window, cx| {
        build_workspace(&[], test_languages(), quiet_watcher(), window, cx)
    });

    visual.dispatch_action(OpenSettings);
    visual.run_until_parked();

    let count = visual.update(|_, cx| {
        cx.windows()
            .into_iter()
            .filter(|window| {
                window
                    .downcast::<zcv_settings_window::SettingsWindow>()
                    .is_some()
            })
            .count()
    });
    assert_eq!(count, 1, "打开设置应创建独立设置窗口");

    visual.dispatch_action(OpenSettings);
    visual.run_until_parked();
    let count = visual.update(|_, cx| {
        cx.windows()
            .into_iter()
            .filter(|window| {
                window
                    .downcast::<zcv_settings_window::SettingsWindow>()
                    .is_some()
            })
            .count()
    });
    assert_eq!(count, 1, "重复打开设置应复用已有窗口");
}

#[gpui::test]
fn opening_files_from_settings_window_uses_the_workspaces_pane(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let path = directory.path().join("settings.jsonc");
    fs::write(&path, "{}").expect("应写入临时设置文件");
    let path = zcv_path::AbsolutePathBuf::canonicalize(&path)
        .expect("应解析设置文件路径")
        .into_path_buf();
    let languages = test_languages();
    cx.update(|cx| {
        init_test_settings(cx);
        zcv_editor::init(cx, languages.clone());
    });
    let (workspace, _) = cx.add_window_view(|window, cx| {
        build_workspace(&[], languages.clone(), quiet_watcher(), window, cx)
    });
    let (_, settings_visual) = cx.add_window_view(zcv_settings_window::SettingsWindow::new);
    settings_visual.update(|window, cx| {
        window.activate_window();
        cx.defer({
            let path = path.clone();
            let languages = languages.clone();
            move |cx| {
                super::open_paths_in_workspace(
                    vec![path],
                    vec![],
                    languages,
                    quiet_watcher_factory(),
                    cx,
                )
                .expect("应从独立窗口委托工作区打开文件");
            }
        });
    });
    cx.run_until_parked();
    cx.read_entity(&workspace, |workspace, cx| {
        let pane = workspace.pane().read(cx);
        assert_eq!(
            pane.active_item().and_then(|item| item.item_path(cx)),
            Some(path)
        );
    });
}

#[gpui::test]
fn unsupported_files_open_as_tabs_without_a_toast(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let root = directory.path().canonicalize().expect("应获得临时目录路径");
    let binary = root.join("binary.bin");
    let invalid_utf8 = root.join("invalid.txt");
    fs::write(&binary, b"binary\0data").expect("应写入二进制文件");
    fs::write(&invalid_utf8, [0xff]).expect("应写入无效 UTF-8 文件");
    let languages = test_languages();
    cx.update(|cx| {
        init_test_settings(cx);
        zcv_editor::init(cx, languages.clone());
    });
    let (workspace, visual) = cx
        .add_window_view(|window, cx| build_workspace(&[], languages, quiet_watcher(), window, cx));

    for path in [&binary, &invalid_utf8, &binary] {
        workspace.update_in(visual, |workspace, window, cx| {
            workspace.open_path(path.clone(), true, window, cx);
        });
        visual.run_until_parked();
        visual.read_entity(&workspace, |workspace, cx| {
            let pane = workspace.pane().read(cx);
            let active = pane.active_item().expect("应打开文件标签");
            assert_eq!(active.item_path(cx).as_deref(), Some(path.as_path()));
            assert!(active.multi_buffer(cx).is_none());
            assert!(!active.can_save(cx));
        });
        assert!(visual.debug_bounds("breadcrumbs-button").is_some());
        assert!(visual.debug_bounds("toast-0").is_none());
    }
    visual.read_entity(&workspace, |workspace, cx| {
        assert_eq!(workspace.pane().read(cx).tabs().len(), 2);
    });
}

#[gpui::test]
fn background_activity_stays_visible_until_all_tasks_finish(cx: &mut TestAppContext) {
    use gpui::{App, Entity, Subscription};
    use zcv_activity::{ActivityChanged, ActivityPhase, ActivitySource, BackgroundActivity};
    use zcv_workspace::ActivityIndicator;

    struct TestSource(Entity<Vec<&'static str>>);

    impl ActivitySource for TestSource {
        fn observe(&self, changed: ActivityChanged, cx: &mut App) -> Subscription {
            cx.observe(&self.0, move |_, cx| changed(cx))
        }

        fn collect(&self, activities: &mut Vec<BackgroundActivity>, cx: &App) {
            activities.extend(
                self.0
                    .read(cx)
                    .iter()
                    .copied()
                    .map(|name| BackgroundActivity::new(name, ActivityPhase::Running)),
            );
        }
    }

    cx.update(init_test_settings);
    let first = cx.new(|_| vec!["执行 Git 操作"]);
    let last = cx.new(|_| vec!["编译 LaTeX"]);
    let (_, visual) = cx.add_window_view(|_, cx| {
        ActivityIndicator::new(
            vec![
                Box::new(TestSource(first.clone())),
                Box::new(TestSource(last.clone())),
            ],
            cx,
        )
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("workspace-background-task").is_some());

    first.update(visual, |activities, cx| {
        activities.clear();
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("workspace-background-task").is_some());

    last.update(visual, |activities, cx| {
        activities.clear();
        cx.notify();
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("workspace-background-task").is_none());
}

#[gpui::test]
fn latex_preview_precedes_search_on_the_document_toolbar(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let root = directory.path().canonicalize().expect("应获得临时目录路径");
    let latex = root.join("paper.tex");
    let text = root.join("notes.txt");
    fs::write(&latex, r"\documentclass{article}").expect("应写入 LaTeX 文件");
    fs::write(&text, "笔记").expect("应写入文本文件");
    let languages = test_languages();
    cx.update(|cx| {
        init_test_settings(cx);
        zcv_editor::init(cx, languages.clone());
        zcv_preview_latex::init(cx);
    });
    let (workspace, visual) = cx
        .add_window_view(|window, cx| build_workspace(&[], languages, quiet_watcher(), window, cx));

    workspace.update_in(visual, |workspace, window, cx| {
        workspace.open_path(latex, true, window, cx);
    });
    visual.run_until_parked();
    let breadcrumbs = visual
        .debug_bounds("breadcrumbs-button")
        .expect("应显示面包屑");
    let preview = visual
        .debug_bounds("document-preview")
        .expect("应显示编译按钮");
    let search = visual
        .debug_bounds("toolbar-file-search")
        .expect("应显示搜索按钮");
    assert_eq!(preview.center().y, search.center().y);
    assert!(preview.contains(&gpui::point(preview.center().x, breadcrumbs.center().y)));
    assert!(preview.right() <= search.origin.x);

    workspace.update_in(visual, |workspace, window, cx| {
        workspace.open_path(text, true, window, cx);
    });
    visual.run_until_parked();
    assert!(visual.debug_bounds("document-preview").is_none());
    assert!(visual.debug_bounds("toolbar-file-search").is_some());
}

#[gpui::test]
fn settings_window_tracks_application_typography(cx: &mut TestAppContext) {
    cx.update(|cx| {
        init_test_settings(cx);
    });
    let (_, visual) = cx.add_window_view(zcv_settings_window::SettingsWindow::new);
    visual.update(|_, cx| {
        cx.update_global::<SettingsStore, _>(|store, _| {
            store.set_user_settings(r#"{"ui_font_size": 19}"#).unwrap();
        });
    });
    visual.run_until_parked();
    visual.update(|window, cx| {
        assert_eq!(zcv_theme::typography::ui_size(cx), gpui::px(19.0));
        assert_eq!(window.rem_size(), gpui::px(19.0));
    });
}

#[gpui::test]
fn about_action_displays_current_version(cx: &mut TestAppContext) {
    cx.update(|cx| {
        init_test_settings(cx);
        zcv_editor::init(cx, test_languages());
        crate::app_menus::install(cx);
    });
    let (workspace, visual) = cx.add_window_view(|window, cx| {
        build_workspace(&[], test_languages(), quiet_watcher(), window, cx)
    });
    let focus = visual.read_entity(&workspace, |workspace, _| workspace.focus_handle().clone());
    visual.update(|window, cx| window.focus(&focus, cx));
    visual.dispatch_action(About);
    visual.run_until_parked();

    assert_eq!(
        cx.pending_prompt(),
        Some((
            "关于 Zcv".to_owned(),
            format!("当前版本：{}", env!("CARGO_PKG_VERSION"))
        ))
    );
    cx.simulate_prompt_answer("知道了");
}

fn test_languages() -> Arc<LanguageRegistry> {
    Arc::new(LanguageRegistry::new())
}

#[gpui::test]
fn add_directory_action_prompts_for_directories_and_ignores_empty_selection(
    cx: &mut TestAppContext,
) {
    let project_dir = tempfile::tempdir().expect("应创建项目目录");
    let languages = test_languages();
    cx.update(|cx| {
        init_test_settings(cx);
        zcv_editor::init(cx, languages.clone());
        app_menus::install(cx);
        super::install_add_directory_to_project_action(cx);
    });
    let (workspace, visual) = cx.add_window_view(|window, cx| {
        build_workspace(
            &[zcv_path::AbsolutePathBuf::canonicalize(project_dir.path())
                .expect("项目根应可规范化")],
            languages,
            quiet_watcher(),
            window,
            cx,
        )
    });
    let focus = visual.read_entity(&workspace, |workspace, _| workspace.focus_handle().clone());
    visual.update(|window, cx| {
        window.activate_window();
        window.focus(&focus, cx);
    });

    visual.dispatch_action(AddDirectoryToProject);
    assert!(visual.did_prompt_for_paths());
    // 目录选择允许多选：一次可加入多个工作区根。
    visual.simulate_path_prompt_response(|options| {
        assert!(!options.files);
        assert!(options.directories);
        assert!(options.multiple);
        Some(Vec::new())
    });
    visual.run_until_parked();

    // 空选择不改变根集合。
    visual.read_entity(&workspace, |workspace, cx| {
        assert_eq!(workspace.project().read(cx).roots().count(), 1);
    });
}

#[gpui::test]
fn add_directory_action_skips_unaddable_paths_without_changing_roots(cx: &mut TestAppContext) {
    let project_dir = tempfile::tempdir().expect("应创建项目目录");
    let languages = test_languages();
    cx.update(|cx| {
        init_test_settings(cx);
        zcv_editor::init(cx, languages.clone());
        app_menus::install(cx);
        super::install_add_directory_to_project_action(cx);
    });
    let (workspace, visual) = cx.add_window_view(|window, cx| {
        build_workspace(
            &[zcv_path::AbsolutePathBuf::canonicalize(project_dir.path())
                .expect("项目根应可规范化")],
            languages,
            quiet_watcher(),
            window,
            cx,
        )
    });
    let focus = visual.read_entity(&workspace, |workspace, _| workspace.focus_handle().clone());
    visual.update(|window, cx| {
        window.activate_window();
        window.focus(&focus, cx);
    });

    // 不存在的目录无法规范化：整条拒绝，不产生部分添加。
    visual.dispatch_action(AddDirectoryToProject);
    assert!(visual.did_prompt_for_paths());
    visual.simulate_path_prompt_response(|_options| {
        Some(vec!["/definitely/missing/zcv-add-dir".into()])
    });
    visual.run_until_parked();

    visual.read_entity(&workspace, |workspace, cx| {
        assert_eq!(workspace.project().read(cx).roots().count(), 1);
    });
}

#[gpui::test]
fn open_local_file_action_opens_file_outside_current_project(cx: &mut TestAppContext) {
    let external_dir = tempfile::tempdir().expect("应创建项目外目录");
    let file = external_dir.path().join("outside.txt");
    fs::write(&file, "项目外文件").expect("应创建项目外文件");
    let canonical_file = zcv_path::AbsolutePathBuf::canonicalize(&file)
        .expect("项目外文件应可规范化")
        .as_path()
        .to_path_buf();
    let languages = test_languages();
    cx.update(|cx| {
        init_test_settings(cx);
        zcv_editor::init(cx, languages.clone());
        app_menus::install(cx);
        super::install_open_local_file_action(cx);
    });
    let (workspace, visual) = cx
        .add_window_view(|window, cx| build_workspace(&[], languages, quiet_watcher(), window, cx));
    let focus = visual.read_entity(&workspace, |workspace, _| workspace.focus_handle().clone());
    visual.update(|window, cx| {
        window.activate_window();
        window.focus(&focus, cx);
    });

    visual.dispatch_action(OpenLocalFile);
    assert!(visual.did_prompt_for_paths());
    visual.simulate_path_prompt_response(|options| {
        assert!(options.files);
        assert!(!options.directories);
        assert!(!options.multiple);
        Some(vec![file.clone()])
    });
    visual.run_until_parked();

    visual.read_entity(&workspace, |workspace, cx| {
        assert_eq!(workspace.project().read(cx).main_root(), None);
        assert_eq!(
            workspace
                .pane()
                .read(cx)
                .active_item()
                .and_then(|item| item.item_path(cx)),
            Some(canonical_file)
        );
    });
}

#[test]
fn finder_file_url_decodes_local_path() {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let path = directory.path().join("love 页面.html");
    let url = Url::from_file_path(&path).expect("本地路径应能编码为文件地址");
    assert_eq!(file_path_from_url(url.as_str()).unwrap(), path);
    assert!(file_path_from_url("https://example.com/love.html").is_err());
}

#[gpui::test]
fn finder_open_event_opens_html_in_active_workspace(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let file = directory.path().join("love 页面.html");
    fs::write(&file, "<h1>你好</h1>").expect("应创建 HTML 文件");
    let canonical_file = zcv_path::AbsolutePathBuf::canonicalize(&file)
        .expect("HTML 文件应可规范化")
        .as_path()
        .to_path_buf();
    let languages = test_languages();
    cx.update(|cx| {
        init_test_settings(cx);
        zcv_editor::init(cx, languages.clone());
        app_menus::install(cx);
    });
    let workspace = {
        let (workspace, visual) = cx.add_window_view(|window, cx| {
            build_workspace(&[], languages.clone(), quiet_watcher(), window, cx)
        });
        visual.update(|window, _| window.activate_window());
        workspace
    };

    let url = Url::from_file_path(&file).expect("HTML 路径应能编码为文件地址");
    cx.update(|cx| {
        handle_open_urls(
            vec![url.to_string()],
            languages,
            quiet_watcher_factory(),
            cx,
        )
    });
    cx.run_until_parked();

    cx.read_entity(&workspace, |workspace, cx| {
        assert_eq!(
            workspace
                .pane()
                .read(cx)
                .active_item()
                .and_then(|item| item.item_path(cx)),
            Some(canonical_file)
        );
    });
}

#[gpui::test]
fn finder_open_event_creates_workspace_when_no_window_exists(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let file = directory.path().join("love.html");
    fs::write(&file, "<h1>你好</h1>").expect("应创建 HTML 文件");
    let canonical_file = zcv_path::AbsolutePathBuf::canonicalize(&file)
        .expect("HTML 文件应可规范化")
        .as_path()
        .to_path_buf();
    let languages = test_languages();
    cx.update(|cx| {
        init_test_settings(cx);
        zcv_editor::init(cx, languages.clone());
        app_menus::install(cx);
    });

    let url = Url::from_file_path(&file).expect("HTML 路径应能编码为文件地址");
    cx.update(|cx| {
        handle_open_urls(
            vec![url.to_string()],
            languages,
            quiet_watcher_factory(),
            cx,
        )
    });
    cx.run_until_parked();

    cx.update(|cx| {
        let windows = cx.windows();
        assert_eq!(windows.len(), 1);
        windows[0]
            .update(cx, |_, window, cx| {
                let workspace = window
                    .root::<Workspace>()
                    .flatten()
                    .expect("窗口应持有工作区");
                let workspace = workspace.read(cx);
                assert_eq!(
                    workspace
                        .pane()
                        .read(cx)
                        .active_item()
                        .and_then(|item| item.item_path(cx)),
                    Some(canonical_file)
                );
            })
            .expect("工作区窗口应有效");
    });
}

/// 空工作区与项目工作区走同一条装配路径：全部面板无条件注册，空态由面板自行渲染。
#[gpui::test]
fn empty_workspace_installs_all_panels(cx: &mut TestAppContext) {
    cx.update(|cx| {
        init_test_settings(cx);
        zcv_editor::init(cx, test_languages());
        app_menus::install(cx);
    });
    let (workspace, cx) = cx.add_window_view(|window, cx| {
        build_workspace(&[], test_languages(), quiet_watcher(), window, cx)
    });

    cx.read_entity(&workspace, |workspace, cx| {
        assert_eq!(workspace.dock(DockPosition::Left).read(cx).panel_count(), 3);
        assert_eq!(
            workspace.dock(DockPosition::Bottom).read(cx).panel_count(),
            1
        );
        // 右 dock 当前无面板：原快捷键面板已由 harness 状态标记按钮取代。
        assert_eq!(
            workspace.dock(DockPosition::Right).read(cx).panel_count(),
            0
        );
    });
}

/// 切换项目在同一窗口内替换工作区根：窗口本体不变，根实体换新。
#[gpui::test]
fn switching_replaces_root_in_same_window(cx: &mut TestAppContext) {
    cx.update(|cx| {
        init_test_settings(cx);
        zcv_editor::init(cx, test_languages());
        app_menus::install(cx);
    });
    let (old_workspace, cx) = cx.add_window_view(|window, cx| {
        build_workspace(&[], test_languages(), quiet_watcher(), window, cx)
    });
    let old_id = old_workspace.entity_id();

    cx.update(|window, app| {
        window.replace_root(app, |window, cx| {
            build_workspace(&[], test_languages(), quiet_watcher(), window, cx)
        });
    });

    // 新根已就位，且不是旧工作区实体。
    cx.update(|window, _| {
        let new_root = window.root::<Workspace>().flatten().expect("新根应已就位");
        assert_ne!(new_root.entity_id(), old_id);
    });
}

#[gpui::test]
fn autosave_distinguishes_document_focus_from_window_deactivation(cx: &mut TestAppContext) {
    use zcv_settings::{AutosaveSetting, UserSettings};

    let mut settings = UserSettings {
        autosave: AutosaveSetting::OnWindowChange,
        ..UserSettings::default()
    };
    cx.update(|cx| {
        init_test_settings(cx);
        cx.set_global(SettingsStore::new(settings.clone()));
    });
    let directory = tempfile::tempdir().unwrap();
    let first = directory.path().join("first.txt");
    let second = directory.path().join("second.txt");
    fs::write(&first, "一\n").unwrap();
    fs::write(&second, "二\n").unwrap();
    let languages = Arc::new(LanguageRegistry::new());
    cx.update(|cx| zcv_editor::init(cx, languages.clone()));
    let (workspace, cx) = cx
        .add_window_view(|window, cx| build_workspace(&[], languages, quiet_watcher(), window, cx));
    cx.update(|window, _| window.activate_window());
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_path(first.clone(), true, window, cx);
    });
    cx.run_until_parked();

    cx.simulate_input("新");
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_path(second.clone(), true, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(&first).unwrap(), "一\n");

    cx.deactivate_window();
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(&first).unwrap(), "新一\n");

    settings.autosave = AutosaveSetting::OnFocusChange;
    cx.update(|window, cx| {
        cx.set_global(SettingsStore::new(settings));
        window.activate_window();
    });
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_path(first.clone(), true, window, cx);
    });
    cx.run_until_parked();
    cx.simulate_input("新");
    cx.run_until_parked();
    workspace.update_in(cx, |workspace, window, cx| {
        workspace.open_path(second.clone(), true, window, cx);
    });
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(&first).unwrap(), "新新一\n");

    cx.update(|_, cx| {
        cx.set_global(SettingsStore::new(UserSettings {
            autosave: AutosaveSetting::AfterDelay {
                milliseconds: 1000.try_into().unwrap(),
            },
            ..UserSettings::default()
        }));
    });
    cx.simulate_input("新");
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(&second).unwrap(), "二\n");
    cx.dispatch_action(zcv_actions::CloseTab);
    cx.run_until_parked();
    assert_eq!(fs::read_to_string(&second).unwrap(), "新二\n");
}
