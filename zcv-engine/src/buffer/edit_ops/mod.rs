//! 文本变异与多选区编辑子系统。
//!
//! # Invariants
//! - 所有写路径在进入事务前必须完成 range/boundary 校验。
//! - selection 级批量替换按归一化顺序构建 edit，确保可验证且无重叠。
//! - no-op 变更不递增版本、不污染历史。

mod basic;
