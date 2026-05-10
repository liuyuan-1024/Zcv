//! M15 机器契约：锁定可变 `Buffer` 的单入口写入边界、不可变 `Snapshot` 的跨线程只读边界、
//! `DeltaEvent` 的顺序消费边界。
//!
//! 小阶段测试聚合为一个 cargo test 入口，子模块按 M15A / M15B / M15C 分组。

mod m15a_single_writer {
    //! M15A：所有写入必须经过 Buffer 单入口（事务管线 / Buffer 编辑入口），
    //! 失败时不动 Buffer，也不留 DeltaEvent；成功时按提交顺序生成 DeltaEvent。

    use zom_engine::{
        Buffer, BufferConfig, BufferVersion, CharOffset, Edit, EngineError, TextRange, Transaction,
        TransactionError,
    };

    fn buffer(text: &str) -> Buffer {
        Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
    }

    fn c(value: usize) -> CharOffset {
        CharOffset::new(value)
    }

    fn range(start: usize, end: usize) -> TextRange {
        TextRange::new(c(start), c(end)).unwrap()
    }

    #[test]
    fn buffer_write_entries_require_mutable_reference() {
        // 编译期契约：Buffer 写入入口必须 &mut self。
        // 这里只是用一次成功提交证明 &mut 链路连通；&self 路径在下面的 Snapshot 测试里反向覆盖。
        let mut buffer = buffer("ab");
        let tx = Transaction::from_edits(
            buffer.version(),
            vec![Edit::insert(c(1), "X".to_string()).unwrap()],
        )
        .unwrap();
        buffer.apply_transaction(tx).unwrap();
        assert_eq!(buffer.version(), BufferVersion::new(1));
    }

    #[test]
    fn successful_transaction_advances_version_and_pushes_delta_event() {
        let mut buffer = buffer("abc");
        let base_version = buffer.version();
        let tx = Transaction::from_edits(
            base_version,
            vec![Edit::insert(c(1), "X".to_string()).unwrap()],
        )
        .unwrap();

        buffer.apply_transaction(tx).unwrap();

        assert_eq!(buffer.version(), BufferVersion::new(base_version.get() + 1));
        let event = buffer.last_delta_event().unwrap();
        assert_eq!(event.old_version, base_version);
        assert_eq!(event.new_version, buffer.version());
        assert_eq!(buffer.pending_delta_event_count(), 1);
    }

    #[test]
    fn version_mismatch_is_atomically_rejected_and_leaves_no_delta_event() {
        let mut buffer = buffer("abc");
        let stale_version = BufferVersion::new(buffer.version().get() + 99);
        let tx = Transaction::from_edits(
            stale_version,
            vec![Edit::insert(c(0), "Z".to_string()).unwrap()],
        )
        .unwrap();

        let outcome = buffer.apply_transaction(tx);

        match outcome {
            Err(EngineError::Transaction(TransactionError::VersionMismatch {
                expected,
                actual,
            })) => {
                assert_eq!(expected, buffer.version());
                assert_eq!(actual, stale_version);
            }
            other => panic!("预期 VersionMismatch，实际 {other:?}"),
        }
        assert_eq!(buffer.version(), BufferVersion::INITIAL);
        assert!(buffer.last_delta_event().is_none());
        assert_eq!(buffer.pending_delta_event_count(), 0);
        assert_eq!(buffer.snapshot().text(), "abc");
    }

    #[test]
    fn multiple_commits_generate_consecutive_versioned_delta_events() {
        let mut buffer = buffer("");
        let base_version = buffer.version();

        for ch in ['a', 'b', 'c'] {
            let offset = buffer.len_chars();
            let tx = Transaction::from_edits(
                buffer.version(),
                vec![Edit::insert(offset, ch.to_string()).unwrap()],
            )
            .unwrap();
            buffer.apply_transaction(tx).unwrap();
        }

        let events = buffer.pending_delta_events();
        assert_eq!(events.len(), 3);
        assert_eq!(events[0].old_version, base_version);
        for window in events.windows(2) {
            assert_eq!(
                window[0].new_version, window[1].old_version,
                "DeltaEvent 必须形成连续版本链"
            );
        }
        assert_eq!(events[2].new_version, buffer.version());
    }

