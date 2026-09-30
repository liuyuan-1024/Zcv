//! Editor / DisplayMap 的编辑、折叠与换行热路径基准。
//!
//! 覆盖连续输入、长行编辑与整文件折叠切换：这些都是显示投影必须增量推进的场景，
//! 用于观察整段物化或全量重建是否重新出现。
//! diff 滚动基准使用 GPUI 测试文本系统，默认关闭软换行；
//! 输入到绘制不包含系统输入队列或 GPU 呈现。软换行测量见本 crate 的 README。

use std::ops::Range;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use gpui::{
    AppContext as _, AvailableSpace, Entity, IntoElement as _, MouseMoveEvent, ScrollDelta,
    ScrollWheelEvent, TestAppContext, TestDispatcher, point, px, size,
};
mod common;

use common::cached_rust_document;
use zcv_buffer_diff::{BufferDiff, BufferDiffInput};
use zcv_editor::Editor;
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_multi_buffer::{DiffExcerptRanges, DiffFile, ExcerptRange, MultiBuffer};
use zcv_text::{Buffer, BufferConfig, ByteOffset, Edit, Line, TransactionMetadata};

const DOC_BYTES: usize = 256 * 1024;
const EXCERPT_DOC_BYTES: usize = 16 * 1024;
const MULTI_EXCERPT_COUNTS: [usize; 3] = [2, 32, 256];
const LONG_LINE_ROWS: usize = 2_000;
const LONG_LINE_COLUMNS: usize = 400;
const DIFF_SCROLL_FILE_COUNTS: [usize; 2] = [2, 16];
const DIFF_SCROLL_LINES_PER_FILE: usize = 1_536;
const DIFF_SCROLL_ADDED_LINES: usize = 640;
const DIFF_SCROLL_SCENARIOS: [(&str, usize, usize); 2] =
    [("word_diff", 8, 32), ("hunk_dense", 1, 8)];
const LARGE_ADDITION_SCENARIO: (&str, usize, usize) = (
    "large_addition",
    DIFF_SCROLL_ADDED_LINES,
    DIFF_SCROLL_LINES_PER_FILE,
);
const LARGE_ADDITION_FILE_COUNTS: [usize; 2] = [16, 64];

fn rust_document() -> String {
    cached_rust_document(DOC_BYTES).to_string()
}

fn long_line_document() -> String {
    let line = "abcdefghij".repeat(LONG_LINE_COLUMNS / 10);
    let mut text = String::with_capacity((line.len() + 1) * LONG_LINE_ROWS);
    for _ in 0..LONG_LINE_ROWS {
        text.push_str(&line);
        text.push('\n');
    }
    text
}

// 基准夹具：实体字段必须声明在 cx 之前，保证实体先于测试上下文释放。
struct Fixture {
    editor: Entity<Editor>,
    source: Entity<LanguageBuffer>,
    cx: TestAppContext,
}

fn fixture(text: String) -> Fixture {
    let mut cx = TestAppContext::build(TestDispatcher::new(1), None);
    let buffer = Buffer::from_text(text, BufferConfig::default()).expect("基准文档应能创建 Buffer");
    let source = cx.new(|cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from("src/main.rs")),
            Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let multi_buffer = cx.new(|cx| MultiBuffer::singleton(source.clone(), cx));
    let editor = cx.new(|cx| Editor::for_multi_buffer(multi_buffer, cx));
    cx.run_until_parked();
    Fixture { editor, source, cx }
}

/// 取文档中部一个合法编辑位置。
///
/// 基准文档含多字节字符，直接用字节长度的一半会落在字符中间；
/// 这里取中间行的行首，保证是字符边界。
fn middle_offset(fixture: &Fixture) -> usize {
    fixture.cx.read_entity(&fixture.source, |source, _| {
        let snapshot = source.text_snapshot();
        snapshot
            .line_start_byte(Line::new(snapshot.line_count() / 2))
            .unwrap_or_else(|_| snapshot.len_bytes())
            .get()
    })
}

