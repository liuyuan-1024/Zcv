//! 内置 provider 实例（手册 §十）。
//!
//! 当前清单：Rust / TOML / Markdown / JSON / YAML / Bash / HTML /
//! CSS / JavaScript / TypeScript / TSX / Java / Python（共 13 门——按"语义独立
//! grammar"计数；TS 与 TSX 同 crate 但是两条 grammar，分别注册）。
//!
//! 不做 cargo feature 拆分。当前语言都由主仓维护并直接编译，避免额外 build matrix。
//!
//! 注册由组合根在创建共享 [`SyntaxEngine`](crate::syntax::SyntaxEngine) 时调用
//! [`install_builtin_providers`] 完成（手册 §十）。

pub(crate) mod common;
pub(crate) mod injection;

pub mod bash;
pub mod css;
pub mod html;
pub mod java;
pub mod javascript;
pub mod json;
pub mod markdown;
pub mod python;
pub mod rust;
pub mod toml;
pub mod typescript;
pub mod yaml;

use crate::syntax::{
    LanguageDetector, LanguageId, LanguageRegistry, ProviderFactory, SyntaxEngine,
};

/// 在 boot 期把所有内置 provider 注入共享的 [`SyntaxEngine`]。
///
/// 调用方必须在 `Rc::new(engine)` **之前**调本函数——一旦引擎被 `Rc` 共享，注册表就不再可变，
/// 详见 [`SyntaxEngine::registry_mut`] 的契约。
pub fn install_builtin_providers(engine: &mut SyntaxEngine) {
    let registry = engine.registry_mut();
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

fn register(
    registry: &mut LanguageRegistry,
    id: &'static str,
    detectors: &[LanguageDetector],
    factory: ProviderFactory,
) {
    registry.register(LanguageId::new(id), detectors.to_vec(), factory);
}
