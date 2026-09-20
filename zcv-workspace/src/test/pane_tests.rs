use std::cell::RefCell;
use std::rc::Rc;

use gpui::{
    AppContext, Context, EventEmitter, Pixels, Point, Render, TestAppContext, Window, div,
    prelude::*,
};
use zcv_language::LanguageBuffer;
use zcv_multi_buffer::MultiBuffer;
use zcv_text::{Buffer, BufferConfig};

use super::*;
use crate::toolbar::{ToolbarItemEvent, ToolbarItemLocation, ToolbarItemView};
use crate::{Item, PreviewItem, PreviewItemHandle, PreviewProvider, register};

/// 辅助视图类型，仅用于测试中创建窗口。
struct TestView;
impl Render for TestView {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

/// 记录最近一次活动 Item 的工具项，用于断言 Pane 会把活动项同步给工具区。
struct RecordingToolbar {
    active_item: Option<gpui::EntityId>,
}

impl EventEmitter<ToolbarItemEvent> for RecordingToolbar {}

impl Render for RecordingToolbar {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl ToolbarItemView for RecordingToolbar {
    fn set_active_pane_item(
        &mut self,
        item: Option<&dyn ItemHandle>,
        _window: &mut Window,
        _cx: &mut Context<Self>,
    ) -> ToolbarItemLocation {
        self.active_item = item.map(ItemHandle::item_id);
        if item.is_some() {
            ToolbarItemLocation::PrimaryLeft
        } else {
            ToolbarItemLocation::Hidden
        }
    }
}

/// Pane 在活动项变化后把新活动项同步给工具区。
#[gpui::test]
fn pane_syncs_toolbar_with_active_item(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "工具区同步");
    let pane = cx.new(Pane::new);
    cx.add_window_view(|window, cx| {
        let recorder = cx.new(|_| RecordingToolbar { active_item: None });
        pane.update(cx, |pane, cx| {
            pane.toolbar().update(cx, |toolbar, cx| {
                toolbar.add_item(recorder.clone(), window, cx)
            });
        });
        let item =
            cx.new(|cx| TestSourceItem::new(buffer.clone(), PathBuf::from("toolbar.txt"), cx));
        pane.update(cx, |pane, cx| {
            pane.open_item(Box::new(item), false, window, cx)
        });

        let active = pane.read(cx).active_item().map(ItemHandle::item_id);
        assert!(active.is_some(), "打开 Item 后应有活动项");
        assert_eq!(
            recorder.read(cx).active_item,
            active,
            "Pane 应把活动项同步给工具区"
        );
        TestView
    });
}

#[test]
fn dirty_indicator_takes_priority_over_preview_indicator() {
    assert_eq!(tab_end_state(false, false), TabEndState::Close);
    assert_eq!(tab_end_state(false, true), TabEndState::Preview);
    assert_eq!(tab_end_state(true, false), TabEndState::Dirty);
    assert_eq!(tab_end_state(true, true), TabEndState::Dirty);
}

/// 辅助：用 add_window_view 提供 window 上下文，以源码 Item 打开文件。
fn open_item_in_test(
    cx: &mut TestAppContext,
    pane: &Entity<Pane>,
    path: PathBuf,
    multi_buffer: Entity<MultiBuffer>,
    allow_transient: bool,
) {
    cx.add_window_view(|window, cx| {
        pane.update(cx, |p, cx| {
            let item = cx.new(|cx| TestSourceItem::new(multi_buffer.clone(), path.clone(), cx));
            p.open_item(Box::new(item), allow_transient, window, cx);
        });
        TestView
    });
}

fn open_file_in_test(
    cx: &mut TestAppContext,
    pane: &Entity<Pane>,
    path: PathBuf,
    multi_buffer: Entity<MultiBuffer>,
) {
    open_item_in_test(cx, pane, path, multi_buffer, false);
}

fn open_transient_file_in_test(
    cx: &mut TestAppContext,
    pane: &Entity<Pane>,
    path: PathBuf,
    multi_buffer: Entity<MultiBuffer>,
) {
    open_item_in_test(cx, pane, path, multi_buffer, true);
}

fn toggle_preview_in_test(cx: &mut TestAppContext, pane: &Entity<Pane>) {
    cx.add_window_view(|window, cx| {
        pane.update(cx, |pane, cx| {
            pane.toggle_preview(window, cx);
        });
        TestView
    });
}

fn test_buffer(cx: &mut TestAppContext, text: impl Into<String>) -> Entity<MultiBuffer> {
    let buffer =
        Buffer::from_text(text.into(), BufferConfig::default()).expect("应创建测试 Buffer");
    let language_buffer = cx.new(|cx| {
        LanguageBuffer::new(
            buffer,
            None,
            std::sync::Arc::new(zcv_language::LanguageRegistry::new()),
            cx,
        )
    });
    cx.new(|cx| MultiBuffer::singleton(language_buffer, cx))
}

/// 测试专用的源码 Item：编辑时标记脏并发射 Edit 事件（Pane 依赖它提升临时标签）。
struct TestSourceItem {
    multi_buffer: Entity<MultiBuffer>,
    path: PathBuf,
    dirty: bool,
    focus: gpui::FocusHandle,
}

#[derive(Clone, Copy)]
enum TestEvent {
    Edited,
}

impl TestSourceItem {
    fn new(multi_buffer: Entity<MultiBuffer>, path: PathBuf, cx: &mut Context<Self>) -> Self {
        Self {
            multi_buffer,
            path,
            dirty: false,
            focus: cx.focus_handle(),
        }
    }

