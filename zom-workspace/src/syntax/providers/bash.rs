//! tree-sitter-bash Tier 1 provider。
//!
//! 机制全部在 [`super::common`] 里——本文件只通过 [`declare_tier1_provider!`] 声明语言常量。
//! 设计说明详见该模块注释。
//!
//! 注意：tree-sitter-bash 用单数 `HIGHLIGHT_QUERY`（其余 grammar 多用复数
//! `HIGHLIGHTS_QUERY`），命名不一致。

use crate::{declare_tier1_provider, standard_provider_tests};

declare_tier1_provider!(
    bash_config,
    new_provider,
    "bash",
    tree_sitter_bash::LANGUAGE,
    tree_sitter_bash::HIGHLIGHT_QUERY
);

standard_provider_tests!(
    bash_config,
    new_provider,
    "bash",
    "#!/bin/bash\necho \"hi\"\nfor x in 1 2; do echo $x; done\n"
);
