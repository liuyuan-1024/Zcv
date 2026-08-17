//! BranchPicker —— git 分支选择器。
//!
//! 自含 glyph 按钮 + 浮层，浮层内嵌 `Picker<BranchPickerDelegate>`。
//! 分支列表由 Workspace 订阅 GitStore 事件后推送（同步快照，打开即渲染，无加载态）；
//! 切换/创建分支通过回调转发到 git_store 后台执行，完成后 GitStore 自动重扫并推送新列表。
//!
//! 搜索无匹配时列表尾部追加"创建分支"虚拟行：以当前 HEAD 为基创建并切换（对齐 Zed 的 Entry::NewBranch）。

use std::cell::Cell;
use std::rc::Rc;

use gpui::{
    App, Context, Corner, Entity, FocusHandle, MouseButton, Pixels, Render, Window, anchored,
    deferred, div, point, prelude::*, px,
};
use zcv_git::Branch;
use zcv_picker::{Picker, PickerDelegate};
use zcv_theme::{color, space};
use zcv_ui::Glyph;
use zcv_ui::ListItem;

const PICKER_WIDTH: Pixels = px(360.0);

use zcv_actions::SelectGitBranch;

// ═══ 回调 ════════════════════════════════════════════════════════

/// 分支操作请求：切换分支 / 以当前 HEAD 为基创建分支。
pub enum GitBranchAction {
    Checkout(String),
    Create(String),
}

/// 分支操作回调 —— 参数为操作请求。
pub type OnBranchSelected = Rc<dyn Fn(GitBranchAction, &mut Window, &mut App)>;

// ═══ 数据源 ═══════════════════════════════════════════════════════

/// 分支选择器数据源。
struct BranchPickerDelegate {
    query: String,
    branches: Vec<Branch>,
    filtered: Vec<usize>,
    selected_index: usize,
    on_select: OnBranchSelected,
}

impl BranchPickerDelegate {
    fn new(branches: Vec<Branch>, on_select: OnBranchSelected) -> Self {
        let filtered: Vec<usize> = (0..branches.len()).collect();
        let selected_index = branches
            .iter()
            .position(|branch| branch.is_head)
            .unwrap_or(0);
        Self {
            query: String::new(),
            branches,
            filtered,
            selected_index,
            on_select,
        }
    }

    /// 替换分支列表并重过滤（toggle 打开时调用；空 query 自动回到当前分支）。
    fn reload(&mut self, branches: Vec<Branch>) {
        self.branches = branches;
        self.do_filter();
    }

    /// 搜索无匹配且 query 非空时，列表尾部追加"创建分支"虚拟行。
    fn create_row_visible(&self) -> bool {
        !self.query.is_empty() && self.filtered.is_empty()
    }

    fn do_filter(&mut self) {
        if self.query.is_empty() {
            self.filtered = (0..self.branches.len()).collect();
            self.selected_index = self.branches.iter().position(|b| b.is_head).unwrap_or(0);
        } else {
            let q = self.query.to_lowercase();
            self.filtered = self
                .branches
                .iter()
                .enumerate()
                .filter(|(_, b)| b.name.to_lowercase().contains(&q))
                .map(|(i, _)| i)
                .collect();
            // 无匹配时选中"创建分支"虚拟行；有匹配时钳制到列表内。
            self.selected_index = if self.filtered.is_empty() {
                0
            } else {
                self.selected_index
                    .min(self.filtered.len().saturating_sub(1))
            };
        }
    }
}

impl PickerDelegate for BranchPickerDelegate {
    fn match_count(&self) -> usize {
        self.filtered.len() + usize::from(self.create_row_visible())
    }

    fn selected_index(&self) -> usize {
        self.selected_index
    }

    fn set_selected_index(&mut self, ix: usize) {
        self.selected_index = ix;
    }

    fn update_matches(&mut self, query: String) {
        self.query = query;
        self.do_filter();
    }

    fn confirm(&mut self, window: &mut Window, cx: &mut App) {
        if self.match_count() == 0 {
            return;
        }
        if self.create_row_visible() {
            // 无匹配分支 → 以当前 HEAD 为基创建。
            let cb = self.on_select.clone();
            cb(GitBranchAction::Create(self.query.clone()), window, cx);
        } else {
            let branch = &self.branches[self.filtered[self.selected_index]];
            let cb = self.on_select.clone();
            cb(GitBranchAction::Checkout(branch.name.clone()), window, cx);
        }
    }

    fn dismissed(&mut self) {}

    fn render_match(
        &self,
        index: usize,
        is_selected: bool,
        _cx: &mut Context<Picker<Self>>,
    ) -> gpui::AnyElement {
        if index == self.filtered.len() {
            return ListItem::new(("create-branch", index))
                .toggle_state(is_selected)
                .child(format!("创建分支：{}", self.query))
                .subtitle("从当前分支创建")
                .into_any_element();
        }
        let branch = &self.branches[self.filtered[index]];
        let row = ListItem::new(index)
            .toggle_state(is_selected)
            .child(branch.name.clone());
        // 当前分支行尾标 ✓。
        let row = if branch.is_head {
            row.end_slot(Glyph::icon(("head", index), "icons/check.svg").label("当前分支"))
        } else {
            row
        };
        row.into_any_element()
    }

