use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&Arc::new(module))
}

fn string_value(vm: &Vm, val: JsValue) -> String {
    vm.lookup_str(val).unwrap_or_default()
}

// -- JSON.parse --

#[test]
fn json_parse_object() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('{\"a\":1,\"b\":2}')").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_count(), 2);
}

#[test]
fn json_parse_array() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('[1,2,3]')").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert!(obj.is_array());
    assert_eq!(obj.prop_count(), 3);
    assert_eq!(obj.get_prop_at(2).as_double(), 3.0);
}

#[test]
fn json_parse_string() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('\"hello\"')").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "hello");
}

#[test]
fn json_parse_number() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('42')").unwrap();
    assert!(result.is_double());
}

#[test]
fn json_parse_float() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('3.14')").unwrap();
    assert!(result.is_double());
}

#[test]
fn json_parse_true() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('true')").unwrap();
    assert!(result.is_bool());
    assert!(result.as_bool());
}

#[test]
fn json_parse_false() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('false')").unwrap();
    assert!(result.is_bool());
    assert!(!result.as_bool());
}

#[test]
fn json_parse_null() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('null')").unwrap();
    assert!(result.is_null());
}

#[test]
fn json_parse_nested() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('{\"a\":1,\"b\":[1,2],\"c\":{\"d\":\"hello\"}}')").unwrap();
    assert!(result.is_object());
}

#[test]
fn json_parse_invalid_throws() {
    let mut vm = Vm::new();
    let err = eval(&mut vm, "JSON.parse('not json')");
    assert!(err.is_err());
}

// -- JSON.stringify --

#[test]
fn json_stringify_object() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify({a:1})").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "{\"a\":1}");
}

#[test]
fn json_stringify_array() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify([1,2,3])").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "[1,2,3]");
}

#[test]
fn json_stringify_array_with_negative_double() {
    // 数组内负 double：out 已含 "[1,"，负号必须追加在当前数前（ryu 路径）。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify([1,-1.5])").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "[1,-1.5]");
}

#[test]
fn json_stringify_string() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify('hello')").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "\"hello\"");
}

#[test]
fn json_stringify_number() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify(42)").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "42");
}

#[test]
fn json_stringify_float() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify(3.14)").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert!(s.starts_with("3.14"));
}

#[test]
fn json_stringify_true() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify(true)").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "true");
}

#[test]
fn json_stringify_false() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify(false)").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "false");
}

#[test]
fn json_stringify_null() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify(null)").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "null");
}

#[test]
fn json_stringify_undefined() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify(undefined)").unwrap();
    assert!(result.is_undefined());
}

// -- Roundtrip --

#[test]
fn json_parse_stringify_roundtrip() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse(JSON.stringify({a:1,b:2}))").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_vec_len(), 2);
}

#[test]
fn json_roundtrip_array() {
    let mut vm = Vm::new();
    let _ = eval(&mut vm, "var arr = JSON.parse(JSON.stringify([1,2,3])); arr").unwrap();
}

// -- Edge cases --

#[test]
fn json_stringify_empty_object() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify({})").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "{}");
}

#[test]
fn json_stringify_empty_array() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify([])").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "[]");
}

#[test]
fn json_stringify_escape_quotes() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify('he\"llo')").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "\"he\\\"llo\"");
}

#[test]
fn json_stringify_escape_backslash() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify('a\\\\b')").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "\"a\\\\b\"");
}

#[test]
fn json_stringify_nested_objects() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify({a: {b: 1}})").unwrap();
    assert!(result.is_string());
    let s = string_value(&vm, result);
    assert_eq!(s, "{\"a\":{\"b\":1}}");
}

#[test]
fn json_parse_empty_object() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('{}')").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert_eq!(obj.prop_vec_len(), 0);
}

#[test]
fn json_parse_empty_array() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('[]')").unwrap();
    assert!(result.is_object());
    let obj = unsafe { &*result.as_js_object_ptr() };
    assert!(obj.is_array());
    assert_eq!(obj.prop_vec_len(), 0);
}

