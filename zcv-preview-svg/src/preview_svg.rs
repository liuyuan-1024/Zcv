//! SVG 文件预览。
//! 此文件是 `zcv-preview-svg` crate 的公共入口。

mod provider;
mod renderer;
mod view;

use gpui::App;
use provider::SvgPreviewProvider;

/// 注册 SVG Preview Provider。可重复调用。
pub fn init(cx: &mut App) {
    zcv_workspace::register(SvgPreviewProvider, cx);
}
