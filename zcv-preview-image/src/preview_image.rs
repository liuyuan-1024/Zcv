//! 栅格图片文件预览。
//!
//! 图片是二进制文件，不能由文本编辑器提供源码 Item；
//! 本 crate 通过独立的 `PreviewProvider` 创建图片 Item，并在后台读取和解码文件。

mod provider;
mod view;

use gpui::App;
use provider::ImagePreviewProvider;

/// 注册栅格图片 Preview Provider。可重复调用。
pub fn init(cx: &mut App) {
    zcv_workspace::register(ImagePreviewProvider, cx);
}