/// 连续输入：每次在文档中部插入一个字符并推进显示投影。
fn continuous_input(c: &mut Criterion) {
    let mut group = c.benchmark_group("editor/continuous_input");
    let mut fixture = fixture(rust_document());
    let midpoint = middle_offset(&fixture);
    group.throughput(Throughput::Bytes(1));
    group.bench_function("insert_char_at_middle", |b| {
        b.iter(|| {
            fixture.cx.update_entity(&fixture.source, |source, cx| {
                let offset = ByteOffset::new(midpoint.min(source.len_bytes().get()));
                source
                    .edit(
                        [Edit::insert(offset, "x").expect("插入编辑必须合法")],
                        TransactionMetadata::default(),
                        cx,
                    )
                    .expect("连续输入应成功");
            });
            fixture.cx.run_until_parked();
            black_box(fixture.editor.entity_id());
        });
    });
    group.finish();
}

/// 长行文档：在超长行上的编辑同样只应推进受影响显示行。
fn long_line_edit(c: &mut Criterion) {
    let mut group = c.benchmark_group("editor/long_line_edit");
    let mut fixture = fixture(long_line_document());
    let midpoint = middle_offset(&fixture);
    group.throughput(Throughput::Bytes(1));
    group.bench_function("insert_char_at_middle", |b| {
        b.iter(|| {
            fixture.cx.update_entity(&fixture.source, |source, cx| {
                let offset = ByteOffset::new(source.len_bytes().get().min(midpoint));
                source
                    .edit(
                        [Edit::insert(offset, "x").expect("插入编辑必须合法")],
                        TransactionMetadata::default(),
                        cx,
                    )
                    .expect("长行输入应成功");
            });
            fixture.cx.run_until_parked();
            black_box(fixture.editor.entity_id());
        });
    });
    group.finish();
}

/// 频繁折叠：整文件 BlockMap 折叠变换在折叠 / 展开之间反复切换。
fn fold_toggle(c: &mut Criterion) {
    let mut group = c.benchmark_group("editor/fold_toggle");
    let mut fixture = fixture(rust_document());
    let buffer_id = fixture
        .cx
        .read_entity(&fixture.source, |source, _| source.buffer_id());
    group.bench_function("whole_file", |b| {
        b.iter(|| {
            fixture.cx.update_entity(&fixture.editor, |editor, cx| {
                editor.toggle_buffer_fold(buffer_id, cx);
            });
            fixture.cx.run_until_parked();
            black_box(fixture.editor.entity_id());
        });
    });
    group.finish();
}

/// 多 excerpt 组合文档的编辑帧：编辑一个源后整帧刷新。
///
/// 锁定「显示帧内不存在随文档规模增长的派生」：每帧时间应随 excerpt 数基本不变。
/// 编辑一个源并推进显示同步，但不强制整帧刷新：隔离编辑/同步成本。
fn multi_excerpt_edit_only(c: &mut Criterion) {
    let mut group = c.benchmark_group("editor/multi_excerpt_edit_only");
    for excerpt_count in MULTI_EXCERPT_COUNTS {
        let mut cx = TestAppContext::build(TestDispatcher::new(1), None);
        let sources = (0..excerpt_count)
            .map(|index| {
                let buffer = Buffer::from_text(
                    cached_rust_document(EXCERPT_DOC_BYTES).to_string(),
                    BufferConfig::default(),
                )
                .expect("基准文档应能创建 Buffer");
                cx.new(|cx| {
                    LanguageBuffer::new(
                        buffer,
                        Some(PathBuf::from(format!("src/f{index}.rs"))),
                        Arc::new(LanguageRegistry::new()),
                        cx,
                    )
                })
            })
            .collect::<Vec<_>>();
        let source = sources[0].clone();
        let multi_buffer = cx.new(MultiBuffer::empty);
        cx.update_entity(&multi_buffer, |buffer, cx| {
            for source in sources {
                let line_count = source.read(cx).text_snapshot().line_count();
                buffer.set_excerpts_for_path(
                    vec![ExcerptRange::line_range(source, 0..line_count, cx)],
                    cx,
                );
            }
        });
        let (editor, cx) =
            cx.add_window_view(move |_, cx| Editor::for_multi_buffer(multi_buffer, cx));
        cx.run_until_parked();
        cx.refresh().expect("组合文档窗口应可刷新");
        group.bench_function(format!("{excerpt_count}"), |b| {
            b.iter(|| {
                cx.update_entity(&source, |source, cx| {
                    source
                        .edit(
                            [Edit::insert(ByteOffset::ZERO, "x").expect("插入编辑必须合法")],
                            TransactionMetadata::default(),
                            cx,
                        )
                        .expect("组合文档源编辑应成功");
                });
                cx.run_until_parked();
                black_box(editor.entity_id());
            });
        });
    }
    group.finish();
}

