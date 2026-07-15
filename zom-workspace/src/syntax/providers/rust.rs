//! tree-sitter-rust 内建 provider。
//!
//! 机制全部在 [`super::common`] 里——本文件只通过 [`declare_builtin_provider!`] 声明语言常量。
//! 设计说明详见该模块注释。

use crate::declare_builtin_provider;

declare_builtin_provider!(
    rust_config,
    new_provider,
    "rust",
    tree_sitter_rust::LANGUAGE,
    tree_sitter_rust::HIGHLIGHTS_QUERY
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::provider::HighlightProvider;
    use crate::syntax::providers::common::test_support::assert_lookup_matches_capture_names;
    use crate::syntax::{BufferSyntax, SyntaxQueryCursor};
    use zom_engine::{Buffer, BufferConfig, ByteOffset, TextRange};

    #[test]
    fn rust_provider_highlights_keyword_and_string() {
        // Rust 语义稳定，可以断言到具体 capture name——保留这条强断言作为 grammar
        // 路径自洽的金标准。其他语言只跑烟雾测。
        let buffer = Buffer::from_text(
            "fn main() { let s = \"hi\"; }".to_string(),
            BufferConfig::default(),
        )
        .unwrap();
        let provider: Box<dyn HighlightProvider> = Box::new(new_provider());
        let worker = std::sync::Arc::new(crate::syntax::SyntaxWorkerHandle::spawn());
        let syntax = BufferSyntax::attach(
            crate::BufferId::from_raw(1),
            crate::syntax::LanguageId::new("rust"),
            provider,
            &buffer,
            worker.clone(),
        );
        worker.wait_for_idle_for_test_or_bench();
        let tree = syntax
            .highlights_slot()
            .load()
            .expect("attach 完成后 slot 必须有 tree");
        let viewport = TextRange::new(ByteOffset::ZERO, buffer.snapshot().len_bytes()).unwrap();
        let mut cursor = SyntaxQueryCursor::new();
        let spans = tree.query_viewport(viewport, &mut cursor);
        assert!(!spans.is_empty(), "rust provider 应产出 spans");
        let names: std::collections::HashSet<&'static str> =
            spans.iter().map(|(_, s)| s.name.as_str()).collect();
        assert!(
            names.contains("keyword"),
            "应包含 'keyword'，实际为 {:?}",
            names
        );
        assert!(
            names.contains("string"),
            "应包含 'string'，实际为 {:?}",
            names
        );
        assert!(
            names.contains("function"),
            "应包含 'function'，实际为 {:?}",
            names
        );
    }

    #[test]
    fn lookup_table_matches_query_capture_names() {
        let cfg = rust_config().expect("rust 配置必须构建");
        assert_lookup_matches_capture_names(&cfg);
    }
}
