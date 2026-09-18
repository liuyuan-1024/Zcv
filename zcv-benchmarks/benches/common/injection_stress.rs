//! 病态宏密集的注入压力语料。
//!
//! 只被使用它的 bench 通过 `#[path]` 引入，避免在其它 bench 中产生未使用代码。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

use crate::common::fill_with_block;

/// 创建固定、病态宏密集（约 88 字节/宏，比代表档密约 10 倍）的 Rust 风格文档。
///
/// 每块约 175 字节含 2 个 `format!` 宏，刻意放大「宏 → 注入 rust 子解析」级联，用作注入引擎的压力测试；
/// 不代表真实负载，解读其绝对数字须与 `rust_document` 对照。
fn injection_stress_document(target_bytes: usize) -> String {
    const BLOCK: &str = "pub fn render_document(index: usize) -> String {\n    let label = format!(\"第 {index} 个条目：Zcv 性能基准\");\n    format!(\"{label} / {}\", index.saturating_mul(17))\n}\n\n";

    fill_with_block(BLOCK, target_bytes)
}

/// 返回指定大小的固定注入压力语料（缓存策略与 `cached_rust_document` 一致）。
pub fn cached_injection_stress_document(target_bytes: usize) -> Arc<str> {
    static DOCUMENTS: OnceLock<Mutex<HashMap<usize, Arc<str>>>> = OnceLock::new();

    let documents = DOCUMENTS.get_or_init(|| Mutex::new(HashMap::new()));
    let mut documents = documents.lock().expect("基准语料缓存锁不应中毒");
    Arc::clone(
        documents
            .entry(target_bytes)
            .or_insert_with(|| Arc::from(injection_stress_document(target_bytes))),
    )
}
