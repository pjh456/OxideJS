use oxide_kernel::string_forge::{
    decode_key, encode_key, single_char_ptr, small_int_ptr, source_escape, source_escape_to_key, typeof_string_ptr,
    PermInterner,
};
use oxide_types::object::JsString;

#[test]
fn intern_dedup() {
    let interner = PermInterner::new();
    let (i1, h1) = interner.intern("abc");
    let (i2, h2) = interner.intern("abc");
    assert_eq!(i1, i2);
    assert_eq!(h1, h2);
}

#[test]
fn intern_different() {
    let interner = PermInterner::new();
    let (i1, _) = interner.intern("x");
    let (i2, _) = interner.intern("y");
    assert_ne!(i1, i2);
}

#[test]
fn lookup_zero_clone() {
    let interner = PermInterner::new();
    let (id, _) = interner.intern("hello");
    assert_eq!(interner.lookup(id), Some("hello"));
}

#[test]
fn lookup_not_found() {
    let interner = PermInterner::new();
    assert_eq!(interner.lookup(99999), None);
}

#[test]
fn entry_count_monotonic() {
    let interner = PermInterner::new();
    assert_eq!(interner.entry_count(), 0);
    interner.intern("a");
    interner.intern("b");
    interner.intern("a");
    assert_eq!(interner.entry_count(), 2);
}

#[test]
fn many_unique_no_collision() {
    let interner = PermInterner::new();
    for i in 0..10_000 {
        let s = format!("key{i}");
        let (id, _) = interner.intern(&s);
        assert_eq!(interner.lookup(id), Some(&*Box::leak(s.into_boxed_str())));
    }
    assert_eq!(interner.entry_count(), 10_000);
}

#[test]
fn string_ptr_roundtrip() {
    let interner = PermInterner::new();
    let (id, _) = interner.intern("perm");
    let ptr = interner.string_ptr(id);
    assert_eq!(unsafe { (*ptr).as_str() }, "perm");
    // 二次调用返回同一稳定指针（仅物化一次）。
    assert_eq!(interner.string_ptr(id), ptr);
}

#[test]
fn single_char_ptr_idempotent() {
    let a = single_char_ptr(b'a');
    assert_eq!(unsafe { (*a).as_str() }, "a");
    // 二次调用返回同一稳定指针（仅物化一次）。
    assert_eq!(single_char_ptr(b'a'), a);
}

#[test]
fn single_char_table_full_ascii() {
    // 全表 128 条目均可物化且内容为对应的单字符文本。
    for b in 0u8..=127 {
        let ptr = single_char_ptr(b);
        assert_eq!(unsafe { (*ptr).as_str() }, (b as char).to_string());
    }
}

#[test]
fn typeof_table_content_and_stability() {
    // 8 个 typeof 结果串与下标约定一一对应，且二次调用返回同一稳定指针。
    let texts = ["undefined", "object", "boolean", "number", "string", "symbol", "bigint", "function"];
    for (i, t) in texts.iter().enumerate() {
        let ptr = typeof_string_ptr(i as u8);
        assert_eq!(unsafe { (*ptr).as_str() }, *t);
        assert_eq!(typeof_string_ptr(i as u8), ptr, "下标 {i} 二次调用应返回同一指针");
    }
}

#[test]
fn encode_key_identity_and_escapes() {
    // 良形且无 FFFD（含超平面良配对）编码为恒等文本。
    assert_eq!(encode_key(&"abc🚀".encode_utf16().collect::<Vec<u16>>()), "abc🚀");
    // 孤立 surrogate：FFFD + 4 位小写十六进制。
    assert_eq!(encode_key(&[0xD800]), "\u{FFFD}d800");
    assert_eq!(encode_key(&[0xDFFF]), "\u{FFFD}dfff");
    assert_eq!(encode_key(&[0xDBFF, 0x61]), "\u{FFFD}dbffa");
    // FFFD 单元：FFFD + 字面 "fffd"。
    assert_eq!(encode_key(&[0xFFFD]), "\u{FFFD}fffd");
    // 两种不同单元序列的键文本互异（解码可区分）。
    assert_ne!(encode_key(&[0xFFFD]), encode_key(&[0xD800]));
    // 文本 [FFFD,'d','8','0','0'] 与单元 [D800] 的键必须可区分（防物化串键）。
    assert_ne!(encode_key(&[0xFFFD, 0x64, 0x38, 0x30, 0x30]), encode_key(&[0xD800]));
}

#[test]
fn encode_decode_roundtrip() {
    let cases: Vec<Vec<u16>> = vec![
        Vec::new(),
        "a".encode_utf16().collect(),
        "abc🚀😀".encode_utf16().collect(),
        vec![0xD800, 0x42, 0xDC00],
        vec![0xFFFD, 0xFFFD, 0x41, 0xFFFD, 0x66, 0x66, 0x66, 0x64],
        vec![0xFFFF, 0x0041, 0xDBFF, 0xDC00, 0xD800],
    ];
    for units in &cases {
        let key = encode_key(units);
        assert_eq!(decode_key(&key), units.as_slice(), "roundtrip 失败: key={key:?}");
    }
}

#[test]
fn decode_key_defensive_raw_fffd() {
    // 裸 FFFD（文本末尾或后跟非转义文本）还原为 FFFD 单元，后续字符不被吞。
    assert_eq!(decode_key("a\u{FFFD}"), &[0x61, 0xFFFD]);
    assert_eq!(decode_key("a\u{FFFD}zz"), &[0x61, 0xFFFD, 0x7A, 0x7A]);
}

