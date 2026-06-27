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
    match vm.run(&module) {
        Ok(result) => format!("{result}"),
        Err(e) => format!("vm error: {e}"),
    }
}

// A class body must not drift the instruction count of code that follows it. Each repro
// places a bounded loop after the class so a wrong count corrupts the loop's jump target.

#[test]
fn non_static_private_method_then_loop() {
    assert_eq!(eval("class C{#m(){return 1;}} var c=0; while(c<3){c=c+1;} c"), "3");
}

#[test]
fn static_private_method_then_loop() {
    assert_eq!(eval("class C{static #m(){return 1;}} var c=0; while(c<3){c=c+1;} c"), "3");
}

// Multi-word computed key — a single-identifier key happens to be one word and would not
// expose the drift.
#[test]
fn computed_method_key_then_loop() {
    assert_eq!(eval("class C{[\"x\"+\"y\"](){return 1;}} var c=0; while(c<3){c=c+1;} c"), "3");
}

#[test]
fn computed_static_field_key_then_loop() {
    assert_eq!(eval("class C{static [\"a\"+\"b\"]=1;} var c=0; while(c<3){c=c+1;} c"), "3");
}

// Derived constructor with an instance field and a loop after super(): the field code is
// injected after SUPER_CALL, so the loop's jump target must account for it.
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

// Non-derived class with an instance field and a ctor loop stays correct (unchanged path).
#[test]
fn non_derived_ctor_field_then_loop() {
    assert_eq!(
        eval("class C{ x=1; constructor(){this.r=0; while(this.r<3){this.r=this.r+1;}} } new C().r"),
        "3"
    );
}

// Behavioral regressions: methods/getters/private calls still work.
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
