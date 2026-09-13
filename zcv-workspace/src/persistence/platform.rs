use std::fs;
use std::path::Path;

use anyhow::{Context, Result};

pub(super) fn replace_file(temporary: &Path, destination: &Path) -> Result<()> {
    #[cfg(windows)]
    if destination.exists() {
        fs::remove_file(destination)
            .with_context(|| format!("无法替换旧文件 {}", destination.display()))?;
    }

    fs::rename(temporary, destination)
        .with_context(|| format!("无法提交文件 {}", destination.display()))?;
    Ok(())
}
