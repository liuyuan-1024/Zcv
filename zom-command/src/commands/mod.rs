//! 命令目录（catalog）。
//!
//! 具体命令按产品功能放在 [`features`] 里；
//! 本模块保留总安装入口和稳定重导出，避免组合根逐个知道 feature 文件。

use crate::{CommandRegistry, Keymap};

pub(crate) mod args;
pub mod features;
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
