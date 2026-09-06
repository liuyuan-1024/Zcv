use std::path::{Path, PathBuf};

use gpui::{AppContext as _, TestAppContext};
use std::sync::Arc;

use crate::DiffFileInput;
use zcv_git::{DiffHunk, DiffHunkKind};
use zcv_language::LanguageBuffer;
use zcv_text::{Buffer, BufferConfig, ByteOffset, Edit, TextRange, TransactionMetadata};

use super::*;

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
            .line_start_byte(zcv_text::Line::new(1))
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
            .line_start_byte(zcv_text::Line::new(1))
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

/// 锚点范围迁移：展开 hunk 后编辑工作区源（行号变化），已有高亮立即跟随，重新注入的新 hunk 仍按文本位置识别为同一 hunk。
#[gpui::test]
fn diff_expansion_migrates_by_anchor_range_across_source_edits(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "zero\none\ntwo\nthree\n", cx);
    let combined = cx.new(|cx| MultiBuffer::from_working_source(source.clone(), cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_diff_projection(
            Some(vec![DiffFileInput {
                working: source.clone(),
                hunks: vec![DiffHunk {
                    range: 1..2,
                    old_range: 1..2,
                    kind: DiffHunkKind::Modified,
                }],
                base_text: Some(Arc::from("zero\none\ntwo\nthree\n")),
                path: PathBuf::from("src/a.rs"),
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
                is_created: false,
                show_file_header: false,
            }]),
            cx,
        );
        buffer.toggle_diff_hunk_at(0, cx);
    });
    assert!(
        cx.read_entity(&combined, |buffer, cx| {
            buffer
                .diff_hunk_expanded(cx)
                .iter()
                .all(|&expanded| expanded)
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

    let hunks_after_edit = cx.read_entity(&combined, |buffer, cx| buffer.diff_hunks(cx).to_vec());
    assert_eq!(hunks_after_edit.len(), 1, "编辑后已有 Git 高亮不应消失");
    assert_eq!(hunks_after_edit[0].range, 3..4);

    // 重新注入：hunk 新侧坐标移到行 2（文件内容已变），展开状态按文本跟踪区间迁移到新 hunk。
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_diff_projection(
            Some(vec![DiffFileInput {
                working: source.clone(),
                hunks: vec![DiffHunk {
                    range: 2..3,
                    old_range: 1..2,
                    kind: DiffHunkKind::Modified,
                }],
                base_text: Some(Arc::from("pre\nzero\none\ntwo\nthree\n")),
                path: PathBuf::from("src/a.rs"),
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
                is_created: false,
                show_file_header: false,
            }]),
            cx,
        );
    });
    let (hunks, expanded) = cx.read_entity(&combined, |buffer, cx| {
        (
            buffer.diff_hunks(cx).to_vec(),
            buffer.diff_hunk_expanded(cx),
        )
    });
    assert_eq!(hunks.len(), 1, "重新注入后应显示新坐标 hunk");
    // 显示坐标 = 源坐标 + 物化旧行偏移（展开 1 行旧行 → 2..3 显示为 3..4）。
    assert_eq!(hunks[0].range, 3..4);
    assert!(
        expanded.first().copied().unwrap_or(false),
        "编辑后重新注入应把展开状态迁移到文本位置相同的 hunk"
    );
}

