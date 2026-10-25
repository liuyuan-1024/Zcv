use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

use gpui::{AppContext, TestAppContext};
use zcv_path::AbsolutePathBuf;
use zcv_text::ByteOffset;

use super::*;
use zcv_settings::{SettingsStore, UserSettings};

static NEXT_TEST_FILE_ID: AtomicU64 = AtomicU64::new(1);

#[gpui::test]
fn opening_the_same_file_reuses_its_buffer(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let path = test_file_path();
    fs::write(&path, "共享内容").expect("测试文件应可写入");

    let (first, second) = cx.update(|cx| {
        let mut store = BufferStore::new(Arc::new(LanguageRegistry::new()));
        let first = store.open_buffer(&path, cx).expect("首次打开应成功");
        let second = store.open_buffer(&path, cx).expect("再次打开应成功");
        (first, second)
    });

    assert_eq!(first, second);
    fs::remove_file(path).expect("测试文件应可删除");
}

#[gpui::test]
fn deleted_file_buffer_keeps_the_same_identity_after_file_is_recreated(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let path = directory.path().join("deleted.txt");
    let mut store = BufferStore::new(Arc::new(LanguageRegistry::new()));
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
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let path = test_file_path();
    fs::write(&path, "项目隔离").expect("测试文件应可写入");

    let (first, second) = cx.update(|cx| {
        let mut first_store = BufferStore::new(Arc::new(LanguageRegistry::new()));
        let mut second_store = BufferStore::new(Arc::new(LanguageRegistry::new()));
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
fn directory_events_find_only_live_buffers_in_the_subtree(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let directory = tempfile::tempdir().expect("应创建临时项目目录");
    let directory = AbsolutePathBuf::canonicalize(directory.path())
        .expect("临时目录应可规范化")
        .into_path_buf();
    let file = directory.join("file.txt");
    let nested = directory.join("sub").join("nested.txt");
    let sibling = directory.join("sibling.txt");
    fs::create_dir_all(directory.join("sub")).expect("应创建子目录");
    fs::write(&file, "文件").expect("应创建测试文件");
    fs::write(&nested, "嵌套").expect("应创建测试文件");
    fs::write(&sibling, "同级").expect("应创建测试文件");

    let mut store = BufferStore::new(Arc::new(LanguageRegistry::new()));
    let (file_buffer, nested_buffer, sibling_buffer) = cx.update(|cx| {
        (
            store.open_buffer(&file, cx).expect("应打开测试文件"),
            store.open_buffer(&nested, cx).expect("应打开测试文件"),
            store.open_buffer(&sibling, cx).expect("应打开测试文件"),
        )
    });

    assert_eq!(store.buffers_under_path(&file), vec![file_buffer]);
    assert_eq!(
        store.buffers_under_path(&directory.join("sub")),
        vec![nested_buffer]
    );
    assert_eq!(store.buffers_under_path(&sibling), vec![sibling_buffer]);
    fs::remove_file(file).expect("测试文件应可删除");
    fs::remove_file(nested).expect("测试文件应可删除");
    fs::remove_file(sibling).expect("测试文件应可删除");
}

#[gpui::test]
fn released_buffer_is_loaded_again(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let path = test_file_path();
    fs::write(&path, "第一次").expect("测试文件应可写入");

    let mut store = BufferStore::new(Arc::new(LanguageRegistry::new()));
    let first_id = cx.update(|cx| {
        let buffer = store.open_buffer(&path, cx).expect("首次打开应成功");
        buffer.entity_id()
    });
    cx.run_until_parked();

    fs::write(&path, "第二次").expect("测试文件应可更新");
    let second = cx.update(|cx| store.open_buffer(&path, cx).expect("重新打开应成功"));

    assert_ne!(first_id, second.entity_id());
    cx.read_entity(&second, |language_buffer, _| {
        let snapshot = language_buffer.text_snapshot();
        assert_eq!(
            snapshot
                .slice_byte_range(ByteOffset::ZERO, snapshot.len_bytes())
                .expect("完整文本应可读取")
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
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let path = directory.path().join("live.txt");
    fs::write(&path, "内存文档").expect("应写入文件");
    let mut store = BufferStore::new(Arc::new(LanguageRegistry::new()));
    let first = cx.update(|cx| store.open_buffer(&path, cx).expect("首次打开应成功"));
    fs::write(&path, [0xff, 0xfe, 0xff]).expect("应替换为非 UTF-8 内容");
    let second = cx.update(|cx| store.open_buffer(&path, cx).expect("存活文档应直接复用"));
    assert_eq!(first, second);
}

#[gpui::test]
fn live_buffer_identity_survives_external_file_removal(cx: &mut TestAppContext) {
    cx.set_global(SettingsStore::new(UserSettings::default()));
    let directory = tempfile::tempdir().expect("应创建临时目录");
    let path = directory.path().join("removed.txt");
    fs::write(&path, "旧内容").expect("应写入文件");
    let mut store = BufferStore::new(Arc::new(LanguageRegistry::new()));
    let first = cx.update(|cx| store.open_buffer(&path, cx).expect("首次打开应成功"));

    fs::remove_file(&path).expect("应删除磁盘文件");
    let reopened = cx
        .update(|cx| store.open_buffer(&path, cx))
        .expect("应复用存活文档");
    assert_eq!(first, reopened);
    cx.read_entity(&first, |language_buffer, _| {
        let snapshot = language_buffer.text_snapshot();
        let range = zcv_text::TextRange::new(ByteOffset::ZERO, snapshot.len_bytes())
            .expect("全文范围应有效");
        assert_eq!(snapshot.slice_text(range).unwrap().as_str(), "旧内容");
    });
}
