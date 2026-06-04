//! 本地项目打开流程。

use std::cell::RefCell;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;

use gpui::{Entity, Window};

use crate::app::App;
use crate::shell::features::panels::file_tree::FileTreeRuntime;
use crate::shell::features::project_picker::ProjectPickerRuntime;
use crate::shell::platform::project as platform_project;
use crate::shell::surfaces::SurfaceManager;
use crate::shell::workbench::controller::WorkbenchController;

use super::actions;

pub(crate) fn open_local_project(
    app: Rc<RefCell<App>>,
    workbench: Rc<RefCell<WorkbenchController>>,
    surfaces: &Entity<SurfaceManager>,
    file_tree: FileTreeRuntime,
    project_picker: ProjectPickerRuntime,
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
                apply_local_project_open(
                    &app,
                    &workbench,
                    &file_tree,
                    &project_picker,
                    project_root,
                    window,
                );
            }) {
                eprintln!("打开本地项目失败：{error}");
            }
        })
        .detach();
}

pub(crate) fn open_recent_project(
    app: Rc<RefCell<App>>,
    workbench: Rc<RefCell<WorkbenchController>>,
    surfaces: &Entity<SurfaceManager>,
    file_tree: FileTreeRuntime,
    project_picker: ProjectPickerRuntime,
    project_root: PathBuf,
    repo: Option<String>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    app.borrow_mut().project_picker_deactivate();
    actions::dismiss_surface(surfaces, window, cx);
    if let Some(repo) = repo {
        apply_git_project_open(
            &app,
            &workbench,
            &file_tree,
            &project_picker,
            project_root,
            repo,
            window,
        );
    } else {
        apply_local_project_open(
            &app,
            &workbench,
            &file_tree,
            &project_picker,
            project_root,
            window,
        );
    }
}

pub(crate) fn clone_git_project(
    app: Rc<RefCell<App>>,
    workbench: Rc<RefCell<WorkbenchController>>,
    surfaces: &Entity<SurfaceManager>,
    file_tree: FileTreeRuntime,
    project_picker: ProjectPickerRuntime,
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
                apply_git_project_open(
                    &app,
                    &workbench,
                    &file_tree,
                    &project_picker,
                    destination,
                    repo,
                    window,
                );
            }) {
                eprintln!("打开克隆项目失败：{error}");
            }
        })
        .detach();
}

/// 打开本地项目的统一落点：更新 `App` 状态、登记到最近项目、展开并聚焦文件树、刷新窗口。
/// 选择器流程与开发阶段默认项目都经由此函数，保证两条路径行为一致。
///
/// "登记最近"由 shell 侧显式做 —— `App::open_project` 只负责 workspace / view / focus
/// 这些底层 crate 的状态，"最近项目"是 picker 自家的 UI 数据，归 picker runtime 拥有。
pub(crate) fn apply_local_project_open(
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    file_tree: &FileTreeRuntime,
    project_picker: &ProjectPickerRuntime,
    project_root: std::path::PathBuf,
    window: &mut Window,
) {
    // 增加路径有效性校验：确保路径存在且为目录
    if !project_root.is_dir() {
        eprintln!(
            "打开本地项目失败：项目目录不存在或无效 {}",
            project_root.display()
        );
        return;
    }

    file_tree.open_project(project_root.clone());
    app.borrow_mut().open_project(project_root.clone());
    project_picker.remember_project(project_root, None);
    file_tree.reveal_after_project_open(workbench, window);
    window.refresh();
}

fn apply_git_project_open(
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    file_tree: &FileTreeRuntime,
    project_picker: &ProjectPickerRuntime,
    project_root: PathBuf,
    repo: String,
    window: &mut Window,
) {
    // 增加路径有效性校验：确保路径存在且为目录
    if !project_root.is_dir() {
        eprintln!(
            "打开 Git 项目失败：项目目录不存在或无效 {}",
            project_root.display()
        );
        return;
    }

    file_tree.open_project(project_root.clone());
    app.borrow_mut().open_project(project_root.clone());
    project_picker.remember_project(project_root, Some(repo));
    file_tree.reveal_after_project_open(workbench, window);
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
