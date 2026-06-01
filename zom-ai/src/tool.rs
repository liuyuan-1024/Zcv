/// 工具定义，由上层在发起请求时随 `ChatRequest.tools` 一起声明。
///
/// `zom-ai` 不规定有哪些工具；"读 buffer / 应用编辑 / 跑命令"等具体能力都由上层
/// 注册，把 buffer id、版本号、范围等通过 `input_schema` 显式约束。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolDef {
    pub name: String,
    pub description: String,
    // JSON Schema，描述 `ToolCall.input` 的结构。
    pub input_schema: serde_json::Value,
}

/// 模型一次工具调用请求。`id` 用于把后续 `ToolResult` 对回这一次调用。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub input: serde_json::Value,
}

/// 工具执行结果，由上层执行完工具后构造，作为下一轮 `Message` 的内容塞回模型。
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ToolResult {
    pub call_id: String,
    pub content: String,
    pub is_error: bool,
}
