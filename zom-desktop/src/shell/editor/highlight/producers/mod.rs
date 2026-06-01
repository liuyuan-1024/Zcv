//! 各 producer 翻译为 [`Decoration`](super::Decoration) 的适配器。
//!
//! 架构手册 §三 的 producer 清单在物理上的对应物：每个子模块对应一类 producer，
//! 输入是它的原生数据（`SelectionSet` / `MetadataLayer<HighlightSpan>` /
//! `BufferSearch`），输出 push 到 `&mut Vec<Decoration>`。
//!
//! ## 为什么适配器都在 desktop
//!
//! [`Decoration`](super::Decoration) 是 desktop crate 内的类型；`zom-workspace` /
//! `zom-engine` 不能反向依赖 desktop。所以「原生数据 → Decoration」的翻译点
//! 物理上必然落在 desktop。producer 在概念上仍是分散的（每家自管数据源、
//! 失效语义、生命周期）；本目录只承担**适配**职责，不承担数据所有权。
//!
//! ## 新增 producer 的步骤
//!
//! 1. 在本目录加 `<name>.rs`，对外暴露 `pub(crate) fn push(...)`。
//! 2. 在 [`super::priority`] 选档位（或新增档位常量）。
//! 3. 在调用方（[`crate::shell::editor::snapshot::builder`] 或
//!    [`crate::shell::workbench::editor_area::text_target`]）加一行 `push(...)`。
//! 4. 必要时在 [`super::StyleClass`] 扩枚举并在 [`super::resolve_named`] 配色。

pub(crate) mod search;
pub(crate) mod selection;
pub(crate) mod syntax;
