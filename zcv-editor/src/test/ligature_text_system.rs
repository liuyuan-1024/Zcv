use std::borrow::Cow;

use gpui::{
    Bounds, DevicePixels, Font, FontId, FontMetrics, FontRun, GlyphId, LineLayout, NoopTextSystem,
    Pixels, PlatformTextSystem, RenderGlyphParams, Size, TextRenderingMode,
};

/// 将 `<=` 塑形成一个覆盖两个字符的字形，其余行为使用确定性的测试字体。
pub(super) struct LigatureTextSystem;

macro_rules! delegate_noop {
    ($($name:ident($($argument:ident: $ty:ty),*) -> $result:ty;)+) => {
        $(fn $name(&self, $($argument: $ty),*) -> $result {
            NoopTextSystem.$name($($argument),*)
        })+
    };
}

impl PlatformTextSystem for LigatureTextSystem {
    delegate_noop! {
        add_fonts(fonts: Vec<Cow<'static, [u8]>>) -> gpui::Result<()>;
        all_font_names() -> Vec<String>;
        font_id(descriptor: &Font) -> gpui::Result<FontId>;
        font_metrics(font_id: FontId) -> FontMetrics;
        typographic_bounds(font_id: FontId, glyph_id: GlyphId) -> gpui::Result<Bounds<f32>>;
        advance(font_id: FontId, glyph_id: GlyphId) -> gpui::Result<Size<f32>>;
        glyph_for_char(font_id: FontId, ch: char) -> Option<GlyphId>;
        glyph_raster_bounds(params: &RenderGlyphParams) -> gpui::Result<Bounds<DevicePixels>>;
        rasterize_glyph(params: &RenderGlyphParams, raster_bounds: Bounds<DevicePixels>) -> gpui::Result<(Size<DevicePixels>, Vec<u8>)>;
        recommended_rendering_mode(font_id: FontId, font_size: Pixels) -> TextRenderingMode;
    }

    fn layout_line(&self, text: &str, font_size: Pixels, runs: &[FontRun]) -> LineLayout {
        let mut line = NoopTextSystem.layout_line(text, font_size, runs);
        for run in &mut line.runs {
            run.glyphs.retain(|glyph| {
                !text
                    .match_indices("<=")
                    .any(|(start, _)| glyph.index == start + 1)
            });
        }
        line
    }
}
