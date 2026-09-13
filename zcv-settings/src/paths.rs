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

    home.map(PathBuf::from)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| ".".into()))
}
