//! LSP 客户端：管理一个语言服务器的完整生命周期。
//!
//! 负责：
//! - 启动/停止 server 进程
//! - `initialize` / `initialized` / `shutdown` 生命周期握手
//! - `textDocument/didOpen` / `didChange` / `didClose` 文档同步
//! - 发送请求并以 `mpsc::channel` 异步等待响应
//!
//! ## 线程模型
//!
//! - **写侧**：`LspClient` 的 public 方法（可在主线程调用）通过 `Arc<Mutex<ChildStdin>>` 写 stdin。
//! - **读侧**：后台读线程独占 `BufReader<ChildStdout>`，阻塞读取原始消息后按 JSON-RPC 语义分发：
//!   - 有 `id` → response → 匹配 `pending` 里的 `mpsc::Sender`
//!   - 有 `method` 无 `id` → notification → 交给 [`NotificationHandler`]

use std::collections::HashMap;
use std::io::BufReader;
use std::process::{Child, ChildStdin, ChildStdout};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;

use lsp_types::{InitializeResult, ServerCapabilities, Uri};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::LspError;
use crate::transport::{self, StdioTransport};

/// Server → client 通知的接收端。
///
/// 上层（zom-desktop）实现此 trait 来消费 LSP 推送的诊断、日志等。
/// 回调在后台读线程上执行——实现方负责把数据同步到主线程。
pub trait NotificationHandler: Send + 'static {
    /// 收到一条通知。`method` 是 LSP 方法名（如 `textDocument/publishDiagnostics`），
    /// `params` 是通知参数的 JSON value。
    fn on_notification(&self, method: &str, params: Value);
}

// ── JSON-RPC 线格式 ────────────────────────────────────────────

#[derive(Debug, Serialize)]
struct JsonRpcRequest<'a> {
    jsonrpc: &'static str,
    id: u64,
    method: &'a str,
    params: Value,
}

impl<'a> JsonRpcRequest<'a> {
    fn to_json(&self) -> Result<String, LspError> {
        serde_json::to_string(self).map_err(|e| LspError::ProtocolViolation {
            detail: format!("序列化请求失败：{e}"),
        })
    }
}

#[derive(Debug, Serialize)]
struct JsonRpcNotification<'a> {
    jsonrpc: &'static str,
    method: &'a str,
    params: Value,
}

impl<'a> JsonRpcNotification<'a> {
    fn to_json(&self) -> Result<String, LspError> {
        serde_json::to_string(self).map_err(|e| LspError::ProtocolViolation {
            detail: format!("序列化通知失败：{e}"),
        })
    }
}

#[derive(Debug, Deserialize)]
struct IncomingMessage {
    #[serde(default)]
    id: Option<u64>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<Value>,
    #[serde(default)]
    result: Option<Value>,
    #[serde(default)]
    error: Option<ResponseError>,
}

#[derive(Debug, Deserialize)]
struct ResponseError {
    code: i64,
    message: String,
}

/// 异步请求的回调：主线程创建 mpsc channel，读线程拿到响应后 send。
type PendingSender = std::sync::mpsc::Sender<Result<Value, LspError>>;

// ── LspClient ───────────────────────────────────────────────────

/// LSP 客户端——语言服务器的本地代理。
///
/// 不可 Clone——每个 `LspClient` 独占一个 server 子进程。
/// 多份持有者通过 `Arc<LspClient>` 共享。
pub struct LspClient {
    /// 下一个请求 id（原子递增）。
    next_id: Arc<AtomicU64>,
    /// 待完成的异步请求：`id → mpsc sender`。
    pending: Arc<Mutex<HashMap<u64, PendingSender>>>,
    /// `initialize` 完成后缓存的 server capabilities。
    capabilities: Arc<ServerCapabilities>,
    /// 写侧：通过 mutex 共享的 stdin。
    stdin: Arc<Mutex<ChildStdin>>,
    /// 子进程句柄——shutdown 时需要。
    child: Arc<Mutex<Option<Child>>>,
    /// 标记读线程是否存活。
    alive: Arc<AtomicBool>,
    /// 后台读线程——`shutdown` 时 join；
    /// `Drop` 时若仍为 `Some`，读线程在 stdout EOF 后自动退出（detached）。
    _read_thread: Option<JoinHandle<()>>,
}

