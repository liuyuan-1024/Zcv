//! 最近项目功能：列表界面与持久化。

mod storage;
mod view;

pub(crate) use storage::{ProjectEntry, load_recent_projects};
pub use storage::{add_to_recent, most_recent_valid_project, remove_from_recent};
pub use view::{OnProjectSelected, RecentProjects};
