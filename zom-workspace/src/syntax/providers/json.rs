//! tree-sitter-json Tier 1 provider。
//!
//! 机制全部在 [`super::common`] 里——本文件只通过 [`declare_tier1_provider!`] 声明语言常量。
//! 设计说明详见该模块注释。

use crate::{declare_tier1_provider, standard_provider_tests};

declare_tier1_provider!(
    json_config,
    new_provider,
    "json",
    tree_sitter_json::LANGUAGE,
    tree_sitter_json::HIGHLIGHTS_QUERY
);

standard_provider_tests!(
    json_config,
    new_provider,
    "json",
    "{\"name\": \"zom\", \"version\": 1, \"keywords\": [\"a\", \"b\"]}"
);
