//! BranchPicker —— git 分支选择器。
//!
//! 自含按钮 + 浮层，浮层内嵌 `Picker<BranchPickerDelegate>`。
//! 分支列表直接读取 GitStore 快照（打开即渲染，无加载态）；
//! 切换/创建分支通过回调转发到 git_store 后台执行，完成后 GitStore 自动重扫。
//!
//! 搜索无匹配时列表尾部追加"创建分支"虚拟行：以当前 HEAD 为基创建并切换。

use std::rc::Rc;

use gpui::{App, Context, Entity, Render, Subscription, Window, div, prelude::*};
use zcv_actions::{DeleteGitBranch, SelectGitBranch};
use zcv_git::Branch;
use zcv_picker::{PICKER_WIDTH, Picker, PickerDelegate, PickerHost};
use zcv_project::{GitStore, GitStoreEvent};
use zcv_theme::color;
use zcv_ui::ListItem;
use zcv_ui::{Button, SvgIcon};

// ═══ 回调 ════════════════════════════════════════════════════════

/// 分支操作请求：切换分支 / 以当前 HEAD 为基创建分支。
pub enum GitBranchAction {
    Checkout(String),
    Create(String),
    Delete(String),
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
        // 当前分支显示对勾，其余分支显示分支图标。
        let row = if branch.is_head {
            row.start_slot(
                SvgIcon::new("icons/check.svg")
                    .id(("head", index))
                    .label("当前分支")
                    .color(color::current(cx).icon_accent),
            )
        } else {
            row.start_slot(
                SvgIcon::new("icons/git_branch.svg")
                    .id(("branch", index))
                    .label("分支"),
            )
        };
        let branch_name = branch.name.clone();
        let on_delete = self.on_select.clone();
        row.end_slot(
            Button::icon(("delete-branch", index), "icons/trash.svg")
                .color(color::current(cx).icon_muted)
                .label("删除分支")
                .shortcut(zcv_keymap::display_shortcut(&DeleteGitBranch, cx))
                .on_click(move |_, window, cx| {
                    on_delete(GitBranchAction::Delete(branch_name.clone()), window, cx);
                }),
        )
        .into_any_element()
    }

    fn placeholder_text(&self) -> &str {
        "搜索分支..."
    }
}

// ═══ Entity ═════════════════════════════════════════════════════

/// 分支选择器 —— 自含按钮 + 浮层。
///
/// 按钮显示当前分支名；项目不是 git 仓库或空仓库时选择器整体不显示。
/// detached HEAD 显示 8 位短 SHA。
pub struct BranchPicker {
    host: PickerHost,
    picker: Entity<Picker<BranchPickerDelegate>>,
    git_store: Entity<GitStore>,
    _git_subscription: Subscription,
}

/// 按钮显示名：分支名 → 8 位短 SHA（detached HEAD）。
fn branch_display_name(current_branch: Option<&str>, head_commit: Option<&str>) -> Option<String> {
    current_branch
        .map(str::to_owned)
        .or_else(|| head_commit.map(|oid| oid.chars().take(8).collect()))
}

/// 当前状态是否足以显示分支选择器。
fn has_branch_context(current_branch: Option<&str>, head_commit: Option<&str>) -> bool {
    current_branch.is_some() || head_commit.is_some()
}

impl BranchPicker {
    pub fn new(
        git_store: Entity<GitStore>,
        on_select: OnBranchSelected,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let delegate = BranchPickerDelegate::new(Vec::new(), on_select);

        let picker = cx.new(|cx| Picker::new(delegate, PICKER_WIDTH, window, cx));
        let host = PickerHost::new(cx.focus_handle());
        picker.update(cx, |picker, _| {
            picker.set_on_dismiss(host.on_dismiss_handler())
        });

        let git_subscription =
            cx.subscribe(&git_store, |picker, store, _event: &GitStoreEvent, cx| {
                if picker.host.is_open(cx) {
                    let branches = store
                        .read(cx)
                        .active_branch_list()
                        .map(<[Branch]>::to_vec)
                        .unwrap_or_default();
                    picker.picker.update(cx, |picker, _| {
                        picker.delegate_mut().reload(branches);
                    });
                }
                cx.notify();
            });

        Self {
            host,
            picker,
            git_store,
            _git_subscription: git_subscription,
        }
    }

    /// 当前状态是否足以显示分支选择器。
    pub(crate) fn has_branch_context(&self, cx: &App) -> bool {
        let store = self.git_store.read(cx);
        has_branch_context(store.current_branch(), store.current_head_commit())
    }

    /// 按钮显示名：分支名 → 8 位短 SHA（detached HEAD）。
    fn display_name(&self, cx: &App) -> Option<String> {
        let store = self.git_store.read(cx);
        branch_display_name(store.current_branch(), store.current_head_commit())
    }

    /// 外部切换（快捷键/点击等）。
    pub fn toggle(&mut self, window: &mut Window, cx: &mut App) {
        if !self.host.is_open(cx) {
            // 打开时用 GitStore 最新快照重建列表，清空搜索框。
            let branches = self
                .git_store
                .read(cx)
                .active_branch_list()
                .map(<[Branch]>::to_vec)
                .unwrap_or_default();
            self.picker.update(cx, |picker, cx| {
                picker.delegate_mut().reload(branches);
                picker.search_input().set_text("", cx);
                cx.notify();
            });
        }
        self.host.toggle(&self.picker, window, cx);
    }

    fn handle_delete_branch(
        &mut self,
        _: &DeleteGitBranch,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.picker.update(cx, |picker, cx| {
            let delegate = picker.delegate();
            if delegate.create_row_visible() || delegate.filtered.is_empty() {
                return;
            }
            let branch = &delegate.branches[delegate.filtered[delegate.selected_index]];
            if branch.is_head {
                return;
            }
            let cb = delegate.on_select.clone();
            cb(GitBranchAction::Delete(branch.name.clone()), window, cx);
        });
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

        let Some(display_name) = self.display_name(cx) else {
            return div();
        };

        // 空仓库没有当前分支或 HEAD，直接不渲染选择器。
        let button = Button::icon_text("top-bar.branch", "icons/git_branch.svg", display_name)
            .label("分支")
            .shortcut(zcv_keymap::display_shortcut(&SelectGitBranch, cx))
            .color(color_value)
            .on_click(cx.listener(|picker, _, window, cx| picker.toggle(window, cx)));

        let mut root = div()
            .track_focus(&self.host.focus_handle())
            // 复合 context 让 Picker 分组的快捷键与 Editor 同深度竞争。
            .key_context("GitBranchSelector")
            .on_action(cx.listener(Self::handle_delete_branch))
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
#[path = "test/branch_picker_tests.rs"]
mod tests;
