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
