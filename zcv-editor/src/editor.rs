//! Editor —— 可嵌入文本编辑组件。
//! 此文件是 `zcv-editor` crate 的公共入口。

mod blink_manager;
mod display_map;
mod element;
mod gutter;
mod item_provider;
mod scroll;
mod scrollbar;
mod selection;
mod view;
mod workspace_item;

pub use item_provider::init as init_item_providers;
pub use view::{Editor, EditorEvent, SoftWrap};
