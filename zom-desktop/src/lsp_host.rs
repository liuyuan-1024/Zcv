//! LSP 主机：管理语言服务器实例池，按语言 id 路由文档同步，每帧推进后台状态。
//!
//! ## 职责
//!
//! - 按 project root 启动/停止 language server（同步 + 后台线程）
//! - 按文件语言 id 路由 `did_open` / `did_change` / `did_close`
//! - 消费 server → client 通知（diagnostics 等），写入共享状态供 UI 读取
//! - 每帧推进：收割后台 server 启动结果 → semantic tokens 响应 → 文档同步 → 请求新 tokens
//!
//! ## 线程安全
//!
//! `LspHost` 本身在 App 主线程上运行（`&mut self` 方法），
//! 但 `LspClient` 内部的写操作是 fire-and-forget 的非阻塞 channel send，通知回调在后台读线程上执行。
//! 因此诊断数据用 `Arc<Mutex<>>` 保护。

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};

use lsp_types::Uri;
use serde_json::Value;
use zom_engine::ByteOffset;
use zom_lsp::{LspClient, LspError, NotificationHandler};

use zom_workspace::syntax::LanguageId;
use zom_workspace::{BufferId, Workspace};

/// 一个文件的 LSP 诊断集合。
/// 预留给诊断面板展示使用，当前通过 diagnostics_handle 对外暴露。
#[derive(Clone, Debug, Default)]
#[allow(dead_code)]
pub struct FileDiagnostics {
    pub diagnostics: Vec<lsp_types::Diagnostic>,
}

/// LSP 主机状态快照——供 UI 只读查询。
/// 预留给语言服务器浮面使用，当前 snapshot() 已构造但消费方尚未接入。
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct LspHostSnapshot {
    /// 每个已连接的语言服务器及其状态。
    pub servers: Vec<ServerStatus>,
}

/// 单个语言服务器的连接状态。
/// 预留给语言服务器浮面使用。
#[derive(Clone, Debug)]
#[allow(dead_code)]
pub struct ServerStatus {
    pub language_id: LanguageId,
    pub command: String,
    pub connected: bool,
    pub has_semantic_tokens: bool,
}

/// LSP 主机：按需启动 language server，路由文档同步，收集诊断，每帧推进后台状态。
pub struct LspHost {
    clients: HashMap<LanguageId, LspClient>,
    diagnostics: Arc<Mutex<HashMap<Uri, FileDiagnostics>>>,
    project_root: Option<std::path::PathBuf>,
    /// 已通过 did_open 通知过 LSP server 的 buffer。
    lsp_opened: HashSet<BufferId>,
    /// buffer_id → 上次 did_change 时的 buffer version（u64 from BufferVersion::get）。
    lsp_sent_version: HashMap<BufferId, u64>,
    /// 飞行中的 semantic tokens 请求：buffer_id → mpsc receiver。
    lsp_pending: HashMap<BufferId, mpsc::Receiver<Result<Value, LspError>>>,
    /// 无已知 LSP server 命令映射的语言——`default_server_command` 返回 None。
    lsp_none_mapped: HashSet<LanguageId>,
    /// server 启动失败的语言——后续可加重试逻辑（当前不重试，行为不变）。
    lsp_launch_failed: HashSet<LanguageId>,
    /// 后台启动中的 server：language_id → 共享结果槽。
    lsp_starting: HashMap<LanguageId, Arc<Mutex<Option<Result<LspClient, LspError>>>>>,
}

impl LspHost {
    /// 新建一个未连接任何 server 的主机。
    pub fn new() -> Self {
        Self {
            clients: HashMap::new(),
            diagnostics: Arc::new(Mutex::new(HashMap::new())),
            project_root: None,
            lsp_opened: HashSet::new(),
            lsp_sent_version: HashMap::new(),
            lsp_pending: HashMap::new(),
            lsp_none_mapped: HashSet::new(),
            lsp_launch_failed: HashSet::new(),
            lsp_starting: HashMap::new(),
        }
    }

