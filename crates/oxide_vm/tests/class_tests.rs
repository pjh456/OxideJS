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

fn assert_num(value: JsValue, expected: f64) {
    let actual = if value.is_int() { value.as_int() as f64 } else { value.as_double() };
    assert!((actual - expected).abs() < 0.0001, "expected {expected}, got {actual}");
}

#[test]
fn class_declaration_is_function_value() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A {} ; typeof A").unwrap();
    let ty = vm.lookup_str(result).expect("typeof result should be string");
    assert_eq!(ty, "function");
}

#[test]
fn class_method_is_on_prototype() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { m() { return 1; } } new A().m()").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn class_instance_getter_returns_value() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { get x() { return 2; } } new A().x").unwrap();
    assert_eq!(result.as_int(), 2);
}

#[test]
fn class_instance_setter_updates_receiver() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { set x(v) { this.y = v; } } var a = new A(); a.x = 4; a.y").unwrap();
    assert_eq!(result.as_int(), 4);
}

#[test]
fn class_static_getter_returns_value() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { static get x() { return 5; } } A.x").unwrap();
    assert_eq!(result.as_int(), 5);
}

#[test]
fn class_constructor_initializes_instance_state() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { constructor(x) { this.x = x; } } new A(3).x").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn class_expression_constructs_instances() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "const C = class Foo { method() { return 2; } }; new C().method()").unwrap();
    assert_eq!(result.as_int(), 2);
}

#[test]
fn class_expression_inner_name_is_visible_in_method_body() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "const C = class Foo { self() { return Foo; } }; new C().self() === C").unwrap();
    assert!(
        result.is_bool() && result.as_bool(),
        "expected inner class name to resolve to constructor"
    );
}

#[test]
fn class_constructor_cannot_be_called_without_new() {
    let mut vm = Vm::new();
    let err = eval(&mut vm, "class A {} ; A()").unwrap_err();
    assert!(err.contains("TypeError"), "expected TypeError, got: {err}");
}

#[test]
fn class_constructor_object_return_overrides_instance() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { constructor() { return { x: 1 }; } } new A().x").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn class_constructor_primitive_return_preserves_instance() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { constructor() { this.x = 1; return 5; } } new A().x").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn derived_constructor_super_initializes_parent_state() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class A { constructor(){ this.x = 1 } } class B extends A { constructor(){ super(); this.y = 2 } } let b = new B(); b.x + b.y",
    )
    .unwrap();
    assert_num(result, 3.0);
}

#[test]
fn derived_default_constructor_delegates_to_parent() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { constructor(){ this.x = 1 } } class B extends A {} new B().x").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn derived_constructor_this_before_super_throws_reference_error() {
    let mut vm = Vm::new();
    let err = eval(
        &mut vm,
        "class A { constructor(){ this.x = 1 } } class B extends A { constructor(){ this.x = 2; super(); } } new B()",
    )
    .unwrap_err();
    assert!(err.contains("ReferenceError"), "expected ReferenceError, got: {err}");
}

#[test]
fn three_level_inheritance_super_chain_constructs_normally() {
    // 多层继承（≥3 层）：中间 derived 帧由 SUPER_CALL 压入，合法 super() 不得误报
    // more-than-once，构造链须逐层完成。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class A { constructor(){ this.v = 1; } } class B extends A { constructor(){ super(); this.w = 2; } } class C extends B { constructor(){ super(); this.u = 3; } } var c = new C(); c.v + c.w + c.u",
    )
    .unwrap();
    assert_num(result, 6.0);
}

#[test]
fn intermediate_derived_returning_without_super_throws() {
    // 中间 derived 构造器未调 super 直接 return：须抛 ReferenceError（此前
    // regs[254] 值判定因 this 为外层构造对象而漏检）。
    let mut vm = Vm::new();
    let err = eval(
        &mut vm,
        "class A{} class B extends A{ constructor(){ return; } } class C extends B{} new C()",
    )
    .unwrap_err();
    assert!(err.contains("ReferenceError"), "expected ReferenceError, got: {err}");
}

