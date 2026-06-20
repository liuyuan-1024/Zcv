//! tree-sitter-typescript Tier 1 provider（TypeScript + TSX 两套 grammar
//! 共一个 crate）。
//!
//! 机制全部在 [`super::common`] 里——本文件只通过 [`declare_tier1_provider!`] 声明语言常量。
//! 设计说明详见该模块注释。
//!
//! ## TypeScript 与 TSX 的关系
//!
//! `tree-sitter-typescript` 这一个 crate 同时导出 `LANGUAGE_TYPESCRIPT`（`.ts`）与 `LANGUAGE_TSX`（`.tsx`）两个 grammar，并共用同一份 `HIGHLIGHTS_QUERY`。
//! 它们对调度层是**两条独立语言**——`LanguageId::new("typescript")` 与 `LanguageId::new("tsx")`——内置 provider 安装入口会分别注册扩展名。

use crate::declare_tier1_provider;

declare_tier1_provider!(
    typescript_config,
    new_typescript_provider,
    "typescript",
    tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
    tree_sitter_typescript::HIGHLIGHTS_QUERY
);

declare_tier1_provider!(
    tsx_config,
    new_tsx_provider,
    "tsx",
    tree_sitter_typescript::LANGUAGE_TSX,
    tree_sitter_typescript::HIGHLIGHTS_QUERY
);

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::LanguageId;
    use crate::syntax::providers::common::test_support::{
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
