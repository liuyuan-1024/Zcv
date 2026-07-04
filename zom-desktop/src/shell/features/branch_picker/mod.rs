//! BranchPicker —— 锚定顶栏分支徽章的切换分支浮面。
//!
//! 走 surface 系统：点击分支名打开，搜索过滤，↑↓ 移动选择，Enter 切换，Esc 关闭。
//! 样式与项目选择器保持一致：三段式（搜索框 / 分支列表 / 无 footer）。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{Context, Div, FocusHandle, Window, div, prelude::*, px};
use zom_command::{EditTarget, KeyContext};

use crate::app::App;
use crate::editor::TextEditorSlot;
use crate::editor::text::{
    EditorSnapshot, EditorSnapshotRequest, ImeQueryTarget, OwnedEditorTarget,
};
use crate::focus::AppFocus;
use crate::git_service::GitService;
use crate::host_intent::CommandRequest;
use crate::shell::shared::{CommandBinding, Glyph};
use crate::shell::surfaces::{SurfaceAnchor, SurfaceRequest};
use crate::text_target::{TextTargetOwner, TextTargetQuery};
use crate::theme::{color, radius, space, typography};
use crate::ui_id::SurfaceId;

mod effects;
pub(crate) use effects::try_apply_effect;

/// 顶栏分支徽章的 element id —— 浮面锚定目标。
const INVOKER_ID: &str = "top-bar.branch-badge";

// ── Model ──

/// 分支选择器运行时模型，承载搜索文本和完整分支列表。
pub(crate) struct BranchPickerModel {
    query: OwnedEditorTarget,
    branches: Vec<(String, bool)>,
    selected_index: usize,
}

impl BranchPickerModel {
    fn new() -> Self {
        Self {
            query: OwnedEditorTarget::new(),
            branches: Vec::new(),
            selected_index: 0,
        }
    }

    fn state(&self) -> BranchPickerState {
        let query_text = self.query.text();
        let filtered: Vec<(String, bool)> = self
            .branches
            .iter()
            .filter(|(name, _)| {
                query_text.is_empty() || name.to_lowercase().contains(&query_text.to_lowercase())
            })
            .cloned()
            .collect();
        // selected_index 以 model 字段为准，仅 clamp 到过滤后范围。
        let selected = self.selected_index.min(filtered.len().saturating_sub(1));
        BranchPickerState {
            query: self.query.snapshot(EditorSnapshotRequest::single_line()),
            filtered_branches: filtered,
            selected_index: selected,
        }
    }

    fn filtered_len(&self) -> usize {
        let q = self.query.text();
        if q.is_empty() {
            self.branches.len()
        } else {
            let q = q.to_lowercase();
            self.branches
                .iter()
                .filter(|(n, _)| n.to_lowercase().contains(&q))
                .count()
        }
    }

    fn move_selection(&mut self, delta: isize) {
        let count = self.filtered_len();
        if count == 0 {
            self.selected_index = 0;
            return;
        }
        let new = self.selected_index as isize + delta;
        self.selected_index = new.rem_euclid(count as isize) as usize;
    }

    fn selected_branch_name(&self) -> Option<String> {
        let state = self.state();
        state
            .filtered_branches
            .get(self.selected_index)
            .map(|(name, _)| name.clone())
    }

    fn selected_is_current(&self) -> bool {
        let state = self.state();
        state
            .filtered_branches
            .get(self.selected_index)
            .map(|(_, cur)| *cur)
            .unwrap_or(false)
    }
}

// ── State (snapshot) ──

#[derive(Clone, Debug)]
pub(crate) struct BranchPickerState {
    pub query: EditorSnapshot,
    pub filtered_branches: Vec<(String, bool)>,
    pub selected_index: usize,
}

// ── TextTarget impls ──

impl TextTargetQuery for BranchPickerModel {
    fn accepts_focus(&self, focus: AppFocus) -> bool {
        focus == AppFocus::branch_picker()
    }

    fn snapshot(&self, _focus: AppFocus) -> EditorSnapshot {
        self.query.snapshot(EditorSnapshotRequest::single_line())
    }

    fn key_contexts(&self) -> Vec<KeyContext> {
        vec![
            KeyContext::branch_picker(),
            KeyContext::text_edit(false, false),
            KeyContext::global(),
        ]
    }