impl LspClient {
    /// 启动语言服务器并完成 `initialize` 握手。
    ///
    /// `root_uri`：项目根目录的 `file://` URI。
    /// `notification_handler`：server → client 通知的消费端。
    pub fn launch(
        command: &str,
        args: &[String],
        root_uri: Uri,
        notification_handler: Box<dyn NotificationHandler>,
    ) -> Result<Self, LspError> {
        let mut transport = StdioTransport::spawn(command, args)?;

        // initialize 阶段仍然同步——必须拿到 capabilities 才能返回
        let capabilities = initialize_handshake(&mut transport, root_uri)?;

        // 拆分 transport，读写分离
        let (stdin, mut stdout_reader, child) = transport.into_parts();

        let next_id = Arc::new(AtomicU64::new(1));
        let pending: Arc<Mutex<HashMap<u64, PendingSender>>> = Arc::new(Mutex::new(HashMap::new()));
        let capabilities = Arc::new(capabilities);
        let stdin = Arc::new(Mutex::new(stdin));
        let child = Arc::new(Mutex::new(Some(child)));
        let notification_handler = Arc::new(Mutex::new(Some(notification_handler)));
        let alive = Arc::new(AtomicBool::new(true));

        // 启动后台读线程
        let read_thread = {
            let pending = Arc::clone(&pending);
            let notification_handler = Arc::clone(&notification_handler);
            let alive = Arc::clone(&alive);
            std::thread::spawn(move || {
                read_loop(&mut stdout_reader, &pending, &notification_handler, &alive);
            })
        };

        Ok(Self {
            next_id,
            pending,
            capabilities,
            stdin,
            child,
            alive,
            _read_thread: Some(read_thread),
        })
    }

    // ── document sync (fire-and-forget) ──────────────────────────

    /// 通知服务器文档已打开。editor 打开文件时调用。
    ///
    /// 不阻塞——消息写入 stdin 后立即返回。
    pub fn did_open(
        &self,
        uri: Uri,
        text: &str,
        language_id: &str,
        version: i32,
    ) -> Result<(), LspError> {
        let params = serde_json::json!({
            "textDocument": {
                "uri": uri,
                "languageId": language_id,
                "version": version,
                "text": text,
            }
        });
        self.notify("textDocument/didOpen", params)
    }

    /// 通知服务器文档内容已变更。editor 每次编辑后调用。
    ///
    /// 不阻塞——消息写入 stdin 后立即返回。
    pub fn did_change(
        &self,
        uri: Uri,
        version: i32,
        changes: &[lsp_types::TextDocumentContentChangeEvent],
    ) -> Result<(), LspError> {
        let params = serde_json::to_value(lsp_types::DidChangeTextDocumentParams {
            text_document: lsp_types::VersionedTextDocumentIdentifier { uri, version },
            content_changes: changes.to_vec(),
        })
        .map_err(|e| LspError::ProtocolViolation {
            detail: format!("序列化 DidChangeTextDocumentParams 失败：{e}"),
        })?;
        self.notify("textDocument/didChange", params)
    }

    /// 通知服务器文档已关闭。editor 关闭文件时调用。
    ///
    /// 不阻塞——消息写入 stdin 后立即返回。
    pub fn did_close(&self, uri: Uri) -> Result<(), LspError> {
        let params = serde_json::to_value(lsp_types::DidCloseTextDocumentParams {
            text_document: lsp_types::TextDocumentIdentifier { uri },
        })
        .map_err(|e| LspError::ProtocolViolation {
            detail: format!("序列化 DidCloseTextDocumentParams 失败：{e}"),
        })?;
        self.notify("textDocument/didClose", params)
    }

    // ── accessors ─────────────────────────────────────────────────

    /// 服务端能力集——上层据此判断哪些 LSP 能力可用。
    pub fn capabilities(&self) -> &ServerCapabilities {
        &self.capabilities
    }

    /// 判断 server 是否支持 semantic tokens。
    pub fn has_semantic_tokens(&self) -> bool {
        self.capabilities.semantic_tokens_provider.is_some()
    }

