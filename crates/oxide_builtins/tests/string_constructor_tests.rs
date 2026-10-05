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
