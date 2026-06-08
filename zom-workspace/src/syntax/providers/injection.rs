//! Fence info-string → `SharedConfig` 的解析表。
//!
//! 仅服务 markdown provider 的 fenced code 注入（手册 §十四「markdown 例外」扩张）。
//! 其他语言不需要注入，所以这条路径在主干上是死代码——只在 [`crate::syntax::providers::markdown`] 内被调用。
//!
//! ## 设计立场
//!
//! - **只覆盖 Tier 1**：tree-sitter grammar 已经编进 zom-desktop 的语言才支持注入。Tier 2 wasm 语言包未来接入，逻辑相同（registry 查表），届时再扩。
//! - **不接 markdown 递归注入**：fenced 内写 ```` ```markdown ```` 不再嵌套展开——栈结构开销不值得，文本编辑里也极罕见。
//! - **不接 html_block / frontmatter / inline 注入**：tree-sitter-md 的 `injections.scm` 还想为 html_block 套 html grammar、frontmatter 套 yaml/toml、`inline` 套 markdown_inline grammar。
//! 第一项暂不接（markdown 文档里 html_block 不多见）；
//! frontmatter 暂不接（编辑场景边界情况）；
//! `inline` 由 markdown provider 自己处理（[`super::markdown`]），不走这里。
//! - **alias 表手工维护**：fence 用户写 `rs` / `py` / `sh` / `yml` 都常见。把这些常见简写直接归一到 canonical 语言 id，与 [`crate::syntax::providers`] 注册时的 LanguageId 对齐。
//!
//! ## 失败模式
//!
//! 任一 config 函数返回 `Err`（query 语法错 / ABI 不匹配）→ `resolve_injection_language` 返回 `None`，
//! markdown provider 把该 fence 当作未识别语言处理（保持 `markup.raw.block`）。
//! 这与 Tier 1 主路径「config 失败 = 发版前必须修」的契约一致：注入路径不该让 fence 处理为 panic / 拖整个 buffer。

use std::sync::Arc;

use crate::syntax::providers::common::SharedConfig;
use crate::syntax::providers::{
    bash, css, html, java, javascript, json, python, rust, toml, typescript, yaml,
};

/// 把 fence info-string 解析到 [`SharedConfig`]。
///
/// `name` 是 fenced code block 紧跟在三个反引号后那段（一般是单词，可能带大小写 / 别名）。
/// 返回 `None` 表示未识别——markdown provider 据此让该 fence 保持 block grammar 默认的 `markup.raw.block`。
///
/// 名字做 `trim` + `to_ascii_lowercase` 归一化；别名走匹配表收敛到 canonical 语言 id（与 [`crate::syntax::providers`] 注册的 LanguageId 同名）。
pub(crate) fn resolve_injection_language(name: &str) -> Option<Arc<SharedConfig>> {
    let canonical = canonicalize(name)?;
    config_for(canonical).ok()
}

/// 把 fence info-string 收敛到 canonical 语言 id。
///
/// 返回 `None` 表示空 / 未知 alias，调用方走兜底——空 fence（` ``` ` 不带语言）是这里返回 `None` 的主要来源。
fn canonicalize(name: &str) -> Option<&'static str> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return None;
    }
    // tree-sitter-md 的 (language) 节点已经把 fence 紧邻 ``` 后面的标识符提取出来，
    // 一般是 ASCII 单词；做一次 lower 即可。
    let lower = trimmed.to_ascii_lowercase();
    let mapped: &'static str = match lower.as_str() {
        "rust" | "rs" => "rust",
        "toml" => "toml",
        "json" => "json",
        "yaml" | "yml" => "yaml",
        "bash" | "sh" | "shell" | "zsh" => "bash",
        "html" | "htm" => "html",
        "css" => "css",
        "javascript" | "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "typescript" | "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "java" => "java",
        "python" | "py" | "python3" => "python",
        _ => return None,
    };
    Some(mapped)
}

/// 拿到 canonical 语言 id 对应的 [`SharedConfig`]。
///
/// 与 [`crate::syntax::providers::install_builtin_providers`] 注册的工厂一一对应。
/// 新加 Tier 1 语言时这里也要补一行——不补就是 fence 注入对该语言无效，与 fallback 行为等价（保持 markup.raw.block）。
fn config_for(canonical: &'static str) -> Result<Arc<SharedConfig>, ()> {
    let cfg = match canonical {
        "rust" => rust::rust_config(),
        "toml" => toml::toml_config(),
        "json" => json::json_config(),
        "yaml" => yaml::yaml_config(),
        "bash" => bash::bash_config(),
        "html" => html::html_config(),
        "css" => css::css_config(),
        "javascript" => javascript::javascript_config(),
        "typescript" => typescript::typescript_config(),
        "tsx" => typescript::tsx_config(),
        "java" => java::java_config(),
        "python" => python::python_config(),
        _ => return Err(()),
    };
    cfg.map_err(|_| ())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolves_canonical_names() {
        assert!(resolve_injection_language("rust").is_some());
        assert!(resolve_injection_language("python").is_some());
        assert!(resolve_injection_language("typescript").is_some());
    }

    #[test]
    fn resolves_common_aliases() {
        // 简写 / 大小写 / 周围空白 都要归一。
        assert!(resolve_injection_language("rs").is_some());
        assert!(resolve_injection_language("Rs").is_some());
        assert!(resolve_injection_language("  py  ").is_some());
        assert!(resolve_injection_language("sh").is_some());
        assert!(resolve_injection_language("yml").is_some());
        assert!(resolve_injection_language("js").is_some());
        assert!(resolve_injection_language("ts").is_some());
        assert!(resolve_injection_language("tsx").is_some());
    }

    #[test]
    fn unknown_languages_fall_back_to_none() {
        assert!(resolve_injection_language("").is_none());
        assert!(resolve_injection_language("   ").is_none());
        assert!(resolve_injection_language("brainfuck").is_none());
        // markdown 自身不接递归注入。
        assert!(resolve_injection_language("markdown").is_none());
        assert!(resolve_injection_language("markdown_inline").is_none());
    }
}
