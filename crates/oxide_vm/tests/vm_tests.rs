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
fn test_throw_caught_by_catch() {
    assert_eq!(eval("try { throw 42; } catch(e) { e; }"), "42");
}

#[test]
fn test_throw_without_catch() {
    let result = eval("throw 'err';");
    assert!(
        result.contains("uncaught"),
        "expected uncaught, got: {result}"
    );
}

#[test]
fn test_try_finally_normal_path() {
    assert_eq!(eval("var x = 1; try { x = 2; } finally { x = 3; } x;"), "3");
}

#[test]
fn test_try_finally_exception_path() {
    let result = eval("var x = 0; try { throw 1; } finally { x = 2; }");
    assert!(
        result.contains("uncaught"),
        "expected uncaught, got: {result}"
    );
}

#[test]
fn test_nested_try_inner_catches() {
    assert_eq!(
        eval("try { try { throw 1; } catch(e) { e; } } catch(e) { 2; }"),
        "1"
    );
}

#[test]
fn test_nested_try_outer_catches() {
    assert_eq!(
        eval("try { try { throw 1; } catch(e) { throw 2; } } catch(e) { e; }"),
        "2"
    );
}

#[test]
fn test_try_catch_finally() {
    assert_eq!(
        eval("var x = 0; try { x = 1; } catch(e) { x = 2; } finally { x = 3; } x;"),
        "3"
    );
}

#[test]
fn test_catch_param_binding() {
    assert_eq!(eval("try { throw 99; } catch(myErr) { myErr; }"), "99");
}
