//! L1 视觉 token —— 命名常量。
//!
//! 颜色按手册 §4 的两轴正交（hue × step）命名；当前骨架未接入 `cx.theme()`，
//! 函数签名暂时零参，保留同名 / 同形状的升级路径（将来改为 `fn xxx(cx: &App)
//! -> Rgba`）。
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
//!
//! 尺寸、字号、圆角、icon 不参与主题切换（手册 5.1 / 6.x）。
//!
use std::sync::OnceLock;

use gpui::{Font, FontFallbacks, Pixels, Rgba, font, px, rgb, rgba};

// color 是设计系统的"集合定义"：90 个 token 一旦发布就锁定，多数会在调用方
// 接入前长期 dead_code（手册 §4.7）。模块级 allow，避免给每个函数挂注解。
#[allow(dead_code)]
pub mod color {
    use super::*;

    /// 中性灰梯度（手册 §4.2 9 档 + §4.3 双阶梯）。
    ///
    /// solid 阶梯按"深 → 浅"线性过渡；alpha 阶梯使用白色基底配阶梯化 alpha，
    /// 用于"叠在 dark 底色上提亮"的色块（hover、subtle panel 等）。
    pub mod gray {
        use super::*;

        // ---- solid 阶梯 ----

        /// 01 app background —— 最深底面（编辑区底）。
        pub fn s01() -> Rgba {
            rgb(0x0d0f12)
        }
        /// 02 surface —— 抬升面（工作台根、Dock、bars 底）。
        pub fn s02() -> Rgba {
            rgb(0x13161b)
        }
        /// 03 ui-bg —— UI 元素背景常态（浮层、tooltip 底）。
        pub fn s03() -> Rgba {
            rgb(0x1b1f26)
        }
        /// 04 ui-active —— UI 元素背景 active/selected/hover 后（文件树活动行）。
        pub fn s04() -> Rgba {
            rgb(0x252a33)
        }
        /// 05 border-subtle —— 次级边框、分隔线。
        pub fn s05() -> Rgba {
            rgb(0x3a4050)
        }
        /// 06 border-focus —— 焦点边框、hover 边框（gray 这档少用；优先用 accent 色相）。
        pub fn s06() -> Rgba {
            rgb(0x4d5468)
        }
        /// 07 solid —— 实心强调（gray 这档少用；优先用 accent 色相）。
        pub fn s07() -> Rgba {
            rgb(0x5e6678)
        }
        /// 08 text-muted —— 次级 / 弱化文字。
        pub fn s08() -> Rgba {
            rgb(0x6e7689)
        }
        /// 09 text —— 正文 / 强调文字（更强对比靠 font_weight，不靠色阶）。
        pub fn s09() -> Rgba {
            rgb(0xa8b0c0)
        }

        // ---- alpha 阶梯（白色基底 + 阶梯化 alpha）----
        //
        // 阶梯按"我们的深底色 gray::s01 (#0d0f12)"调过，比 Zed 整体抬一档：
        // 0x1a 0x26 0x3d 0x66 0x80 0xb3 0xcc 0xe6 0xf5。Zed 的 alpha 是为 Zed 底色
        // 调的，搬到我们更深的底色上 alpha 会被吞掉视觉，所以整条抬。

        pub fn a01() -> Rgba {
            rgba(0xffffff1a)
        }
        pub fn a02() -> Rgba {
            rgba(0xffffff26)
        }
        pub fn a03() -> Rgba {
            rgba(0xffffff3d)
        }
        pub fn a04() -> Rgba {
            rgba(0xffffff66)
        }
        pub fn a05() -> Rgba {
            rgba(0xffffff80)
        }
        pub fn a06() -> Rgba {
            rgba(0xffffffb3)
        }
        pub fn a07() -> Rgba {
            rgba(0xffffffcc)
        }
        pub fn a08() -> Rgba {
            rgba(0xffffffe6)
        }
        pub fn a09() -> Rgba {
            rgba(0xfffffff5)
        }
    }

    /// 蓝色相——焦点、选区、搜索、信息态。s07 取 Zed One Dark `players[0].cursor`
    /// (`#74ade8`)；alpha 阶梯共用 s07 作为基底。
    pub mod blue {
        use super::*;

        pub fn s01() -> Rgba {
            rgb(0x0e1a2e)
        }
        pub fn s02() -> Rgba {
            rgb(0x122544)
        }
        pub fn s03() -> Rgba {
            rgb(0x173159)
        }
        pub fn s04() -> Rgba {
            rgb(0x1c3d6e)
        }
        pub fn s05() -> Rgba {
            rgb(0x284a85)
        }
        pub fn s06() -> Rgba {
            rgb(0x5a8fd6)
        }
        /// 07 solid —— 实心强调（caret、品牌色填充）。Zed `players[0].cursor`。
        pub fn s07() -> Rgba {
            rgb(0x74ade8)
        }
        pub fn s08() -> Rgba {
            rgb(0x94c1f0)
        }
        pub fn s09() -> Rgba {
            rgb(0xc8def7)
        }