    fn ime_query_target(&self, _focus: AppFocus) -> Option<ImeQueryTarget<'_>> {
        Some(self.query.as_ime_query_target())
    }
}

impl TextTargetOwner for BranchPickerModel {
    fn edit_target(&mut self, _focus: AppFocus) -> Option<EditTarget<'_>> {
        Some(self.query.as_edit_target())
    }

    /// 查询文本变化后重置选中项，避免旧下标越界。
    fn after_text_changed(&mut self) {
        self.selected_index = 0;
    }
}

// ── Runtime ──

#[derive(Clone)]
pub(crate) struct BranchPickerRuntime {
    focus: FocusHandle,
    model: Rc<RefCell<BranchPickerModel>>,
    slot: Rc<RefCell<Option<Rc<TextEditorSlot>>>>,
    git_handle: Rc<RefCell<GitService>>,
    switch_request: Rc<RefCell<Option<CommandRequest>>>,
    delete_request: Rc<RefCell<Option<CommandRequest>>>,
}

impl BranchPickerRuntime {
    pub(crate) fn new<T>(cx: &mut Context<T>, git_handle: Rc<RefCell<GitService>>) -> Self {
        Self {
            focus: cx.focus_handle(),
            model: Rc::new(RefCell::new(BranchPickerModel::new())),
            slot: Rc::new(RefCell::new(None)),
            git_handle,
            switch_request: Rc::new(RefCell::new(None)),
            delete_request: Rc::new(RefCell::new(None)),
        }
    }

    pub(crate) fn set_switch_request(&self, req: CommandRequest) {
        *self.switch_request.borrow_mut() = Some(req);
    }

    pub(crate) fn set_delete_request(&self, req: CommandRequest) {
        *self.delete_request.borrow_mut() = Some(req);
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(crate) fn git_handle(&self) -> Rc<RefCell<GitService>> {
        self.git_handle.clone()
    }

    pub(crate) fn owner_handle(&self) -> Rc<RefCell<dyn TextTargetOwner>> {
        self.model.clone()
    }

    pub(crate) fn set_slot(&self, slot: Rc<TextEditorSlot>) {
        *self.slot.borrow_mut() = Some(slot);
    }

    pub(crate) fn set_branches(&self, branches: Vec<(String, bool)>) {
        let mut model = self.model.borrow_mut();
        // 默认选中当前分支
        model.selected_index = branches
            .iter()
            .position(|(_, is_current)| *is_current)
            .unwrap_or(0);
        model.branches = branches;
    }

    pub(crate) fn move_selection(&self, delta: isize) {
        self.model.borrow_mut().move_selection(delta);
    }

    pub(crate) fn selected_branch(&self) -> Option<String> {
        self.model.borrow().selected_branch_name()
    }

    pub(crate) fn selected_is_current(&self) -> bool {
        self.model.borrow().selected_is_current()
    }

    pub(crate) fn install_listeners<T: 'static>(
        &self,
        app: Rc<RefCell<App>>,
        window: &mut Window,
        cx: &mut Context<T>,
    ) {
        let app_on_focus = Rc::clone(&app);
        cx.on_focus(&self.focus, window, move |_, _, cx| {
            app_on_focus
                .borrow_mut()
                .request_focus_from_shell(AppFocus::branch_picker());
            cx.notify();
        })
        .detach();
    }
}

// ── Surface request ──

