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
#[path = "test/provider_tests.rs"]
mod tests;
