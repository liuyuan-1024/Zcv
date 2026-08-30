//! 从已经签名、公证的发布包生成 Zcv 更新清单。

use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

use anyhow::{Context as _, Result, ensure};
use semver::Version;
use zcv_update::{
    MANIFEST_SCHEMA_VERSION, ReleaseAsset, ReleaseManifest, STABLE_CHANNEL, sha256_file,
};

fn main() {
    if let Err(error) = run() {
        eprintln!("生成更新清单失败：{error:#}");
        std::process::exit(1);
    }
}

fn run() -> Result<()> {
    let mut args = std::env::args_os().skip(1);
    let version: Version = args
        .next()
        .context("缺少版本号")?
        .to_str()
        .context("版本号不是 UTF-8")?
        .parse()
        .context("版本号不是合法 SemVer")?;
    ensure!(version.pre.is_empty(), "stable 清单不接受预发布版本");
    let published_at = args
        .next()
        .context("缺少发布时间")?
        .to_str()
        .context("发布时间不是 UTF-8")?
        .to_owned();
    ensure!(!published_at.trim().is_empty(), "发布时间不能为空");
    let output_path = PathBuf::from(args.next().context("缺少清单输出路径")?);

    let mut assets = BTreeMap::new();
    while let Some(platform) = args.next() {
        let platform = platform.to_str().context("平台名称不是 UTF-8")?.to_owned();
        ensure!(
            platform == "macos-aarch64",
            "仅支持 Apple Silicon 清单平台 {platform}"
        );
        let url = args
            .next()
            .context("平台参数缺少下载地址")?
            .to_str()
            .context("下载地址不是 UTF-8")?
            .to_owned();
        ensure!(url.starts_with("https://"), "下载地址必须使用 HTTPS");
        let asset_path = PathBuf::from(args.next().context("平台参数缺少产物路径")?);
        let size = fs::metadata(&asset_path)
            .with_context(|| format!("无法读取发布包 {}", asset_path.display()))?
            .len();
        ensure!(size > 0, "发布包 {} 为空", asset_path.display());
        let asset = ReleaseAsset {
            url,
            size,
            sha256: sha256_file(&asset_path)?,
        };
        ensure!(
            assets.insert(platform.clone(), asset).is_none(),
            "平台 {platform} 重复"
        );
    }
    ensure!(!assets.is_empty(), "至少需要一个平台产物");

    let manifest = ReleaseManifest {
        schema_version: MANIFEST_SCHEMA_VERSION,
        channel: STABLE_CHANNEL.to_owned(),
        version,
        published_at,
        assets,
    };
    let mut content = serde_json::to_vec_pretty(&manifest).context("无法序列化更新清单")?;
    content.push(b'\n');
    fs::write(&output_path, content)
        .with_context(|| format!("无法写入更新清单 {}", output_path.display()))?;
    Ok(())
}
