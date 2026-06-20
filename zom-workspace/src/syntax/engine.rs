//! `SyntaxEngine` —— 语法高亮子系统的共享资源池。
//!
//! 把原本散在 [`crate::Workspace`] 上的「语言注册表 + 后台 worker + buffer id 分配器」收口成一个值，
//! 按 [`std::rc::Rc`] 在主工作区与任意数量的嵌入式文档 ([`crate::SyntaxDocument`]) 之间共享：
//!
//! - **一次注册，全局可见**：组合根启动时通过 [`Self::registry_mut`] 注一遍内置 provider 工厂，所有持有同一 `Rc<SyntaxEngine>` 的容器都能 detect。
//! - **一根后台线程**：嵌入式编辑器不再各自 `SyntaxWorkerHandle::spawn`，都搭在共享 worker 上。
//! - **跨容器的稳定 buffer id**：worker 用 [`crate::BufferId`] 做任务寻址；id 由本结构集中分配，主工作区的常规缓冲区与嵌入文档不会撞 id。
//!
//! 启动期 [`SyntaxEngine::new`] 创建后**通常一次性配置完毕再 `Rc::new` 共享**
//! —— 注册表本身没用 RefCell，因为运行期只读路径足够。需要热插拔语言时再升级。

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use crate::BufferId;

use super::language::LanguageRegistry;
use super::payload::HighlightName;
use super::worker::SyntaxWorkerHandle;

/// 语法高亮子系统的共享资源——语言注册表、后台 worker、buffer id 分配器。
///
/// 不可 `Clone`：跨容器共享通过 `Rc<SyntaxEngine>` 完成。
/// `SyntaxWorkerHandle` 内部已是 `Arc`，跨 `Rc<SyntaxEngine>` 引用同一根后台线程。
#[derive(Debug)]
pub struct SyntaxEngine {
    registry: LanguageRegistry,
    worker: Arc<SyntaxWorkerHandle>,
    next_buffer_id: AtomicU64,
}

impl SyntaxEngine {
    /// 全新引擎：空注册表 + 新启动的 worker 线程 + id 起点 1。
    pub fn new() -> Self {
        Self {
            registry: LanguageRegistry::new(),
            worker: Arc::new(SyntaxWorkerHandle::spawn()),
            next_buffer_id: AtomicU64::new(1),
        }
    }

    pub fn registry(&self) -> &LanguageRegistry {
        &self.registry
    }

    /// 仅启动期可变：组合根在 `Rc::new` 之前注册内置 provider 工厂。
    pub fn registry_mut(&mut self) -> &mut LanguageRegistry {
        &mut self.registry
    }

    pub fn worker(&self) -> &Arc<SyntaxWorkerHandle> {
        &self.worker
    }

    /// 分配下一个全局唯一 [`BufferId`]。
    ///
    /// 主工作区的 `open_*` 与 [`crate::SyntaxDocument::new`] 都走这条入口，
    /// 保证共享 worker 上的任务寻址不会撞 id。
    pub fn allocate_buffer_id(&self) -> BufferId {
        BufferId::from_raw(self.next_buffer_id.fetch_add(1, Ordering::Relaxed))
    }

    /// 对独立代码片段做一次性语法高亮（如 Markdown fenced code block）。
    ///
    /// `lang` 是代码块的语言标签（如 `"rust"`、`"javascript"`、`"py"`）。
    /// 内部处理常见简写别名、查找已注册 provider、创建临时 parser 做一次性 highlight query。
    ///
    /// 返回按 byte range 排序的 `(range, HighlightName)` 列表；
    /// 语言未注册或 provider 不支持 snippet 时返回 `None`。
    ///
    /// 所有返回的 range 端点均已钳位到 UTF-8 char boundary（tree-sitter 某些 grammar 可能产出落在多字节字符中间的端点）。
    pub fn highlight_snippet(
        &self,
        lang: &str,
        code: &str,
    ) -> Option<Vec<(std::ops::Range<usize>, HighlightName)>> {
        let normalized = normalize_lang_alias(lang);
        let id = self.registry.find_by_name(normalized)?;
        let provider = self.registry.make_provider(id)?;
        let spans = provider.highlight_snippet(code);
        if spans.is_empty() {
            return None;
        }
        let spans: Vec<_> = spans
            .into_iter()
            .filter_map(|(range, name)| {
                let start = floor_char_boundary(range.start, code);
                let end = ceil_char_boundary(range.end, code);
                if start < end {
                    Some((start..end, name))
                } else {
                    None
                }
            })
            .collect();
        if spans.is_empty() { None } else { Some(spans) }
    }
}

impl Default for SyntaxEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// 把 byte 位置钳位到最近的 UTF-8 char boundary（向下取整）。
fn floor_char_boundary(pos: usize, text: &str) -> usize {
    let pos = pos.min(text.len());
    if text.is_char_boundary(pos) {
        return pos;
    }
    (0..pos)
        .rev()
        .find(|&p| text.is_char_boundary(p))
        .unwrap_or(0)
}

/// 把 byte 位置钳位到最近的 UTF-8 char boundary（向上取整）。
///
/// 用于 range.end：若 tree-sitter 把 end 落在多字节字符中间，向上走到字符末尾。
fn ceil_char_boundary(pos: usize, text: &str) -> usize {
    let pos = pos.min(text.len());
    if text.is_char_boundary(pos) {
        return pos;
    }
    (pos + 1..=text.len())
        .find(|&p| text.is_char_boundary(p))
        .unwrap_or(text.len())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::install_builtin_providers;

    #[test]
    fn highlight_snippet_json_unicode_all_ranges_on_char_boundaries() {
        let mut engine = SyntaxEngine::new();
        install_builtin_providers(&mut engine);

        // 来自 grammar-test/markdown.md §5.6 的 JSON 代码块内容
        let json = r#"{
  "name": "markdown-highlighter-test",
  "version": "1.0.0",
  "features": [
    "headings",
    "lists",
    "tables",
    "code_fences",
    "frontmatter"
  ],
  "unicode": "中文 😀 é ñ"
}"#;

        let spans = engine
            .highlight_snippet("json", json)
            .expect("json provider 应产出高亮结果");

        assert!(!spans.is_empty(), "应至少有一个 span");

        for (range, name) in &spans {
            let start_ok = json.is_char_boundary(range.start);
            let end_ok = json.is_char_boundary(range.end);
            if !start_ok || !end_ok {
                // 打印附近上下文帮助定位
                let ctx_start = range.start.saturating_sub(10);
                let ctx_end = (range.end + 10).min(json.len());
                let snippet = &json[ctx_start..ctx_end];
                panic!(
                    "span {name} ({:?}) 端点不在 char boundary 上: \
                     start_boundary={start_ok}, end_boundary={end_ok}, \
                     context=\"{snippet}\"",
                    range,
                );
            }
        }
    }
}

/// 把 Markdown fenced code block 语言标签映射为 [`LanguageId`] 名。
///
/// 处理常见简写别名（如 `js` → `javascript`、`py` → `python`）。
fn normalize_lang_alias(tag: &str) -> &str {
    match tag {
        "js" => "javascript",
        "ts" => "typescript",
        "py" => "python",
        "rb" => "ruby",
        "sh" | "shell" => "bash",
        "yml" => "yaml",
        "md" | "mkd" => "markdown",
        other => other,
    }
}
