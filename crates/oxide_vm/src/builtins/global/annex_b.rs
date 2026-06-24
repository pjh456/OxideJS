use crate::native::NativeResult;
use crate::vm::Vm;

const ESCAPE_SAFE: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789@*_+-./";

fn parse_hex_u16(slice: &[u8]) -> Option<u16> {
    std::str::from_utf8(slice).ok().and_then(|s| u16::from_str_radix(s, 16).ok())
}

fn escape_string(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for ch in input.chars() {
        if ESCAPE_SAFE.contains(ch) {
            out.push(ch);
            continue;
        }

        let code = ch as u32;
        if code <= 0xFF {
            out.push('%');
            out.push_str(&format!("{code:02X}"));
            continue;
        }

        let mut buf = [0u16; 2];
        for unit in ch.encode_utf16(&mut buf).iter() {
            out.push_str(&format!("%u{:04X}", *unit));
        }
    }
    out
}

fn unescape_string(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = String::with_capacity(input.len());
    let mut i = 0;

    while i < bytes.len() {
        if bytes[i] == b'%' {
            if i + 6 <= bytes.len() && bytes[i + 1] == b'u' {
                if let Some(unit) = parse_hex_u16(&bytes[i + 2..i + 6]) {
                    if (0xD800..=0xDBFF).contains(&unit)
                        && i + 12 <= bytes.len()
                        && bytes[i + 6] == b'%'
                        && bytes[i + 7] == b'u'
                    {
                        if let Some(low) = parse_hex_u16(&bytes[i + 8..i + 12]) {
                            if (0xDC00..=0xDFFF).contains(&low) {
                                let pair = [unit, low];
                                let decoded = char::decode_utf16(pair).next();
                                match decoded {
                                    Some(Ok(ch)) => out.push(ch),
                                    _ => {
                                        out.push_str(&input[i..i + 12]);
                                    }
                                }
                                i += 12;
                                continue;
                            }
                        }
                    }

                    if let Some(ch) = char::from_u32(unit as u32) {
                        out.push(ch);
                    } else {
                        out.push_str(&input[i..i + 6]);
                    }
                    i += 6;
                    continue;
                }
            } else if i + 3 <= bytes.len() {
                if let Some(value) = super::parse_hex_u8(&bytes[i + 1..i + 3]) {
                    out.push(value as char);
                    i += 3;
                    continue;
                }
            }
        }

        let ch = input[i..].chars().next().expect("valid utf-8 char boundary");
        out.push(ch);
        i += ch.len_utf8();
    }

    out
}

pub fn js_escape(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let input = super::string_arg(vm, args);
    NativeResult::Ok(vm.new_string(&escape_string(&input)))
}

pub fn js_unescape(vm: &mut Vm, args: &[u8]) -> NativeResult {
    let input = super::string_arg(vm, args);
    NativeResult::Ok(vm.new_string(&unescape_string(&input)))
}

#[cfg(test)]
mod tests {
    use super::{escape_string, unescape_string};

    #[test]
    fn escape_encodes_spaces_and_unicode() {
        assert_eq!(escape_string("hello world"), "hello%20world");
        assert_eq!(escape_string("AΩ你"), "A%u03A9%u4F60");
    }

    #[test]
    fn unescape_decodes_percent_sequences() {
        assert_eq!(unescape_string("hello%20world"), "hello world");
        assert_eq!(unescape_string("%u03A9%u4F60"), "Ω你");
    }

    #[test]
    fn unescape_keeps_invalid_sequences() {
        assert_eq!(unescape_string("%uXYZ1%2G"), "%uXYZ1%2G");
    }
}
