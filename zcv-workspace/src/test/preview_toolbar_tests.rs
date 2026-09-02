use std::path::{Path, PathBuf};

use gpui::{
    App, Context, EventEmitter, FocusHandle, Focusable, Render, TestAppContext, Window, div,
    prelude::*,
};

use crate::{
    Item, ItemEvent, ItemHandle, PreviewDocument, PreviewProvider, PreviewToolbarButton,
    ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView, register,
};

struct TestView;

impl Render for TestView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

#[derive(Clone, Copy)]
enum TestItemEvent {
    PathChanged,
}

struct RenamableItem {
    path: PathBuf,
    focus: FocusHandle,
}

impl EventEmitter<TestItemEvent> for RenamableItem {}

impl Focusable for RenamableItem {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for RenamableItem {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl Item for RenamableItem {
    type Event = TestItemEvent;

    fn tab_content_text(&self, _cx: &App) -> gpui::SharedString {
        self.path.to_string_lossy().into_owned().into()
    }

    fn to_item_events(event: &Self::Event, emit: &mut dyn FnMut(ItemEvent)) {
        match event {
            TestItemEvent::PathChanged => emit(ItemEvent::PathChanged),
        }
    }

    fn item_path(&self, _cx: &App) -> Option<PathBuf> {
        Some(self.path.clone())
    }

    fn rename_path(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        if self.path == from {
            self.path = to.to_path_buf();
            cx.emit(TestItemEvent::PathChanged);
        }
    }
}

struct SvgPreviewProvider;

impl PreviewProvider for SvgPreviewProvider {
    fn supports(&self, path: &Path, _cx: &App) -> bool {
        path.extension().is_some_and(|extension| extension == "svg")
    }

    fn create(&self, _document: PreviewDocument, _cx: &mut App) -> Box<dyn ItemHandle> {
        panic!("工具栏测试不应创建预览视图")
    }
}

#[gpui::test]
fn renaming_the_active_item_updates_preview_button_immediately(cx: &mut TestAppContext) {
    cx.update(|cx| register(SvgPreviewProvider, cx));
    let item = cx.new(|cx| RenamableItem {
        path: PathBuf::from("diagram.txt"),
        focus: cx.focus_handle(),
    });
    let button = cx.new(|_| PreviewToolbarButton::new());

    cx.add_window_view(|window, cx| {
        let location = button.update(cx, |button, cx| {
            button.set_active_pane_item(Some(&item as &dyn ItemHandle), window, cx)
        });
        assert_eq!(location, ToolbarItemLocation::Hidden);
        TestView
    });

    let mut events = cx.events::<ToolbarItemEvent, PreviewToolbarButton>(&button);
    cx.update_entity(&item, |item, cx| {
        item.rename_path(Path::new("diagram.txt"), Path::new("diagram.svg"), cx)
    });
    cx.run_until_parked();
    assert_eq!(
        events.try_recv().unwrap(),
        ToolbarItemEvent::ChangeLocation(ToolbarItemLocation::PrimaryRight)
    );

    cx.update_entity(&item, |item, cx| {
        item.rename_path(Path::new("diagram.svg"), Path::new("diagram.txt"), cx)
    });
    cx.run_until_parked();
    assert_eq!(
        events.try_recv().unwrap(),
        ToolbarItemEvent::ChangeLocation(ToolbarItemLocation::Hidden)
    );
}
