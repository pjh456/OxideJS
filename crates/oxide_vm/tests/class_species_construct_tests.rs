//! Array[Symbol.species] 访问器绑定 + 内建侧真 [[Construct]] + derived 构造器
//! 隐式返回交付当前 this 的引擎钉：派生类 B 经 map/filter/slice/flat/flatMap 的
//! 结果实例身份回 B，静态链解析 B 自身，显式 super 构造器多参元素拷贝。

use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval_value(source: &str) -> Result<(Vm, JsValue), String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    let result = vm.run(&Arc::new(module))?;
    Ok((vm, result))
}

fn eval_str(source: &str) -> Result<String, String> {
    let (vm, result) = eval_value(source)?;
    vm.lookup_str(result)
        .ok_or_else(|| "completion value is not a string".to_string())
}

// Array[Symbol.species] 是访问器属性：get 函数、set undefined、!enumerable、configurable。
#[test]
fn eval_species_accessor_descriptor() {
    let out = eval_str(
        "(() => { const d = Object.getOwnPropertyDescriptor(Array, Symbol.species); \
         return typeof d.get + '|' + (d.set === undefined) + '|' + d.enumerable + '|' + d.configurable; })()",
    )
    .unwrap();
    assert_eq!(out, "function|true|false|true");
}

// species getter 的 Function.length 为 0。
#[test]
fn eval_species_getter_length_zero() {
    let out = eval_str("String(Object.getOwnPropertyDescriptor(Array, Symbol.species).get.length)").unwrap();
    assert_eq!(out, "0");
}

// species getter 的 Function.name 为规范标签。
#[test]
fn eval_species_getter_name() {
    let out = eval_str("Object.getOwnPropertyDescriptor(Array, Symbol.species).get.name").unwrap();
    assert_eq!(out, "get [Symbol.species]");
}

// species getter 返回 receiver（显式 call 形态）。
#[test]
fn eval_species_getter_returns_receiver() {
    let out = eval_str(
        "(() => { const o = {}; \
         return (Object.getOwnPropertyDescriptor(Array, Symbol.species).get.call(o) === o) + ''; })()",
    )
    .unwrap();
    assert_eq!(out, "true");
}

// 派生类沿静态原型链解析 @@species 得自身构造器，且类上无 own 属性。
#[test]
fn eval_species_static_chain_resolves_self() {
    let out = eval_str(
        "(() => { class B extends Array {} \
         return (B[Symbol.species] === B) + '|' + \
         (Object.getOwnPropertyDescriptor(B, Symbol.species) === undefined); })()",
    )
    .unwrap();
    assert_eq!(out, "true|true");
}

// B 实例 map 结果 instanceof B（species 主形态）。
#[test]
fn eval_map_instance_of_derived() {
    let out =
        eval_str("(() => { class B extends Array {} return (new B(2).map(x => x) instanceof B) + ''; })()").unwrap();
    assert_eq!(out, "true");
}

// B 实例 filter/slice/flat/flatMap 四挂载全回 B 实例。
#[test]
fn eval_filter_slice_flat_flatmap_instance_of_derived() {
    let out = eval_str(
        "(() => { class B extends Array {} const b = new B(2); b[0] = 1; b[1] = 2; \
         return (b.filter(x => true) instanceof B) + '|' + \
         (b.slice(0) instanceof B) + '|' + \
         (b.flat() instanceof B) + '|' + \
         (b.flatMap(x => x) instanceof B); })()",
    )
    .unwrap();
    assert_eq!(out, "true|true|true|true");
}

// 嵌套继承 C extends B：C 实例 map 回 C 实例。
#[test]
fn eval_nested_inheritance_map_instance_of() {
    let out = eval_str(
        "(() => { class B extends Array {} class C extends B {} \
                  return (new C(2).map(x => x) instanceof C) + ''; })()",
    )
    .unwrap();
    assert_eq!(out, "true");
}

// species 结果是真数组 exotic（Array.isArray 识别）。
#[test]
fn eval_species_result_is_array() {
    let out =
        eval_str("(() => { class B extends Array {} return Array.isArray(new B(2).map(x => x)) + ''; })()").unwrap();
    assert_eq!(out, "true");
}

