//! tree-sitter-java Tier 1 provider。
//!
//! 机制全部在 [`super::common`] 里。设计说明详见该模块注释。

use std::sync::{Arc, OnceLock};

use tree_sitter::QueryError;

use crate::syntax::LanguageId;
use crate::syntax::providers::common::{HighlightWorker, SharedConfig, build_shared_config};

fn java_config() -> Result<Arc<SharedConfig>, &'static QueryError> {
    static CELL: OnceLock<Result<Arc<SharedConfig>, QueryError>> = OnceLock::new();
    CELL.get_or_init(|| {
        build_shared_config(
            tree_sitter_java::LANGUAGE.into(),
            tree_sitter_java::HIGHLIGHTS_QUERY,
        )
        .map(Arc::new)
    })
    .as_ref()
    .map(|c| c.clone())
}

pub fn new_provider() -> HighlightWorker {
    let config = java_config().expect("tree-sitter-java 高亮配置必须构建");
    HighlightWorker::new(LanguageId::new("java"), config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::providers::common::{
        assert_lookup_matches_capture_names, smoke_test_provider,
    };

    const SAMPLE: &str = "public class Main {\n    public static void main(String[] args) {}\n}\n";

    #[test]
    fn java_provider_emits_spans() {
        smoke_test_provider(LanguageId::new("java"), SAMPLE, new_provider);
    }

    #[test]
    fn lookup_table_matches_query_capture_names() {
        let cfg = java_config().expect("java 配置必须构建");
        assert_lookup_matches_capture_names(&cfg);
    }
}
