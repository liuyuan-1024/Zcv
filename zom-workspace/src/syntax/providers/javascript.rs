//! tree-sitter-javascript Tier 1 provider。
//!
//! 机制全部在 [`super::common`] 里——本文件只通过 [`declare_tier1_provider!`] 声明语言常量。
//! 设计说明详见该模块注释。
//!
//! 注意：当前 JavaScript 复用 `tree_sitter_typescript` crate 的 TSX grammar
//! （`tree-sitter-javascript` 未单独作为 crate 在 workspace 里声明）；
//! JSX / locals 支持待 injection 机制到位后再接入。

use crate::{declare_tier1_provider, standard_provider_tests};

declare_tier1_provider!(
    javascript_config,
    new_provider,
    "javascript",
    tree_sitter_typescript::LANGUAGE_TSX,
    tree_sitter_typescript::HIGHLIGHTS_QUERY
);

standard_provider_tests!(
    javascript_config,
    new_provider,
    "javascript",
    "const x = 1;\nfunction f(a) { return a + 1; }\n"
);
