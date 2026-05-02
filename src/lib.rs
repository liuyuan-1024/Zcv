//! Zom Engine - 底层文本编辑引擎
//!
//! 不直接负责 UI 渲染、LSP 协议、语法树生成或项目级索引。
//! 仅专注于文本存储、坐标模型、事务变异、历史系统以及外部系统所需的底层文本协作接口。

pub mod config;
pub mod errors;
pub mod types;

pub use config::*;
pub use errors::*;
pub use types::*;
