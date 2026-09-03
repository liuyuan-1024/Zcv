use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

const SVG_PREVIEW_MAX_RASTER_EDGE: f32 = 2048.0;

pub(crate) struct RasterizedSvg {
    pub(crate) png: Vec<u8>,
}

pub(crate) fn rasterize_svg(
    bytes: &[u8],
    resources_dir: Option<PathBuf>,
) -> Result<RasterizedSvg, String> {
    let options = resvg::usvg::Options {
        resources_dir,
        fontdb: system_font_database(),
        ..Default::default()
    };
    let tree = resvg::usvg::Tree::from_data(bytes, &options).map_err(|error| error.to_string())?;
    let svg_size = tree.size();
    let longest_edge = svg_size.width().max(svg_size.height());
    let scale = (SVG_PREVIEW_MAX_RASTER_EDGE / longest_edge).min(1.0);
    let width = (svg_size.width() * scale).ceil().max(1.0) as u32;
    let height = (svg_size.height() * scale).ceil().max(1.0) as u32;
    let mut pixmap = resvg::tiny_skia::Pixmap::new(width, height)
        .ok_or_else(|| "SVG 预览尺寸无效".to_string())?;
    let transform = resvg::tiny_skia::Transform::from_scale(scale, scale);
    resvg::render(&tree, transform, &mut pixmap.as_mut());
    let png = pixmap.encode_png().map_err(|error| error.to_string())?;
    Ok(RasterizedSvg { png })
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
    fn small_svg_preserves_its_intrinsic_resolution() {
        let image = rasterize_svg(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="8"><rect width="16" height="8"/></svg>"#,
            None,
        )
        .expect("有效 SVG 应能渲染");
        assert_eq!(
            u32::from_be_bytes(image.png[16..20].try_into().unwrap()),
            16
        );
        assert_eq!(u32::from_be_bytes(image.png[20..24].try_into().unwrap()), 8);
    }

    #[test]
    fn oversized_svg_is_limited_to_the_maximum_preview_edge() {
        let image = rasterize_svg(
            br#"<svg xmlns="http://www.w3.org/2000/svg" width="4096" height="1024"><rect width="4096" height="1024"/></svg>"#,
            None,
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
}
