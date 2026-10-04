//! `BufferDiff` 后台任务所有权与输入版本门控的定向测试。

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{AppContext as _, Entity, Task, TestAppContext};
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_text::{Buffer, BufferConfig, ByteOffset, Edit, TransactionMetadata};

use crate::{BufferDiff, BufferDiffInput, DiffHunkStaging, PendingHunk, diff_line_boundary};

#[test]
fn diff_line_boundary_excludes_only_the_terminal_empty_line() {
    let trailing_newline = Buffer::from_text("a\nb\n".to_owned(), BufferConfig::default())
        .expect("带末尾换行的测试文本必须能创建")
        .snapshot();
    let no_trailing_newline = Buffer::from_text("a\nb".to_owned(), BufferConfig::default())
        .expect("无末尾换行的测试文本必须能创建")
        .snapshot();

    assert_eq!(
        diff_line_boundary(&trailing_newline, trailing_newline.len_bytes()),
        trailing_newline.line_count() - 1
    );
    assert_eq!(
        diff_line_boundary(&no_trailing_newline, no_trailing_newline.len_bytes()),
        no_trailing_newline.line_count()
    );
}

/// 建立一个指定文本与路径的语言 Buffer（diff 输入源）。
fn language_buffer(
    text: &str,
    path: &str,
    cx: &mut impl gpui::AppContext,
) -> Entity<LanguageBuffer> {
    let buffer = Buffer::from_text(text.to_string(), BufferConfig::default())
        .expect("测试文本必须能创建 Buffer");
    let path = PathBuf::from(path);
    let registry = Arc::new(LanguageRegistry::new());
    cx.new(|cx| LanguageBuffer::new(buffer, Some(path), registry, cx))
}

/// 建立只含 working 与 base 文本的 diff 输入。
fn buffer_diff_input(
    working: Entity<LanguageBuffer>,
    base_text: Option<&str>,
    path: &str,
) -> BufferDiffInput {
    BufferDiffInput {
        working,
        path: PathBuf::from(path),
        base_text: base_text.map(Arc::from),
        index_text: None,
        language_registry: Arc::new(LanguageRegistry::new()),
        key: 0,
        operations: None,
    }
}

/// 回归（M-E）：连续重算始终只替换实体拥有的同一个在途任务，实体销毁时随字段取消。
#[gpui::test]
fn rapid_recomputes_replace_the_single_owned_task(cx: &mut TestAppContext) {
    let working = language_buffer("a\nb\nc\n", "src/a.rs", cx);
    let diff = cx.new(|cx| {
        BufferDiff::new(
            buffer_diff_input(working, Some("a\nB\nc\n"), "src/a.rs"),
            cx,
        )
    });

    // 创建即拥有一个在途任务；每次重算都替换同一字段，而不是堆积多个任务。
    for _ in 0..8 {
        assert!(
            cx.read_entity(&diff, |diff, _| diff.calculation_task.is_some()),
            "实体必须拥有在途任务"
        );
        cx.update_entity(&diff, |diff, cx| {
            diff.recompute(cx);
        });
    }
    cx.run_until_parked();
    assert!(
        cx.read_entity(&diff, |diff, _| diff
            .calculation_task
            .as_ref()
            .is_some_and(Task::is_ready)),
        "任务完成后仍由实体持有，下一次重算替换它"
    );
    assert!(
        cx.update_entity(&diff, |diff, cx| diff.is_current_version_calculated(cx)),
        "连续重算必须收敛到当前输入版本"
    );
}

