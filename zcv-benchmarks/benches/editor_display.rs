//! Editor / DisplayMap 的编辑、折叠与换行热路径基准。
//!
//! 覆盖连续输入、长行编辑与整文件折叠切换：这些都是显示投影必须增量推进的场景，
//! 用于观察整段物化或全量重建是否重新出现。

use std::path::PathBuf;
use std::sync::Arc;

use criterion::{Criterion, Throughput, black_box, criterion_group, criterion_main};
use gpui::{AppContext as _, Entity, TestAppContext, TestDispatcher};
mod common;

use common::cached_rust_document;
use zcv_editor::Editor;
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

criterion_group!(
    editor_display_benches,
    continuous_input,
    long_line_edit,
    fold_toggle
);
criterion_main!(editor_display_benches);
