use std::{fs, sync::Arc};

use futures::channel::oneshot;
use gpui::{AppContext, TestAppContext};
use zcv_actions::{About, OpenLocalFile};
use zcv_language::LanguageRegistry;
use zcv_settings::{
    GlobalSettingsErrorReporter, SettingsErrorReporter, SettingsStore, UserSettings,
};

use super::{DockPosition, Workspace, build_workspace, retain_prompt_until_answer};

#[gpui::test]
fn prompt_remains_answerable_until_dismissed(cx: &mut TestAppContext) {
    let (answer, prompt) = oneshot::channel();
    cx.update(|cx| retain_prompt_until_answer(prompt, cx));
    cx.run_until_parked();
    assert!(answer.send(0).is_ok());
}

#[gpui::test]
fn about_action_displays_current_version(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let reporter = cx.new(|_| SettingsErrorReporter::default());
        cx.set_global(GlobalSettingsErrorReporter(reporter));
        cx.set_global(SettingsStore::new(UserSettings::default()));
        zcv_editor::init(cx, test_languages());
        crate::app_menus::install(cx);
    });
    let (workspace, visual) =
        cx.add_window_view(|window, cx| build_workspace(&None, test_languages(), window, cx));
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
fn open_local_file_action_opens_file_outside_current_project(cx: &mut TestAppContext) {
    let project_dir = tempfile::tempdir().expect("应创建项目目录");
    let external_dir = tempfile::tempdir().expect("应创建项目外目录");
    let file = external_dir.path().join("outside.txt");
    fs::write(&file, "项目外文件").expect("应创建项目外文件");
    let canonical_file = fs::canonicalize(&file).expect("项目外文件应可规范化");
    let root =
        zcv_path::AbsolutePathBuf::canonicalize(project_dir.path()).expect("项目目录应可规范化");
    let languages = test_languages();
    cx.update(|cx| {
        let reporter = cx.new(|_| SettingsErrorReporter::default());
        cx.set_global(GlobalSettingsErrorReporter(reporter));
        cx.set_global(SettingsStore::new(UserSettings::default()));
        zcv_editor::init(cx, languages.clone());
        super::install_open_local_file_action(cx);
    });
    let (workspace, visual) = cx
        .add_window_view(|window, cx| build_workspace(&Some(root.clone()), languages, window, cx));
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
        assert_eq!(workspace.project().read(cx).root(), Some(root.as_path()));
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

/// 空工作区与项目工作区走同一条装配路径：全部面板无条件注册，空态由面板自行渲染。
#[gpui::test]
fn empty_workspace_installs_all_panels(cx: &mut TestAppContext) {
    cx.update(|cx| {
        let reporter = cx.new(|_| SettingsErrorReporter::default());
        cx.set_global(GlobalSettingsErrorReporter(reporter));
        cx.set_global(SettingsStore::new(UserSettings::default()));
        zcv_editor::init(cx, test_languages());
    });
    let (workspace, cx) =
        cx.add_window_view(|window, cx| build_workspace(&None, test_languages(), window, cx));

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
        let reporter = cx.new(|_| SettingsErrorReporter::default());
        cx.set_global(GlobalSettingsErrorReporter(reporter));
        cx.set_global(SettingsStore::new(UserSettings::default()));
        zcv_editor::init(cx, test_languages());
    });
    let (old_workspace, cx) =
        cx.add_window_view(|window, cx| build_workspace(&None, test_languages(), window, cx));
    let old_id = old_workspace.entity_id();

    cx.update(|window, app| {
        window.replace_root(app, |window, cx| {
            build_workspace(&None, test_languages(), window, cx)
        });
    });

    // 新根已就位，且不是旧工作区实体。
    cx.update(|window, _| {
        let new_root = window.root::<Workspace>().flatten().expect("新根应已就位");
        assert_ne!(new_root.entity_id(), old_id);
    });
}