    /// 当 project root 变更时调用——关闭所有现有 server，清空全部追踪状态。
    pub fn set_project_root(&mut self, root: Option<&Path>) {
        self.shutdown_all(5000);
        self.project_root = root.map(|p| p.to_path_buf());
        self.lsp_opened.clear();
        self.lsp_sent_version.clear();
        self.lsp_pending.clear();
        self.lsp_none_mapped.clear();
        self.lsp_launch_failed.clear();
        self.lsp_starting.clear();
        if let Ok(mut diags) = self.diagnostics.lock() {
            diags.clear();
        }
    }

    /// 诊断数据的共享句柄——UI 侧在 frame pump 中轮询读取。
    pub fn diagnostics_handle(&self) -> Arc<Mutex<HashMap<Uri, FileDiagnostics>>> {
        Arc::clone(&self.diagnostics)
    }

    /// 主机状态快照——供语言服务器浮面等 UI 消费。
    /// 预留给语言服务器浮面使用，当前已构造但消费方尚未接入。
    #[allow(dead_code)]
    pub fn snapshot(&self) -> LspHostSnapshot {
        let servers = self
            .clients
            .iter()
            .map(|(lang_id, client)| ServerStatus {
                language_id: *lang_id,
                command: default_server_command(*lang_id)
                    .map(|(cmd, _)| cmd.to_string())
                    .unwrap_or_else(|| "-".to_string()),
                connected: true,
                has_semantic_tokens: client.has_semantic_tokens(),
            })
            .collect();
        LspHostSnapshot { servers }
    }

    /// 手动插入一个已启动的 client——供异步启动路径使用。
    pub fn insert_client(&mut self, language_id: LanguageId, client: LspClient) {
        self.clients.insert(language_id, client);
    }

    /// 是否有任何已连接的 server。
    /// 此方法预留给 UI 层（如语言服务器浮面）展示连接状态使用，当前尚未接入。
    #[allow(dead_code)]
    pub fn has_connected_servers(&self) -> bool {
        !self.clients.is_empty()
    }

    /// 返回 `language_id` 对应的已连接 client——用于发送异步请求。
    pub fn client_for(&self, language_id: LanguageId) -> Option<&LspClient> {
        self.clients.get(&language_id)
    }

    // ── frame pump ────────────────────────────────────────────────

    /// 每帧推进：收割后台 server 启动结果 → semantic tokens 响应 → 文档同步 → 请求新 tokens。
    pub fn pump(&mut self, workspace: &Workspace) {
        self.reap_started_servers();
        self.drain_semantic_tokens(workspace);
        self.sync_and_request(workspace);
    }

    fn reap_started_servers(&mut self) {
        let mut completed: Vec<(LanguageId, Result<LspClient, LspError>)> = Vec::new();
        for (lang_id, slot) in &self.lsp_starting {
            if let Ok(mut guard) = slot.lock() {
                if let Some(result) = guard.take() {
                    completed.push((*lang_id, result));
                }
            }
        }
        for (lang_id, result) in completed {
            self.lsp_starting.remove(&lang_id);
            match result {
                Ok(client) => {
                    self.insert_client(lang_id, client);
                }
                Err(_) => {
                    self.lsp_launch_failed.insert(lang_id);
                }
            }
        }
    }

