use oxide_runtime_api::{NativeResult, VmHost};

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

fn read_percent_byte(bytes: &[u8], pos: usize) -> Result<u8, ()> {
    if pos + 3 > bytes.len() || bytes[pos] != b'%' {
        return Err(());
    }
    super::parse_hex_u8(&bytes[pos + 1..pos + 3]).ok_or(())
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

fn decode_uri_string(input: &str, preserve_reserved: bool) -> Result<String, ()> {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] != b'%' {
            let ch = input[i..].chars().next().expect("valid utf-8 char boundary");
            out.push(ch);
            i += ch.len_utf8();
            continue;
        }

        let first = read_percent_byte(bytes, i)?;
        let sequence_len = utf8_sequence_len(first)?;
        let raw_start = i;
        let mut encoded = Vec::with_capacity(sequence_len);
        encoded.push(first);
        i += 3;

        for _ in 1..sequence_len {
            let byte = read_percent_byte(bytes, i)?;
            if byte & 0b1100_0000 != 0b1000_0000 {
                return Err(());
            }
            encoded.push(byte);
            i += 3;
        }

        let decoded = std::str::from_utf8(&encoded).map_err(|_| ())?;
        if preserve_reserved && decoded.len() == 1 {
            let ch = decoded.chars().next().expect("single decoded char");
            if URI_RESERVED.contains(ch) {
                out.push_str(&input[raw_start..i]);
                continue;
            }
        }
        out.push_str(decoded);
    }

    Ok(out)
}

fn uri_error<H: VmHost>(vm: &mut H) -> NativeResult {
    NativeResult::Err(crate::error::create_uri_error(vm, URI_ERROR_MESSAGE))
}

pub fn encode_uri<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let input = super::string_arg(vm, args);
    NativeResult::Ok(vm.new_string(&encode_uri_string(&input, URI_SAFE)))
}

pub fn encode_uri_component<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let input = super::string_arg(vm, args);
    NativeResult::Ok(vm.new_string(&encode_uri_string(&input, URI_UNESCAPED)))
}

pub fn decode_uri<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let input = super::string_arg(vm, args);
    match decode_uri_string(&input, true) {
        Ok(decoded) => NativeResult::Ok(vm.new_string(&decoded)),
        Err(()) => uri_error(vm),
    }
}

pub fn decode_uri_component<H: VmHost>(vm: &mut H, args: &[u8]) -> NativeResult {
    let input = super::string_arg(vm, args);
    match decode_uri_string(&input, false) {
        Ok(decoded) => NativeResult::Ok(vm.new_string(&decoded)),
        Err(()) => uri_error(vm),
    }
}

#[cfg(test)]
mod tests {
    use super::{decode_uri_string, encode_uri_string, URI_UNESCAPED};

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
            decode_uri_string("https://example.com/path%3Fq=hello%20world", true).unwrap(),
            "https://example.com/path%3Fq=hello world"
        );
    }

    #[test]
    fn decode_uri_component_decodes_reserved() {
        assert_eq!(decode_uri_string("a%3D1%26b%3D2", false).unwrap(), "a=1&b=2");
    }

    #[test]
    fn decode_uri_rejects_malformed_sequences() {
        assert!(decode_uri_string("%", false).is_err());
        assert!(decode_uri_string("%E0%A4", false).is_err());
        assert!(decode_uri_string("%ED%A0%80", false).is_err());
    }
}
