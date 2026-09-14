use oxide_runtime_api::{NativeResult, VmHost};
use oxide_types::value::JsValue;

const URI_UNESCAPED: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.!~*'()";
const URI_RESERVED: &str = ";/?:@&=+$,#";
const URI_SAFE: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_.!~*'();/?:@&=+$,#";
const URI_ERROR_MESSAGE: &str = "malformed URI sequence";

fn encode_uri_string(input: &str, safe: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ch.is_ascii() && safe.contains(ch) {
            out.push(ch);
            continue;
        }

        let mut buf = [0u8; 4];
        for byte in ch.encode_utf8(&mut buf).as_bytes() {
            out.push_str(&format!("%{byte:02X}"));
        }
    }
    out
}

/// 从单元序列的 `%` 位置读一个 `%XX` 字节（hex 两位须为 ASCII 十六进制单元）。
fn read_percent_unit(input: &[u16], pos: usize) -> Result<u8, ()> {
    if pos + 3 > input.len() || input[pos] != 0x25 {
        return Err(());
    }
    let hi = input[pos + 1];
    let lo = input[pos + 2];
    if hi > 0x7F || lo > 0x7F {
        return Err(());
    }
    super::parse_hex_u8(&[hi as u8, lo as u8]).ok_or(())
}

fn utf8_sequence_len(first: u8) -> Result<usize, ()> {
    match first {
        0x00..=0x7F => Ok(1),
        0xC2..=0xDF => Ok(2),
        0xE0..=0xEF => Ok(3),
        0xF0..=0xF4 => Ok(4),
        _ => Err(()),
    }
}

/// 单元口径的 `%XX` 解码：非 `%` 单元原样透传（孤立 surrogate 单元按规范
/// 不变地往返），`%XX` 序列收集为 UTF-8 字节后还原为码点单元（超平面还原
/// 为良配 surrogate 对）。`preserve_reserved` 为 true 时单个保留字符的
/// 转义形态原样保留（decodeURI 的 decode(encode(x)) 稳定性）。
fn decode_uri_units(input: &[u16], preserve_reserved: bool) -> Result<Vec<u16>, ()> {
    let mut out = Vec::with_capacity(input.len());
    let mut i = 0;

    while i < input.len() {
        if input[i] != 0x25 {
            out.push(input[i]);
            i += 1;
            continue;
        }

        let first = read_percent_unit(input, i)?;
        let sequence_len = utf8_sequence_len(first)?;
        let raw_start = i;
        let mut encoded = Vec::with_capacity(sequence_len);
        encoded.push(first);
        i += 3;

        for _ in 1..sequence_len {
            let byte = read_percent_unit(input, i)?;
            if byte & 0b1100_0000 != 0b1000_0000 {
                return Err(());
            }
            encoded.push(byte);
            i += 3;
        }

        let decoded = std::str::from_utf8(&encoded).map_err(|_| ())?;
        let ch = decoded.chars().next().expect("单字节序解出恰一个码点");
        if preserve_reserved && ch.is_ascii() && URI_RESERVED.contains(ch) {
            for k in raw_start..i {
                out.push(input[k]);
            }
            continue;
        }
        let cp = ch as u32;
        if cp <= 0xFFFF {
            out.push(cp as u16);
        } else {
            let v = cp - 0x10000;
            out.push(0xD800 + (v >> 10) as u16);
            out.push(0xDC00 + (v & 0x3FF) as u16);
        }
    }

    Ok(out)
}

fn uri_error<H: VmHost>(vm: &mut H) -> NativeResult {
    NativeResult::Err(crate::error::create_uri_error(vm, URI_ERROR_MESSAGE))
}

/// 取 URI 函数实参的单元序列（单元口径，不 lossy）：无实参等价于处理
/// `"undefined"`。ToPrimitive 抛错（Symbol 等）原样返回异常值。
fn string_arg_units<H: VmHost>(vm: &mut H, args: &[u8]) -> Result<Vec<u16>, NativeResult> {
    let val = if args.len() > 1 { vm.reg(args[1]) } else { JsValue::undefined() };
    match oxide_runtime_api::to_units_full(val, vm) {
        Ok(u) => Ok(u),
        Err(e) => Err(NativeResult::Err(crate::error::create_from_text(vm, &e))),
    }
}