#[test]
fn source_escape_marker_forms() {
    // 孤立 surrogate / FFFD → 源码域 \u 转义形态；配对与 BMP 恒等。
    assert_eq!(source_escape(&[0xD800]), "\\ud800");
    assert_eq!(source_escape(&[0xDFFF]), "\\udfff");
    assert_eq!(source_escape(&[0xFFFD]), "\\ufffd");
    assert_eq!(source_escape(&[0x61, 0xD800, 0x62]), "a\\ud800b");
    assert_eq!(source_escape(&[0xD83D, 0xDE00]), "\u{1F600}");
}

/// 源码是 JS 程序，反斜杠为语法字符：原样透传，转义文本形态逐字保留
/// （正则/字符串字面量内 `\u{1d306}`、`\\` 等序列不得被改写）。
#[test]
fn source_escape_backslash_passthrough() {
    // 正则字面量源文本（u 模式 \u{..} 转义）：逐字恒等。
    let re_src: Vec<u16> = "/\\u{1d306}/u".encode_utf16().collect();
    assert_eq!(source_escape(&re_src), "/\\u{1d306}/u");
    // 转义文本（反斜杠 + "ud800" 字面文本）与孤立 surrogate 单元同形：
    // 源码域中 `\ud800` 文本即孤立 surrogate 的源码表示，两者编码同一
    // 源码文本是设计使然（源码即程序，oxc 只见转义文本）。
    let text: Vec<u16> = "\\ud800".encode_utf16().collect();
    assert_eq!(source_escape(&text), "\\ud800");
    assert_eq!(source_escape(&[0xD800]), source_escape(&text));
    // 双反斜杠（字面量内转义反斜杠）恒等。
    let dbl: Vec<u16> = "\\\\".encode_utf16().collect();
    assert_eq!(source_escape(&dbl), "\\\\");
}

/// 键域闭环：`decode_key ∘ source_escape_to_key ∘ source_escape` 对单元
/// 序列恒等（注入 marker 还原、用户转义文本透传、双反斜杠与 astral 对）。
#[test]
fn source_escape_roundtrip_via_key() {
    let cases: Vec<Vec<u16>> = vec![
        vec![0xD800],
        vec![0xDFFF],
        vec![0xFFFD],
        vec![0x5C],
        // 用户字面转义文本（非 marker 值域 / \u{..} 形态）：逐字透传。
        vec![0x5C, 0x75, 0x30, 0x30, 0x34, 0x31],
        vec![0x5C, 0x75, 0x7B, 0x31, 0x64, 0x33, 0x30, 0x36, 0x7D],
        vec![0x5C, 0x5C],
        // 数据反斜杠紧邻注入 marker（`\\ud800`/`\\ufffd` 拼接形态）。
        vec![0x5C, 0xD800],
        vec![0x5C, 0xFFFD],
        // FFFD 单元后跟 "d800" 文本：marker 消费恰 4 字符，不吞后续。
        vec![0xFFFD, 0x64, 0x38, 0x30, 0x30],
        vec![0x66, 0x66, 0x66, 0x64],
        vec![0xD83D, 0xDE00],
    ];
    for units in &cases {
        let escaped = source_escape(units);
        let key = source_escape_to_key(&escaped);
        assert_eq!(&decode_key(&key), units.as_slice(), "roundtrip 失败: escaped={escaped:?}");
    }
}

#[test]
fn string_ptr_materializes_units() {
    let interner = PermInterner::new();
    // 良形键：物化内容与键文本逐字一致，且二次调用返回同一稳定指针。
    let (id, _) = interner.intern("perm");
    let ptr = interner.string_ptr(id);
    assert_eq!(unsafe { (*ptr).as_str() }, "perm");
    assert_eq!(interner.string_ptr(id), ptr);
    // 含转义的键：物化出孤立 surrogate 单元形态。
    let (id2, _) = interner.intern(&encode_key(&[0xD800]));
    let ptr2 = interner.string_ptr(id2);
    assert!(unsafe { (*ptr2).has_lone_surrogate() });
    assert_eq!(unsafe { (*ptr2).units() }, &[0xD800][..]);
}

#[test]
fn perm_table_concurrent_first_use_unique_ptr() {
    // 并发首用竞态回归：96 线程同一时刻命中冷表槽，强制多线程同入慢路径。
    // 所有调用方必须拿到同一稳定指针，且该指针恒指向存活的 JsString。
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;

    const N: usize = 96;
    let go = Arc::new(AtomicBool::new(false));
    let mut handles = Vec::with_capacity(N);
    for _ in 0..N {
        let go = Arc::clone(&go);
        handles.push(std::thread::spawn(move || {
            while !go.load(Ordering::Acquire) {
                std::hint::spin_loop();
            }
            // 裸指针 !Send，跨线程以 usize 传递，出口再转回。
            let (i, c, t) = (small_int_ptr(42).unwrap(), single_char_ptr(b'x'), typeof_string_ptr(1));
            (i as usize, c as usize, t as usize)
        }));
    }
    go.store(true, Ordering::Release);

    let mut results = Vec::with_capacity(N);
    for h in handles {
        results.push(h.join().expect("竞态线程"));
    }
    let (i0, c0, t0) = results[0];
    for (i, c, t) in &results[1..] {
        assert_eq!(*i, i0, "小整数表首用应全局唯一指针");
        assert_eq!(*c, c0, "单字符表首用应全局唯一指针");
        assert_eq!(*t, t0, "typeof 表首用应全局唯一指针");
    }

    // 返回指针必须指向存活 JsString：内容与长度可回读。
    assert_eq!(unsafe { (*(i0 as *const JsString)).as_str() }, "42");
    assert_eq!(unsafe { (*(c0 as *const JsString)).as_str() }, "x");
    let t0 = t0 as *const JsString;
    assert_eq!(unsafe { (*t0).as_str() }, "object");
    assert_eq!(unsafe { (*t0).utf16_len() }, 6);
}
