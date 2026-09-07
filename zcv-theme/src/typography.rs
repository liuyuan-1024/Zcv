//! 排版 token：字号、字体栈与行高。
//!
//! 垂直体系的唯一基准是墨迹高度：
//! 行高派生自启动时对字体栈的实测墨迹（完全容纳字形墨迹的最小行盒），保证「行盒 ⊇ 墨迹」恒成立。
//! overflow_hidden 容器不裁剪字形，padding 自墨迹盒边缘起算，同一值在所有组件上视觉一致；
//! 需要呼吸感的场景由调用方叠加 padding，不引入另一套行高标准。
//!
//! 字形尺度的标准是 em 字号（用户设置，跨工具通用）；墨迹是测量结果，只作行高基准，不反向定义字号。
//!
//! 运行时状态是进程级单一快照 [`STATE`]：`set_typography` 写入设置并重测墨迹，读取零散落在各 token 函数。

use std::sync::{OnceLock, RwLock};

use gpui::{
    App, Font, FontFallbacks, Pixels, SharedString, TextRun, WindowTextSystem, black, font, px,
};

/// 编译期嵌入的内置设置文件（与 zcv-assets 运行时嵌入为同一文件）：
/// 排版默认值（字号/行高倍数）的唯一数据源，不重复硬编码。
const INITIAL_SETTINGS: &str = include_str!("../../assets/settings/initial_user_settings.json");

/// 内置设置文件的排版默认值；命名字段杜绝位置索引取错。
#[derive(Clone, Copy)]
struct Defaults {
    ui_size: f32,
    content_size: f32,
    /// 内容行高倍数（相对字号）。
    content_line_height: f32,
}

/// 内置默认值（唯一数据源：编译期嵌入的设置文件），只解析一次。
fn defaults() -> Defaults {
    static DEFAULTS: OnceLock<Defaults> = OnceLock::new();
    *DEFAULTS.get_or_init(|| {
        let value: serde_json::Value =
            serde_json::from_str(INITIAL_SETTINGS).expect("内置设置文件应合法");
        let get = |key: &str| value[key].as_f64().expect("内置默认应存在") as f32;
        Defaults {
            ui_size: get("ui_font_size"),
            content_size: get("content_font_size"),
            content_line_height: get("content_line_height"),
        }
    })
}

/// 排版运行时快照：设置值与由设置派生的墨迹测量值同属一份事实，整体读写。
#[derive(Clone, Copy)]
struct TypographyState {
    ui_size: f32,
    content_size: f32,
    /// 内容行高倍数（相对字号）。
    content_line_multiplier: f32,
    /// UI 字体栈墨迹实测值；0 表示尚未测量，读取时回退黄金比例。
    ui_ink: f32,
    /// 内容字体栈墨迹实测值；0 表示尚未测量，读取时回退黄金比例。
    content_ink: f32,
}

static STATE: RwLock<TypographyState> = RwLock::new(TypographyState {
    ui_size: 0.,
    content_size: 0.,
    content_line_multiplier: 0.,
    ui_ink: 0.,
    content_ink: 0.,
});

/// 读取当前快照。
fn state() -> TypographyState {
    *STATE.read().expect("排版状态锁不应中毒")
}

// ── 字号与行高 ─────────────────────────────────────────────────────

/// UI 字号（em）；未注入设置时回退内置默认。
pub fn ui_size() -> Pixels {
    let size = state().ui_size;
    px(if size == 0. { defaults().ui_size } else { size })
}

/// 内容字号（em）；未注入设置时回退内置默认。
pub fn content_size() -> Pixels {
    let size = state().content_size;
    px(if size == 0. {
        defaults().content_size
    } else {
        size
    })
}

/// UI 行高 = UI 墨迹高度：完全容纳 UI 字体（含 CJK 回退）墨迹的最小行盒，`set_typography` 时实测。
/// 全项目默认行高以此为准；测量前（无文本系统上下文）保守回退黄金比例：1.618 倍恒大于常见字体的墨迹比。
pub fn ui_line() -> Pixels {
    let state = state();
    let size = if state.ui_size == 0. {
        defaults().ui_size
    } else {
        state.ui_size
    };
    ink_or_golden(state.ui_ink, size)
}

/// 内容行高：在内容墨迹高度（唯一垂直基准）之上按用户倍数衍生 `round(字号 × 倍数)`；
/// 不低于墨迹：倍数调小也不会让行盒小于墨迹（行盒 ⊇ 墨迹不变式）。
pub fn content_line() -> Pixels {
    let state = state();
    let size = if state.content_size == 0. {
        defaults().content_size
    } else {
        state.content_size
    };
    let multiplier = if state.content_line_multiplier == 0. {
        defaults().content_line_height
    } else {
        state.content_line_multiplier
    };
    px((size * multiplier).round()).max(ink_or_golden(state.content_ink, size))
}

/// 墨迹实测值；0（尚未测量）时回退黄金比例行高。
fn ink_or_golden(ink: f32, size: f32) -> Pixels {
    if ink == 0. {
        px((size * 1.618_034).round())
    } else {
        px(ink)
    }
}

// ── 字体栈 ─────────────────────────────────────────────────────────

/// UI 字体：gpui 系统字体（比例，界面文案更圆润），非 ASCII 回退到项目字体资源。
pub fn ui_font() -> Font {
    with_cjk_fallback(font(".SystemUIFont"))
}

