use std::path::{Path, PathBuf};

use gpui::{AppContext as _, TestAppContext};
use std::sync::Arc;

use crate::{DiffFile, DisplayHunk};
use zcv_buffer_diff::{BufferDiff, BufferDiffInput, DiffHunkKind, DiffHunkStaging, DiffOperations};
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_text::{
    Buffer, BufferConfig, ByteOffset, CharOffset, Edit, LargeFilePolicy, Line, StorageError,
    TextError, TextRange, TransactionMetadata, Utf16Offset,
};

use super::*;

/// 测试辅助：按路径分组写入，替代已删除的整篇 set_excerpts 入口。
trait SetExcerptsByPath {
    fn set_excerpts(&mut self, excerpts: Vec<ExcerptRange>, cx: &mut Context<Self>)
    where
        Self: Sized;
}

impl SetExcerptsByPath for MultiBuffer {
    fn set_excerpts(&mut self, excerpts: Vec<ExcerptRange>, cx: &mut Context<Self>) {
        for group in group_excerpts_by_path(excerpts, cx) {
            self.set_excerpts_for_path(group, cx);
        }
    }
}

/// 按源文件路径分组；同一路径的片段保持调用方顺序。
fn group_excerpts_by_path(excerpts: Vec<ExcerptRange>, cx: &App) -> Vec<Vec<ExcerptRange>> {
    let mut groups: Vec<(PathBuf, Vec<ExcerptRange>)> = Vec::new();
    for excerpt in excerpts {
        let path = excerpt.source.read(cx).file_path().unwrap_or_default();
        match groups.iter_mut().find(|(candidate, _)| *candidate == path) {
            Some((_, group)) => group.push(excerpt),
            None => groups.push((path, vec![excerpt])),
        }
    }
    groups.into_iter().map(|(_, group)| group).collect()
}

/// 测试辅助：按文本与路径建立修订文档（diff 的 base/index 侧）。
fn revision_document(
    text: &str,
    path: &Path,
    cx: &mut impl gpui::AppContext,
) -> gpui::Entity<LanguageBuffer> {
    let buffer = Buffer::from_text(text.to_string(), BufferConfig::default())
        .expect("测试文本必须能创建 Buffer");
    cx.new(|cx| {
        LanguageBuffer::new(
            buffer,
            Some(path.to_path_buf()),
            Arc::new(LanguageRegistry::new()),
            cx,
        )
    })
}

/// 测试辅助：按可选 base/index 文本建立独立 diff 实体（供直接读取快照的测试使用）。
fn test_diff_entity(
    working: gpui::Entity<LanguageBuffer>,
    path: &str,
    base: Option<&str>,
    index: Option<&str>,
    cx: &mut impl gpui::AppContext,
) -> gpui::Entity<BufferDiff> {
    let native_path = PathBuf::from(path);
    let base = base.map(|text| revision_document(text, &native_path, cx));
    let index = index.map(|text| revision_document(text, &native_path, cx));
    cx.new(|cx| {
        BufferDiff::new(
            BufferDiffInput {
                working,
                path: native_path,
                base,
                index,
                operations: None,
            },
            cx,
        )
    })
}

/// 测试用的 diff 注入描述；由 `inject_diffs` 转成预创建的 `DiffFile`。
struct TestDiff {
    working: gpui::Entity<LanguageBuffer>,
    path: PathBuf,
    base_text: Option<Arc<str>>,
    index_text: Option<Arc<str>>,
    operations: Option<Arc<dyn DiffOperations>>,
    display_path: PathBuf,
    context_lines: Option<usize>,
}

impl MultiBuffer {
    /// 测试辅助：按描述预创建 `BufferDiff`，再以 `DiffFile` 注入。
    fn inject_diffs(&mut self, files: Option<Vec<TestDiff>>, cx: &mut gpui::Context<Self>) -> bool {
        let Some(files) = files else {
            return false;
        };
        let files = files
            .into_iter()
            .map(|file| {
                let base = file
                    .base_text
                    .as_deref()
                    .map(|text| revision_document(text, &file.path, cx));
                let index = file
                    .index_text
                    .as_deref()
                    .map(|text| revision_document(text, &file.path, cx));
                DiffFile {
                    diff: cx.new(|cx| {
                        BufferDiff::new(
                            BufferDiffInput {
                                working: file.working,
                                path: file.path,
                                base,
                                index,
                                operations: file.operations,
                            },
                            cx,
                        )
                    }),
                    display_path: file.display_path,
                    context_lines: file.context_lines,
                }
            })
            .collect();
        self.set_diff_files(files, cx)
    }
}

/// 测试辅助：构造一个注入用 diff 描述。
fn test_diff(working: gpui::Entity<LanguageBuffer>, path: &str, base: &str) -> TestDiff {
    TestDiff {
        working,
        path: PathBuf::from(path),
        base_text: Some(Arc::from(base)),
        index_text: None,
        operations: None,
        display_path: PathBuf::from(path),
        context_lines: None,
    }
}

/// 测试辅助：构造一个可直接 add_diff 的 DiffFile。
fn test_diff_file(
    working: gpui::Entity<LanguageBuffer>,
    path: &str,
    base: &str,
    cx: &mut gpui::Context<MultiBuffer>,
) -> DiffFile {
    let native_path = PathBuf::from(path);
    let base_document = revision_document(base, &native_path, cx);
    DiffFile {
        diff: cx.new(|cx| {
            BufferDiff::new(
                BufferDiffInput {
                    working,
                    path: native_path,
                    base: Some(base_document),
                    index: None,
                    operations: None,
                },
                cx,
            )
        }),
        display_path: PathBuf::from(path),
        context_lines: None,
    }
}

/// 清空 Git 状态后，组合文档必须移除旧的 diff 投影，而不是保留过期 hunk。
/// 移除中间文件后的投影必须与"从一开始就没有该文件"完全一致（含增量下标顺延）。
/// 中段插入文件后的投影必须与"一开始就包含该文件"完全一致（含增量下标顺延）。
#[gpui::test]
fn inserting_middle_diff_file_matches_fresh_three_file_build(cx: &mut TestAppContext) {
    let a = singleton("src/a.rs", "a1\na2\n", cx);
    let b = singleton("src/b.rs", "b1\nb2\n", cx);
    let c = singleton("src/c.rs", "c1\nc2\n", cx);

    let incremental = cx.new(MultiBuffer::empty);
    incremental.update(cx, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![
                test_diff(a.clone(), "src/a.rs", "a1\naX\n"),
                test_diff(c.clone(), "src/c.rs", "c1\ncX\n"),
            ]),
            cx,
        );
    });
    cx.run_until_parked();
    incremental.update(cx, |buffer, cx| {
        buffer.add_diff(test_diff_file(b.clone(), "src/b.rs", "b1\nbX\n", cx), cx);
    });
    cx.run_until_parked();

    let fresh = cx.new(MultiBuffer::empty);
    fresh.update(cx, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![
                test_diff(a.clone(), "src/a.rs", "a1\naX\n"),
                test_diff(b.clone(), "src/b.rs", "b1\nbX\n"),
                test_diff(c.clone(), "src/c.rs", "c1\ncX\n"),
            ]),
            cx,
        );
    });
    cx.run_until_parked();

    cx.update_entity(&incremental, |buffer, cx| {
        buffer.snapshot(cx);
    });
    let incremental_state = cx.read_entity(&incremental, |buffer, _| {
        (
            buffer
                .diff_paths()
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            buffer.diff_hunks().to_vec(),
        )
    });
    let fresh_state = cx.read_entity(&fresh, |buffer, _| {
        (
            buffer
                .diff_paths()
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            buffer.diff_hunks().to_vec(),
        )
    });
    assert_eq!(incremental_state, fresh_state);
}

#[gpui::test]
fn removing_middle_diff_file_matches_fresh_two_file_build(cx: &mut TestAppContext) {
    let a = singleton("src/a.rs", "a1\na2\n", cx);
    let b = singleton("src/b.rs", "b1\nb2\n", cx);
    let c = singleton("src/c.rs", "c1\nc2\n", cx);

    let three = cx.new(MultiBuffer::empty);
    three.update(cx, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![
                test_diff(a.clone(), "src/a.rs", "a1\naX\n"),
                test_diff(b.clone(), "src/b.rs", "b1\nbX\n"),
                test_diff(c.clone(), "src/c.rs", "c1\ncX\n"),
            ]),
            cx,
        );
    });
    cx.run_until_parked();

    let two = cx.new(MultiBuffer::empty);
    two.update(cx, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![
                test_diff(a.clone(), "src/a.rs", "a1\naX\n"),
                test_diff(c.clone(), "src/c.rs", "c1\ncX\n"),
            ]),
            cx,
        );
    });
    cx.run_until_parked();

    let removed = three.update(cx, |buffer, cx| {
        buffer.remove_diff(Path::new("src/b.rs"), cx)
    });
    assert!(removed);

    let incremental = cx.read_entity(&three, |buffer, _| {
        (
            buffer
                .diff_paths()
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            buffer.diff_hunks().to_vec(),
        )
    });
    let fresh = cx.read_entity(&two, |buffer, _| {
        (
            buffer
                .diff_paths()
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            buffer.diff_hunks().to_vec(),
        )
    });
    assert_eq!(incremental, fresh);
}

/// 移除路径顺序末尾的文件后，前一个文件不再需要分隔合成换行，投影必须与全新单文件构建一致。
#[gpui::test]
fn removing_last_diff_file_matches_fresh_single_file_build(cx: &mut TestAppContext) {
    let a = singleton("src/a.rs", "a1\na2", cx);
    let c = singleton("src/c.rs", "c1\nc2", cx);

    let two = cx.new(MultiBuffer::empty);
    two.update(cx, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![
                test_diff(a.clone(), "src/a.rs", "a1\naX\n"),
                test_diff(c.clone(), "src/c.rs", "c1\ncX\n"),
            ]),
            cx,
        );
    });
    cx.run_until_parked();

    let one = cx.new(MultiBuffer::empty);
    one.update(cx, |buffer, cx| {
        buffer.inject_diffs(Some(vec![test_diff(a.clone(), "src/a.rs", "a1\naX\n")]), cx);
    });
    cx.run_until_parked();

    let removed = two.update(cx, |buffer, cx| {
        buffer.remove_diff(Path::new("src/c.rs"), cx)
    });
    assert!(removed);
    cx.run_until_parked();

    let incremental = cx.update_entity(&two, |buffer, cx| {
        String::from_utf8(buffer.snapshot(cx).text_bytes()).expect("投影应为 UTF-8")
    });
    let fresh = cx.update_entity(&one, |buffer, cx| {
        String::from_utf8(buffer.snapshot(cx).text_bytes()).expect("投影应为 UTF-8")
    });
    assert_eq!(incremental, fresh);
}

/// 单个文件的 diff 版本变化必须只原地重物化该路径，其余文件与全新构建一致。
#[gpui::test]
fn single_file_version_change_matches_fresh_three_file_build(cx: &mut TestAppContext) {
    let a = singleton("src/a.rs", "a1\na2\n", cx);
    let b = singleton("src/b.rs", "b1\nb2\nb3\n", cx);
    let c = singleton("src/c.rs", "c1\nc2\n", cx);

    let incremental = cx.new(MultiBuffer::empty);
    incremental.update(cx, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![
                test_diff(a.clone(), "src/a.rs", "a1\naX\n"),
                test_diff(b.clone(), "src/b.rs", "b1\nb2\n"),
                test_diff(c.clone(), "src/c.rs", "c1\ncX\n"),
            ]),
            cx,
        );
    });
    cx.run_until_parked();

    // 编辑 b 的 working 文本，只触发 b 的 diff 版本变化（既有 Added hunk 变为 Modified + Added）；
    // 保存使 b 不再是 dirty 源，diff 结果回合时投影必须跟随新快照重物化。
    cx.update_entity(&b, |source, cx| {
        source
            .edit(
                vec![Edit::replace(
                    TextRange::new(ByteOffset::new(3), ByteOffset::new(5)).unwrap(),
                    "bX",
                )],
                TransactionMetadata::default(),
                cx,
            )
            .unwrap();
        source.mark_saved(cx);
    });
    cx.run_until_parked();

    cx.update_entity(&incremental, |buffer, cx| {
        buffer.snapshot(cx);
    });
    let fresh = cx.new(MultiBuffer::empty);
    fresh.update(cx, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![
                test_diff(a.clone(), "src/a.rs", "a1\naX\n"),
                test_diff(b.clone(), "src/b.rs", "b1\nb2\n"),
                test_diff(c.clone(), "src/c.rs", "c1\ncX\n"),
            ]),
            cx,
        );
    });
    cx.run_until_parked();

    cx.update_entity(&incremental, |buffer, cx| {
        buffer.snapshot(cx);
    });
    cx.run_until_parked();
    cx.update_entity(&incremental, |buffer, cx| {
        buffer.snapshot(cx);
    });
    let incremental_state = cx.read_entity(&incremental, |buffer, _| {
        (
            buffer
                .diff_paths()
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            buffer.diff_hunks().to_vec(),
        )
    });
    let fresh_state = cx.read_entity(&fresh, |buffer, _| {
        (
            buffer
                .diff_paths()
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect::<Vec<_>>(),
            buffer.diff_hunks().to_vec(),
        )
    });
    assert_eq!(incremental_state, fresh_state);
}

