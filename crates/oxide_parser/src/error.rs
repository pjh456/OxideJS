//! 解析错误类型。
//!
//! [`OxideError`] 把 oxc 的 `OxcDiagnostic` 归一化为引擎可消费的
//! 消息 + 字节区间，供 `parse` 返回错误列表。

use std::fmt;

/// 一条解析错误：消息与出错源码区间（`(start, end)` 字节偏移）。
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
            .and_then(|labels| labels.first().map(|l| (l.offset(), l.offset() + l.len())))
            .unwrap_or((0, 0));

        OxideError {
            message: diag.message.to_string(),
            span,
        }
    }
}
