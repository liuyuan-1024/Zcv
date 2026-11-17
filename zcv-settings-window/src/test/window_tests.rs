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

fn open_settings_page(
    cx: &mut gpui::TestAppContext,
    ui_font_size: f32,
    page: SettingsPage,
) -> &mut gpui::VisualTestContext {
    cx.update(|cx| {
        cx.set_global(SettingsStore::new(zcv_settings::UserSettings::default()));
        typography::set_base_typography(cx, None, Some(ui_font_size), None);
    });
    let (_, visual) = cx.add_window_view(move |window, cx| {
        let mut settings = SettingsWindow::new(window, cx);
        settings.set_active_page(page, cx);
        window.focus(&settings.focus_handle(cx), cx);
        settings
    });
    let window = *visual.windows().last().expect("设置窗口应已创建");
    visual.simulate_window_resize(window, gpui::size(px(680.0), px(520.0)));
    visual
}

#[gpui::test]
fn sidebar_width_follows_ui_font_and_preserves_content_space(cx: &mut gpui::TestAppContext) {
    let visual = open_settings_page(cx, 14.0, SettingsPage::Appearance);
    let sidebar_width = |visual: &mut gpui::VisualTestContext| {
        visual.debug_bounds("settings-sidebar").unwrap().size.width
    };
    assert_eq!(sidebar_width(visual), px(160.0));

    visual.update(|_, cx| typography::set_base_typography(cx, None, Some(17.0), None));
    assert!((px(160.0)..px(200.0)).contains(&sidebar_width(visual)));

    visual.update(|_, cx| typography::set_base_typography(cx, None, Some(24.0), None));
    assert_eq!(sidebar_width(visual), px(200.0));
}

#[gpui::test]
fn sidebar_highlights_hover_and_selects_a_page_on_click(cx: &mut gpui::TestAppContext) {
    let visual = open_settings_page(cx, 14.0, SettingsPage::Appearance);
    let selected = visual.debug_bounds("settings-nav-appearance").unwrap();
    let hovered = visual.debug_bounds("settings-nav-terminal").unwrap();
    let paints_highlight = |visual: &mut gpui::VisualTestContext, row: gpui::Bounds<_>| {
        visual.update(|window, cx| {
            let row = row.scale(window.scale_factor());
            let highlight = color::current(cx).element_highlight.into();
            window
                .painted_quads()
                .iter()
                .any(|quad| quad.bounds == row && quad.background == highlight)
        })
    };
    assert!(paints_highlight(visual, selected));
    assert!(!paints_highlight(visual, hovered));

    visual.simulate_mouse_move(hovered.center(), None, Modifiers::default());
    assert!(paints_highlight(visual, hovered));
    assert!(paints_highlight(visual, selected));
    assert!(visual.debug_bounds("settings-row-theme").is_some());

    visual.simulate_mouse_move(
        gpui::point(px(500.0), px(100.0)),
        None,
        Modifiers::default(),
    );
    assert!(!paints_highlight(visual, hovered));

    visual.simulate_click(hovered.center(), Modifiers::default());
    visual.simulate_mouse_move(
        gpui::point(px(500.0), px(100.0)),
        None,
        Modifiers::default(),
    );
    assert!(paints_highlight(visual, hovered));
    assert!(!paints_highlight(visual, selected));
    assert!(
        visual
            .debug_bounds("settings-row-terminal-cursor-shape")
            .is_some()
    );
}

#[gpui::test]
fn dependent_rows_follow_the_applied_setting_modes(cx: &mut gpui::TestAppContext) {
    let visual = open_settings_page(cx, 14.0, SettingsPage::Editor);
    let delay_row = "settings-row-autosave-delay";
    let line_width_row = "settings-row-preferred-line-length";
    assert!(visual.debug_bounds(delay_row).is_none());
    assert!(visual.debug_bounds(line_width_row).is_none());

    visual.update(|_, cx| {
        cx.update_global::<SettingsStore, _>(|store, _| {
            store
                .set_user_settings(
                    r#"{"autosave":{"after_delay":{"milliseconds":1250}},"soft_wrap":"bounded","preferred_line_length":100}"#,
                )
                .expect("应接受有效的设置值");
        });
    });
    assert!(visual.debug_bounds(delay_row).is_some());
    assert!(visual.debug_bounds(line_width_row).is_some());
    for (parent, child) in [
        ("settings-label-autosave", "settings-label-autosave-delay"),
        (
            "settings-label-soft-wrap",
            "settings-label-preferred-line-length",
        ),
    ] {
        assert!(
            visual.debug_bounds(child).unwrap().left()
                > visual.debug_bounds(parent).unwrap().left()
        );
    }

    visual.update(|_, cx| {
        cx.update_global::<SettingsStore, _>(|store, _| {
            store
                .set_user_settings(
                    r#"{"autosave":"on_window_change","soft_wrap":"none","preferred_line_length":100}"#,
                )
                .expect("应接受切换后的设置值");
        });
    });
    assert!(visual.debug_bounds(delay_row).is_none());
    assert!(visual.debug_bounds(line_width_row).is_none());
    assert_eq!(
        visual.update(|_, cx| SettingsStore::get(cx).preferred_line_length),
        100
    );
}