#[test]
fn json_parse_int_is_double() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('123')").unwrap();
    assert!((result.as_double() - 123.0).abs() < 0.0001);
}

// -- Cycle detection --

#[test]
fn json_stringify_cycle_throws_type_error() {
    let mut vm = Vm::new();
    let err = eval(&mut vm, "var a = {}; a.self = a; JSON.stringify(a)");
    assert!(err.is_err());
}

// -- 数字键对象（P1-2 回归防线） --

#[test]
fn json_parse_numeric_object_key_reads_via_index() {
    // JSON 键 "5" 经字符串规范化映射整数键：o[5] 与 o["5"] 命中同一键。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.parse('{\"5\":1}')[5]").unwrap();
    assert!((result.as_double() - 1.0).abs() < 0.0001);
}

#[test]
fn json_parse_numeric_keys_do_not_clash_between_keys() {
    // 多个数字键各自独立：{"0":7,"2024":8} 的 0 与 2024 不互相覆盖。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var o = JSON.parse('{\"0\":7,\"2024\":8}'); o[0] + o[2024]").unwrap();
    assert!((result.as_double() - 15.0).abs() < 0.0001);
}

#[test]
fn json_parse_numeric_keys_stringify_roundtrip() {
    // 数字键对象经 stringify 后键名保持 "5"（键字符串化语义）。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify(JSON.parse('{\"5\":1}'))").unwrap();
    let s = string_value(&vm, result);
    assert_eq!(s, "{\"5\":1}");
}

// -- JSON.stringify 单元语义（孤立 surrogate 转义 / 代理对原样 / 真实键串） --

#[test]
fn json_stringify_unpaired_surrogate_escaped() {
    // 非配对孤立 surrogate 输出 \uXXXX（well-formed JSON 要求）。
    let mut vm = Vm::new();
    let r1 = eval(&mut vm, "JSON.stringify(String.fromCharCode(0xD800)) === '\"\\\\ud800\"'").unwrap();
    assert!(r1.as_bool(), "unpaired high surrogate must be \\ud800-escaped");
    let r2 = eval(&mut vm, "JSON.stringify(String.fromCharCode(0xDF06)) === '\"\\\\udf06\"'").unwrap();
    assert!(r2.as_bool(), "unpaired low surrogate must be \\udf06-escaped");
}

#[test]
fn json_stringify_surrogate_pair_raw() {
    // 配对 surrogate 原样输出 astral 字符；混串按 spec 逐单元处置。
    let mut vm = Vm::new();
    let r1 = eval(&mut vm, "JSON.stringify(String.fromCharCode(0xD834, 0xDF06)) === '\"\\u{1D306}\"'").unwrap();
    assert!(r1.as_bool(), "paired surrogates must serialize as the raw astral char");
    let r2 = eval(
        &mut vm,
        "JSON.stringify(String.fromCharCode(0xD834, 0xD834, 0xDF06, 0xD834)) === '\"\\\\ud834\\u{1D306}\\\\ud834\"'",
    )
    .unwrap();
    assert!(r2.as_bool(), "unpaired units around a pair must each be escaped");
}

// -- JSON.stringify 装箱 String（[[StringData]] 臂） --

#[test]
fn json_stringify_boxed_string_top_level() {
    // 顶层装箱 String 序列化为载荷文本（[[StringData]] 臂 ToString，非索引枚举）。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify(new String('x'))").unwrap();
    assert_eq!(string_value(&vm, result), "\"x\"");
}

#[test]
fn json_stringify_boxed_string_object_property() {
    // 对象属性位同臂：{"a":"x"}。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify({a: new String('x')})").unwrap();
    assert_eq!(string_value(&vm, result), "{\"a\":\"x\"}");
}

#[test]
fn json_stringify_boxed_string_array_elements() {
    // 数组元素位：["x","yz"]。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify([new String('x'), new String('yz')])").unwrap();
    assert_eq!(string_value(&vm, result), "[\"x\",\"yz\"]");
}

