//! tree-sitter-yaml Tier 1 provider（`tree-sitter-grammars/tree-sitter-yaml`，
//! 不是已停更的同名包；这一份是 amaanq 维护的活跃版本）。
//!
//! 机制全部在 [`super::common`] 里。设计说明详见该模块注释。

use std::sync::{Arc, OnceLock};

use tree_sitter::QueryError;

use crate::syntax::LanguageId;
use crate::syntax::providers::common::{HighlightWorker, SharedConfig, build_shared_config};

pub(crate) fn yaml_config() -> Result<Arc<SharedConfig>, &'static QueryError> {
    static CELL: OnceLock<Result<Arc<SharedConfig>, QueryError>> = OnceLock::new();
    CELL.get_or_init(|| {
        build_shared_config(
            tree_sitter_yaml::LANGUAGE.into(),
            tree_sitter_yaml::HIGHLIGHTS_QUERY,
        )
        .map(Arc::new)
    })
    .as_ref()
    .map(|c| c.clone())
}

pub fn new_provider() -> HighlightWorker {
    let config = yaml_config().expect("tree-sitter-yaml 高亮配置必须构建");
    HighlightWorker::new(LanguageId::new("yaml"), config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::providers::common::test_support::{
        assert_lookup_matches_capture_names, smoke_test_provider,
    };

    const SAMPLE: &str = "name: zom\nversion: 1\nfeatures:\n  - alpha\n  - beta\n";

    #[test]
    fn yaml_provider_emits_spans() {
        smoke_test_provider(LanguageId::new("yaml"), SAMPLE, new_provider);
    }

    #[test]
    fn lookup_table_matches_query_capture_names() {
        let cfg = yaml_config().expect("yaml 配置必须构建");
        assert_lookup_matches_capture_names(&cfg);
    }
}
