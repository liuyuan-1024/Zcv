//! 树内拖拽移动：拖拽载荷、跟随预览、落点目标解析与合法性过滤。
//!
//! 拖拽会话持有临时导航状态；落点解析为渲染、高亮和执行提供同一结果。
//! 文件移动与剪切粘贴共用传输管线，文件状态不由拖拽会话持有。

use std::{collections::HashSet, path::Path, rc::Rc};

use gpui::{App, Context, Div, FontWeight, Task, Window, div, prelude::*, px};
use zcv_path::AbsolutePathBuf;
use zcv_theme::{FileIcons, color, scale, typography};
use zcv_ui::SvgIcon;
use zcv_workspace::typography_for_window;

use super::transfer::{filter_movable_sources, paste_target_dir, sanitize_selection};

/// 树内拖拽载荷：被拖行与渲染期冻结的多选标记快照，跟随预览显示的名称。
///
/// 载荷在渲染期随元素构造冻结，拖拽发起与放下都信任这份快照（gpui 的 on_drag 取的是最后一次渲染写入元素状态的值），因此拖拽内容恒等于用户发起拖拽时所见选区，不受拖拽期间任何状态变化影响。
#[derive(Clone)]
pub(crate) struct TreeDrag {
    /// 被拖行路径。
    pub(crate) active_selection: AbsolutePathBuf,
    /// 渲染期快照的多选标记集（可见行序）。
    pub(crate) marked_selections: Rc<[AbsolutePathBuf]>,
    /// 跟随预览显示的条目名（被拖行的名称）。
    pub(crate) preview_name: String,
}

impl TreeDrag {
    /// 参与本次拖拽的源路径：被拖行在多选标记集内 → 整个标记集；否则仅被拖行。
    pub(crate) fn items(&self) -> Vec<AbsolutePathBuf> {
        if self.marked_selections.contains(&self.active_selection) {
            self.marked_selections.to_vec()
        } else {
            vec![self.active_selection.clone()]
        }
    }
}

/// 一次树内拖拽拥有的导航状态；临时展开不写入用户的目录展开意图。
pub(crate) struct DragSession {
    pub(crate) dragged: TreeDrag,
    pub(crate) hovered_directory: Option<AbsolutePathBuf>,
    pub(crate) hover_expand_task: Option<Task<()>>,
    pub(crate) temporary_expanded: HashSet<AbsolutePathBuf>,
}

impl DragSession {
    pub(crate) fn new(dragged: TreeDrag) -> Self {
        Self {
            dragged,
            hovered_directory: None,
            hover_expand_task: None,
            temporary_expanded: HashSet::new(),
        }
    }

    pub(crate) fn clear_hover(&mut self) {
        self.hovered_directory = None;
        self.hover_expand_task = None;
    }
}

/// 拖拽跟随预览视图（拖拽幽灵）：单项为单行（图标 + 名称）；
/// 多选为堆叠卡片——顶层被拖行附醒目数量徽标，其后最多两层后景卡片（真实选中项，逐层右下偏移），使选区拖影与单项拖影一眼可辨。
///
/// 由行元素 `on_drag` 构造为 Entity 注册，拖拽进行时由 gpui 拖拽系统跟随光标渲染。
pub(crate) struct DraggedEntryView {
    drag: TreeDrag,
}

/// 顶层之外的后景卡片层数：堆叠共 3 层，精确数量由徽标传达。
const STACK_BACK_LAYERS: usize = 6;
/// 后景卡片逐层向右下偏移的步长（px）：步长要足够露出堆叠轮廓，与单项拖影一眼可辨。
const LAYER_OFFSET_X: f32 = 6.;
const LAYER_OFFSET_Y: f32 = 8.;

impl DraggedEntryView {
    pub(crate) fn new(drag: TreeDrag) -> Self {
        Self { drag }
    }
}

