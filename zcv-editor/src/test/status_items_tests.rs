use super::*;
use gpui::{
    AnyEntity, App, Context, EventEmitter, FocusHandle, Focusable, SharedString, TestAppContext,
    Window,
};
use std::{any::TypeId, sync::Arc};
use zcv_language::LanguageRegistry;
use zcv_settings::{SettingsStore, UserSettings};
use zcv_workspace::{Item, PreviewItem, PreviewItemHandle};

struct SourcePreview {
    focus: FocusHandle,
    source: Entity<Editor>,
}

impl Focusable for SourcePreview {
    fn focus_handle(&self, _: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl EventEmitter<()> for SourcePreview {}

impl Render for SourcePreview {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl Item for SourcePreview {
    type Event = ();

    fn tab_content_text(&self, _: &App) -> SharedString {
        "预览".into()
    }

    fn can_navigate_to_line_column(&self, cx: &App) -> bool {
        self.source.read(cx).can_navigate_to_line_column(cx)
    }

    fn as_preview_item(
        &self,
        self_handle: &Entity<Self>,
        _: &App,
    ) -> Option<Box<dyn PreviewItemHandle>> {
        Some(Box::new(self_handle.clone()))
    }

    fn act_as_type(
        &self,
        type_id: TypeId,
        self_handle: &Entity<Self>,
        _cx: &App,
    ) -> Option<AnyEntity> {
        if type_id == TypeId::of::<Self>() {
            Some(self_handle.clone().into())
        } else if type_id == TypeId::of::<Editor>() {
            Some(self.source.clone().into())
        } else {
            None
        }
    }
}

impl PreviewItem for SourcePreview {
    fn source_item(&self, _: &App) -> Option<Box<dyn ItemHandle>> {
        Some(Box::new(self.source.clone()))
    }
}

#[gpui::test]
fn status_items_start_without_projection(cx: &mut TestAppContext) {
    let cursor = cx.new(|_| CursorPosition::new());
    let language = cx.new(|_| ActiveBufferLanguage::new());
    cx.read_entity(&cursor, |item, _| assert!(item.cursor_text.is_empty()));
    cx.read_entity(&language, |item, _| assert!(item.language.is_empty()));
}

#[gpui::test]
fn preview_exposes_source_cursor_without_language(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let editor = cx.new(|cx| Editor::single_line(Arc::new(LanguageRegistry::new()), cx));
    let preview = cx.new(|cx| SourcePreview {
        focus: cx.focus_handle(),
        source: editor.clone(),
    });
    let preview: Box<dyn ItemHandle> = Box::new(preview);
    let cursor = cx.new(|_| CursorPosition::new());
    let language = cx.new(|_| ActiveBufferLanguage::new());
    cursor.update(cx, |cursor, cx| {
        cursor.set_active_pane_item(Some(preview.as_ref()), cx)
    });
    language.update(cx, |language, cx| {
        language.set_active_pane_item(Some(preview.as_ref()), cx)
    });
    cx.read_entity(&cursor, |cursor, _| {
        assert!(!cursor.cursor_text.is_empty());
        assert!(cursor.can_navigate);
        assert!(cursor._subscription.is_some());
    });
    cx.read_entity(&language, |language, _| {
        assert!(language.language.is_empty());
        assert!(language._subscription.is_none());
    });
    let editor: Box<dyn ItemHandle> = Box::new(editor);
    cursor.update(cx, |cursor, cx| {
        cursor.set_active_pane_item(Some(editor.as_ref()), cx)
    });
    cx.read_entity(&cursor, |cursor, _| assert!(cursor.can_navigate));
}
