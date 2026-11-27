use super::*;

#[test]
fn svg_preserves_color_channels_and_transparency() {
    let rasterized = rasterize_svg(
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="8"><rect width="8" height="8" fill="red"/><rect x="8" width="8" height="8" fill="blue"/><rect x="16" width="8" height="8" fill="red" fill-opacity="0.5"/></svg>"#,
        None,
        1.,
    )
    .expect("有效 SVG 应能渲染");
    let image = rasterized.image;
    assert_eq!(image.frame_count(), 1);
    let bytes = image.as_bytes(0).expect("应包含一帧像素");
    assert_eq!(&bytes[..4], &[0, 0, 255, 255]);
    assert_eq!(&bytes[8 * 4..9 * 4], &[255, 0, 0, 255]);
    assert_eq!(&bytes[16 * 4..17 * 4], &[0, 0, 255, 128]);
    assert_eq!(&bytes[24 * 4..25 * 4], &[0, 0, 0, 0]);
}

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
        image.image.size(0).width.0 as u32,
        (16. * expected_scale).ceil() as u32
    );
    assert_eq!(
        image.image.size(0).height.0 as u32,
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
    assert_eq!(image.image.size(0).width.0 as u32, 2048);
    assert_eq!(image.image.size(0).height.0 as u32, 512);
}

#[test]
fn content_scale_changes_the_rasterized_preview_size() {
    let image = rasterize_svg(
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="512" height="256"><rect width="512" height="256"/></svg>"#,
        None,
        2.,
    )
    .expect("有效 SVG 应能渲染");
    assert_eq!(image.image.size(0).width.0 as u32, 1024);
    assert_eq!(image.image.size(0).height.0 as u32, 512);
}

#[test]
fn svg_resolves_images_relative_to_the_source_directory() {
    let directory = tempfile::tempdir().expect("应能创建资源目录");
    image::RgbaImage::from_pixel(1, 1, image::Rgba([0, 255, 0, 255]))
        .save(directory.path().join("pixel.png"))
        .expect("应能写入图片资源");
    let rasterized = rasterize_svg(
        br#"<svg xmlns="http://www.w3.org/2000/svg" width="32" height="32"><image href="pixel.png" width="32" height="32"/></svg>"#,
        Some(directory.path().to_path_buf()),
        1.,
    )
    .expect("引用相对路径图片的 SVG 应能渲染");
    let bytes = rasterized.image.as_bytes(0).expect("应包含一帧像素");
    let center = (16 * 32 + 16) * 4;
    assert_eq!(&bytes[center..center + 4], &[0, 255, 0, 255]);
}

#[test]
fn invalid_svg_reports_a_render_error() {
    assert!(rasterize_svg(b"invalid svg", None, 1.).is_err());
}
