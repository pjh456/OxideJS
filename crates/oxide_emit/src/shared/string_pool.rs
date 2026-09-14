//! 常量池字符串键编码：池统一存无标志键编码（`oxide_kernel::string_forge::encode_key`
//! 语义），物化侧（perm_string → `decode_key`）是其精确逆变换。
//!
//! 两类来源文本：
//! - oxc marker 文本（`StringLiteral.value` / `TemplateElement.cooked`）：
//!   `lone_surrogates` 置位时 marker 与键编码同构（FFFD+hex4 = 孤立 surrogate 单元、
//!   FFFD+fffd = 真实 FFFD），原样即池键；未置位时为普通文本（可含裸 FFFD），
//!   须逃逸编码防裸 FFFD 与 surrogate marker 碰撞；
//! - 普通文本（标识符 / 数字 / 源文本）：非 FFFD 恒等，真实 FFFD 逃逸为 FFFD+"fffd"。

use oxide_kernel::string_forge::encode_key;
use oxide_parser::PropertyKey;

/// 普通文本 → 池键：真实 FFFD 逃逸为 FFFD+"fffd"，其余逐单元恒等。
pub(crate) fn pool_key_plain(text: &str) -> String {
    let units: Vec<u16> = text.encode_utf16().collect();
    encode_key(&units)
}

/// oxc marker 文本 → 池键，按 `lone_surrogates` 分派（语义见模块头注释）。
pub(crate) fn pool_key_marker(text: &str, lone_surrogates: bool) -> String {
    if lone_surrogates {
        text.to_string()
    } else {
        pool_key_plain(text)
    }
}

/// 属性键 → 池键：字符串字面量键经 marker 解码，标识符 / 数字键走普通编码。
pub(crate) fn pool_key_property(key: &PropertyKey) -> Result<String, String> {
    match key {
        PropertyKey::StaticIdentifier(ident) => Ok(pool_key_plain(ident.name.as_str())),
        PropertyKey::Identifier(ident) => Ok(pool_key_plain(ident.name.as_str())),
        PropertyKey::StringLiteral(s) => Ok(pool_key_marker(&s.value, s.lone_surrogates)),
        PropertyKey::NumericLiteral(n) => Ok(pool_key_plain(&n.value.to_string())),
        PropertyKey::PrivateIdentifier(_) => Err("private class elements not yet supported".into()),
        _ => Err("unsupported class property key type".into()),
    }
}
