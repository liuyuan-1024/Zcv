//! 按产品功能组织的内建命令 catalog。
//!
//! 每个 feature 模块同处声明命令 id、typed builder、handler 与默认键位。

use crate::{
    CommandArgs, CommandId, CommandOutcome, CommandRegistry, HostEffect, Invocation, Keymap, NoArgs,
};

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
/// 组合根只需要选择"是否安装内建命令集"；具体 feature 的注册顺序和完整性
/// 由 zom-command 自己维护。
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

pub(super) fn panel_toggle_invocation(command_id: &'static str) -> Invocation {
    (cid(command_id), CommandArgs::new())
}

pub(super) fn register_panel_toggle(
    registry: &mut CommandRegistry,
    keymap: &mut Keymap,
    command_id: &'static str,
    panel_str_id: &'static str,
    title: &'static str,
    description: &'static str,
    default_chord: &'static str,
) {
    let panel = panel_str_id.to_string();
    registry
        .install(
            keymap,
            command_id,
            title,
            Box::new(move |ctx, args| {
                NoArgs::try_from(args)?;
                ctx.effects.push(HostEffect::TogglePanel(panel.clone()));
                Ok(CommandOutcome::default())
            }),
        )
        .description(description)
        .key(default_chord);
}

fn cid(id: &'static str) -> CommandId {
    CommandId::new(id).expect("内建命令 ID 必须非空")
}