/// 回归（M-F）：working 不变而 base 连续前进时，旧 base 对应的结果不得安装。
#[gpui::test]
fn base_version_change_discards_stale_hunks(cx: &mut TestAppContext) {
    let working = language_buffer("a\nb\nc\n", "src/a.rs", cx);
    let diff = cx.new(|cx| {
        BufferDiff::new(
            buffer_diff_input(working, Some("a\nX\nc\n"), "src/a.rs"),
            cx,
        )
    });

    // 初始 diff 在途时刷新 base；working 版本保持不变。
    cx.update_entity(&diff, |diff, cx| {
        diff.set_revisions(Some(Arc::from("a\nLONGER\nc\n")), None, cx)
    });
    cx.run_until_parked();

    let hunks = cx.read_entity(&diff, |diff, _| {
        diff.snapshot().hunks().cloned().collect::<Vec<_>>()
    });
    assert_eq!(hunks.len(), 1, "base 变化后必须重算出唯一修改块");
    assert_eq!(
        hunks[0].diff_base_byte_range,
        2..9,
        "只有与最终 base 对应的旧侧字节范围可以落地"
    );
    assert!(
        cx.update_entity(&diff, |diff, cx| diff.is_current_version_calculated(cx)),
        "base 前进后必须补算到当前输入版本"
    );
}

#[gpui::test]
fn latest_revision_text_supersedes_an_in_flight_change(cx: &mut TestAppContext) {
    let working = language_buffer("原始\n", "src/a.rs", cx);
    let diff = cx.new(|cx| {
        let mut input = buffer_diff_input(working, Some("原始\n"), "src/a.rs");
        input.index_text = Some("原始\n".into());
        BufferDiff::new(input, cx)
    });
    cx.run_until_parked();
    for base in [true, false] {
        diff.update(cx, |diff, cx| {
            if base {
                diff.set_revisions(Some(Arc::from("过期\n")), Some(Arc::from("原始\n")), cx);
                diff.set_revisions(Some(Arc::from("原始\n")), Some(Arc::from("原始\n")), cx);
            } else {
                diff.set_revisions(Some(Arc::from("原始\n")), Some(Arc::from("过期\n")), cx);
                diff.set_revisions(Some(Arc::from("原始\n")), Some(Arc::from("原始\n")), cx);
            }
        });
        cx.run_until_parked();
        diff.read_with(cx, |diff, cx| {
            let source = if base {
                diff.base_source()
            } else {
                diff.index_source()
            }
            .unwrap();
            assert_eq!(
                super::full_text(&source.read(cx).text_snapshot()),
                "原始\n",
                "最新文本与当前相同时也必须淘汰旧安装任务"
            );
            assert!(diff.is_current_version_calculated(cx));
            assert_eq!(diff.snapshot().hunk_count(), 0);
        });
    }
}

#[gpui::test]
fn source_edits_recalculate_without_a_projection_consumer(cx: &mut TestAppContext) {
    let working = language_buffer("原始\n", "src/a.rs", cx);
    let diff = cx.new(|cx| {
        BufferDiff::new(
            buffer_diff_input(working.clone(), Some("原始\n"), "src/a.rs"),
            cx,
        )
    });
    cx.run_until_parked();
    for (text, count) in [("变更\n", 1), ("原始\n", 0)] {
        working.update(cx, |working, cx| {
            working.replace_text(text.into(), cx).unwrap()
        });
        cx.run_until_parked();
        diff.read_with(cx, |diff, cx| {
            assert!(diff.is_current_version_calculated(cx));
            assert_eq!(
                diff.snapshot().hunk_count(),
                count,
                "没有挂接组合文档时，源编辑也必须推进差异结果"
            );
        });
    }
}

