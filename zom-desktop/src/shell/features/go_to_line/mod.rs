//! GoToLine —— 锚定底栏光标位置 glyph 的跳转到行浮面（⌘G，Zed 风格）。
//!
//! 走 surface 系统：⌘G 打开、Esc 关闭，SurfaceManager 管生命周期。

use std::cell::RefCell;
use std::rc::Rc;

use gpui::{AnyElement, Context, Div, FocusHandle, IntoElement, Window, div, prelude::*};
use zom_command::{EditTarget, KeyContext};

use crate::app::App;
use crate::editor::TextEditorSlot;
use crate::editor::text::EditorSnapshotRequest;
use crate::editor::text::{EditorSnapshot, ImeQueryTarget, OwnedEditorTarget};
use crate::focus::AppFocus;
use crate::shell::shared::{CommandBinding, Glyph};
use crate::shell::surfaces::{SurfaceAnchor, SurfaceRequest, track_surface_anchor};
use crate::text_target::{TextTargetOwner, TextTargetQuery};
use crate::theme::{color, radius, space, typography};
use crate::ui_id::SurfaceId;

mod effects;
pub(crate) use effects::try_apply_effect;

/// 底栏光标位置 glyph 的 element id —— 浮面锚定目标。
const INVOKER_ID: &str = "bottom-bar.cursor-position";

fn key_contexts() -> Vec<KeyContext> {
    vec![
        KeyContext::go_to_line_input(),
        KeyContext::text_edit(false, false),
        KeyContext::global(),
    ]
}

fn go_to_line_field(focus: AppFocus) -> Option<()> {
    match focus {
        AppFocus::GoToLine => Some(()),
        _ => None,
    }
}

pub(crate) struct GoToLineModel {
    input: OwnedEditorTarget,
}

impl GoToLineModel {
    pub(crate) fn new() -> Self {
        Self {
            input: OwnedEditorTarget::new(),
        }
    }

    fn state(&self) -> EditorSnapshot {
        self.input.snapshot(EditorSnapshotRequest::single_line())
    }

    pub(crate) fn edit_target_for_focus(&mut self, focus: AppFocus) -> Option<EditTarget<'_>> {
        go_to_line_field(focus)?;
        Some(self.input.as_edit_target())
    }
}

impl TextTargetQuery for GoToLineModel {
    fn accepts_focus(&self, focus: AppFocus) -> bool {
        go_to_line_field(focus).is_some()
    }

    fn snapshot(&self, focus: AppFocus) -> EditorSnapshot {
        if go_to_line_field(focus).is_some() {
            self.state()
        } else {
            EditorSnapshot::default()
        }
    }

    fn key_contexts(&self) -> Vec<KeyContext> {
        key_contexts()
    }

    fn ime_query_target(&self, focus: AppFocus) -> Option<ImeQueryTarget<'_>> {
        go_to_line_field(focus)?;
        Some(self.input.as_ime_query_target())
    }
}

impl TextTargetOwner for GoToLineModel {
    fn edit_target(&mut self, focus: AppFocus) -> Option<EditTarget<'_>> {
        self.edit_target_for_focus(focus)
    }
}

#[derive(Clone)]
pub(crate) struct GoToLineRuntime {
    focus: FocusHandle,
    model: Rc<RefCell<GoToLineModel>>,
    slot: Rc<RefCell<Option<Rc<TextEditorSlot>>>>,
}

impl GoToLineRuntime {
    pub(crate) fn new<T>(cx: &mut Context<T>) -> Self {
        Self {
            focus: cx.focus_handle(),
            model: Rc::new(RefCell::new(GoToLineModel::new())),
            slot: Rc::new(RefCell::new(None)),
        }
    }

    pub(crate) fn owner_handle(&self) -> Rc<RefCell<dyn TextTargetOwner>> {
        self.model.clone()
    }

    pub(crate) fn focus_handle(&self) -> FocusHandle {
        self.focus.clone()
    }

    pub(crate) fn set_slot(&self, slot: Rc<TextEditorSlot>) {
        *self.slot.borrow_mut() = Some(slot);
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
                .request_focus_from_shell(AppFocus::go_to_line());
            cx.notify();
        })
        .detach();
    }
}

pub(crate) fn request(runtime: GoToLineRuntime) -> SurfaceRequest {
    let focus = runtime.focus.clone();
    let model = runtime.model.clone();
    let slot = Rc::clone(&runtime.slot);

    SurfaceRequest {
        id: SurfaceId::GoToLine,
        anchor: SurfaceAnchor::Invoker {
            id: INVOKER_ID.into(),
            attachment: gpui::Corner::BottomRight,
            fallback_position: gpui::point(gpui::px(0.0), gpui::px(540.0)),
        },
        focus_on_open: Some(focus.clone()),
        render: Rc::new(move || {
            let slot = slot.borrow();
            let Some(slot) = slot.as_ref() else {
                return div().into_any_element();
            };

            let show_placeholder = model
                .borrow()
                .state()
                .lines
                .first()
                .map(|line| line.text.is_empty())
                .unwrap_or(true);

            div()
                .w(gpui::px(240.0))
                .flex()
                .items_center()
                .p(space::s6())
                .border_1()
                .rounded(radius::r4())
                .border_color(color::current().gray.s05)
                .bg(color::current().gray.s01)
                .text_color(color::current().gray.s08)
                .track_focus(&focus)
                .tab_index(0)
                .cursor_pointer()
                .child(editor(slot, show_placeholder, "跳转到行..."))
                .into_any_element()
        }),
    }
}

fn editor(slot: &Rc<TextEditorSlot>, show_placeholder: bool, placeholder: &'static str) -> Div {
    let mut editor = div()
        .relative()
        .h(typography::ui_line())
        .w_full()
        .overflow_hidden()
        .text_color(color::current().gray.s09);
    if show_placeholder {
        editor = editor.child(
            div()
                .absolute()
                .top_0()
                .left_0()
                .text_color(color::current().gray.s08)
                .child(placeholder),
        );
    }
    editor.child(slot.embed())
}

/// 底栏跳转到行入口：用 track_surface_anchor 包裹，surface 锚定此 glyph。
pub(crate) fn entry(line: u64, column: u64, command: CommandBinding) -> AnyElement {
    let element_id: gpui::ElementId = INVOKER_ID.into();
    track_surface_anchor(
        element_id,
        Glyph::text(INVOKER_ID, format!("{line}:{column}"))
            .command(command)
            .render(),
    )
    .into_any_element()
}
