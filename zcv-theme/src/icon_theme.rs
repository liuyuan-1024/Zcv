use std::collections::BTreeMap;
use std::path::Path;
use std::sync::LazyLock;

use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub struct IconTheme {
    #[serde(default)]
    file_stems: BTreeMap<String, String>,
    #[serde(default)]
    file_suffixes: BTreeMap<String, String>,
    #[serde(default)]
    file_icons: BTreeMap<String, IconDefinition>,
    directory_icons: DirectoryIcons,
    #[serde(default)]
    named_directory_icons: BTreeMap<String, DirectoryIcons>,
}

#[derive(Debug, Deserialize)]
struct IconDefinition {
    path: String,
}

#[derive(Debug, Default, Deserialize)]
struct DirectoryIcons {
    collapsed: Option<String>,
    expanded: Option<String>,
}

impl IconTheme {
    pub fn file_type_for(&self, key: &str) -> Option<&str> {
        self.file_stems
            .get(key)
            .or_else(|| self.file_suffixes.get(key))
            .map(String::as_str)
    }

    pub fn icon_for_type(&self, typ: &str) -> Option<&str> {
        self.file_icons.get(typ).map(|icon| icon.path.as_str())
    }

    pub fn folder_icon(&self, expanded: bool) -> Option<&str> {
        directory_icon(&self.directory_icons, expanded)
    }

    pub fn named_folder_icon(&self, name: &str, expanded: bool) -> Option<&str> {
        self.named_directory_icons
            .get(name)
            .and_then(|icons| directory_icon(icons, expanded))
    }
}

/// 根据路径从当前图标主题解析文件与目录图标。
pub struct FileIcons;

impl FileIcons {
    /// 按完整文件名和逐级后缀匹配图标，未知类型回退到主题默认图标。
    pub fn get_icon(path: &Path) -> String {
        let theme = default_icon_theme();
        let lookup = |key: &str| icon_for_key(theme, key);

        if let Some(mut name) = path.file_name().and_then(|name| name.to_str()) {
            if let Some(icon) = lookup(name) {
                return icon;
            }

            while let Some((_, suffix)) = name.split_once('.') {
                if let Some(icon) = lookup(suffix) {
                    return icon;
                }
                name = suffix;
            }
        }

        if let Some(extension) = path.extension().and_then(|extension| extension.to_str())
            && let Some(icon) = lookup(extension)
        {
            return icon;
        }

        theme
            .icon_for_type("default")
            .unwrap_or("icons/file_icons/file.svg")
            .to_string()
    }

    pub fn get_folder_icon(expanded: bool, path: &Path) -> String {
        path.file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| default_icon_theme().named_folder_icon(name, expanded))
            .or_else(|| default_icon_theme().folder_icon(expanded))
            .unwrap_or(if expanded {
                "icons/file_icons/folder_open.svg"
            } else {
                "icons/file_icons/folder.svg"
            })
            .to_string()
    }
}

fn icon_for_key(theme: &IconTheme, key: &str) -> Option<String> {
    let typ = theme
        .file_type_for(key)
        .or_else(|| theme.file_type_for(&key.to_ascii_lowercase()))?;
    theme.icon_for_type(typ).map(str::to_string)
}

fn directory_icon(icons: &DirectoryIcons, expanded: bool) -> Option<&str> {
    if expanded {
        icons.expanded.as_deref()
    } else {
        icons.collapsed.as_deref()
    }
}

static DEFAULT_ICON_THEME: LazyLock<IconTheme> = LazyLock::new(|| {
    let source = zcv_assets::text("icon_themes/default.json").expect("内置默认图标主题应存在");
    serde_json::from_str(&source).expect("内置默认图标主题应合法")
});

pub fn default_icon_theme() -> &'static IconTheme {
    &DEFAULT_ICON_THEME
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_theme_has_required_fallbacks() {
        let theme = default_icon_theme();
        assert_eq!(
            theme.icon_for_type("default"),
            Some("icons/file_icons/file.svg")
        );
        assert_eq!(
            theme.folder_icon(false),
            Some("icons/file_icons/folder.svg")
        );
    }

    #[test]
    fn resolves_common_file_types() {
        for (path, icon) in [
            ("src/main.rs", "icons/file_icons/rust.svg"),
            ("app.test.tsx", "icons/file_icons/react.svg"),
            ("Cargo.toml", "icons/file_icons/rust.svg"),
            (".gitignore", "icons/file_icons/git.svg"),
            ("README.md", "icons/file_icons/book.svg"),
            ("unknown.xyzzy", "icons/file_icons/file.svg"),
        ] {
            assert_eq!(FileIcons::get_icon(Path::new(path)), icon);
        }
    }

    #[test]
    fn resolves_folder_state() {
        assert_eq!(
            FileIcons::get_folder_icon(false, Path::new("src")),
            "icons/file_icons/folder.svg"
        );
        assert_eq!(
            FileIcons::get_folder_icon(true, Path::new("src")),
            "icons/file_icons/folder_open.svg"
        );
    }
}
