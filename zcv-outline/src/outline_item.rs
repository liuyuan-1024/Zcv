//! 大纲面板的单行视图。

use gpui::{
    AnyElement, App, HighlightStyle, IntoElement, MouseButton, StyledText, Window, div, prelude::*,
    px,
};
use zcv_language::OutlineItem;
use zcv_theme::typography;
use zcv_ui::{ButtonLike, SvgIcon, TooltipSpec, TreeRowFrame, tree_row_label};

/// 大纲项在当前编辑器中的稳定身份。
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct OutlineItemKey {
    start: usize,
    end: usize,
    language: &'static str,
    language_depth: u32,
}

impl OutlineItemKey {
    pub(crate) fn from_item(item: &OutlineItem) -> Self {
        Self {
            start: item.range.start,
            end: item.range.end,
            language: item.language,
            language_depth: item.language_depth,
        }
    }

    fn element_id(&self, role: &str) -> String {
        format!(
            "outline-{role}-{}-{}-{}-{}",
            self.start, self.end, self.language, self.language_depth
        )
    }
}

/// 渲染一个大纲项：折叠箭头独立响应，其他区域负责导航。
pub(crate) fn render(
    item: OutlineItem,
    has_children: bool,
    collapsed: bool,
    highlights: Vec<(std::ops::Range<usize>, HighlightStyle)>,
    cx: &App,
    on_toggle: impl Fn(OutlineItemKey, &mut App) + 'static,
    on_navigate: impl Fn(OutlineItem, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let key = OutlineItemKey::from_item(&item);
    let label = StyledText::new(item.text.clone()).with_highlights(highlights);
    let arrow = if has_children {
        let arrow_key = key.clone();
        ButtonLike::new(key.element_id("toggle"))
            .padding(px(0.))
            .tooltip(TooltipSpec::new(if collapsed {
                "展开"
            } else {
                "折叠"
            }))
            .on_click(move |_, _, cx| on_toggle(arrow_key.clone(), cx))
            .child(
                SvgIcon::new(if collapsed {
                    "icons/chevron_right.svg"
                } else {
                    "icons/chevron_down.svg"
                })
                .size(typography::ui_size()),
            )
            .into_any_element()
    } else {
        div().w(typography::ui_size()).into_any_element()
    };

    let row = TreeRowFrame::default()
        .tree_depth(item.depth, cx)
        .leading(arrow)
        .content(tree_row_label(label).text_size(typography::ui_size()));

    row.interactive(key.element_id("row"), cx)
        .on_mouse_down(MouseButton::Left, move |_event, window, cx| {
            on_navigate(item.clone(), window, cx);
            cx.stop_propagation();
        })
        .into_any_element()
}
