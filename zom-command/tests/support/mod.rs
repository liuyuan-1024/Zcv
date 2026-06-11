use zom_command::ClipboardPort;

/// 测试用内存剪贴板。
#[derive(Clone, Debug, Default)]
pub(crate) struct MockClipboard {
    contents: Option<String>,
}

impl MockClipboard {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn with_contents(text: impl Into<String>) -> Self {
        Self {
            contents: Some(text.into()),
        }
    }

    pub(crate) fn contents(&self) -> Option<&str> {
        self.contents.as_deref()
    }
}

impl ClipboardPort for MockClipboard {
    fn write(&mut self, text: &str) {
        self.contents = Some(text.to_string());
    }

    fn read(&self) -> Option<String> {
        self.contents.clone()
    }
}