/// 只推进 MultiBuffer，不创建 Editor 和显示投影，用于隔离组合模型同步成本。
fn multi_excerpt_model_edit_only(c: &mut Criterion) {
    let mut group = c.benchmark_group("editor/multi_excerpt_model_edit_only");
    for excerpt_count in MULTI_EXCERPT_COUNTS {
        let mut cx = TestAppContext::build(TestDispatcher::new(1), None);
        let sources = (0..excerpt_count)
            .map(|index| {
                let buffer = Buffer::from_text(
                    cached_rust_document(EXCERPT_DOC_BYTES).to_string(),
                    BufferConfig::default(),
                )
                .expect("基准文档应能创建 Buffer");
                cx.new(|cx| {
                    LanguageBuffer::new(
                        buffer,
                        Some(PathBuf::from(format!("src/f{index}.rs"))),
                        Arc::new(LanguageRegistry::new()),
                        cx,
                    )
                })
            })
            .collect::<Vec<_>>();
        let source = sources[0].clone();
        let multi_buffer = cx.new(MultiBuffer::empty);
        cx.update_entity(&multi_buffer, |buffer, cx| {
            for source in sources {
                let line_count = source.read(cx).text_snapshot().line_count();
                buffer.set_excerpts_for_path(
                    vec![ExcerptRange::line_range(source, 0..line_count, cx)],
                    cx,
                );
            }
        });
        cx.run_until_parked();
        cx.update_entity(&multi_buffer, |buffer, cx| {
            let _ = buffer.snapshot(cx);
        });
        group.bench_function(format!("{excerpt_count}"), |b| {
            b.iter(|| {
                cx.update_entity(&source, |source, cx| {
                    source
                        .edit(
                            [Edit::insert(ByteOffset::ZERO, "x").expect("插入编辑必须合法")],
                            TransactionMetadata::default(),
                            cx,
                        )
                        .expect("组合文档源编辑应成功");
                });
                cx.run_until_parked();
                cx.update_entity(&multi_buffer, |buffer, cx| {
                    black_box(buffer.snapshot(cx));
                });
            });
        });
    }
    group.finish();
}

/// 不编辑，只整帧刷新：隔离渲染帧成本。
fn multi_excerpt_idle_frame(c: &mut Criterion) {
    let mut group = c.benchmark_group("editor/multi_excerpt_idle_frame");
    for excerpt_count in MULTI_EXCERPT_COUNTS {
        let mut cx = TestAppContext::build(TestDispatcher::new(1), None);
        let sources = (0..excerpt_count)
            .map(|index| {
                let buffer = Buffer::from_text(
                    cached_rust_document(EXCERPT_DOC_BYTES).to_string(),
                    BufferConfig::default(),
                )
                .expect("基准文档应能创建 Buffer");
                cx.new(|cx| {
                    LanguageBuffer::new(
                        buffer,
                        Some(PathBuf::from(format!("src/f{index}.rs"))),
                        Arc::new(LanguageRegistry::new()),
                        cx,
                    )
                })
            })
            .collect::<Vec<_>>();
        let source = sources[0].clone();
        let multi_buffer = cx.new(MultiBuffer::empty);
        cx.update_entity(&multi_buffer, |buffer, cx| {
            for source in sources {
                let line_count = source.read(cx).text_snapshot().line_count();
                buffer.set_excerpts_for_path(
                    vec![ExcerptRange::line_range(source, 0..line_count, cx)],
                    cx,
                );
            }
        });
        let (editor, cx) =
            cx.add_window_view(move |_, cx| Editor::for_multi_buffer(multi_buffer, cx));
        cx.run_until_parked();
        cx.refresh().expect("组合文档窗口应可刷新");
        let _ = &source;
        group.bench_function(format!("{excerpt_count}"), |b| {
            b.iter(|| {
                cx.refresh().expect("组合文档空闲帧应可刷新");
                black_box(editor.entity_id());
            });
        });
    }
    group.finish();
}

