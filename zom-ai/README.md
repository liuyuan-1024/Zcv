# zom-ai

`zom-ai` 是 chat / agentic（多轮 + 工具调用）AI 请求的**协议层**：定义消息、工具、流式事件、错误，以及 `AiProvider` trait。

完整设计与权衡见 [`docs/抽象重设计.md`](docs/抽象重设计.md)。

## 定位

`zom-ai` 只放稳定的协议边界：

- **数据模型**：`Role / Content / Message`、`ToolDef / ToolCall / ToolResult`、`ChatRequest / Options`。
- **流式协议**：`StreamEvent / StopReason / Usage`。
- **服务接口**：`AiProvider`（async，返回 `EventStream`）。
- **错误**：`AiError`。

它**不做**这些事：

- 不持有会话状态。`Vec<Message>` 由上层维护，裁剪 / 压缩 / 持久化由上层决定。
- 不实现 agent loop（多轮 + 工具执行循环）。
- 不知道"编辑"是什么。"读 buffer / 应用编辑 / 跑命令"等能力由上层注册为 `ToolDef`，通过 `input_schema` 显式约束参数。
- 不接入任何具体厂商。HTTP / 鉴权 / SDK 适配放在独立 provider crate（如未来的 `zom-ai-anthropic`）。

## 取消

取消语义统一为 **drop `EventStream`**。上层 drop 掉返回的 stream，具体 provider 在 stream 的 drop 实现里关闭底层连接，不引入第二条取消通道。

## 依赖边界

```text
zom-ai → zom-engine
zom-ai → futures-core      （流式协议）
zom-ai → async-trait       （async trait 写法）
zom-ai → serde_json        （tool 的 input_schema / input / 结果）
```

不依赖：网络栈、tokio、任何厂商 SDK、`zom-command`、`zom-workspace`。

## 文件结构

```text
src/
├── lib.rs           重新导出
├── message.rs       Role / Content / Message
├── tool.rs          ToolDef / ToolCall / ToolResult
├── request.rs       ChatRequest / Options
├── stream.rs        StreamEvent / StopReason / Usage
├── provider.rs      AiProvider / EventStream
└── error.rs         AiError
docs/
└── 抽象重设计.md    设计文档（背景、目标、数据模型、迁移计划等）
```

## 发布状态

0.2.0：本轮重写为协议层形状，无对外 stable 承诺；不保留旧 `AiRequest / AiProposal / ProposedEdit / ProposedRange` 等类型的 deprecation alias。