/// 构建 SurfaceRequest，样式与项目选择器一致。
pub(crate) fn request(runtime: BranchPickerRuntime) -> SurfaceRequest {
    let focus = runtime.focus.clone();
    let model = runtime.model.clone();
    let slot = Rc::clone(&runtime.slot);
    let switch_request = Rc::clone(&runtime.switch_request);
    let delete_request = Rc::clone(&runtime.delete_request);

    SurfaceRequest {
        id: SurfaceId::BranchPicker,
        anchor: SurfaceAnchor::Invoker {
            id: INVOKER_ID.into(),
            attachment: gpui::Corner::TopLeft,
        },
        focus_on_open: Some(focus.clone()),
        render: Rc::new(move || {
            let slot_guard = slot.borrow();
            let Some(slot) = slot_guard.as_ref() else {
                return div().into_any_element();
            };

            let state = model.borrow().state();
            let show_placeholder = state
                .query
                .lines
                .first()
                .map(|line| line.text.is_empty())
                .unwrap_or(true);

            div()
                .w(px(320.0))
                .rounded(radius::r4())
                .border_1()
                .border_color(color::current().gray.s05)
                .bg(color::current().gray.s03)
                .text_color(color::current().gray.s09)
                .text_size(typography::ui())
                .line_height(typography::ui_line())
                .overflow_hidden()
                .track_focus(&focus)
                .tab_index(0)
                .child(search_box(slot, show_placeholder))
                .child(branch_list(
                    &state,
                    &model,
                    &switch_request,
                    &delete_request,
                ))
                .into_any_element()
        }),
    }
}

// ── 搜索框 ──

fn search_box(slot: &Rc<TextEditorSlot>, show_placeholder: bool) -> Div {
    div()
        .w_full()
        .flex()
        .items_center()
        .overflow_hidden()
        .p(space::s6())
        .border_b_1()
        .border_color(color::current().gray.s05)
        .text_color(color::current().gray.s09)
        .child({
            let editor_div = div()
                .flex_1()
                .relative()
                .flex()
                .items_center()
                .overflow_hidden()
                .h(typography::ui_line())
                .text_color(color::current().gray.s09);

            let editor_div = if show_placeholder {
                editor_div.child(
                    div()
                        .absolute()
                        .top_0()
                        .left_0()
                        .text_color(color::current().gray.s08)
                        .child("搜索分支..."),
                )
            } else {
                editor_div
            };

            editor_div.child(slot.embed())
        })
}

// ── 分支列表 ──

fn branch_list(
    state: &BranchPickerState,
    model: &Rc<RefCell<BranchPickerModel>>,
    switch_request: &Rc<RefCell<Option<CommandRequest>>>,
    delete_request: &Rc<RefCell<Option<CommandRequest>>>,
) -> Div {
    div()
        .flex()
        .flex_col()
        .p(space::s6())
        .children(
            state
                .filtered_branches
                .iter()
                .enumerate()
                .map(|(i, (name, is_current))| {
                    let is_selected = i == state.selected_index;
                    let border_color = if is_selected {
                        color::current().blue.s07
                    } else {
                        gpui::rgba(0)
                    };

                    let prefix = if *is_current { "✓ " } else { "  " };

                    let model = Rc::clone(model);
                    let switch_request = Rc::clone(switch_request);
                    // 删除按钮——当前分支不显示
                    let trash_button = if *is_current {
                        None
                    } else {
                        let delete_request = Rc::clone(delete_request);
                        let model = Rc::clone(&model);
                        let delete_binding = CommandBinding {
                            id: "branch_picker.delete".to_string(),
                            title: Rc::new(|_| Some("删除分支".into())),
                            shortcut: Rc::new(|_| None),
                            request: Rc::new(move |window, cx| {
                                model.borrow_mut().selected_index = i;
                                if let Some(req) = delete_request.borrow().as_ref() {
                                    req(window, cx);
                                }
                            }),
                        };
                        Some(
                            Glyph::icon(("branch-picker.delete", i), "icons/actions/trash.svg")
                                .command(delete_binding)
                                .render(),
                        )
                    };

                    div()
                        .flex()
                        .flex_row()
                        .items_center()
                        .rounded(radius::r2())
                        .border_1()
                        .border_color(border_color)
                        .overflow_hidden()
                        .text_color(color::current().gray.s09)
                        .cursor_pointer()
                        .hover(|style| style.bg(color::current().gray.s04))
                        .on_mouse_down(gpui::MouseButton::Left, move |_, window, cx| {
                            model.borrow_mut().selected_index = i;
                            if let Some(req) = switch_request.borrow().as_ref() {
                                req(window, cx);
                            }
                        })
                        .child(
                            div()
                                .flex_1()
                                .overflow_hidden()
                                .truncate()
                                .child(format!("{prefix}{name}")),
                        )
                        .when_some(trash_button, |row, btn| row.child(btn))
                        .into_any_element()
                }),
        )
}
