//! 调色板：两轴正交（色相 × 语义档），切换主题时原子替换。
//!
//! | 索引 | 语义 |
//! |---|---|
//! | 0 | app background |
//! | 1 | surface |
//! | 2 | ui-bg |
//! | 3 | ui-active |
//! | 4 | border-subtle |
//! | 5 | border-focus |
//! | 6 | solid 强调 |
//! | 7 | text-muted |
//! | 8 | text |

use std::sync::{OnceLock, RwLock};

use gpui::{Rgba, rgb, rgba};

use crate::theme::ConcreteTheme;

/// 单个色相的 solid + alpha 双阶梯。索引 0–8 对应语义 01–09。
#[derive(Clone, Copy, Debug)]
pub struct HuePalette {
    pub s: [Rgba; 9],
    pub a: [Rgba; 9],
}

#[derive(Clone, Copy, Debug)]
pub struct Palette {
    pub gray: HuePalette,
    pub blue: HuePalette,
    pub green: HuePalette,
    pub yellow: HuePalette,
    pub red: HuePalette,
}

static ACTIVE: OnceLock<RwLock<Palette>> = OnceLock::new();

impl Palette {
    fn one_dark() -> Self {
        Self {
            gray: HuePalette {
                s: [
                    rgb(0x0d0f12),
                    rgb(0x13161b),
                    rgb(0x1b1f26),
                    rgb(0x252a33),
                    rgb(0x3a4050),
                    rgb(0x4d5468),
                    rgb(0x5e6678),
                    rgb(0x6e7689),
                    rgb(0xa8b0c0),
                ],
                a: [
                    rgba(0xffffff1a),
                    rgba(0xffffff26),
                    rgba(0xffffff3d),
                    rgba(0xffffff66),
                    rgba(0xffffff80),
                    rgba(0xffffffb3),
                    rgba(0xffffffcc),
                    rgba(0xffffffe6),
                    rgba(0xfffffff5),
                ],
            },
            blue: HuePalette {
                s: [
                    rgb(0x0e1a2e),
                    rgb(0x122544),
                    rgb(0x173159),
                    rgb(0x1c3d6e),
                    rgb(0x284a85),
                    rgb(0x5a8fd6),
                    rgb(0x74ade8),
                    rgb(0x94c1f0),
                    rgb(0xc8def7),
                ],
                a: [
                    rgba(0x74ade81a),
                    rgba(0x74ade826),
                    rgba(0x74ade83d),
                    rgba(0x74ade848),
                    rgba(0x74ade84d),
                    rgba(0x74ade8b3),
                    rgba(0x74ade8cc),
                    rgba(0x74ade8e6),
                    rgba(0x74ade8f5),
                ],
            },
            green: HuePalette {
                s: [
                    rgb(0x0a2b1b),
                    rgb(0x103e28),
                    rgb(0x155235),
                    rgb(0x1a6541),
                    rgb(0x20784e),
                    rgb(0x2cab69),
                    rgb(0x3ddc84),
                    rgb(0x79e8a8),
                    rgb(0xb5f0c8),
                ],
                a: [
                    rgba(0x3ddc841a),
                    rgba(0x3ddc8426),
                    rgba(0x3ddc843d),
                    rgba(0x3ddc8440),
                    rgba(0x3ddc844d),
                    rgba(0x3ddc84b3),
                    rgba(0x3ddc84cc),
                    rgba(0x3ddc84e6),
                    rgba(0x3ddc84f5),
                ],
            },
            yellow: HuePalette {
                s: [
                    rgb(0x2d2410),
                    rgb(0x43361a),
                    rgb(0x5c4824),
                    rgb(0x715a2c),
                    rgb(0x8a6d36),
                    rgb(0xb0934d),
                    rgb(0xe8cf74),
                    rgb(0xf0dc99),
                    rgb(0xf7ebbf),
                ],
                a: [
                    rgba(0xe8cf741a),
                    rgba(0xe8cf7426),
                    rgba(0xe8cf743d),
                    rgba(0xe8cf7440),
                    rgba(0xe8cf744d),
                    rgba(0xe8cf74b3),
                    rgba(0xe8cf74cc),
                    rgba(0xe8cf74e6),
                    rgba(0xe8cf74f5),
                ],
            },
            red: HuePalette {
                s: [
                    rgb(0x2e0e0e),
                    rgb(0x441616),
                    rgb(0x5b2020),
                    rgb(0x732a2a),
                    rgb(0x8b3434),
                    rgb(0xbf4949),
                    rgb(0xff6b6b),
                    rgb(0xff9595),
                    rgb(0xffc2c2),
                ],
                a: [
                    rgba(0xff6b6b1a),
                    rgba(0xff6b6b26),
                    rgba(0xff6b6b3d),
                    rgba(0xff6b6b40),
                    rgba(0xff6b6b4d),
                    rgba(0xff6b6bb3),
                    rgba(0xff6b6bcc),
                    rgba(0xff6b6be6),
                    rgba(0xff6b6bf5),
                ],
            },
        }
    }

