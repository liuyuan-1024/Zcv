//! 系统打开方式注册。
//!
//! macOS 由 LaunchServices 在应用首次出现时自动完成；
//! Windows 没有等价的系统服务，启动时把当前可执行文件写入当前用户的注册表，让用户无需运行任何脚本即可在“打开方式”和“默认应用”中选择 Zcv。

mod platform;

use anyhow::{Result, ensure};

/// 与 macOS 打包共用的文档扩展名清单；每行一个纯小写字母数字扩展名。
const DOCUMENT_EXTENSIONS: &str = include_str!("../../assets/document-extensions.txt");

/// 解析扩展名清单；空行忽略，任何非法行都是开发错误。
fn parse_document_extensions(content: &str) -> Result<Vec<String>> {
    let mut extensions = Vec::new();
    for line in content.lines() {
        let name = line.trim();
        if name.is_empty() {
            continue;
        }
        ensure!(
            name.bytes()
                .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()),
            "内置扩展名清单包含非法项：{name}"
        );
        extensions.push(name.to_owned());
    }
    ensure!(!extensions.is_empty(), "内置扩展名清单为空");
    Ok(extensions)
}

fn document_extensions() -> Result<Vec<String>> {
    parse_document_extensions(DOCUMENT_EXTENSIONS)
}

/// 确保当前安装已注册为系统打开方式候选。
///
/// Windows 下从任意位置启动时生效；重复调用幂等。
pub(crate) fn ensure_open_with_registration() {
    platform::ensure_open_with_registration();
}

#[cfg(test)]
#[path = "test/open_with_tests.rs"]
mod tests;
