//! 原生窗口 + 真实 dispatcher 的组合文档滚动探针。
//!
//! 使用 Application::with_platform(gpui_platform::current_platform(false))，前后台任务由平台真实执行器调度，避免把测试调度器在事件里排空后台任务的耗时误算成滚轮处理延迟。
//!
//! 运行：
//!   cargo bench --offline -p zcv-benchmarks --bench native_dispatcher_scroll_probe
//!
//! 可用环境变量：
//!   ZCV_PROBE_FILES     组合文档文件数（默认 300）
//!   ZCV_PROBE_INITIAL   打开窗口时先挂载的文件数；其余文件在窗口打开后逐个 add_diff，
//!                       用于复现真实「边加载组合文档边滚动」的场景（默认等于 FILES，即全部预置）
//!   ZCV_PROBE_STAGED    true/1 使用只读暂存组合文档（默认未暂存可编辑）

use std::cell::Cell;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::time::{Duration, Instant};

use gpui::{
    App, AppContext as _, Application, AsyncApp, Bounds, Context, DispatchPhase, Entity,
    IntoElement, PlatformInput, Render, ScrollDelta, ScrollWheelEvent, Window, WindowBounds,
    WindowHandle, WindowOptions, point, px, size,
};
use zcv_assets::Assets;
use zcv_buffer_diff::{BufferDiff, BufferDiffInput};
use zcv_editor::Editor;
use zcv_language::{LanguageBuffer, LanguageRegistry};
use zcv_multi_buffer::{DiffExcerptRanges, DiffFile, MultiBuffer};
use zcv_text::{Buffer, BufferConfig};

const DEFAULT_FILE_COUNT: usize = 300;
const ROWS_PER_FILE: usize = 50;
const WARMUP_EVENTS: usize = 8;
const MEASURED_EVENTS: usize = 240;
const FRAME_WAIT_TIMEOUT: Duration = Duration::from_millis(500);

#[derive(Clone, Copy)]
struct Sample {
    /// 注册下一帧回调的同步耗时（反映主线程当前是否被占用）。
    prepare: Duration,
    /// window.dispatch_event(ScrollWheel) 的同步耗时。
    dispatch: Duration,
    /// 从派发到下一帧回调触发的耗时（真实「输入到绘制」）。
    frame: Option<Duration>,
    /// 从开始本次测量到下一帧回调触发的总耗时。
    total: Option<Duration>,
}

/// 根视图：承载 Editor，并注册一个只计数的滚动监听。
///
/// Editor 的滚动监听在 Bubble 阶段会 stop_propagation，因此本监听收到事件说明该次滚动
/// 没有被编辑器消费。用「未消费计数」间接确认滚动确实命中了编辑器。
struct ProbeRoot {
    editor: Entity<Editor>,
    unconsumed_scrolls: Rc<Cell<usize>>,
}

impl Render for ProbeRoot {
    fn render(&mut self, window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let counter = self.unconsumed_scrolls.clone();
        window.on_mouse_event(move |_: &ScrollWheelEvent, phase, _window, _cx| {
            if phase == DispatchPhase::Bubble {
                counter.set(counter.get() + 1);
            }
        });
        self.editor.clone()
    }
}

fn env_usize(name: &str, default: usize) -> usize {
    std::env::var(name)
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(default)
}

fn env_bool(name: &str, default: bool) -> bool {
    std::env::var(name).ok().map_or(default, |value| {
        value == "1" || value.eq_ignore_ascii_case("true")
    })
}

