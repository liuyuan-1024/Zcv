//! 从内置设置文件生成编译期默认 UI 字号，避免在代码里重复硬编码。
//!
//! 结构刻度以「默认字号下的像素值」定义，基准字号必须与设置文件一致；
//! 这里在构建期读取同一份 `initial_user_settings.json`，供 `scale` 模块 `include!`。

use std::{env, fs, path::Path};

fn main() {
    let manifest = env::var("CARGO_MANIFEST_DIR").expect("构建脚本应能读取 CARGO_MANIFEST_DIR");
    let settings = Path::new(&manifest).join("../assets/settings/initial_user_settings.json");
    println!("cargo:rerun-if-changed={}", settings.display());

    let content = fs::read_to_string(&settings).expect("内置设置文件应可读");
    let settings: serde_json_lenient::Value =
        serde_json_lenient::from_str(&content).expect("内置设置文件应是合法 JSONC");
    let ui_size = settings["ui_font_size"]
        .as_f64()
        .expect("内置设置应包含 ui_font_size 数值");

    let out_dir = env::var("OUT_DIR").expect("构建脚本应能读取 OUT_DIR");
    fs::write(
        Path::new(&out_dir).join("default_ui_size.rs"),
        format!("pub const DEFAULT_UI_SIZE: f32 = {ui_size}f32;\n"),
    )
    .expect("应能写出默认字号常量");
}