    /// 语义 token 类型与修饰符的 legend——解码 `semanticTokens/full` 响应时所需。
    pub fn semantic_tokens_legend(&self) -> Option<lsp_types::SemanticTokensLegend> {
        match &self.capabilities.semantic_tokens_provider {
            Some(lsp_types::SemanticTokensServerCapabilities::SemanticTokensOptions(opts)) => {
                Some(opts.legend.clone())
            }
            _ => None,
        }
    }

    /// 请求全量 semantic tokens。返回 mpsc receiver，调用方应在 pump 中 try_recv 消费。
    pub fn request_semantic_tokens(
        &self,
        uri: Uri,
    ) -> Result<std::sync::mpsc::Receiver<Result<Value, LspError>>, LspError> {
        let params = serde_json::json!({
            "textDocument": { "uri": uri }
        });
        self.send_request("textDocument/semanticTokens/full", params)
    }

    // ── internals ─────────────────────────────────────────────────

    /// 发送一条通知（无 id，不期待响应）。
    fn notify(&self, method: &str, params: Value) -> Result<(), LspError> {
        let json = JsonRpcNotification {
            jsonrpc: "2.0",
            method,
            params,
        }
        .to_json()?;
        let mut stdin = self.stdin.lock().map_err(|_| LspError::ChannelClosed)?;
        transport::write_message(&mut stdin, &json)
    }

    /// 发送一条请求并以 mpsc channel 异步等待响应。
    ///
    /// 返回值是 `mpsc::Receiver`——调用方应尽快 `recv` 或 drop 它，
    /// 否则读线程在 `sender.send()` 时会阻塞（unbuffered channel）。
    pub(crate) fn send_request(
        &self,
        method: &str,
        params: Value,
    ) -> Result<std::sync::mpsc::Receiver<Result<Value, LspError>>, LspError> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = std::sync::mpsc::channel();

        self.pending
            .lock()
            .map_err(|_| LspError::ChannelClosed)?
            .insert(id, tx);

        let req = JsonRpcRequest {
            jsonrpc: "2.0",
            id,
            method,
            params,
        };
        let json = req.to_json()?;

        // 在获取 stdin 锁之前删除挂起锁以避免死锁
        let mut stdin = self.stdin.lock().map_err(|_| LspError::ChannelClosed)?;
        let result = transport::write_message(&mut stdin, &json);
        if result.is_err() {
            // 写失败时清理 pending entry
            if let Ok(mut pending) = self.pending.lock() {
                pending.remove(&id);
            }
        }
        result?;
        Ok(rx)
    }

    /// 优雅关闭：发 `shutdown` 请求 + `exit` 通知，等子进程退出后 join 读线程。
    pub fn shutdown(mut self, timeout_ms: u64) {
        // 发 shutdown 请求后立刻 drop rx——不等待响应。
        // 避免读线程在 `sender.send()` 时阻塞（mpsc channel 无缓冲）。
        if let Ok(rx) = self.send_request("shutdown", serde_json::json!({})) {
            drop(rx);
        }
        let _ = self.notify("exit", serde_json::json!({}));

        // 标记停止，让读线程在下一个循环迭代退出
        self.alive.store(false, Ordering::Relaxed);

        // 先确保子进程终止——关闭 stdout 管道让读线程读到 EOF。
        if let Ok(mut child_opt) = self.child.lock()
            && let Some(mut child) = child_opt.take()
        {
            let start = std::time::Instant::now();
            while start.elapsed() < std::time::Duration::from_millis(timeout_ms) {
                match child.try_wait() {
                    Ok(Some(_)) => break,
                    Ok(None) => {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                    }
                    Err(_) => {
                        let _ = child.kill();
                        break;
                    }
                }
            }
            if start.elapsed() >= std::time::Duration::from_millis(timeout_ms) {
                let _ = child.kill();
            }
        }

        // 子进程已退出，stdout 管道已关闭，读线程读到 EOF 后退出——join 它。
        if let Some(handle) = self._read_thread.take() {
            let _ = handle.join();
        }
    }
}

// ── initialize handshake ────────────────────────────────────────

