//! 语言服务器状态面 HostEffect 落地。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Entity, FocusHandle, Window};
use zom_command::{HostEffect, SurfaceEffect};

use crate::app::App;
use crate::shell::features::language_servers::{self, LanguageServersRuntime};
use crate::shell::surfaces::SurfaceManager;
use crate::shell::view::actions::open_surface;
use crate::ui_id::SurfaceId;

pub(crate) fn try_apply_effect(
    effect: &HostEffect,
    app: &Rc<RefCell<App>>,
    surfaces: &Entity<SurfaceManager>,
    editor_focus_fallback: &FocusHandle,
    language_servers_runtime: &LanguageServersRuntime,
    window: &mut Window,
    cx: &mut gpui::App,
) -> bool {
    match effect {
        HostEffect::Surface(SurfaceEffect::ShowLanguageServers) => {
            if surfaces.read_with(cx, |manager, _| manager.is_active(SurfaceId::ProjectPicker)) {
                app.borrow_mut().project_picker_deactivate();
            }
            open_surface(
                language_servers::request(language_servers_runtime.clone()),
                surfaces,
                editor_focus_fallback,
                window,
                cx,
            );
        }
        _ => return false,
    }
    true
}
