use std::path::{Path, PathBuf};

use gpui::{AppContext as _, TestAppContext};
use std::sync::Arc;

use crate::{
    BufferDiff, BufferDiffInput, DiffFile, DiffHunkStaging, DiffOperations, DiffProjection,
    DisplayHunk,
};
use zcv_git::DiffHunkKind;
use zcv_language::LanguageBuffer;
use zcv_text::{
    Buffer, BufferConfig, ByteOffset, Edit, Line, StorageError, TextError, TextRange,
    TransactionMetadata,
};

use super::*;

/// 测试用的 diff 注入描述；由 `inject_diffs` 转成预创建的 `DiffFile`。
struct TestDiff {
    working: gpui::Entity<LanguageBuffer>,
    path: PathBuf,
    base_text: Option<Arc<str>>,
    index_text: Option<Arc<str>>,
    operations: Option<Arc<dyn DiffOperations>>,
    display_path: PathBuf,
    context_lines: Option<usize>,
    show_file_header: bool,
}

impl MultiBuffer {
    /// 测试辅助：按描述预创建 `BufferDiff`，再以 `DiffFile` 注入。
    fn inject_diffs(&mut self, files: Option<Vec<TestDiff>>, cx: &mut gpui::Context<Self>) -> bool {
        let files = files.map(|files| {
            files
                .into_iter()
                .map(|file| DiffFile {
                    diff: cx.new(|cx| {
                        BufferDiff::new(
                            BufferDiffInput {
                                working: file.working,
                                path: file.path,
                                base_text: file.base_text,
                                index_text: file.index_text,
                                operations: file.operations,
                            },
                            cx,
                        )
                    }),
                    display_path: file.display_path,
                    context_lines: file.context_lines,
                    show_file_header: file.show_file_header,
                })
                .collect()
        });
        self.set_diff_projection(files.map(DiffProjection::new), cx)
    }
}

/// 清空 Git 状态后，组合文档必须移除旧的 diff 投影，而不是保留过期 hunk。
#[gpui::test]
fn clearing_buffer_diffs_removes_previous_hunks(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "one\nworking\nthree\n", cx);
    let combined = cx.new(|cx| MultiBuffer::from_working_source(source.clone(), cx));
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
                show_file_header: false,
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
        buffer.set_diff_projection(Some(DiffProjection::empty()), cx);
    });
    assert!(cx.read_entity(&combined, |buffer, _| buffer.diff_hunks().is_empty()));
}

/// 未提交 diff 以 HEAD 为 base、工作区为 working，由 index → working 参照逐 hunk 标注暂存语义。
#[gpui::test]
fn unified_diff_marks_staged_and_unstaged_hunks(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "one\nworking\nthree\n", cx);
    let diff = cx.update(|cx| {
        cx.new(|cx| {
            BufferDiff::new(
                BufferDiffInput {
                    working: source.clone(),
                    base_text: Some(Arc::from("one\nhead\nthree\n")),
                    index_text: Some(Arc::from("one\nindex\nthree\n")),
                    path: PathBuf::from("src/a.rs"),
                    operations: None,
                },
                cx,
            )
        })
    });
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
    let staged_diff = cx.update(|cx| {
        cx.new(|cx| {
            BufferDiff::new(
                BufferDiffInput {
                    working: staged_source,
                    base_text: Some(Arc::from("one\nhead\nthree\n")),
                    index_text: Some(Arc::from("one\nstaged\nthree\n")),
                    path: PathBuf::from("src/staged.rs"),
                    operations: None,
                },
                cx,
            )
        })
    });
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
    let diff = cx.update(|cx| {
        cx.new(|cx| {
            BufferDiff::new(
                BufferDiffInput {
                    working: source.clone(),
                    base_text: Some(Arc::from("a\nb\nc\nd\ne\n")),
                    index_text: Some(Arc::from("a\nB\nc\nd\ne\n")),
                    path: PathBuf::from("src/a.rs"),
                    operations: None,
                },
                cx,
            )
        })
    });
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
    let combined = cx.new(|cx| MultiBuffer::from_working_source(index_source.clone(), cx));
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
                show_file_header: false,
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
    let diff = cx.update(|cx| {
        cx.new(|cx| {
            BufferDiff::new(
                BufferDiffInput {
                    working: source.clone(),
                    base_text: Some(Arc::from("line1\nline2\nline3\n")),
                    index_text: Some(Arc::from("line1\nSTAGED\nline3\n")),
                    path: PathBuf::from("src/a.rs"),
                    operations: None,
                },
                cx,
            )
        })
    });
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
    let diff = cx.update(|cx| {
        cx.new(|cx| {
            BufferDiff::new(
                BufferDiffInput {
                    working: source.clone(),
                    base_text: Some(Arc::from("one\nhead\nthree\n")),
                    index_text: Some(Arc::from("one\nX\nthree\n")),
                    path: PathBuf::from("src/a.rs"),
                    operations: None,
                },
                cx,
            )
        })
    });
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

