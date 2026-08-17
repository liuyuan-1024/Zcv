//! TransactionMetadata：事务来源、历史合并和描述信息的受控组合。
//!
//! 字段保持私有，避免调用方自由拼装后续历史系统无法维护的不变量。
//!
//! **Zero-copy 纪律**：`description` 用 `Arc<str>`，事务热路径反复传递元数据时只递增引用计数。

use std::sync::Arc;

use super::TransactionSource;

/// 历史合并策略。
///
/// 完整 Smart Debounce 由宿主输入层基于时间窗口决定是否选择 `MergeWithPrevious`，
/// 引擎层只负责确定性地执行合并。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TransactionMergePolicy {
    /// 明确形成一个独立 Undo 步骤。
    #[default]
    Never,
    /// 与前一个历史节点合并为一个 Undo 步骤。
    MergeWithPrevious,
}

/// 事务元数据。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct TransactionMetadata {
    source: TransactionSource,
    merge_policy: TransactionMergePolicy,
    record_history: bool,
    description: Option<Arc<str>>,
}

impl TransactionMetadata {
    pub fn new(source: TransactionSource) -> Self {
        Self {
            source,
            ..Self::default()
        }
    }

    pub fn with_merge_policy(mut self, merge_policy: TransactionMergePolicy) -> Self {
        self.merge_policy = merge_policy;
        self
    }

    pub fn without_history(mut self) -> Self {
        self.record_history = false;
        self
    }

    pub fn with_description(mut self, description: impl Into<Arc<str>>) -> Self {
        self.description = Some(description.into());
        self
    }

    /// 返回 `Arc<str>` 引用以便复用同一份共享字符串。
    pub(crate) fn description_arc(&self) -> Option<&Arc<str>> {
        self.description.as_ref()
    }

    pub fn source(&self) -> TransactionSource {
        self.source
    }

    pub fn merge_policy(&self) -> TransactionMergePolicy {
        self.merge_policy
    }

    pub fn record_history(&self) -> bool {
        self.record_history
    }

    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }
}

impl Default for TransactionMetadata {
    fn default() -> Self {
        Self {
            source: TransactionSource::Programmatic,
            merge_policy: TransactionMergePolicy::Never,
            record_history: true,
            description: None,
        }
    }
}
