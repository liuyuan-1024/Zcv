use std::{path::PathBuf, sync::Arc};

use gpui::{TestAppContext, px, size};
use zcv_editor::Editor;
use zcv_language::LanguageRegistry;

use super::*;
use zcv_settings::{SettingsStore, UserSettings};

#[test]
fn small_svg_gets_a_readable_initial_display_size() {
    let expected_scale = (SVG_PREVIEW_MIN_DISPLAY_EDGE / 16.).max(1.);
    assert_eq!(
        svg_display_scale(size(px(16.), px(16.)), 1.),
        expected_scale
    );
    assert_eq!(svg_display_scale(size(px(512.), px(256.)), 1.), 1.);
}

#[test]
fn zooming_out_keeps_the_base_raster_resolution() {
    assert_eq!(svg_raster_scale(0.8), 1.);
    assert_eq!(svg_raster_scale(1.), 1.);
    assert_eq!(svg_raster_scale(1.5), 1.5);
}

#[gpui::test]
fn preview_starts_loading_and_installs_background_result(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let editor = cx.new(|cx| Editor::single_line(Arc::new(LanguageRegistry::new()), cx));
    editor.update(cx, |editor, cx| {
        editor.set_text(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="8"/>"#,
            cx,
        );
        editor.set_file_path(PathBuf::from("icon.svg"), cx);
    });
    let multi_buffer = cx.read_entity(&editor, |editor, _| editor.multi_buffer());
    let view = cx.new(|cx| {
        SvgPreviewView::new(
            PreviewDocument::Source {
                path: PathBuf::from("icon.svg"),
                source_item: Box::new(editor),
                multi_buffer,
                open_path: None,
            },
            cx,
        )
    });

    cx.read_entity(&view, |view, _| {
        assert!(matches!(view.state, SvgPreviewState::Loading));
        assert!(view.render_task.is_some());
    });
    cx.run_until_parked();
    cx.read_entity(&view, |view, _| {
        assert!(matches!(view.state, SvgPreviewState::Ready(_)));
        assert!(view.render_task.is_none());
    });
}

/// 预览保持独立 Item 身份，源码只通过 PreviewItem 关系访问。
#[gpui::test]
fn preview_keeps_its_identity_and_exposes_source_explicitly(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let editor = cx.new(|cx| Editor::single_line(Arc::new(LanguageRegistry::new()), cx));
    editor.update(cx, |editor, cx| {
        editor.set_text(
            r#"<svg xmlns="http://www.w3.org/2000/svg" width="16" height="8"/>"#,
            cx,
        );
        editor.set_file_path(PathBuf::from("icon.svg"), cx);
    });
    let multi_buffer = cx.read_entity(&editor, |editor, _| editor.multi_buffer());
    let view = cx.new(|cx| {
        SvgPreviewView::new(
            PreviewDocument::Source {
                path: PathBuf::from("icon.svg"),
                source_item: Box::new(editor),
                multi_buffer,
                open_path: None,
            },
            cx,
        )
    });
    let item_id = view.entity_id();
    let handle: Box<dyn zcv_workspace::ItemHandle> = Box::new(view);
    cx.read(|cx| {
        assert!(handle.act_as::<Editor>(cx).is_none());
        let exposed = handle
            .as_preview_item(cx)
            .expect("预览应暴露预览身份")
            .source_item(cx)
            .expect("源码派生预览应提供源码 Item")
            .act_as::<Editor>(cx)
            .expect("源码 Item 应为编辑器");
        assert_ne!(
            exposed.entity_id(),
            item_id,
            "预览暴露的是源码编辑器，自身不是编辑器 Item"
        );
    });
}
