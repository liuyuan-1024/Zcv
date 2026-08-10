use std::path::Path;

use gpui::{App, AppContext};
use zcv_preview::{PreviewDescriptor, PreviewDocument, PreviewProvider, PreviewProviderId};
use zcv_workspace::ItemHandle;

use crate::view::SvgPreviewView;

pub(crate) struct SvgPreviewProvider;

impl PreviewProvider for SvgPreviewProvider {
    fn descriptor(&self) -> PreviewDescriptor {
        PreviewDescriptor {
            id: PreviewProviderId("svg"),
            display_name: "SVG",
        }
    }

    fn supports(&self, path: &Path, _cx: &App) -> bool {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
    }

    fn create(&self, document: PreviewDocument, cx: &mut App) -> Box<dyn ItemHandle> {
        let view = cx.new(|cx| SvgPreviewView::new(document, cx));
        Box::new(view)
    }
}
