//! 搜索面板的查询/结果转换 helper。
//!
//! 都是纯函数：把面板态（[`SearchOptions`]、用户输入的 query）转成 zom-engine
//! 能消费的查询参数；再把 engine 给出的命中范围转成展示信息（行号 / 列 / 预览）。

use zom_engine::{SearchOptions as EngineSearchOptions, TextRange};
use zom_workspace::WorkspaceBuffer;

use super::model::SearchOptions;

/// 把面板的字面量查询选项转成 engine 接受的选项。
pub(super) fn literal_search_options(options: SearchOptions) -> EngineSearchOptions {
    EngineSearchOptions::new()
        .with_case_sensitive(options.case_sensitive)
        .with_whole_word(options.whole_word)
}

/// 整词选项打开时，给 regex 模式两端套上 `\b`。
pub(super) fn regex_pattern(query: &str, whole_word: bool) -> String {
    if whole_word {
        format!(r"\b(?:{query})\b")
    } else {
        query.to_string()
    }
}

/// 由命中范围算出行号、列号与该行的预览文本。
pub(super) fn search_result_location(text: &str, range: TextRange) -> (usize, usize, String) {
    let start = range.start().get().min(text.len());
    let before = text.get(..start).unwrap_or("");
    let line = before.bytes().filter(|byte| *byte == b'\n').count() + 1;
    let line_start = before.rfind('\n').map(|index| index + 1).unwrap_or(0);
    let column = before[line_start..].chars().count() + 1;
    let line_end = text[start..]
        .find('\n')
        .map(|index| start + index)
        .unwrap_or(text.len());
    let preview_start = text[..start]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0);
    let preview = text[preview_start..line_end].trim().to_string();
    (line, column, preview)
}

/// 取 buffer 的展示标题：有路径用文件名，否则显示「未命名」。
pub(super) fn buffer_title(buffer: &WorkspaceBuffer) -> String {
    buffer
        .path()
        .and_then(|path| path.file_name())
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "未命名".to_string())
}
