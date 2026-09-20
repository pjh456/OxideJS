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

fn to_str(vm: &Vm, val: JsValue) -> String {
    vm.lookup_str(val).unwrap_or_default()
}

#[test]
fn reflect_global_and_methods_exist() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "typeof Reflect === 'object' && typeof Reflect.get === 'function' && typeof Reflect.construct === 'function'",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn reflect_get_set_has_delete_property() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var obj = {a: 1, b: 2}; \
         var getOk = Reflect.get(obj, 'a') === 1; \
         var setOk = Reflect.set(obj, 'c', 3) === true && obj.c === 3; \
         var hasOk = Reflect.has(obj, 'a') === true && Reflect.has(obj, 'z') === false; \
         var delOk = Reflect.deleteProperty(obj, 'a') === true && Reflect.has(obj, 'a') === false; \
         getOk && setOk && hasOk && delOk",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn reflect_own_keys_returns_own_property_names() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var obj = {a: 1, b: 2}; Reflect.set(obj, 'c', 3); Reflect.ownKeys(obj).join(',')",
    )
    .unwrap();
    assert_eq!(to_str(&vm, result), "a,b,c");
}

#[test]
fn reflect_get_own_property_descriptor_returns_descriptor() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var obj = {}; Reflect.defineProperty(obj, 'x', { value: 9, writable: true, enumerable: true, configurable: true }); \
         var d = Reflect.getOwnPropertyDescriptor(obj, 'x'); d.value === 9 && d.writable === true",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn reflect_prototype_and_extensible_methods() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var proto = {p: 1}; var obj = {}; \
         var setProto = Reflect.setPrototypeOf(obj, proto); \
         var getProto = Reflect.getPrototypeOf(obj) === proto; \
         var ext1 = Reflect.isExtensible(obj); \
         var prevent = Reflect.preventExtensions(obj); \
         var ext2 = Reflect.isExtensible(obj); \
         setProto && getProto && ext1 === true && prevent === true && ext2 === false",
    )
    .unwrap();
    assert!(result.as_bool());
}

// 不可扩展目标：新旧不同返回 false，新旧相同返回 true，且原值不被改写。
#[test]
fn reflect_set_prototype_of_non_extensible_returns_false() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var a = {}; Object.preventExtensions(a); \
         var diff = Reflect.setPrototypeOf(a, null); \
         var kept = Object.getPrototypeOf(a) === Object.prototype; \
         var same = Reflect.setPrototypeOf(a, Object.prototype); \
         diff === false && kept && same === true",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn reflect_apply_calls_function() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "function add(a, b) { return this.base + a + b; } var receiver = {base: 10}; var args = [1, 2]; Reflect.apply(add, receiver, args)",
    )
      .unwrap();
    assert_eq!(result.as_int(), 13, "this.base 10 + 1 + 2 = 13");
}

#[test]
fn reflect_construct_returns_new_instance() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var o = Reflect.construct(function(){ this.x = 5; }, []); typeof o === 'object' && o.x === 5",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn reflect_construct_non_callable_target_throws_type_error() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "try { Reflect.construct(123, []) } catch (e) { e instanceof TypeError }").unwrap();
    assert!(result.as_bool());
}

#[test]
fn reflect_non_object_target_throws_type_error() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "try { Reflect.get(1, 'x') } catch (e) { e instanceof TypeError }").unwrap();
    assert!(result.as_bool());
}

// 派生类（extends Array）经 Reflect.construct 产出真实例：isArray 与 length 就位。
#[test]
fn reflect_construct_derived_class_instance() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class B extends Array { constructor(x) { super(x); } } \
         var inst = Reflect.construct(B, [2]); \
         inst instanceof B && Array.isArray(inst) && inst.length === 2",
    )
    .unwrap();
    assert!(result.as_bool());
}

// 派生类经 Reflect.construct 正常走 super 链构造，实例值判别。
#[test]
fn reflect_construct_derived_extends_base_class() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class Base { constructor(x) { this.x = x; } } \
         class D extends Base { constructor(x) { super(x); } } \
         var inst = Reflect.construct(D, [9]); \
         inst instanceof D && inst.x === 9",
    )
    .unwrap();
    assert!(result.as_bool());
}

// 无 newTarget 时构造帧内 new.target = target。
#[test]
fn reflect_construct_new_target_defaults_to_target() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var newTarget = null; function f() { newTarget = new.target; } \
         Reflect.construct(f, []); newTarget === f",
    )
    .unwrap();
    assert!(result.as_bool());
}

// newTarget 覆写臂：构造帧内 new.target = newTarget，实例 proto 取 newTarget.prototype。
#[test]
fn reflect_construct_new_target_override() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var newTarget = null; function f() { newTarget = new.target; } \
         var NT = function() {}; \
         Reflect.construct(f, [], NT); \
         newTarget === NT && Object.getPrototypeOf(Reflect.construct(f, [], NT)) === NT.prototype",
    )
    .unwrap();
    assert!(result.as_bool());
}

