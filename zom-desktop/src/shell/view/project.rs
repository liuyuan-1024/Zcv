//! 本地项目打开流程。

use std::cell::RefCell;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;

use gpui::{Entity, Window};

use crate::app::App;
use crate::shell::features::file_tree::FileTreeRuntime;
use crate::shell::platform::project as platform_project;
use crate::shell::surfaces::SurfaceManager;
use crate::shell::workbench::controller::WorkbenchController;

use super::actions;

pub(super) fn open_local_project(
    app: Rc<RefCell<App>>,
    workbench: Rc<RefCell<WorkbenchController>>,
    surfaces: &Entity<SurfaceManager>,
    file_tree: FileTreeRuntime,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    app.borrow_mut().project_picker_deactivate();
    actions::dismiss_surface(surfaces, window, cx);
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

pub(super) fn open_recent_project(
    app: Rc<RefCell<App>>,
    workbench: Rc<RefCell<WorkbenchController>>,
    surfaces: &Entity<SurfaceManager>,
    file_tree: FileTreeRuntime,
    project_root: PathBuf,
    repo: Option<String>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    app.borrow_mut().project_picker_deactivate();
    actions::dismiss_surface(surfaces, window, cx);
    if let Some(repo) = repo {
        apply_git_project_open(&app, &workbench, &file_tree, project_root, repo, window);
    } else {
        apply_project_open(&app, &workbench, &file_tree, project_root, window);
    }
}

pub(super) fn clone_git_project(
    app: Rc<RefCell<App>>,
    workbench: Rc<RefCell<WorkbenchController>>,
    surfaces: &Entity<SurfaceManager>,
    file_tree: FileTreeRuntime,
    repo: String,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    app.borrow_mut().project_picker_deactivate();
    actions::dismiss_surface(surfaces, window, cx);
    let selection = platform_project::prompt_for_clone_parent(cx);
    window
        .spawn(cx, async move |cx| {
            let Some(parent) = selection.await else {
                return;
            };
            let destination = parent.join(infer_repo_directory_name(&repo));
            let repo_for_clone = repo.clone();
            let destination_for_clone = destination.clone();
            let clone_result = clone_repo(&repo_for_clone, destination_for_clone);
            if let Err(error) = clone_result {
                eprintln!("克隆 Git 项目失败：{error}");
                return;
            }
            if let Err(error) = cx.update(|window, _| {
                apply_git_project_open(&app, &workbench, &file_tree, destination, repo, window);
            }) {
                eprintln!("打开克隆项目失败：{error}");
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

fn apply_git_project_open(
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    file_tree: &FileTreeRuntime,
    project_root: PathBuf,
    repo: String,
    window: &mut Window,
) {
    app.borrow_mut().open_git_project(project_root, repo);
    file_tree.reveal_after_project_open(app, workbench, window);
    window.refresh();
}

fn clone_repo(repo: &str, destination: PathBuf) -> Result<(), String> {
    if destination.exists() {
        return Err(format!("目标目录已存在：{}", destination.display()));
    }
    let output = Command::new("git")
        .arg("clone")
        .arg("--")
        .arg(repo)
        .arg(&destination)
        .output()
        .map_err(|error| format!("无法启动 git：{error}"))?;
    if output.status.success() {
        return Ok(());
    }
    let stderr = String::from_utf8_lossy(&output.stderr);
    Err(stderr.trim().to_string())
}

fn infer_repo_directory_name(repo: &str) -> String {
    let trimmed = repo.trim().trim_end_matches('/');
    let last_segment = trimmed
        .rsplit(['/', ':'])
        .find(|segment| !segment.is_empty())
        .unwrap_or("project");
    let without_git = last_segment.strip_suffix(".git").unwrap_or(last_segment);
    let sanitized: String = without_git
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.') {
                ch
            } else {
                '-'
            }
        })
        .collect();
    if sanitized.is_empty() {
        "project".to_string()
    } else {
        sanitized
    }
}