/// `encodeURI`：编码输入为 URI，保留未转义字符与保留字符 `;/?:@&=+$,#`，
/// 其余按 UTF-8 字节转成 `%XX`。
pub fn encode_uri<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let input = super::string_arg(vm, args);
    NativeResult::Ok(vm.new_string(&encode_uri_string(&input, URI_SAFE)))
}

/// `encodeURIComponent`：编码输入为 URI component，仅保留未转义字符，
/// 保留字符（如 `=&`）也会被转成 `%XX`。
pub fn encode_uri_component<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let input = super::string_arg(vm, args);
    NativeResult::Ok(vm.new_string(&encode_uri_string(&input, URI_UNESCAPED)))
}

/// `decodeURI`：解码 `%XX` 序列，但保留字符的转义形式原样保留（不还原），
/// 保证 decode(encode(uri)) 不变。非法序列抛 URIError。单元口径处理：非 `%`
/// 单元（含孤立 surrogate）原样往返。
pub fn decode_uri<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let input = match string_arg_units(vm, args) {
        Ok(u) => u,
        Err(e) => return e,
    };
    match decode_uri_units(&input, true) {
        Ok(decoded) => NativeResult::Ok(vm.new_string_units(&decoded)),
        Err(()) => uri_error(vm),
    }
}

/// `decodeURIComponent`：解码所有 `%XX` 序列（含保留字符），
/// 非法或截断的 UTF-8 序列抛 URIError。单元口径，孤立 surrogate 原样往返。
pub fn decode_uri_component<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let input = match string_arg_units(vm, args) {
        Ok(u) => u,
        Err(e) => return e,
    };
    match decode_uri_units(&input, false) {
        Ok(decoded) => NativeResult::Ok(vm.new_string_units(&decoded)),
        Err(()) => uri_error(vm),
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_uri_units, encode_uri_string, URI_UNESCAPED};

    fn units(s: &str) -> Vec<u16> {
        s.encode_utf16().collect()
    }

    #[test]
    fn encode_uri_keeps_reserved_and_encodes_space() {
        assert_eq!(
            encode_uri_string("https://example.com/path?q=hello world", super::URI_SAFE),
            "https://example.com/path?q=hello%20world"
        );
    }

    #[test]
    fn encode_uri_component_encodes_reserved() {
        assert_eq!(encode_uri_string("a=1&b=2", URI_UNESCAPED), "a%3D1%26b%3D2");
    }

    #[test]
    fn decode_uri_preserves_reserved_escapes() {
        assert_eq!(
            decode_uri_units(&units("https://example.com/path%3Fq=hello%20world"), true).unwrap(),
            units("https://example.com/path%3Fq=hello world")
        );
    }

    #[test]
    fn decode_uri_component_decodes_reserved() {
        assert_eq!(decode_uri_units(&units("a%3D1%26b%3D2"), false).unwrap(), units("a=1&b=2"));
    }

    #[test]
    fn decode_uri_rejects_malformed_sequences() {
        assert!(decode_uri_units(&units("%"), false).is_err());
        assert!(decode_uri_units(&units("%E0%A4"), false).is_err());
        assert!(decode_uri_units(&units("%ED%A0%80"), false).is_err());
    }

    /// 孤立 surrogate 单元非 `%` 序列：decodeURI/decodeURIComponent 均须原样
    /// 往返（S15.1.3.1/2 A2.1 族遍历 0..0xFFFF 的要求）。
    #[test]
    fn decode_round_trips_lone_surrogates() {
        assert_eq!(decode_uri_units(&[0xD800], true).unwrap(), vec![0xD800]);
        assert_eq!(decode_uri_units(&[0xDFFF], false).unwrap(), vec![0xDFFF]);
        assert_eq!(
            decode_uri_units(&[0x61, 0x25, 0x32, 0x30, 0x62, 0xDBFF], false).unwrap(),
            vec![0x61, 0x20, 0x62, 0xDBFF]
        );
    }
}
