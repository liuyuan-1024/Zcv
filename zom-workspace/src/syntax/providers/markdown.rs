//! tree-sitter-md Tier 1 provider。
//!
//! 机制全部在 [`super::common`] 里。设计说明详见该模块注释。
//!
//! ## 当前只接 block 语法
//!
//! tree-sitter-md 把 Markdown 拆成 `LANGUAGE`（block 结构：标题 / 列表 / 代码块 /
//! 引用块）与 `INLINE_LANGUAGE`（行内 emphasis / link / inline code）两套
//! grammar，配套两份 query。当前只用 block，行内高亮要 injection 才能
//! 完整覆盖——而 injection / combined parsers 是手册 §十四 明列的非目标。
//! 行内的强调 / 链接现阶段走 theme 默认前景色，等 injection 接入再补。

use std::sync::{Arc, OnceLock};

use tree_sitter::QueryError;

use crate::syntax::LanguageId;
use crate::syntax::providers::common::{HighlightWorker, SharedConfig, build_shared_config};

fn markdown_config() -> Result<Arc<SharedConfig>, &'static QueryError> {
    static CELL: OnceLock<Result<Arc<SharedConfig>, QueryError>> = OnceLock::new();
    CELL.get_or_init(|| {
        build_shared_config(
            tree_sitter_md::LANGUAGE.into(),
            tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
        )
        .map(Arc::new)
    })
    .as_ref()
    .map(|c| c.clone())
}

pub fn new_provider() -> HighlightWorker {
    let config = markdown_config().expect("tree-sitter-md 高亮配置必须构建");
    HighlightWorker::new(LanguageId::new("markdown"), config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::payload::syntax_layer_kind;
    use crate::syntax::providers::common::{
        assert_lookup_matches_capture_names, smoke_test_provider,
    };
    use crate::syntax::{BufferSyntaxState, HighlightProvider, HighlightSpan};
    use zom_engine::{Buffer, BufferConfig, MetadataLayers};

    const SAMPLE: &str = "# heading\n\n- item\n\n```rust\nfn x() {}\n```\n";

    #[test]
    fn markdown_provider_emits_spans() {
        smoke_test_provider(LanguageId::new("markdown"), SAMPLE, new_provider);
    }

    #[test]
    fn lookup_table_matches_query_capture_names() {
        let cfg = markdown_config().expect("markdown 配置必须构建");
        assert_lookup_matches_capture_names(&cfg);
    }

    #[test]
    fn markdown_heading_uses_canonical_markup_name() {
        let buffer = Buffer::from_text("# heading\n".to_string(), BufferConfig::default()).unwrap();
        let mut layers = MetadataLayers::<HighlightSpan>::new();
        let provider: Box<dyn HighlightProvider> = Box::new(new_provider());

        let worker = std::sync::Arc::new(crate::syntax::SyntaxWorkerHandle::spawn());
        let state = BufferSyntaxState::attach(
            crate::BufferId::from_raw(1),
            LanguageId::new("markdown"),
            provider,
            &buffer,
            &mut layers,
            worker.clone(),
            None,
        );
        worker.wait_for_idle();
        state.drain_into_layers(buffer.version(), &mut layers);
        let names = layers
            .layer(&syntax_layer_kind())
            .expect("syntax layer 必须存在")
            .as_slice()
            .iter()
            .map(|range| range.metadata().name.as_str())
            .collect::<Vec<_>>();

        assert!(
            names.contains(&"markup.heading"),
            "markdown 标题应归一化为 markup.heading，实际为 {names:?}"
        );
    }
}
