//! 后续启动请求交给当前运行的应用进程。

mod platform;

#[cfg(target_os = "windows")]
pub(crate) use platform::start;