#[test]
fn derived_returning_without_super_throws_reference_error() {
    let mut vm = Vm::new();
    let err = eval(&mut vm, "class A{} class B extends A{ constructor(){ return; } } new B()").unwrap_err();
    assert!(err.contains("ReferenceError"), "expected ReferenceError, got: {err}");
}

#[test]
fn intermediate_derived_this_before_super_throws() {
    // 中间 derived 帧由 SUPER_CALL 压入后 super() 前读 this：须抛 ReferenceError。
    let mut vm = Vm::new();
    let err = eval(
        &mut vm,
        "class A{} class B extends A{ constructor(){ this.x = 1; super(); } } class C extends B{ constructor(){ super(); } } new C()",
    )
    .unwrap_err();
    assert!(err.contains("ReferenceError"), "expected ReferenceError, got: {err}");
}

#[test]
fn intermediate_derived_double_super_throws() {
    // 中间 derived 帧二次调用 super()：须抛 more-than-once（字节码父构造器返回后
    // 置位 super_called 生效）。
    let mut vm = Vm::new();
    let err = eval(
        &mut vm,
        "class A{} class B extends A{ constructor(){ super(); super(); } } class C extends B{} new C()",
    )
    .unwrap_err();
    assert!(err.contains("ReferenceError"), "expected ReferenceError, got: {err}");
}

#[test]
fn derived_constructor_returning_primitive_without_super_throws() {
    // 规范 §9.2.2.2：derived 返回非对象值（42/null）且未调 super → ReferenceError，
    // 而非静默回退构造 this。
    let mut vm = Vm::new();
    let err = eval(&mut vm, "class A{} class B extends A{ constructor(){ return 42; } } new B()").unwrap_err();
    assert!(err.contains("ReferenceError"), "expected ReferenceError, got: {err}");
}

#[test]
fn derived_constructor_returning_null_without_super_throws() {
    let mut vm = Vm::new();
    let err = eval(&mut vm, "class A{} class B extends A{ constructor(){ return null; } } new B()").unwrap_err();
    assert!(err.contains("ReferenceError"), "expected ReferenceError, got: {err}");
}

#[test]
fn derived_constructor_returning_object_overrides_instance() {
    // derived 返回对象值：直接作为构造结果，不校验 super。
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A{} class B extends A{ constructor(){ return { x: 1 }; } } new B().x").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn derived_constructor_primitive_return_after_super_preserves_this() {
    // derived 调过 super 后返回原始值：回退构造 this，实例字段保留。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class A{ constructor(){ this.v = 5; } } class B extends A{ constructor(){ super(); return 42; } } new B().v",
    )
    .unwrap();
    assert_num(result, 5.0);
}

#[test]
fn derived_method_super_call_uses_current_receiver() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class A { m(){ return this.x } } class B extends A { constructor(){ super(); this.x = 2 } m(){ return super.m() + 1 } } new B().m()",
    )
    .unwrap();
    assert_num(result, 3.0);
}

#[test]
fn static_method_is_callable_on_constructor() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { static m(){ return 1 } } A.m()").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn static_and_instance_methods_do_not_collide() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class A { m(){ return 1 } static m(){ return 2 } } let a = new A(); a.m() + A.m()",
    )
    .unwrap();
    assert_num(result, 3.0);
}

#[test]
fn derived_static_method_super_call_resolves_parent_constructor() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class A { static m(){ return 1 } } class B extends A { static n(){ return super.m() + 1 } } B.n()",
    )
    .unwrap();
    assert_num(result, 2.0);
}

#[test]
fn inherited_static_method_is_found_on_constructor_chain() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { static m(){ return 4 } } class B extends A {} B.m()").unwrap();
    assert_eq!(result.as_int(), 4);
}

#[test]
fn public_class_field_initializer_sets_own_property() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class C { x = 1; } new C().x").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn public_class_field_without_initializer_is_undefined() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class C { x; } typeof new C().x").unwrap();
    let ty = vm.lookup_str(result).expect("typeof result should be string");
    assert_eq!(ty, "undefined");
}

