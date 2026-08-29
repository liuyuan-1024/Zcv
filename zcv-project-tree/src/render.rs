//! 行模型渲染：可见行列表、行元素（背景/拖拽/点击交互）与渲染上下文。

use std::cell::RefCell;
use std::path::PathBuf;
use std::rc::Rc;

use crate::git_status_color;
use gpui::{
    App, Div, ElementId, Entity, MouseButton, UniformListScrollHandle, WeakEntity, div, prelude::*,
    uniform_list,
};
use zcv_editor::Editor;
use zcv_git::FileStatus;
use zcv_theme::color;
use zcv_ui::Scrollbar;
use zcv_ui::tree::{self, TreeState};

use super::drag::{DraggedEntryView, TreeDrag, drop_target_dir, filter_movable_sources};
use super::editing::{EditOperation, EditState};
use super::{ProjectTreePanel, ProjectTreeRow};

impl ProjectTreePanel {
    pub(super) fn display_rows(&self, cx: &gpui::App) -> Rc<[ProjectTreeRow]> {
        let Some(EditState {
            operation: EditOperation::Create { parent },
            ..
        }) = &self.edit_state
        else {
            return Rc::clone(&self.row_snapshot);
        };
        let Some((index, depth)) = self
            .row_snapshot
            .iter()
            .enumerate()
            .find(|(_, row)| !row.is_new && &row.path == parent)
            .map(|(index, row)| (index, row.depth + 1))
        else {
            return Rc::clone(&self.row_snapshot);
        };
        let mut rows = self.row_snapshot.to_vec();
        rows.insert(
            index + 1,
            ProjectTreeRow {
                path: parent.clone(),
                name: String::new(),
                depth,
                is_dir: self.entry_name_editor.read(cx).text(cx).ends_with('/'),
                expanded: false,
                is_new: true,
                git_status: None,
            },
        );
        rows.into()
    }
}

