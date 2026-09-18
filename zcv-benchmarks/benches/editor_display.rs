//! Editor / DisplayMap 的编辑、折叠与换行热路径基准。
//!
//! 覆盖连续输入、长行编辑、整文件折叠切换与软换行切换：这些都是显示投影必须增量推进的场景，
//! 用于观察整段物化或全量重建是否重新出现。

use std::path::PathBuf;
use std::sync::Arc;

use criterion::{BenchmarkId, Criterion, Throughput, black_box, criterion_group, criterion_main};
use gpui::{AppContext as _, Entity, TestAppContext, TestDispatcher};
use zcv_benchmarks::cached_rust_document;
use zcv_editor::{Editor, SoftWrap};
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_multi_buffer::MultiBuffer;
use zcv_text::{Buffer, BufferConfig, ByteOffset, Edit, Line, TransactionMetadata};

const DOC_BYTES: usize = 256 * 1024;
const LONG_LINE_ROWS: usize = 2_000;
const LONG_LINE_COLUMNS: usize = 400;

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
    buffer: Entity<Buffer>,
    cx: TestAppContext,
}

fn fixture(text: String) -> Fixture {
    let mut cx = TestAppContext::build(TestDispatcher::new(1), None);
    let buffer = cx.new(|_| {
        Buffer::from_text(text, BufferConfig::default()).expect("基准文档应能创建 Buffer")
    });
    let language = cx.new(|cx| {
        LanguageBuffer::new(
            buffer.clone(),
            Some(PathBuf::from("src/main.rs")),
            Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let multi_buffer = cx.new(|cx| MultiBuffer::singleton(language, cx));
    let editor = cx.new(|cx| Editor::for_multi_buffer(multi_buffer, cx));
    cx.run_until_parked();
    Fixture { editor, buffer, cx }
}

/// 取文档中部一个合法编辑位置。
///
/// 基准文档含多字节字符，直接用字节长度的一半会落在字符中间；
/// 这里取中间行的行首，保证是字符边界。
fn middle_offset(fixture: &Fixture) -> usize {
    fixture.cx.read_entity(&fixture.buffer, |buffer, _| {
        let snapshot = buffer.snapshot();
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
            fixture.cx.update_entity(&fixture.buffer, |buffer, _| {
                let offset = ByteOffset::new(midpoint.min(buffer.len_bytes().get()));
                buffer
                    .edit(
                        [Edit::insert(offset, "x").expect("插入编辑必须合法")],
                        TransactionMetadata::default(),
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
            fixture.cx.update_entity(&fixture.buffer, |buffer, _| {
                let offset = ByteOffset::new(buffer.len_bytes().get().min(midpoint));
                buffer
                    .edit(
                        [Edit::insert(offset, "x").expect("插入编辑必须合法")],
                        TransactionMetadata::default(),
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
    let path = PathBuf::from("src/main.rs");
    group.bench_function("whole_file", |b| {
        b.iter(|| {
            fixture.cx.update_entity(&fixture.editor, |editor, cx| {
                editor.toggle_buffer_fold(path.clone(), cx);
            });
            fixture.cx.run_until_parked();
            black_box(fixture.editor.entity_id());
        });
    });
    group.finish();
}

/// 频繁换行：在窗口内反复切换软换行模式，测量下一帧的换行重排。
fn wrap_toggle(c: &mut Criterion) {
    let mut setup = TestAppContext::build(TestDispatcher::new(1), None);
    let mode = std::env::var("ZCV_BENCH_WRAP").unwrap_or_else(|_| "editor_width".to_owned());
    let text = if mode == "long_line" {
        long_line_document()
    } else {
        rust_document()
    };
    let buffer = setup.new(|_| {
        Buffer::from_text(text, BufferConfig::default()).expect("基准文档应能创建 Buffer")
    });
    let language = setup.new(|cx| {
        LanguageBuffer::new(
            buffer,
            Some(PathBuf::from("src/main.rs")),
            Arc::new(LanguageRegistry::new()),
            cx,
        )
    });
    let multi_buffer = setup.new(|cx| MultiBuffer::singleton(language, cx));
    let (editor, cx) = setup.add_window_view({
        let multi_buffer = multi_buffer.clone();
        move |_, cx| Editor::for_multi_buffer(multi_buffer, cx)
    });
    cx.run_until_parked();
    cx.refresh().expect("首帧布局应成功");

    let mut group = c.benchmark_group("editor/wrap_toggle");
    group.bench_function(BenchmarkId::from_parameter(&mode), |b| {
        b.iter(|| {
            cx.update_entity(&editor, |editor, cx| {
                editor.set_soft_wrap_mode(Some(SoftWrap::EditorWidth), cx);
            });
            cx.refresh().expect("换行帧应成功");
            cx.update_entity(&editor, |editor, cx| {
                editor.set_soft_wrap_mode(Some(SoftWrap::None), cx);
            });
            cx.refresh().expect("取消换行帧应成功");
            black_box(editor.entity_id());
        });
    });
    group.finish();
}

criterion_group!(
    editor_display_benches,
    continuous_input,
    long_line_edit,
    fold_toggle,
    wrap_toggle
);
criterion_main!(editor_display_benches);
