//! tree-sitter-python Tier 1 provider。
//!
//! 机制全部在 [`super::common`] 里——本文件只通过 [`declare_tier1_provider!`] 声明语言常量。
//! 设计说明详见该模块注释。

use crate::{declare_tier1_provider, standard_provider_tests};

declare_tier1_provider!(
    python_config,
    new_provider,
    "python",
    tree_sitter_python::LANGUAGE,
    tree_sitter_python::HIGHLIGHTS_QUERY
);

standard_provider_tests!(
    python_config,
    new_provider,
    "python",
    "def greet(name: str) -> str:\n    return f\"hi {name}\"\n"
);