pub(super) fn render_empty_state(cx: &App) -> Div {
    div()
        .size_full()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .text_color(color::current(cx).text_placeholder)
        .child("没有打开的项目")
}
pub(super) fn render_list(
    scroll_handle: &UniformListScrollHandle,
    scrollbar: &Scrollbar<UniformListScrollHandle>,
    len: usize,
    is_focused: bool,
    render_context: ProjectTreeRenderContext,
) -> gpui::UniformList {
    let handle = scroll_handle.clone();

    uniform_list("project-tree-list", len, move |range, _, cx| {
        let mut render_context = render_context.clone();
        let state = render_context.state.borrow();
        let rows = &render_context.rows;
        // 拖拽载荷的多选标记快照：每帧按可见行序展开一次，所有行共享同一份。
        render_context.drag_marked = rows
            .iter()
            .filter(|row| state.is_in_selection_set(&row.path))
            .map(|row| row.path.clone())
            .collect();
        range
            .filter_map(|i| rows.get(i))
            .map(|row| {
                let sel = row.is_new || state.selected.as_ref() == Some(&row.path);
                let marked = !row.is_new && render_context.active_path.as_ref() == Some(&row.path);
                let in_set = !row.is_new && state.is_in_selection_set(&row.path);
                render_row(
                    row,
                    sel,
                    marked,
                    in_set,
                    is_focused,
                    render_context.clone(),
                    cx,
                )
                .into_any_element()
            })
            .collect()
    })
    .size_full()
    .track_scroll(handle)
    .with_decoration(scrollbar.clone())
}
pub(super) fn render_row(
    row: &ProjectTreeRow,
    sel: bool,
    marked: bool,
    in_set: bool,
    focused: bool,
    render_context: ProjectTreeRenderContext,
    cx: &mut App,
) -> impl IntoElement {
    let path = row.path.clone();
    let is_dir = row.is_dir;
    let depth = row.depth;
    let name = row.name.clone();
    let expanded = row.expanded;
    let is_editing = render_context
        .edit_state
        .as_ref()
        .is_some_and(|edit_state| edit_state.matches_row(row));
    let has_error = is_editing
        && render_context
            .edit_state
            .as_ref()
            .is_some_and(|edit_state| edit_state.validation_error.is_some());
    let content = if is_editing {
        div()
            .key_context("ProjectTreeEdit")
            .flex_1()
            .overflow_hidden()
            .when(has_error, |element| {
                element
                    .border_1()
                    .border_color(color::current(cx).status_error)
            })
            .child(render_context.entry_name_editor.clone())
    } else {
        // 淡显优先级：编辑态（外层分支）> cut 淡显 > git 颜色。
        // 剪切项整体淡显（含其子路径），git 状态色不再叠加。
        let is_cut = !row.is_new
            && render_context
                .clipboard_cut
                .iter()
                .any(|cut| row.path.starts_with(cut));
        let status_color = if is_cut {
            None
        } else {
            row.git_status
                .and_then(|status| git_status_color(status, cx))
        };
        let is_ignored = !is_cut && matches!(row.git_status, Some(FileStatus::Ignored));
        div()
            .flex_1()
            .overflow_hidden()
            .truncate()
            .when(is_cut, |element| {
                element.text_color(color::current(cx).text_muted)
            })
            .when_some(status_color, |element, status_color| {
                element.text_color(status_color)
            })
            .when(is_ignored && status_color.is_none(), |element| {
                element.text_color(color::current(cx).text_muted)
            })
            .child(name)
    };

    // 拖拽载荷 = 被拖行 + 渲染期冻结的多选标记快照（对齐 Zed 的 DraggedSelection）。
    // gpui 的 on_drag 载荷在渲染期随元素构造冻结、发起时直接取用；
    // 快照与视觉同帧——任何修改选区的交互都会触发重绘，故载荷恒等于用户所见选区，发起与放下都信任它。
    // 编辑态、冲突浮层活跃或临时新建行时不允许发起拖拽。
    let drag_payload = if is_editing || render_context.drag_blocked || row.is_new {
        None
    } else {
        Some(TreeDrag {
            active_selection: path.clone(),
            marked_selections: render_context.drag_marked.clone(),
            preview_name: row.name.clone(),
        })
    };
    // 行唯一元素 id：新建占位行的 path 是父目录路径，与父目录行 id 冲突，追加后缀区分。
    let row_id = ElementId::Name(
        format!(
            "project-tree-row-{}{}",
            path.display(),
            if row.is_new { "-new" } else { "" }
        )
        .into(),
    );

    tree::render_row_base(depth, &row.path, is_dir, row.expanded, content, cx)
        .id(row_id)
        .cursor_pointer()
        // 多选标记用选中背景；活动文件标记用更弱的悬停背景，两者不同色——用户据此区分「选区成员」与「编辑器当前打开的文件」，避免把后者误当作选区参与拖拽。
        .when(marked, |el| el.bg(color::current(cx).element_hover))
        .when(in_set, |el| el.bg(color::current(cx).element_selected))
        .hover(|style| style.bg(color::current(cx).element_hover))
        .when(sel && focused, |el| el.child(tree::selection_border(cx)))
        .when_some(drag_payload, |element, drag| {
            let weak = render_context.weak.clone();
            element.on_drag(drag, move |drag, _, _, cx| {
                // 拖拽正式开始：清掉上一次拖拽遗留的悬停展开状态。
                // 载荷即渲染期冻结的选区快照（见 drag_payload 处源码结论），与用户按下时所见选区一致；
                // 堆叠预览与数量徽标都以载荷 items 为准。
                if let Some(tree) = weak.upgrade() {
                    tree.update(cx, |tree, _| tree.reset_drag_hover());
                }
                cx.new(|_| DraggedEntryView::new(drag.clone()))
            })
        })
        // 落点候选行高亮（主题悬停色）：仅对存在可移动项的落点提示，移入自身子树、落回原目录等非法落点不亮。
        .drag_over::<TreeDrag>({
            let path = path.clone();
            move |mut style: gpui::StyleRefinement, dragged: &TreeDrag, _, cx| {
                let droppable = drop_target_dir(&path, is_dir).is_some_and(|target| {
                    !filter_movable_sources(&dragged.items(), &target).is_empty()
                });
                if droppable {
                    style.background = Some(gpui::Fill::from(color::current(cx).element_hover));
                }
                style
            }
        })
        // 拖拽悬停自动展开：悬停折叠目录行约 500ms 后展开（计时器存面板字段）。
        // on_drag_move 在捕获阶段对全部注册行派发且不做命中检测：仅光标真正位于本行范围内才处理，命中后阻断其余行的后续派发。
        .on_drag_move::<TreeDrag>({
            let weak = render_context.weak.clone();
            let path = path.clone();
            move |drag_event, _window, cx| {
                if !drag_event.bounds.contains(&drag_event.event.position) {
                    return;
                }
                if let Some(tree) = weak.upgrade() {
                    tree.update(cx, |tree, cx| {
                        tree.handle_drag_hover(path.clone(), is_dir, expanded, cx);
                    });
                }
                // 行互不重叠：命中行处理后阻断，避免其余行的清理逻辑覆盖刚调度的计时。
                cx.stop_propagation();
            }
        })
        // 放下：解析目标目录并复用 M3 的 Move + 冲突确认管线（含守卫与非法项过滤）。
        .on_drop({
            let weak = render_context.weak.clone();
            let path = path.clone();
            move |dragged: &TreeDrag, _window, cx| {
                if let Some(tree) = weak.upgrade() {
                    tree.update(cx, |tree, cx| {
                        tree.handle_row_drop(dragged, &path, is_dir, cx)
                    });
                }
            }
        })
        .when(!is_editing, |row| {
            // 预先克隆路径：下方 mouse_down 与 on_click 两个 move 闭包各自消费一份。
            let click_path = path.clone();
            row.on_mouse_down(MouseButton::Left, {
                // 焦点句柄与 weak 引用先取出，避免 move 整个 render_context。
                let focus = render_context.focus.clone();
                let weak = render_context.weak.clone();
                move |event, window, cx| {
                    let was_focused = focus.contains_focused(window, cx);
                    window.focus(&focus);
                    // 已聚焦时的修饰键多选：shift 从锚点扩展区间、secondary 切换单项标记，都只改选中态不触发展开/打开；未聚焦首击保持纯聚焦+选中。
                    if was_focused && event.modifiers.shift {
                        if let Some(tree) = weak.upgrade() {
                            tree.update(cx, |tree, cx| {
                                // 修饰键点击只改选中态、不打开/展开：清掉上次残留的意图。
                                tree.pending_click_intent = None;
                                tree.state.borrow_mut().extend_to(&path);
                                cx.notify();
                            });
                        }
                        cx.stop_propagation();
                        return;
                    }
                    if was_focused && event.modifiers.secondary() {
                        if let Some(tree) = weak.upgrade() {
                            tree.update(cx, |tree, cx| {
                                // 修饰键点击只改选中态、不打开/展开：清掉上次残留的意图。
                                tree.pending_click_intent = None;
                                tree.state.borrow_mut().toggle_selection(&path);
                                cx.notify();
                            });
                        }
                        cx.stop_propagation();
                        return;
                    }
                    // 收拢/保持判定依据按下时刻的实时集合（渲染期 in_set 只是行背景快照）：
                    // 行已不在多选集合内时收拢为单选；在集合内则保持（多选整体拖拽移动的前提）。
                    let in_set_now = weak.upgrade().is_some_and(|tree| {
                        tree.read(cx).state.borrow().is_in_selection_set(&path)
                    });
                    if let Some(tree) = weak.upgrade() {
                        tree.update(cx, |tree, cx| {
                            // 行已不在多选集合内时收拢为单选；在集合内保持选中集。
                            // 收拢清空集合后必须立即重绘：行背景与拖拽载荷都在渲染期求值，不通知时旧帧高亮残留，用户感知「选区还在」而拖拽已退化为单项。
                            if !in_set_now {
                                tree.state.borrow_mut().select(path.clone());
                                cx.notify();
                            }
                            // 打开/展开动作延迟到 click（mouse_up 未拖拽）派发时执行：
                            // 按下即执行会在拖动时误打开文件预览——打开文件经 reveal_active_path 的 select() 清空多选集合，选区拖拽随之中途退化为单项；
                            // 拖动目录行也会误展开/折叠。
                            // 未聚焦首击只聚焦不动作，意图记 None；
                            // 拖拽消费了 click 时意图残留，下次按下清掉。
                            tree.pending_click_intent =
                                tree::row_mouse_down_action(is_dir, event.click_count, was_focused)
                                    .map(|action| (path.clone(), action));
                        });
                    }
                    cx.stop_propagation();
                }
            })
            .on_click({
                let weak = render_context.weak.clone();
                let path = click_path;
                move |event: &gpui::ClickEvent, window, cx| {
                    // click 派发 = 按下与抬起在同一行且未拖拽（拖拽发起会取走按压记录，mouse_up 不再派发 click）；
                    // 右击走上下文菜单，不消费按下意图。
                    if event.is_right_click() {
                        return;
                    }
                    if let Some(tree) = weak.upgrade() {
                        tree.update(cx, |tree, cx| {
                            // 消费按下时记录的意图执行打开/展开；
                            // 拖拽路径上意图残留，由下次按下（各分支开头）清掉，不会造成延后误触发。
                            if let Some((intent_path, action)) = tree.pending_click_intent.take()
                                && intent_path == path
                            {
                                match action {
                                    tree::RowClickAction::Toggle => {
                                        tree.activate_selected(true, window, cx)
                                    }
                                    tree::RowClickAction::Preview => {
                                        tree.activate_selected(false, window, cx)
                                    }
                                    tree::RowClickAction::Activate => {
                                        tree.activate_selected(true, window, cx)
                                    }
                                }
                            }
                        });
                    }
                }
            })
        })
}

