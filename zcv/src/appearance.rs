//! 从用户设置派生应用级主题与基础排版。

use gpui::{App, Global, Window, WindowAppearance};
use zcv_settings::{SettingsStore, UserSettings};
use zcv_theme::{ThemeChoice, typography};

struct SystemAppearance(WindowAppearance);

impl Global for SystemAppearance {}

pub(crate) fn init(cx: &mut App) {
    let settings = SettingsStore::get(cx);
    let mut choice = settings.theme;
    let mut base_sizes = typography_sizes(&settings);
    apply_typography(cx, base_sizes);

    let system_appearance = cx.window_appearance();
    cx.set_global(SystemAppearance(system_appearance));
    choice.apply_for_appearance(cx, system_appearance);

    cx.observe_global::<SettingsStore>(move |cx| {
        let settings = SettingsStore::get(cx);
        let sizes = typography_sizes(&settings);
        if base_sizes != sizes {
            base_sizes = sizes;
            apply_typography(cx, sizes);
        }
        if choice != settings.theme {
            choice = settings.theme;
            choice.apply_for_appearance(cx, cx.global::<SystemAppearance>().0);
            cx.refresh_windows();
        }
    })
    .detach();
}

fn typography_sizes(settings: &UserSettings) -> (f32, f32, f32) {
    (
        settings.content_font_size,
        settings.ui_font_size,
        settings.content_line_height,
    )
}

fn apply_typography(cx: &mut App, (content_size, ui_size, line_height): (f32, f32, f32)) {
    typography::set_base_typography(cx, Some(content_size), Some(ui_size), Some(line_height));
}

/// GPUI 通过窗口报告系统外观变化。窗口订阅随窗口释放。
pub(crate) fn observe_window(window: &Window) {
    window
        .observe_window_appearance(|_, cx| {
            let appearance = cx.window_appearance();
            if cx.global::<SystemAppearance>().0 == appearance {
                return;
            }
            cx.set_global(SystemAppearance(appearance));
            if SettingsStore::get(cx).theme == ThemeChoice::System {
                ThemeChoice::System.apply_for_appearance(cx, appearance);
                cx.refresh_windows();
            }
        })
        .detach();
}
