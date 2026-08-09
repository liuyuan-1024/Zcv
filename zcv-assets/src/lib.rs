//! 内置资源的编译期嵌入与统一读取入口。

use std::borrow::Cow;
use std::str;

use anyhow::{Context as _, Result as AnyhowResult};
use gpui::{App, AssetSource, Result, SharedString};
use rust_embed::RustEmbed;

#[derive(RustEmbed)]
#[folder = "../assets"]
#[include = "fonts/**/*"]
#[include = "icons/**/*"]
#[include = "icon_themes/**/*"]
#[include = "keymaps/**/*"]
#[include = "settings/**/*"]
#[include = "themes/**/*"]
pub struct Assets;

impl AssetSource for Assets {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        Self::get(path)
            .map(|file| Some(file.data))
            .with_context(|| format!("无法加载内置资源 {path:?}"))
    }

    fn list(&self, prefix: &str) -> Result<Vec<SharedString>> {
        Ok(Self::iter()
            .filter(|path| path.starts_with(prefix))
            .map(SharedString::from)
            .collect())
    }
}

impl Assets {
    /// 注册全部内置字体。
    pub fn load_fonts(&self, cx: &App) -> AnyhowResult<()> {
        let fonts = AssetSource::list(self, "fonts/")?
            .into_iter()
            .filter(|path| path.ends_with(".ttf"))
            .map(|path| {
                AssetSource::load(self, &path)?.with_context(|| format!("内置字体 {path:?} 不存在"))
            })
            .collect::<AnyhowResult<Vec<_>>>()?;

        cx.text_system().add_fonts(fonts)
    }
}

/// 读取内置文本资源。
pub fn text(path: &str) -> AnyhowResult<Cow<'static, str>> {
    let data = Assets::get(path)
        .with_context(|| format!("内置资源 {path:?} 不存在"))?
        .data;
    match data {
        Cow::Borrowed(bytes) => str::from_utf8(bytes)
            .map(Cow::Borrowed)
            .with_context(|| format!("内置资源 {path:?} 不是 UTF-8 文本")),
        Cow::Owned(bytes) => String::from_utf8(bytes)
            .map(Cow::Owned)
            .with_context(|| format!("内置资源 {path:?} 不是 UTF-8 文本")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embeds_each_resource_category() {
        for path in [
            "fonts/JetBrainsMono-Regular.ttf",
            "icons/settings.svg",
            "icon_themes/default.json",
            "keymaps/default-linux.json",
            "settings/initial_user_settings.json",
            "themes/onedark.toml",
        ] {
            assert!(Assets::get(path).is_some(), "缺少内置资源 {path}");
        }
    }

    #[test]
    fn reads_text_resources() {
        assert!(text("themes/onedark.toml").unwrap().contains("\"keyword\""));
    }
}
