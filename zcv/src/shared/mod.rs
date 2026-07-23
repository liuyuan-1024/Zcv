//! shell 共享基础设施。

pub(crate) mod assets;
pub(crate) mod context_menu;
mod glyph;
mod icon;
pub(crate) mod picker;
pub(crate) mod tree;

pub(crate) use glyph::Glyph;
pub(crate) use icon::SvgIcon;
