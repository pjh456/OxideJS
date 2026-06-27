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

// Destructured parameter followed by a loop body: the counter must count the parameter
// destructuring prologue or the loop's back-jump lands at the wrong PC.
#[test]
fn param_array_pattern_with_loop_body() {
    assert_eq!(eval("function f([a]){var s=0; while(a>0){s=s+1;a=a-1;} return s;} f([3])"), "3");
}

#[test]
fn param_array_pattern_default_element() {
    assert_eq!(eval("function f([a=5]){return a;} f([])"), "5");
}

// Array assignment target with a default element followed by a loop.
#[test]
fn array_assignment_default_element_then_loop() {
    assert_eq!(eval("var a,c=0; [a=5]=[]; while(c<a){c=c+1;} c"), "5");
}

// Nested array assignment target element.
#[test]
fn array_assignment_nested_element() {
    assert_eq!(eval("var a,b; [[a],b]=[[1],2]; a*10+b"), "12");
}

// Destructuring catch param followed by a loop: the counter must not count a STORE_VAR the
// emitter does not emit for a pattern catch binding.
#[test]
fn destructuring_catch_param_then_loop() {
    assert_eq!(eval("var c=0; try{throw 1;}catch({e}){} while(c<3){c=c+1;} c"), "3");
}

// Regression: for-init declaration without an initializer followed by loop iterations.
#[test]
fn for_init_no_initializer_declarator() {
    assert_eq!(eval("var c=0; for(var i; c<3; c=c+1){} c"), "3");
}

// Identifier element array assignment stays correct (simple path unchanged).
#[test]
fn array_assignment_identifier_elements() {
    assert_eq!(eval("var a,b; [a,b]=[7,8]; a*10+b"), "78");
}
