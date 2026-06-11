//! Phase 4 不变量：编辑后主线程同步 `tree.edit` 立即把 slot 里 tree 的字节坐标推
//! 进到新 snapshot —— paint 这一帧不必等 worker reparse。
//!
//! 这条钉死「编辑当帧 paint 永远拿到与 buffer 一致的字节坐标」，是 Phase 1 主线程
//! `tree_slot.try_edit` 的存在理由。

use std::rc::Rc;

use zom_engine::{Buffer, BufferConfig};
use zom_workspace::SyntaxDocument;
use zom_workspace::syntax::{LanguageId, SyntaxEngine, install_builtin_providers};

fn engine_with_builtins() -> Rc<SyntaxEngine> {
    let mut engine = SyntaxEngine::new();
    install_builtin_providers(&mut engine);
    Rc::new(engine)
}

#[test]
fn insert_advances_tree_end_byte_in_same_main_thread_tick() {
    let engine = engine_with_builtins();
    let buffer = Buffer::from_text("fn main() {}\n".to_string(), BufferConfig::default()).unwrap();
    let mut doc = SyntaxDocument::from_buffer(engine.clone(), buffer, LanguageId::new("rust"));
    // 等首份 tree 落到 slot（attach 是异步的）。
    engine.worker().wait_for_idle_for_test_or_bench();

    let slot = doc
        .syntax_tree_slot()
        .expect("rust buffer 必须挂 provider")
        .clone();
    let initial = slot.load().expect("attach 完成后必须有 tree");
    assert_eq!(
        initial.tree().root_node().end_byte(),
        doc.buffer().snapshot().len_bytes().get(),
        "首次 attach 后 tree 应当与 buffer 字节长度对齐"
    );

    // 在 buffer 末尾插入若干字符，不等 worker 跑 —— 测的就是主线程 try_edit 把坐标
    // 推进到新版本的能力。
    let insert_at = doc.buffer().snapshot().len_bytes();
    doc.buffer_mut().insert(insert_at, " // tail").unwrap();
    doc.pump_post_edit().unwrap();

    let advanced = slot
        .load()
        .expect("handle_edit 后 slot 仍应当有 tree（主线程 tree.edit）");
    assert_eq!(
        advanced.version(),
        doc.buffer().version(),
        "slot 的版本应当被 try_edit 同步推到编辑后的新版本"
    );
    assert_eq!(
        advanced.tree().root_node().end_byte(),
        doc.buffer().snapshot().len_bytes().get(),
        "tree.edit 后根节点 end_byte 必须等于新 buffer 字节长度"
    );
}

#[test]
fn multiple_consecutive_inserts_keep_tree_aligned() {
    // 连续插入多次：每次主线程 try_edit 都把 slot 推到最新版本；worker 端的
    // reparse 异步覆盖。
    // 本测不调 wait_for_idle_for_test_or_bench，直接比对主线程视角下的不变量。
    let engine = engine_with_builtins();
    let buffer = Buffer::from_text(
        "fn main() { let mut s = String::new(); }\n".to_string(),
        BufferConfig::default(),
    )
    .unwrap();
    let mut doc = SyntaxDocument::from_buffer(engine.clone(), buffer, LanguageId::new("rust"));
    engine.worker().wait_for_idle_for_test_or_bench();
    let slot = doc.syntax_tree_slot().unwrap().clone();

    for ch in ["a", "b", "c", "d", "e"] {
        let at = doc.buffer().snapshot().len_bytes();
        doc.buffer_mut().insert(at, ch).unwrap();
        doc.pump_post_edit().unwrap();

        let tree = slot.load().expect("每次编辑后 slot 应当持有 tree");
        assert_eq!(
            tree.version(),
            doc.buffer().version(),
            "slot 版本必须紧跟 buffer 版本"
        );
        assert_eq!(
            tree.tree().root_node().end_byte(),
            doc.buffer().snapshot().len_bytes().get(),
            "tree.edit 链必须严格跟住 buffer 字节长度（插入 '{ch}' 后）"
        );
    }
}
