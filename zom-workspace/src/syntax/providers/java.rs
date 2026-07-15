//! tree-sitter-java 内建 provider。
//!
//! 机制全部在 [`super::common`] 里——本文件只通过 [`declare_builtin_provider!`] 声明语言常量。
//! 设计说明详见该模块注释。

use crate::{declare_builtin_provider, standard_provider_tests};

declare_builtin_provider!(
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
