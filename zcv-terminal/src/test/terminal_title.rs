use gpui::{AppContext as _, TestAppContext};

use crate::{Terminal, TerminalBuilder};

#[gpui::test]
fn tab_title_tracks_current_directory_and_keeps_shell_name(cx: &mut TestAppContext) {
    let temporary_directory = tempfile::tempdir().expect("应创建临时目录");
    let starting_directory = temporary_directory.path().join("starting-directory");
    std::fs::create_dir(&starting_directory).expect("应创建终端启动目录");
    let terminal = cx.new(|cx| {
        Terminal::new_display_only(
            &TerminalBuilder::new().set_cwd(Some(starting_directory)),
            cx,
        )
    });

    let initial_title = cx.read_entity(&terminal, |terminal, _| terminal.tab_title());
    let shell_suffix = initial_title
        .strip_prefix("starting-directory — ")
        .expect("标题应包含启动目录和 shell")
        .to_owned();

    let expected_directory = temporary_directory
        .path()
        .file_name()
        .expect("临时目录应有名称")
        .to_string_lossy()
        .into_owned();
    terminal.update(cx, |terminal, _| {
        terminal.cwd = Some(temporary_directory.path().to_path_buf())
    });
    let changed_title = cx.read_entity(&terminal, |terminal, _| terminal.tab_title());
    assert_eq!(
        changed_title,
        format!("{expected_directory} — {shell_suffix}")
    );
}
