use zcv_text::{Buffer, BufferConfig, ByteOffset, TransactionMetadata};

fn text(buffer: &Buffer) -> String {
    let snapshot = buffer.snapshot();
    snapshot
        .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
        .unwrap()
        .as_str()
        .to_owned()
}

fn format(input: &str) -> String {
    let mut buffer = Buffer::from_text(input.to_owned(), BufferConfig::default()).unwrap();
    let trailing = buffer.snapshot().trailing_whitespace_edits().unwrap();
    if !trailing.is_empty() {
        buffer
            .edit(trailing, TransactionMetadata::default())
            .unwrap();
    }
    if let Some(edit) = buffer.snapshot().final_newline_edit().unwrap() {
        buffer.edit([edit], TransactionMetadata::default()).unwrap();
    }
    text(&buffer)
}

#[test]
fn save_format_cleans_line_ends_and_collapses_final_ascii_whitespace() {
    for (input, expected) in [
        ("", ""),
        ("a", "a\n"),
        ("a\n", "a\n"),
        ("a  \n\tb  \n  \n", "a\n\tb\n"),
        (" \t\n \t", "\n"),
        ("中\u{3000}\n", "中\u{3000}\n"),
        ("a\u{000b}", "a\u{000b}\n"),
    ] {
        assert_eq!(format(input), expected, "输入 {input:?}");
    }
}

#[test]
fn each_save_format_rule_can_be_applied_independently() {
    let mut buffer = Buffer::from_text("a  \n\n".to_owned(), BufferConfig::default()).unwrap();
    let trailing = buffer.snapshot().trailing_whitespace_edits().unwrap();
    buffer
        .edit(trailing, TransactionMetadata::default())
        .unwrap();
    assert_eq!(text(&buffer), "a\n\n");

    let mut buffer = Buffer::from_text("a  \n\n".to_owned(), BufferConfig::default()).unwrap();
    let final_edit = buffer.snapshot().final_newline_edit().unwrap().unwrap();
    buffer
        .edit([final_edit], TransactionMetadata::default())
        .unwrap();
    assert_eq!(text(&buffer), "a\n");
}

#[test]
fn trailing_whitespace_spanning_rope_chunks_is_one_valid_edit() {
    let input = format!("a{}\n", " ".repeat(4096));
    let buffer = Buffer::from_text(input, BufferConfig::default()).unwrap();
    let edits = buffer.snapshot().trailing_whitespace_edits().unwrap();
    assert_eq!(edits.len(), 1);
    assert_eq!(edits[0].range().start(), ByteOffset::new(1));
    assert_eq!(edits[0].range().end(), ByteOffset::new(4097));
}