    #[test]
    fn editlist_overlap_rejection_is_atomic_pre_commit() {
        let buffer = buffer("abcdef");
        // 编辑列表自身重叠在事务构造层就拒绝，绝不可能进入 Buffer 状态。
        let outcome = Transaction::from_edits(
            buffer.version(),
            vec![
                Edit::replace(range(0, 3), "X".to_string()),
                Edit::insert(c(2), "Y".to_string()).unwrap(),
            ],
        );

        assert!(matches!(outcome, Err(EngineError::Edit(_))));
        assert_eq!(buffer.version(), BufferVersion::INITIAL);
        assert!(buffer.last_delta_event().is_none());
    }
}

mod m15b_snapshot_reader {
    //! M15B：Snapshot 跨线程只读，与可变 Buffer 解耦；查询结果绑定 BufferVersion。

    use std::thread;

    use zom_engine::{
        Buffer, BufferConfig, CharOffset, Edit, SearchOptions, Snapshot, Transaction,
    };

    fn buffer(text: &str) -> Buffer {
        Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
    }

    fn c(value: usize) -> CharOffset {
        CharOffset::new(value)
    }

    /// 编译期断言：Snapshot 必须 `Send + Sync`，否则不能跨线程下发只读查询。
    #[test]
    fn snapshot_is_send_and_sync() {
        fn assert_send<T: Send>() {}
        fn assert_sync<T: Sync>() {}
        assert_send::<Snapshot>();
        assert_sync::<Snapshot>();
    }

    #[test]
    fn snapshot_outlives_subsequent_buffer_mutation() {
        let mut buffer = buffer("abc");
        let original_version = buffer.version();
        let snapshot = buffer.snapshot();

        let tx = Transaction::from_edits(
            buffer.version(),
            vec![Edit::insert(c(1), "XYZ".to_string()).unwrap()],
        )
        .unwrap();
        buffer.apply_transaction(tx).unwrap();

        // Buffer 已经推进，但旧 snapshot 仍然只读且文本/版本不变。
        assert_ne!(buffer.version(), original_version);
        assert_eq!(snapshot.version(), original_version);
        assert_eq!(snapshot.text(), "abc");
        assert!(buffer.is_snapshot_stale(&snapshot));
    }

    #[test]
    fn snapshot_can_be_moved_to_another_thread_for_read_only_query() {
        let buffer = buffer("hello world");
        let snapshot = buffer.snapshot();
        let captured_version = snapshot.version();

        let handle = thread::spawn(move || {
            // 跨线程读取文本与查询结果，不依赖原 Buffer。
            let text = snapshot.text().into_owned();
            let result = snapshot
                .search("world", SearchOptions::new().with_case_sensitive(true))
                .unwrap();
            (text, result.version(), result.len())
        });

        let (text, result_version, hits) = handle.join().unwrap();
        assert_eq!(text, "hello world");
        assert_eq!(result_version, captured_version);
        assert_eq!(hits, 1);
    }

    #[test]
    fn snapshot_search_result_binds_snapshot_version_for_host_acceptance() {
        let mut buffer = buffer("foo foo");
        let snapshot = buffer.snapshot();
        let result = snapshot
            .search("foo", SearchOptions::new().with_case_sensitive(true))
            .unwrap();
        assert_eq!(result.version(), snapshot.version());
        assert!(!result.is_stale(buffer.version()));

        // Buffer 推进后，snapshot 派生的结果就过期了，宿主可以按版本号丢弃。
        let tx = Transaction::from_edits(
            buffer.version(),
            vec![Edit::insert(c(0), "Z".to_string()).unwrap()],
        )
        .unwrap();
        buffer.apply_transaction(tx).unwrap();
        assert!(result.is_stale(buffer.version()));
    }
}

mod m15c_delta_consumer {
    //! M15C：DeltaEvent 队列按提交顺序累积，可读最近事件、可顺序消费、可按版本检测漏读。

    use zom_engine::{
        Buffer, BufferConfig, BufferVersion, CharOffset, DeltaEvent, Edit, Transaction,
    };

    fn buffer(text: &str) -> Buffer {
        Buffer::from_text(text.to_string(), BufferConfig::default()).unwrap()
    }

    fn c(value: usize) -> CharOffset {
        CharOffset::new(value)
    }

    fn commit_insert(buffer: &mut Buffer, offset: usize, text: &str) {
        let tx = Transaction::from_edits(
            buffer.version(),
            vec![Edit::insert(c(offset), text.to_string()).unwrap()],
        )
        .unwrap();
        buffer.apply_transaction(tx).unwrap();
    }