/// 加载态刷新后，合并后的新 hunk 仍保留用户对旧 hunk 的展开状态。
#[gpui::test]
fn diff_expansion_survives_hunk_refresh_and_merge(cx: &mut TestAppContext) {
    let source = singleton("tracked.txt", "line0\n改过\nline2\nline3\n", cx);
    let combined = cx.new(|cx| MultiBuffer::from_working_source(source.clone(), cx));
    let base_text: Arc<str> = Arc::from("line0\nline1\nline2\nline3\n");

    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_diff_projection(
            Some(vec![DiffFileInput {
                working: source.clone(),
                hunks: vec![DiffHunk {
                    range: 1..2,
                    old_range: 1..2,
                    kind: DiffHunkKind::Modified,
                }],
                base_text: Some(base_text.clone()),
                path: PathBuf::from("tracked.txt"),
                display_path: PathBuf::from("tracked.txt"),
                context_lines: None,
                is_created: false,
                show_file_header: false,
            }]),
            cx,
        );
        buffer.toggle_diff_hunk_at(0, cx);
    });

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
        assert!(!buffer.set_diff_projection(None, cx));
        buffer.set_diff_projection(
            Some(vec![DiffFileInput {
                working: source.clone(),
                hunks: vec![DiffHunk {
                    range: 1..3,
                    old_range: 1..3,
                    kind: DiffHunkKind::Modified,
                }],
                base_text: Some(base_text),
                path: PathBuf::from("tracked.txt"),
                display_path: PathBuf::from("tracked.txt"),
                context_lines: None,
                is_created: false,
                show_file_header: false,
            }]),
            cx,
        );
    });

    let (hunks, expanded) = cx.read_entity(&combined, |buffer, cx| {
        (
            buffer.diff_hunks(cx).to_vec(),
            buffer.diff_hunk_expanded(cx),
        )
    });
    assert_eq!(hunks.len(), 1, "刷新后相邻改动应合并为一个 hunk");
    assert_eq!(hunks[0].old_range, 1..3, "合并后的 hunk 旧侧应为 1..3");
    assert!(
        expanded.first().copied().unwrap_or(false),
        "hunk 刷新后展开状态应迁移到合并后的新 hunk"
    );
}

