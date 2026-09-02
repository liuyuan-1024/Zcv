use std::path::Path;

use gpui::{App, AppContext};
use zcv_workspace::{ItemHandle, PreviewDocument, PreviewProvider};

use crate::view::MarkdownPreviewView;

pub(crate) struct MarkdownPreviewProvider;

impl PreviewProvider for MarkdownPreviewProvider {
    fn supports(&self, path: &Path, _cx: &App) -> bool {
        zcv_language::language_for_file(path, None)
            .is_some_and(|language| language.name() == "Markdown")
    }

    fn create(&self, document: PreviewDocument, cx: &mut App) -> Box<dyn ItemHandle> {
        Box::new(cx.new(|cx| MarkdownPreviewView::new(document, cx)))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use gpui::TestAppContext;
    use zcv_workspace::PreviewProvider;

    use super::MarkdownPreviewProvider;

    #[gpui::test]
    fn supports_paths_recognized_as_markdown(cx: &mut TestAppContext) {
        let provider = MarkdownPreviewProvider;
        cx.update(|cx| {
            assert!(provider.supports(Path::new("README.md"), cx));
            assert!(provider.supports(Path::new("notes.markdown"), cx));
            assert!(!provider.supports(Path::new("notes.txt"), cx));
        });
    }
}
