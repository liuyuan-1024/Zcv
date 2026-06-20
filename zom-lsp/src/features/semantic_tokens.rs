//! `textDocument/semanticTokens` —— 语义高亮。
//!
//! 后续在此封装：
//! - `textDocument/semanticTokens/full` 请求
//! - `textDocument/semanticTokens/full/delta` 增量刷新
//! - LSP `SemanticTokens` → `BufferSyntaxTree` 的转换逻辑（或留在 zom-workspace 边界做）
