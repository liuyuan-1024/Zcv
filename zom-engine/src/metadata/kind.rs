//! Metadata layer 分类键：让宿主按业务类别替换、查询一组外部区间。
//!
//! Kind 只是分层键。业务类别（diagnostics / syntax highlight / semantic token / breakpoint / bookmark / inlay hint / code lens 等）属于宿主词汇表，不进入引擎核心枚举；宿主自定义类别统一通过 `MetadataLayerKind::Custom(name)` 表达。

/// MetadataLayer 的分类键。
///
/// 引擎当前不预定义任何业务分类；所有 layer 都通过 `Custom(String)` 表达，引擎不对其附加 schema 或语义。
/// 搜索高亮、语法高亮、诊断、断点等业务分类全部由宿主侧决定。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MetadataLayerKind {
    /// 宿主自定义 layer，字符串只作为类别键，不携带 schema 或渲染语义。
    Custom(String),
}

impl MetadataLayerKind {
    pub fn custom(name: impl Into<String>) -> Self {
        Self::Custom(name.into())
    }
}
