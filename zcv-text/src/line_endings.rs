//! Buffer 内部统一使用 LF；文件换行策略由文件边界决定。

use std::sync::Arc;

/// 把 CRLF 与孤立 CR 转为 LF；已是 LF 的文本不重新分配。
pub fn normalize_to_lf(text: String) -> String {
    normalized_to_lf(&text).unwrap_or(text)
}

/// 与 `normalize_to_lf` 相同，但保留已规范化修订文本的共享存储。
pub fn normalize_arc_to_lf(text: Arc<str>) -> Arc<str> {
    normalized_to_lf(&text).map_or(text, Arc::from)
}

/// 只在文本含有 CR 时分配新字符串，避免普通编辑复制替换文本。
pub(crate) fn normalized_to_lf(text: &str) -> Option<String> {
    let first_cr = text.find('\r')?;
    let mut normalized = String::with_capacity(text.len());
    normalized.push_str(&text[..first_cr]);
    let mut remaining = &text[first_cr..];

    while let Some(cr) = remaining.find('\r') {
        normalized.push_str(&remaining[..cr]);
        remaining = &remaining[cr + 1..];
        if let Some(after_lf) = remaining.strip_prefix('\n') {
            remaining = after_lf;
        }
        normalized.push('\n');
    }

    normalized.push_str(remaining);
    Some(normalized)
}
