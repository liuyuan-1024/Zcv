//! 通用搜索选择器。
//! 此文件是 `zcv-picker` crate 的公共入口。
//!
//! 消费 `zcv-editor` 作为搜索输入框，供项目选择、最近项目等宿主场景复用；
//! 基础展示组件（Glyph / ListItem 等）在 `zcv-ui`，本 crate 只承担选择器逻辑。

mod picker_view;

pub use picker_view::{Picker, PickerDelegate, picker_divider};
