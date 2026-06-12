//! L1 视觉 token —— 命名常量。
//!
//! 颜色按手册 §4 的两轴正交（hue × step）命名；值从当前 [`Palette`] 读取，
//! 切换主题时通过 [`set_palette`] 原子替换。
//!
//! 每个色相提供两条平行阶梯（手册 §4.3）：
//! - `sNN` solid，不透明 RGB —— 文字、边框、caret、实心填充
//! - `aNN` alpha，半透明 RGBA —— 选区背景、搜索高亮等需要让下层透过的色块
//!
//! 9 档语义（手册 §4.2）跨色相严格对齐：
//!
//! | 档 | 角色 |
//! |---|---|
//! | 01 | app background |
//! | 02 | surface |
//! | 03 | ui-bg（常态） |
//! | 04 | ui-active（active / selected / hover 后） |
//! | 05 | border-subtle |
//! | 06 | border-focus |
//! | 07 | solid（实心强调） |
//! | 08 | text-muted |
//! | 09 | text |

use std::sync::{OnceLock, RwLock};

use gpui::{Rgba, rgb, rgba};

use crate::config::{THEME_ONE_DARK, THEME_ONE_LIGHT};

/// 单个色相的 solid + alpha 双阶梯。
///
/// `s01`–`s09` 是不透明 RGB，`a01`–`a09` 是半透明 RGBA。
/// 语义角色固定后阶数不变，切换主题时整块替换。
#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub struct HuePalette {
    pub s01: Rgba,
    pub s02: Rgba,
    pub s03: Rgba,
    pub s04: Rgba,
    pub s05: Rgba,
    pub s06: Rgba,
    pub s07: Rgba,
    pub s08: Rgba,
    pub s09: Rgba,
    pub a01: Rgba,
    pub a02: Rgba,
    pub a03: Rgba,
    pub a04: Rgba,
    pub a05: Rgba,
    pub a06: Rgba,
    pub a07: Rgba,
    pub a08: Rgba,
    pub a09: Rgba,
}

/// 全 UI 配色表 —— 5 个色相 × 双阶梯。
///
/// 调用方通过 [`current`] 获取当前主题的 palette，直接访问字段：
/// `color::current().gray.s01`、`color::current().blue.s07` 等。
#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub gray: HuePalette,
    pub blue: HuePalette,
    pub green: HuePalette,
    pub yellow: HuePalette,
    pub red: HuePalette,
}

/// 进程级当前调色板。写端：`set_palette`。读端：`current`。
static ACTIVE_PALETTE: OnceLock<RwLock<Palette>> = OnceLock::new();

