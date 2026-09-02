//! BranchPicker —— git 分支选择器。
//!
//! 自含按钮 + 浮层，浮层内嵌 `Picker<BranchPickerDelegate>`。
//! 分支列表由 Workspace 订阅 GitStore 事件后推送（同步快照，打开即渲染，无加载态）；
//! 切换/创建分支通过回调转发到 git_store 后台执行，完成后 GitStore 自动重扫并推送新列表。
//!
//! 搜索无匹配时列表尾部追加"创建分支"虚拟行：以当前 HEAD 为基创建并切换。

use std::rc::Rc;

use gpui::{App, Context, Entity, Render, Window, div, prelude::*};
use zcv_actions::SelectGitBranch;
use zcv_git::Branch;
use zcv_picker::{PICKER_WIDTH, Picker, PickerDelegate, PickerHost};
use zcv_theme::color;
use zcv_ui::ListItem;
use zcv_ui::{Button, SvgIcon};

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
        cx: &mut Context<Picker<Self>>,
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
        // 当前分支行首标 ✓。
        let row = if branch.is_head {
            row.start_slot(
                SvgIcon::new("icons/check.svg")
                    .id(("head", index))
                    .label("当前分支")
                    .color(color::current(cx).icon_accent),
            )
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

/// 分支选择器 —— 自含按钮 + 浮层。
///
/// 按钮显示当前分支名；项目不是 git 仓库时选择器整体不显示。
/// 仅在存在仓库时渲染，此时按显示策略回退：有分支显示分支名，
/// detached HEAD 显示 8 位短 SHA，空仓库（无提交）显示 `(没有分支)`。
pub struct BranchPicker {
    host: PickerHost,
    picker: Entity<Picker<BranchPickerDelegate>>,
    /// 当前分支名（由 Workspace 订阅 GitStore 的 Head 事件刷新）。
    current_branch: Option<String>,
    /// HEAD 提交的完整 oid（detached HEAD 时用于显示短 SHA）。
    head_commit: Option<String>,
    /// 分支列表快照（由 Workspace 订阅 GitStore 事件推送；打开时同步渲染）。
    branches: Vec<Branch>,
}

impl BranchPicker {
    pub fn new(on_select: OnBranchSelected, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let delegate = BranchPickerDelegate::new(Vec::new(), on_select);

        let picker = cx.new(|cx| Picker::new(delegate, PICKER_WIDTH, window, cx));
        let host = PickerHost::new(cx.focus_handle());
        picker.update(cx, |picker, _| {
            picker.set_on_dismiss(host.on_dismiss_handler())
        });

        Self {
            host,
            picker,
            current_branch: None,
            head_commit: None,
            branches: Vec::new(),
        }
    }

    /// 设置当前分支名。
    pub fn set_branch(&mut self, branch: Option<String>) {
        self.current_branch = branch;
    }

    /// 设置 HEAD 提交的完整 oid（按钮短 SHA 回退的数据源）。
    pub fn set_head_commit(&mut self, head_commit: Option<String>) {
        self.head_commit = head_commit;
    }

    /// 按钮显示名：分支名 → 8 位短 SHA（detached HEAD）→ 没有分支（空仓库）。
    fn display_name(&self) -> String {
        self.current_branch
            .clone()
            .or_else(|| {
                self.head_commit
                    .as_ref()
                    .map(|oid| oid.chars().take(8).collect())
            })
            .unwrap_or_else(|| "没有分支".to_string())
    }

    /// 设置分支列表快照（打开时同步渲染，无加载态）。
    pub fn set_branches(&mut self, branches: Vec<Branch>) {
        self.branches = branches;
    }

    /// 外部切换（快捷键/点击等）。
    pub fn toggle(&mut self, window: &mut Window, cx: &mut App) {
        if !self.host.is_open(cx) {
            // 打开时用最新快照重建列表，清空搜索框。
            let branches = self.branches.clone();
            self.picker.update(cx, |picker, cx| {
                picker.delegate_mut().reload(branches);
                if let Some(input) = picker.search_input() {
                    input.set_text("", cx);
                }
                cx.notify();
            });
        }
        self.host.toggle(&self.picker, window, cx);
    }
}

impl Render for BranchPicker {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // 检查是否需要关闭（Escape / 点击外部）。
        if self.host.consume_dismiss(cx) {
            self.host.close_and_refocus(window, cx);
        }

        let color_value = if self.host.is_open(cx) {
            color::current(cx).icon_accent
        } else {
            color::current(cx).text
        };

        // 按钮上显示分支名 → 短 SHA → 没有分支的三层回退。
        let button = Button::icon_text(
            "top-bar.branch",
            "icons/git_branch.svg",
            self.display_name(),
        )
        .label("分支")
        .shortcut(&SelectGitBranch, cx)
        .color(color_value)
        .on_click(cx.listener(|picker, _, window, cx| picker.toggle(window, cx)));

        let mut root = div()
            .track_focus(&self.host.focus_handle())
            // 复合 context 让 Picker 分组的快捷键与 Editor 同深度竞争。
            .key_context("GitBranchSelector")
            .relative()
            .child(button);

        // 浮层
        if self.host.is_open(cx) {
            root = root.child(self.host.overlay(window, cx, &self.picker));
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

    #[gpui::test]
    fn display_name_falls_back_through_branch_sha_and_no_branch(cx: &mut gpui::TestAppContext) {
        let on_select: OnBranchSelected = Rc::new(|_, _, _| {});
        let window = cx.add_window(|window, cx| BranchPicker::new(on_select, window, cx));

        // 有分支：显示分支名。
        let _ = window.update(cx, |picker, _, _| picker.set_branch(Some("feature".into())));
        let _ = window.update(cx, |picker, _, _| {
            assert_eq!(picker.display_name(), "feature");
        });

        // detached HEAD（无分支但有提交）：显示 8 位短 SHA。
        let _ = window.update(cx, |picker, _, _| {
            picker.set_branch(None);
            picker.set_head_commit(Some("0123456789abcdef".into()));
        });
        let _ = window.update(cx, |picker, _, _| {
            assert_eq!(picker.display_name(), "01234567");
        });

        // 空仓库（无分支无提交）：显示「没有分支」。
        let _ = window.update(cx, |picker, _, _| picker.set_head_commit(None));
        let _ = window.update(cx, |picker, _, _| {
            assert_eq!(picker.display_name(), "没有分支");
        });
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