/// 多 excerpt 组合文档的滚动帧：走真实的 layout→prepaint→paint。
///
/// 锁定「滚动帧成本不随组合文档规模增长」：滚动只改变滚动位置，
/// 悬浮标题、折叠入口与滚动条标记都必须按视口求解，而不是随文件 / hunk 数遍历。
fn multi_excerpt_scroll_frame(c: &mut Criterion) {
    let mut group = c.benchmark_group("editor/multi_excerpt_scroll_render_frame");
    group.sample_size(30);
    group.measurement_time(Duration::from_secs(2));
    group.warm_up_time(Duration::from_secs(1));
    for excerpt_count in MULTI_EXCERPT_COUNTS {
        let mut cx = TestAppContext::build(TestDispatcher::new(1), None);
        let sources = (0..excerpt_count)
            .map(|index| {
                let buffer = Buffer::from_text(
                    cached_rust_document(EXCERPT_DOC_BYTES).to_string(),
                    BufferConfig::default(),
                )
                .expect("基准文档应能创建 Buffer");
                cx.new(|cx| {
                    LanguageBuffer::new(
                        buffer,
                        Some(PathBuf::from(format!("src/f{index}.rs"))),
                        Arc::new(LanguageRegistry::new()),
                        cx,
                    )
                })
            })
            .collect::<Vec<_>>();
        let multi_buffer = cx.new(MultiBuffer::empty);
        cx.update_entity(&multi_buffer, |buffer, cx| {
            for source in sources {
                let line_count = source.read(cx).text_snapshot().line_count();
                buffer.set_excerpts_for_path(
                    vec![ExcerptRange::line_range(source, 0..line_count, cx)],
                    cx,
                );
            }
        });
        let (editor, cx) =
            cx.add_window_view(move |_, cx| Editor::for_multi_buffer(multi_buffer, cx));
        cx.run_until_parked();
        let origin = point(px(0.), px(0.));
        let space = size(
            AvailableSpace::Definite(px(1200.)),
            AvailableSpace::Definite(px(800.)),
        );
        cx.draw(origin, space, |_, _cx| editor.clone().into_any_element());
        // 先滚到底部：滚动帧在组合文档底部最容易暴露随位置或总规模增长的视口查询。
        cx.simulate_event(ScrollWheelEvent {
            position: point(px(600.), px(400.)),
            delta: ScrollDelta::Pixels(point(px(0.), px(-10_000_000.))),
            ..Default::default()
        });
        cx.run_until_parked();
        group.bench_function(format!("{excerpt_count}"), |b| {
            b.iter_custom(|iterations| {
                let mut elapsed = Duration::ZERO;
                for _ in 0..iterations {
                    cx.simulate_event(ScrollWheelEvent {
                        position: point(px(600.), px(400.)),
                        delta: ScrollDelta::Pixels(point(px(0.), px(24.))),
                        ..Default::default()
                    });
                    let started = std::time::Instant::now();
                    cx.draw(origin, space, |_, _cx| editor.clone().into_any_element());
                    elapsed += started.elapsed();
                    black_box(editor.entity_id());
                }
                elapsed
            });
        });
    }
    group.finish();
}

fn diff_scroll_documents(
    changed_lines: usize,
    block_spacing: usize,
) -> (String, String, Vec<Range<usize>>) {
    let mut base = String::new();
    let mut working = String::new();
    let mut windows = Vec::new();
    for line in 0..DIFF_SCROLL_LINES_PER_FILE {
        let changed_line = line % block_spacing < changed_lines;
        if changed_line {
            base.push_str(&format!("let old_{line:05} = aa + bb + cc + dd + ee;\n"));
            working.push_str(&format!("let new_{line:05} = uu + vv + ww + xx + yy;\n"));
        } else {
            let text = format!("let value_{line:05} = 0;\n");
            base.push_str(&text);
            working.push_str(&text);
        }

        if line % block_spacing == changed_lines - 1 {
            let block_start = line + 1 - changed_lines;
            windows.push(block_start.saturating_sub(3)..(line + 4).min(DIFF_SCROLL_LINES_PER_FILE));
        }
    }
    (base, working, windows)
}

