use std::fs;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use gpui::{AppContext, TestAppContext};

use super::*;

static NEXT_TEST_FILE_ID: AtomicU64 = AtomicU64::new(1);

#[gpui::test]
fn opening_the_same_file_reuses_its_buffer(cx: &mut TestAppContext) {
    let path = test_file_path();
    fs::write(&path, "共享内容").expect("测试文件应可写入");

    let (first, second) = cx.update(|cx| {
        let mut store = BufferStore::new();
        let first = store.open_buffer(&path, cx).expect("首次打开应成功");
        let second = store.open_buffer(&path, cx).expect("再次打开应成功");
        (first, second)
    });

    assert_eq!(first, second);
    fs::remove_file(path).expect("测试文件应可删除");
}

#[gpui::test]
fn deleted_file_buffer_keeps_the_same_identity_after_file_is_recreated(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let path = directory.path().join("deleted.txt");
    let mut store = BufferStore::new();
    let deleted = cx.update(|cx| {
        store
            .open_deleted_buffer(&path, cx)
            .expect("应打开删除文件的空文档")
    });

    fs::write(&path, "重新创建").expect("应重新创建文件");
    let reopened = cx.update(|cx| store.open_buffer(&path, cx).expect("应重新打开文件"));

    assert_eq!(
        deleted, reopened,
        "删除状态与重新创建后必须复用同一文档实体"
    );
}

#[gpui::test]
fn separate_project_buffer_stores_do_not_share_buffers(cx: &mut TestAppContext) {
    let path = test_file_path();
    fs::write(&path, "项目隔离").expect("测试文件应可写入");

    let (first, second) = cx.update(|cx| {
        let mut first_store = BufferStore::new();
        let mut second_store = BufferStore::new();
        let first = first_store
            .open_buffer(&path, cx)
            .expect("第一项目应打开文件");
        let second = second_store
            .open_buffer(&path, cx)
            .expect("第二项目应打开文件");
        (first, second)
    });

    assert_ne!(first, second);
    fs::remove_file(path).expect("测试文件应可删除");
}

#[gpui::test]
fn remove_path_drops_matching_indexes_and_keeps_others(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let directory = directory.path().canonicalize().expect("临时目录应可规范化");
    let file = directory.join("file.txt");
    let nested = directory.join("sub").join("nested.txt");
    let sibling = directory.join("sibling.txt");
    fs::create_dir_all(directory.join("sub")).expect("应创建子目录");
    fs::write(&file, "文件").expect("应创建测试文件");
    fs::write(&nested, "嵌套").expect("应创建测试文件");
    fs::write(&sibling, "同级").expect("应创建测试文件");

    let mut store = BufferStore::new();
    let (file_buffer, nested_buffer, sibling_buffer) = cx.update(|cx| {
        (
            store.open_buffer(&file, cx).expect("应打开测试文件"),
            store.open_buffer(&nested, cx).expect("应打开测试文件"),
            store.open_buffer(&sibling, cx).expect("应打开测试文件"),
        )
    });

    // 精确文件删除只移除该文件的索引；目录删除连同其中已打开的 Buffer 一起移除。
    store.remove_path(&file);
    store.remove_path(&directory.join("sub"));

    let (reloaded_file, reloaded_nested, kept_sibling) = cx.update(|cx| {
        (
            store.open_buffer(&file, cx).expect("应重新加载测试文件"),
            store.open_buffer(&nested, cx).expect("应重新加载测试文件"),
            store.open_buffer(&sibling, cx).expect("应重新打开测试文件"),
        )
    });
    assert_ne!(file_buffer, reloaded_file, "被删文件的索引应被移除");
    assert_ne!(
        nested_buffer, reloaded_nested,
        "被删目录内 Buffer 的索引应被移除"
    );
    assert_eq!(sibling_buffer, kept_sibling, "未匹配的索引应保留并复用");
    fs::remove_file(file).expect("测试文件应可删除");
    fs::remove_file(nested).expect("测试文件应可删除");
    fs::remove_file(sibling).expect("测试文件应可删除");
}

#[gpui::test]
fn released_buffer_is_loaded_again(cx: &mut TestAppContext) {
    let path = test_file_path();
    fs::write(&path, "第一次").expect("测试文件应可写入");

    let mut store = BufferStore::new();
    let first_id = cx.update(|cx| {
        let buffer = store.open_buffer(&path, cx).expect("首次打开应成功");
        buffer.entity_id()
    });
    cx.run_until_parked();

    fs::write(&path, "第二次").expect("测试文件应可更新");
    let second = cx.update(|cx| store.open_buffer(&path, cx).expect("重新打开应成功"));

    assert_ne!(first_id, second.entity_id());
    let buffer = cx.read_entity(&second, |language_buffer, _| language_buffer.buffer());
    cx.read_entity(&buffer, |buffer, _| {
        assert_eq!(
            buffer
                .slice_byte_range(zcv_text::ByteOffset::ZERO, buffer.len_bytes())
                .expect("完整 Buffer 应可读取")
                .as_str(),
            "第二次"
        );
    });
    fs::remove_file(path).expect("测试文件应可删除");
}

fn test_file_path() -> PathBuf {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("系统时间应晚于 Unix Epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "buffer-store-test-{}-{nonce}-{}.txt",
        std::process::id(),
        NEXT_TEST_FILE_ID.fetch_add(1, Ordering::Relaxed)
    ))
}

#[gpui::test]
fn reopening_live_buffer_does_not_load_changed_disk_contents(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let path = directory.path().join("live.txt");
    fs::write(&path, "内存文档").expect("应写入文件");
    let mut store = BufferStore::new();
    let first = cx.update(|cx| store.open_buffer(&path, cx).expect("首次打开应成功"));
    fs::write(&path, [0xff, 0xfe, 0xff]).expect("应替换为非 UTF-8 内容");
    let second = cx.update(|cx| store.open_buffer(&path, cx).expect("存活文档应直接复用"));
    assert_eq!(first, second);
    fs::remove_file(&path).expect("应删除磁盘文件");
    let third = cx.update(|cx| {
        store
            .open_buffer(&path, cx)
            .expect("磁盘删除不应阻止复用文档")
    });
    assert_eq!(first, third);
}

#[gpui::test]
#[ignore = "手动测量重复打开 32 MiB 已加载文档的耗时和进程峰值内存"]
fn repeated_open_buffer_measurement(cx: &mut TestAppContext) {
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let path = directory.path().join("large.txt");
    fs::write(&path, "abcdefghijklmno\n".repeat(2_097_152)).expect("应写入大文件");
    let mut store = BufferStore::new();
    let first = cx.update(|cx| store.open_buffer(&path, cx).expect("首次打开应成功"));
    cx.run_until_parked();
    let start = std::time::Instant::now();
    for _ in 0..10 {
        let next = cx.update(|cx| store.open_buffer(&path, cx).expect("重复打开应成功"));
        assert_eq!(first, next);
    }
    eprintln!("重复打开 10 次耗时：{:?}", start.elapsed());
}