    /// 模拟用户编辑：标记脏并发射编辑事件。
    fn set_text(&mut self, _text: &str, cx: &mut Context<Self>) {
        self.dirty = true;
        cx.emit(TestEvent::Edited);
        cx.notify();
    }
}

impl EventEmitter<TestEvent> for TestSourceItem {}

impl gpui::Focusable for TestSourceItem {
    fn focus_handle(&self, _cx: &App) -> gpui::FocusHandle {
        self.focus.clone()
    }
}

impl Render for TestSourceItem {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl Item for TestSourceItem {
    type Event = TestEvent;

    fn tab_content_text(&self, _cx: &App) -> gpui::SharedString {
        self.path
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default()
            .into()
    }

    fn to_item_events(event: &Self::Event, emit: &mut dyn FnMut(ItemEvent)) {
        match event {
            TestEvent::Edited => emit(ItemEvent::Edit),
        }
    }

    fn is_dirty(&self, _cx: &App) -> bool {
        self.dirty
    }

    fn item_path(&self, _cx: &App) -> Option<PathBuf> {
        Some(self.path.clone())
    }

    fn multi_buffer(&self, _cx: &App) -> Option<Entity<MultiBuffer>> {
        Some(self.multi_buffer.clone())
    }
}

struct TestCompositeItem {
    focus: FocusHandle,
}

impl EventEmitter<TestEvent> for TestCompositeItem {}

impl gpui::Focusable for TestCompositeItem {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus.clone()
    }
}

impl Render for TestCompositeItem {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div()
    }
}

impl Item for TestCompositeItem {
    type Event = TestEvent;

    fn tab_content_text(&self, _cx: &App) -> gpui::SharedString {
        "组合文档".into()
    }

    fn serialized_pane_item(&self, _cx: &App) -> Option<SerializedPaneItem> {
        Some(SerializedPaneItem::Custom {
            kind: "test-composite".into(),
            state: serde_json::json!({ "group": "staged" }),
        })
    }
}

/// 测试专用的假预览 Item：转发源码 Item 元数据，展示键为 Preview("fake")。
struct FakePreviewItem {
    source_item: Box<dyn ItemHandle>,
    focus: gpui::FocusHandle,
}

impl EventEmitter<()> for FakePreviewItem {}

impl gpui::Focusable for FakePreviewItem {
    fn focus_handle(&self, _cx: &App) -> gpui::FocusHandle {
        self.focus.clone()
    }
}

impl Render for FakePreviewItem {
    fn render(&mut self, _: &mut Window, _: &mut Context<Self>) -> impl IntoElement {
        div().child("fake preview")
    }
}

impl Item for FakePreviewItem {
    type Event = ();

    fn tab_content_text(&self, cx: &App) -> gpui::SharedString {
        self.source_item.tab_content_text(cx)
    }

    fn is_dirty(&self, cx: &App) -> bool {
        self.source_item.is_dirty(cx)
    }

    fn item_path(&self, cx: &App) -> Option<PathBuf> {
        self.source_item.item_path(cx)
    }

    fn breadcrumbs(
        &self,
        project_root: Option<&Path>,
        cx: &App,
    ) -> Option<(Vec<gpui::SharedString>, Option<gpui::Font>)> {
        self.source_item.breadcrumbs(project_root, cx)
    }

    fn rename_path(&mut self, from: &Path, to: &Path, cx: &mut Context<Self>) {
        self.source_item.rename_path(from, to, cx);
    }

    fn multi_buffer(&self, cx: &App) -> Option<Entity<MultiBuffer>> {
        self.source_item.multi_buffer(cx)
    }

    fn as_preview_item(
        &self,
        self_handle: &Entity<Self>,
        _cx: &App,
    ) -> Option<Box<dyn PreviewItemHandle>> {
        Some(Box::new(self_handle.clone()))
    }
}

impl PreviewItem for FakePreviewItem {
    fn source_item(&self, _cx: &App) -> Option<Box<dyn ItemHandle>> {
        Some(self.source_item.boxed_clone())
    }
}

