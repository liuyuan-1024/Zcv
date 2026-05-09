//! TransactionSource：标记事务由哪类引擎内部入口产生。
//!
//! 仅保留引擎内部需要据此分支的来源；宿主输入分类（键盘 / 鼠标 / 粘贴 / 格式化器 / 文件 watcher 等）属于宿主词汇表，不进入 engine 核心枚举。宿主如需追加来源标识，应在自己的类型中维护，并通过 `TransactionMetadata::description` 透传。

/// 事务来源（engine 内部分支用）。
///
/// 引擎只对以下来源做条件分支：
/// - `Composition` 触发 IME 状态边界；
/// - `Undo` / `Redo` 标记历史回放，避免被当成新一轮编辑触发 redo 清理；
/// - `Programmatic` 是默认值，引擎不对其附加任何用户交互假设。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TransactionSource {
    /// 引擎调用方直接构造事务提交，不附加任何宿主输入语义。
    #[default]
    Programmatic,
    /// IME / composition preedit 或 commit 产生的文本变更。
    Composition,
    /// 历史系统回放 undo 产生的反向事务。
    Undo,
    /// 历史系统回放 redo 产生的正向事务。
    Redo,
}
