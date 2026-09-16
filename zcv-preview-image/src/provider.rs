use std::path::Path;

use gpui::{App, AppContext};
use zcv_workspace::{
    ItemHandle, PreviewDocument, PreviewMode, PreviewPresentation, PreviewProvider,
};

use crate::view::ImagePreviewView;

pub(crate) struct ImagePreviewProvider;

impl PreviewProvider for ImagePreviewProvider {
    fn supports(&self, path: &Path, _cx: &App) -> bool {
        image_format_for_path(path).is_some()
    }

    fn mode(&self) -> PreviewMode {
        PreviewMode::Standalone
    }

    fn presentation(&self) -> PreviewPresentation {
        PreviewPresentation::Canvas
    }

    fn create(&self, document: PreviewDocument, cx: &mut App) -> Box<dyn ItemHandle> {
        let PreviewDocument::Standalone { path } = document else {
            panic!("图片预览必须从独立文件创建")
        };
        let view = cx.new(|cx| ImagePreviewView::new(path, cx));
        Box::new(view)
    }
}

pub(crate) fn image_format_for_path(path: &Path) -> Option<gpui::ImageFormat> {
    let extension = path.extension()?.to_str()?.to_ascii_lowercase();
    let format = match extension.as_str() {
        "png" => gpui::ImageFormat::Png,
        "jpg" | "jpeg" => gpui::ImageFormat::Jpeg,
        "webp" => gpui::ImageFormat::Webp,
        "gif" => gpui::ImageFormat::Gif,
        "bmp" => gpui::ImageFormat::Bmp,
        "tif" | "tiff" => gpui::ImageFormat::Tiff,
        "ico" => gpui::ImageFormat::Ico,
        "pbm" | "pgm" | "ppm" | "pnm" => gpui::ImageFormat::Pnm,
        _ => return None,
    };
    Some(format)
}

#[cfg(test)]
mod tests {
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
}