#[gpui::test]
fn undo_keeps_rust_highlighting_in_diff_projection(cx: &mut TestAppContext) {
    let source = singleton(
        "src/window_controls.rs",
        "fn main() { let value = 1; }\n",
        cx,
    );
    let combined = cx.new(|cx| MultiBuffer::empty(cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_diff_projection(
            Some(vec![DiffFileInput {
                working: source.clone(),
                hunks: vec![DiffHunk {
                    range: 0..1,
                    old_range: 0..1,
                    kind: DiffHunkKind::Modified,
                }],
                base_text: Some(Arc::from("fn main() { let value = 0; }\n")),
                path: PathBuf::from("src/window_controls.rs"),
                display_path: PathBuf::from("src/window_controls.rs"),
                context_lines: Some(2),
                is_created: false,
                show_file_header: false,
            }]),
            cx,
        );
        buffer.start_transaction(cx).expect("应开始 hunk 编辑事务");
        buffer
            .edit(
                vec![zcv_text::Edit::insert(ByteOffset::new(3), "async ").unwrap()],
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
    let combined = cx.new(|cx| MultiBuffer::empty(cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_diff_projection(
            Some(vec![DiffFileInput {
                working: source.clone(),
                hunks: vec![DiffHunk {
                    range: 0..1,
                    old_range: 0..1,
                    kind: DiffHunkKind::Modified,
                }],
                base_text: Some(Arc::from("fn main() { let value = 0; }\n")),
                path: PathBuf::from("src/window_controls.rs"),
                display_path: PathBuf::from("src/window_controls.rs"),
                context_lines: Some(2),
                is_created: false,
                show_file_header: false,
            }]),
            cx,
        );
        buffer
            .edit(
                vec![zcv_text::Edit::insert(ByteOffset::new(3), "async ").unwrap()],
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
        buffer.set_diff_projection(
            Some(vec![
                DiffFileInput {
                    working: created,
                    hunks: Vec::new(),
                    base_text: Some(Arc::from("")),
                    path: PathBuf::from("created.txt"),
                    display_path: PathBuf::from("created.txt"),
                    context_lines: Some(2),
                    is_created: true,
                    show_file_header: true,
                },
                DiffFileInput {
                    working: modified,
                    hunks: vec![DiffHunk {
                        range: 1..2,
                        old_range: 1..2,
                        kind: DiffHunkKind::Modified,
                    }],
                    base_text: Some(Arc::from("before\nold\nafter\n")),
                    path: PathBuf::from("modified.txt"),
                    display_path: PathBuf::from("modified.txt"),
                    context_lines: Some(2),
                    is_created: false,
                    show_file_header: true,
                },
            ]),
            cx,
        );
    });

    cx.read_entity(&combined, |buffer, cx| {
        let snapshot = buffer.snapshot(cx);
        assert_eq!(
            String::from_utf8(snapshot.text_bytes()).expect("投影应为 UTF-8"),
            "created\nbefore\nold\nnew\nafter\n"
        );
        assert_eq!(
            buffer.diff_hunk_old_ranges(cx),
            &[None, Some(2..3)],
            "旧侧范围应落在实际物化的 old 行"
        );
        assert_eq!(
            buffer.diff_hunks(cx),
            &[
                DiffHunk {
                    range: 0..1,
                    old_range: 0..0,
                    kind: DiffHunkKind::Added,
                },
                DiffHunk {
                    range: 3..4,
                    old_range: 1..2,
                    kind: DiffHunkKind::Modified,
                },
            ],
            "新侧范围应落在实际物化的 new 行，不能受前一文件末尾空逻辑行影响"
        );
        assert_eq!(
            buffer.diff_hunk_expanded(cx),
            &[true, true],
            "展开状态必须与物化后的显示 hunk 同序，不能遗漏整文件新增块"
        );
        assert!(
            buffer
                .diff_hunk_source_at(0, cx)
                .expect("整文件新增块应有显示来源")
                .source
                .is_none(),
            "整文件新增块不应伪造源 hunk"
        );
        assert_eq!(
            buffer
                .diff_hunk_source_at(1, cx)
                .expect("修改 hunk 应有显示来源")
                .source,
            Some(DiffHunk {
                range: 1..2,
                old_range: 1..2,
                kind: DiffHunkKind::Modified,
            })
        );
    });
}

/// 文本跟踪区间在 base 版本变化（提交等）后依然有效：工作区文本未变时重新注入的 hunk
/// 按文本位置识别为同一 hunk，展开状态保留（对应 Zed 的
/// `test_diff_base_change_with_expanded_diff_hunks`）。
#[gpui::test]
fn diff_expansion_survives_base_change_when_working_text_is_unchanged(cx: &mut TestAppContext) {
    let source = singleton("src/a.rs", "zero\none\ntwo\nthree\n", cx);
    let combined = cx.new(|cx| MultiBuffer::from_working_source(source.clone(), cx));
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_diff_projection(
            Some(vec![DiffFileInput {
                working: source.clone(),
                hunks: vec![DiffHunk {
                    range: 1..2,
                    old_range: 1..2,
                    kind: DiffHunkKind::Modified,
                }],
                base_text: Some(Arc::from("zero\none\ntwo\nthree\n")),
                path: PathBuf::from("src/a.rs"),
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
                is_created: false,
                show_file_header: false,
            }]),
            cx,
        );
        buffer.toggle_diff_hunk_at(0, cx);
    });
    // base 完全变化（模拟提交后新 HEAD）：旧侧行号空间整体失效（指向新 base 的其他行），
    // 但工作区文本未变，锚点仍把新 hunk 识别为同一 hunk。
    cx.update_entity(&combined, |buffer, cx| {
        buffer.set_diff_projection(
            Some(vec![DiffFileInput {
                working: source.clone(),
                hunks: vec![DiffHunk {
                    range: 1..2,
                    old_range: 2..3,
                    kind: DiffHunkKind::Modified,
                }],
                base_text: Some(Arc::from("zero\none\ntwo\nthree\n")),
                path: PathBuf::from("src/a.rs"),
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
                is_created: false,
                show_file_header: false,
            }]),
            cx,
        );
    });
    assert!(
        cx.read_entity(&combined, |buffer, cx| {
            buffer
                .diff_hunk_expanded(cx)
                .iter()
                .all(|&expanded| expanded)
        }),
        "base 变化但工作区文本未变时，展开状态应按文本跟踪区间保留"
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
        assert_eq!(
            error,
            zcv_text::TextError::Storage(zcv_text::StorageError::ReadOnly)
        );
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
        assert_eq!(
            old_error,
            zcv_text::TextError::Storage(zcv_text::StorageError::ReadOnly)
        );

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
