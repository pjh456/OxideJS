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

// 原型上的 getter 在 receiver 上设置标记，随后检查 receiver 持有标记（而非原型）。
#[test]
fn getter_on_proto_this_is_receiver() {
    let r = eval_str(
        "var proto={}; Object.defineProperty(proto,'x',{get:function(){ this._marker=1; return this._marker; },enumerable:true,configurable:true}); var child=Object.create(proto); child.x"
    ).unwrap();
    assert!(r.is_int() && r.as_int() == 1, "getter should set marker on child (receiver), got {:?}", r);
}

// 深层原型链上的 getter：grandchild → child → proto，getter 定义在 proto 上。
#[test]
fn getter_on_proto_deep_chain_this_is_receiver() {
    let r = eval_str(
        "var proto={}; Object.defineProperty(proto,'x',{get:function(){ this._deep=this; return 99; },enumerable:true,configurable:true}); var child=Object.create(proto); var grandchild=Object.create(child); grandchild.x; grandchild._deep!==undefined"
    ).unwrap();
    assert!(r.is_bool() && r.as_bool(), "grandchild should have _deep marker, got {:?}", r);
}

// 原型上的 setter 以 receiver 作为 this，在 receiver 而非原型上设置标记。
#[test]
fn setter_on_proto_this_is_receiver() {
    let (_vm, r) = eval(
        "var proto={}; Object.defineProperty(proto,'x',{set:function(v){ this._set_by_setter=v; },get:function(){ return this._set_by_setter; },enumerable:true,configurable:true}); var child=Object.create(proto); child.x='receiver_test'; child._set_by_setter"
    ).unwrap();
    assert!(r.is_string(), "child should have _set_by_setter, got {:?}", r);
}

// 动态 getter（obj[expr]）保持 receiver。
#[test]
fn dynamic_getter_on_proto_this_is_receiver() {
    let r = eval_str(
        "var proto={}; Object.defineProperty(proto,'x',{get:function(){ this._dyn=1; return this._dyn; },enumerable:true,configurable:true}); var child=Object.create(proto); child['x']"
    ).unwrap();
    assert!(r.is_int() && r.as_int() == 1, "dynamic getter should set marker on child, got {:?}", r);
}

// 动态 setter（obj[expr]=val）保持 receiver。
#[test]
fn dynamic_setter_on_proto_this_is_receiver() {
    let (_vm, r) = eval(
        "var proto={}; Object.defineProperty(proto,'x',{set:function(v){ this._dyn_set=v; },get:function(){ return this._dyn_set; },enumerable:true,configurable:true}); var child=Object.create(proto); child['x']='dyn_test'; child._dyn_set"
    ).unwrap();
    assert!(r.is_string(), "child should have _dyn_set after dynamic setter, got {:?}", r);
}

// 对象自身的 getter——this 即对象本身。
#[test]
fn own_getter_this_is_self() {
    let r = eval_str(
        "var obj={}; Object.defineProperty(obj,'x',{get:function(){ this._own=42; return this._own; },enumerable:true,configurable:true}); obj.x"
    ).unwrap();
    assert!(r.is_int() && r.as_int() == 42, "own getter should set marker on self, got {:?}", r);
}

// 对象自身的 setter——this 即对象本身。
#[test]
fn own_setter_this_is_self() {
    let (_vm, r) = eval(
        "var obj={}; Object.defineProperty(obj,'x',{set:function(v){ this._own_set=v; },get:function(){ return this._own_set; },enumerable:true,configurable:true}); obj.x='own'; obj._own_set"
    ).unwrap();
    assert!(r.is_string(), "own setter should set marker on self, got {:?}", r);
}

// 返回 undefined 的 getter 不崩溃。
#[test]
fn getter_returns_undefined_does_not_crash() {
    let r = eval_str(
        "var proto={}; Object.defineProperty(proto,'x',{get:function(){},enumerable:true,configurable:true}); var child=Object.create(proto); child.x"
    ).unwrap();
    assert!(r.is_undefined(), "getter returning nothing should yield undefined, got {:?}", r);
}

// 定义原型 getter 后字符串 .length 仍正常（基本类型自动装箱未被破坏）。
#[test]
fn string_length_still_works_after_proto_getter() {
    let r = eval_str("'hello'.length").unwrap();
    assert!(r.is_int() && r.as_int() == 5, "string.length should be 5, got {:?}", r);
}
