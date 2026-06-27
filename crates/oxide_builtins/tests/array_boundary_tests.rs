use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(source: &str) -> Result<(Vm, JsValue), String> {
    let allocator = oxide_parser::Allocator::default();
    let program = oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new().compile(&program).map_err(|e| format!("Compile error: {}", e))?;
    let mut vm = Vm::new();
    let result = vm.run(&module)?;
    Ok((vm, result))
}

#[test]
fn test_push_returns_new_length() {
    let (_vm, result) = eval("var a = [1, 2]; a.push(3)").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn test_push_multiple_args() {
    let (_vm, result) = eval("var a = [1]; a.push(2, 3)").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn test_pop_empty_returns_undefined() {
    let (_vm, result) = eval("[].pop()").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn test_pop_returns_last_element() {
    let (_vm, result) = eval("[1, 2, 3].pop()").unwrap();
    assert_eq!(result.as_int(), 3);
}

#[test]
fn test_shift_empty_returns_undefined() {
    let (_vm, result) = eval("[].shift()").unwrap();
    assert!(result.is_undefined());
}

#[test]
fn test_shift_returns_first_element() {
    let (_vm, result) = eval("[1, 2, 3].shift()").unwrap();
    assert_eq!(result.as_int(), 1);
}

#[test]
fn test_unshift_returns_new_length() {
    let (_vm, result) = eval("var a = [3, 4]; a.unshift(1, 2)").unwrap();
    assert_eq!(result.as_int(), 4);
}

#[test]
fn test_slice_negative_start() {
    let (_vm, result) = eval("[1, 2, 3, 4].slice(-2).length").unwrap();
    assert_eq!(result.as_int(), 2);
}

#[test]
fn test_slice_negative_end() {
    let (_vm, result) = eval("[1, 2, 3].slice(0, -1).length").unwrap();
    assert_eq!(result.as_int(), 2);
}