#[gpui::test]
fn pending_stage_survives_unrelated_working_edits_until_revision_changes(cx: &mut TestAppContext) {
    let working = language_buffer("a\nchanged\nz\n", "src/a.rs", cx);
    let diff = cx.new(|cx| {
        BufferDiff::new(
            buffer_diff_input(working.clone(), Some("a\nold\nz\n"), "src/a.rs"),
            cx,
        )
    });
    cx.run_until_parked();
    diff.update(cx, |diff, cx| {
        let hunk = diff
            .snapshot()
            .hunks()
            .next()
            .expect("必须有变更块")
            .clone();
        let version = diff.working().read(cx).text_snapshot().version();
        diff.set_pending_hunks(vec![PendingHunk::set_staging(&hunk, version, true)], cx);
    });
    working.update(cx, |working, cx| {
        let end = working.text_snapshot().len_bytes();
        working
            .edit(
                [Edit::insert(end, "tail\n").expect("末尾插入有效")],
                TransactionMetadata::default(),
                cx,
            )
            .expect("应编辑工作区");
    });
    cx.run_until_parked();
    diff.read_with(cx, |diff, cx| {
        let working = diff.working().read(cx).text_snapshot();
        assert_eq!(diff.snapshot().pending_hunks().len(), 1);
        assert_eq!(
            diff.snapshot().visible_hunks(&working)[0].staging,
            DiffHunkStaging::StagingPending,
            "无关源编辑不应提前清除暂存状态"
        );
    });
    diff.update(cx, |diff, cx| {
        diff.set_revisions(
            Some(Arc::from("a\nold\nz\n")),
            Some(Arc::from("a\nchanged\nz\n")),
            cx,
        );
    });
    cx.run_until_parked();
    diff.read_with(cx, |diff, _| {
        assert!(diff.snapshot().pending_hunks().is_empty());
    });
}

#[gpui::test]
fn pending_stage_survives_edit_log_eviction_after_unrelated_edits(cx: &mut TestAppContext) {
    let mut config = BufferConfig::default();
    config.large_file.max_edit_history_entries = 1;
    let buffer = Buffer::from_text("a\nchanged\nz\n".into(), config).unwrap();
    let registry = Arc::new(LanguageRegistry::new());
    let working =
        cx.new(|cx| LanguageBuffer::new(buffer, Some(PathBuf::from("src/a.rs")), registry, cx));
    let diff = cx.new(|cx| {
        BufferDiff::new(
            buffer_diff_input(working.clone(), Some("a\nold\nz\n"), "src/a.rs"),
            cx,
        )
    });
    cx.run_until_parked();
    diff.update(cx, |diff, cx| {
        let hunk = diff.snapshot().hunks().next().unwrap().clone();
        let version = diff.working().read(cx).text_snapshot().version();
        diff.set_pending_hunks(vec![PendingHunk::set_staging(&hunk, version, true)], cx);
    });

    for _ in 0..3 {
        working.update(cx, |working, cx| {
            let end = working.text_snapshot().len_bytes();
            working
                .edit(
                    [Edit::insert(end, "tail\n").unwrap()],
                    TransactionMetadata::default(),
                    cx,
                )
                .unwrap();
        });
        cx.run_until_parked();
    }
    diff.read_with(cx, |diff, cx| {
        let working = diff.working().read(cx).text_snapshot();
        assert_eq!(diff.snapshot().pending_hunks().len(), 1);
        assert_eq!(
            diff.snapshot().visible_hunks(&working)[0].staging,
            DiffHunkStaging::StagingPending,
            "编辑日志裁剪后，未触及的 hunk 仍应保持 pending 状态"
        );
    });
}

#[gpui::test]
fn pending_restore_expires_after_editing_its_working_range(cx: &mut TestAppContext) {
    let working = language_buffer("a\nchanged\nz\n", "src/a.rs", cx);
    let diff = cx.new(|cx| {
        BufferDiff::new(
            buffer_diff_input(working.clone(), Some("a\nold\nz\n"), "src/a.rs"),
            cx,
        )
    });
    cx.run_until_parked();
    diff.update(cx, |diff, cx| {
        let hunk = diff
            .snapshot()
            .hunks()
            .next()
            .expect("必须有变更块")
            .clone();
        let version = diff.working().read(cx).text_snapshot().version();
        diff.set_pending_hunks(vec![PendingHunk::suppress(&hunk, version)], cx);
    });
    working.update(cx, |working, cx| {
        working
            .edit(
                [Edit::insert(ByteOffset::new(3), "x").expect("块内插入有效")],
                TransactionMetadata::default(),
                cx,
            )
            .expect("应编辑工作区");
    });
    cx.run_until_parked();
    diff.read_with(cx, |diff, cx| {
        let working = diff.working().read(cx).text_snapshot();
        assert!(diff.snapshot().pending_hunks().is_empty());
        assert_eq!(diff.snapshot().visible_hunks(&working).len(), 1);
    });
}