#[test]
fn public_class_fields_run_before_base_constructor_body() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class C { x = this.y; constructor(){ this.y = 2; } } typeof new C().x").unwrap();
    let ty = vm.lookup_str(result).expect("typeof result should be string");
    assert_eq!(ty, "undefined");
}

#[test]
fn public_class_fields_run_after_super_in_derived_constructor() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class B { constructor(){ this.b = 1; } } class D extends B { x = this.b; } new D().x",
    )
    .unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn static_class_field_sets_constructor_property() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class C { static x = 1; } C.x").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn static_class_block_binds_this_to_constructor() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { static x = 1; static { this.y = this.x + 1; } static z = this.y + 1; } C.z",
    )
    .unwrap();
    assert_num(result, 3.0);
}

#[test]
fn computed_public_class_method_key_is_supported() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var k = 'm'; class C { [k](){ return 3; } } new C().m()").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn computed_public_class_field_key_is_supported() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var k = 'x'; class C { [k] = 5; } new C().x").unwrap();
    assert_eq!(result.as_int(), 5);
}

#[test]
fn computed_static_class_field_key_is_supported() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var k = 'x'; class C { static [k] = 7; } C.x").unwrap();
    assert_eq!(result.as_int(), 7);
}

#[test]
fn private_class_field_read_returns_value() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class C { #x = 1; get(){ return this.#x; } } new C().get()").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn private_class_field_write_round_trips() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { #x; set(v){ this.#x = v; } get(){ return this.#x; } } var c = new C(); c.set(4); c.get()",
    )
    .unwrap();
    assert_eq!(result.as_int(), 4);
}

#[test]
fn private_class_method_call_uses_receiver() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class C { #m(){ return 4; } get(){ return this.#m(); } } new C().get()").unwrap();
    assert_eq!(result.as_int(), 4);
}

#[test]
fn private_class_brand_in_checks_presence() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { #x = 1; has(o){ return #x in o; } } var c = new C(); c.has(c) && !c.has({})",
    )
    .unwrap();
    assert!(result.is_bool() && result.as_bool());
}

#[test]
fn private_class_missing_brand_throws_type_error() {
    let mut vm = Vm::new();
    let err = eval(&mut vm, "class C { #x = 1; get(o){ return o.#x; } } new C().get({})").unwrap_err();
    assert!(err.contains("TypeError"), "expected TypeError, got: {err}");
}

#[test]
fn private_static_field_and_method_work_on_constructor() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { static #x = 2; static #m(){ return this.#x + 3; } static get(){ return this.#m(); } } C.get()",
    )
    .unwrap();
    assert_num(result, 5.0);
}

#[test]
fn derived_private_field_initializes_after_super() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class B { constructor(){ this.b = 3; } } class D extends B { #x = this.b; get(){ return this.#x; } } new D().get()",
    )
    .unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn private_class_fields_are_hidden_from_reflection_and_string_access() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { #x = 1; y = 2; keys(){ return Object.keys(this).length + Object.getOwnPropertyNames(this).length + (this['#x'] === undefined ? 10 : 0); } } new C().keys()",
    )
    .unwrap();
    assert_num(result, 12.0);
}

#[test]
fn private_class_fields_are_hidden_from_for_in() {
    let mut vm = Vm::new();
    // 公有字段 y 可枚举（计数 1），私有 #x 不可见：若 #x 泄漏则计数为 2。
    let result = eval(
        &mut vm,
        "class C { #x = 1; y = 2; } var o = new C(); var n = 0; for (var k in o) { n = n + 1; } n",
    )
    .unwrap();
    assert_num(result, 1.0);
}

#[test]
fn private_class_same_name_different_classes_do_not_share_brand() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class A { #x = 1; has(o){ return #x in o; } } class B { #x = 2; } var a = new A(); a.has(a) && !a.has(new B())",
    )
    .unwrap();
    assert!(result.is_bool() && result.as_bool());
}

#[test]
fn this_survives_native_call_inside_method() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { v=7; m(){ Object.keys(this); Object.getOwnPropertyNames(this); return this.v; } } new C().m()",
    )
    .unwrap();
    assert_eq!(result.as_int(), 7);
}