/// ProjectDiffView 形态：源路径为绝对路径、显示路径为相对路径。
/// 组合映射树按源路径排序，中间插入与按显示路径移除都必须与之对齐。
#[gpui::test]
fn relative_display_paths_stay_consistent_across_middle_edit(cx: &mut TestAppContext) {
    let a = singleton("/repo/src/a.rs", "a1\na2\n", cx);
    let b = singleton("/repo/src/b.rs", "b1\nb2\n", cx);
    let c = singleton("/repo/src/c.rs", "c1\nc2\n", cx);
    let diff = |working, display: &str, base: &str| TestDiff {
        working,
        path: PathBuf::from(format!("/repo/{display}")),
        base_text: Some(Arc::from(base)),
        index_text: None,
        operations: None,
        display_path: PathBuf::from(display),
        context_lines: None,
    };

    let three = cx.new(MultiBuffer::empty);
    three.update(cx, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![
                diff(a.clone(), "src/a.rs", "a1\naX\n"),
                diff(b.clone(), "src/b.rs", "b1\nbX\n"),
                diff(c.clone(), "src/c.rs", "c1\ncX\n"),
            ]),
            cx,
        );
    });
    cx.run_until_parked();
    let expected = read_diff_state(&three, cx);

    // 按显示路径移除中间文件，必须真正移除对应源路径的 excerpts。
    three.update(cx, |buffer, cx| {
        assert!(buffer.remove_diff(Path::new("src/b.rs"), cx));
    });
    cx.run_until_parked();
    let two = cx.new(MultiBuffer::empty);
    two.update(cx, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![
                diff(a.clone(), "src/a.rs", "a1\naX\n"),
                diff(c.clone(), "src/c.rs", "c1\ncX\n"),
            ]),
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(read_diff_state(&three, cx), read_diff_state(&two, cx));

    // 再以显示相对路径插回中间，投影必须与初始三文件一致。
    let b_diff = DiffFile {
        diff: test_diff_entity(b.clone(), "/repo/src/b.rs", Some("b1\nbX\n"), None, cx),
        display_path: PathBuf::from("src/b.rs"),
        context_lines: None,
    };
    three.update(cx, |buffer, cx| {
        assert!(buffer.add_diff(b_diff, cx));
    });
    cx.run_until_parked();
    assert_eq!(read_diff_state(&three, cx), expected);
}

/// 读取投影的可观察状态（显示路径顺序 + 显示 hunk）。
fn read_diff_state(
    buffer: &gpui::Entity<MultiBuffer>,
    cx: &mut TestAppContext,
) -> (Vec<String>, Vec<DisplayHunk>) {
    cx.read_entity(buffer, |buffer, _| {
        (
            buffer
                .diff_paths()
                .into_iter()
                .map(|path| path.to_string_lossy().into_owned())
                .collect(),
            buffer.diff_hunks().to_vec(),
        )
    })
}

#[gpui::test]
fn clearing_buffer_diffs_removes_previous_hunks(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "one\nworking\nthree\n", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![TestDiff {
                working: source,
                path: PathBuf::from("src/a.rs"),
                base_text: Some(Arc::from("one\nhead\nthree\n")),
                index_text: None,
                operations: None,
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
            }]),
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(
        cx.read_entity(&combined, |buffer, _| buffer.diff_hunks().len()),
        1
    );

    cx.update_entity(&combined, |buffer, cx| {
        buffer.clear_diffs(cx);
    });
    assert!(cx.read_entity(&combined, |buffer, _| buffer.diff_hunks().is_empty()));
}

/// 未提交 diff 以 HEAD 为 base、工作区为 working，由 index → working 参照逐 hunk 标注暂存语义。
#[gpui::test]
fn unified_diff_marks_staged_and_unstaged_hunks(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "one\nworking\nthree\n", cx);
    let diff = test_diff_entity(
        source.clone(),
        "src/a.rs",
        Some("one\nhead\nthree\n"),
        Some("one\nindex\nthree\n"),
        cx,
    );
    cx.run_until_parked();
    assert_eq!(
        cx.read_entity(&diff, |diff, _| {
            diff.snapshot()
                .visible_hunks()
                .into_iter()
                .map(|hunk| hunk.staging)
                .collect::<Vec<_>>()
        }),
        vec![DiffHunkStaging::Unstaged]
    );

    let staged_source = singleton("src/staged.rs", "one\nstaged\nthree\n", cx);
    let staged_diff = test_diff_entity(
        staged_source,
        "src/staged.rs",
        Some("one\nhead\nthree\n"),
        Some("one\nstaged\nthree\n"),
        cx,
    );
    cx.run_until_parked();
    assert_eq!(
        cx.read_entity(&staged_diff, |diff, _| {
            diff.snapshot()
                .visible_hunks()
                .into_iter()
                .map(|hunk| hunk.staging)
                .collect::<Vec<_>>()
        }),
        vec![DiffHunkStaging::Staged]
    );
}

/// 同一文件同时存在已暂存与未暂存 hunk 时逐 hunk 判定，而不是整文件一刀切。
#[gpui::test]
fn unified_diff_marks_mixed_staged_and_unstaged_hunks(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "a\nB\nc\nd\nE\n", cx);
    let diff = test_diff_entity(
        source.clone(),
        "src/a.rs",
        Some("a\nb\nc\nd\ne\n"),
        Some("a\nB\nc\nd\ne\n"),
        cx,
    );
    cx.run_until_parked();
    assert_eq!(
        cx.read_entity(&diff, |diff, _| {
            diff.snapshot()
                .visible_hunks()
                .into_iter()
                .map(|hunk| hunk.staging)
                .collect::<Vec<_>>()
        }),
        vec![DiffHunkStaging::Staged, DiffHunkStaging::Unstaged],
        "b→B 在 index 中已是该内容（已暂存），e→E 仅在 worktree（未暂存）"
    );
}

/// 已暂存视图：working 本身就是 index，分类自然得到全部 Staged。
#[gpui::test]
fn standalone_staged_view_classifies_all_hunks_as_staged(cx: &mut TestAppContext) {
    let index_source = singleton("src/a.rs", "one\nstaged\nthree\n", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(index_source.clone(), cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![TestDiff {
                operations: None,
                working: index_source.clone(),
                base_text: Some(Arc::from("one\nhead\nthree\n")),
                index_text: Some(Arc::from("one\nstaged\nthree\n")),
                path: PathBuf::from("src/a.rs"),
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
            }]),
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(
        cx.read_entity(&combined, |buffer, _| {
            buffer
                .diff_hunks()
                .iter()
                .map(|hunk| hunk.staging)
                .collect::<Vec<_>>()
        }),
        vec![DiffHunkStaging::Staged],
        "已暂存视图的 hunk 应带 Staged 语义（空心）"
    );
}

/// 已暂存 hunk 又被部分编辑（同段内追加未暂存内容）时，HEAD→工作区主 hunk 与
/// index→工作区 hunk 部分重叠，应标记为 PartiallyStaged（实心渲染）。
#[gpui::test]
fn staged_hunk_with_partial_unstaged_edit_is_partially_staged(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "line1\nSTAGED\nNEW\nline3\n", cx);
    let diff = test_diff_entity(
        source.clone(),
        "src/a.rs",
        Some("line1\nline2\nline3\n"),
        Some("line1\nSTAGED\nline3\n"),
        cx,
    );
    cx.run_until_parked();
    let stagings = cx.read_entity(&diff, |diff, _| {
        diff.snapshot()
            .visible_hunks()
            .into_iter()
            .map(|hunk| hunk.staging)
            .collect::<Vec<_>>()
    });
    assert_eq!(stagings, vec![DiffHunkStaging::PartiallyStaged]);
}

/// 主 hunk 与未暂存 hunk 仅部分重叠时标记为部分暂存。
#[gpui::test]
fn unified_diff_marks_partially_staged_hunk(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "one\nX\nY\nthree\n", cx);
    let diff = test_diff_entity(
        source.clone(),
        "src/a.rs",
        Some("one\nhead\nthree\n"),
        Some("one\nX\nthree\n"),
        cx,
    );
    cx.run_until_parked();
    assert_eq!(
        cx.read_entity(&diff, |diff, _| {
            diff.snapshot()
                .visible_hunks()
                .into_iter()
                .map(|hunk| hunk.staging)
                .collect::<Vec<_>>()
        }),
        vec![DiffHunkStaging::PartiallyStaged]
    );
}

/// 回归：不改变 diff 几何的刷新既不推进语言元数据，也不创建新的显示输入。
#[gpui::test]
fn unchanged_diff_display_does_not_advance_metadata_or_display_version(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "one\nworking\nthree\n", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![TestDiff {
                operations: None,
                working: source,
                base_text: Some(Arc::from("one\nhead\nthree\n")),
                index_text: Some(Arc::from("one\nindex\nthree\n")),
                path: PathBuf::from("src/a.rs"),
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
            }]),
            cx,
        );
    });
    cx.run_until_parked();

    let before = cx.update_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        (
            snapshot.metadata_version(),
            snapshot
                .diff_display()
                .expect("已注入 diff 必须携带显示输入")
                .version(),
        )
    });
    cx.update_entity(&combined, |buffer, cx| buffer.refresh_diff_display(cx));
    let after = cx.update_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        (
            snapshot.metadata_version(),
            snapshot
                .diff_display()
                .expect("已注入 diff 必须携带显示输入")
                .version(),
        )
    });
    assert_eq!(
        after, before,
        "几何不变的 diff 刷新不能推进元数据或显示版本（before={before:?}, after={after:?}）"
    );
}

/// 回归：展开状态改变只推进 diff 显示版本，不能污染语法／设置元数据版本。
#[gpui::test]
fn diff_display_change_uses_its_own_version(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "one\nworking\nthree\n", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![test_diff(source, "src/a.rs", "one\nhead\nthree\n")]),
            cx,
        );
    });
    cx.run_until_parked();

    let before = cx.update_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        (
            snapshot.metadata_version(),
            snapshot
                .diff_display()
                .expect("已注入 diff 必须携带显示输入")
                .version(),
        )
    });
    cx.update_entity(&combined, |buffer, cx| buffer.toggle_diff_hunk_at(0, cx));
    let after = cx.update_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        (
            snapshot.metadata_version(),
            snapshot
                .diff_display()
                .expect("已注入 diff 必须携带显示输入")
                .version(),
        )
    });

    assert_eq!(after.0, before.0, "diff 显示变化不能推进语言元数据版本");
    assert!(after.1 > before.1, "展开状态变化必须推进 diff 显示版本");
}

fn singleton(path: &str, text: &str, cx: &mut TestAppContext) -> gpui::Entity<LanguageBuffer> {
    let buffer =
        Buffer::from_text(text.to_owned(), BufferConfig::default()).expect("应创建测试 Buffer");
    cx.new(|cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from(path)),
            Arc::new(LanguageRegistry::new()),
            cx,
        )
    })
}

/// 测试辅助：在小阈值大文件策略下构造自动只读的源，用于多源编辑预检。
fn read_only_singleton(
    path: &str,
    text: &str,
    cx: &mut TestAppContext,
) -> gpui::Entity<LanguageBuffer> {
    let config = BufferConfig {
        large_file: LargeFilePolicy {
            large_file_threshold_bytes: 1,
            auto_read_only_on_large_file: true,
            ..LargeFilePolicy::default()
        },
    };
    let buffer = Buffer::from_text(text.to_owned(), config).expect("应创建测试 Buffer");
    assert!(buffer.is_read_only(), "超过 1 字节的文本必须自动切只读");
    cx.new(|cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from(path)),
            Arc::new(LanguageRegistry::new()),
            cx,
        )
    })
}

#[gpui::test]
fn title_prefers_explicit_value_and_derives_from_path(cx: &mut TestAppContext) {
    let source = singleton(
        "src/main.rs",
        "fn main() {}
",
        cx,
    );
    let multi_buffer = cx.new(|cx| MultiBuffer::singleton(source, cx));

    cx.update_entity(&multi_buffer, |buffer, cx| {
        assert_eq!(buffer.title(cx).as_deref(), Some("main.rs"));
    });
    cx.update_entity(&multi_buffer, |buffer, cx| {
        buffer.set_title(Some("变更".to_owned()), cx);
    });
    cx.update_entity(&multi_buffer, |buffer, cx| {
        assert_eq!(buffer.title(cx).as_deref(), Some("变更"));
    });
    cx.update_entity(&multi_buffer, |buffer, cx| {
        buffer.set_title(None, cx);
    });
    cx.update_entity(&multi_buffer, |buffer, cx| {
        assert_eq!(buffer.title(cx).as_deref(), Some("main.rs"));
    });
}

#[gpui::test]
fn anchor_resolves_to_neighbor_path_after_removal(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "a\n", cx);
    let second = singleton("src/b.rs", "b\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::line_range(first.clone(), 0..1, cx),
                ExcerptRange::line_range(second.clone(), 0..1, cx),
            ],
            cx,
        );
    });

    // b.rs 的组合起点是 2（a "a\n" 占 2 字节）。
    let anchor = cx.read_entity(&combined, |buffer, _| {
        buffer.anchor_at(ByteOffset::new(2), Affinity::After)
    });

    cx.update_entity(&combined, |buffer, cx| {
        buffer.remove_excerpts_for_path(Path::new("src/b.rs"), cx)
    });

    let resolved = cx.read_entity(&combined, |buffer, _| buffer.resolve_anchor(&anchor));
    assert_eq!(
        resolved,
        Some(Into::into(ByteOffset::new(2))),
        "b.rs 消失后应回退到前驱 a.rs 的末尾"
    );
}

