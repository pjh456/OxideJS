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

// gOPD 不变量检查（writable 位方向与 value 一致性）：target 不可写加 result
// 可写时抛 TypeError（规范 ValidateAndApplyPropertyDescriptor 步 5.5.1 口径）。
#[test]
fn gopd_trap_writable_relaxation_throws() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(() => { \
         var t = { a: 1 }; \
         Object.defineProperty(t, 'a', { writable: false, configurable: false, enumerable: false }); \
         Object.preventExtensions(t); \
         var p = new Proxy(t, { getOwnPropertyDescriptor: function (tt, k) { \
             return { value: 1, writable: true, enumerable: false, configurable: false }; \
         } }); \
         try { Object.getOwnPropertyDescriptor(p, 'a'); return 'no-throw'; } \
         catch (e) { return e.name; } \
       })()",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap_or_default(), "TypeError");
}

// target 可配置时放宽：writable 交叉检查不执行，result 可写不抛。
#[test]
fn gopd_trap_configurable_target_relaxed() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(() => { \
         var t = { a: 1 }; \
         Object.defineProperty(t, 'a', { writable: false, enumerable: false }); \
         var p = new Proxy(t, { getOwnPropertyDescriptor: function (tt, k) { \
             return { value: 1, writable: true, enumerable: false, configurable: true }; \
         } }); \
         try { Object.getOwnPropertyDescriptor(p, 'a'); return 'ok'; } \
         catch (e) { return e.name; } \
       })()",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap_or_default(), "ok");
}

// value 一致性：target 可写加 result 不可写且值不同抛 TypeError。
#[test]
fn gopd_trap_value_mismatch_throws() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(() => { \
         var t = { a: 1 }; \
         Object.defineProperty(t, 'a', { writable: true, configurable: false, enumerable: false }); \
         Object.preventExtensions(t); \
         var p = new Proxy(t, { getOwnPropertyDescriptor: function (tt, k) { \
             return { value: 2, writable: false, enumerable: false, configurable: false }; \
         } }); \
         try { Object.getOwnPropertyDescriptor(p, 'a'); return 'no-throw'; } \
         catch (e) { return e.name; } \
       })()",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap_or_default(), "TypeError");
}

// value 一致性：target 可写加 result 不可写且值 SameValue 不抛。
#[test]
fn gopd_trap_value_same_ok() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(() => { \
         var t = { a: 1 }; \
         Object.defineProperty(t, 'a', { writable: true, configurable: false, enumerable: false }); \
         Object.preventExtensions(t); \
         var p = new Proxy(t, { getOwnPropertyDescriptor: function (tt, k) { \
             return { value: 1, writable: false, enumerable: false, configurable: false }; \
         } }); \
         try { Object.getOwnPropertyDescriptor(p, 'a'); return 'ok'; } \
         catch (e) { return e.name; } \
       })()",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap_or_default(), "ok");
}
