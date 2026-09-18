use super::*;

fn key(font_size: Pixels, cell_width: Pixels, scale_factor: f32) -> ShapedRunKey {
    let font = typography::content_font();
    ShapedRunKey {
        text: "a".to_owned(),
        styles: vec![RunStyle {
            len: 1,
            font,
            color: 0,
            background_color: None,
            underline: None,
            strikethrough: None,
        }],
        font_size,
        cell_width,
        scale_factor_bits: scale_factor.to_bits(),
    }
}

#[test]
fn layout_changes_invalidate_shaped_run_keys() {
    let base = key(px(14.), px(8.), 1.);
    assert_ne!(base, key(px(15.), px(8.), 1.));
    assert_ne!(base, key(px(14.), px(9.), 1.));
    assert_ne!(base, key(px(14.), px(8.), 1.25));

    let mut changed_font = base.clone();
    changed_font.styles[0].font = gpui::font(".SystemUIFont");
    assert_ne!(base, changed_font);
}
