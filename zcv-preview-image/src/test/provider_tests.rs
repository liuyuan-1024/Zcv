use std::path::Path;

use gpui::TestAppContext;

use super::*;

#[gpui::test]
fn supports_common_raster_image_extensions(cx: &mut TestAppContext) {
    let provider = ImagePreviewProvider;
    cx.read(|cx| {
        for path in [
            "icon.png",
            "photo.JPG",
            "animation.gif",
            "asset.webp",
            "bitmap.bmp",
            "scan.tiff",
            "favicon.ico",
            "image.ppm",
        ] {
            assert!(provider.supports(Path::new(path), cx), "应支持 {path}");
        }
        assert!(!provider.supports(Path::new("diagram.svg"), cx));
        assert!(!provider.supports(Path::new("notes.md"), cx));
    });
}

#[test]
fn maps_jpeg_aliases_to_the_same_format() {
    assert_eq!(
        image_format_for_path(Path::new("photo.jpg")),
        Some(gpui::ImageFormat::Jpeg)
    );
    assert_eq!(
        image_format_for_path(Path::new("photo.jpeg")),
        Some(gpui::ImageFormat::Jpeg)
    );
}

#[test]
fn is_a_standalone_canvas_preview() {
    assert_eq!(ImagePreviewProvider.mode(), PreviewMode::Standalone);
    assert_eq!(
        ImagePreviewProvider.presentation(),
        PreviewPresentation::Canvas
    );
}
