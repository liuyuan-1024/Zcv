//! tree-sitter-typescript Tier 1 provider（TypeScript + TSX 两套 grammar
//! 共一个 crate）。
//!
//! 机制全部在 [`super::common`] 里。设计说明详见该模块注释。
//!
//! ## TypeScript 与 TSX 的关系
//!
//! `tree-sitter-typescript` 这一个 crate 同时导出 `LANGUAGE_TYPESCRIPT`（`.ts`）与 `LANGUAGE_TSX`（`.tsx`）两个 grammar，并共用同一份 `HIGHLIGHTS_QUERY`。
//! 它们对调度层是**两条独立语言**——`LanguageId::new("typescript")` 与 `LanguageId::new("tsx")`——内置 provider 安装入口会分别注册扩展名。

use std::sync::{Arc, OnceLock};

use tree_sitter::QueryError;

use crate::syntax::LanguageId;
use crate::syntax::providers::common::{HighlightWorker, SharedConfig, build_shared_config};

fn typescript_config() -> Result<Arc<SharedConfig>, &'static QueryError> {
    static CELL: OnceLock<Result<Arc<SharedConfig>, QueryError>> = OnceLock::new();
    CELL.get_or_init(|| {
        build_shared_config(
            tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
        )
        .map(Arc::new)
    })
    .as_ref()
    .map(|c| c.clone())
}

fn tsx_config() -> Result<Arc<SharedConfig>, &'static QueryError> {
    static CELL: OnceLock<Result<Arc<SharedConfig>, QueryError>> = OnceLock::new();
    CELL.get_or_init(|| {
        build_shared_config(
            tree_sitter_typescript::LANGUAGE_TSX.into(),
            tree_sitter_typescript::HIGHLIGHTS_QUERY,
        )
        .map(Arc::new)
    })
    .as_ref()
    .map(|c| c.clone())
}

pub fn new_typescript_provider() -> HighlightWorker {
    let config = typescript_config().expect("tree-sitter-typescript (TypeScript) 高亮配置必须构建");
    HighlightWorker::new(LanguageId::new("typescript"), config)
}

pub fn new_tsx_provider() -> HighlightWorker {
    let config = tsx_config().expect("tree-sitter-typescript (TSX) 高亮配置必须构建");
    HighlightWorker::new(LanguageId::new("tsx"), config)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::providers::common::{
        assert_lookup_matches_capture_names, smoke_test_provider,
    };

    const TS_SAMPLE: &str = "interface P { name: string }\nconst x: number = 1;\n";
    const TSX_SAMPLE: &str =
        "const C = (p: { name: string }) => <div className=\"x\">{p.name}</div>;\n";

    #[test]
    fn typescript_provider_emits_spans() {
        smoke_test_provider(
            LanguageId::new("typescript"),
            TS_SAMPLE,
            new_typescript_provider,
        );
    }

    #[test]
    fn tsx_provider_emits_spans() {
        smoke_test_provider(LanguageId::new("tsx"), TSX_SAMPLE, new_tsx_provider);
    }

    #[test]
    fn typescript_lookup_matches_query_capture_names() {
        let cfg = typescript_config().expect("typescript 配置必须构建");
        assert_lookup_matches_capture_names(&cfg);
    }

    #[test]
    fn tsx_lookup_matches_query_capture_names() {
        let cfg = tsx_config().expect("tsx 配置必须构建");
        assert_lookup_matches_capture_names(&cfg);
    }
}
