//! 编辑器输入目标：系统输入法、编辑命令与嵌入控件之间的稳定身份。

/// 一个可接收文本输入的编辑目标。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TextTargetId {
    /// 主编辑区当前活动 view。
    MainEditor,
    /// 文件树中新建文件 / 目录的内联名称输入框。
    FileTreePendingName,
    /// 项目选择器顶部查询输入框。
    ProjectPickerQuery,
}
