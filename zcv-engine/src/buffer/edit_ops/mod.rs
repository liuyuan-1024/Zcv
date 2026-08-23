//! 文本变异与编辑子系统。
//!
//! # Invariants
//! - 所有写路径在进入事务前必须完成 range/boundary 校验。
//! - no-op 变更不递增版本、不污染历史。

mod basic;
