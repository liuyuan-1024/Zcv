#[cfg(windows)]
fn main() {
    use std::{env, fs, path::PathBuf};

    let icon =
        PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap()).join("../assets/branding/Zcv.ico");
    println!("cargo:rerun-if-changed={}", icon.display());

    let icon = icon
        .canonicalize()
        .expect("应用图标不存在，请先运行 scripts/generate-app-icons");
    let icon_path = icon.to_string_lossy().replace('\\', "\\\\");
    let rc = PathBuf::from(env::var("OUT_DIR").unwrap()).join("zcv-icon.rc");
    fs::write(
        &rc,
        format!("#pragma code_page(65001)\n1 ICON \"{icon_path}\"\n"),
    )
    .expect("无法写入 Windows 图标资源文件");

    embed_resource::compile(&rc, embed_resource::NONE)
        .manifest_required()
        .expect("无法将应用图标嵌入 Zcv.exe");
}

#[cfg(not(windows))]
fn main() {}