    fn drain_semantic_tokens(&mut self, workspace: &Workspace) {
        let mut completed: Vec<(BufferId, Value)> = Vec::new();
        let mut to_remove: Vec<BufferId> = Vec::new();
        for (id, rx) in self.lsp_pending.iter_mut() {
            match rx.try_recv() {
                Ok(Ok(value)) => {
                    completed.push((*id, value));
                    to_remove.push(*id);
                }
                Ok(Err(_)) => {
                    to_remove.push(*id);
                }
                Err(mpsc::TryRecvError::Empty) => {
                    // 仍在等待中，保留
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    to_remove.push(*id);
                }
            }
        }
        for id in to_remove {
            self.lsp_pending.remove(&id);
        }

        for (buffer_id, response) in completed {
            let Some(wb) = workspace.buffer(buffer_id) else {
                continue;
            };
            let Some(language) = wb.language() else {
                continue;
            };
            let Some(client) = self.client_for(language) else {
                continue;
            };
            let Some(legend) = client.semantic_tokens_legend() else {
                continue;
            };
            let Some(slot) = wb.highlights_slot() else {
                continue;
            };

            let data = response
                .get("data")
                .and_then(|d| d.as_array())
                .map(|arr| {
                    arr.iter()
                        .map(|v| v.as_u64().unwrap_or(0) as u32)
                        .collect::<Vec<u32>>()
                })
                .unwrap_or_default();

            let snapshot = wb.buffer().snapshot();
            let version = wb.buffer().version();
            let spans = zom_workspace::syntax::lsp_tokens::decode_semantic_tokens(
                &data, &legend, &snapshot,
            );
            slot.store_lsp(Arc::new(spans), version);
        }
    }

    fn sync_and_request(&mut self, workspace: &Workspace) {
        let project_root = self.project_root.clone();
        let buffers: Vec<(BufferId, Option<LanguageId>)> = workspace
            .buffers()
            .map(|(id, wb)| (id, wb.language()))
            .collect();

        for (buffer_id, language) in buffers {
            let Some(language) = language else {
                continue;
            };
            if self.lsp_none_mapped.contains(&language)
                || self.lsp_launch_failed.contains(&language)
            {
                continue;
            }
            let Some(wb) = workspace.buffer(buffer_id) else {
                continue;
            };

            let server_ready = self.client_for(language).is_some();
            let server_starting = self.lsp_starting.contains_key(&language);
            if !server_ready && !server_starting {
                self.spawn_server_background(language);
                continue;
            }
            if !server_ready {
                continue;
            }

            let text = {
                let snapshot = wb.buffer().snapshot();
                let len = snapshot.len_bytes();
                snapshot
                    .slice_byte_range(ByteOffset::ZERO, len)
                    .map(|s| s.into_text().into_owned())
                    .unwrap_or_default()
            };

            // did_open —— 首次同步文档到 server
            if !self.lsp_opened.contains(&buffer_id) {
                if let Some(path) = wb.path() {
                    let version = wb.buffer().version();
                    if self.did_open(path, language, &text, 1).is_ok() {
                        self.lsp_opened.insert(buffer_id);
                        self.lsp_sent_version.insert(buffer_id, version.get());
                    }
                }
                continue;
            }

            let supports_tokens = self
                .client_for(language)
                .map(|c| c.has_semantic_tokens())
                .unwrap_or(false);
            if !supports_tokens {
                continue;
            }

            // did_change (full-text sync)
            let cur_version = wb.buffer().version();
            let sent_version = self.lsp_sent_version.get(&buffer_id).copied().unwrap_or(0);
            if cur_version.get() > sent_version {
                if let Some(path) = wb.path() {
                    let _ = self.did_change(path, language, &text, cur_version.get() as i32);
                    self.lsp_sent_version.insert(buffer_id, cur_version.get());
                }
            }

            // Request new semantic tokens if stale
            let already_pending = self.lsp_pending.contains_key(&buffer_id);
            let needs_tokens = !already_pending
                && wb
                    .highlights_slot()
                    .and_then(|slot| slot.lsp_version())
                    .map(|v| v.get() < cur_version.get())
                    .unwrap_or(true);
            if needs_tokens && project_root.is_some() {
                if let Some(path) = wb.path() {
                    if let Some(uri) = path_to_uri(path) {
                        if let Some(client) = self.client_for(language) {
                            if let Ok(rx) = client.request_semantic_tokens(uri) {
                                self.lsp_pending.insert(buffer_id, rx);
                            }
                        }
                    }
                }
            }
        }
    }

