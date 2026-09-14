//! 终端面板行为测试：新终端的工作目录来自面板所属 Project 的根。

use gpui::{AppContext as _, TestAppContext};
use zcv_project::Project;

use super::project_terminal_cwd;

#[gpui::test]
fn new_terminal_uses_the_project_root_as_working_directory(cx: &mut TestAppContext) {
    let temporary_directory = tempfile::tempdir().expect("应创建临时项目目录");
    let expected_root = temporary_directory
        .path()
        .canonicalize()
        .expect("临时项目根目录应可规范化");
    let project = cx.new(|cx| Project::new(temporary_directory.path().to_path_buf(), cx));
    let cwd = cx.read_entity(&project, |project, _| project_terminal_cwd(project));
    assert_eq!(
        cwd.as_deref(),
        Some(expected_root.as_path()),
        "新终端的工作目录应为所属项目的根"
    );
}