/// 测试专用的假预览 Provider：匹配 svg 扩展名，创建 [`FakePreviewItem`]。
struct FakePreviewProvider;

impl PreviewProvider for FakePreviewProvider {
    fn supports(&self, path: &Path, _cx: &App) -> bool {
        path.extension()
            .and_then(|extension| extension.to_str())
            .is_some_and(|extension| extension == "svg")
    }

    fn mode(&self) -> crate::PreviewMode {
        crate::PreviewMode::Source
    }

    fn presentation(&self) -> crate::PreviewPresentation {
        crate::PreviewPresentation::Canvas
    }

    fn create(&self, document: PreviewDocument, cx: &mut App) -> Box<dyn ItemHandle> {
        let PreviewDocument::Source { source_item, .. } = document else {
            panic!("测试预览 Provider 应接收源码预览文档")
        };
        let view = cx.new(|cx| FakePreviewItem {
            source_item,
            focus: cx.focus_handle(),
        });
        Box::new(view)
    }
}

fn init_previews(cx: &mut TestAppContext) {
    cx.update(|cx| register(FakePreviewProvider, cx));
}

/// 断言辅助：当前活动标签是否预览视图。
fn assert_active_is_preview(pane: &Pane, cx: &App, expected: bool) {
    assert_eq!(
        pane.active_item().map(|item| is_preview_item(item, cx)),
        Some(expected),
        "活动标签的预览状态应一致"
    );
}

#[gpui::test]
fn pane_owns_file_path_and_item_backed_by_the_given_buffer(cx: &mut TestAppContext) {
    let buffer = test_buffer(cx, "真实编辑器");
    let pane = cx.new(Pane::new);
    open_file_in_test(cx, &pane, PathBuf::from("demo.txt"), buffer.clone());

    let item = cx.read_entity(&pane, |pane, cx| {
        pane.active_item()
            .unwrap()
            .act_as::<TestSourceItem>(cx)
            .unwrap()
    });
    cx.read_entity(&item, |item, cx| assert!(!item.is_dirty(cx)));
    cx.update_entity(&item, |item, cx| item.set_text("阶段七", cx));
    cx.read_entity(&item, |item, cx| assert!(item.is_dirty(cx)));

    cx.read_entity(&pane, |pane, cx| {
        assert_eq!(pane.tabs.len(), 1);
        assert_eq!(
            pane.tabs[0]
                .item_path(cx)
                .as_deref()
                .map(|p| p.to_string_lossy().to_string()),
            Some("demo.txt".to_string())
        );
        assert_eq!(pane.active, Some(pane.tabs[0].item_id()));
        assert!(
            pane.active_item()
                .unwrap()
                .act_as::<TestSourceItem>(cx)
                .is_some()
        );
    });
}

#[gpui::test]
fn opening_the_same_path_reuses_the_pane_editor(cx: &mut TestAppContext) {
    let first_buffer = test_buffer(cx, "首次");
    let second_buffer = test_buffer(cx, "重复");

    let pane = cx.new(Pane::new);
    open_file_in_test(cx, &pane, PathBuf::from("demo.txt"), first_buffer);
    open_file_in_test(cx, &pane, PathBuf::from("demo.txt"), second_buffer);

    // 同一路径不应创建重复标签
    cx.read_entity(&pane, |pane, _| assert_eq!(pane.tabs.len(), 1));
}

#[gpui::test]
fn transient_tab_is_replaced_and_permanent_open_promotes_it(cx: &mut TestAppContext) {
    let pane = cx.new(Pane::new);
    let permanent_buffer = test_buffer(cx, "固定");
    open_file_in_test(cx, &pane, PathBuf::from("permanent.txt"), permanent_buffer);
    let first_buffer = test_buffer(cx, "第一个临时标签");
    open_transient_file_in_test(cx, &pane, PathBuf::from("first.txt"), first_buffer);
    let second_buffer = test_buffer(cx, "第二个临时标签");
    open_transient_file_in_test(cx, &pane, PathBuf::from("second.txt"), second_buffer);

    cx.read_entity(&pane, |pane, cx| {
        assert_eq!(pane.tabs.len(), 2, "新临时标签应替换旧临时标签");
        assert_eq!(
            pane.tabs[1].item_path(cx).as_deref(),
            Some(Path::new("second.txt"))
        );
        assert_eq!(pane.transient_source_item_id, Some(pane.tabs[1].item_id()));
    });

    // 模拟双击的第二次打开：同一路径固定打开，临时标签应被提升而非重复创建。
    let duplicate_buffer = test_buffer(cx, "不会替换已有 buffer");
    open_file_in_test(cx, &pane, PathBuf::from("second.txt"), duplicate_buffer);
    cx.read_entity(&pane, |pane, _| {
        assert_eq!(pane.tabs.len(), 2);
        assert_eq!(pane.transient_source_item_id, None);
    });

    let third_buffer = test_buffer(cx, "第三个临时标签");
    open_transient_file_in_test(cx, &pane, PathBuf::from("third.txt"), third_buffer);
    cx.read_entity(&pane, |pane, _| assert_eq!(pane.tabs.len(), 3));
}

