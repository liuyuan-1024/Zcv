//! tree-sitter-java Tier 1 provider。
//!
//! 机制全部在 [`super::common`] 里——本文件只通过 [`declare_tier1_provider!`] 声明语言常量。
//! 设计说明详见该模块注释。

use crate::{declare_tier1_provider, standard_provider_tests};

declare_tier1_provider!(
    java_config,
    new_provider,
    "java",
    tree_sitter_java::LANGUAGE,
    tree_sitter_java::HIGHLIGHTS_QUERY
);

standard_provider_tests!(
    java_config,
    new_provider,
    "java",
    "public class Main {\n    public static void main(String[] args) {}\n}\n"
);
