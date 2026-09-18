use std::path::Path;
use std::sync::Arc;

use gpui::{App, AppContext};
use zcv_language::LanguageRegistry;
use zcv_workspace::{
    ItemHandle, PreviewDocument, PreviewMode, PreviewPresentation, PreviewProvider,
};

use crate::view::MarkdownPreviewView;

pub(crate) struct MarkdownPreviewProvider {
    pub(crate) language_registry: Arc<LanguageRegistry>,
}

impl PreviewProvider for MarkdownPreviewProvider {
    fn supports(&self, path: &Path, _cx: &App) -> bool {
        self.language_registry
            .language_for_file(path, None)
            .is_some_and(|language| language.name() == "Markdown")
    }

    fn mode(&self) -> PreviewMode {
        PreviewMode::Source
    }

    fn presentation(&self) -> PreviewPresentation {
        PreviewPresentation::Flow
    }

    fn create(&self, document: PreviewDocument, cx: &mut App) -> Box<dyn ItemHandle> {
        Box::new(cx.new(|cx| MarkdownPreviewView::new(document, cx)))
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;
    use std::sync::Arc;

    use gpui::TestAppContext;
    use zcv_language::LanguageRegistry;
    use zcv_workspace::PreviewProvider;

    use super::MarkdownPreviewProvider;

    #[gpui::test]
    fn supports_paths_recognized_as_markdown(cx: &mut TestAppContext) {
        let provider = MarkdownPreviewProvider {
            language_registry: Arc::new(LanguageRegistry::new()),
        };
        cx.update(|cx| {
            assert!(provider.supports(Path::new("README.md"), cx));
            assert!(provider.supports(Path::new("notes.markdown"), cx));
            assert!(!provider.supports(Path::new("notes.txt"), cx));
        });
    }
}