fn singleton(path: &str, text: &str, cx: &mut TestAppContext) -> gpui::Entity<LanguageBuffer> {
    let buffer = cx.new(|_| {
        Buffer::scratch(text.to_owned(), BufferConfig::default()).expect("应创建测试 Buffer")
    });
    cx.new(|cx| LanguageBuffer::new(buffer, Some(PathBuf::from(path)), cx))
}

#[gpui::test]
fn working_source_snapshot_tracks_text_syntax_and_path(cx: &mut TestAppContext) {
    let source = singleton("src/main.rs", "fn main() {}\n", cx);
    let multi_buffer = cx.new(|cx| MultiBuffer::from_working_source(source.clone(), cx));
    let text_buffer = cx.read_entity(&multi_buffer, |buffer, cx| {
        assert_eq!(buffer.file_path(cx), Some(PathBuf::from("src/main.rs")));
        buffer.as_singleton(cx).expect("应为整文件单 excerpt")
    });

    cx.update_entity(&text_buffer, |buffer, cx| {
        buffer
            .edit(
                [Edit::insert(ByteOffset::new(3), "async ").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("测试编辑应成功");
        cx.notify();
    });
    cx.run_until_parked();

    let updated = cx.read_entity(&multi_buffer, |buffer, cx| buffer.snapshot(cx));
    assert_eq!(updated.text().version(), updated.syntax().version());
}

#[gpui::test]
fn source_edit_updates_only_its_composite_excerpt_without_reset(cx: &mut TestAppContext) {
    let first = singleton("src/first.rs", "first\n", cx);
    let second = singleton("src/second.rs", "second\n", cx);
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                MultiBufferExcerpt::line_range(first, 0..1, cx),
                MultiBufferExcerpt::line_range(second.clone(), 0..1, cx),
            ],
            cx,
        );
    });
    let subscription =
        cx.update_entity(&combined, |buffer, cx| buffer.subscribe_and_snapshot(cx).0);

    let source_buffer = cx.read_entity(&second, |source, _| source.buffer());
    cx.update_entity(&source_buffer, |buffer, cx| {
        buffer
            .edit(
                [Edit::insert(ByteOffset::new(0), "changed ").unwrap()],
                TransactionMetadata::default(),
            )
            .expect("源编辑应成功");
        cx.notify();
    });
    cx.run_until_parked();

    let changes = subscription.consume();
    assert!(!changes.requires_reset(), "源编辑不得整体重载组合投影");
    assert_eq!(
        cx.read_entity(&combined, |buffer, cx| {
            String::from_utf8(buffer.snapshot(cx).text_bytes()).expect("组合文本必须是 UTF-8")
        }),
        "first\nchanged second\n"
    );
}

#[gpui::test]
fn singleton_role_does_not_depend_on_current_excerpt_shape(cx: &mut TestAppContext) {
    let source = singleton("src/main.rs", "first\nsecond\n", cx);
    let source_buffer = cx.read_entity(&source, |source, _| source.buffer());
    let working = cx.new(|cx| MultiBuffer::from_working_source(source.clone(), cx));
    cx.update_entity(&working, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                MultiBufferExcerpt::line_range(source.clone(), 0..1, cx),
                MultiBufferExcerpt::line_range(source.clone(), 1..2, cx),
            ],
            cx,
        );
    });
    cx.read_entity(&working, |buffer, cx| {
        assert_eq!(buffer.as_singleton(cx), Some(source_buffer.clone()));
    });

    let composite = cx.new(MultiBuffer::empty);
    cx.update_entity(&composite, |buffer, cx| {
        buffer.set_excerpts(
            vec![MultiBufferExcerpt::new(
                source,
                TextRange::new(ByteOffset::ZERO, ByteOffset::new(13)).unwrap(),
                Vec::new(),
            )],
            cx,
        );
    });
    cx.read_entity(&composite, |buffer, cx| {
        assert!(
            buffer.as_singleton(cx).is_none(),
            "完整文件单 excerpt 也不能把组合文档误判为普通文档"
        );
    });
}

/// 普通编辑器把完整文件包装成工作源 excerpt 后，语言层折叠范围必须投影到组合坐标。
#[gpui::test]
fn working_source_preserves_rust_fold_ranges(cx: &mut TestAppContext) {
    let source = singleton(
        "src/main.rs",
        "fn main() {\n    let value = 1;\n}\nfn other() {\n    let value = 2;\n}\n",
        cx,
    );
    let combined = cx.new(|cx| MultiBuffer::from_working_source(source.clone(), cx));
    cx.run_until_parked();

    let source_folds = cx.read_entity(&source, |buffer, _| buffer.fold_ranges());
    let projected_folds = cx.read_entity(&combined, |buffer, cx| buffer.fold_ranges(cx));

    assert_eq!(source_folds.len(), 2, "Rust 源文档应产生两个折叠范围");
    assert_eq!(
        projected_folds.as_ref(),
        source_folds.as_ref(),
        "整文件 excerpt 不得丢失或偏移源折叠范围"
    );
}

