//! 把 AppConfig 的视觉字段投影到 shell 全局视觉状态。

use crate::config::AppConfig;
use crate::shell::shared::theme::{syntax, typography};

pub(super) fn apply(config: &AppConfig) {
    typography::set_sizes(config.ui.font_size, config.editor.font_size);
    syntax::set_theme(&config.general.theme);
}
