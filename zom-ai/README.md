# zom-ai

`zom-ai` 是 zom 宿主层的 AI 抽象 crate，定义 AI provider 抽象、请求 / 提案类型和 proposal→transaction 转换。

## 定位

`zom-ai` 是 AI 接入的**抽象层**：trait + 数据类型 + 纯函数转换。

它只依赖 `zom-engine`，并保持**零网络依赖** —— 具体 provider 的网络实现（HTTP 客户端、API key、serde）属于未来的 `zom-ai-<provider>` leaf crate。

它与 `zom-command` **无依赖边**（两个方向都没有）。AI 是命令的*来源*与*目标*，但翻译发生在 `zom-desktop` 组合根，不靠 `zom-ai` 认识命令词表 —— AI 与键盘是对称的命令来源。

## 核心能力

- **版本绑定一切**：`AiRequest` / `AiProposal` 携带 `BufferVersion`，`ProposedEdit` 用 engine `TextRange`，不用裸 byte offset。陈旧提案靠版本检查拒绝，与 `Transaction` 绑定 `base_version` 同构。
- **one-shot 核心 trait**：`AiProvider::propose` 是 async 且 one-shot —— 返回一个完整的、版本绑定的提案。流式是 `zom-desktop` chat/agent 层的附加能力，不进核心 trait。
- **proposal→transaction 转换**：`proposal_to_transaction` 是纯函数，只吃一个 `Snapshot`，不认识 `Workspace` —— 因此 `zom-ai` 是端到端可测的完整单元（提案进 → 事务或拒绝出）。
- **AI 作为命令来源**：`AiAction` 是 plain 意图（`command: String` + args），由 `zom-desktop` 边界解析成 `(CommandId, CommandArgs)`。

## 依赖

```text
zom-ai → zom-engine
```

只依赖 `zom-engine`，零网络依赖。**不依赖 `zom-command`**。

## 结构概览

```text
src/lib.rs    AiRequest / AiProposal / ProposedEdit / AiAction
              AiProvider / proposal_to_transaction
              AiError / ProposalRejected
```

骨架阶段为单文件 `lib.rs`。

## 相关文档

- `../AGENTS.md`：workspace 全局协作规则。
- `../TODO.md`：宿主层开发规划，本 crate 对应能力域 6（区间追踪与外部结果承载），阶段 P4。

## 状态

骨架阶段：类型形状与 trait 签名已定，`proposal_to_transaction` 的校验转换实现留待 `TODO.md` P4。
