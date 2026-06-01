//! tree-sitter-json Tier 1 provider。
//!
//! 机制全部在 [`super::common`] 里。设计说明详见该模块注释。

use std::sync::{Arc, OnceLock};

use tree_sitter::QueryError;

use crate::syntax::LanguageId;
use crate::syntax::providers::common::{HighlightWorker, SharedConfig, build_shared_config};

fn json_config() -> Result<Arc<SharedConfig>, &'static QueryError> {
    static CELL: OnceLock<Result<Arc<SharedConfig>, QueryError>> = OnceLock::new();
    CELL.get_or_init(|| {
        build_shared_config(
            tree_sitter_json::LANGUAGE.into(),
            tree_sitter_json::HIGHLIGHTS_QUERY,
        )
        .map(Arc::new)
    })
    .as_ref()
    .map(|c| c.clone())
}

pub fn new_provider() -> HighlightWorker {
    let config = json_config().expect("tree-sitter-json 高亮配置必须构建");
    HighlightWorker::new(LanguageId::new("json"), config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::providers::common::{
        assert_lookup_matches_capture_names, smoke_test_provider,
    };

    const SAMPLE: &str = "{\"name\": \"zom\", \"version\": 1, \"keywords\": [\"a\", \"b\"]}";

    #[test]
    fn json_provider_emits_spans() {
        smoke_test_provider(LanguageId::new("json"), SAMPLE, new_provider);
    }

    #[test]
    fn lookup_table_matches_query_capture_names() {
        let cfg = json_config().expect("json 配置必须构建");
        assert_lookup_matches_capture_names(&cfg);
    }
}
