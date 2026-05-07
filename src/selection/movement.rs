//! Movement 词汇表：定义文本移动方向和粒度。
//!
//! 这里是 public 策略枚举，不实现移动算法；具体边界搜索在 Buffer movement 能力域。

/// M6B 文本移动方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MovementDirection {
    /// 向前 / 向左移动。
    Previous,
    /// 向后 / 向右移动。
    Next,
}

/// M6B 文本移动粒度。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MovementUnit {
    /// 用户感知字符，复用 M5A grapheme boundary。
    Grapheme,
    /// Unicode 自然语言 word，基于 `unicode-segmentation`。
    Word,
    /// 编程语言标识符片段，默认包含 Unicode 字母数字、组合音标、`_` 和 `$`。
    Identifier,
    /// 标识符内的子词，支持 snake_case、camelCase、PascalCase 与字母/数字切分。
    Subword,
    /// 操作符 / 标点 / emoji 等非空白、非 identifier 的符号 run。
    Symbol,
}
