//! Uint8Array base64/hex 编解码纯函数层（无 VM 依赖）。
//!
//! 承载 FromBase64/FromHex 状态机与编码输出，供 typed_array.rs 的
//! setFromBase64/setFromHex/toBase64/toHex/fromBase64/fromHex 六个内置共用。
//! 解码逐块产出 `(read, written)`：`read` 计全部已消费字符（含跳过的 ASCII
//! 空白），`written` 为已写入字节数；`Err(DecodeError)` 统一表示 SyntaxError。已解码
//! 块在出错前经 `write` 回调逐个写出（setFrom 族"前块已写入后抛错"语义）。
//! 解码失败统一以 `Err(DecodeError)` 表示，绑定层映射为 SyntaxError。

/// 解码失败标记（绑定层映射为 SyntaxError）。
#[derive(Debug, Clone, Copy)]
pub struct DecodeError;

/// base64 字母表：标准表（`+/`）与 URL 安全表（`-_`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Base64Alphabet {
    Standard,
    Url,
}

/// 尾块处理模式（lastChunkHandling 三取值）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LastChunkHandling {
    Loose,
    Strict,
    StopBeforePartial,
}

const B64_ALPHABET_STANDARD: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
const B64_ALPHABET_URL: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

fn b64_table(alphabet: Base64Alphabet) -> &'static [u8; 64] {
    match alphabet {
        Base64Alphabet::Standard => B64_ALPHABET_STANDARD,
        Base64Alphabet::Url => B64_ALPHABET_URL,
    }
}

/// 判 ASCII 空白（0x09/0x0A/0x0C/0x0D/0x20 五码；VT 与所有非 ASCII 空格均不算）。
fn is_base64_whitespace(unit: u16) -> bool {
    matches!(unit, 0x09 | 0x0A | 0x0C | 0x0D | 0x20)
}

/// 取 base64 数据字符的值（0-63）；`=` 与任何非字母表字符返回 `None`。
fn base64_value(unit: u16, alphabet: Base64Alphabet) -> Option<u32> {
    match unit {
        0x41..=0x5a => Some(unit as u32 - 0x41),
        0x61..=0x7a => Some(unit as u32 - 0x61 + 26),
        0x30..=0x39 => Some(unit as u32 - 0x30 + 52),
        0x2b if alphabet == Base64Alphabet::Standard => Some(62),
        0x2f if alphabet == Base64Alphabet::Standard => Some(63),
        0x2d if alphabet == Base64Alphabet::Url => Some(62),
        0x5f if alphabet == Base64Alphabet::Url => Some(63),
        _ => None,
    }
}

/// 解码一个满 4 字符块：校验 padding 位置（`=` 只可居 3/4 位）与数据字符，
/// strict 模式另查 padding 位必须为 0。
///
/// # 边界与前提
/// - `chunk` 恰 4 字符；调用方保证只在此处解码满块。
/// - 返回 `(24 位值, 解码字节数 1-3)`：24 位值按 4 槽位左对齐（padding 槽补 0），
///   字节数 = 3 - padding 数。
fn decode_full_chunk(
    chunk: [u16; 4], alphabet: Base64Alphabet, handling: LastChunkHandling,
) -> Result<(u32, usize), DecodeError> {
    let pad = match (chunk[2] == 0x3d, chunk[3] == 0x3d) {
        (false, false) => 0,
        (false, true) => 1,
        (true, true) => 2,
        // `XX=X`：padding 居第 3 位，四种模式全非法。
        (true, false) => return Err(DecodeError),
    };
    let data = &chunk[..4 - pad];
    let mut bits: u32 = 0;
    for &c in data {
        let v = base64_value(c, alphabet).ok_or(DecodeError)?;
        bits = (bits << 6) | v;
    }
    bits <<= 6 * pad as u32;
    if handling == LastChunkHandling::Strict && pad > 0 {
        // padding 位即末数据字符的低 2/4 位（pad 1 查 2 位、pad 2 查 4 位）。
        let last = base64_value(*data.last().unwrap(), alphabet).unwrap();
        let mask = if pad == 1 { 0b11 } else { 0b1111 };
        if last & mask != 0 {
            return Err(DecodeError);
        }
    }
    Ok((bits, 3 - pad))
}

