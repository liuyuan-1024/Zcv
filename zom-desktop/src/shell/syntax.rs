//! 语法高亮 Tier 1 装配（手册《桌面端语法高亮》§十.1）。
//!
//! 把 Tier 1 provider 工厂注入 [`Workspace::language_registry_mut`]。
//!
//! 这一步**只能在 desktop 组合根**做（手册 §六 / §十）：
//! - `zom-workspace` 提供 trait / Registry / Coordinator 这些**机制**
//! - desktop 决定**装哪些**Tier 1 grammar
//!
//! 后续接 LSP / wasm 时同样走 `register`，本函数不需要为之新增分支。
//!
//! ## 当前 Tier 1 清单
//!
//! Rust / TOML / Markdown / JSON / YAML / Bash / HTML / CSS / JavaScript /
//! TypeScript / TSX / Java / Python（共 13 门）。新增语言只需：
//!
//! 1. 根 `Cargo.toml` + `zom-workspace/Cargo.toml` 加 grammar crate；
//! 2. `zom-workspace/src/syntax/providers/<lang>.rs` 写 ~40 行 provider；
//! 3. `providers/mod.rs` 加一行 `pub mod <lang>;`；
//! 4. 本函数加一行 `register_lang(...)`。
//!
//! ## 失败语义
//!
//! provider 的 `new_provider()` 在 config build 失败时自身 panic——失败仅
//! 发生在 query 语法错误或 ABI 不匹配（静态资源问题，发版前必须被测试覆盖）。
//! 因此本函数的 factory 闭包不需要也不应当再写 `.expect(...)`：错误信息归
//! provider 自己（见各 provider 文件内的 expect 文本）。

use zom_workspace::Workspace;
use zom_workspace::syntax::providers::{
    bash, css, html, java, javascript, json, markdown, python, rust, toml, typescript, yaml,
};
use zom_workspace::syntax::{LanguageDetector, LanguageId, LanguageRegistry, ProviderFactory};

/// 在 app boot 期把所有 Tier 1 provider 注入 workspace 的 registry。
pub(crate) fn install_tier1(workspace: &mut Workspace) {
    let registry = workspace.language_registry_mut();
    register(
        registry,
        "rust",
        &[LanguageDetector::Extension(&["rs"])],
        Box::new(|| Box::new(rust::new_provider())),
    );
    register(
        registry,
        "toml",
        &[
            LanguageDetector::Extension(&["toml"]),
            LanguageDetector::Filename(&["Cargo.lock"]),
        ],
        Box::new(|| Box::new(toml::new_provider())),
    );
    register(
        registry,
        "markdown",
        &[LanguageDetector::Extension(&["md", "markdown"])],
        Box::new(|| Box::new(markdown::new_provider())),
    );
    register(
        registry,
        "json",
        &[LanguageDetector::Extension(&["json"])],
        Box::new(|| Box::new(json::new_provider())),
    );
    register(
        registry,
        "yaml",
        &[LanguageDetector::Extension(&["yaml", "yml"])],
        Box::new(|| Box::new(yaml::new_provider())),
    );
    register(
        registry,
        "bash",
        &[
            LanguageDetector::Extension(&["sh", "bash"]),
            LanguageDetector::Shebang(&["sh", "bash"]),
        ],
        Box::new(|| Box::new(bash::new_provider())),
    );
    register(
        registry,
        "html",
        &[LanguageDetector::Extension(&["html", "htm"])],
        Box::new(|| Box::new(html::new_provider())),
    );
    register(
        registry,
        "css",
        &[LanguageDetector::Extension(&["css"])],
        Box::new(|| Box::new(css::new_provider())),
    );
    register(
        registry,
        "javascript",
        // .jsx 暂用 javascript grammar；如需 TSX 风格 JSX 高亮，未来可单拆。
        &[LanguageDetector::Extension(&["js", "mjs", "cjs", "jsx"])],
        Box::new(|| Box::new(javascript::new_provider())),
    );
    register(
        registry,
        "typescript",
        &[LanguageDetector::Extension(&["ts", "mts", "cts"])],
        Box::new(|| Box::new(typescript::new_typescript_provider())),
    );
    register(
        registry,
        "tsx",
        &[LanguageDetector::Extension(&["tsx"])],
        Box::new(|| Box::new(typescript::new_tsx_provider())),
    );
    register(
        registry,
        "java",
        &[LanguageDetector::Extension(&["java"])],
        Box::new(|| Box::new(java::new_provider())),
    );
    register(
        registry,
        "python",
        &[
            LanguageDetector::Extension(&["py", "pyi"]),
            LanguageDetector::Shebang(&["python", "python3"]),
        ],
        Box::new(|| Box::new(python::new_provider())),
    );
}

/// 把 register 三段式封装一层：调用方只写 (id, detectors, factory)，减少重复噪音。
fn register(
    registry: &mut LanguageRegistry,
    id: &'static str,
    detectors: &[LanguageDetector],
    factory: ProviderFactory,
) {
    registry.register(LanguageId::new(id), detectors.to_vec(), factory);
}
