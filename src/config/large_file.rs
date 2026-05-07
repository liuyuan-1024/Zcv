//! 大文件降级策略：集中表达可能影响历史、索引和外部分析成本的阈值。
//!
//! 当前阶段只暴露策略数据，不在这里触发降级，也不绑定具体宿主提示方式。

/// 大文件与降级策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LargeFilePolicy {
    /// 触发大文件降级策略的阈值，单位是字节。
    pub threshold_bytes: usize,
    /// 触发超长行降级策略的阈值，单位是字节。
    pub long_line_threshold_bytes: usize,
    /// 最大允许保留的 Undo 历史节点数。
    pub max_undo_history: usize,
    /// 是否启用高成本内部索引。
    pub enable_expensive_indices: bool,
    /// 是否允许向外部分析系统暴露“建议降级”的提示。
    pub allow_external_analysis_hints: bool,
}

impl Default for LargeFilePolicy {
    fn default() -> Self {
        Self {
            threshold_bytes: 5 * 1024 * 1024,
            long_line_threshold_bytes: 512 * 1024,
            max_undo_history: 1000,
            enable_expensive_indices: true,
            allow_external_analysis_hints: true,
        }
    }
}
