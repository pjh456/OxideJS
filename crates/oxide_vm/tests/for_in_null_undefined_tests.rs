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

#[test]
fn for_in_null_empty_loop() {
    assert_eq!(eval("var r=0;for(var k in null){r=r+1;}r"), "0", "for-in null iterates zero times");
}

#[test]
fn for_in_undefined_empty_loop() {
    assert_eq!(
        eval("var r=0;for(var k in undefined){r=r+1;}r"),
        "0",
        "for-in undefined iterates zero times"
    );
}

#[test]
fn for_in_null_does_not_execute_body() {
    assert_eq!(
        eval("var hit=false;for(var k in null){hit=true;}hit"),
        "false",
        "for-in null skips the body"
    );
}

#[test]
fn for_in_null_does_not_throw() {
    // 不得产生 "vm error: ..."——静默空循环，然后哨兵值。
    assert_eq!(eval("for(var k in null){} 7"), "7", "for-in null must not throw");
}

#[test]
fn for_in_number_primitive_empty_loop() {
    // 右值经 ToObject 装箱：数字盒无自身可枚举属性，空循环。
    assert_eq!(
        eval("var r=0;for(var k in 42){r=r+1;}r"),
        "0",
        "for-in number primitive iterates zero times"
    );
}
