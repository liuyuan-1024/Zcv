//! 大文件降级策略：只表达引擎实际会读取的阈值。
//!
//! 文件体积、长行、外部分析提示等阈值字段不在此承诺；待引擎真正在某个路径上
//! 强制执行时再添加。

/// 大文件与降级策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LargeFilePolicy {
    /// 最大允许保留的 Undo 历史节点数。
    pub max_undo_history: usize,
    /// 历史保留的最大累积字节数（含 undo + redo 的 replacement 文本）；`0` 表示不限。
    ///
    /// 超出预算时，截断按节点序号从最老的非 current 叶子开始丢弃，直到 ≤ 预算
    /// 或没有可丢弃叶子。current 节点永不被丢弃。
    pub max_undo_history_bytes: usize,
    /// 单事务允许进入历史的最大字节数；`0` 表示不限。
    ///
    /// 超过阈值时按 `large_transaction_policy` 处理。
    pub large_transaction_threshold_bytes: usize,
    /// 超过 `large_transaction_threshold_bytes` 时的处理策略。
    pub large_transaction_policy: LargeTransactionPolicy,
    /// 文本字节数大于此阈值的 Buffer 视为大文件；`0` 表示不限。
    ///
    /// 引擎本身不拒绝大文件加载，只把判断结果暴露给 `Buffer::is_large_file()` /
    /// `LoadedTextInfo::is_large`，并按 `auto_read_only_on_large_file` 决定是否
    /// 在加载 / reload 时切到只读。
    pub large_file_threshold_bytes: usize,
    /// 任意单行字符数超此阈值的 Buffer 视为含超长行；`0` 表示不限。
    ///
    /// 引擎不拒绝超长行，只在加载时通过 `LoadedTextInfo::has_long_line` 暴露事实，
    /// 宿主自行决定是否禁用 high-cost 能力（如行级 fold、UI 视觉列重排等）。
    pub long_line_threshold_chars: usize,
    /// 超过 `large_file_threshold_bytes` 的 Buffer 在加载 / reload 时是否自动
    /// 切到只读。默认 `false`：仅暴露事实，行为由宿主控制。
    pub auto_read_only_on_large_file: bool,
}

/// 单事务字节超过 `large_transaction_threshold_bytes` 时的处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LargeTransactionPolicy {
    /// 默认：仍提交文本，但不进入历史（同 `record_history=false` 路径，
    /// 丢弃当前节点子树）。引擎不替宿主决定是否拒绝整事务。
    #[default]
    SkipHistory,
    /// 在事务管线内原子拒绝；返回 `EditError::PayloadTooLarge { size, limit }`，
    /// Buffer 文本 / 版本 / dirty / 历史完全不变。
    Reject,
}

impl Default for LargeFilePolicy {
    fn default() -> Self {
        Self {
            max_undo_history: 1000,
            max_undo_history_bytes: 64 * 1024 * 1024,
            large_transaction_threshold_bytes: 16 * 1024 * 1024,
            large_transaction_policy: LargeTransactionPolicy::SkipHistory,
            large_file_threshold_bytes: 5 * 1024 * 1024,
            long_line_threshold_chars: 10_000,
            auto_read_only_on_large_file: false,
        }
    }
}

impl LargeFilePolicy {
    /// 判断 `byte_size` 是否被视为大文件；`large_file_threshold_bytes == 0` 表示不限。
    pub fn is_large_byte_size(&self, byte_size: usize) -> bool {
        self.large_file_threshold_bytes != 0 && byte_size > self.large_file_threshold_bytes
    }

    /// 判断 `chars` 是否被视为超长行；`long_line_threshold_chars == 0` 表示不限。
    pub fn is_long_line(&self, chars: usize) -> bool {
        self.long_line_threshold_chars != 0 && chars > self.long_line_threshold_chars
    }
}