#[test]
fn private_field_access_does_not_pierce_prototype() {
    // P4：Object.create(instance) 的对象访问 #x 抛 TypeError（字段不跨原型链）。
    let mut vm = Vm::new();
    let err = eval(&mut vm, "class C { #x = 1; m() { return this.#x; } } Object.create(new C()).m()").unwrap_err();
    assert!(err.contains("TypeError"), "expected TypeError, got: {err}");
}

#[test]
fn private_brand_in_does_not_pierce_prototype() {
    // P5：#x in Object.create(instance) 为 false（PrivateFieldIn 只查 own）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { #x = 1; check(o) { return #x in o; } } var inst = new C(); var o = Object.create(inst); new C().check(o) === false && new C().check(inst) === true",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn private_method_in_requires_own_brand() {
    // 方法场景：#m in Object.create(instance) 为 false（无 own brand）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { #m(){ return 1; } check(o) { return #m in o; } } var inst = new C(); new C().check(inst) === true && new C().check(Object.create(inst)) === false",
    )
    .unwrap();
    assert!(result.as_bool());
}

#[test]
fn private_field_read_write_after_two_statements() {
    // 回归：字段 set/get 跨语句边界正常（不依赖 brand cell 值比较）。
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { #x = 0; set(v){ this.#x = v; } get(){ return this.#x; } } var c = new C(); c.set(4); c.get()",
    )
    .unwrap();
    assert_eq!(result.as_int(), 4);
}

#[test]
fn computed_field_key_evaluated_once_at_class_definition() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var n=0; class C { [++n] = 1; [++n] = 2; } n").unwrap();
    assert_num(result, 2.0);
}

#[test]
fn computed_field_key_once_but_value_per_instance() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var n=0; class C { [++n] = ++n; } var a=new C(); var b=new C(); n*100 + a[1]*10 + b[1]",
    )
    .unwrap();
    assert_num(result, 323.0);
}

#[test]
fn computed_keys_evaluate_in_source_order_across_static_and_instance() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var i=0; class C { [i++] = i++; static [i++] = i++; [i++] = i++; } var c = new C(); i*1000 + c[0]*100 + c[2]*10 + C[1]",
    )
    .unwrap();
    assert_num(result, 6453.0);
}

#[test]
fn field_definition_does_not_trigger_inherited_setter() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var log=[]; class B { set x(v){ log.push('setter'); } } class C extends B { x = 1; } var c = new C(); log.length*100 + (c.hasOwnProperty('x')?10:0) + c.x",
    )
    .unwrap();
    assert_num(result, 11.0);
}

#[test]
fn field_value_captures_outer_variable() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var n=0; class C { x = ++n; } var c = new C(); n*10 + c.x").unwrap();
    assert_num(result, 11.0);
}

#[test]
fn class_expression_field_value_captures_outer_variable() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var n=0; var C = class { x = ++n; }; new C(); n").unwrap();
    assert_num(result, 1.0);
}

#[test]
fn field_initializers_run_once_on_second_super_call() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var n=0; class B {} var C = class extends B { field = ++n; constructor(){ super(); super(); } }; var err; try { new C(); } catch(e) { err = e.name; } (err === 'ReferenceError'?1:0)*10 + n",
    )
    .unwrap();
    assert_num(result, 11.0);
}

#[test]
fn postfix_increment_on_captured_cell_returns_old_value() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var n=0; var arr = (function(){ return [n++, n, n++]; })(); arr[0]*1000 + arr[1]*100 + arr[2]*10 + n",
    )
    .unwrap();
    assert_num(result, 112.0);
}

// ── class 生成器方法 ──

// 实例生成器方法：yield 顺序与 done 收敛，生成器对象经 next 驱动。
#[test]
fn class_generator_method_yields_in_order() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { *g() { yield 10; yield 20; } } var it = new C().g(); [it.next().value, it.next().value, it.next().done].join(',')",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "10,20,true");
}

// 静态生成器方法：挂在构造函数上，调用形态与实例方法一致。
#[test]
fn class_static_generator_method_yields() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class D { static *sg() { yield 30; } } var it = D.sg(); [it.next().value, it.next().done].join(',')",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "30,true");
}

