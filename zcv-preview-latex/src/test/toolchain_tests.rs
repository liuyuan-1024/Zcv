use std::{env::consts::EXE_SUFFIX, fs, process::ExitStatus};

use super::*;

#[test]
fn toolchain_selects_the_first_directory_with_both_programs() -> Result<()> {
    let root = tempfile::tempdir()?;
    let latexmk_only = root.path().join("仅有 latexmk");
    let xelatex_only = root.path().join("仅有 xelatex");
    let selected = root.path().join("第一套工具");
    let other = root.path().join("第二套工具");

    for (directory, programs) in [
        (&latexmk_only, &["latexmk"][..]),
        (&xelatex_only, &["xelatex"][..]),
        (&selected, &["latexmk", "xelatex"][..]),
        (&other, &["latexmk", "xelatex"][..]),
    ] {
        fs::create_dir(directory)?;
        for program in programs {
            let path = directory.join(format!("{program}{EXE_SUFFIX}"));
            fs::write(&path, [])?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt as _;
                fs::set_permissions(&path, fs::Permissions::from_mode(0o755))?;
            }
        }
    }

    assert!(Toolchain::resolve(vec![latexmk_only.clone(), xelatex_only.clone()]).is_err());
    let toolchain = Toolchain::resolve(vec![latexmk_only, xelatex_only, selected.clone(), other])?;
    assert_eq!(
        toolchain.latexmk,
        selected.join(format!("latexmk{EXE_SUFFIX}"))
    );
    assert_eq!(
        toolchain.xelatex,
        selected.join(format!("xelatex{EXE_SUFFIX}"))
    );
    assert_eq!(
        env::split_paths(&toolchain.search_path).next(),
        Some(selected)
    );
    Ok(())
}

#[test]
fn latexmk_selects_bibtex_from_the_xelatex_distribution() {
    let toolchain = Toolchain {
        latexmk: "latexmk".into(),
        xelatex: "xelatex".into(),
        search_path: "tex-bin".into(),
    };
    for (version, expression) in [
        (
            "MiKTeX-XeTeX 4.16 (MiKTeX 25.12)\r\n",
            "$bibtex = q{miktex-bibtexu %O %S};",
        ),
        (
            "XeTeX 3.141592653-2.6-0.999997 (TeX Live 2026)\n",
            "$bibtex = q{bibtex %O %S};",
        ),
    ] {
        let command = toolchain.latexmk_command(
            Path::new("thesis"),
            OsStr::new("proposal.tex"),
            Path::new("preview directory"),
            version,
        );
        let args: Vec<_> = command.get_args().collect();
        assert!(
            args.windows(2)
                .any(|args| args == [OsStr::new("-e"), OsStr::new(expression)])
        );
    }
}

#[test]
fn inaccessible_miktex_log_reports_the_stage_and_path() {
    let log = r"C:\Users\user\AppData\Local\MiKTeX\miktex\log/latexmk.log";
    let stderr = format!(
        "log4cxx: setFile({log},true) call failed.\r\nlog4cxx: IO Exception : status code = 720005(拒绝访问。)\r\n"
    );
    let output = Output {
        status: ExitStatus::from_raw(1),
        stdout: Vec::new(),
        stderr: stderr.clone().into_bytes(),
    };
    let error = check_output(&output, "latexmk 编译失败").unwrap_err();
    let summary = error.to_string();
    assert!(summary.contains("latexmk 编译失败"));
    assert!(summary.contains("MiKTeX 无法写入日志"));
    assert!(summary.contains(log));
    assert!(
        error
            .downcast_ref::<CompilerError>()
            .unwrap()
            .output
            .contains(&stderr)
    );
}

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

#[cfg(windows)]
use std::os::windows::process::ExitStatusExt;