#[gpui::test]
fn editing_transient_tab_promotes_it_before_next_transient_tab(cx: &mut TestAppContext) {
    let pane = cx.new(Pane::new);
    let edited_buffer = test_buffer(cx, "临时标签");
    open_transient_file_in_test(cx, &pane, PathBuf::from("edited.txt"), edited_buffer);
    let item = cx.read_entity(&pane, |pane, cx| {
        pane.active_item()
            .unwrap()
            .act_as::<TestSourceItem>(cx)
            .unwrap()
    });
    cx.update_entity(&item, |item, cx| item.set_text("已修改", cx));
    cx.run_until_parked();
    cx.read_entity(&pane, |pane, _| {
        assert_eq!(pane.transient_source_item_id, None)
    });

    let next_buffer = test_buffer(cx, "下一项");
    open_transient_file_in_test(cx, &pane, PathBuf::from("next.txt"), next_buffer);
    cx.read_entity(&pane, |pane, cx| {
        assert_eq!(pane.tabs.len(), 2, "已编辑的原临时标签不应被替换");
        assert!(
            pane.tabs
                .iter()
                .any(|item| { item.item_path(cx).as_deref() == Some(Path::new("edited.txt")) })
        );
    });
}

#[gpui::test]
fn single_click_opens_preview_but_double_click_replaces_it_with_source(cx: &mut TestAppContext) {
    init_previews(cx);
    let pane = cx.new(Pane::new);
    let svg_buffer = test_buffer(
        cx,
        r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="16"><circle cx="8" cy="8" r="8"/></svg>"#,
    );
    open_transient_file_in_test(cx, &pane, PathBuf::from("icon.svg"), svg_buffer);

    cx.read_entity(&pane, |pane, cx| {
        let preview_id = pane.active.unwrap();
        assert_eq!(pane.tabs.len(), 1);
        assert_eq!(pane.transient_source_item_id, None);
        assert_eq!(pane.transient_preview_item_id, Some(preview_id));
        assert_active_is_preview(pane, cx, true);
        assert_eq!(pane.active_item().unwrap().tab_content_text(cx), "icon.svg");
    });

    // 双击文件关闭临时预览，并在其位置打开固定源码。
    let source_buffer = test_buffer(cx, "固定源码");
    open_file_in_test(cx, &pane, PathBuf::from("icon.svg"), source_buffer);
    cx.read_entity(&pane, |pane, cx| {
        assert_eq!(pane.transient_source_item_id, None);
        assert_eq!(pane.transient_preview_item_id, None);
        assert_eq!(pane.tabs.len(), 1);
        assert_active_is_preview(pane, cx, false);
    });
}

#[gpui::test]
fn opening_another_transient_preview_replaces_the_previous_preview(cx: &mut TestAppContext) {
    init_previews(cx);
    let pane = cx.new(Pane::new);
    let first_buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
    open_transient_file_in_test(cx, &pane, PathBuf::from("first.svg"), first_buffer);
    let second_buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
    open_transient_file_in_test(cx, &pane, PathBuf::from("second.svg"), second_buffer);

    cx.read_entity(&pane, |pane, cx| {
        assert_eq!(pane.tabs.len(), 1);
        assert!(
            pane.tabs
                .iter()
                .all(|item| { item.item_path(cx).as_deref() == Some(Path::new("second.svg")) })
        );
    });
}

#[gpui::test]
fn preview_toggle_switches_tabs_and_preserves_transient_lifecycle(cx: &mut TestAppContext) {
    init_previews(cx);
    let pane = cx.new(Pane::new);
    let svg_buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
    open_transient_file_in_test(cx, &pane, PathBuf::from("toggle.svg"), svg_buffer);

    let preview_id = cx.read_entity(&pane, |pane, _| {
        let preview_id = pane.active.unwrap();
        assert_eq!(pane.tabs.len(), 1);
        assert_eq!(pane.transient_source_item_id, None);
        assert_eq!(pane.transient_preview_item_id, Some(preview_id));
        preview_id
    });
    let source_id = cx.read_entity(&pane, |pane, cx| {
        pane.tabs[0]
            .as_preview_item(cx)
            .and_then(|preview| preview.source_item(cx))
            .unwrap()
            .item_id()
    });
    toggle_preview_in_test(cx, &pane);
    cx.read_entity(&pane, |pane, cx| {
        assert_eq!(pane.tabs.len(), 2);
        assert_eq!(pane.transient_source_item_id, Some(source_id));
        assert_eq!(pane.transient_preview_item_id, Some(preview_id));
        assert_eq!(pane.active, Some(source_id));
        assert!(is_preview_item(pane.tabs[0].as_ref(), cx));
        assert!(!is_preview_item(pane.tabs[1].as_ref(), cx));
        assert_active_is_preview(pane, cx, false);
    });
    toggle_preview_in_test(cx, &pane);
    cx.read_entity(&pane, |pane, cx| {
        assert_eq!(pane.tabs.len(), 2);
        assert_eq!(pane.active, Some(preview_id));
        assert_active_is_preview(pane, cx, true);
    });
}

