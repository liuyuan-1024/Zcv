//! 栅格图片文件预览。
//!
//! 图片是二进制文件，不能由文本编辑器提供源码 Item；
//! 本 crate 通过 `PreviewProvider` 创建图片预览 Item，图片内容由 `Project` 的 `ImageDocument` 在后台读取和解码。

mod provider;
mod view;

use gpui::App;
use provider::ImagePreviewProvider;

/// 注册栅格图片 Preview Provider。可重复调用。
pub fn init(cx: &mut App) {
    zcv_workspace::register(ImagePreviewProvider, cx);
}
