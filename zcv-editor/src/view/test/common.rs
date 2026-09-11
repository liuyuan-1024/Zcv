use super::*;
use gpui::{Bounds, Pixels, TestAppContext, VisualTestContext, point, size};
use zcv_multi_buffer::{BufferDiff, BufferDiffInput, DiffFile, DiffProjection, DisplayHunk};
use zcv_text::{Buffer, BufferConfig, ByteOffset};

use crate::scrollbar::{SCROLLBAR_WIDTH, thumb_geometry};

impl Editor {
    pub(super) fn for_language_buffer(
        language_buffer: Entity<LanguageBuffer>,
        cx: &mut Context<Self>,
    ) -> Self {
        Self::from_language_buffer(language_buffer, EditorMode::Full, cx)
    }
}

pub(super) fn test_buffer(
    cx: &mut TestAppContext,
    text: impl Into<String>,
) -> Entity<LanguageBuffer> {
    let buffer =
        Buffer::scratch(text.into(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    let buffer = cx.new(|_| buffer);
    cx.new(|cx| LanguageBuffer::new(buffer, None, cx))
}

pub(super) fn focus_editor(editor: &Entity<Editor>, cx: &mut VisualTestContext) {
    cx.update(|window, cx| {
        let focus = editor.read(cx).focus_handle();
        window.focus(&focus, cx);
    });
}
pub(super) fn engine_buffer(
    buffer: &Entity<LanguageBuffer>,
    cx: &TestAppContext,
) -> Entity<Buffer> {
    cx.read_entity(buffer, |buffer, _| buffer.buffer())
}
pub(super) fn buffer_text(buffer: &Entity<LanguageBuffer>, cx: &TestAppContext) -> String {
    let buffer = engine_buffer(buffer, cx);
    cx.read_entity(&buffer, |buffer, _| {
        buffer
            .slice_byte_range(ByteOffset::ZERO, buffer.len_bytes())
            .expect("完整测试 Buffer 应可读取")
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
        let diff = cx.new(|cx| {
            BufferDiff::new(
                BufferDiffInput {
                    operations: None,
                    working: source.clone(),
                    // 旧测试会用 None 表示“整份文本均为新增”。现在仍由一对真实文本快照
                    // 派生该 Added hunk，而不是注入 hunk。
                    base_text: Some(base_text.unwrap_or_else(|| Arc::from(""))),
                    index_text: None,
                    path: PathBuf::from("src/a.rs"),
                },
                cx,
            )
        });
        editor.set_diff_projection(
            Some(DiffProjection::new(vec![DiffFile {
                diff,
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
                show_file_header: false,
            }])),
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
        let diff = cx.new(|cx| {
            BufferDiff::new(
                BufferDiffInput {
                    operations: None,
                    working: source.clone(),
                    base_text: Some(base_text),
                    index_text: None,
                    path: PathBuf::from("src/a.rs"),
                },
                cx,
            )
        });
        editor.set_diff_projection(
            Some(DiffProjection::new(vec![DiffFile {
                diff,
                display_path: PathBuf::from("src/a.rs"),
                context_lines: None,
                show_file_header: false,
            }])),
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