#[gpui::test]
fn only_transient_svg_files_open_in_preview_by_default(cx: &mut TestAppContext) {
    init_previews(cx);
    let permanent_pane = cx.new(Pane::new);
    let permanent_svg = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
    open_file_in_test(
        cx,
        &permanent_pane,
        PathBuf::from("permanent.svg"),
        permanent_svg,
    );
    cx.read_entity(&permanent_pane, |pane, cx| {
        assert_active_is_preview(pane, cx, false);
    });

    let text_pane = cx.new(Pane::new);
    let text_buffer = test_buffer(cx, "普通文本");
    open_transient_file_in_test(cx, &text_pane, PathBuf::from("preview.txt"), text_buffer);
    cx.read_entity(&text_pane, |pane, cx| {
        assert_active_is_preview(pane, cx, false);
    });
}

#[gpui::test]
fn preview_is_unique_and_shares_the_source_document(cx: &mut TestAppContext) {
    init_previews(cx);
    let pane = cx.new(Pane::new);
    let svg_buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
    open_file_in_test(cx, &pane, PathBuf::from("shared.svg"), svg_buffer);
    let source_document = cx.read_entity(&pane, |pane, cx| {
        pane.active_item().unwrap().multi_buffer(cx).unwrap()
    });
    toggle_preview_in_test(cx, &pane);
    toggle_preview_in_test(cx, &pane);
    toggle_preview_in_test(cx, &pane);

    cx.read_entity(&pane, |pane, cx| {
        assert_eq!(pane.tabs.len(), 2, "源码和预览应分别保留为两个标签");
        let active_document = pane.active_item().unwrap().multi_buffer(cx).unwrap();
        assert_eq!(source_document.entity_id(), active_document.entity_id());
        assert_active_is_preview(pane, cx, true);
    });
}

#[gpui::test]
fn serialized_pane_keeps_fixed_preview_and_its_active_position(cx: &mut TestAppContext) {
    init_previews(cx);
    let pane = cx.new(Pane::new);
    let buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
    open_file_in_test(cx, &pane, PathBuf::from("persisted.svg"), buffer);
    toggle_preview_in_test(cx, &pane);

    cx.read_entity(&pane, |pane, cx| {
        let state = pane.serialized(cx);
        assert_eq!(
            state.items,
            vec![
                SerializedPaneItem::Source(PathBuf::from("persisted.svg")),
                SerializedPaneItem::Preview(PathBuf::from("persisted.svg")),
            ]
        );
        assert_eq!(state.active_item, Some(1));
    });
}

#[gpui::test]
fn serialized_pane_omits_transient_preview(cx: &mut TestAppContext) {
    init_previews(cx);
    let pane = cx.new(Pane::new);
    let buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
    open_transient_file_in_test(cx, &pane, PathBuf::from("transient.svg"), buffer);

    cx.read_entity(&pane, |pane, cx| {
        assert_eq!(pane.serialized(cx), SerializedPane::default());
    });
}

#[gpui::test]
fn serialized_pane_keeps_fixed_custom_item(cx: &mut TestAppContext) {
    let pane = cx.new(Pane::new);
    cx.add_window_view(|window, cx| {
        pane.update(cx, |pane, cx| {
            let item = cx.new(|cx| TestCompositeItem {
                focus: cx.focus_handle(),
            });
            pane.open_item(Box::new(item), false, window, cx);
        });
        TestView
    });

    cx.read_entity(&pane, |pane, cx| {
        assert_eq!(
            pane.serialized(cx),
            SerializedPane {
                items: vec![SerializedPaneItem::Custom {
                    kind: "test-composite".into(),
                    state: serde_json::json!({ "group": "staged" }),
                }],
                active_item: Some(0),
            }
        );
    });
}

