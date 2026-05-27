//! zom-desktop —— GPUI 外壳 + 组合根
//!
//! `shell` 负责视觉与平台；`app` 负责组合根。本入口只把两者拼起来。

mod app;
mod focus;
mod shell;

fn main() {
    shell::run(app::App::new_persistent());
}