// 生成器方法 throw 传播：throw 进挂起点，异常可被 body 内 catch 拦截。
#[test]
fn class_generator_method_throw_reaches_inner_catch() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class F { *t() { try { yield 1; } catch (e) { yield 'caught'; } } } var it = new F().t(); var r1 = it.next(); var r2 = it.throw(new Error('x')); [r1.value, r2.value].join(',')",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "1,caught");
}

// 生成器方法 next(arg)：参数作为 yield 表达式结果传入 body。
#[test]
fn class_generator_method_next_arg_feeds_yield_expression() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class G { *a() { var v = yield 1; yield v * 2; } } var it = new G().a(); var r1 = it.next(); var r2 = it.next(21); [r1.value, r2.value].join(',')",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "1,42");
}

// 生成器方法 yield* 委托：与函数式生成器一致的委托序列。
#[test]
fn class_generator_method_yield_star_delegates() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class H { *ys() { yield* [1, 2, 3]; } } Array.from(new H().ys()).join(',')").unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "1,2,3");
}

// 生成器方法 return()：提前关闭生成器并携带返回值。
#[test]
fn class_generator_method_return_closes_with_value() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class I { *rt() { yield 1; yield 2; } } var it = new I().rt(); var r1 = it.next(); var r2 = it.return(99); [r1.value, r2.value, r2.done].join(',')",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "1,99,true");
}

// ── 私有字段复合赋值（+= 等）与逻辑赋值（&&= ||= ??=）──

// data 字段复合赋值：旧值读取 → 运算 → 写回，表达式结果与后续读取一致。
#[test]
fn private_field_compound_assignment_writes_back() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { #x = 1; m(){ return this.#x += 2; } get(){ return this.#x; } } var c = new C(); c.m()*10 + c.get()",
    )
    .unwrap();
    assert_num(result, 33.0);
}

// 复合赋值先读旧值再求值 RHS：RHS 改写同一字段时仍以旧值参与运算。
#[test]
fn private_field_compound_assignment_reads_old_value_before_rhs() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { #x = 1; m(){ return this.#x += (this.#x = 5); } get(){ return this.#x; } } var c = new C(); c.m()*10 + c.get()",
    )
    .unwrap();
    // 旧值 1 先读，RHS 把 #x 写为 5，1+5=6 再写回 → 结果 6。
    assert_num(result, 66.0);
}

// 减法/乘法/除法复合赋值序列，验证三操作数二元运算路径。
#[test]
fn private_field_compound_assignment_arithmetic_operators() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { #x = 10; a(){ return this.#x -= 3; } b(){ return this.#x *= 2; } c(){ return this.#x /= 4; } } var c = new C(); c.a()*10000 + c.b()*100 + c.c()",
    )
    .unwrap();
    assert_num(result, 7.0 * 10000.0 + 14.0 * 100.0 + 3.5);
}

// 位与/左移/位或复合赋值序列。
#[test]
fn private_field_compound_assignment_bitwise_and_shift() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { #x = 0b1010; a(){ return this.#x &= 0b1100; } b(){ return this.#x <<= 1; } c(){ return this.#x |= 0b1; } } var c = new C(); c.a()*100 + c.b()*10 + c.c()",
    )
    .unwrap();
    assert_num(result, 8.0 * 100.0 + 16.0 * 10.0 + 17.0);
}

// 指数复合赋值走 COMPOUND_EXP（rhs 在 a 槽）。
#[test]
fn private_field_exponential_compound_assignment() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class C { #x = 2; m(){ return this.#x **= 3; } } new C().m()").unwrap();
    assert_num(result, 8.0);
}

// 私有访问器复合赋值：getter 读旧值、setter 写新值。
#[test]
fn private_accessor_compound_assignment_uses_getter_setter() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { static get #x(){ return this._v; } static set #x(v){ this._v = v; } static m(){ this.#x = 1; return this.#x += 2; } static get(){ return this._v; } } C.m()*10 + C.get()",
    )
    .unwrap();
    assert_num(result, 33.0);
}

