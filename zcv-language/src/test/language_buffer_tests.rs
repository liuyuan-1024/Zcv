use std::cell::RefCell;
use std::rc::Rc;

use gpui::TestAppContext;
use zcv_text::{BufferConfig, Edit, TransactionMetadata};

use super::*;

fn test_registry() -> Arc<LanguageRegistry> {
    Arc::new(LanguageRegistry::new())
}

fn test_buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_owned(), BufferConfig::default()).expect("应创建测试 Buffer")
}

#[test]
fn sync_parse_wait_returns_completed_result_within_timeout() {
    // 后台解析（真实线程）完成前主线程阻塞等待，完成后立即返回结果。
    let completion: Arc<ParseCompletion> = Arc::default();
    let worker = Arc::clone(&completion);
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(5));
        let (lock, cvar) = &*worker;
        *lock.lock().expect("解析完成信号锁不应中毒") =
            Some(SyntaxSnapshot::empty(BufferVersion::INITIAL));
        cvar.notify_one();
    });
    let outcome = wait_parse_completion(&completion, Duration::from_millis(100));
    assert!(outcome.is_some(), "已完成的解析应在超时前被主线程取到");
}

#[test]
fn sync_parse_wait_times_out_when_parse_is_slow() {
    // 超过预算的解析：等待超时返回 None，留给后台任务稍后经 Reparsed 安装。
    let completion: Arc<ParseCompletion> = Arc::default();
    let start = Instant::now();
    let outcome = wait_parse_completion(&completion, Duration::from_millis(10));
    assert!(outcome.is_none(), "慢解析等待应超时");
    assert!(
        start.elapsed() >= Duration::from_millis(8),
        "等待应消耗接近完整的预算"
    );
}