#[gpui::test]
fn restored_preview_is_fixed(cx: &mut TestAppContext) {
    init_previews(cx);
    let pane = cx.new(Pane::new);
    let buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);

    cx.add_window_view(|window, cx| {
        pane.update(cx, |pane, cx| {
            let source =
                cx.new(|cx| TestSourceItem::new(buffer.clone(), PathBuf::from("restored.svg"), cx));
            pane.open_persistent_preview(Box::new(source), window, cx)
                .expect("已注册的预览应能恢复");
        });
        TestView
    });

    cx.read_entity(&pane, |pane, cx| {
        let preview = pane.active_item().expect("预览应被加入标签栏");
        assert!(is_preview_item(preview, cx));
        assert!(!pane.is_transient_item(preview.item_id()));
    });
}

#[gpui::test]
fn unsupported_file_does_not_open_a_preview_tab(cx: &mut TestAppContext) {
    init_previews(cx);
    let pane = cx.new(Pane::new);
    let text_buffer = test_buffer(cx, "普通文本");
    open_file_in_test(cx, &pane, PathBuf::from("plain.txt"), text_buffer);
    toggle_preview_in_test(cx, &pane);
    cx.read_entity(&pane, |pane, _| assert_eq!(pane.tabs.len(), 1));
}

#[gpui::test]
fn closing_preview_keeps_the_source_tab(cx: &mut TestAppContext) {
    init_previews(cx);
    let pane = cx.new(Pane::new);
    let svg_buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
    open_file_in_test(cx, &pane, PathBuf::from("close.svg"), svg_buffer);
    toggle_preview_in_test(cx, &pane);
    let preview_id = cx.read_entity(&pane, |pane, _| pane.active.unwrap());

    cx.add_window_view(|window, cx| {
        pane.update(cx, |pane, cx| pane.close_tab(preview_id, window, cx));
        TestView
    });
    cx.read_entity(&pane, |pane, cx| {
        assert_eq!(pane.tabs.len(), 1);
        assert!(!is_preview_item(pane.active_item().unwrap(), cx));
        assert!(pane.active.is_some());
    });
}

#[gpui::test]
fn closing_source_keeps_its_preview(cx: &mut TestAppContext) {
    init_previews(cx);
    let pane = cx.new(Pane::new);
    let buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
    open_file_in_test(cx, &pane, PathBuf::from("close-source.svg"), buffer);
    toggle_preview_in_test(cx, &pane);
    let source_id = cx.read_entity(&pane, |pane, cx| {
        pane.tabs
            .iter()
            .find(|item| !is_preview_item(item.as_ref(), cx))
            .unwrap()
            .item_id()
    });

    cx.add_window_view(|window, cx| {
        pane.update(cx, |pane, cx| pane.close_tab(source_id, window, cx));
        TestView
    });
    cx.read_entity(&pane, |pane, cx| {
        assert_eq!(pane.tabs.len(), 1);
        assert!(is_preview_item(pane.active_item().unwrap(), cx));
        assert!(pane.active.is_some());
    });
}

#[gpui::test]
fn showing_source_from_an_orphaned_preview_appends_a_tab(cx: &mut TestAppContext) {
    init_previews(cx);
    let pane = cx.new(Pane::new);
    let svg_buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
    open_file_in_test(cx, &pane, PathBuf::from("orphaned.svg"), svg_buffer);
    toggle_preview_in_test(cx, &pane);
    let source_id = cx.read_entity(&pane, |pane, cx| {
        pane.tabs
            .iter()
            .find(|item| !is_preview_item(item.as_ref(), cx))
            .unwrap()
            .item_id()
    });

    cx.add_window_view(|window, cx| {
        pane.update(cx, |pane, cx| pane.close_tab(source_id, window, cx));
        TestView
    });
    let text_buffer = test_buffer(cx, "另一个标签");
    open_file_in_test(cx, &pane, PathBuf::from("other.txt"), text_buffer);
    cx.add_window_view(|window, cx| {
        pane.update(cx, |pane, cx| {
            pane.activate_item_at(0, window, cx);
            pane.toggle_preview(window, cx);
        });
        TestView
    });

    cx.read_entity(&pane, |pane, cx| {
        assert_eq!(pane.tabs.len(), 3);
        assert!(is_preview_item(pane.tabs[0].as_ref(), cx));
        assert_eq!(
            pane.tabs[1].item_path(cx).as_deref(),
            Some(Path::new("other.txt"))
        );
        assert_eq!(
            pane.tabs[2].item_path(cx).as_deref(),
            Some(Path::new("orphaned.svg"))
        );
        assert_eq!(pane.active, Some(pane.tabs[2].item_id()));
    });
}

#[gpui::test]
fn double_clicking_a_transient_tab_promotes_it(cx: &mut TestAppContext) {
    let pane = cx.new(Pane::new);
    let buffer = test_buffer(cx, "临时标签");
    open_transient_file_in_test(cx, &pane, PathBuf::from("preview.txt"), buffer);
    let item_id = cx.read_entity(&pane, |pane, _| pane.active.unwrap());

    cx.update_entity(&pane, |pane, cx| {
        pane.promote_transient_tab(item_id, cx);
    });
    cx.read_entity(&pane, |pane, _| {
        assert_eq!(pane.transient_source_item_id, None);
        assert_eq!(pane.tabs.len(), 1);
        assert_eq!(pane.active, Some(item_id));
    });
}

