use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

const SVG_PREVIEW_MAX_RASTER_EDGE: f32 = 2048.0;
pub(crate) const SVG_PREVIEW_MIN_DISPLAY_EDGE: f32 = 32.0;

pub(crate) struct RasterizedSvg {
    pub(crate) png: Vec<u8>,
    pub(crate) scale: f32,
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
    let png = pixmap.encode_png().map_err(|error| error.to_string())?;
    Ok(RasterizedSvg { png, scale })
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
mod tests {
    use super::*;

    #[test]
    fn small_svg_uses_a_readable_raster_resolution() {
        let image = rasterize_svg(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="8"><rect width="16" height="8"/></svg>"#,
            None,
            1.,
        )
        .expect("有效 SVG 应能渲染");
        let expected_scale = (SVG_PREVIEW_MIN_DISPLAY_EDGE / 16.).max(1.);
        assert_eq!(
            u32::from_be_bytes(image.png[16..20].try_into().unwrap()),
            (16. * expected_scale).ceil() as u32
        );
        assert_eq!(
            u32::from_be_bytes(image.png[20..24].try_into().unwrap()),
            (8. * expected_scale).ceil() as u32
        );
    }

    #[test]
    fn oversized_svg_is_limited_to_the_maximum_preview_edge() {
        let image = rasterize_svg(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="4096" height="1024"><rect width="4096" height="1024"/></svg>"#,
            None,
            1.,
        )
        .expect("有效 SVG 应能渲染");
        assert_eq!(
            u32::from_be_bytes(image.png[16..20].try_into().unwrap()),
            2048
        );
        assert_eq!(
            u32::from_be_bytes(image.png[20..24].try_into().unwrap()),
            512
        );
    }

    #[test]
    fn content_scale_changes_the_rasterized_preview_size() {
        let image = rasterize_svg(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="512" height="256"><rect width="512" height="256"/></svg>"#,
            None,
            2.,
        )
        .expect("有效 SVG 应能渲染");
        assert_eq!(
            u32::from_be_bytes(image.png[16..20].try_into().unwrap()),
            1024
        );
        assert_eq!(
            u32::from_be_bytes(image.png[20..24].try_into().unwrap()),
            512
        );
    }
}
