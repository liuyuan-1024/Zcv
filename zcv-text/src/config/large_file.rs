//! 大文件降级策略：只表达文本内核实际会读取的阈值。

/// 大文件与降级策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LargeFilePolicy {
    /// 最大保留的 Undo 历史节点数（会话算一个节点）；`0` 表示禁用 Undo / Redo。
    pub max_undo_history: usize,
    /// 编辑日志保留的最大版本条目数。
    ///
    /// 独立于 Undo 深度命名：即使禁用 Undo，增量同步窗口仍可由本预算维持。
    pub max_edit_history_entries: usize,
    /// 编辑日志保留的最大累积字节数（含 forward 与 undo 的 replacement 文本）；`0` 表示不限。
    pub max_edit_history_bytes: usize,
    /// 单事务允许进入历史的最大字节数；`0` 表示不限。
    ///
    /// 超过阈值时按 `large_transaction_policy` 处理。
    pub large_transaction_threshold_bytes: usize,
    /// 超过 `large_transaction_threshold_bytes` 时的处理策略。
    pub large_transaction_policy: LargeTransactionPolicy,
    /// 文本字节数大于此阈值的 Buffer 视为大文件；`0` 表示不限。
    ///
    /// 文本内核本身不拒绝大文件加载，只把判断结果暴露给 `Buffer::is_large_file()`，并按 `auto_read_only_on_large_file` 决定是否在加载 / 外部重置时切到只读。
    pub large_file_threshold_bytes: usize,
    /// 超过 `large_file_threshold_bytes` 的 Buffer 在加载 / 外部重置时是否自动切到只读。
    /// 默认 `false`：仅暴露事实，行为由宿主控制。
    pub auto_read_only_on_large_file: bool,
}

/// 单事务字节超过 `large_transaction_threshold_bytes` 时的处理策略。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LargeTransactionPolicy {
    /// 默认：仍提交文本，但不进入历史（同 `record_history=false` 路径，丢弃当前节点子树）。文本内核不替宿主决定是否拒绝整事务。
    #[default]
    SkipHistory,
    /// 在事务管线内原子拒绝；返回 `EditError::PayloadTooLarge { size, limit }`，Buffer 文本 / 版本 / dirty / 历史完全不变。
    Reject,
}

impl Default for LargeFilePolicy {
    fn default() -> Self {
        Self {
            max_undo_history: 1000,
            max_edit_history_entries: 1000,
            max_edit_history_bytes: 64 * 1024 * 1024,
            large_transaction_threshold_bytes: 16 * 1024 * 1024,
            large_transaction_policy: LargeTransactionPolicy::SkipHistory,
            large_file_threshold_bytes: 5 * 1024 * 1024,
            auto_read_only_on_large_file: false,
        }
    }
}

impl LargeFilePolicy {
    /// `large_file_threshold_bytes == 0` 表示不限。
    pub fn is_large_byte_size(&self, byte_size: usize) -> bool {
        self.large_file_threshold_bytes != 0 && byte_size > self.large_file_threshold_bytes
    }
}
