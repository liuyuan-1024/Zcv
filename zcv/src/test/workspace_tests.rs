use std::sync::Arc;

use gpui::{AppContext, TestAppContext};
use zcv_language::LanguageRegistry;

use super::{DockPosition, Workspace, build_workspace};

fn test_languages() -> Arc<LanguageRegistry> {
    Arc::new(LanguageRegistry::new())
}

/// 空工作区与项目工作区走同一条装配路径：全部面板无条件注册，空态由面板自行渲染。
#[gpui::test]
fn empty_workspace_installs_all_panels(cx: &mut TestAppContext) {
    cx.update(|cx| {
        zcv_settings::init(cx);
        zcv_editor::init(cx);
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
        zcv_settings::init(cx);
        zcv_editor::init(cx);
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