#[gpui::test]
fn repeated_pending_on_pure_deletion_replaces_the_previous_state(cx: &mut TestAppContext) {
    let working = language_buffer("a\nc\n", "src/a.rs", cx);
    let diff = cx.new(|cx| {
        BufferDiff::new(
            buffer_diff_input(working, Some("a\nb\nc\n"), "src/a.rs"),
            cx,
        )
    });
    cx.run_until_parked();
    diff.update(cx, |diff, cx| {
        let hunk = diff
            .snapshot()
            .hunks()
            .next()
            .expect("必须有删除块")
            .clone();
        let version = diff.working().read(cx).text_snapshot().version();
        diff.set_pending_hunks(vec![PendingHunk::set_staging(&hunk, version, true)], cx);
        diff.set_pending_hunks(vec![PendingHunk::set_staging(&hunk, version, false)], cx);
    });
    diff.read_with(cx, |diff, cx| {
        let working = diff.working().read(cx).text_snapshot();
        assert_eq!(diff.snapshot().pending_hunks().len(), 1);
        assert_eq!(
            diff.snapshot().visible_hunks(&working)[0].staging,
            DiffHunkStaging::UnstagingPending
        );
    });
}

#[gpui::test]
fn pending_hunks_sort_by_current_anchors_after_prefix_edit(cx: &mut TestAppContext) {
    let working = language_buffer("a\nchanged-one\nmiddle\nchanged-two\nz\n", "src/a.rs", cx);
    let diff = cx.new(|cx| {
        BufferDiff::new(
            buffer_diff_input(
                working.clone(),
                Some("a\nold-one\nmiddle\nold-two\nz\n"),
                "src/a.rs",
            ),
            cx,
        )
    });
    cx.run_until_parked();
    let (first, second) = diff.read_with(cx, |diff, _| {
        let hunks = diff.snapshot().hunks().cloned().collect::<Vec<_>>();
        assert_eq!(hunks.len(), 2);
        (hunks[0].clone(), hunks[1].clone())
    });
    diff.update(cx, |diff, cx| {
        let version = diff.working().read(cx).text_snapshot().version();
        diff.set_pending_hunks(vec![PendingHunk::set_staging(&second, version, true)], cx);
    });

    working.update(cx, |working, cx| {
        working
            .edit(
                [Edit::insert(ByteOffset::ZERO, "long-prefix-before-both-hunks\n").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .unwrap();
    });
    let current = cx.read_entity(&working, |working, _| working.text_snapshot());
    let mut fresh_first = first;
    fresh_first.buffer_range = current
        .anchor_before(fresh_first.buffer_range.start.resolve_in(&current).unwrap())
        ..current.anchor_before(fresh_first.buffer_range.end.resolve_in(&current).unwrap());
    diff.update(cx, |diff, cx| {
        diff.set_pending_hunks(
            vec![PendingHunk::set_staging(
                &fresh_first,
                current.version(),
                true,
            )],
            cx,
        );
    });

    diff.read_with(cx, |diff, _| {
        let pending = diff.snapshot().pending_hunks();
        assert_eq!(pending.len(), 2);
        let starts = pending
            .iter()
            .map(|hunk| hunk.buffer_range.start.resolve_in(&current).unwrap())
            .collect::<Vec<_>>();
        assert!(starts[0] < starts[1], "pending 必须按当前锚点顺序排列");
    });
}

#[gpui::test]
fn hunk_boundaries_remain_ordered_during_source_edits(cx: &mut TestAppContext) {
    for (text, end) in [("a\nb\nc", 2), ("a\nedited\nb\nc", 9)] {
        let working = language_buffer(text, "src/a.rs", cx);
        let diff = cx.new(|cx| {
            BufferDiff::new(
                buffer_diff_input(working.clone(), Some("a\nold1\nold2\nb\nc"), "src/a.rs"),
                cx,
            )
        });
        cx.run_until_parked();
        let hunk = diff.read_with(cx, |diff, _| {
            assert_eq!(diff.snapshot().hunk_count(), 1);
            diff.snapshot().hunks().next().unwrap().clone()
        });

        working.update(cx, |working, cx| {
            working
                .edit(
                    [Edit::insert(ByteOffset::new(2), "more\n").unwrap()],
                    TransactionMetadata::default(),
                    cx,
                )
                .unwrap();
            let snapshot = working.text_snapshot();
            assert_eq!(
                hunk.buffer_range.start.resolve_in(&snapshot).unwrap(),
                ByteOffset::new(2),
                "起点插入不能把已有区块身份推到插入内容之后"
            );
            assert_eq!(
                hunk.buffer_range.end.resolve_in(&snapshot).unwrap(),
                ByteOffset::new(if end == 2 { 2 } else { end + 5 }),
                "纯删除区块在后台重算前仍须保持空范围"
            );
        });
        cx.run_until_parked();
        let hunk = diff.read_with(cx, |diff, _| {
            diff.snapshot().hunks().next().unwrap().clone()
        });
        working.update(cx, |working, cx| {
            let end = hunk
                .buffer_range
                .end
                .resolve_in(&working.text_snapshot())
                .unwrap();
            working
                .edit(
                    [Edit::insert(end, "after\n").unwrap()],
                    TransactionMetadata::default(),
                    cx,
                )
                .unwrap();
            assert_eq!(
                hunk.buffer_range
                    .end
                    .resolve_in(&working.text_snapshot())
                    .unwrap(),
                end,
                "半开区块范围不吸收终点插入"
            );
        });
        cx.run_until_parked();
    }
}

#[gpui::test]
fn revision_inputs_and_hunks_are_published_together(cx: &mut TestAppContext) {
    let working = language_buffer("工作区\n", "src/a.rs", cx);
    let diff =
        cx.new(|cx| BufferDiff::new(buffer_diff_input(working, Some("旧基线\n"), "src/a.rs"), cx));
    cx.run_until_parked();
    let published = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let observed = published.clone();
    let _subscription = cx.update(|cx| {
        cx.subscribe(&diff, move |diff, _, cx| {
            observed.store(true, std::sync::atomic::Ordering::SeqCst);
            diff.read_with(cx, |diff, cx| {
                assert_eq!(
                    super::full_text(&diff.base_source().unwrap().read(cx).text_snapshot()),
                    "新基线\n"
                );
                assert_eq!(
                    super::full_text(&diff.index_source().unwrap().read(cx).text_snapshot()),
                    "工作区\n"
                );
                assert!(diff.is_current_version_calculated(cx));
                assert!(
                    diff.snapshot()
                        .hunks()
                        .all(|hunk| hunk.staging == crate::DiffHunkStaging::Staged)
                );
            });
        })
    });
    diff.update(cx, |diff, cx| {
        diff.set_revisions(Some(Arc::from("新基线\n")), Some(Arc::from("工作区\n")), cx)
    });
    diff.read_with(cx, |diff, cx| {
        assert_eq!(
            super::full_text(&diff.base_source().unwrap().read(cx).text_snapshot()),
            "旧基线\n",
            "在途计算保留上一批完整输入"
        );
        assert!(diff.index_source().is_none());
    });
    cx.run_until_parked();
    assert!(
        published.load(std::sync::atomic::Ordering::SeqCst),
        "必须发布新的差异结果"
    );
}
