//! 大纲面板的树行视图：目录、文件与符号行。

use std::ops::Range;
use std::path::Path;

use gpui::{
    AnyElement, App, HighlightStyle, IntoElement, MouseButton, StyledText, Window, prelude::*,
};
use zcv_editor::OutlineEntry;
use zcv_language::OutlineItem;
use zcv_theme::{FileIcons, typography};
use zcv_ui::{SvgIcon, TreeDisclosure, TreeRowFrame, tree_row_label};

use crate::outline_tree::{OutlineRow, OutlineRowKey, OutlineRowKind};

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

    pub(crate) fn element_id(&self, role: &str) -> String {
        format!(
            "outline-{role}-{}-{}-{}-{}",
            self.start, self.end, self.language, self.language_depth
        )
    }
}

/// 大纲行的折叠呈现状态。
#[derive(Clone, Copy)]
pub(crate) struct OutlineItemFold {
    pub(crate) has_children: bool,
    pub(crate) collapsed: bool,
}

/// 行绘制上下文：窗口与应用状态在整行渲染期间保持不变。
#[derive(Clone, Copy)]
struct RowContext<'a> {
    window: &'a Window,
    cx: &'a App,
}

/// 渲染一个大纲树行。
///
/// 符号行：折叠箭头独立切换，其余区域导航；
/// 目录/文件行：整行切换展开状态，避免与箭头重复响应。
pub(crate) fn render(
    row: OutlineRow,
    fold: OutlineItemFold,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    window: &Window,
    cx: &App,
    on_toggle: impl Fn(OutlineRowKey, &mut App) + 'static,
    on_navigate: impl Fn(OutlineEntry, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let context = RowContext { window, cx };
    match row.kind {
        OutlineRowKind::Symbol(entry) => render_symbol(
            entry,
            row.depth,
            fold,
            highlights,
            context,
            on_toggle,
            on_navigate,
        ),
        OutlineRowKind::Directory { path, name } => {
            render_node(&path, &name, true, row.depth, fold, context, on_toggle)
        }
        OutlineRowKind::File { path, name } => {
            render_node(&path, &name, false, row.depth, fold, context, on_toggle)
        }
    }
}

fn render_symbol(
    entry: OutlineEntry,
    depth: usize,
    fold: OutlineItemFold,
    highlights: Vec<(Range<usize>, HighlightStyle)>,
    context: RowContext<'_>,
    on_toggle: impl Fn(OutlineRowKey, &mut App) + 'static,
    on_navigate: impl Fn(OutlineEntry, &mut Window, &mut App) + 'static,
) -> AnyElement {
    let key = OutlineRowKey::Symbol(OutlineItemKey::from_item(&entry.item));
    let label = StyledText::new(entry.item.text.clone()).with_highlights(highlights);
    let mut row = TreeRowFrame::default()
        .tree_depth(depth, context.window, context.cx)
        .content(tree_row_label(label).text_size(typography::ui_size(context.cx)));
    if fold.has_children {
        let arrow_key = key.clone();
        row = row.leading(TreeDisclosure::new(
            row_element_id(&key, "toggle"),
            !fold.collapsed,
            move |cx| on_toggle(arrow_key.clone(), cx),
        ));
    }
    row.interactive(row_element_id(&key, "row"), context.window, context.cx)
        .on_mouse_down(MouseButton::Left, move |_event, window, cx| {
            on_navigate(entry.clone(), window, cx);
            cx.stop_propagation();
        })
        .into_any_element()
}

fn render_node(
    path: &Path,
    name: &str,
    is_dir: bool,
    depth: usize,
    fold: OutlineItemFold,
    context: RowContext<'_>,
    on_toggle: impl Fn(OutlineRowKey, &mut App) + 'static,
) -> AnyElement {
    let key = if is_dir {
        OutlineRowKey::Directory(path.to_path_buf())
    } else {
        OutlineRowKey::File(path.to_path_buf())
    };
    let icon = if is_dir {
        FileIcons::get_folder_icon(!fold.collapsed, path)
    } else {
        FileIcons::get_icon(path)
    };
    // 目录/文件行不画折叠箭头：文件夹图标本身表达开合，整行负责切换。
    let row = TreeRowFrame::default()
        .tree_depth(depth, context.window, context.cx)
        .leading(SvgIcon::new(icon).size(typography::ui_size(context.cx)))
        .content(
            tree_row_label(StyledText::new(name.to_owned()))
                .text_size(typography::ui_size(context.cx)),
        );
    let click_key = key.clone();
    row.interactive(row_element_id(&key, "row"), context.window, context.cx)
        .on_mouse_down(MouseButton::Left, move |_event, _window, cx| {
            on_toggle(click_key.clone(), cx);
            cx.stop_propagation();
        })
        .into_any_element()
}

fn row_element_id(key: &OutlineRowKey, role: &str) -> String {
    match key {
        OutlineRowKey::Directory(path) => format!("outline-{role}-dir-{}", path.display()),
        OutlineRowKey::File(path) => format!("outline-{role}-file-{}", path.display()),
        OutlineRowKey::Symbol(item) => item.element_id(role),
    }
}