/// 构建组合 diff 编辑器，并把 initial 之后的文件作为待增量挂载项返回。
fn build_editor(
    cx: &mut App,
    file_count: usize,
    initial: usize,
    staged: bool,
) -> (Entity<Editor>, Vec<DiffFile>) {
    let registry = Arc::new(LanguageRegistry::new());
    let base = (0..ROWS_PER_FILE)
        .map(|row| format!("old\t{row} {}\n", "中文 abcdefghij ".repeat(20)))
        .collect::<String>();
    let working = base.replace("old", "new");
    let mut files: Vec<DiffFile> = (0..file_count)
        .map(|file| {
            let path = PathBuf::from(format!("src/file_{file:03}.rs"));
            let buffer = Buffer::from_text(working.clone(), BufferConfig::default())
                .expect("夹具文本必须合法 UTF-8");
            let source =
                cx.new(|cx| LanguageBuffer::new(buffer, Some(path.clone()), registry.clone(), cx));
            let diff = cx.new(|cx| {
                BufferDiff::new(
                    BufferDiffInput {
                        working: source,
                        path: path.clone(),
                        base_text: Some(Arc::from(base.clone())),
                        index_text: Some(Arc::from(if staged {
                            working.clone()
                        } else {
                            base.clone()
                        })),
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
    let multi = cx.new(if staged {
        MultiBuffer::empty_read_only
    } else {
        MultiBuffer::empty
    });
    multi.update(cx, |buffer, cx| {
        buffer.set_diff_hunks_expanded_by_default(true, cx);
        buffer.set_diff_files(files.drain(..initial.min(files.len())).collect(), cx);
    });
    let editor = cx.new(|cx| Editor::for_multi_buffer(multi, cx));
    (editor, files)
}

/// 等待下一次帧回调。on_next_frame 会主动 schedule_frame，即使窗口不脏也会触发。
async fn wait_frames(cx: &mut AsyncApp, handle: WindowHandle<ProbeRoot>, frames: usize) {
    for _ in 0..frames {
        let fired = Rc::new(Cell::new(false));
        handle
            .update(cx, |_, window, _| {
                let fired = fired.clone();
                window.on_next_frame(move |_, _| fired.set(true));
            })
            .ok();
        let deadline = Instant::now() + FRAME_WAIT_TIMEOUT;
        while !fired.get() && Instant::now() < deadline {
            cx.background_executor()
                .timer(Duration::from_millis(1))
                .await;
        }
    }
}

async fn measure_scroll(cx: &mut AsyncApp, handle: WindowHandle<ProbeRoot>, delta: f32) -> Sample {
    let measure_started = Instant::now();
    let frame_time = Rc::new(Cell::new(None::<Instant>));
    handle
        .update(cx, |_, window, _| {
            let slot = frame_time.clone();
            window.on_next_frame(move |_, _| slot.set(Some(Instant::now())));
        })
        .ok();
    let prepare = measure_started.elapsed();

    let started = Instant::now();
    handle
        .update(cx, |_, window, cx| {
            window.dispatch_event(
                PlatformInput::ScrollWheel(ScrollWheelEvent {
                    position: point(px(300.), px(300.)),
                    delta: ScrollDelta::Pixels(point(px(0.), px(delta))),
                    ..Default::default()
                }),
                cx,
            );
        })
        .ok();
    let dispatch = started.elapsed();

    let deadline = Instant::now() + FRAME_WAIT_TIMEOUT;
    while frame_time.get().is_none() && Instant::now() < deadline {
        cx.background_executor()
            .timer(Duration::from_millis(1))
            .await;
    }
    Sample {
        prepare,
        dispatch,
        frame: frame_time.get().map(|t| t.duration_since(started)),
        total: frame_time.get().map(|t| t.duration_since(measure_started)),
    }
}

async fn measure_phase(cx: &mut AsyncApp, handle: WindowHandle<ProbeRoot>, label: &str) {
    for frame in 0..WARMUP_EVENTS {
        let delta = [-120., -1_200., 120., 1_200.][frame % 4];
        let _ = measure_scroll(cx, handle, delta).await;
    }
    let phase_started = Instant::now();
    let mut samples = Vec::with_capacity(MEASURED_EVENTS);
    for frame in 0..MEASURED_EVENTS {
        let delta = [-120., -1_200., 120., 1_200.][frame % 4];
        samples.push(measure_scroll(cx, handle, delta).await);
    }
    let wall = phase_started.elapsed().as_secs_f64() * 1_000.;
    println!("--- {label}：{MEASURED_EVENTS} 次滚动，墙钟 {wall:.1} 毫秒 ---");
    report("注册", &samples, |sample| sample.prepare);
    report("派发", &samples, |sample| sample.dispatch);
    report("派发到绘制", &samples, |sample| {
        sample.frame.unwrap_or(Duration::ZERO)
    });
    report("测量到绘制", &samples, |sample| {
        sample.total.unwrap_or(Duration::ZERO)
    });
}

fn report(label: &str, samples: &[Sample], value: impl Fn(&Sample) -> Duration) {
    report_values(
        label,
        samples
            .iter()
            .map(|sample| value(sample).as_secs_f64() * 1_000.)
            .collect(),
    );
}

fn report_durations(label: &str, durations: &[Duration]) {
    report_values(
        label,
        durations
            .iter()
            .map(|duration| duration.as_secs_f64() * 1_000.)
            .collect(),
    );
}

fn report_values(label: &str, mut values: Vec<f64>) {
    values.sort_by(f64::total_cmp);
    let len = values.len();
    let median = values[len / 2];
    let p95 = values[(len * 95 / 100).min(len - 1)];
    println!(
        "  {label}：中位数={median:.3} 毫秒，P95={p95:.3}，最大={:.3}，最小={:.3}",
        values[len - 1],
        values[0]
    );
}

async fn run_probe(
    cx: &mut AsyncApp,
    handle: WindowHandle<ProbeRoot>,
    unconsumed_scrolls: Rc<Cell<usize>>,
    file_count: usize,
    initial: usize,
    remaining: Vec<DiffFile>,
    staged: bool,
) {
    wait_frames(cx, handle, 2).await;

    // 冷阶段：窗口首帧后尽快派发第一次滚动，此时初始软换行重排通常仍在进行。
    let cold_started = Instant::now();
    let cold = measure_scroll(cx, handle, -8_000.).await;
    let cold_wall = cold_started.elapsed().as_secs_f64() * 1_000.;
    println!(
        "组合 diff 原生窗口：文件={file_count}，初始挂载={initial}，暂存={staged}；冷启动首次滚动：注册={:.3} 毫秒，派发={:.3} 毫秒，派发到绘制={:.3} 毫秒，测量到绘制={:.3} 毫秒（墙钟 {cold_wall:.3}）",
        cold.prepare.as_secs_f64() * 1_000.,
        cold.dispatch.as_secs_f64() * 1_000.,
        cold.frame.map_or(0., |d| d.as_secs_f64() * 1_000.),
        cold.total.map_or(0., |d| d.as_secs_f64() * 1_000.),
    );

    let unconsumed_before = unconsumed_scrolls.get();
    measure_phase(cx, handle, "稳定滚动").await;

    // 增量挂载：窗口打开后逐个 add_diff，并在每次挂载后立即测量滚动输入。
    // 这一段复现真实 DiffView 边加载组合文档边滚动的时序。
    if !remaining.is_empty() {
        let load_count = remaining.len();
        let mut add_times = Vec::with_capacity(load_count);
        let mut scrolls = Vec::with_capacity(load_count);
        let phase_started = Instant::now();
        for (index, file) in remaining.into_iter().enumerate() {
            let add_started = Instant::now();
            handle
                .update(cx, |root, _, cx| {
                    root.editor.update(cx, |editor, cx| {
                        editor.add_diff(file, cx);
                    });
                })
                .ok();
            add_times.push(add_started.elapsed());
            let delta = [-120., -1_200., 120., 1_200.][index % 4];
            scrolls.push(measure_scroll(cx, handle, delta).await);
        }
        println!(
            "--- 增量挂载 {load_count} 个文件期间，墙钟 {:.1} 毫秒 ---",
            phase_started.elapsed().as_secs_f64() * 1_000.
        );
        report_durations("add_diff", &add_times);
        report("派发", &scrolls, |sample| sample.dispatch);
        report("派发到绘制", &scrolls, |sample| {
            sample.frame.unwrap_or(Duration::ZERO)
        });
        report("测量到绘制", &scrolls, |sample| {
            sample.total.unwrap_or(Duration::ZERO)
        });
    }

    // 触发一次完整重排：窗口宽度变化会让软换行层全量重算。
    handle
        .update(cx, |_, window, _| {
            window.resize(size(px(960.), px(800.)));
        })
        .ok();
    wait_frames(cx, handle, 1).await;
    measure_phase(cx, handle, "宽度变化后重排期间").await;

    let unconsumed = unconsumed_scrolls.get() - unconsumed_before;
    println!(
        "编辑器未消费的滚动事件数={unconsumed}（0 表示全部命中编辑器并触发滚动；大于 0 说明部分事件未命中）"
    );
    cx.update(|cx| cx.quit());
}

fn main() {
    let file_count = env_usize("ZCV_PROBE_FILES", DEFAULT_FILE_COUNT);
    let initial = env_usize("ZCV_PROBE_INITIAL", file_count).min(file_count);
    let staged = env_bool("ZCV_PROBE_STAGED", false);
    Application::with_platform(gpui_platform::current_platform(false))
        .with_assets(Assets)
        .run(move |cx: &mut App| {
            Assets.load_fonts(cx).expect("内置字体应能注册");
            zcv_settings::init(cx);
            let (editor, remaining) = build_editor(cx, file_count, initial, staged);
            let unconsumed_scrolls = Rc::new(Cell::new(0));
            let root = cx.new(|_| ProbeRoot {
                editor,
                unconsumed_scrolls: unconsumed_scrolls.clone(),
            });
            let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
            let handle = cx
                .open_window(
                    WindowOptions {
                        window_bounds: Some(WindowBounds::Windowed(bounds)),
                        focus: true,
                        show: true,
                        ..Default::default()
                    },
                    move |_, _| root,
                )
                .expect("探针窗口应能创建");
            cx.activate(true);
            cx.spawn(async move |cx| {
                run_probe(
                    cx,
                    handle,
                    unconsumed_scrolls,
                    file_count,
                    initial,
                    remaining,
                    staged,
                )
                .await;
            })
            .detach();
        });
}
