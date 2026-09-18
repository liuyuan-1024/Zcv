//! Zcv 退出后执行的跨平台更新辅助程序。
//!
//! 参数解析与事务编排都在 `zcv-update` 库中，本入口只负责进程边界与错误输出。

fn main() {
    if let Err(error) = zcv_update::run_helper(std::env::args_os().skip(1)) {
        eprintln!("Zcv 更新失败：{error:#}");
        std::process::exit(1);
    }
}