        // alpha 基底 = s07 `#74ade8`；alpha 阶梯比 Zed 抬一档以补偿我们更深的底色
        // （见 gray.alpha 注释）。a04 用于选区、a05 用于搜索普通命中。
        pub fn a01() -> Rgba {
            rgba(0x74ade81a)
        }
        pub fn a02() -> Rgba {
            rgba(0x74ade826)
        }
        pub fn a03() -> Rgba {
            rgba(0x74ade83d)
        }
        /// 04 ui-active —— 选区色块。alpha 抬到 0x66 以在 #0d0f12 底色上保住与
        /// caret (`s07`) 的视觉关联（Zed 原值 0x3d 在我们底色上会被吞掉）。
        pub fn a04() -> Rgba {
            rgba(0x74ade866)
        }
        /// 05 border-subtle —— 搜索普通命中。
        pub fn a05() -> Rgba {
            rgba(0x74ade880)
        }
        pub fn a06() -> Rgba {
            rgba(0x74ade8b3)
        }
        pub fn a07() -> Rgba {
            rgba(0x74ade8cc)
        }
        pub fn a08() -> Rgba {
            rgba(0x74ade8e6)
        }
        pub fn a09() -> Rgba {
            rgba(0x74ade8f5)
        }
    }

    /// 橙色相——warning、当前搜索命中。s07 取 Zed `search.active_match_background`
    /// 的基底色 (`#e8af74`)；alpha 阶梯共用 s07 作为基底。
    pub mod orange {
        use super::*;

        pub fn s01() -> Rgba {
            rgb(0x2d1f10)
        }
        pub fn s02() -> Rgba {
            rgb(0x432e1a)
        }
        pub fn s03() -> Rgba {
            rgb(0x5c3e24)
        }
        pub fn s04() -> Rgba {
            rgb(0x714d2c)
        }
        pub fn s05() -> Rgba {
            rgb(0x8a5d36)
        }
        pub fn s06() -> Rgba {
            rgb(0xb07a4d)
        }
        /// 07 solid —— 实心强调（warning 等）。
        pub fn s07() -> Rgba {
            rgb(0xe8af74)
        }
        pub fn s08() -> Rgba {
            rgb(0xf0c899)
        }
        pub fn s09() -> Rgba {
            rgb(0xf7dfbf)
        }

        pub fn a01() -> Rgba {
            rgba(0xe8af741a)
        }
        pub fn a02() -> Rgba {
            rgba(0xe8af7426)
        }
        pub fn a03() -> Rgba {
            rgba(0xe8af743d)
        }
        pub fn a04() -> Rgba {
            rgba(0xe8af7466)
        }
        /// 05 border-subtle —— 当前搜索命中（暖橙）。
        pub fn a05() -> Rgba {
            rgba(0xe8af7480)
        }
        pub fn a06() -> Rgba {
            rgba(0xe8af74b3)
        }
        pub fn a07() -> Rgba {
            rgba(0xe8af74cc)
        }
        pub fn a08() -> Rgba {
            rgba(0xe8af74e6)
        }
        pub fn a09() -> Rgba {
            rgba(0xe8af74f5)
        }
    }

    /// 红色相——danger、error。s07 取 `#ff6b6b`（沿用旧 `accent::danger`）。
    pub mod red {
        use super::*;

        pub fn s01() -> Rgba {
            rgb(0x2e0e0e)
        }
        pub fn s02() -> Rgba {
            rgb(0x441616)
        }
        pub fn s03() -> Rgba {
            rgb(0x5b2020)
        }
        pub fn s04() -> Rgba {
            rgb(0x732a2a)
        }
        pub fn s05() -> Rgba {
            rgb(0x8b3434)
        }
        pub fn s06() -> Rgba {
            rgb(0xbf4949)
        }
        /// 07 solid —— 实心强调（danger / error）。
        pub fn s07() -> Rgba {
            rgb(0xff6b6b)
        }
        pub fn s08() -> Rgba {
            rgb(0xff9595)
        }
        pub fn s09() -> Rgba {
            rgb(0xffc2c2)
        }

        pub fn a01() -> Rgba {
            rgba(0xff6b6b1a)
        }
        pub fn a02() -> Rgba {
            rgba(0xff6b6b26)
        }
        pub fn a03() -> Rgba {
            rgba(0xff6b6b3d)
        }
        pub fn a04() -> Rgba {
            rgba(0xff6b6b66)
        }
        pub fn a05() -> Rgba {
            rgba(0xff6b6b80)
        }
        pub fn a06() -> Rgba {
            rgba(0xff6b6bb3)
        }
        pub fn a07() -> Rgba {
            rgba(0xff6b6bcc)
        }
        pub fn a08() -> Rgba {
            rgba(0xff6b6be6)
        }
        pub fn a09() -> Rgba {
            rgba(0xff6b6bf5)
        }
    }