/// 把满块解码出的 24 位值按字节数经回调写出，返回新的已写字节数。
fn write_chunk_bytes(write: &mut dyn FnMut(usize, u8), written: usize, bits: u32, count: usize) -> usize {
    for i in 0..count {
        write(written + i, (bits >> (16 - 8 * i)) as u8);
    }
    written + count
}

/// 尾块（1-3 个非空白字符，已越过全部满块）的终态裁决。
///
/// # 边界与前提
/// - 1 字符尾块：`=` 三种模式全抛；数据字符仅 stop-before-partial 停（成功）；
/// - 含 `=` 尾块：唯一放行形态是 3 字符 `XX=` 且 stop-before-partial（停在不写
///   出的块前）；`X==`/`X=`/`=` 居首等其余 padding 形态三种模式全抛；
/// - 全数据 2-3 字符尾块：strict 抛、stop-before-partial 停在块前、loose 解
///   `clen - 1` 字节（2 字符 1 字节、3 字符 2 字节）并写出。
/// - 成功 `read`：解出字节的 loose 路径取块后消费位置；各"停在块前"路径
///   （sbp、余量不足）取块前位置 `chunk_start`。
#[allow(clippy::too_many_arguments)]
fn finish_partial(
    chunk: [u16; 4], clen: usize, alphabet: Base64Alphabet, handling: LastChunkHandling, max_len: Option<usize>,
    write: &mut dyn FnMut(usize, u8), mut written: usize, chunk_start: usize, read: usize,
) -> Result<(usize, usize), DecodeError> {
    if (0..clen).any(|i| chunk[i] == 0x3d) {
        let trailing = (0..clen).rev().take_while(|&i| chunk[i] == 0x3d).count();
        if clen == 3 && trailing == 1 && handling == LastChunkHandling::StopBeforePartial {
            for &c in &chunk[..2] {
                base64_value(c, alphabet).ok_or(DecodeError)?;
            }
            return Ok((chunk_start, written));
        }
        return Err(DecodeError);
    }
    for &c in &chunk[..clen] {
        base64_value(c, alphabet).ok_or(DecodeError)?;
    }
    match handling {
        LastChunkHandling::StopBeforePartial => Ok((chunk_start, written)),
        LastChunkHandling::Strict => Err(DecodeError),
        LastChunkHandling::Loose => {
            if clen == 1 {
                return Err(DecodeError);
            }
            // maxLength 余量不足时不消费该块直接停（不写出）。
            if let Some(m) = max_len {
                if written + clen - 1 > m {
                    return Ok((chunk_start, written));
                }
            }
            let v = |i: usize| base64_value(chunk[i], alphabet).unwrap();
            write(written, (v(0) << 2 | v(1) >> 4) as u8);
            written += 1;
            if clen == 3 {
                write(written, ((v(1) << 4 | v(2) >> 2) & 0xFF) as u8);
                written += 1;
            }
            Ok((read, written))
        }
    }
}

