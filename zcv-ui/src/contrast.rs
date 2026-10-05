//! 编辑器高亮背景上的文字对比度计算。

use gpui::Hsla;

/// 与 Zed 一致的 APCA 0.0.98G-4g W3 参数。
struct ApcaConstants {
    main_trc: f32,
    s_rco: f32,
    s_gco: f32,
    s_bco: f32,
    norm_bg: f32,
    norm_txt: f32,
    rev_txt: f32,
    rev_bg: f32,
    blk_thrs: f32,
    blk_clmp: f32,
    scale_bow: f32,
    scale_wob: f32,
    lo_bow_offset: f32,
    lo_wob_offset: f32,
    delta_y_min: f32,
    lo_clip: f32,
}

impl Default for ApcaConstants {
    fn default() -> Self {
        Self {
            main_trc: 2.4,
            s_rco: 0.2126729,
            s_gco: 0.7151522,
            s_bco: 0.0721750,
            norm_bg: 0.56,
            norm_txt: 0.57,
            rev_txt: 0.62,
            rev_bg: 0.65,
            blk_thrs: 0.022,
            blk_clmp: 1.414,
            scale_bow: 1.14,
            scale_wob: 1.14,
            lo_bow_offset: 0.027,
            lo_wob_offset: 0.027,
            delta_y_min: 0.0005,
            lo_clip: 0.1,
        }
    }
}

fn srgb_to_y(color: Hsla, constants: &ApcaConstants) -> f32 {
    let rgb = color.to_rgb();
    constants.s_rco * rgb.r.powf(constants.main_trc)
        + constants.s_gco * rgb.g.powf(constants.main_trc)
        + constants.s_bco * rgb.b.powf(constants.main_trc)
}

/// 返回 APCA 亮度对比度；正值为浅底深字，负值为深底浅字。
pub fn apca_contrast(foreground: Hsla, background: Hsla) -> f32 {
    let constants = ApcaConstants::default();
    let text_y = srgb_to_y(foreground, &constants);
    let background_y = srgb_to_y(background, &constants);
    let text_y = if text_y > constants.blk_thrs {
        text_y
    } else {
        text_y + (constants.blk_thrs - text_y).powf(constants.blk_clmp)
    };
    let background_y = if background_y > constants.blk_thrs {
        background_y
    } else {
        background_y + (constants.blk_thrs - background_y).powf(constants.blk_clmp)
    };
    if (background_y - text_y).abs() < constants.delta_y_min {
        return 0.0;
    }
    let contrast = if background_y > text_y {
        let value = (background_y.powf(constants.norm_bg) - text_y.powf(constants.norm_txt))
            * constants.scale_bow;
        if value < constants.lo_clip {
            0.0
        } else {
            value - constants.lo_bow_offset
        }
    } else {
        let value = (background_y.powf(constants.rev_bg) - text_y.powf(constants.rev_txt))
            * constants.scale_wob;
        if value > -constants.lo_clip {
            0.0
        } else {
            value + constants.lo_wob_offset
        }
    };
    contrast * 100.0
}

fn adjust_lightness(foreground: Hsla, background: Hsla, minimum: f32) -> Hsla {
    let darker = srgb_to_y(background, &ApcaConstants::default()) > 0.5;
    let mut low = if darker { 0.0 } else { foreground.l };
    let mut high = if darker { foreground.l } else { 1.0 };
    let mut best = foreground.l;
    for _ in 0..20 {
        let middle = (low + high) / 2.0;
        let candidate = Hsla {
            l: middle,
            ..foreground
        };
        let contrast = apca_contrast(candidate, background).abs();
        if contrast >= minimum {
            best = middle;
            if darker {
                low = middle;
            } else {
                high = middle;
            }
        } else if darker {
            high = middle;
        } else {
            low = middle;
        }
        if (contrast - minimum).abs() < 1.0 {
            best = middle;
            break;
        }
    }
    Hsla {
        l: best,
        ..foreground
    }
}

/// 达到最低 APCA 对比度时保留原色；否则优先调整明度，最后才降低饱和度。
pub fn ensure_minimum_contrast(foreground: Hsla, background: Hsla, minimum: f32) -> Hsla {
    if minimum <= 0.0 || apca_contrast(foreground, background).abs() >= minimum {
        return foreground;
    }
    let adjusted = adjust_lightness(foreground, background, minimum);
    if apca_contrast(adjusted, background).abs() >= minimum {
        return adjusted;
    }
    for saturation in [1.0, 0.8, 0.6, 0.4, 0.2, 0.0] {
        let candidate = adjust_lightness(
            Hsla {
                s: foreground.s * saturation,
                ..foreground
            },
            background,
            minimum,
        );
        if apca_contrast(candidate, background).abs() >= minimum {
            return candidate;
        }
    }
    let black = Hsla {
        h: 0.0,
        s: 0.0,
        l: 0.0,
        a: foreground.a,
    };
    let white = Hsla { l: 1.0, ..black };
    if apca_contrast(white, background).abs() > apca_contrast(black, background).abs() {
        white
    } else {
        black
    }
}

#[cfg(test)]
#[path = "test/contrast_tests.rs"]
mod tests;
