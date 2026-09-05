//! ANSI/256 色解析：把 alacritty 的颜色枚举映射为 RGBA。
//!
//! ANSI 16 色与 dim 变体从当前主题读取（`terminal.ansi.*` token，跟随主题切换）；
//! xterm 256 色算法与主题无关，在此内联实现。

use gpui::{App, Rgba, Window};
use zcv_theme::color;

use alacritty_terminal::vte::ansi::{Color, NamedColor};

const fn rgb_u8(r: u8, g: u8, b: u8) -> Rgba {
    Rgba {
        r: r as f32 / 255.,
        g: g as f32 / 255.,
        b: b as f32 / 255.,
        a: 1.,
    }
}

/// 把 alacritty 颜色枚举解析为 RGBA。
pub(crate) fn color_to_rgba(color: &Color, _window: &Window, cx: &App) -> Rgba {
    match color {
        Color::Named(NamedColor::Foreground) => color::current(cx).text,
        Color::Named(NamedColor::Background) => color::current(cx).editor_background,
        Color::Named(named) => {
            let index = named_index(*named);
            if index >= 16 {
                theme_ansi_dim_colors(cx)[index - 16]
            } else {
                theme_ansi_colors(cx)[index]
            }
        }
        Color::Indexed(index) => indexed_color(*index),
        Color::Spec(rgb) => rgb_u8(rgb.r, rgb.g, rgb.b),
    }
}

/// OSC 颜色查询（ColorRequest）的兜底：按索引查 16 色表 / 256 色算法 / 语义占位。
pub(crate) fn get_color_at_index(index: usize, _window: &Window, cx: &App) -> Rgba {
    match index {
        0..=15 => theme_ansi_colors(cx)[index],
        16..=255 => indexed_color(index as u8),
        // 256 前景 / 257 背景 / 258 光标：回退主题语义色。
        256 => color::current(cx).text,
        257 => color::current(cx).editor_background,
        258 => color::current(cx).editor_cursor,
        // 259..267 为 dim 变体。
        259..=266 => theme_ansi_dim_colors(cx)[index - 259],
        267 => color::current(cx).text,
        268 => color::current(cx).editor_background,
        _ => rgb_u8(128, 128, 128),
    }
}

/// Named 枚举 → 16 色表索引。
pub(crate) fn named_index(named: NamedColor) -> usize {
    match named {
        NamedColor::Black => 0,
        NamedColor::Red => 1,
        NamedColor::Green => 2,
        NamedColor::Yellow => 3,
        NamedColor::Blue => 4,
        NamedColor::Magenta => 5,
        NamedColor::Cyan => 6,
        NamedColor::White => 7,
        NamedColor::BrightBlack => 8,
        NamedColor::BrightRed => 9,
        NamedColor::BrightGreen => 10,
        NamedColor::BrightYellow => 11,
        NamedColor::BrightBlue => 12,
        NamedColor::BrightMagenta => 13,
        NamedColor::BrightCyan => 14,
        NamedColor::BrightWhite => 15,
        // 暗化变体映射到 dim 色表（索引 16..23）。
        NamedColor::DimBlack => 16,
        NamedColor::DimRed => 17,
        NamedColor::DimGreen => 18,
        NamedColor::DimYellow => 19,
        NamedColor::DimBlue => 20,
        NamedColor::DimMagenta => 21,
        NamedColor::DimCyan => 22,
        NamedColor::DimWhite => 23,
        NamedColor::Cursor
        | NamedColor::DimForeground
        | NamedColor::BrightForeground
        | NamedColor::Foreground
        | NamedColor::Background => 7,
    }
}

/// 主题的 ANSI 16 色表（`terminal.ansi.*`，跟随应用主题）。
fn theme_ansi_colors(cx: &App) -> [Rgba; 16] {
    let c = color::current(cx);
    [
        c.terminal_ansi_black,
        c.terminal_ansi_red,
        c.terminal_ansi_green,
        c.terminal_ansi_yellow,
        c.terminal_ansi_blue,
        c.terminal_ansi_magenta,
        c.terminal_ansi_cyan,
        c.terminal_ansi_white,
        c.terminal_ansi_bright_black,
        c.terminal_ansi_bright_red,
        c.terminal_ansi_bright_green,
        c.terminal_ansi_bright_yellow,
        c.terminal_ansi_bright_blue,
        c.terminal_ansi_bright_magenta,
        c.terminal_ansi_bright_cyan,
        c.terminal_ansi_bright_white,
    ]
}

/// 主题的 dim 变体表（DimBlack..DimWhite，索引 16..23）。
fn theme_ansi_dim_colors(cx: &App) -> [Rgba; 8] {
    let c = color::current(cx);
    [
        c.terminal_ansi_dim_black,
        c.terminal_ansi_dim_red,
        c.terminal_ansi_dim_green,
        c.terminal_ansi_dim_yellow,
        c.terminal_ansi_dim_blue,
        c.terminal_ansi_dim_magenta,
        c.terminal_ansi_dim_cyan,
        c.terminal_ansi_dim_white,
    ]
}

/// xterm 256 色：16 个基础色 + 6×6×6 立方体 + 24 级灰阶。
fn indexed_color(index: u8) -> Rgba {
    const CUBE_LEVELS: [u8; 6] = [0, 95, 135, 175, 215, 255];
    match index {
        // 0..15 为标准 ANSI 色，由 Named 路径处理；此处兜底灰色。
        0..=15 => rgb_u8(128, 128, 128),
        16..=231 => {
            let value = index - 16;
            let (r, g, b) = (
                CUBE_LEVELS[(value / 36) as usize],
                CUBE_LEVELS[(value / 6 % 6) as usize],
                CUBE_LEVELS[(value % 6) as usize],
            );
            rgb_u8(r, g, b)
        }
        232..=255 => {
            let level = 8 + (index - 232) * 10;
            rgb_u8(level, level, level)
        }
    }
}
