//! 把 AppConfig 的视觉字段投影到 shell 全局视觉状态。

use gpui::Window;

use crate::config::AppConfig;
use crate::theme::{Theme, typography};

pub(super) fn apply(config: &AppConfig, window: Option<&Window>) {
    typography::set_sizes(config.ui.font_size, config.editor.font_size);
    Theme::from_config(&config.general.theme).apply(window);
}