fn diff_scroll_addition_documents() -> (String, String, Vec<Range<usize>>) {
    let insertion_row = DIFF_SCROLL_LINES_PER_FILE / 2;
    let mut base = String::new();
    let mut working = String::new();
    // 单个新增窗口；用 once 显式构造单元素 Vec，避免单元素 vec! 触发 clippy 误报。
    let windows = std::iter::once(
        insertion_row.saturating_sub(3)..(insertion_row + DIFF_SCROLL_ADDED_LINES + 3),
    )
    .collect();
    for line in 0..DIFF_SCROLL_LINES_PER_FILE {
        if line == insertion_row {
            for added_line in 0..DIFF_SCROLL_ADDED_LINES {
                working.push_str(&format!(
                    "let added_{added_line:05} = value_{added_line:05} + 1;\n"
                ));
            }
        }
        let text = format!("let value_{line:05} = 0;\n");
        base.push_str(&text);
        working.push_str(&text);
    }
    (base, working, windows)
}

/// 暂存与未暂存多文件 diff 组合文档的 Editor 滚动绘制耗时。
///
/// 覆盖分散小 hunk、密集 hunk 与单个 640 行新增块，并分别观察绘制与滚轮输入到绘制的耗时。
fn diff_scroll_frame(c: &mut Criterion) {
    diff_scroll_frame_scenarios(c, &DIFF_SCROLL_SCENARIOS, &DIFF_SCROLL_FILE_COUNTS);
}

fn diff_scroll_large_addition_frame(c: &mut Criterion) {
    diff_scroll_frame_scenarios(
        c,
        std::slice::from_ref(&LARGE_ADDITION_SCENARIO),
        &LARGE_ADDITION_FILE_COUNTS,
    );
}