    fn one_light() -> Self {
        Self {
            gray: HuePalette {
                s: [
                    rgb(0xfafafa),
                    rgb(0xebebeb),
                    rgb(0xdfdfdf),
                    rgb(0xd3d3d3),
                    rgb(0xbcbcbc),
                    rgb(0xa6a6a6),
                    rgb(0x919191),
                    rgb(0x838383),
                    rgb(0x1e1e1e),
                ],
                a: [
                    rgba(0x0000001a),
                    rgba(0x00000026),
                    rgba(0x0000003d),
                    rgba(0x00000066),
                    rgba(0x00000080),
                    rgba(0x000000b3),
                    rgba(0x000000cc),
                    rgba(0x000000e6),
                    rgba(0x000000f5),
                ],
            },
            blue: HuePalette {
                s: [
                    rgb(0xeef3fe),
                    rgb(0xdce7fd),
                    rgb(0xc6d8fb),
                    rgb(0xacc6f8),
                    rgb(0x8baef3),
                    rgb(0x6293ed),
                    rgb(0x2563eb),
                    rgb(0x1d4ab8),
                    rgb(0x162e7a),
                ],
                a: [
                    rgba(0x2563eb1a),
                    rgba(0x2563eb26),
                    rgba(0x2563eb3d),
                    rgba(0x2563eb40),
                    rgba(0x2563eb4d),
                    rgba(0x2563ebb3),
                    rgba(0x2563ebcc),
                    rgba(0x2563ebe6),
                    rgba(0x2563ebf5),
                ],
            },
            green: HuePalette {
                s: [
                    rgb(0xecf8ef),
                    rgb(0xd8f0dd),
                    rgb(0xc0e6c7),
                    rgb(0xa3daae),
                    rgb(0x7ec98f),
                    rgb(0x4eb56a),
                    rgb(0x16a34a),
                    rgb(0x0f7a35),
                    rgb(0x0a4d21),
                ],
                a: [
                    rgba(0x16a34a1a),
                    rgba(0x16a34a26),
                    rgba(0x16a34a3d),
                    rgba(0x16a34a40),
                    rgba(0x16a34a4d),
                    rgba(0x16a34ab3),
                    rgba(0x16a34acc),
                    rgba(0x16a34ae6),
                    rgba(0x16a34af5),
                ],
            },
            yellow: HuePalette {
                s: [
                    rgb(0xfffef4),
                    rgb(0xfefada),
                    rgb(0xfdf0a8),
                    rgb(0xfce776),
                    rgb(0xfad644),
                    rgb(0xe8c01a),
                    rgb(0xca8a04),
                    rgb(0x9e6c04),
                    rgb(0x6b4808),
                ],
                a: [
                    rgba(0xca8a041a),
                    rgba(0xca8a0426),
                    rgba(0xca8a043d),
                    rgba(0xca8a0440),
                    rgba(0xca8a044d),
                    rgba(0xca8a04b3),
                    rgba(0xca8a04cc),
                    rgba(0xca8a04e6),
                    rgba(0xca8a04f5),
                ],
            },
            red: HuePalette {
                s: [
                    rgb(0xfef4f4),
                    rgb(0xfde4e4),
                    rgb(0xfccccc),
                    rgb(0xf9adad),
                    rgb(0xf58686),
                    rgb(0xee5555),
                    rgb(0xdc2626),
                    rgb(0xb91c1c),
                    rgb(0x7f1d1d),
                ],
                a: [
                    rgba(0xdc26261a),
                    rgba(0xdc262626),
                    rgba(0xdc26263d),
                    rgba(0xdc262640),
                    rgba(0xdc26264d),
                    rgba(0xdc2626b3),
                    rgba(0xdc2626cc),
                    rgba(0xdc2626e6),
                    rgba(0xdc2626f5),
                ],
            },
        }
    }
}

pub(crate) fn set_palette(theme: ConcreteTheme) {
    let palette = match theme {
        ConcreteTheme::Dark => Palette::one_dark(),
        ConcreteTheme::Light => Palette::one_light(),
    };
    let lock = ACTIVE.get_or_init(|| RwLock::new(Palette::one_dark()));
    match lock.write() {
        Ok(mut active) => *active = palette,
        Err(error) => eprintln!("更新调色板失败：{error}"),
    }
}

pub fn current() -> Palette {
    ACTIVE
        .get_or_init(|| RwLock::new(Palette::one_dark()))
        .read()
        .map(|p| *p)
        .unwrap_or_else(|_| Palette::one_dark())
}

// ── 主题色 ─────────────────────────────────────────────────────────

pub fn default() -> Rgba {
    current().gray.s[8]
}
pub fn highlight() -> Rgba {
    current().blue.s[6]
}
