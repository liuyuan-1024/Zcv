//! 按产品功能组织的内建命令 catalog。
//!
//! 每个 feature 模块同处声明命令 id、typed builder、handler 与默认键位。

use crate::{CommandRegistry, HostEffect, Keymap, PanelKind};

pub mod debug;
pub mod diagnostics;
pub mod editor;
pub mod file_tree;
pub mod keyboard_shortcuts;
pub mod language_servers;
pub mod outline;
pub mod project_picker;
pub mod search;
pub mod settings;
pub mod terminal;
pub mod version_control;

/// 安装全部内建命令。
///
/// 组合根只需要选择"是否安装内建命令集"；具体 feature 的注册顺序和完整性由 zom-command 自己维护。
pub fn install_all(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    editor::install(registry, keymap);
    project_picker::install(registry, keymap);
    settings::install(registry, keymap);
    file_tree::install(registry, keymap);
    version_control::install(registry, keymap);
    outline::install(registry, keymap);
    search::install(registry, keymap);
    terminal::install(registry, keymap);
    debug::install(registry, keymap);
    keyboard_shortcuts::install(registry, keymap);
    language_servers::install(registry, keymap);
    diagnostics::install(registry, keymap);
}

/// 注册"切换某 panel"的内建命令。命令 id 直接取自 [`PanelKind::toggle_command_id`]，
/// 保证 emit 与查询用的 id 强绑定 —— 不再手写两端字符串。
pub(super) fn register_panel_toggle(
    registry: &mut CommandRegistry,
    keymap: &mut Keymap,
    panel: PanelKind,
    title: &'static str,
    description: &'static str,
    default_chord: &'static str,
) {
    registry
        .install(
            keymap,
            panel.toggle_command_id(),
            title,
            super::emit(HostEffect::TogglePanel(panel)),
        )
        .description(description)
        .key(default_chord);
}
