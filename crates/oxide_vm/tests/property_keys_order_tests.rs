use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<(Vm, JsValue), String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    let result = vm.run(&module)?;
    Ok((vm, result))
}

fn eval_str(source: &str) -> Result<JsValue, String> {
    eval(source).map(|(_vm, v)| v)
}

fn keys_to_strings(result: JsValue) -> Vec<String> {
    let obj = unsafe { &*result.as_js_object_ptr() };
    let len = obj.prop_count();
    let mut keys = Vec::new();
    for i in 0..len {
        let v = obj.get_prop_at(i);
        if v.is_string() {
            // SAFETY: 字符串值指向 VM arena（调用方保持 vm 存活）。
            let s = unsafe { &*v.as_string_ptr() };
            keys.push(s.to_owned_string());
        }
    }
    keys
}

// ── 整型键按升序 ──
#[test]
fn keys_integer_order_ascending() {
    let r = eval_str("var obj={}; obj[3]='c'; obj[1]='a'; obj[2]='b'; Object.keys(obj).length").unwrap();
    assert!(r.is_int() && r.as_int() == 3, "should have 3 keys, got {:?}", r);
    let (_vm, r2) = eval("var obj={}; obj[3]='c'; obj[1]='a'; obj[2]='b'; Object.keys(obj)").unwrap();
    let keys = keys_to_strings(r2);
    assert_eq!(keys, vec!["1", "2", "3"], "integer keys should be ascending, got {:?}", keys);
}

// ── 混合键：整型优先，字符串按创建序 ──
#[test]
fn keys_mixed_order_integers_first() {
    let (_vm, r) = eval("var obj={}; obj[2]='b'; obj['1']='a'; obj['a']=1; Object.keys(obj)").unwrap();
    let keys = keys_to_strings(r);
    assert!(keys[0] == "1" || keys[0] == "2", "first key should be integer, got {:?}", keys);
    let str_idx = keys.iter().position(|k| k == "a").unwrap();
    let int_positions: Vec<usize> = keys
        .iter()
        .enumerate()
        .filter(|(_, k)| k.parse::<u32>().is_ok())
        .map(|(i, _)| i)
        .collect();
    for ip in &int_positions {
        assert!(*ip < str_idx, "integer at {} should come before string 'a' at {}", ip, str_idx);
    }
}

// ── Object.keys 排除不可枚举属性 ──
#[test]
fn keys_excludes_non_enumerable() {
    let r = eval_str(
        "var obj={a:1}; Object.defineProperty(obj,'hidden',{value:2,enumerable:false}); Object.keys(obj).length",
    )
    .unwrap();
    assert!(r.is_int() && r.as_int() == 1, "keys should exclude non-enumerable, got {:?}", r);
}

// ── Object.getOwnPropertyNames 包含不可枚举属性 ──
#[test]
fn get_own_property_names_includes_non_enumerable() {
    let r = eval_str("var obj={a:1}; Object.defineProperty(obj,'hidden',{value:2,enumerable:false}); Object.getOwnPropertyNames(obj).length").unwrap();
    assert!(
        r.is_int() && r.as_int() == 2,
        "getOwnPropertyNames should include non-enumerable, got {:?}",
        r
    );
}

// ── Object.entries 排除不可枚举属性 ──
#[test]
fn entries_excludes_non_enumerable() {
    let r = eval_str(
        "var obj={a:1}; Object.defineProperty(obj,'hidden',{value:2,enumerable:false}); Object.entries(obj).length",
    )
    .unwrap();
    assert!(r.is_int() && r.as_int() == 1, "entries should exclude non-enumerable, got {:?}", r);
}

// ── Object.values 排除不可枚举属性 ──
#[test]
fn values_excludes_non_enumerable() {
    let r = eval_str(
        "var obj={a:1}; Object.defineProperty(obj,'hidden',{value:2,enumerable:false}); Object.values(obj).length",
    )
    .unwrap();
    assert!(r.is_int() && r.as_int() == 1, "values should exclude non-enumerable, got {:?}", r);
}

// ── 前导零字符串不是整型下标 ──
#[test]
fn keys_with_leading_zero_not_integer_index() {
    let (_vm, r) = eval("Object.keys({'01':'a','1':'b'})").unwrap();
    let keys = keys_to_strings(r);
    assert!(keys.contains(&"01".to_string()), "should contain '01' as string key");
    assert!(keys.contains(&"1".to_string()), "should contain '1' as integer");
    let idx_01 = keys.iter().position(|k| k == "01").unwrap();
    let idx_1 = keys.iter().position(|k| k == "1").unwrap();
    assert!(idx_1 < idx_01, "integer '1' should come before string '01'");
}

// ── 对象字面量数字键字符串化 ──
#[test]
fn numeric_literal_key_materializes_as_string() {
    // `{5:1}` 的键是字符串 "5"（键经 ToPropertyKey 字符串化），读 o[5]/o["5"] 同一键。
    let r = eval_str("var o={5:1}; Object.getOwnPropertyNames(o)[0]").unwrap();
    assert!(r.is_string());
    let s = unsafe { &*r.as_string_ptr() }.to_owned_string();
    assert_eq!(s, "5");
    let r = eval_str("var o={5:1}; o[5] + o['5']").unwrap();
    assert!(r.is_int() && r.as_int() == 2);
}
