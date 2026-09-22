//! `BufferDiff` 后台任务所有权与输入版本门控的定向测试。

use std::path::PathBuf;
use std::sync::Arc;

use gpui::{AppContext as _, Entity, Task, TestAppContext};
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_text::{Buffer, BufferConfig};

use crate::{BufferDiff, BufferDiffInput, DiffRefresh};

/// 建立一个指定文本与路径的语言 Buffer（diff 输入源）。
fn language_buffer(
    text: &str,
    path: &str,
    cx: &mut impl gpui::AppContext,
) -> Entity<LanguageBuffer> {
    let buffer = Buffer::from_text(text.to_string(), BufferConfig::default())
        .expect("测试文本必须能创建 Buffer");
    let path = PathBuf::from(path);
    let registry = Arc::new(LanguageRegistry::new());
    cx.new(|cx| LanguageBuffer::new(buffer, Some(path), registry, cx))
}

/// 建立只含 working 与 base 文本的 diff 输入。
fn buffer_diff_input(
    working: Entity<LanguageBuffer>,
    base_text: Option<&str>,
    path: &str,
) -> BufferDiffInput {
    BufferDiffInput {
        working,
        path: PathBuf::from(path),
        base_text: base_text.map(str::to_owned),
        index_text: None,
        language_registry: Arc::new(LanguageRegistry::new()),
        key: 0,
        operations: None,
    }
}

/// 回归（M-E）：连续重算始终只替换实体拥有的同一个在途任务，实体销毁时随字段取消。
#[gpui::test]
fn rapid_recomputes_replace_the_single_owned_task(cx: &mut TestAppContext) {
    let working = language_buffer("a\nb\nc\n", "src/a.rs", cx);
    let diff = cx.new(|cx| {
        BufferDiff::new(
            buffer_diff_input(working, Some("a\nB\nc\n"), "src/a.rs"),
            cx,
        )
    });

    // 创建即拥有一个在途任务；每次重算都替换同一字段，而不是堆积多个任务。
    for _ in 0..8 {
        assert!(
            cx.read_entity(&diff, |diff, _| diff.calculation_task.is_some()),
            "实体必须拥有在途任务"
        );
        cx.update_entity(&diff, |diff, cx| {
            diff.recompute_with_refresh(DiffRefresh::RebuildProjection, cx);
        });
    }
    cx.run_until_parked();
    assert!(
        cx.read_entity(&diff, |diff, _| diff
            .calculation_task
            .as_ref()
            .is_some_and(Task::is_ready)),
        "任务完成后仍由实体持有，下一次重算替换它"
    );
    assert!(
        cx.update_entity(&diff, |diff, cx| diff.is_current_version_calculated(cx)),
        "连续重算必须收敛到当前输入版本"
    );
}

/// 回归（M-F）：working 不变而 base 连续前进时，旧 base 对应的结果不得安装。
#[gpui::test]
fn base_version_change_discards_stale_hunks(cx: &mut TestAppContext) {
    let working = language_buffer("a\nb\nc\n", "src/a.rs", cx);
    let diff = cx.new(|cx| {
        BufferDiff::new(
            buffer_diff_input(working, Some("a\nX\nc\n"), "src/a.rs"),
            cx,
        )
    });

    // 初始 diff 在途时刷新 base；working 版本保持不变。
    let _task = cx.update_entity(&diff, |diff, cx| {
        diff.set_base_text(Some("a\nLONGER\nc\n".to_string()), cx)
    });
    cx.run_until_parked();

    let hunks = cx.read_entity(&diff, |diff, _| diff.snapshot().hunks().to_vec());
    assert_eq!(hunks.len(), 1, "base 变化后必须重算出唯一修改块");
    assert_eq!(
        hunks[0].diff_base_byte_range,
        2..9,
        "只有与最终 base 对应的旧侧字节范围可以落地"
    );
    assert!(
        cx.update_entity(&diff, |diff, cx| diff.is_current_version_calculated(cx)),
        "base 前进后必须补算到当前输入版本"
    );
}
