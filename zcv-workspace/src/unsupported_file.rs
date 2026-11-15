use std::path::{Path, PathBuf};

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, Font, Render, SharedString, Window, div,
    prelude::*,
};
use zcv_ui::{Button, ButtonSize, ButtonStyle};

use crate::{Item, ItemEvent};

/// 由工作区承载无法在内部展示内容的本地文件。
pub(crate) struct UnsupportedFile {
    path: PathBuf,
    focus: FocusHandle,
}

impl UnsupportedFile {
    pub(crate) fn new(path: PathBuf, cx: &mut Context<Self>) -> Self {
        Self {
            path,
            focus: cx.focus_handle(),
        }
    }
}

impl Focusable for UnsupportedFile {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<ItemEvent> for UnsupportedFile {}

impl Item for UnsupportedFile {
    type Event = ItemEvent;

    fn tab_content_text(&self, _: &App) -> SharedString {
        self.path
            .file_name()
            .unwrap_or(self.path.as_os_str())
            .to_string_lossy()
            .into_owned()
            .into()
    }

    fn item_path(&self, _: &App) -> Option<PathBuf> {
        Some(self.path.clone())
    }

    fn breadcrumbs(
        &self,
        relative_path: Option<&Path>,
        _: &App,
    ) -> Option<(Vec<SharedString>, Option<Font>)> {
        let path = relative_path.unwrap_or(&self.path);
        Some((vec![path.to_string_lossy().into_owned().into()], None))
    }

    fn rename_path(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        if let Ok(relative) = self.path.strip_prefix(from) {
            self.path = to.join(relative);
            cx.emit(ItemEvent::PathChanged);
            cx.notify();
        }
    }

    fn to_item_events(event: &ItemEvent, emit: &mut dyn FnMut(ItemEvent)) {
        emit(event.clone());
    }
}

impl Render for UnsupportedFile {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        div()
            .size_full()
            .track_focus(&self.focus)
            .flex()
            .items_center()
            .justify_center()
            .child(
                Button::icon_text(
                    "open-with-system",
                    "icons/arrow_up_right.svg",
                    "使用本机默认应用打开",
                )
                .size(ButtonSize::Loose)
                .style(ButtonStyle::Solid)
                .on_click(cx.listener(|this, _, _, cx| cx.open_with_system(&this.path))),
            )
    }
}
