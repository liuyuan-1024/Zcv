use std::path::{Path, PathBuf};

pub(super) fn config_dir() -> &'static Path {
    static CONFIG_DIR: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();
    CONFIG_DIR.get_or_init(|| home_dir().join(".zcv")).as_path()
}

fn home_dir() -> PathBuf {
    #[cfg(windows)]
    let home = std::env::var_os("USERPROFILE");
    #[cfg(not(windows))]
    let home = std::env::var_os("HOME");

    // 配置目录是所有持久化数据的根；缺少主目录时必须在最早的使用点失败，
    // 不能把进程当前目录当作数据目录静默继续。
    home.map(PathBuf::from)
        .expect("无法确定用户主目录：缺少 HOME/USERPROFILE 环境变量")
}
