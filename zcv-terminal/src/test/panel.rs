//! 终端面板行为测试：新终端的工作目录来自面板所属 Project 的根。

use gpui::{AppContext as _, TestAppContext};
use zcv_project::Project;
use zcv_workspace::Panel;

use crate::TerminalPanel;

#[gpui::test]
fn new_terminal_uses_the_project_root_as_working_directory(cx: &mut TestAppContext) {
    let temporary_directory = tempfile::tempdir().expect("应创建临时项目目录");
    let project = cx.new(|cx| Project::new(temporary_directory.path().to_path_buf(), cx));
    let (panel, cx) = cx.add_window_view(move |_window, cx| TerminalPanel::new(project, cx));

    cx.update(|window, cx| {
        panel.update(cx, |panel, cx| panel.new_terminal(window, cx));
    });

    let state = cx
        .update(|_window, cx| panel.read(cx).serialized_state(cx))
        .expect("终端面板应序列化会话");
    let sessions: Vec<serde_json::Value> =
        serde_json::from_value(state).expect("序列化结果应为会话数组");
    assert_eq!(sessions.len(), 1, "应创建一个终端");
    let expected_cwd = serde_json::to_value(temporary_directory.path()).expect("应可序列化路径");
    assert_eq!(
        sessions[0]["cwd"], expected_cwd,
        "新终端的工作目录应为所属项目的根"
    );
}
