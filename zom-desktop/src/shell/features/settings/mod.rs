//! 设置功能。
//!
//! 当前只提供入口：功能图标归本模块所有，命令标题归
//! `zom_command::commands::settings`。设置 UI 接入时在本目录补
//! `view.rs` / `state.rs` 等文件。

use gpui::AnyElement;
use zom_command::commands::settings;

use crate::shell::shared::Glyph;
use crate::shell::{CommandTitleLookup, ShortcutLookup};

const INVOKER_ID: &str = "top-bar.settings";
const COMMAND: &str = settings::OPEN;

pub(crate) fn entry(shortcuts: &ShortcutLookup, titles: &CommandTitleLookup) -> AnyElement {
    let title = titles(COMMAND).unwrap_or_else(|| COMMAND.to_string());
    Glyph::icon(INVOKER_ID, "icons/actions/settings.svg", title)
        .hint(shortcuts(COMMAND))
        .render()
}
