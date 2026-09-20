use super::*;

#[test]
fn embeds_each_resource_category() {
    for path in [
        "fonts/JetBrainsMono-Regular.ttf",
        "fonts/JetBrainsMono-Italic.ttf",
        "fonts/SarasaMonoSC-Italic.ttf",
        "icons/settings.svg",
        "icon_themes/default.json",
        "keymaps/default-linux.json",
        "settings/initial_user_settings.json",
        "themes/dark.toml",
    ] {
        assert!(Assets::get(path).is_some(), "缺少内置资源 {path}");
    }
}

#[test]
fn reads_text_resources() {
    assert!(text("themes/dark.toml").unwrap().contains("\"keyword\""));
}
