//! 调试 feature 命令。

use crate::{CommandRegistry, Keymap, PanelKind};

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    super::register_panel_toggle(
        registry,
        keymap,
        PanelKind::Debug,
        "调试",
        "打开或关闭调试面板。",
        "mod shift d",
    );
}
