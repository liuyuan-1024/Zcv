//! 应用层的内置文件预览装配入口。
//!
//! 公共协议和注册表位于 `zcv-workspace`；这里仅决定应用启用哪些格式实现。

use gpui::App;

/// 注册全部内置 Preview Provider。各格式注册保持幂等。
pub(crate) fn init(cx: &mut App) {
    zcv_preview_svg::init(cx);
}
