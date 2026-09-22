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

/// 七构造器 `Symbol.species` 静态 getter 的规范描述符面：
/// `{ get, set:undefined, enumerable:false, configurable:true }`、`get.length === 0`、
/// `get.name === "get [Symbol.species]"`，getter 返回 receiver。
fn assert_species_accessor(ctor_name: &str) {
    let mut vm = Vm::new();
    let source = format!(
        "var d = Object.getOwnPropertyDescriptor({ctor_name}, Symbol.species); \
         d !== undefined && d.set === undefined && d.enumerable === false && d.configurable === true && \
         typeof d.get === 'function' && d.get.length === 0 && d.get.name === 'get [Symbol.species]' && \
         d.get.call({ctor_name}) === {ctor_name} && d.get.call('x') === 'x'"
    );
    let result = eval(&mut vm, &source).unwrap();
    assert!(result.as_bool(), "{ctor_name} species accessor descriptor mismatch");
}

#[test]
fn array_buffer_species_accessor_descriptor() {
    assert_species_accessor("ArrayBuffer");
}

#[test]
fn map_species_accessor_descriptor() {
    assert_species_accessor("Map");
}

#[test]
fn set_species_accessor_descriptor() {
    assert_species_accessor("Set");
}

#[test]
fn regexp_species_accessor_descriptor() {
    assert_species_accessor("RegExp");
}

#[test]
fn promise_species_accessor_descriptor() {
    assert_species_accessor("Promise");
}

#[test]
fn typed_array_abstract_species_descriptor_and_chain_read() {
    // 抽象构造器 own 访问器；具体构造器无 own（gOPD 为 undefined），链上读回自身。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var d = Object.getOwnPropertyDescriptor(TypedArray, Symbol.species); \
         d !== undefined && d.set === undefined && d.enumerable === false && d.configurable === true && \
         d.get.name === 'get [Symbol.species]' && d.get.length === 0 && \
         TypedArray[Symbol.species] === TypedArray && \
         Object.getOwnPropertyDescriptor(Uint8Array, Symbol.species) === undefined && \
         Uint8Array[Symbol.species] === Uint8Array && \
         Object.getOwnPropertyDescriptor(BigUint64Array, Symbol.species) === undefined && \
         BigUint64Array[Symbol.species] === BigUint64Array",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn species_subclasses_resolve_own_constructor() {
    // 派生类无 own @@species，沿静态原型链命中基类同一 getter，receiver 为自身。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class SubMap extends Map {} \
         class SubSet extends Set {} \
         class SubBuf extends ArrayBuffer {} \
         class SubRe extends RegExp {} \
         class SubP extends Promise {} \
         SubMap[Symbol.species] === SubMap && SubSet[Symbol.species] === SubSet && \
         SubBuf[Symbol.species] === SubBuf && SubRe[Symbol.species] === SubRe && SubP[Symbol.species] === SubP && \
         Object.getOwnPropertyDescriptor(SubMap, Symbol.species) === undefined",
    )
    .unwrap();
    assert!(result.as_bool());
}
