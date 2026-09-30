use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;

use gpui::{AppContext as _, ScrollDelta, ScrollWheelEvent, VisualTestAppContext, point, px};
use zcv_assets::Assets;
use zcv_buffer_diff::{BufferDiff, BufferDiffInput};
use zcv_editor::Editor;
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_multi_buffer::{DiffExcerptRanges, DiffFile, MultiBuffer};
use zcv_text::{Buffer, BufferConfig};

fn main() {
    for file_count in [2, 10] {
        let mut visual = VisualTestAppContext::with_asset_source(
            gpui_platform::current_platform(false),
            Arc::new(Assets),
        );
        let editor = visual.update(|cx| {
            Assets.load_fonts(cx).unwrap();
            zcv_settings::init(cx);
            let registry = Arc::new(LanguageRegistry::new());
            let base = (0..1_000)
                .map(|row| format!("old {row} {}\n", "中文 abcdefghij ".repeat(16)))
                .collect::<String>();
            let working = base.replace("old", "new");
            let files = (0..file_count)
                .map(|file| {
                    let path = PathBuf::from(format!("src/file_{file:03}.rs"));
                    let buffer =
                        Buffer::from_text(working.clone(), BufferConfig::default()).unwrap();
                    let source = cx.new(|cx| {
                        LanguageBuffer::new(buffer, Some(path.clone()), registry.clone(), cx)
                    });
                    let diff = cx.new(|cx| {
                        BufferDiff::new(
                            BufferDiffInput {
                                working: source,
                                path: path.clone(),
                                base_text: Some(Arc::from(base.clone())),
                                index_text: Some(Arc::from(base.clone())),
                                language_registry: registry.clone(),
                                key: file as u64,
                                operations: None,
                            },
                            cx,
                        )
                    });
                    DiffFile {
                        diff,
                        display_path: path,
                        excerpt_ranges: DiffExcerptRanges::FullFile,
                    }
                })
                .collect();
            let multi = cx.new(MultiBuffer::empty);
            multi.update(cx, |buffer, cx| {
                buffer.set_diff_hunks_expanded_by_default(true, cx);
                buffer.set_diff_files(files, cx);
            });
            cx.new(|cx| Editor::for_multi_buffer(multi, cx))
        });
        visual.run_until_parked();
        let window = visual
            .open_offscreen_window_default(move |_, _| editor.clone())
            .unwrap();
        visual.run_until_parked();
        visual
            .update_window(window.into(), |_, window, cx| {
                window.draw(cx).clear(cx);
            })
            .unwrap();
        let mut samples = Vec::new();
        for _ in 0..200 {
            let started = Instant::now();
            visual.simulate_event(
                window.into(),
                ScrollWheelEvent {
                    position: point(px(300.), px(300.)),
                    delta: ScrollDelta::Pixels(point(px(0.), px(-120.))),
                    ..Default::default()
                },
            );
            visual
                .update_window(window.into(), |_, window, cx| {
                    window.draw(cx).clear(cx);
                })
                .unwrap();
            samples.push(started.elapsed().as_secs_f64() * 1_000.);
        }
        samples.sort_by(f64::total_cmp);
        println!(
            "原生 CoreText/Metal 组合滚动，文件={file_count}：中位数={:.3} 毫秒，P95={:.3}，范围={:.3}..{:.3}",
            samples[100], samples[189], samples[0], samples[199]
        );
        std::mem::forget(visual);
    }
}
