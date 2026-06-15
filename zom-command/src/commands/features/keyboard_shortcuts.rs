//! 快捷键 feature 命令。

use crate::{CommandRegistry, Keymap, PanelKind};

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    super::register_panel_toggle(
        registry,
        keymap,
        PanelKind::KeyboardShortcuts,
        "快捷键",
        "打开或关闭快捷键面板。",
        "mod shift k",
    );
}
