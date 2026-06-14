//! 命令目录（catalog）。
//!
//! `features` / `system` 是 zom-command 的实现细节 —— 整个目录的 install 与 handler 都关在内部。
//! 对外只暴露：
//! - [`install_all`]：组合根一次性安装全套内建命令；
//! - 下面这一组按角色筛过的子模块（typed builders / 命令 id / 必要的 KeyContext 枚举）。
//! 纯 panel 切换类（terminal / debug / outline / version_control / keyboard_shortcuts）
//! 已通过 [`crate::PanelKind`] 暴露，不再单独 re-export。

use crate::{CommandHandler, CommandOutcome, CommandRegistry, HostEffect, Keymap, NoArgs};

pub(crate) mod args;
mod features;
pub(crate) mod reconcile;
mod system;

pub use features::diagnostics;
pub use features::editor;
pub use features::file_tree;
pub use features::go_to_line;
pub use features::language_servers;
pub use features::project_picker;
pub use features::search;
pub use features::settings;
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
