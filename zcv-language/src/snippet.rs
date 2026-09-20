//! 独立代码片段的同步语法高亮入口。
//!
//! 适合 Markdown 预览等不拥有可编辑 Buffer 的只读消费者；
//! 调用方应在后台执行，避免长代码片段阻塞 UI 线程。

use std::sync::Arc;

use zcv_text::{Buffer, BufferConfig};

use crate::HighlightSpan;
use crate::registry::LanguageRegistry;
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

/// 可取消的一次代码片段高亮。
///
/// Markdown 预览在源文档更新后取消过期任务，避免后台继续完成已无消费方的大代码块解析与查询。
#[derive(Clone, Debug, Default)]
pub struct SnippetHighlightCancellation(ParseCancellation);

impl SnippetHighlightCancellation {
    pub fn cancel(&self) {
        self.0.cancel();
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.is_cancelled()
    }
}

/// 使用给定语言注册表高亮一段代码，并允许调用方取消过期计算。
///
/// `language` 可使用语言名、文件扩展名或注入别名，例如 `Rust`、`rs`、`typescript`、`ts`、`golang`。
/// 未知语言或不含语法树的语言返回 `None`。
pub fn highlight_snippet_with_cancellation(
    registry: &Arc<LanguageRegistry>,
    language: &str,
    source: &str,
    cancellation: &SnippetHighlightCancellation,
) -> Option<SnippetHighlights> {
    if cancellation.is_cancelled() {
        return None;
    }
    let language = language.split_whitespace().next()?;
    let language = registry.language_for_name_or_extension(language)?;
    language.grammar()?;

    let buffer = Buffer::from_text(source.to_owned(), BufferConfig::default()).ok()?;
    let text = buffer.snapshot();
    let mut syntax = SyntaxMap::new(Arc::clone(registry), &text);
    syntax.set_language(Some(language), &text);
    let syntax = syntax
        .snapshot()
        .reparse(&text, registry, &cancellation.0)?;
    Some(SnippetHighlights {
        spans: syntax.highlights_with_cancellation(
            0..text.len_bytes().get(),
            &text,
            &cancellation.0,
        )?,
        capture_names: syntax.capture_names(),
    })
}

#[cfg(test)]
#[path = "test/snippet_tests.rs"]
mod tests;
