//! `ContextMenu` —— 通用菜单面板。
//!
//! 承载菜单项呈现、键盘导航与确认派发，不决定浮层挂载方式：
//! 挂载呈现由调用方经浮层原语完成，开合会话由 PopoverHost 承担。
//! 菜单外观（背景、边框、勾选列、尾部说明列）由本组件统一提供。

use std::rc::Rc;

use gpui::{
    Action, App, Context, DismissEvent, Entity, EventEmitter, FocusHandle, Focusable, Pixels,
    SharedString, Subscription, Window, div, prelude::*, px,
};
use zcv_actions::menu::{Cancel, Confirm, SelectFirst, SelectLast, SelectNext, SelectPrevious};
use zcv_theme::{color, scale};

use crate::{ListItem, SvgIcon};

/// 菜单项确认回调；参数为菜单打开时的焦点上下文。
type EntryHandler = Rc<dyn Fn(Option<&FocusHandle>, &mut Window, &mut App)>;

/// 菜单项：分隔线或可选中的条目。
pub enum ContextMenuItem {
    Separator,
    Entry(ContextMenuEntry),
}

/// 可选中的菜单项。
///
/// `handler` 的第一个参数是菜单打开时的焦点上下文：
/// 确认前先把焦点归还给该上下文，保证 action 在打开菜单前的焦点链上派发。
pub struct ContextMenuEntry {
    label: SharedString,
    end_slot_text: Option<SharedString>,
    checked: bool,
    disabled: bool,
    handler: EntryHandler,
}

impl ContextMenuEntry {
    /// 空操作条目；实际行为通过 `handler` 或 `action` 提供。
    pub fn new(label: impl Into<SharedString>) -> Self {
        Self {
            label: label.into(),
            end_slot_text: None,
            checked: false,
            disabled: false,
            handler: Rc::new(|_, _, _| {}),
        }
    }

    /// 执行指定 action 的条目；确认时先归还焦点上下文再派发。
    pub fn action(label: impl Into<SharedString>, action: Box<dyn Action>) -> Self {
        Self::new(label).handler(
            move |context: Option<&FocusHandle>, window: &mut Window, cx: &mut App| {
                if let Some(context) = context {
                    window.focus(context, cx);
                }
                window.dispatch_action(action.boxed_clone(), cx);
            },
        )
    }

    /// 勾选态：菜单含勾选项时统一预留勾选列。
    pub fn checked(mut self, checked: bool) -> Self {
        self.checked = checked;
        self
    }

    /// 禁用态：不可选中、不可确认，渲染置灰。
    pub fn disabled(mut self, disabled: bool) -> Self {
        self.disabled = disabled;
        self
    }

    pub fn handler(
        mut self,
        handler: impl Fn(Option<&FocusHandle>, &mut Window, &mut App) + 'static,
    ) -> Self {
        self.handler = Rc::new(handler);
        self
    }

    /// 行尾说明文本（如快捷键）；没有时不占位。
    pub fn end_slot_text(mut self, text: impl Into<SharedString>) -> Self {
        self.end_slot_text = Some(text.into());
        self
    }
}

impl From<ContextMenuEntry> for ContextMenuItem {
    fn from(entry: ContextMenuEntry) -> Self {
        Self::Entry(entry)
    }
}

/// 通用菜单面板。
pub struct ContextMenu {
    items: Vec<ContextMenuItem>,
    focus_handle: FocusHandle,
    action_context: Option<FocusHandle>,
    selected_index: Option<usize>,
    _on_blur_subscription: Subscription,
}

impl Focusable for ContextMenu {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl EventEmitter<DismissEvent> for ContextMenu {}

impl FluentBuilder for ContextMenu {}

impl ContextMenu {
    /// 菜单建议宽度：内容不足时保持体面宽度；由浮层挂载方消费（窗口收缩在浮层原语）。
    pub const SUGGESTED_WIDTH: Pixels = px(200.0);

