use super::*;

#[test]
fn adequate_contrast_keeps_the_original_text_color() {
    let foreground = Hsla::black();
    let background = Hsla::white();
    assert!(apca_contrast(foreground, background).abs() >= 45.0);
    assert_eq!(
        ensure_minimum_contrast(foreground, background, 45.0),
        foreground
    );
}

#[test]
fn low_contrast_adjusts_text_and_zero_disables_adjustment() {
    let foreground = Hsla {
        h: 0.58,
        s: 0.4,
        l: 0.4,
        a: 1.0,
    };
    let background = Hsla {
        h: 0.58,
        s: 0.4,
        l: 0.42,
        a: 1.0,
    };
    assert_eq!(
        ensure_minimum_contrast(foreground, background, 0.0),
        foreground
    );
    let adjusted = ensure_minimum_contrast(foreground, background, 45.0);
    assert_ne!(adjusted, foreground);
    assert!(apca_contrast(adjusted, background).abs() >= 45.0);
    assert_eq!(adjusted.h, foreground.h);
}