#[gpui::test]
fn excerpt_at_output_offset_uses_the_offset_cursor(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "a\nb\n", cx);
    let second = singleton("src/b.rs", "c\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::line_range(first, 0..2, cx),
                ExcerptRange::line_range(second, 0..1, cx),
            ],
            cx,
        );
    });

    let snapshot = cx.update_entity(&combined, |buffer, cx| buffer.snapshot(cx));
    assert_eq!(snapshot.len_bytes(), MultiBufferOffset::new(6));
    assert_eq!(
        snapshot
            .excerpt_at_output_offset(ByteOffset::new(0).into())
            .map(|excerpt| excerpt.path().to_path_buf()),
        Some(PathBuf::from("src/a.rs"))
    );
    assert_eq!(
        snapshot
            .excerpt_at_output_offset(ByteOffset::new(4).into())
            .map(|excerpt| excerpt.path().to_path_buf()),
        Some(PathBuf::from("src/b.rs"))
    );
}

#[gpui::test]
fn set_excerpts_for_path_replaces_only_that_path(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "a\nb\n", cx);
    let second = singleton("src/b.rs", "c\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::line_range(first.clone(), 0..1, cx),
                ExcerptRange::line_range(second.clone(), 0..1, cx),
            ],
            cx,
        );
    });

    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts_for_path(
            vec![
                ExcerptRange::line_range(first.clone(), 0..1, cx),
                ExcerptRange::line_range(first.clone(), 1..2, cx),
            ],
            cx,
        )
    });

    let snapshot = cx.update_entity(&combined, |buffer, cx| buffer.snapshot(cx));
    assert_eq!(snapshot.excerpts().count(), 3);
    assert_eq!(snapshot.excerpts_for_path(Path::new("src/a.rs")).count(), 2);
    assert_eq!(snapshot.excerpts_for_path(Path::new("src/b.rs")).count(), 1);
    assert_eq!(
        snapshot.excerpts().next().unwrap().path(),
        Path::new("src/a.rs")
    );
    assert_eq!(
        snapshot.excerpts().nth(1).unwrap().path(),
        Path::new("src/a.rs")
    );
    assert_eq!(
        snapshot.excerpts().nth(2).unwrap().path(),
        Path::new("src/b.rs")
    );
}

/// M-8：excerpt 增删等结构变更必须在文本事务之外，否则组合事务身份与坐标基准会错配。
#[gpui::test]
#[should_panic(expected = "set_excerpts_for_path 必须在文本事务之外")]
fn structural_change_inside_a_transaction_fails(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "a\nb\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(vec![ExcerptRange::line_range(first.clone(), 0..1, cx)], cx);
    });

    cx.update_entity(&combined, |buffer, cx| {
        buffer
            .start_transaction(cx)
            .expect("空组合文档应能开始事务");
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(first.clone(), 0..1, cx)], cx);
    });
}

#[gpui::test]
fn excerpt_view_is_derived_and_shared_once_per_snapshot(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "a\nb\n", cx);
    let second = singleton("src/b.rs", "c\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::line_range(first.clone(), 0..1, cx),
                ExcerptRange::line_range(second.clone(), 0..1, cx),
            ],
            cx,
        );
    });

    let snapshot = cx.update_entity(&combined, |buffer, cx| buffer.snapshot(cx));
    let view = snapshot.excerpts_arc();
    // 派生视图按快照惰性物化一次；重复读取共用同一份分配，权威树不因此变成第二数据源。
    assert!(Arc::ptr_eq(&view, &snapshot.excerpts_arc()));
    assert_eq!(view.len(), 2);
}

#[gpui::test]
fn remove_excerpts_for_path_drops_only_that_path(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "a", cx);
    let second = singleton("src/b.rs", "b", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::line_range(first, 0..1, cx),
                ExcerptRange::line_range(second, 0..1, cx),
            ],
            cx,
        );
    });

    let before = cx.update_entity(&combined, |buffer, cx| buffer.snapshot(cx).len_bytes());
    assert_eq!(before, MultiBufferOffset::new(3), "a + 合成换行 + b");

    let removed = cx.update_entity(&combined, |buffer, cx| {
        buffer.remove_excerpts_for_path(Path::new("src/a.rs"), cx)
    });
    assert!(removed);

    let snapshot = cx.update_entity(&combined, |buffer, cx| buffer.snapshot(cx));
    assert_eq!(snapshot.excerpts().count(), 1);
    assert_eq!(
        snapshot.excerpts().next().unwrap().path(),
        Path::new("src/b.rs")
    );
    assert_eq!(
        snapshot.len_bytes(),
        MultiBufferOffset::new(1),
        "b 成为末尾片段后不再补合成换行"
    );

    let again = cx.update_entity(&combined, |buffer, cx| {
        buffer.remove_excerpts_for_path(Path::new("src/a.rs"), cx)
    });
    assert!(!again);
}

#[gpui::test]
fn excerpts_for_path_uses_the_path_cursor(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "a\n", cx);
    let second = singleton("src/b.rs", "b\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::line_range(first, 0..1, cx),
                ExcerptRange::line_range(second, 0..1, cx),
            ],
            cx,
        );
    });

    let snapshot = cx.update_entity(&combined, |buffer, cx| buffer.snapshot(cx));
    let paths = snapshot
        .excerpts_for_path(Path::new("src/a.rs"))
        .map(|excerpt| excerpt.path().to_path_buf())
        .collect::<Vec<_>>();
    assert_eq!(paths, vec![PathBuf::from("src/a.rs")]);
    assert_eq!(snapshot.excerpts_for_path(Path::new("src/c.rs")).count(), 0);
}

#[gpui::test]
fn text_chunks_stream_excerpt_sources_and_inserted_boundary(cx: &mut TestAppContext) {
    let first = singleton("src/first.rs", "first", cx);
    let second = singleton("src/second.rs", "second\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::line_range(first, 0..1, cx),
                ExcerptRange::line_range(second, 0..1, cx),
            ],
            cx,
        );
    });

    let snapshot = cx.update_entity(&combined, |buffer, cx| buffer.snapshot(cx));
    let chunks = snapshot
        .bytes_in_range(ByteOffset::ZERO.into()..snapshot.len_bytes())
        .collect::<Vec<_>>();

    assert_eq!(
        chunks.iter().map(|chunk| chunk.text).collect::<String>(),
        "first\nsecond\n"
    );
    assert_eq!(
        chunks[0].output_range,
        MultiBufferOffset::new(0)..MultiBufferOffset::new(5)
    );
    assert_eq!(chunks[1].text, "\n");
    assert_eq!(
        chunks[1].output_range,
        MultiBufferOffset::new(5)..MultiBufferOffset::new(6)
    );
    assert_eq!(
        chunks
            .last()
            .expect("第二个 excerpt 必须有文本")
            .output_range
            .end,
        snapshot.len_bytes()
    );
    assert_eq!(snapshot.text_bytes(), b"first\nsecond\n");
    assert_eq!(snapshot.line_count(), 3);
    assert_eq!(
        snapshot.byte_to_line(ByteOffset::new(6).into()).unwrap(),
        Line::new(1)
    );
    assert_eq!(
        snapshot.line_start_byte(Line::new(1)).unwrap(),
        MultiBufferOffset::new(6)
    );
    assert_eq!(
        snapshot
            .byte_to_position(ByteOffset::new(9).into())
            .unwrap(),
        zcv_text::Position::new(Line::new(1), zcv_text::LogicalColumn::new(3))
    );
    assert_eq!(
        snapshot.line_start_byte(Line::new(2)).unwrap(),
        snapshot.len_bytes()
    );
    assert_eq!(
        snapshot.byte_to_line(snapshot.len_bytes()).unwrap(),
        Line::new(2)
    );
    assert!(snapshot.line_start_byte(Line::new(3)).is_err());
}

#[test]
fn plain_snapshot_streams_its_source_without_materializing() {
    let buffer = Buffer::from_text("alpha\nbeta".to_string(), BufferConfig::default())
        .expect("测试文本必须能创建");
    let snapshot = MultiBufferSnapshot::from(buffer.snapshot());

    let chunks = snapshot
        .bytes_in_range(MultiBufferOffset::new(2)..MultiBufferOffset::new(8))
        .map(|chunk| chunk.text)
        .collect::<String>();
    assert_eq!(chunks, "pha\nbe");
    assert_eq!(
        snapshot.chunk_at_byte(ByteOffset::new(6).into()).unwrap().0,
        "alpha\nbeta"
    );
}

#[gpui::test]
fn composite_snapshot_remains_immutable_until_a_new_frame_is_read(cx: &mut TestAppContext) {
    let source = singleton("src/main.rs", "one\ntwo\n", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    let before = cx.update_entity(&combined, |buffer, cx| buffer.snapshot(cx));
    cx.update_entity(&source, |source, cx| {
        source
            .edit(
                [Edit::insert(ByteOffset::ZERO, "zero\n").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("源编辑应成功");
    });
    cx.run_until_parked();

    let after = cx.update_entity(&combined, |buffer, cx| buffer.snapshot(cx));
    assert_ne!(before.version(), after.version());
    assert_eq!(before.text_bytes(), b"one\ntwo\n");
    assert_eq!(after.text_bytes(), b"zero\none\ntwo\n");
    assert_eq!(before.line_count(), 3);
    assert_eq!(after.line_count(), 4);
}

#[gpui::test]
fn singleton_source_updates_the_display_stream_incrementally(cx: &mut TestAppContext) {
    let source = singleton("src/main.rs", "fn main() {}\n", cx);
    let multi_buffer = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    cx.update_entity(&multi_buffer, |buffer, cx| {
        assert_eq!(buffer.file_path(cx), Some(PathBuf::from("src/main.rs")));
        assert!(buffer.singleton_source().is_some(), "应为整文件单 excerpt");
    });
    let subscription = cx.update_entity(&multi_buffer, |buffer, cx| {
        buffer.subscribe_and_snapshot(cx).0
    });
    let source_subscription = cx.read_entity(&source, |source, _| source.subscribe());

    cx.update_entity(&source, |source, cx| {
        source
            .edit(
                [Edit::insert(ByteOffset::new(3), "async ").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("测试编辑应成功");
    });
    cx.run_until_parked();

    cx.update_entity(&multi_buffer, |buffer, cx| {
        buffer.snapshot(cx);
    });
    let source_changes = source_subscription.consume();
    let projection_changes = subscription.consume();
    assert_eq!(
        projection_changes.transaction_id(),
        source_changes.transaction_id(),
        "组合投影必须保留源事务身份"
    );
    assert_eq!(
        projection_changes.patch(),
        source_changes.patch(),
        "单文件投影必须直接转发源批次的编辑范围"
    );

    let updated = cx.update_entity(&multi_buffer, |buffer, cx| buffer.snapshot(cx));
    assert_eq!(
        String::from_utf8(updated.text_bytes()).expect("编辑器快照必须是 UTF-8"),
        "fn async main() {}\n"
    );
    assert!(
        updated.metadata_version() > 0,
        "源编辑后的组合快照必须携带新的源元数据版本"
    );

    cx.update_entity(&multi_buffer, |buffer, cx| {
        buffer.undo(cx).expect("单文件源撤销应成功");
    });
    assert_eq!(
        cx.update_entity(&multi_buffer, |buffer, cx| {
            String::from_utf8(buffer.snapshot(cx).text_bytes()).expect("编辑器快照必须是 UTF-8")
        }),
        "fn main() {}\n"
    );
    let _ = subscription.consume();
}

#[gpui::test]
fn source_edit_updates_only_its_composite_excerpt(cx: &mut TestAppContext) {
    let first = singleton("src/first.rs", "first\n", cx);
    let second = singleton("src/second.rs", "second\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::line_range(first, 0..1, cx),
                ExcerptRange::line_range(second.clone(), 0..1, cx),
            ],
            cx,
        );
    });
    let subscription =
        cx.update_entity(&combined, |buffer, cx| buffer.subscribe_and_snapshot(cx).0);

    cx.update_entity(&second, |source, cx| {
        source
            .edit(
                [Edit::insert(ByteOffset::new(0), "changed ").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("源编辑应成功");
    });
    cx.run_until_parked();

    cx.update_entity(&combined, |buffer, cx| {
        buffer.snapshot(cx);
    });
    let changes = subscription.consume();
    assert!(!changes.patch().is_empty(), "源编辑必须发布组合输出编辑");
    assert_eq!(
        cx.update_entity(&combined, |buffer, cx| {
            String::from_utf8(buffer.snapshot(cx).text_bytes()).expect("组合文本必须是 UTF-8")
        }),
        "first\nchanged second\n"
    );
}
#[gpui::test]
fn multiple_source_edits_before_a_read_compose_into_one_incremental_batch(cx: &mut TestAppContext) {
    let first = singleton("src/first.rs", "first\n", cx);
    let second = singleton("src/second.rs", "second\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::line_range(first.clone(), 0..1, cx),
                ExcerptRange::line_range(second.clone(), 0..1, cx),
            ],
            cx,
        );
    });
    let subscription =
        cx.update_entity(&combined, |buffer, cx| buffer.subscribe_and_snapshot(cx).0);

    // 一次读取前两个源各编辑一次：组合层必须组合为一段连续增量，而不是整体重载。
    cx.update_entity(&first, |source, cx| {
        source
            .edit(
                [Edit::insert(ByteOffset::ZERO, "a ").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("源编辑应成功");
    });
    cx.update_entity(&second, |source, cx| {
        source
            .edit(
                [Edit::insert(ByteOffset::ZERO, "b ").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("源编辑应成功");
    });
    cx.run_until_parked();

    cx.update_entity(&combined, |buffer, cx| {
        buffer.snapshot(cx);
    });
    let changes = subscription.consume();
    assert_eq!(changes.patch().edits().len(), 2);
    assert_eq!(
        cx.update_entity(&combined, |buffer, cx| {
            String::from_utf8(buffer.snapshot(cx).text_bytes()).expect("组合文本必须是 UTF-8")
        }),
        "a first\nb second\n"
    );
}

#[gpui::test]
fn source_edit_outside_excerpts_publishes_an_empty_incremental_batch(cx: &mut TestAppContext) {
    let source = singleton("src/partial.rs", "shown\nhidden\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(vec![ExcerptRange::line_range(source.clone(), 0..1, cx)], cx);
    });
    let subscription =
        cx.update_entity(&combined, |buffer, cx| buffer.subscribe_and_snapshot(cx).0);

    // 编辑第二行：不在任何 excerpt 内，组合输出几何不变，仍必须发布空批次。
    cx.update_entity(&source, |source, cx| {
        source
            .edit(
                [Edit::insert(ByteOffset::new(12), "more ").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("源编辑应成功");
    });
    cx.run_until_parked();

    cx.update_entity(&combined, |buffer, cx| {
        buffer.snapshot(cx);
    });
    let changes = subscription.consume();
    assert!(
        changes.patch().edits().is_empty(),
        "未展示区域的源编辑不产生输出编辑"
    );
    assert_eq!(
        cx.update_entity(&combined, |buffer, cx| {
            String::from_utf8(buffer.snapshot(cx).text_bytes()).expect("组合文本必须是 UTF-8")
        }),
        "shown\n"
    );
}

#[gpui::test]
fn one_source_edit_updates_all_visible_excerpts_incrementally(cx: &mut TestAppContext) {
    let source = singleton("src/repeated.rs", "line\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::line_range(source.clone(), 0..1, cx),
                ExcerptRange::line_range(source.clone(), 0..1, cx),
            ],
            cx,
        );
    });
    let subscription =
        cx.update_entity(&combined, |buffer, cx| buffer.subscribe_and_snapshot(cx).0);
    cx.update_entity(&source, |source, cx| {
        source
            .edit(
                [Edit::insert(ByteOffset::ZERO, "changed ").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("源编辑应成功");
    });
    cx.run_until_parked();

    cx.update_entity(&combined, |buffer, cx| {
        buffer.snapshot(cx);
    });
    let changes = subscription.consume();
    assert_eq!(changes.patch().edits().len(), 2);
    assert_eq!(
        cx.update_entity(&combined, |buffer, cx| {
            String::from_utf8(buffer.snapshot(cx).text_bytes()).expect("组合文本必须是 UTF-8")
        }),
        "changed line\nchanged line\n"
    );
}

#[gpui::test]
fn excerpt_topology_changes_publish_output_edits(cx: &mut TestAppContext) {
    let first = singleton("src/first.rs", "first\n", cx);
    let second = singleton("src/second.rs", "second\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(vec![ExcerptRange::line_range(first.clone(), 0..1, cx)], cx);
    });
    let subscription =
        cx.update_entity(&combined, |buffer, cx| buffer.subscribe_and_snapshot(cx).0);

    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(second, 0..1, cx)], cx);
    });

    let changes = subscription.consume();
    assert_eq!(changes.patch().edits().len(), 1);
    assert_eq!(
        changes.patch().edits()[0].old_range(),
        TextRange::new(ByteOffset::new(6), ByteOffset::new(6)).unwrap()
    );
    assert_eq!(
        changes.patch().edits()[0].new_range(),
        TextRange::new(ByteOffset::new(6), ByteOffset::new(13)).unwrap()
    );
}

