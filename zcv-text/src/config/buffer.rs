//! BufferConfig 聚合层：把独立策略组合成创建 Buffer 时的一组默认行为。
//!
//! 本文件只做配置拼装和默认值，不把策略应用到文本，也不读取宿主环境。

use super::LargeFilePolicy;

/// Buffer 级别的综合配置。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BufferConfig {
    /// 大文件、长行和历史保留相关的降级阈值。
    pub large_file: LargeFilePolicy,
}

impl Default for BufferConfig {
    fn default() -> Self {
        Self {
            large_file: LargeFilePolicy::default(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn config_strategy_values_should_expose_stable_defaults_and_boundaries() {
        let config = BufferConfig::default();
        let large = LargeFilePolicy {
            large_file_threshold_bytes: 8,
            ..LargeFilePolicy::default()
        };

        assert_eq!(config.large_file.max_edit_history_entries, 1000);
        assert!(large.is_large_byte_size(9));
        assert!(!large.is_large_byte_size(8));
    }
}
