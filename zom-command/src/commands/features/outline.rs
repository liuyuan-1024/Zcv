//! 大纲 feature 命令。

use crate::{CommandRegistry, Keymap, PanelKind};

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    super::register_panel_toggle(
        registry,
        keymap,
        PanelKind::Outline,
        "大纲",
        "打开或关闭当前文件的大纲面板。",
        "mod shift o",
    );
}
