use std::{
    path::PathBuf,
    sync::{Arc, OnceLock},
};

use gpui::RenderImage;
use image::{Frame, RgbaImage};

const SVG_PREVIEW_MAX_RASTER_EDGE: f32 = 2048.0;
pub(crate) const SVG_PREVIEW_MIN_DISPLAY_EDGE: f32 = 32.0;

pub(crate) struct RasterizedSvg {
    pub(crate) image: Arc<RenderImage>,
    pub(crate) raster_scale: f32,
}

pub(crate) fn rasterize_svg(
    bytes: &[u8],
    resources_dir: Option<PathBuf>,
    content_scale: f32,
) -> Result<RasterizedSvg, String> {
    let options = resvg::usvg::Options {
        resources_dir,
        fontdb: system_font_database(),
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_data(bytes, &options).map_err(|error| error.to_string())?;
    let svg_size = tree.size();
    let longest_edge = svg_size.width().max(svg_size.height());
    let minimum_size_scale = (SVG_PREVIEW_MIN_DISPLAY_EDGE / longest_edge).max(1.);
    let max_raster_edge = SVG_PREVIEW_MAX_RASTER_EDGE * content_scale;
    let scale = (content_scale * minimum_size_scale).min(max_raster_edge / longest_edge);
    let width = (svg_size.width() * scale).ceil().max(1.0) as u32;
    let height = (svg_size.height() * scale).ceil().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| "SVG 预览尺寸无效".to_string())?;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    // 沿用 tiny-skia PNG 输出的去预乘舍入规则，直接生成 GPUI 消费的非预乘 BGRA。
    let pixels = pixmap
        .pixels()
        .iter()
        .flat_map(|pixel| {
            let color = pixel.demultiply();
            [color.blue(), color.green(), color.red(), color.alpha()]
        })
        .collect();
    let buffer =
        RgbaImage::from_raw(width, height, pixels).expect("Pixmap 的像素长度必须与光栅尺寸一致");
    Ok(RasterizedSvg {
        image: Arc::new(RenderImage::new(vec![Frame::new(buffer)])),
        raster_scale: scale,
    })
}

fn system_font_database() -> Arc<resvg::usvg::fontdb::Database> {
    static FONT_DATABASE: OnceLock<Arc<resvg::usvg::fontdb::Database>> = OnceLock::new();
    FONT_DATABASE
        .get_or_init(|| {
            let mut database = resvg::usvg::fontdb::Database::new();
            database.load_system_fonts();
            Arc::new(database)
        })
        .clone()
}

#[cfg(test)]
#[path = "test/renderer_tests.rs"]
mod tests;
