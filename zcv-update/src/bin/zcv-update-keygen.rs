//! 生成用于签署 Zcv 更新清单的 Ed25519 密钥。

use std::fs::OpenOptions;
use std::io::Write as _;
use std::path::PathBuf;

use anyhow::{Context as _, Result, ensure};
use base64::Engine as _;
use ring::rand::SystemRandom;
use ring::signature::{Ed25519KeyPair, KeyPair as _};

fn main() {
    if let Err(error) = run() {
        eprintln!("生成更新签名密钥失败：{error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args_os().skip(1).map(PathBuf::from);
    let private_key_path = args.next().context("缺少私钥输出路径")?;
    ensure!(args.next().is_none(), "参数过多");

    let document = Ed25519KeyPair::generate_pkcs8(&SystemRandom::new())
        .map_err(|_| anyhow::anyhow!("系统随机数生成失败"))?;
    let key_pair = Ed25519KeyPair::from_pkcs8(document.as_ref())
        .map_err(|error| anyhow::anyhow!("无法读取新生成的 Ed25519 私钥：{error}"))?;
    let encoded_private_key = base64::engine::general_purpose::STANDARD.encode(document.as_ref());

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt as _;

        options.mode(0o600);
    }
    let mut file = options
        .open(&private_key_path)
        .with_context(|| format!("无法创建私钥文件 {}", private_key_path.display()))?;
    writeln!(file, "{encoded_private_key}")
        .with_context(|| format!("无法写入私钥文件 {}", private_key_path.display()))?;
    file.sync_all()
        .with_context(|| format!("无法同步私钥文件 {}", private_key_path.display()))?;

    println!(
        "{}",
        base64::engine::general_purpose::STANDARD.encode(key_pair.public_key().as_ref())
    );
    Ok(())
}