// readonly-accessor 复合赋值：无 setter 抛 TypeError。
#[test]
fn private_readonly_accessor_compound_assignment_throws_type_error() {
    let mut vm = Vm::new();
    let err = eval(
        &mut vm,
        "class C { static get #x(){ return 1; } static m(){ return this.#x += 1; } } C.m()",
    )
    .unwrap_err();
    assert!(err.contains("TypeError"), "expected TypeError, got: {err}");
}

// 私有方法槽复合赋值：写方法槽抛 TypeError。
#[test]
fn private_method_compound_assignment_throws_type_error() {
    let mut vm = Vm::new();
    let err = eval(&mut vm, "class C { #m(){ return 1; } m(){ return this.#m += 1; } } new C().m()").unwrap_err();
    assert!(err.contains("TypeError"), "expected TypeError, got: {err}");
}

// 私有方法槽逻辑赋值（&&= 走写回路径）同样抛 TypeError。
#[test]
fn private_method_logical_assignment_throws_type_error() {
    let mut vm = Vm::new();
    let err = eval(&mut vm, "class C { #m(){ return 1; } m(){ return this.#m &&= 2; } } new C().m()").unwrap_err();
    assert!(err.contains("TypeError"), "expected TypeError, got: {err}");
}

// ||= 对 truthy 旧值短路：RHS 副作用不执行，字段保持原值。
#[test]
fn private_field_logical_or_keeps_truthy_value_and_short_circuits() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { #x = 1; m(){ var n = 0; var r = this.#x ||= ++n; return r*10 + n; } } new C().m()",
    )
    .unwrap();
    // #x=1 为 truthy → 短路，RHS 不执行，结果保持 1。
    assert_num(result, 10.0);
}

// ||= 对 falsy 旧值写回 RHS 值。
#[test]
fn private_field_logical_or_assigns_when_falsy() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { #x = 0; m(){ return this.#x ||= 5; } get(){ return this.#x; } } var c = new C(); c.m()*10 + c.get()",
    )
    .unwrap();
    assert_num(result, 55.0);
}

// &&= 对 truthy 旧值写回 RHS 值。
#[test]
fn private_field_logical_and_assigns_when_truthy() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { #x = 1; m(){ return this.#x &&= 2; } get(){ return this.#x; } } var c = new C(); c.m()*10 + c.get()",
    )
    .unwrap();
    assert_num(result, 22.0);
}

// &&= 对 falsy 旧值短路：RHS 副作用不执行，字段保持原值。
#[test]
fn private_field_logical_and_short_circuits_on_falsy() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { #x = 0; m(){ var n = 0; var r = this.#x &&= ++n; return r*10 + n; } } new C().m()",
    )
    .unwrap();
    // #x=0 为 falsy → 短路，RHS 不执行，结果保持 0。
    assert_num(result, 0.0);
}

// ??= 对 nullish（未初始化字段为 undefined）写回 RHS 值。
#[test]
fn private_field_nullish_coalesce_assigns_when_nullish() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { #x; m(){ return this.#x ??= 7; } get(){ return this.#x; } } var c = new C(); c.m()*10 + c.get()",
    )
    .unwrap();
    assert_num(result, 77.0);
}

// ??= 对已定义旧值短路：RHS 副作用不执行。
#[test]
fn private_field_nullish_coalesce_short_circuits_on_defined() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C { #x = 1; m(){ var n = 0; var r = this.#x ??= ++n; return r*10 + n; } } new C().m()",
    )
    .unwrap();
    assert_num(result, 10.0);
}

// ── 类表达式真实名 cell 边界：类名绑定不触碰外层同名绑定 ──

// 类表达式名是类体内独立 const 绑定：真实名 cell 初始化不得按名命中外层
// 同名绑定 cell 并覆盖之。外层 var E 被嵌套函数捕获时，读 E 应仍是原值。
#[test]
fn class_expression_inner_name_does_not_clobber_outer_binding() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var E = 1; var h = class E { m() { return E; } }; function g() { return E; } g()",
    )
    .unwrap();
    assert_eq!(result.as_int(), 1);
}

