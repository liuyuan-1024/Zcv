//! L1 视觉 token —— 命名常量。
//!
//! 颜色按手册 4.1 的三轴正交（role × scale_or_semantic × usage）命名；
//! 当前骨架未接入 `cx.theme()`，函数签名暂时零参，保留同名 / 同形状的
//! 升级路径（将来改为 `fn xxx(cx: &App) -> Rgba`）。
//!
//! 尺寸、字号、圆角、icon 不参与主题切换（手册 5.1 / 6.x）。
//!
use gpui::{Pixels, Rgba, px, rgb};

pub mod color {
    use super::*;

    /// 灰度梯度
    ///
    /// 从 `g00`（最深）到 `g95`（最浅）：低段是背景面，中段是边框/高亮，
    /// 高段是文本/图标。每个常量的注释标注它「一般用在哪些元素」，新增用色
    /// 时请按语义复用，不要凭明度临时挑一个。
    pub mod gray {
        use super::*;
        /// 最深背景。用于编辑区正文底面；也作圆点窗控内符号的反差深色。
        pub fn g00() -> Rgba {
            rgb(0x0d0f12)
        }
        /// 基础背景面。工作台根、Dock 容器、编辑区外层的底色。
        pub fn g05() -> Rgba {
            rgb(0x13161b)
        }
        /// 抬升背景面。浮层（项目选择器 / 语言服务器）、TopBar/BottomBar、
        /// tooltip 等「浮在基础面之上」的容器底色。
        pub fn g10() -> Rgba {
            rgb(0x1b1f26)
        }
        /// 行级背景高亮。选中 / 活动行的填充（文件树活动行、浮层选中项）。
        pub fn g20() -> Rgba {
            rgb(0x252a33)
        }
        /// 分隔与边框。窗口外框、Dock/Bar 分隔线、浮层与 tooltip 边框、
        /// dock resize 拖拽手柄。
        pub fn g40() -> Rgba {
            rgb(0x3a4050)
        }
        /// 次级 / 弱化文本。占位提示、编辑区状态与行号、tooltip 快捷键、
        /// Dock 空态文案。
        pub fn g60() -> Rgba {
            rgb(0x6e7689)
        }
        /// 常态正文文本与图标。Bar/Glyph 常态、Dock 文本、文件树行与图标、
        /// 编辑区代码正文与标题——UI 的默认前景色。
        pub fn g75() -> Rgba {
            rgb(0xa8b0c0)
        }
        /// 较强正文。浮层正文、工作台根默认文本——比 g75 更突出但非强调态。
        pub fn g90() -> Rgba {
            rgb(0xe2e6ee)
        }
        /// 最强 / 活动态文本。浮层标题、文件树活动行与图标、Glyph 活动态、
        /// tooltip 标题——需要最高对比的强调文字。
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

    /// 焦点边界高亮。用于键盘焦点、选中项边框等需要明确定位当前位置的区域。
    pub mod focus {
        use super::*;
        pub fn border() -> Rgba {
            rgb(0x5aa8ff)
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
    /// body 文字的紧凑行高。用于标签等需要图标与文字精确对齐、又不想让
    /// 默认行高把行撑得过高的紧凑行。
    pub fn body_line_tight() -> Pixels {
        px(16.0)
    }
    pub fn editor_body() -> Pixels {
        px(14.0)
    }
    pub fn editor_line() -> Pixels {
        px(20.0)
    }
}