/// `initialize` + `initialized` 同步握手。
/// 此时还在单线程模式，直接用 transport 收发。
fn initialize_handshake(
    transport: &mut StdioTransport,
    root_uri: Uri,
) -> Result<ServerCapabilities, LspError> {
    let params = serde_json::json!({
        "processId": std::process::id(),
        "rootUri": root_uri,
        "capabilities": {
            "textDocument": {
                "semanticTokens": {
                    "requests": {
                        "full": true
                    },
                    "tokenTypes": [],
                    "tokenModifiers": [],
                    "formats": ["relative"]
                }
            }
        }
    });

    let response = send_request_sync(transport, "initialize", params, 0)?;

    let result: InitializeResult =
        serde_json::from_value(response).map_err(|e| LspError::ProtocolViolation {
            detail: format!("解析 InitializeResult 失败：{e}"),
        })?;

    send_notification_sync(transport, "initialized", serde_json::json!({}))?;

    Ok(result.capabilities)
}

// ── read loop ───────────────────────────────────────────────────

/// 后台读线程的主循环：阻塞读取 stdout，按 JSON-RPC 语义分发。
fn read_loop(
    reader: &mut BufReader<ChildStdout>,
    pending: &Arc<Mutex<HashMap<u64, PendingSender>>>,
    notification_handler: &Arc<Mutex<Option<Box<dyn NotificationHandler>>>>,
    alive: &AtomicBool,
) {
    while alive.load(Ordering::Relaxed) {
        let raw = match transport::read_message(reader) {
            Ok(Some(msg)) => msg,
            Ok(None) => break, // EOF
            Err(e) => {
                eprintln!("[LSP] 读消息失败：{e}");
                continue;
            }
        };

        let message: IncomingMessage = match serde_json::from_str(&raw.body) {
            Ok(m) => m,
            Err(e) => {
                eprintln!("[LSP] JSON 反序列化失败：{e}");
                continue;
            }
        };

        if let Some(id) = message.id {
            // Response：按 id 匹配 pending sender
            if let Ok(mut pending) = pending.lock()
                && let Some(sender) = pending.remove(&id)
            {
                let result = if let Some(err) = message.error {
                    Err(LspError::ServerError {
                        code: err.code,
                        message: err.message,
                    })
                } else {
                    message.result.ok_or(LspError::ProtocolViolation {
                        detail: "响应缺少 result".to_string(),
                    })
                };
                // send 失败 = 调用方已 drop rx（正常情况）
                let _ = sender.send(result);
            }
        } else if let Some(method) = message.method {
            // Notification：交给上层回调
            if let Ok(handler) = notification_handler.lock()
                && let Some(ref h) = *handler
            {
                h.on_notification(&method, message.params.unwrap_or(Value::Null));
            }
        }
        // else: server→client request（少见的反向请求）——MVP 忽略
    }
}

// ── sync helpers（仅 initialize 阶段使用）───────────────────────

fn send_request_sync(
    transport: &mut StdioTransport,
    method: &str,
    params: Value,
    id: u64,
) -> Result<Value, LspError> {
    let req = JsonRpcRequest {
        jsonrpc: "2.0",
        id,
        method,
        params,
    };
    let json = req.to_json()?;
    transport.send(&json)?;

    loop {
        let raw = transport.recv()?.ok_or(LspError::ChannelClosed)?;
        let resp: IncomingMessage =
            serde_json::from_str(&raw.body).map_err(|e| LspError::ProtocolViolation {
                detail: format!("解析响应失败：{e}"),
            })?;

        if resp.id != Some(id) {
            continue;
        }

        if let Some(err) = resp.error {
            return Err(LspError::ServerError {
                code: err.code,
                message: err.message,
            });
        }

        return resp.result.ok_or(LspError::ProtocolViolation {
            detail: "响应缺少 result".to_string(),
        });
    }
}

fn send_notification_sync(
    transport: &mut StdioTransport,
    method: &str,
    params: Value,
) -> Result<(), LspError> {
    let json = JsonRpcNotification {
        jsonrpc: "2.0",
        method,
        params,
    }
    .to_json()?;
    transport.send(&json)
}
