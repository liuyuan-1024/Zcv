use gpui::TestAppContext;

use super::*;

#[gpui::test]
fn image_preview_loads_a_png_in_the_background(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let path = directory.path().join("pixel.png");
    std::fs::write(
        &path,
        [
            0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48,
            0x44, 0x52, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x06, 0x00, 0x00,
            0x00, 0x1f, 0x15, 0xc4, 0x89, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x44, 0x41, 0x54, 0x78,
            0x9c, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x00, 0x05, 0x00, 0x01, 0xff, 0x89, 0x99,
            0x3d, 0x1d, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44, 0xae, 0x42, 0x60, 0x82,
        ],
    )
    .expect("应写入测试图片");

    let view = cx.new(|cx| ImagePreviewView::new(path, cx));
    cx.read_entity(&view, |view, _| {
        assert!(matches!(view.state, ImagePreviewState::Loading));
        assert!(view.load_task.is_some());
    });
    cx.run_until_parked();
    cx.read_entity(&view, |view, _| {
        assert!(matches!(view.state, ImagePreviewState::Ready(_)));
        assert!(view.load_task.is_none());
    });
}

#[gpui::test]
fn image_preview_provides_breadcrumbs_without_a_source_item(cx: &mut TestAppContext) {
    let path = PathBuf::from("/project/assets/icon.png");
    let view = cx.new(|cx| ImagePreviewView::new(path, cx));
    cx.read(|cx| {
        let preview = view
            .read(cx)
            .as_preview_item(&view, cx)
            .expect("图片应被识别为预览 Item");
        assert!(preview.source_item(cx).is_none());
        let (segments, _) = view
            .read(cx)
            .breadcrumbs(Some(Path::new("/project")), cx)
            .expect("图片应提供文件面包屑");
        let segments: Vec<_> = segments
            .into_iter()
            .map(|segment| segment.to_string())
            .collect();
        assert_eq!(segments, ["assets", "icon.png"]);
    });
}