// 显式单参 super 构造器：隐式返回交付 super 新建的实例（length 来自父构造器）。
#[test]
fn eval_explicit_super_single_arg_length() {
    let out = eval_str(
        "(() => { class Sub extends Array { constructor(a) { super(a); } } \
         return new Sub(42).length + ''; })()",
    )
    .unwrap();
    assert_eq!(out, "42");
}

// 显式多参 super 构造器：元素经父构造器拷贝进实例。
#[test]
fn eval_explicit_super_spread_elements() {
    let out = eval_str(
        "(() => { class Sub extends Array { constructor(...a) { super(...a); } } \
         return JSON.stringify(new Sub(1, 2, 3)); })()",
    )
    .unwrap();
    assert_eq!(out, "[1,2,3]");
}

// Array.of 经 C 构造：class 形态回 C 实例。
#[test]
fn eval_array_of_via_derived_ctor() {
    let out =
        eval_str("(() => { class B extends Array {} return (Array.of.call(B, 1, 2) instanceof B) + ''; })()").unwrap();
    assert_eq!(out, "true");
}

// species 为普通可构造 Ctor：返回体直接作结果（元素 CDO 写入其上）。
#[test]
fn eval_species_plain_ctor_returned_body() {
    let out = eval_str(
        "(() => { function Ctor(n) { const a = [7, 8]; a.push(n); return a; } \
         const a = [1, 2]; a.constructor = {}; a.constructor[Symbol.species] = Ctor; \
         const r = a.map(x => x); \
         return r.length + '|' + r[0] + '|' + r[1] + '|' + r[2] + '|' + (r instanceof Array); })()",
    )
    .unwrap();
    assert_eq!(out, "3|1|2|2|true");
}

// 守卫：构造器直 new 面不回归。
#[test]
fn eval_guard_derived_new_instance() {
    let out = eval_str("(() => { class B extends Array {} return (new B(2) instanceof B) + ''; })()").unwrap();
    assert_eq!(out, "true");
}

// 守卫：普通数组 map 结果 isArray + 值保真。
#[test]
fn eval_guard_plain_map_values() {
    let out = eval_str("(() => { const m = [1, 2].map(x => x); return Array.isArray(m) + '|' + m[1]; })()").unwrap();
    assert_eq!(out, "true|2");
}

// 守卫：隐式 super 构造器 length 与多参元素。
#[test]
fn eval_guard_implicit_super() {
    let out = eval_str(
        "(() => { class Sub extends Array {} const s = new Sub(7); const t = new Sub(7, 8); \
         return s.length + '|' + t.length + '|' + t[0] + '|' + t[1]; })()",
    )
    .unwrap();
    assert_eq!(out, "7|2|7|8");
}

// 守卫：native 父构造器 Reflect.construct 路径在位。
#[test]
fn eval_guard_reflect_construct_native() {
    let out = eval_str("String(Reflect.construct(Array, [3]).length)").unwrap();
    assert_eq!(out, "3");
}

// 守卫：静态原型链果——B.prototype 的原型即 Array.prototype。
#[test]
fn eval_guard_prototype_chain() {
    let out = eval_str(
        "(() => { class B extends Array {} \
         return (Object.getPrototypeOf(B.prototype) === Array.prototype) + ''; })()",
    )
    .unwrap();
    assert_eq!(out, "true");
}

// 守卫：c={} 形态（constructor 为普通对象、无 species）slice 回普通数组。
#[test]
fn eval_guard_plain_ctor_lookup() {
    let out = eval_str(
        "(() => { const a = [1, 2, 3]; a.constructor = {}; const s = a.slice(1); \
         return s.length + '|' + (s instanceof Array) + '|' + s[0]; })()",
    )
    .unwrap();
    assert_eq!(out, "2|true|2");
}

// 守卫：普通数组 slice/filter 结果 plain Array。
#[test]
fn eval_guard_plain_slice_filter() {
    let out = eval_str(
        "(() => { const a = [1, 2, 3]; \
         return (a.slice(1) instanceof Array) + '|' + (a.filter(x => true) instanceof Array); })()",
    )
    .unwrap();
    assert_eq!(out, "true|true");
}

// 守卫：String 无 species 绑定（不引入 String[Symbol.species]）。
#[test]
fn eval_guard_string_no_species() {
    let out = eval_str(
        "(() => { class S extends String {} \
         return (S[Symbol.species] === undefined) + '|' + (String[Symbol.species] === undefined); })()",
    )
    .unwrap();
    assert_eq!(out, "true|true");
}
