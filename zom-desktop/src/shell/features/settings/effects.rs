//! 设置 HostEffect 落地。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Entity, FocusHandle, Window};
use zom_command::HostEffect;

use crate::app::App;
use crate::shell::features::settings::{self, SettingsRuntime};
use crate::shell::surfaces::SurfaceManager;
use crate::shell::view::actions::open_surface;
use crate::ui_id::SurfaceId;

pub(crate) fn try_apply_effect(
    effect: &HostEffect,
    app: &Rc<RefCell<App>>,
    surfaces: &Entity<SurfaceManager>,
    editor_focus_fallback: &FocusHandle,
    settings_runtime: &SettingsRuntime,
    window: &mut Window,
    cx: &mut gpui::App,
) -> bool {
    match effect {
        HostEffect::ShowSettings => {
            if surfaces.read_with(cx, |manager, _| manager.is_active(SurfaceId::ProjectPicker)) {
                app.borrow_mut().project_picker_deactivate();
            }
            open_surface(
                settings::request(settings_runtime.clone()),
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
