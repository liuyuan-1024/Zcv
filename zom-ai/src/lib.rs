use std::fmt;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiRequest {
    pub instruction: String,
    pub context: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AiProposal {
    pub summary: String,
    pub edits: Vec<ProposedEdit>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ProposedEdit {
    pub range: ProposedRange,
    pub replacement: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProposedRange {
    pub start_byte: u64,
    pub end_byte: u64,
}

pub trait AiProvider {
    fn propose(&self, request: AiRequest) -> Result<AiProposal, AiError>;
}

#[derive(Debug, Eq, PartialEq)]
pub enum AiError {
    ProviderUnavailable,
    RequestRejected(String),
}

impl fmt::Display for AiError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ProviderUnavailable => f.write_str("AI 服务提供方不可用"),
            Self::RequestRejected(reason) => write!(f, "AI 请求被拒绝：{reason}"),
        }
    }
}

impl std::error::Error for AiError {}
