//! 组件消费的语义颜色。
//!
//! 具体色相和色阶属于主题实现细节。业务组件只通过本模块表达颜色的界面职责。

use gpui::Rgba;

use crate::palette::{self, Palette};

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
    /// 文字或内容选区背景。
    pub element_selection_background: Rgba,
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
    /// 警告状态颜色。
    pub status_warning: Rgba,
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
}

impl ThemeColors {
    fn from_palette(palette: Palette) -> Self {
        Self {
            background: palette.gray.s[0],
            surface_background: palette.gray.s[1],
            elevated_surface_background: palette.gray.s[2],
            element_hover: palette.gray.s[3],
            element_selected: palette.gray.s[3],
            element_selection_background: palette.blue.a[2],
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
            status_warning: palette.yellow.s[6],
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
            editor_active_line_background: palette.gray.s[3],
            editor_line_number: palette.gray.s[6],
            editor_active_line_number: palette.gray.s[8],
            editor_selection_background: palette.blue.a[2],
            editor_cursor: palette.blue.s[6],
        }
    }
}

/// 返回当前主题的语义颜色。
pub fn current() -> ThemeColors {
    ThemeColors::from_palette(palette::current())
}
