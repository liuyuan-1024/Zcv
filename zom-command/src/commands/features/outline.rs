//! 大纲 feature 命令。

use crate::{CommandRegistry, Invocation, Keymap};

pub const TOGGLE_PANEL: &str = "panel.toggle.outline";

pub fn toggle_panel() -> Invocation {
    super::panel_toggle_invocation(TOGGLE_PANEL)
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    super::register_panel_toggle(
        registry,
        keymap,
        TOGGLE_PANEL,
        "outline",
        "大纲",
        "mod-shift-o",
    );
}
