//! 通用搜索选择器。
//!
//! 消费 `zcv-editor` 作为搜索输入框，供项目选择、最近项目等宿主场景复用；
//! 基础展示组件（Glyph / ListItem 等）在 `zcv-ui`，本 crate 只承担选择器逻辑。

mod picker;

pub use picker::{Picker, PickerDelegate, picker_divider};