    #[test]
    fn last_delta_event_reflects_most_recent_commit() {
        let mut buffer = buffer("");
        assert!(buffer.last_delta_event().is_none());

        commit_insert(&mut buffer, 0, "a");
        let first_version = buffer.version();
        assert_eq!(
            buffer.last_delta_event().unwrap().new_version,
            first_version
        );

        commit_insert(&mut buffer, 1, "b");
        let last = buffer.last_delta_event().unwrap();
        assert_eq!(last.new_version, buffer.version());
        assert_eq!(last.old_version, first_version);
    }

    #[test]
    fn pending_delta_events_can_be_peeked_without_draining() {
        let mut buffer = buffer("");
        commit_insert(&mut buffer, 0, "a");
        commit_insert(&mut buffer, 1, "b");

        let peeked = buffer.pending_delta_events();
        assert_eq!(peeked.len(), 2);
        // peek 不消费——再次调用仍然能看到原队列。
        assert_eq!(buffer.pending_delta_event_count(), 2);
        assert_eq!(buffer.pending_delta_events().len(), 2);
    }

    #[test]
    fn take_pending_events_drains_in_commit_order_and_resets_queue() {
        let mut buffer = buffer("");
        commit_insert(&mut buffer, 0, "a");
        commit_insert(&mut buffer, 1, "b");
        commit_insert(&mut buffer, 2, "c");

        let drained: Vec<DeltaEvent> = buffer.take_pending_events();

        assert_eq!(drained.len(), 3);
        assert_eq!(drained[0].old_version, BufferVersion::INITIAL);
        for window in drained.windows(2) {
            assert_eq!(window[0].new_version, window[1].old_version);
        }
        assert_eq!(drained.last().unwrap().new_version, buffer.version());
        assert_eq!(buffer.pending_delta_event_count(), 0);

        // 新提交在排空后继续累积。
        commit_insert(&mut buffer, 3, "d");
        let after = buffer.pending_delta_events();
        assert_eq!(after.len(), 1);
        assert_eq!(after[0].old_version, drained.last().unwrap().new_version);
    }

    #[test]
    fn consumer_can_detect_missed_events_by_version_chain() {
        let mut buffer = buffer("");
        commit_insert(&mut buffer, 0, "a");
        commit_insert(&mut buffer, 1, "b");
        commit_insert(&mut buffer, 2, "c");

        // 模拟一个本地订阅者：只取走第一批事件，后又懒惰地等到下一批。
        let mut last_seen = BufferVersion::INITIAL;
        let first_batch = buffer.take_pending_events();
        for event in &first_batch {
            assert_eq!(
                event.old_version, last_seen,
                "首批事件必须紧跟订阅者最后看到的版本"
            );
            last_seen = event.new_version;
        }
        assert_eq!(last_seen, buffer.version());

        // 期间又有新提交，下一批仍能拼接到 last_seen。
        commit_insert(&mut buffer, 3, "d");
        commit_insert(&mut buffer, 4, "e");

        let next_batch = buffer.take_pending_events();
        assert_eq!(
            next_batch.first().unwrap().old_version,
            last_seen,
            "下一批的首事件 old_version 必须接续 last_seen，否则代表漏读"
        );
        for event in &next_batch {
            assert_eq!(event.old_version, last_seen);
            last_seen = event.new_version;
        }
        assert_eq!(last_seen, buffer.version());
    }

    #[test]
    fn deferred_subscriber_keeps_snapshot_for_required_version_facts() {
        let mut buffer = buffer("hello");
        // 订阅者拿走当前 snapshot，之后即使 Buffer 推进也能基于版本事实判断结果是否过期。
        let snapshot = buffer.snapshot();
        let stored_version = snapshot.version();

        commit_insert(&mut buffer, 5, "!");
        let event = buffer.last_delta_event().cloned().unwrap();

        // 延迟处理：订阅者把 snapshot.version 作为旧版本基线，DeltaEvent 提供新版本 + position_map。
        assert_eq!(event.old_version, stored_version);
        assert_eq!(event.new_version, buffer.version());
        assert_eq!(snapshot.text(), "hello");
        assert_eq!(buffer.snapshot().text(), "hello!");
    }
}