    fn placeholder_text(&self) -> &str {
        "搜索分支..."
    }
}

// ═══ Entity ═════════════════════════════════════════════════════

/// 分支选择器 —— 自含 glyph 按钮 + 浮层。
///
/// glyph 显示当前分支名，无分支（空仓库/detached）时显示占位 `--`。
pub struct BranchPicker {
    is_open: bool,
    dismiss_flag: Rc<Cell<bool>>,
    focus: FocusHandle,
    picker: Entity<Picker<BranchPickerDelegate>>,
    /// 当前分支名（由 Workspace 订阅 GitStore 的 Head 事件刷新）。
    current_branch: Option<String>,
    /// 分支列表快照（由 Workspace 订阅 GitStore 事件推送；打开时同步渲染）。
    branches: Vec<Branch>,
}

impl BranchPicker {
    pub fn new(on_select: OnBranchSelected, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let delegate = BranchPickerDelegate::new(Vec::new(), on_select);
        let dismiss_flag = Rc::new(Cell::new(false));

        let picker = cx.new(|cx| Picker::new(delegate, PICKER_WIDTH, window, cx));
        let on_dismiss = {
            let df = dismiss_flag.clone();
            Box::new(move |window: &mut Window, _app: &mut App| {
                df.set(true);
                window.refresh();
            })
        };
        picker.update(cx, |picker, _| picker.set_on_dismiss(on_dismiss));

        Self {
            is_open: false,
            dismiss_flag,
            focus: cx.focus_handle(),
            picker,
            current_branch: None,
            branches: Vec::new(),
        }
    }

    /// 设置当前分支名（glyph 显示）。
    pub fn set_branch(&mut self, branch: Option<String>) {
        self.current_branch = branch;
    }

    /// 设置分支列表快照（打开时同步渲染，无加载态）。
    pub fn set_branches(&mut self, branches: Vec<Branch>) {
        self.branches = branches;
    }

    /// 外部切换（快捷键/glyph 点击等）。
    pub fn toggle(&mut self, window: &mut Window, cx: &mut App) {
        self.dismiss_flag.set(false);
        self.is_open = !self.is_open;
        if self.is_open {
            // 打开时用最新快照重建列表，清空搜索框。
            let branches = self.branches.clone();
            self.picker.update(cx, |picker, cx| {
                picker.delegate_mut().reload(branches);
                picker.search_input().set_text("", cx);
                cx.notify();
            });
            let input = self.picker.read(cx).search_input().clone();
            let focus = input.focus_handle(cx);
            window.focus(&focus);
        } else {
            window.focus(&self.focus);
        }
        window.refresh();
    }

    fn handle_toggle(&mut self, _: &SelectGitBranch, window: &mut Window, cx: &mut Context<Self>) {
        self.toggle(window, cx);
    }
}

impl Render for BranchPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 检查是否需要关闭（Escape / 点击外部）。
        if self.dismiss_flag.replace(false) {
            self.is_open = false;
            window.focus(&self.focus);
        }

        let color_value = if self.is_open {
            color::current(cx).icon_accent
        } else {
            color::current(cx).text
        };

        // glyph 上显示当前分支名，没有时显示占位。
        let glyph = Glyph::icon_text(
            "top-bar.branch",
            "icons/git_branch.svg",
            self.current_branch.as_deref().unwrap_or("--"),
        )
        .label("分支")
        .shortcut(&SelectGitBranch, cx)
        .color(color_value)
        .on_click(|_, window, cx| {
            window.dispatch_action(Box::new(SelectGitBranch), cx);
        });

        let mut root = div()
            .track_focus(&self.focus)
            // 复合 context 让 Picker 分组的快捷键与 Editor 同深度竞争。
            .key_context("BranchPicker")
            .on_action(cx.listener(Self::handle_toggle))
            .relative()
            .child(glyph);

        // 浮层
        if self.is_open {
            let dismiss = self.dismiss_flag.clone();
            let win_size = window.bounds().size;

            // 全屏点击拦截（优先级 0，垫底）
            root = root
                .child(
                    deferred(
                        div()
                            .absolute()
                            .top(Pixels::ZERO)
                            .left(Pixels::ZERO)
                            .w(win_size.width)
                            .h(win_size.height)
                            .occlude()
                            .on_mouse_down(MouseButton::Left, move |_, window, _cx| {
                                dismiss.set(true);
                                window.refresh();
                            }),
                    )
                    .with_priority(0),
                )
                // Picker 浮层（优先级 1，Local 定位到 glyph 旁边）
                .child(
                    deferred(
                        anchored()
                            .anchor(Corner::TopLeft)
                            .position(point(Pixels::ZERO, Pixels::ZERO))
                            .position_mode(gpui::AnchoredPositionMode::Local)
                            .snap_to_window_with_margin(space::S6)
                            .child(
                                div()
                                    .occlude()
                                    .on_mouse_down(MouseButton::Left, move |_, _, cx| {
                                        cx.stop_propagation();
                                    })
                                    .child(
                                        div()
                                            .bg(color::current(cx).elevated_surface_background)
                                            .border_l_3()
                                            .border_color(color::current(cx).border_focused)
                                            .border_1()
                                            .border_color(color::current(cx).border_variant)
                                            .rounded(px(8.0))
                                            .overflow_hidden()
                                            .child(self.picker.clone()),
                                    ),
                            ),
                    )
                    .with_priority(1),
                );
        }

        root
    }
}

