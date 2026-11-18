use std::path::Path;

use gpui::{App, AppContext};
use zcv_project::image_format_for_path;
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
        let PreviewDocument::Standalone { path, project } = document else {
            panic!("图片预览必须从独立文件创建");
        };
        let view = cx.new(|cx| ImagePreviewView::new(path, project, cx));
        Box::new(view)
    }
}

#[cfg(test)]
#[path = "test/provider_tests.rs"]
mod tests;
