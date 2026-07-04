//! `HostEffect::VersionControl(…)` → Runtime 动作。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Entity, Window};
use zom_command::{BubbleRequest, HostEffect, VersionControlEffect};

use crate::app::App;
use crate::focus::{AppFocus, PanelFocus};
use crate::shell::bubble::BubbleRuntime;

use super::VersionControlRuntime;

pub(crate) fn try_apply_effect(
    runtime: &VersionControlRuntime,
    effect: &HostEffect,
    app: &Rc<RefCell<App>>,
    bubbles: &Entity<BubbleRuntime>,
    window: &mut Window,
    cx: &mut gpui::App,
) -> Option<()> {
    let mut requests: Vec<BubbleRequest> = Vec::new();
    match effect {
        HostEffect::VersionControl(VersionControlEffect::MoveSelection(delta)) => {
            runtime.move_selection(*delta);
        }
        HostEffect::VersionControl(VersionControlEffect::Toggle) => {
            runtime.toggle_selected();
        }
        HostEffect::VersionControl(VersionControlEffect::Activate) => {
            if let Some(abs_path) = runtime.activate_selected() {
                app.borrow_mut().session.open_file(abs_path);
            }
        }
        HostEffect::VersionControl(VersionControlEffect::CollapseOrParent) => {
            runtime.collapse_or_parent();
        }
        HostEffect::VersionControl(VersionControlEffect::ExpandOrInto) => {
            runtime.expand_or_into();
        }
        HostEffect::VersionControl(VersionControlEffect::EditCommitMessage) => {
            app.borrow_mut()
                .request_focus(AppFocus::Panel(PanelFocus::version_control_commit()));
            // 把 GPUI 焦点转移到提交信息编辑器的独立 FocusHandle——仅更新 AppFocus 不会让窗口系统把按键路由到编辑器。
            window.focus(&runtime.commit_focus_handle());
            window.refresh();
        }
        HostEffect::VersionControl(VersionControlEffect::CancelCommitMessage) => {
            app.borrow_mut()
                .request_focus(AppFocus::Panel(PanelFocus::version_control()));
            window.focus(&runtime.focus_handle());
            window.refresh();
        }
        HostEffect::VersionControl(VersionControlEffect::Commit) => {
            runtime.perform_commit(app);
            requests.append(&mut runtime.take_pending_bubbles());
            window.refresh();
        }
        _ => return None,
    }
    // 将收集到的 bubbles 推入 BubbleRuntime。
    if !requests.is_empty() {
        for request in requests {
            bubbles.update(cx, |runtime, cx| runtime.push(request, cx));
        }
        window.refresh();
    }
    Some(())
}
