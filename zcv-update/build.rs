use std::{env, path::PathBuf};

fn main() {
    println!("cargo:rerun-if-changed=windows.manifest");

    if env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }

    let manifest = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").expect("缺少 Cargo 清单目录"))
        .join("windows.manifest");
    println!(
        "cargo:rustc-link-arg-bin=zcv-update-helper=/MANIFESTINPUT:{}",
        manifest.display()
    );
    println!("cargo:rustc-link-arg-bin=zcv-update-helper=/MANIFEST:EMBED");
}
