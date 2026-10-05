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

fn string_of(source: &str) -> String {
    let (_vm, result) = eval(source).unwrap();
    assert!(result.is_string(), "expected a string result from {source}");
    unsafe { &*result.as_string_ptr() }.as_str().to_string()
}

#[test]
fn string_constructor_primitive_content() {
    // 原始值各类别的内容正确性：整数、负整数、有限 double、布尔、
    // null/undefined、NaN/±Infinity、-0、大指数、字符串直通、无参。
    assert_eq!(string_of("String(42)"), "42");
    assert_eq!(string_of("String(-5)"), "-5");
    assert_eq!(string_of("String(3.14)"), "3.14");
    assert_eq!(string_of("String(true)"), "true");
    assert_eq!(string_of("String(false)"), "false");
    assert_eq!(string_of("String(null)"), "null");
    assert_eq!(string_of("String(undefined)"), "undefined");
    assert_eq!(string_of("String(NaN)"), "NaN");
    assert_eq!(string_of("String(Infinity)"), "Infinity");
    assert_eq!(string_of("String(-Infinity)"), "-Infinity");
    assert_eq!(string_of("String(-0)"), "0");
    assert_eq!(string_of("String(1e21)"), "1e+21");
    assert_eq!(string_of("String(\"abc\")"), "abc");
    assert_eq!(string_of("String()"), "");
}

#[test]
fn string_constructor_perm_identity() {
    // 0..=99 小整数命中小整数永久表：两次调用返回同一指针，且与表项一致。
    let (_vm, a) = eval("String(42)").unwrap();
    let (_vm, b) = eval("String(42)").unwrap();
    assert!(a.is_string() && b.is_string());
    assert_eq!(a.as_string_ptr(), b.as_string_ptr());
    assert_eq!(a.as_string_ptr(), oxide_kernel::string_forge::small_int_ptr(42).unwrap());

    // 布尔/null/undefined/NaN/±Infinity 命中常量表：两次调用返回同一指针。
    for src in [
        "String(true)",
        "String(false)",
        "String(null)",
        "String(undefined)",
        "String(NaN)",
        "String(Infinity)",
        "String(-Infinity)",
    ] {
        let (_vm, a) = eval(src).unwrap();
        let (_vm, b) = eval(src).unwrap();
        assert!(a.is_string() && b.is_string());
        assert_eq!(a.as_string_ptr(), b.as_string_ptr(), "{src} 应返回同一指针");
    }
}

#[test]
fn string_constructor_string_passthrough() {
    // 字符串入参恒等返回：内容不变，且跨 VM 实例与永久表同源（免克隆）。
    let (_vm, result) = eval("String(\"abc\")").unwrap();
    assert!(result.is_string());
    assert_eq!(unsafe { &*result.as_string_ptr() }.as_str(), "abc");
}

#[test]
fn string_constructor_fallback_paths() {
    // 对象参数走完整转换链（valueOf/toString）。
    assert_eq!(string_of("String({})"), "[object Object]");
    // BigInt 参数走 BigInt 路径。
    assert_eq!(string_of("String(1n)"), "1");
    // 装箱 Symbol 抛 TypeError（抛错点不变）。
    assert!(eval("String(Object(Symbol('s')))").is_err());
    // 构造形态返回包装对象，length 为 2。
    let (_vm, result) = eval("new String(42)").unwrap();
    assert!(result.is_object());
    let (_vm, len) = eval("new String(42).length").unwrap();
    assert_eq!(len.as_int(), 2);
    // 构造形态字符串入参：从原串物化盒体。
    let (_vm, len) = eval("new String(\"ab\").length").unwrap();
    assert_eq!(len.as_int(), 2);
}

// 位模式低 4 位互异的十六个有限 double（槽位互不冲突的测试前提）。
const CACHE_TEST_VALUES: [f64; 16] = [
    0.119, 0.103, 0.087, 0.071, 0.063, 0.207, 0.175, 0.143, 0.015, 0.007, 0.003, 0.005, 0.001, 0.205, 0.173, 0.141,
];

#[test]
fn number_to_string_cache_slot_correctness() {
    // 十六个槽位互异的值两轮各调一次：命中须键全等才返回，
    // 钉住槽位碰撞不得错返。
    let mut vm = Vm::new();
    let mut seen = [false; 16];
    for &d in &CACHE_TEST_VALUES {
        let slot = (d.to_bits() as usize) & 15;
        assert!(!seen[slot], "槽位冲突：{d}");
        seen[slot] = true;
    }
    for _round in 0..2 {
        for &d in &CACHE_TEST_VALUES {
            let v = vm.number_to_string_cached(d);
            assert!(v.is_string());
            let expected = oxide_runtime_api::js_number_to_string(d);
            assert_eq!(unsafe { &*v.as_string_ptr() }.as_str(), expected.as_str(), "值 {d} 应与原转换链一致");
        }
    }
}

#[test]
fn number_to_string_cache_survives_session_gc() {
    // 填满十六槽后强制完整 session 收集：缓存串已登记为 GC 根，
    // 收集后读回仍为原内容（漏根登记此测必挂）。
    let mut vm = Vm::new();
    for &d in &CACHE_TEST_VALUES {
        vm.number_to_string_cached(d);
    }
    vm.collect_session_gc();
    for &d in &CACHE_TEST_VALUES {
        let v = vm.number_to_string_cached(d);
        assert!(v.is_string());
        let expected = oxide_runtime_api::js_number_to_string(d);
        assert_eq!(unsafe { &*v.as_string_ptr() }.as_str(), expected.as_str(), "收集后值 {d} 应可读回");
    }
    // 2.5 挤占槽 0 后写新串：新串同样须存活后续收集。
    let v = vm.number_to_string_cached(2.5);
    vm.collect_session_gc();
    let v2 = vm.number_to_string_cached(2.5);
    assert_eq!(unsafe { &*v2.as_string_ptr() }.as_str(), "2.5");
    assert_eq!(v.as_string_ptr(), v2.as_string_ptr());
}

#[test]
fn number_to_string_cache_identity() {
    // 同值两次调用命中同槽：返回指针相等（命中复用 session 串）。
    let mut vm = Vm::new();
    let a = vm.number_to_string_cached(2.5);
    let b = vm.number_to_string_cached(2.5);
    assert!(a.is_string() && b.is_string());
    assert_eq!(a.as_string_ptr(), b.as_string_ptr());
}
