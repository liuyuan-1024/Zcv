//! Editor —— 可嵌入文本编辑组件。
//! 此文件是 `zcv-editor` crate 的公共入口。

use std::sync::Arc;

use gpui::{AnyElement, App, AppContext, Entity, IntoElement, Subscription, Window};
use zcv_ui::{EDITOR_FACTORY, ErasedEditor, ErasedEditorEvent};

mod blink_manager;
mod display_map;
mod element;
mod gutter;
mod item_provider;
mod scroll;
mod scrollbar;
mod selection;
mod view;
mod workspace_item;

pub use display_map::{
    Chunk, RenderChunks, StyledLine, StyledSpan, chunks_to_runs, render_plain_line,
};
pub use selection::{Selection, SelectionSet};
pub use view::{Editor, EditorEvent, SoftWrap};

pub fn init(cx: &mut App) {
    EDITOR_FACTORY.get_or_init(|| |cx| Arc::new(ErasedEditorHandle(cx.new(Editor::single_line))));
    item_provider::init(cx);
}

#[derive(Clone)]
struct ErasedEditorHandle(Entity<Editor>);

impl ErasedEditor for ErasedEditorHandle {
    fn text(&self, cx: &App) -> String {
        self.0.read(cx).text(cx)
    }

    fn set_text(&self, text: &str, cx: &mut App) {
        self.0.update(cx, |editor, cx| editor.set_text(text, cx));
    }

    fn set_placeholder_text(&self, text: &str, cx: &mut App) {
        self.0
            .update(cx, |editor, cx| editor.set_placeholder_text(text, cx));
    }

    fn focus_handle(&self, cx: &App) -> gpui::FocusHandle {
        self.0.read(cx).focus_handle()
    }

    fn subscribe(
        &self,
        mut callback: Box<dyn FnMut(ErasedEditorEvent, &mut Window, &mut App) + 'static>,
        window: &mut Window,
        cx: &mut App,
    ) -> Subscription {
        window.subscribe(&self.0, cx, move |_, event: &EditorEvent, window, cx| {
            if *event == EditorEvent::Edited {
                callback(ErasedEditorEvent::Edited, window, cx);
            }
        })
    }

    fn render(&self) -> AnyElement {
        self.0.clone().into_any_element()
    }
}