#[gpui::test]
fn outline_projects_source_ranges_into_an_excerpt(cx: &mut TestAppContext) {
    let source = singleton("src/main.rs", "// 前置\nfn 数据() {}\n// 后置\n", cx);
    cx.run_until_parked();
    let (function_range, source_name_start) = cx.read_entity(&source, |source, cx| {
        let text = source.text_snapshot(cx);
        let syntax = source.syntax_snapshot();
        let item = syntax
            .outline(0..text.len_bytes().get(), &text)
            .into_iter()
            .find(|item| item.name == "数据")
            .expect("Rust 函数应出现在源大纲中");
        (item.range, item.name_range.start)
    });
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![MultiBufferExcerpt::new(
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

    let items = cx.read_entity(&combined, |buffer, cx| buffer.snapshot(cx).outline_items());
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
    let function_range = cx.read_entity(&source, |source, cx| {
        let text = source.text_snapshot(cx);
        let syntax = source.syntax_snapshot();
        let node = syntax
            .node_at(source_name_start, &text)
            .expect("Rust 函数名应有语法节点");
        let function = syntax
            .node_ancestors(node.range.clone(), &text)
            .into_iter()
            .find(|node| node.kind == "function_item")
            .expect("Rust 函数应出现在语法祖先链");
        function.range
    });
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![MultiBufferExcerpt::new(
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
        .read_entity(&combined, |buffer, cx| {
            buffer
                .snapshot(cx)
                .node_at(ByteOffset::new(source_name_start - function_range.start))
        })
        .expect("excerpt 内函数名应保留在组合语法节点中");
    assert_eq!(node.kind, "identifier");
    assert_eq!(node.range.start, source_name_start - function_range.start);
}

/// 非零源起点的 excerpt 需要把折叠范围换算到组合坐标，不能沿用源字节偏移。
#[gpui::test]
fn excerpt_projects_contained_fold_range_to_output_coordinates(cx: &mut TestAppContext) {
    let source = singleton(
        "src/main.rs",
        "// 前置行\nfn main() {\n    let value = 1;\n}\n// 后置行\n",
        cx,
    );
    cx.run_until_parked();
    let source_fold = cx.read_entity(&source, |buffer, _| {
        buffer
            .fold_ranges()
            .first()
            .expect("Rust 函数应产生折叠范围")
            .clone()
    });
    let source_start = cx.read_entity(&source, |buffer, cx| {
        buffer
            .text_snapshot(cx)
            .line_start_byte(Line::new(1))
            .expect("函数起始行应存在")
            .get()
    });
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(vec![MultiBufferExcerpt::line_range(source, 1..4, cx)], cx);
    });

    let projected = cx.read_entity(&combined, |buffer, cx| buffer.fold_ranges(cx));
    assert_eq!(
        projected.as_ref(),
        [FoldRange {
            range: source_fold.range.start - source_start..source_fold.range.end - source_start,
        }],
        "折叠范围应相对 excerpt 输出起点投影"
    );
}

/// 后续 excerpt 的组合起点非零：折叠范围投影除了扣掉源起点，还必须叠加组合偏移。
#[gpui::test]
fn fold_projection_accounts_for_nonzero_output_start(cx: &mut TestAppContext) {
    let filler = singleton("src/a.rs", "zero\n", cx);
    let source = singleton(
        "src/main.rs",
        "// 前置行\nfn main() {\n    let value = 1;\n}\n",
        cx,
    );
    cx.run_until_parked();
    let source_folds = cx.read_entity(&source, |buffer, _| buffer.fold_ranges());
    let source_start = cx.read_entity(&source, |buffer, cx| {
        buffer
            .text_snapshot(cx)
            .line_start_byte(Line::new(1))
            .expect("函数起始行应存在")
            .get()
    });
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                MultiBufferExcerpt::line_range(filler, 0..1, cx),
                MultiBufferExcerpt::line_range(source, 1..4, cx),
            ],
            cx,
        );
    });

    let (projected, output_start) = cx.read_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        let output_start = snapshot.excerpts()[1].output_range().start().get();
        assert!(output_start > 0, "第二个 excerpt 的组合起点必须非零");
        (buffer.fold_ranges(cx), output_start)
    });
    assert_eq!(
        projected.as_ref(),
        [FoldRange {
            range: output_start + source_folds[0].range.start - source_start
                ..output_start + source_folds[0].range.end - source_start,
        }],
        "折叠范围应叠加后续 excerpt 的组合起点偏移"
    );
}