    fn spawn_server_background(&mut self, language: LanguageId) {
        let Some(ref root) = self.project_root else {
            return;
        };
        let Some(root_uri) = file_uri(root) else {
            self.lsp_launch_failed.insert(language);
            return;
        };
        let Some((command, args)) = default_server_command(language) else {
            self.lsp_none_mapped.insert(language);
            return;
        };
        let command = command.to_string();
        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let diags = self.diagnostics_handle();
        let slot = Arc::new(Mutex::new(None));
        let slot_for_thread = Arc::clone(&slot);
        std::thread::spawn(move || {
            let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                let handler = Box::new(LspHostHandler { diagnostics: diags });
                LspClient::launch(&command, &args, root_uri, handler)
            }));
            let result = match result {
                Ok(inner) => inner,
                Err(e) => {
                    let msg = e
                        .downcast_ref::<&str>()
                        .map(|s| s.to_string())
                        .or_else(|| e.downcast_ref::<String>().cloned())
                        .unwrap_or_else(|| "unknown panic".to_string());
                    Err(LspError::ProtocolViolation {
                        detail: format!("thread panicked: {msg}"),
                    })
                }
            };
            if let Ok(mut guard) = slot_for_thread.lock() {
                *guard = Some(result);
            }
        });
        self.lsp_starting.insert(language, slot);
    }

    // ── document sync ────────────────────────────────────────────

    /// buffer 打开时调用：按 language id 路由 did_open。
    ///
    /// 如果该语言的 server 尚未启动，先用 [`Self::ensure_server`] 懒启动。
    /// 返回 `true` 表示已发送文档同步通知。
    pub fn did_open(
        &mut self,
        path: &Path,
        language_id: LanguageId,
        text: &str,
        version: i32,
    ) -> Result<(), LspError> {
        let Some(uri) = path_to_uri(path) else {
            return Ok(());
        };
        let lang_str = language_id_to_lsp(language_id);
        let Some(client) = self.ensure_server(language_id)? else {
            return Ok(());
        };
        client.did_open(uri, text, lang_str, version)
    }

    /// buffer 编辑后调用。`text` 是全文——MVP 走 full-document sync。
    pub fn did_change(
        &mut self,
        path: &Path,
        language_id: LanguageId,
        text: &str,
        version: i32,
    ) -> Result<(), LspError> {
        let Some(uri) = path_to_uri(path) else {
            return Ok(());
        };
        let Some(client) = self.clients.get_mut(&language_id) else {
            return Ok(());
        };
        client.did_change(
            uri,
            version,
            &[lsp_types::TextDocumentContentChangeEvent {
                range: None,
                range_length: None,
                text: text.to_string(),
            }],
        )
    }

    /// buffer 关闭时调用。
    /// 此方法预留给 LSP didClose 协议——当前 buffer 生命周期由 workspace 管理，
    /// 尚未接入关闭通知。保留以便后续完善协议完整性。
    #[allow(dead_code)]
    pub fn did_close(&mut self, path: &Path, language_id: LanguageId) -> Result<(), LspError> {
        let Some(uri) = path_to_uri(path) else {
            return Ok(());
        };
        let Some(client) = self.clients.get_mut(&language_id) else {
            return Ok(());
        };
        client.did_close(uri)
    }

    // ── server lifecycle ─────────────────────────────────────────

    /// 确保 `language_id` 对应的 server 已启动。
    /// server 已存在则直接返回；否则按内置映射查找命令并启动。
    fn ensure_server(
        &mut self,
        language_id: LanguageId,
    ) -> Result<Option<&mut LspClient>, LspError> {
        if self.clients.contains_key(&language_id) {
            return Ok(self.clients.get_mut(&language_id));
        }

        let Some((command, args)) = default_server_command(language_id) else {
            return Ok(None);
        };
        let root = match &self.project_root {
            Some(r) => r.clone(),
            None => return Ok(None),
        };

        let root_uri = file_uri(&root).ok_or_else(|| LspError::ChannelClosed)?;

        let diags = Arc::clone(&self.diagnostics);
        let handler = Box::new(LspHostHandler { diagnostics: diags });

        let args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
        let client = LspClient::launch(command, &args, root_uri, handler)?;
        self.clients.insert(language_id, client);
        Ok(self.clients.get_mut(&language_id))
    }

    /// 关闭所有已连接的 server。
    fn shutdown_all(&mut self, timeout_ms: u64) {
        let clients = std::mem::take(&mut self.clients);
        for (_, client) in clients {
            client.shutdown(timeout_ms);
        }
    }
}

