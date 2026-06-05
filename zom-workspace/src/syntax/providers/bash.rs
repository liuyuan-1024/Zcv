//! tree-sitter-bash Tier 1 provider。
//!
//! 机制全部在 [`super::common`] 里。设计说明详见该模块注释。
//!
//! 注意：tree-sitter-bash 用单数 `HIGHLIGHT_QUERY`（其余 grammar 多用复数
//! `HIGHLIGHTS_QUERY`），命名不一致。

use std::sync::{Arc, OnceLock};

use tree_sitter::QueryError;

use crate::syntax::LanguageId;
use crate::syntax::providers::common::{HighlightWorker, SharedConfig, build_shared_config};

pub(crate) fn bash_config() -> Result<Arc<SharedConfig>, &'static QueryError> {
    static CELL: OnceLock<Result<Arc<SharedConfig>, QueryError>> = OnceLock::new();
    CELL.get_or_init(|| {
        build_shared_config(
            tree_sitter_bash::LANGUAGE.into(),
            tree_sitter_bash::HIGHLIGHT_QUERY,
        )
        .map(Arc::new)
    })
    .as_ref()
    .map(|c| c.clone())
}

pub fn new_provider() -> HighlightWorker {
    let config = bash_config().expect("tree-sitter-bash 高亮配置必须构建");
    HighlightWorker::new(LanguageId::new("bash"), config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::providers::common::{
        assert_lookup_matches_capture_names, smoke_test_provider,
    };

    const SAMPLE: &str = "#!/bin/bash\necho \"hi\"\nfor x in 1 2; do echo $x; done\n";

    #[test]
    fn bash_provider_emits_spans() {
        smoke_test_provider(LanguageId::new("bash"), SAMPLE, new_provider);
    }

    #[test]
    fn lookup_table_matches_query_capture_names() {
        let cfg = bash_config().expect("bash 配置必须构建");
        assert_lookup_matches_capture_names(&cfg);
    }
}
