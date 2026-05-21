//! 本地项目打开流程。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Entity, Window};

use crate::app::App;
use crate::shell::features::file_tree::FileTreeRuntime;
use crate::shell::platform::project as platform_project;
use crate::shell::workbench::controller::WorkbenchController;
use crate::shell::workbench::overlay::OverlayManager;

use super::actions;

pub(super) fn open_local_project(
    app: Rc<RefCell<App>>,
    workbench: Rc<RefCell<WorkbenchController>>,
    overlays: &Entity<OverlayManager>,
    file_tree: FileTreeRuntime,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    actions::dismiss_overlay(overlays, window, cx);
    let selection = platform_project::prompt_for_local_project(cx);
    window
        .spawn(cx, async move |cx| {
            let Some(project_root) = selection.await else {
                return;
            };
            if let Err(error) = cx.update(|window, _| {
                apply_project_open(&app, &workbench, &file_tree, project_root, window);
            }) {
                eprintln!("打开本地项目失败：{error}");
            }
        })
        .detach();
}

/// 打开本地项目的统一落点：更新 `App` 状态、展开并聚焦文件树、刷新窗口。
/// 选择器流程与开发阶段默认项目都经由此函数，保证两条路径行为一致。
pub(super) fn apply_project_open(
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    file_tree: &FileTreeRuntime,
    project_root: std::path::PathBuf,
    window: &mut Window,
) {
    app.borrow_mut().open_local_project(project_root);
    file_tree.reveal_after_project_open(app, workbench, window);
    window.refresh();
}