#[gpui::test]
fn excerpts_preserve_order_and_map_output_to_source(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "zero\none\ntwo\n", cx);
    let second = singleton("src/b.rs", "alpha\nbeta\n", cx);
    let combined = cx.new(MultiBuffer::empty);

    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                MultiBufferExcerpt::new(
                    first,
                    TextRange::new(ByteOffset::new(5), ByteOffset::new(9)).unwrap(),
                    vec![TextRange::new(ByteOffset::new(5), ByteOffset::new(8)).unwrap()],
                ),
                MultiBufferExcerpt::new(
                    second,
                    TextRange::new(ByteOffset::new(6), ByteOffset::new(11)).unwrap(),
                    vec![TextRange::new(ByteOffset::new(6), ByteOffset::new(10)).unwrap()],
                ),
            ],
            cx,
        );
    });

    let (text, excerpts, first_location, second_location, match_ranges) =
        cx.read_entity(&combined, |buffer, cx| {
            let snapshot = buffer.snapshot(cx);
            let text = String::from_utf8(snapshot.text_bytes()).unwrap();
            let first_offset = ByteOffset::new(text.find("one").unwrap());
            let second_offset = ByteOffset::new(text.find("beta").unwrap());
            (
                text,
                snapshot.excerpts().to_vec(),
                buffer
                    .location_for_range(
                        TextRange::new(first_offset, ByteOffset::new(first_offset.get() + 3))
                            .unwrap(),
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
fn append_excerpts_extends_projection_without_rebuilding_existing_ranges(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "zero\none\n", cx);
    let second = singleton("src/b.rs", "alpha\nbeta\n", cx);
    let combined = cx.new(MultiBuffer::empty);

    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![MultiBufferExcerpt::new(
                first,
                TextRange::new(ByteOffset::new(5), ByteOffset::new(8)).unwrap(),
                vec![TextRange::new(ByteOffset::new(5), ByteOffset::new(8)).unwrap()],
            )],
            cx,
        );
        buffer.append_excerpts(
            vec![MultiBufferExcerpt::new(
                second,
                TextRange::new(ByteOffset::new(6), ByteOffset::new(10)).unwrap(),
                vec![TextRange::new(ByteOffset::new(6), ByteOffset::new(10)).unwrap()],
            )],
            cx,
        );
    });

    cx.read_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        assert_eq!(
            String::from_utf8(snapshot.text_bytes()).unwrap(),
            "one\nbeta"
        );
        assert_eq!(snapshot.excerpts().len(), 2);
        assert_eq!(
            snapshot.excerpts()[0].output_range().start(),
            ByteOffset::ZERO
        );
        assert_eq!(
            snapshot.excerpts()[1].output_range().start(),
            ByteOffset::new(4)
        );
        assert_eq!(
            buffer
                .match_ranges()
                .iter()
                .map(|range| range.start().get())
                .collect::<Vec<_>>(),
            vec![0, 4]
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
                MultiBufferExcerpt::line_range(first.clone(), 0..2, cx),
                MultiBufferExcerpt::line_range(first.clone(), 3..5, cx),
                MultiBufferExcerpt::line_range(second, 0..2, cx),
            ],
            cx,
        );
    });
    let anchor = cx.read_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        let excerpt = &snapshot.excerpts()[1];
        buffer
            .anchor_for_offset(ByteOffset::new(excerpt.output_range().start().get() + 2))
            .expect("应捕获第二个 hunk 内的位置")
    });

    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(vec![MultiBufferExcerpt::line_range(first, 0..2, cx)], cx);
        let offset = buffer
            .resolve_anchor(&anchor)
            .expect("同一文件仍有 excerpt 时应解析到最近位置");
        assert_eq!(
            offset,
            buffer.snapshot(cx).excerpts()[0].output_range().end()
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
                MultiBufferExcerpt::line_range(source.clone(), 0..1, cx),
                MultiBufferExcerpt::line_range(source, 1..3, cx),
            ],
            cx,
        );
        let snapshot = buffer.snapshot(cx);
        let boundary = snapshot.excerpts()[1].output_range().start();
        let anchor = buffer
            .anchor_for_offset(boundary)
            .expect("后续 excerpt 起点必须可以锚定");
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
                MultiBufferExcerpt::line_range(first.clone(), 0..1, cx),
                MultiBufferExcerpt::line_range(second, 0..1, cx),
                MultiBufferExcerpt::line_range(third.clone(), 0..1, cx),
            ],
            cx,
        );
    });
    let anchor = cx.read_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        let excerpt = &snapshot.excerpts()[0];
        buffer
            .anchor_for_offset(excerpt.output_range().start())
            .expect("应捕获首文件位置")
    });

    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(vec![MultiBufferExcerpt::line_range(third, 0..1, cx)], cx);
        let offset = buffer
            .resolve_anchor(&anchor)
            .expect("原文件消失后应解析到仍存在的后继文件");
        assert_eq!(
            offset,
            buffer.snapshot(cx).excerpts()[0].output_range().start()
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
                MultiBufferExcerpt::line_range(first, 0..1, cx),
                MultiBufferExcerpt::line_range(second, 0..1, cx),
            ],
            cx,
        );
    });

    cx.read_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        // 尾换行不变式：非末尾空片段补一个换行占边界行，末尾片段保留原样（文档自身的末尾空行仍为其保留组合行）。
        assert_eq!(String::from_utf8(snapshot.text_bytes()).unwrap(), "\n");
        assert_eq!(snapshot.excerpts()[0].output_start_line(), 0);
        assert_eq!(snapshot.excerpts()[1].output_start_line(), 1);
        assert_eq!(
            buffer.location_for_offset(ByteOffset::new(1)).unwrap().path,
            PathBuf::from("deleted/b.rs")
        );
    });
}

