//! 独立代码片段的同步语法高亮入口。
//!
//! 适合 Markdown 预览等不拥有可编辑 Buffer 的只读消费者；
//! 调用方应在后台执行，避免长代码片段阻塞 UI 线程。

use std::sync::Arc;

use zcv_text::{Buffer, BufferConfig};

use crate::HighlightSpan;
use crate::registry::language_for_name_or_extension;
use crate::syntax_map::SyntaxMap;
use crate::tree_sitter_utils::ParseCancellation;

/// 一段代码片段的高亮 capture 区间和对应 capture 名称表。
///
/// 样式由渲染层根据当前主题解析，因而主题切换时无需重新解析代码。
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SnippetHighlights {
    pub spans: Vec<HighlightSpan>,
    pub capture_names: Arc<[Arc<str>]>,
}

/// 使用已注册的 Tree-sitter 语言高亮一段代码。
///
/// `language` 可使用语言名、文件扩展名或注入别名，例如 `Rust`、`rs`、`typescript`、`ts`、`golang`。
/// 未知语言或不含语法树的语言返回 `None`。
pub fn highlight_snippet(language: &str, source: &str) -> Option<SnippetHighlights> {
    let language = language.split_whitespace().next()?;
    let language = language_for_name_or_extension(language)?;
    language.grammar()?;

    let buffer = Buffer::scratch(source.to_owned(), BufferConfig::default()).ok()?;
    let text = buffer.snapshot();
    let mut syntax = SyntaxMap::new(&text);
    syntax.set_language(Some(language), &text);
    let syntax = syntax
        .snapshot()
        .reparse(&text, None, &ParseCancellation::default())?;
    Some(SnippetHighlights {
        spans: syntax.highlights(0..text.len_bytes().get(), &text),
        capture_names: syntax.capture_names(),
    })
}

#[cfg(test)]
mod tests {
    use super::highlight_snippet;

    #[test]
    fn highlights_rust_with_the_registered_language() {
        let highlights = highlight_snippet("rust", "fn main() { let count = 1; }")
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
        assert!(highlight_snippet("ts", "const value: number = 1;").is_some());
        assert!(highlight_snippet("golang", "package main").is_some());
    }

    #[test]
    fn recognizes_the_fence_languages_used_by_the_markdown_fixture() {
        for language in [
            "rust",
            "python",
            "javascript",
            "typescript",
            "json",
            "toml",
            "yaml",
            "bash",
            "sql",
            "css",
            "html",
            "c",
            "cpp",
            "go",
            "java",
            "kotlin",
            "ruby",
            "swift",
            "lua",
        ] {
            assert!(
                highlight_snippet(language, "value").is_some(),
                "围栏语言 {language} 应由内置语言注册表识别"
            );
        }
    }

    #[test]
    fn leaves_unknown_languages_unhighlighted() {
        assert!(highlight_snippet("not-a-language", "plain text").is_none());
    }
}
