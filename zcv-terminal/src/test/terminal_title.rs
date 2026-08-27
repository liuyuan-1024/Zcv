use std::time::{Duration, Instant};

use gpui::{AppContext as _, TestAppContext};
use zcv_workspace::Item;

use crate::{TerminalBuilder, TerminalView};

#[gpui::test]
async fn tab_title_tracks_current_directory_and_keeps_shell_name(cx: &mut TestAppContext) {
    let temporary_directory = tempfile::tempdir().expect("应创建临时目录");
    let starting_directory = temporary_directory.path().join("starting-directory");
    std::fs::create_dir(&starting_directory).expect("应创建终端启动目录");
    let terminal = cx.new(|cx| {
        TerminalBuilder::new()
            .set_cwd(Some(starting_directory))
            .build(cx)
            .expect("应启动终端")
    });
    let terminal_for_view = terminal.clone();
    let (view, cx) =
        cx.add_window_view(move |_window, cx| TerminalView::new(terminal_for_view, cx));

    wait_for_directory_title(cx, &view, "starting-directory").await;
    let initial_title = cx.update(|_window, cx| view.read(cx).tab_content_text(cx).to_string());
    let shell_suffix = initial_title
        .strip_prefix("starting-directory — ")
        .expect("标题应包含启动目录和 shell")
        .to_owned();

    cx.update(|_window, cx| {
        terminal.update(cx, |terminal, cx| {
            terminal.write_input(b"cd ..\n".to_vec(), cx);
        });
    });

    let expected_directory = temporary_directory
        .path()
        .file_name()
        .expect("临时目录应有名称")
        .to_string_lossy()
        .into_owned();
    wait_for_directory_title(cx, &view, &expected_directory).await;
    let changed_title = cx.update(|_window, cx| view.read(cx).tab_content_text(cx).to_string());
    assert_eq!(
        changed_title,
        format!("{expected_directory} — {shell_suffix}")
    );
}

async fn wait_for_directory_title(
    cx: &mut gpui::VisualTestContext,
    view: &gpui::Entity<TerminalView>,
    directory_name: &str,
) {
    let expected_prefix = format!("{directory_name} — ");
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let matched = cx.update(|window, cx| {
            view.update(cx, |view, cx| view.sync(window, cx));
            view.read(cx)
                .tab_content_text(cx)
                .starts_with(&expected_prefix)
        });
        if matched {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "等待终端目录标题超时：当前为 {}",
            cx.update(|_window, cx| view.read(cx).tab_content_text(cx))
        );
        cx.background_executor
            .timer(Duration::from_millis(20))
            .await;
        cx.run_until_parked();
    }
}
