//! tree-sitter-toml 内建 provider。
//!
//! 机制全部在 [`super::common`] 里——本文件只通过 [`declare_builtin_provider!`] 声明语言常量。
//! 设计说明详见该模块注释。

use crate::{declare_builtin_provider, standard_provider_tests};

declare_builtin_provider!(
    toml_config,
    new_provider,
    "toml",
    tree_sitter_toml_ng::LANGUAGE,
    tree_sitter_toml_ng::HIGHLIGHTS_QUERY
);

standard_provider_tests!(
    toml_config,
    new_provider,
    "toml",
    "key = \"value\"\n[section]\nname = 1\n"
);
