//! zcv-keymap —— 快捷键加载器。
//!
//! 内置平台快捷键随程序发布，运行时通过 GPUI action registry 解析为[`KeyBindings`]，供应用注册和 UI 反向查询。

mod keymap;

pub use keymap::{KeyBindings, load, load_json};