/// FromBase64 状态机：按块消费输入串，已解码字节经 `write` 回调逐块写出。
///
/// # 步骤
/// 1. `max_len == 0` 短路：输入整体忽略，直接成功空结果。
/// 2. 循环收集至多 4 个非空白字符（跳过五码 ASCII 空白）组成块。
/// 3. 满块先过 maxLength 余量判定（不足则块前停、不消费、不写出），再解码写出；
///    写出后恰好补齐 maxLength 即成功返回。
/// 4. 块不足 4 字符时走终态裁决（[`finish_partial`]）。
///
/// # 边界与前提
/// - `units` 为输入串的 UTF-16 单元序列；非 ASCII 码元一律是非法字符。
/// - 出错时 `write` 已产生的写入保留（调用方承载"前块已写入"语义）。
/// - `Ok((read, written))` 中 `read` 为成功语义下的消费位置（含跳过空白，
///   停止路径为块前位置）。
pub fn decode_base64(
    units: &[u16], alphabet: Base64Alphabet, handling: LastChunkHandling, max_len: Option<usize>,
    write: &mut dyn FnMut(usize, u8),
) -> Result<(usize, usize), DecodeError> {
    if max_len == Some(0) {
        return Ok((0, 0));
    }
    let n = units.len();
    let mut read = 0usize;
    let mut written = 0usize;
    loop {
        let chunk_start = read;
        let mut chunk = [0u16; 4];
        let mut clen = 0usize;
        while clen < 4 && read < n {
            let c = units[read];
            read += 1;
            if is_base64_whitespace(c) {
                continue;
            }
            chunk[clen] = c;
            clen += 1;
        }
        match clen {
            4 => {
                if chunk[2] == 0x3d || chunk[3] == 0x3d {
                    // 带 padding 的满块只可作末块：其后还有非空白字符时
                    // （"extra padding"）在写出该块字节前抛。
                    if units[read..].iter().any(|&c| !is_base64_whitespace(c)) {
                        return Err(DecodeError);
                    }
                }
                let (bits, count) = decode_full_chunk(chunk, alphabet, handling)?;
                if let Some(m) = max_len {
                    if written + count > m {
                        return Ok((chunk_start, written));
                    }
                }
                written = write_chunk_bytes(write, written, bits, count);
                if let Some(m) = max_len {
                    if written == m {
                        return Ok((read, written));
                    }
                }
            }
            0 => return Ok((read, written)),
            _ => return finish_partial(chunk, clen, alphabet, handling, max_len, write, written, chunk_start, read),
        }
    }
}

/// 取单个 hex 字符值（大小写宽容）；非 hex 字符返回 `None`。
fn hex_value(unit: u16) -> Option<u8> {
    match unit {
        0x30..=0x39 => Some(unit as u8 - 0x30),
        0x61..=0x66 => Some(unit as u8 - 0x61 + 10),
        0x41..=0x46 => Some(unit as u8 - 0x41 + 10),
        _ => None,
    }
}

/// FromHex 状态机：无空白跳过、无 padding；长度奇数在最前判定
/// （`read` 0、零写入）；坏字符抛错时保留已写出的前字节。
///
/// # 边界与前提
/// - `units` 为输入串的 UTF-16 单元序列；任何非 hex 字符（含所有空白）非法。
/// - 每对解码前判 `written + 1 > max_len` 则对该对前停（不消费、不写出）。
pub fn decode_hex(
    units: &[u16], max_len: Option<usize>, write: &mut dyn FnMut(usize, u8),
) -> Result<(usize, usize), DecodeError> {
    if units.len() % 2 == 1 {
        return Err(DecodeError);
    }
    let mut written = 0usize;
    let mut read = 0usize;
    for (i, pair) in units.chunks_exact(2).enumerate() {
        let hi = hex_value(pair[0]).ok_or(DecodeError)?;
        let lo = hex_value(pair[1]).ok_or(DecodeError)?;
        if let Some(m) = max_len {
            if written + 1 > m {
                return Ok((i * 2, written));
            }
        }
        write(written, hi * 16 + lo);
        written += 1;
        read = i * 2 + 2;
    }
    Ok((read, written))
}

/// 编码 base64：标准表或 URL 表；`omit_padding` 时剥尾部 `=`。
pub fn encode_base64(bytes: &[u8], alphabet: Base64Alphabet, omit_padding: bool) -> String {
    let table = b64_table(alphabet);
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let bits = (chunk[0] as u32) << 16
            | (chunk.get(1).copied().unwrap_or(0) as u32) << 8
            | chunk.get(2).copied().unwrap_or(0) as u32;
        out.push(table[(bits >> 18) as usize] as char);
        out.push(table[((bits >> 12) & 0x3f) as usize] as char);
        if chunk.len() > 1 {
            out.push(table[((bits >> 6) & 0x3f) as usize] as char);
        } else if !omit_padding {
            out.push('=');
        }
        if chunk.len() > 2 {
            out.push(table[(bits & 0x3f) as usize] as char);
        } else if !omit_padding {
            out.push('=');
        }
    }
    out
}

