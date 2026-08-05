//! 运行时错误类型。
//!
//! [`JsErrorKind`] 对应 ECMAScript 内置错误类型；[`JsError`] 携带 kind 与消息，
//! 在引擎内作为错误值在栈间传播，可通过 `From<JsError> for String` 转成消息文本。

use std::fmt;

/// ECMAScript 内置错误类型。
///
/// 对应全局构造函数 `TypeError`、`RangeError`、`ReferenceError` 等，
/// `Display` 输出即为错误名字符串。
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

/// 一个引擎运行时错误：错误类型 [`kind`](JsError::kind) + 消息文本。
///
/// `Display` 输出为 `"{kind}: {message}"` 形式，与 JS `Error#toString()` 一致。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JsError {
    pub kind: JsErrorKind,
    pub message: String,
}

impl JsError {
    /// 构造 [`JsErrorKind::TypeError`]。
    pub fn type_error(msg: impl Into<String>) -> Self {
        Self::new(JsErrorKind::TypeError, msg)
    }

    /// 构造 [`JsErrorKind::RangeError`]。
    pub fn range_error(msg: impl Into<String>) -> Self {
        Self::new(JsErrorKind::RangeError, msg)
    }

    /// 构造 [`JsErrorKind::ReferenceError`]。
    pub fn reference_error(msg: impl Into<String>) -> Self {
        Self::new(JsErrorKind::ReferenceError, msg)
    }

    /// 构造 [`JsErrorKind::SyntaxError`]。
    pub fn syntax_error(msg: impl Into<String>) -> Self {
        Self::new(JsErrorKind::SyntaxError, msg)
    }

    /// 构造无特定类型的 [`JsErrorKind::Error`]。
    pub fn generic(msg: impl Into<String>) -> Self {
        Self::new(JsErrorKind::Error, msg)
    }

    /// 按指定 kind 与消息构造错误。
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
