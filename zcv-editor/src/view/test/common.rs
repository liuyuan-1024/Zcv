use super::*;
use crate::scrollbar::{SCROLLBAR_WIDTH, thumb_geometry};
use gpui::{Bounds, Pixels, TestAppContext, VisualTestContext, point, size};
use zcv_engine::{Buffer, BufferConfig, ByteOffset};

pub(super) fn test_buffer(
    cx: &mut TestAppContext,
    text: impl Into<String>,
) -> Entity<LanguageBuffer> {
    let buffer =
        Buffer::scratch(text.into(), BufferConfig::default()).expect("测试 Buffer 应能创建");
    let buffer = cx.new(|_| buffer);
    cx.new(|cx| LanguageBuffer::new(buffer, None, cx))
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
pub(super) fn scrolling_text() -> String {
    (0..100)
        .map(|row| format!("line {row}\n"))
        .collect::<String>()
}
