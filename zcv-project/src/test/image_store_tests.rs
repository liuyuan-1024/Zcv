use gpui::TestAppContext;

use crate::test_support::test_project;

const SMALL_SVG: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="10" height="10"></svg>"#;
const LARGE_SVG: &[u8] =
    br#"<svg xmlns="http://www.w3.org/2000/svg" width="24" height="24"></svg>"#;

#[gpui::test]
fn image_document_reloads_when_disk_content_changes(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let path = directory.path().join("diagram.svg");
    std::fs::write(&path, SMALL_SVG).expect("应写入初始图片");

    let project = test_project(directory.path().to_path_buf(), cx);
    let image = project
        .update(cx, |project, cx| project.open_image(&path, cx))
        .expect("应能打开图片文件");
    cx.run_until_parked();

    let first = image
        .read_with(cx, |document, _| document.image())
        .expect("首次加载应成功");
    let first_width = first.size(0).width;

    std::fs::write(&path, LARGE_SVG).expect("应覆盖图片内容");
    image.update(cx, |document, cx| document.file_changed_on_disk(cx));
    cx.run_until_parked();

    let second = image
        .read_with(cx, |document, _| document.image())
        .expect("变化后应重新加载成功");
    assert!(
        second.size(0).width > first_width,
        "图片内容变化后应产生新的解码结果"
    );
}

#[gpui::test]
fn open_image_reuses_the_living_document(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let path = directory.path().join("pixel.svg");
    std::fs::write(&path, SMALL_SVG).expect("应写入图片");

    let project = test_project(directory.path().to_path_buf(), cx);
    let first = project
        .update(cx, |project, cx| project.open_image(&path, cx))
        .expect("应能打开图片文件");
    let second = project
        .update(cx, |project, cx| project.open_image(&path, cx))
        .expect("应复用图片文件");
    assert_eq!(
        first.entity_id(),
        second.entity_id(),
        "同一路径必须复用仍然存活的图片文档"
    );
}
