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

// Non-simple `??` (side-effecting operands) followed by a label-bearing construct must not
// shift the downstream label id (which produced `Label ... not found` at emit time).

#[test]
fn coalesce_nonsimple_then_if() {
    assert_eq!(
        eval("function a(){return null;} function b(){return 2;} var v=a()??b(); if(v>0){v=v+10;} v"),
        "12"
    );
}

#[test]
fn coalesce_nonsimple_then_while() {
    assert_eq!(
        eval("function a(){return null;} function b(){return 3;} var v=a()??b(); var c=0; while(c<v){c=c+1;} c"),
        "3"
    );
}

// A jump-bearing construct (if/loop) followed by a hoisted function declaration:
// the count pass must walk in the same function-hoisting order as the emit pass,
// otherwise the construct's jump targets drift past the hoisted body and the
// program loops forever at run time.

#[test]
fn if_then_hoisted_function() {
    assert_eq!(eval("if(false){} function f(){return 1} f()"), "1");
}

#[test]
fn for_of_then_hoisted_function() {
    assert_eq!(eval("var r=0; for(const x of [1,2,3]) r+=x; function f(){return 100} r + f()"), "106");
}

#[test]
fn while_then_hoisted_function() {
    assert_eq!(eval("var n=0; while(n<3){n=n+1;} function f(){return 100} n + f()"), "103");
}

// A hoisted function declaration is emitted before the `var` statements it closes over,
// so the count pass must register top-level `var` names too (not just function names);
// otherwise the hoisted body cannot resolve the outer var ("Identifier not defined").

#[test]
fn hoisted_function_reads_outer_var() {
    assert_eq!(eval("var s=3; function f(){return s} f()"), "3");
}

#[test]
fn hoisted_function_reads_outer_var_and_function() {
    assert_eq!(eval("var s=3; function g(){return 5} function f(){return g()+s} f()"), "8");
}

#[test]
fn coalesce_nonsimple_then_for() {
    assert_eq!(
        eval("function a(){return null;} var v=a()??4; var s=0; for(var i=0;i<v;i=i+1){s=s+i;} s"),
        "6"
    );
}

#[test]
fn coalesce_nonsimple_short_circuits_on_value() {
    // 0 is not nullish, so `a() ?? b()` is 0 and the loop never runs.
    assert_eq!(
        eval("function a(){return 0;} function b(){return 9;} var v=a()??b(); var c=0; while(c<v){c=c+1;} c"),
        "0"
    );
}

// Static member compound assignment must not over-count vs the emitter (the extra +1 drifted
// the following loop's jump target).

#[test]
fn static_member_compound_assign_then_loop() {
    assert_eq!(eval("var o={x:1}; o.x+=2; var c=0; while(c<3){c=c+1;} c"), "3");
}

#[test]
fn static_member_compound_assign_in_loop() {
    assert_eq!(eval("var o={x:0}; var c=0; while(o.x<3){o.x+=1; c=c+1;} c"), "3");
}

// Regressions: simple coalesce, nested coalesce, And/Or, compound member value, plain assign.
#[test]
fn simple_coalesce_regression() {
    assert_eq!(eval("var x = null ?? 5; x"), "5");
}

#[test]
fn nested_coalesce_regression() {
    assert_eq!(eval("function a(){return null;} var v = a() ?? a() ?? 7; v"), "7");
}

#[test]
fn logical_and_nonsimple_regression() {
    assert_eq!(eval("function a(){return 1;} function b(){return 2;} var v=a()&&b(); if(v>0){} v"), "2");
}

#[test]
fn compound_member_value_regression() {
    assert_eq!(eval("var o={x:10}; o.x-=3; o.x"), "7");
}