#[gpui::test]
fn dropping_a_middle_excerpt_publishes_a_single_output_edit(cx: &mut TestAppContext) {
    let first = singleton("src/first.rs", "a\n", cx);
    let second = singleton("src/second.rs", "b\n", cx);
    let third = singleton("src/third.rs", "c\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::line_range(first.clone(), 0..1, cx),
                ExcerptRange::line_range(second, 0..1, cx),
                ExcerptRange::line_range(third.clone(), 0..1, cx),
            ],
            cx,
        );
    });
    let subscription =
        cx.update_entity(&combined, |buffer, cx| buffer.subscribe_and_snapshot(cx).0);

    cx.update_entity(&combined, |buffer, cx| {
        buffer.remove_excerpts_for_path(Path::new("src/second.rs"), cx);
    });

    // 组合拓扑变化必须沿增量 output edit 发布；范围由前后 excerpt 游标推导，不物化组合文本。
    let changes = subscription.consume();
    assert_eq!(changes.patch().edits().len(), 1);
    assert_eq!(
        changes.patch().edits()[0].old_range(),
        TextRange::new(ByteOffset::new(2), ByteOffset::new(4)).unwrap()
    );
    assert_eq!(
        changes.patch().edits()[0].new_range(),
        TextRange::new(ByteOffset::new(2), ByteOffset::new(2)).unwrap()
    );
}

#[gpui::test]
fn composite_char_and_utf16_coordinates_count_synthetic_newlines(cx: &mut TestAppContext) {
    let first = singleton("src/first.rs", "αβ", cx);
    let second = singleton("src/second.rs", "γ\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::line_range(first, 0..1, cx),
                ExcerptRange::line_range(second, 0..1, cx),
            ],
            cx,
        );
    });

    // 组合文本为「αβ\nγ\n」：excerpt 间的合成换行同时计入 char 与 UTF-16 坐标。
    let snapshot = cx.update_entity(&combined, |buffer, cx| buffer.snapshot(cx));
    assert_eq!(snapshot.len_bytes(), MultiBufferOffset::new(8));
    for (byte, character) in [(0, 0), (4, 2), (5, 3), (8, 5)] {
        assert_eq!(
            snapshot.byte_to_char(ByteOffset::new(byte).into()).unwrap(),
            CharOffset::new(character)
        );
        assert_eq!(
            snapshot.char_to_byte(CharOffset::new(character)).unwrap(),
            MultiBufferOffset::new(byte)
        );
    }
    for (byte, units) in [(0, 0), (4, 2), (5, 3), (8, 5)] {
        assert_eq!(
            snapshot
                .byte_to_utf16_cu(ByteOffset::new(byte).into())
                .unwrap(),
            Utf16Offset::new(units)
        );
        assert_eq!(
            snapshot.utf16_cu_to_byte(Utf16Offset::new(units)).unwrap(),
            MultiBufferOffset::new(byte)
        );
    }
}

#[gpui::test]
fn source_excerpts_and_display_transforms_use_separate_coordinate_trees(cx: &mut TestAppContext) {
    let source = singleton("src/diff.rs", "working\nremoved", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::new(
                    source.clone(),
                    TextRange::new(ByteOffset::new(0), ByteOffset::new(7)).unwrap(),
                    Vec::new(),
                ),
                ExcerptRange::new(
                    source,
                    TextRange::new(ByteOffset::new(8), ByteOffset::new(15)).unwrap(),
                    Vec::new(),
                )
                .with_diff_kind(ExcerptDiffKind::Deleted)
                .with_editable(false),
            ],
            cx,
        );
    });

    cx.update_entity(&combined, |buffer, cx| {
        // 删除块只占输出坐标：输入树只含消费输入的工作区片段。
        assert_eq!(buffer.state.excerpts.summary().count, 1);
        assert_eq!(buffer.state.diff_transforms.summary().output.count, 2);
        assert_eq!(buffer.state.diff_transforms.summary().input.len.get(), 7);
        assert_eq!(buffer.state.diff_transforms.summary().output.text.len, 15);

        let snapshot = buffer.snapshot(cx);
        assert_eq!(snapshot.excerpts.summary().count, 1);
        assert_eq!(snapshot.diff_transforms.summary().output.count, 2);
        assert_eq!(snapshot.excerpts().count(), 2);
        assert_eq!(
            snapshot.excerpts().nth(1).unwrap().diff_kind(),
            Some(crate::ExcerptDiffKind::Deleted)
        );
    });
}

#[gpui::test]
fn singleton_role_does_not_depend_on_current_excerpt_shape(cx: &mut TestAppContext) {
    let source = singleton("src/main.rs", "first\nsecond\n", cx);
    let working = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    cx.update_entity(&working, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::line_range(source.clone(), 0..1, cx),
                ExcerptRange::line_range(source.clone(), 1..2, cx),
            ],
            cx,
        );
    });
    cx.read_entity(&working, |buffer, _| {
        assert_eq!(buffer.singleton_source(), Some(source.clone()));
    });

    let composite = cx.new(MultiBuffer::empty);
    cx.update_entity(&composite, |buffer, cx| {
        buffer.set_excerpts(
            vec![ExcerptRange::new(
                source,
                TextRange::new(ByteOffset::ZERO, ByteOffset::new(13)).unwrap(),
                Vec::new(),
            )],
            cx,
        );
    });
    cx.read_entity(&composite, |buffer, _| {
        assert!(
            buffer.singleton_source().is_none(),
            "完整文件单 excerpt 也不能把组合文档误判为普通文档"
        );
    });
}

#[gpui::test]
fn outline_projects_source_ranges_into_an_excerpt(cx: &mut TestAppContext) {
    let source = singleton("src/main.rs", "// 前置\nfn 数据() {}\n// 后置\n", cx);
    cx.run_until_parked();
    let (function_range, source_name_start) = cx.read_entity(&source, |source, _| {
        let snapshot = source.snapshot();
        let item = snapshot
            .syntax
            .outline(0..snapshot.text.len_bytes().get(), &snapshot.text)
            .into_iter()
            .find(|item| item.name == "数据")
            .expect("Rust 函数应出现在源大纲中");
        (item.range, item.name_range.start)
    });
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![ExcerptRange::new(
                source,
                TextRange::new(
                    ByteOffset::new(function_range.start),
                    ByteOffset::new(function_range.end),
                )
                .unwrap(),
                Vec::new(),
            )],
            cx,
        );
    });

    let items = cx.update_entity(&combined, |buffer, cx| buffer.snapshot(cx).outline_items());
    let function = items
        .iter()
        .find(|item| item.name == "数据")
        .expect("excerpt 内函数应保留在组合大纲中");
    assert_eq!(function.range.start, 0);
    assert_eq!(
        function.name_range.start,
        source_name_start - function_range.start
    );
}

#[gpui::test]
fn syntax_nodes_project_source_ranges_into_an_excerpt(cx: &mut TestAppContext) {
    let source = singleton("src/main.rs", "// 前置\nfn 数据() {}\n// 后置\n", cx);
    cx.run_until_parked();
    let source_name_start = "// 前置\nfn ".len();
    let function_range = cx.read_entity(&source, |source, _| {
        let snapshot = source.snapshot();
        let node = snapshot
            .syntax
            .node_at(source_name_start, &snapshot.text)
            .expect("Rust 函数名应有语法节点");
        let function = snapshot
            .syntax
            .node_ancestors(node.range.clone(), &snapshot.text)
            .into_iter()
            .find(|node| node.kind == "function_item")
            .expect("Rust 函数应出现在语法祖先链");
        function.range
    });
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![ExcerptRange::new(
                source,
                TextRange::new(
                    ByteOffset::new(function_range.start),
                    ByteOffset::new(function_range.end),
                )
                .unwrap(),
                Vec::new(),
            )],
            cx,
        );
    });

    let node = cx
        .update_entity(&combined, |buffer, cx| {
            buffer
                .snapshot(cx)
                .node_at(ByteOffset::new(source_name_start - function_range.start))
        })
        .expect("excerpt 内函数名应保留在组合语法节点中");
    assert_eq!(node.kind, "identifier");
    assert_eq!(node.range.start, source_name_start - function_range.start);
}

#[gpui::test]
fn excerpts_preserve_order_and_map_output_to_source(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "zero\none\ntwo\n", cx);
    let second = singleton("src/b.rs", "alpha\nbeta\n", cx);
    let combined = cx.new(MultiBuffer::empty);

    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::new(
                    first,
                    TextRange::new(ByteOffset::new(5), ByteOffset::new(9)).unwrap(),
                    vec![TextRange::new(ByteOffset::new(5), ByteOffset::new(8)).unwrap()],
                ),
                ExcerptRange::new(
                    second,
                    TextRange::new(ByteOffset::new(6), ByteOffset::new(11)).unwrap(),
                    vec![TextRange::new(ByteOffset::new(6), ByteOffset::new(10)).unwrap()],
                ),
            ],
            cx,
        );
    });

    let (text, excerpts, first_location, second_location, match_ranges) =
        cx.update_entity(&combined, |buffer, cx| {
            let snapshot = buffer.snapshot(cx);
            let text = String::from_utf8(snapshot.text_bytes()).unwrap();
            let first_offset = ByteOffset::new(text.find("one").unwrap());
            let second_offset = ByteOffset::new(text.find("beta").unwrap());
            (
                text,
                snapshot.excerpts().collect::<Vec<_>>(),
                buffer
                    .location_for_range(
                        TextRange::new(first_offset, ByteOffset::new(first_offset.get() + 3))
                            .unwrap()
                            .into(),
                    )
                    .unwrap(),
                buffer.location_for_offset(second_offset).unwrap(),
                buffer.match_ranges().to_vec(),
            )
        });

    assert_eq!(text, "one\nbeta\n");
    assert_eq!(excerpts.len(), 2);
    assert_eq!(excerpts[0].display_path(), Path::new("src/a.rs"));
    assert_eq!(excerpts[1].display_path(), Path::new("src/b.rs"));
    assert_eq!(excerpts[0].source_start_line(), 2);
    assert_eq!(excerpts[1].source_start_line(), 2);
    assert_eq!(first_location.path, PathBuf::from("src/a.rs"));
    assert_eq!(
        first_location.source_range,
        TextRange::new(ByteOffset::new(5), ByteOffset::new(8)).unwrap()
    );
    assert_eq!(second_location.path, PathBuf::from("src/b.rs"));
    assert_eq!(second_location.source_range.start(), ByteOffset::new(6));
    assert_eq!(match_ranges.len(), 2);
    assert_eq!(match_ranges[0].start().get(), text.find("one").unwrap());
    assert_eq!(match_ranges[1].start().get(), text.find("beta").unwrap());
}