impl Default for LspHost {
    fn default() -> Self {
        Self::new()
    }
}

impl Drop for LspHost {
    fn drop(&mut self) {
        self.shutdown_all(5000);
    }
}

// ── Notification handler ────────────────────────────────────────

/// `LspHost` 内部的 `NotificationHandler` 实现。
///
/// 在后台读线程上执行——只做 `Arc<Mutex<>>` 写入，不碰 UI 状态。
struct LspHostHandler {
    diagnostics: Arc<Mutex<HashMap<Uri, FileDiagnostics>>>,
}

impl NotificationHandler for LspHostHandler {
    fn on_notification(&self, method: &str, params: Value) {
        if method == "textDocument/publishDiagnostics" {
            let uri = match params.get("uri").and_then(|v| v.as_str()) {
                Some(u) => u.to_string(),
                None => return,
            };
            let parsed_uri: Uri = match uri.parse() {
                Ok(u) => u,
                Err(_) => return,
            };
            let diags: Vec<lsp_types::Diagnostic> = params
                .get("diagnostics")
                .and_then(|d| serde_json::from_value(d.clone()).ok())
                .unwrap_or_default();

            if let Ok(mut map) = self.diagnostics.lock() {
                map.insert(parsed_uri, FileDiagnostics { diagnostics: diags });
            }
        }
        // 后续扩展：window/logMessage, window/showMessage 等
    }
}

// ── built-in server table ───────────────────────────────────────

/// 内置语言服务器映射——根据 language id 返回 `(command, args)`。
/// 不在表里的语言不发生 LSP 启动。
fn default_server_command(id: LanguageId) -> Option<(&'static str, &'static [&'static str])> {
    match id.as_str() {
        "rust" => Some(("rust-analyzer", &[])),
        "typescript" => Some(("typescript-language-server", &["--stdio"])),
        "tsx" => Some(("typescript-language-server", &["--stdio"])),
        "javascript" => Some(("typescript-language-server", &["--stdio"])),
        "jsx" => Some(("typescript-language-server", &["--stdio"])),
        "python" => Some(("pyright-langserver", &["--stdio"])),
        "go" => Some(("gopls", &[])),
        "c" => Some(("clangd", &[])),
        "cpp" => Some(("clangd", &[])),
        "java" => Some(("jdtls", &[])),
        "html" => Some(("vscode-html-language-server", &["--stdio"])),
        "css" => Some(("vscode-css-language-server", &["--stdio"])),
        "json" => Some(("vscode-json-language-server", &["--stdio"])),
        "bash" => Some(("bash-language-server", &["start"])),
        "yaml" => Some(("yaml-language-server", &["--stdio"])),
        "toml" => Some(("taplo", &["lsp", "stdio"])),
        "markdown" => Some(("marksman", &[])),
        _ => None,
    }
}

// ── helpers ─────────────────────────────────────────────────────

fn path_to_uri(path: &Path) -> Option<Uri> {
    let absolute = std::fs::canonicalize(path).ok()?;
    format!("file://{}", absolute.display()).parse().ok()
}

fn file_uri(path: &Path) -> Option<Uri> {
    let absolute = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::fs::canonicalize(path).ok()?
    };
    format!("file://{}", absolute.display()).parse().ok()
}

/// zom 内部 LanguageId → LSP language identifier 映射。
fn language_id_to_lsp(id: LanguageId) -> &'static str {
    match id.as_str() {
        "tsx" => "typescriptreact",
        other => other,
    }
}