#[gpui::test]
fn source_reparse_does_not_reload_composite_text(cx: &mut TestAppContext) {
    let source = singleton("src/main.rs", "fn main() {\n    println!(\"ok\");\n}\n", cx);
    let source_len = cx.read_entity(&source, |source, cx| source.text_snapshot(cx).len_bytes());
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |combined, cx| {
        combined.set_excerpts(
            vec![MultiBufferExcerpt::new(
                source,
                TextRange::new(ByteOffset::ZERO, source_len).unwrap(),
                Vec::new(),
            )],
            cx,
        );
    });
    let before = cx.read_entity(&combined, |combined, cx| combined.snapshot(cx).version());

    cx.run_until_parked();

    let after = cx.read_entity(&combined, |combined, cx| combined.snapshot(cx).version());
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
            vec![MultiBufferExcerpt::new(
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
    cx.read_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        assert_eq!(
            snapshot.excerpts()[0].source_range(),
            TextRange::new(ByteOffset::new(5), ByteOffset::new(10)).unwrap()
        );
    });
    cx.run_until_parked();
    cx.read_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        let excerpt = &snapshot.excerpts()[0];
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
    let combined = cx.new(|cx| MultiBuffer::from_working_source(source.clone(), cx));
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
                show_file_header: false,
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
    let source_text = cx.read_entity(&source, |buffer, _| buffer.buffer());
    cx.update_entity(&source_text, |buffer, cx| {
        buffer
            .edit(
                vec![Edit::insert(ByteOffset::new(0), "pre\n").unwrap()],
                TransactionMetadata::default(),
            )
            .unwrap();
        cx.notify();
    });
    cx.run_until_parked();

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
                show_file_header: false,
            }]),
            cx,
        );
    });
    cx.run_until_parked();
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
    let combined = cx.new(|cx| MultiBuffer::from_working_source(source.clone(), cx));
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
                show_file_header: false,
            }]),
            cx,
        );
    });
    cx.run_until_parked();
    cx.update_entity(&combined, |buffer, cx| buffer.toggle_diff_hunk_at(0, cx));

    let source_buffer = cx.read_entity(&source, |source, _| source.buffer());
    cx.update_entity(&source_buffer, |buffer, cx| {
        buffer
            .edit(
                [Edit::insert(ByteOffset::new(13), "改过2\n").unwrap()],
                TransactionMetadata::default(),
            )
            .unwrap();
        cx.notify();
    });
    cx.run_until_parked();

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
                show_file_header: false,
            }]),
            cx,
        );
    });
    cx.run_until_parked();

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
                show_file_header: false,
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

    cx.read_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        assert!(
            !snapshot
                .highlights(0..snapshot.text().len_bytes().get())
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
                show_file_header: false,
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
        source.buffer().update(cx, |buffer, cx| {
            buffer.mark_saved();
            cx.notify();
        });
    });
    cx.run_until_parked();

    cx.read_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        assert!(
            !snapshot
                .highlights(0..snapshot.text().len_bytes().get())
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
                    show_file_header: true,
                },
                TestDiff {
                    operations: None,
                    working: modified,
                    base_text: Some(Arc::from("before\nold\nafter\n")),
                    index_text: None,
                    path: PathBuf::from("modified.txt"),
                    display_path: PathBuf::from("modified.txt"),
                    context_lines: Some(2),
                    show_file_header: true,
                },
            ]),
            cx,
        );
    });
    cx.run_until_parked();

    cx.read_entity(&combined, |buffer, cx| {
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
    let combined = cx.new(|cx| MultiBuffer::from_working_source(source.clone(), cx));
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
                show_file_header: false,
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
                show_file_header: false,
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
    let combined = cx.new(|cx| MultiBuffer::from_working_source(source.clone(), cx));
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
                show_file_header: false,
            }]),
            cx,
        );
    });

    let source_text = cx.read_entity(&source, |source, _| source.buffer());
    cx.update_entity(&source_text, |buffer, cx| {
        buffer
            .reload_from_text("replacement\n".to_owned())
            .expect("外部整体替换应成功");
        cx.notify();
    });
    cx.run_until_parked();

    cx.read_entity(&combined, |buffer, cx| {
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

/// BufferDiff 不自行订阅源：working 文本变化由宿主（组合文档投影）驱动重算。
#[gpui::test]
fn host_drives_buffer_diff_recompute_from_source_edits(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "a\nb\n", cx);
    let combined = cx.new(|cx| MultiBuffer::from_working_source(source.clone(), cx));
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
                show_file_header: false,
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
    let source_buffer = cx.read_entity(&source, |source, _| source.buffer());
    cx.update_entity(&source_buffer, |buffer, cx| {
        buffer
            .edit(
                vec![Edit::replace(
                    TextRange::new(ByteOffset::new(2), ByteOffset::new(4)).unwrap(),
                    "",
                )],
                TransactionMetadata::default(),
            )
            .unwrap();
        cx.notify();
    });
    cx.run_until_parked();

    assert_eq!(
        cx.read_entity(&combined, |buffer, _cx| buffer.diff_hunks().len()),
        1,
        "未保存期间应保留原有组合 hunk"
    );

    // 保存后由宿主重新注入 diff，才提交新的 hunk 投影。
    cx.update_entity(&source_buffer, |buffer, cx| {
        buffer.mark_saved();
        cx.notify();
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
                show_file_header: false,
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
    let combined = cx.new(|cx| MultiBuffer::from_working_source(source.clone(), cx));
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
                show_file_header: true,
            }]),
            cx,
        );
    });
    cx.run_until_parked();
    let initial_excerpt_count =
        cx.read_entity(&combined, |buffer, cx| buffer.snapshot(cx).excerpts().len());
    assert!(initial_excerpt_count > 0, "初始 hunk 应生成 excerpt");

    let source_buffer = cx.read_entity(&source, |source, _| source.buffer());
    cx.update_entity(&source_buffer, |buffer, cx| {
        buffer
            .edit(
                vec![Edit::replace(
                    TextRange::new(ByteOffset::new(2), ByteOffset::new(4)).unwrap(),
                    "",
                )],
                TransactionMetadata::default(),
            )
            .unwrap();
        cx.notify();
    });
    cx.run_until_parked();

    cx.read_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        assert!(
            buffer.is_diff_file_dirty(Path::new("src/a.rs"), cx),
            "组合文档应能读取文件 working source 的 dirty 状态"
        );
        assert_eq!(
            snapshot.excerpts().len(),
            initial_excerpt_count,
            "未保存期间不能因 hunk 为空而移除既有 excerpt"
        );
        assert!(
            snapshot
                .excerpts()
                .iter()
                .all(|excerpt| excerpt.path() == Path::new("src/a.rs"))
        );
        assert_eq!(String::from_utf8(snapshot.text_bytes()).unwrap(), "a\n");
    });
}