impl Palette {
    fn one_dark() -> Self {
        Self {
            gray: HuePalette {
                s01: rgb(0x0d0f12), // 01 app background
                s02: rgb(0x13161b), // 02 surface
                s03: rgb(0x1b1f26), // 03 ui-bg
                s04: rgb(0x252a33), // 04 ui-active
                s05: rgb(0x3a4050), // 05 border-subtle
                s06: rgb(0x4d5468), // 06 border-focus
                s07: rgb(0x5e6678), // 07 solid
                s08: rgb(0x6e7689), // 08 text-muted
                s09: rgb(0xa8b0c0), // 09 text
                a01: rgba(0xffffff1a),
                a02: rgba(0xffffff26),
                a03: rgba(0xffffff3d),
                a04: rgba(0xffffff66),
                a05: rgba(0xffffff80),
                a06: rgba(0xffffffb3),
                a07: rgba(0xffffffcc),
                a08: rgba(0xffffffe6),
                a09: rgba(0xfffffff5),
            },
            blue: HuePalette {
                s01: rgb(0x0e1a2e),
                s02: rgb(0x122544),
                s03: rgb(0x173159),
                s04: rgb(0x1c3d6e),
                s05: rgb(0x284a85),
                s06: rgb(0x5a8fd6),
                s07: rgb(0x74ade8), // caret、品牌色填充
                s08: rgb(0x94c1f0),
                s09: rgb(0xc8def7),
                a01: rgba(0x74ade81a),
                a02: rgba(0x74ade826),
                a03: rgba(0x74ade83d),
                a04: rgba(0x74ade848), // 选区色块
                a05: rgba(0x74ade84d), // 搜索普通命中
                a06: rgba(0x74ade8b3),
                a07: rgba(0x74ade8cc),
                a08: rgba(0x74ade8e6),
                a09: rgba(0x74ade8f5),
            },
            green: HuePalette {
                s01: rgb(0x0a2b1b),
                s02: rgb(0x103e28),
                s03: rgb(0x155235),
                s04: rgb(0x1a6541),
                s05: rgb(0x20784e),
                s06: rgb(0x2cab69),
                s07: rgb(0x3ddc84), // success
                s08: rgb(0x79e8a8),
                s09: rgb(0xb5f0c8),
                a01: rgba(0x3ddc841a),
                a02: rgba(0x3ddc8426),
                a03: rgba(0x3ddc843d),
                a04: rgba(0x3ddc8440),
                a05: rgba(0x3ddc844d),
                a06: rgba(0x3ddc84b3),
                a07: rgba(0x3ddc84cc),
                a08: rgba(0x3ddc84e6),
                a09: rgba(0x3ddc84f5),
            },
            yellow: HuePalette {
                s01: rgb(0x2d2410),
                s02: rgb(0x43361a),
                s03: rgb(0x5c4824),
                s04: rgb(0x715a2c),
                s05: rgb(0x8a6d36),
                s06: rgb(0xb0934d),
                s07: rgb(0xe8cf74), // warning
                s08: rgb(0xf0dc99),
                s09: rgb(0xf7ebbf),
                a01: rgba(0xe8cf741a),
                a02: rgba(0xe8cf7426),
                a03: rgba(0xe8cf743d),
                a04: rgba(0xe8cf7440),
                a05: rgba(0xe8cf744d), // 当前搜索命中
                a06: rgba(0xe8cf74b3),
                a07: rgba(0xe8cf74cc),
                a08: rgba(0xe8cf74e6),
                a09: rgba(0xe8cf74f5),
            },
            red: HuePalette {
                s01: rgb(0x2e0e0e),
                s02: rgb(0x441616),
                s03: rgb(0x5b2020),
                s04: rgb(0x732a2a),
                s05: rgb(0x8b3434),
                s06: rgb(0xbf4949),
                s07: rgb(0xff6b6b), // danger / error
                s08: rgb(0xff9595),
                s09: rgb(0xffc2c2),
                a01: rgba(0xff6b6b1a),
                a02: rgba(0xff6b6b26),
                a03: rgba(0xff6b6b3d),
                a04: rgba(0xff6b6b40),
                a05: rgba(0xff6b6b4d),
                a06: rgba(0xff6b6bb3),
                a07: rgba(0xff6b6bcc),
                a08: rgba(0xff6b6be6),
                a09: rgba(0xff6b6bf5),
            },
        }
    }

