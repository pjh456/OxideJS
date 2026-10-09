use oxide_runtime_api::{NativeResult, VmHost};

const ESCAPE_SAFE: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789@*_+-./";

/// 单个码元的十六进制值（逐位严格匹配，不带符号位）；非十六进制数字返回 None。
fn hex_val(c: u16) -> Option<u16> {
    match c {
        48..=57 => Some(c - 48),
        97..=102 => Some(c - 97 + 10),
        65..=70 => Some(c - 65 + 10),
        _ => None,
    }
}

/// escape 核（规范 B.2.1.1 逐码元迭代）：安全集 `A-Za-z0-9@*_+-./` 直留，
/// <=0xFF 码元产 `%XX`，其余产 `%uXXXX`；孤立 surrogate 码元按码元保真
/// 输出（不替 U+FFFD）。
fn escape_units(input: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(input.len() * 6);
    for &u in input {
        if u < 0x80 && ESCAPE_SAFE.contains(char::from(u as u8)) {
            out.push(u);
            continue;
        }
        // 转义序列恒为全 ASCII，按码元推入。
        let text = if u <= 0xFF { format!("%{u:02X}") } else { format!("%u{u:04X}") };
        out.extend(text.encode_utf16());
    }
    out
}

/// unescape 核（规范 B.2.1.2 逐码元扫描）：`%uXXXX` 优先于 `%XX` 解码，
/// 解码值直接推码元（孤立 surrogate 码元按规范合法输出）；无法解析的
/// 序列原样保留。
fn unescape_units(input: &[u16]) -> Vec<u16> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;
    while i < input.len() {
        if input[i] == b'%' as u16 {
            // %uXXXX：后跟 4 位十六进制。
            if i + 6 <= input.len() && input[i + 1] == b'u' as u16 && (2..6).all(|k| hex_val(input[i + k]).is_some()) {
                let v = (0..4).fold(0u16, |acc, k| acc * 16 + hex_val(input[i + 2 + k]).unwrap());
                out.push(v);
                i += 6;
                continue;
            }
            // %XX：后跟 2 位十六进制。
            if i + 3 <= input.len() && (1..3).all(|k| hex_val(input[i + k]).is_some()) {
                out.push(hex_val(input[i + 1]).unwrap() * 16 + hex_val(input[i + 2]).unwrap());
                i += 3;
                continue;
            }
        }
        out.push(input[i]);
        i += 1;
    }
    out
}

/// Annex B 的全局 `escape(string)`：除 ASCII 字母数字与 `@*_+-./` 外全部编码。
/// <=0xFF 码元用 `%XX`，其它按 UTF-16 码元用 `%uXXXX` 转义。
/// 参数按 `? ToString` 完整转换：Symbol 抛 TypeError，对象方法抛出的原生
/// 异常原样传播。
pub fn js_escape<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let input = match super::units_arg_full(vm, args) {
        Ok(u) => u,
        Err(exc) => return NativeResult::Err(exc),
    };
    NativeResult::Ok(vm.new_string_units_owned(escape_units(&input)))
}

/// Annex B 的全局 `unescape(string)`：解码 `escape` 生成的 `%XX`/`%uXXXX`
/// 序列；无法解析的序列原样保留。
/// 参数按 `? ToString` 完整转换：Symbol 抛 TypeError，对象方法抛出的原生
/// 异常原样传播。
pub fn js_unescape<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let input = match super::units_arg_full(vm, args) {
        Ok(u) => u,
        Err(exc) => return NativeResult::Err(exc),
    };
    NativeResult::Ok(vm.new_string_units_owned(unescape_units(&input)))
}

#[cfg(test)]
mod tests {
    use super::{escape_units, unescape_units};

    fn units(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn escape_encodes_spaces_and_unicode() {
        assert_eq!(escape_units(&units("hello world")), units("hello%20world"));
        assert_eq!(escape_units(&units("AΩ你")), units("A%u03A9%u4F60"));
    }

    #[test]
    fn unescape_decodes_percent_sequences() {
        assert_eq!(unescape_units(&units("hello%20world")), units("hello world"));
        assert_eq!(unescape_units(&units("%u03A9%u4F60")), units("Ω你"));
    }

    #[test]
    fn unescape_keeps_invalid_sequences() {
        assert_eq!(unescape_units(&units("%uXYZ1%2G")), units("%uXYZ1%2G"));
    }

    #[test]
    fn escape_preserves_lone_surrogates() {
        assert_eq!(escape_units(&[0xD834, 0xDF06]), units("%uD834%uDF06"));
    }

    #[test]
    fn unescape_outputs_lone_surrogate_unit() {
        assert_eq!(unescape_units(&units("%uD834")), vec![0xD834]);
    }
}
