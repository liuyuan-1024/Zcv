//! 调试 feature 命令。

use crate::{CommandRegistry, Invocation, Keymap};

pub const TOGGLE_PANEL: &str = "panel.toggle.debug";

pub fn toggle_panel() -> Invocation {
    super::panel_toggle_invocation(TOGGLE_PANEL)
}

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    super::register_panel_toggle(
        registry,
        keymap,
        TOGGLE_PANEL,
        "debug",
        "调试",
        "打开或关闭调试面板。",
        "mod-shift-d",
    );
}
