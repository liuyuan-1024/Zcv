use std::cell::RefCell;
use std::rc::Rc;

use gpui::TestAppContext;
use zcv_text::{BufferConfig, ByteOffset, Edit, TextRange, TransactionMetadata};

use super::*;

fn test_registry() -> Arc<LanguageRegistry> {
    Arc::new(LanguageRegistry::new())
}

fn test_buffer(text: &str) -> Buffer {
    Buffer::from_text(text.to_owned(), BufferConfig::default()).expect("应创建测试 Buffer")
}

#[test]
fn foreground_parse_obeys_budget_and_can_complete() {
    let registry = test_registry();
    let text = test_buffer("fn main() {}\n").snapshot();
    let mut syntax = SyntaxMap::new(Arc::clone(&registry), &text);
    assert!(syntax.set_language_for_file(std::path::Path::new("main.rs"), None, &text));
    let syntax = syntax.snapshot();

    let expired = ParseCancellation::with_timeout(Duration::ZERO);
    assert!(
        syntax.clone().reparse(&text, &registry, &expired).is_none(),
        "预算耗尽时不能安装未完成的语法树"
    );
    assert!(expired.was_timed_out(), "超时必须明确标记，供后台路径判断");

    let available = ParseCancellation::with_timeout(Duration::from_secs(1));
    let parsed = syntax
        .reparse(&text, &registry, &available)
        .expect("预算充足时应直接完成解析");
    assert_eq!(parsed.version(), text.version());
    assert!(parsed.root_tree().is_some());
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
        let version = language_buffer.version();
        language_buffer.did_save(version, cx);
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

    language_buffer.update(cx, |language_buffer, cx| {
        language_buffer.start_reparse(false, cx);
    });
    let in_flight = language_buffer.read_with(cx, |language_buffer, _| {
        language_buffer
            .parse_task
            .as_ref()
            .expect("测试重解析应在后台运行")
            .cancellation
            .clone()
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
    assert!(
        !in_flight.is_cancelled(),
        "连续编辑应合并到在途任务之后，不能逐次取消并重启"
    );
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

#[gpui::test]
fn language_switch_during_parse_installs_the_new_grammar(cx: &mut TestAppContext) {
    let language_buffer = cx.new(|cx| {
        LanguageBuffer::new(
            test_buffer("print('ok')\n"),
            Some(PathBuf::from("script.rs")),
            test_registry(),
            cx,
        )
    });
    cx.run_until_parked();

    language_buffer.update(cx, |language_buffer, cx| {
        language_buffer.start_reparse(false, cx);
        language_buffer.set_file_path(PathBuf::from("script.py"), cx);
        assert!(language_buffer.parse_task.is_some());
    });
    cx.run_until_parked();

    language_buffer.read_with(cx, |language_buffer, _| {
        let snapshot = language_buffer.snapshot();
        assert_eq!(language_buffer.language_name(), Some("Python"));
        assert_eq!(snapshot.syntax.version(), snapshot.text.version());
        assert_eq!(
            snapshot
                .syntax
                .root_tree()
                .expect("语言切换后应完成 Python 解析")
                .root_node()
                .kind(),
            "module"
        );
    });
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

#[gpui::test]
fn is_dirty_ignores_edits_that_restore_the_visible_fragments(cx: &mut TestAppContext) {
    let language_buffer =
        cx.new(|cx| LanguageBuffer::new(test_buffer("hello"), None, test_registry(), cx));
    language_buffer.update(cx, |language_buffer, cx| {
        let version = language_buffer.version();
        language_buffer.did_save(version, cx);
    });

    language_buffer.update(cx, |language_buffer, cx| {
        language_buffer
            .edit(
                [Edit::insert(ByteOffset::new(5), "!").expect("插入编辑必须合法")],
                TransactionMetadata::default(),
                cx,
            )
            .expect("编辑应成功");
    });
    assert!(language_buffer.read_with(cx, |language_buffer, _| language_buffer.is_dirty()));

    language_buffer.update(cx, |language_buffer, cx| {
        language_buffer
            .edit(
                [Edit::delete(
                    TextRange::new(ByteOffset::new(5), ByteOffset::new(6))
                        .expect("删除范围必须合法"),
                )],
                TransactionMetadata::default(),
                cx,
            )
            .expect("编辑应成功");
    });
    assert!(!language_buffer.read_with(cx, |language_buffer, _| language_buffer.is_dirty()));
}

#[gpui::test]
fn is_dirty_is_clean_after_undoing_a_deletion(cx: &mut TestAppContext) {
    let language_buffer =
        cx.new(|cx| LanguageBuffer::new(test_buffer("hello"), None, test_registry(), cx));
    language_buffer.update(cx, |language_buffer, cx| {
        let version = language_buffer.version();
        language_buffer.did_save(version, cx);
    });

    language_buffer.update(cx, |language_buffer, cx| {
        language_buffer
            .edit(
                [Edit::delete(
                    TextRange::new(ByteOffset::new(1), ByteOffset::new(3))
                        .expect("删除范围必须合法"),
                )],
                TransactionMetadata::default(),
                cx,
            )
            .expect("编辑应成功");
    });
    assert!(language_buffer.read_with(cx, |language_buffer, _| language_buffer.is_dirty()));

    language_buffer.update(cx, |language_buffer, cx| {
        language_buffer.undo(cx).expect("撤销应成功");
    });
    assert!(!language_buffer.read_with(cx, |language_buffer, _| language_buffer.is_dirty()));
}
