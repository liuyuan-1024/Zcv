use std::path::PathBuf;

use gpui::{AppContext as _, TestAppContext, px, size};
use zcv_editor::Editor;

use super::*;

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
    let editor = cx.new(Editor::single_line);
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
