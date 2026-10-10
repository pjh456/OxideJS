//! super 属性读运行期面：对象字面量方法门、home object 挂接、嵌套箭头继承、
//! null-proto 抛 TypeError、GetSuperBase 先于 ToPropertyKey、super 调用 IsConstructor
//! 后移、派生构造器 this 未初始化键求值面。

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

// 对象方法 super.x 读：proto 链取值，receiver 为方法接收者。
#[test]
fn object_method_super_read_walks_proto_chain() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var A = { fromA: 'a', fromB: 'a' }; var B = { fromB: 'b' }; \
         Object.setPrototypeOf(B, A); \
         var obj = { fromA: 'c', fromB: 'c', method() { return super.fromA + '|' + super.fromB; } }; \
         Object.setPrototypeOf(obj, B); obj.method()",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "a|b");
}

// 对象方法内嵌套箭头继承 super 上下文：home object 从外层方法继承。
#[test]
fn object_method_nested_arrow_inherits_super() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var A = { fromA: 'a', fromB: 'a' }; var B = { fromB: 'b' }; \
         Object.setPrototypeOf(B, A); \
         var obj = { method() { return (() => super.fromA + super.fromB)(); } }; \
         Object.setPrototypeOf(obj, B); obj.method()",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "ab");
}

// home proto 为 null 时 super 读抛 TypeError（RequireObjectCoercible 面）。
#[test]
fn object_method_super_read_null_proto_throws_type_error() {
    let mut vm = Vm::new();
    let err = eval(
        &mut vm,
        "var obj = { method() { return super.x; } }; \
         Object.setPrototypeOf(obj, null); \
         try { obj.method(); } catch (e) { e.name; }",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(err).unwrap(), "TypeError");
}

// 键对象 toString 副作用换 proto：GetSuperBase 先于 ToPropertyKey，读旧 proto 值。
#[test]
fn object_method_super_read_getsuperbase_before_topropertykey() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var proto = { p: 'ok' }; var proto2 = { p: 'bad' }; \
         var obj = { __proto__: proto, m() { return super[key]; } }; \
         var key = { toString() { Object.setPrototypeOf(obj, proto2); return 'p'; } }; \
         obj.m()",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "ok");
}

// super 调用 IsConstructor 检查在实参物化之后：非构造器 proto 时实参副作用先执行。
#[test]
fn super_call_evaluates_args_before_is_constructor() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var evaluatedArg = false; var caught; \
         class C extends Object { constructor() { \
           try { super(evaluatedArg = true); } catch (err) { caught = err; } \
         } } \
         Object.setPrototypeOf(C, parseInt); \
         try { new C(); } catch (_) {} \
         evaluatedArg + '|' + (caught ? caught.name : 'none')",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "true|TypeError");
}

// 派生构造器 this 未初始化时 super[键]：GetThisBinding 先于键求值，抛 ReferenceError。
#[test]
fn derived_ctor_super_computed_uninitialized_this_throws_reference_error() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class Base { constructor() { throw new Error('base constructor'); } } \
         class Derived extends Base { constructor() { return super[super()]; } } \
         try { new Derived(); } catch (e) { e.name; }",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "ReferenceError");
}

// 对象 getter 内 super.x（Get/Set 臂 home object 面）。
#[test]
fn object_getter_super_read_uses_home_object() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var A = { v: 'a' }; var B = { v: 'b' }; Object.setPrototypeOf(B, A); \
         var obj = { get g() { return super.v; } }; \
         Object.setPrototypeOf(obj, B); obj.g",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "b");
}

// 对象 setter 内 super.x（Set 臂 home object 面）。
#[test]
fn object_setter_super_read_uses_home_object() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var A = { v: 'a' }; var B = { v: 'b' }; Object.setPrototypeOf(B, A); \
         var seen; var obj = { set s(x) { seen = super.v; } }; \
         Object.setPrototypeOf(obj, B); obj.s = 1; seen",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "b");
}

// 类表达式值不挂外层对象作 home：其方法 super 读以类自身 proto 为基。
#[test]
fn class_expression_value_not_homed_to_outer_object() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "function Base() {} Base.prototype.q = 'base'; \
         var C = class extends Base { m() { return super.q; } }; \
         var o = { c: C }; new C().m()",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "base");
}

// 对象方法 super 调用：callee 为 proto 链上的方法，receiver 为方法接收者。
#[test]
fn object_method_super_call_walks_proto_chain() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var A = { f() { return 'A:' + this.tag; } }; var B = { f() { return 'B:' + this.tag; } }; \
         Object.setPrototypeOf(B, A); \
         var obj = { tag: 't', m() { return super.f(); } }; \
         Object.setPrototypeOf(obj, B); obj.m()",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "B:t");
}

// 类方法 super 计算成员读：this 未初始化（派生构造器）时抛 ReferenceError。
#[test]
fn class_method_super_computed_uninitialized_this_throws() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class Base { constructor() { throw new Error('base'); } } \
         class Derived extends Base { constructor() { return super['x']; } } \
         try { new Derived(); } catch (e) { e.name; }",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "ReferenceError");
}

