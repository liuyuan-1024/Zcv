//! 快捷键 feature 命令。

use crate::{CommandRegistry, Invocation, Keymap};

pub const TOGGLE_PANEL: &str = "panel.toggle.keyboard_shortcuts";

pub fn toggle_panel() -> Invocation {
    super::panel_toggle_invocation(TOGGLE_PANEL)
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    super::register_panel_toggle(
        registry,
        keymap,
        TOGGLE_PANEL,
        "keyboard_shortcuts",
        "快捷键",
        "打开或关闭快捷键面板。",
        "mod-shift-k",
    );
}
