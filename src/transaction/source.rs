//! TransactionSource：记录事务来自哪类底层编辑入口。
//!
//! 它服务历史合并和事件观察，不引入 Command、快捷键或宏录制层概念。

/// 事务来源。
///
/// 这里记录“哪类编辑入口产生了事务”，供历史合并、事件观察和调试使用；
/// 它不是 Command 层，也不表达快捷键、菜单项或宏录制语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum TransactionSource {
    /// 引擎调用方直接构造事务提交，没有用户交互语义。
    #[default]
    Programmatic,
    /// 鼠标驱动的编辑入口，例如拖放文本；不表示 selection movement 本身。
    Mouse,
    /// 普通键盘输入产生的文本变更，例如字符输入或 Enter。
    Keyboard,
    /// IME / composition preedit 或 commit 产生的文本变更。
    Composition,
    /// 粘贴入口产生的文本变更；宿主可据此选择不同的历史合并策略。
    Paste,
    /// 删除类编辑入口产生的文本变更。
    Delete,
    /// 格式化器或代码整理工具产生的批量文本变更。
    Formatter,
    /// 外部系统同步进来的文本变更，例如文件 watcher 或协作层适配。
    External,
    /// 历史系统回放 undo 产生的反向事务。
    Undo,
    /// 历史系统回放 redo 产生的正向事务。
    Redo,
}
