use std::fmt;

#[derive(Debug, Clone)]
pub struct OxideError {
    pub message: String,
    pub span: (usize, usize),
}

impl fmt::Display for OxideError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} at {:?}", self.message, self.span)
    }
}

impl std::error::Error for OxideError {}

impl From<oxc_diagnostics::OxcDiagnostic> for OxideError {
    fn from(diag: oxc_diagnostics::OxcDiagnostic) -> Self {
        let span = diag
            .labels
            .clone()
            .and_then(|labels| labels.first().map(|l| (l.offset() as usize, (l.offset() + l.len()) as usize)))
            .unwrap_or((0, 0));

        OxideError {
            message: diag.message.to_string(),
            span,
        }
    }
}
