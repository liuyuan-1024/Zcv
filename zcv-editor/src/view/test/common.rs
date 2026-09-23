use zcv_language::LanguageRegistry;
use zcv_multi_buffer::MultiBufferOffset;

use super::*;
use gpui::{Bounds, Pixels, TestAppContext, VisualTestContext, point, size};
use zcv_buffer_diff::{BufferDiff, BufferDiffInput};
use zcv_multi_buffer::{DiffFile, DisplayHunk};
use zcv_text::{Buffer, BufferConfig};

use crate::scrollbar::{SCROLLBAR_WIDTH, thumb_geometry};

impl Editor {
    pub(super) fn for_language_buffer(
        language_buffer: Entity<LanguageBuffer>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::from_language_buffer(language_buffer, EditorMode::Full, cx)
    }

    /// 测试用：设置换行模式并触发显示重排。
    pub(super) fn set_soft_wrap_mode(
        &mut self,
        soft_wrap: Option<SoftWrap>,
        cx: &mut Context<Self>,
    ) {
        let soft_wrap = soft_wrap.unwrap_or_default();
        if self.soft_wrap == soft_wrap {
            return;
        }
        self.soft_wrap = soft_wrap;
        cx.notify();
    }
}

pub(super) fn test_buffer(
    cx: &mut TestAppContext,
    text: impl Into<String>,
) -> Entity<LanguageBuffer> {
    let buffer =
        Buffer::from_text(text.into(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    cx.new(|cx| {
        LanguageBuffer::new(
            buffer,
            None,
            std::sync::Arc::new(LanguageRegistry::new()),
            cx,
        )
    })
}

pub(super) fn focus_editor(editor: &Entity<Editor>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let focus = editor.read(cx).focus_handle();
        window.focus(&focus, cx);
    });
}
pub(super) fn buffer_text(buffer: &Entity<LanguageBuffer>, cx: &TestAppContext) -> String {
    cx.read_entity(buffer, |language_buffer, _| {
        let snapshot = language_buffer.text_snapshot();
        snapshot
            .slice_byte_range(MultiBufferOffset::ZERO.into(), snapshot.len_bytes())
            .expect("完整文本应可读取")
            .as_str()
            .to_owned()
    })
}
/// 读取滚动轴几何：返回 (track_bounds, thumb_bounds, scroll_per_pixel)。
/// thumb 几何与渲染侧共用 thumb_geometry，保证断言与真实几何一致。
pub(super) fn scrollbar_geometry(
    editor: &Entity<Editor>,
    cx: &mut VisualTestContext,
) -> (Bounds<Pixels>, Option<Bounds<Pixels>>, f32) {
    let window_bounds = cx.update(|window, _| window.bounds());
    let track_bounds = Bounds {
        origin: point(window_bounds.right() - SCROLLBAR_WIDTH, window_bounds.top()),
        size: size(SCROLLBAR_WIDTH, window_bounds.size.height),
    };
    cx.read_entity(editor, |editor, _| {
        let (thumb_bounds, per_pixel) =
            thumb_geometry(track_bounds, editor.max_scroll_top(), editor.scroll_top())
                .map_or((None, 0.0), |(bounds, scale)| (Some(bounds), scale));
        (track_bounds, thumb_bounds, per_pixel)
    })
}

/// 为普通编辑器注入统一 diff 投影（工作区源为文件级 LanguageBuffer）。
///
/// 与 item_provider 打开路径一致：工作区源实体由测试创建并跨注入复用
/// （同一实体重新注入时展开状态按文本跟踪区间迁移）。
pub(super) fn inject_editor_diff(
    editor: &Entity<Editor>,
    source: &Entity<LanguageBuffer>,
    _hunks: Vec<DisplayHunk>,
    base_text: Option<Arc<str>>,
    cx: &mut TestAppContext,
) {
    editor.update(cx, |editor, cx| {
        // diff 路径必须与工作区源一致：excerpt 的路径身份来自源，生产不变式是两者相同。
        let working_path = source
            .read(cx)
            .file_path()
            .map_or_else(|| PathBuf::from("src/a.rs"), |path| path.to_path_buf());
        // 旧测试会用 None 表示“整份文本均为新增”。现在仍由一对真实文档快照
        // 派生该 Added hunk，而不是注入 hunk。
        let language_registry = source.read(cx).language_registry();
        let line_count = source.read(cx).text_snapshot().line_count();
        let diff = cx.new(|cx| {
            BufferDiff::new(
                BufferDiffInput {
                    operations: None,
                    working: source.clone(),
                    base_text: Some(base_text.as_deref().unwrap_or_default().to_owned()),
                    index_text: None,
                    path: working_path.clone(),
                    language_registry,
                    key: 0,
                },
                cx,
            )
        });
        editor.set_diff_files(
            vec![DiffFile {
                diff,
                display_path: working_path.clone(),
                excerpt_ranges: vec![0..line_count],
            }],
            cx,
        );
    });
    // diff 在后台异步计算；注入后等待落定，测试才能看到派生 hunk。
    cx.run_until_parked();
}

/// 为单文件组合文档注入 Git diff 投影。
pub(super) fn inject_file_diff(
    editor: &Entity<Editor>,
    source: &Entity<LanguageBuffer>,
    base_text: Arc<str>,
    cx: &mut TestAppContext,
) {
    editor.update(cx, |editor, cx| {
        // diff 路径必须与工作区源一致：excerpt 的路径身份来自源，生产不变式是两者相同。
        let working_path = source
            .read(cx)
            .file_path()
            .map_or_else(|| PathBuf::from("src/a.rs"), |path| path.to_path_buf());
        let language_registry = source.read(cx).language_registry();
        let line_count = source.read(cx).text_snapshot().line_count();
        let diff = cx.new(|cx| {
            BufferDiff::new(
                BufferDiffInput {
                    operations: None,
                    working: source.clone(),
                    base_text: Some(base_text.to_string()),
                    index_text: None,
                    path: working_path.clone(),
                    language_registry,
                    key: 0,
                },
                cx,
            )
        });
        editor.set_diff_files(
            vec![DiffFile {
                diff,
                display_path: working_path.clone(),
                excerpt_ranges: vec![0..line_count],
            }],
            cx,
        );
    });
    cx.run_until_parked();
}

pub(super) fn scrolling_text() -> String {
    (0..100)
        .map(|row| format!("line {row}\n"))
        .collect::<String>()
}
