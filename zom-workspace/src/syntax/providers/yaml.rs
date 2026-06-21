//! tree-sitter-yaml Tier 1 provider（`tree-sitter-grammars/tree-sitter-yaml`，
//! 不是已停更的同名包；这一份是 amaanq 维护的活跃版本）。
//!
//! 机制全部在 [`super::common`] 里——本文件只通过 [`declare_tier1_provider!`] 声明语言常量。
//! 设计说明详见该模块注释。

use crate::{declare_tier1_provider, standard_provider_tests};

declare_tier1_provider!(
    yaml_config,
    new_provider,
    "yaml",
    tree_sitter_yaml::LANGUAGE,
    tree_sitter_yaml::HIGHLIGHTS_QUERY
);

standard_provider_tests!(
    yaml_config,
    new_provider,
    "yaml",
    "name: zom\nversion: 1\nfeatures:\n  - alpha\n  - beta\n"
);
