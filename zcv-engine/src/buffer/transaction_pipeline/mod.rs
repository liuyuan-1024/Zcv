//! 事务管线子系统：准备、提交、selection 映射、历史收尾。
//!
//! # Invariants
//! - 任何事务在提交前必须通过 base_version 校验与 edit 边界校验。
//! - 事务提交采用“clone 后应用，再一次性替换”策略，保证失败原子性。
//! - history 记录由 metadata 显式控制；非记录型事务仍会失效 redo 分支。
//! - selection 更新在文本提交后统一计算，避免中间态泄漏。

mod apply;
mod prepared;
