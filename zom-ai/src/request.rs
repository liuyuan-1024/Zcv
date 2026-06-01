use crate::message::Message;
use crate::tool::ToolDef;

/// 一次 chat 请求。`zom-ai` 不持有会话状态，每次请求都由上层完整提交 `messages`。
#[derive(Clone, Debug)]
pub struct ChatRequest {
    pub system: Option<String>,
    pub messages: Vec<Message>,
    pub tools: Vec<ToolDef>,
    pub options: Options,
}

/// 跨厂商通用的运行参数。厂商私有参数留在各 provider crate 内部，不污染抽象层。
#[derive(Clone, Debug)]
pub struct Options {
    pub model: String,
    pub max_tokens: u32,
    pub temperature: Option<f32>,
}
