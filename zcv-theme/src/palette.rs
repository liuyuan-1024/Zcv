//! 调色板：两轴正交（色相 × 语义档）。
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
//!
//! **色值数据源**：主题 TOML 的 `[ui]` 段（与语法色同一主题文件，主题自包含），由 [`crate::theme_data`] 单一解析器统一解析。
//! 代码内不持有任何色值；alpha 阶梯由各色相强调色 `s[6]` 叠加统一透明度序列派生。
//!
//! 本模块只定义调色板类型，不持有运行期状态；
//! 语义色快照的缓存与切换由 [`crate::color`] 承担。

use gpui::Rgba;

/// 单个色相的 solid + alpha 双阶梯。索引 0–8 对应语义 01–09。
#[derive(Clone, Copy, Debug)]
pub(crate) struct HuePalette {
    pub(crate) s: [Rgba; 9],
    pub(crate) a: [Rgba; 9],
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct Palette {
    pub(crate) gray: HuePalette,
    pub(crate) blue: HuePalette,
    pub(crate) green: HuePalette,
    pub(crate) yellow: HuePalette,
    pub(crate) red: HuePalette,
}
