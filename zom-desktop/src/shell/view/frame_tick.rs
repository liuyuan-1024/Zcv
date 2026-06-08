//! 每帧 prepaint 起手的后台子系统收割。
//!
//! 跑跨 feature 的 [`FramePump`] 收割（search 后台命中等）。
//! 语法高亮没有需要 drain 的中间产物 —— paint 阶段直接从共享 `BufferSyntaxTreeSlot` 现查 tree-sitter Query。
//!
//! [`FramePump`]: crate::ports::FramePump

use crate::app::App;

/// 把一次 frame 起手的所有后台收割集中执行。
///
/// 调用者必须先借出 App 可变引用——focus 反向同步与本函数共享同一个 borrow scope，避免一帧内对 RefCell 多次借用。
pub(super) fn advance(app: &mut App) {
    // 跑所有注册的 FramePump——search 的"收割后台命中"目前是唯一登记者：
    // 大文件 search 在后台跑，这一拍把已就绪 SearchResult 落到 slot 并 reveal 首条命中。
    // 其它 feature 同节奏的 drain 走同一端口注册。
    app.pump_frame_observers();
}
