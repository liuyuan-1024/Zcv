//! Buffer 身份与生命周期状态：表达 Buffer 来源和当前对宿主可见的状态。
//!
//! **领域防腐纪律**：引擎 **不认识 `PathBuf` / `Uri`** 这类宿主操作系统概念。
//! 来源标识只是一个由宿主自由解释的不透明字符串句柄——可以是文件路径、
//! URL、UUID、ROM 资源 ID、虚拟资源名称，引擎一概不解析、不规范化、不做 I/O。
//! 路径/URL 的解析与持久化策略留给宿主层。

use std::sync::Arc;

/// Buffer 来源类型标签。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum OriginKind {
    /// 未命名 / 临时草稿；宿主端没有持久化句柄。
    #[default]
    Anonymous,
    /// 宿主声明对应某个外部资源；具体语义（文件、URL、远程文档…）由宿主自解释。
    External,
}

/// Buffer 的来源句柄。
///
/// **不透明且不可解析**：引擎只用它做相等性 / 哈希 / 调试展示，**不**根据它做 I/O、
/// 不规范化、不在乎它是路径还是 URI 还是其他形态。
///
/// 句柄在内部以共享字符串存储，避免 `BufferOrigin` clone 时复制文本内容。
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct BufferOrigin {
    kind: OriginKind,
    /// 宿主提供的不透明标识。`Anonymous` 来源允许为 `None`。
    handle: Option<Arc<str>>,
}

impl BufferOrigin {
    /// 未命名 / 临时草稿来源。
    pub fn anonymous() -> Self {
        Self {
            kind: OriginKind::Anonymous,
            handle: None,
        }
    }

    /// 关联外部资源来源；`handle` 由宿主自由解释。
    pub fn external(handle: impl Into<String>) -> Self {
        Self {
            kind: OriginKind::External,
            handle: Some(Arc::<str>::from(handle.into())),
        }
    }

    pub fn kind(&self) -> OriginKind {
        self.kind
    }

    /// 宿主句柄（可能为 `None`，仅在 `Anonymous` 时）。
    pub fn handle(&self) -> Option<&str> {
        self.handle.as_deref()
    }

    /// 是否是匿名 / 临时来源（无 host 持久化句柄）。
    pub fn is_anonymous(&self) -> bool {
        matches!(self.kind, OriginKind::Anonymous)
    }
}

impl Default for BufferOrigin {
    fn default() -> Self {
        Self::anonymous()
    }
}

/// Buffer 当前对宿主可见的生命周期状态。
///
/// 只暴露由 Buffer 状态机真实承载的状态；Loading / Reloading / Conflict 等
/// 待对应生命周期语义实现后再进入 public API。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum BufferState {
    /// 当前文本与最近保存基线一致，且 Buffer 允许正常编辑。
    Clean,
    /// 当前文本相对最近保存基线已有变更。
    Dirty,
    /// 当前文本不可通过普通编辑入口修改；dirty 与否不由该状态表达。
    ReadOnly,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn origin_handle_should_remain_host_opaque() {
        let anonymous = BufferOrigin::anonymous();
        let external = BufferOrigin::external("zom://opaque/path");

        assert_eq!(anonymous.kind(), OriginKind::Anonymous);
        assert_eq!(anonymous.handle(), None);
        assert!(anonymous.is_anonymous());
        assert_eq!(external.kind(), OriginKind::External);
        assert_eq!(external.handle(), Some("zom://opaque/path"));
        assert!(!external.is_anonymous());
    }
}