fn diff_scroll_frame_scenarios(
    c: &mut Criterion,
    scenarios: &[(&str, usize, usize)],
    file_counts: &[usize],
) {
    let mut group = c.benchmark_group("editor/diff_scroll_render_frame");
    group.sample_size(30);
    group.measurement_time(Duration::from_secs(2));
    group.warm_up_time(Duration::from_secs(1));

    for (scenario, changed_lines, block_spacing) in scenarios.iter().copied() {
        let large_file_documents = if scenario == "large_addition" {
            diff_scroll_addition_documents()
        } else {
            diff_scroll_documents(changed_lines, block_spacing)
        };
        let small_file_documents =
            (scenario == "large_addition").then(|| diff_scroll_documents(8, 32));
        for (staging, staged) in [("staged", true), ("unstaged", false)] {
            for file_count in file_counts.iter().copied() {
                let mut cx = TestAppContext::build(TestDispatcher::new(1), None);
                let language_registry = Arc::new(LanguageRegistry::new());
                let files = (0..file_count)
                    .map(|index| {
                        let (base_text, working_text, windows) = small_file_documents
                            .as_ref()
                            .filter(|_| index + 1 != file_count)
                            .unwrap_or(&large_file_documents);
                        let index_text = if staged { working_text } else { base_text };
                        let path = PathBuf::from(format!("src/file_{index}.rs"));
                        let source = cx.new(|cx| {
                            let buffer =
                                Buffer::from_text(working_text.clone(), BufferConfig::default())
                                    .expect("基准工作区文本应能创建 Buffer");
                            LanguageBuffer::new(
                                buffer,
                                Some(path.clone()),
                                Arc::clone(&language_registry),
                                cx,
                            )
                        });
                        let diff = cx.new(|cx| {
                            BufferDiff::new(
                                BufferDiffInput {
                                    working: source,
                                    path: path.clone(),
                                    base_text: Some(Arc::from(base_text.clone())),
                                    index_text: Some(Arc::from(index_text.clone())),
                                    language_registry: Arc::clone(&language_registry),
                                    key: index as u64,
                                    operations: None,
                                },
                                cx,
                            )
                        });
                        DiffFile {
                            diff,
                            display_path: path,
                            excerpt_ranges: DiffExcerptRanges::Windows(windows.clone()),
                        }
                    })
                    .collect::<Vec<_>>();
                let multi_buffer = cx.new(MultiBuffer::empty);
                cx.update_entity(&multi_buffer, |buffer, cx| {
                    buffer.set_diff_files(files, cx);
                    buffer.set_diff_hunks_expanded_by_default(true, cx);
                });
                cx.run_until_parked();
                let large_addition_row = if scenario == "large_addition" {
                    let path = PathBuf::from(format!("src/file_{}.rs", file_count - 1));
                    let snapshot =
                        cx.update_entity(&multi_buffer, |buffer, cx| buffer.snapshot(cx));
                    let insertion_row = DIFF_SCROLL_LINES_PER_FILE / 2;
                    let source_row = insertion_row + DIFF_SCROLL_ADDED_LINES / 2;
                    snapshot
                        .excerpts()
                        .filter(|excerpt| excerpt.path() == path)
                        .find(|excerpt| {
                            let source_start = excerpt.source_start_line().saturating_sub(1);
                            let source_end = excerpt
                                .source_line_for_output_line(
                                    excerpt.output_end_line().saturating_sub(1),
                                )
                                .map_or(source_start, |line| line.saturating_sub(1));
                            source_start <= source_row && source_row <= source_end
                        })
                        .map(|excerpt| {
                            excerpt.output_start_line()
                                + source_row
                                    .saturating_sub(excerpt.source_start_line().saturating_sub(1))
                        })
                } else {
                    None
                };
                let (editor, cx) =
                    cx.add_window_view(move |_, cx| Editor::for_multi_buffer(multi_buffer, cx));
                cx.run_until_parked();

                let origin = point(px(0.), px(0.));
                let space = size(
                    AvailableSpace::Definite(px(1200.)),
                    AvailableSpace::Definite(px(800.)),
                );
                cx.draw(origin, space, |_, _cx| editor.clone().into_any_element());
                cx.simulate_event(ScrollWheelEvent {
                    position: point(px(600.), px(400.)),
                    delta: large_addition_row.map_or_else(
                        || ScrollDelta::Pixels(point(px(0.), px(-10_000_000.))),
                        |row| ScrollDelta::Lines(point(0., -(row as f32))),
                    ),
                    ..Default::default()
                });
                cx.run_until_parked();
                if large_addition_row.is_some() {
                    cx.draw(origin, space, |_, _cx| editor.clone().into_any_element());
                }
                group.bench_function(
                    format!("{scenario}/{staging}/{file_count}/render_only"),
                    |b| {
                        b.iter_custom(|iterations| {
                            let mut elapsed = Duration::ZERO;
                            for frame in 0..iterations {
                                cx.simulate_event(ScrollWheelEvent {
                                    position: point(px(600.), px(400.)),
                                    delta: ScrollDelta::Pixels(point(
                                        px(0.),
                                        px(if frame % 2 == 0 { -24. } else { 24. }),
                                    )),
                                    ..Default::default()
                                });
                                let started = std::time::Instant::now();
                                cx.draw(origin, space, |_, _cx| editor.clone().into_any_element());
                                elapsed += started.elapsed();
                                black_box(editor.entity_id());
                            }
                            elapsed
                        });
                    },
                );
                group.bench_function(
                    format!("{scenario}/{staging}/{file_count}/input_to_frame"),
                    |b| {
                        b.iter_custom(|iterations| {
                            let mut elapsed = Duration::ZERO;
                            for frame in 0..iterations {
                                let started = std::time::Instant::now();
                                cx.simulate_event(ScrollWheelEvent {
                                    position: point(px(600.), px(400.)),
                                    delta: ScrollDelta::Pixels(point(
                                        px(0.),
                                        px(if frame % 2 == 0 { -24. } else { 24. }),
                                    )),
                                    ..Default::default()
                                });
                                cx.draw(origin, space, |_, _cx| editor.clone().into_any_element());
                                elapsed += started.elapsed();
                                black_box(editor.entity_id());
                            }
                            elapsed
                        });
                    },
                );
                if let Some(target_row) = large_addition_row {
                    group.bench_function(
                        format!("{scenario}/{staging}/{file_count}/jump_to_warmed_region"),
                        |b| {
                            b.iter_custom(|iterations| {
                                let mut elapsed = Duration::ZERO;
                                for _ in 0..iterations {
                                    cx.simulate_event(ScrollWheelEvent {
                                        position: point(px(600.), px(400.)),
                                        delta: ScrollDelta::Lines(point(0., 1_000_000.)),
                                        ..Default::default()
                                    });
                                    cx.draw(origin, space, |_, _cx| {
                                        editor.clone().into_any_element()
                                    });
                                    let started = std::time::Instant::now();
                                    cx.simulate_event(ScrollWheelEvent {
                                        position: point(px(600.), px(400.)),
                                        delta: ScrollDelta::Lines(point(0., -(target_row as f32))),
                                        ..Default::default()
                                    });
                                    cx.draw(origin, space, |_, _cx| {
                                        editor.clone().into_any_element()
                                    });
                                    elapsed += started.elapsed();
                                    black_box((editor.entity_id(), target_row));
                                }
                                elapsed
                            });
                        },
                    );

                    let gutter_position = point(px(20.), px(400.));
                    cx.simulate_event(MouseMoveEvent {
                        position: gutter_position,
                        ..Default::default()
                    });
                    cx.run_until_parked();
                    cx.draw(origin, space, |_, _cx| editor.clone().into_any_element());
                    group.bench_function(
                        format!("{scenario}/{staging}/{file_count}/gutter_hover_scroll"),
                        |b| {
                            b.iter_custom(|iterations| {
                                let mut elapsed = Duration::ZERO;
                                for frame in 0..iterations {
                                    let started = std::time::Instant::now();
                                    cx.simulate_event(ScrollWheelEvent {
                                        position: gutter_position,
                                        delta: ScrollDelta::Pixels(point(
                                            px(0.),
                                            px(if frame % 2 == 0 { -24. } else { 24. }),
                                        )),
                                        ..Default::default()
                                    });
                                    cx.draw(origin, space, |_, _cx| {
                                        editor.clone().into_any_element()
                                    });
                                    elapsed += started.elapsed();
                                    black_box(editor.entity_id());
                                }
                                elapsed
                            });
                        },
                    );
                }
            }
        }
    }

    group.finish();
}