#[gpui::test]
fn per_path_excerpts_keep_path_order_regardless_of_insertion_order(cx: &mut TestAppContext) {
    // 这两个路径的字节序与 PathKey 组件序相反：git ls-files 先给 zcv-workspace，
    // 但 PathKey 序要求 zcv 在前。按字节序插入，验证组合文档仍按 PathKey 排序。
    let first = singleton("zcv/src/workspace.rs", "zero\none\n", cx);
    let second = singleton("zcv-workspace/src/workspace_state.rs", "alpha\nbeta\n", cx);
    let combined = cx.new(MultiBuffer::empty);

    cx.update_entity(&combined, |buffer, cx| {
        // 先插入 PathKey 较大的 zcv-workspace，再插入 zcv：路径序由 MultiBuffer 维护，不由调用方顺序决定。
        buffer.set_excerpts_for_path(
            vec![ExcerptRange::new(
                second,
                TextRange::new(ByteOffset::new(6), ByteOffset::new(10)).unwrap(),
                vec![TextRange::new(ByteOffset::new(6), ByteOffset::new(10)).unwrap()],
            )],
            cx,
        );
        buffer.set_excerpts_for_path(
            vec![ExcerptRange::new(
                first,
                TextRange::new(ByteOffset::new(5), ByteOffset::new(8)).unwrap(),
                vec![TextRange::new(ByteOffset::new(5), ByteOffset::new(8)).unwrap()],
            )],
            cx,
        );
    });

    cx.update_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        assert_eq!(
            String::from_utf8(snapshot.text_bytes()).unwrap(),
            "one\nbeta"
        );
        assert_eq!(snapshot.excerpts().count(), 2);
        assert_eq!(
            snapshot.excerpts().next().unwrap().path(),
            Path::new("zcv/src/workspace.rs")
        );
        assert_eq!(
            snapshot.excerpts().nth(1).unwrap().path(),
            Path::new("zcv-workspace/src/workspace_state.rs")
        );
    });
}
#[gpui::test]
fn composite_anchor_resolves_in_the_same_file_after_excerpt_refresh(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "zero\none\ntwo\nthree\nfour\n", cx);
    let second = singleton("src/b.rs", "alpha\nbeta\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::line_range(first.clone(), 0..2, cx),
                ExcerptRange::line_range(first.clone(), 3..5, cx),
                ExcerptRange::line_range(second, 0..2, cx),
            ],
            cx,
        );
    });
    let anchor = cx.update_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        let excerpt = &snapshot.excerpts().nth(1).unwrap();
        buffer.anchor_at(
            ByteOffset::new(excerpt.output_range().start().get() + 2),
            Affinity::After,
        )
    });

    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(vec![ExcerptRange::line_range(first, 0..2, cx)], cx);
        let offset = buffer
            .resolve_anchor(&anchor)
            .expect("同一文件仍有 excerpt 时应解析到最近位置");
        assert_eq!(
            offset,
            buffer
                .snapshot(cx)
                .excerpts()
                .next()
                .unwrap()
                .output_range()
                .end()
        );
    });
}

#[gpui::test]
fn source_anchor_at_excerpt_boundary_resolves_to_following_excerpt(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "one\ntwo\nthree\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::line_range(source.clone(), 0..1, cx),
                ExcerptRange::line_range(source, 1..3, cx),
            ],
            cx,
        );
        let snapshot = buffer.snapshot(cx);
        let boundary = snapshot.excerpts().nth(1).unwrap().output_range().start();
        let anchor = buffer.anchor_at(boundary, Affinity::After);
        assert_eq!(
            buffer.resolve_anchor(&anchor),
            Some(boundary),
            "共享源边界必须归属后续 excerpt"
        );
    });
}

#[gpui::test]
fn composite_anchor_falls_forward_when_its_file_leaves_the_diff(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "one\n", cx);
    let second = singleton("src/b.rs", "two\n", cx);
    let third = singleton("src/c.rs", "three\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::line_range(first.clone(), 0..1, cx),
                ExcerptRange::line_range(second, 0..1, cx),
                ExcerptRange::line_range(third.clone(), 0..1, cx),
            ],
            cx,
        );
    });
    let anchor = cx.update_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        let excerpt = &snapshot.excerpts().next().unwrap();
        buffer.anchor_at(excerpt.output_range().start(), Affinity::After)
    });

    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(vec![ExcerptRange::line_range(third, 0..1, cx)], cx);
        let offset = buffer
            .resolve_anchor(&anchor)
            .expect("原文件消失后应解析到仍存在的后继文件");
        assert_eq!(
            offset,
            buffer
                .snapshot(cx)
                .excerpts()
                .next()
                .unwrap()
                .output_range()
                .start()
        );
    });
}

/// 源锚点版本晚于目标快照时，禁止落到邻近文件/坐标。
#[gpui::test]
fn invalid_source_anchor_does_not_fall_forward_to_another_file(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "one\n", cx);
    let second = singleton("src/b.rs", "two\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::line_range(first, 0..1, cx),
                ExcerptRange::line_range(second, 0..1, cx),
            ],
            cx,
        );
    });
    let valid = cx.update_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        let excerpt = snapshot.excerpts().next().unwrap();
        buffer.anchor_at(excerpt.output_range().start(), Affinity::After)
    });
    let invalid = match valid {
        MultiBufferAnchor::Excerpt(anchor) => MultiBufferAnchor::Excerpt(ExcerptAnchor {
            path: anchor.path,
            source_id: anchor.source_id,
            text_anchor: Anchor::new(BufferVersion::new(u64::MAX), anchor.text_anchor.offset())
                .with_affinity(anchor.text_anchor.affinity()),
        }),
        other => other,
    };

    cx.read_entity(&combined, |buffer, _| {
        assert!(buffer.resolve_anchor(&valid).is_some());
        assert!(
            buffer.resolve_anchor(&invalid).is_none(),
            "版本失效的源锚点不得落到邻近文件/坐标"
        );
    });
}
/// 外部文本更新与普通编辑共用版本化坐标链，已有锚点自动跟随差异编辑。
#[gpui::test]
fn external_text_update_keeps_existing_anchor_mapped(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "alpha\ncharlie\n", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    let anchor = cx.update_entity(&combined, |buffer, cx| {
        buffer.snapshot(cx);
        // \"charlie\" 行首（源 offset 6）。
        buffer.anchor_at(ByteOffset::new(6), Affinity::After)
    });

    // 外部文本更新：在 \"alpha\" 后插入一整行 \"bravo\"。
    cx.update_entity(&source, |source, cx| {
        source
            .replace_text("alpha\nbravo\ncharlie\n".to_owned(), cx)
            .expect("外部 reload 应成功");
    });
    cx.run_until_parked();

    cx.update_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        assert_eq!(
            snapshot.resolve_anchor(&anchor),
            Some(MultiBufferOffset::new(12)),
            "锚点应随插入行下移"
        );
    });
}

#[gpui::test]
fn empty_files_keep_distinct_composite_lines_and_locations(cx: &mut TestAppContext) {
    let first = singleton("deleted/a.rs", "", cx);
    let second = singleton("deleted/b.rs", "", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::line_range(first, 0..1, cx),
                ExcerptRange::line_range(second, 0..1, cx),
            ],
            cx,
        );
    });

    cx.update_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        // 尾换行不变式：非末尾空片段补一个换行占边界行，末尾片段保留原样（文档自身的末尾空行仍为其保留组合行）。
        assert_eq!(String::from_utf8(snapshot.text_bytes()).unwrap(), "\n");
        assert_eq!(snapshot.excerpts().next().unwrap().output_start_line(), 0);
        assert_eq!(snapshot.excerpts().nth(1).unwrap().output_start_line(), 1);
        assert_eq!(
            buffer.location_for_offset(ByteOffset::new(1)).unwrap().path,
            PathBuf::from("deleted/b.rs")
        );
    });
}

#[gpui::test]
fn source_reparse_does_not_reload_composite_text(cx: &mut TestAppContext) {
    let source = singleton("src/main.rs", "fn main() {\n    println!(\"ok\");\n}\n", cx);
    let source_len = cx.read_entity(&source, |source, _| source.text_snapshot().len_bytes());
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |combined, cx| {
        combined.set_excerpts(
            vec![ExcerptRange::new(
                source,
                TextRange::new(ByteOffset::ZERO, source_len).unwrap(),
                Vec::new(),
            )],
            cx,
        );
    });
    let before = cx.update_entity(&combined, |combined, cx| combined.snapshot(cx).version());

    cx.run_until_parked();

    let after = cx.update_entity(&combined, |combined, cx| combined.snapshot(cx).version());
    assert_eq!(after, before, "语法解析完成不应重载组合投影文本");
}

/// 不变量：组合编辑后 excerpt 源坐标只映射一次。
/// 组合编辑在 `MultiBuffer::edit` 内同步映射并重建订阅，`source_changed` 的消费为空，两条路径不会重复映射——这是 hunk 文本跟踪区间更新机制的前提。
#[gpui::test]
fn composite_edit_maps_excerpt_source_ranges_exactly_once(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "zero\none\ntwo\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![ExcerptRange::new(
                source,
                TextRange::new(ByteOffset::new(5), ByteOffset::new(9)).unwrap(),
                Vec::new(),
            )],
            cx,
        );
    });
    cx.update_entity(&combined, |buffer, cx| {
        buffer
            .edit(
                vec![Edit::replace(
                    TextRange::new(ByteOffset::ZERO, ByteOffset::new(1)).unwrap(),
                    "OO",
                )],
                TransactionMetadata::default(),
                cx,
            )
            .unwrap();
    });
    // 同步路径（edit 内手动映射）后的结果。
    cx.update_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        assert_eq!(
            snapshot.excerpts().next().unwrap().source_range(),
            TextRange::new(ByteOffset::new(5), ByteOffset::new(10)).unwrap()
        );
    });
    cx.run_until_parked();
    cx.update_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        let excerpt = &snapshot.excerpts().next().unwrap();
        // 只映射一次：源 'o'（5..6）替换为 "OO" → 源范围 5..10；二次映射会变成 5..11。
        assert_eq!(
            excerpt.source_range(),
            TextRange::new(ByteOffset::new(5), ByteOffset::new(10)).unwrap(),
            "组合编辑后 excerpt 源坐标应只映射一次"
        );
    });
}

/// 外部源编辑后，显示 hunk 必须从当前 base/working 快照重算，而不是平移旧 Git hunk。
#[gpui::test]
fn diff_hunks_follow_external_source_edits(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "zero\none\ntwo\nthree\n", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![TestDiff {
                operations: None,
                working: source.clone(),
                base_text: Some(Arc::from("zero\nold\ntwo\nthree\n")),
                index_text: None,
                path: PathBuf::from("src/a.rs"),
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
            }]),
            cx,
        );
    });
    cx.run_until_parked();
    cx.update_entity(&combined, |buffer, cx| buffer.toggle_diff_hunk_at(0, cx));
    assert!(
        cx.read_entity(&combined, |buffer, _cx| {
            buffer.diff_hunk_expanded().iter().all(|&expanded| expanded)
        }),
        "展开后应记录展开状态"
    );

    // 编辑工作区源：文件头部插入一行（行号整体 +1），显示坐标应随编辑推进。
    cx.update_entity(&source, |source, cx| {
        source
            .edit(
                vec![Edit::insert(ByteOffset::new(0), "pre\n").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .unwrap();
    });
    cx.run_until_parked();

    cx.update_entity(&combined, |buffer, cx| {
        buffer.snapshot(cx);
    });
    cx.run_until_parked();
    cx.update_entity(&combined, |buffer, cx| {
        buffer.snapshot(cx);
    });
    let hunks_after_edit = cx.read_entity(&combined, |buffer, _cx| buffer.diff_hunks().to_vec());
    assert_eq!(
        hunks_after_edit.len(),
        2,
        "新增的文件头与原有修改都必须反映在当前快照"
    );
    assert_eq!(hunks_after_edit[0].kind, DiffHunkKind::Added);
    assert_eq!(hunks_after_edit[1].kind, DiffHunkKind::Modified);
    assert!(
        !hunks_after_edit[0].range.is_empty(),
        "映射后的 hunk 必须仍可显示"
    );

    // 重新注入：hunk 新侧坐标移到行 2（文件内容已变），展开状态按文本跟踪区间迁移到新 hunk。
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![TestDiff {
                operations: None,
                working: source.clone(),
                base_text: Some(Arc::from("pre\nzero\nold\ntwo\nthree\n")),
                index_text: None,
                path: PathBuf::from("src/a.rs"),
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
            }]),
            cx,
        );
    });
    cx.run_until_parked();
    cx.update_entity(&combined, |buffer, cx| {
        buffer.snapshot(cx);
    });
    cx.update_entity(&combined, |buffer, cx| {
        buffer.snapshot(cx);
    });
    let (hunks, expanded) = cx.read_entity(&combined, |buffer, _cx| {
        (buffer.diff_hunks().to_vec(), buffer.diff_hunk_expanded())
    });
    assert_eq!(hunks.len(), 1, "重新注入后应显示新坐标 hunk");
    // 新 hunk 采用默认折叠状态，并由新 Git 快照提供坐标。
    assert_eq!(hunks[0].kind, DiffHunkKind::Modified);
    assert!(
        expanded.iter().all(|&expanded| expanded),
        "同一 Git hunk 刷新后应保留用户显式展开状态"
    );
}

