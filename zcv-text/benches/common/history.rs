use zcv_text::{Buffer, BufferConfig, ByteOffset, Edit, TransactionMetadata};

pub(super) fn buffer_with_history_steps(count: usize) -> Buffer {
    let mut config = BufferConfig::default();
    config.large_file.max_edit_history_entries = usize::MAX;
    config.large_file.max_edit_history_bytes = 0;
    let mut buffer = Buffer::from_text(String::new(), config).unwrap();
    let metadata = TransactionMetadata::default().without_history();
    for offset in 0..count {
        buffer
            .edit(
                [Edit::insert(ByteOffset::new(offset), "x").unwrap()],
                metadata.clone(),
            )
            .unwrap();
    }
    buffer
}
