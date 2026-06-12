//! 项目会话打开流程。
//!
//! 本模块是 shell 侧的项目会话边界：负责把“选择/克隆/打开项目”落到
//! App workspace、文件树、最近项目与窗口焦点上。view 层只触发这里的入口，
//! 不直接编排项目状态。

use std::cell::RefCell;
use std::path::PathBuf;
use std::process::Command;
use std::rc::Rc;

use gpui::{Entity, Window};
use zom_command::BubbleRequest;

use crate::app::App;
use crate::shell::bubble::BubbleRuntime;
use crate::shell::features::panels::file_tree::FileTreeRuntime;
use crate::shell::features::project_picker::ProjectPickerRuntime;
use crate::shell::platform::project as platform_project;
use crate::shell::surfaces::SurfaceManager;
use crate::shell::workbench::controller::WorkbenchController;

pub(crate) fn open_local_project(
    app: Rc<RefCell<App>>,
    workbench: Rc<RefCell<WorkbenchController>>,
    surfaces: &Entity<SurfaceManager>,
    file_tree: FileTreeRuntime,
    project_picker: ProjectPickerRuntime,
    bubbles: Entity<BubbleRuntime>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    dismiss_project_picker(&app, surfaces, window, cx);
    let selection = platform_project::prompt_for_local_project(cx);
    window
        .spawn(cx, async move |cx| {
            let project_root = match selection.await {
                Ok(Some(path)) => path,
                Ok(None) => return,
                Err(message) => {
                    let bubbles_for_error = bubbles.clone();
                    cx.update(|_, cx| {
                        bubbles_for_error.update(cx, |runtime, cx| {
                            runtime.push(
                                BubbleRequest::error(message).dedupe("project.open_local"),
                                cx,
                            );
                        });
                    })
                    .ok();
                    return;
                }
            };
            if let Err(error) = cx.update(|window, cx| {
                apply_local_project_open(
                    &app,
                    &workbench,
                    &file_tree,
                    &project_picker,
                    &bubbles,
                    project_root,
                    window,
                    cx,
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
    bubbles: Entity<BubbleRuntime>,
    project_root: PathBuf,
    repo: Option<String>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    dismiss_project_picker(&app, surfaces, window, cx);
    if let Some(repo) = repo {
        apply_git_project_open(
            &app,
            &workbench,
            &file_tree,
            &project_picker,
            &bubbles,
            project_root,
            repo,
            window,
            cx,
        );
    } else {
        apply_local_project_open(
            &app,
            &workbench,
            &file_tree,
            &project_picker,
            &bubbles,
            project_root,
            window,
            cx,
        );
    }
}

pub(crate) fn clone_git_project(
    app: Rc<RefCell<App>>,
    workbench: Rc<RefCell<WorkbenchController>>,
    surfaces: &Entity<SurfaceManager>,
    file_tree: FileTreeRuntime,
    project_picker: ProjectPickerRuntime,
    bubbles: Entity<BubbleRuntime>,
    repo: String,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    dismiss_project_picker(&app, surfaces, window, cx);
    let selection = platform_project::prompt_for_clone_parent(cx);
    window
        .spawn(cx, async move |cx| {
            let parent = match selection.await {
                Ok(Some(path)) => path,
                Ok(None) => return,
                Err(message) => {
                    let bubbles_for_error = bubbles.clone();
                    cx.update(|_, cx| {
                        bubbles_for_error.update(cx, |runtime, cx| {
                            runtime.push(BubbleRequest::error(message).dedupe("project.clone"), cx);
                        });
                    })
                    .ok();
                    return;
                }
            };
            let destination = parent.join(infer_repo_directory_name(&repo));
            let repo_for_clone = repo.clone();
            let destination_for_clone = destination.clone();
            let clone_result = clone_repo(&repo_for_clone, destination_for_clone);
            if let Err(error) = clone_result {
                let bubbles_for_error = bubbles.clone();
                cx.update(|_, cx| {
                    bubbles_for_error.update(cx, |runtime, cx| {
                        runtime.push(
                            BubbleRequest::error(format!("克隆 Git 项目失败：{error}"))
                                .dedupe("project.clone"),
                            cx,
                        );
                    });
                })
                .ok();
                return;
            }
            if let Err(error) = cx.update(|window, cx| {
                apply_git_project_open(
                    &app,
                    &workbench,
                    &file_tree,
                    &project_picker,
                    &bubbles,
                    destination,
                    repo,
                    window,
                    cx,
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
/// “登记最近”由 shell 侧显式做：`App::open_project` 只负责 workspace / view / focus
/// 这些底层 crate 的状态；“最近项目”是 picker 自家的 UI 数据，归 picker runtime 拥有。
pub(crate) fn apply_local_project_open(
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    file_tree: &FileTreeRuntime,
    project_picker: &ProjectPickerRuntime,
    bubbles: &Entity<BubbleRuntime>,
    project_root: PathBuf,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    if !project_root.is_dir() {
        push_bubble(
            bubbles,
            cx,
            format!(
                "打开本地项目失败：项目目录不存在或无效 {}",
                project_root.display()
            ),
            "project.open_local",
        );
        return;
    }

    file_tree.open_project(project_root.clone());
    app.borrow_mut()
        .apply_open_project_from_effect(project_root.clone());
    project_picker.remember_project(project_root, None);
    file_tree.reveal_after_project_open(workbench, window);
    drain_open_bubbles(app, project_picker, bubbles, cx);
    window.refresh();
}

fn apply_git_project_open(
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    file_tree: &FileTreeRuntime,
    project_picker: &ProjectPickerRuntime,
    bubbles: &Entity<BubbleRuntime>,
    project_root: PathBuf,
    repo: String,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    if !project_root.is_dir() {
        push_bubble(
            bubbles,
            cx,
            format!(
                "打开 Git 项目失败：项目目录不存在或无效 {}",
                project_root.display()
            ),
            "project.open_git",
        );
        return;
    }

    file_tree.open_project(project_root.clone());
    app.borrow_mut()
        .apply_open_project_from_effect(project_root.clone());
    project_picker.remember_project(project_root, Some(repo));
    file_tree.reveal_after_project_open(workbench, window);
    drain_open_bubbles(app, project_picker, bubbles, cx);
    window.refresh();
}

fn push_bubble(
    bubbles: &Entity<BubbleRuntime>,
    cx: &mut gpui::App,
    message: String,
    dedupe: &'static str,
) {
    bubbles.update(cx, |runtime, cx| {
        runtime.push(BubbleRequest::error(message).dedupe(dedupe), cx);
    });
}

/// 把 workspace session 与 project picker 累积的气泡一并落到 BubbleRuntime。
fn drain_open_bubbles(
    app: &Rc<RefCell<App>>,
    project_picker: &ProjectPickerRuntime,
    bubbles: &Entity<BubbleRuntime>,
    cx: &mut gpui::App,
) {
    let mut requests = Vec::new();
    requests.extend(app.borrow_mut().take_session_bubbles());
    for warning in project_picker.take_recent_warnings() {
        requests.push(BubbleRequest::error(warning).dedupe("project.recent"));
    }
    for request in requests {
        bubbles.update(cx, |runtime, cx| runtime.push(request, cx));
    }
}

fn dismiss_project_picker(
    app: &Rc<RefCell<App>>,
    surfaces: &Entity<SurfaceManager>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    app.borrow_mut().project_picker_deactivate();
    let Some(focus_to_restore) = surfaces.update(cx, |surfaces, cx| surfaces.dismiss(cx)) else {
        return;
    };
    window.focus(&focus_to_restore);
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
