//! tree-sitter-html Tier 1 provider。
//!
//! 机制全部在 [`super::common`] 里——本文件只通过 [`declare_tier1_provider!`] 声明语言常量。
//! 设计说明详见该模块注释。
//!
//! 当前不接 `INJECTIONS_QUERY`——`<script>` / `<style>` 块的 JS / CSS 高亮要
//! injection 才能给出，而 injection 是手册 §十四 明列的非目标。HTML 块内
//! 嵌入的代码暂走 theme 默认前景色。

use crate::{declare_tier1_provider, standard_provider_tests};

declare_tier1_provider!(
    html_config,
    new_provider,
    "html",
    tree_sitter_html::LANGUAGE,
    tree_sitter_html::HIGHLIGHTS_QUERY
);

standard_provider_tests!(
    html_config,
    new_provider,
    "html",
    "<!DOCTYPE html>\n<html><body><h1 class=\"t\">hi</h1></body></html>\n"
);