/// 回归：编辑内容但 hunk 几何不变时，diff 高亮不能因显示坐标门控而整体消失。
#[gpui::test]
fn diff_hunks_survive_geometry_preserving_edits(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "a\nb\nc\n", cx);
    let combined = cx.new(|cx| MultiBuffer::from_working_source(source.clone(), cx));
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
                show_file_header: false,
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
    let source_buffer = cx.read_entity(&source, |source, _| source.buffer());
    cx.update_entity(&source_buffer, |buffer, cx| {
        buffer
            .edit(
                vec![Edit::replace(
                    TextRange::new(ByteOffset::new(2), ByteOffset::new(3)).unwrap(),
                    "z",
                )],
                TransactionMetadata::default(),
            )
            .unwrap();
        cx.notify();
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
    let combined = cx.new(|cx| MultiBuffer::from_working_source(source_a.clone(), cx));
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
                show_file_header: false,
            }]),
            cx,
        );
    });
    cx.run_until_parked();
    assert_eq!(
        cx.read_entity(&combined, |buffer, _| buffer.diff_hunks().len()),
        1
    );

    // 加入第二个文件；不 park，使它的 diff 仍在计算中（set_diff_projection 因而提前返回）。
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
                    show_file_header: false,
                },
                TestDiff {
                    working: source_c.clone(),
                    base_text: Some(Arc::from("x\nY\nz\n")),
                    index_text: None,
                    path: PathBuf::from("src/c.rs"),
                    operations: None,
                    display_path: PathBuf::from("src/c.rs"),
                    context_lines: None,
                    show_file_header: false,
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
    let diff = cx.update(|cx| {
        cx.new(|cx| {
            BufferDiff::new(
                BufferDiffInput {
                    working: source.clone(),
                    base_text: Some(Arc::from("a\nB\nc\n")),
                    index_text: None,
                    path: PathBuf::from("src/a.rs"),
                    operations: None,
                },
                cx,
            )
        })
    });
    // 不 park，立即编辑源：初始后台结果会对应旧版本并被版本门控拒绝。
    let source_buffer = cx.read_entity(&source, |source, _| source.buffer());
    cx.update_entity(&source_buffer, |buffer, cx| {
        buffer
            .edit(
                vec![Edit::replace(
                    TextRange::new(ByteOffset::new(2), ByteOffset::new(3)).unwrap(),
                    "z",
                )],
                TransactionMetadata::default(),
            )
            .unwrap();
        cx.notify();
    });
    cx.run_until_parked();
    assert!(
        cx.read_entity(&diff, |diff, cx| diff.is_current_version_calculated(cx)),
        "过期结果被拒后必须补算到当前版本"
    );
}

