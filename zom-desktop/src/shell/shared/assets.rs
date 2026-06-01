//! `EmbeddedAssets` —— GPUI `AssetSource` 的 embedded 实现（手册 22.2 / 22.3）。
//!
//! 当前把全部 svg / 主题文件编译进二进制；用户主题另走设置与主题加载路径。
//! 路径不带 `assets/` 前缀（手册 22.4）。

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
    asset!("icons/actions/settings.svg"),
    asset!("icons/actions/close.svg"),
    asset!("icons/actions/check.svg"),
    asset!("icons/actions/case_sensitive.svg"),
    asset!("icons/actions/regex.svg"),
    asset!("icons/actions/replace_next.svg"),
    asset!("icons/actions/replace_all.svg"),
    asset!("icons/actions/whole_word.svg"),
    asset!("icons/window/close.svg"),
    asset!("icons/window/minimize.svg"),
    asset!("icons/window/maximize.svg"),
    asset!("icons/panels/file_tree.svg"),
    asset!("icons/panels/version_control.svg"),
    asset!("icons/panels/outline.svg"),
    asset!("icons/panels/search.svg"),
    asset!("icons/panels/terminal.svg"),
    asset!("icons/panels/debug.svg"),
    asset!("icons/panels/keyboard_shortcuts.svg"),
    asset!("icons/status/diagnostics.svg"),
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
];

macro_rules! font_asset {
    ($path:literal) => {
        Cow::Borrowed(include_bytes!(concat!("../../../assets/fonts/", $path)).as_slice())
    };
}

pub(crate) fn embedded_fonts() -> Vec<Cow<'static, [u8]>> {
    vec![
        font_asset!("JetBrainsMono-Regular.ttf"),
        font_asset!("JetBrainsMono-SemiBold.ttf"),
        font_asset!("SarasaMonoSC-Regular.ttf"),
        font_asset!("SarasaMonoSC-Bold.ttf"),
    ]
}

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

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;
    use std::path::Path;

    use super::*;

    #[test]
    fn shell_icon_paths_are_registered_as_embedded_assets() {
        let shell_dir = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/shell");
        let mut referenced = BTreeSet::new();

        collect_icon_paths(&shell_dir, &mut referenced);

        let missing = referenced
            .into_iter()
            .filter(|path| !ASSETS.iter().any(|(asset_path, _)| asset_path == path))
            .collect::<Vec<_>>();

        assert!(
            missing.is_empty(),
            "shell 引用的图标路径必须存在于 EmbeddedAssets：{missing:#?}"
        );
    }

    fn collect_icon_paths(dir: &Path, out: &mut BTreeSet<String>) {
        let entries = fs::read_dir(dir)
            .unwrap_or_else(|error| panic!("读取目录失败：{}：{error}", dir.display()));

        for entry in entries {
            let path = entry
                .unwrap_or_else(|error| panic!("读取目录项失败：{}：{error}", dir.display()))
                .path();

            if path == platform_dir() {
                continue;
            }

            if path.is_dir() {
                collect_icon_paths(&path, out);
            } else if path.extension().and_then(|ext| ext.to_str()) == Some("rs")
                && path != current_file()
            {
                collect_icon_paths_from_file(&path, out);
            }
        }
    }

    fn current_file() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/shell/shared/assets.rs")
    }

    fn platform_dir() -> std::path::PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("src/shell/platform")
    }

    fn collect_icon_paths_from_file(path: &Path, out: &mut BTreeSet<String>) {
        let source = fs::read_to_string(path)
            .unwrap_or_else(|error| panic!("读取文件失败：{}：{error}", path.display()));

        for literal in source.split('"').skip(1).step_by(2) {
            if literal.starts_with("icons/") && literal.ends_with(".svg") {
                out.insert(literal.to_string());
            }
        }
    }
}
