//! 快捷键加载与解析。
//! 此文件是 `zcv-keymap` crate 的公共入口。
//!
//! 内置平台快捷键随程序发布，运行时通过 GPUI action registry 解析为[`KeyBindings`]，供应用注册和 UI 反向查询。

mod loader;

pub use loader::{KeyBindings, init};
