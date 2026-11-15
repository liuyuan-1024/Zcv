use std::process::ExitStatus;

use super::*;

#[test]
fn failed_build_preserves_the_last_successful_preview() -> Result<()> {
    let directory = tempfile::tempdir()?;
    let staging = tempfile::tempdir()?;
    let output = |status| Output {
        status: ExitStatus::from_raw(status),
        stdout: b"compiler output".to_vec(),
        stderr: Vec::new(),
    };

    fs::write(staging.path().join("preview.pdf"), b"%PDF-1.5 first")?;
    let preview = publish_preview(directory.path(), staging.path(), output(0))?;
    assert_eq!(fs::read(&preview)?, b"%PDF-1.5 first");

    fs::write(staging.path().join("preview.pdf"), b"%PDF-1.5 broken")?;
    let error = publish_preview(directory.path(), staging.path(), output(1)).unwrap_err();
    assert!(
        error
            .downcast_ref::<CompilerError>()
            .expect("编译失败应携带输出")
            .output
            .contains("compiler output")
    );
    assert_eq!(fs::read(&preview)?, b"%PDF-1.5 first");

    fs::write(staging.path().join("preview.pdf"), b"%PDF-1.5 second")?;
    assert_eq!(
        publish_preview(directory.path(), staging.path(), output(0))?,
        preview
    );
    assert_eq!(fs::read(&preview)?, b"%PDF-1.5 second");
    Ok(())
}

#[cfg(unix)]
use std::os::unix::process::ExitStatusExt;

#[cfg(windows)]
use std::os::windows::process::ExitStatusExt;
