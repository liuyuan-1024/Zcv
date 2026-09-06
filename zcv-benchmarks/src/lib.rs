//! Zcv 的性能基准目标。
//!
//! 基准独立于产品 crate，避免 `criterion` 等仅测量使用的依赖进入正常构建。
//! 每个 `benches/` 文件覆盖一条可感知的编辑器核心路径。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

/// 创建固定、包含 Unicode 的 Rust 风格文档。
///
/// 生成器不依赖随机数，以便不同提交之间的结果可直接比较。
fn rust_document(target_bytes: usize) -> String {
    const BLOCK: &str = "pub fn render_document(index: usize) -> String {\n    let label = format!(\"第 {index} 个条目：Zcv 性能基准\");\n    format!(\"{label} / {}\", index.saturating_mul(17))\n}\n\n";

    let mut text = String::with_capacity(target_bytes);
    let mut index = 0;
    while text.len() + BLOCK.len() <= target_bytes {
        text.push_str(&BLOCK.replace("{index}", &index.to_string()));
        index += 1;
    }
    text.push_str("// benchmark padding\n");
    text
}

/// 返回指定大小的固定 Rust 语料。
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
