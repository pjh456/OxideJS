use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum JsErrorKind {
    TypeError,
    RangeError,
    ReferenceError,
    SyntaxError,
    Error,
    URIError,
    EvalError,
}

impl fmt::Display for JsErrorKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let name = match self {
            Self::TypeError => "TypeError",
            Self::RangeError => "RangeError",
            Self::ReferenceError => "ReferenceError",
            Self::SyntaxError => "SyntaxError",
            Self::Error => "Error",
            Self::URIError => "URIError",
            Self::EvalError => "EvalError",
        };
        f.write_str(name)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsError {
    pub kind: JsErrorKind,
    pub message: String,
}

impl JsError {
    pub fn type_error(msg: impl Into<String>) -> Self {
        Self::new(JsErrorKind::TypeError, msg)
    }

    pub fn range_error(msg: impl Into<String>) -> Self {
        Self::new(JsErrorKind::RangeError, msg)
    }

    pub fn reference_error(msg: impl Into<String>) -> Self {
        Self::new(JsErrorKind::ReferenceError, msg)
    }

    pub fn syntax_error(msg: impl Into<String>) -> Self {
        Self::new(JsErrorKind::SyntaxError, msg)
    }

    pub fn generic(msg: impl Into<String>) -> Self {
        Self::new(JsErrorKind::Error, msg)
    }

    pub fn new(kind: JsErrorKind, msg: impl Into<String>) -> Self {
        Self { kind, message: msg.into() }
    }
}

impl fmt::Display for JsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.kind, self.message)
    }
}

impl From<JsError> for String {
    fn from(value: JsError) -> Self {
        value.to_string()
    }
}
