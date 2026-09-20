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
