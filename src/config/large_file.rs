//! 大文件降级策略：只表达引擎实际会读取的阈值。
//!
//! 文件体积、长行、外部分析提示等阈值字段不在此承诺；待引擎真正在某个路径上
//! 强制执行时再添加。

/// 大文件与降级策略。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LargeFilePolicy {
    /// 最大允许保留的 Undo 历史节点数。
    pub max_undo_history: usize,
}

impl Default for LargeFilePolicy {
    fn default() -> Self {
        Self {
            max_undo_history: 1000,
        }
    }
}
