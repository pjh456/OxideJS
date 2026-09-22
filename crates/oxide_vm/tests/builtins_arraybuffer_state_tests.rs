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

/// 原型 byteLength 描述符钉：get 为函数、set 缺失、不可枚举、可配置。
#[test]
fn ab_bytelength_proto_is_accessor() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength'); \
         typeof d.get === 'function' && d.set === undefined \
         && d.enumerable === false && d.configurable === true",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 访问器 getter 函数对象元数据钉：name 标签与 length。
#[test]
fn ab_bytelength_getter_name_and_length() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength'); \
         d.get.name === 'get byteLength' && d.get.length === 0",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 访问器返回值钉：构造长度经原型访问器读回。
#[test]
fn ab_bytelength_returns_constructed_len() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "new ArrayBuffer(0).byteLength === 0 && new ArrayBuffer(42).byteLength === 42").unwrap();
    assert!(result.as_bool());
}

/// receiver 校验钉：非 ArrayBuffer this（原型自身 / undefined）抛 TypeError。
#[test]
fn ab_bytelength_this_checks() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(ArrayBuffer.prototype, 'byteLength'); \
         var proto_threw = false; var undef_threw = false; \
         try { ArrayBuffer.prototype.byteLength } catch (e) { proto_threw = e instanceof TypeError; } \
         try { d.get.call(undefined) } catch (e) { undef_threw = e instanceof TypeError; } \
         proto_threw && undef_threw",
    )
    .unwrap();
    assert!(result.as_bool());
}

/// 实例无 own byteLength 属性钉（构造器不写数据属性）。
#[test]
fn ab_instance_has_no_own_bytelength() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Object.getOwnPropertyNames(new ArrayBuffer(8)).length === 0").unwrap();
    assert!(result.as_bool());
}
