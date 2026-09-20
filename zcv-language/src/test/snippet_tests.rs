use std::sync::Arc;

use super::{LanguageRegistry, SnippetHighlightCancellation, highlight_snippet_with_cancellation};

fn test_registry() -> Arc<LanguageRegistry> {
    Arc::new(LanguageRegistry::new())
}

#[test]
fn highlights_rust_with_the_registered_language() {
    let highlights = highlight_snippet_with_cancellation(
        &test_registry(),
        "rust",
        "fn main() { let count = 1; }",
        &SnippetHighlightCancellation::default(),
    )
    .expect("rust 围栏语言应被识别");
    assert!(!highlights.spans.is_empty());
    assert!(
        highlights
            .spans
            .iter()
            .any(|span| highlights.capture_names[span.capture as usize].starts_with("keyword"))
    );
}

#[test]
fn accepts_extensions_and_injection_aliases() {
    let cancellation = SnippetHighlightCancellation::default();
    assert!(
        highlight_snippet_with_cancellation(
            &test_registry(),
            "ts",
            "const value: number = 1;",
            &cancellation
        )
        .is_some()
    );
    assert!(
        highlight_snippet_with_cancellation(
            &test_registry(),
            "golang",
            "package main",
            &cancellation
        )
        .is_some()
    );
}

#[test]
fn leaves_unknown_languages_unhighlighted() {
    assert!(
        highlight_snippet_with_cancellation(
            &test_registry(),
            "not-a-language",
            "plain text",
            &SnippetHighlightCancellation::default()
        )
        .is_none()
    );
}

#[test]
fn cancelled_highlight_returns_without_parsing() {
    let cancellation = SnippetHighlightCancellation::default();
    cancellation.cancel();
    assert!(
        highlight_snippet_with_cancellation(
            &test_registry(),
            "rust",
            "fn main() {}",
            &cancellation
        )
        .is_none()
    );
}
