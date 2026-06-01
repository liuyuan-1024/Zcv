//! Tier 1 内置 provider 实例（手册 §十.1）。
//!
//! 当前 Tier 1 清单：Rust / TOML / Markdown / JSON / YAML / Bash / HTML /
//! CSS / JavaScript / TypeScript / TSX / Java / Python（共 13 门——按"语义独立
//! grammar"计数；TS 与 TSX 同 crate 但是两条 grammar，分别注册）。
//!
//! 不做 cargo feature 拆分——provider 数量在 Tier 1 范围内（手册 §十估 15–25
//! 门），tree-sitter grammar crate 单体积都很小，feature 门带来的 build matrix
//! 与心智成本大于收益。Tier 1 进一步扩张到需要按需裁剪二进制时再分（手册
//! §十「Tier 2 语言包」）。
//!
//! 注册由 desktop 组合根负责（手册 §十）。

mod common;

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
