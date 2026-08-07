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
    asset!("icons/actions/preview.svg"),
    asset!("icons/actions/settings.svg"),
    asset!("icons/actions/close.svg"),
    asset!("icons/actions/circle.svg"),
    asset!("icons/actions/check.svg"),
    asset!("icons/actions/case_sensitive.svg"),
    asset!("icons/actions/regex.svg"),
    asset!("icons/actions/replace_next.svg"),
    asset!("icons/actions/replace_all.svg"),
    asset!("icons/actions/whole_word.svg"),
    asset!("icons/actions/copy.svg"),
    asset!("icons/actions/dash.svg"),
    asset!("icons/actions/square_plus.svg"),
    asset!("icons/actions/square_minus.svg"),
    asset!("icons/actions/trash.svg"),
    asset!("icons/window/close.svg"),
    asset!("icons/window/minimize.svg"),
    asset!("icons/window/maximize.svg"),
    asset!("icons/panels/project_tree.svg"),
    asset!("icons/panels/version_control.svg"),
    asset!("icons/editor/chevron_right.svg"),
    asset!("icons/editor/chevron_down.svg"),
    asset!("icons/panels/outline.svg"),
    asset!("icons/panels/search.svg"),
    asset!("icons/panels/terminal.svg"),
    asset!("icons/panels/debug.svg"),
    asset!("icons/panels/keyboard_shortcuts.svg"),
    asset!("icons/status/diagnostics.svg"),
    asset!("icons/status/diff.svg"),
    asset!("icons/status/language_server.svg"),
    asset!("icons/navigation/chevron_right.svg"),
    asset!("icons/navigation/chevron_left.svg"),
    asset!("icons/navigation/chevron_up.svg"),
    asset!("icons/navigation/chevron_down.svg"),
    asset!("icons/navigation/chevron_down_up.svg"),
    asset!("icons/status/signal_low.svg"),
    asset!("icons/status/signal_medium.svg"),
    asset!("icons/status/signal_high.svg"),
    asset!("icons/files/folder.svg"),
    asset!("icons/files/folder_open.svg"),
    asset!("icons/files/file.svg"),
    asset!("icons/actions/arrow_circle.svg"),
    asset!("icons/actions/arrow_down.svg"),
    asset!("icons/actions/arrow_up.svg"),
    asset!("icons/actions/undo.svg"),
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
