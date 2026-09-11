use std::sync::Arc;

use oxide_compiler::compiler::Compiler;
use oxide_parser::Allocator;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> String {
    let allocator = Allocator::default();
    let program = match oxide_parser::parse(&allocator, source) {
        Ok(p) => p,
        Err(e) => return format!("parse error: {}", e[0].message),
    };
    let module = match Compiler::new().compile(&program) {
        Ok(m) => m,
        Err(e) => return format!("compile error: {e}"),
    };
    let mut vm = Vm::new();
    match vm.run(&Arc::new(module)) {
        Ok(result) => format!("{result}"),
        Err(e) => format!("vm error: {e}"),
    }
}

// 类体不得漂移其后代码的指令计数。每个复现都在类后放置有界循环，
// 计数错误会破坏循环跳转目标。

#[test]
fn non_static_private_method_then_loop() {
    assert_eq!(eval("class C{#m(){return 1;}} var c=0; while(c<3){c=c+1;} c"), "3");
}

#[test]
fn static_private_method_then_loop() {
    assert_eq!(eval("class C{static #m(){return 1;}} var c=0; while(c<3){c=c+1;} c"), "3");
}

// 多字计算键——单标识符键恰好是单字，暴露不出漂移。
#[test]
fn computed_method_key_then_loop() {
    assert_eq!(eval("class C{[\"x\"+\"y\"](){return 1;}} var c=0; while(c<3){c=c+1;} c"), "3");
}

#[test]
fn computed_static_field_key_then_loop() {
    assert_eq!(eval("class C{static [\"a\"+\"b\"]=1;} var c=0; while(c<3){c=c+1;} c"), "3");
}

// 含实例字段的派生构造函数在 super() 后跟循环：字段代码注入在 SUPER_CALL 之后，
// 循环跳转目标必须计入它。
#[test]
fn derived_ctor_field_then_post_super_loop() {
    assert_eq!(
        eval("class A{} class B extends A{ x=2; constructor(){super(); this.r=0; while(this.r<3){this.r=this.r+1;}}} new B().r"),
        "3"
    );
}

#[test]
fn derived_ctor_field_value_initialized() {
    assert_eq!(eval("class A{} class B extends A{ x=5; constructor(){super();} } new B().x"), "5");
}

#[test]
fn derived_ctor_statement_before_super() {
    assert_eq!(
        eval("class A{constructor(){this.a=1;}} class B extends A{ y=2; constructor(){var z=0; super(); while(z<3){z=z+1;} this.z=z;} } new B().z"),
        "3"
    );
}

// 非派生类含实例字段与构造循环保持正确（路径未变）。
#[test]
fn non_derived_ctor_field_then_loop() {
    assert_eq!(
        eval("class C{ x=1; constructor(){this.r=0; while(this.r<3){this.r=this.r+1;}} } new C().r"),
        "3"
    );
}

// 行为回归：方法/getter/私有调用仍正常。
#[test]
fn plain_method_call_regression() {
    assert_eq!(eval("class C{m(){return 9;}} new C().m()"), "9");
}

#[test]
fn private_method_call_regression() {
    assert_eq!(eval("class C{#m(){return 7;} run(){return this.#m();}} new C().run()"), "7");
}

#[test]
fn getter_regression() {
    assert_eq!(eval("class C{get x(){return 5;}} new C().x"), "5");
}
