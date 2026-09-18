//! Editor 的选区领域模型与编辑语义。
//!
//! Selection 属于视图状态而非文本存储；底层文本内核只负责坐标映射，
//! Editor 负责选区归一化、锚点跟随和事务历史。

mod core;
mod selection_set;
mod state;

pub(crate) use core::Selection;
pub(crate) use selection_set::SelectionSet;
pub(crate) use state::{
    EditOutcome, EditPlan, SelectionHistory, apply_edits, apply_targeted_edits, replace_selections,
};
