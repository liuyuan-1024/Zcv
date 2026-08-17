//! 组件消费的语义颜色。
//!
//! 具体色相和色阶属于主题实现细节。业务组件只通过本模块表达颜色的界面职责。
//!
//! **缓存语义**：当前主题语义色由 gpui global 承载，主题切换时构建一次并整体替换；
//! `current(cx)` 返回借引用，每帧每元素零拷贝、零原子操作。

use std::sync::OnceLock;

use gpui::{App, Global, Rgba};

use crate::palette::Palette;
use crate::theme_data::ThemeData;

/// 当前主题语义色的 gpui global 载体（对齐 Zed 的 ThemeRegistry）。
struct ThemeColorsGlobal(ThemeColors);

impl Global for ThemeColorsGlobal {}

/// 当前主题提供给 UI 组件的颜色角色。
#[derive(Clone, Copy, Debug)]
pub struct ThemeColors {
    /// 应用最底层背景。
    pub background: Rgba,
    /// 面板、编辑区等基础表面背景。
    pub surface_background: Rgba,
    /// Tooltip、菜单等浮层背景。
    pub elevated_surface_background: Rgba,
    /// 普通元素的悬停背景。
    pub element_hover: Rgba,
    /// 普通元素的选中背景。
    pub element_selected: Rgba,
    /// 弱分隔边框。
    pub border_variant: Rgba,
    /// 焦点或强调边框。
    pub border_focused: Rgba,
    /// 默认正文颜色。
    pub text: Rgba,
    /// 弱化正文颜色。
    pub text_muted: Rgba,
    /// 禁用正文颜色。
    pub text_disabled: Rgba,
    /// 占位内容颜色。
    pub text_placeholder: Rgba,
    /// 默认图标颜色。
    pub icon: Rgba,
    /// 弱化图标颜色。
    pub icon_muted: Rgba,
    /// 强调色背景上的图标颜色。
    pub icon_on_accent: Rgba,
    /// 强调图标颜色。
    pub icon_accent: Rgba,
    /// 成功状态颜色。
    pub status_success: Rgba,
    /// 错误状态颜色。
    pub status_error: Rgba,
    /// 新建/未跟踪条目颜色。
    pub status_created: Rgba,
    /// 已修改条目颜色。
    pub status_modified: Rgba,
    /// 已删除条目颜色。
    pub status_deleted: Rgba,
    /// 冲突条目颜色。
    pub status_conflict: Rgba,
    /// 顶栏背景。
    pub title_bar_background: Rgba,
    /// 状态栏背景。
    pub status_bar_background: Rgba,
    /// 标签栏背景。
    pub tab_bar_background: Rgba,
    /// 活动标签背景。
    pub tab_active_background: Rgba,
    /// 工具栏背景。
    pub toolbar_background: Rgba,
    /// 面板背景。
    pub panel_background: Rgba,
    /// 编辑区背景。
    pub editor_background: Rgba,
    /// 编辑器 gutter 背景。
    pub editor_gutter_background: Rgba,
    /// 编辑器活动行背景。
    pub editor_active_line_background: Rgba,
    /// 编辑器普通行号颜色。
    pub editor_line_number: Rgba,
    /// 编辑器活动行号颜色。
    pub editor_active_line_number: Rgba,
    /// 编辑器选区背景。
    pub editor_selection_background: Rgba,
    /// 编辑器光标颜色。
    pub editor_cursor: Rgba,
    /// 编辑器 diff 新增行背景（status_created 的低透明版本）。
    pub editor_diff_added_background: Rgba,
    /// 编辑器 diff 修改行背景（status_modified 的低透明版本）。
    pub editor_diff_modified_background: Rgba,
    /// 编辑器 diff 删除行背景（status_deleted 的低透明版本）。
    pub editor_diff_deleted_background: Rgba,
    /// 滚动轴轨道背景（默认透明，marker 与 thumb 绘制在其上方）。
    pub scrollbar_track_background: Rgba,
    /// 滚动轴 thumb 静止色。
    pub scrollbar_thumb_background: Rgba,
    /// 滚动轴 thumb 悬停色。
    pub scrollbar_thumb_hover_background: Rgba,
    /// 滚动轴 thumb 拖动色。
    pub scrollbar_thumb_active_background: Rgba,
}

