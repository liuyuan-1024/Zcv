//! EditorTabBar —— 编辑区标签栏（手册 19 tab group）。
//!
//! 一个 tab ↔ 一个 `View`：把 `ViewSet` 的多视图可视化。本模块只做 UI——
//! 渲染标签、标出活动项与 dirty 态；切换 / 关闭等交互（命令、键位、鼠标）
//!
//! 高度不写死：由标签内文字 line_height + 内边距撑开（见 MEMORY 约定）。
//!
//! 标签数超出栏宽时横向滚动；活动标签每帧滚动进可视区，避免键盘切换后
//! 当前文件的标签停在屏外看不见。`ScrollHandle` 由 `ShellView` 跨帧持有。

use gpui::{AnyElement, Rgba, ScrollHandle, SharedString, Stateful, div, prelude::*};
use zom_command::commands::editor;

use crate::editor_state::{EditorState, EditorTab};
use crate::shell::shared::Glyph;
use crate::shell::{CommandTitleLookup, ShortcutLookup};
use crate::theme::{color, radius, space, typography};

pub(crate) fn render(
    state: &EditorState,
    scroll: ScrollHandle,
    shortcuts: &ShortcutLookup,
    titles: &CommandTitleLookup,
) -> Stateful<gpui::Div> {
    // 把活动标签滚进可视区——scroll_to_item 记下目标，实际滚动在 prepaint 完成。
    if let Some(active) = state.tabs.iter().position(|tab| tab.is_active) {
        scroll.scroll_to_item(active);
    }

    let mut bar = div()
        .id("editor-tab-bar")
        .track_scroll(&scroll)
        .flex()
        .flex_row()
        .items_center()
        .w_full()
        .min_h_0()
        .bg(color::gray::s04())
        .border_color(color::gray::s05())
        .overflow_hidden()
        .overflow_x_scroll();

    for tab in &state.tabs {
        bar = bar.child(render_tab(tab, shortcuts, titles));
    }
    bar
}

fn render_tab(
    tab: &EditorTab,
    shortcuts: &ShortcutLookup,
    titles: &CommandTitleLookup,
) -> Stateful<gpui::Div> {
    // 活动标签背景与编辑器正文一致，标签与内容视觉连成一体；
    // 非活动标签透明底，沿用标签栏自身的 s04 底色。
    let (bg, text) = if tab.is_active {
        (color::gray::s01(), color::gray::s09())
    } else {
        (gpui::rgba(0), color::gray::s09())
    };

    // 每个标签一个唯一 group：让关闭 glyph 只在悬停「本标签」时显现，而不会因为同名 group 连带点亮其它标签。
    let hover_group = SharedString::from(format!("editor-tab-{}", tab.element_key()));

    div()
        .id(("editor-tab", tab.element_key() as usize))
        .group(hover_group.clone())
        .flex()
        .flex_row()
        .items_center()
        .flex_shrink_0()
        .gap(space::s4())
        .p(space::s4())
        .bg(bg)
        .text_size(typography::ui())
        .line_height(typography::ui_line())
        .text_color(text)
        // 修改标志放文字左侧；标志槽常驻，dirty 切换时文字不跳。
        .child(dirty_marker(tab.dirty, text))
        .child(div().whitespace_nowrap().child(tab.title.clone()))
        .child(close_glyph(tab, shortcuts, titles, hover_group))
}

/// 标签右侧的关闭标记：纯视觉 `Glyph` + tooltip（含 mod-w 快捷键）。
/// 不接鼠标点击——关闭动作由 keymap 的 `editor.close_tab` 承载。
///
/// 默认 `opacity(0)` 隐身、仅占位（不挪动标签宽度），悬停本标签时浮现。
fn close_glyph(
    tab: &EditorTab,
    shortcuts: &ShortcutLookup,
    titles: &CommandTitleLookup,
    hover_group: SharedString,
) -> AnyElement {
    let close_title = titles(editor::CLOSE_TAB).unwrap_or_else(|| editor::CLOSE_TAB.to_string());
    let glyph = Glyph::icon(
        ("editor-tab-close", tab.element_key() as usize),
        "icons/actions/close.svg",
        close_title,
    )
    .hint(shortcuts(editor::CLOSE_TAB))
    .active(tab.is_active)
    .render();

    div()
        .opacity(0.0)
        .group_hover(hover_group, |style| style.opacity(1.0))
        .child(glyph)
        .into_any_element()
}

/// 文字左侧的修改标志槽。
///
/// 槽宽对齐右侧关闭 Glyph，让文字两侧对称；槽内 dirty 时填
/// 一个小圆点、否则透明——固定占位避免 dirty 切换时文字跳动。
/// 纯视觉标记，固定尺寸是 MEMORY 约定里的例外。
fn dirty_marker(dirty: bool, color: Rgba) -> gpui::Div {
    let mut dot = div().size(space::s4());
    if dirty {
        dot = dot.rounded(radius::full()).bg(color);
    }
    div()
        .flex_shrink_0()
        .size(typography::ui_line())
        .flex()
        .items_center()
        .justify_center()
        .child(dot)
}