/// 文本对改变后，合并出的新 hunk 不继承旧 hunk 的展开状态。
#[gpui::test]
fn diff_expansion_survives_hunk_refresh_and_merge(cx: &mut TestAppContext) {
    let source = singleton("tracked.txt", "line0\n改过\nline2\nline3\n", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    let base_text: Arc<str> = Arc::from("line0\nline1\nline2\nline3\n");

    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![TestDiff {
                operations: None,
                working: source.clone(),
                base_text: Some(base_text.clone()),
                index_text: None,
                path: PathBuf::from("tracked.txt"),
                display_path: PathBuf::from("tracked.txt"),
                context_lines: None,
            }]),
            cx,
        );
    });
    cx.run_until_parked();
    cx.update_entity(&combined, |buffer, cx| buffer.toggle_diff_hunk_at(0, cx));

    cx.update_entity(&source, |source, cx| {
        source
            .edit(
                [Edit::insert(ByteOffset::new(13), "改过2\n").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .unwrap();
    });
    cx.run_until_parked();

    cx.update_entity(&combined, |buffer, cx| {
        buffer.snapshot(cx);
    });
    cx.run_until_parked();
    cx.update_entity(&combined, |buffer, cx| {
        buffer.snapshot(cx);
    });
    // 模拟 GitStore 刷新期间的加载态，再注入合并后的新结果。
    cx.update_entity(&combined, |buffer, cx| {
        assert!(!buffer.inject_diffs(None, cx));
        buffer.inject_diffs(
            Some(vec![TestDiff {
                operations: None,
                working: source.clone(),
                base_text: Some(base_text),
                index_text: None,
                path: PathBuf::from("tracked.txt"),
                display_path: PathBuf::from("tracked.txt"),
                context_lines: None,
            }]),
            cx,
        );
    });
    cx.run_until_parked();

    cx.update_entity(&combined, |buffer, cx| {
        buffer.snapshot(cx);
    });
    cx.run_until_parked();
    cx.update_entity(&combined, |buffer, cx| {
        buffer.snapshot(cx);
    });
    let (hunks, expanded) = cx.read_entity(&combined, |buffer, _cx| {
        (buffer.diff_hunks().to_vec(), buffer.diff_hunk_expanded())
    });
    assert_eq!(hunks.len(), 1, "刷新后相邻改动应合并为一个 hunk");
    assert_eq!(hunks[0].old_range, 1..2, "合并后的 hunk 旧侧应为实际替换行");
    assert!(
        expanded.iter().all(|&expanded| expanded),
        "Git 刷新的同一 hunk 应保留用户显式展开状态"
    );
}

/// 回归：working 版本推进、旧 DiffState 尚未重算时重新注入等价 hunk，展开状态必须保留。
///
/// 旧实现按 working Anchor 的版本与偏移比较 hunk 身份，旧 diff 锚点停留在编辑前版本时
/// 迁移会丢失显式展开。身份改为在当前工作区快照上重新解析后，同一 hunk 不因版本推进而失效。
#[gpui::test]
fn diff_expansion_survives_working_version_advance_with_stale_old_diff(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "zero\nworking\ntwo\n", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![test_diff(
                source.clone(),
                "src/a.rs",
                "zero\nbase\ntwo\n",
            )]),
            cx,
        );
    });
    cx.run_until_parked();
    cx.update_entity(&combined, |buffer, cx| buffer.toggle_diff_hunk_at(0, cx));
    assert!(
        cx.read_entity(&combined, |buffer, _cx| {
            buffer.diff_hunk_expanded().iter().all(|&expanded| expanded)
        }),
        "展开后应记录展开状态"
    );

    // 编辑工作区源后立即重新注入，不等待旧 DiffState 重算：
    // 旧 diff 的 hunk Anchor 仍停留在编辑前版本，新 diff 已从推进后的 working 计算。
    cx.update_entity(&source, |source, cx| {
        source
            .edit(
                vec![Edit::insert(ByteOffset::ZERO, "pre\n").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .unwrap();
    });
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![test_diff(
                source.clone(),
                "src/a.rs",
                "pre\nzero\nbase\ntwo\n",
            )]),
            cx,
        );
    });
    cx.run_until_parked();
    cx.update_entity(&combined, |buffer, cx| {
        buffer.snapshot(cx);
    });
    cx.run_until_parked();

    let (hunks, expanded) = cx.read_entity(&combined, |buffer, _cx| {
        (buffer.diff_hunks().to_vec(), buffer.diff_hunk_expanded())
    });
    assert_eq!(hunks.len(), 1, "重新注入后应显示推进坐标后的等价 hunk");
    assert!(
        expanded.iter().all(|&expanded| expanded),
        "working 版本推进但 hunk 等价时必须保留显式展开状态"
    );
}

#[gpui::test]
fn undo_keeps_rust_highlighting_in_diff_projection(cx: &mut TestAppContext) {
    let source = singleton(
        "src/window_controls.rs",
        "fn main() { let value = 1; }\n",
        cx,
    );
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![TestDiff {
                operations: None,
                working: source.clone(),
                base_text: Some(Arc::from("fn main() { let value = 0; }\n")),
                index_text: None,
                path: PathBuf::from("src/window_controls.rs"),
                display_path: PathBuf::from("src/window_controls.rs"),
                context_lines: Some(2),
            }]),
            cx,
        );
    });
    cx.run_until_parked();
    cx.update_entity(&combined, |buffer, cx| {
        buffer.start_transaction(cx).expect("应开始 hunk 编辑事务");
        buffer
            .edit(
                vec![Edit::insert(ByteOffset::new(3), "async ").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("hunk 编辑应成功");
        buffer.end_transaction(cx);
    });
    cx.update_entity(&combined, |buffer, cx| {
        buffer.undo(cx).expect("撤销应成功");
    });
    cx.run_until_parked();

    cx.update_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        assert!(
            !snapshot
                .highlights(0..snapshot.len_bytes().get())
                .is_empty(),
            "撤销后 Git hunk 投影仍应保留 Rust 高亮"
        );
    });
}

#[gpui::test]
fn save_after_diff_hunk_edit_keeps_rust_highlighting(cx: &mut TestAppContext) {
    let source = singleton(
        "src/window_controls.rs",
        "fn main() { let value = 1; }\n",
        cx,
    );
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![TestDiff {
                operations: None,
                working: source.clone(),
                base_text: Some(Arc::from("fn main() { let value = 0; }\n")),
                index_text: None,
                path: PathBuf::from("src/window_controls.rs"),
                display_path: PathBuf::from("src/window_controls.rs"),
                context_lines: Some(2),
            }]),
            cx,
        );
    });
    cx.run_until_parked();
    cx.update_entity(&combined, |buffer, cx| {
        buffer
            .edit(
                vec![Edit::insert(ByteOffset::new(3), "async ").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("hunk 编辑应成功");
    });
    cx.update_entity(&source, |source, cx| {
        source.mark_saved(cx);
    });
    cx.run_until_parked();

    cx.update_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        assert!(
            !snapshot
                .highlights(0..snapshot.len_bytes().get())
                .is_empty()
        );
    });
}

/// 回归：前一个整文件投影以换行结尾时，其末尾空逻辑行不会物化为组合文档行；
/// 后续文件的 hunk 坐标必须来自实际 excerpt 映射，不能按源行数累计后发生偏移。
#[gpui::test]
fn diff_hunk_coordinates_follow_materialized_excerpts_across_files(cx: &mut TestAppContext) {
    let created = singleton("created.txt", "created\n", cx);
    let modified = singleton("modified.txt", "before\nnew\nafter\n", cx);
    let combined = cx.new(MultiBuffer::empty);

    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_diff_hunks_expanded_by_default(true, cx);
        buffer.inject_diffs(
            Some(vec![
                TestDiff {
                    operations: None,
                    working: created,
                    base_text: None,
                    index_text: None,
                    path: PathBuf::from("created.txt"),
                    display_path: PathBuf::from("created.txt"),
                    context_lines: Some(2),
                },
                TestDiff {
                    operations: None,
                    working: modified,
                    base_text: Some(Arc::from("before\nold\nafter\n")),
                    index_text: None,
                    path: PathBuf::from("modified.txt"),
                    display_path: PathBuf::from("modified.txt"),
                    context_lines: Some(2),
                },
            ]),
            cx,
        );
    });
    cx.run_until_parked();

    cx.update_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        assert_eq!(
            String::from_utf8(snapshot.text_bytes()).expect("投影应为 UTF-8"),
            "created\nbefore\nold\nnew\nafter\n"
        );
        assert_eq!(
            buffer.diff_hunk_old_ranges(),
            &[None, Some(2..3)],
            "旧侧范围应落在实际物化的 old 行"
        );
        assert_eq!(
            buffer.diff_hunks(),
            &[
                DisplayHunk {
                    range: 0..1,
                    old_range: 0..0,
                    kind: DiffHunkKind::Added,
                    staging: DiffHunkStaging::NoStaging,
                },
                DisplayHunk {
                    range: 3..4,
                    old_range: 1..2,
                    kind: DiffHunkKind::Modified,
                    staging: DiffHunkStaging::NoStaging,
                },
            ],
            "新侧范围应落在实际物化的 new 行，不能受前一文件末尾空逻辑行影响"
        );
        assert_eq!(
            buffer.diff_hunk_expanded(),
            &[true, true],
            "展开状态必须与物化后的显示 hunk 同序，不能遗漏整文件新增块"
        );
        assert!(
            buffer
                .buffer_diff_hunk_at(0, cx)
                .expect("整文件新增块应有显示来源")
                .range
                .is_none(),
            "整文件新增块不应伪造源 hunk"
        );
        assert!(
            buffer
                .buffer_diff_hunk_at(1, cx)
                .expect("修改 hunk 应有显示来源")
                .range
                .is_some(),
            "修改 hunk 应有可重解析的源范围"
        );
    });
}

/// base 版本变化时，新的文本对重新定义 hunk 身份；旧 base 的展开状态不得迁移。
#[gpui::test]
fn diff_expansion_survives_base_change_when_working_text_is_unchanged(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "zero\none\ntwo\nthree\n", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![TestDiff {
                operations: None,
                working: source.clone(),
                base_text: Some(Arc::from("zero\nold\ntwo\nthree\n")),
                index_text: None,
                path: PathBuf::from("src/a.rs"),
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
            }]),
            cx,
        );
        buffer.toggle_diff_hunk_at(0, cx);
    });
    // base 完全变化（模拟提交后新 HEAD）：旧侧文本改变后，当前文本对派生出另一个 hunk。
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![TestDiff {
                operations: None,
                working: source.clone(),
                base_text: Some(Arc::from("zero\none\nold\nthree\n")),
                index_text: None,
                path: PathBuf::from("src/a.rs"),
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
            }]),
            cx,
        );
    });
    assert!(
        cx.read_entity(&combined, |buffer, _cx| {
            buffer
                .diff_hunk_expanded()
                .iter()
                .all(|&expanded| !expanded)
        }),
        "base 变化后的新 hunk 不得继承旧 base 的展开状态"
    );
}

/// 回归：外部整体替换后，显示 hunk 必须从新工作区快照重算，旧 Git 操作 hunk 不得复用。
#[gpui::test]
fn external_full_replacement_invalidates_stale_diff_hunks(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "first\nchanged\nthird\n", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![TestDiff {
                operations: None,
                working: source.clone(),
                base_text: Some(Arc::from("first\noriginal\nthird\n")),
                index_text: None,
                path: PathBuf::from("src/a.rs"),
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
            }]),
            cx,
        );
    });

    cx.update_entity(&source, |source, cx| {
        source
            .replace_text("replacement\n".to_owned(), cx)
            .expect("外部整体替换应成功");
    });
    cx.run_until_parked();

    cx.update_entity(&combined, |buffer, cx| {
        assert_eq!(
            String::from_utf8(buffer.snapshot(cx).text_bytes()).expect("投影应为 UTF-8"),
            "replacement\n"
        );
        assert_eq!(buffer.diff_hunks().len(), 1, "新快照应生成新的显示 hunk");
        assert!(buffer.buffer_diff_hunk_at(0, cx).is_some());
        assert!(
            buffer
                .buffer_diff_hunk_at(0, cx)
                .is_some_and(|source| source.range.is_some()),
            "新显示 hunk 必须暴露当前工作区锚点范围"
        );
    });
}