/// 回归：连续多次编辑后，diff 高亮仍必须可用（不依赖异步 DiffChanged 恰好重建）。
#[gpui::test]
fn diff_hunks_survive_rapid_edits(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "a\nb\nc\n", cx);
    let combined = cx.new(|cx| MultiBuffer::from_working_source(source.clone(), cx));
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
                show_file_header: false,
            }]),
            cx,
        );
    });
    cx.run_until_parked();
    let source_buffer = cx.read_entity(&source, |source, _| source.buffer());
    // 快速连续编辑同一行（每次都改变 working 版本，但 hunk 几何不变）。
    for i in 0..6u8 {
        let replacement = char::from(b'a' + i).to_string();
        cx.update_entity(&source_buffer, |buffer, cx| {
            buffer
                .edit(
                    vec![Edit::replace(
                        TextRange::new(ByteOffset::new(2), ByteOffset::new(3)).unwrap(),
                        replacement.clone(),
                    )],
                    TransactionMetadata::default(),
                )
                .unwrap();
            cx.notify();
        });
    }
    cx.run_until_parked();
    assert!(
        !cx.read_entity(&combined, |buffer, _cx| buffer.diff_hunks().is_empty()),
        "连续编辑后 diff 高亮不能消失"
    );
}

/// 新增块没有旧侧内容，不参与展开/折叠：整行背景只由展开策略默认值决定。
/// 普通文档默认折叠（只保留 gutter 竖条），差异审阅视图默认展开展示背景色。
#[gpui::test]
fn added_hunk_background_follows_view_expansion_policy(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "a\nb\nc\n", cx);
    let combined = cx.new(|cx| MultiBuffer::from_working_source(source.clone(), cx));
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
                show_file_header: false,
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

/// 词级 diff 按空白 / 单词 / 标点切分：只有真正变化的词进入范围，相同文本无范围。
#[test]
fn word_diff_ranges_split_words_and_punctuation() {
    let (old, new) = crate::word_diff::word_diff_ranges("let x = 1;\n", "let x = 2;\n");
    assert_eq!(old, vec![8..9], "旧侧只应包含变化的数字");
    assert_eq!(new, vec![8..9], "新侧只应包含变化的数字");

    let (old, new) = crate::word_diff::word_diff_ranges("a b c", "a X c");
    assert_eq!(old, vec![2..3]);
    assert_eq!(new, vec![2..3]);

    assert_eq!(
        crate::word_diff::word_diff_ranges("same\n", "same\n"),
        (Vec::new(), Vec::new()),
        "相同文本不应产生词级范围"
    );
}

/// 展开的修改块词级范围必须落在组合文档坐标：旧侧指针指向 base 文本，新侧指针指向 working 文本。
#[gpui::test]
fn expanded_modified_hunk_exposes_word_diffs_in_composite_coordinates(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "let x = 2;\n", cx);
    let combined = cx.new(|cx| MultiBuffer::from_working_source(source.clone(), cx));
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
                show_file_header: false,
            }]),
            cx,
        );
        buffer.set_diff_hunks_expanded_by_default(true, cx);
    });
    cx.run_until_parked();

    let (text, word_diffs) = cx.read_entity(&combined, |buffer, cx| {
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
    let source_text = cx.read_entity(&source, |buffer, _| buffer.buffer());
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![MultiBufferExcerpt::new(
                source,
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

    let source_contents = cx.read_entity(&source_text, |buffer, _| {
        buffer
            .slice_byte_range(ByteOffset::ZERO, buffer.len_bytes())
            .unwrap()
            .as_str()
            .to_owned()
    });
    assert_eq!(source_contents, "zero\nONE\ntwo\n");
    let projection = cx.read_entity(&combined, |buffer, cx| {
        String::from_utf8(buffer.snapshot(cx).text_bytes()).unwrap()
    });
    assert_eq!(projection, "ONE\n");
    cx.read_entity(&combined, |buffer, cx| {
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
                MultiBufferExcerpt::new(
                    source.clone(),
                    TextRange::new(ByteOffset::ZERO, ByteOffset::new(5)).unwrap(),
                    Vec::new(),
                ),
                MultiBufferExcerpt::new(
                    source,
                    TextRange::new(ByteOffset::new(5), ByteOffset::new(9)).unwrap(),
                    Vec::new(),
                ),
            ],
            cx,
        );
    });

    cx.read_entity(&combined, |buffer, cx| {
        assert_eq!(buffer.file_buffers(cx).len(), 1);
    });
}

#[gpui::test]
fn composite_tracks_edits_made_through_another_editor(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "zero\none\ntwo\n", cx);
    let source_text = cx.read_entity(&source, |buffer, _| buffer.buffer());
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![MultiBufferExcerpt::new(
                source,
                TextRange::new(ByteOffset::new(5), ByteOffset::new(9)).unwrap(),
                Vec::new(),
            )],
            cx,
        )
    });

    cx.update_entity(&source_text, |buffer, cx| {
        buffer
            .edit(
                [Edit::replace(
                    TextRange::new(ByteOffset::new(5), ByteOffset::new(8)).unwrap(),
                    "ONE",
                )],
                TransactionMetadata::default(),
            )
            .unwrap();
        cx.notify();
    });
    cx.run_until_parked();

    let projection = cx.read_entity(&combined, |buffer, cx| {
        String::from_utf8(buffer.snapshot(cx).text_bytes()).unwrap()
    });
    assert_eq!(projection, "ONE\n");
}