    /// 构建菜单面板实体。
    pub fn build(
        window: &mut Window,
        cx: &mut App,
        build: impl FnOnce(Self, &mut Window, &mut Context<Self>) -> Self,
    ) -> Entity<Self> {
        cx.new(|cx| Self::new(window, cx, build))
    }

    fn new(
        window: &mut Window,
        cx: &mut Context<Self>,
        build: impl FnOnce(Self, &mut Window, &mut Context<Self>) -> Self,
    ) -> Self {
        let focus_handle = cx.focus_handle();
        // 焦点离开菜单（如点击窗口其他区域）即关闭。
        let _on_blur_subscription = cx.on_blur(&focus_handle, window, |this, window, cx| {
            this.cancel(&Cancel, window, cx);
        });
        build(
            Self {
                items: Vec::new(),
                focus_handle,
                action_context: None,
                selected_index: None,
                _on_blur_subscription,
            },
            window,
            cx,
        )
    }

    /// 记录菜单打开时的焦点上下文；
    /// 确认 action 条目前先归还焦点，使 action 在打开菜单前的焦点链上派发。
    pub fn context(mut self, focus: FocusHandle) -> Self {
        self.action_context = Some(focus);
        self
    }

    pub fn separator(mut self) -> Self {
        self.items.push(ContextMenuItem::Separator);
        self
    }

    pub fn entry(mut self, entry: ContextMenuEntry) -> Self {
        self.items.push(ContextMenuItem::Entry(entry));
        self
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.selected_index
    }

    fn is_selectable(item: &ContextMenuItem) -> bool {
        matches!(item, ContextMenuItem::Entry(entry) if !entry.disabled)
    }

    /// 确认当前选中项；无选中项时关闭菜单。
    pub fn confirm(&mut self, _: &Confirm, window: &mut Window, cx: &mut Context<Self>) {
        let Some(ix) = self.selected_index else {
            cx.emit(DismissEvent);
            return;
        };
        // 选中项只会落在可选中条目上（导航只选可选中项），无需再查禁用态。
        let Some(ContextMenuItem::Entry(entry)) = self.items.get(ix) else {
            return;
        };
        let context = self.action_context.clone();
        let handler = entry.handler.clone();
        handler(context.as_ref(), window, cx);
        cx.emit(DismissEvent);
    }

    /// 关闭菜单。
    pub fn cancel(&mut self, _: &Cancel, _: &mut Window, cx: &mut Context<Self>) {
        cx.emit(DismissEvent);
    }

    pub fn select_first(&mut self, _: &SelectFirst, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.items.iter().position(Self::is_selectable) {
            self.select_index(ix, cx);
        }
        cx.notify();
    }

    fn handle_select_last(&mut self, _: &SelectLast, _: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.items.iter().rposition(Self::is_selectable) {
            self.select_index(ix, cx);
            cx.notify();
        }
    }

    pub fn select_next(&mut self, _: &SelectNext, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(ix) = self.selected_index
            && let Some(next) = self
                .items
                .iter()
                .enumerate()
                .skip(ix + 1)
                .find(|(_, item)| Self::is_selectable(item))
                .map(|(ix, _)| ix)
        {
            self.select_index(next, cx);
            cx.notify();
            return;
        }
        self.select_first(&SelectFirst, window, cx);
    }

    pub fn select_previous(
        &mut self,
        _: &SelectPrevious,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if let Some(ix) = self.selected_index
            && let Some(previous) = self
                .items
                .iter()
                .enumerate()
                .take(ix)
                .rev()
                .find(|(_, item)| Self::is_selectable(item))
                .map(|(ix, _)| ix)
        {
            self.select_index(previous, cx);
            cx.notify();
            return;
        }
        self.handle_select_last(&SelectLast, window, cx);
    }

    fn select_index(&mut self, ix: usize, cx: &mut Context<Self>) {
        self.selected_index = Some(ix);
        cx.notify();
    }

