//! ShellView 的命令动作与 HostEffect 解释。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Entity, FocusHandle, Window};
use zom_command::{HostEffect, Invocation};

use crate::app::App;
use crate::shell::ActionRequest;
use crate::shell::features::PanelId;
use crate::shell::features::file_tree::FileTreeRuntime;
use crate::shell::platform::window as platform_window;
use crate::shell::shared::element_ids;
use crate::shell::workbench::controller::WorkbenchController;
use crate::shell::workbench::overlay::{OverlayAnchor, OverlayKind, OverlayManager};

use super::project;

pub(super) fn bind_action_request(
    app: Rc<RefCell<App>>,
    workbench: Rc<RefCell<WorkbenchController>>,
    overlays: Entity<OverlayManager>,
    editor_focus_fallback: FocusHandle,
    file_tree: FileTreeRuntime,
    invocation: Invocation,
) -> ActionRequest {
    Rc::new(move |window, cx| {
        let effects = match app.borrow_mut().dispatch(invocation.clone()) {
            Ok(effects) => effects,
            Err(error) => {
                eprintln!("命令执行失败：{error}");
                return;
            }
        };
        apply_host_effects(
            effects,
            &app,
            &workbench,
            &overlays,
            &editor_focus_fallback,
            &file_tree,
            window,
            cx,
        );
    })
}

pub(super) fn apply_host_effects(
    effects: Vec<HostEffect>,
    app: &Rc<RefCell<App>>,
    workbench: &Rc<RefCell<WorkbenchController>>,
    overlays: &Entity<OverlayManager>,
    editor_focus_fallback: &FocusHandle,
    file_tree: &FileTreeRuntime,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    for effect in effects {
        match effect {
            HostEffect::Quit => platform_window::quit(cx),
            HostEffect::Minimize => platform_window::minimize(window),
            HostEffect::ToggleMaximize => platform_window::toggle_maximize(window),
            HostEffect::TogglePanel(panel_str_id) => {
                let Some(panel) = PanelId::from_command_str_id(&panel_str_id) else {
                    eprintln!("HostEffect::TogglePanel 收到未知 panel id：{panel_str_id}");
                    continue;
                };
                if panel == PanelId::FileTree {
                    file_tree.handle_toggle_request(workbench, editor_focus_fallback, window);
                } else {
                    workbench.borrow_mut().toggle_panel(panel);
                    window.refresh();
                }
            }
            HostEffect::ShowProjectPicker => {
                open_overlay(
                    OverlayKind::ProjectPicker,
                    overlays,
                    editor_focus_fallback,
                    window,
                    cx,
                );
            }
            HostEffect::OpenLocalProject => {
                project::open_local_project(
                    Rc::clone(app),
                    Rc::clone(workbench),
                    overlays,
                    file_tree.clone(),
                    window,
                    cx,
                );
            }
            HostEffect::ShowLanguageServers => {
                open_overlay(
                    OverlayKind::LanguageServers,
                    overlays,
                    editor_focus_fallback,
                    window,
                    cx,
                );
            }
            HostEffect::DismissOverlay => dismiss_overlay(overlays, window, cx),
        }
    }
}

pub(super) fn dismiss_overlay(
    overlays: &Entity<OverlayManager>,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let Some(focus_to_restore) = overlays.update(cx, |overlays, cx| overlays.dismiss(cx)) else {
        return;
    };
    window.focus(&focus_to_restore);
    window.refresh();
}

fn open_overlay(
    kind: OverlayKind,
    overlays: &Entity<OverlayManager>,
    editor_focus_fallback: &FocusHandle,
    window: &mut Window,
    cx: &mut gpui::App,
) {
    let anchor = anchor_for_overlay(kind);
    // 手册 21.7：关闭时焦点回到"先前 focus 目标"——open 这一帧 window
    // 里实际聚焦的元素。查不到（窗口刚启动等）退回 editor 焦点，避免
    // 关闭后焦点悬空。
    let focus_to_restore = window
        .focused(cx)
        .unwrap_or_else(|| editor_focus_fallback.clone());
    overlays.update(cx, |overlays, cx| {
        overlays.open(kind, anchor, focus_to_restore, cx);
    });
    window.refresh();
}

fn anchor_for_overlay(kind: OverlayKind) -> OverlayAnchor {
    match kind {
        OverlayKind::ProjectPicker => OverlayAnchor::Element(element_ids::TOP_BAR_WORKSPACE.into()),
        OverlayKind::LanguageServers => {
            OverlayAnchor::Element(element_ids::BOTTOM_BAR_LANGUAGE_SERVER.into())
        }
    }
}