#[gpui::test]
fn composite_splits_cross_excerpt_edits_across_source_buffers(cx: &mut TestAppContext) {
    let first = singleton("src/a.rs", "one\n", cx);
    let second = singleton("src/b.rs", "two\n", cx);
    let first_text = cx.read_entity(&first, |buffer, _| buffer.buffer());
    let second_text = cx.read_entity(&second, |buffer, _| buffer.buffer());
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                MultiBufferExcerpt::new(
                    first,
                    TextRange::new(ByteOffset::ZERO, ByteOffset::new(4)).unwrap(),
                    Vec::new(),
                ),
                MultiBufferExcerpt::new(
                    second,
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

    let read = |buffer: &gpui::Entity<Buffer>, cx: &TestAppContext| {
        cx.read_entity(buffer, |buffer, _| {
            buffer
                .slice_byte_range(ByteOffset::ZERO, buffer.len_bytes())
                .unwrap()
                .as_str()
                .to_owned()
        })
    };
    assert_eq!(read(&first_text, cx), "oX");
    assert_eq!(read(&second_text, cx), "o\n");
}

#[gpui::test]
fn read_only_composite_rejects_edits(cx: &mut TestAppContext) {
    let source = singleton("index.txt", "index 内容\n", cx);
    let combined = cx.new(MultiBuffer::empty_read_only);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(vec![MultiBufferExcerpt::line_range(source, 0..1, cx)], cx);
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
    let current_buffer = cx.read_entity(&current, |source, _| source.buffer());
    let combined = cx.new(MultiBuffer::empty);
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_excerpts(
            vec![
                MultiBufferExcerpt::line_range(current.clone(), 0..1, cx),
                MultiBufferExcerpt::line_range(old.clone(), 0..1, cx)
                    .with_editable(false)
                    .with_starts_new_excerpt(false)
                    .with_diff_kind(ExcerptDiffKind::Deleted),
                MultiBufferExcerpt::line_range(current.clone(), 1..2, cx)
                    .with_starts_new_excerpt(false)
                    .with_diff_kind(ExcerptDiffKind::Added),
                MultiBufferExcerpt::line_range(current, 2..3, cx).with_starts_new_excerpt(false),
            ],
            cx,
        );
    });

    cx.read_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        assert_eq!(
            String::from_utf8(snapshot.text_bytes()).unwrap(),
            "上下文\n旧内容\n新内容\n之后\n"
        );
        assert_eq!(snapshot.excerpts().len(), 4);
        assert!(snapshot.excerpts()[0].starts_new_excerpt());
        assert!(!snapshot.excerpts()[1].starts_new_excerpt());
        assert_eq!(snapshot.excerpts()[1].source_line_for_output_line(1), None);
        assert_eq!(buffer.file_buffers(cx).len(), 1, "旧修订来源不能参与保存");

        let old_offset = "上下文\n".len() + 1;
        let old_anchor = buffer
            .anchor_for_offset(ByteOffset::new(old_offset))
            .expect("旧侧必须能建立普通 MultiBuffer 锚点");
        assert_eq!(
            buffer.resolve_anchor(&old_anchor),
            Some(ByteOffset::new(old_offset))
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
        buffer
            .slice_byte_range(ByteOffset::ZERO, buffer.len_bytes())
            .unwrap()
            .as_str()
            .to_owned()
    });
    assert_eq!(current_text, "上下文\n可写新内容\n之后\n");
    cx.read_entity(&combined, |buffer, cx| {
        assert_eq!(
            String::from_utf8(buffer.snapshot(cx).text_bytes()).unwrap(),
            "上下文\n旧内容\n可写新内容\n之后\n"
        );
    });
}
