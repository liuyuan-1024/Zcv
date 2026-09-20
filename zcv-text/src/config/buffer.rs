//! BufferConfig 聚合层：把独立策略组合成创建 Buffer 时的一组默认行为。
//!
//! 本文件只做配置拼装和默认值，不把策略应用到文本，也不读取宿主环境。

use super::LargeFilePolicy;

/// Buffer 级别的综合配置。
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct BufferConfig {
    /// 大文件、长行和历史保留相关的降级阈值。
    pub large_file: LargeFilePolicy,
}

#[cfg(test)]
#[path = "test/buffer_tests.rs"]
mod tests;
