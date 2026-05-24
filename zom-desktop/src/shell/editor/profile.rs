//! 可嵌入编辑器的按键上下文 profile。
//!
//! 业务组件只选择一个 profile；具体 text-edit / feature / global 的优先级
//! 由编辑器子系统统一生成，避免每个嵌入点重复拼上下文栈。

use zom_command::KeyContext;
use zom_command::commands::file_tree::FileTreeKeyMode;

/// 一个嵌入式文本编辑器在按键解析时的上下文策略。
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum TextInputProfile {
    /// 主编辑区：多行文本编辑，其后落到全局快捷键。
    MainEditor,
    /// 文件树新建名称：单行文本编辑，未命中后落到文件树 pending-name，再落到全局。
    FileTreePendingName,
    /// 项目选择器查询框：选择器命令优先，其次单行文本编辑，最后全局。
    ProjectPickerQuery,
}

impl TextInputProfile {
    pub(crate) fn accepts_newline(self) -> bool {
        matches!(self, Self::MainEditor)
    }

    pub(crate) fn key_contexts(self) -> Vec<KeyContext> {
        match self {
            Self::MainEditor => vec![
                KeyContext::text_edit(self.accepts_newline(), false),
                KeyContext::global(),
            ],
            Self::FileTreePendingName => vec![
                KeyContext::text_edit(self.accepts_newline(), false),
                KeyContext::file_tree(FileTreeKeyMode::PendingName),
                KeyContext::global(),
            ],
            Self::ProjectPickerQuery => vec![
                KeyContext::project_picker(),
                KeyContext::text_edit(self.accepts_newline(), false),
                KeyContext::global(),
            ],
        }
    }
}
