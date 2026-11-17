use std::fs;

use super::*;
use crate::toolchain::CompilerError;

#[test]
#[ignore = "需要安装 latexmk、XeLaTeX 和 BibTeX"]
fn builds_preview_for_chinese_file_and_directory() -> Result<()> {
    let root = tempfile::tempdir()?;
    let directory = root.path().join("测试 文档");
    fs::create_dir(&directory)?;
    let source = directory.join("开题报告.tex");
    fs::write(
        &source,
        r"\documentclass{article}
\begin{document}
\input{section}
\bibliographystyle{plain}
\bibliography{refs}
\end{document}",
    )?;
    fs::write(directory.join("section.tex"), r"See \cite{sample}.")?;
    fs::write(
        directory.join("refs.bib"),
        "@book{sample, author={Sample Author}, title={Sample Title}, year={2026}, publisher={Sample Publisher}}",
    )?;

    let preview = compile(&source).map_err(|error| {
        if let Some(compiler) = error.downcast_ref::<CompilerError>() {
            let output = compiler.output.clone();
            error.context(output)
        } else {
            error
        }
    })?;
    assert_eq!(preview, directory.join(".zcv-latex/开题报告.pdf"));
    let pdf = fs::read(&preview)?;
    assert!(pdf.starts_with(b"%PDF-"));

    fs::write(&source, r"\documentclass{zcv-missing-document-class}")?;
    assert!(compile(&source).unwrap_err().is::<CompilerError>());
    assert_eq!(fs::read(&preview)?, pdf);
    let cached_entries = fs::read_dir(directory.join(".zcv-latex"))?
        .map(|entry| entry.map(|entry| entry.path()))
        .collect::<io::Result<Vec<_>>>()?;
    assert_eq!(cached_entries, [preview]);
    Ok(())
}

#[test]
fn failed_publication_preserves_the_last_successful_preview() -> Result<()> {
    struct UnreadablePdf;

    impl io::Read for UnreadablePdf {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("读取 PDF 失败"))
        }
    }

    let directory = tempfile::tempdir()?;
    let pdf_name = Path::new("开题报告.pdf");

    let preview = publish_preview(directory.path(), pdf_name, &b"%PDF-1.5 first"[..])?;
    assert_eq!(preview, directory.path().join("开题报告.pdf"));
    assert_eq!(fs::read(&preview)?, b"%PDF-1.5 first");

    assert!(publish_preview(directory.path(), pdf_name, UnreadablePdf).is_err());
    assert_eq!(fs::read(&preview)?, b"%PDF-1.5 first");

    assert_eq!(
        publish_preview(directory.path(), pdf_name, &b"%PDF-1.5 second"[..])?,
        preview
    );
    assert_eq!(fs::read(&preview)?, b"%PDF-1.5 second");
    Ok(())
}
