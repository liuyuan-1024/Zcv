//! Windows 打开方式注册后端。
//!
//! 注册全部写入 HKEY_CURRENT_USER，无需管理员权限：
//!
//! - `Software\Classes\<ProgID>`：打开命令与图标；
//! - `Software\Classes\Applications\Zcv.exe`：手动选择可执行文件时的打开命令；
//! - `Software\Zcv\Capabilities`：默认应用注册入口；
//! - 各扩展名 `Software\Classes\<扩展名>\OpenWithProgids`：打开方式候选；
//! - `Software\RegisteredApplications`：默认应用设置页条目。
//!
//! 写入幂等；从其他位置启动后会把命令重新指向当前位置。

use std::path::Path;

use anyhow::{Context as _, Result};
use windows_registry::CURRENT_USER;

use crate::open_with::document_extensions;

/// 生产注册使用的命名空间。
const PRODUCTION_NAMES: RegistrationNames = RegistrationNames {
    prog_id: "Zcv.Document",
    application_exe: "Zcv.exe",
    capabilities_key: "Software\\Zcv\\Capabilities",
    registered_name: "Zcv",
};

const PROG_ID_DESCRIPTION: &str = "Zcv 文档";
const APPLICATION_DESCRIPTION: &str = "Zcv 本地代码编辑器";

/// 注册键命名空间；测试注入独立命名以隔离真实注册表。
pub(crate) struct RegistrationNames {
    pub(crate) prog_id: &'static str,
    pub(crate) application_exe: &'static str,
    pub(crate) capabilities_key: &'static str,
    pub(crate) registered_name: &'static str,
}

pub(super) fn ensure_open_with_registration() {
    if let Err(error) = register_current_executable() {
        // 注册是启动期的后台保障：失败不阻断应用，也不是用户发起的操作，不弹 toast；
        // 保留一条诊断输出，便于定位“为什么列表里没有 Zcv”。
        eprintln!("注册打开方式失败：{error:#}");
    }
}

fn register_current_executable() -> Result<()> {
    let process_path = std::env::current_exe().context("无法确定当前进程路径")?;
    let extensions = document_extensions()?;
    write_registration(&PRODUCTION_NAMES, &process_path, &extensions)
}

/// 把给定可执行文件注册为 `extensions` 中各扩展名的打开方式候选。
pub(crate) fn write_registration(
    names: &RegistrationNames,
    executable: &Path,
    extensions: &[String],
) -> Result<()> {
    let executable = executable.display().to_string();
    let command = format!("\"{executable}\" \"%1\"");

    let prog_id_key = CURRENT_USER.create(format!("Software\\Classes\\{}", names.prog_id))?;
    prog_id_key.set_string("", PROG_ID_DESCRIPTION)?;
    let open_key =
        CURRENT_USER.create(format!("Software\\Classes\\{}\\shell\\open", names.prog_id))?;
    open_key.set_string("Icon", format!("\"{executable}\""))?;
    let command_key = CURRENT_USER.create(format!(
        "Software\\Classes\\{}\\shell\\open\\command",
        names.prog_id
    ))?;
    command_key.set_string("", &command)?;

    let application_key = CURRENT_USER.create(format!(
        "Software\\Classes\\Applications\\{}",
        names.application_exe
    ))?;
    application_key.set_string("FriendlyAppName", "Zcv")?;
    let application_command = CURRENT_USER.create(format!(
        "Software\\Classes\\Applications\\{}\\shell\\open\\command",
        names.application_exe
    ))?;
    application_command.set_string("", &command)?;

    let capabilities = CURRENT_USER.create(names.capabilities_key)?;
    capabilities.set_string("ApplicationName", names.registered_name)?;
    capabilities.set_string("ApplicationDescription", APPLICATION_DESCRIPTION)?;
    let file_associations =
        CURRENT_USER.create(format!("{}\\FileAssociations", names.capabilities_key))?;
    for extension in extensions {
        let extension_name = format!(".{extension}");
        file_associations.set_string(&extension_name, names.prog_id)?;
        let open_with_progids = CURRENT_USER.create(format!(
            "Software\\Classes\\{extension_name}\\OpenWithProgids"
        ))?;
        open_with_progids.set_string(names.prog_id, "")?;
    }

    let registered = CURRENT_USER.create("Software\\RegisteredApplications")?;
    registered.set_string(names.registered_name, names.capabilities_key)?;
    Ok(())
}