/// 行渲染上下文：渲染期冻结的面板状态快照，行闭包借引用读取。
#[derive(Clone)]
pub(super) struct ProjectTreeRenderContext {
    pub(super) state: Rc<RefCell<TreeState<PathBuf, ProjectTreeRow>>>,
    pub(super) rows: Rc<[ProjectTreeRow]>,
    pub(super) focus: gpui::FocusHandle,
    /// 条目点击直接调用 Entity 方法（对齐 Zed 的 `cx.listener` 路径），
    /// 不依赖 dispatch_action 的焦点链分发。
    pub(super) weak: WeakEntity<ProjectTreePanel>,
    pub(super) edit_state: Option<EditState>,
    pub(super) entry_name_editor: Entity<Editor>,
    /// 活动文件标记（渲染时快照，与选中行独立）。
    pub(super) active_path: Option<PathBuf>,
    /// 剪切剪贴板路径快照：命中行淡显（Copy 无淡显）。
    pub(super) clipboard_cut: Rc<[PathBuf]>,
    /// 拖拽载荷的多选标记快照（渲染期按可见行序展开一次，所有行共享）。
    pub(super) drag_marked: Rc<[PathBuf]>,
    /// 禁止发起拖拽（编辑态或冲突浮层活跃时为真）。
    pub(super) drag_blocked: bool,
}
