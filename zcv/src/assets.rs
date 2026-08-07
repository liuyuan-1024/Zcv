//! `EmbeddedAssets` —— GPUI `AssetSource` 的 embedded 实现。
//!
//! 编译时将全部 SVG 嵌入二进制。

use std::borrow::Cow;

use gpui::{AssetSource, Result, SharedString};

pub(crate) struct EmbeddedAssets;

macro_rules! asset {
    ($path:literal) => {
        (
            $path,
            include_bytes!(concat!("../assets/", $path)).as_slice(),
        )
    };
}

const ASSETS: &[(&str, &[u8])] = &[
    asset!("icons/preview.svg"),
    asset!("icons/settings.svg"),
    asset!("icons/close.svg"),
    asset!("icons/circle.svg"),
    asset!("icons/check.svg"),
    asset!("icons/case_sensitive.svg"),
    asset!("icons/regex.svg"),
    asset!("icons/replace_next.svg"),
    asset!("icons/replace_all.svg"),
    asset!("icons/whole_word.svg"),
    asset!("icons/copy.svg"),
    asset!("icons/dash.svg"),
    asset!("icons/square_plus.svg"),
    asset!("icons/square_minus.svg"),
    asset!("icons/trash.svg"),
    asset!("icons/generic_close.svg"),
    asset!("icons/minimize.svg"),
    asset!("icons/maximize.svg"),
    asset!("icons/project_tree.svg"),
    asset!("icons/version_control.svg"),
    asset!("icons/chevron_right.svg"),
    asset!("icons/chevron_down.svg"),
    asset!("icons/outline.svg"),
    asset!("icons/search.svg"),
    asset!("icons/terminal.svg"),
    asset!("icons/debug.svg"),
    asset!("icons/keyboard_shortcuts.svg"),
    asset!("icons/diagnostics.svg"),
    asset!("icons/diff.svg"),
    asset!("icons/language_server.svg"),
    asset!("icons/chevron_right.svg"),
    asset!("icons/chevron_left.svg"),
    asset!("icons/chevron_up.svg"),
    asset!("icons/chevron_down.svg"),
    asset!("icons/chevron_down_up.svg"),
    asset!("icons/signal_low.svg"),
    asset!("icons/signal_medium.svg"),
    asset!("icons/signal_high.svg"),
    asset!("icons/folder.svg"),
    asset!("icons/folder_open.svg"),
    asset!("icons/file.svg"),
    asset!("icons/arrow_circle.svg"),
    asset!("icons/arrow_down.svg"),
    asset!("icons/arrow_up.svg"),
    asset!("icons/undo.svg"),
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

/// 内嵌字体（字体不通过 AssetSource 加载，GPUI 要求直接传字节）。
pub(crate) fn embedded_fonts() -> Vec<Cow<'static, [u8]>> {
    vec![
        Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-Regular.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-SemiBold.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../assets/fonts/JetBrainsMono-Italic.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../assets/fonts/SarasaMonoSC-Regular.ttf").as_slice()),
        Cow::Borrowed(include_bytes!("../assets/fonts/SarasaMonoSC-Bold.ttf").as_slice()),
    ]
}
