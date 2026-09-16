use std::path::Path;

use gpui::{App, AppContext};
use zcv_workspace::{
    ItemHandle, PreviewDocument, PreviewMode, PreviewPresentation, PreviewProvider,
};

use crate::view::SvgPreviewView;

pub(crate) struct SvgPreviewProvider;

impl PreviewProvider for SvgPreviewProvider {
    fn supports(&self, path: &Path, _cx: &App) -> bool {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
    }

    fn mode(&self) -> PreviewMode {
        PreviewMode::Source
    }

    fn presentation(&self) -> PreviewPresentation {
        PreviewPresentation::Canvas
    }

    fn create(&self, document: PreviewDocument, cx: &mut App) -> Box<dyn ItemHandle> {
        let view = cx.new(|cx| SvgPreviewView::new(document, cx));
        Box::new(view)
    }
}
