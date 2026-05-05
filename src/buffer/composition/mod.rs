//! IME composition 子系统。
//!
//! # Invariants
//! - 同一时刻最多只有一个 active composition。
//! - active composition 的 `range` 必须始终位于有效文本范围内。
//! - composition selection 必须落在 grapheme 边界，不能切开 cluster。
//! - 非 composition 来源的文本编辑必须先取消 active composition。

mod state;
mod validation;
mod workflow;
