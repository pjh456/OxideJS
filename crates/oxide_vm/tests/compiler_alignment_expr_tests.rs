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

// 非简单 `??`（带副作用操作数）后跟带标签的构造不得把下游 label id 移位
// （否则 emit 期报 `Label ... not found`）。

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

// 含跳转的构造（if/loop）后跟被提升的函数声明：
// 计数 pass 必须与 emit pass 采用相同的函数提升顺序，
// 否则构造的跳转目标越过提升的函数体，程序运行时无限循环。

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

// 提升的函数声明先于它闭包的 `var` 语句发射，
// 因此计数 pass 必须同时登记顶层 `var` 名（不只是函数名）；
// 否则提升的函数体无法解析外层 var（"Identifier not defined"）。

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
    // 0 不是 nullish，因此 `a() ?? b()` 为 0，循环不会运行。
    assert_eq!(
        eval("function a(){return 0;} function b(){return 9;} var v=a()??b(); var c=0; while(c<v){c=c+1;} c"),
        "0"
    );
}

// 静态成员复合赋值不得相对 emitter 多计（多出的 +1 会漂移其后循环的跳转目标）。

#[test]
fn static_member_compound_assign_then_loop() {
    assert_eq!(eval("var o={x:1}; o.x+=2; var c=0; while(c<3){c=c+1;} c"), "3");
}

#[test]
fn static_member_compound_assign_in_loop() {
    assert_eq!(eval("var o={x:0}; var c=0; while(o.x<3){o.x+=1; c=c+1;} c"), "3");
}

// 回归：简单 coalesce、嵌套 coalesce、And/Or、复合成员值、普通赋值。
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
