use oxide_compiler::compiler::Compiler;
use oxide_vm::vm::Vm;

fn eval(vm: &mut Vm, source: &str) -> Result<oxide_types::value::JsValue, String> {
    let allocator = oxide_parser::Allocator::default();
    let program =
        oxide_parser::parse(&allocator, source).map_err(|e| format!("Parse error: {:?}", e))?;
    let module = Compiler::new()
        .compile(&program)
        .map_err(|e| format!("Compile error: {}", e))?;
    vm.run(&module)
}

#[test]
fn date_now_returns_number() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Date.now()").unwrap();
    assert!(result.is_double());
    assert!(result.as_double() > 0.0);
}

#[test]
fn date_now_increases() {
    let mut vm = Vm::new();
    let t1 = eval(&mut vm, "Date.now()").unwrap().as_double();
    let t2 = eval(&mut vm, "Date.now()").unwrap().as_double();
    assert!(t2 >= t1);
}

#[test]
fn date_parse_returns_nan() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Date.parse('invalid-date')").unwrap();
    assert!(result.is_double());
    assert!(result.as_double().is_nan());
}

#[test]
fn date_parse_iso_format() {
    let mut vm = Vm::new();
    let result = eval(&mut vm, "Date.parse('2020-01-15')").unwrap();
    assert!(result.is_double());
    assert!(!result.as_double().is_nan());
    assert!(result.as_double() > 0.0);
}
