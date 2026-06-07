//! 非产品 feature 的系统级命令。
//!
//! 这些命令仍属于内建命令集，但它们描述窗口和浮面这类 shell 能力，不归入某个用户功能目录。

use crate::{CommandRegistry, Keymap};

pub mod dismiss;
pub mod window;

pub fn install_all(registry: &mut CommandRegistry, keymap: &mut Keymap) {
    dismiss::install(registry, keymap);
    window::install(registry, keymap);
}