/// 编辑一个源后整帧刷新：组合文档完整编辑帧。
fn multi_excerpt_edit_frame(c: &mut Criterion) {
    let mut group = c.benchmark_group("editor/multi_excerpt_edit_frame");
    for excerpt_count in MULTI_EXCERPT_COUNTS {
        let mut cx = TestAppContext::build(TestDispatcher::new(1), None);
        let sources = (0..excerpt_count)
            .map(|index| {
                let buffer = Buffer::from_text(
                    cached_rust_document(EXCERPT_DOC_BYTES).to_string(),
                    BufferConfig::default(),
                )
                .expect("基准文档应能创建 Buffer");
                cx.new(|cx| {
                    LanguageBuffer::new(
                        buffer,
                        Some(PathBuf::from(format!("src/f{index}.rs"))),
                        Arc::new(LanguageRegistry::new()),
                        cx,
                    )
                })
            })
            .collect::<Vec<_>>();
        let source = sources[0].clone();
        let multi_buffer = cx.new(MultiBuffer::empty);
        cx.update_entity(&multi_buffer, |buffer, cx| {
            for source in sources {
                let line_count = source.read(cx).text_snapshot().line_count();
                buffer.set_excerpts_for_path(
                    vec![ExcerptRange::line_range(source, 0..line_count, cx)],
                    cx,
                );
            }
        });
        let (editor, cx) =
            cx.add_window_view(move |_, cx| Editor::for_multi_buffer(multi_buffer, cx));
        cx.run_until_parked();
        cx.refresh().expect("组合文档窗口应可刷新");
        group.bench_function(format!("{excerpt_count}"), |b| {
            b.iter(|| {
                cx.update_entity(&source, |source, cx| {
                    source
                        .edit(
                            [Edit::insert(ByteOffset::ZERO, "x").expect("插入编辑必须合法")],
                            TransactionMetadata::default(),
                            cx,
                        )
                        .expect("组合文档源编辑应成功");
                });
                cx.run_until_parked();
                cx.refresh().expect("组合文档编辑帧应可刷新");
                black_box(editor.entity_id());
            });
        });
    }
    group.finish();
}

criterion_group!(
    editor_display_benches,
    continuous_input,
    long_line_edit,
    fold_toggle,
    multi_excerpt_edit_only,
    multi_excerpt_model_edit_only,
    multi_excerpt_idle_frame,
    multi_excerpt_scroll_frame,
    diff_scroll_frame,
    diff_scroll_large_addition_frame,
    multi_excerpt_edit_frame
);
criterion_main!(editor_display_benches);
