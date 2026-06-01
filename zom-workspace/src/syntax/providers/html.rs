//! tree-sitter-html Tier 1 provider。
//!
//! 机制全部在 [`super::common`] 里。设计说明详见该模块注释。
//!
//! 当前不接 `INJECTIONS_QUERY`——`<script>` / `<style>` 块的 JS / CSS 高亮要
//! injection 才能给出，而 injection 是手册 §十四 明列的非目标。HTML 块内
//! 嵌入的代码暂走 theme 默认前景色。

use std::sync::{Arc, OnceLock};

use tree_sitter::QueryError;

use crate::syntax::LanguageId;
use crate::syntax::providers::common::{HighlightWorker, SharedConfig, build_shared_config};

fn html_config() -> Result<Arc<SharedConfig>, &'static QueryError> {
    static CELL: OnceLock<Result<Arc<SharedConfig>, QueryError>> = OnceLock::new();
    CELL.get_or_init(|| {
        build_shared_config(
            tree_sitter_html::LANGUAGE.into(),
            tree_sitter_html::HIGHLIGHTS_QUERY,
        )
        .map(Arc::new)
    })
    .as_ref()
    .map(|c| c.clone())
}

pub fn new_provider() -> HighlightWorker {
    let config = html_config().expect("tree-sitter-html 高亮配置必须构建");
    HighlightWorker::new(LanguageId::new("html"), config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::providers::common::{
        assert_lookup_matches_capture_names, smoke_test_provider,
    };

    const SAMPLE: &str = "<!DOCTYPE html>\n<html><body><h1 class=\"t\">hi</h1></body></html>\n";

    #[test]
    fn html_provider_emits_spans() {
        smoke_test_provider(LanguageId::new("html"), SAMPLE, new_provider);
    }

    #[test]
    fn lookup_table_matches_query_capture_names() {
        let cfg = html_config().expect("html 配置必须构建");
        assert_lookup_matches_capture_names(&cfg);
    }
}
