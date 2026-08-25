use gpui::{AppContext as _, TestAppContext};
use std::process::Command;
use zcv_text::SearchQuery;

use crate::Project;

#[gpui::test]
async fn searches_file_contents_and_builds_ordered_excerpts(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let root = directory.path().canonicalize().expect("项目根应可规范化");
    std::fs::create_dir_all(root.join("src")).expect("应创建源码目录");
    std::fs::write(root.join("src/a.rs"), "first\nneedle one\nlast\n").expect("应创建文件");
    std::fs::write(root.join("src/b.rs"), "needle two\n").expect("应创建文件");
    std::fs::write(root.join("src/c.rs"), "nothing\n").expect("应创建文件");

    let project = cx.new(|cx| Project::new(root, cx));
    let task = project.update(cx, |project, cx| {
        project.search(
            SearchQuery {
                query: "needle".to_string(),
                ..Default::default()
            },
            cx,
        )
    });
    let results = task.await.expect("项目搜索应成功");

    assert_eq!(results.match_count, 2);
    assert_eq!(results.file_count, 2);
    assert_eq!(results.into_excerpts().len(), 2);
}

#[gpui::test]
async fn honors_exclusions_and_reports_invalid_regex(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let root = directory.path().canonicalize().expect("项目根应可规范化");
    std::fs::create_dir_all(root.join("target")).expect("应创建排除目录");
    std::fs::write(root.join("target/hidden.txt"), "needle").expect("应创建文件");
    std::fs::write(root.join("visible.txt"), "needle").expect("应创建文件");

    let project = cx.new(|cx| Project::new(root, cx));
    project.update(cx, |project, _| {
        project.set_exclusions(&["**/target".to_string()]);
    });
    let search = project.update(cx, |project, cx| {
        project.search(
            SearchQuery {
                query: "needle".to_string(),
                ..Default::default()
            },
            cx,
        )
    });
    let results = search.await.expect("项目搜索应成功");
    assert_eq!(results.match_count, 1);
    assert_eq!(results.file_count, 1);

    let invalid = project.update(cx, |project, cx| {
        project.search(
            SearchQuery {
                query: "(".to_string(),
                regex: true,
                ..Default::default()
            },
            cx,
        )
    });
    assert!(invalid.await.is_err());
}

#[gpui::test]
async fn git_search_skips_ignored_build_outputs(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let root = directory.path().canonicalize().expect("项目根应可规范化");
    assert!(
        Command::new("git")
            .args(["init", "--quiet"])
            .current_dir(&root)
            .status()
            .expect("测试环境应能启动 git")
            .success()
    );
    std::fs::write(root.join(".gitignore"), "/target/\n").expect("应创建 .gitignore");
    std::fs::write(root.join("visible.md"), "文本引擎\n").expect("应创建可搜索文件");
    std::fs::create_dir_all(root.join("target/deep")).expect("应创建忽略目录");
    std::fs::write(root.join("target/deep/generated.txt"), "生成引擎\n").expect("应创建被忽略文件");

    let project = cx.new(|cx| Project::new(root, cx));
    let results = project
        .update(cx, |project, cx| {
            project.search(
                SearchQuery {
                    query: "引擎".to_string(),
                    ..Default::default()
                },
                cx,
            )
        })
        .await
        .expect("项目搜索应成功");

    assert_eq!(results.match_count, 1);
    assert_eq!(results.file_count, 1);
}
