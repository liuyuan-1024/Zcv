//! 命令目录（catalog）。
//!
//! 具体命令按产品功能放在 [`features`] 里；
//! 本模块保留总安装入口和稳定重导出，避免组合根逐个知道 feature 文件。

use crate::{CommandHandler, CommandOutcome, CommandRegistry, HostEffect, Keymap, NoArgs};

pub(crate) mod args;
pub mod features;
pub(crate) mod reconcile;
pub mod system;

pub use features::debug;
pub use features::diagnostics;
pub use features::editor;
pub use features::file_tree;
pub use features::keyboard_shortcuts;
pub use features::language_servers;
pub use features::outline;
pub use features::project_picker;
pub use features::search;
pub use features::settings;
pub use features::terminal;
pub use features::version_control;
pub use system::window;

/// 安装全部内建命令。
pub fn install_all(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    features::install_all(registry, keymap);
    system::install_all(registry, keymap);
}

/// 把"无参命令 → 推一个固定 [`HostEffect`]"打包成 handler。
///
/// 内建 catalog 大量命令是同一形态 —— 翻译 keymap 到一个意图。
/// 这里集中样板，避免每个 feature 各写一份 `run_xxx` 函数或 inline 闭包。
/// 有参命令仍由 feature 自己写 handler（要走 typed args 解析）。
pub(super) fn emit(effect: HostEffect) -> CommandHandler {
    Box::new(move |ctx, args| {
        NoArgs::try_from(args)?;
        ctx.effects.push(effect.clone());
        Ok(CommandOutcome::default())
    })
}