    fn one_light() -> Self {
        Self {
            gray: HuePalette {
                s01: rgb(0xfafafa),
                s02: rgb(0xebebeb),
                s03: rgb(0xdfdfdf),
                s04: rgb(0xd3d3d3),
                s05: rgb(0xbcbcbc),
                s06: rgb(0xa6a6a6),
                s07: rgb(0x919191),
                s08: rgb(0x838383),
                s09: rgb(0x1e1e1e),
                a01: rgba(0x0000001a),
                a02: rgba(0x00000026),
                a03: rgba(0x0000003d),
                a04: rgba(0x00000066),
                a05: rgba(0x00000080),
                a06: rgba(0x000000b3),
                a07: rgba(0x000000cc),
                a08: rgba(0x000000e6),
                a09: rgba(0x000000f5),
            },
            blue: HuePalette {
                s01: rgb(0xeef3fe),
                s02: rgb(0xdce7fd),
                s03: rgb(0xc6d8fb),
                s04: rgb(0xacc6f8),
                s05: rgb(0x8baef3),
                s06: rgb(0x6293ed),
                s07: rgb(0x2563eb),
                s08: rgb(0x1d4ab8),
                s09: rgb(0x162e7a),
                a01: rgba(0x2563eb1a),
                a02: rgba(0x2563eb26),
                a03: rgba(0x2563eb3d),
                a04: rgba(0x2563eb40),
                a05: rgba(0x2563eb4d),
                a06: rgba(0x2563ebb3),
                a07: rgba(0x2563ebcc),
                a08: rgba(0x2563ebe6),
                a09: rgba(0x2563ebf5),
            },
            green: HuePalette {
                s01: rgb(0xecf8ef),
                s02: rgb(0xd8f0dd),
                s03: rgb(0xc0e6c7),
                s04: rgb(0xa3daae),
                s05: rgb(0x7ec98f),
                s06: rgb(0x4eb56a),
                s07: rgb(0x16a34a),
                s08: rgb(0x0f7a35),
                s09: rgb(0x0a4d21),
                a01: rgba(0x16a34a1a),
                a02: rgba(0x16a34a26),
                a03: rgba(0x16a34a3d),
                a04: rgba(0x16a34a40),
                a05: rgba(0x16a34a4d),
                a06: rgba(0x16a34ab3),
                a07: rgba(0x16a34acc),
                a08: rgba(0x16a34ae6),
                a09: rgba(0x16a34af5),
            },
            yellow: HuePalette {
                s01: rgb(0xfffef4),
                s02: rgb(0xfefada),
                s03: rgb(0xfdf0a8),
                s04: rgb(0xfce776),
                s05: rgb(0xfad644),
                s06: rgb(0xe8c01a),
                s07: rgb(0xca8a04),
                s08: rgb(0x9e6c04),
                s09: rgb(0x6b4808),
                a01: rgba(0xca8a041a),
                a02: rgba(0xca8a0426),
                a03: rgba(0xca8a043d),
                a04: rgba(0xca8a0440),
                a05: rgba(0xca8a044d),
                a06: rgba(0xca8a04b3),
                a07: rgba(0xca8a04cc),
                a08: rgba(0xca8a04e6),
                a09: rgba(0xca8a04f5),
            },
            red: HuePalette {
                s01: rgb(0xfef4f4),
                s02: rgb(0xfde4e4),
                s03: rgb(0xfccccc),
                s04: rgb(0xf9adad),
                s05: rgb(0xf58686),
                s06: rgb(0xee5555),
                s07: rgb(0xdc2626),
                s08: rgb(0xb91c1c),
                s09: rgb(0x7f1d1d),
                a01: rgba(0xdc26261a),
                a02: rgba(0xdc262626),
                a03: rgba(0xdc26263d),
                a04: rgba(0xdc262640),
                a05: rgba(0xdc26264d),
                a06: rgba(0xdc2626b3),
                a07: rgba(0xdc2626cc),
                a08: rgba(0xdc2626e6),
                a09: rgba(0xdc2626f5),
            },
        }
    }
}

/// 按主题名切换调色板。不匹配时静默回退 one-dark。
pub(crate) fn set_palette(theme: &str) {
    let palette = match theme {
        THEME_ONE_DARK => Palette::one_dark(),
        THEME_ONE_LIGHT => Palette::one_light(),
        _ => Palette::one_dark(),
    };
    let lock = ACTIVE_PALETTE.get_or_init(|| RwLock::new(Palette::one_dark()));
    match lock.write() {
        Ok(mut active) => *active = palette,
        Err(error) => eprintln!("更新调色板失败：{error}"),
    }
}

/// 当前主题调色板。调用方直接访问字段：
pub fn current() -> Palette {
    ACTIVE_PALETTE
        .get_or_init(|| RwLock::new(Palette::one_dark()))
        .read()
        .map(|p| *p)
        .unwrap_or_else(|_| Palette::one_dark())
}

// ---------- 语义色（跨色相、不受 hue 命名的颜色角色）----------

/// 选区背景色。
pub fn selection_bg() -> Rgba {
    current().blue.a04
}
/// 搜索普通命中背景色。
pub fn search_normal_bg() -> Rgba {
    current().blue.a05
}
/// 搜索当前命中背景色（暖黄）。
pub fn search_current_bg() -> Rgba {
    current().yellow.a05
}
