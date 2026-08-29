//! 通用 OS 文件监听基础设施。
//! 此文件是 `zcv-fs-watch` crate 的公共入口。
//! 独立成 crate，供项目数据层与设置系统等跨域消费方直接使用。

mod fs_watcher;

pub use fs_watcher::{FsEventStream, FsWatcher, PathEvent, PathEventKind, Watcher};
