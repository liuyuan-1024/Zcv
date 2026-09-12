//! 基于 Tree-sitter 的结构查询。
//!
//! 每个查询家族独立维护自己的结果类型、查询实现和辅助函数；
//! `SyntaxSnapshot` 仍是所有查询共享的不可变语法状态。

mod brackets;
mod folds;
mod indent;
mod locals;
mod nodes;
mod outline;

pub use brackets::BracketPair;
pub use folds::FoldRange;
pub use indent::NewlineIndent;
pub use locals::LocalBinding;
pub use nodes::SyntaxNode;
pub use outline::{OutlineItem, OutlineTextRange};

#[cfg(test)]
mod tests;
