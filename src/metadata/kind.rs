//! Metadata layer 分类键：让宿主按业务类别替换、查询一组外部区间。
//!
//! Kind 只是分层键。业务类别（diagnostics / syntax highlight / semantic token / breakpoint / bookmark / inlay hint / code lens 等）属于宿主词汇表，不进入引擎核心枚举；宿主自定义类别统一通过 `MetadataLayerKind::Custom(name)` 表达。

/// MetadataLayer 的分类键。
///
/// 引擎只预定义自身会创建的分类（目前仅 `SearchMatch`）；宿主侧业务分类一律使用
/// `Custom(String)`，引擎不对其附加 schema 或语义。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MetadataLayerKind {
    /// 引擎搜索结果区间；由 engine 内部 search 模块创建，不计算匹配文本或搜索状态。
    SearchMatch,
    /// 宿主自定义 layer，字符串只作为类别键，不携带 schema 或渲染语义。
    Custom(String),
}

impl MetadataLayerKind {
    pub fn custom(name: impl Into<String>) -> Self {
        Self::Custom(name.into())
    }
}
