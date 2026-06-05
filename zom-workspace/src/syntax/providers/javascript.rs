//! tree-sitter-javascript Tier 1 provider。
//!
//! 机制全部在 [`super::common`] 里。设计说明详见该模块注释。
//!
//! 注意：tree-sitter-javascript 用单数 `HIGHLIGHT_QUERY`，另外还有
//! `JSX_HIGHLIGHT_QUERY` / `LOCALS_QUERY`——当前只接基础 highlights；JSX
//! / locals 需要 injection / locals 支持后再接入。

use std::sync::{Arc, OnceLock};

use tree_sitter::QueryError;

use crate::syntax::LanguageId;
use crate::syntax::providers::common::{HighlightWorker, SharedConfig, build_shared_config};

pub(crate) fn javascript_config() -> Result<Arc<SharedConfig>, &'static QueryError> {
    static CELL: OnceLock<Result<Arc<SharedConfig>, QueryError>> = OnceLock::new();
    CELL.get_or_init(|| {
        build_shared_config(
            tree_sitter_javascript::LANGUAGE.into(),
            tree_sitter_javascript::HIGHLIGHT_QUERY,
        )
        .map(Arc::new)
    })
    .as_ref()
    .map(|c| c.clone())
}

pub fn new_provider() -> HighlightWorker {
    let config = javascript_config().expect("tree-sitter-javascript 高亮配置必须构建");
    HighlightWorker::new(LanguageId::new("javascript"), config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::providers::common::{
        assert_lookup_matches_capture_names, smoke_test_provider,
    };

    const SAMPLE: &str = "const x = 1;\nfunction f(a) { return a + 1; }\n";

    #[test]
    fn javascript_provider_emits_spans() {
        smoke_test_provider(LanguageId::new("javascript"), SAMPLE, new_provider);
    }

    #[test]
    fn lookup_table_matches_query_capture_names() {
        let cfg = javascript_config().expect("javascript 配置必须构建");
        assert_lookup_matches_capture_names(&cfg);
    }
}