// 类表达式名不得破坏外层同名 let 的 TDZ：提前调用的捕获函数读该名仍抛
// ReferenceError，而非读到被类值污染的 cell。
#[test]
fn class_expression_inner_name_preserves_outer_let_tdz() {
    let mut vm = Vm::new();
    let err = eval(
        &mut vm,
        "function g(){ return E; } var h = class E { m(){ return E; } }; var r = g(); let E = 5; r",
    )
    .unwrap_err();
    assert!(err.contains("ReferenceError"), "expected ReferenceError, got: {err}");
}

// 类声明路径不回归：真实名 cell 仍须初始化（嵌套函数经 cell 读类值），
// 且外层同名 var 不被类声明覆盖。
#[test]
fn class_declaration_captured_name_cell_still_initialized() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var E = 1; function mk(){ class E { static n(){ return 5; } } return function(){ return E.n(); }; } var f = mk(); f()*10 + E",
    )
    .unwrap();
    assert_num(result, 51.0);
}

// 类声明的外层绑定是可变的（let 类）：顶层重赋类名合法并生效。
// 类体内对类名的引用另由独立不可变绑定承载，见 name-binding/const.js 语料。
#[test]
fn class_declaration_outer_binding_is_mutable() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { m(){ return 1; } } A = 1; A").unwrap();
    assert_num(result, 1.0);
}

// ── delete super 属性引用 ──

// 派生构造器内 delete super.x：运行期抛 ReferenceError，不落入成员删除。
#[test]
fn delete_super_property_in_derived_constructor_throws() {
    let mut vm = Vm::new();
    let err =
        eval(&mut vm, "class C extends Object { constructor() { super(); delete super.x; } } new C()").unwrap_err();
    assert!(err.contains("ReferenceError"), "expected ReferenceError, got: {err}");
}

// 实例方法/getter/setter 三种方法体内 delete super.x 均抛 ReferenceError。
#[test]
fn delete_super_property_in_method_getter_and_setter_throws() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class C extends Object { \
           method() { delete super.x; } \
           get g() { delete super.x; } \
           set s(v) { delete super.x; } \
         } \
         var names = []; var c = new C(); \
         try { c.method(); } catch (e) { names.push(e.name); } \
         try { c.g; } catch (e) { names.push(e.name); } \
         try { c.s = 1; } catch (e) { names.push(e.name); } \
         names.join(',')",
    )
    .unwrap();
    assert_eq!(vm.lookup_str(result).unwrap(), "ReferenceError,ReferenceError,ReferenceError");
}

// 静态方法 + null 原型链基：基限制不得先于删除判定抛出 TypeError。
#[test]
fn delete_super_property_in_static_method_with_null_base_throws() {
    let mut vm = Vm::new();
    let err = eval(
        &mut vm,
        "class C { static m() { delete super.x; } } Object.setPrototypeOf(C, null); C.m()",
    )
    .unwrap_err();
    assert!(err.contains("ReferenceError"), "expected ReferenceError, got: {err}");
}

// 计算键 delete super[key]：键表达式不得求值（toString 不执行即抛 ReferenceError）。
#[test]
fn delete_super_property_computed_key_not_evaluated_throws() {
    let mut vm = Vm::new();
    let err = eval(
        &mut vm,
        "var key = { toString() { throw new Error('ToPropertyKey performed'); } }; \
         var obj = { m() { delete super[key]; } }; obj.m()",
    )
    .unwrap_err();
    assert!(err.contains("ReferenceError"), "expected ReferenceError, got: {err}");
}

// this 未初始化时 delete super[键]：键表达式不得求值，基构造器也不得执行。
#[test]
fn delete_super_property_uninitialized_this_key_not_evaluated_throws() {
    let mut vm = Vm::new();
    let err = eval(
        &mut vm,
        "class Base { constructor() { throw new Error('base constructor called'); } } \
         class Derived extends Base { constructor() { delete super[(super(), 0)]; } } new Derived()",
    )
    .unwrap_err();
    assert!(err.contains("ReferenceError"), "expected ReferenceError, got: {err}");
    assert!(!err.contains("base constructor called"), "base constructor must not run: {err}");
}

