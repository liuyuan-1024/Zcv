//! 版本管理 feature 命令。

use crate::{CommandRegistry, Keymap, PanelKind};

pub fn install(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    super::register_panel_toggle(
        registry,
        keymap,
        PanelKind::VersionControl,
        "版本管理",
        "打开或关闭版本管理面板。",
        "mod shift g",
    );
}
