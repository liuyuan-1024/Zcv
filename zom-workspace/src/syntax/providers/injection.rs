//! 语言注入（language injection）配置入口。
//!
//! 当前唯一调用方是 [`super::markdown`]——它把 fenced code block 的 info string 当作语言名，到这里查对应 grammar 的 [`SharedConfig`] 跑高亮。
//!
//! ## 形态：静态 match，不是 HashMap
//!
//! 注入语言集合在编译期就固定为 [`super`] 里已经注册的 Tier 1 grammar。
//! 用 `match` 路由有两条好处：
//! 1. 多一份语言要补上时编译期就报错——比运行时 `unknown language` silent fail 友好；
//! 2. 不引入运行时锁 / 全局可变状态——所有 `SharedConfig` 的复用仍然走各 provider 模块本来的 `OnceLock`，单进程内全 buffer 共享一份。
//!
//! ## 与 [`super::LanguageRegistry`] 的区别
//!
//! 那个 registry 面向 "打开文件 → 挑 provider"，存的是 [`super::ProviderFactory`]
//! ——每次 attach 造一个全新 [`super::HighlightProvider`] 实例（带独立 Parser / cursor 状态）。
//! injection 这边要的是**共享 grammar 配置**，给注入路径就近 parse 用，不开新 provider。
//! 两条路径职责清晰、不复用结构。

use std::sync::Arc;

use crate::syntax::providers::common::SharedConfig;
use crate::syntax::providers::{
    bash, css, html, java, javascript, json, python, rust, toml, typescript, yaml,
};

/// 查 `lang` 对应的注入 grammar 配置。
///
/// `lang` 是 fenced code block info string 里的语言名字面值（已去前后空白；大小写敏感由 [`normalize_alias`] 折叠）。
/// 返回 `None` 时调用方应当跳过注入（代码块保持宿主 grammar 给的 `markup.raw.block` 即可）。
///
/// 标识失败也走 `None`：grammar 自身静态资源问题（query 语法 / ABI），在各 provider 模块的单测里就会被发现，这里没必要把错传给调用方。
pub(crate) fn injection_config(lang: &str) -> Option<Arc<SharedConfig>> {
    let canonical = normalize_alias(lang);
    let result = match canonical {
        "rust" => rust::rust_config(),
        "json" => json::json_config(),
        "toml" => toml::toml_config(),
        "bash" => bash::bash_config(),
        "html" => html::html_config(),
        "css" => css::css_config(),
        "javascript" => javascript::javascript_config(),
        "typescript" => typescript::typescript_config(),
        "tsx" => typescript::tsx_config(),
        "java" => java::java_config(),
        "python" => python::python_config(),
        "yaml" => yaml::yaml_config(),
        // markdown 自己不递归注入——避免代码块里再写 ```markdown 套娃。
        _ => return None,
    };
    result.ok()
}

/// 把常见 fence 标签别名折叠到注册表里的规范名。大小写归一到小写。
///
/// 不识别的标签原样返回——上游 match 会落到 `_ => None`。
///
/// 取舍：只覆盖文档里大量出现 / 与 Tier 1 grammar 直接对应的别名，没必要收容所有 highlight.js / linguist 列出的 alias
/// ——后者动辄上百条，多数指向当前并不在 Tier 1 的 grammar，匹配上也没意义。
pub(crate) fn normalize_alias(raw: &str) -> &'static str {
    // raw 入口前已 trim，这里只折叠大小写。比直接 `eq_ignore_ascii_case` 多一份分配，
    // 但同进程内同一组别名命中率高、`Arc<SharedConfig>` 是 OnceLock 缓存的，调用成本本不在这一行。
    let lower = raw.to_ascii_lowercase();
    match lower.as_str() {
        "rust" | "rs" => "rust",
        "json" => "json",
        "toml" => "toml",
        "bash" | "sh" | "shell" | "zsh" => "bash",
        "html" | "htm" => "html",
        "css" => "css",
        "javascript" | "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "typescript" | "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "java" => "java",
        "python" | "py" | "python3" => "python",
        "yaml" | "yml" => "yaml",
        "markdown" | "md" => "markdown",
        _ => "",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alias_folds_to_canonical_names() {
        assert_eq!(normalize_alias("rs"), "rust");
        assert_eq!(normalize_alias("Rust"), "rust");
        assert_eq!(normalize_alias("RUST"), "rust");
        assert_eq!(normalize_alias("js"), "javascript");
        assert_eq!(normalize_alias("ts"), "typescript");
        assert_eq!(normalize_alias("sh"), "bash");
        assert_eq!(normalize_alias("py"), "python");
        assert_eq!(normalize_alias("yml"), "yaml");
        assert_eq!(normalize_alias("md"), "markdown");
    }

    #[test]
    fn unknown_alias_returns_empty() {
        assert_eq!(normalize_alias("zig"), "");
        assert_eq!(normalize_alias(""), "");
    }

    #[test]
    fn lookup_returns_config_for_known_languages() {
        for lang in ["rust", "json", "bash", "javascript", "typescript", "tsx"] {
            assert!(
                injection_config(lang).is_some(),
                "{lang} 应当有可用注入 config"
            );
        }
    }

    #[test]
    fn lookup_skips_unknown_and_markdown() {
        assert!(injection_config("zig").is_none());
        assert!(
            injection_config("markdown").is_none(),
            "markdown 不递归注入"
        );
        assert!(injection_config("").is_none());
    }
}