// ── 计算键访问器（get/set [dyn]）：类定义期键数组取键 + 运行期取键定义 ──

// 计算键实例访问器对：string 键读写。
#[test]
fn computed_instance_accessor_pair() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var k = 'p'; class A { get [k]() { return this.v + 1; } set [k](x) { this.v = x; } } var a = new A(); a.p = 41; a.p",
    )
    .unwrap();
    assert_num(result, 42.0);
}

// 计算键静态访问器：Symbol.species 键（Promise 同形）。
#[test]
fn computed_static_accessor_symbol_species_key() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "class A { static get [Symbol.species]() { return 7; } } A[Symbol.species]").unwrap();
    assert_eq!(result.as_int(), 7);
}

// 半对：只有 getter，setter 边为 undefined。
#[test]
fn computed_accessor_get_only_set_undefined() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var k = 'g'; class A { get [k]() { return 3; } } Object.getOwnPropertyDescriptor(Object.getPrototypeOf(new A()), 'g').set === undefined",
    )
    .unwrap();
    assert!(result.is_bool() && result.as_bool(), "set should be undefined");
}

// 半对：只有 setter，getter 边为 undefined。
#[test]
fn computed_accessor_set_only_get_undefined() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var k = 's'; class A { set [k](v) { this._v = v; } } Object.getOwnPropertyDescriptor(Object.getPrototypeOf(new A()), 's').get === undefined",
    )
    .unwrap();
    assert!(result.is_bool() && result.as_bool(), "get should be undefined");
}

// 与既有访问器对合并：静态 get b 后被计算键 get ['b'] 覆盖，getter 替换、setter 保留。
#[test]
fn computed_accessor_merges_existing_pair() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var k = 'b'; class A { get b() { return 1; } set b(v) { this._v = v; } get [k]() { return 9; } } var a = new A(); a.b = 5; a.b * 10 + a._v",
    )
    .unwrap();
    assert_num(result, 95.0);
}

// 描述符核：enumerable=false、configurable=true（规范 DefineMethod，区别于对象
// 字面量访问器的可枚举）。
#[test]
fn computed_accessor_descriptor_non_enumerable_configurable() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var k = 'd'; class A { get [k]() { return 1; } } var d = Object.getOwnPropertyDescriptor(Object.getPrototypeOf(new A()), 'd'); (d.enumerable === false) * 10 + (d.configurable === true)",
    )
    .unwrap();
    assert_num(result, 11.0);
}

// 计算键 'constructor'：覆写原型 constructor 数据属性为访问器。
#[test]
fn computed_accessor_constructor_key_overrides_prototype() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "class A { get ['constructor']() { return 6; } } var a = new A(); Object.getOwnPropertyDescriptor(Object.getPrototypeOf(a), 'constructor').get !== undefined ? a.constructor : -1",
    )
    .unwrap();
    assert_num(result, 6.0);
}

// static get ['prototype'] 抛 TypeError（prototype 属性不可配置，定义被拒）。
#[test]
fn computed_static_accessor_prototype_key_throws() {
    let mut vm = Vm::new();
    let err = eval(&mut vm, "class A { static get ['prototype']() { return 1; } }").unwrap_err();
    assert!(err.contains("TypeError"), "expected TypeError, got: {err}");
}

// 键表达式副作用恰执行一次（类定义期键数组构建，方法发射不重求值）。
#[test]
fn computed_accessor_key_evaluated_once() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var n = 0; class A { get [(n++, 'z')]() { return 1; } } new A().z; n").unwrap();
    assert_eq!(result.as_int(), 1);
}

// 命名 symbol 计算键：运行期键走 symbol 键面读写。
#[test]
fn computed_accessor_symbol_key() {
    let mut vm = Vm::new();
    let result = eval(
        &mut vm,
        "var s = Symbol('tag'); class A { get [s]() { return 4; } set [s](v) { this._v = v; } } var a = new A(); a[s] = 3; a[s] * 10 + a._v",
    )
    .unwrap();
    assert_num(result, 43.0);
}
