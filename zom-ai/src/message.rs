use crate::tool::{ToolCall, ToolResult};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Role {
    System,
    User,
    Assistant,
    Tool,
}

/// 一条消息内的单个内容段。
///
/// assistant 消息可能同时包含 text 段与一个或多个 tool_use 段，因此 `Message.content`
/// 用有序数组而非二选一枚举。
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Content {
    Text(String),
    ToolUse(ToolCall),
    ToolResult(ToolResult),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Message {
    pub role: Role,
    pub content: Vec<Content>,
}