// super 调用：native 构造器 proto 正常路径（回归面，IsConstructor 检查后移不破坏）。
#[test]
fn super_call_native_constructor_still_works() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C extends Object { constructor(n) { super(n); } } \
         new C(5) instanceof Object",
    )
    .unwrap();
    assert!(result.is_bool() && result.as_bool(), "super() 后实例应为 Object 实例");
}

// 对象方法 super.x 写（静态成员）：写经 proto 链落基（super base），receiver 为方法接收者。
#[test]
fn object_method_super_put_static_member() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var base = { x: 'a' }; \
         var obj = { __proto__: base, method() { super.x = 'written'; return base.x; } }; \
         obj.method()",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "written");
}

// 对象方法 super['x'] 写（计算成员）：键运行期求值，写经 proto 链落基。
#[test]
fn object_method_super_put_computed_member() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var base = { x: 'a' }; \
         var obj = { __proto__: base, method() { super['x'] = 'written'; return base.x; } }; \
         obj.method()",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "written");
}

// 严格模式 super 写失败（基对象冻结）抛 TypeError。
#[test]
fn object_method_super_put_strict_frozen_throws_type_error() {
    let mut vm = Vm::new();
    let err = eval(
        &mut vm,
        "'use strict'; \
         var base = { y: 0 }; Object.freeze(base); \
         var obj = { __proto__: base, method() { super.y = 9; } }; \
         try { obj.method(); } catch (e) { e.name; }",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(err).unwrap(), "TypeError");
}

// 非严格模式 super 写失败（基对象冻结）静默跳过，不抛错。
#[test]
fn object_method_super_put_non_strict_frozen_silent() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var base = { y: 0 }; Object.freeze(base); \
         var obj = { __proto__: base, method() { super.y = 9; return 'done'; } }; \
         obj.method()",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "done");
}

// 键对象 toString 副作用换 proto：GetSuperBase 先于 ToPropertyKey，写旧 proto。
#[test]
fn object_method_super_put_getsuperbase_before_topropertykey() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var proto = { p: 1 }; var proto2 = { p: -1 }; \
         var obj = { __proto__: proto, m() { super[key] = 10; } }; \
         var key = { toString() { Object.setPrototypeOf(obj, proto2); return 'p'; } }; \
         obj.m(); proto.p + '|' + proto2.p",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "10|-1");
}

// 复合赋值 GetSuperBase 先于 ToPropertyKey：读旧值、运算、写回均用旧 proto 基。
#[test]
fn object_method_super_put_compound_getsuperbase_before_topropertykey() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var proto = { p: 1 }; var proto2 = { p: -1 }; \
         var obj = { __proto__: proto, m() { return super[key] += 1; } }; \
         var key = { toString() { Object.setPrototypeOf(obj, proto2); return 'p'; } }; \
         String(obj.m())",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "2");
}

// 自增 GetSuperBase 先于 ToPropertyKey：前缀自增返回新值。
#[test]
fn object_method_super_put_increment_getsuperbase_before_topropertykey() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var proto = { p: 1 }; var proto2 = { p: -1 }; \
         var obj = { __proto__: proto, m() { return ++super[key]; } }; \
         var key = { toString() { Object.setPrototypeOf(obj, proto2); return 'p'; } }; \
         String(obj.m())",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "2");
}

// 派生构造器 this 未初始化时 super[键] = 值：GetThisBinding 先于键求值，抛 ReferenceError。
#[test]
fn derived_ctor_super_put_uninitialized_this_throws_reference_error() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class Base { constructor() { throw new Error('base constructor'); } } \
         class Derived extends Base { constructor() { super[super()] = 0; } } \
         try { new Derived(); } catch (e) { e.name; }",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "ReferenceError");
}

// 派生构造器 this 未初始化时 super[键] += 值：GetThisBinding 先于键求值，抛 ReferenceError。
#[test]
fn derived_ctor_super_put_compound_uninitialized_this_throws_reference_error() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class Base { constructor() { throw new Error('base constructor'); } } \
         class Derived extends Base { constructor() { super[super()] += 0; } } \
         try { new Derived(); } catch (e) { e.name; }",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "ReferenceError");
}

// 派生构造器 this 未初始化时 ++super[键]：GetThisBinding 先于键求值，抛 ReferenceError。
#[test]
fn derived_ctor_super_put_increment_uninitialized_this_throws_reference_error() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class Base { constructor() { throw new Error('base constructor'); } } \
         class Derived extends Base { constructor() { ++super[super()]; } } \
         try { new Derived(); } catch (e) { e.name; }",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "ReferenceError");
}

// home proto 为 null 时 super 写抛 TypeError（RequireObjectCoercible 面）。
#[test]
fn object_method_super_put_null_proto_throws_type_error() {
    let mut vm = Vm::new();
    let err = eval(
        &mut vm,
        "var obj = { method() { super.x = 1; } }; \
         Object.setPrototypeOf(obj, null); \
         try { obj.method(); } catch (e) { e.name; }",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(err).unwrap(), "TypeError");
}
