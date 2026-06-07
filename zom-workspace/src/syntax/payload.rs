//! 语法高亮 payload 类型。
//!
//! 设计来自《桌面端语法高亮》§三。本模块**不**定义颜色 / 字重 / 字号——只承载 tree-sitter highlight name 与修饰位，theme 在 desktop 端按 name 解析为 Hsla。
//!
//! name 取值域 = tree-sitter highlight name 命名空间，不在本仓维护词汇表。
//!
//! Phase 3 后这里只剩 [`HighlightSpan`] / [`HighlightName`] / [`TokenModifiers`] 三个值类型——它们仍是 [`crate::syntax::BufferSyntaxTree::query_viewport`] 的返回元素，由 paint 阶段消费。
//! `syntax_confirmed_layer_kind` 在 Phase 3 删除：没有 layer 了，没有 kind 可言。

/// 写入 [`zom_engine::MetadataLayer`] 的 syntax payload。
///
/// 不携带颜色——颜色由 desktop 端按 `name` 通过点分前缀回退链查 theme。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct HighlightSpan {
    pub name: HighlightName,
    pub modifiers: TokenModifiers,
}

impl HighlightSpan {
    pub const fn new(name: HighlightName, modifiers: TokenModifiers) -> Self {
        Self { name, modifiers }
    }

    /// 仅取 name，modifiers 走 [`TokenModifiers::EMPTY`]。tree-sitter provider 默认产 0 修饰（手册 §三），LSP provider 内部按需叠加。
    pub const fn from_name(name: HighlightName) -> Self {
        Self {
            name,
            modifiers: TokenModifiers::EMPTY,
        }
    }
}

/// tree-sitter highlight name 的 newtype。
///
/// 用 `&'static str` 是因为 name 来源是注册时 / 编译期就确定的字符串表，
/// runtime 不分配；provider / theme 两端都按完整字符串比较。
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct HighlightName(&'static str);

impl HighlightName {
    pub const fn new(name: &'static str) -> Self {
        Self(name)
    }

    pub const fn as_str(self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for HighlightName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// token 修饰位（手册 §三）。
///
/// 主要服务 LSP `SemanticTokenModifiers`，tree-sitter provider 默认产 EMPTY。
/// 这些位与 LSP 语义 token 修饰符同形；
/// theme 端可按需把 `deprecated` 映射到下划线 / 半透明，把 `async` 映射到斜体等样式叠加。
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Hash)]
pub struct TokenModifiers(u32);

impl TokenModifiers {
    pub const EMPTY: Self = Self(0);

    pub const STATIC: Self = Self(1 << 0);
    pub const READONLY: Self = Self(1 << 1);
    pub const DEPRECATED: Self = Self(1 << 2);
    pub const ASYNC: Self = Self(1 << 3);
    pub const ABSTRACT: Self = Self(1 << 4);

    pub const fn from_bits(bits: u32) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u32 {
        self.0
    }

    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }

    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }
}