impl Render for DraggedEntryView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let typography = typography_for_window(window, cx);
        let theme = *color::current(cx);
        let items = self.drag.items();
        let count = items.len();
        if count == 1 {
            // 单项拖影：单行图标 + 名称，无徽标，保持简洁。
            return entry_card(cx)
                .child(
                    SvgIcon::new(entry_icon(&self.drag.active_selection))
                        .size(typography.ui_size()),
                )
                .child(self.drag.preview_name.clone());
        }
        // 多选堆叠卡片：后景层绝对定位逐层右下偏移、先挂载（越远越先），顶层卡片常流最后挂载；
        // gpui 子元素按序绘制，保证顶层在最上且撑起容器尺寸；
        // 后景层越界溢出容器是预期效果（拖拽幽灵不裁剪）。
        let active = &self.drag.active_selection;
        let mut stack = div().relative();
        // 后景层取选区内除被拖行外的前几项（被拖行在快照中的位置不一定居首）。
        let back_layers: Vec<_> = items
            .iter()
            .filter(|path| *path != active)
            .take(STACK_BACK_LAYERS)
            .collect();
        let layer_total = back_layers.len();
        for (i, path) in back_layers.into_iter().rev().enumerate() {
            let depth = (layer_total - i) as f32;
            stack = stack.child(
                entry_card(cx)
                    .border_1()
                    .border_color(theme.border)
                    .absolute()
                    .top(px(LAYER_OFFSET_Y * depth))
                    .left(px(LAYER_OFFSET_X * depth))
                    .text_color(theme.text_muted)
                    .child(SvgIcon::new(entry_icon(path)).size(typography.ui_size()))
                    .child(entry_name(path)),
            );
        }
        stack.child(
            entry_card(cx)
                .border_1()
                .border_color(theme.border)
                .shadow_sm()
                .child(SvgIcon::new(entry_icon(active)).size(typography.ui_size()))
                .child(self.drag.preview_name.clone())
                // 数量徽标：反色实心圆角胶囊（正文色底 + 底色字），任何主题下都高对比醒目。
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_center()
                        .h(scale::S16)
                        .min_w(scale::S16)
                        .px(scale::S2)
                        .rounded_full()
                        .bg(theme.text)
                        .text_color(theme.background)
                        .font_weight(FontWeight::BOLD)
                        .child(format!("{count} 项")),
                ),
        )
    }
}

/// 幽灵卡片基调：与面板行同源（一行高、选中色底、图标 + 名称）。
fn entry_card(cx: &App) -> Div {
    div()
        .flex()
        .flex_row()
        .items_center()
        .gap(scale::S6)
        .px(scale::S6)
        .h(typography::ui_line(cx))
        .rounded_xs()
        .bg(color::current(cx).element_selected)
}

/// 选中条目的图标：目录/文件按本地 metadata 实时判断（后景层每帧至多 2 次，开销可忽略）。
fn entry_icon(path: &Path) -> String {
    if path.is_dir() {
        FileIcons::get_folder_icon(false, path)
    } else {
        FileIcons::get_icon(path)
    }
}

/// 后景卡片显示的条目名：取路径文件名。
fn entry_name(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_default()
}

/// 落点目标目录解析（纯函数）：目录行（含根行）→自身路径；文件行→其父目录。
///
/// 根行即项目根目录行（`is_dir` 为真且路径等于根），命中「目录行→自身」规则；
/// 与粘贴目标推断复用同一实现，避免两处语义漂移。
pub(crate) fn drop_target_dir(
    row_path: &AbsolutePathBuf,
    row_is_dir: bool,
) -> Option<AbsolutePathBuf> {
    paste_target_dir(Some(row_path), row_is_dir)
}

/// 树内拖拽落点：放回本次拖拽载荷中的任一行视为取消整次拖拽。
pub(crate) struct DropCandidate {
    pub(crate) target_dir: AbsolutePathBuf,
    pub(crate) sources: Vec<AbsolutePathBuf>,
}

pub(crate) fn resolve_internal_drop(
    dragged: &TreeDrag,
    row_path: &AbsolutePathBuf,
    row_is_dir: bool,
    roots: &[AbsolutePathBuf],
) -> Option<DropCandidate> {
    let items = dragged.items();
    if items.contains(row_path) {
        return None;
    }
    let target_dir = drop_target_dir(row_path, row_is_dir)?;
    let sources = filter_movable_sources(&sanitize_selection(items, roots), &target_dir);
    (!sources.is_empty()).then_some(DropCandidate {
        target_dir,
        sources,
    })
}

#[cfg(test)]
#[path = "test/drag_tests.rs"]
mod tests;
