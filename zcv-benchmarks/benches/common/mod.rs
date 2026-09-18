//! 基准共享语料生成器。
//!
//! 每个 bench 通过 `mod common;` 引入，避免为仅测量使用的 helper 保留生产 lib target。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// 用固定块重复填充到目标字节数，尾部补一行注释对齐边界。
///
/// 生成器不依赖随机数，以便不同提交之间的结果可直接比较；
/// `{index}` 占位符按块序号替换，避免整篇字节完全重复。
pub(crate) fn fill_with_block(block: &str, target_bytes: usize) -> String {
    let mut text = String::with_capacity(target_bytes);
    let mut index = 0;
    while text.len() + block.len() <= target_bytes {
        text.push_str(&block.replace("{index}", &index.to_string()));
        index += 1;
    }
    text.push_str("// benchmark padding\n");
    text
}

/// 创建固定、包含 Unicode、宏密度贴近真实源码（约 856 字节/宏，落在真实 .rs 文件实测的 494–17678 字节/宏区间）的 Rust 风格文档。
///
/// 这是解读高亮与解析成本的默认代表档。
fn rust_document(target_bytes: usize) -> String {
    const BLOCK: &str = "pub fn render_document(index: usize) -> String {\n    let label = format!(\"第 {index} 个条目：Zcv 性能基准\");\n    let mut summary = String::with_capacity(192);\n    summary.push_str(\"渲染文档条目，穿插足够的普通语句，使宏密度贴近真实源码。\");\n    summary.push_str(&label);\n    let weight = index.saturating_mul(17).rem_euclid(97);\n    if weight > 48 {\n        summary.push_str(\"权重偏高：追加一段说明文字，用于填充字节并模拟真实分支。\");\n    } else {\n        summary.push_str(\"权重正常：保持默认描述，不额外追加内容。\");\n    }\n    for offset in 0..weight {\n        summary.push_str(&offset.to_string());\n        summary.push('、');\n    }\n    summary.push_str(\"条目渲染结束，返回聚合后的摘要字符串，供上层视图直接消费展示。\");\n    summary\n}\n\n";

    fill_with_block(BLOCK, target_bytes)
}

/// 返回指定大小的固定代表性 Rust 语料。
///
/// 同一基准进程内每种大小只生成一次；缓存只在进程存续期间存在，且不属于计时区间。
pub fn cached_rust_document(target_bytes: usize) -> Arc<str> {
    static DOCUMENTS: OnceLock<Mutex<HashMap<usize, Arc<str>>>> = OnceLock::new();

    let documents = DOCUMENTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut documents = documents.lock().expect("基准语料缓存锁不应中毒");
    Arc::clone(
        documents
            .entry(target_bytes)
            .or_insert_with(|| Arc::from(rust_document(target_bytes))),
    )
}
