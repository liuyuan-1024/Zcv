//! LSP 传输层：stdio 子进程通信。
//!
//! LSP 协议基于 JSON-RPC 2.0，消息格式为：
//! ```text
//! Content-Length: <byte_count>\r\n
//! \r\n
//! <json_body>
//! ```
//!
//! 本模块负责消息的**收发编解码**——启动子进程、读写 stdin/stdout、组帧/解帧。

use std::io::{BufRead, BufReader, Read, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use crate::error::{ExitStatus, LspError};

/// 一条从服务器收到的原始消息（已解帧的 JSON 字符串）。
#[derive(Debug)]
pub(crate) struct RawMessage {
    pub body: String,
}

/// stdio 传输：持有一个语言服务器子进程，通过 stdin/stdout 做 JSON-RPC 通信。
///
/// 约 16 MiB/s 的序列化竞品吞吐，超出此量请用 IPC / socket 方案。
pub(crate) struct StdioTransport {
    child: Child,
    reader: BufReader<ChildStdout>,
}

impl StdioTransport {
    /// 启动 `command` 进程，参数为 `args`。进程启动后即可收发。
    pub fn spawn(command: &str, args: &[String]) -> Result<Self, LspError> {
        let mut child = Command::new(command)
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .map_err(|e| {
                if e.kind() == std::io::ErrorKind::NotFound {
                    LspError::ServerNotFound {
                        command: command.to_string(),
                    }
                } else {
                    LspError::ServerStartFailed {
                        command: command.to_string(),
                        reason: e.to_string(),
                    }
                }
            })?;

        let stdout = child.stdout.take().expect("stdout 已 pipe");
        let reader = BufReader::new(stdout);

        Ok(Self { child, reader })
    }

    /// 发送一条 JSON 消息到服务器 stdin。
    pub fn send(&mut self, json: &str) -> Result<(), LspError> {
        let stdin = self.child.stdin.as_mut().expect("stdin 已 pipe");
        write_message(stdin, json)
    }

    /// 阻塞读取下一条消息。
    ///
    /// 返回 `None` 表示 stdout 已关闭（服务器退出）。
    pub fn recv(&mut self) -> Result<Option<RawMessage>, LspError> {
        read_message(&mut self.reader)
    }

    /// 检查进程是否已退出。返回退出状态。
    #[allow(dead_code)]
    pub fn try_wait(&mut self) -> Option<ExitStatus> {
        match self.child.try_wait() {
            Ok(Some(status)) => {
                if let Some(code) = status.code() {
                    Some(ExitStatus::Code(code))
                } else {
                    Some(ExitStatus::Signal)
                }
            }
            Ok(None) => None,
            Err(_) => Some(ExitStatus::Signal),
        }
    }

    /// 优雅关闭：发 shutdown + exit 后等待进程退出。
    #[allow(dead_code)]
    pub fn shutdown(&mut self, timeout_ms: u64) {
        let _ = self.send(r#"{"jsonrpc":"2.0","method":"shutdown","id":0}"#);
        let _ = self.send(r#"{"jsonrpc":"2.0","method":"exit","id":0}"#);

        let start = std::time::Instant::now();
        while start.elapsed() < std::time::Duration::from_millis(timeout_ms) {
            if self.try_wait().is_some() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }

        let _ = self.child.kill();
    }

    /// 拆分为独立部件——供多线程使用。
    ///
    /// `stdin` 由写侧持有（主线程通过 `Arc<Mutex<>>` 共享），
    /// `stdout` 由后台读线程独占。
    pub fn into_parts(mut self) -> (ChildStdin, BufReader<ChildStdout>, Child) {
        (
            self.child.stdin.take().expect("stdin 已 pipe"),
            self.reader,
            self.child,
        )
    }
}

/// 从 reader 阻塞读取一条 LSP 消息。
///
/// 拆出来当 free function，让后台读线程可以在剥离 transport 后继续读消息。
pub(crate) fn read_message(
    reader: &mut BufReader<ChildStdout>,
) -> Result<Option<RawMessage>, LspError> {
    let mut header = String::new();
    loop {
        let mut line = String::new();
        let n = reader
            .read_line(&mut line)
            .map_err(|_| LspError::ChannelClosed)?;
        if n == 0 {
            return Ok(None);
        }
        if line.trim().is_empty() {
            break;
        }
        header.push_str(&line);
    }

    let content_length = parse_content_length(&header).ok_or(LspError::ProtocolViolation {
        detail: format!("缺少 Content-Length 头：{header:?}"),
    })?;

    let mut body = vec![0u8; content_length];
    reader
        .read_exact(&mut body)
        .map_err(|_| LspError::ChannelClosed)?;

    let body = String::from_utf8(body).map_err(|e| LspError::ProtocolViolation {
        detail: format!("消息体不是合法 UTF-8：{e}"),
    })?;

    Ok(Some(RawMessage { body }))
}

/// 把一条 JSON 消息写入 stdin。
pub(crate) fn write_message(stdin: &mut ChildStdin, json: &str) -> Result<(), LspError> {
    let header = format!("Content-Length: {}\r\n\r\n", json.len());
    stdin
        .write_all(header.as_bytes())
        .map_err(|_| LspError::ChannelClosed)?;
    stdin
        .write_all(json.as_bytes())
        .map_err(|_| LspError::ChannelClosed)?;
    stdin.flush().map_err(|_| LspError::ChannelClosed)
}

fn parse_content_length(header: &str) -> Option<usize> {
    for line in header.lines() {
        if let Some(value) = line.trim().strip_prefix("Content-Length:") {
            return value.trim().parse().ok();
        }
    }
    None
}
