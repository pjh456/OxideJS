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

// gOPD 不变量检查（writable 位方向与值一致性）：target 不可写且 result 可写时
// 抛 TypeError（规范 ValidateAndApplyPropertyDescriptor 步 5e.i，与可扩展性
// 无关）；result 不可配置且不可写、target 可写时抛 TypeError（规范
// [[GetOwnProperty]] 步 16b 形态，无值相等豁免）。
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

// 5e.i 可扩展 target 情形：target 可扩展、targetDesc 不可写且不可配置、
// result 可写时同样抛 TypeError。
#[test]
fn gopd_trap_writable_relaxation_extensible_target_throws() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(() => { \
         var t = { a: 1 }; \
         Object.defineProperty(t, 'a', { value: 1, writable: false, configurable: false, enumerable: false }); \
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

// targetDesc 可配置时放宽：交叉检查不执行，result 可写且可配置不抛。
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

// target 不可扩展且 targetDesc 可配置、result 可配置：规范对 current 可配置
// 无约束，不抛。
#[test]
fn gopd_trap_nonextensible_configurable_target_relaxed() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(() => { \
         var t = { a: 1 }; \
         Object.defineProperty(t, 'a', { value: 1, writable: false, configurable: true, enumerable: false }); \
         Object.preventExtensions(t); \
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

// 步 16b 方向：result 不可配置且不可写、target 可写时抛 TypeError（与值无关）。
#[test]
fn gopd_trap_nonwritable_result_writable_target_throws() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(() => { \
         var t = { a: 1 }; \
         Object.defineProperty(t, 'a', { value: 1, writable: true, configurable: false, enumerable: false }); \
         var p = new Proxy(t, { getOwnPropertyDescriptor: function (tt, k) { \
             return { configurable: false, writable: false }; \
         } }); \
         try { Object.getOwnPropertyDescriptor(p, 'a'); return 'no-throw'; } \
         catch (e) { return e.name; } \
       })()",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap_or_default(), "TypeError");
}

// 步 16b 无值相等豁免：target 可写、result 不可配置且不可写且值 SameValue
// 时同样抛 TypeError。
#[test]
fn gopd_trap_nonwritable_result_same_value_throws() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(() => { \
         var t = { a: 1 }; \
         Object.defineProperty(t, 'a', { value: 1, writable: true, configurable: false, enumerable: false }); \
         Object.preventExtensions(t); \
         var p = new Proxy(t, { getOwnPropertyDescriptor: function (tt, k) { \
             return { value: 1, writable: false, enumerable: false, configurable: false }; \
         } }); \
         try { Object.getOwnPropertyDescriptor(p, 'a'); return 'no-throw'; } \
         catch (e) { return e.name; } \
       })()",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap_or_default(), "TypeError");
}

// 步 16b 方向：target 可写、result 不可写且值不一致时抛 TypeError。
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

// 5e.ii 值检查：target 不可写且 result 有值字段、值不一致时抛 TypeError
// （target 不可写时步 16b 不触发，此情形独立钉 5e.ii）。
#[test]
fn gopd_trap_nonwritable_target_value_mismatch_throws() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(() => { \
         var t = { a: 1 }; \
         Object.defineProperty(t, 'a', { value: 1, writable: false, configurable: false, enumerable: false }); \
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

// 5e.ii 值检查通过侧：target 不可写且 result 有值字段、值 SameValue 时不抛。
#[test]
fn gopd_trap_nonwritable_target_value_same_ok() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "(() => { \
         var t = { a: 1 }; \
         Object.defineProperty(t, 'a', { value: 1, writable: false, configurable: false, enumerable: false }); \
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