/// 回归：工作区整份被删除且没有内容节点时，纯删除 hunk 仍必须挂到输出变换树并可见。
#[gpui::test]
fn fully_deleted_file_keeps_boundary_hunk(cx: &mut TestAppContext) {
    let source = singleton("src/gone.rs", "", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![test_diff(source.clone(), "src/gone.rs", "removed\n")]),
            cx,
        );
    });
    cx.run_until_parked();

    cx.update_entity(&combined, |buffer, cx| {
        assert_eq!(
            buffer.diff_hunks().len(),
            1,
            "整份删除必须保留一个显示 hunk"
        );
        assert_eq!(buffer.diff_hunks()[0].kind, DiffHunkKind::Deleted);
        assert_eq!(buffer.diff_hunks()[0].range, 0..0);
        assert!(
            buffer.diff_hunk_old_ranges()[0].is_none(),
            "整文件模式折叠态不物化旧侧"
        );
        assert!(
            buffer.buffer_diff_hunk_at(0, cx).is_some(),
            "纯删除 hunk 必须能定位到源"
        );
    });
}

/// BufferDiff 不自行订阅源：working 文本变化由宿主（组合文档投影）驱动重算。
#[gpui::test]
fn host_drives_buffer_diff_recompute_from_source_edits(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "a\nb\n", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![TestDiff {
                working: source.clone(),
                base_text: Some(Arc::from("a\n")),
                index_text: None,
                path: PathBuf::from("src/a.rs"),
                operations: None,
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
            }]),
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(
        cx.read_entity(&combined, |buffer, _cx| buffer.diff_hunks().len()),
        1,
        "初始应有一个新增 hunk"
    );

    // 直接编辑 working buffer，不经过任何显示层调用；宿主订阅到变化后驱动重算。
    cx.update_entity(&source, |source, cx| {
        source
            .edit(
                vec![Edit::replace(
                    TextRange::new(ByteOffset::new(2), ByteOffset::new(4)).unwrap(),
                    "",
                )],
                TransactionMetadata::default(),
                cx,
            )
            .unwrap();
    });
    cx.run_until_parked();

    assert_eq!(
        cx.read_entity(&combined, |buffer, _cx| buffer.diff_hunks().len()),
        1,
        "未保存期间应保留原有组合 hunk"
    );

    // 保存后由宿主重新注入 diff，才提交新的 hunk 投影。
    cx.update_entity(&source, |source, cx| {
        source.mark_saved(cx);
    });
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![TestDiff {
                working: source.clone(),
                base_text: Some(Arc::from("a\n")),
                index_text: None,
                path: PathBuf::from("src/a.rs"),
                operations: None,
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
            }]),
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(
        cx.read_entity(&combined, |buffer, _cx| buffer.diff_hunks().len()),
        0,
        "保存后重新注入才应移除已无差异的 hunk"
    );
}

/// dirty working source 的 hunk 变化不能提前删除组合文档中的既有 excerpt。
#[gpui::test]
fn dirty_source_keeps_existing_diff_projection_until_saved(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "a\nb\n", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![TestDiff {
                working: source.clone(),
                base_text: Some(Arc::from("a\n")),
                index_text: None,
                path: PathBuf::from("src/a.rs"),
                operations: None,
                display_path: PathBuf::from("src/a.rs"),
                context_lines: Some(2),
            }]),
            cx,
        );
    });
    cx.run_until_parked();
    let initial_excerpt_count = cx.update_entity(&combined, |buffer, cx| {
        buffer.snapshot(cx).excerpts().count()
    });
    assert!(initial_excerpt_count > 0, "初始 hunk 应生成 excerpt");

    cx.update_entity(&source, |source, cx| {
        source
            .edit(
                vec![Edit::replace(
                    TextRange::new(ByteOffset::new(2), ByteOffset::new(4)).unwrap(),
                    "",
                )],
                TransactionMetadata::default(),
                cx,
            )
            .unwrap();
    });
    cx.run_until_parked();

    cx.update_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        assert!(
            buffer.is_diff_file_dirty(Path::new("src/a.rs"), cx),
            "组合文档应能读取文件 working source 的 dirty 状态"
        );
        assert_eq!(
            snapshot.excerpts().count(),
            initial_excerpt_count,
            "未保存期间不能因 hunk 为空而移除既有 excerpt"
        );
        assert!(
            snapshot
                .excerpts()
                .all(|excerpt| excerpt.path() == Path::new("src/a.rs"))
        );
        assert_eq!(String::from_utf8(snapshot.text_bytes()).unwrap(), "a\n");
    });
}

/// 回归：编辑内容但 hunk 几何不变时，diff 高亮不能因显示坐标门控而整体消失。
#[gpui::test]
fn diff_hunks_survive_geometry_preserving_edits(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "a\nb\nc\n", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![TestDiff {
                working: source.clone(),
                base_text: Some(Arc::from("a\nB\nc\n")),
                index_text: None,
                path: PathBuf::from("src/a.rs"),
                operations: None,
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
            }]),
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(
        cx.read_entity(&combined, |buffer, _cx| buffer.diff_hunks().len()),
        1
    );

    // 同一行内改内容：行数不变、hunk 行范围与词级范围都不变，diff 不会发出 DiffChanged。
    cx.update_entity(&source, |source, cx| {
        source
            .edit(
                vec![Edit::replace(
                    TextRange::new(ByteOffset::new(2), ByteOffset::new(3)).unwrap(),
                    "z",
                )],
                TransactionMetadata::default(),
                cx,
            )
            .unwrap();
    });
    cx.run_until_parked();

    assert_eq!(
        cx.read_entity(&combined, |buffer, _cx| buffer.diff_hunks().len()),
        1,
        "hunk 几何不变的编辑后 diff 高亮必须保留"
    );
    assert_eq!(
        cx.read_entity(&combined, |buffer, _cx| buffer.diff_hunk_expanded()),
        vec![false],
        "展开状态同样受该门控影响，必须保持可用"
    );
}

/// 回归：某个文件尚未算完时，不能清空其他文件已经就绪的 diff 高亮。
#[gpui::test]
fn pending_new_file_does_not_hide_ready_diff_hunks(cx: &mut TestAppContext) {
    let source_a = singleton("src/a.rs", "a\nb\nc\n", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(source_a.clone(), cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![TestDiff {
                working: source_a.clone(),
                base_text: Some(Arc::from("a\nB\nc\n")),
                index_text: None,
                path: PathBuf::from("src/a.rs"),
                operations: None,
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
            }]),
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(
        cx.read_entity(&combined, |buffer, _| buffer.diff_hunks().len()),
        1
    );

    // 加入第二个文件；不 park，使它的 diff 仍在计算中（add_diff 因而提前返回）。
    let source_c = singleton("src/c.rs", "x\ny\nz\n", cx);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![
                TestDiff {
                    working: source_a.clone(),
                    base_text: Some(Arc::from("a\nB\nc\n")),
                    index_text: None,
                    path: PathBuf::from("src/a.rs"),
                    operations: None,
                    display_path: PathBuf::from("src/a.rs"),
                    context_lines: None,
                },
                TestDiff {
                    working: source_c.clone(),
                    base_text: Some(Arc::from("x\nY\nz\n")),
                    index_text: None,
                    path: PathBuf::from("src/c.rs"),
                    operations: None,
                    display_path: PathBuf::from("src/c.rs"),
                    context_lines: None,
                },
            ]),
            cx,
        );
    });
    assert!(
        !cx.read_entity(&combined, |buffer, _| buffer.diff_hunks().is_empty()),
        "新文件未就绪不能清空已有文件的 diff 高亮"
    );
}

/// 回归：初始后台结果对应旧版本被拒后，diff 必须补算到当前版本，不能永久停在未计算状态。
#[gpui::test]
fn diff_recovers_when_initial_result_is_stale(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "a\nb\nc\n", cx);
    let diff = test_diff_entity(source.clone(), "src/a.rs", Some("a\nB\nc\n"), None, cx);
    // 不 park，立即编辑源：初始后台结果会对应旧版本并被版本门控拒绝。
    cx.update_entity(&source, |source, cx| {
        source
            .edit(
                vec![Edit::replace(
                    TextRange::new(ByteOffset::new(2), ByteOffset::new(3)).unwrap(),
                    "z",
                )],
                TransactionMetadata::default(),
                cx,
            )
            .unwrap();
    });
    cx.run_until_parked();
    assert!(
        cx.update_entity(&diff, |diff, cx| diff.is_current_version_calculated(cx)),
        "过期结果被拒后必须补算到当前版本"
    );
}

/// 回归：连续多次编辑后，diff 高亮仍必须可用（不依赖异步 DiffChanged 恰好重建）。
#[gpui::test]
fn diff_hunks_survive_rapid_edits(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "a\nb\nc\n", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![TestDiff {
                working: source.clone(),
                base_text: Some(Arc::from("a\nB\nc\n")),
                index_text: None,
                path: PathBuf::from("src/a.rs"),
                operations: None,
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
            }]),
            cx,
        );
    });
    cx.run_until_parked();
    // 快速连续编辑同一行（每次都改变 working 版本，但 hunk 几何不变）。
    for i in 0..6u8 {
        let replacement = char::from(b'a' + i).to_string();
        cx.update_entity(&source, |source, cx| {
            source
                .edit(
                    vec![Edit::replace(
                        TextRange::new(ByteOffset::new(2), ByteOffset::new(3)).unwrap(),
                        replacement.clone(),
                    )],
                    TransactionMetadata::default(),
                    cx,
                )
                .unwrap();
        });
    }
    cx.run_until_parked();
    assert!(
        !cx.read_entity(&combined, |buffer, _cx| buffer.diff_hunks().is_empty()),
        "连续编辑后 diff 高亮不能消失"
    );
}

/// 回归（M-D）：diff 同步帧内到达的等长源替换必须发布精确输出增量，
/// 不能因为前后投影摘要相同而退化为空范围。
#[gpui::test]
fn diff_sync_frame_publishes_equal_length_source_replacement(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "a\nb\nc\n", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![TestDiff {
                working: source.clone(),
                base_text: Some(Arc::from("a\nB\nc\n")),
                index_text: None,
                path: PathBuf::from("src/a.rs"),
                operations: None,
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
            }]),
            cx,
        );
    });
    cx.run_until_parked();
    let subscription =
        cx.update_entity(&combined, |buffer, cx| buffer.subscribe_and_snapshot(cx).0);

    // 第一次编辑触发后台 diff 计算并立即发布；消费掉，使第二次编辑落在同步帧内。
    cx.update_entity(&source, |source, cx| {
        source
            .edit(
                [Edit::replace(
                    TextRange::new(ByteOffset::new(2), ByteOffset::new(3)).unwrap(),
                    "x",
                )],
                TransactionMetadata::default(),
                cx,
            )
            .unwrap();
    });
    cx.update_entity(&combined, |buffer, cx| {
        buffer.snapshot(cx);
    });
    let first = subscription.consume();
    assert_eq!(
        first.patch().edits().len(),
        1,
        "帧外源编辑必须立即发布精确输出编辑"
    );

    // 第二次等长替换：在途 diff 结果先落地，帧内同步该编辑。
    cx.update_entity(&source, |source, cx| {
        source
            .edit(
                [Edit::replace(
                    TextRange::new(ByteOffset::new(2), ByteOffset::new(3)).unwrap(),
                    "y",
                )],
                TransactionMetadata::default(),
                cx,
            )
            .unwrap();
    });
    cx.run_until_parked();

    let second = subscription.consume();
    assert_eq!(
        second.patch().edits().len(),
        1,
        "diff 同步帧必须发布精确的等长源替换增量，而不是空范围"
    );
    let edit = &second.patch().edits()[0];
    assert_eq!(edit.old_range().len(), 1, "等长替换的旧输出范围长度为 1");
    assert_eq!(edit.new_range().len(), 1, "等长替换的新输出范围长度为 1");
}

/// 新增块没有旧侧内容，不参与展开/折叠：整行背景只由展开策略默认值决定。
/// 普通文档默认折叠（只保留 gutter 竖条），差异审阅视图默认展开展示背景色。
#[gpui::test]
fn added_hunk_background_follows_view_expansion_policy(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "a\nb\nc\n", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![TestDiff {
                operations: None,
                working: source.clone(),
                // base 缺少 b：派生一个纯新增 hunk。
                base_text: Some(Arc::from("a\nc\n")),
                index_text: None,
                path: PathBuf::from("src/a.rs"),
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
            }]),
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(
        cx.read_entity(&combined, |buffer, _cx| buffer.diff_hunks().len()),
        1
    );
    // 普通文档默认折叠：不整行着色。
    assert_eq!(
        cx.read_entity(&combined, |buffer, _cx| buffer.diff_hunk_expanded()),
        vec![false]
    );
    // 差异审阅视图默认展开：显示整行背景。
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_diff_hunks_expanded_by_default(true, cx)
    });
    assert_eq!(
        cx.read_entity(&combined, |buffer, _cx| buffer.diff_hunk_expanded()),
        vec![true]
    );
}

