//! zcv-keymap —— 快捷键加载器。
//!
//! 编译时通过 `include_str!` 嵌入平台 keymap JSON（本 crate `assets/keymaps/`），运行时通过 GPUI action registry 解析为 [`KeyBindings`]，供应用注册和 UI 反向查询。

mod keymap;

pub use keymap::{KeyBindings, load, load_json};
