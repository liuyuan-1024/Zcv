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
