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
