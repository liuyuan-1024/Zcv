//! LSP 协议层：语言服务器客户端的 JSON-RPC 通信、生命周期管理与能力抽象。
//!
//! ## 定位
//!
//! `zom-lsp` 是 language server protocol 的客户端协议层。它负责：
//!
//! - 启动和管理 language server 子进程（stdio transport）
//! - JSON-RPC 2.0 消息编解码
//! - `initialize` / `shutdown` 生命周期握手
//! - `textDocument/didOpen` / `didChange` / `didClose` 文档同步
//! - 各 LSP 能力的类型安全请求封装（semantic tokens、diagnostics、completion 等）
//!
//! ## 边界
//!
//! - 不持有 buffer、不操作编辑器状态——只做协议通信
//! - 不知道 GPUI、UI、命令系统——上层（zom-desktop）负责接线
//! - 不实现 `HighlightProvider` trait——provider 适配在 zom-workspace 或 zom-desktop 的边界层完成
//!
//! ## 依赖
//!
//! ```text
//! zom-lsp → zom-engine          （UTF-16 坐标类型）
//! zom-lsp → lsp-types            （LSP 协议类型定义）
//! zom-lsp → serde_json            （JSON-RPC 序列化）
//! ```
//!
//! 不依赖：tokio、网络栈、zom-command、zom-workspace、zom-desktop、GPUI。

pub mod client;
pub mod error;
pub(crate) mod transport;

pub use client::{LspClient, NotificationHandler};
pub use error::LspError;
