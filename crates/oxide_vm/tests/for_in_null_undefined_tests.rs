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
    // Must NOT produce a "vm error: ..." — a quiet empty loop, then a sentinel value.
    assert_eq!(eval("for(var k in null){} 7"), "7", "for-in null must not throw");
}

#[test]
fn for_in_number_primitive_currently_throws() {
    // Interim behavior: ToObject coercion for non-null/undefined primitives is not
    // implemented yet, so a number right-hand side still throws TypeError. When
    // ToObject lands this becomes an empty loop and this assertion should change.
    let out = eval("for(var k in 42){}");
    assert!(out.contains("TypeError"), "expected TypeError for number primitive, got: {out}");
}
