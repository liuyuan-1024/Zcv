use super::*;

#[test]
fn stepped_values_stay_on_the_advertised_precision() {
    assert_eq!(stepped(1.5, 0.1, 1.0, 3.0), 1.6);
    assert_eq!(stepped(1.0, -0.1, 1.0, 3.0), 1.0);
    assert_eq!(stepped(32.0, 1.0, 10.0, 32.0), 32.0);
}

#[test]
fn indent_guide_updates_preserve_other_explicit_fields() {
    let mut content = UserSettingsContent {
        indent_guides: Some(IndentGuideSettingsContent {
            enabled: Some(true),
            line_width: std::num::NonZeroU32::new(3),
        }),
        ..UserSettingsContent::default()
    };

    update_indent_guides(&mut content, None, Some(5));
    assert_eq!(
        content.indent_guides,
        Some(IndentGuideSettingsContent {
            enabled: Some(true),
            line_width: std::num::NonZeroU32::new(5),
        })
    );

    update_indent_guides(&mut content, Some(false), None);
    assert_eq!(
        content.indent_guides,
        Some(IndentGuideSettingsContent {
            enabled: Some(false),
            line_width: std::num::NonZeroU32::new(5),
        })
    );
}

#[test]
fn override_count_counts_top_level_explicit_fields() {
    let content = UserSettingsContent {
        theme: Some(ThemeChoice::Named("dark")),
        content_font_size: Some(18.0),
        indent_guides: Some(IndentGuideSettingsContent::default()),
        ..UserSettingsContent::default()
    };
    assert_eq!(explicit_override_count(&content), 3);
    assert_eq!(explicit_override_count(&UserSettingsContent::default()), 0);
}

#[test]
fn every_registered_theme_has_a_stable_window_label() {
    let labels: Vec<_> = ThemeChoice::available().map(theme_label).collect();
    assert_eq!(labels, ["跟随系统", "深色", "浅色"]);
}
