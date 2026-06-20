//! Phase 4 等价性护栏：`BufferSyntaxTree::query_viewport` 在 viewport 上跑出的 spans
//! 等于「全文 query 再过滤到 viewport 区间」的子集。
//!
//! 替代 Phase 3 删掉的 `viewport_scoped_spans_equal_full_parse_filtered` ——
//! 那条是测 `HighlightWorker.viewport_hint`（已删）。新的不变量是 paint 端的
//! viewport-scoped Query 入口必须与全文 query 在同区间内**逐项一致**。

use std::rc::Rc;

use zom_engine::{Buffer, BufferConfig, ByteOffset, TextRange};
use zom_workspace::SyntaxDocument;
use zom_workspace::syntax::{
    LanguageId, SyntaxEngine, SyntaxQueryCursor, install_builtin_providers,
};

const SOURCE: &str = "fn a() -> i32 { let x = 1; x }\n\
                     fn b() -> i32 { let y = 2; y }\n\
                     fn c() -> i32 { let z = 3; z }\n\
                     fn d() -> i32 { 4 }\n\
                     fn e() -> i32 { 5 }\n";

fn engine_with_builtins() -> Rc<SyntaxEngine> {
    let mut engine = SyntaxEngine::new();
    install_builtin_providers(&mut engine);
    Rc::new(engine)
}

fn tuples_in_range(
    spans: &[(TextRange, zom_workspace::syntax::HighlightSpan)],
    range: TextRange,
) -> Vec<(usize, usize, &'static str)> {
    spans
        .iter()
        .filter(|(r, _)| r.start() >= range.start() && r.end() <= range.end())
        .map(|(r, s)| (r.start().get(), r.end().get(), s.name.as_str()))
        .collect()
}

#[test]
fn viewport_scoped_query_equals_full_query_filtered() {
    let engine = engine_with_builtins();
    let buffer = Buffer::from_text(SOURCE.to_string(), BufferConfig::default()).unwrap();
    let doc = SyntaxDocument::from_buffer(engine.clone(), buffer, LanguageId::new("rust"));
    engine.worker().wait_for_idle_for_test_or_bench();

    let slot = doc.highlights_slot().expect("rust 必须挂 provider");
    let highlights = slot.load().expect("首份 highlights 必须就位");

    // viewport 覆盖前两行——切到 fn c 之前。
    let cutoff = SOURCE.find("fn c").unwrap();
    let viewport = TextRange::new(ByteOffset::ZERO, ByteOffset::new(cutoff)).unwrap();
    let full = TextRange::new(ByteOffset::ZERO, doc.buffer().snapshot().len_bytes()).unwrap();

    let mut cursor = SyntaxQueryCursor::new();
    let viewport_spans = highlights.query_viewport(viewport, &mut cursor);
    let full_spans = highlights.query_viewport(full, &mut cursor);

    let mut viewport_tuples = tuples_in_range(&viewport_spans, viewport);
    let mut baseline = tuples_in_range(&full_spans, viewport);
    viewport_tuples.sort();
    baseline.sort();

    assert_eq!(
        viewport_tuples, baseline,
        "viewport-scoped query 必须等于全文 query 在同区间内的子集"
    );

    // 同时确认 viewport 段内的每个 span 起点都落在 viewport 内（不会泄露 set_byte_range 外的命中）。
    for (r, _) in &viewport_spans {
        assert!(
            r.start().get() < cutoff,
            "viewport-scoped span 不应越界：{r:?}"
        );
    }
}

#[test]
fn viewport_scoped_query_subset_when_viewport_in_middle() {
    let engine = engine_with_builtins();
    let buffer = Buffer::from_text(SOURCE.to_string(), BufferConfig::default()).unwrap();
    let doc = SyntaxDocument::from_buffer(engine.clone(), buffer, LanguageId::new("rust"));
    engine.worker().wait_for_idle_for_test_or_bench();

    let slot = doc.highlights_slot().unwrap();
    let highlights = slot.load().unwrap();

    // viewport 卡在第三行（fn c）—— 测中间区间的等价性。
    let start = SOURCE.find("fn c").unwrap();
    let end = SOURCE.find("fn e").unwrap();
    let viewport = TextRange::new(ByteOffset::new(start), ByteOffset::new(end)).unwrap();
    let full = TextRange::new(ByteOffset::ZERO, doc.buffer().snapshot().len_bytes()).unwrap();

    let mut cursor = SyntaxQueryCursor::new();
    let viewport_spans = highlights.query_viewport(viewport, &mut cursor);
    let full_spans = highlights.query_viewport(full, &mut cursor);

    let mut viewport_tuples = tuples_in_range(&viewport_spans, viewport);
    let mut baseline = tuples_in_range(&full_spans, viewport);
    viewport_tuples.sort();
    baseline.sort();

    assert_eq!(viewport_tuples, baseline);
}
