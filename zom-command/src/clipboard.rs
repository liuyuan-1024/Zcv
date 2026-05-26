//! 剪贴板端口：handler 与系统剪贴板之间的同步抽象。
//!
//! engine 不持有剪贴板状态——「粘贴」属于宿主词汇表，详见 zom-engine
//! `TransactionSource` 头注。zom-command 自身也不直接依赖 GPUI，剪贴板交互
//! 都走这个 trait：宿主在派发命令前往 [`crate::CommandContext`] 注入一个
//! 具体实现（zom-desktop 的 GPUI 适配器，或测试用的 [`MockClipboard`]）。
//!
//! 端口故意只暴露 `&str` / `String`：剪贴板层不携带"行模式"等额外语义；
//! 复制到什么字符串，粘贴就插入什么字符串。"空选区复制整行"是 handler 的
//! UX 规则，跟端口无关。

/// 剪贴板端口：handler 在派发同一帧内同步读 / 写剪贴板。
///
/// 实现者负责跟底层剪贴板交互；handler 只看到字符串。
pub trait ClipboardPort {
    /// 把 `text` 写入剪贴板，覆盖原有内容。
    fn write(&mut self, text: &str);

    /// 读取剪贴板当前内容。无内容（首次启动 / 系统拒绝 / 非文本）返回 `None`。
    fn read(&self) -> Option<String>;
}

/// 测试用内存剪贴板。
#[derive(Clone, Debug, Default)]
pub struct MockClipboard {
    contents: Option<String>,
}

impl MockClipboard {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_contents(text: impl Into<String>) -> Self {
        Self {
            contents: Some(text.into()),
        }
    }

    pub fn contents(&self) -> Option<&str> {
        self.contents.as_deref()
    }
}

impl ClipboardPort for MockClipboard {
    fn write(&mut self, text: &str) {
        self.contents = Some(text.to_string());
    }

    fn read(&self) -> Option<String> {
        self.contents.clone()
    }
}
