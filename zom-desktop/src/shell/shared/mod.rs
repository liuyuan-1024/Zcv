//! shell 共享基础设施。
//!
//! 这里放跨功能、跨区域复用的主题 token、资源与输入 / 交互工具。

pub(crate) mod assets;
pub(crate) mod glyph;
pub(crate) mod interaction;
pub(crate) mod keyboard;
pub(crate) mod scroll;

pub(crate) use glyph::Glyph;
