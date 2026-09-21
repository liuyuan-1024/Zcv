//! 文本变异与编辑子系统。
//!
//! # Invariants
//! - 所有写路径在进入事务前必须完成 range/boundary 校验。
//! - 空编辑（`edits` 为空）由 `Transaction` 拒绝，不递增版本，也不产生历史节点。
//! - 同文本替换仍是有效编辑：它推进版本并记录历史；Zed 的 `apply_edit_internal` 也只在 `edits` 为空时提前返回，不比较新旧文本。

mod basic;
