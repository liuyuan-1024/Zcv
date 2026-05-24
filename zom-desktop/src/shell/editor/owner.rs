//! 文本编辑目标 owner —— 嵌入式编辑器路由的反向接口。
//!
//! 主编辑区、文件树新建条目输入框、项目选择器查询框各自的业务模型在 App 端
//! 持有；本模块给它们一个统一的接口，让 editor 子系统的 [`super::router`]
//! 能按 [`TextTargetId`] 反查到正确的 IME 目标 / 编辑目标 / 快照 / profile，
//! 而不必在 App 端为每个嵌入点写散落的 match。
//!
//! 拆成两层：
//! - [`TextTargetQuery`] —— 只读路径，`&self` 方法。
//! - [`TextTargetOwner`] —— 可写路径，扩展 `&mut self` 方法。
//!
//! 主编辑区的只读 owner（只持 `&workspace + &views`）只能实现 query 层。

use zom_command::EditTarget;

use super::ime::{ImeQueryTarget, ImeTarget};
use super::{EditorSnapshot, TextInputProfile, TextTargetId};

/// 只读侧：是哪个 target、当前是否活跃、给路由用的查询能力。
pub(crate) trait TextTargetQuery {
    fn target_id(&self) -> TextTargetId;

    /// 当前是否处于"被聚焦、能接收输入"的活跃态。
    ///
    /// 优先级由路由按 owner 数组顺序决定 —— 第一个 `true` 的 owner 即为
    /// 当前焦点目标。主编辑区作为兜底，只要有活动视图就视为活跃。
    fn is_active(&self) -> bool;

    fn snapshot(&self) -> EditorSnapshot;

    /// 该 owner 的按键上下文 profile。文本输入类 surface 的 `key_contexts`
    /// 派生于此。
    fn profile(&self) -> TextInputProfile;

    fn ime_query_target(&self) -> Option<ImeQueryTarget<'_>>;
}

/// 可写侧：IME 写入与编辑命令作用目标。
pub(crate) trait TextTargetOwner: TextTargetQuery {
    fn ime_target(&mut self) -> Option<ImeTarget<'_>>;
    fn edit_target(&mut self) -> Option<EditTarget<'_>>;

    /// 文本输入后置钩子 —— IME 写入成功后由 [`super::EditorRouterMut`]
    /// 调一次。默认 no-op；owner 想响应"我的文本变了"时自行 override，
    /// 不必让宿主代为分发（例如项目选择器在查询变化后重置候选选区）。
    fn after_text_changed(&mut self) {}
}
