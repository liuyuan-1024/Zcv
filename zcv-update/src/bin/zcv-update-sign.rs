//! 使用 CI 中的 Ed25519 PKCS#8 私钥签署更新清单。

use std::fs;
use std::path::PathBuf;

use anyhow::{Context as _, Result, ensure};
use base64::Engine as _;
use ring::signature::{Ed25519KeyPair, KeyPair as _};

fn main() {
    if let Err(error) = run() {
        eprintln!("签署更新清单失败：{error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args_os().skip(1).map(PathBuf::from);
    let key_path = args.next().context("缺少私钥文件路径")?;
    let manifest_path = args.next().context("缺少清单文件路径")?;
    let signature_path = args.next().context("缺少签名输出路径")?;
    ensure!(args.next().is_none(), "参数过多");

    let encoded_key = fs::read_to_string(&key_path)
        .with_context(|| format!("无法读取私钥文件 {}", key_path.display()))?;
    let key = base64::engine::general_purpose::STANDARD
        .decode(encoded_key.trim())
        .context("私钥不是合法 Base64 PKCS#8")?;
    let key_pair = Ed25519KeyPair::from_pkcs8(&key)
        .map_err(|error| anyhow::anyhow!("无法解析 Ed25519 PKCS#8 私钥：{error}"))?;
    let manifest = fs::read(&manifest_path)
        .with_context(|| format!("无法读取更新清单 {}", manifest_path.display()))?;
    let signature =
        base64::engine::general_purpose::STANDARD.encode(key_pair.sign(&manifest).as_ref());
    fs::write(&signature_path, format!("{signature}\n"))
        .with_context(|| format!("无法写入签名文件 {}", signature_path.display()))?;

    println!(
        "{}",
        base64::engine::general_purpose::STANDARD.encode(key_pair.public_key().as_ref())
    );
    Ok(())
}
