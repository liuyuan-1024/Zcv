//! Metadata layer 分类键：让宿主按来源或业务类别替换、查询一组外部区间。
//!
//! Kind 只是分层键，不让引擎承担 diagnostics、高亮、断点等业务语义。

/// MetadataLayer 的通用类别。
///
/// 这些类别只用于分层和查询，不代表引擎会生成对应业务含义。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum MetadataLayerKind {
    /// 搜索结果区间；引擎只保存范围，不计算匹配文本或搜索状态。
    SearchMatch,
    /// 诊断区间；严重级别、来源和 message 应放在 metadata payload 中。
    Diagnostics,
    /// 语法高亮区间；token kind 或主题信息不进入引擎核心类型。
    SyntaxHighlight,
    /// 语义 token 区间；与 SyntaxHighlight 分开，便于宿主按来源替换。
    SemanticToken,
    /// 断点标记区间；启用状态和调试器信息属于宿主 payload。
    Breakpoint,
    /// 书签或用户标记区间；引擎不区分命名书签、匿名书签或颜色。
    Bookmark,
    /// Inlay hint 绑定范围；提示文本和位置偏好由 metadata payload 表达。
    InlayHint,
    /// CodeLens 绑定范围；命令、标题和可用性不进入 M10 引擎契约。
    CodeLens,
    /// 宿主自定义 layer，字符串只作为类别键，不携带 schema 或渲染语义。
    Custom(String),
}

impl MetadataLayerKind {
    pub fn custom(name: impl Into<String>) -> Self {
        Self::Custom(name.into())
    }
}

impl Default for MetadataLayerKind {
    fn default() -> Self {
        Self::Custom(String::new())
    }
}