    /// 单个菜单项的行内容：名称靠左、行尾说明贴行尾。
    fn render_entry(
        &self,
        ix: usize,
        entry: &ContextMenuEntry,
        has_toggles: bool,
        cx: &Context<Self>,
    ) -> impl IntoElement + use<> {
        let colors = *color::current(cx);
        let ContextMenuEntry {
            label,
            end_slot_text,
            checked,
            disabled,
            ..
        } = entry;
        let label_color = if *disabled {
            colors.text_disabled
        } else {
            colors.text
        };

        let row = ListItem::new(("context-menu-item", ix))
            .disabled(*disabled)
            .toggle_state(Some(ix) == self.selected_index)
            .when(has_toggles, |row| {
                row.start_slot(
                    div()
                        .debug_selector(move || format!("context-menu-check-{ix}"))
                        // 未勾选的项用不可见占位保持同列对齐。
                        .when(!*checked, |slot| slot.invisible())
                        .child(SvgIcon::new("icons/check.svg").color(if *disabled {
                            colors.text_disabled
                        } else {
                            colors.icon_accent
                        })),
                )
            })
            .child(
                div()
                    .debug_selector(move || format!("context-menu-label-{ix}"))
                    .text_color(label_color)
                    .child(label.clone()),
            )
            .when_some(end_slot_text.clone(), |row, text| {
                row.end_slot(
                    div()
                        .debug_selector(move || format!("context-menu-shortcut-{ix}"))
                        .text_color(if *disabled {
                            colors.text_disabled
                        } else {
                            colors.text_placeholder
                        })
                        .child(text),
                )
            });

        div()
            .id(("context-menu-row", ix))
            .debug_selector(move || format!("context-menu-item-{ix}"))
            .when(!*disabled, |row| {
                row.on_click(cx.listener(move |this, _, window, cx| {
                    // 点击与键盘确认共用一条路径。
                    this.selected_index = Some(ix);
                    this.confirm(&Confirm, window, cx);
                }))
            })
            .child(row)
    }
}

impl Render for ContextMenu {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let colors = *color::current(cx);
        // 勾选列只在菜单确实含勾选项时占用，普通菜单项不预留缩进。
        let has_toggles = self
            .items
            .iter()
            .any(|item| matches!(item, ContextMenuItem::Entry(entry) if entry.checked));
        let window_margin = scale::to_pixels(scale::S12, window);
        let max_height = window.bounds().size.height - window_margin;

        let mut rows = div()
            .id("context-menu-rows")
            // test cfg 下注册 debug bounds，供面板布局断言使用。
            .debug_selector(|| "context-menu-rows".into())
            .max_h(max_height)
            .py(scale::S4)
            .overflow_y_scroll();

        for (ix, item) in self.items.iter().enumerate() {
            match item {
                ContextMenuItem::Separator => {
                    rows = rows.child(
                        div()
                            .h(px(1.0))
                            .my(scale::S4)
                            .mx(scale::S8)
                            .bg(colors.border),
                    );
                }
                ContextMenuItem::Entry(entry) => {
                    rows = rows.child(self.render_entry(ix, entry, has_toggles, cx));
                }
            }
        }

        div()
            .id("context-menu")
            .occlude()
            .track_focus(&self.focus_handle)
            .key_context("menu")
            .on_action(cx.listener(Self::select_first))
            .on_action(cx.listener(Self::handle_select_last))
            .on_action(cx.listener(Self::select_next))
            .on_action(cx.listener(Self::select_previous))
            .on_action(cx.listener(Self::confirm))
            .on_action(cx.listener(Self::cancel))
            .bg(colors.elevated_surface_background)
            .border_1()
            .border_color(colors.border)
            .rounded_lg()
            .child(rows)
    }
}

#[cfg(test)]
#[path = "test/context_menu_tests.rs"]
mod tests;
