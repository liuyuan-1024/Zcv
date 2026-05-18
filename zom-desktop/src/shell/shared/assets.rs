//! `EmbeddedAssets` —— GPUI `AssetSource` 的 embedded 实现（手册 22.2 / 22.3）。
//!
//! 第一版把全部 svg / 主题文件编译进二进制；用户主题除外（5.2，骨架阶段
//! 尚未接入）。路径不带 `assets/` 前缀（手册 22.4）。

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

pub(crate) struct EmbeddedAssets;

macro_rules! asset {
    ($path:literal) => {
        (
            $path,
            include_bytes!(concat!("../../../assets/", $path)).as_slice(),
        )
    };
}

const ASSETS: &[(&str, &[u8])] = &[
    asset!("icons/top_bar/settings.svg"),
    asset!("icons/top_bar/window_controls/close.svg"),
    asset!("icons/top_bar/window_controls/minimize.svg"),
    asset!("icons/top_bar/window_controls/maximize.svg"),
    asset!("icons/bottom_bar/file_tree.svg"),
    asset!("icons/bottom_bar/version_control.svg"),
    asset!("icons/bottom_bar/outline.svg"),
    asset!("icons/bottom_bar/project_search.svg"),
    asset!("icons/bottom_bar/terminal.svg"),
    asset!("icons/bottom_bar/debug.svg"),
    asset!("icons/bottom_bar/keyboard_shortcuts.svg"),
    asset!("icons/bottom_bar/diagnostics.svg"),
    asset!("icons/bottom_bar/language_server.svg"),
    asset!("icons/primitives/chevron_right.svg"),
    asset!("icons/primitives/chevron_left.svg"),
    asset!("icons/primitives/chevron_up.svg"),
    asset!("icons/primitives/chevron_down.svg"),
    asset!("icons/primitives/chevron_down_up.svg"),
    asset!("icons/primitives/check.svg"),
    asset!("icons/primitives/signal_low.svg"),
    asset!("icons/primitives/signal_medium.svg"),
    asset!("icons/primitives/signal_high.svg"),
];

impl AssetSource for EmbeddedAssets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Ok(ASSETS
            .iter()
            .find(|(p, _)| *p == path)
            .map(|(_, bytes)| Cow::Borrowed(*bytes)))
    }

    fn list(&self, prefix: &str) -> Result<Vec<SharedString>> {
        Ok(ASSETS
            .iter()
            .filter_map(|(p, _)| p.strip_prefix(prefix).map(|_| SharedString::from(*p)))
            .collect())
    }
}
