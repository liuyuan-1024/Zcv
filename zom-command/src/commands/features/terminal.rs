//! 终端 feature 命令。

use crate::{CommandRegistry, Keymap, PanelKind};

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    super::register_panel_toggle(
        registry,
        keymap,
        PanelKind::Terminal,
        "终端",
        "打开或关闭终端面板。",
        "mod j",
    );
}
