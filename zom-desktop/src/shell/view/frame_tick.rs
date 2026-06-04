//! 每帧 prepaint 起手的后台子系统收割。
//!
//! 主工作区高亮 / 搜索 / viewport hint 由 App 拥有，settings TOML 编辑器的
//! 高亮由 [`SettingsRuntime`] 自有 —— 两条独立后台子系统在同一拍统一驱动，
//! 避免散落在 render 头上五行不带名字的 pump_* 调用。

use crate::app::App;
use crate::shell::features::settings::SettingsRuntime;

/// 把一次 frame 起手的所有后台收割集中执行。
///
/// 调用者必须先借出 App 可变引用——focus 反向同步与本函数共享同一个 borrow scope，
/// 避免一帧内对 RefCell 多次借用。
pub(super) fn advance(app: &mut App, settings: &SettingsRuntime) {
    // 主线程上 drain SyntaxWorker 已就绪的高亮 spans 到 MetadataLayers。
    // 没有这一拍即便 worker 算完也上不了屏。
    app.pump_pending_highlights();
    // 跑所有注册的 FramePump——search 的"收割后台命中"目前是唯一登记者：
    // 大文件 search 在后台跑，这一拍把已就绪 SearchResult 落到 slot 并 reveal 首条命中。
    // 其它 feature 同节奏的 drain 走同一端口注册。
    app.pump_frame_observers();
    // 把当前活动 view 的 viewport ± padding 推给 worker，让下一拍 on_edit 走 viewport-scoped query + ReplaceRange，仅产可见区段 spans。
    // worker 内部去重，无变化时不重 query。
    app.pump_active_viewport_hint();
    // settings TOML 编辑器自家后台子系统：SettingsRuntime 拥有 toml 编辑器，
    // 与上面三条 app 级 pump 并排独立 drain。
    settings.pump_pending_highlights();
}
