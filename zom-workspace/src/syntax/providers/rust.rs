//! tree-sitter-rust Tier 1 provider。
//!
//! 机制全部在 [`super::common`] 里——本文件只声明语言常量与 OnceLock 配置入口。
//! 设计说明详见 [`super::common`] 模块注释。

use std::sync::{Arc, OnceLock};

use tree_sitter::QueryError;

use crate::syntax::LanguageId;
use crate::syntax::providers::common::{HighlightWorker, SharedConfig, build_shared_config};

pub(crate) fn rust_config() -> Result<Arc<SharedConfig>, &'static QueryError> {
    static CELL: OnceLock<Result<Arc<SharedConfig>, QueryError>> = OnceLock::new();
    CELL.get_or_init(|| {
        build_shared_config(
            tree_sitter_rust::LANGUAGE.into(),
            tree_sitter_rust::HIGHLIGHTS_QUERY,
        )
        .map(Arc::new)
    })
    .as_ref()
    .map(|c| c.clone())
}

/// 构造一个 Rust provider。
///
/// 失败仅发生在 config 首次 build——query 语法错误或 ABI 不匹配——属静态资源问题，发版前必须被测试测到，此处 expect 即视为发版前快速失败信号。
pub fn new_provider() -> HighlightWorker {
    let config = rust_config().expect("tree-sitter-rust 高亮配置必须构建");
    HighlightWorker::new(LanguageId::new("rust"), config)
}

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
            LanguageId::new("rust"),
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