    /// 绿色相——success、created。s07 取 `#3ddc84`（沿用旧 `accent::success`）。
    pub mod green {
        use super::*;

        pub fn s01() -> Rgba {
            rgb(0x0a2b1b)
        }
        pub fn s02() -> Rgba {
            rgb(0x103e28)
        }
        pub fn s03() -> Rgba {
            rgb(0x155235)
        }
        pub fn s04() -> Rgba {
            rgb(0x1a6541)
        }
        pub fn s05() -> Rgba {
            rgb(0x20784e)
        }
        pub fn s06() -> Rgba {
            rgb(0x2cab69)
        }
        /// 07 solid —— 实心强调（success）。
        pub fn s07() -> Rgba {
            rgb(0x3ddc84)
        }
        pub fn s08() -> Rgba {
            rgb(0x79e8a8)
        }
        pub fn s09() -> Rgba {
            rgb(0xb5f0c8)
        }

        pub fn a01() -> Rgba {
            rgba(0x3ddc841a)
        }
        pub fn a02() -> Rgba {
            rgba(0x3ddc8426)
        }
        pub fn a03() -> Rgba {
            rgba(0x3ddc843d)
        }
        pub fn a04() -> Rgba {
            rgba(0x3ddc8466)
        }
        pub fn a05() -> Rgba {
            rgba(0x3ddc8480)
        }
        pub fn a06() -> Rgba {
            rgba(0x3ddc84b3)
        }
        pub fn a07() -> Rgba {
            rgba(0x3ddc84cc)
        }
        pub fn a08() -> Rgba {
            rgba(0x3ddc84e6)
        }
        pub fn a09() -> Rgba {
            rgba(0x3ddc84f5)
        }
    }
}

/// 距离类节拍尺（手册 6.1 / 6.2）。
pub mod space {
    use super::*;
    pub fn s4() -> Pixels {
        px(4.0)
    }
    pub fn s6() -> Pixels {
        px(6.0)
    }
    pub fn s8() -> Pixels {
        px(8.0)
    }
    pub fn s12() -> Pixels {
        px(12.0)
    }
    pub fn s16() -> Pixels {
        px(16.0)
    }
}

pub mod radius {
    use super::*;
    pub fn r2() -> Pixels {
        px(2.0)
    }
    pub fn r4() -> Pixels {
        px(4.0)
    }
    /// 圆点（control pip 等）专用，组件本地常量复用此 token。
    pub fn full() -> Pixels {
        px(999.0)
    }
    /// 窗口外框圆角（自绘标题栏时与系统视觉协调）。
    pub fn window() -> Pixels {
        px(8.0)
    }
}

/// 字号 + 行高（手册 6.3 / 6.4）。骨架阶段固定默认值；将来从 `cx.fonts()` 取。
///
/// 只有两套字号：`ui()` 给全部 UI chrome，`editor()` 给编辑区代码正文。
/// UI 层级靠颜色 / 字重区分，不靠字号。
pub mod typography {
    use super::*;
    /// 桌面 UI 字体。当前先使用随应用内置的 Lilex + Sarasa Mono SC，
    /// 以后若加入非等宽 UI 字体，只需要改这里。
    pub fn ui_font() -> Font {
        let mut font = font("Lilex");
        font.fallbacks = Some(cjk_font_fallbacks());
        font
    }

    /// 编辑区代码字体。Lilex 与 Sarasa Mono SC 随应用内置注册；
    /// Lilex 负责拉丁字符，Sarasa Mono SC 负责中文 fallback。
    pub fn editor_font() -> Font {
        let mut font = font("Lilex");
        font.fallbacks = Some(cjk_font_fallbacks());
        font
    }

    fn cjk_font_fallbacks() -> FontFallbacks {
        static FALLBACKS: OnceLock<FontFallbacks> = OnceLock::new();
        FALLBACKS
            .get_or_init(|| FontFallbacks::from_fonts(vec!["Sarasa Mono SC".to_string()]))
            .clone()
    }

    /// UI 文字字号：文件树、顶栏、Dock、标签、浮层、tooltip 等全部 chrome。
    pub fn ui() -> Pixels {
        px(13.0)
    }
    /// UI 行的标准尺寸：既是文字行高，也是 UI 图标尺寸 —— 二者同一个值，
    /// 让「图标 + 文字」一行等高、`items_center` 后盒子对齐。改 UI 行尺寸
    pub fn ui_line() -> Pixels {
        px(16.0)
    }
    /// 编辑区代码字号。后续做成用户可调。
    pub fn editor() -> Pixels {
        px(16.0)
    }
    /// 编辑区行高（约 1.5×，照顾代码可读性）。
    pub fn editor_line() -> Pixels {
        px(24.0)
    }
}