/// 内容字体：等宽（代码缩进/列对齐依赖等宽），含 CJK 回退。
pub fn content_font() -> Font {
    with_cjk_fallback(font("JetBrains Mono"))
}

/// 给字体挂载 CJK 回退（Sarasa Mono SC）；回退表只构建一次，之后各次调用共享同一 Arc。
fn with_cjk_fallback(mut font: Font) -> Font {
    static FALLBACKS: OnceLock<FontFallbacks> = OnceLock::new();
    font.fallbacks = Some(
        FALLBACKS
            .get_or_init(|| FontFallbacks::from_fonts(vec!["Sarasa Mono SC".to_string()]))
            .clone(),
    );
    font
}

// ── 墨迹测量 ───────────────────────────────────────────────────────

/// 用文本系统实测指定字体栈的墨迹行高：
/// probe 同时覆盖拉丁与 CJK（触发回退字体），取塑形后整行 ascent+descent（各 run 字体度量的最大值）向上取整。
/// descent 符号跨平台不一致（mac 塑形为正、测试平台为负），取绝对值得基线下方墨迹深度。
fn measure_ink_line(cx: &App, font: Font, font_size: Pixels) -> f32 {
    let probe: SharedString = "A中".into();
    let run = TextRun {
        len: probe.len(),
        font,
        color: black(),
        background_color: None,
        underline: None,
        strikethrough: None,
    };
    let shaped =
        WindowTextSystem::new(cx.text_system().clone()).shape_line(probe, font_size, &[run], None);
    f32::from((shaped.ascent + shaped.descent.abs()).ceil())
}

// ── 设置入口 ───────────────────────────────────────────────────────

/// 应用排版设置（内容/UI 字号与内容行高倍数）；
/// 启动与设置变更时调用，未配置的维度保持当前值不变。
/// 行高派生自墨迹测量（依赖字号与字体栈），在此对两个字体栈统一重测，调用方无需各自维护。
pub fn set_typography(
    cx: &App,
    content_size: Option<f32>,
    ui_size: Option<f32>,
    content_line_height: Option<f32>,
) {
    let mut next = state();
    if let Some(content_size) = content_size {
        next.content_size = content_size;
    }
    if let Some(ui_size) = ui_size {
        next.ui_size = ui_size;
    }
    if let Some(line_height) = content_line_height {
        next.content_line_multiplier = line_height;
    }
    // 墨迹重测基于生效字号：未注入的维度回退内置默认（与 token 读取同一回退规则）。
    let ui_size = if next.ui_size == 0. {
        defaults().ui_size
    } else {
        next.ui_size
    };
    let content_size = if next.content_size == 0. {
        defaults().content_size
    } else {
        next.content_size
    };
    next.ui_ink = measure_ink_line(cx, ui_font(), px(ui_size));
    next.content_ink = measure_ink_line(cx, content_font(), px(content_size));
    *STATE.write().expect("排版状态锁不应中毒") = next;
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    /// UI 行高（=墨迹）必须容纳 UI 字体（含 CJK 回退）的真实墨迹：行盒 ⊇ 墨迹是裁剪容器不切字的前提。
    #[gpui::test]
    fn ui_line_covers_shaped_ink(cx: &mut TestAppContext) {
        cx.update(|cx| {
            set_typography(cx, None, Some(13.), None);
            let line = ui_line();
            assert!(
                line > ui_size(),
                "行高应大于字号：ui_line={line}，ui_size={}",
                ui_size()
            );
            for probe in ["Ag中", "gpqyj_j", "汉字徽章"] {
                let probe_ink = shaped_ink(cx, ui_font(), ui_size(), probe);
                assert!(
                    line >= probe_ink,
                    "行盒应容纳墨迹：probe={probe}，ui_line={line}，墨迹={probe_ink}"
                );
            }
        });
    }

    /// 内容行高同理容纳内容字体墨迹；且不低于墨迹下限（倍数调小也不裁剪的不变式）。
    #[gpui::test]
    fn content_line_covers_shaped_ink(cx: &mut TestAppContext) {
        cx.update(|cx| {
            // 故意把行高倍数调到小于墨迹比：content_line 的墨迹下限应兜底。
            set_typography(cx, Some(16.), None, Some(1.1));
            let line = content_line();
            assert!(
                line > content_size(),
                "行高应大于字号：content_line={line}，content_size={}",
                content_size()
            );
            for probe in ["Ag中", "gpqyj_j", "汉字徽章"] {
                let probe_ink = shaped_ink(cx, content_font(), content_size(), probe);
                assert!(
                    line >= probe_ink,
                    "行盒应容纳墨迹：probe={probe}，content_line={line}，墨迹={probe_ink}"
                );
            }
        });
    }

    /// probe 文本塑形后的墨迹高度（ascent + |descent|）；descent 符号跨平台不一致，取绝对值。
    fn shaped_ink(cx: &App, font: Font, font_size: Pixels, probe: &str) -> Pixels {
        let text: SharedString = probe.into();
        let run = TextRun {
            len: text.len(),
            font,
            color: black(),
            background_color: None,
            underline: None,
            strikethrough: None,
        };
        let shaped = WindowTextSystem::new(cx.text_system().clone()).shape_line(
            text,
            font_size,
            &[run],
            None,
        );
        shaped.ascent + shaped.descent.abs()
    }
}
