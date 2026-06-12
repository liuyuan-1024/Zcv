//! AI 抽象层：chat / agentic 请求的协议层。
//!
//! 设计与边界见 `docs/协议层设计.md`。本 crate 不持有会话状态、不实现具体厂商、
//! 不知道"编辑"是什么 —— 编辑能力以 tool 形式由上层注册。

pub mod error;
pub mod message;
pub mod provider;
pub mod request;
pub mod stream;
pub mod tool;

pub use error::AiError;
pub use message::{Content, Message, Role};
pub use provider::{AiProvider, EventStream};
pub use request::{ChatRequest, Options};
pub use stream::{StopReason, StreamEvent, Usage};
pub use tool::{ToolCall, ToolDef, ToolResult};
