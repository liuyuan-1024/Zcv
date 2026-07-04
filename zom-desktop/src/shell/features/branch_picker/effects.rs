//! branch_picker HostEffect 落地 —— 走 surface 系统。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Entity, FocusHandle, Window};
use zom_command::{BranchEffect, BubbleRequest, HostEffect};

use crate::app::App;
use crate::focus::AppFocus;
use crate::shell::bubble::BubbleRuntime;
use crate::shell::features::branch_picker::{self, BranchPickerRuntime};
use crate::shell::surfaces::SurfaceManager;
use crate::shell::view::actions::{dismiss_surface, open_surface, request_focus};
use crate::shell::view::focus::FocusProjection;

pub(crate) fn try_apply_effect(
    effect: &HostEffect,
    app: &Rc<RefCell<App>>,
    focus: &FocusProjection,
    surfaces: &Entity<SurfaceManager>,
    editor_focus_fallback: &FocusHandle,
    branch_picker_runtime: &BranchPickerRuntime,
    bubbles: &Entity<BubbleRuntime>,
    window: &mut Window,
    cx: &mut gpui::App,
) -> bool {
    match effect {
        HostEffect::Branch(BranchEffect::ShowPicker) => {
            // 从 git 获取分支列表
            let git = branch_picker_runtime.git_handle();
            match git.borrow().list_branches() {
                Ok(branches) => {
                    branch_picker_runtime.set_branches(branches);
                    open_surface(
                        branch_picker::request(branch_picker_runtime.clone()),
                        surfaces,
                        editor_focus_fallback,
                        window,
                        cx,
                    );
                    request_focus(app, focus, AppFocus::branch_picker(), window);
                }
                Err(e) => {
                    bubbles.update(cx, |runtime, cx| {
                        runtime.push(
                            BubbleRequest::error(format!("获取分支列表失败：{e}"))
                                .dedupe("branch_picker.list_error"),
                            cx,
                        );
                    });
                }
            }
        }
        HostEffect::Branch(BranchEffect::Dismiss) => {
            dismiss_surface(surfaces, window, cx);
            let previous = app.borrow_mut().restore_previous_focus();
            focus.apply(previous, window);
        }
        HostEffect::Branch(BranchEffect::MoveSelection(delta)) => {
            branch_picker_runtime.move_selection(*delta);
        }
        HostEffect::Branch(BranchEffect::Switch) => {
            if let Some(name) = branch_picker_runtime.selected_branch() {
                if branch_picker_runtime.selected_is_current() {
                    // 已是当前分支，仅关闭 surface
                    dismiss_surface(surfaces, window, cx);
                    let previous = app.borrow_mut().restore_previous_focus();
                    focus.apply(previous, window);
                    return true;
                }

                let git = branch_picker_runtime.git_handle();
                let git_ref = git.borrow();
                if git_ref.has_changes() {
                    bubbles.update(cx, |runtime, cx| {
                        runtime.push(
                            BubbleRequest::error(
                                "当前分支有未提交的变更，请先提交或暂存后再切换分支。",
                            )
                            .dedupe("branch_picker.has_changes"),
                            cx,
                        );
                    });
                    return true;
                }
                drop(git_ref);
                match git.borrow().switch_branch(&name) {
                    Ok(()) => {
                        // 更新 App 中的分支名并刷新 git 状态
                        let project_root = app.borrow().project_root().map(|p| p.to_path_buf());
                        app.borrow_mut().set_branch(name.clone());
                        if let Some(root) = project_root {
                            app.borrow_mut()
                                .apply_open_project_from_effect(root, Some(name.clone()));
                        }
                        dismiss_surface(surfaces, window, cx);
                        let previous = app.borrow_mut().restore_previous_focus();
                        focus.apply(previous, window);
                        bubbles.update(cx, |runtime, cx| {
                            runtime.push(
                                BubbleRequest::success(format!("已切换到分支 {name}"))
                                    .dedupe("branch_picker.switch_ok"),
                                cx,
                            );
                        });
                        window.refresh();
                    }
                    Err(e) => {
                        bubbles.update(cx, |runtime, cx| {
                            runtime.push(
                                BubbleRequest::error(format!("切换分支失败：{e}"))
                                    .dedupe("branch_picker.switch_error"),
                                cx,
                            );
                        });
                    }
                }
            }
        }
        HostEffect::Branch(BranchEffect::DeleteSelected) => {
            if let Some(name) = branch_picker_runtime.selected_branch() {
                let git = branch_picker_runtime.git_handle();
                match git.borrow().delete_branch(&name) {
                    Ok(()) => {
                        // 刷新分支列表
                        if let Ok(branches) = git.borrow().list_branches() {
                            branch_picker_runtime.set_branches(branches);
                        }
                        bubbles.update(cx, |runtime, cx| {
                            runtime.push(
                                BubbleRequest::success(format!("已删除分支 {name}"))
                                    .dedupe("branch_picker.delete_ok"),
                                cx,
                            );
                        });
                        window.refresh();
                    }
                    Err(e) => {
                        bubbles.update(cx, |runtime, cx| {
                            runtime.push(
                                BubbleRequest::error(format!("删除分支失败：{e}"))
                                    .dedupe("branch_picker.delete_error"),
                                cx,
                            );
                        });
                    }
                }
            }
        }
        _ => return false,
    }
    true
}
