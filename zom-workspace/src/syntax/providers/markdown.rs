//! Markdown provider (Phase 3 后形态).
//!
//! Phase 3 整段清理把原 `MarkdownWorker`（block + inline + injection 三层 tree） 收口为单层 block tree：
//! 复用通用的 [`super::common::HighlightWorker`]，只挂 `tree-sitter-md` 的 block grammar。
//!
//! ## 已下线（计划 §"风险与未知点" 3）
//!
//! - **inline 着色**：emphasis / strong / code span / strikethrough 不再单独标色。
//! 它们仍是 block 树里的子节点，但 block query 不覆盖。
//! - **fenced code 注入**：```` ```rust ```` 之类的代码块不再按宿主语言着色，整段走 `markup.raw.block`。
//!
//! 恢复这两条特性需要扩展 [`crate::syntax::BufferSyntaxTree`] 为多树容器(`Arc<Vec<Arc<Tree>>>`)，paint 端 query 入口能跨多 grammar，是独立工作项。
//!
//! ## 仍保留
//!
//! - block grammar + 本仓的 query 扩展（任务列表标记、表格分隔符、`@attribute`）。
//! - capture name 走默认归一化：`text.title → markup.heading`、`text.literal → markup.raw.block`、`text.uri / text.reference → markup.link.*`。

use std::sync::{Arc, OnceLock};

use tree_sitter::QueryError;

use crate::syntax::LanguageId;
use crate::syntax::providers::common::{HighlightWorker, SharedConfig, build_shared_config};

const MARKDOWN_BLOCK_QUERY_EXTENSION: &str = r#"
; zom 本地扩展：补齐 tree-sitter-md 随包 nvim query 未覆盖的 Markdown 源码标记。
[
  (task_list_marker_checked)
  (task_list_marker_unchecked)
] @markup.list

(language) @attribute

(pipe_table_header
  (pipe_table_cell) @markup.heading)

[
  (pipe_table_delimiter_cell)
  (pipe_table_align_left)
  (pipe_table_align_right)
] @punctuation.delimiter
"#;

fn extended_markdown_block_query() -> &'static str {
    static CELL: OnceLock<&'static str> = OnceLock::new();
    CELL.get_or_init(|| {
        Box::leak(
            format!(
                "{}\n{}",
                tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
                MARKDOWN_BLOCK_QUERY_EXTENSION
            )
            .into_boxed_str(),
        )
    })
}

fn markdown_block_config() -> Result<Arc<SharedConfig>, &'static QueryError> {
    static CELL: OnceLock<Result<Arc<SharedConfig>, QueryError>> = OnceLock::new();
    CELL.get_or_init(|| {
        build_shared_config(
            tree_sitter_md::LANGUAGE.into(),
            extended_markdown_block_query(),
        )
        .map(Arc::new)
    })
    .as_ref()
    .map(|c| c.clone())
}

pub fn new_provider() -> HighlightWorker {
    let block = markdown_block_config().expect("tree-sitter-md block 高亮配置必须构建");
    HighlightWorker::new(LanguageId::new("markdown"), block)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::providers::common::smoke_test_provider;

    #[test]
    fn markdown_provider_emits_block_spans_for_sample() {
        smoke_test_provider(
            LanguageId::new("markdown"),
            "# title\n\n- item one\n- item two\n\n```rust\nlet x = 1;\n```\n",
            new_provider,
        );
    }
}