/// 展开的修改块词级范围必须落在组合文档坐标：旧侧指针指向 base 文本，新侧指针指向 working 文本。
#[gpui::test]
fn expanded_modified_hunk_exposes_word_diffs_in_composite_coordinates(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "let x = 2;\n", cx);
    let combined = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.inject_diffs(
            Some(vec![TestDiff {
                operations: None,
                working: source.clone(),
                base_text: Some(Arc::from("let x = 1;\n")),
                index_text: None,
                path: PathBuf::from("src/a.rs"),
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
            }]),
            cx,
        );
        buffer.set_diff_hunks_expanded_by_default(true, cx);
    });
    cx.run_until_parked();

    let (text, word_diffs) = cx.update_entity(&combined, |buffer, cx| {
        let text =
            String::from_utf8(buffer.snapshot(cx).text_bytes()).expect("组合文本必须是 UTF-8");
        (text, buffer.diff_hunk_word_diffs().to_vec())
    });
    assert_eq!(word_diffs.len(), 1, "应有一个显示 hunk");
    let diffs = &word_diffs[0];
    assert_eq!(diffs.len(), 2, "修改块应同时给出旧侧与新侧词级范围");
    assert_eq!(diffs[0].0, DiffHunkKind::Deleted);
    assert_eq!(
        &text[diffs[0].1.clone()],
        "1",
        "旧侧词级范围应指向 base 中的变化词"
    );
    assert_eq!(diffs[1].0, DiffHunkKind::Added);
    assert_eq!(
        &text[diffs[1].1.clone()],
        "2",
        "新侧词级范围应指向 working 中的变化词"
    );
}

#[gpui::test]
fn composite_edits_are_applied_to_the_underlying_buffer(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "zero\none\ntwo\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![ExcerptRange::new(
                source.clone(),
                TextRange::new(ByteOffset::new(5), ByteOffset::new(9)).unwrap(),
                Vec::new(),
            )],
            cx,
        );
        buffer.start_transaction(cx).unwrap();
        buffer
            .edit(
                vec![Edit::replace(
                    TextRange::new(ByteOffset::ZERO, ByteOffset::new(3)).unwrap(),
                    "ONE",
                )],
                TransactionMetadata::default(),
                cx,
            )
            .unwrap();
        assert!(buffer.end_transaction(cx).is_some());
    });

    let source_contents = cx.read_entity(&source, |source, _| {
        let snapshot = source.text_snapshot();
        snapshot
            .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
            .unwrap()
            .as_str()
            .to_owned()
    });
    assert_eq!(source_contents, "zero\nONE\ntwo\n");
    let projection = cx.update_entity(&combined, |buffer, cx| {
        String::from_utf8(buffer.snapshot(cx).text_bytes()).unwrap()
    });
    assert_eq!(projection, "ONE\n");
    cx.update_entity(&combined, |buffer, cx| {
        let files = buffer.file_buffers(cx);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].1, PathBuf::from("src/a.rs"));
        assert!(
            buffer.is_dirty(cx),
            "源 Buffer 变脏时组合文档也必须是 dirty"
        );
    });
}

#[gpui::test]
fn composite_file_buffers_are_deduplicated_across_excerpts(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "zero\none\ntwo\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::new(
                    source.clone(),
                    TextRange::new(ByteOffset::ZERO, ByteOffset::new(5)).unwrap(),
                    Vec::new(),
                ),
                ExcerptRange::new(
                    source,
                    TextRange::new(ByteOffset::new(5), ByteOffset::new(9)).unwrap(),
                    Vec::new(),
                ),
            ],
            cx,
        );
    });

    cx.update_entity(&combined, |buffer, cx| {
        assert_eq!(buffer.file_buffers(cx).len(), 1);
    });
}

#[gpui::test]
fn composite_tracks_edits_made_through_another_editor(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "zero\none\ntwo\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![ExcerptRange::new(
                source.clone(),
                TextRange::new(ByteOffset::new(5), ByteOffset::new(9)).unwrap(),
                Vec::new(),
            )],
            cx,
        )
    });

    cx.update_entity(&source, |source, cx| {
        source
            .edit(
                [Edit::replace(
                    TextRange::new(ByteOffset::new(5), ByteOffset::new(8)).unwrap(),
                    "ONE",
                )],
                TransactionMetadata::default(),
                cx,
            )
            .unwrap();
    });
    cx.run_until_parked();

    let projection = cx.update_entity(&combined, |buffer, cx| {
        String::from_utf8(buffer.snapshot(cx).text_bytes()).unwrap()
    });
    assert_eq!(projection, "ONE\n");
}

#[gpui::test]
fn composite_splits_cross_excerpt_edits_across_source_buffers(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "one\n", cx);
    let second = singleton("src/b.rs", "two\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::new(
                    first.clone(),
                    TextRange::new(ByteOffset::ZERO, ByteOffset::new(4)).unwrap(),
                    Vec::new(),
                ),
                ExcerptRange::new(
                    second.clone(),
                    TextRange::new(ByteOffset::ZERO, ByteOffset::new(4)).unwrap(),
                    Vec::new(),
                ),
            ],
            cx,
        );
        buffer.start_transaction(cx).unwrap();
        buffer
            .edit(
                vec![Edit::replace(
                    TextRange::new(ByteOffset::new(1), ByteOffset::new(6)).unwrap(),
                    "X",
                )],
                TransactionMetadata::default(),
                cx,
            )
            .unwrap();
        assert!(buffer.end_transaction(cx).is_some());
    });

    let read = |buffer: &gpui::Entity<LanguageBuffer>, cx: &TestAppContext| {
        cx.read_entity(buffer, |buffer, _| {
            let snapshot = buffer.text_snapshot();
            snapshot
                .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
                .unwrap()
                .as_str()
                .to_owned()
        })
    };
    assert_eq!(read(&first, cx), "oX");
    assert_eq!(read(&second, cx), "o\n");
}

/// 回归：多源编辑中任一源在映射建立后推进版本时，整体失败且不得部分提交其它源。
///
/// 映射的源范围只在建立映射的那份源快照上有效；提交前必须一次性校验全部源。
#[gpui::test]
fn multi_source_edit_does_not_partially_commit_when_a_source_advanced(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "one\n", cx);
    let second = singleton("src/b.rs", "two\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::new(
                    first.clone(),
                    TextRange::new(ByteOffset::ZERO, ByteOffset::new(4)).unwrap(),
                    Vec::new(),
                ),
                ExcerptRange::new(
                    second.clone(),
                    TextRange::new(ByteOffset::ZERO, ByteOffset::new(4)).unwrap(),
                    Vec::new(),
                ),
            ],
            cx,
        );
    });

    // 第二源在映射建立后推进版本，旧范围对新文本不再有效。
    cx.update_entity(&second, |second, cx| {
        second.replace_text(String::new(), cx).expect("清空第二源");
    });

    let error = cx
        .update_entity(&combined, |buffer, cx| {
            buffer.edit(
                vec![Edit::replace(
                    TextRange::new(ByteOffset::new(1), ByteOffset::new(6)).unwrap(),
                    "X",
                )],
                TransactionMetadata::default(),
                cx,
            )
        })
        .expect_err("第二源版本已推进时多源编辑必须整体失败");
    assert!(
        matches!(error, TextError::Transaction(_)),
        "失败必须是版本不匹配，而不是第一源提交后第二源越界：{error:?}"
    );

    let read = |buffer: &gpui::Entity<LanguageBuffer>, cx: &TestAppContext| {
        cx.read_entity(buffer, |buffer, _| {
            let snapshot = buffer.text_snapshot();
            snapshot
                .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
                .unwrap()
                .as_str()
                .to_owned()
        })
    };
    assert_eq!(
        read(&first, cx),
        "one\n",
        "预检失败不得让第一源留下部分提交文本"
    );
    assert_eq!(read(&second, cx), "");
}

/// 回归：多源编辑中任一源只读时，整体失败且不得部分提交其它源。
///
/// 只读拒绝与版本失配一样必须在任何源提交前一次性判定。
#[gpui::test]
fn multi_source_edit_does_not_partially_commit_when_a_source_is_read_only(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "one\n", cx);
    let second = read_only_singleton("src/b.rs", "two\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::new(
                    first.clone(),
                    TextRange::new(ByteOffset::ZERO, ByteOffset::new(4)).unwrap(),
                    Vec::new(),
                ),
                ExcerptRange::new(
                    second.clone(),
                    TextRange::new(ByteOffset::ZERO, ByteOffset::new(4)).unwrap(),
                    Vec::new(),
                ),
            ],
            cx,
        );
    });

    let error = cx
        .update_entity(&combined, |buffer, cx| {
            buffer.edit(
                vec![Edit::replace(
                    TextRange::new(ByteOffset::new(1), ByteOffset::new(6)).unwrap(),
                    "X",
                )],
                TransactionMetadata::default(),
                cx,
            )
        })
        .expect_err("存在只读源时多源编辑必须整体失败");
    assert!(
        matches!(error, TextError::Storage(StorageError::ReadOnly)),
        "失败必须是只读拒绝，而不是第一源提交后第二源才失败：{error:?}"
    );

    let read = |buffer: &gpui::Entity<LanguageBuffer>, cx: &TestAppContext| {
        cx.read_entity(buffer, |buffer, _| {
            let snapshot = buffer.text_snapshot();
            snapshot
                .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
                .unwrap()
                .as_str()
                .to_owned()
        })
    };
    assert_eq!(
        read(&first, cx),
        "one\n",
        "只读预检失败不得让第一源留下部分提交文本"
    );
}

#[gpui::test]
fn read_only_composite_rejects_edits(cx: &mut TestAppContext) {
    let source = singleton("index.txt", "index 内容\n", cx);
    let combined = cx.new(MultiBuffer::empty_read_only);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(vec![ExcerptRange::line_range(source, 0..1, cx)], cx);
        assert!(buffer.is_read_only());
        let error = buffer
            .edit(
                vec![Edit::insert(ByteOffset::ZERO, "不能写入").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect_err("只读组合文档必须拒绝编辑");
        assert_eq!(error, TextError::Storage(StorageError::ReadOnly));
    });
}

#[gpui::test]
fn materialized_diff_old_side_is_selectable_but_only_new_side_is_editable(cx: &mut TestAppContext) {
    let old = singleton("src/a.rs", "旧内容\n", cx);
    let current = singleton("src/a.rs", "上下文\n新内容\n之后\n", cx);
    let current_buffer = current.clone();
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                ExcerptRange::line_range(current.clone(), 0..1, cx),
                ExcerptRange::line_range(old.clone(), 0..1, cx)
                    .with_editable(false)
                    .with_starts_logical_excerpt(false)
                    .with_diff_kind(ExcerptDiffKind::Deleted),
                ExcerptRange::line_range(current.clone(), 1..2, cx)
                    .with_starts_logical_excerpt(false)
                    .with_diff_kind(ExcerptDiffKind::Added),
                ExcerptRange::line_range(current, 2..3, cx).with_starts_logical_excerpt(false),
            ],
            cx,
        );
    });

    cx.update_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        assert_eq!(
            String::from_utf8(snapshot.text_bytes()).unwrap(),
            "上下文\n旧内容\n新内容\n之后\n"
        );
        assert_eq!(snapshot.excerpts().count(), 4);
        assert_eq!(
            snapshot.excerpt_boundaries().count(),
            1,
            "同一 diff hunk 的物理片段只能形成一个逻辑 excerpt"
        );
        assert_eq!(
            snapshot
                .excerpts()
                .nth(1)
                .unwrap()
                .source_line_for_output_line(1),
            None
        );
        assert_eq!(buffer.file_buffers(cx).len(), 1, "旧修订来源不能参与保存");

        let old_offset = "上下文\n".len() + 1;
        let old_anchor = buffer.anchor_at(ByteOffset::new(old_offset), Affinity::After);
        assert_eq!(
            buffer.resolve_anchor(&old_anchor),
            Some(Into::into(ByteOffset::new(old_offset)))
        );
    });

    cx.update_entity(&combined, |buffer, cx| {
        let old_error = buffer
            .edit(
                vec![Edit::insert(ByteOffset::new("上下文\n".len() + 1), "不能写").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect_err("旧侧只允许选择和导航");
        assert_eq!(old_error, TextError::Storage(StorageError::ReadOnly));

        buffer
            .edit(
                vec![Edit::insert(ByteOffset::new("上下文\n旧内容\n".len()), "可写").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("新侧行首必须归属于可编辑片段");
    });

    let current_text = cx.read_entity(&current_buffer, |buffer, _| {
        let snapshot = buffer.text_snapshot();
        snapshot
            .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
            .unwrap()
            .as_str()
            .to_owned()
    });
    assert_eq!(current_text, "上下文\n可写新内容\n之后\n");
    cx.update_entity(&combined, |buffer, cx| {
        assert_eq!(
            String::from_utf8(buffer.snapshot(cx).text_bytes()).unwrap(),
            "上下文\n旧内容\n可写新内容\n之后\n"
        );
    });
}

/// 文档起点的零长度 excerpt（空文件）必须被 excerpts() 访问；
/// 统一用 Bias::Left 起始遍历，避免跳过零长度边界节点。
#[gpui::test]
fn zero_length_excerpt_at_document_start_is_visited(cx: &mut TestAppContext) {
    let empty = singleton("src/empty.rs", "", cx);
    let other = singleton("src/other.rs", "x\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(empty, 0..1, cx)], cx);
        buffer.set_excerpts_for_path(vec![ExcerptRange::line_range(other, 0..1, cx)], cx);
    });

    let snapshot = cx.update_entity(&combined, |buffer, cx| buffer.snapshot(cx));
    let paths = snapshot
        .excerpts()
        .map(|excerpt| excerpt.path().to_path_buf())
        .collect::<Vec<_>>();
    assert_eq!(
        paths,
        vec![PathBuf::from("src/empty.rs"), PathBuf::from("src/other.rs")],
        "零长度 excerpt 位于文档起点时不能被跳过"
    );
    assert_eq!(snapshot.excerpts_arc().len(), 2);
}
