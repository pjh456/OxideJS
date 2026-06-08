use oxide_compiler::compiler::Compiler;
use oxide_types::value::JsValue;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program =
        oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new()
        .compile(&program)
        .map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&module)
}

#[test]
fn function_call_changes_this() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.max.call(null, 10, 5)").unwrap();
    assert!((result.as_double() - 10.0).abs() < 0.0001);
}

#[test]
fn function_apply_changes_this() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.max.apply(null, [10, 5])").unwrap();
    assert!((result.as_double() - 10.0).abs() < 0.0001);
}

#[test]
fn function_bind_creates_wrapper() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "var b = Math.max.bind(null, 1); b(5)").unwrap();
    assert!((result.as_double() - 5.0).abs() < 0.0001);
}

#[test]
fn function_to_string_includes_function() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Math.max.toString()").unwrap();
    assert!(result.is_string());
    let rendered = vm
        .kernel()
        .string_forge()
        .lookup(result.as_string_index())
        .unwrap_or_default();
    assert!(rendered.contains("function"), "got: {}", rendered);
}

#[test]
fn function_to_string_non_function_throws() {
    let mut vm = Vm::new();
    let err = eval(&mut vm, "Function.prototype.toString.call(1)").unwrap_err();
    assert!(err.contains("TypeError"), "got: {}", err);
}