#[test]
fn json_stringify_boxed_string_respects_tostring_override() {
    // 完整 ToString 语义：toString 覆盖生效，valueOf 不在 string hint 路径被调用。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var s = new String('x'); s.toString = function() { return 'y'; }; \
         s.valueOf = function() { throw new Error('no'); }; JSON.stringify(s)",
    )
    .unwrap();
    assert_eq!(string_value(&vm, result), "\"y\"");
}

#[test]
fn json_stringify_boxed_string_abrupt_tostring_propagates() {
    // 值位 abrupt toString 抛原始值（原样传播，不折叠为环检 TypeError）。
    let mut vm = Vm::new();
    let err = eval(
        &mut vm,
        "var s = new String('x'); s.toString = function() { throw new RangeError('SE'); }; \
         JSON.stringify(s)",
    );
    assert!(err.is_err());
}

#[test]
fn json_stringify_string_prototype_is_empty_string() {
    // String.prototype 是规范 String exotic 对象（[[StringData]] 空串）→ ""。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "JSON.stringify(String.prototype)").unwrap();
    assert_eq!(string_value(&vm, result), "\"\"");
}

#[test]
fn json_stringify_boxed_string_space_argument() {
    // space 位装箱 String 与同文本原语串缩进同形。
    let mut vm = Vm::new();
    let r1 = eval(&mut vm, "JSON.stringify({a: 1}, undefined, new String('xxx'))").unwrap();
    let r2 = eval(&mut vm, "JSON.stringify({a: 1}, undefined, 'xxx')").unwrap();
    assert_eq!(string_value(&vm, r1), string_value(&vm, r2));
}

#[test]
fn json_stringify_boxed_string_space_tostring_override() {
    // space 位 toString 覆盖生效（完整 ToString，非直读载荷）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var s = new String('zzz'); s.toString = function() { return '---'; }; \
         JSON.stringify({a: 1}, undefined, s)",
    )
    .unwrap();
    let plain = eval(&mut vm, "JSON.stringify({a: 1}, undefined, '---')").unwrap();
    assert_eq!(string_value(&vm, result), string_value(&vm, plain));
}

#[test]
fn json_stringify_boxed_string_space_abrupt_throws() {
    // space 位 abrupt toString 抛原始值。
    let mut vm = Vm::new();
    let err = eval(
        &mut vm,
        "var s = new String('x'); s.toString = function() { throw new RangeError('SE'); }; \
         JSON.stringify({a: 1}, undefined, s)",
    );
    assert!(err.is_err());
}

#[test]
fn json_stringify_boxed_string_tostring_result_units_escaped() {
    // 完整 ToString 产物经单元流序列化：孤立 surrogate → \uXXXX 转义（well-formed JSON）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var s = new String('x'); \
         s.toString = function() { return String.fromCharCode(0xD800); }; \
         JSON.stringify(s) === '\"\\\\ud800\"'",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn json_stringify_key_units_real() {
    // 键序列化用真实键串（孤立 surrogate 转义），toJSON/replacer 键参数同口径。
    let mut vm = Vm::new();
    let r1 = eval(
        &mut vm,
        "var o = {}; o[String.fromCharCode(0xD800)] = 1; JSON.stringify(o) === '{\"\\\\ud800\":1}'",
    )
    .unwrap();
    assert!(r1.as_bool(), "object key with lone surrogate must serialize escaped");
    let r2 = eval(
        &mut vm,
        "var ks = []; JSON.stringify({[String.fromCharCode(0xD800)]: 1}, function(k, v) { if (k !== '') ks.push(k.charCodeAt(0)); return v; }); ks.length === 1 && ks[0] === 0xD800",
    )
    .unwrap();
    assert!(r2.as_bool(), "replacer must receive the real key string (lone unit, not FFFD)");
    let r3 = eval(
        &mut vm,
        "var tk = -1; var inner = { toJSON: function(k) { tk = k.charCodeAt(0); return 'x'; } }; var outer = {}; outer[String.fromCharCode(0xD800)] = inner; JSON.stringify(outer); tk === 0xD800",
    )
    .unwrap();
    assert!(r3.as_bool(), "toJSON must receive the real key string (lone unit, not FFFD)");
}
