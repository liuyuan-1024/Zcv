//! tree-sitter-toml-ng Tier 1 provider。
//!
//! 机制全部在 [`super::common`] 里——本文件只声明语言常量与 OnceLock 配置
//! 入口。设计说明详见 [`super::common`] 模块注释。

use std::sync::{Arc, OnceLock};

use tree_sitter::QueryError;

use crate::syntax::LanguageId;
use crate::syntax::providers::common::{HighlightWorker, SharedConfig, build_shared_config};

pub(crate) fn toml_config() -> Result<Arc<SharedConfig>, &'static QueryError> {
    static CELL: OnceLock<Result<Arc<SharedConfig>, QueryError>> = OnceLock::new();
    CELL.get_or_init(|| {
        build_shared_config(
            tree_sitter_toml_ng::LANGUAGE.into(),
            tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
        )
        .map(Arc::new)
    })
    .as_ref()
    .map(|c| c.clone())
}

pub fn new_provider() -> HighlightWorker {
    let config = toml_config().expect("tree-sitter-toml-ng 高亮配置必须构建");
    HighlightWorker::new(LanguageId::new("toml"), config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::providers::common::test_support::{
        assert_lookup_matches_capture_names, smoke_test_provider,
    };

    const SAMPLE: &str = "key = \"value\"\n[section]\nname = 1\n";

    #[test]
    fn toml_provider_emits_spans() {
        smoke_test_provider(LanguageId::new("toml"), SAMPLE, new_provider);
    }

    #[test]
    fn lookup_table_matches_query_capture_names() {
        let cfg = toml_config().expect("toml 配置必须构建");
        assert_lookup_matches_capture_names(&cfg);
    }
}
