//! L1 视觉 token —— 命名常量。
//!
//! 颜色按手册 4.1 的三轴正交（role × scale_or_semantic × usage）命名；
//! 当前骨架未接入 `cx.theme()`，函数签名暂时零参，保留同名 / 同形状的
//! 升级路径（将来改为 `fn xxx(cx: &App) -> Rgba`）。
//!
//! 尺寸、字号、圆角、icon 不参与主题切换（手册 5.1 / 6.x）。
//!
//! Token 表是「设计契约」——即便当前未被消费也保留，骨架阶段统一抑制
//! dead_code 警告。

#![allow(dead_code)]

use gpui::{Pixels, Rgba, px, rgb};

pub mod color {
    use super::*;

    /// 灰度梯度（手册 4.x、布局模型 2 视觉哲学）。
    pub mod gray {
        use super::*;
        pub fn g00() -> Rgba {
            rgb(0x0d0f12)
        }
        pub fn g05() -> Rgba {
            rgb(0x13161b)
        }
        pub fn g10() -> Rgba {
            rgb(0x1b1f26)
        }
        pub fn g20() -> Rgba {
            rgb(0x252a33)
        }
        pub fn g40() -> Rgba {
            rgb(0x3a4050)
        }
        pub fn g60() -> Rgba {
            rgb(0x6e7689)
        }
        pub fn g75() -> Rgba {
            rgb(0xa8b0c0)
        }
        pub fn g90() -> Rgba {
            rgb(0xe2e6ee)
        }
        pub fn g95() -> Rgba {
            rgb(0xf2f4f8)
        }
    }

    /// 语义色 —— 仅用于状态、风险、行动反馈。
    pub mod accent {
        use super::*;
        pub fn danger() -> Rgba {
            rgb(0xff6b6b)
        }
        pub fn warning() -> Rgba {
            rgb(0xf5b400)
        }
        pub fn success() -> Rgba {
            rgb(0x3ddc84)
        }
        pub fn info() -> Rgba {
            rgb(0x5aa8ff)
        }
    }

    /// 圆点控制按钮（control pip）的本地色（手册 3.4 —— 拒绝主题改写）。
    /// 视觉灵感来自 macOS 三色窗控，但仅作为我们外壳的设计选型，不携带
    /// 平台语义；实际平台动作由 `shell::platform::window` 决定。
    pub mod control_pip {
        use super::*;
        pub fn close_fill() -> Rgba {
            rgb(0xff5f57)
        }
        pub fn close_border() -> Rgba {
            rgb(0xe0443e)
        }
        pub fn minimize_fill() -> Rgba {
            rgb(0xffbd2e)
        }
        pub fn minimize_border() -> Rgba {
            rgb(0xde9f18)
        }
        pub fn maximize_fill() -> Rgba {
            rgb(0x28c840)
        }
        pub fn maximize_border() -> Rgba {
            rgb(0x1aab29)
        }
        pub fn inactive_fill() -> Rgba {
            rgb(0x6f7378)
        }
        pub fn inactive_border() -> Rgba {
            rgb(0x5c6066)
        }
    }
}

/// 距离类节拍尺（手册 6.1 / 6.2）。
pub mod space {
    use super::*;
    pub fn s2() -> Pixels {
        px(2.0)
    }
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
    pub fn s24() -> Pixels {
        px(24.0)
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
    pub fn r6() -> Pixels {
        px(6.0)
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

pub mod icon {
    use super::*;
    pub fn i10() -> Pixels {
        px(10.0)
    }
    pub fn i12() -> Pixels {
        px(12.0)
    }
    pub fn i14() -> Pixels {
        px(14.0)
    }
    pub fn i16() -> Pixels {
        px(16.0)
    }
    pub fn i20() -> Pixels {
        px(20.0)
    }
}

/// 字号 + 行高（手册 6.3 / 6.4）。骨架阶段固定默认值；将来从 `cx.fonts()` 取。
pub mod typography {
    use super::*;
    pub fn caption() -> Pixels {
        px(12.0)
    }
    pub fn body() -> Pixels {
        px(13.0)
    }
    pub fn title() -> Pixels {
        px(14.0)
    }
    pub fn large() -> Pixels {
        px(16.0)
    }
    pub fn body_line() -> Pixels {
        px(20.0)
    }
    pub fn editor_body() -> Pixels {
        px(14.0)
    }
    pub fn editor_line() -> Pixels {
        px(20.0)
    }
}
