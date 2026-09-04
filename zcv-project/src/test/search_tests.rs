use gpui::{AppContext as _, Entity, TestAppContext};
use std::path::PathBuf;
use std::process::Command;
use zcv_text::SearchQuery;

use crate::Project;
use crate::search::FileSearchResult;

/// 收集一次流式搜索的全部命中（直到后台关闭通道）。
async fn collect_search(
    project: &Entity<Project>,
    query: SearchQuery,
    cx: &mut TestAppContext,
) -> Vec<FileSearchResult> {
    let stream = project.update(cx, |project, cx| project.search(query, cx));
    let mut results = Vec::new();
    while let Ok(item) = stream.rx.recv().await {
        results.push(item);
    }
    stream.task.await;
    results
}

fn match_count(results: &[FileSearchResult]) -> usize {
    results
        .iter()
        .flat_map(|file| file.excerpts.iter().map(|excerpt| excerpt.matches.len()))
        .sum()
}

#[gpui::test]
async fn searches_file_contents_and_builds_ordered_excerpts(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let root = directory.path().canonicalize().expect("项目根应可规范化");
    std::fs::create_dir_all(root.join("src")).expect("应创建源码目录");
    std::fs::write(root.join("src/a.rs"), "first\nneedle one\nlast\n").expect("应创建文件");
    std::fs::write(root.join("src/b.rs"), "needle two\n").expect("应创建文件");
    std::fs::write(root.join("src/c.rs"), "nothing\n").expect("应创建文件");

    let project = cx.new(|cx| Project::new(root, cx));
    let results = collect_search(
        &project,
        SearchQuery {
            query: "needle".to_string(),
            ..Default::default()
        },
        cx,
    )
    .await;

    assert_eq!(results.len(), 2);
    assert_eq!(match_count(&results), 2);
    assert_eq!(
        results
            .iter()
            .map(|file| file.display_path.clone())
            .collect::<Vec<_>>(),
        vec![PathBuf::from("src/a.rs"), PathBuf::from("src/b.rs")],
        "并行扫描必须仍按工作区路径顺序交付结果"
    );
    assert!(
        results.iter().all(|file| file
            .excerpts
            .iter()
            .any(|excerpt| excerpt.matches.len() == 1)),
        "每个文件应产出带单个命中的上下文块"
    );
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
    let results = collect_search(
        &project,
        SearchQuery {
            query: "needle".to_string(),
            ..Default::default()
        },
        cx,
    )
    .await;
    assert_eq!(results.len(), 1);
    assert_eq!(match_count(&results), 1);

    // 非法正则不产出任何命中，流正常关闭且不 panic。
    let invalid = collect_search(
        &project,
        SearchQuery {
            query: "(".to_string(),
            regex: true,
            ..Default::default()
        },
        cx,
    )
    .await;
    assert!(invalid.is_empty());
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
    let results = collect_search(
        &project,
        SearchQuery {
            query: "引擎".to_string(),
            ..Default::default()
        },
        cx,
    )
    .await;

    assert_eq!(results.len(), 1);
    assert_eq!(match_count(&results), 1);
}
