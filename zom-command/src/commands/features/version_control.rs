//! 版本管理 feature 命令。

use crate::{CommandRegistry, Invocation, Keymap};

pub const TOGGLE_PANEL: &str = "panel.toggle.version_control";

pub fn toggle_panel() -> Invocation {
    super::panel_toggle_invocation(TOGGLE_PANEL)
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    super::register_panel_toggle(
        registry,
        keymap,
        TOGGLE_PANEL,
        "version_control",
        "版本管理",
        "mod-shift-g",
    );
}