/// 编码小写两位 hex。
pub fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn units(s: &str) -> Vec<u16> {
        s.chars().map(|c| c as u16).collect()
    }

    fn decode(
        units: &[u16], alphabet: Base64Alphabet, handling: LastChunkHandling, max_len: Option<usize>,
    ) -> Result<(usize, Vec<u8>), DecodeError> {
        let mut bytes = Vec::new();
        decode_base64(units, alphabet, handling, max_len, &mut |_, b| bytes.push(b))
            .map(|(r, w)| (r, bytes.into_iter().take(w).collect()))
    }

    fn decode_hex_full(units: &[u16], max_len: Option<usize>) -> Result<(usize, Vec<u8>), DecodeError> {
        let mut bytes = Vec::new();
        decode_hex(units, max_len, &mut |_, b| bytes.push(b)).map(|(r, w)| (r, bytes.into_iter().take(w).collect()))
    }

    /// RFC 4648 标准向量：编码/解码双向。
    #[test]
    fn rfc4648_vectors_roundtrip() {
        let vectors: [(&str, &[u8]); 7] = [
            ("", &[]),
            ("Zg==", &[102]),
            ("Zm8=", &[102, 111]),
            ("Zm9v", &[102, 111, 111]),
            ("Zm9vYg==", &[102, 111, 111, 98]),
            ("Zm9vYmE=", &[102, 111, 111, 98, 97]),
            ("Zm9vYmFy", &[102, 111, 111, 98, 97, 114]),
        ];
        for (s, bytes) in vectors {
            assert_eq!(encode_base64(bytes, Base64Alphabet::Standard, false), s);
            let (read, decoded) = decode(&units(s), Base64Alphabet::Standard, LastChunkHandling::Loose, None).unwrap();
            assert_eq!(decoded, *bytes);
            assert_eq!(read, s.chars().count());
        }
    }

    #[test]
    fn encode_base64_url_alphabet_and_omit_padding() {
        assert_eq!(encode_base64(&[199, 239, 242], Base64Alphabet::Standard, false), "x+/y");
        assert_eq!(encode_base64(&[199, 239, 242], Base64Alphabet::Url, false), "x-_y");
        assert_eq!(encode_base64(&[199, 239], Base64Alphabet::Standard, false), "x+8=");
        assert_eq!(encode_base64(&[199, 239], Base64Alphabet::Standard, true), "x+8");
        assert_eq!(encode_base64(&[255], Base64Alphabet::Standard, true), "/w");
        assert_eq!(encode_base64(&[255], Base64Alphabet::Url, true), "_w");
    }

    #[test]
    fn decode_base64_whitespace_and_read_count() {
        for ws in [' ', '\t', '\n', '\u{0C}', '\r'] {
            let s = format!("Z{ws}g==");
            let (read, bytes) = decode(&units(&s), Base64Alphabet::Standard, LastChunkHandling::Loose, None).unwrap();
            assert_eq!(bytes, vec![102]);
            assert_eq!(read, 5);
        }
        // VT 不是 ASCII 空白：非法字符。
        assert!(decode(&units("Z\u{0B}g=="), Base64Alphabet::Standard, LastChunkHandling::Loose, None).is_err());
    }

    #[test]
    fn decode_base64_strict_padding_bits() {
        // 末数据字符低 2 位非 0（`h` = 33 = 0b100001）：strict 抛，其余过。
        for (h, ok) in [
            (LastChunkHandling::Loose, true),
            (LastChunkHandling::Strict, false),
            (LastChunkHandling::StopBeforePartial, true),
        ] {
            let r = decode(&units("ZXhhZh=="), Base64Alphabet::Standard, h, None);
            assert_eq!(r.is_ok(), ok, "ZXhhZh== {h:?}");
            if ok {
                assert_eq!(r.unwrap().1, vec![101, 120, 97, 102]);
            }
        }
        // 低 4 位非 0（`h` 居 pad 2 末位）：strict 抛；`g` 低 4 位全 0 则过。
        assert!(decode(&units("ZXhhZg=="), Base64Alphabet::Standard, LastChunkHandling::Strict, None).is_ok());
        assert!(decode(&units("Zg1="), Base64Alphabet::Standard, LastChunkHandling::Strict, None).is_err());
        // pad 2 块字节值：`Zg==` → 102。
        assert_eq!(
            decode(&units("Zg=="), Base64Alphabet::Standard, LastChunkHandling::Loose, None)
                .unwrap()
                .1,
            vec![102]
        );
        // pad 1 块字节值：`Zm8=` → [102, 111]。
        assert_eq!(
            decode(&units("Zm8="), Base64Alphabet::Standard, LastChunkHandling::Loose, None)
                .unwrap()
                .1,
            vec![102, 111]
        );
    }

    #[test]
    fn decode_base64_last_chunk_matrix() {
        let cases: [(&str, [bool; 3]); 13] = [
            // (输入, [loose, strict, sbp] 是否成功)
            ("A", [false, false, true]),
            ("ABCDA", [false, false, true]),
            ("AA=", [false, false, true]),
            ("aQ=", [false, false, true]),
            ("ABCDAA=", [false, false, true]),
            ("AAA", [true, false, true]),
            ("ZXhhZg", [true, false, true]),
            ("ZXhhZg=", [false, false, true]),
            ("ZXhhZg===", [false, false, false]),
            ("=", [false, false, false]),
            ("A==", [false, false, false]),
            ("AAAA=", [false, false, false]),
            ("ZXhhZgg", [true, false, true]),
        ];
        for (s, expect) in cases {
            for (h, ok) in [
                (LastChunkHandling::Loose, expect[0]),
                (LastChunkHandling::Strict, expect[1]),
                (LastChunkHandling::StopBeforePartial, expect[2]),
            ] {
                let r = decode(&units(s), Base64Alphabet::Standard, h, None);
                assert_eq!(r.is_ok(), ok, "{s:?} {h:?}");
            }
        }
        // sbp 停止路径：前块保留、尾块不消费。
        let (read, bytes) =
            decode(&units("ABCDAA="), Base64Alphabet::Standard, LastChunkHandling::StopBeforePartial, None).unwrap();
        assert_eq!((read, bytes), (4, vec![0, 16, 131]));
        let (read, bytes) =
            decode(&units("ZXhhZg"), Base64Alphabet::Standard, LastChunkHandling::StopBeforePartial, None).unwrap();
        assert_eq!((read, bytes), (4, vec![101, 120, 97]));
        // loose 对 2 字符尾块解 1 字节。
        let (read, bytes) = decode(&units("ZXhhZg"), Base64Alphabet::Standard, LastChunkHandling::Loose, None).unwrap();
        assert_eq!((read, bytes), (6, vec![101, 120, 97, 102]));
        // loose 对 3 字符尾块解 2 字节。
        let (read, bytes) =
            decode(&units("ZXhhZgg"), Base64Alphabet::Standard, LastChunkHandling::Loose, None).unwrap();
        assert_eq!((read, bytes), (7, vec![101, 120, 97, 102, 8]));
    }

    #[test]
    fn decode_base64_max_length_stop_timing() {
        // 块前停：第 2 块 3 字节会超限，不消费（read 停在前值 4）。
        let (read, bytes) =
            decode(&units("Zm9vYmFy"), Base64Alphabet::Standard, LastChunkHandling::Loose, Some(5)).unwrap();
        assert_eq!((read, bytes), (4, vec![102, 111, 111]));
        // 恰好补齐：`YmE=` 出 2 字节补到 5。
        let (read, bytes) =
            decode(&units("Zm9vYmE="), Base64Alphabet::Standard, LastChunkHandling::Loose, Some(5)).unwrap();
        assert_eq!((read, bytes), (8, vec![102, 111, 111, 98, 97]));
        // 零长短路：垃圾输入整体忽略。
        let (read, bytes) =
            decode(&units("aaaa#"), Base64Alphabet::Standard, LastChunkHandling::Strict, Some(0)).unwrap();
        assert_eq!((read, bytes), (0, Vec::new()));
        // 尾 3 字符块余量不足：不消费。
        let (read, bytes) =
            decode(&units("Zm9vYmE"), Base64Alphabet::Standard, LastChunkHandling::Loose, Some(3)).unwrap();
        assert_eq!((read, bytes), (4, vec![102, 111, 111]));
    }

    #[test]
    fn decode_base64_illegal_characters() {
        for s in ["Zm.9v", "Zm9v^", "Zg==&", "Z\u{2212}==", "Zg\u{00A0}==", "Zg\u{2009}==", "Zg\u{2028}=="] {
            assert!(
                decode(&units(s), Base64Alphabet::Standard, LastChunkHandling::Loose, None).is_err(),
                "{s:?}"
            );
        }
    }

    #[test]
    fn decode_base64_url_alphabet_decode() {
        let (read, bytes) = decode(&units("x-_y"), Base64Alphabet::Url, LastChunkHandling::Loose, None).unwrap();
        assert_eq!((read, bytes), (4, vec![199, 239, 242]));
        // URL 表中标准表符号非法。
        assert!(decode(&units("x+/y"), Base64Alphabet::Url, LastChunkHandling::Loose, None).is_err());
        assert!(decode(&units("x-_y"), Base64Alphabet::Standard, LastChunkHandling::Loose, None).is_err());
    }

    #[test]
    fn decode_base64_writes_up_to_error() {
        // 坏字符前已写出的字节保留（setFrom 语义同）。
        let mut out = Vec::new();
        let r = decode_base64(
            &units("MjYyZm.9v"),
            Base64Alphabet::Standard,
            LastChunkHandling::Loose,
            None,
            &mut |i, b| out.insert(i, b),
        );
        assert!(r.is_err());
        assert_eq!(out, vec![50, 54, 50]);
        // 带 padding 满块后随非空白字符：该块字节不写出（extra padding）。
        let mut out = Vec::new();
        let r = decode_base64(
            &units("MjYyZg==="),
            Base64Alphabet::Standard,
            LastChunkHandling::Loose,
            None,
            &mut |i, b| out.insert(i, b),
        );
        assert!(r.is_err());
        assert_eq!(out, vec![50, 54, 50]);
    }

    #[test]
    fn decode_hex_vectors_and_case_insensitive() {
        let cases: [(&str, &[u8]); 5] = [
            ("", &[]),
            ("66", &[102]),
            ("666f", &[102, 111]),
            ("666F", &[102, 111]),
            ("666f6f626172", &[102, 111, 111, 98, 97, 114]),
        ];
        for (s, bytes) in cases {
            let (read, decoded) = decode_hex_full(&units(s), None).unwrap();
            assert_eq!(decoded, *bytes);
            assert_eq!(read, s.len());
        }
    }

    #[test]
    fn decode_hex_odd_and_max_length() {
        assert!(decode_hex_full(&units("a"), None).is_err());
        assert!(decode_hex_full(&units("aaa"), None).is_err());
        // 奇数在最前判定：零长目标也抛（零写入）。
        assert!(decode_hex_full(&units("1"), Some(0)).is_err());
        // 对前停：2 字节目标下 `aabbcc` 停在第 3 对前（read 4）。
        let (read, bytes) = decode_hex_full(&units("aabbcc"), Some(2)).unwrap();
        assert_eq!((read, bytes), (4, vec![0xaa, 0xbb]));
        // 恰好补齐。
        let (read, bytes) = decode_hex_full(&units("aabbcc"), Some(3)).unwrap();
        assert_eq!((read, bytes), (6, vec![0xaa, 0xbb, 0xcc]));
    }

    #[test]
    fn decode_hex_illegal_characters() {
        for s in ["a.a", "aa^", "a a", "a\ta", "a\u{00A0}a", "a\u{2009}a", "a\u{2028}a"] {
            assert!(decode_hex_full(&units(s), None).is_err(), "{s:?}");
        }
    }

    #[test]
    fn decode_hex_writes_up_to_error() {
        // 前对写出、坏对抛错。
        let mut out = Vec::new();
        let r = decode_hex(&units("aaag"), None, &mut |i, b| out.insert(i, b));
        assert!(r.is_err());
        assert_eq!(out, vec![0xaa]);
    }

    #[test]
    fn encode_hex_lowercase() {
        assert_eq!(encode_hex(&[]), "");
        assert_eq!(encode_hex(&[102]), "66");
        assert_eq!(encode_hex(&[102, 111, 111, 98, 97, 114]), "666f6f626172");
        assert_eq!(encode_hex(&[170, 187, 204]), "aabbcc");
    }
}