#[gpui::test]
fn editor_form_keeps_controls_visible_at_minimum_width_and_large_font(
    cx: &mut gpui::TestAppContext,
) {
    let visual = open_settings_page(cx, 24.0, SettingsPage::Editor);
    visual.update(|_, cx| {
        cx.update_global::<SettingsStore, _>(|store, _| {
            store
                .set_user_settings(
                    r#"{"autosave":{"after_delay":{"milliseconds":1250}},"soft_wrap":"bounded"}"#,
                )
                .expect("应接受有效的设置值");
        });
    });

    assert_right_aligned_controls(
        visual,
        &[
            ("settings-label-autosave", "settings-control-autosave"),
            (
                "settings-label-autosave-delay",
                "settings-control-autosave-delay",
            ),
            ("settings-label-soft-wrap", "settings-control-soft-wrap"),
            (
                "settings-label-preferred-line-length",
                "settings-control-preferred-line-length",
            ),
            ("settings-label-tab-size", "settings-control-tab-size"),
            (
                "settings-label-insert-spaces",
                "settings-control-insert-spaces",
            ),
            (
                "settings-label-indent-guides",
                "settings-control-indent-guides",
            ),
            (
                "settings-label-indent-guide-width",
                "settings-control-indent-guide-width",
            ),
            (
                "settings-label-cursor-shape",
                "settings-control-cursor-shape",
            ),
            (
                "settings-label-cursor-blink",
                "settings-control-cursor-blink",
            ),
        ],
    );

    visual.simulate_event(gpui::ScrollEvent {
        position: gpui::point(px(420.0), px(320.0)),
        delta: gpui::ScrollDelta::Pixels(gpui::point(px(0.0), px(-160.0))),
        ..Default::default()
    });
    let control = visual
        .debug_bounds("settings-control-soft-wrap")
        .expect("滚动后应能访问软换行控件");
    visual.simulate_click(control.center(), gpui::Modifiers::default());
    let menu = visual
        .debug_bounds("overlay-panel")
        .expect("应能展开换行菜单");
    assert!(menu.right() <= px(680.0));
    assert!(menu.bottom() <= control.top() || menu.top() >= control.bottom());
}

#[gpui::test]
fn appearance_controls_stay_right_aligned_and_theme_opens_a_dropdown(
    cx: &mut gpui::TestAppContext,
) {
    let visual = open_settings_page(cx, 24.0, SettingsPage::Appearance);
    assert_right_aligned_controls(
        visual,
        &[
            ("settings-label-theme", "settings-control-theme"),
            (
                "settings-label-ui-font-size",
                "settings-control-ui-font-size",
            ),
            (
                "settings-label-content-font-size",
                "settings-control-content-font-size",
            ),
            (
                "settings-label-content-line-height",
                "settings-control-content-line-height",
            ),
        ],
    );

    let initial_focus = visual.update(|window, cx| window.focused(cx));
    visual.simulate_keystrokes("tab");
    let first_control = visual.update(|window, cx| window.focused(cx));
    assert_ne!(first_control, initial_focus, "Tab 应聚焦第一个控件");

    visual.simulate_keystrokes("tab");
    assert_ne!(
        visual.update(|window, cx| window.focused(cx)),
        first_control,
        "再次按 Tab 应聚焦下一个控件",
    );
    visual.simulate_keystrokes("shift-tab");
    assert_eq!(
        visual.update(|window, cx| window.focused(cx)),
        first_control,
        "Shift+Tab 应返回上一个控件",
    );

    visual.simulate_keystrokes("enter");
    visual.simulate_event(gpui::KeyUpEvent {
        keystroke: gpui::Keystroke::parse("enter").expect("Enter 应能解析"),
    });
    assert!(
        visual.debug_bounds("overlay-panel").is_some(),
        "按 Enter 应展开聚焦的主题菜单"
    );
    assert!(
        visual.debug_bounds("context-menu-item-2").is_some(),
        "主题菜单应包含三种选项"
    );
}

#[gpui::test]
fn terminal_controls_stay_compact_and_right_aligned(cx: &mut gpui::TestAppContext) {
    let visual = open_settings_page(cx, 24.0, SettingsPage::Terminal);
    assert_right_aligned_controls(
        visual,
        &[
            (
                "settings-label-terminal-font-size",
                "settings-control-terminal-font-size",
            ),
            (
                "settings-label-terminal-line-height",
                "settings-control-terminal-line-height",
            ),
            (
                "settings-label-terminal-cursor-shape",
                "settings-control-terminal-cursor-shape",
            ),
        ],
    );
}

fn assert_right_aligned_controls(
    visual: &mut gpui::VisualTestContext,
    selectors: &[(&'static str, &'static str)],
) {
    let mut right_edge = None;
    for &(label_selector, control_selector) in selectors {
        let label = visual.debug_bounds(label_selector).expect("设置标签应显示");
        let control = visual
            .debug_bounds(control_selector)
            .expect("设置控件应显示");
        assert!(
            label.right() < control.left(),
            "{control_selector} 的标签与控件应保持分栏：标签 {label:?}，控件 {control:?}",
        );
        assert!(
            control.top() < label.bottom() && label.top() < control.bottom(),
            "{control_selector} 的标签和控件应在同一行",
        );
        assert!(
            control.right() < px(680.0),
            "{control_selector} 的控件应保持在窗口内"
        );
        let edge = right_edge.get_or_insert(control.right());
        assert_eq!(control.right(), *edge, "控件应沿右侧对齐");
    }
}
