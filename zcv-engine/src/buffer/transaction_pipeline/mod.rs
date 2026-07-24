//! 事务管线子系统：准备、提交和纯文本历史收尾。
//!
//! # Invariants
//! - 任何事务在提交前必须通过 base_version 校验与 edit 边界校验。
//! - 事务提交采用“clone 后应用，再一次性替换”策略，保证失败原子性。
//! - history 记录由 metadata 显式控制；非记录型事务仍会失效 redo 分支。
//! - 事务结果返回 PositionMap 等事实，视图状态由宿主据此更新。

mod apply;
mod prepared;
