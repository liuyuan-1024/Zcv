use crate::message::Role;

/// 流式事件。形状对齐 Anthropic SSE 事件流，OpenAI 的 delta 也能映射进来。
///
/// `ToolInputDelta.json_fragment` 是累积型片段，由上层拼成完整 JSON 后再 parse ——
/// 与厂商对 tool input 分块下发的现实一致。
#[derive(Clone, Debug, PartialEq)]
pub enum StreamEvent {
    MessageStart {
        role: Role,
    },
    TextDelta {
        text: String,
    },
    ToolUseStart {
        id: String,
        name: String,
    },
    ToolInputDelta {
        id: String,
        json_fragment: String,
    },
    ContentBlockStop {
        index: u32,
    },
    MessageStop {
        stop_reason: StopReason,
        usage: Usage,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StopReason {
    EndTurn,
    ToolUse,
    MaxTokens,
    StopSequence,
    Cancelled,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Usage {
    pub input_tokens: u32,
    pub output_tokens: u32,
}
