use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<(Vm, JsValue), String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(module))?;
    Ok((vm, result))
}

fn stringify_val(val: &JsValue) -> String {
    if val.is_string() {
        unsafe { (*val.as_string_ptr()).to_owned_string() }
    } else {
        format!("{:?}", val)
    }
}

// --- replacer array ---

#[test]
fn replacer_array_filters_properties() {
    let (_vm, result) = eval(r#"JSON.stringify({a:1,b:2,c:3}, ['a','c'])"#).unwrap();
    assert_eq!(stringify_val(&result), r#"{"a":1,"c":3}"#);
}

#[test]
fn replacer_array_numeric_keys() {
    let (_vm, result) = eval(r#"JSON.stringify({a:1,b:2}, ['b'])"#).unwrap();
    assert_eq!(stringify_val(&result), r#"{"b":2}"#);
}

// --- replacer function ---

#[test]
fn replacer_function_skip_property() {
    let (_vm, result) =
        eval(r#"JSON.stringify({a:1,b:2}, function(k,v){if(k==='a')return undefined;return v})"#).unwrap();
    assert_eq!(stringify_val(&result), r#"{"b":2}"#);
}

#[test]
fn replacer_function_array_null() {
    let (_vm, result) =
        eval(r#"JSON.stringify([1,2,3], function(k,v){if(k==='1')return undefined;return v})"#).unwrap();
    assert_eq!(stringify_val(&result), "[1,null,3]");
}

#[test]
fn replacer_function_transform() {
    let (_vm, result) =
        eval(r#"JSON.stringify({a:1}, function(k,v){if(typeof v==='number')return v*2;return v})"#).unwrap();
    assert_eq!(stringify_val(&result), r#"{"a":2}"#);
}

// --- space ---

#[test]
fn space_number_indent() {
    let (_vm, result) = eval(r#"JSON.stringify({a:1}, null, 2)"#).unwrap();
    let s = stringify_val(&result);
    assert!(s.len() > 5, "expected indented output, got: {}", s);
    assert!(s.starts_with('{'), "should start with brace");
}

#[test]
fn space_negative_clamped() {
    let (_vm, result) = eval(r#"JSON.stringify({a:1}, null, -5)"#).unwrap();
    assert_eq!(stringify_val(&result), r#"{"a":1}"#);
}

// --- toJSON ---

#[test]
fn tojson_called_before_serialize() {
    let (_vm, result) = eval(r#"JSON.stringify({toJSON:function(){return {x:1}}})"#).unwrap();
    assert_eq!(stringify_val(&result), r#"{"x":1}"#);
}

#[test]
fn tojson_non_callable_ignored() {
    let (_vm, result) = eval(r#"JSON.stringify({toJSON:'not-a-function', a:1})"#).unwrap();
    // toJSON 属性是字符串，应按普通属性序列化。
    assert!(stringify_val(&result).contains("toJSON"));
    assert!(stringify_val(&result).contains(r#""a":1"#));
}

// --- cycle detection ---

#[test]
fn cycle_throws_type_error() {
    let result = eval("var a={};a.self=a;JSON.stringify(a)");
    match result {
        Err(e) => assert!(e.to_lowercase().contains("circular"), "error should mention 'circular', got: {}", e),
        Ok(_) => panic!("expected cycle error, got ok"),
    }
}

#[test]
fn no_cycle_distinct_objects_ok() {
    let (_vm, result) = eval(r#"JSON.stringify([{a:1},{a:1}])"#).unwrap();
    assert_eq!(stringify_val(&result), r#"[{"a":1},{"a":1}]"#);
}

// --- reviver ---

#[test]
fn reviver_transform_values() {
    let (_vm, result) =
        eval(r#"JSON.parse('{"a":1,"b":2}', function(k,v){if(typeof v==='number')return v*2;return v})"#).unwrap();
    assert!(result.is_object());
}

#[test]
fn reviver_delete_property() {
    let (_vm, result) =
        eval(r#"JSON.parse('{"a":1,"b":2}', function(k,v){if(k==='a')return undefined;return v})"#).unwrap();
    // 属性被软删除（置为 undefined）。
    // 为 test262 兼容，stringify 应省略 undefined 属性。
    assert!(result.is_object());
}

#[test]
fn reviver_root_key_empty_string() {
    let (_vm, result) = eval(r#"JSON.parse('42', function(k,v){return v+1})"#).unwrap();
    assert!(result.is_int() || result.is_double(), "expected number");
    if result.is_int() {
        assert_eq!(result.as_int(), 43);
    } else {
        assert_eq!(result.as_double() as i32, 43);
    }
}

#[test]
fn reviver_no_reviver_works() {
    let (_vm, result) = eval(r#"JSON.parse('{"a":1}')"#).unwrap();
    assert!(result.is_object());
}

// --- accessor 自身属性值读（SerializeJSONProperty 步 2 Get）---

// reverse arraylike + length accessor：getter 被触发，length 键保留在输出。
#[test]
fn stringify_reverse_arraylike_length_accessor() {
    let (_vm, result) = eval(
        "(() => { const o = {0:'x',1:'y',length:2}; \
         Object.defineProperty(o,'length',{get(){return 2;},set(v){},enumerable:true,configurable:true}); \
         Array.prototype.reverse.call(o); return JSON.stringify(o); })()",
    )
    .unwrap();
    assert_eq!(stringify_val(&result), r#"{"0":"y","1":"x","length":2}"#);
}

// 对象属性访问器：字面量 getter 与 defineProperty 重定义 accessor 两形态。
#[test]
fn stringify_object_accessor_getter() {
    let (_vm, result) = eval(r#"JSON.stringify({get x(){return 5;}, y:1})"#).unwrap();
    assert_eq!(stringify_val(&result), r#"{"x":5,"y":1}"#);
    let (_vm, result) = eval(
        "(() => { const o = {x:1}; \
         Object.defineProperty(o,'x',{get(){return 2;}}); return JSON.stringify(o); })()",
    )
    .unwrap();
    assert_eq!(stringify_val(&result), r#"{"x":2}"#);
}

// getter 的 this 绑定为属性所在对象。
#[test]
fn stringify_accessor_this_binding() {
    let (_vm, result) = eval(r#"JSON.stringify({v:1, get p(){return this.v+10;}})"#).unwrap();
    assert_eq!(stringify_val(&result), r#"{"v":1,"p":11}"#);
}

// 数组 replacer 白名单判定先于 Get：非白名单键的 getter 不触发。
#[test]
fn stringify_whitelist_before_accessor_get() {
    let out = eval_str(
        "(() => { let fired = false; const o = {a:1, get b(){fired=true;return 2;}}; \
         return JSON.stringify(o,['a']) + '|' + fired; })()",
    )
    .unwrap();
    assert_eq!(out, r#"{"a":1}|false"#);
}

// 数组元素访问器：元素位触发 getter，值为 getter 返回值。
#[test]
fn stringify_array_element_accessor() {
    let out = eval_str(
        "(() => { const a = [1,2]; \
         Object.defineProperty(a,0,{get(){return 9;},configurable:true}); \
         Object.defineProperty(a,1,{get(){return 2;},configurable:true}); \
         return JSON.stringify(a); })()",
    )
    .unwrap();
    assert_eq!(out, "[9,2]");
}

// getter 返回值形态：function/undefined 省略，带 toJSON 的对象走钩子，setter-only 省略。
#[test]
fn stringify_accessor_return_shapes() {
    let out = eval_str(
        "(() => { const o = {}; \
         Object.defineProperty(o,'f',{get(){ return function g(){return 7;}; },enumerable:true,configurable:true}); \
         Object.defineProperty(o,'u',{get(){ return undefined; },enumerable:true,configurable:true}); \
         Object.defineProperty(o,'t',{get(){ return {toJSON(){return 7;}}; },enumerable:true,configurable:true}); \
         Object.defineProperty(o,'s',{set(v){},enumerable:true,configurable:true}); \
         return JSON.stringify(o); })()",
    )
    .unwrap();
    assert_eq!(out, r#"{"t":7}"#);
}

// space 缩进 + 访问器：getter 值正常入输出（判别 p 键存在；对象开括号换行
// 位置是 space 格式面 pre-existing 偏差，不属本钉面）。
#[test]
fn stringify_space_with_accessor() {
    let (_vm, result) = eval(
        "(() => { const o = {}; \
         Object.defineProperty(o,'p',{get(){return [1];},enumerable:true,configurable:true}); \
         return JSON.stringify({o},null,2); })()",
    )
    .unwrap();
    let s = stringify_val(&result);
    assert!(s.contains(r#""p": ["#), "getter value missing from indented output: {}", s);
    assert!(s.contains("1"), "getter value absent: {}", s);
}

// getter 抛非 Error 原始值：原值传播（捕获侧见 number 42，非 TypeError 对象）。
#[test]
fn stringify_accessor_throws_original_value() {
    let out = eval_str(
        "(() => { try { JSON.stringify({get x(){throw 42;}, y:1}); } \
         catch (e) { return typeof e + '|' + e; } })()",
    )
    .unwrap();
    assert_eq!(out, "number|42");
}

// toJSON 抛非 Error 原始值：原值传播。
#[test]
fn stringify_tojson_throws_original_value() {
    let out = eval_str(
        "(() => { try { JSON.stringify({toJSON(){throw 42;}}); } \
         catch (e) { return typeof e + '|' + e; } })()",
    )
    .unwrap();
    assert_eq!(out, "number|42");
}

// replacer 抛非 Error 原始值：原值传播。
#[test]
fn stringify_replacer_throws_original_value() {
    let out = eval_str(
        "(() => { try { JSON.stringify({a:1}, function(k,v){ if(k==='a') throw 'boom'; return v; }); } \
         catch (e) { return typeof e + '|' + e; } })()",
    )
    .unwrap();
    assert_eq!(out, "string|boom");
}

// 嵌套序：toJSON 先于 replacer 应用，replacer 见到 toJSON 结果。
#[test]
fn stringify_nested_tojson_before_replacer() {
    let out = eval_str(
        "(() => { let seen; const inner = {toJSON(){return 'T';}}; \
         const s = JSON.stringify({a:inner}, function(k,v){ if(k==='a') seen = String(v); return v; }); \
         return seen + '|' + s; })()",
    )
    .unwrap();
    assert_eq!(out, r#"T|{"a":"T"}"#);
}

// 顶层 toJSON 结果自身带 toJSON：只应用一次。
#[test]
fn stringify_top_level_tojson_once() {
    let (_vm, result) = eval(r#"JSON.stringify({toJSON(){return {toJSON(){return 'inner';}};}})"#).unwrap();
    assert_eq!(stringify_val(&result), r#"{}"#);
}

// 访问器返回自身成环：getter 触发后环检抛 TypeError。
#[test]
fn stringify_accessor_self_cycle() {
    let result = eval(
        "(() => { const o = {n:1}; \
         Object.defineProperty(o,'self',{get(){return o;},enumerable:true,configurable:true}); \
         return JSON.stringify(o); })()",
    );
    match result {
        Err(e) => assert!(e.to_lowercase().contains("circular"), "expected circular error, got: {}", e),
        Ok(_) => panic!("expected cycle error"),
    }
}

// 不可枚举访问器：省略（守卫，防修后翻红）。
#[test]
fn stringify_non_enumerable_accessor_skipped() {
    let out = eval_str(
        "(() => { const o = {v:2}; \
         Object.defineProperty(o,'h',{get(){return 99;},enumerable:false,configurable:true}); \
         return JSON.stringify(o); })()",
    )
    .unwrap();
    assert_eq!(out, r#"{"v":2}"#);
}

// 嵌套 toJSON 结果为对象：单应用下正常展开（守卫）。
#[test]
fn stringify_nested_tojson_result_object() {
    let (_vm, result) = eval(r#"JSON.stringify({a:{toJSON(){return {};}}})"#).unwrap();
    assert_eq!(stringify_val(&result), r#"{"a":{}}"#);
}

// 数组附加命名属性：序列化省略（守卫）。
#[test]
fn stringify_array_ignores_named_props() {
    let out = eval_str("(() => { const a = [1,2]; a.x = 'hello'; return JSON.stringify(a); })()").unwrap();
    assert_eq!(out, "[1,2]");
}

fn eval_str(source: &str) -> Result<String, String> {
    let (vm, result) = eval(source)?;
    vm.lookup_str(result)
        .ok_or_else(|| "completion value is not a string".to_string())
}
