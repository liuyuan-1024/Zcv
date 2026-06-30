//! `HostEffect::VersionControl(…)` → Runtime 动作。

use std::cell::RefCell;
use std::rc::Rc;

use zom_command::{HostEffect, VersionControlEffect};

use crate::app::App;

use super::VersionControlRuntime;

pub(crate) fn try_apply_effect(
    runtime: &VersionControlRuntime,
    effect: &HostEffect,
    app: &Rc<RefCell<App>>,
) -> Option<()> {
    match effect {
        HostEffect::VersionControl(VersionControlEffect::MoveSelection(delta)) => {
            runtime.move_selection(*delta);
            Some(())
        }
        HostEffect::VersionControl(VersionControlEffect::Toggle) => {
            runtime.toggle_selected();
            Some(())
        }
        HostEffect::VersionControl(VersionControlEffect::Activate) => {
            if let Some(abs_path) = runtime.activate_selected() {
                app.borrow_mut().session.open_file(abs_path);
            }
            Some(())
        }
        HostEffect::VersionControl(VersionControlEffect::CollapseOrParent) => {
            runtime.collapse_or_parent();
            Some(())
        }
        HostEffect::VersionControl(VersionControlEffect::ExpandOrInto) => {
            runtime.expand_or_into();
            Some(())
        }
        _ => None,
    }
}
