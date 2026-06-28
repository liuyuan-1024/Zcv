//! go_to_line HostEffect 落地 —— 走 surface 系统。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Entity, FocusHandle, Window};
use zom_command::{GoToLineEffect, HostEffect};

use crate::app::App;
use crate::focus::AppFocus;
use crate::shell::features::go_to_line::{self, GoToLineRuntime};
use crate::shell::surfaces::SurfaceManager;
use crate::shell::view::actions::{dismiss_surface, open_surface, request_focus};
use crate::shell::view::focus::FocusProjection;

pub(crate) fn try_apply_effect(
    effect: &HostEffect,
    app: &Rc<RefCell<App>>,
    focus: &FocusProjection,
    surfaces: &Entity<SurfaceManager>,
    editor_focus_fallback: &FocusHandle,
    go_to_line_runtime: &GoToLineRuntime,
    window: &mut Window,
    cx: &mut gpui::App,
) -> bool {
    match effect {
        HostEffect::GoToLine(GoToLineEffect::Activate) => {
            open_surface(
                go_to_line::request(go_to_line_runtime.clone()),
                surfaces,
                editor_focus_fallback,
                window,
                cx,
            );
            request_focus(app, focus, AppFocus::go_to_line(), window);
        }
        HostEffect::GoToLine(GoToLineEffect::Dismiss) => {
            dismiss_surface(surfaces, window, cx);
            let previous = app.borrow_mut().restore_previous_focus();
            focus.apply(previous, window);
        }
        HostEffect::GoToLine(GoToLineEffect::Jump(target_byte)) => {
            dismiss_surface(surfaces, window, cx);
            app.borrow_mut().go_to_line_jump(*target_byte);
            let previous = app.borrow_mut().restore_previous_focus();
            focus.apply(previous, window);
        }
        _ => return false,
    }
    true
}