#[gpui::test]
fn parsing_finishes_without_blocking_buffer_edits(cx: &mut TestAppContext) {
    let language_buffer = cx.new(|cx| {
        LanguageBuffer::new(
            test_buffer("fn main() {}\n"),
            Some(PathBuf::from("main.rs")),
            test_registry(),
            cx,
        )
    });

    language_buffer.update(cx, |language_buffer, cx| {
        language_buffer
            .edit(
                [Edit::insert(ByteOffset::new(3), "async ").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("测试编辑应成功");
    });
    cx.run_until_parked();

    language_buffer.read_with(cx, |language_buffer, _| {
        let snapshot = language_buffer.snapshot();
        assert_eq!(snapshot.syntax.version(), snapshot.text.version());
    });
}

#[gpui::test]
fn language_name_and_syntax_follow_first_line_changes(cx: &mut TestAppContext) {
    let language_buffer = cx.new(|cx| {
        LanguageBuffer::new(
            test_buffer(""),
            Some(PathBuf::from("script")),
            test_registry(),
            cx,
        )
    });

    language_buffer.read_with(cx, |language_buffer, _| {
        // 未识别文件以纯文本兜底，且无语法树。
        assert_eq!(language_buffer.language_name(), Some("纯文本"));
        let language = language_buffer.language().expect("兜底语言应存在");
        assert_eq!(language.name(), "纯文本");
        assert!(language.grammar().is_none(), "纯文本兜底不应有语法树");
        assert!(
            language_buffer.parse_task.is_none(),
            "纯文本不应启动无意义的后台解析任务"
        );
    });
    language_buffer.update(cx, |language_buffer, cx| {
        language_buffer
            .edit(
                [Edit::insert(ByteOffset::ZERO, "#!/usr/bin/env python\nprint('ok')\n").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("测试编辑应成功");
    });
    cx.run_until_parked();

    language_buffer.read_with(cx, |language_buffer, _cx| {
        assert_eq!(language_buffer.language_name(), Some("Python"));
    });
}

#[gpui::test]
fn distinguishes_text_parse_and_metadata_events(cx: &mut TestAppContext) {
    let language_buffer = cx.new(|cx| {
        LanguageBuffer::new(
            test_buffer("fn main() {}\n"),
            Some(PathBuf::from("main.rs")),
            test_registry(),
            cx,
        )
    });
    cx.run_until_parked();

    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = Rc::clone(&events);
    let _subscription = cx.update(|cx| {
        cx.subscribe(&language_buffer, move |_, event, _| {
            observed.borrow_mut().push(match event {
                LanguageBufferEvent::TextChanged => "text",
                LanguageBufferEvent::Reparsed => "reparsed",
                LanguageBufferEvent::MetadataChanged => "metadata",
            });
        })
    });

    language_buffer.update(cx, |language_buffer, cx| {
        language_buffer
            .edit(
                [Edit::insert(ByteOffset::new(3), "async ").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("测试编辑应成功");
    });
    cx.run_until_parked();
    assert_eq!(events.borrow().as_slice(), ["text", "reparsed"]);

    events.borrow_mut().clear();
    language_buffer.update(cx, |language_buffer, cx| {
        language_buffer.mark_saved(cx);
    });
    cx.run_until_parked();
    assert_eq!(events.borrow().as_slice(), ["metadata"]);
}

#[gpui::test]
fn text_event_wakes_consumers_after_language_snapshot_reaches_the_batch_version(
    cx: &mut TestAppContext,
) {
    let language_buffer = cx.new(|cx| {
        LanguageBuffer::new(
            test_buffer("fn main() {}\n"),
            Some(PathBuf::from("main.rs")),
            test_registry(),
            cx,
        )
    });
    cx.run_until_parked();

    let direct_subscription =
        language_buffer.read_with(cx, |language_buffer, _| language_buffer.subscribe());
    let text_event_count = Rc::new(RefCell::new(0));
    let observed = Rc::clone(&text_event_count);
    let _subscription = cx.update(|cx| {
        cx.subscribe(&language_buffer, move |_, event, _| {
            if *event == LanguageBufferEvent::TextChanged {
                *observed.borrow_mut() += 1;
            }
        })
    });

    language_buffer.update(cx, |language_buffer, cx| {
        language_buffer
            .edit(
                [Edit::insert(ByteOffset::new(3), "async ").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("测试编辑应成功");
    });
    cx.run_until_parked();

    let direct = direct_subscription.consume();
    assert_eq!(*text_event_count.borrow(), 1);
    assert!(direct.transaction_id().is_some());
    let language_snapshot = cx.read_entity(&language_buffer, |buffer, _| buffer.snapshot());
    assert_eq!(
        language_snapshot.text.version(),
        direct.new_version().expect("文本变化应有新版本")
    );
    assert_eq!(
        language_snapshot.syntax.version(),
        language_snapshot.text.version()
    );
}

#[gpui::test]
fn rapid_edits_install_only_the_latest_parse(cx: &mut TestAppContext) {
    let language_buffer = cx.new(|cx| {
        LanguageBuffer::new(
            test_buffer("fn main() {}\n"),
            Some(PathBuf::from("main.rs")),
            test_registry(),
            cx,
        )
    });
    cx.run_until_parked();

    let events = Rc::new(RefCell::new(Vec::new()));
    let observed = Rc::clone(&events);
    let _subscription = cx.update(|cx| {
        cx.subscribe(&language_buffer, move |_, event, _| {
            observed.borrow_mut().push(match event {
                LanguageBufferEvent::TextChanged => "text",
                LanguageBufferEvent::Reparsed => "reparsed",
                LanguageBufferEvent::MetadataChanged => "metadata",
            });
        })
    });

    for text in ["a", "b", "c"] {
        language_buffer.update(cx, |language_buffer, cx| {
            let offset = language_buffer.len_bytes();
            language_buffer
                .edit(
                    [Edit::insert(offset, text).unwrap()],
                    TransactionMetadata::default(),
                    cx,
                )
                .expect("测试编辑应成功");
        });
    }
    let latest_version =
        language_buffer.read_with(cx, |language_buffer, _| language_buffer.version());
    cx.run_until_parked();

    language_buffer.read_with(cx, |language_buffer, _| {
        assert_eq!(language_buffer.snapshot().syntax.version(), latest_version);
    });
    assert_eq!(
        events
            .borrow()
            .iter()
            .filter(|event| **event == "reparsed")
            .count(),
        1
    );
}

/// 回归：文本与语法在同一实体上编辑后必须停留在同一版本，不再依赖跨实体观察者。
#[gpui::test]
fn text_and_syntax_share_the_version_after_same_entity_edit(cx: &mut TestAppContext) {
    let language_buffer = cx.new(|cx| {
        LanguageBuffer::new(
            test_buffer("fn main() {}\n"),
            Some(PathBuf::from("main.rs")),
            test_registry(),
            cx,
        )
    });
    language_buffer.update(cx, |language_buffer, cx| {
        language_buffer
            .edit(
                [Edit::insert(ByteOffset::new(3), "async ").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("测试编辑应成功");
    });
    cx.run_until_parked();

    language_buffer.read_with(cx, |language_buffer, _| {
        let snapshot = language_buffer.snapshot();
        assert_eq!(snapshot.text.version(), language_buffer.version());
        assert_eq!(snapshot.syntax.version(), snapshot.text.version());
    });
}

/// 回归：LanguageBuffer 仍需暴露与持有文本同源的版本化增量，供组合层惰性拉取。
#[gpui::test]
fn versioned_incremental_batch_is_still_available(cx: &mut TestAppContext) {
    let language_buffer = cx.new(|cx| {
        LanguageBuffer::new(
            test_buffer("fn main() {}\n"),
            Some(PathBuf::from("main.rs")),
            test_registry(),
            cx,
        )
    });
    let subscription =
        language_buffer.read_with(cx, |language_buffer, _| language_buffer.subscribe());
    let old_version = language_buffer.read_with(cx, |language_buffer, _| language_buffer.version());

    language_buffer.update(cx, |language_buffer, cx| {
        language_buffer
            .edit(
                [Edit::insert(ByteOffset::new(3), "async ").unwrap()],
                TransactionMetadata::default(),
                cx,
            )
            .expect("测试编辑应成功");
    });

    let changes = subscription.consume();
    assert!(!changes.is_empty(), "应能拉取到版本化增量");
    assert_eq!(changes.old_version(), Some(old_version));
    assert!(changes.new_version().is_some());
}