// 派生类 + newTarget 覆写全形：super 实参逐位、new.target = NT、this = 实例。
#[test]
fn reflect_construct_derived_with_new_target() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var expectedNewTarget = function() {}; \
         var thisValue, instance, args, actualNewTarget; \
         function Parent() { thisValue = this; args = arguments; actualNewTarget = new.target; } \
         class Child extends Parent { constructor() { super(1, 2, 3); } } \
         instance = Reflect.construct(Child, [4, 5, 6], expectedNewTarget); \
         thisValue === instance && args.length === 3 && args[0] === 1 && args[1] === 2 && args[2] === 3 \
         && actualNewTarget === expectedNewTarget \
         && Object.getPrototypeOf(instance) === expectedNewTarget.prototype",
    )
    .unwrap();
    assert!(result.as_bool());
}

// 基类 + newTarget 覆写：实例 proto 取 NT.prototype，new.target = NT。
#[test]
fn reflect_construct_base_class_with_new_target() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class CBase { constructor(x) { this.x = x; this.nt = new.target; } } \
         var NT = function() {}; \
         var inst = Reflect.construct(CBase, [5], NT); \
         Object.getPrototypeOf(inst) === NT.prototype && inst.nt === NT && inst.x === 5",
    )
    .unwrap();
    assert!(result.as_bool());
}

// 入参面：target 非构造器（Date.now / 箭头 / 基元）均 TypeError。
#[test]
fn reflect_construct_non_constructor_target_throws() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var t1 = false, t2 = false, t3 = false; \
         try { Reflect.construct(Date.now, [], Array); } catch (e) { t1 = e instanceof TypeError; } \
         try { Reflect.construct((a) => a, []); } catch (e) { t2 = e instanceof TypeError; } \
         try { Reflect.construct(1, []); } catch (e) { t3 = e instanceof TypeError; } \
         t1 && t2 && t3",
    )
    .unwrap();
    assert!(result.as_bool());
}

// 入参面：newTarget 非构造器与 argumentsList 非对象均 TypeError。
#[test]
fn reflect_construct_invalid_new_target_or_arguments_list_throws() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var t1 = false, t2 = false; \
         try { Reflect.construct(function(){}, [], 1); } catch (e) { t1 = e instanceof TypeError; } \
         try { Reflect.construct(function(){}, 1); } catch (e) { t2 = e instanceof TypeError; } \
         t1 && t2",
    )
    .unwrap();
    assert!(result.as_bool());
}

// length 访问器抛出的异常原值传播（不降级、不吞没）。
#[test]
fn reflect_construct_length_getter_exception_propagates() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var o = {}; \
         Object.defineProperty(o, 'length', { get: function() { throw new RangeError('len'); } }); \
         try { Reflect.construct(function(a){ return a; }, o); false } catch (e) { e instanceof RangeError }",
    )
    .unwrap();
    assert!(result.as_bool());
}

// array-like 对象按规范 Get 逐位取值（非位序直读），值与实参个数就位。
#[test]
fn reflect_construct_array_like_elements_read_by_get() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var captured; \
         function C(a, b) { captured = [typeof a, arguments.length]; } \
         Reflect.construct(C, { length: 2, 0: 'a', 1: 'b' }); \
         captured[0] === 'string' && captured[1] === 2",
    )
    .unwrap();
    assert!(result.as_bool());
}

// 守卫面：native 构造器（Array 填槽）、构造帧 arguments 完整、显式返回对象
// 优先、绑定元数据（isConstructor(Reflect.construct) === false）。
#[test]
fn reflect_construct_guard_face() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var arr = Reflect.construct(Array, [3]); \
         function fn() { this.args = arguments; } \
         var result = Reflect.construct(fn, [42, 'Mike', 'Leo']); \
         var retObj = Reflect.construct(function() { return { z: 9 }; }, []); \
         var meta = false; \
         try { Reflect.construct(Reflect.construct, []); } catch (e) { meta = e instanceof TypeError; } \
         arr instanceof Array && arr.length === 3 \
         && result.args.length === 3 && result.args[0] === 42 && result.args[1] === 'Mike' && result.args[2] === 'Leo' \
         && retObj.z === 9 && !(retObj instanceof Function) && meta",
    )
    .unwrap();
    assert!(result.as_bool());
}

// 227.1 回归守卫：隐式 super 面经 `new` 直接构造不回归。
#[test]
fn construct_direct_new_derived_array_guard() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class B extends Array { constructor(x) { super(x); } } \
         var sub = new B(2); \
         class Sub extends Array { constructor(x) { super(x); } } \
         var s7 = new Sub(7); \
         sub instanceof B && sub.length === 2 && s7.length === 7",
    )
    .unwrap();
    assert!(result.as_bool());
}