#[gpui::test]
fn double_clicking_a_preview_tab_keeps_its_preview_content(cx: &mut TestAppContext) {
    init_previews(cx);
    let pane = cx.new(Pane::new);
    let buffer = test_buffer(cx, r#"<svg xmlns="http://www.w3.org/2000/svg"/>"#);
    open_transient_file_in_test(cx, &pane, PathBuf::from("previewed.svg"), buffer);
    let item_id = cx.read_entity(&pane, |pane, _| pane.active.unwrap());

    cx.update_entity(&pane, |pane, cx| {
        pane.promote_transient_tab(item_id, cx);
    });
    cx.read_entity(&pane, |pane, cx| {
        assert_eq!(pane.transient_source_item_id, None);
        assert_eq!(pane.active, Some(item_id));
        assert_active_is_preview(pane, cx, true);
    });
}

#[gpui::test]
fn move_tab_reorders_tabs_correctly(cx: &mut TestAppContext) {
    let pane = cx.new(Pane::new);

    // 用 scratch Buffer 模拟多个标签
    for i in 0..4 {
        let buffer = test_buffer(cx, format!("内容{i}"));
        let path = PathBuf::from(format!("file{i}.txt"));
        open_file_in_test(cx, &pane, path, buffer);
    }

    cx.read_entity(&pane, |pane, cx| {
        assert_eq!(pane.tabs.len(), 4);
        assert_eq!(pane.tabs[0].tab_content_text(cx).as_ref(), "file0.txt");
        assert_eq!(pane.tabs[1].tab_content_text(cx).as_ref(), "file1.txt");
        assert_eq!(pane.tabs[2].tab_content_text(cx).as_ref(), "file2.txt");
        assert_eq!(pane.tabs[3].tab_content_text(cx).as_ref(), "file3.txt");
    });

    // 移动：将索引 2 移到索引 0
    cx.update_entity(&pane, |pane, _| pane.move_tab(2, 0));
    cx.read_entity(&pane, |pane, cx| {
        assert_eq!(pane.tabs.len(), 4);
        assert_eq!(pane.tabs[0].tab_content_text(cx).as_ref(), "file2.txt");
        assert_eq!(pane.tabs[1].tab_content_text(cx).as_ref(), "file0.txt");
        assert_eq!(pane.tabs[2].tab_content_text(cx).as_ref(), "file1.txt");
        assert_eq!(pane.tabs[3].tab_content_text(cx).as_ref(), "file3.txt");
    });

    // 移动：将索引 0 移到索引 3（拖到末尾）
    cx.update_entity(&pane, |pane, _| pane.move_tab(0, 3));
    cx.read_entity(&pane, |pane, cx| {
        assert_eq!(pane.tabs.len(), 4);
        assert_eq!(pane.tabs[0].tab_content_text(cx).as_ref(), "file0.txt");
        assert_eq!(pane.tabs[1].tab_content_text(cx).as_ref(), "file1.txt");
        assert_eq!(pane.tabs[2].tab_content_text(cx).as_ref(), "file3.txt");
        assert_eq!(pane.tabs[3].tab_content_text(cx).as_ref(), "file2.txt");
    });

    // 移动：不动（自身）
    cx.update_entity(&pane, |pane, _| pane.move_tab(1, 1));
    cx.read_entity(&pane, |pane, cx| {
        assert_eq!(pane.tabs.len(), 4);
        assert_eq!(pane.tabs[0].tab_content_text(cx).as_ref(), "file0.txt");
    });

    // 移动：单标签拖到末尾 → 不应闪退
    let single_pane = cx.new(Pane::new);
    let buffer = test_buffer(cx, "仅一个标签");
    open_file_in_test(cx, &single_pane, PathBuf::from("solo.txt"), buffer);
    cx.read_entity(&single_pane, |pane, _cx| {
        assert_eq!(pane.tabs.len(), 1);
    });
    // 拖到自身（from == to）— 无操作
    cx.update_entity(&single_pane, |pane, _| pane.move_tab(0, 0));
    cx.read_entity(&single_pane, |pane, cx| {
        assert_eq!(pane.tabs.len(), 1);
        assert_eq!(pane.tabs[0].tab_content_text(cx).as_ref(), "solo.txt");
    });
    // 拖到末尾（to_ix 超出范围）— clamp 后不应闪退
    cx.update_entity(&single_pane, |pane, _| pane.move_tab(0, 1));
    cx.read_entity(&single_pane, |pane, cx| {
        assert_eq!(pane.tabs.len(), 1);
        assert_eq!(pane.tabs[0].tab_content_text(cx).as_ref(), "solo.txt");
    });
}

#[gpui::test]
fn every_close_path_emits_removed(cx: &mut TestAppContext) {
    // 回归：三条关闭路径（close_tab 直接关闭、删除文件触发）都必须发射 Removed，订阅方（项目树高亮）才能刷新。
    let buffer = test_buffer(cx, "内容");
    let pane = cx.new(Pane::new);
    open_file_in_test(cx, &pane, PathBuf::from("a.txt"), buffer);
    let item_id = cx.read_entity(&pane, |pane, _| pane.active.unwrap());

    let removed = Rc::new(RefCell::new(Vec::new()));
    let observed = Rc::clone(&removed);
    let _subscription = cx.update(|cx| {
        cx.subscribe(&pane, move |_, event, _| {
            if matches!(event, PaneEvent::RemovedItem { .. }) {
                observed.borrow_mut().push(event.clone());
            }
        })
    });

    // 路径 1：close_tab 直接关闭 → Removed。
    cx.add_window_view(|window, cx| {
        pane.update(cx, |pane, cx| {
            pane.close_tab(item_id, window, cx);
        });
        TestView
    });
    assert_eq!(removed.borrow().len(), 1, "close_tab 应发射 Removed");

    // 路径 2：删除文件触发 remove_path 关闭 → Removed。
    let buffer = test_buffer(cx, "内容");
    open_file_in_test(cx, &pane, PathBuf::from("b.txt"), buffer);
    cx.add_window_view(|window, cx| {
        pane.update(cx, |pane, cx| {
            pane.remove_path(Path::new("b.txt"), window, cx);
        });
        TestView
    });
    assert_eq!(
        removed.borrow().len(),
        2,
        "remove_path 关闭 tab 应发射 Removed"
    );
    cx.read_entity(&pane, |pane, _| assert!(pane.tabs.is_empty()));
}

#[gpui::test]
fn tab_bar_is_only_rendered_when_pane_has_an_active_item(cx: &mut TestAppContext) {
    let (pane, cx) = cx.add_window_view(|_, cx| Pane::new(cx));
    cx.run_until_parked();
    cx.refresh().expect("测试窗口应可刷新");
    assert!(
        cx.debug_bounds("tab-bar-area").is_none(),
        "空 Pane 不应残留 TabBar 下边框"
    );

    let buffer = test_buffer(cx, "内容");
    cx.update(|window, cx| {
        pane.update(cx, |pane, cx| {
            let item = cx.new(|cx| TestSourceItem::new(buffer, PathBuf::from("demo.txt"), cx));
            pane.open_item(Box::new(item), false, window, cx);
        });
    });
    cx.run_until_parked();
    cx.refresh().expect("测试窗口应可刷新");
    assert!(
        cx.debug_bounds("tab-bar-area").is_some(),
        "存在活动项时应恢复 TabBar"
    );
}

#[gpui::test]
fn clicking_empty_pane_area_moves_focus_to_pane(cx: &mut TestAppContext) {
    // 决定性验证：点击空白 pane（无活动文件）后，焦点转移给 pane 自身
    // （gpui 对 track_focus 元素的"聚焦点击"内建行为：mouse down Bubble 阶段自动 window.focus）。
    // 这解释了"项目树/终端点击空白 pane 后失焦"——焦点去了 pane，行为正确；
    // 终端光标消失依赖 on_blur（焦点事件在下一帧绘制时分发），测试环境窗口不激活、
    // 焦点事件路径被清空，无法在单测中验证 on_blur 时序。
    let (pane, cx) = cx.add_window_view(|_, cx| Pane::new(cx));
    cx.run_until_parked();
    cx.refresh().expect("测试窗口应可刷新");

    // 用一个独立的焦点句柄模拟"外部组件（如终端）持有焦点"。
    let external_focus = cx.update(|_, app| app.focus_handle());
    cx.update(|window, cx| window.focus(&external_focus, cx));
    cx.run_until_parked();
    cx.update(|window, _| {
        assert!(external_focus.is_focused(window), "前置：外部句柄应已聚焦");
    });

    // 点击空白 pane 内容区（窗口中心）。
    let bounds = cx.update(|window, _| window.bounds());
    let click: Point<Pixels> = Point::new(bounds.size.width / 2.0, bounds.size.height / 2.0);
    cx.simulate_mouse_down(click, gpui::MouseButton::Left, gpui::Modifiers::default());
    cx.run_until_parked();

    cx.update(|window, cx| {
        assert!(
            !external_focus.is_focused(window),
            "点击空白 pane 转移了焦点"
        );
        assert!(
            pane.read(cx).focus.is_focused(window),
            "焦点应转移到 pane 自身（track_focus 的聚焦点击）"
        );
    });
}
