use super::*;

#[test]
fn js_string_cons_basic() {
    // 左右子 Flat 构造 Cons：utf16_len/is_empty/文本视图全部按拼接语义。
    let left = Box::new(JsString::new("ab".to_string()));
    let right = Box::new(JsString::new("cd".to_string()));
    let cons = Box::new(unsafe { JsString::new_cons(&*left, &*right) });
    assert_eq!(cons.utf16_len(), 4);
    assert!(!cons.is_empty());
    assert_eq!(cons.as_lossy_str(), "abcd");
    assert_eq!(cons.to_owned_string(), "abcd");
    assert!(cons.is_cons());
    // 扁平化产物缓存命中：内容仍一致（units 视图同样走缓存）。
    assert_eq!(cons.as_lossy_str(), "abcd");
    assert_eq!(cons.units().as_ref(), &['a' as u16, 'b' as u16, 'c' as u16, 'd' as u16]);
}

#[test]
fn js_string_layout_anchor_and_tags() {
    // 32B 锚 + 三形态 tag/utf16_len 预写口径（布局变更先同步头注释再改断言）。
    let flat = JsString::new("ab".to_string());
    assert_eq!(std::mem::size_of::<JsString>(), 32);
    assert!(flat.is_flat() && !flat.is_cons() && !flat.is_flat_u16());
    assert_eq!(flat.tag, TAG_FLAT);
    assert_eq!(flat.utf16_len(), 2);
    assert_eq!(flat.payload_bytes(), 2);

    let u16 = JsString::new_flat_u16(vec![0xD800, 0xDC00]);
    assert!(u16.is_flat_u16());
    assert_eq!(u16.tag, TAG_FLAT_U16);
    assert_eq!(u16.utf16_len(), 2);
    assert_eq!(u16.payload_bytes(), 4);
    assert_eq!(u16.units_borrowed(), Some(&[0xD800, 0xDC00][..]));
    // 代理对 well-formed：smart 路由落 Flat 且内容等价。
    let smart = JsString::from_units(vec![0xD800, 0xDC00]);
    assert!(smart.is_flat());
    assert_eq!(smart.as_str(), "\u{10000}");
    // 孤立 surrogate：smart 路由落 FlatU16。
    let lone = JsString::from_units(vec![0xD800]);
    assert!(lone.is_flat_u16());
    assert!(lone.has_lone_surrogate());
    assert_eq!(lone.units().as_ref(), &[0xD800]);
    assert_eq!(lone.as_lossy_str(), "\u{FFFD}");

    let left = Box::new(JsString::new("ab".to_string()));
    let right = Box::new(JsString::new("cd".to_string()));
    let cons = Box::new(unsafe { JsString::new_cons(&*left, &*right) });
    assert!(cons.is_cons());
    assert_eq!(cons.tag, TAG_CONS);
    assert_eq!(cons.utf16_len(), 4, "Cons 单元长构造期 eager 归纳");
    assert_eq!(cons.payload_bytes(), 8);
    assert!(!cons.has_lone_surrogate());
    // Cons 扁平化产物恒 FlatU16：units 零拷贝借用来自缓存。
    let u16_node = cons.units().into_owned();
    assert_eq!(u16_node, vec![0x61, 0x62, 0x63, 0x64]);
    assert!(!cons.flat_cache_ptr().is_null());
    unsafe { JsString::drop_cons_node(cons.cons_node_ptr()) };
}

#[test]
fn js_string_cons_deep_chain_flattens_iteratively() {
    // 2000 层左倾链：显式栈展开不爆栈，产物与逐段拼接一致。
    let mut nodes: Vec<Box<JsString>> = Vec::new();
    nodes.push(Box::new(JsString::new("x".to_string())));
    for _ in 0..2000 {
        nodes.push(Box::new(JsString::new("y".to_string())));
    }
    let mut chain_ptr = nodes[0].as_ref() as *const JsString;
    for i in 1..nodes.len() {
        let cons = Box::new(unsafe { JsString::new_cons(chain_ptr, nodes[i].as_ref() as *const JsString) });
        chain_ptr = cons.as_ref() as *const JsString;
        nodes.push(cons);
    }
    let text = nodes.last().unwrap().as_lossy_str();
    assert_eq!(text.len(), 1 + 2000);
    assert!(text.starts_with('x'));
    assert!(text.ends_with('y'));
}

#[test]
fn js_string_cons_empty_parts() {
    // 双空串 Cons：空判定与长度均为 0，扁平化为空文本。
    let left = Box::new(JsString::new(String::new()));
    let right = Box::new(JsString::new(String::new()));
    let cons = Box::new(unsafe { JsString::new_cons(&*left, &*right) });
    assert!(cons.is_empty());
    assert_eq!(cons.utf16_len(), 0);
    assert_eq!(cons.as_lossy_str(), "");
}
