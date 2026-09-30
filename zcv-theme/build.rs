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
    let ui_size = read_number(&content, "ui_font_size").expect("内置设置应包含 ui_font_size 数值");

    let out_dir = env::var("OUT_DIR").expect("构建脚本应能读取 OUT_DIR");
    fs::write(
        Path::new(&out_dir).join("default_ui_size.rs"),
        format!("pub const DEFAULT_UI_SIZE: f32 = {ui_size}f32;\n"),
    )
    .expect("应能写出默认字号常量");
}

/// 从 JSON 文本中取出指定键的数值；只处理内置设置使用的简单标量格式。
fn read_number(content: &str, key: &str) -> Option<f64> {
    let needle = format!("\"{key}\"");
    let rest = &content[content.find(&needle)? + needle.len()..];
    let rest = &rest[rest.find(':')? + 1..];
    let token: String = rest
        .trim_start()
        .chars()
        .take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+' | 'e' | 'E'))
        .collect();
    token.parse().ok()
}