#[cfg(test)]
mod tests {
    use std::cell::Cell;

    use gpui::{Context, div, prelude::*};

    use super::*;

    #[derive(Default)]
    struct TestView;

    impl Render for TestView {
        fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
            div()
        }
    }

    /// 构造三分支的数据源，第 2 个是当前分支。
    fn test_delegate() -> BranchPickerDelegate {
        let on_select: OnBranchSelected = Rc::new(|_, _, _| {});
        BranchPickerDelegate::new(
            vec![
                Branch {
                    name: "master".into(),
                    is_head: false,
                },
                Branch {
                    name: "feature".into(),
                    is_head: true,
                },
                Branch {
                    name: "hotfix".into(),
                    is_head: false,
                },
            ],
            on_select,
        )
    }

    #[test]
    fn empty_query_selects_current_branch() {
        let delegate = test_delegate();
        assert_eq!(delegate.selected_index, 1);
        assert_eq!(delegate.match_count(), 3);
        assert!(!delegate.create_row_visible());
    }

    #[test]
    fn query_filters_by_substring() {
        let mut delegate = test_delegate();
        delegate.update_matches("feat".into());
        assert_eq!(delegate.filtered, vec![1]);
        assert_eq!(delegate.match_count(), 1);
    }

    #[test]
    fn no_match_shows_create_row() {
        let mut delegate = test_delegate();
        delegate.update_matches("zzz".into());
        assert!(delegate.create_row_visible());
        // 只显示"创建分支"虚拟行，且默认选中它。
        assert_eq!(delegate.match_count(), 1);
        assert_eq!(delegate.selected_index, 0);
    }

    #[gpui::test]
    fn confirm_create_invokes_callback(cx: &mut gpui::TestAppContext) {
        let triggered = Rc::new(Cell::new(None::<GitBranchAction>));
        let on_select: OnBranchSelected = {
            let triggered = triggered.clone();
            Rc::new(move |action, _window, _cx| triggered.set(Some(action)))
        };
        let mut delegate = BranchPickerDelegate::new(Vec::new(), on_select);
        delegate.update_matches("new-feat".into());

        let window = cx.add_window(|_window, _cx| TestView);
        let _ = window.update(cx, |_, window, cx| {
            delegate.confirm(window, cx);
        });
        assert!(matches!(
            triggered.take(),
            Some(GitBranchAction::Create(name)) if name == "new-feat"
        ));
    }

    #[gpui::test]
    fn confirm_checkout_invokes_callback(cx: &mut gpui::TestAppContext) {
        let triggered = Rc::new(Cell::new(None::<GitBranchAction>));
        let on_select: OnBranchSelected = {
            let triggered = triggered.clone();
            Rc::new(move |action, _window, _cx| triggered.set(Some(action)))
        };
        let mut delegate = BranchPickerDelegate::new(
            vec![Branch {
                name: "feature".into(),
                is_head: false,
            }],
            on_select,
        );

        let window = cx.add_window(|_window, _cx| TestView);
        let _ = window.update(cx, |_, window, cx| {
            delegate.confirm(window, cx);
        });
        assert!(matches!(
            triggered.take(),
            Some(GitBranchAction::Checkout(name)) if name == "feature"
        ));
    }

    #[test]
    fn reload_replaces_branches_and_keeps_query() {
        let mut delegate = test_delegate();
        delegate.update_matches("feat".into());
        delegate.reload(vec![
            Branch {
                name: "feat-x".into(),
                is_head: true,
            },
            Branch {
                name: "other".into(),
                is_head: false,
            },
        ]);
        // query 保留：新列表按原 query 重过滤，命中项成为选中行。
        assert_eq!(delegate.filtered, vec![0]);
        assert_eq!(delegate.selected_index, 0);
    }

    #[test]
    fn reload_without_match_shows_create_row() {
        let mut delegate = test_delegate();
        delegate.update_matches("feat".into());
        delegate.reload(vec![Branch {
            name: "master".into(),
            is_head: true,
        }]);
        // query 保留且新列表无匹配：落到"创建分支"虚拟行。
        assert!(delegate.filtered.is_empty());
        assert!(delegate.create_row_visible());
        assert_eq!(delegate.selected_index, 0);
    }
}
