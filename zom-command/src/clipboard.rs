//! 剪贴板端口：handler 与系统剪贴板之间的同步抽象。
//!
//! engine 不持有剪贴板状态——「粘贴」属于宿主词汇表，详见 zom-engine `TransactionSource` 头注。
//! zom-command 自身也不直接依赖 GPUI。
//! 剪贴板交互都走这个 trait：宿主在派发命令前，往 [`crate::CommandContext`] 注入具体实现（zom-desktop 的 GPUI 适配器，或 headless 默认的 [`NoopClipboard`]）。
//!
//! 端口故意只暴露 `&str` / `String`：剪贴板层不携带"行模式"等额外语义；
//! 复制到什么字符串，粘贴就插入什么字符串。
//! "空选区复制整行"是 handler 的 UX 规则，跟端口无关。

/// 剪贴板端口：handler 在派发同一帧内同步读 / 写剪贴板。
///
/// 实现者负责跟底层剪贴板交互；handler 只看到字符串。
pub trait ClipboardPort {
    /// 把 `text` 写入剪贴板，覆盖原有内容。
    fn write(&mut self, text: &str);

    /// 读取剪贴板当前内容。无内容（首次启动 / 系统拒绝 / 非文本）返回 `None`。
    fn read(&self) -> Option<String>;
}

/// 不连接系统剪贴板的生产默认实现。
///
/// headless 运行时可以安全持有它；真正的宿主需要在组合根注入平台剪贴板适配器。
#[derive(Clone, Debug, Default)]
pub struct NoopClipboard;

impl NoopClipboard {
    pub fn new() -> Self {
        Self
    }
}

impl ClipboardPort for NoopClipboard {
    fn write(&mut self, _text: &str) {}

    fn read(&self) -> Option<String> {
        None
    }
}