impl ThemeColors {
    fn from_palette(palette: Palette) -> Self {
        Self {
            background: palette.gray.s[0],
            surface_background: palette.gray.s[1],
            elevated_surface_background: palette.gray.s[2],
            element_hover: palette.gray.s[3],
            element_selected: palette.gray.s[3],
            border_variant: palette.gray.s[4],
            border_focused: palette.blue.s[6],
            text: palette.gray.s[8],
            text_muted: palette.gray.s[7],
            text_disabled: palette.gray.s[6],
            text_placeholder: palette.gray.s[5],
            icon: palette.gray.s[7],
            icon_muted: palette.gray.s[6],
            icon_on_accent: palette.gray.s[0],
            icon_accent: palette.blue.s[6],
            status_success: palette.green.s[6],
            status_error: palette.red.s[6],
            status_created: palette.green.s[6],
            status_modified: palette.yellow.s[6],
            status_deleted: palette.red.s[6],
            status_conflict: palette.red.s[6],
            title_bar_background: palette.gray.s[2],
            status_bar_background: palette.gray.s[2],
            tab_bar_background: palette.gray.s[2],
            tab_active_background: palette.gray.s[1],
            toolbar_background: palette.gray.s[1],
            panel_background: palette.gray.s[1],
            editor_background: palette.gray.s[1],
            editor_gutter_background: palette.gray.s[1],
            // 当前行背景用半透明版本：滚动轴 thumb 同为 gray 色相，实色会与其在同一行处视觉融合。
            editor_active_line_background: Rgba {
                a: 0.5,
                ..palette.gray.s[3]
            },
            editor_line_number: palette.gray.s[6],
            editor_active_line_number: palette.gray.s[8],
            editor_selection_background: palette.blue.a[2],
            editor_cursor: palette.blue.s[6],
            // diff 背景 = 状态色 30% 透明度（深色/浅色主题下都成立）。
            editor_diff_added_background: Rgba {
                a: 0.3,
                ..palette.green.s[6]
            },
            editor_diff_modified_background: Rgba {
                a: 0.3,
                ..palette.yellow.s[6]
            },
            editor_diff_deleted_background: Rgba {
                a: 0.3,
                ..palette.red.s[6]
            },
            // 滚动轴 track 透明；thumb 用中性色 s[6] 叠加 45/55/65% 不透明度：
            // 原 a 阶梯 3/4/5 级（24/25/30%）过浅，与当前行背景同色相时难以区分。
            scrollbar_track_background: Rgba {
                r: 0.,
                g: 0.,
                b: 0.,
                a: 0.,
            },
            scrollbar_thumb_background: Rgba {
                a: 0.45,
                ..palette.gray.s[6]
            },
            scrollbar_thumb_hover_background: Rgba {
                a: 0.55,
                ..palette.gray.s[6]
            },
            scrollbar_thumb_active_background: Rgba {
                a: 0.65,
                ..palette.gray.s[6]
            },
        }
    }
}

/// 切换主题：从注册表主题数据构建语义色快照 → 写入 gpui global（整体替换）。
pub(crate) fn set_theme(theme: &ThemeData, cx: &mut App) {
    cx.set_global(ThemeColorsGlobal(ThemeColors::from_palette(theme.palette)));
}

/// 返回当前主题的语义色（对齐 Zed `cx.theme()` 的借引用语义）。
///
/// 主题尚未设置（窗口构建前）时返回注册表首个主题的默认快照。
pub fn current(cx: &App) -> &ThemeColors {
    cx.try_global::<ThemeColorsGlobal>()
        .map(|global| &global.0)
        .unwrap_or_else(|| {
            static DEFAULT: OnceLock<ThemeColors> = OnceLock::new();
            DEFAULT.get_or_init(|| {
                let theme = super::first_theme();
                ThemeColors::from_palette(theme.palette)
            })
        })
}
