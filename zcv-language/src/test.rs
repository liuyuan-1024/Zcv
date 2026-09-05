use std::path::Path;

use crate::syntax_map::SyntaxMap;
use crate::tree_sitter_utils::ParseCancellation;

/// 按 Rust 文件解析文本，返回 Buffer 与已安装解析结果的语法映射。
pub(crate) fn rust_buffer(text: &str) -> (zcv_text::Buffer, SyntaxMap) {
    parsed_syntax("main.rs", text)
}

/// 按给定路径解析文本，返回 Buffer 与已安装解析结果的语法映射。
pub(crate) fn parsed_syntax(path: &str, text: &str) -> (zcv_text::Buffer, SyntaxMap) {
    let buffer =
        zcv_text::Buffer::from_text(text.to_owned(), zcv_text::BufferConfig::default()).unwrap();
    let snapshot = buffer.snapshot();
    let mut syntax = SyntaxMap::new(&snapshot);
    let first_line = snapshot
        .slice_line(zcv_text::Line::ZERO)
        .unwrap()
        .as_str()
        .trim_end_matches(['\r', '\n'])
        .to_owned();
    syntax.set_language_for_file(Path::new(path), Some(&first_line), &snapshot);
    let parsed = syntax
        .snapshot()
        .reparse(&snapshot, None, &ParseCancellation::default())
        .expect("测试解析不应取消");
    assert!(syntax.did_parse(parsed));
    (buffer, syntax)
}
